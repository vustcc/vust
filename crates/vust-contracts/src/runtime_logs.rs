//! 运行日志查询契约：受控读取 tracing 轮转文件。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 文件列表可用性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "runtime_logs/")]
pub enum RuntimeLogAvailability {
    Available,
    NodeOffline,
    ServiceStopped,
    NoLogFiles,
    Unavailable,
}

/// 运行日志文件摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "runtime_logs/", optional_fields)]
pub struct RuntimeLogFile {
    pub service: String,
    pub node_id: Option<String>,
    pub node_name: Option<String>,
    pub file_id: String,
    pub file_version: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
}

/// 文件列表与明确可用性。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "runtime_logs/", optional_fields)]
pub struct RuntimeLogFileList {
    pub availability: RuntimeLogAvailability,
    pub reason_code: Option<String>,
    pub files: Vec<RuntimeLogFile>,
}

/// 运行日志查询参数。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "runtime_logs/", optional_fields)]
pub struct RuntimeLogQuery {
    pub service: String,
    pub node_id: Option<String>,
    pub file_id: String,
    pub file_name: String,
    pub level: Option<String>,
    pub target: Option<String>,
    pub keyword: Option<String>,
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// 运行日志单行结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "runtime_logs/", optional_fields)]
pub struct RuntimeLogLine {
    pub offset: u64,
    pub timestamp: Option<String>,
    pub level: Option<String>,
    pub target: Option<String>,
    pub source: Option<String>,
    pub message: String,
    pub parse_error: bool,
}

/// 运行日志查询结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "runtime_logs/", optional_fields)]
pub struct RuntimeLogQueryResult {
    pub lines: Vec<RuntimeLogLine>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub scan_truncated: bool,
    pub file_version: String,
}

fn default_limit() -> u32 {
    200
}
