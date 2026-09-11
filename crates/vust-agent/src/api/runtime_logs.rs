//! 运行日志 API：供 VUST 主控按节点读取 Agent 轮转日志。

use crate::services::runtime_logs;
use crate::types::{ApiResponse, ApiResult};
use axum::{
    Json, Router,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use vust_contracts::runtime_logs::RuntimeLogQuery;

pub fn runtime_log_router() -> Router<std::sync::Arc<crate::state::AppState>> {
    Router::new()
        .route("/files", get(runtime_log_files))
        .route("/query", post(query_runtime_logs))
}

/// 查询当前 Agent 的运行日志文件列表。
pub async fn runtime_log_files() -> ApiResult<Response> {
    let files = runtime_logs::list_runtime_log_files(Some("agent")).await?;
    Ok(
        ApiResponse::success_with_raw("Agent runtime log files loaded", Some(files))
            .into_response(),
    )
}

/// 查询当前 Agent 的运行日志文件内容。
pub async fn query_runtime_logs(Json(payload): Json<RuntimeLogQuery>) -> ApiResult<Response> {
    let result = runtime_logs::query_runtime_logs(payload).await?;
    Ok(ApiResponse::success_with_raw("Agent runtime logs loaded", Some(result)).into_response())
}
