//! 进程管理语义网关：节点绑定、共享采样查询、信号确认、幂等与操作日志。

use crate::{
    api::auth::AuthenticatedAdmin,
    models::{
        logging::{LogModule, LogStatus, PlatformLogLevel},
        node_runtime_client::{AgentOperationContext, NodeRuntimeClient},
        nodes::{self, NodeStatus},
    },
    services::{
        logging::{self, OperationEventBuilder},
        node_state_machine,
    },
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Duration, SecondsFormat, Utc};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::json;
use sqlx::{FromRow, SqlitePool};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, LazyLock, Mutex},
};
use uuid::Uuid;
use vust_contracts::{
    api::ErrorCode,
    process::{
        NetworkConnectionListPage, NetworkConnectionListQuery, NetworkConnectionState,
        NetworkProtocol, NetworkSortBy, ProcessActionRequest, ProcessForceKillConfirmation,
        ProcessListPage, ProcessListQuery, ProcessSignal, ProcessSignalDeliveryStatus,
        ProcessSignalResult, ProcessSortBy, ProcessState, SortOrder,
    },
};

const AGENT_BASE_PATH: &str = "/api/v1/agent";
const CONFIRMATION_TTL_SECONDS: i64 = 60;
const CONFIRMATION_TOKEN_BYTES: usize = 32;

static FORCE_KILL_CONFIRMATIONS: LazyLock<Mutex<HashMap<String, ConfirmationGrant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
struct ConfirmationGrant {
    session_id: String,
    node_id: String,
    process_id: String,
    expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct StoredProcessAction {
    idempotency_key: String,
    user_id: i64,
    session_id: String,
    node_id: String,
    process_id: String,
    signal: String,
    status: String,
    pid: Option<i64>,
    process_name: Option<String>,
    delivered_at: Option<String>,
    error_code: Option<String>,
    error_summary: Option<String>,
}

/// 构建 node-scoped 进程与网络公共路由。
pub fn process_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{node_id}/processes/list", get(list_processes))
        .route(
            "/{node_id}/network-connections/list",
            get(list_network_connections),
        )
        .route("/{node_id}/process/{process_id}/terminate", post(terminate))
        .route(
            "/{node_id}/process/{process_id}/force-kill-confirmation",
            post(create_force_kill_confirmation),
        )
        .route(
            "/{node_id}/process/{process_id}/force-kill",
            post(force_kill),
        )
}

async fn list_processes(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    Query(query): Query<ProcessListQuery>,
) -> ApiResult<Response> {
    let (client, _) = node_client(&state, &node_id).await?;
    let data: ProcessListPage = client.get_domain(&process_query_path(&query)?).await?;
    Ok(ApiResponse::success_with_raw("Process list loaded", Some(data)).into_response())
}

async fn list_network_connections(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    Query(query): Query<NetworkConnectionListQuery>,
) -> ApiResult<Response> {
    let (client, _) = node_client(&state, &node_id).await?;
    let data: NetworkConnectionListPage = client.get_domain(&network_query_path(&query)?).await?;
    Ok(ApiResponse::success_with_raw("Network connection list loaded", Some(data)).into_response())
}

async fn terminate(
    State(state): State<Arc<AppState>>,
    Path((node_id, process_id)): Path<(String, String)>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(request): Json<ProcessActionRequest>,
) -> ApiResult<Response> {
    if request.confirmation_token.is_some() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "terminate does not accept a confirmation token",
        ));
    }
    execute_signal(
        &state,
        &admin,
        &headers,
        &node_id,
        &process_id,
        ProcessSignal::Term,
        request,
    )
    .await
}

