//! 系统监控领域契约：定义稳定的概览、趋势与设置 DTO。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 系统监控资源归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SystemMonitoringOwnership {
    System,
}

/// 实时快照质量状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SystemMonitoringSnapshotStatus {
    Fresh,
    Partial,
    Stale,
    Unavailable,
}

/// 历史趋势整体质量状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SystemMonitoringSeriesStatus {
    Complete,
    Partial,
    Stale,
    Empty,
}

/// 历史采集器的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SystemMonitoringCollectionState {
    Initializing,
    Running,
    Degraded,
    Stopped,
}

/// 单个采集来源的可用状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SystemMonitoringSourceState {
    Available,
    Unavailable,
}

/// 系统监控管理能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SystemMonitoringCapabilities {
    pub can_manage_collection: bool,
    pub can_clear_history: bool,
}

/// 单个采集来源的质量摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SystemMonitoringSourceStatus {
    pub source: String,
    #[ts(inline)]
    pub state: SystemMonitoringSourceState,
}

/// 系统监控指标值；`None` 明确表示缺失或不可用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SystemMonitoringMetrics {
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_percent: Option<f64>,
    pub load_average_1m: Option<f64>,
    pub load_average_5m: Option<f64>,
    pub load_average_15m: Option<f64>,
    pub disk_read_bytes_per_second: Option<f64>,
    pub disk_write_bytes_per_second: Option<f64>,
    pub network_receive_bytes_per_second: Option<f64>,
    pub network_transmit_bytes_per_second: Option<f64>,
}

/// 历史采集状态摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SystemMonitoringHistorySummary {
    #[ts(inline)]
    pub state: SystemMonitoringCollectionState,
    pub sample_interval_seconds: u32,
    pub retention_days: u8,
    pub last_sampled_at: Option<String>,
}

/// 系统监控实时概览。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "monitoring/")]
pub struct SystemMonitoringOverview {
    #[ts(inline)]
    pub ownership: SystemMonitoringOwnership,
    pub observed_at: Option<String>,
    #[ts(inline)]
    pub snapshot_status: SystemMonitoringSnapshotStatus,
    pub coverage_percent: f64,
    #[ts(inline)]
    pub sources: Vec<SystemMonitoringSourceStatus>,
    #[ts(inline)]
    pub metrics: SystemMonitoringMetrics,
    #[ts(inline)]
    pub history: SystemMonitoringHistorySummary,
    #[ts(inline)]
    pub capabilities: SystemMonitoringCapabilities,
}

/// 历史趋势中的一个时间点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SystemMonitoringSeriesPoint {
    pub sampled_at: String,
    pub coverage_percent: f64,
    #[ts(inline)]
    pub metrics: SystemMonitoringMetrics,
}

/// 历史趋势游标分页信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SystemMonitoringPageInfo {
    pub limit: u16,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

/// 系统监控历史趋势页。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "monitoring/")]
pub struct SystemMonitoringSeriesPage {
    pub range: String,
    pub from: String,
    pub to: String,
    pub resolution_seconds: u32,
    #[ts(inline)]
    pub series_status: SystemMonitoringSeriesStatus,
    pub expected_point_count: u32,
    pub actual_point_count: u32,
    pub coverage_percent: f64,
    #[ts(inline)]
    pub points: Vec<SystemMonitoringSeriesPoint>,
    #[ts(inline)]
    pub page_info: SystemMonitoringPageInfo,
}

/// 系统监控设置与存储摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "monitoring/")]
pub struct SystemMonitoringSettings {
    #[ts(inline)]
    pub ownership: SystemMonitoringOwnership,
    pub history_collection_enabled: bool,
    pub history_sample_interval_seconds: u32,
    pub retention_days: u8,
    pub stored_sample_count: u64,
    pub oldest_sampled_at: Option<String>,
    pub newest_sampled_at: Option<String>,
    #[ts(inline)]
    pub capabilities: SystemMonitoringCapabilities,
}
