//! 操作日志 API：只读查询摘要与按需详情。

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use vust_contracts::logging::OperationLogQuery;

use crate::{
    services::logging,
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};

/// 查询操作日志。
async fn query(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<OperationLogQuery>,
) -> ApiResult<Response> {
    let page = logging::query_operation_logs(&state.metadata_db, payload).await?;
    Ok(ApiResponse::success_with_raw("Operation logs loaded", Some(page)).into_response())
}

/// 查询操作日志安全详情；不存在与不可见统一返回 404。
async fn detail(
    State(state): State<Arc<AppState>>,
    Path(event_id): Path<String>,
) -> ApiResult<Response> {
    let detail = logging::get_operation_log(&state.metadata_db, &event_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                vust_contracts::api::ErrorCode::OperationLogNotFound,
                "operation log not found",
            )
        })?;
    Ok(ApiResponse::success_with_raw("Operation log loaded", Some(detail)).into_response())
}

/// 操作日志只读路由。
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/query", post(query))
        .route("/{event_id}", get(detail))
}
