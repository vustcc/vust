//! 路由注册：挂载所有 API 子路由并拼装主路由器。

use super::{
    apps, auth, desktop_apps, disks, docker, files, firewall, nodes, notifications, operation_logs,
    process, runtime, runtime_logs, scripts, security, suites, system_monitoring, task_scheduler,
    terminal, upgrades, vust,
};
use crate::api::node_proxy;
use crate::db::init_db_pool;
use crate::services::static_handler::fallback_handler;
use crate::services::user_service::create_initial_admin_if_not_exists;
use crate::state::AppState;
use anyhow::Result;
use axum::extract::connect_info::ConnectInfo;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{Method, Request},
    middleware,
    routing::get,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::info_span;

/// 只包含公共路由的函数
pub fn public_api_router() -> Router {
    Router::new()
}

/// 需要 state 的路由
pub fn state_api_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    let protected = Router::new()
        .nest("/operation-logs", operation_logs::router())
        .nest("/runtime-logs", runtime_logs::router())
        .nest("/nodes", nodes::nodes_router())
        .nest("/node", nodes::single_node_router())
        .nest("/node", system_monitoring::system_monitoring_router())
        .nest("/node", terminal::terminal_router())
        .nest("/node", files::file_router())
        .nest("/node", firewall::firewall_router())
        .nest("/node", process::process_router())
        .nest("/node", disks::disk_router())
        .nest("/agent", node_proxy::agent_router())
        .nest("/vust", vust::vust_router())
        .nest("/docker", docker::docker_router())
        .nest("/notifications", notifications::notifications_router())
        .nest("/apps", apps::apps_router())
        .nest("/desktop", desktop_apps::desktop_apps_router())
        .nest("/scheduled-tasks", task_scheduler::scheduled_tasks_router())
        .nest(
            "/scheduled-task-operations",
            task_scheduler::scheduled_task_operations_router(),
        )
        .nest(
            "/scheduled-task-batches",
            task_scheduler::scheduled_task_batches_router(),
        )
        .nest("/upgrades", upgrades::upgrades_router())
        .nest("/scripts", scripts::scripts_router())
        .nest("/script-runs", scripts::script_runs_router())
        .nest("/security", security::security_router())
        .nest("/suites", suites::suites_router())
        .nest("/suite-instances", suites::suite_instances_router())
        .nest("/suite-install-tasks", suites::suite_install_tasks_router())
        .layer(middleware::from_fn_with_state(
            Arc::clone(&app_state),
            auth::auth_layer,
        ));

    Router::new()
        .route("/captcha", get(auth::captcha))
        .nest("/auth", auth::auth_router())
        .nest("/runtime", runtime::runtime_router())
        .merge(protected)
}

// 共享中间件已从 vust-api 导入

/// 创建路由
pub async fn create_router() -> Result<(Router, crate::state::DbPool)> {
    let db = init_db_pool().await?;
    let db_for_shutdown = db.clone();

    tracing::info!("Database connection pool established successfully.");

    // 初始化默认用户
    // 确保在数据库连接和迁移成功后执行
    create_initial_admin_if_not_exists(&db)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to initialize default admin user: {}", e))?;

    // 创建共享的应用状态
    let app_state = Arc::new(AppState {
        server_name: "VUST".to_string(),
        metadata_db: db,
        captcha_service: crate::security::captcha::CaptchaService::default(),
        login_tracker: crate::security::login_tracker::LoginTracker::default(),
        local_node_resource: Arc::new(tokio::sync::Mutex::new(None)),
        image_acquisition: crate::services::image_acquisition::ImageAcquisitionService::new(),
        terminal_tickets: Arc::new(
            crate::services::terminal_ticket::TerminalTicketStore::default(),
        ),
    });
    crate::services::node_session_reaper::spawn_session_reaper(Arc::clone(&app_state));
    crate::services::upgrades::spawn_upgrade_scheduler(Arc::clone(&app_state));
    crate::services::node_read_model::spawn_local_node_monitor(Arc::clone(&app_state));
    crate::services::node_task_recovery::spawn_node_task_recovery(Arc::clone(&app_state));
    crate::services::task_sync::spawn_sync_queue_worker(Arc::clone(&app_state));
    crate::services::script_runs::spawn_worker(Arc::clone(&app_state));
    crate::services::file_task_audit::spawn_reconciler(Arc::clone(&app_state));
    crate::services::logging::init_operation_log_writer(app_state.metadata_db.clone());
    crate::services::logging::spawn_retention_worker(app_state.metadata_db.clone());
    notifications::spawn_retention_worker(app_state.metadata_db.clone());
    disks::recover_operations(Arc::clone(&app_state)).await?;

    // 配置 CORS：允许所有来源、常用方法
    let cors = CorsLayer::new()
        .allow_origin(Any) // 你也可以用 .allow_origin("https://example.com".parse().unwrap())
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers(Any);

    // 创建一个需要状态的路由实例，并立即为其提供状态
    let state_routes = state_api_router(Arc::clone(&app_state)).with_state(Arc::clone(&app_state));
    let routed_app = Router::new()
        .nest("/api/v1", public_api_router())
        .nest("/api/v1", state_routes)
        .fallback(fallback_handler);

    let router = routed_app
        .layer(cors)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&app_state),
            crate::security::safe_entry::safe_entry_layer,
        ))
        .layer(middleware::from_fn(vust_api::logging::http_log_middleware))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let forwarded_ip = request
                    .headers()
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.split(',').next().unwrap_or("").trim().to_string());
                let connect_ip = request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|ConnectInfo(addr)| addr.to_string());
                let client_ip = forwarded_ip
                    .or(connect_ip)
                    .unwrap_or_else(|| "unknown".to_string());

                info_span!(
                    env!("CARGO_CRATE_NAME"),
                    client_ip,
                    method = ?request.method(),
                    path = ?request.uri().path(),
                    some_other_field = tracing::field::Empty,
                )
            }),
        )
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(
            10 * 1024 * 1024 * 1024, /* 10GB */
        ));

    Ok((router, db_for_shutdown))
}
