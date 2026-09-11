//! 通用埋点信封；操作审计使用 `logging` 模块的严格领域契约。

use serde::{Deserialize, Serialize};

/// 事件来源端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TelemetrySource {
    Vust,
    Agent,
    Frontend,
}

impl TelemetrySource {
    /// 返回稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vust => "vust",
            Self::Agent => "agent",
            Self::Frontend => "frontend",
        }
    }
}

/// 通用非审计埋点信封。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformEventEnvelope<T> {
    pub event_name: String,
    pub trace_id: String,
    pub source: TelemetrySource,
    pub payload: T,
}
