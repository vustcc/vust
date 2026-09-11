//! 进程与网络观察服务：维护节点级共享采样快照并执行稳定分页查询。

use crate::{
    state::{AppState, DbPool},
    types::{ApiError, ApiResult},
};
use axum::http::StatusCode;
use bollard::{models::ContainerSummary, query_parameters::ListContainersOptionsBuilder};
use chrono::{SecondsFormat, TimeZone, Utc};
use ring::digest::{SHA256, digest};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    os::fd::{FromRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Arc,
    time::Instant,
};
use tokio::{
    sync::{Mutex, RwLock},
    task,
    time::{self, Duration, MissedTickBehavior},
};
use tracing::warn;
use vust_contracts::{
    api::ErrorCode,
    process::{
        NetworkConnectionListPage, NetworkConnectionListQuery, NetworkConnectionOwner,
        NetworkConnectionState, NetworkConnectionSummary, NetworkEndpoint, NetworkProtocol,
        NetworkSortBy, ProcessCapabilities, ProcessListPage, ProcessListQuery, ProcessManagement,
        ProcessManagementKind, ProcessSignal, ProcessSignalDeliveryStatus, ProcessSignalResult,
        ProcessSortBy, ProcessState, ProcessSummary, SamplingCoverage, SamplingStatus, SortOrder,
    },
};

const PROCESS_SAMPLE_INTERVAL: Duration = Duration::from_secs(3);
const NETWORK_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_CPU_SAMPLE_GAP_SECONDS: f64 = 6.0;
const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 500;

/// 供信号执行阶段重新核验的稳定进程身份。
#[derive(Debug, Clone)]
pub struct ProcessIdentity {
    pub process_id: String,
    pub pid: u32,
    pub start_time_ticks: u64,
    pub name: String,
}

#[derive(Debug, Clone)]
struct RawProcess {
    identity: ProcessIdentity,
    parent_pid: u32,
    thread_count: u64,
    user_name: String,
    state: ProcessState,
    total_cpu_ticks: u64,
    resident_memory_bytes: u64,
    started_at: Option<String>,
    cgroup: String,
}

#[derive(Debug, Clone)]
struct ProcessSnapshot {
    entries: Vec<ProcessSummary>,
    identities: HashMap<String, ProcessIdentity>,
    sampled_at: String,
    coverage: SamplingCoverage,
}

#[derive(Debug, Clone)]
struct NetworkSnapshot {
    entries: Vec<NetworkConnectionSummary>,
    counts_by_process: HashMap<String, u64>,
    sampled_at: String,
    coverage: SamplingCoverage,
}

/// 解析后的连接及其是否具备可追溯的 socket inode。
#[derive(Debug)]
struct ParsedNetworkConnection {
    summary: NetworkConnectionSummary,
    owner_attribution_expected: bool,
}

#[derive(Debug)]
struct PreviousCpuSample {
    sampled_at: Instant,
    ticks_by_process: HashMap<String, u64>,
}

/// Agent 进程管理共享运行时。采样成本与浏览器连接数无关。
pub struct ProcessManagerRuntime {
    process_snapshot: RwLock<Option<ProcessSnapshot>>,
    network_snapshot: RwLock<Option<NetworkSnapshot>>,
    previous_cpu: RwLock<Option<PreviousCpuSample>>,
    signal_locks: Mutex<HashSet<String>>,
}

impl Default for ProcessManagerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessManagerRuntime {
    /// 创建尚未采样的运行时。
    pub fn new() -> Self {
        Self {
            process_snapshot: RwLock::new(None),
            network_snapshot: RwLock::new(None),
            previous_cpu: RwLock::new(None),
            signal_locks: Mutex::new(HashSet::new()),
        }
    }

