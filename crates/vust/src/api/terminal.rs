//! 宿主机终端语义网关。

use crate::{
    api::auth::AuthenticatedAdmin,
    models::{
        logging::{LogModule, LogStatus, PlatformLogLevel},
        node_runtime_client::NodeRuntimeClient,
    },
    services::{
        logging::{self, OperationEventBuilder},
        node_inventory::get_node_display_name,
    },
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Router,
    extract::{Path, State, WebSocketUpgrade, ws::Message},
    http::{HeaderMap, StatusCode, header::HeaderName},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{CloseCode, RequestBuilderExt};
use serde_json::{Value, json};
use std::{net::IpAddr, sync::Arc};
use vust_contracts::{
    api::{ApiResponse as ContractApiResponse, ErrorCode},
    terminal::{
        TerminalAccess, TerminalCapabilities, TerminalErrorCode, TerminalExitReason,
        TerminalOwnership, TerminalRuntimeAccess, TerminalServerMessage, TerminalTicketContext,
    },
};

const AGENT_ACCESS_PATH: &str = "/api/v1/agent/terminal/access";
const AGENT_WS_PATH: &str = "/api/v1/agent/terminal/ws";
const TERMINAL_TICKET_HEADER: HeaderName = HeaderName::from_static("x-vust-terminal-ticket");

/// 构建单节点宿主机终端语义路由。
pub fn terminal_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{node_id}/terminal/access", get(access))
        .route("/{node_id}/terminal/ws", get(websocket))
}

async fn access(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    _admin: AuthenticatedAdmin,
) -> ApiResult<Response> {
    let (client, node_name) = node_client_and_name(&state, &node_id).await?;
    let runtime: TerminalRuntimeAccess = client.get_domain(AGENT_ACCESS_PATH).await?;
    Ok(ApiResponse::success_with_raw(
        "Host terminal access loaded",
        Some(TerminalAccess {
            ownership: TerminalOwnership::System,
            node_id,
            node_name,
            availability: runtime.availability,
            shell: runtime.shell,
            idle_timeout_seconds: runtime.idle_timeout_seconds,
            capabilities: TerminalCapabilities {
                can_start_session: runtime.can_start_session,
            },
            unavailable_reason: runtime.unavailable_reason,
        }),
    )
    .into_response())
}

async fn websocket(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (runtime_client, node_name) = node_client_and_name(&state, &node_id).await?;
    let trace_id = logging::resolve_trace_id(&headers);
    let client_ip_text = admin.session.client_ip.clone().ok_or_else(|| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session is missing a trusted client IP",
        )
    })?;
    let client_ip = client_ip_text.parse::<IpAddr>().map_err(|_| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session contains an invalid client IP",
        )
    })?;
    let ticket = state
        .terminal_tickets
        .issue(TerminalTicketContext {
            actor_name: admin.username.clone(),
            client_ip: client_ip_text,
            trace_id: trace_id.clone(),
            node_id: node_id.clone(),
        })
        .map_err(|error| ApiError::internal(error.to_string()))?;

    let upstream = runtime_client
        .authorize_request(
            runtime_client
                .client
                .get(runtime_client.build_ws_uri(AGENT_WS_PATH)),
        )
        .header(&TERMINAL_TICKET_HEADER, &ticket)
        .upgrade()
        .send()
        .await;
    let upstream = match upstream {
        Ok(response) => {
            if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                state.terminal_tickets.revoke(&ticket);
                let rejection = AgentUpgradeRejection::from_response(response.into_inner()).await;
                record_start_failure(
                    &state,
                    &admin,
                    client_ip,
                    &trace_id,
                    &node_id,
                    &node_name,
                    rejection.code.as_str(),
                    &rejection.message,
                );
                return Err(rejection.into_api_error());
            }
            match response.into_websocket().await {
                Ok(upstream) => upstream,
                Err(error) => {
                    state.terminal_tickets.revoke(&ticket);
                    record_start_failure(
                        &state,
                        &admin,
                        client_ip,
                        &trace_id,
                        &node_id,
                        &node_name,
                        ErrorCode::AgentRequestFailed.as_str(),
                        "terminal agent returned an invalid WebSocket upgrade response",
                    );
                    return Err(ApiError::bad_gateway(
                        ErrorCode::AgentRequestFailed,
                        "terminal agent returned an invalid WebSocket upgrade response",
                    )
                    .with_detail(error.to_string()));
                }
            }
        }
        Err(error) => {
            state.terminal_tickets.revoke(&ticket);
            record_start_failure(
                &state,
                &admin,
                client_ip,
                &trace_id,
                &node_id,
                &node_name,
                ErrorCode::AgentUnavailable.as_str(),
                "terminal agent is unavailable",
            );
            return Err(ApiError::bad_gateway(
                ErrorCode::AgentUnavailable,
                "terminal agent is unavailable",
            )
            .with_detail(error.to_string()));
        }
    };

    let audit = TerminalAuditContext {
        state: Arc::clone(&state),
        username: admin.username,
        user_id: admin.id,
        client_ip,
        trace_id,
        node_id,
        node_name,
    };
    Ok(ws
        .on_upgrade(move |client| bridge(client, upstream, audit))
        .into_response())
}

