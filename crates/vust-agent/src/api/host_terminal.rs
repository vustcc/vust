//! 宿主机终端 Agent API。

use crate::{
    services::host_terminal::{
        self, HostTerminalEvent, HostTerminalSession, MAX_CONTROL_BYTES, MAX_INPUT_BYTES,
    },
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Router,
    extract::{State, WebSocketUpgrade, ws::Message},
    http::{HeaderMap, header::HeaderName},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use std::{future::pending, sync::Arc};
use tokio::sync::mpsc;
use tracing::{info, warn};
use vust_contracts::{
    api::{ApiResponse as ContractApiResponse, ErrorCode},
    terminal::{
        TerminalClientMessage, TerminalErrorCode, TerminalExitReason, TerminalServerMessage,
        TerminalTicketConsumeRequest, TerminalTicketConsumeResponse,
    },
};

const TERMINAL_TICKET_HEADER: HeaderName = HeaderName::from_static("x-vust-terminal-ticket");

/// 创建宿主机终端能力和 WebSocket 路由。
pub fn host_terminal_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/access", get(access))
        .route("/ws", get(websocket))
}

async fn access() -> Response {
    ApiResponse::success_with_raw(
        "Host terminal access loaded",
        Some(host_terminal::runtime_access()),
    )
    .into_response()
}

async fn websocket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    let ticket = headers
        .get(&TERMINAL_TICKET_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::forbidden(ErrorCode::AuthForbidden, "terminal ticket is required")
        })?;
    let context = consume_ticket(&state, ticket).await?;
    Ok(ws
        .on_upgrade(move |socket| handle_socket(socket, context))
        .into_response())
}

async fn consume_ticket(
    state: &AppState,
    ticket: &str,
) -> ApiResult<TerminalTicketConsumeResponse> {
    let runtime = crate::services::agent_runtime::load(&state.metadata_db)
        .await
        .map_err(|_| {
            ApiError::forbidden(ErrorCode::AuthForbidden, "agent identity is unavailable")
        })?;
    let node_id = runtime.node_id;
    let vust_url = runtime.controller_url;
    let client = vust_security::client::build_tls_client().map_err(|_| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "terminal ticket validation is unavailable",
        )
    })?;
    let response = client
        .post(format!(
            "{}/api/v1/runtime/terminal-tickets/consume",
            vust_url.trim_end_matches('/')
        ))
        .json(&TerminalTicketConsumeRequest {
            ticket: ticket.to_string(),
            node_id,
        })
        .send()
        .await
        .map_err(|_| {
            ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "terminal ticket validation failed",
            )
        })?;
    if !response.status().is_success() {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "terminal ticket is invalid or expired",
        ));
    }
    let body = response
        .json::<ContractApiResponse<TerminalTicketConsumeResponse>>()
        .await
        .map_err(|_| {
            ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "terminal ticket response is invalid",
            )
        })?;
    body.data.ok_or_else(|| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "terminal ticket response is empty",
        )
    })
}