async fn create_force_kill_confirmation(
    State(state): State<Arc<AppState>>,
    Path((node_id, process_id)): Path<(String, String)>,
    admin: AuthenticatedAdmin,
) -> ApiResult<Response> {
    validate_process_id(&process_id)?;
    let _ = node_client(&state, &node_id).await?;
    let expires_at = Utc::now() + Duration::seconds(CONFIRMATION_TTL_SECONDS);
    let token = generate_confirmation_token()?;
    let mut grants = FORCE_KILL_CONFIRMATIONS
        .lock()
        .map_err(|_| ApiError::internal("force-kill confirmation store is unavailable"))?;
    grants.retain(|_, grant| grant.expires_at > Utc::now());
    grants.insert(
        hash_confirmation_token(&token),
        ConfirmationGrant {
            session_id: admin.session.id.clone(),
            node_id,
            process_id,
            expires_at,
        },
    );
    let confirmation = ProcessForceKillConfirmation {
        confirmation_token: token,
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    Ok(
        ApiResponse::success_with_raw("Force-kill confirmation created", Some(confirmation))
            .into_response(),
    )
}

async fn force_kill(
    State(state): State<Arc<AppState>>,
    Path((node_id, process_id)): Path<(String, String)>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(request): Json<ProcessActionRequest>,
) -> ApiResult<Response> {
    execute_signal(
        &state,
        &admin,
        &headers,
        &node_id,
        &process_id,
        ProcessSignal::Kill,
        request,
    )
    .await
}

async fn execute_signal(
    state: &AppState,
    admin: &AuthenticatedAdmin,
    headers: &HeaderMap,
    node_id: &str,
    process_id: &str,
    signal: ProcessSignal,
    request: ProcessActionRequest,
) -> ApiResult<Response> {
    validate_process_id(process_id)?;
    validate_idempotency_key(&request.idempotency_key)?;
    if let Some(existing) = load_action(&state.metadata_db, &request.idempotency_key).await? {
        return stored_action_response(existing, admin, node_id, process_id, signal);
    }

    let context = operation_context(admin, headers)?;
    let client_ip = context.client_ip.parse::<IpAddr>().map_err(|_| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session has an invalid trusted client IP",
        )
    })?;
    let (client, _) = node_client(state, node_id).await?;
    let inserted = insert_action(
        &state.metadata_db,
        admin,
        node_id,
        process_id,
        signal,
        &request.idempotency_key,
        &context,
    )
    .await?;
    if !inserted {
        let existing = load_action(&state.metadata_db, &request.idempotency_key)
            .await?
            .ok_or_else(|| ApiError::internal("process idempotency record disappeared"))?;
        return stored_action_response(existing, admin, node_id, process_id, signal);
    }

    if signal == ProcessSignal::Kill {
        let confirmation = request.confirmation_token.as_deref().ok_or_else(|| {
            ApiError::new(
                StatusCode::PRECONDITION_REQUIRED,
                ErrorCode::ProcessConfirmationRequired,
                "force-kill confirmation is required",
            )
        });
        let confirmation_result = confirmation
            .and_then(|token| consume_confirmation(token, &admin.session.id, node_id, process_id));
        if let Err(error) = confirmation_result {
            finish_action_failure(&state.metadata_db, &request.idempotency_key, &error).await?;
            record_signal_log(
                state,
                admin,
                client_ip,
                &context.trace_id,
                node_id,
                process_id,
                signal,
                None,
                None,
                "failed",
                Some(error.code),
            );
            return Err(error);
        }
    }

    let agent_path = format!(
        "{AGENT_BASE_PATH}/process/{process_id}/{}",
        match signal {
            ProcessSignal::Term => "terminate",
            ProcessSignal::Kill => "force-kill",
        }
    );
    let forwarded = ProcessActionRequest {
        idempotency_key: request.idempotency_key.clone(),
        confirmation_token: request.confirmation_token,
    };
    let result = client
        .post_domain_with_operation_context::<ProcessSignalResult, _>(
            &agent_path,
            &forwarded,
            &context,
        )
        .await;
    match result {
        Ok(result) => {
            finish_action_success(&state.metadata_db, &result).await?;
            record_signal_log(
                state,
                admin,
                client_ip,
                &context.trace_id,
                node_id,
                process_id,
                signal,
                result.pid,
                result.process_name.as_deref(),
                delivery_status_key(result.status),
                None,
            );
            Ok(signal_response(result))
        }
        Err(error) if is_uncertain_delivery_error(&error) => {
            let result = ProcessSignalResult {
                idempotency_key: request.idempotency_key,
                process_id: process_id.to_string(),
                pid: None,
                process_name: None,
                signal,
                status: ProcessSignalDeliveryStatus::OutcomeUnknown,
                delivered_at: None,
                error_summary: Some("signal delivery outcome is unknown".to_string()),
            };
            finish_action_success(&state.metadata_db, &result).await?;
            record_signal_log(
                state,
                admin,
                client_ip,
                &context.trace_id,
                node_id,
                process_id,
                signal,
                None,
                None,
                "outcomeUnknown",
                Some(error.code),
            );
            Ok(signal_response(result))
        }
        Err(error) => {
            finish_action_failure(&state.metadata_db, &request.idempotency_key, &error).await?;
            record_signal_log(
                state,
                admin,
                client_ip,
                &context.trace_id,
                node_id,
                process_id,
                signal,
                None,
                None,
                "failed",
                Some(error.code),
            );
            Err(error)
        }
    }
}