    /// 启动进程与网络共享采样循环。
    pub fn spawn_samplers(self: &Arc<Self>, state: Arc<AppState>) {
        let runtime = Arc::clone(self);
        let process_state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut ticker = time::interval(PROCESS_SAMPLE_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                if let Err(error) = runtime.refresh_process_snapshot(&process_state).await {
                    warn!(error = %error, "Process sampler refresh failed");
                }
            }
        });
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = time::interval(NETWORK_SAMPLE_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                if let Err(error) = runtime.refresh_network_snapshot().await {
                    warn!(error = %error, "Network sampler refresh failed");
                }
            }
        });
    }

    /// 立即刷新一次进程快照，启动阶段用它避免伪装为空数据。
    pub async fn refresh_process_snapshot(&self, state: &AppState) -> Result<(), String> {
        let (containers, ownership_complete) = container_management_facts(state).await;
        self.refresh_process_snapshot_with_management(containers, ownership_complete)
            .await
    }

    async fn refresh_process_snapshot_with_management(
        &self,
        container_facts: HashMap<String, ProcessManagement>,
        ownership_complete: bool,
    ) -> Result<(), String> {
        let collected = task::spawn_blocking(|| collect_raw_processes(Path::new("/proc")))
            .await
            .map_err(|_| "process sampler worker interrupted".to_string())??;
        let now = Instant::now();
        let mut previous = self.previous_cpu.write().await;
        let elapsed_seconds = previous
            .as_ref()
            .map(|sample| now.duration_since(sample.sampled_at).as_secs_f64())
            .filter(|elapsed| *elapsed > 0.0 && *elapsed <= MAX_CPU_SAMPLE_GAP_SECONDS);
        let previous_ticks = previous.as_ref().map(|sample| &sample.ticks_by_process);
        let clock_ticks = clock_ticks_per_second();
        let memory_total_bytes = read_memory_total_bytes(Path::new("/proc")).unwrap_or_default();

        let mut entries = Vec::with_capacity(collected.processes.len());
        let mut identities = HashMap::with_capacity(collected.processes.len());
        let mut next_ticks = HashMap::with_capacity(collected.processes.len());
        for process in collected.processes {
            let cpu_percent = previous_ticks
                .and_then(|ticks| ticks.get(&process.identity.process_id))
                .zip(elapsed_seconds)
                .and_then(|(previous_ticks, elapsed)| {
                    process
                        .total_cpu_ticks
                        .checked_sub(*previous_ticks)
                        .map(|delta| delta as f64 / clock_ticks / elapsed * 100.0)
                });
            let memory_percent = (memory_total_bytes > 0)
                .then(|| process.resident_memory_bytes as f64 / memory_total_bytes as f64 * 100.0);
            next_ticks.insert(process.identity.process_id.clone(), process.total_cpu_ticks);
            identities.insert(
                process.identity.process_id.clone(),
                process.identity.clone(),
            );
            let management = classify_process_management(
                process.identity.pid,
                &process.identity.name,
                &process.cgroup,
                &container_facts,
            );
            entries.push(ProcessSummary {
                process_id: process.identity.process_id,
                pid: process.identity.pid,
                name: process.identity.name,
                parent_pid: process.parent_pid,
                thread_count: process.thread_count,
                user_name: process.user_name,
                state: process.state,
                cpu_percent,
                memory_percent,
                resident_memory_bytes: Some(process.resident_memory_bytes),
                connection_count: None,
                started_at: process.started_at,
                management,
                capabilities: ProcessCapabilities {
                    can_terminate: true,
                    can_force_kill: true,
                },
            });
        }
        entries.sort_by_key(|entry| entry.pid);
        *previous = Some(PreviousCpuSample {
            sampled_at: now,
            ticks_by_process: next_ticks,
        });
        let mut coverage = collected.coverage;
        if !ownership_complete
            && entries
                .iter()
                .any(|entry| entry.management.manage_via.as_deref() == Some("docker"))
        {
            coverage.status = SamplingStatus::Partial;
            coverage
                .warnings
                .push("container ownership metadata was unavailable during sampling".to_string());
        }
        *self.process_snapshot.write().await = Some(ProcessSnapshot {
            entries,
            identities,
            sampled_at: collected.sampled_at,
            coverage,
        });
        Ok(())
    }

    /// 返回经过服务端筛选、排序和分页的进程摘要。
    pub async fn list_processes(&self, query: ProcessListQuery) -> ApiResult<ProcessListPage> {
        let snapshot = self
            .process_snapshot
            .read()
            .await
            .clone()
            .ok_or_else(process_sampler_unavailable)?;
        let (page, page_size) = validate_page(query.page, query.page_size)?;
        let network_counts = self
            .network_snapshot
            .read()
            .await
            .as_ref()
            .map(|snapshot| snapshot.counts_by_process.clone());
        let mut available_entries = snapshot.entries.clone();
        if let Some(counts) = network_counts {
            for entry in &mut available_entries {
                entry.connection_count = Some(counts.get(&entry.process_id).copied().unwrap_or(0));
            }
        }
        let available_total = available_entries.len();
        let keyword = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let keyword = keyword.map(str::to_lowercase);
        let mut filtered = available_entries
            .iter()
            .filter(|entry| query.status.is_none_or(|state| entry.state == state))
            .filter(|entry| {
                keyword.as_ref().is_none_or(|keyword| {
                    entry.name.to_lowercase().contains(keyword)
                        || entry.user_name.to_lowercase().contains(keyword)
                        || entry.pid.to_string().contains(keyword)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        sort_processes(
            &mut filtered,
            query.sort_by.unwrap_or(ProcessSortBy::Pid),
            query.sort_order.unwrap_or(SortOrder::Asc),
        );
        let total = filtered.len();
        let start = page.saturating_sub(1).saturating_mul(page_size);
        let entries = filtered.into_iter().skip(start).take(page_size).collect();
        let mut counts = BTreeMap::new();
        for entry in &available_entries {
            *counts
                .entry(process_state_key(entry.state).to_string())
                .or_insert(0) += 1;
        }
        Ok(ProcessListPage {
            entries,
            page,
            page_size,
            available_total,
            total,
            counts,
            sampled_at: snapshot.sampled_at,
            coverage: snapshot.coverage,
        })
    }

    /// 根据不透明 processId 返回最近采样到的稳定身份。
    pub async fn process_identity(&self, process_id: &str) -> Option<ProcessIdentity> {
        self.process_snapshot
            .read()
            .await
            .as_ref()
            .and_then(|snapshot| snapshot.identities.get(process_id).cloned())
    }

    /// 使用稳定 processId 和 pidfd 投递进程信号。
    pub async fn deliver_signal(
        &self,
        pool: &DbPool,
        process_id: &str,
        signal: ProcessSignal,
        idempotency_key: &str,
        actor: &SignalActorContext,
    ) -> ApiResult<ProcessSignalResult> {
        validate_signal_request(process_id, idempotency_key)?;
        if let Some(existing) = load_signal_operation(pool, idempotency_key).await? {
            return stored_signal_result(existing, process_id, signal);
        }
        {
            let mut locks = self.signal_locks.lock().await;
            if !locks.insert(process_id.to_string()) {
                return Err(ApiError::conflict(
                    ErrorCode::ProcessOperationConflict,
                    "another signal operation is already running for this process",
                ));
            }
        }
        let result = self
            .deliver_signal_inner(pool, process_id, signal, idempotency_key, actor)
            .await;
        self.signal_locks.lock().await.remove(process_id);
        result
    }

    async fn deliver_signal_inner(
        &self,
        pool: &DbPool,
        process_id: &str,
        signal: ProcessSignal,
        idempotency_key: &str,
        actor: &SignalActorContext,
    ) -> ApiResult<ProcessSignalResult> {
        if let Some(existing) = load_signal_operation(pool, idempotency_key).await? {
            return stored_signal_result(existing, process_id, signal);
        }
        let identity = self.process_identity(process_id).await.ok_or_else(|| {
            ApiError::not_found(ErrorCode::ProcessNotFound, "process no longer exists")
        })?;
        insert_signal_operation(pool, idempotency_key, &identity, signal, actor).await?;
        let result = deliver_signal_to_identity(&identity, signal);
        match result {
            Ok(()) => {
                let delivered_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
                finish_signal_operation(
                    pool,
                    idempotency_key,
                    "delivered",
                    Some(&delivered_at),
                    None,
                    None,
                )
                .await?;
                Ok(ProcessSignalResult {
                    idempotency_key: idempotency_key.to_string(),
                    process_id: identity.process_id,
                    pid: Some(identity.pid),
                    process_name: Some(identity.name),
                    signal,
                    status: ProcessSignalDeliveryStatus::Delivered,
                    delivered_at: Some(delivered_at),
                    error_summary: None,
                })
            }
            Err(error) => {
                finish_signal_operation(
                    pool,
                    idempotency_key,
                    "failed",
                    None,
                    Some(error.code.as_str()),
                    Some(error.message.as_ref()),
                )
                .await?;
                Err(error)
            }
        }
    }

    /// 立即刷新一次网络连接快照。
    pub async fn refresh_network_snapshot(&self) -> Result<(), String> {
        let identities = self
            .process_snapshot
            .read()
            .await
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .identities
                    .values()
                    .map(|identity| (identity.pid, identity.clone()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let snapshot =
            task::spawn_blocking(move || collect_network_snapshot(Path::new("/proc"), &identities))
                .await
                .map_err(|_| "network sampler worker interrupted".to_string())??;
        *self.network_snapshot.write().await = Some(snapshot);
        Ok(())
    }

    /// 返回经过服务端筛选、排序和分页的网络连接摘要。
    pub async fn list_network_connections(
        &self,
        query: NetworkConnectionListQuery,
    ) -> ApiResult<NetworkConnectionListPage> {
        let snapshot = self
            .network_snapshot
            .read()
            .await
            .clone()
            .ok_or_else(network_sampler_unavailable)?;
        let (page, page_size) = validate_page(query.page, query.page_size)?;
        let available_total = snapshot.entries.len();
        let keyword = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);
        let mut filtered = snapshot
            .entries
            .iter()
            .filter(|entry| query.state.is_none_or(|state| entry.state == state))
            .filter(|entry| {
                query
                    .protocol
                    .is_none_or(|protocol| entry.protocol == protocol)
            })
            .filter(|entry| {
                keyword.as_ref().is_none_or(|keyword| {
                    format_endpoint(&entry.local_endpoint)
                        .to_lowercase()
                        .contains(keyword)
                        || entry.remote_endpoint.as_ref().is_some_and(|endpoint| {
                            format_endpoint(endpoint).to_lowercase().contains(keyword)
                        })
                        || network_protocol_key(entry.protocol).contains(keyword)
                        || entry.owners.iter().any(|owner| {
                            owner.process_name.to_lowercase().contains(keyword)
                                || owner.pid.to_string().contains(keyword)
                        })
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        sort_network_connections(
            &mut filtered,
            query.sort_by.unwrap_or(NetworkSortBy::Protocol),
            query.sort_order.unwrap_or(SortOrder::Asc),
        );
        let total = filtered.len();
        let start = page.saturating_sub(1).saturating_mul(page_size);
        let entries = filtered.into_iter().skip(start).take(page_size).collect();
        let mut by_state = BTreeMap::new();
        let mut by_protocol = BTreeMap::new();
        for entry in &snapshot.entries {
            *by_state
                .entry(network_state_key(entry.state).to_string())
                .or_insert(0) += 1;
            *by_protocol
                .entry(network_protocol_key(entry.protocol).to_string())
                .or_insert(0) += 1;
        }
        Ok(NetworkConnectionListPage {
            entries,
            page,
            page_size,
            available_total,
            total,
            by_state,
            by_protocol,
            sampled_at: snapshot.sampled_at,
            coverage: snapshot.coverage,
        })
    }
}

/// Master 注入并由 Agent 完整校验的可信操作上下文。
#[derive(Debug, Clone)]
pub struct SignalActorContext {
    pub actor_name: String,
    pub client_ip: String,
    pub trace_id: String,
}

#[derive(Debug)]
struct StoredSignalOperation {
    result: ProcessSignalResult,
    error_code: Option<String>,
}

/// Agent 启动时将未完成信号标记为不可确定，绝不自动重放。
pub async fn initialize_signal_operations(pool: &DbPool) -> ApiResult<()> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query(
        "UPDATE process_signal_operations SET status = 'outcome_unknown', updated_at = ?1, \
         error_summary = 'signal delivery outcome is unknown after agent restart' \
         WHERE status = 'submitting'",
    )
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

fn validate_signal_request(process_id: &str, idempotency_key: &str) -> ApiResult<()> {
    if process_id.len() != 64 || !process_id.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ProcessInvalidId,
            "invalid process identity",
        ));
    }
    if uuid::Uuid::parse_str(idempotency_key).is_err() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "idempotencyKey must be a UUID",
        ));
    }
    Ok(())
}

fn deliver_signal_to_identity(identity: &ProcessIdentity, signal: ProcessSignal) -> ApiResult<()> {
    let current_start_time = fs::read_to_string(format!("/proc/{}/stat", identity.pid))
        .ok()
        .and_then(|value| parse_proc_stat(&value))
        .map(|value| value.start_time_ticks)
        .ok_or_else(|| {
            ApiError::not_found(ErrorCode::ProcessNotFound, "process no longer exists")
        })?;
    if current_start_time != identity.start_time_ticks {
        return Err(ApiError::conflict(
            ErrorCode::ProcessChanged,
            "process identity changed before signal delivery",
        ));
    }
    let raw_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid as libc::pid_t, 0) };
    if raw_fd < 0 {
        return Err(map_pidfd_error(std::io::Error::last_os_error(), true));
    }
    let pidfd = unsafe { OwnedFd::from_raw_fd(raw_fd as i32) };
    let signal_number = match signal {
        ProcessSignal::Term => libc::SIGTERM,
        ProcessSignal::Kill => libc::SIGKILL,
    };
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            std::os::fd::AsRawFd::as_raw_fd(&pidfd),
            signal_number,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        return Err(map_pidfd_error(std::io::Error::last_os_error(), false));
    }
    Ok(())
}

