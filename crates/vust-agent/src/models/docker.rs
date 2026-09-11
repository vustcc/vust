//! Docker 客户端模型：与 Docker daemon 通信封装。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Docker 容器生命周期动作。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerContainerAction {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Kill,
    Remove,
}

/// 批量执行容器生命周期动作的请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerBatchActionRequest {
    pub ids: Vec<String>,
    pub action: DockerContainerAction,
}

/// 单个容器的批量动作执行结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerBatchActionItem {
    pub id: String,
    pub name: String,
    pub success: bool,
    pub state: Option<DockerContainerState>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

/// Docker 容器批量动作结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerBatchActionResult {
    pub items: Vec<DockerContainerBatchActionItem>,
}

/// Docker 容器状态分布。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStateCounts {
    pub total: usize,
    pub running: usize,
    pub paused: usize,
    pub restarting: usize,
    pub exited: usize,
    pub other: usize,
}

/// Docker 容器在容器模块中的归属类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerContainerManagementKind {
    Suite,
    Compose,
    Custom,
}

/// Docker 容器的管理归属。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerManagement {
    pub kind: DockerContainerManagementKind,
    pub owner_name: Option<String>,
}

/// 供前端稳定消费的 Docker 容器状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Stopping,
    Exited,
    Removing,
    Dead,
    Unknown,
}

/// 容器模块允许执行的状态相关操作。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerCapabilities {
    pub can_start: bool,
    pub can_stop: bool,
    pub can_restart: bool,
    pub can_pause: bool,
    pub can_unpause: bool,
    pub can_kill: bool,
    pub can_remove: bool,
    pub can_exec: bool,
}

/// 容器端口映射摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerPort {
    pub host_ip: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: u16,
    pub protocol: String,
}

/// 容器健康检查摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerHealth {
    pub status: String,
    pub failing_streak: i64,
}

/// Docker 容器列表使用的稳定领域摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerSummary {
    pub id: String,
    pub name: String,
    pub image_ref: String,
    pub image_id: String,
    pub command: String,
    pub created_at: i64,
    pub state: DockerContainerState,
    pub status_text: String,
    pub health: Option<DockerContainerHealth>,
    pub ports: Vec<DockerContainerPort>,
    pub management: DockerContainerManagement,
    pub capabilities: DockerContainerCapabilities,
}

/// 容器环境变量键值。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerEnvironmentVariable {
    pub name: String,
    pub value: String,
}

/// 容器挂载详情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerMount {
    pub kind: String,
    pub name: Option<String>,
    pub source: String,
    pub target: String,
    pub read_only: bool,
    pub mode: Option<String>,
}

/// 容器网络端点详情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerNetworkEndpoint {
    pub id: String,
    pub name: String,
    pub endpoint_id: String,
    pub mac_address: String,
    pub ipv4_address: String,
    pub ipv6_address: String,
    pub gateway: String,
    pub aliases: Vec<String>,
}

/// 容器重启策略详情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerRestartPolicy {
    pub name: String,
    pub maximum_retry_count: i64,
}

/// Docker 容器详情使用的稳定领域模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerDetail {
    pub summary: DockerContainerSummary,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub error_message: Option<String>,
    pub oom_killed: bool,
    pub restart_count: i64,
    pub restart_policy: DockerContainerRestartPolicy,
    pub entrypoint: Vec<String>,
    pub command: Vec<String>,
    pub environment: Vec<DockerContainerEnvironmentVariable>,
    pub mounts: Vec<DockerContainerMount>,
    pub networks: Vec<DockerContainerNetworkEndpoint>,
    pub labels: HashMap<String, String>,
    pub log_driver: String,
}

/// Docker Compose 项目健康状态分布。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStateCounts {
    pub total: usize,
    pub healthy: usize,
    pub partial: usize,
    pub stopped: usize,
    pub unknown: usize,
}

/// Docker 镜像数量分布。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageCounts {
    pub total: usize,
    pub dangling: usize,
}