async fn handle_socket(
    socket: axum::extract::ws::WebSocket,
    context: TerminalTicketConsumeResponse,
) {
    let (mut sender, mut receiver) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Message>(128);
    let send_task = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });
    let mut session: Option<HostTerminalSession> = None;
    let mut events: Option<mpsc::Receiver<HostTerminalEvent>> = None;
    let mut closing = false;
    info!(
        actor = %context.actor_name,
        client_ip = %context.client_ip,
        trace_id = %context.trace_id,
        node_id = %context.node_id,
        "Host terminal WebSocket authorized"
    );

    loop {
        tokio::select! {
            incoming = receiver.next(), if !closing => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if text.len() > MAX_CONTROL_BYTES {
                        send_error(&outbound_tx, TerminalErrorCode::TerminalProtocolViolation, "terminal control frame is too large", false).await;
                        break;
                    }
                    let message = match serde_json::from_str::<TerminalClientMessage>(&text) {
                        Ok(message) => message,
                        Err(_) => {
                            send_error(&outbound_tx, TerminalErrorCode::TerminalProtocolViolation, "invalid terminal control frame", false).await;
                            break;
                        }
                    };
                    match message {
                        TerminalClientMessage::Start { cols, rows } => {
                            if session.is_some() {
                                send_error(&outbound_tx, TerminalErrorCode::TerminalSessionAlreadyActive, "terminal session is already active", true).await;
                                continue;
                            }
                            match host_terminal::start(cols, rows).await {
                                Ok((started, event_rx)) => {
                                    let control = TerminalServerMessage::Started {
                                        session_id: started.session_id.clone(),
                                        shell: started.shell,
                                        started_at: chrono::Utc::now().to_rfc3339(),
                                        idle_timeout_seconds: host_terminal::IDLE_TIMEOUT.as_secs(),
                                    };
                                    if send_control(&outbound_tx, &control).await.is_err() {
                                        shutdown_and_drain(
                                            started,
                                            event_rx,
                                            TerminalExitReason::TransportClosed,
                                        )
                                        .await;
                                        break;
                                    }
                                    session = Some(started);
                                    events = Some(event_rx);
                                }
                                Err(error) => {
                                    send_error(&outbound_tx, TerminalErrorCode::TerminalStartFailed, &error, false).await;
                                    break;
                                }
                            }
                        }
                        TerminalClientMessage::Resize { cols, rows } => {
                            let Some(active) = session.as_ref() else {
                                send_error(&outbound_tx, TerminalErrorCode::TerminalProtocolViolation, "terminal session is not active", true).await;
                                continue;
                            };
                            if let Err(code) = active.resize(cols, rows).await {
                                send_error(&outbound_tx, code, "invalid terminal size", true).await;
                            }
                        }
                        TerminalClientMessage::Close => {
                            if let Some(active) = session.as_ref() {
                                if active.request_close(TerminalExitReason::UserClosed).await.is_err() {
                                    send_error(&outbound_tx, TerminalErrorCode::TerminalIoFailed, "terminal session could not be closed", false).await;
                                    break;
                                }
                                closing = true;
                            } else {
                                break;
                            }
                        }
                    }
                }
                Some(Ok(Message::Binary(data))) => {
                    let Some(active) = session.as_ref() else {
                        send_error(&outbound_tx, TerminalErrorCode::TerminalProtocolViolation, "terminal session is not active", false).await;
                        break;
                    };
                    if data.len() > MAX_INPUT_BYTES || active.input(data.to_vec()).await.is_err() {
                        send_error(&outbound_tx, TerminalErrorCode::TerminalProtocolViolation, "terminal input frame is invalid", false).await;
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    warn!(error = %error, "Host terminal WebSocket receive failed");
                    break;
                }
                _ => {}
            },
            event = receive_event(&mut events), if events.is_some() => {
                match event {
                    Some(event) => {
                        let terminal_ended = forward_event(&outbound_tx, event).await;
                        if terminal_ended {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    if let Some(active) = session {
        if let Some(event_rx) = events {
            shutdown_and_drain(active, event_rx, TerminalExitReason::TransportClosed).await;
        } else {
            let _ = active
                .request_close(TerminalExitReason::TransportClosed)
                .await;
            active.wait().await;
        }
    }
    drop(outbound_tx);
    let _ = send_task.await;
    info!(
        actor = %context.actor_name,
        trace_id = %context.trace_id,
        node_id = %context.node_id,
        "Host terminal WebSocket closed"
    );
}

/// 关闭 PTY 时并行排空事件，避免满载队列阻塞驱动器终态与进程回收。
async fn shutdown_and_drain(
    session: HostTerminalSession,
    mut events: mpsc::Receiver<HostTerminalEvent>,
    reason: TerminalExitReason,
) {
    let _ = session.request_close(reason).await;
    let wait = session.wait();
    tokio::pin!(wait);
    loop {
        tokio::select! {
            _ = &mut wait => break,
            event = events.recv() => {
                if event.is_none() {
                    break;
                }
            }
        }
    }
}

async fn receive_event(
    events: &mut Option<mpsc::Receiver<HostTerminalEvent>>,
) -> Option<HostTerminalEvent> {
    match events {
        Some(receiver) => receiver.recv().await,
        None => pending().await,
    }
}

async fn forward_event(sender: &mpsc::Sender<Message>, event: HostTerminalEvent) -> bool {
    match event {
        HostTerminalEvent::Output(bytes) => {
            let _ = sender.send(Message::Binary(bytes.into())).await;
            false
        }
        HostTerminalEvent::Control(control) => {
            let ended = matches!(control, TerminalServerMessage::Exited { .. });
            let _ = send_control(sender, &control).await;
            ended
        }
    }
}

async fn send_control(
    sender: &mpsc::Sender<Message>,
    message: &TerminalServerMessage,
) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|_| ())?;
    sender
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

async fn send_error(
    sender: &mpsc::Sender<Message>,
    code: TerminalErrorCode,
    message: impl Into<String>,
    recoverable: bool,
) {
    let _ = send_control(
        sender,
        &TerminalServerMessage::Error {
            code,
            message: message.into(),
            recoverable,
        },
    )
    .await;
}