fn map_pidfd_error(error: std::io::Error, opening: bool) -> ApiError {
    match error.raw_os_error() {
        Some(libc::ESRCH) if opening => {
            ApiError::not_found(ErrorCode::ProcessNotFound, "process no longer exists")
        }
        Some(libc::ESRCH) => ApiError::conflict(
            ErrorCode::ProcessChanged,
            "process identity changed before signal delivery",
        ),
        Some(libc::EPERM | libc::EACCES) => ApiError::forbidden(
            ErrorCode::ProcessPermissionDenied,
            "kernel denied process signal delivery",
        ),
        Some(libc::ENOSYS | libc::EINVAL) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProcessSignalUnavailable,
            "stable process signaling is unavailable on this kernel",
        ),
        _ => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProcessSignalUnavailable,
            "process signal delivery failed",
        ),
    }
}

async fn insert_signal_operation(
    pool: &DbPool,
    idempotency_key: &str,
    identity: &ProcessIdentity,
    signal: ProcessSignal,
    actor: &SignalActorContext,
) -> ApiResult<()> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query(
        "INSERT INTO process_signal_operations (idempotency_key, process_id, pid, process_name, \
         signal, status, actor_name, client_ip, trace_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'submitting', ?6, ?7, ?8, ?9, ?9)",
    )
    .bind(idempotency_key)
    .bind(&identity.process_id)
    .bind(i64::from(identity.pid))
    .bind(&identity.name)
    .bind(signal_key(signal))
    .bind(&actor.actor_name)
    .bind(&actor.client_ip)
    .bind(&actor.trace_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

async fn finish_signal_operation(
    pool: &DbPool,
    idempotency_key: &str,
    status: &str,
    delivered_at: Option<&str>,
    error_code: Option<&str>,
    error_summary: Option<&str>,
) -> ApiResult<()> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query(
        "UPDATE process_signal_operations SET status = ?2, delivered_at = ?3, error_code = ?4, \
         error_summary = ?5, updated_at = ?6 WHERE idempotency_key = ?1",
    )
    .bind(idempotency_key)
    .bind(status)
    .bind(delivered_at)
    .bind(error_code)
    .bind(error_summary)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

async fn load_signal_operation(
    pool: &DbPool,
    idempotency_key: &str,
) -> ApiResult<Option<StoredSignalOperation>> {
    use sqlx::Row;
    let Some(row) = sqlx::query(
        "SELECT process_id, pid, process_name, signal, status, delivered_at, error_code, \
         error_summary FROM process_signal_operations WHERE idempotency_key = ?1",
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?
    else {
        return Ok(None);
    };
    let signal_text: String = row.try_get("signal").unwrap_or_else(|_| "term".to_string());
    let status_text: String = row
        .try_get("status")
        .unwrap_or_else(|_| "failed".to_string());
    Ok(Some(StoredSignalOperation {
        result: ProcessSignalResult {
            idempotency_key: idempotency_key.to_string(),
            process_id: row.try_get("process_id").unwrap_or_default(),
            pid: row
                .try_get::<Option<i64>, _>("pid")
                .ok()
                .flatten()
                .map(|value| value as u32),
            process_name: row.try_get("process_name").ok().flatten(),
            signal: if signal_text == "kill" {
                ProcessSignal::Kill
            } else {
                ProcessSignal::Term
            },
            status: match status_text.as_str() {
                "delivered" => ProcessSignalDeliveryStatus::Delivered,
                "outcome_unknown" | "submitting" => ProcessSignalDeliveryStatus::OutcomeUnknown,
                _ => ProcessSignalDeliveryStatus::Failed,
            },
            delivered_at: row.try_get("delivered_at").ok().flatten(),
            error_summary: row.try_get("error_summary").ok().flatten(),
        },
        error_code: row.try_get("error_code").ok().flatten(),
    }))
}

fn stored_signal_result(
    stored: StoredSignalOperation,
    process_id: &str,
    signal: ProcessSignal,
) -> ApiResult<ProcessSignalResult> {
    if stored.result.process_id != process_id || stored.result.signal != signal {
        return Err(ApiError::conflict(
            ErrorCode::ProcessOperationConflict,
            "idempotencyKey is already bound to another process operation",
        ));
    }
    if stored.result.status != ProcessSignalDeliveryStatus::Failed {
        return Ok(stored.result);
    }
    let code = stored
        .error_code
        .as_deref()
        .unwrap_or("PROCESS_SIGNAL_UNAVAILABLE");
    let message = stored
        .result
        .error_summary
        .as_deref()
        .unwrap_or("process signal delivery failed");
    Err(match code {
        "PROCESS_NOT_FOUND" => ApiError::not_found(ErrorCode::ProcessNotFound, message.to_string()),
        "PROCESS_CHANGED" => ApiError::conflict(ErrorCode::ProcessChanged, message.to_string()),
        "PROCESS_PERMISSION_DENIED" => {
            ApiError::forbidden(ErrorCode::ProcessPermissionDenied, message.to_string())
        }
        _ => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProcessSignalUnavailable,
            message.to_string(),
        ),
    })
}

fn signal_key(signal: ProcessSignal) -> &'static str {
    match signal {
        ProcessSignal::Term => "term",
        ProcessSignal::Kill => "kill",
    }
}

