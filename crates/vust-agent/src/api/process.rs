//! 进程与网络观察内部 HTTP API，仅供 Master 语义网关调用。

use crate::{
    services::process_manager::{ProcessManagerRuntime, SignalActorContext},
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::sync::Arc;
use vust_contracts::{
    api::ErrorCode,
    process::{
        NetworkConnectionListQuery, ProcessActionRequest, ProcessListQuery, ProcessSignal,
        ProcessSignalDeliveryStatus, ProcessSignalResult,
    },
};

const ACTOR_KIND_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-kind");
const ACTOR_NAME_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-name");
const CLIENT_IP_HEADER: HeaderName = HeaderName::from_static("x-vust-client-ip");
const TRACE_ID_HEADER: HeaderName = HeaderName::from_static("x-vust-trace-id");

/// 注册进程与网络内部领域路由。
pub fn process_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/processes/list", get(list_processes))
        .route("/network-connections/list", get(list_network_connections))
        .route("/process/{process_id}/terminate", post(terminate))
        .route("/process/{process_id}/force-kill", post(force_kill))
}

async fn list_processes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProcessListQuery>,
) -> ApiResult<Response> {
    let page = state.process_manager.list_processes(query).await?;
    Ok(ApiResponse::success_with_raw("Process list loaded", Some(page)).into_response())
}

async fn list_network_connections(
    State(state): State<Arc<AppState>>,
    Query(query): Query<NetworkConnectionListQuery>,
) -> ApiResult<Response> {
    let page = state
        .process_manager
        .list_network_connections(query)
        .await?;
    Ok(ApiResponse::success_with_raw("Network connection list loaded", Some(page)).into_response())
}

async fn terminate(
    State(state): State<Arc<AppState>>,
    Path(process_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<ProcessActionRequest>,
) -> ApiResult<Response> {
    if payload.confirmation_token.is_some() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "terminate does not accept a confirmation token",
        ));
    }
    deliver(
        &state.process_manager,
        &state,
        &process_id,
        ProcessSignal::Term,
        &payload,
        &headers,
    )
    .await
}

async fn force_kill(
    State(state): State<Arc<AppState>>,
    Path(process_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<ProcessActionRequest>,
) -> ApiResult<Response> {
    if payload
        .confirmation_token
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            ErrorCode::ProcessConfirmationRequired,
            "force-kill confirmation is required",
        ));
    }
    deliver(
        &state.process_manager,
        &state,
        &process_id,
        ProcessSignal::Kill,
        &payload,
        &headers,
    )
    .await
}

async fn deliver(
    runtime: &ProcessManagerRuntime,
    state: &AppState,
    process_id: &str,
    signal: ProcessSignal,
    payload: &ProcessActionRequest,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let actor = trusted_actor(headers)?;
    let result = runtime
        .deliver_signal(
            &state.metadata_db,
            process_id,
            signal,
            &payload.idempotency_key,
            &actor,
        )
        .await?;
    Ok(signal_response(result))
}

fn signal_response(result: ProcessSignalResult) -> Response {
    let status = if result.status == ProcessSignalDeliveryStatus::OutcomeUnknown {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    let mut response =
        ApiResponse::success_with_raw("Process signal processed", Some(result)).into_response();
    *response.status_mut() = status;
    response
}

fn trusted_actor(headers: &HeaderMap) -> ApiResult<SignalActorContext> {
    let value = |name: &HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    };
    if value(&ACTOR_KIND_HEADER).as_deref() != Some("user") {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "trusted operation context is required",
        ));
    }
    Ok(SignalActorContext {
        actor_name: value(&ACTOR_NAME_HEADER).ok_or_else(|| {
            ApiError::forbidden(ErrorCode::AuthForbidden, "trusted actor name is required")
        })?,
        client_ip: value(&CLIENT_IP_HEADER).ok_or_else(|| {
            ApiError::forbidden(ErrorCode::AuthForbidden, "trusted client IP is required")
        })?,
        trace_id: value(&TRACE_ID_HEADER).ok_or_else(|| {
            ApiError::forbidden(ErrorCode::AuthForbidden, "trusted trace ID is required")
        })?,
    })
}

#[cfg(test)]
mod tests {
    use super::trusted_actor;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn mutations_require_complete_trusted_context() {
        assert!(trusted_actor(&HeaderMap::new()).is_err());
        let mut headers = HeaderMap::new();
        headers.insert("x-vust-actor-kind", HeaderValue::from_static("user"));
        headers.insert("x-vust-actor-name", HeaderValue::from_static("admin"));
        headers.insert("x-vust-client-ip", HeaderValue::from_static("192.0.2.1"));
        headers.insert("x-vust-trace-id", HeaderValue::from_static("trace-1"));
        assert!(trusted_actor(&headers).is_ok());
    }
}
