//! WebSocket API：与前端建立双向通信入口。

use crate::api::docker::context::DockerOperationContext;
use crate::services::websocket_messages::{
    ClientWsMessage, ContainerTerminalClientMessage, ContainerTerminalClosePayload,
    ContainerTerminalErrorPayload, ContainerTerminalExitPayload, ContainerTerminalInputPayload,
    ContainerTerminalOutputPayload, ContainerTerminalResizePayload, ContainerTerminalServerMessage,
    ContainerTerminalStartPayload, ContainerTerminalStartedPayload, LogPayload, MessagePayload,
    ServerWsMessage,
};
use crate::state::AppState;
use crate::types::ApiError;
use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
};
use bollard::{
    container::LogOutput,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults},
    query_parameters::LogsOptions,
};
use chrono::{DateTime, Local};
use futures_util::{SinkExt, StreamExt};
use serde_json::{self, json};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tracing::{debug, error, info, warn};

/// 创建 WebSocket 路由，用于双向通信
pub fn websocket_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events/ws", get(websocket_handler))
        .route("/terminal/ws", get(terminal_websocket_handler))
}

/// WebSocket 升级入口
async fn websocket_handler(
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

type LogTaskMap = HashMap<String, JoinHandle<()>>;

/// 处理 WebSocket 连接，包括接收客户端消息和管理日志订阅
async fn handle_socket(state: Arc<AppState>, socket: WebSocket) {
    info!("New WebSocket client connected.");

    let (mut sender, mut receiver) = socket.split();
    let mut log_tasks: LogTaskMap = HashMap::new();

    // 用于从日志任务向 WebSocket 发送消息的通道
    let (tx, mut rx) = mpsc::channel::<ServerWsMessage>(100);

    // 任务：将通道中的消息发送到 WebSocket 客户端
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(payload) = serde_json::to_string(&msg)
                && sender.send(Message::Text(payload.into())).await.is_err()
            {
                warn!("Failed to send message to WebSocket client.");
                break;
            }
        }
    });

    // 循环处理来自客户端的消息
    loop {
        tokio::select! {
            res = receiver.next() => {
                match res {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientWsMessage>(&text) {
                            Ok(ClientWsMessage::SubscribeLogs { container_id }) => {
                                info!("Received subscribe request for container: {}", container_id);
                                if let Some(task) = log_tasks.remove(&container_id) {
                                    task.abort();
                                }
                                let new_task = spawn_log_streaming_task(container_id.clone(), state.clone(), tx.clone());
                                log_tasks.insert(container_id, new_task);
                            }
                            Ok(ClientWsMessage::UnsubscribeLogs { container_id }) => {
                                info!("Received unsubscribe request for container: {}", container_id);
                                if let Some(task) = log_tasks.remove(&container_id) {
                                    task.abort();
                                    debug!("Aborted log stream task for container: {}", container_id);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to deserialize client message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("Client disconnected.");
                        break;
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket receive error: {}", e);
                        break;
                    }
                    _ => {
                        // 忽略其他类型的消息
                    }
                }
            },
            _ = &mut send_task => {
                // `send_task` 结束，通常意味着客户端已断开
                break;
            }
        }
    }

    // 清理：当客户端断开连接时，中止所有活动的日志任务
    for (id, task) in log_tasks {
        task.abort();
        debug!("Aborted log stream task for container: {}", id);
    }
    // `send_task` 会在 `rx` drop 后自动结束，但我们还是显式 abort
    send_task.abort();

    info!("WebSocket client connection closed.");
}

/// 生成一个新的任务来流式传输指定容器的日志
fn spawn_log_streaming_task(
    container_id: String,
    state: Arc<AppState>,
    tx: mpsc::Sender<ServerWsMessage>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let docker = match state.docker_client().await {
            Ok(client) => client,
            Err(err) => {
                let message = err.message.into_owned();
                let _ = tx
                    .send(ServerWsMessage::Error(MessagePayload {
                        container_id: container_id.clone(),
                        message,
                    }))
                    .await;
                return;
            }
        };

        // 1. 发送初始快照
        let initial_logs_options = LogsOptions {
            stdout: true,
            stderr: true,
            timestamps: true,
            tail: "100".to_string(),
            ..Default::default()
        };

        let mut initial_stream = docker.logs(&container_id, Some(initial_logs_options));
        let mut lines = Vec::new();
        while let Some(item) = initial_stream.next().await {
            match item {
                Ok(log) => {
                    if let Some(line) = format_log_output(log) {
                        lines.push(line);
                    }
                }
                Err(error) => {
                    error!(
                        "Failed to fetch initial logs for {}: {}",
                        container_id, error
                    );
                    let _ = tx
                        .send(ServerWsMessage::Error(MessagePayload {
                            container_id: container_id.clone(),
                            message: format!("failed to fetch logs: {error}"),
                        }))
                        .await;
                    return;
                }
            }
        }
        if tx
            .send(ServerWsMessage::Snapshot(LogPayload {
                container_id: container_id.clone(),
                lines,
            }))
            .await
            .is_err()
        {
            return;
        }

        // 2. 实时流式传输新日志
        let mut log_stream = docker
            .logs(
                &container_id,
                Some(LogsOptions {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    timestamps: true,
                    tail: "0".to_string(),
                    ..Default::default()
                }),
            )
            .fuse();

        loop {
            tokio::select! {
                log_item = log_stream.next() => {
                    match log_item {
                        Some(Ok(log)) => {
                            if let Some(line) = format_log_output(log) {
                                let frame = ServerWsMessage::Append(LogPayload { container_id: container_id.clone(), lines: vec![line] });
                                if tx.send(frame).await.is_err() {
                                    break; // 发送失败，退出任务
                                }
                            }
                        }
                        Some(Err(err)) => {
                            error!("Error reading log stream for {}: {}", container_id, err);
                            let _ = tx.send(ServerWsMessage::Error(MessagePayload { container_id: container_id.clone(), message: format!("failed to fetch logs: {}", err) })).await;
                            break;
                        }
                        None => {
                            let _ = tx.send(ServerWsMessage::End(MessagePayload { container_id: container_id.clone(), message: "log stream ended".to_string() })).await;
                            break;
                        }
                    }
                }
                _ = tx.closed() => {
                    // 主任务的接收端已关闭，意味着连接已断开
                    debug!("WebSocket connection closed for container log stream: {}", container_id);
                    break;
                }
            }
        }
    })
}