fn signal_response(result: ProcessSignalResult) -> Response {
    let status = if result.status == ProcessSignalDeliveryStatus::OutcomeUnknown {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    ApiResponse::success("Process signal processed", Some(result), status.as_u16()).into_response()
}

fn stored_action_response(
    stored: StoredProcessAction,
    admin: &AuthenticatedAdmin,
    node_id: &str,
    process_id: &str,
    signal: ProcessSignal,
) -> ApiResult<Response> {
    if stored.user_id != admin.id
        || stored.session_id != admin.session.id
        || stored.node_id != node_id
        || stored.process_id != process_id
        || stored.signal != signal_key(signal)
    {
        return Err(ApiError::conflict(
            ErrorCode::ProcessOperationConflict,
            "idempotencyKey is already bound to another process operation",
        ));
    }
    if stored.status == "failed" {
        return Err(stored_action_error(&stored));
    }
    let status = match stored.status.as_str() {
        "delivered" => ProcessSignalDeliveryStatus::Delivered,
        _ => ProcessSignalDeliveryStatus::OutcomeUnknown,
    };
    Ok(signal_response(ProcessSignalResult {
        idempotency_key: stored.idempotency_key,
        process_id: stored.process_id,
        pid: stored.pid.and_then(|value| u32::try_from(value).ok()),
        process_name: stored.process_name,
        signal,
        status,
        delivered_at: stored.delivered_at,
        error_summary: stored.error_summary,
    }))
}

async fn insert_action(
    pool: &SqlitePool,
    admin: &AuthenticatedAdmin,
    node_id: &str,
    process_id: &str,
    signal: ProcessSignal,
    idempotency_key: &str,
    context: &AgentOperationContext,
) -> ApiResult<bool> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let result = sqlx::query(
        "INSERT OR IGNORE INTO process_action_requests (idempotency_key, user_id, actor_name, \
         session_id, node_id, process_id, signal, status, client_ip, trace_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'submitting', ?8, ?9, ?10, ?10)",
    )
    .bind(idempotency_key)
    .bind(admin.id)
    .bind(&admin.username)
    .bind(&admin.session.id)
    .bind(node_id)
    .bind(process_id)
    .bind(signal_key(signal))
    .bind(&context.client_ip)
    .bind(&context.trace_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(result.rows_affected() == 1)
}

async fn load_action(
    pool: &SqlitePool,
    idempotency_key: &str,
) -> ApiResult<Option<StoredProcessAction>> {
    sqlx::query_as::<_, StoredProcessAction>(
        "SELECT idempotency_key, user_id, session_id, node_id, process_id, signal, status, pid, \
         process_name, delivered_at, error_code, error_summary FROM process_action_requests \
         WHERE idempotency_key = ?1",
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))
}

async fn finish_action_success(pool: &SqlitePool, result: &ProcessSignalResult) -> ApiResult<()> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query(
        "UPDATE process_action_requests SET status = ?2, pid = ?3, process_name = ?4, \
         delivered_at = ?5, error_summary = ?6, updated_at = ?7 WHERE idempotency_key = ?1",
    )
    .bind(&result.idempotency_key)
    .bind(match result.status {
        ProcessSignalDeliveryStatus::Delivered => "delivered",
        ProcessSignalDeliveryStatus::OutcomeUnknown => "outcome_unknown",
        ProcessSignalDeliveryStatus::Failed => "failed",
    })
    .bind(result.pid.map(i64::from))
    .bind(&result.process_name)
    .bind(&result.delivered_at)
    .bind(&result.error_summary)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

async fn finish_action_failure(
    pool: &SqlitePool,
    idempotency_key: &str,
    error: &ApiError,
) -> ApiResult<()> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    sqlx::query(
        "UPDATE process_action_requests SET status = 'failed', error_code = ?2, \
         error_summary = ?3, updated_at = ?4 WHERE idempotency_key = ?1",
    )
    .bind(idempotency_key)
    .bind(error.code.as_str())
    .bind(error.message.as_ref())
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