struct CollectedProcesses {
    processes: Vec<RawProcess>,
    sampled_at: String,
    coverage: SamplingCoverage,
}

fn collect_raw_processes(proc_root: &Path) -> Result<CollectedProcesses, String> {
    let entries = fs::read_dir(proc_root).map_err(|_| "process filesystem is unavailable")?;
    let boot_id = fs::read_to_string(proc_root.join("sys/kernel/random/boot_id"))
        .unwrap_or_else(|_| "unknown-boot".to_string());
    let boot_time = read_boot_time(proc_root);
    let users = read_passwd_users(Path::new("/etc/passwd"));
    let clock_ticks = clock_ticks_per_second();
    let page_size = page_size_bytes();
    let mut scanned_count = 0_usize;
    let mut failed_count = 0_usize;
    let mut processes = Vec::new();

    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        scanned_count += 1;
        match read_raw_process(
            proc_root,
            pid,
            &boot_id,
            boot_time,
            clock_ticks,
            page_size,
            &users,
        ) {
            Some(process) => processes.push(process),
            None => failed_count += 1,
        }
    }
    let succeeded_count = processes.len();
    let warnings = if failed_count > 0 {
        vec!["some processes changed or were unreadable during sampling".to_string()]
    } else {
        Vec::new()
    };
    Ok(CollectedProcesses {
        processes,
        sampled_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        coverage: SamplingCoverage {
            status: if failed_count > 0 {
                SamplingStatus::Partial
            } else {
                SamplingStatus::Complete
            },
            scanned_count,
            succeeded_count,
            failed_count,
            owner_coverage_percent: None,
            warnings,
        },
    })
}

