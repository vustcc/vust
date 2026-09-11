//! 套件工作负载流量取证服务：基于 Linux AF_PACKET 原始套接字捕获端口流量并生成 PCAP。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::Context;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{error, info, warn};

const PCAP_MAX_DURATION_SECS: u64 = 300;
const PCAP_COMPLETED_RETENTION_SECS: u64 = 300;

#[cfg(target_os = "linux")]
struct InterfaceListener {
    interface: String,
    async_fd: tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>,
}

#[derive(Debug, Clone)]
struct RawPacketFrame {
    timestamp_sec: u32,
    timestamp_usec: u32,
    payload: Vec<u8>,
}

struct PcapSlot {
    suite_instance_id: Option<String>,
    workload_id: Option<String>,
    pcap_writer_tx: mpsc::Sender<RawPacketFrame>,
    cancel_tx: Option<oneshot::Sender<()>>,
    done_rx: oneshot::Receiver<anyhow::Result<Vec<u8>>>,
    watchdog_cancel_tx: Option<oneshot::Sender<()>>,
}

/// 抓包端点使用的传输协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureTransport {
    Tcp,
    Udp,
}

/// 由工作负载所有权校验后传入抓包服务的命名端点。
#[derive(Debug, Clone)]
pub struct CaptureEndpoint {
    pub endpoint_id: String,
    pub host_port: u16,
    pub transport: CaptureTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PortKey {
    transport: CaptureTransport,
    port: u16,
}

/// Agent 内部共享的原生抓包分发中心。
pub struct PcapMuxHub {
    active_slots: Arc<Mutex<HashMap<String, PcapSlot>>>,
    port_index: Arc<Mutex<HashMap<PortKey, String>>>,
    global_listener_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

static GLOBAL_PCAP_HUB: OnceLock<PcapMuxHub> = OnceLock::new();

impl PcapMuxHub {
    /// 返回进程内全局抓包分发中心。
    pub fn global() -> &'static Self {
        GLOBAL_PCAP_HUB.get_or_init(Self::new)
    }

    fn new() -> Self {
        Self {
            active_slots: Arc::new(Mutex::new(HashMap::new())),
            port_index: Arc::new(Mutex::new(HashMap::new())),
            global_listener_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// 启动指定宿主机端口的抓包任务。
    pub async fn start_capture(&self, capture_id: String, port: u16) -> anyhow::Result<()> {
        self.start_capture_for_workload(
            capture_id,
            vec![CaptureEndpoint {
                endpoint_id: "tcp".to_string(),
                host_port: port,
                transport: CaptureTransport::Tcp,
            }],
            None,
            None,
        )
        .await
    }

    /// 启动指定工作负载宿主机端口的抓包任务。
    pub async fn start_capture_for_workload(
        &self,
        capture_id: String,
        endpoints: Vec<CaptureEndpoint>,
        suite_instance_id: Option<String>,
        workload_id: Option<String>,
    ) -> anyhow::Result<()> {
        if endpoints.is_empty() {
            anyhow::bail!("at least one capture endpoint is required");
        }
        let endpoint_keys = endpoints
            .iter()
            .map(|endpoint| PortKey {
                transport: endpoint.transport,
                port: endpoint.host_port,
            })
            .collect::<HashSet<_>>();
        if endpoint_keys.len() != endpoints.len() {
            anyhow::bail!("capture endpoints contain duplicate transport/port bindings");
        }
        let mut port_index = self.port_index.lock().await;
        if let Some(key) = endpoint_keys
            .iter()
            .find(|key| port_index.contains_key(key))
        {
            anyhow::bail!(
                "capture already active on {}/{}",
                key.port,
                transport_name(key.transport)
            );
        }

        let (pcap_tx, pcap_rx) = mpsc::channel::<RawPacketFrame>(1000);
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let (done_tx, done_rx) = oneshot::channel::<anyhow::Result<Vec<u8>>>();
        let file_path = capture_file_path(&capture_id);

        tokio::spawn(async move {
            let result = run_pcap_writer(file_path, pcap_rx, cancel_rx).await;
            let _ = done_tx.send(result);
        });

        let (watchdog_cancel_tx, watchdog_cancel_rx) = oneshot::channel::<()>();
        let capture_id_for_watchdog = capture_id.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(PCAP_MAX_DURATION_SECS)) => {
                    warn!(capture_id = %capture_id_for_watchdog, "pcap capture reached timeout limit");
                    if let Err(err) = PcapMuxHub::global()
                        .timeout_capture(&capture_id_for_watchdog)
                        .await
                    {
                        warn!(capture_id = %capture_id_for_watchdog, error = %err, "failed to finish timed out pcap capture");
                    }
                }
                _ = watchdog_cancel_rx => {}
            }
        });

