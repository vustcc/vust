//! 路由注册：挂载所有 API 子路由并拼装主路由器。

use super::api::{
    disks, docker, firewall, fs, host_terminal, process, runtime_logs, scheduled_tasks,
    script_runs, suite_operation_logs, suite_workloads, system, system_monitoring, upgrade,
    websocket,
};
use crate::db;
use crate::state::DbPool;

use crate::services::{
    docker_project_tasks, docker_stats, system_monitoring as monitoring_service, task_scheduler,
    websocket::create_channel,
};
use crate::state::AppState;
use anyhow::Result;
use axum::extract::connect_info::ConnectInfo;
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::info_span;
use vust_contracts::api::ErrorCode;

/// 仅允许 Controller principal 访问的管理路由。
fn controller_api_router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/docker", docker::docker_router())
        .nest("/disks", disks::disk_router())
        .nest("/files", fs::fs_router())
        .nest("/firewall", firewall::firewall_router())
        .nest("/terminal", host_terminal::host_terminal_router())
        .nest("/runtime-logs", runtime_logs::runtime_log_router())
        .nest("/system", system::system_router())
        .nest(
            "/system-monitoring",
            system_monitoring::system_monitoring_router(),
        )
        .nest("/scheduled-tasks", scheduled_tasks::scheduled_task_router())
        .nest("/script-runs", script_runs::script_run_router())
        .nest("/upgrade", upgrade::upgrade_router())
        .nest("/websocket", websocket::websocket_router())
        .merge(process::process_router())
}

/// 按 principal 边界组装 Agent API，避免通过 URI 字符串推断授权方式。
fn state_api_router(state: Arc<AppState>) -> Router {
    let controller_routes = controller_api_router().layer(axum::middleware::from_fn_with_state(
        Arc::clone(&state),
        require_command_credential,
    ));
    Router::new()
        .route("/runtime/health", get(runtime_health))
        .merge(controller_routes)
        .merge(suite_operation_logs::router())
        .merge(suite_workloads::suite_workloads_router())
        .with_state(state)
}

/// 无状态健康检查；启动期不暴露任何管理能力。
async fn runtime_health() -> StatusCode {
    StatusCode::OK
}

/// 创建路由
pub async fn create_router() -> Result<(
    Router,
    DbPool,
    Arc<crate::services::controller_runtime::ControllerRuntime>,
)> {
    // 建立数据库连接池。
    let pool = db::establish_connection().await?;
    let pool_for_shutdown = pool.clone();
    tracing::info!("Database connection pool established successfully.");
    let system_monitoring =
        Arc::new(monitoring_service::SystemMonitoringRuntime::load(&pool).await?);
    let process_manager = Arc::new(crate::services::process_manager::ProcessManagerRuntime::new());
    let controller_runtime =
        Arc::new(crate::services::controller_runtime::ControllerRuntime::new());
    docker_project_tasks::initialize(&pool).await?;
    crate::services::file_tasks::initialize(&pool).await?;
    crate::services::process_manager::initialize_signal_operations(&pool).await?;

    // 初始化 Docker 客户端，连接失败时仅记录日志
    let (docker, docker_status) = AppState::init_docker_state().await;

    let websocket_sender = create_channel();

    // 创建共享的应用状态
    let app_state = Arc::new(AppState {
        server_name: "VUST".to_string(),
        docker: tokio::sync::RwLock::new(docker),
        docker_status: tokio::sync::RwLock::new(docker_status),
        system_monitoring: Arc::clone(&system_monitoring),
        process_manager: Arc::clone(&process_manager),
        controller_runtime: Arc::clone(&controller_runtime),
        metadata_db: pool,
        websocket_sender,
        running_task_ids: tokio::sync::Mutex::new(std::collections::HashSet::new()),
    });

    docker_stats::spawn_stats_collector(Arc::clone(&app_state));
    crate::services::database_maintenance::spawn_worker(app_state.metadata_db.clone());
    docker_project_tasks::spawn_retention_worker(app_state.metadata_db.clone());
    crate::services::file_transfers::spawn_retention_worker(app_state.metadata_db.clone());
    monitoring_service::spawn_sampler(app_state.metadata_db.clone(), system_monitoring);
    process_manager
        .refresh_process_snapshot(&app_state)
        .await
        .map_err(anyhow::Error::msg)?;
    process_manager
        .refresh_network_snapshot()
        .await
        .map_err(anyhow::Error::msg)?;
    process_manager.spawn_samplers(Arc::clone(&app_state));
    task_scheduler::spawn_scheduler(Arc::clone(&app_state));
    crate::services::script_runs::recover(&app_state)
        .await
        .map_err(|error| anyhow::anyhow!("failed to recover script runs: {error:?}"))?;
    crate::services::disk_operations::recover(&app_state)
        .await
        .map_err(|error| anyhow::anyhow!("failed to recover disk operations: {error:?}"))?;

    // 配置 CORS：允许所有来源、常用方法
    let cors = CorsLayer::new()
        .allow_origin(Any) // 你也可以用 .allow_origin("https://example.com".parse().unwrap())
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any);

    // 创建一个需要状态的路由实例，并立即为其提供状态
    let state_routes = state_api_router(Arc::clone(&app_state));

    let router = Router::new()
        .nest("/api/v1/agent", state_routes)
        .layer(cors)
        .layer(axum::middleware::from_fn(
            vust_api::logging::http_log_middleware,
        ))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                // 尝试从 request 的 extensions 中获取 ConnectInfo
                let client_ip = request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|ConnectInfo(addr)| addr.to_string())
                    .unwrap_or_else(|| "unix_socket".to_string());

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

    Ok((router, pool_for_shutdown, controller_runtime))
}