/// Docker 镜像列表使用的稳定领域摘要。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerImageSummary {
    pub id: String,
    pub tags: Vec<String>,
    pub digests: Vec<String>,
    pub created_at: i64,
    pub size_bytes: i64,
    pub container_count: i64,
    pub dangling: bool,
}

/// 精简的容器引用信息。
#[derive(Debug, Deserialize)]
pub struct ContainerRef {
    pub id: String,
    pub name: String,
}

/// 创建容器时支持的挂载类型。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerContainerCreateMountKind {
    Bind,
    Volume,
}

/// 创建容器时的结构化端口映射。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerCreatePort {
    pub host_ip: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: u16,
    pub protocol: String,
}

/// 创建容器时的结构化挂载配置。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerCreateMount {
    pub kind: DockerContainerCreateMountKind,
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
}

/// 创建容器时支持的重启策略。
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerContainerCreateRestartPolicy {
    #[default]
    No,
    Always,
    UnlessStopped,
    OnFailure,
}

/// 创建容器所需的结构化参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerCreateRequest {
    pub name: String,
    pub image_ref: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub environment: Vec<DockerContainerEnvironmentVariable>,
    #[serde(default)]
    pub ports: Vec<DockerContainerCreatePort>,
    #[serde(default)]
    pub mounts: Vec<DockerContainerCreateMount>,
    #[serde(default)]
    pub restart_policy: DockerContainerCreateRestartPolicy,
    pub maximum_retry_count: Option<i64>,
    pub network_id: Option<String>,
    #[serde(default)]
    pub auto_remove: bool,
    #[serde(default = "default_auto_start")]
    pub auto_start: bool,
}

/// 容器创建完成后的稳定结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerContainerCreateResult {
    pub id: String,
    pub name: String,
    pub started: bool,
    pub warnings: Vec<String>,
}

fn default_auto_start() -> bool {
    true
}

/// 容器重命名请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRenameRequest {
    pub name: String,
}

/// 容器内执行命令的请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerExecRequest {
    pub command: String,
}

/// 容器内执行命令的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerExecResult {
    pub exit_code: Option<i64>,
    pub output: String,
}

/// Compose 项目的运行状态。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectRuntimeState {
    Running,
    Partial,
    Stopped,
    Unknown,
}

/// Compose 项目的配置应用状态。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectConfigurationState {
    Applied,
    Pending,
    Missing,
}

/// Compose 项目的管理归属。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectManagementKind {
    Custom,
    Suite,
    System,
}

impl DockerProjectManagementKind {
    /// 返回数据库使用的稳定标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Suite => "suite",
            Self::System => "system",
        }
    }
}

/// Compose 项目的管理入口。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectManageVia {
    Projects,
    SuiteCenter,
    System,
}

/// Compose 项目的归属信息。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectManagement {
    pub kind: DockerProjectManagementKind,
    pub owner_name: Option<String>,
    pub read_only: bool,
    pub manage_via: DockerProjectManageVia,
}

/// Compose 项目允许执行的操作。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectCapabilities {
    pub can_start: bool,
    pub can_stop: bool,
    pub can_restart: bool,
    pub can_redeploy: bool,
    pub can_edit_configuration: bool,
    pub can_scale: bool,
    pub can_remove: bool,
}

/// Compose 项目容器状态分布。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectContainerStates {
    pub total: usize,
    pub running: usize,
    pub paused: usize,
    pub restarting: usize,
    pub exited: usize,
    pub other: usize,
}

/// Compose 项目列表项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectSummary {
    pub name: String,
    pub created_at: i64,
    pub runtime_state: DockerProjectRuntimeState,
    pub configuration_state: DockerProjectConfigurationState,
    pub service_count: usize,
    pub container_states: DockerProjectContainerStates,
    pub management: DockerProjectManagement,
    pub capabilities: DockerProjectCapabilities,
}

/// Compose 项目分页列表。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectPage {
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub items: Vec<DockerProjectSummary>,
}