        self.active_slots.lock().await.insert(
            capture_id.clone(),
            PcapSlot {
                suite_instance_id,
                workload_id,
                pcap_writer_tx: pcap_tx,
                cancel_tx: Some(cancel_tx),
                done_rx,
                watchdog_cancel_tx: Some(watchdog_cancel_tx),
            },
        );
        for key in &endpoint_keys {
            port_index.insert(*key, capture_id.clone());
        }
        drop(port_index);

        if let Err(err) = self.ensure_global_listener().await {
            if let Some(slot) = self.active_slots.lock().await.remove(&capture_id) {
                if let Some(cancel_tx) = slot.cancel_tx {
                    let _ = cancel_tx.send(());
                }
                if let Some(watchdog_cancel_tx) = slot.watchdog_cancel_tx {
                    let _ = watchdog_cancel_tx.send(());
                }
            }
            self.port_index
                .lock()
                .await
                .retain(|_, current_capture_id| current_capture_id != &capture_id);
            return Err(err);
        }
        Ok(())
    }

    /// 停止指定抓包任务并返回完整 PCAP 字节。
    pub async fn stop_capture(&self, capture_id: &str) -> anyhow::Result<Vec<u8>> {
        let Some(mut slot) = self.active_slots.lock().await.remove(capture_id) else {
            anyhow::bail!("pcap capture not found: {capture_id}");
        };
        self.port_index
            .lock()
            .await
            .retain(|_, current_capture_id| current_capture_id != capture_id);
        if let Some(cancel_tx) = slot.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
        if let Some(watchdog_cancel_tx) = slot.watchdog_cancel_tx {
            let _ = watchdog_cancel_tx.send(());
        }

        let result = slot
            .done_rx
            .await
            .context("pcap writer stopped before returning capture data")?;

        if self.active_slots.lock().await.is_empty()
            && let Some(handle) = self.global_listener_handle.lock().await.take()
        {
            handle.abort();
            info!("pcap raw socket listener stopped because no captures remain");
        }

        result
    }

    /// 到达最长抓包时限后停止写入，但保留结果，等待套件通过 finish 接口领取。
    async fn timeout_capture(&self, capture_id: &str) -> anyhow::Result<()> {
        let mut slots = self.active_slots.lock().await;
        let Some(slot) = slots.get_mut(capture_id) else {
            return Ok(());
        };
        if let Some(cancel_tx) = slot.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
        slot.watchdog_cancel_tx = None;
        drop(slots);

        self.port_index
            .lock()
            .await
            .retain(|_, current_capture_id| current_capture_id != capture_id);
        if self.port_index.lock().await.is_empty()
            && let Some(handle) = self.global_listener_handle.lock().await.take()
        {
            handle.abort();
            info!("pcap raw socket listener stopped because no captures remain");
        }
        let capture_id = capture_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(
                PCAP_COMPLETED_RETENTION_SECS,
            ))
            .await;
            PcapMuxHub::global()
                .discard_completed_capture(&capture_id)
                .await;
        });
        Ok(())
    }

    async fn discard_completed_capture(&self, capture_id: &str) {
        let mut slots = self.active_slots.lock().await;
        let is_completed = slots
            .get(capture_id)
            .is_some_and(|slot| slot.cancel_tx.is_none());
        let slot = is_completed.then(|| slots.remove(capture_id)).flatten();
        drop(slots);
        if let Some(slot) = slot {
            let _ = slot.done_rx.await;
            warn!(capture_id, "discarded unclaimed timed out pcap capture");
        }
    }

    /// 仅在抓包任务属于指定套件实例和工作负载时停止并返回数据。
    pub async fn stop_capture_owned(
        &self,
        capture_id: &str,
        suite_instance_id: &str,
        workload_id: &str,
    ) -> anyhow::Result<Vec<u8>> {
        {
            let slots = self.active_slots.lock().await;
            let Some(slot) = slots.get(capture_id) else {
                anyhow::bail!("pcap capture not found: {capture_id}");
            };
            if slot.suite_instance_id.as_deref() != Some(suite_instance_id)
                || slot.workload_id.as_deref() != Some(workload_id)
            {
                anyhow::bail!("pcap capture does not belong to suite workload");
            }
        }
        self.stop_capture(capture_id).await
    }

    /// 停止指定套件实例下的所有抓包任务，返回已停止的抓包 ID。
    pub async fn stop_captures_for_suite_instance(&self, suite_instance_id: &str) -> Vec<String> {
        let captures = {
            let slots = self.active_slots.lock().await;
            slots
                .iter()
                .filter(|(_, slot)| slot.suite_instance_id.as_deref() == Some(suite_instance_id))
                .map(|(capture_id, slot)| (capture_id.clone(), slot.workload_id.clone()))
                .collect::<Vec<_>>()
        };

        let mut stopped = Vec::new();
        for (capture_id, workload_id) in captures {
            match self.stop_capture(&capture_id).await {
                Ok(_) => {
                    info!(
                        capture_id = %capture_id,
                        workload_id = workload_id.as_deref().unwrap_or_default(),
                        "stopped suite workload pcap capture during cleanup"
                    );
                    stopped.push(capture_id);
                }
                Err(err) => {
                    warn!(capture_id = %capture_id, error = %err, "failed to stop suite workload pcap capture during cleanup")
                }
            }
        }
        stopped
    }

    /// 停止并丢弃指定套件工作负载下的全部抓包任务。
    pub async fn stop_captures_for_workload(
        &self,
        suite_instance_id: &str,
        workload_id: &str,
    ) -> Vec<String> {
        let captures = {
            let slots = self.active_slots.lock().await;
            slots
                .iter()
                .filter(|(_, slot)| {
                    slot.suite_instance_id.as_deref() == Some(suite_instance_id)
                        && slot.workload_id.as_deref() == Some(workload_id)
                })
                .map(|(capture_id, _)| capture_id.clone())
                .collect::<Vec<_>>()
        };

        let mut stopped = Vec::new();
        for capture_id in captures {
            match self.stop_capture(&capture_id).await {
                Ok(_) => {
                    info!(
                        capture_id = %capture_id,
                        workload_id,
                        "stopped suite workload pcap capture during workload cleanup"
                    );
                    stopped.push(capture_id);
                }
                Err(err) => {
                    warn!(
                        capture_id = %capture_id,
                        workload_id,
                        error = %err,
                        "suite workload pcap payload was unavailable during workload cleanup"
                    );
                    stopped.push(capture_id);
                }
            }
        }
        stopped
    }

    async fn ensure_global_listener(&self) -> anyhow::Result<()> {
        let mut handle_lock = self.global_listener_handle.lock().await;
        if handle_lock.is_some() {
            return Ok(());
        }
        let active_slots = self.active_slots.clone();
        let port_index = self.port_index.clone();
        let listeners = prepare_interface_listeners().await?;
        let handle = tokio::spawn(async move {
            if let Err(err) =
                run_global_raw_socket_listener(active_slots, port_index, listeners).await
            {
                error!(error = %err, "pcap raw socket listener failed");
            }
        });
        *handle_lock = Some(handle);
        Ok(())
    }
}