/// 终端 WebSocket 升级入口。
async fn terminal_websocket_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let context = DockerOperationContext::from_trusted_headers(&headers)
        .ok_or(ApiError::ForbiddenResource)?;
    Ok(ws
        .on_upgrade(move |socket| handle_terminal_socket(state, context, socket))
        .into_response())
}

type TerminalSessionMap = HashMap<String, TerminalSession>;

struct TerminalSession {
    exec_id: String,
    input: Arc<Mutex<Pin<Box<dyn AsyncWrite + Send>>>>,
    output_task: JoinHandle<()>,
}

/// 处理终端 WebSocket 连接。
async fn handle_terminal_socket(
    state: Arc<AppState>,
    context: DockerOperationContext,
    socket: WebSocket,
) {
    info!("New terminal WebSocket client connected.");

    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<ContainerTerminalServerMessage>(200);
    let (cleanup_tx, mut cleanup_rx) = mpsc::channel::<String>(64);
    let mut sessions: TerminalSessionMap = HashMap::new();

    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(payload) = serde_json::to_string(&msg)
                && sender.send(Message::Text(payload.into())).await.is_err()
            {
                warn!("Failed to send terminal message to WebSocket client.");
                break;
            }
        }
        // 优雅向客户端发送 WebSocket 关闭帧以完成关闭握手
        let _ = sender.send(Message::Close(None)).await;
    });

    loop {
        tokio::select! {
            res = receiver.next() => {
                match res {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ContainerTerminalClientMessage>(&text) {
                            Ok(ContainerTerminalClientMessage::TerminalStart(payload)) => {
                                let _ = handle_terminal_start(
                                    payload,
                                    state.clone(),
                                    &context,
                                    tx.clone(),
                                    cleanup_tx.clone(),
                                    &mut sessions,
                                )
                                .await;
                            }
                            Ok(ContainerTerminalClientMessage::TerminalInput(payload)) => {
                                let _ = handle_terminal_input(payload, tx.clone(), &sessions).await;
                            }
                            Ok(ContainerTerminalClientMessage::TerminalResize(payload)) => {
                                let _ = handle_terminal_resize(payload, state.clone(), tx.clone(), &sessions).await;
                            }
                            Ok(ContainerTerminalClientMessage::TerminalClose(payload)) => {
                                let _ = handle_terminal_close(payload, tx.clone(), &mut sessions).await;
                            }
                            Err(err) => {
                                let _ = tx
                                    .send(ContainerTerminalServerMessage::TerminalError(ContainerTerminalErrorPayload {
                                        message: format!("failed to parse terminal message: {err}"),
                                    }))
                                    .await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Err(err)) => {
                        warn!("Terminal WebSocket receive error: {}", err);
                        break;
                    }
                    _ => {}
                }
            },
            _ = &mut send_task => {
                break;
            },
            maybe_session_id = cleanup_rx.recv() => {
                if let Some(session_id) = maybe_session_id {
                    let _ = sessions.remove(&session_id);
                    debug!("Terminal session {} removed after natural exit", session_id);
                } else {
                    break;
                }
            }
        }
    }

    for (session_id, session) in sessions {
        session.output_task.abort();
        debug!("Terminal session {} aborted on disconnect", session_id);
    }

    send_task.abort();
    info!("Terminal WebSocket client connection closed.");
}