fn read_raw_process(
    proc_root: &Path,
    pid: u32,
    boot_id: &str,
    boot_time: Option<i64>,
    clock_ticks: f64,
    page_size: u64,
    users: &HashMap<u32, String>,
) -> Option<RawProcess> {
    let process_dir = proc_root.join(pid.to_string());
    let stat = fs::read_to_string(process_dir.join("stat")).ok()?;
    let parsed = parse_proc_stat(&stat)?;
    let uid = fs::metadata(&process_dir).ok()?.uid();
    let user_name = users.get(&uid).cloned().unwrap_or_else(|| uid.to_string());
    let process_id = process_id(boot_id.trim(), pid, parsed.start_time_ticks);
    let started_at = boot_time.and_then(|boot_time| {
        let timestamp = boot_time as f64 + parsed.start_time_ticks as f64 / clock_ticks;
        Utc.timestamp_opt(timestamp as i64, 0)
            .single()
            .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
    });
    let cgroup = fs::read_to_string(process_dir.join("cgroup")).unwrap_or_default();
    Some(RawProcess {
        identity: ProcessIdentity {
            process_id,
            pid,
            start_time_ticks: parsed.start_time_ticks,
            name: parsed.name,
        },
        parent_pid: parsed.parent_pid,
        thread_count: parsed.thread_count,
        user_name,
        state: parsed.state,
        total_cpu_ticks: parsed.user_ticks.saturating_add(parsed.system_ticks),
        resident_memory_bytes: parsed.resident_pages.saturating_mul(page_size),
        started_at,
        cgroup,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedProcStat {
    name: String,
    state: ProcessState,
    parent_pid: u32,
    user_ticks: u64,
    system_ticks: u64,
    thread_count: u64,
    start_time_ticks: u64,
    resident_pages: u64,
}

fn parse_proc_stat(value: &str) -> Option<ParsedProcStat> {
    let open = value.find('(')?;
    let close = value.rfind(')')?;
    if close <= open {
        return None;
    }
    let name = value.get(open + 1..close)?.to_string();
    let fields = value
        .get(close + 1..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields.len() < 22 {
        return None;
    }
    Some(ParsedProcStat {
        name,
        state: parse_process_state(fields[0]),
        parent_pid: fields[1].parse().ok()?,
        user_ticks: fields[11].parse().ok()?,
        system_ticks: fields[12].parse().ok()?,
        thread_count: fields[17].parse().ok()?,
        start_time_ticks: fields[19].parse().ok()?,
        resident_pages: fields[21].parse::<i64>().ok()?.max(0) as u64,
    })
}

fn parse_process_state(value: &str) -> ProcessState {
    match value.chars().next() {
        Some('R') => ProcessState::Running,
        Some('S') => ProcessState::Sleeping,
        Some('T' | 't') => ProcessState::Stopped,
        Some('I') => ProcessState::Idle,
        Some('D') => ProcessState::Uninterruptible,
        Some('Z') => ProcessState::Zombie,
        Some('X' | 'x') => ProcessState::Dead,
        _ => ProcessState::Unknown,
    }
}

fn classify_process_management(
    pid: u32,
    name: &str,
    cgroup: &str,
    container_facts: &HashMap<String, ProcessManagement>,
) -> ProcessManagement {
    if pid == 1 || name.starts_with("vust") {
        return ProcessManagement {
            kind: ProcessManagementKind::System,
            owner_name: Some(if pid == 1 { "system" } else { "VUST" }.to_string()),
            manage_via: Some("system".to_string()),
        };
    }
    if let Some((_, management)) = container_facts
        .iter()
        .find(|(container_id, _)| cgroup.contains(container_id.as_str()))
    {
        return management.clone();
    }
    if cgroup.contains("docker") || cgroup.contains("containerd") || cgroup.contains("kubepods") {
        return ProcessManagement {
            kind: ProcessManagementKind::Custom,
            owner_name: None,
            manage_via: Some("docker".to_string()),
        };
    }
    ProcessManagement {
        kind: ProcessManagementKind::Custom,
        owner_name: None,
        manage_via: Some("processManager".to_string()),
    }
}

async fn container_management_facts(
    state: &AppState,
) -> (HashMap<String, ProcessManagement>, bool) {
    let Some(docker) = state.docker.read().await.clone() else {
        return (HashMap::new(), false);
    };
    let options = ListContainersOptionsBuilder::new().all(true).build();
    let containers: Vec<ContainerSummary> = match docker.list_containers(Some(options)).await {
        Ok(containers) => containers,
        Err(error) => {
            warn!(error = %error, "Container ownership sampling failed");
            return (HashMap::new(), false);
        }
    };
    let project_facts = match compose_project_management_facts(&state.metadata_db).await {
        Ok(facts) => Some(facts),
        Err(error) => {
            warn!(error = %error, "Compose ownership facts could not be loaded");
            None
        }
    };
    let mut facts = HashMap::new();
    for container in containers {
        let labels = container.labels.unwrap_or_default();
        let names = container.names.unwrap_or_default();
        let owner_name = names
            .first()
            .map(|name| name.trim_start_matches('/').to_string())
            .filter(|name| !name.is_empty());
        let suite_managed = labels.get("vust.owner").map(String::as_str) == Some("suite")
            || labels.contains_key("vust.suite_id")
            || labels.contains_key("vust.workload_id");
        let management = if let Some(project) = labels.get("com.docker.compose.project") {
            project_facts
                .as_ref()
                .and_then(|facts| facts.get(project))
                .cloned()
                .unwrap_or_else(|| ProcessManagement {
                    kind: if suite_managed {
                        ProcessManagementKind::Suite
                    } else {
                        ProcessManagementKind::Compose
                    },
                    owner_name: Some(project.clone()),
                    manage_via: Some(if suite_managed {
                        "suiteCenter".to_string()
                    } else {
                        "dockerProjects".to_string()
                    }),
                })
        } else if suite_managed {
            ProcessManagement {
                kind: ProcessManagementKind::Suite,
                owner_name: labels
                    .get("vust.suite_name")
                    .or_else(|| labels.get("vust.suite_id"))
                    .or_else(|| labels.get("vust.suite_instance_id"))
                    .cloned()
                    .or(owner_name),
                manage_via: Some("suiteCenter".to_string()),
            }
        } else {
            ProcessManagement {
                kind: ProcessManagementKind::Custom,
                owner_name,
                manage_via: Some("docker".to_string()),
            }
        };
        if let Some(container_id) = container.id.filter(|value| !value.is_empty()) {
            facts.insert(container_id, management);
        }
    }
    (facts, project_facts.is_some())
}

async fn compose_project_management_facts(
    pool: &DbPool,
) -> Result<HashMap<String, ProcessManagement>, sqlx::Error> {
    use sqlx::Row;
    let rows = sqlx::query("SELECT name, management_kind, owner_name FROM docker_compose_projects")
        .fetch_all(pool)
        .await?;
    let mut facts = HashMap::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let management_kind: String = row.try_get("management_kind")?;
        let owner_name: Option<String> = row.try_get("owner_name")?;
        let (kind, manage_via) = match management_kind.as_str() {
            "suite" => (ProcessManagementKind::Suite, "suiteCenter"),
            "system" => (ProcessManagementKind::System, "system"),
            _ => (ProcessManagementKind::Compose, "dockerProjects"),
        };
        facts.insert(
            name.clone(),
            ProcessManagement {
                kind,
                owner_name: owner_name.or(Some(name)),
                manage_via: Some(manage_via.to_string()),
            },
        );
    }
    Ok(facts)
}

fn process_id(boot_id: &str, pid: u32, start_time_ticks: u64) -> String {
    let input = format!("{boot_id}\0{pid}\0{start_time_ticks}");
    hex::encode(digest(&SHA256, input.as_bytes()).as_ref())
}

fn read_boot_time(proc_root: &Path) -> Option<i64> {
    fs::read_to_string(proc_root.join("stat"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

fn read_memory_total_bytes(proc_root: &Path) -> Option<u64> {
    let kib = fs::read_to_string(proc_root.join("meminfo"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    Some(kib.saturating_mul(1024))
}

fn read_passwd_users(path: &Path) -> HashMap<u32, String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let fields = line.split(':').collect::<Vec<_>>();
            Some((
                fields.get(2)?.parse::<u32>().ok()?,
                fields.first()?.to_string(),
            ))
        })
        .collect()
}

fn collect_network_snapshot(
    proc_root: &Path,
    identities: &HashMap<u32, ProcessIdentity>,
) -> Result<NetworkSnapshot, String> {
    let (owners_by_inode, owner_scan_failures) = collect_socket_owners(proc_root, identities);
    let sources = [
        (NetworkProtocol::Tcp, "net/tcp", false),
        (NetworkProtocol::Tcp6, "net/tcp6", true),
        (NetworkProtocol::Udp, "net/udp", false),
        (NetworkProtocol::Udp6, "net/udp6", true),
    ];
    let mut parsed_connections = Vec::new();
    let mut warnings = Vec::new();
    let mut failed_count = 0_usize;
    let mut scanned_count = 0_usize;
    for (protocol, relative_path, ipv6) in sources {
        match parse_proc_network_file(
            protocol,
            &proc_root.join(relative_path),
            ipv6,
            &owners_by_inode,
        ) {
            Ok((mut source_entries, scanned, failed)) => {
                parsed_connections.append(&mut source_entries);
                scanned_count += scanned;
                failed_count += failed;
            }
            Err(()) => {
                failed_count += 1;
                warnings.push(format!(
                    "{} connection source was unavailable",
                    network_protocol_key(protocol)
                ));
            }
        }
    }
    parsed_connections
        .sort_by(|left, right| left.summary.connection_id.cmp(&right.summary.connection_id));
    parsed_connections
        .dedup_by(|left, right| left.summary.connection_id == right.summary.connection_id);
    let owner_expected_count = parsed_connections
        .iter()
        .filter(|entry| entry.owner_attribution_expected)
        .count();
    let owned_count = parsed_connections
        .iter()
        .filter(|entry| entry.owner_attribution_expected && !entry.summary.owners.is_empty())
        .count();
    let owner_coverage_percent = if owner_expected_count == 0 {
        Some(100.0)
    } else {
        Some(owned_count as f64 / owner_expected_count as f64 * 100.0)
    };
    if owned_count < owner_expected_count {
        warnings.push("some network connections could not be attributed to a process".to_string());
    }
    if owner_scan_failures > 0 {
        warnings.push("some process socket descriptors were unreadable".to_string());
    }
    let mut counts_by_process = HashMap::new();
    for entry in &parsed_connections {
        for owner in &entry.summary.owners {
            *counts_by_process
                .entry(owner.process_id.clone())
                .or_insert(0) += 1;
        }
    }
    let entries = parsed_connections
        .into_iter()
        .map(|entry| entry.summary)
        .collect::<Vec<_>>();
    let succeeded_count = entries.len();
    Ok(NetworkSnapshot {
        entries,
        counts_by_process,
        sampled_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        coverage: SamplingCoverage {
            status: if failed_count > 0 {
                SamplingStatus::Partial
            } else {
                SamplingStatus::Complete
            },
            scanned_count,
            succeeded_count,
            failed_count,
            owner_coverage_percent,
            warnings,
        },
    })
}

fn collect_socket_owners(
    proc_root: &Path,
    identities: &HashMap<u32, ProcessIdentity>,
) -> (HashMap<u64, Vec<NetworkConnectionOwner>>, usize) {
    let mut owners = HashMap::<u64, Vec<NetworkConnectionOwner>>::new();
    let mut failures = 0_usize;
    for identity in identities.values() {
        let process_dir = proc_root.join(identity.pid.to_string());
        let fd_entries = match fs::read_dir(process_dir.join("fd")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                failures += 1;
                continue;
            }
        };
        let mut seen = HashSet::new();
        for fd in fd_entries.flatten() {
            let Some(inode) = fs::read_link(fd.path())
                .ok()
                .and_then(|target| extract_socket_inode(&target.to_string_lossy()))
            else {
                continue;
            };
            if !seen.insert(inode) {
                continue;
            }
            owners
                .entry(inode)
                .or_default()
                .push(NetworkConnectionOwner {
                    process_id: identity.process_id.clone(),
                    pid: identity.pid,
                    process_name: identity.name.clone(),
                });
        }
    }
    for values in owners.values_mut() {
        values.sort_by_key(|owner| owner.pid);
    }
    (owners, failures)
}

fn extract_socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

fn parse_proc_network_file(
    protocol: NetworkProtocol,
    path: &Path,
    ipv6: bool,
    owners_by_inode: &HashMap<u64, Vec<NetworkConnectionOwner>>,
) -> Result<(Vec<ParsedNetworkConnection>, usize, usize), ()> {
    let content = fs::read_to_string(path).map_err(|_| ())?;
    let mut entries = Vec::new();
    let mut scanned = 0_usize;
    let mut failed = 0_usize;
    for line in content.lines().skip(1) {
        scanned += 1;
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 10 {
            failed += 1;
            continue;
        }
        let Some(local_endpoint) = parse_endpoint(fields[1], ipv6) else {
            failed += 1;
            continue;
        };
        let remote_endpoint = parse_endpoint(fields[2], ipv6).filter(|endpoint| {
            endpoint.port != 0
                || !endpoint
                    .address
                    .chars()
                    .all(|value| value == '0' || value == ':' || value == '.')
        });
        let state = if matches!(protocol, NetworkProtocol::Tcp | NetworkProtocol::Tcp6) {
            parse_tcp_state(fields[3])
        } else {
            NetworkConnectionState::Unconnected
        };
        let Some(inode) = fields[9].parse::<u64>().ok() else {
            failed += 1;
            continue;
        };
        let owner_attribution_expected = inode != 0;
        let owners = if owner_attribution_expected {
            owners_by_inode.get(&inode).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        let connection_key = format!(
            "{}\0{}\0{}\0{}",
            network_protocol_key(protocol),
            inode,
            format_endpoint(&local_endpoint),
            remote_endpoint
                .as_ref()
                .map(format_endpoint)
                .unwrap_or_default()
        );
        entries.push(ParsedNetworkConnection {
            summary: NetworkConnectionSummary {
                connection_id: hex::encode(digest(&SHA256, connection_key.as_bytes()).as_ref()),
                protocol,
                local_endpoint,
                remote_endpoint,
                state,
                owners,
            },
            owner_attribution_expected,
        });
    }
    Ok((entries, scanned, failed))
}

fn parse_endpoint(value: &str, ipv6: bool) -> Option<NetworkEndpoint> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let (address, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let address = if ipv6 {
        if address.len() != 32 {
            return None;
        }
        let mut bytes = [0_u8; 16];
        for (index, chunk) in address.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
        }
        for chunk in bytes.chunks_exact_mut(4) {
            chunk.reverse();
        }
        Ipv6Addr::from(bytes).to_string()
    } else {
        let raw = u32::from_str_radix(address, 16).ok()?;
        Ipv4Addr::from(raw.to_le_bytes()).to_string()
    };
    Some(NetworkEndpoint { address, port })
}

fn parse_tcp_state(value: &str) -> NetworkConnectionState {
    match value {
        "01" => NetworkConnectionState::Established,
        "02" => NetworkConnectionState::SynSent,
        "03" => NetworkConnectionState::SynReceived,
        "04" => NetworkConnectionState::FinWait1,
        "05" => NetworkConnectionState::FinWait2,
        "06" => NetworkConnectionState::TimeWait,
        "07" => NetworkConnectionState::Closed,
        "08" => NetworkConnectionState::CloseWait,
        "09" => NetworkConnectionState::LastAck,
        "0A" => NetworkConnectionState::Listen,
        "0B" => NetworkConnectionState::Closing,
        _ => NetworkConnectionState::Unknown,
    }
}

fn clock_ticks_per_second() -> f64 {
    let value = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if value > 0 { value as f64 } else { 100.0 }
}

fn page_size_bytes() -> u64 {
    let value = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if value > 0 { value as u64 } else { 4096 }
}

fn validate_page(page: Option<usize>, page_size: Option<usize>) -> ApiResult<(usize, usize)> {
    let page = page.unwrap_or(1);
    let page_size = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
    if page == 0 || page_size == 0 || page_size > MAX_PAGE_SIZE {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "invalid pagination parameters",
        ));
    }
    Ok((page, page_size))
}

fn process_sampler_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ProcessSamplerUnavailable,
        "process sampler is not ready",
    )
}