async fn run_pcap_writer(
    file_path: PathBuf,
    mut pcap_rx: mpsc::Receiver<RawPacketFrame>,
    mut cancel_rx: oneshot::Receiver<()>,
) -> anyhow::Result<Vec<u8>> {
    let mut file = File::create(&file_path)
        .await
        .with_context(|| format!("failed to create pcap file {}", file_path.display()))?;
    file.write_all(&pcap_global_header()).await?;
    file.flush().await?;

    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                break;
            }
            frame_opt = pcap_rx.recv() => {
                let Some(frame) = frame_opt else {
                    break;
                };
                let len = frame.payload.len() as u32;
                file.write_all(&frame.timestamp_sec.to_ne_bytes()).await?;
                file.write_all(&frame.timestamp_usec.to_ne_bytes()).await?;
                file.write_all(&len.to_ne_bytes()).await?;
                file.write_all(&len.to_ne_bytes()).await?;
                file.write_all(&frame.payload).await?;
            }
        }
    }

    file.flush().await?;
    drop(file);
    let bytes = tokio::fs::read(&file_path)
        .await
        .with_context(|| format!("failed to read pcap file {}", file_path.display()))?;
    let _ = tokio::fs::remove_file(&file_path).await;
    Ok(bytes)
}

fn pcap_global_header() -> [u8; 24] {
    let mut header = [0u8; 24];
    header[0..4].copy_from_slice(&0xa1b2c3d4u32.to_ne_bytes());
    header[4..6].copy_from_slice(&2u16.to_ne_bytes());
    header[6..8].copy_from_slice(&4u16.to_ne_bytes());
    header[8..12].copy_from_slice(&0i32.to_ne_bytes());
    header[12..16].copy_from_slice(&0u32.to_ne_bytes());
    header[16..20].copy_from_slice(&65535u32.to_ne_bytes());
    header[20..24].copy_from_slice(&101u32.to_ne_bytes());
    header
}

