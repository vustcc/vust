//! Master 专用脚本运行 API：幂等准备、终端连接和取消。

use crate::{
    models::script_runs,
    services::script_runs as script_run_service,
    services::script_terminal::{self, ScriptTerminalEvent},
    state::AppState,
    types::{ApiResponse, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use vust_contracts::scripts::{
    AgentStartScriptRunRequest, ScriptRunTerminalClientMessage, ScriptRunTerminalErrorCode,
    ScriptRunTerminalServerMessage,
};

/// 构建只允许 Master 调用的脚本运行路由。
pub fn script_run_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(start))
        .route("/{run_id}/cancel", post(cancel))
        .route("/{run_id}/ws", get(terminal_websocket))
}

async fn terminal_websocket(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    Ok(ws
        .on_upgrade(move |socket| handle_terminal_socket(socket, state, run_id))
        .into_response())
}

async fn handle_terminal_socket(
    socket: axum::extract::ws::WebSocket,
    state: Arc<AppState>,
    run_id: String,
) {
    let (mut sender, mut receiver) = socket.split();
    let run = match script_runs::attach_terminal(&state.metadata_db, &run_id).await {
        Ok(run) => run,
        Err(error) => {
            let _ = send_error(
                &mut sender,
                ScriptRunTerminalErrorCode::ScriptRunSessionAlreadyAttached,
                &error.to_string(),
            )
            .await;
            return;
        }
    };
    let mut session: Option<script_terminal::ScriptTerminalSession> = None;
    let mut events: Option<tokio::sync::mpsc::Receiver<ScriptTerminalEvent>> = None;
    let mut prestart_failed = false;
    loop {
        tokio::select! {
            incoming=receiver.next()=>match incoming {
                Some(Ok(Message::Text(text))) if text.len()<=script_terminal::MAX_CONTROL_BYTES => {
                    let Ok(control)=serde_json::from_str::<ScriptRunTerminalClientMessage>(&text) else { let _=send_error(&mut sender,ScriptRunTerminalErrorCode::ScriptRunTerminalProtocolViolation,"invalid script terminal control frame").await; break; };
                    match control {
                        ScriptRunTerminalClientMessage::Start{cols,rows} if session.is_none()=>match script_terminal::start(Arc::clone(&state),&run.run_id,cols,rows).await {
                            Ok((started,event_rx))=>{let control=ScriptRunTerminalServerMessage::Started{run_id:run.run_id.clone(),started_at:chrono::Utc::now().to_rfc3339(),timeout_seconds:run.timeout_seconds as u32};if send_control(&mut sender,&control).await.is_err(){break}session=Some(started);events=Some(event_rx);},
                            Err(error)=>{prestart_failed=true;let _=send_error(&mut sender,ScriptRunTerminalErrorCode::ScriptRunTerminalStartFailed,&error.to_string()).await;break;}
                        },
                        ScriptRunTerminalClientMessage::Resize{cols,rows}=>{let invalid=match session.as_ref(){Some(active)=>active.resize(cols,rows).await.is_err(),None=>true};if invalid{let _=send_error(&mut sender,ScriptRunTerminalErrorCode::ScriptRunInvalidTerminalSize,"invalid terminal size").await;}},
                        ScriptRunTerminalClientMessage::Close=>{if let Some(active)=session.as_ref(){active.close().await;}else{break}},
                        _=>{let _=send_error(&mut sender,ScriptRunTerminalErrorCode::ScriptRunTerminalProtocolViolation,"script terminal session is already active").await;}
                    }
                }
                Some(Ok(Message::Binary(data)))=>{
                    let invalid = if data.len()>script_terminal::MAX_INPUT_BYTES { true } else { match session.as_ref(){Some(active)=>active.input(data.to_vec()).await.is_err(),None=>true} };
                    if invalid {let _=send_error(&mut sender,ScriptRunTerminalErrorCode::ScriptRunTerminalProtocolViolation,"invalid script terminal input frame").await;break;}
                },
                Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,
                _=>{}
            },
            event=async { match events.as_mut(){Some(rx)=>rx.recv().await,None=>std::future::pending().await} }=>match event {
                Some(ScriptTerminalEvent::Output(data))=>if sender.send(Message::Binary(data.into())).await.is_err(){break},
                Some(ScriptTerminalEvent::Exited{status,exit_code,ended_at})=>{let control=ScriptRunTerminalServerMessage::Exited{run_id:run.run_id.clone(),exit_code,status,ended_at};let _=send_control(&mut sender,&control).await;return;},
                Some(ScriptTerminalEvent::Error(message))=>{let _=send_error(&mut sender,ScriptRunTerminalErrorCode::ScriptRunTerminalIoFailed,&message).await;},
                None=>break,
            }
        }
    }
    if let Some(active) = session.as_ref() {
        active.close().await;
    } else {
        let cancelled = script_runs::required(&state.metadata_db, &run.run_id)
            .await
            .is_ok_and(|current| current.status == "cancelling");
        let status = if cancelled {
            vust_contracts::scripts::ScriptRunStatus::Cancelled
        } else if prestart_failed {
            vust_contracts::scripts::ScriptRunStatus::Failed
        } else {
            vust_contracts::scripts::ScriptRunStatus::Cancelled
        };
        let failed = status == vust_contracts::scripts::ScriptRunStatus::Failed;
        if let Err(error) = script_runs::finish(
            &state.metadata_db,
            &run.run_id,
            status,
            None,
            failed.then_some("SCRIPT_RUN_TERMINAL_START_FAILED"),
            failed.then_some("failed to start script PTY"),
        )
        .await
        {
            tracing::warn!(run_id = %run.run_id, %error, "failed to persist pre-start terminal result");
        }
    }
}

async fn send_control(
    sender: &mut futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    control: &ScriptRunTerminalServerMessage,
) -> Result<(), axum::Error> {
    sender
        .send(Message::Text(
            serde_json::to_string(control).unwrap_or_default().into(),
        ))
        .await
}
async fn send_error(
    sender: &mut futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    code: ScriptRunTerminalErrorCode,
    message: &str,
) -> Result<(), axum::Error> {
    send_control(
        sender,
        &ScriptRunTerminalServerMessage::Error {
            code,
            message: message.to_string(),
            recoverable: false,
        },
    )
    .await
}

async fn start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AgentStartScriptRunRequest>,
) -> ApiResult<Response> {
    let run_id = script_run_service::submit(state, request).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Script run accepted",
            Some(serde_json::json!({ "runId": run_id })),
        )),
    )
        .into_response())
}

async fn cancel(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> ApiResult<Response> {
    let run = script_runs::request_cancel(&state.metadata_db, &run_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Script run cancellation accepted",
            Some(serde_json::json!({ "runId": run.run_id, "status": run.status })),
        )),
    )
        .into_response())
}