/// 校验 Master→Agent 管理请求；该中间件只挂载于 Controller 路由。
async fn require_command_credential(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let expected = crate::services::settings::get_string_or_default(
        &state.metadata_db,
        "runtime.command_credential_hash",
        "",
    )
    .await
    .unwrap_or_default();
    if expected.is_empty() {
        return crate::types::ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::AgentUnavailable,
            "Agent runtime registration is not active",
        )
        .into_response();
    }
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();
    let actual = hex::encode(ring::digest::digest(
        &ring::digest::SHA256,
        token.as_bytes(),
    ));
    if actual != expected {
        return crate::types::ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "Agent command credential is invalid",
        )
        .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn principal_routes_use_independent_authorization() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        let controller_token = "controller-command-token";
        let controller_hash = hex::encode(ring::digest::digest(
            &ring::digest::SHA256,
            controller_token.as_bytes(),
        ));
        crate::services::settings::set_string(
            &state.metadata_db,
            "runtime.command_credential_hash",
            &controller_hash,
        )
        .await
        .unwrap();

        let app = Router::new().nest("/api/v1/agent", state_api_router(Arc::clone(&state)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::new();

        let suite_response = client
            .get(format!(
                "http://{address}/api/v1/agent/suite-runtime/workloads"
            ))
            .bearer_auth("invalid-suite-token")
            .send()
            .await
            .unwrap();
        assert_eq!(suite_response.status(), StatusCode::FORBIDDEN);
        assert!(
            suite_response
                .text()
                .await
                .unwrap()
                .contains("suite runtime token is invalid")
        );

        let controller_response = client
            .get(format!("http://{address}/api/v1/agent/system/summary"))
            .bearer_auth("invalid-suite-token")
            .send()
            .await
            .unwrap();
        assert_eq!(controller_response.status(), StatusCode::FORBIDDEN);
        assert!(
            controller_response
                .text()
                .await
                .unwrap()
                .contains("Agent command credential is invalid")
        );

        let health_response = client
            .get(format!("http://{address}/api/v1/agent/runtime/health"))
            .send()
            .await
            .unwrap();
        assert_eq!(health_response.status(), StatusCode::OK);
        server.abort();
    }
}