async fn handle_terminal_start(
    payload: ContainerTerminalStartPayload,
    state: Arc<AppState>,
    context: &DockerOperationContext,
    tx: mpsc::Sender<ContainerTerminalServerMessage>,
    cleanup_tx: mpsc::Sender<String>,
    sessions: &mut TerminalSessionMap,
) -> Result<(), ()> {
    let requested_shell = payload.shell.trim().to_ascii_lowercase();
    let preferred_shell = if requested_shell == "sh" {
        "sh"
    } else {
        "bash"
    };

    let docker = match state.docker_client().await {
        Ok(client) => client,
        Err(err) => {
            let message = err.message.into_owned();
            context
                .record_failure(
                    &state.metadata_db,
                    "docker_container_exec",
                    Some(("container", &payload.container_id)),
                    json!({ "name": payload.container_id }),
                    &message,
                )
                .await;
            let _ = tx
                .send(ContainerTerminalServerMessage::TerminalError(
                    ContainerTerminalErrorPayload { message },
                ))
                .await;
            return Err(());
        }
    };

    let inspect = match docker
        .inspect_container(
            &payload.container_id,
            None::<bollard::query_parameters::InspectContainerOptions>,
        )
        .await
    {
        Ok(inspect) => inspect,
        Err(err) => {
            let message = format!("failed to inspect terminal container: {err}");
            context
                .record_failure(
                    &state.metadata_db,
                    "docker_container_exec",
                    Some(("container", &payload.container_id)),
                    json!({ "name": payload.container_id }),
                    &message,
                )
                .await;
            let _ = tx
                .send(ContainerTerminalServerMessage::TerminalError(
                    ContainerTerminalErrorPayload { message },
                ))
                .await;
            return Err(());
        }
    };
    let container_name = inspect
        .name
        .as_deref()
        .map(|name| name.trim_start_matches('/').to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| payload.container_id.clone());
    let running = inspect
        .state
        .as_ref()
        .and_then(|state| state.status.as_ref())
        .map(ToString::to_string)
        .as_deref()
        == Some("running");
    if !running {
        let message = "terminal requires a running container".to_string();
        context
            .record_failure(
                &state.metadata_db,
                "docker_container_exec",
                Some(("container", &container_name)),
                json!({ "name": container_name }),
                &message,
            )
            .await;
        let _ = tx
            .send(ContainerTerminalServerMessage::TerminalError(
                ContainerTerminalErrorPayload { message },
            ))
            .await;
        return Err(());
    }

    let start_result = if preferred_shell == "bash" {
        match create_terminal_session(
            &docker,
            &payload.container_id,
            "bash",
            payload.cols,
            payload.rows,
            tx.clone(),
            cleanup_tx.clone(),
        )
        .await
        {
            Ok(session) => Ok(session),
            Err(err) => {
                warn!(
                    "Failed to start bash for {}: {}. Falling back to sh.",
                    payload.container_id, err
                );
                create_terminal_session(
                    &docker,
                    &payload.container_id,
                    "sh",
                    payload.cols,
                    payload.rows,
                    tx.clone(),
                    cleanup_tx.clone(),
                )
                .await
            }
        }
    } else {
        create_terminal_session(
            &docker,
            &payload.container_id,
            "sh",
            payload.cols,
            payload.rows,
            tx.clone(),
            cleanup_tx.clone(),
        )
        .await
    };

    let (session, actual_shell) = match start_result {
        Ok(value) => value,
        Err(err) => {
            let message = format!("failed to start terminal: {err}");
            context
                .record_failure(
                    &state.metadata_db,
                    "docker_container_exec",
                    Some(("container", &container_name)),
                    json!({ "name": container_name }),
                    &message,
                )
                .await;
            let _ = tx
                .send(ContainerTerminalServerMessage::TerminalError(
                    ContainerTerminalErrorPayload { message },
                ))
                .await;
            return Err(());
        }
    };

    let session_id = session.exec_id.clone();
    sessions.insert(session_id.clone(), session);

    context
        .record_success(
            &state.metadata_db,
            "docker_container_exec",
            Some(("container", &container_name)),
            json!({ "name": container_name }),
            false,
        )
        .await;

    let _ = tx
        .send(ContainerTerminalServerMessage::TerminalStarted(
            ContainerTerminalStartedPayload {
                session_id,
                shell: actual_shell.to_string(),
            },
        ))
        .await;

    Ok(())
}

