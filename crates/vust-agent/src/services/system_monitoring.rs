//! 系统监控采样核心：复用宿主机采样器并维护独立的历史存储。

use chrono::Utc;
use sqlx::FromRow;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{Networks, System};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};
use vust_contracts::monitoring::{
    SystemMonitoringCollectionState, SystemMonitoringMetrics, SystemMonitoringSourceState,
    SystemMonitoringSourceStatus,
};

use crate::state::DbPool;

/// 实时采样间隔。
pub const LIVE_SAMPLE_INTERVAL_SECONDS: u64 = 5;
/// 历史持久化间隔。
pub const HISTORY_SAMPLE_INTERVAL_SECONDS: u32 = 60;
/// 实时快照超过该时间后视为过期。
pub const STALE_AFTER_SECONDS: i64 = 10;
/// 两次累计计数采样间隔超过该时间时不计算速率。
pub const MAX_RATE_INTERVAL_SECONDS: i64 = 150;
const SOURCE_COUNT: u8 = 5;

/// 系统监控持久化设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRow)]
pub struct SystemMonitoringStorageSettings {
    pub history_collection_enabled: bool,
    pub retention_days: u8,
}

/// 单次宿主机原始采样；累计计数仅在 Agent 内部保存。
#[derive(Debug, Clone, PartialEq)]
pub struct RawHostMetricSample {
    pub sampled_at: i64,
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_percent: Option<f64>,
    pub load_average_1m: Option<f64>,
    pub load_average_5m: Option<f64>,
    pub load_average_15m: Option<f64>,
    pub disk_read_bytes: Option<u64>,
    pub disk_write_bytes: Option<u64>,
    pub network_receive_bytes: Option<u64>,
    pub network_transmit_bytes: Option<u64>,
    pub source_statuses: Vec<SystemMonitoringSourceStatus>,
}

impl RawHostMetricSample {
    /// 返回本次采样中可用来源数量。
    pub fn available_source_count(&self) -> u8 {
        self.source_statuses
            .iter()
            .filter(|source| source.state == SystemMonitoringSourceState::Available)
            .count() as u8
    }

    /// 按服务端口径将累计计数转换为速率。
    pub fn metrics(&self, previous: Option<&Self>) -> SystemMonitoringMetrics {
        let interval_seconds = previous.map(|value| self.sampled_at - value.sampled_at);
        let rate = |current: Option<u64>, prior: Option<u64>| -> Option<f64> {
            let interval = interval_seconds?;
            if interval <= 0 || interval > MAX_RATE_INTERVAL_SECONDS {
                return None;
            }
            let delta = current?.checked_sub(prior?)?;
            Some(delta as f64 / interval as f64)
        };

        SystemMonitoringMetrics {
            cpu_percent: self.cpu_percent,
            memory_used_bytes: self.memory_used_bytes,
            memory_total_bytes: self.memory_total_bytes,
            memory_percent: self.memory_percent,
            load_average_1m: self.load_average_1m,
            load_average_5m: self.load_average_5m,
            load_average_15m: self.load_average_15m,
            disk_read_bytes_per_second: rate(
                self.disk_read_bytes,
                previous.and_then(|value| value.disk_read_bytes),
            ),
            disk_write_bytes_per_second: rate(
                self.disk_write_bytes,
                previous.and_then(|value| value.disk_write_bytes),
            ),
            network_receive_bytes_per_second: rate(
                self.network_receive_bytes,
                previous.and_then(|value| value.network_receive_bytes),
            ),
            network_transmit_bytes_per_second: rate(
                self.network_transmit_bytes,
                previous.and_then(|value| value.network_transmit_bytes),
            ),
        }
    }
}

/// 系统监控共享运行时，不把读取请求转换为新的主机采样。
pub struct SystemMonitoringRuntime {
    pub latest_sample: RwLock<Option<RawHostMetricSample>>,
    pub previous_sample: RwLock<Option<RawHostMetricSample>>,
    pub settings: RwLock<SystemMonitoringStorageSettings>,
    pub collection_state: RwLock<SystemMonitoringCollectionState>,
    pub maintenance: Mutex<()>,
}

impl SystemMonitoringRuntime {
    /// 从独立设置表加载运行时。
    pub async fn load(pool: &DbPool) -> anyhow::Result<Self> {
        let settings = load_storage_settings(pool).await?;
        let collection_state = if settings.history_collection_enabled {
            SystemMonitoringCollectionState::Initializing
        } else {
            SystemMonitoringCollectionState::Stopped
        };
        Ok(Self {
            latest_sample: RwLock::new(None),
            previous_sample: RwLock::new(None),
            settings: RwLock::new(settings),
            collection_state: RwLock::new(collection_state),
            maintenance: Mutex::new(()),
        })
    }
}

