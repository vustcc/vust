//! Agent 在线升级 API：暴露本地升级状态、准备、应用与回滚接口。

use crate::services::upgrade::{
    self, UpgradeApplyRequest, UpgradePrepareRequest, UpgradeRollbackRequest,
};
use crate::types::{ApiResponse, ApiResult};
use axum::{
    Json, Router,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::sync::Arc;

/// 返回当前升级状态。
pub async fn status() -> ApiResult<Response> {
    let state = upgrade::status().await?;
    Ok(ApiResponse::success_with_raw("Agent upgrade status loaded", Some(state)).into_response())
}

/// 下载并校验目标升级二进制。
pub async fn prepare(Json(payload): Json<UpgradePrepareRequest>) -> ApiResult<Response> {
    let state = upgrade::prepare(payload).await?;
    Ok(ApiResponse::success_with_raw("Agent upgrade prepared", Some(state)).into_response())
}

/// 应用已准备好的升级二进制。
pub async fn apply(Json(payload): Json<UpgradeApplyRequest>) -> ApiResult<Response> {
    let state = upgrade::apply(payload).await?;
    Ok(ApiResponse::success_with_raw("Agent upgrade applied", Some(state)).into_response())
}

/// 回滚到升级前备份二进制。
pub async fn rollback(Json(payload): Json<UpgradeRollbackRequest>) -> ApiResult<Response> {
    let state = upgrade::rollback(payload).await?;
    Ok(
        ApiResponse::success_with_raw("Agent upgrade rollback submitted", Some(state))
            .into_response(),
    )
}

/// Agent 在线升级路由。
pub fn upgrade_router() -> Router<Arc<crate::state::AppState>> {
    Router::new()
        .route("/status", get(status))
        .route("/prepare", post(prepare))
        .route("/apply", post(apply))
        .route("/rollback", post(rollback))
}
