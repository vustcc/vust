//! 宿主机终端共享契约。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 宿主机终端归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TerminalOwnership {
    System,
}

/// 宿主机终端可用状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TerminalAvailability {
    Available,
    Unsupported,
    Unavailable,
}

/// Agent 实际选择的 Shell。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TerminalShell {
    Bash,
    Sh,
}

impl TerminalShell {
    /// 返回 Shell 可执行文件的绝对路径。
    pub const fn path(self) -> &'static str {
        match self {
            Self::Bash => "/bin/bash",
            Self::Sh => "/bin/sh",
        }
    }
}

/// 宿主机终端稳定错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TerminalErrorCode {
    TerminalUnavailable,
    TerminalInvalidSize,
    TerminalSessionAlreadyActive,
    TerminalStartFailed,
    TerminalIoFailed,
    TerminalProtocolViolation,
}

/// 宿主机终端退出原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TerminalExitReason {
    ProcessExited,
    UserClosed,
    IdleTimeout,
    TransportClosed,
    IoFailed,
}

/// 宿主机终端能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCapabilities {
    pub can_start_session: bool,
}

/// 前端加载终端前使用的稳定访问模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "terminal/")]
pub struct TerminalAccess {
    #[ts(inline)]
    pub ownership: TerminalOwnership,
    pub node_id: String,
    pub node_name: String,
    #[ts(inline)]
    pub availability: TerminalAvailability,
    #[ts(inline)]
    pub shell: Option<TerminalShell>,
    pub idle_timeout_seconds: u64,
    #[ts(inline)]
    pub capabilities: TerminalCapabilities,
    #[ts(inline)]
    pub unavailable_reason: Option<TerminalErrorCode>,
}

/// Agent 向 Master 返回的运行时访问事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRuntimeAccess {
    pub availability: TerminalAvailability,
    pub shell: Option<TerminalShell>,
    pub idle_timeout_seconds: u64,
    pub can_start_session: bool,
    pub unavailable_reason: Option<TerminalErrorCode>,
}

/// 前端发往宿主机终端 WebSocket 的控制消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
#[ts(export_to = "terminal/")]
pub enum TerminalClientMessage {
    Start { cols: u16, rows: u16 },
    Resize { cols: u16, rows: u16 },
    Close,
}

/// Agent 发往前端的宿主机终端控制事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
#[ts(export_to = "terminal/")]
pub enum TerminalServerMessage {
    Started {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        #[ts(inline)]
        shell: TerminalShell,
        #[serde(rename = "startedAt")]
        #[ts(rename = "startedAt")]
        started_at: String,
        #[serde(rename = "idleTimeoutSeconds")]
        #[ts(rename = "idleTimeoutSeconds")]
        idle_timeout_seconds: u64,
    },
    IdleWarning {
        #[serde(rename = "expiresInSeconds")]
        #[ts(rename = "expiresInSeconds")]
        expires_in_seconds: u64,
    },
    Exited {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "exitCode")]
        #[ts(rename = "exitCode")]
        exit_code: Option<i32>,
        #[ts(inline)]
        reason: TerminalExitReason,
        #[serde(rename = "endedAt")]
        #[ts(rename = "endedAt")]
        ended_at: String,
    },
    Error {
        #[ts(inline)]
        code: TerminalErrorCode,
        message: String,
        recoverable: bool,
    },
}

/// Master 签发并等待 Agent 消费的一次性终端票据上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTicketContext {
    pub actor_name: String,
    pub client_ip: String,
    pub trace_id: String,
    pub node_id: String,
}

/// Agent 消费一次性终端票据的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalTicketConsumeRequest {
    pub ticket: String,
    pub node_id: String,
}

/// Agent 消费票据后获得的可信操作上下文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTicketConsumeResponse {
    pub actor_name: String,
    pub client_ip: String,
    pub trace_id: String,
    pub node_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_protocol_uses_camel_case_control_frames() {
        let json = serde_json::to_value(TerminalClientMessage::Start { cols: 80, rows: 24 })
            .expect("client message should serialize");
        assert_eq!(json["type"], "start");
        assert_eq!(json["payload"]["cols"], 80);
        assert_eq!(json["payload"]["rows"], 24);
    }

    #[test]
    fn server_protocol_exposes_stable_error_code() {
        let json = serde_json::to_value(TerminalServerMessage::Error {
            code: TerminalErrorCode::TerminalInvalidSize,
            message: "invalid terminal size".to_string(),
            recoverable: true,
        })
        .expect("server message should serialize");
        assert_eq!(json["kind"], "error");
        assert_eq!(json["payload"]["code"], "TERMINAL_INVALID_SIZE");
    }

    #[test]
    fn server_protocol_uses_camel_case_payload_fields() {
        let json = serde_json::to_value(TerminalServerMessage::Started {
            session_id: "session-1".to_string(),
            shell: TerminalShell::Bash,
            started_at: "2026-07-16T00:00:00Z".to_string(),
            idle_timeout_seconds: 1_800,
        })
        .expect("server message should serialize");
        assert_eq!(json["payload"]["sessionId"], "session-1");
        assert_eq!(json["payload"]["startedAt"], "2026-07-16T00:00:00Z");
        assert_eq!(json["payload"]["idleTimeoutSeconds"], 1_800);
        assert!(json["payload"].get("session_id").is_none());
    }

    #[test]
    fn ticket_request_rejects_unknown_fields() {
        let error = serde_json::from_str::<TerminalTicketConsumeRequest>(
            r#"{"ticket":"secret","nodeId":"local","actorName":"forged"}"#,
        )
        .expect_err("unknown identity fields must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }
}
