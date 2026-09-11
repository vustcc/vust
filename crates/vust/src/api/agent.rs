//! 节点运行时代理 API：将前端请求转发至节点执行面。

use crate::api::auth::AuthenticatedAdmin;
use crate::models::node_runtime_client::NodeRuntimeClient;
use crate::services::logging;
use crate::services::node_inventory::get_node_display_name;
use crate::services::runtime_metrics;
use crate::state::AppState;
use crate::types::{ApiError, ApiResult};
use axum::{
    Router,
    body::Body,
    extract::{
        OriginalUri, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, Request, header::HeaderName},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use reqwest_websocket::{CloseCode, RequestBuilderExt};
use std::sync::Arc;
use tracing::{debug, info, warn};
use vust_contracts::api::ErrorCode;

const ACTOR_KIND_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-kind");
const ACTOR_USER_ID_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-user-id");
const ACTOR_NAME_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-name");
const CLIENT_IP_HEADER: HeaderName = HeaderName::from_static("x-vust-client-ip");
const TRACE_ID_HEADER: HeaderName = HeaderName::from_static("x-vust-trace-id");

fn inject_trusted_operation_context(
    headers: &mut HeaderMap,
    actor_user_id: i64,
    actor_name: &str,
    client_ip: &str,
    trace_id: &str,
) {
    for name in [
        &ACTOR_KIND_HEADER,
        &ACTOR_USER_ID_HEADER,
        &ACTOR_NAME_HEADER,
        &CLIENT_IP_HEADER,
        &TRACE_ID_HEADER,
    ] {
        headers.remove(name);
    }
    headers.insert(&ACTOR_KIND_HEADER, HeaderValue::from_static("user"));
    if let Ok(value) = HeaderValue::from_str(&actor_user_id.to_string()) {
        headers.insert(&ACTOR_USER_ID_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(actor_name) {
        headers.insert(&ACTOR_NAME_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(client_ip) {
        headers.insert(&CLIENT_IP_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(trace_id) {
        headers.insert(&TRACE_ID_HEADER, value);
    }
}

fn prepare_proxy_headers(parts: &axum::http::request::Parts) -> HeaderMap {
    let mut headers = parts.headers.clone();
    let Some(admin) = parts.extensions.get::<AuthenticatedAdmin>() else {
        for name in [
            &ACTOR_KIND_HEADER,
            &ACTOR_USER_ID_HEADER,
            &ACTOR_NAME_HEADER,
            &CLIENT_IP_HEADER,
            &TRACE_ID_HEADER,
        ] {
            headers.remove(name);
        }
        return headers;
    };
    let client_ip = admin
        .session
        .client_ip
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let trace_id = logging::resolve_trace_id(&headers);
    inject_trusted_operation_context(
        &mut headers,
        admin.id,
        &admin.username,
        &client_ip,
        &trace_id,
    );
    headers
}

/// 为 WebSocket 上游连接仅构造可信操作上下文，避免转发客户端的升级控制头。
fn prepare_websocket_proxy_headers(
    admin: &AuthenticatedAdmin,
    incoming_headers: &HeaderMap,
) -> HeaderMap {
    let client_ip = admin.session.client_ip.as_deref().unwrap_or("unknown");
    let trace_id = logging::resolve_trace_id(incoming_headers);
    let mut headers = HeaderMap::new();
    inject_trusted_operation_context(
        &mut headers,
        admin.id,
        &admin.username,
        client_ip,
        &trace_id,
    );
    headers
}

fn rebuild_scoped_proxy_path(path_with_query: &str, node_id: &str) -> String {
    let prefix = format!("/api/v1/node/{node_id}");
    match path_with_query.strip_prefix(&prefix) {
        Some(suffix) => format!("/api/v1{}", suffix),
        None => path_with_query.to_string(),
    }
}

fn ensure_proxy_path_allowed(path_with_query: &str) -> ApiResult<()> {
    let path = path_with_query.split('?').next().unwrap_or(path_with_query);
    if path == "/api/v1/agent/disks"
        || path.starts_with("/api/v1/agent/disks/")
        || path == "/api/v1/agent/disk-operations"
        || path.starts_with("/api/v1/agent/disk-operations/")
    {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "disk management must use the node semantic API",
        ));
    }
    if path == "/api/v1/agent/script-runs" || path.starts_with("/api/v1/agent/script-runs/") {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "script runs must use the script library semantic API",
        ));
    }
    if path == "/api/v1/agent/fs"
        || path.starts_with("/api/v1/agent/fs/")
        || path == "/api/v1/agent/files"
        || path.starts_with("/api/v1/agent/files/")
    {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "file operations must use the node semantic gateway",
        ));
    }
    if path.starts_with("/api/v1/agent/system-monitoring/") {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "system monitoring must use the node semantic gateway",
        ));
    }
    if path == "/api/v1/agent/firewall" || path.starts_with("/api/v1/agent/firewall/") {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "firewall observation must use the node semantic gateway",
        ));
    }
    if path == "/api/v1/agent/websocket/process-manager/ws"
        || path == "/api/v1/agent/processes"
        || path.starts_with("/api/v1/agent/processes/")
        || path == "/api/v1/agent/network-connections"
        || path.starts_with("/api/v1/agent/network-connections/")
        || path == "/api/v1/agent/process"
        || path.starts_with("/api/v1/agent/process/")
    {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "process operations must use the node semantic gateway",
        ));
    }
    Ok(())
}