fn stored_action_error(stored: &StoredProcessAction) -> ApiError {
    let message = stored
        .error_summary
        .clone()
        .unwrap_or_else(|| "process signal delivery failed".to_string());
    match stored.error_code.as_deref() {
        Some("PROCESS_NOT_FOUND") => ApiError::not_found(ErrorCode::ProcessNotFound, message),
        Some("PROCESS_CHANGED") => ApiError::conflict(ErrorCode::ProcessChanged, message),
        Some("PROCESS_OPERATION_CONFLICT") => {
            ApiError::conflict(ErrorCode::ProcessOperationConflict, message)
        }
        Some("PROCESS_PERMISSION_DENIED") => {
            ApiError::forbidden(ErrorCode::ProcessPermissionDenied, message)
        }
        Some("PROCESS_CONFIRMATION_REQUIRED") => ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            ErrorCode::ProcessConfirmationRequired,
            message,
        ),
        Some("PROCESS_CONFIRMATION_INVALID") => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ProcessConfirmationInvalid,
            message,
        ),
        _ => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProcessSignalUnavailable,
            message,
        ),
    }
}

fn generate_confirmation_token() -> ApiResult<String> {
    let mut bytes = [0_u8; CONFIRMATION_TOKEN_BYTES];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ApiError::internal("failed to generate force-kill confirmation"))?;
    Ok(hex::encode(bytes))
}

fn hash_confirmation_token(token: &str) -> String {
    hex::encode(digest::digest(&digest::SHA256, token.as_bytes()).as_ref())
}

fn consume_confirmation(
    token: &str,
    session_id: &str,
    node_id: &str,
    process_id: &str,
) -> ApiResult<()> {
    let mut grants = FORCE_KILL_CONFIRMATIONS
        .lock()
        .map_err(|_| ApiError::internal("force-kill confirmation store is unavailable"))?;
    let now = Utc::now();
    grants.retain(|_, grant| grant.expires_at > now);
    let grant = grants
        .remove(&hash_confirmation_token(token))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ProcessConfirmationInvalid,
                "force-kill confirmation is invalid or expired",
            )
        })?;
    if grant.session_id != session_id
        || grant.node_id != node_id
        || grant.process_id != process_id
        || grant.expires_at <= now
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ProcessConfirmationInvalid,
            "force-kill confirmation does not match this operation",
        ));
    }
    Ok(())
}

fn validate_process_id(process_id: &str) -> ApiResult<()> {
    if process_id.len() != 64 || !process_id.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ProcessInvalidId,
            "invalid process identity",
        ));
    }
    Ok(())
}

fn validate_idempotency_key(idempotency_key: &str) -> ApiResult<()> {
    if Uuid::parse_str(idempotency_key).is_err() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "idempotencyKey must be a UUID",
        ));
    }
    Ok(())
}

fn is_uncertain_delivery_error(error: &ApiError) -> bool {
    matches!(
        error.code,
        ErrorCode::AgentTimeout
            | ErrorCode::AgentRefused
            | ErrorCode::AgentRequestFailed
            | ErrorCode::AgentUnavailable
            | ErrorCode::InternalServerError
            | ErrorCode::DatabaseError
    )
}

#[allow(clippy::too_many_arguments)]
fn record_signal_log(
    state: &AppState,
    admin: &AuthenticatedAdmin,
    client_ip: IpAddr,
    trace_id: &str,
    node_id: &str,
    process_id: &str,
    signal: ProcessSignal,
    pid: Option<u32>,
    process_name: Option<&str>,
    result: &str,
    error_code: Option<ErrorCode>,
) {
    let delivered = result == "delivered";
    let (status, level) = if !delivered && result != "outcomeUnknown" {
        (LogStatus::Failed, PlatformLogLevel::Error)
    } else if signal == ProcessSignal::Kill || result == "outcomeUnknown" {
        (LogStatus::Success, PlatformLogLevel::Warning)
    } else {
        (LogStatus::Success, PlatformLogLevel::Info)
    };
    OperationEventBuilder::new(
        &admin.username,
        match signal {
            ProcessSignal::Term => "process_signal_terminate",
            ProcessSignal::Kill => "process_signal_force_kill",
        },
        client_ip,
    )
    .user_id(admin.id)
    .module(LogModule::Process)
    .target_type("process")
    .target_id(process_id)
    .trace_id(trace_id)
    .source("vust_api")
    .request("POST", "/api/v1/node/{node_id}/process/{process_id}/signal")
    .metadata(json!({
        "nodeId": node_id,
        "processId": process_id,
        "pid": pid,
        "processName": process_name,
        "signal": signal_key(signal),
        "result": result,
        "errorCode": error_code.map(ErrorCode::as_str),
    }))
    .status(status)
    .level(level)
    .finish(&state.metadata_db);
}

