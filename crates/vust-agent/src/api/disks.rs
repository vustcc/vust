//! Agent 磁盘内部 API：提供可信事实清单和按需详情。

use crate::services::{disk_inventory, disk_operations};
use crate::state::AppState;
use crate::types::{ApiResponse, ApiResult};
use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, State},
    http::{HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::sync::Arc;
use vust_contracts::{api::ErrorCode, disks::AgentCreateDiskOperationRequest};

const ACTOR_KIND_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-kind");
const ACTOR_NAME_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-name");

/// 仅接受 Master 内部链路注入的可信操作上下文。
struct TrustedOperationContext;

impl<S> FromRequestParts<S> for TrustedOperationContext
where
    S: Send + Sync,
{
    type Rejection = crate::types::ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let kind = parts
            .headers
            .get(&ACTOR_KIND_HEADER)
            .and_then(|value| value.to_str().ok());
        let name = parts
            .headers
            .get(&ACTOR_NAME_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if matches!(kind, Some("user" | "system")) && name.is_some() {
            Ok(Self)
        } else {
            Err(crate::types::ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "trusted operation context is required",
            ))
        }
    }
}

/// 返回节点磁盘摘要。
async fn inventory(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let value = disk_inventory::inventory(&state.metadata_db).await?;
    Ok(ApiResponse::success_with_raw("Disk inventory loaded", value).into_response())
}

/// 返回单块磁盘详情。
async fn detail(
    State(state): State<Arc<AppState>>,
    Path(disk_id): Path<String>,
) -> ApiResult<Response> {
    let value = disk_inventory::detail(&state.metadata_db, &disk_id).await?;
    Ok(ApiResponse::success_with_raw("Disk detail loaded", value).into_response())
}

/// 幂等创建磁盘后台操作。
async fn create_operation(
    State(state): State<Arc<AppState>>,
    _trusted: TrustedOperationContext,
    Json(request): Json<AgentCreateDiskOperationRequest>,
) -> ApiResult<Response> {
    let value = disk_operations::submit(state, request).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Disk operation accepted",
            value,
        )),
    )
        .into_response())
}

/// 返回磁盘后台操作状态。
async fn operation(
    State(state): State<Arc<AppState>>,
    Path(operation_id): Path<String>,
) -> ApiResult<Response> {
    let value = disk_operations::get(&state, &operation_id).await?;
    Ok(ApiResponse::success_with_raw("Disk operation loaded", value).into_response())
}

/// 请求取消仍处于安全阶段的操作。
async fn cancel_operation(
    State(state): State<Arc<AppState>>,
    _trusted: TrustedOperationContext,
    Path(operation_id): Path<String>,
) -> ApiResult<Response> {
    let value = disk_operations::cancel(&state, &operation_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Disk operation cancellation accepted",
            value,
        )),
    )
        .into_response())
}

/// 构建磁盘内部路由。
pub fn disk_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(inventory))
        .route("/operations", post(create_operation))
        .route("/operations/{operation_id}", get(operation))
        .route("/operations/{operation_id}/cancel", post(cancel_operation))
        .route("/{disk_id}", get(detail))
}