fn network_sampler_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::NetworkSamplerUnavailable,
        "network sampler is not ready",
    )
}

fn sort_processes(entries: &mut [ProcessSummary], sort_by: ProcessSortBy, order: SortOrder) {
    entries.sort_by(|left, right| {
        let ordering = match sort_by {
            ProcessSortBy::Pid => apply_sort_order(left.pid.cmp(&right.pid), order),
            ProcessSortBy::Name => apply_sort_order(
                left.name.to_lowercase().cmp(&right.name.to_lowercase()),
                order,
            ),
            ProcessSortBy::CpuPercent => {
                compare_optional(left.cpu_percent, right.cpu_percent, order, |left, right| {
                    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
                })
            }
            ProcessSortBy::MemoryPercent => compare_optional(
                left.memory_percent,
                right.memory_percent,
                order,
                |left, right| left.partial_cmp(&right).unwrap_or(Ordering::Equal),
            ),
            ProcessSortBy::ConnectionCount => compare_optional(
                left.connection_count,
                right.connection_count,
                order,
                |left, right| left.cmp(&right),
            ),
            ProcessSortBy::StartedAt => compare_optional(
                left.started_at.as_ref(),
                right.started_at.as_ref(),
                order,
                Ord::cmp,
            ),
        };
        ordering.then_with(|| left.pid.cmp(&right.pid))
    });
}