async fn node_client(state: &AppState, node_id: &str) -> ApiResult<(NodeRuntimeClient, String)> {
    let name = if node_id == "local" {
        "Local Node".to_string()
    } else {
        let node = nodes::get_node_by_id(&state.metadata_db, node_id)
            .await
            .map_err(|error| ApiError::database(error.to_string()))?
            .ok_or_else(|| ApiError::not_found(ErrorCode::NodeNotFound, "node does not exist"))?;
        let status = NodeStatus::parse(&node.status)
            .ok_or_else(|| ApiError::internal("node has an invalid lifecycle status"))?;
        if !node_state_machine::is_proxyable(status) {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::NodeUnavailable,
                "node is not available for process operations",
            ));
        }
        node.name
    };
    Ok((
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?,
        name,
    ))
}

fn operation_context(
    admin: &AuthenticatedAdmin,
    headers: &HeaderMap,
) -> ApiResult<AgentOperationContext> {
    let client_ip = admin.session.client_ip.clone().ok_or_else(|| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session is missing a trusted client IP",
        )
    })?;
    Ok(AgentOperationContext {
        actor_user_id: admin.id,
        actor_name: admin.username.clone(),
        client_ip,
        trace_id: logging::resolve_trace_id(headers),
    })
}

fn process_query_path(query: &ProcessListQuery) -> ApiResult<String> {
    let mut pairs = Vec::new();
    push_option(&mut pairs, "query", query.query.as_ref().cloned());
    push_option(&mut pairs, "status", query.status.map(process_state_key));
    push_option(
        &mut pairs,
        "page",
        query.page.map(|value| value.to_string()),
    );
    push_option(
        &mut pairs,
        "pageSize",
        query.page_size.map(|value| value.to_string()),
    );
    push_option(&mut pairs, "sortBy", query.sort_by.map(process_sort_key));
    push_option(
        &mut pairs,
        "sortOrder",
        query.sort_order.map(sort_order_key),
    );
    agent_query_path("/processes/list", &pairs)
}

fn network_query_path(query: &NetworkConnectionListQuery) -> ApiResult<String> {
    let mut pairs = Vec::new();
    push_option(&mut pairs, "query", query.query.as_ref().cloned());
    push_option(&mut pairs, "state", query.state.map(network_state_key));
    push_option(
        &mut pairs,
        "protocol",
        query.protocol.map(network_protocol_key),
    );
    push_option(
        &mut pairs,
        "page",
        query.page.map(|value| value.to_string()),
    );
    push_option(
        &mut pairs,
        "pageSize",
        query.page_size.map(|value| value.to_string()),
    );
    push_option(&mut pairs, "sortBy", query.sort_by.map(network_sort_key));
    push_option(
        &mut pairs,
        "sortOrder",
        query.sort_order.map(sort_order_key),
    );
    agent_query_path("/network-connections/list", &pairs)
}

fn push_option(pairs: &mut Vec<(&'static str, String)>, key: &'static str, value: Option<String>) {
    if let Some(value) = value {
        pairs.push((key, value));
    }
}

fn agent_query_path(suffix: &str, pairs: &[(&str, String)]) -> ApiResult<String> {
    let mut url = reqwest::Url::parse(&format!("http://agent{AGENT_BASE_PATH}{suffix}"))
        .map_err(|_| ApiError::internal("failed to build Agent process URL"))?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
    }
    let query = url
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    Ok(format!("{}{query}", url.path()))
}

fn signal_key(signal: ProcessSignal) -> &'static str {
    match signal {
        ProcessSignal::Term => "term",
        ProcessSignal::Kill => "kill",
    }
}

fn delivery_status_key(status: ProcessSignalDeliveryStatus) -> &'static str {
    match status {
        ProcessSignalDeliveryStatus::Delivered => "delivered",
        ProcessSignalDeliveryStatus::Failed => "failed",
        ProcessSignalDeliveryStatus::OutcomeUnknown => "outcomeUnknown",
    }
}

fn sort_order_key(value: SortOrder) -> String {
    match value {
        SortOrder::Asc => "asc",
        SortOrder::Desc => "desc",
    }
    .to_string()
}

fn process_state_key(value: ProcessState) -> String {
    match value {
        ProcessState::Running => "running",
        ProcessState::Sleeping => "sleeping",
        ProcessState::Stopped => "stopped",
        ProcessState::Idle => "idle",
        ProcessState::Uninterruptible => "uninterruptible",
        ProcessState::Zombie => "zombie",
        ProcessState::Dead => "dead",
        ProcessState::Unknown => "unknown",
    }
    .to_string()
}