struct TerminalAuditContext {
    state: Arc<AppState>,
    username: String,
    user_id: i64,
    client_ip: IpAddr,
    trace_id: String,
    node_id: String,
    node_name: String,
}

struct AgentUpgradeRejection {
    status: StatusCode,
    code: ErrorCode,
    message: String,
}

impl AgentUpgradeRejection {
    async fn from_response(response: reqwest::Response) -> Self {
        let status = response.status();
        match response.json::<ContractApiResponse<Value>>().await {
            Ok(body) => Self {
                status,
                code: body.error_code.unwrap_or(ErrorCode::AgentRequestFailed),
                message: body.message,
            },
            Err(_) => Self {
                status,
                code: ErrorCode::AgentRequestFailed,
                message: "terminal agent rejected the WebSocket upgrade".to_string(),
            },
        }
    }

    fn into_api_error(self) -> ApiError {
        ApiError::new(self.status, self.code, self.message)
    }
}

async fn bridge(
    client: axum::extract::ws::WebSocket,
    upstream: reqwest_websocket::WebSocket,
    audit: TerminalAuditContext,
) {
    let (mut client_sender, mut client_receiver) = client.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream.split();
    let client_to_agent = tokio::spawn(async move {
        while let Some(message) = client_receiver.next().await {
            let Ok(message) = message else { break };
            let (message, close) = client_to_upstream(message);
            if upstream_sender.send(message).await.is_err() || close {
                break;
            }
        }
        let _ = upstream_sender
            .send(reqwest_websocket::Message::Close {
                code: CloseCode::Normal,
                reason: String::new(),
            })
            .await;
    });

    let agent_to_client = tokio::spawn(async move {
        let mut started = false;
        let mut ended = false;
        while let Some(message) = upstream_receiver.next().await {
            let Ok(message) = message else { break };
            audit_control_message(&audit, &message, &mut started, &mut ended);
            let (message, close) = upstream_to_client(message);
            if client_sender.send(message).await.is_err() || close {
                break;
            }
        }
        if started && !ended {
            record_end(
                &audit,
                None,
                TerminalExitReason::TransportClosed,
                None,
                PlatformLogLevel::Error,
                LogStatus::Failed,
            );
        }
        let _ = client_sender.send(Message::Close(None)).await;
    });
    let _ = tokio::join!(client_to_agent, agent_to_client);
}

fn audit_control_message(
    audit: &TerminalAuditContext,
    message: &reqwest_websocket::Message,
    started: &mut bool,
    ended: &mut bool,
) {
    let reqwest_websocket::Message::Text(text) = message else {
        return;
    };
    let Ok(control) = serde_json::from_str::<TerminalServerMessage>(text) else {
        return;
    };
    match control {
        TerminalServerMessage::Started {
            session_id, shell, ..
        } => {
            *started = true;
            OperationEventBuilder::new(&audit.username, "terminal_session_start", audit.client_ip)
                .user_id(audit.user_id)
                .module(LogModule::System)
                .target_type("node")
                .target_id(&audit.node_id)
                .trace_id(&audit.trace_id)
                .source("vust_api")
                .request("GET", "/api/v1/node/{node_id}/terminal/ws")
                .level(PlatformLogLevel::Warning)
                .set_success()
                .metadata(json!({
                    "nodeId": audit.node_id,
                    "nodeName": audit.node_name,
                    "sessionId": session_id,
                    "shell": shell,
                }))
                .finish(&audit.state.metadata_db);
        }
        TerminalServerMessage::Exited {
            session_id,
            exit_code,
            reason,
            ..
        } => {
            *ended = true;
            let (level, status) = match reason {
                TerminalExitReason::ProcessExited | TerminalExitReason::UserClosed => {
                    (PlatformLogLevel::Info, LogStatus::Success)
                }
                TerminalExitReason::IdleTimeout => (PlatformLogLevel::Warning, LogStatus::Success),
                TerminalExitReason::TransportClosed | TerminalExitReason::IoFailed => {
                    (PlatformLogLevel::Error, LogStatus::Failed)
                }
            };
            record_end(audit, Some(session_id), reason, exit_code, level, status);
        }
        TerminalServerMessage::Error { code, message, .. } if !*started => {
            record_start_failure(
                &audit.state,
                &AuthenticatedAdminForLog {
                    id: audit.user_id,
                    username: audit.username.clone(),
                },
                audit.client_ip,
                &audit.trace_id,
                &audit.node_id,
                &audit.node_name,
                terminal_error_code_name(code),
                &message,
            );
        }
        _ => {}
    }
}