fn compare_optional<T>(
    left: Option<T>,
    right: Option<T>,
    order: SortOrder,
    compare: impl FnOnce(T, T) -> Ordering,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => apply_sort_order(compare(left, right), order),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn apply_sort_order(ordering: Ordering, order: SortOrder) -> Ordering {
    if order == SortOrder::Desc {
        ordering.reverse()
    } else {
        ordering
    }
}

fn process_state_key(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Running => "running",
        ProcessState::Sleeping => "sleeping",
        ProcessState::Stopped => "stopped",
        ProcessState::Idle => "idle",
        ProcessState::Uninterruptible => "uninterruptible",
        ProcessState::Zombie => "zombie",
        ProcessState::Dead => "dead",
        ProcessState::Unknown => "unknown",
    }
}

fn sort_network_connections(
    entries: &mut [NetworkConnectionSummary],
    sort_by: NetworkSortBy,
    order: SortOrder,
) {
    entries.sort_by(|left, right| {
        let ordering = match sort_by {
            NetworkSortBy::Protocol => left.protocol.cmp(&right.protocol),
            NetworkSortBy::LocalEndpoint => {
                format_endpoint(&left.local_endpoint).cmp(&format_endpoint(&right.local_endpoint))
            }
            NetworkSortBy::RemoteEndpoint => left
                .remote_endpoint
                .as_ref()
                .map(format_endpoint)
                .cmp(&right.remote_endpoint.as_ref().map(format_endpoint)),
            NetworkSortBy::State => left.state.cmp(&right.state),
            NetworkSortBy::ProcessName => left
                .owners
                .first()
                .map(|owner| owner.process_name.to_lowercase())
                .cmp(
                    &right
                        .owners
                        .first()
                        .map(|owner| owner.process_name.to_lowercase()),
                ),
        };
        let ordering = if order == SortOrder::Desc {
            ordering.reverse()
        } else {
            ordering
        };
        ordering.then_with(|| left.connection_id.cmp(&right.connection_id))
    });
}

fn format_endpoint(endpoint: &NetworkEndpoint) -> String {
    if endpoint.address.contains(':') {
        format!("[{}]:{}", endpoint.address, endpoint.port)
    } else {
        format!("{}:{}", endpoint.address, endpoint.port)
    }
}

fn network_protocol_key(protocol: NetworkProtocol) -> &'static str {
    match protocol {
        NetworkProtocol::Tcp => "tcp",
        NetworkProtocol::Tcp6 => "tcp6",
        NetworkProtocol::Udp => "udp",
        NetworkProtocol::Udp6 => "udp6",
    }
}