fn process_sort_key(value: ProcessSortBy) -> String {
    match value {
        ProcessSortBy::Pid => "pid",
        ProcessSortBy::Name => "name",
        ProcessSortBy::CpuPercent => "cpuPercent",
        ProcessSortBy::MemoryPercent => "memoryPercent",
        ProcessSortBy::ConnectionCount => "connectionCount",
        ProcessSortBy::StartedAt => "startedAt",
    }
    .to_string()
}

fn network_protocol_key(value: NetworkProtocol) -> String {
    match value {
        NetworkProtocol::Tcp => "tcp",
        NetworkProtocol::Tcp6 => "tcp6",
        NetworkProtocol::Udp => "udp",
        NetworkProtocol::Udp6 => "udp6",
    }
    .to_string()
}

fn network_state_key(value: NetworkConnectionState) -> String {
    match value {
        NetworkConnectionState::Established => "established",
        NetworkConnectionState::SynSent => "synSent",
        NetworkConnectionState::SynReceived => "synReceived",
        NetworkConnectionState::FinWait1 => "finWait1",
        NetworkConnectionState::FinWait2 => "finWait2",
        NetworkConnectionState::TimeWait => "timeWait",
        NetworkConnectionState::Closed => "closed",
        NetworkConnectionState::CloseWait => "closeWait",
        NetworkConnectionState::LastAck => "lastAck",
        NetworkConnectionState::Listen => "listen",
        NetworkConnectionState::Closing => "closing",
        NetworkConnectionState::Unconnected => "unconnected",
        NetworkConnectionState::Unknown => "unknown",
    }
    .to_string()
}

fn network_sort_key(value: NetworkSortBy) -> String {
    match value {
        NetworkSortBy::Protocol => "protocol",
        NetworkSortBy::LocalEndpoint => "localEndpoint",
        NetworkSortBy::RemoteEndpoint => "remoteEndpoint",
        NetworkSortBy::State => "state",
        NetworkSortBy::ProcessName => "processName",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_identity_requires_sha256_hex() {
        assert!(validate_process_id(&"a".repeat(64)).is_ok());
        assert!(validate_process_id("1234").is_err());
        assert!(validate_process_id(&"z".repeat(64)).is_err());
    }

    #[test]
    fn query_builder_uses_camel_case_contract_values() {
        let query = ProcessListQuery {
            sort_by: Some(ProcessSortBy::CpuPercent),
            sort_order: Some(SortOrder::Desc),
            ..ProcessListQuery::default()
        };
        let path = process_query_path(&query).expect("query path");
        assert!(path.contains("sortBy=cpuPercent"));
        assert!(path.contains("sortOrder=desc"));
    }

    #[test]
    fn force_kill_confirmation_is_bound_and_single_use() {
        let token = generate_confirmation_token().expect("token");
        FORCE_KILL_CONFIRMATIONS
            .lock()
            .expect("confirmation store")
            .insert(
                hash_confirmation_token(&token),
                ConfirmationGrant {
                    session_id: "session-a".to_string(),
                    node_id: "node-a".to_string(),
                    process_id: "a".repeat(64),
                    expires_at: Utc::now() + Duration::seconds(60),
                },
            );

        assert!(consume_confirmation(&token, "session-a", "node-a", &"a".repeat(64)).is_ok());
        let reused = consume_confirmation(&token, "session-a", "node-a", &"a".repeat(64))
            .expect_err("confirmation token must be single use");
        assert_eq!(reused.code, ErrorCode::ProcessConfirmationInvalid);
    }

    #[test]
    fn force_kill_confirmation_rejects_other_sessions() {
        let token = generate_confirmation_token().expect("token");
        FORCE_KILL_CONFIRMATIONS
            .lock()
            .expect("confirmation store")
            .insert(
                hash_confirmation_token(&token),
                ConfirmationGrant {
                    session_id: "session-a".to_string(),
                    node_id: "node-a".to_string(),
                    process_id: "a".repeat(64),
                    expires_at: Utc::now() + Duration::seconds(60),
                },
            );

        let error = consume_confirmation(&token, "session-b", "node-a", &"a".repeat(64))
            .expect_err("session mismatch must fail");
        assert_eq!(error.code, ErrorCode::ProcessConfirmationInvalid);
    }
}