/// Compose 服务中的容器摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectContainerSummary {
    pub id: String,
    pub name: String,
    pub state: String,
    pub ip_addresses: Vec<String>,
    pub published_ports: Vec<String>,
}

/// Compose 服务运行摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectServiceSummary {
    pub name: String,
    pub image: Option<String>,
    pub runtime_state: DockerProjectRuntimeState,
    pub container_states: DockerProjectContainerStates,
    pub containers: Vec<DockerProjectContainerSummary>,
}

/// Compose 项目详情。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectDetail {
    pub summary: DockerProjectSummary,
    pub services: Vec<DockerProjectServiceSummary>,
}

/// Compose 项目创建请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectCreateRequest {
    pub name: String,
    pub compose_yaml: String,
}

/// Compose 配置保存请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectConfigurationUpdateRequest {
    pub compose_yaml: String,
    pub expected_revision: i64,
}

/// Compose 配置响应。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectConfiguration {
    pub compose_yaml: String,
    pub revision: i64,
    pub applied_revision: Option<i64>,
    pub state: DockerProjectConfigurationState,
}

/// Compose 配置校验请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectConfigurationValidateRequest {
    pub compose_yaml: String,
}

/// Compose 配置校验响应。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectConfigurationValidateResponse {
    pub valid: bool,
    pub error: Option<String>,
}

/// Compose 项目重新部署请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectDeploymentRequest {
    #[serde(default)]
    pub pull_images: bool,
}

/// Compose 服务副本数修改请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectScaleRequest {
    pub replicas: usize,
}

/// Compose 项目任务操作。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectTaskOperation {
    Create,
    Start,
    Stop,
    Restart,
    Redeploy,
    Scale,
    Remove,
}

impl DockerProjectTaskOperation {
    /// 返回数据库使用的稳定标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Redeploy => "redeploy",
            Self::Scale => "scale",
            Self::Remove => "remove",
        }
    }
}

/// Compose 项目任务状态。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectTaskStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// Compose 项目任务阶段。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectTaskStage {
    Validating,
    Preparing,
    Pulling,
    Applying,
    Verifying,
    RollingBack,
    CleaningUp,
    Completed,
    Cancelled,
    Interrupted,
}

/// Compose 任务进度输出模式。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectProgressMode {
    Structured,
    Text,
    Unavailable,
}

/// Compose 任务进度所属阶段。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectProgressPhase {
    Pulling,
    Applying,
}

/// Compose 资源事件状态。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockerProjectProgressStatus {
    Working,
    Done,
    Warning,
    Error,
}

/// Compose 命令输出中的单个资源进度快照。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectTaskProgressItem {
    pub id: String,
    pub parent_id: Option<String>,
    pub phase: DockerProjectProgressPhase,
    pub status: DockerProjectProgressStatus,
    pub label: String,
    pub action: String,
    pub details: Option<String>,
    pub current_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub percent: Option<u8>,
}

/// Compose 项目任务通过事件流发送的单项进度更新。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectTaskProgressUpdate {
    pub progress_mode: DockerProjectProgressMode,
    pub progress_percent: u8,
    pub item: DockerProjectTaskProgressItem,
}

/// Compose 项目后台任务。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectTask {
    pub id: String,
    pub project_name: String,
    pub operation: DockerProjectTaskOperation,
    pub status: DockerProjectTaskStatus,
    pub stage: DockerProjectTaskStage,
    pub progress_percent: u8,
    pub progress_mode: DockerProjectProgressMode,
    pub progress_items: Vec<DockerProjectTaskProgressItem>,
    pub service_name: Option<String>,
    pub replicas: Option<usize>,
    pub pull_images: bool,
    pub error_summary: Option<String>,
    pub cleanup_warning: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// 创建网络的请求参数。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkIpamConfig {
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub ip_range: Option<String>,
}

