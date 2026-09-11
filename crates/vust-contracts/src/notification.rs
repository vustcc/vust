//! 个人通知中心契约：定义可信事件投影、个人状态与查询边界。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::logging::{OperationModule, OperationOutcome, OperationParameterValue};

/// 通知注册表中的稳定事件码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub enum NotificationCode {
    NodeDeploymentFinished,
    SuiteInstallationFinished,
    ScriptRunFinished,
    ScheduledTaskOperationFinished,
    ScheduledTaskRunFinished,
    FileTaskFinished,
    FileTransferFinished,
    DiskOperationFinished,
    DockerImageTaskFinished,
    DockerProjectTaskFinished,
    DockerEngineInstallationFinished,
    UpgradePlanFinished,
    NodeOffline,
    NodeRecovered,
    LoginLockout,
}

impl NotificationCode {
    /// 返回数据库使用的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NodeDeploymentFinished => "nodeDeploymentFinished",
            Self::SuiteInstallationFinished => "suiteInstallationFinished",
            Self::ScriptRunFinished => "scriptRunFinished",
            Self::ScheduledTaskOperationFinished => "scheduledTaskOperationFinished",
            Self::ScheduledTaskRunFinished => "scheduledTaskRunFinished",
            Self::FileTaskFinished => "fileTaskFinished",
            Self::FileTransferFinished => "fileTransferFinished",
            Self::DiskOperationFinished => "diskOperationFinished",
            Self::DockerImageTaskFinished => "dockerImageTaskFinished",
            Self::DockerProjectTaskFinished => "dockerProjectTaskFinished",
            Self::DockerEngineInstallationFinished => "dockerEngineInstallationFinished",
            Self::UpgradePlanFinished => "upgradePlanFinished",
            Self::NodeOffline => "nodeOffline",
            Self::NodeRecovered => "nodeRecovered",
            Self::LoginLockout => "loginLockout",
        }
    }
}

/// 通知所属类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub enum NotificationCategory {
    Task,
    Security,
    System,
}

impl NotificationCategory {
    /// 返回数据库使用的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Security => "security",
            Self::System => "system",
        }
    }
}

/// 通知需要用户关注的程度，与操作结果相互独立。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub enum NotificationAttentionLevel {
    Info,
    Warning,
    Critical,
}

impl NotificationAttentionLevel {
    /// 返回数据库使用的稳定值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// 通知查询范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub enum NotificationArchiveScope {
    Active,
    Archived,
}

/// 通知读取状态筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub enum NotificationReadFilter {
    All,
    Read,
    Unread,
}

/// 通知来源摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/", optional_fields)]
pub struct NotificationSource {
    pub module: OperationModule,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
}

/// 通知关联的安全目标摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/", optional_fields)]
pub struct NotificationSubject {
    pub kind: String,
    pub id: String,
    pub display_name: Option<String>,
}

/// 后端注册表计算的安全应用动作。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub struct NotificationAction {
    pub app_id: String,
    pub label_key: String,
    #[ts(type = "Record<string, string | number | boolean>")]
    pub payload: BTreeMap<String, OperationParameterValue>,
}

/// 当前用户对通知可执行的能力。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub struct NotificationCapabilities {
    pub can_view_details: bool,
    pub can_mark_read: bool,
    pub can_mark_unread: bool,
    pub can_archive: bool,
    pub can_restore: bool,
    pub can_open_target: bool,
}

/// 通知列表摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/", optional_fields)]
pub struct NotificationSummary {
    pub notification_id: String,
    pub created_at: String,
    pub code: NotificationCode,
    pub category: NotificationCategory,
    pub attention_level: NotificationAttentionLevel,
    pub outcome: Option<OperationOutcome>,
    pub source: NotificationSource,
    pub subject: Option<NotificationSubject>,
    pub task_id: Option<String>,
    pub operation_event_id: String,
    #[ts(type = "Record<string, string | number | boolean>")]
    pub parameters: BTreeMap<String, OperationParameterValue>,
    pub read_at: Option<String>,
    pub archived_at: Option<String>,
    pub action: Option<NotificationAction>,
    pub capabilities: NotificationCapabilities,
}

/// 通知详情。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/", optional_fields)]
pub struct NotificationDetail {
    #[serde(flatten)]
    pub summary: NotificationSummary,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub trace_id: String,
}

/// 通知分页查询。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/", optional_fields)]
pub struct NotificationQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default = "default_archive_scope")]
    pub archive_scope: NotificationArchiveScope,
    #[serde(default = "default_read_filter")]
    pub read_filter: NotificationReadFilter,
    pub categories: Option<Vec<NotificationCategory>>,
    pub attention_levels: Option<Vec<NotificationAttentionLevel>>,
    pub modules: Option<Vec<OperationModule>>,
    pub codes: Option<Vec<NotificationCode>>,
    pub created_from: Option<String>,
    pub created_to: Option<String>,
    pub keyword: Option<String>,
}

/// 通知分页结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub struct NotificationPage {
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub items: Vec<NotificationSummary>,
}

/// 未读角标摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/", optional_fields)]
pub struct NotificationUnreadSummary {
    pub unread_count: i64,
    pub latest_created_at: Option<String>,
    pub version: String,
}

/// 单条通知读取状态变更。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub struct NotificationReadStateRequest {
    pub read: bool,
}

/// 单条通知归档状态变更。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub struct NotificationArchiveStateRequest {
    pub archived: bool,
}

/// 批量通知归档状态变更。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "notification/")]
pub struct NotificationBatchArchiveStateRequest {
    pub notification_ids: Vec<String>,
    pub archived: bool,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    20
}

fn default_archive_scope() -> NotificationArchiveScope {
    NotificationArchiveScope::Active
}

fn default_read_filter() -> NotificationReadFilter {
    NotificationReadFilter::All
}