fn capture_file_path(capture_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{capture_id}.pcap"))
}

fn parse_ip_and_ports(packet: &[u8]) -> Option<(CaptureTransport, u16, u16, usize)> {
    if packet.len() < 20 {
        return None;
    }

    let mut ip_start = None;
    if packet.len() >= 34 {
        let ether_type = u16::from_be_bytes([packet[12], packet[13]]);
        let version = packet[14] >> 4;
        if matches!(ether_type, 0x0800 | 0x86dd) && matches!(version, 4 | 6) {
            ip_start = Some(14);
        }
    }

    if ip_start.is_none() && packet.len() >= 36 {
        let proto = u16::from_be_bytes([packet[14], packet[15]]);
        let version = packet[16] >> 4;
        if matches!(proto, 0x0800 | 0x86dd) && matches!(version, 4 | 6) {
            ip_start = Some(16);
        }
    }

    let offset = ip_start?;
    let ip_packet = &packet[offset..];
    let (transport, transport_offset) = match ip_packet.first().map(|value| value >> 4)? {
        4 => {
            if ip_packet.len() < 20 {
                return None;
            }
            let fragment = u16::from_be_bytes([ip_packet[6], ip_packet[7]]) & 0x1fff;
            if fragment != 0 {
                return None;
            }
            let transport = ip_transport(ip_packet[9])?;
            let ihl = (ip_packet[0] & 0x0f) as usize * 4;
            (transport, ihl)
        }
        6 => {
            if ip_packet.len() < 40 {
                return None;
            }
            (ip_transport(ip_packet[6])?, 40)
        }
        _ => return None,
    };
    if ip_packet.len() < transport_offset + 4 {
        return None;
    }
    let segment = &ip_packet[transport_offset..];
    let src_port = u16::from_be_bytes([segment[0], segment[1]]);
    let dst_port = u16::from_be_bytes([segment[2], segment[3]]);
    Some((transport, src_port, dst_port, offset))
}

fn ip_transport(protocol: u8) -> Option<CaptureTransport> {
    match protocol {
        6 => Some(CaptureTransport::Tcp),
        17 => Some(CaptureTransport::Udp),
        _ => None,
    }
}

fn transport_name(transport: CaptureTransport) -> &'static str {
    match transport {
        CaptureTransport::Tcp => "tcp",
        CaptureTransport::Udp => "udp",
    }
}

