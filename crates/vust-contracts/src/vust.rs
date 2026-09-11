//! VUST 管理共享契约。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 当前 VUST 监听配置。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "vust/")]
pub struct VustNetworkConfig {
    pub host: String,
    pub port: u16,
    pub public_host: Option<String>,
}

/// 更新 VUST 监听配置后的返回载荷。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "vust/")]
pub struct VustNetworkUpdateResult {
    pub host: String,
    pub port: u16,
    pub next_url: String,
}