/// 启动唯一的后台宿主机采样器。
pub fn spawn_sampler(pool: DbPool, runtime: Arc<SystemMonitoringRuntime>) {
    tokio::spawn(async move {
        let mut sampler = HostSampler::new();
        let mut ticker = tokio::time::interval(Duration::from_secs(LIVE_SAMPLE_INTERVAL_SECONDS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_persisted_at = 0_i64;

        loop {
            ticker.tick().await;
            let sample = sampler.sample();
            {
                let mut previous = runtime.previous_sample.write().await;
                let mut latest = runtime.latest_sample.write().await;
                *previous = latest.take();
                *latest = Some(sample.clone());
            }

            let settings = *runtime.settings.read().await;
            if !settings.history_collection_enabled {
                *runtime.collection_state.write().await = SystemMonitoringCollectionState::Stopped;
                continue;
            }
            if sample.sampled_at - last_persisted_at < i64::from(HISTORY_SAMPLE_INTERVAL_SECONDS) {
                continue;
            }

            match persist_sample(&pool, &sample, settings.retention_days).await {
                Ok(()) => {
                    last_persisted_at = sample.sampled_at;
                    *runtime.collection_state.write().await =
                        SystemMonitoringCollectionState::Running;
                }
                Err(error) => {
                    *runtime.collection_state.write().await =
                        SystemMonitoringCollectionState::Degraded;
                    warn!(%error, "System monitoring history persistence failed");
                }
            }
        }
    });
    debug!("System monitoring sampler started");
}

/// 从专用表读取系统监控设置。
pub async fn load_storage_settings(
    pool: &DbPool,
) -> anyhow::Result<SystemMonitoringStorageSettings> {
    sqlx::query_as::<_, SystemMonitoringStorageSettings>(
        r#"
        SELECT history_collection_enabled, retention_days
        FROM system_monitoring_settings
        WHERE singleton_id = 1
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

async fn persist_sample(
    pool: &DbPool,
    sample: &RawHostMetricSample,
    retention_days: u8,
) -> anyhow::Result<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT OR REPLACE INTO system_monitoring_samples (
            sampled_at, cpu_percent, memory_used_bytes, memory_total_bytes, memory_percent,
            load_average_1m, load_average_5m, load_average_15m,
            disk_read_bytes, disk_write_bytes,
            network_receive_bytes, network_transmit_bytes,
            available_source_count, source_count
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(sample.sampled_at)
    .bind(sample.cpu_percent)
    .bind(as_i64(sample.memory_used_bytes))
    .bind(as_i64(sample.memory_total_bytes))
    .bind(sample.memory_percent)
    .bind(sample.load_average_1m)
    .bind(sample.load_average_5m)
    .bind(sample.load_average_15m)
    .bind(as_i64(sample.disk_read_bytes))
    .bind(as_i64(sample.disk_write_bytes))
    .bind(as_i64(sample.network_receive_bytes))
    .bind(as_i64(sample.network_transmit_bytes))
    .bind(i64::from(sample.available_source_count()))
    .bind(i64::from(SOURCE_COUNT))
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        UPDATE system_monitoring_collector_state
        SET last_attempted_at = ?, last_sampled_at = ?, last_error_summary = NULL
        WHERE singleton_id = 1
        "#,
    )
    .bind(sample.sampled_at)
    .bind(sample.sampled_at)
    .execute(&mut *transaction)
    .await?;

    let cutoff = sample.sampled_at - i64::from(retention_days) * 86_400;
    sqlx::query("DELETE FROM system_monitoring_samples WHERE sampled_at < ?")
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

fn as_i64(value: Option<u64>) -> Option<i64> {
    value.and_then(|value| i64::try_from(value).ok())
}

struct HostSampler {
    system: System,
    networks: Networks,
    cpu_ready: bool,
}

impl HostSampler {
    fn new() -> Self {
        Self {
            system: System::new_all(),
            networks: Networks::new_with_refreshed_list(),
            cpu_ready: false,
        }
    }

    fn sample(&mut self) -> RawHostMetricSample {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh();
        let sampled_at = Utc::now().timestamp();

        let cpu_percent = self
            .cpu_ready
            .then(|| f64::from(self.system.global_cpu_info().cpu_usage()));
        self.cpu_ready = true;
        let memory_total_bytes =
            (self.system.total_memory() > 0).then_some(self.system.total_memory());
        let memory_used_bytes =
            memory_total_bytes.map(|total| total.saturating_sub(self.system.available_memory()));
        let memory_percent = memory_used_bytes
            .zip(memory_total_bytes)
            .map(|(used, total)| used as f64 / total as f64 * 100.0);
        let load = System::load_average();
        let disk = collect_disk_io_totals();
        let network = collect_network_io_totals(&self.networks);

        let sources = vec![
            source_status("cpu", cpu_percent.is_some()),
            source_status("memory", memory_total_bytes.is_some()),
            source_status("load", true),
            source_status("diskIo", disk.is_some()),
            source_status(
                "networkIo",
                network.as_ref().is_some_and(|value| value.reliable),
            ),
        ];
        let (disk_read_bytes, disk_write_bytes) = disk.unzip();
        let (network_receive_bytes, network_transmit_bytes) = network
            .map(|value| (value.received_bytes, value.transmitted_bytes))
            .unzip();

        RawHostMetricSample {
            sampled_at,
            cpu_percent,
            memory_used_bytes,
            memory_total_bytes,
            memory_percent,
            load_average_1m: Some(load.one),
            load_average_5m: Some(load.five),
            load_average_15m: Some(load.fifteen),
            disk_read_bytes,
            disk_write_bytes,
            network_receive_bytes,
            network_transmit_bytes,
            source_statuses: sources,
        }
    }
}

fn source_status(source: &str, available: bool) -> SystemMonitoringSourceStatus {
    SystemMonitoringSourceStatus {
        source: source.to_string(),
        state: if available {
            SystemMonitoringSourceState::Available
        } else {
            SystemMonitoringSourceState::Unavailable
        },
    }
}

#[derive(Debug)]
struct NetworkIoTotals {
    received_bytes: u64,
    transmitted_bytes: u64,
    reliable: bool,
}

fn collect_network_io_totals(networks: &Networks) -> Option<NetworkIoTotals> {
    let mut physical = Vec::new();
    for (name, data) in networks {
        if name == "lo" || !interface_is_up(name) {
            continue;
        }
        if Path::new("/sys/class/net")
            .join(name)
            .join("device")
            .exists()
        {
            physical.push(data);
        }
    }
    if !physical.is_empty() {
        return Some(NetworkIoTotals {
            received_bytes: physical.iter().map(|data| data.total_received()).sum(),
            transmitted_bytes: physical.iter().map(|data| data.total_transmitted()).sum(),
            reliable: true,
        });
    }

    let default_interface = default_route_interface()?;
    let data = networks.get(&default_interface)?;
    Some(NetworkIoTotals {
        received_bytes: data.total_received(),
        transmitted_bytes: data.total_transmitted(),
        reliable: false,
    })
}

fn interface_is_up(name: &str) -> bool {
    std::fs::read_to_string(Path::new("/sys/class/net").join(name).join("operstate"))
        .map(|state| state.trim() == "up")
        .unwrap_or(false)
}

fn default_route_interface() -> Option<String> {
    let content = std::fs::read_to_string("/proc/net/route").ok()?;
    content.lines().skip(1).find_map(|line| {
        let columns: Vec<_> = line.split_whitespace().collect();
        (columns.len() > 3 && columns[1] == "00000000" && columns[3] != "0000")
            .then(|| columns[0].to_string())
    })
}

fn collect_disk_io_totals() -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/proc/diskstats").ok()?;
        let mut read_sectors = 0_u64;
        let mut write_sectors = 0_u64;
        let mut found = false;

        for line in content.lines() {
            let columns: Vec<_> = line.split_whitespace().collect();
            if columns.len() < 14 {
                continue;
            }
            let name = columns[2];
            let device_path = Path::new("/sys/class/block").join(name);
            if !device_path.join("device").exists()
                || device_path.join("partition").exists()
                || is_pseudo_block_device(name)
            {
                continue;
            }
            let read = columns[5].parse::<u64>().ok()?;
            let write = columns[9].parse::<u64>().ok()?;
            read_sectors = read_sectors.saturating_add(read);
            write_sectors = write_sectors.saturating_add(write);
            found = true;
        }
        found.then(|| {
            (
                read_sectors.saturating_mul(512),
                write_sectors.saturating_mul(512),
            )
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn is_pseudo_block_device(name: &str) -> bool {
    ["loop", "ram", "zram", "fd", "sr", "dm-", "md"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: i64, disk: Option<u64>, network: Option<u64>) -> RawHostMetricSample {
        RawHostMetricSample {
            sampled_at: at,
            cpu_percent: Some(0.0),
            memory_used_bytes: Some(0),
            memory_total_bytes: Some(1),
            memory_percent: Some(0.0),
            load_average_1m: Some(0.0),
            load_average_5m: Some(0.0),
            load_average_15m: Some(0.0),
            disk_read_bytes: disk,
            disk_write_bytes: disk,
            network_receive_bytes: network,
            network_transmit_bytes: network,
            source_statuses: Vec::new(),
        }
    }

    #[test]
    fn rate_requires_previous_sample() {
        let current = sample(60, Some(600), Some(1_200));
        assert_eq!(current.metrics(None).disk_read_bytes_per_second, None);
    }

    #[test]
    fn rate_distinguishes_real_zero_from_missing() {
        let previous = sample(60, Some(600), Some(1_200));
        let current = sample(120, Some(600), Some(1_200));
        let metrics = current.metrics(Some(&previous));
        assert_eq!(metrics.disk_read_bytes_per_second, Some(0.0));
        assert_eq!(metrics.network_receive_bytes_per_second, Some(0.0));
    }

    #[test]
    fn rate_rejects_counter_reset_and_long_gap() {
        let previous = sample(60, Some(600), Some(1_200));
        let reset = sample(120, Some(100), Some(100));
        assert_eq!(
            reset.metrics(Some(&previous)).disk_read_bytes_per_second,
            None
        );
        let delayed = sample(211, Some(1_000), Some(2_000));
        assert_eq!(
            delayed.metrics(Some(&previous)).disk_read_bytes_per_second,
            None
        );
    }
}