async fn resolve_node_name(state: &Arc<AppState>, node_id: Option<&str>) -> Option<String> {
    match node_id {
        Some("local") => Some("local".to_string()),
        Some(id) => get_node_display_name(&state.metadata_db, id)
            .await
            .ok()
            .flatten(),
        None => None,
    }
}

/// `vust` 服务中用于代理 WebSocket 请求到节点运行时的处理器。
///
/// 当前端发起本地 Agent WebSocket 连接到 `/api/v1/agent/websocket/{*path}` 时，此处理器会被调用。
/// 它负责以下职责：
/// 1. 接收前端的 WebSocket 升级请求 (`WebSocketUpgrade`)。
/// 2. 使用 `runtime_client` 向节点执行面建立一个新的 WebSocket 连接。
/// 3. 在客户端和节点执行面之间建立双向消息中继。
/// 4. 处理 WebSocket 关闭握手和错误，确保连接的正确关闭和资源释放。
///
/// 远端 Agent 必须使用 `/api/v1/nodes/{node_id}/agent/websocket/{*path}`。
/// 注意：此函数不处理常规 HTTP 请求，常规 HTTP 请求由 `proxy_handler` 处理。
pub async fn websocket_proxy_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    admin: AuthenticatedAdmin,
    incoming_headers: HeaderMap,
) -> Response {
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(uri.path())
        .to_string();
    if path.ends_with("/websocket/host-terminal/ws") {
        return ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "host terminal must use the node terminal gateway",
        )
        .into_response();
    }

    let node_name = resolve_node_name(&state, None).await;
    let headers = prepare_websocket_proxy_headers(&admin, &incoming_headers);
    ws.on_upgrade(move |socket| async move {
        match NodeRuntimeClient::from_node_route(&state.metadata_db, None).await {
            Ok(client) => handle_socket(socket, Arc::new(client), path, headers).await,
            Err(err) => {
                runtime_metrics::record_proxy_ws(false);
                warn!(
                    "Failed to init node runtime client: node_id={:?}, node_name={:?}, err={:?}",
                    Option::<&str>::None,
                    node_name,
                    err
                );
            }
        }
    })
    .into_response()
}

