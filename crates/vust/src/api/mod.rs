//! API 入口：各业务路由模块的统一聚合。

mod agent;
pub mod apps;
pub mod auth;
pub mod desktop_apps;
pub mod disks;
pub mod docker;
pub mod files;
pub mod firewall;
pub mod node_proxy;
pub mod nodes;
pub mod notifications;
pub mod operation_logs;
pub mod process;
pub mod routes;
pub mod runtime;
pub mod runtime_logs;
pub mod scripts;
pub mod security;
pub mod suites;
pub mod system_monitoring;
pub mod task_scheduler;
pub mod terminal;
pub mod upgrades;
pub mod vust;