trait AuditActor {
    fn id(&self) -> i64;
    fn username(&self) -> &str;
}

impl AuditActor for AuthenticatedAdmin {
    fn id(&self) -> i64 {
        self.id
    }
    fn username(&self) -> &str {
        &self.username
    }
}

struct AuthenticatedAdminForLog {
    id: i64,
    username: String,
}
impl AuditActor for AuthenticatedAdminForLog {
    fn id(&self) -> i64 {
        self.id
    }
    fn username(&self) -> &str {
        &self.username
    }
}

#[allow(clippy::too_many_arguments)]
fn record_start_failure(
    state: &AppState,
    actor: &impl AuditActor,
    client_ip: IpAddr,
    trace_id: &str,
    node_id: &str,
    node_name: &str,
    error_code: &str,
    error: &str,
) {
    OperationEventBuilder::new(actor.username(), "terminal_session_start", client_ip)
        .user_id(actor.id())
        .module(LogModule::System)
        .target_type("node")
        .target_id(node_id)
        .trace_id(trace_id)
        .source("vust_api")
        .request("GET", "/api/v1/node/{node_id}/terminal/ws")
        .level(PlatformLogLevel::Error)
        .status(LogStatus::Failed)
        .metadata(json!({
            "nodeId": node_id,
            "nodeName": node_name,
            "errorCode": error_code,
            "error": error,
        }))
        .finish(&state.metadata_db);
}

fn terminal_error_code_name(code: TerminalErrorCode) -> &'static str {
    match code {
        TerminalErrorCode::TerminalUnavailable => "TERMINAL_UNAVAILABLE",
        TerminalErrorCode::TerminalInvalidSize => "TERMINAL_INVALID_SIZE",
        TerminalErrorCode::TerminalSessionAlreadyActive => "TERMINAL_SESSION_ALREADY_ACTIVE",
        TerminalErrorCode::TerminalStartFailed => "TERMINAL_START_FAILED",
        TerminalErrorCode::TerminalIoFailed => "TERMINAL_IO_FAILED",
        TerminalErrorCode::TerminalProtocolViolation => "TERMINAL_PROTOCOL_VIOLATION",
    }
}

fn record_end(
    audit: &TerminalAuditContext,
    session_id: Option<String>,
    reason: TerminalExitReason,
    exit_code: Option<i32>,
    level: PlatformLogLevel,
    status: LogStatus,
) {
    OperationEventBuilder::new(&audit.username, "terminal_session_end", audit.client_ip)
        .user_id(audit.user_id)
        .module(LogModule::System)
        .target_type("node")
        .target_id(&audit.node_id)
        .trace_id(&audit.trace_id)
        .source("vust_api")
        .request("GET", "/api/v1/node/{node_id}/terminal/ws")
        .level(level)
        .status(status)
        .metadata(json!({
            "nodeId": audit.node_id,
            "nodeName": audit.node_name,
            "sessionId": session_id,
            "exitCode": exit_code,
            "reason": reason,
        }))
        .finish(&audit.state.metadata_db);
}

fn client_to_upstream(message: Message) -> (reqwest_websocket::Message, bool) {
    match message {
        Message::Text(value) => (reqwest_websocket::Message::Text(value.to_string()), false),
        Message::Binary(value) => (reqwest_websocket::Message::Binary(value), false),
        Message::Ping(value) => (reqwest_websocket::Message::Ping(value), false),
        Message::Pong(value) => (reqwest_websocket::Message::Pong(value), false),
        Message::Close(frame) => (
            reqwest_websocket::Message::Close {
                code: frame
                    .as_ref()
                    .map(|frame| CloseCode::from(frame.code))
                    .unwrap_or(CloseCode::Normal),
                reason: frame
                    .map(|frame| frame.reason.to_string())
                    .unwrap_or_default(),
            },
            true,
        ),
    }
}

fn upstream_to_client(message: reqwest_websocket::Message) -> (Message, bool) {
    match message {
        reqwest_websocket::Message::Text(value) => (Message::Text(value.into()), false),
        reqwest_websocket::Message::Binary(value) => (Message::Binary(value), false),
        reqwest_websocket::Message::Ping(value) => (Message::Ping(value), false),
        reqwest_websocket::Message::Pong(value) => (Message::Pong(value), false),
        reqwest_websocket::Message::Close { code, reason } => (
            Message::Close(Some(axum::extract::ws::CloseFrame {
                code: code.into(),
                reason: reason.into(),
            })),
            true,
        ),
    }
}

async fn node_client_and_name(
    state: &AppState,
    node_id: &str,
) -> ApiResult<(NodeRuntimeClient, String)> {
    let node_name = get_node_display_name(&state.metadata_db, node_id)
        .await
        .map_err(|error| ApiError::database(error.to_string()))?
        .unwrap_or_else(|| {
            if node_id == "local" {
                "local".to_string()
            } else {
                node_id.to_string()
            }
        });
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?;
    Ok((client, node_name))
}