/// 创建 Bridge 网络的请求参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkCreateRequest {
    pub name: String,
    #[serde(default)]
    pub internal: bool,
    #[serde(default)]
    pub enable_ipv6: bool,
    pub ipv4: Option<DockerNetworkIpamConfig>,
    pub ipv6: Option<DockerNetworkIpamConfig>,
    pub options: Option<HashMap<String, String>>,
    pub labels: Option<HashMap<String, String>>,
}

/// Docker 网络的管理归属。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DockerNetworkManagementKind {
    System,
    Compose,
    Suite,
    Custom,
}

/// Docker 网络的管理信息。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkManagement {
    pub kind: DockerNetworkManagementKind,
    pub owner_name: Option<String>,
}

/// Docker 网络允许执行的操作。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkCapabilities {
    pub can_remove: bool,
    pub can_manage_connections: bool,
}

/// Docker 网络列表项。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkSummary {
    pub id: String,
    pub name: String,
    pub created_at: Option<i64>,
    pub driver: String,
    pub scope: String,
    pub enable_ipv4: bool,
    pub enable_ipv6: bool,
    pub internal: bool,
    pub attachable: bool,
    pub ingress: bool,
    pub config_only: bool,
    pub subnets: Vec<String>,
    pub management: DockerNetworkManagement,
    pub capabilities: DockerNetworkCapabilities,
}

/// Docker 网络中连接的容器端点。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkContainer {
    pub id: String,
    pub name: String,
    pub endpoint_id: Option<String>,
    pub mac_address: Option<String>,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
}

/// Docker Overlay 网络的对等节点。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkPeer {
    pub name: Option<String>,
    pub ip: Option<String>,
}

/// Docker 网络详情。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkDetail {
    pub summary: DockerNetworkSummary,
    pub ipam_configs: Vec<DockerNetworkIpamConfig>,
    pub options: HashMap<String, String>,
    pub labels: HashMap<String, String>,
    pub containers: Vec<DockerNetworkContainer>,
    pub peers: Vec<DockerNetworkPeer>,
}

/// Docker 网络创建结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkCreateResult {
    pub id: String,
    pub warning: Option<String>,
}

/// 将容器连接到网络的请求参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConnectRequest {
    pub container: String,
}

/// 从网络断开容器的请求参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDisconnectRequest {
    pub container: String,
    #[serde(default)]
    pub force: bool,
}

/// 资源采样数据的新鲜度与完整性状态。
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResourceSampleStatus {
    Fresh,
    Partial,
    Stale,
    Unavailable,
}

/// Docker 采样器内部保存的容器累计资源计数。
#[derive(Debug)]
pub struct ContainerResourceUsageSample {
    pub cpu_core_percent: f64,
    pub memory_working_set_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
}

/// Docker 容器最新资源统计及数据新鲜度。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerResourceUsageSummary {
    pub status: ResourceSampleStatus,
    pub collected_at: Option<i64>,
    pub cpu_core_percent: f64,
    pub memory_working_set_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
}

/// Docker 宿主机资源汇总。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResourceUsageSummary {
    pub status: ResourceSampleStatus,
    pub collected_at: Option<i64>,
    pub running_container_count: usize,
    pub sampled_container_count: usize,
    pub cpu_host_percent: f64,
    pub cpu_core_percent: f64,
    pub memory_working_set_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
}

/// Docker 宿主机单个时间点的资源使用数据。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResourceUsagePoint {
    pub timestamp: i64,
    pub cpu_host_percent: f64,
    pub cpu_core_percent: f64,
    pub memory_working_set_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
    pub running_container_count: usize,
    pub sampled_container_count: usize,
}

/// Docker 容器单个时间点的资源使用数据。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerResourceUsagePoint {
    pub timestamp: i64,
    pub cpu_core_percent: f64,
    pub memory_working_set_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
    pub network_rx_bytes_per_second: Option<f64>,
    pub network_tx_bytes_per_second: Option<f64>,
}

/// Docker 宿主机资源历史序列。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResourceUsageHistory {
    pub points: Vec<HostResourceUsagePoint>,
}

