//! Deprecated.
//!
//! 历史上用于存放 Controller 与 Agent 之间共享的数据结构。
//!
//! 随着 VUST Workspace 重构，公共类型已按领域迁移至
//! `vust-contracts` crate 中进行统一管理。
//!
//! 本文件仅作为迁移过渡保留，禁止新增类型。
//!
//! TODO:
//! - 删除所有对 `types.rs` 的引用
//! - 完成剩余类型迁移
//! - 删除本文件

use std::env;
use std::path::{Path, PathBuf};
use ts_rs::TS;

const DEFAULT_PRODUCTION_HOME: &str = "/opt/vust";
const DEFAULT_DEV_DATA_DIR: &str = ".vust";

/// Agent 的 socket 文件路径
pub fn agent_socket_path() -> PathBuf {
    if let Some(value) = env::var_os("VUST_AGENT_SOCKET") {
        return PathBuf::from(value);
    }
    if use_production_layout() {
        production_home().join("run/vust-agent.sock")
    } else {
        dev_base_dir().join("vust-agent.sock")
    }
}

fn production_home() -> PathBuf {
    env::var_os("VUST_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PRODUCTION_HOME))
}

fn dev_base_dir() -> PathBuf {
    if let Some(value) = env::var_os("VUST_DATA_DIR") {
        return PathBuf::from(value);
    }
    if let Some(value) = env::var_os("VUST_DEV_HOME") {
        return PathBuf::from(value);
    }
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    let root = find_workspace_root(&cwd).unwrap_or(cwd);
    root.join(DEFAULT_DEV_DATA_DIR)
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join("crates").is_dir() && ancestor.join("frontend").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn use_production_layout() -> bool {
    if env::var_os("VUST_DATA_DIR").is_some() {
        return false;
    }
    if env::var_os("VUST_DEV_HOME").is_some() {
        return false;
    }
    // 如果存在系统基准目录环境变量，即便处于 debug 模式也强制使用生产环境路径布局，保持路径行为的全局一致
    if env::var_os("VUST_HOME").is_some() {
        return true;
    }
    !cfg!(debug_assertions)
}

/// 表示 Docker 服务的可用性与故障类型。
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum DockerServiceStatus {
    Available,
    NotInstalled,
    NotRunning,
    PermissionDenied,
    #[default]
    Unknown,
}

/// Agent 端主机系统资源摘要。
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "types/")]
pub struct HostSystemSummary {
    pub version: String,
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub memory_percent: f64,
    #[serde(default)]
    pub load_avg_1: f64,
    #[serde(default)]
    pub load_avg_5: f64,
    #[serde(default)]
    pub load_avg_15: f64,
    #[serde(default)]
    pub disk_read_bytes: u64,
    #[serde(default)]
    pub disk_write_bytes: u64,
    #[serde(default)]
    pub network_rx_bytes: u64,
    #[serde(default)]
    pub network_tx_bytes: u64,
    #[serde(default)]
    pub collected_at: i64,
}

/// 仅包含 Docker 可用性与状态的简化摘要。
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DockerStatusSummary {
    pub docker_available: bool,
    pub docker_status: DockerServiceStatus,
}
