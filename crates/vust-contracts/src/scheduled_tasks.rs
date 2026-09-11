//! 计划任务领域契约：任务定义、部署、运行、后台操作与批量操作。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 计划任务资源归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskOwnershipKind {
    Custom,
    Compose,
    Suite,
    System,
}

/// 用户期望的任务启停状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskDesiredState {
    Enabled,
    Disabled,
}

/// 任务定义在执行节点上的部署状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskDeploymentStatus {
    Pending,
    Applying,
    Ready,
    WaitingForNode,
    Failed,
    Deleting,
    Migrating,
}

/// 下一次执行时间的语义状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskNextRunStatus {
    Scheduled,
    Disabled,
    NotDeployed,
    Unavailable,
}

/// 任务运行触发来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskTriggerSource {
    Schedule,
    Manual,
    Batch,
}

/// 任务运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskRunStatus {
    Queued,
    Starting,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

impl ScheduledTaskRunStatus {
    /// 判断运行是否已经进入终态。
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::TimedOut | Self::Cancelled
        )
    }
}

/// 计划任务后台操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskOperationKind {
    Deploy,
    Update,
    StateChange,
    Remove,
    Migrate,
    Batch,
}

/// 计划任务后台操作状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskOperationStatus {
    Queued,
    Running,
    Cancelling,
    Succeeded,
    Partial,
    Failed,
    Cancelled,
}

impl ScheduledTaskOperationStatus {
    /// 判断后台操作是否已经进入终态。
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Partial | Self::Failed | Self::Cancelled
        )
    }
}

/// 批量操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub enum ScheduledTaskBatchAction {
    Enable,
    Disable,
    Run,
    Remove,
}

/// 执行节点摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskNode {
    pub node_id: String,
    pub node_name: String,
}

/// 任务的分钟级 Cron 调度定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskSchedule {
    pub cron_expr: String,
    pub time_zone: String,
    pub summary: String,
}

/// 计划任务资源归属信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskOwnership {
    pub kind: ScheduledTaskOwnershipKind,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    pub manager_path: Option<String>,
}

/// Master 统一计算的任务操作能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskCapabilities {
    pub can_update: bool,
    pub can_change_state: bool,
    pub can_run: bool,
    pub can_remove: bool,
    pub can_migrate: bool,
}

/// 任务定义在节点上的部署摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskDeployment {
    pub status: ScheduledTaskDeploymentStatus,
    pub revision: i64,
    pub last_synced_at: Option<String>,
    pub error_summary: Option<String>,
}

/// 下一次运行时间及其不可用原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskNextRun {
    pub status: ScheduledTaskNextRunStatus,
    pub at: Option<String>,
}

/// 最近一次运行摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskLastRun {
    pub run_id: String,
    pub status: ScheduledTaskRunStatus,
    pub finished_at: Option<String>,
}

/// 计划任务列表摘要，不包含命令正文。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskSummary {
    pub task_id: String,
    pub name: String,
    pub description: Option<String>,
    #[ts(inline)]
    pub node: ScheduledTaskNode,
    #[ts(inline)]
    pub schedule: ScheduledTaskSchedule,
    pub desired_state: ScheduledTaskDesiredState,
    #[ts(inline)]
    pub deployment: ScheduledTaskDeployment,
    #[ts(inline)]
    pub next_run: ScheduledTaskNextRun,
    pub last_run: Option<ScheduledTaskLastRun>,
    #[ts(inline)]
    pub ownership: ScheduledTaskOwnership,
    #[ts(inline)]
    pub capabilities: ScheduledTaskCapabilities,
    pub created_at: String,
    pub updated_at: String,
}

/// Shell 执行配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskExecution {
    #[ts(type = "\"shell\"")]
    pub kind: String,
    pub command: String,
    pub timeout_seconds: u32,
    pub prevent_overlap: bool,
}

/// 计划任务按需加载的详情。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskDetail {
    #[serde(flatten)]
    #[ts(flatten)]
    pub summary: ScheduledTaskSummary,
    #[ts(inline)]
    pub execution: ScheduledTaskExecution,
}

/// 分页任务列表。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskListPage {
    pub items: Vec<ScheduledTaskSummary>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub loaded_at: String,
}

