//! 应用状态：共享依赖（单库、缓存、配置）聚合。

use std::sync::Arc;
pub use vust_contracts::DbPool;

/// 节点部署会话状态，用于实时向前端反馈部署进度与日志。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploySession {
    pub progress_percent: u32,
    pub logs: Vec<String>,
    pub is_finished: bool,
    pub error: Option<String>,
}

/// 应用程序的共享状态，通过 `axum::extract::State` 在所有 `handler` 之间共享。
///
/// `AppState` 实例是在应用程序启动时创建的，并使用 `Arc` 进行包裹，
/// 以实现在多线程环境下的安全共享。
///
/// # 字段
/// - `server_name`: 服务器的名称，通常用于日志和标识。
/// - `metadata_db`: `vust` 服务的单一数据库连接池。
#[derive(Clone)]
pub struct AppState {
    pub server_name: String,
    pub metadata_db: DbPool,
    pub captcha_service: crate::security::captcha::CaptchaService,
    pub login_tracker: crate::security::login_tracker::LoginTracker,
    pub local_node_resource: Arc<tokio::sync::Mutex<Option<serde_json::Value>>>,
    pub image_acquisition: crate::services::image_acquisition::ImageAcquisitionService,
    pub terminal_tickets: Arc<crate::services::terminal_ticket::TerminalTicketStore>,
}