/// 基于显式 `node_id` 的 WebSocket 代理处理器。
pub async fn websocket_proxy_handler_for_node(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    axum::extract::Path((node_id, _path)): axum::extract::Path<(String, String)>,
    admin: AuthenticatedAdmin,
    incoming_headers: HeaderMap,
) -> impl IntoResponse {
    let raw_path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(uri.path())
        .to_string();
    if raw_path.ends_with("/websocket/host-terminal/ws") {
        return ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "host terminal must use the node terminal gateway",
        )
        .into_response();
    }
    let path = rebuild_scoped_proxy_path(&raw_path, &node_id);
    let node_name = resolve_node_name(&state, Some(node_id.as_str())).await;
    let headers = prepare_websocket_proxy_headers(&admin, &incoming_headers);
    ws.on_upgrade(move |socket| async move {
        match NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id.as_str())).await {
            Ok(client) => handle_socket(socket, Arc::new(client), path, headers).await,
            Err(err) => {
                runtime_metrics::record_proxy_ws(false);
                warn!(
                    "Failed to init scoped node runtime client: node_id={:?}, node_name={:?}, err={:?}",
                    node_id, node_name, err
                );
            }
        }
    })
}

/// 处理客户端 WebSocket 连接的实际逻辑，实现与节点运行时之间的双向数据中继。
///
/// 此函数在一个 `tokio` 任务中运行，负责：
/// 1. 与节点执行面建立 WebSocket 连接。
/// 2. 将客户端 WebSocket 分割为发送端和接收端。
/// 3. 将目标运行时 WebSocket 分割为发送端和接收端。
/// 4. 创建两个异步任务：
///    - `client_to_target`: 从客户端接收消息并转发给目标运行时。
///    - `target_to_client`: 从目标运行时接收消息并转发给客户端。
/// 5. 确保在任一方向连接中断或出错时，两个任务都能优雅地停止，并关闭整个 WebSocket 连接。
/// 6. 对 WebSocket 消息（Text, Binary, Ping, Pong, Close）进行类型转换，以适配 `axum` 和
///    `reqwest_websocket` 库之间的消息格式。
async fn handle_socket(
    mut client_ws: WebSocket,
    runtime_client: Arc<NodeRuntimeClient>,
    path: String,
    headers: HeaderMap,
) {
    let url = runtime_client.build_ws_uri(&path);
    info!("Attempting to connect to target WebSocket: {}", url);

    // Establish outgoing WebSocket connection using reqwest-websocket
    let target_ws_result = runtime_client
        .authorize_request(runtime_client.client.get(&url).headers(headers))
        .upgrade()
        .send()
        .await;

    let target_ws = match target_ws_result {
        Ok(response) => match response.into_websocket().await {
            Ok(ws) => {
                runtime_metrics::record_proxy_ws(true);
                info!("Successfully connected to target WebSocket.");
                ws
            }
            Err(e) => {
                runtime_metrics::record_proxy_ws(false);
                warn!("Failed to upgrade connection to WebSocket: {:?}", e);
                let _ = client_ws
                    .send(Message::Text(format!("Proxy error: {}", e).into()))
                    .await;
                let _ = client_ws.close().await;
                return;
            }
        },
        Err(e) => {
            runtime_metrics::record_proxy_ws(false);
            warn!("Failed to send upgrade request to target: {:?}", e);
            let _ = client_ws
                .send(Message::Text(format!("Proxy error: {}", e).into()))
                .await;
            let _ = client_ws.close().await;
            return;
        }
    };

    let (mut client_sender, mut client_receiver) = client_ws.split();
    let (mut target_sender, mut target_receiver) = target_ws.split();

    // Task to forward messages from client to target
    let client_to_target = tokio::spawn(async move {
        let mut sent_close = false;
        while let Some(msg) = client_receiver.next().await {
            match msg {
                Ok(msg) => {
                    debug!("Client -> Target: {:?}", msg);
                    let converted_msg = match msg {
                        Message::Text(t) => reqwest_websocket::Message::Text(t.to_string()),
                        Message::Binary(b) => reqwest_websocket::Message::Binary(b),
                        Message::Ping(p) => reqwest_websocket::Message::Ping(p),
                        Message::Pong(p) => reqwest_websocket::Message::Pong(p),
                        Message::Close(c) => {
                            let code = c.as_ref().map(|c| CloseCode::from(c.code));
                            let reason = c.map(|c| c.reason.to_string());
                            reqwest_websocket::Message::Close {
                                code: code.unwrap_or(CloseCode::Normal),
                                reason: reason.unwrap_or_default(),
                            }
                        }
                    };
                    let is_close =
                        matches!(converted_msg, reqwest_websocket::Message::Close { .. });
                    if let Err(e) = target_sender.send(converted_msg).await {
                        warn!("Failed to send message to target: {:?}", e);
                        break;
                    }
                    if is_close {
                        sent_close = true;
                        break;
                    }
                }
                Err(e) => {
                    warn!("Error receiving message from client: {:?}", e);
                    break;
                }
            }
        }
        // 当客户端（前端）断开，只有在未发送过 Close 帧时，才向目标（Agent）优雅地发送一个关闭帧，避免硬掐断
        if !sent_close {
            let _ = target_sender
                .send(reqwest_websocket::Message::Close {
                    code: CloseCode::Normal,
                    reason: String::new(),
                })
                .await;
        }
        info!("Client to target forwarding stopped.");
    });

    // Task to forward messages from target to client
    let target_to_client = tokio::spawn(async move {
        let mut sent_close = false;
        while let Some(msg) = target_receiver.next().await {
            match msg {
                Ok(msg) => {
                    debug!("Target -> Client: {:?}", msg);
                    let converted_msg = match msg {
                        reqwest_websocket::Message::Text(t) => Message::Text(t.into()),
                        reqwest_websocket::Message::Binary(b) => Message::Binary(b),
                        reqwest_websocket::Message::Ping(p) => Message::Ping(p),
                        reqwest_websocket::Message::Pong(p) => Message::Pong(p),
                        reqwest_websocket::Message::Close { code, reason } => {
                            let close_frame = axum::extract::ws::CloseFrame {
                                code: code.into(),
                                reason: reason.into(),
                            };
                            Message::Close(Some(close_frame))
                        }
                    };
                    let is_close = matches!(converted_msg, Message::Close(_));
                    if let Err(e) = client_sender.send(converted_msg).await {
                        warn!("Failed to send message to client: {:?}", e);
                        break;
                    }
                    if is_close {
                        sent_close = true;
                        break;
                    }
                }
                Err(e) => {
                    let err_str = format!("{:?}", e);
                    if err_str.contains("UnexpectedEof") || err_str.contains("close_notify") {
                        debug!("Target connection ended naturally via TLS EOF: {:?}", e);
                    } else {
                        warn!("Error receiving message from target: {:?}", e);
                    }
                    break;
                }
            }
        }
        // 当目标（Agent）断开，只有在未发送过 Close 帧时，才向客户端（前端）发送关闭通知
        if !sent_close {
            let _ = client_sender.send(Message::Close(None)).await;
        }
        info!("Target to client forwarding stopped.");
    });

    // 等待两个数据中继转发协程全部优雅关闭并自然退出
    let _ = tokio::join!(client_to_target, target_to_client);

    info!("WebSocket proxy connection closed.");
}