async fn create_terminal_session(
    docker: &bollard::Docker,
    container_id: &str,
    shell: &str,
    cols: u16,
    rows: u16,
    tx: mpsc::Sender<ContainerTerminalServerMessage>,
    cleanup_tx: mpsc::Sender<String>,
) -> Result<(TerminalSession, &'static str), bollard::errors::Error> {
    let shell_cmd = if shell == "sh" {
        "/bin/sh"
    } else {
        "/bin/bash"
    };

    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                attach_stdin: Some(true),
                tty: Some(true),
                cmd: Some(vec![shell_cmd.to_string()]),
                ..Default::default()
            },
        )
        .await?;

    let start_options = StartExecOptions {
        detach: false,
        tty: true,
        output_capacity: None,
    };

    let StartExecResults::Attached { mut output, input } =
        docker.start_exec(&exec.id, Some(start_options)).await?
    else {
        return Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 500,
            message: "failed to establish terminal connection".to_string(),
        });
    };

    let _ = docker
        .resize_exec(
            &exec.id,
            ResizeExecOptions {
                width: cols.max(1),
                height: rows.max(1),
            },
        )
        .await;

    let input = Arc::new(Mutex::new(input));
    let output_exec_id = exec.id.clone();
    let docker_for_output = docker.clone();
    let output_tx = tx.clone();
    let output_cleanup_tx = cleanup_tx.clone();
    let output_shell = if shell == "sh" { "sh" } else { "bash" };

    let output_task = tokio::spawn(async move {
        while let Some(item) = output.next().await {
            match item {
                Ok(log) => {
                    let data = String::from_utf8_lossy(log.as_ref()).to_string();
                    if !data.is_empty()
                        && output_tx
                            .send(ContainerTerminalServerMessage::TerminalOutput(
                                ContainerTerminalOutputPayload {
                                    session_id: output_exec_id.clone(),
                                    data,
                                },
                            ))
                            .await
                            .is_err()
                    {
                        return;
                    }
                }
                Err(err) => {
                    let _ = output_tx
                        .send(ContainerTerminalServerMessage::TerminalError(
                            ContainerTerminalErrorPayload {
                                message: format!("failed to read terminal output: {err}"),
                            },
                        ))
                        .await;
                    return;
                }
            }
        }

        let exit_code = docker_for_output
            .inspect_exec(&output_exec_id)
            .await
            .ok()
            .and_then(|v| v.exit_code);
        let _ = output_tx
            .send(ContainerTerminalServerMessage::TerminalExit(
                ContainerTerminalExitPayload {
                    session_id: output_exec_id.clone(),
                    exit_code,
                },
            ))
            .await;
        let _ = output_cleanup_tx.send(output_exec_id).await;
    });

    Ok((
        TerminalSession {
            exec_id: exec.id,
            input,
            output_task,
        },
        output_shell,
    ))
}