/// 创建自定义计划任务请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct CreateScheduledTaskRequest {
    pub name: String,
    pub description: Option<String>,
    pub node_id: String,
    pub cron_expr: String,
    pub time_zone: String,
    pub command: String,
    pub timeout_seconds: u32,
    pub prevent_overlap: bool,
    pub enabled: bool,
}

/// 更新计划任务请求；节点和资源归属不可通过编辑变更。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct UpdateScheduledTaskRequest {
    pub name: String,
    pub description: Option<String>,
    pub cron_expr: String,
    pub time_zone: String,
    pub command: String,
    pub timeout_seconds: u32,
    pub prevent_overlap: bool,
}

/// 更新任务启停状态请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scheduled-tasks/")]
pub struct UpdateScheduledTaskStateRequest {
    pub enabled: bool,
}

/// 创建节点迁移操作请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scheduled-tasks/")]
pub struct CreateScheduledTaskMigrationRequest {
    pub target_node_id: String,
}

/// 创建批量操作请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scheduled-tasks/")]
pub struct CreateScheduledTaskBatchRequest {
    pub action: ScheduledTaskBatchAction,
    pub task_ids: Vec<String>,
}

/// 运行输出可用性摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskRunOutputSummary {
    pub available: bool,
    pub truncated: bool,
    pub size_bytes: u64,
}

/// 任务运行操作能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskRunCapabilities {
    pub can_cancel: bool,
}

/// 可恢复跟踪的单次任务运行。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskRun {
    pub run_id: String,
    pub task_id: String,
    pub node_id: String,
    pub trigger_source: ScheduledTaskTriggerSource,
    pub status: ScheduledTaskRunStatus,
    pub phase: Option<String>,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    #[ts(inline)]
    pub output: ScheduledTaskRunOutputSummary,
    #[ts(inline)]
    pub capabilities: ScheduledTaskRunCapabilities,
}

/// 分页任务运行记录。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskRunPage {
    pub items: Vec<ScheduledTaskRun>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub loaded_at: String,
}

/// 有界任务输出分页。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskRunOutput {
    pub run_id: String,
    pub content: String,
    pub offset_bytes: u64,
    pub next_offset_bytes: Option<u64>,
    pub size_bytes: u64,
    pub truncated: bool,
}

/// 可恢复跟踪的任务后台操作。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskOperation {
    pub operation_id: String,
    pub task_id: String,
    pub kind: ScheduledTaskOperationKind,
    pub status: ScheduledTaskOperationStatus,
    pub phase: Option<String>,
    pub completed_steps: u32,
    pub total_steps: u32,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub warning_summary: Option<String>,
    pub can_cancel: bool,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

/// 批量操作中的单项结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scheduled-tasks/", optional_fields)]
pub struct ScheduledTaskBatchItem {
    pub task_id: String,
    pub run_id: Option<String>,
    pub operation_id: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

/// 可恢复跟踪的批量操作。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scheduled-tasks/")]
pub struct ScheduledTaskBatch {
    pub batch_id: String,
    pub action: ScheduledTaskBatchAction,
    pub status: ScheduledTaskOperationStatus,
    pub items: Vec<ScheduledTaskBatchItem>,
    pub created_at: String,
    pub updated_at: String,
}

/// Master 下发给 Agent 的任务定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentScheduledTaskDefinition {
    pub operation_id: String,
    pub task_id: String,
    pub revision: i64,
    pub name: String,
    pub command: String,
    pub cron_expr: String,
    pub time_zone: String,
    pub desired_state: ScheduledTaskDesiredState,
    pub timeout_seconds: u32,
    pub prevent_overlap: bool,
    pub ownership: ScheduledTaskOwnership,
}

/// Master 请求 Agent 创建一次后台运行。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentStartScheduledTaskRunRequest {
    pub operation_id: String,
    pub run_id: String,
    pub trigger_source: ScheduledTaskTriggerSource,
}

/// Agent 向 Master 可靠上报的运行事实。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentScheduledTaskRunReport {
    pub run: ScheduledTaskRun,
    pub output_content: Option<String>,
}