async fn run_global_raw_socket_listener(
    active_slots: Arc<Mutex<HashMap<String, PcapSlot>>>,
    port_index: Arc<Mutex<HashMap<PortKey, String>>>,
    listeners: Vec<InterfaceListener>,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let interfaces = listeners
            .iter()
            .map(|listener| listener.interface.clone())
            .collect::<Vec<_>>();
        info!(
            interfaces = ?interfaces,
            "pcap raw socket listener selected host interfaces"
        );

        let mut join_set = JoinSet::new();
        for listener in listeners {
            let active_slots = active_slots.clone();
            let port_index = port_index.clone();
            join_set.spawn(async move {
                run_interface_raw_socket_listener(listener, active_slots, port_index).await
            });
        }

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => error!(error = %err, "pcap interface listener failed"),
                Err(err) => error!(error = %err, "pcap interface listener task failed"),
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = active_slots;
        let _ = port_index;
        let _ = listeners;
        anyhow::bail!("pcap capture is only supported on Linux");
    }
}

#[cfg(target_os = "linux")]
async fn prepare_interface_listeners() -> anyhow::Result<Vec<InterfaceListener>> {
    let interfaces = capture_interfaces().await?;
    if interfaces.is_empty() {
        anyhow::bail!("no host network interface is eligible for pcap capture");
    }

    let mut listeners = Vec::new();
    let mut failures = Vec::new();
    for interface in interfaces {
        match open_interface_raw_socket(&interface) {
            Ok(async_fd) => listeners.push(InterfaceListener {
                interface,
                async_fd,
            }),
            Err(err) => failures.push(format!("{interface}: {err:#}")),
        }
    }

    if listeners.is_empty() {
        anyhow::bail!(
            "failed to bind pcap raw socket to host interfaces: {}",
            failures.join("; ")
        );
    }
    for failure in failures {
        warn!(failure, "pcap skipped host interface");
    }
    Ok(listeners)
}

#[cfg(not(target_os = "linux"))]
async fn prepare_interface_listeners() -> anyhow::Result<Vec<()>> {
    anyhow::bail!("pcap capture is only supported on Linux");
}

#[cfg(target_os = "linux")]
async fn capture_interfaces() -> anyhow::Result<Vec<String>> {
    let mut entries = tokio::fs::read_dir("/sys/class/net")
        .await
        .context("failed to read network interfaces")?;
    let mut interfaces = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_excluded_capture_interface(&name) {
            continue;
        }
        let operstate = tokio::fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
            .await
            .unwrap_or_default();
        if !matches!(operstate.trim(), "up" | "unknown") {
            continue;
        }
        interfaces.push(name);
    }
    interfaces.sort();
    Ok(interfaces)
}

#[cfg(target_os = "linux")]
fn is_excluded_capture_interface(name: &str) -> bool {
    name == "lo"
        || name.starts_with("docker")
        || name.starts_with("br-")
        || name.starts_with("veth")
        || name.starts_with("cni")
        || name.starts_with("flannel")
        || name.starts_with("virbr")
        || name.starts_with("kube")
        || name.starts_with("tun")
        || name.starts_with("tap")
}

#[cfg(target_os = "linux")]
fn open_interface_raw_socket(
    interface: &str,
) -> anyhow::Result<tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>> {
    use std::ffi::CString;
    use std::mem;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let raw_fd = unsafe {
        libc::socket(
            libc::PF_PACKET,
            libc::SOCK_RAW,
            (libc::ETH_P_ALL as u16).to_be() as i32,
        )
    };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error()).context(
            "failed to open PF_PACKET socket; agent requires cap_net_raw or root for pcap",
        );
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

    let interface_c = CString::new(interface).context("invalid interface name")?;
    let if_index = unsafe { libc::if_nametoindex(interface_c.as_ptr()) };
    if if_index == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to resolve interface index for {interface}"));
    }

    let mut addr: libc::sockaddr_ll = unsafe { mem::zeroed() };
    addr.sll_family = libc::AF_PACKET as libc::c_ushort;
    addr.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
    addr.sll_ifindex = if_index as i32;
    let bind_result = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            &addr as *const libc::sockaddr_ll as *const libc::sockaddr,
            mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
        )
    };
    if bind_result < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to bind raw socket to {interface}"));
    }

    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags >= 0 {
        let _ = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    }

    tokio::io::unix::AsyncFd::new(fd).context("failed to register raw socket with tokio")
}