fn network_state_key(state: NetworkConnectionState) -> &'static str {
    match state {
        NetworkConnectionState::Established => "established",
        NetworkConnectionState::SynSent => "synSent",
        NetworkConnectionState::SynReceived => "synReceived",
        NetworkConnectionState::FinWait1 => "finWait1",
        NetworkConnectionState::FinWait2 => "finWait2",
        NetworkConnectionState::TimeWait => "timeWait",
        NetworkConnectionState::Closed => "closed",
        NetworkConnectionState::CloseWait => "closeWait",
        NetworkConnectionState::LastAck => "lastAck",
        NetworkConnectionState::Listen => "listen",
        NetworkConnectionState::Closing => "closing",
        NetworkConnectionState::Unconnected => "unconnected",
        NetworkConnectionState::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestProcRoot {
        path: std::path::PathBuf,
    }

    impl TestProcRoot {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("vust-process-test-{}", uuid::Uuid::now_v7()));
            fs::create_dir_all(path.join("net")).expect("test proc network directory should exist");
            for source in ["tcp", "tcp6", "udp", "udp6"] {
                fs::write(path.join("net").join(source), "header\n")
                    .expect("empty network source should be written");
            }
            Self { path }
        }

        fn write_source(&self, source: &str, rows: &str) {
            fs::write(
                self.path.join("net").join(source),
                format!("header\n{rows}"),
            )
            .expect("network source should be written");
        }
    }

    impl Drop for TestProcRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn proc_network_row(state: &str, inode: &str) -> String {
        format!("0: 0100007F:1F90 00000000:0000 {state} 0 0 0 0 0 {inode}\n")
    }

    #[test]
    fn proc_stat_parser_preserves_names_with_spaces() {
        let value =
            "123 (worker with spaces) R 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 4 18 999 20 5";
        let parsed = parse_proc_stat(value).expect("stat should parse");
        assert_eq!(parsed.name, "worker with spaces");
        assert_eq!(parsed.parent_pid, 1);
        assert_eq!(parsed.user_ticks, 11);
        assert_eq!(parsed.system_ticks, 12);
        assert_eq!(parsed.thread_count, 4);
        assert_eq!(parsed.start_time_ticks, 999);
        assert_eq!(parsed.resident_pages, 5);
    }

    #[test]
    fn process_state_does_not_treat_modifiers_as_primary_state() {
        assert_eq!(parse_process_state("R"), ProcessState::Running);
        assert_eq!(parse_process_state("D"), ProcessState::Uninterruptible);
        assert_eq!(parse_process_state("?"), ProcessState::Unknown);
    }

    #[test]
    fn process_id_changes_when_pid_is_reused() {
        let first = process_id("boot", 10, 100);
        let reused = process_id("boot", 10, 200);
        assert_ne!(first, reused);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn pagination_rejects_unbounded_requests() {
        assert!(validate_page(Some(0), Some(100)).is_err());
        assert!(validate_page(Some(1), Some(501)).is_err());
        assert_eq!(validate_page(None, None).expect("defaults"), (1, 100));
    }

    #[test]
    fn process_sort_keeps_missing_cpu_values_last_in_both_directions() {
        let summary = |pid: u32, cpu_percent: Option<f64>| ProcessSummary {
            process_id: pid.to_string(),
            pid,
            name: format!("process-{pid}"),
            parent_pid: 1,
            thread_count: 1,
            user_name: "root".to_string(),
            state: ProcessState::Sleeping,
            cpu_percent,
            memory_percent: None,
            resident_memory_bytes: None,
            connection_count: None,
            started_at: None,
            management: ProcessManagement {
                kind: ProcessManagementKind::Custom,
                owner_name: None,
                manage_via: None,
            },
            capabilities: ProcessCapabilities {
                can_terminate: true,
                can_force_kill: true,
            },
        };
        let mut descending = vec![
            summary(1, None),
            summary(2, Some(1.0)),
            summary(3, Some(2.0)),
        ];
        sort_processes(&mut descending, ProcessSortBy::CpuPercent, SortOrder::Desc);
        assert_eq!(
            descending.iter().map(|entry| entry.pid).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );

        let mut ascending = descending;
        sort_processes(&mut ascending, ProcessSortBy::CpuPercent, SortOrder::Asc);
        assert_eq!(
            ascending.iter().map(|entry| entry.pid).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
    }

    #[test]
    fn proc_endpoints_decode_ipv4_and_ipv6() {
        assert_eq!(
            parse_endpoint("0100007F:1F90", false),
            Some(NetworkEndpoint {
                address: "127.0.0.1".to_string(),
                port: 8080,
            })
        );
        assert_eq!(
            parse_endpoint("00000000000000000000000001000000:01BB", true),
            Some(NetworkEndpoint {
                address: "::1".to_string(),
                port: 443,
            })
        );
    }

    #[test]
    fn time_wait_without_inode_does_not_reduce_network_coverage() {
        let proc_root = TestProcRoot::new();
        proc_root.write_source("tcp", &proc_network_row("06", "0"));

        let snapshot = collect_network_snapshot(&proc_root.path, &HashMap::new())
            .expect("network snapshot should be collected");

        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].state, NetworkConnectionState::TimeWait);
        assert!(snapshot.entries[0].owners.is_empty());
        assert_eq!(snapshot.coverage.status, SamplingStatus::Complete);
        assert_eq!(snapshot.coverage.failed_count, 0);
        assert_eq!(snapshot.coverage.owner_coverage_percent, Some(100.0));
    }

    #[test]
    fn unattributed_live_socket_keeps_data_complete_but_reduces_owner_coverage() {
        let proc_root = TestProcRoot::new();
        proc_root.write_source("tcp", &proc_network_row("01", "42"));

        let snapshot = collect_network_snapshot(&proc_root.path, &HashMap::new())
            .expect("network snapshot should be collected");

        assert_eq!(snapshot.coverage.status, SamplingStatus::Complete);
        assert_eq!(snapshot.coverage.failed_count, 0);
        assert_eq!(snapshot.coverage.owner_coverage_percent, Some(0.0));
        assert!(
            snapshot
                .coverage
                .warnings
                .iter()
                .any(|warning| warning.contains("could not be attributed"))
        );
    }

    #[test]
    fn missing_or_malformed_network_source_marks_snapshot_partial() {
        let proc_root = TestProcRoot::new();
        proc_root.write_source("tcp", &proc_network_row("01", "invalid"));
        fs::remove_file(proc_root.path.join("net/udp6")).expect("test source should be removable");

        let snapshot = collect_network_snapshot(&proc_root.path, &HashMap::new())
            .expect("partial network snapshot should still be returned");

        assert_eq!(snapshot.coverage.status, SamplingStatus::Partial);
        assert_eq!(snapshot.coverage.scanned_count, 1);
        assert_eq!(snapshot.coverage.failed_count, 2);
    }

    #[test]
    fn exited_process_during_owner_scan_is_not_a_failure() {
        let proc_root = TestProcRoot::new();
        let identity = ProcessIdentity {
            process_id: "a".repeat(64),
            pid: 424_242,
            start_time_ticks: 1,
            name: "exited".to_string(),
        };
        let (_, failures) =
            collect_socket_owners(&proc_root.path, &HashMap::from([(identity.pid, identity)]));

        assert_eq!(failures, 0);
    }

    #[test]
    fn process_management_facts_do_not_reduce_capabilities() {
        let mut facts = HashMap::new();
        facts.insert(
            "container-id".to_string(),
            ProcessManagement {
                kind: ProcessManagementKind::Suite,
                owner_name: Some("suite-a".to_string()),
                manage_via: Some("suiteCenter".to_string()),
            },
        );
        let management =
            classify_process_management(42, "worker", "0::/docker/container-id", &facts);
        assert_eq!(management.kind, ProcessManagementKind::Suite);
        assert_eq!(
            classify_process_management(1, "init", "/", &HashMap::new()).kind,
            ProcessManagementKind::System
        );
    }

    #[test]
    fn pidfd_errors_map_to_stable_process_codes() {
        let unavailable = map_pidfd_error(std::io::Error::from_raw_os_error(libc::ENOSYS), true);
        assert_eq!(unavailable.code, ErrorCode::ProcessSignalUnavailable);
        assert_eq!(unavailable.status, StatusCode::SERVICE_UNAVAILABLE);
        let denied = map_pidfd_error(std::io::Error::from_raw_os_error(libc::EPERM), false);
        assert_eq!(denied.code, ErrorCode::ProcessPermissionDenied);
        assert_eq!(denied.status, StatusCode::FORBIDDEN);
        let changed = map_pidfd_error(std::io::Error::from_raw_os_error(libc::ESRCH), false);
        assert_eq!(changed.code, ErrorCode::ProcessChanged);
    }

    #[test]
    fn idempotency_key_cannot_be_rebound_to_another_process() {
        let stored = StoredSignalOperation {
            result: ProcessSignalResult {
                idempotency_key: uuid::Uuid::now_v7().to_string(),
                process_id: "a".repeat(64),
                pid: Some(42),
                process_name: Some("worker".to_string()),
                signal: ProcessSignal::Term,
                status: ProcessSignalDeliveryStatus::Delivered,
                delivered_at: None,
                error_summary: None,
            },
            error_code: None,
        };
        let error = stored_signal_result(stored, &"b".repeat(64), ProcessSignal::Term)
            .expect_err("rebind must fail");
        assert_eq!(error.code, ErrorCode::ProcessOperationConflict);
    }
}
