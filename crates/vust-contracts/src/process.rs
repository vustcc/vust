//! 进程与网络观察领域共享契约。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// 采样结果完整性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SamplingStatus {
    Complete,
    Partial,
}

/// 单次采样覆盖率与安全告警。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct SamplingCoverage {
    #[ts(inline)]
    pub status: SamplingStatus,
    pub scanned_count: usize,
    pub succeeded_count: usize,
    pub failed_count: usize,
    pub owner_coverage_percent: Option<f64>,
    pub warnings: Vec<String>,
}

/// 进程的内核主状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProcessState {
    Running,
    Sleeping,
    Stopped,
    Idle,
    Uninterruptible,
    Zombie,
    Dead,
    Unknown,
}

/// 进程所属的管理领域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProcessManagementKind {
    Custom,
    Compose,
    Suite,
    System,
}

/// 进程归属事实。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ProcessManagement {
    #[ts(inline)]
    pub kind: ProcessManagementKind,
    pub owner_name: Option<String>,
    pub manage_via: Option<String>,
}

/// 进程管理能力。归属只作标注，root 管理策略不按归属收紧能力。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProcessCapabilities {
    pub can_terminate: bool,
    pub can_force_kill: bool,
}

/// 进程列表安全摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ProcessSummary {
    pub process_id: String,
    pub pid: u32,
    pub name: String,
    pub parent_pid: u32,
    pub thread_count: u64,
    pub user_name: String,
    #[ts(inline)]
    pub state: ProcessState,
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub resident_memory_bytes: Option<u64>,
    pub connection_count: Option<u64>,
    pub started_at: Option<String>,
    #[ts(inline)]
    pub management: ProcessManagement,
    #[ts(inline)]
    pub capabilities: ProcessCapabilities,
}

/// 进程列表查询参数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListQuery {
    pub query: Option<String>,
    pub status: Option<ProcessState>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub sort_by: Option<ProcessSortBy>,
    pub sort_order: Option<SortOrder>,
}

/// 进程排序字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSortBy {
    Pid,
    Name,
    CpuPercent,
    MemoryPercent,
    ConnectionCount,
    StartedAt,
}

/// 通用排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    Asc,
    Desc,
}

/// 进程分页响应。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "process/")]
pub struct ProcessListPage {
    #[ts(inline)]
    pub entries: Vec<ProcessSummary>,
    pub page: usize,
    pub page_size: usize,
    pub available_total: usize,
    pub total: usize,
    pub counts: BTreeMap<String, usize>,
    pub sampled_at: String,
    #[ts(inline)]
    pub coverage: SamplingCoverage,
}

/// 主机网络协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NetworkProtocol {
    Tcp,
    Tcp6,
    Udp,
    Udp6,
}

/// 主机网络连接状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NetworkConnectionState {
    Established,
    SynSent,
    SynReceived,
    FinWait1,
    FinWait2,
    TimeWait,
    Closed,
    CloseWait,
    LastAck,
    Listen,
    Closing,
    Unconnected,
    Unknown,
}

/// 网络端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEndpoint {
    pub address: String,
    pub port: u16,
}

/// 网络连接关联进程。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConnectionOwner {
    pub process_id: String,
    pub pid: u32,
    pub process_name: String,
}

/// 主机网络连接摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct NetworkConnectionSummary {
    pub connection_id: String,
    #[ts(inline)]
    pub protocol: NetworkProtocol,
    #[ts(inline)]
    pub local_endpoint: NetworkEndpoint,
    #[ts(inline)]
    pub remote_endpoint: Option<NetworkEndpoint>,
    #[ts(inline)]
    pub state: NetworkConnectionState,
    #[ts(inline)]
    pub owners: Vec<NetworkConnectionOwner>,
}

/// 网络连接列表查询参数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConnectionListQuery {
    pub query: Option<String>,
    pub state: Option<NetworkConnectionState>,
    pub protocol: Option<NetworkProtocol>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub sort_by: Option<NetworkSortBy>,
    pub sort_order: Option<SortOrder>,
}

/// 网络连接排序字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkSortBy {
    Protocol,
    LocalEndpoint,
    RemoteEndpoint,
    State,
    ProcessName,
}

/// 网络连接分页响应。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "process/")]
pub struct NetworkConnectionListPage {
    #[ts(inline)]
    pub entries: Vec<NetworkConnectionSummary>,
    pub page: usize,
    pub page_size: usize,
    pub available_total: usize,
    pub total: usize,
    pub by_state: BTreeMap<String, usize>,
    pub by_protocol: BTreeMap<String, usize>,
    pub sampled_at: String,
    #[ts(inline)]
    pub coverage: SamplingCoverage,
}

/// 进程动作请求。force-kill 要求 confirmationToken，terminate 必须省略。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "process/", optional_fields)]
pub struct ProcessActionRequest {
    pub idempotency_key: String,
    pub confirmation_token: Option<String>,
}

/// 强制终止二次确认令牌。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "process/")]
pub struct ProcessForceKillConfirmation {
    pub confirmation_token: String,
    pub expires_at: String,
}

/// 进程信号类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSignal {
    Term,
    Kill,
}

/// 进程信号投递状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProcessSignalDeliveryStatus {
    Delivered,
    Failed,
    OutcomeUnknown,
}

/// 进程信号结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "process/", optional_fields)]
pub struct ProcessSignalResult {
    pub idempotency_key: String,
    pub process_id: String,
    pub pid: Option<u32>,
    pub process_name: Option<String>,
    #[ts(inline)]
    pub signal: ProcessSignal,
    #[ts(inline)]
    pub status: ProcessSignalDeliveryStatus,
    pub delivered_at: Option<String>,
    pub error_summary: Option<String>,
}
