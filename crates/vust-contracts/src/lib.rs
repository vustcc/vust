//! 共享契约：统一维护前后端与节点间的 DTO、错误码与埋点模型。

pub mod api;
pub mod auth;
pub mod disks;
pub mod files;
pub mod firewall;
pub mod logging;
pub mod monitoring;
pub mod notification;
pub mod process;
pub mod runtime_docker;
pub mod runtime_logs;
pub mod scheduled_tasks;
pub mod scripts;
pub mod telemetry;
pub mod terminal;
pub mod types;
pub mod vust;

use sqlx::SqlitePool;
/// 统一的 SQLite 连接池别名，便于跨 crate 复用。
pub type DbPool = SqlitePool;