async fn handle_terminal_input(
    payload: ContainerTerminalInputPayload,
    tx: mpsc::Sender<ContainerTerminalServerMessage>,
    sessions: &TerminalSessionMap,
) -> Result<(), ()> {
    let Some(session) = sessions.get(&payload.session_id) else {
        let _ = tx
            .send(ContainerTerminalServerMessage::TerminalError(
                ContainerTerminalErrorPayload {
                    message: "terminal session does not exist".to_string(),
                },
            ))
            .await;
        return Err(());
    };

    let mut writer = session.input.lock().await;
    if let Err(err) = writer.write_all(payload.data.as_bytes()).await {
        let _ = tx
            .send(ContainerTerminalServerMessage::TerminalError(
                ContainerTerminalErrorPayload {
                    message: format!("failed to write terminal input: {err}"),
                },
            ))
            .await;
        return Err(());
    }
    let _ = writer.flush().await;
    Ok(())
}

async fn handle_terminal_resize(
    payload: ContainerTerminalResizePayload,
    state: Arc<AppState>,
    tx: mpsc::Sender<ContainerTerminalServerMessage>,
    sessions: &TerminalSessionMap,
) -> Result<(), ()> {
    let Some(session) = sessions.get(&payload.session_id) else {
        let _ = tx
            .send(ContainerTerminalServerMessage::TerminalError(
                ContainerTerminalErrorPayload {
                    message: "terminal session does not exist".to_string(),
                },
            ))
            .await;
        return Err(());
    };

    let docker = match state.docker_client().await {
        Ok(client) => client,
        Err(err) => {
            let _ = tx
                .send(ContainerTerminalServerMessage::TerminalError(
                    ContainerTerminalErrorPayload {
                        message: format!("failed to connect Docker: {err:?}"),
                    },
                ))
                .await;
            return Err(());
        }
    };

    if let Err(err) = docker
        .resize_exec(
            &session.exec_id,
            ResizeExecOptions {
                width: payload.cols.max(1),
                height: payload.rows.max(1),
            },
        )
        .await
    {
        let _ = tx
            .send(ContainerTerminalServerMessage::TerminalError(
                ContainerTerminalErrorPayload {
                    message: format!("failed to resize terminal: {err}"),
                },
            ))
            .await;
        return Err(());
    }

    Ok(())
}

async fn handle_terminal_close(
    payload: ContainerTerminalClosePayload,
    tx: mpsc::Sender<ContainerTerminalServerMessage>,
    sessions: &mut TerminalSessionMap,
) -> Result<(), ()> {
    let Some(session) = sessions.remove(&payload.session_id) else {
        return Ok(());
    };

    session.output_task.abort();
    let _ = tx
        .send(ContainerTerminalServerMessage::TerminalExit(
            ContainerTerminalExitPayload {
                session_id: payload.session_id,
                exit_code: None,
            },
        ))
        .await;
    Ok(())
}

/// 格式化 Docker 日志行为可读的字符串
fn format_log_output(log: LogOutput) -> Option<String> {
    let message_bytes = match log {
        LogOutput::StdOut { message } => message,
        LogOutput::StdErr { message } => message,
        _ => return None,
    };

    let line = String::from_utf8_lossy(&message_bytes);

    if let Some(space_index) = line.find(' ') {
        let timestamp_part = &line[..space_index];
        if let Ok(datetime_utc) = timestamp_part.parse::<DateTime<chrono::Utc>>() {
            let local_time: DateTime<Local> = datetime_utc.with_timezone(&Local);
            let message_part = line[space_index + 1..].trim();
            return Some(format!(
                "{} {}",
                local_time.format("[%Y/%m/%d %H:%M:%S]"),
                message_part
            ));
        }
    }
    Some(line.trim().to_string())
}
