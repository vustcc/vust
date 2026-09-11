//! 操作审计契约：可信状态变更与安全事件的查询、详情和汇聚模型。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 操作审计所属领域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub enum OperationModule {
    Auth,
    Nodes,
    Suites,
    Docker,
    Files,
    Processes,
    Disks,
    Monitoring,
    Scripts,
    ScheduledTasks,
    Upgrades,
    Terminal,
    Settings,
}

impl OperationModule {
    /// 返回稳定的数据库值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Nodes => "nodes",
            Self::Suites => "suites",
            Self::Docker => "docker",
            Self::Files => "files",
            Self::Processes => "processes",
            Self::Disks => "disks",
            Self::Monitoring => "monitoring",
            Self::Scripts => "scripts",
            Self::ScheduledTasks => "scheduledTasks",
            Self::Upgrades => "upgrades",
            Self::Terminal => "terminal",
            Self::Settings => "settings",
        }
    }
}

/// 操作最终结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub enum OperationOutcome {
    Success,
    Failure,
    Partial,
    Canceled,
    TimedOut,
}

impl OperationOutcome {
    /// 返回稳定的数据库值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Partial => "partial",
            Self::Canceled => "canceled",
            Self::TimedOut => "timedOut",
        }
    }
}

/// 操作影响级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub enum OperationImpact {
    Info,
    Warning,
    Error,
}

impl OperationImpact {
    /// 返回稳定的数据库值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// 操作者类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub enum OperationActorKind {
    User,
    Anonymous,
    System,
    Agent,
    Suite,
}

/// 动态操作事件的双语名称快照。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub struct OperationEventLabel {
    pub zh_cn: String,
    pub en_us: String,
}

/// 操作者摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct OperationActor {
    pub kind: OperationActorKind,
    pub user_id: Option<i64>,
    pub display_name: String,
}

/// 事件产生位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub enum OperationOriginKind {
    Master,
    Agent,
}

/// 事件来源摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct OperationOrigin {
    pub kind: OperationOriginKind,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
}

/// 操作目标摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct OperationTarget {
    pub kind: String,
    pub id: String,
    pub display_name: Option<String>,
    pub ownership: Option<String>,
}

/// 单个操作对象的执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub enum OperationItemStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl OperationItemStatus {
    /// 返回稳定的数据库值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

/// 操作日志详情中的逐项执行事实。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct OperationLogItem {
    pub sequence: u32,
    pub source: OperationTarget,
    pub destination: Option<OperationTarget>,
    pub status: OperationItemStatus,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

/// 事件详情中允许保存的参数值。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export_to = "logging/")]
pub enum OperationParameterValue {
    String(String),
    Number(f64),
    Boolean(bool),
}

/// 操作日志能力。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub struct OperationLogCapabilities {
    pub can_view_details: bool,
}

/// 操作日志列表摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct OperationLogSummary {
    pub event_id: String,
    pub occurred_at: String,
    pub module: OperationModule,
    pub event_code: String,
    pub event_label: Option<OperationEventLabel>,
    pub actor: OperationActor,
    pub client_ip: Option<String>,
    pub origin: OperationOrigin,
    pub target: Option<OperationTarget>,
    pub outcome: OperationOutcome,
    pub impact: OperationImpact,
    pub trace_id: String,
    pub task_id: Option<String>,
    pub capabilities: OperationLogCapabilities,
}

/// 操作日志详情。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct OperationLogDetail {
    #[serde(flatten)]
    pub summary: OperationLogSummary,
    pub request_method: Option<String>,
    pub route_template: Option<String>,
    #[ts(type = "Record<string, string | number | boolean>")]
    pub parameters: BTreeMap<String, OperationParameterValue>,
    pub items: Vec<OperationLogItem>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

/// 操作日志查询。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct OperationLogQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    pub modules: Option<Vec<OperationModule>>,
    pub event_codes: Option<Vec<String>>,
    pub outcomes: Option<Vec<OperationOutcome>>,
    pub impacts: Option<Vec<OperationImpact>>,
    pub user_ids: Option<Vec<i64>>,
    pub node_ids: Option<Vec<String>>,
    pub occurred_from: Option<String>,
    pub occurred_to: Option<String>,
    pub keyword: Option<String>,
}

/// 操作日志分页结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub struct OperationLogPage {
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub items: Vec<OperationLogSummary>,
}

/// Agent 上报的操作审计事件。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct AgentOperationEvent {
    pub event_id: String,
    pub occurred_at: String,
    pub module: OperationModule,
    pub event_code: String,
    pub event_label: Option<OperationEventLabel>,
    pub actor: OperationActor,
    pub client_ip: Option<String>,
    pub target: Option<OperationTarget>,
    pub outcome: OperationOutcome,
    pub impact: OperationImpact,
    pub trace_id: String,
    pub task_id: Option<String>,
    #[ts(type = "Record<string, string | number | boolean>")]
    pub parameters: BTreeMap<String, OperationParameterValue>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

/// 套件运行时提交的语义操作事件。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/", optional_fields)]
pub struct SuiteOperationEventRequest {
    pub event_id: String,
    pub operation_context_id: Option<String>,
    pub event_code: String,
    pub event_label: OperationEventLabel,
    pub outcome: OperationOutcome,
    pub impact: OperationImpact,
    pub target: Option<OperationTarget>,
    pub task_id: Option<String>,
    #[ts(type = "Record<string, string | number | boolean>")]
    pub parameters: BTreeMap<String, OperationParameterValue>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

/// Agent 批量上报请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub struct AgentOperationEventBatch {
    pub events: Vec<AgentOperationEvent>,
}

/// Master 对批量上报的确认。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "logging/")]
pub struct AgentOperationEventAck {
    pub accepted_event_ids: Vec<String>,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    20
}