#[cfg(target_os = "linux")]
async fn run_interface_raw_socket_listener(
    listener: InterfaceListener,
    active_slots: Arc<Mutex<HashMap<String, PcapSlot>>>,
    port_index: Arc<Mutex<HashMap<PortKey, String>>>,
) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    let InterfaceListener {
        interface,
        async_fd,
    } = listener;
    let mut buf = vec![0u8; 65535];

    loop {
        let mut guard = async_fd.readable().await?;
        match guard.try_io(|inner| {
            let result = unsafe {
                libc::recv(
                    inner.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                    0,
                )
            };
            if result < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(result as usize)
            }
        }) {
            Ok(Ok(bytes_read)) => {
                if let Some((transport, src_port, dst_port, ip_offset)) =
                    parse_ip_and_ports(&buf[..bytes_read])
                {
                    let capture_id = {
                        let port_index = port_index.lock().await;
                        port_index
                            .get(&PortKey {
                                transport,
                                port: src_port,
                            })
                            .or_else(|| {
                                port_index.get(&PortKey {
                                    transport,
                                    port: dst_port,
                                })
                            })
                            .cloned()
                    };
                    if let Some(capture_id) = capture_id {
                        let now = chrono::Utc::now();
                        let frame = RawPacketFrame {
                            timestamp_sec: now.timestamp() as u32,
                            timestamp_usec: now.timestamp_subsec_micros(),
                            payload: buf[ip_offset..bytes_read].to_vec(),
                        };
                        let slots = active_slots.lock().await;
                        if let Some(slot) = slots.get(&capture_id) {
                            let _ = slot.pcap_writer_tx.try_send(frame);
                        }
                    }
                }
            }
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok(Err(err)) => {
                return Err(err).with_context(|| format!("failed to read from {interface}"));
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureTransport, PcapMuxHub, PcapSlot, PortKey, RawPacketFrame, parse_ip_and_ports,
    };
    use tokio::sync::{mpsc, oneshot};

    async fn insert_completed_capture(
        hub: &PcapMuxHub,
        capture_id: &str,
        suite_instance_id: &str,
        workload_id: &str,
    ) {
        let (writer_tx, _writer_rx) = mpsc::channel::<RawPacketFrame>(1);
        let (done_tx, done_rx) = oneshot::channel();
        done_tx.send(Ok(vec![1, 2, 3])).unwrap();
        hub.active_slots.lock().await.insert(
            capture_id.to_string(),
            PcapSlot {
                suite_instance_id: Some(suite_instance_id.to_string()),
                workload_id: Some(workload_id.to_string()),
                pcap_writer_tx: writer_tx,
                cancel_tx: None,
                done_rx,
                watchdog_cancel_tx: None,
            },
        );
    }

    fn ethernet_ipv4(protocol: u8, source: u16, destination: u16) -> Vec<u8> {
        let mut packet = vec![0_u8; 14 + 20 + 8];
        packet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        packet[14] = 0x45;
        packet[23] = protocol;
        packet[34..36].copy_from_slice(&source.to_be_bytes());
        packet[36..38].copy_from_slice(&destination.to_be_bytes());
        packet
    }

    fn ethernet_ipv6(protocol: u8, source: u16, destination: u16) -> Vec<u8> {
        let mut packet = vec![0_u8; 14 + 40 + 8];
        packet[12..14].copy_from_slice(&0x86dd_u16.to_be_bytes());
        packet[14] = 0x60;
        packet[20] = protocol;
        packet[54..56].copy_from_slice(&source.to_be_bytes());
        packet[56..58].copy_from_slice(&destination.to_be_bytes());
        packet
    }

    #[test]
    fn parses_ipv4_tcp_and_udp_bindings() {
        assert_eq!(
            parse_ip_and_ports(&ethernet_ipv4(6, 50_000, 8080)),
            Some((CaptureTransport::Tcp, 50_000, 8080, 14))
        );
        assert_eq!(
            parse_ip_and_ports(&ethernet_ipv4(17, 53, 40_000)),
            Some((CaptureTransport::Udp, 53, 40_000, 14))
        );
    }

    #[test]
    fn parses_ipv6_tcp_and_udp_bindings() {
        assert_eq!(
            parse_ip_and_ports(&ethernet_ipv6(6, 50_001, 445)),
            Some((CaptureTransport::Tcp, 50_001, 445, 14))
        );
        assert_eq!(
            parse_ip_and_ports(&ethernet_ipv6(17, 53, 50_002)),
            Some((CaptureTransport::Udp, 53, 50_002, 14))
        );
    }

    #[test]
    fn rejects_non_initial_ipv4_fragments_and_unknown_protocols() {
        let mut fragment = ethernet_ipv4(6, 1234, 8080);
        fragment[20..22].copy_from_slice(&1_u16.to_be_bytes());
        assert_eq!(parse_ip_and_ports(&fragment), None);
        assert_eq!(parse_ip_and_ports(&ethernet_ipv4(1, 0, 0)), None);
    }

    #[tokio::test]
    async fn timed_out_capture_remains_available_to_finish() {
        let hub = PcapMuxHub::new();
        let (writer_tx, _writer_rx) = mpsc::channel::<RawPacketFrame>(1);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (done_tx, done_rx) = oneshot::channel();
        let (watchdog_cancel_tx, _watchdog_cancel_rx) = oneshot::channel();
        done_tx.send(Ok(vec![1, 2, 3])).unwrap();

        hub.active_slots.lock().await.insert(
            "capture-1".to_string(),
            PcapSlot {
                suite_instance_id: Some("suite-1".to_string()),
                workload_id: Some("workload-1".to_string()),
                pcap_writer_tx: writer_tx,
                cancel_tx: Some(cancel_tx),
                done_rx,
                watchdog_cancel_tx: Some(watchdog_cancel_tx),
            },
        );
        hub.port_index.lock().await.insert(
            PortKey {
                transport: CaptureTransport::Tcp,
                port: 445,
            },
            "capture-1".to_string(),
        );

        hub.timeout_capture("capture-1").await.unwrap();

        assert!(cancel_rx.await.is_ok());
        assert!(hub.port_index.lock().await.is_empty());
        let bytes = hub
            .stop_capture_owned("capture-1", "suite-1", "workload-1")
            .await
            .unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn workload_cleanup_only_discards_owned_captures() {
        let hub = PcapMuxHub::new();
        insert_completed_capture(&hub, "capture-1", "suite-1", "workload-1").await;
        insert_completed_capture(&hub, "capture-2", "suite-1", "workload-2").await;
        insert_completed_capture(&hub, "capture-3", "suite-2", "workload-1").await;

        let stopped = hub
            .stop_captures_for_workload("suite-1", "workload-1")
            .await;

        assert_eq!(stopped, vec!["capture-1"]);
        let slots = hub.active_slots.lock().await;
        assert!(!slots.contains_key("capture-1"));
        assert!(slots.contains_key("capture-2"));
        assert!(slots.contains_key("capture-3"));
    }

    #[tokio::test]
    async fn writer_failure_still_releases_the_last_global_listener() {
        let hub = PcapMuxHub::new();
        let (writer_tx, _writer_rx) = mpsc::channel::<RawPacketFrame>(1);
        let (done_tx, done_rx) = oneshot::channel();
        done_tx.send(Err(anyhow::anyhow!("writer failed"))).unwrap();
        hub.active_slots.lock().await.insert(
            "capture-1".to_string(),
            PcapSlot {
                suite_instance_id: Some("suite-1".to_string()),
                workload_id: Some("workload-1".to_string()),
                pcap_writer_tx: writer_tx,
                cancel_tx: None,
                done_rx,
                watchdog_cancel_tx: None,
            },
        );
        *hub.global_listener_handle.lock().await = Some(tokio::spawn(std::future::pending::<()>()));

        let error = hub.stop_capture("capture-1").await.unwrap_err();

        assert!(error.to_string().contains("writer failed"));
        assert!(hub.global_listener_handle.lock().await.is_none());
    }
}