/// `vust` 服务中用于代理常规 HTTP 请求到节点运行时的处理器。
///
/// 此处理器是本地 Agent `/api/v1/agent/{*path}` 路由的 `fallback` 处理函数，
/// 意味着所有不匹配其他更具体路由（例如 WebSocket 路由）的
/// `/api/v1/agent/` 下的请求都会由此函数处理。
///
/// 它负责以下职责：
/// 1. 从传入的 `axum::Request<Body>` 中提取原始的 HTTP 方法、URI 路径、请求头和请求体。
/// 2. 使用 `runtime_client` 将完整的 HTTP 请求转发到节点执行面。
/// 3. 将目标运行时返回的 `reqwest::Response` 转换回 `axum::response::Response`
///    并返回给原始客户端（通常是前端）。
/// 4. 过程中处理节点执行面的连接错误、超时等情况，并转换为统一的 `ApiError` 响应。
///
/// 这个代理层使得 `vust` 服务可以作为节点执行面的前置网关，
/// 统一处理认证、路由等逻辑，而节点执行面可以专注于其业务逻辑。
///
/// 远端 Agent 必须使用 `/api/v1/nodes/{node_id}/agent/{*path}`。
pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    OriginalUri(ori): OriginalUri,
    req: Request<Body>,
) -> ApiResult<Response> {
    let (parts, body) = req.into_parts();
    let headers = prepare_proxy_headers(&parts);
    let method = parts.method;

    let path_with_query = ori
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(ori.path());
    ensure_proxy_path_allowed(path_with_query)?;

    let node_name = resolve_node_name(&state, None).await;
    let runtime_client = match NodeRuntimeClient::from_node_route(&state.metadata_db, None).await {
        Ok(client) => client,
        Err(err) => {
            runtime_metrics::record_proxy_http_failure(&err);
            warn!(
                "Failed to init node runtime client: node_id={:?}, node_name={:?}, err={:?}",
                Option::<&str>::None,
                node_name,
                err
            );
            return Err(err);
        }
    };
    let resp = match runtime_client
        .forward_streaming(method.clone(), path_with_query, headers, body)
        .await
    {
        Ok(resp) => {
            runtime_metrics::record_proxy_http_success();
            resp
        }
        Err(err) => {
            runtime_metrics::record_proxy_http_failure(&err);
            return Err(err);
        }
    };
    Ok(resp)
}