/// 批量查询容器统计的请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStatsBatchRequest {
    pub ids: Vec<String>,
}

/// 批量容器统计的响应数据。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStatsBatchResponse {
    pub summaries: HashMap<String, ContainerResourceUsageSummary>,
}

/// 概览趋势选择器中的容器简要信息。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendContainerItem {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub state: String,
}

/// 概览页所需的实时数据汇总。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewRealtimeResponse {
    pub collected_at: i64,
    pub container_states: ContainerStateCounts,
    pub project_states: ProjectStateCounts,
    pub images: ImageCounts,
    pub resource_usage: HostResourceUsageSummary,
    pub trend_containers: Vec<TrendContainerItem>,
}

/// 单个容器的历史趋势数据项。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStatsHistoryAllItem {
    pub id: String,
    pub name: String,
    pub points: Vec<ContainerResourceUsagePoint>,
}

/// 多容器历史趋势的汇总响应。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStatsHistoryAllResponse {
    pub containers: Vec<ContainerStatsHistoryAllItem>,
}

/// 批量查询容器资源趋势的请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStatsHistoryQuery {
    pub ids: Vec<String>,
    pub hours: Option<i64>,
}

/// Docker 磁盘使用分类统计。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerDiskUsageCategory {
    pub total_count: usize,
    pub active_count: usize,
    pub size_bytes: u64,
    pub reclaimable_bytes: u64,
}

/// Docker 系统磁盘使用汇总。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerDiskUsageSummary {
    pub collected_at: i64,
    pub images: DockerDiskUsageCategory,
    pub containers: DockerDiskUsageCategory,
    pub volumes: DockerDiskUsageCategory,
    pub build_cache: DockerDiskUsageCategory,
}

/// Docker 卷归属类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DockerVolumeManagementKind {
    Suite,
    Compose,
    Custom,
}

impl DockerVolumeManagementKind {
    /// 返回操作日志使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Suite => "suite",
            Self::Compose => "compose",
            Self::Custom => "custom",
        }
    }
}

/// Docker 卷归属信息。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolumeManagement {
    pub kind: DockerVolumeManagementKind,
    pub owner_name: Option<String>,
}

/// Docker 卷在当前模块允许执行的操作。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolumeCapabilities {
    pub can_remove: bool,
}

/// Docker 卷列表摘要。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolumeSummary {
    pub name: String,
    pub driver: String,
    pub created_at: Option<i64>,
    pub management: DockerVolumeManagement,
    pub capabilities: DockerVolumeCapabilities,
}

/// Docker 卷列表响应。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolumeListResponse {
    pub items: Vec<DockerVolumeSummary>,
    pub warnings: Vec<String>,
}

/// 引用 Docker 卷的容器挂载信息。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolumeContainerReference {
    pub id: String,
    pub name: String,
    pub state: String,
    pub destination: Option<String>,
    pub read_only: bool,
}

/// Docker 卷详情。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolumeDetail {
    pub summary: DockerVolumeSummary,
    pub mountpoint: String,
    pub scope: String,
    pub options: HashMap<String, String>,
    pub labels: HashMap<String, String>,
    pub referenced_containers: Vec<DockerVolumeContainerReference>,
}

/// Docker 本地卷创建请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerVolumeCreateRequest {
    pub name: String,
    pub options: Option<HashMap<String, String>>,
    pub labels: Option<HashMap<String, String>>,
}

/// Docker 操作日志的发起者类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DockerActivityActorKind {
    User,
    System,
}

impl DockerActivityActorKind {
    /// 返回数据库使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }
}

/// Docker 操作日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DockerActivityLevel {
    Info,
    Warning,
    Error,
}

impl DockerActivityLevel {
    /// 返回数据库使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Docker 操作执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DockerActivityOutcome {
    Success,
    Failure,
    Canceled,
}

impl DockerActivityOutcome {
    /// 返回数据库使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Canceled => "canceled",
        }
    }
}
