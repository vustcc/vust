//! 脚本库领域契约：脚本资产、一次性终端运行与 Agent 上报。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 脚本资源归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub enum ScriptOwnershipKind {
    Custom,
    Compose,
    Suite,
    System,
}

/// 脚本运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub enum ScriptRunStatus {
    Queued,
    Starting,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

impl ScriptRunStatus {
    /// 判断运行是否进入终态。
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::TimedOut | Self::Cancelled
        )
    }
}

/// 脚本运行终端稳定错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export_to = "scripts/")]
pub enum ScriptRunTerminalErrorCode {
    ScriptRunSessionAlreadyAttached,
    ScriptRunInvalidTerminalSize,
    ScriptRunTerminalStartFailed,
    ScriptRunTerminalIoFailed,
    ScriptRunTerminalAttachTimeout,
    ScriptRunTerminalProtocolViolation,
}

/// 浏览器发往脚本运行 WebSocket 的控制消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub enum ScriptRunTerminalClientMessage {
    Start { cols: u16, rows: u16 },
    Resize { cols: u16, rows: u16 },
    Close,
}

/// Agent 发往浏览器的脚本运行控制事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub enum ScriptRunTerminalServerMessage {
    Started {
        #[serde(rename = "runId")]
        #[ts(rename = "runId")]
        run_id: String,
        #[serde(rename = "startedAt")]
        #[ts(rename = "startedAt")]
        started_at: String,
        #[serde(rename = "timeoutSeconds")]
        #[ts(rename = "timeoutSeconds")]
        timeout_seconds: u32,
    },
    Exited {
        #[serde(rename = "runId")]
        #[ts(rename = "runId")]
        run_id: String,
        #[serde(rename = "exitCode")]
        #[ts(rename = "exitCode")]
        exit_code: Option<i32>,
        status: ScriptRunStatus,
        #[serde(rename = "endedAt")]
        #[ts(rename = "endedAt")]
        ended_at: String,
    },
    Error {
        #[ts(inline)]
        code: ScriptRunTerminalErrorCode,
        message: String,
        recoverable: bool,
    },
}

/// 脚本资源归属信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/", optional_fields)]
pub struct ScriptOwnership {
    pub kind: ScriptOwnershipKind,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    pub manager_path: Option<String>,
}

/// Master 统一计算的脚本能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub struct ScriptCapabilities {
    pub can_update: bool,
    pub can_remove: bool,
    pub can_clone: bool,
    pub can_run: bool,
}

/// 脚本列表摘要，不包含正文。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/", optional_fields)]
pub struct ScriptSummary {
    pub script_id: String,
    pub name: String,
    pub description: Option<String>,
    pub interactive: bool,
    #[ts(type = "\"shell\"")]
    pub language: String,
    pub revision: i64,
    #[ts(inline)]
    pub ownership: ScriptOwnership,
    #[ts(inline)]
    pub capabilities: ScriptCapabilities,
    pub created_at: String,
    pub updated_at: String,
    pub updated_by: String,
}

/// 脚本正文元数据。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub struct ScriptSource {
    pub content: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// 脚本默认执行配置。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub struct ScriptExecutionDefaults {
    pub timeout_seconds: u32,
}

/// 按需加载的脚本详情。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub struct ScriptDetail {
    #[serde(flatten)]
    #[ts(flatten)]
    pub summary: ScriptSummary,
    #[ts(inline)]
    pub source: ScriptSource,
    #[ts(inline)]
    pub execution_defaults: ScriptExecutionDefaults,
}

/// 分页脚本列表。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub struct ScriptListPage {
    pub items: Vec<ScriptSummary>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub loaded_at: String,
}

/// 创建脚本请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scripts/", optional_fields)]
pub struct CreateScriptRequest {
    pub name: String,
    pub description: Option<String>,
    pub interactive: bool,
    pub content: String,
    pub timeout_seconds: Option<u32>,
}

/// 更新脚本请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scripts/", optional_fields)]
pub struct UpdateScriptRequest {
    pub expected_revision: i64,
    pub name: String,
    pub description: Option<String>,
    pub interactive: bool,
    pub content: String,
    pub timeout_seconds: u32,
}

/// 创建单节点脚本运行请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "scripts/", optional_fields)]
pub struct CreateScriptRunRequest {
    pub node_id: String,
    pub timeout_seconds: Option<u32>,
}

/// 运行操作能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/")]
pub struct ScriptRunCapabilities {
    pub can_cancel: bool,
}

/// 可恢复跟踪的脚本运行。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "scripts/", optional_fields)]
pub struct ScriptRun {
    pub run_id: String,
    pub script_id: String,
    pub script_name: String,
    pub script_revision: i64,
    pub source_sha256: String,
    pub node_id: String,
    pub node_name: String,
    pub status: ScriptRunStatus,
    pub phase: Option<String>,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    #[ts(inline)]
    pub capabilities: ScriptRunCapabilities,
}

/// Master 下发给 Agent 的脚本快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentStartScriptRunRequest {
    pub run_id: String,
    pub script_id: String,
    pub script_name: String,
    pub script_revision: i64,
    pub source_content: String,
    pub source_sha256: String,
    pub timeout_seconds: u32,
    pub ownership_kind: ScriptOwnershipKind,
}

/// Agent 向 Master 上报的运行事实。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentScriptRunReport {
    pub run_id: String,
    pub script_id: String,
    pub script_revision: i64,
    pub source_sha256: String,
    pub status: ScriptRunStatus,
    pub phase: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

/// Agent 批量上报请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentScriptRunReportBatch {
    pub reports: Vec<AgentScriptRunReport>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_control_frames_use_tagged_camel_case_payloads() {
        assert_eq!(
            serde_json::to_value(ScriptRunTerminalClientMessage::Start { cols: 80, rows: 24 })
                .unwrap(),
            serde_json::json!({"type":"start","payload":{"cols":80,"rows":24}})
        );
        assert_eq!(
            serde_json::to_value(ScriptRunTerminalClientMessage::Close).unwrap(),
            serde_json::json!({"type":"close"})
        );
        let error = ScriptRunTerminalServerMessage::Error {
            code: ScriptRunTerminalErrorCode::ScriptRunTerminalProtocolViolation,
            message: "invalid".into(),
            recoverable: false,
        };
        assert_eq!(
            serde_json::to_value(error).unwrap()["payload"]["code"],
            "SCRIPT_RUN_TERMINAL_PROTOCOL_VIOLATION"
        );
    }
}