/// 基于显式 `node_id` 的常规 HTTP 代理处理器。
pub async fn proxy_handler_for_node(
    State(state): State<Arc<AppState>>,
    OriginalUri(ori): OriginalUri,
    axum::extract::Path((node_id, _path)): axum::extract::Path<(String, String)>,
    req: Request<Body>,
) -> ApiResult<Response> {
    let (parts, body) = req.into_parts();
    let headers = prepare_proxy_headers(&parts);
    let method = parts.method;

    let raw_path = ori
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(ori.path());
    let path_with_query = rebuild_scoped_proxy_path(raw_path, &node_id);
    ensure_proxy_path_allowed(&path_with_query)?;
    let node_name = resolve_node_name(&state, Some(node_id.as_str())).await;

    let runtime_client = match NodeRuntimeClient::from_node_route(
        &state.metadata_db,
        Some(node_id.as_str()),
    )
    .await
    {
        Ok(client) => client,
        Err(err) => {
            runtime_metrics::record_proxy_http_failure(&err);
            warn!(
                "Failed to init scoped node runtime client: node_id={:?}, node_name={:?}, err={:?}",
                node_id, node_name, err
            );
            return Err(err);
        }
    };
    let resp = runtime_client
        .forward_streaming(method.clone(), &path_with_query, headers, body)
        .await;
    match resp {
        Ok(resp) => {
            runtime_metrics::record_proxy_http_success();
            Ok(resp)
        }
        Err(err) => {
            runtime_metrics::record_proxy_http_failure(&err);
            Err(err)
        }
    }
}

/// 注册节点运行时代理路由。
///
/// 此路由器聚合了所有需要代理到节点执行面的请求。
/// 它包括两类代理：
/// 1. `/websocket/{*path}`: WebSocket 请求，由 `websocket_proxy_handler` 处理。
/// 2. `/{*path}`: 所有其他常规 HTTP 请求，由 `proxy_handler` 作为 `fallback` 处理。
///
/// 前端发往 `/api/v1/agent/...` 的请求，都会先由 `vust` 服务的根路由器转发到这里。
/// 此路由器内部再根据路径判断是 WebSocket 请求还是常规 HTTP 请求，并转发给对应的处理器。
pub fn agent_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/websocket/{*path}", get(websocket_proxy_handler))
        .route("/{*path}", any(proxy_handler))
}

#[cfg(test)]
mod tests {
    use super::{
        ACTOR_KIND_HEADER, ACTOR_NAME_HEADER, ACTOR_USER_ID_HEADER, CLIENT_IP_HEADER,
        TRACE_ID_HEADER, ensure_proxy_path_allowed, inject_trusted_operation_context,
        rebuild_scoped_proxy_path,
    };
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn trusted_context_overwrites_spoofed_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(&ACTOR_KIND_HEADER, HeaderValue::from_static("system"));
        headers.insert(&ACTOR_NAME_HEADER, HeaderValue::from_static("spoofed"));

        inject_trusted_operation_context(&mut headers, 1, "admin", "192.0.2.10", "trace-1");

        assert_eq!(headers[&ACTOR_KIND_HEADER], "user");
        assert_eq!(headers[&ACTOR_NAME_HEADER], "admin");
        assert_eq!(headers[&ACTOR_USER_ID_HEADER], "1");
        assert_eq!(headers[&CLIENT_IP_HEADER], "192.0.2.10");
        assert_eq!(headers[&TRACE_ID_HEADER], "trace-1");
    }

    #[test]
    fn rebuild_scoped_proxy_path_rewrites_node_scoped_prefix() {
        let path = "/api/v1/node/node-1/agent/docker/containers?all=true";
        let rewritten = rebuild_scoped_proxy_path(path, "node-1");
        assert_eq!(rewritten, "/api/v1/agent/docker/containers?all=true");
    }

    #[test]
    fn rebuild_scoped_proxy_path_keeps_unmatched_path() {
        let path = "/api/v1/agent/docker/containers";
        let rewritten = rebuild_scoped_proxy_path(path, "node-1");
        assert_eq!(rewritten, path);
    }

    #[test]
    fn rebuild_scoped_proxy_path_preserves_websocket_path_and_query() {
        let path = "/api/v1/node/node-1/agent/websocket/events/ws?stream=1";
        let rewritten = rebuild_scoped_proxy_path(path, "node-1");
        assert_eq!(rewritten, "/api/v1/agent/websocket/events/ws?stream=1");
    }

    #[test]
    fn rebuild_scoped_proxy_path_only_rewrites_matching_node_prefix() {
        let path = "/api/v1/node/node-2/agent/system/summary";
        let rewritten = rebuild_scoped_proxy_path(path, "node-1");
        assert_eq!(rewritten, path);
    }

    #[test]
    fn semantic_system_monitoring_cannot_bypass_master_gateway() {
        assert!(ensure_proxy_path_allowed("/api/v1/agent/system-monitoring/settings").is_err());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/system/about").is_ok());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/firewall/rules/list").is_err());
    }

    #[test]
    fn semantic_file_api_cannot_bypass_master_gateway() {
        assert!(ensure_proxy_path_allowed("/api/v1/agent/fs/ls?path=/").is_err());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/files/list?path=/").is_err());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/system/about").is_ok());
    }

    #[test]
    fn semantic_process_api_cannot_bypass_master_gateway() {
        assert!(ensure_proxy_path_allowed("/api/v1/agent/websocket/process-manager/ws").is_err());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/processes/list").is_err());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/network-connections/list").is_err());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/process/abc/terminate").is_err());
    }

    #[test]
    fn semantic_script_run_api_cannot_bypass_master_gateway() {
        assert!(ensure_proxy_path_allowed("/api/v1/agent/script-runs").is_err());
        assert!(ensure_proxy_path_allowed("/api/v1/agent/script-runs/run-1/cancel").is_err());
    }
}
