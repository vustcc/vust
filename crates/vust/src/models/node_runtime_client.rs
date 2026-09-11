//! 节点运行时客户端模型：封装 vust 到节点执行面的 HTTP/WS 通信。

use crate::models::node_sessions::{AgentCommandTransport, resolve_active_command_access};
use crate::state::DbPool;
use crate::types::{AgentClientError, ApiError, ApiResult, agent_socket_path};
use axum::{
    body::Body,
    http::{HeaderMap, header},
    response::Response,
};
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::error::Error;
use std::time::Duration;
use tokio_stream::StreamExt;
use vust_contracts::api::{ApiResponse as ContractApiResponse, ErrorCode};
use vust_security::client::build_mtls_client;

const PROXY_CONNECT_TIMEOUT_SECS: u64 = 3;
const PROXY_REQUEST_TIMEOUT_SECS: u64 = 20;
const PROXY_LONG_RUNNING_REQUEST_TIMEOUT_SECS: u64 = 600;
const PROXY_FILE_STREAM_TIMEOUT_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Deserialize)]
struct RuntimeErrorResponse {
    success: bool,
    message: String,
    data: Option<Value>,
    #[serde(rename = "errorCode")]
    error_code: Option<Value>,
}

/// 封装与节点运行时的 HTTP/WS 通讯与认证。
#[derive(Debug, Clone)]
pub struct NodeRuntimeClient {
    pub client: Client,
    base_url: String,
    ws_base_url: String,
    command_credential: Option<String>,
}

/// Master 向 Agent 发起变更请求时注入的可信操作者上下文。
#[derive(Debug, Clone)]
pub struct AgentOperationContext {
    pub actor_user_id: i64,
    pub actor_name: String,
    pub client_ip: String,
    pub trace_id: String,
}

impl NodeRuntimeClient {
    /// 根据目标节点标识选择 UDS 或 TLS 连接。
    pub async fn from_node_route(pool: &DbPool, node_id: Option<&str>) -> ApiResult<Self> {
        let target = node_id.unwrap_or("local");
        let access = resolve_active_command_access(pool, target)
            .await
            .map_err(|err| ApiError::Internal(err.to_string()))?
            .ok_or(AgentClientError::NotFound)?;
        match access.transport {
            AgentCommandTransport::Uds => Self::from_uds(Some(access.credential)),
            AgentCommandTransport::Https => {
                let address = access
                    .session
                    .advertise_addr
                    .zip(access.session.listen_port)
                    .map(|(host, port)| format!("https://{host}:{port}"))
                    .ok_or(AgentClientError::NotFound)?;
                Self::from_tls(address, Some(access.credential))
            }
        }
        .map_err(|err| ApiError::Internal(err.to_string()))
    }

    /// 从给定的 API 地址创建 TLS 客户端。
    pub fn from_api_url(base_url: String) -> ApiResult<Self> {
        Self::from_tls(base_url, None).map_err(|err| ApiError::Internal(err.to_string()))
    }

    fn from_uds(command_credential: Option<String>) -> anyhow::Result<Self> {
        let socket_path = agent_socket_path();
        if !socket_path.exists() {
            return Err(anyhow::anyhow!(
                "agent unix socket not found: {}",
                socket_path.display()
            ));
        }
        let client = Client::builder()
            .unix_socket(socket_path)
            .connect_timeout(Duration::from_secs(PROXY_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(PROXY_REQUEST_TIMEOUT_SECS))
            .no_proxy()
            .build()?;
        Ok(Self {
            client,
            base_url: "http://local".to_string(),
            ws_base_url: "ws://local".to_string(),
            command_credential,
        })
    }

    fn from_tls(base_url: String, command_credential: Option<String>) -> anyhow::Result<Self> {
        let client = build_mtls_client("vust")?;

        let base_url = base_url.trim_end_matches('/').to_string();
        let ws_base_url = base_url.replace("https://", "wss://");
        Ok(Self {
            client,
            base_url,
            ws_base_url,
            command_credential,
        })
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        match self.command_credential.as_deref() {
            Some(credential) => request.bearer_auth(credential),
            None => request,
        }
    }

    /// 为需要自行完成 WebSocket upgrade 的请求附加节点命令凭据。
    pub fn authorize_request(&self, request: RequestBuilder) -> RequestBuilder {
        self.authorize(request)
    }

    /// 构造运行时 WebSocket URI。
    pub fn build_ws_uri(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{}", self.ws_base_url, path)
        } else {
            format!("{}/{}", self.ws_base_url, path)
        }
    }

    /// 构造运行时 HTTP URI。
    pub fn build_uri(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}/{}", self.base_url, path)
        }
    }

    /// 发起 GET 请求并解析 JSON 响应。
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let url = self.build_uri(path);
        let resp = self.authorize(self.client.get(url)).send().await?;
        decode_json_response("GET", path, resp).await
    }

    /// 发起 POST 请求并解析 JSON 响应。
    pub async fn post_json<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
    ) -> anyhow::Result<T> {
        let url = self.build_uri(path);
        let mut request = self.authorize(self.client.post(url)).json(payload);
        if is_long_running_request(path) {
            request = request.timeout(Duration::from_secs(PROXY_LONG_RUNNING_REQUEST_TIMEOUT_SECS));
        }
        let resp = request.send().await?;
        decode_json_response("POST", path, resp).await
    }

    /// 携带可信操作者上下文发起 POST 请求。
    pub async fn post_json_with_operation_context<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
        context: &AgentOperationContext,
    ) -> anyhow::Result<T> {
        let url = self.build_uri(path);
        let mut request = apply_operation_context(
            self.authorize(self.client.post(url)),
            "user",
            Some(context.actor_user_id),
            &context.actor_name,
            Some(&context.client_ip),
            &context.trace_id,
        )
        .json(payload);
        if is_long_running_request(path) {
            request = request.timeout(Duration::from_secs(PROXY_LONG_RUNNING_REQUEST_TIMEOUT_SECS));
        }
        let response = request.send().await?;
        decode_json_response("POST", path, response).await
    }

    /// 为后台任务请求注入可信系统操作上下文。
    pub fn with_system_operation_context(
        &self,
        request: RequestBuilder,
        source: &str,
        trace_id: &str,
    ) -> RequestBuilder {
        apply_operation_context(
            self.authorize(request),
            "system",
            None,
            source,
            None,
            trace_id,
        )
    }

    /// 携带可信系统操作上下文发起 POST 请求。
    pub async fn post_json_with_system_context<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
        source: &str,
        trace_id: &str,
    ) -> anyhow::Result<T> {
        let url = self.build_uri(path);
        let mut request = self
            .with_system_operation_context(self.client.post(url), source, trace_id)
            .json(payload);
        if is_long_running_request(path) {
            request = request.timeout(Duration::from_secs(PROXY_LONG_RUNNING_REQUEST_TIMEOUT_SECS));
        }
        let response = request.send().await?;
        decode_json_response("POST", path, response).await
    }

    /// 携带可信系统操作上下文发起 DELETE 请求。
    pub async fn delete_json_with_system_context<T: DeserializeOwned>(
        &self,
        path: &str,
        source: &str,
        trace_id: &str,
    ) -> anyhow::Result<T> {
        let url = self.build_uri(path);
        let response = self
            .with_system_operation_context(self.client.delete(url), source, trace_id)
            .send()
            .await?;
        decode_json_response("DELETE", path, response).await
    }

    /// 发起 PUT 请求并解析 JSON 响应。
    pub async fn put_json<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
    ) -> anyhow::Result<T> {
        let url = self.build_uri(path);
        let resp = self
            .authorize(self.client.put(url))
            .json(payload)
            .send()
            .await?;
        decode_json_response("PUT", path, resp).await
    }

    /// 发起 DELETE 请求并解析 JSON 响应。
    pub async fn delete_json<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let url = self.build_uri(path);
        let resp = self.authorize(self.client.delete(url)).send().await?;
        decode_json_response("DELETE", path, resp).await
    }

    /// 读取领域接口并保留 Agent 返回的 HTTP 状态与稳定错误码。
    pub async fn get_domain<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        let response = self
            .authorize(self.client.get(self.build_uri(path)))
            .send()
            .await
            .map_err(AgentClientError::from)?;
        decode_domain_response(path, response).await
    }

    /// 携带可信用户上下文更新领域资源。
    pub async fn put_domain_with_operation_context<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
        context: &AgentOperationContext,
    ) -> ApiResult<T> {
        let response = apply_operation_context(
            self.authorize(self.client.put(self.build_uri(path))),
            "user",
            Some(context.actor_user_id),
            &context.actor_name,
            Some(&context.client_ip),
            &context.trace_id,
        )
        .json(payload)
        .send()
        .await
        .map_err(AgentClientError::from)?;
        decode_domain_response(path, response).await
    }

    /// 携带可信用户上下文创建领域资源或后台任务。
    pub async fn post_domain_with_operation_context<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
        context: &AgentOperationContext,
    ) -> ApiResult<T> {
        let response = apply_operation_context(
            self.authorize(self.client.post(self.build_uri(path))),
            "user",
            Some(context.actor_user_id),
            &context.actor_name,
            Some(&context.client_ip),
            &context.trace_id,
        )
        .json(payload)
        .send()
        .await
        .map_err(AgentClientError::from)?;
        decode_domain_response(path, response).await
    }

    /// 携带可信用户上下文转发文件分块或下载流。
    pub async fn forward_streaming_with_operation_context(
        &self,
        method: Method,
        path: &str,
        mut headers: HeaderMap,
        body: Body,
        context: &AgentOperationContext,
    ) -> ApiResult<Response> {
        for name in [
            "x-vust-actor-kind",
            "x-vust-actor-user-id",
            "x-vust-actor-name",
            "x-vust-client-ip",
            "x-vust-trace-id",
        ] {
            headers.remove(name);
        }
        headers.insert(
            "x-vust-actor-kind",
            header::HeaderValue::from_static("user"),
        );
        if let Ok(value) = header::HeaderValue::from_str(&context.actor_user_id.to_string()) {
            headers.insert("x-vust-actor-user-id", value);
        }
        for (name, value) in [
            ("x-vust-actor-name", context.actor_name.as_str()),
            ("x-vust-client-ip", context.client_ip.as_str()),
            ("x-vust-trace-id", context.trace_id.as_str()),
        ] {
            if let Ok(value) = header::HeaderValue::from_str(value) {
                headers.insert(name, value);
            }
        }
        self.forward_streaming(method, path, headers, body).await
    }

    /// 携带可信用户上下文删除领域资源。
    pub async fn delete_domain_with_operation_context<T: DeserializeOwned>(
        &self,
        path: &str,
        context: &AgentOperationContext,
    ) -> ApiResult<T> {
        let response = apply_operation_context(
            self.authorize(self.client.delete(self.build_uri(path))),
            "user",
            Some(context.actor_user_id),
            &context.actor_name,
            Some(&context.client_ip),
            &context.trace_id,
        )
        .send()
        .await
        .map_err(AgentClientError::from)?;
        decode_domain_response(path, response).await
    }

    /// 通用请求转发
    pub async fn forward_streaming(
        &self,
        method: Method,
        uri_path: &str,
        headers: HeaderMap,
        body: Body,
    ) -> ApiResult<Response> {
        let url = self.build_uri(uri_path);

        let reqwest_stream = body
            .into_data_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other));

        let reqwest_body = reqwest::Body::wrap_stream(reqwest_stream);

        let mut req_builder = self
            .authorize(self.client.request(method, url))
            .headers(headers)
            .body(reqwest_body);

        if is_file_content_stream(uri_path) {
            req_builder = req_builder.timeout(Duration::from_secs(PROXY_FILE_STREAM_TIMEOUT_SECS));
        } else if is_long_running_request(uri_path) {
            req_builder =
                req_builder.timeout(Duration::from_secs(PROXY_LONG_RUNNING_REQUEST_TIMEOUT_SECS));
        }

        let result = req_builder.send().await;
        let resp = match result {
            Ok(resp) => resp,
            Err(err) => {
                let source = err.source().map(|value| value.to_string());
                tracing::error!(
                    "Request node runtime error: status: {:?} >>> {:?} source={:?}",
                    err.status(),
                    err,
                    source
                );
                if err.is_connect() {
                    return Err(AgentClientError::Refused.into());
                } else if err.is_timeout() {
                    return Err(AgentClientError::Timeout.into());
                } else {
                    return Err(AgentClientError::Other(err.to_string()).into());
                };
            }
        };

        reqwest_response_to_axum(resp).await
    }
}

async fn decode_domain_response<T: DeserializeOwned>(
    path: &str,
    response: reqwest::Response,
) -> ApiResult<T> {
    let status = response.status();
    let body = response.text().await.map_err(AgentClientError::from)?;
    decode_domain_body(path, status, &body)
}

/// 解析 Agent 领域响应；失败 envelope 不按成功领域 DTO 反序列化。
fn decode_domain_body<T: DeserializeOwned>(
    path: &str,
    status: StatusCode,
    body: &str,
) -> ApiResult<T> {
    let payload = serde_json::from_str::<ContractApiResponse<Value>>(body).map_err(|error| {
        ApiError::bad_gateway(ErrorCode::AgentRequestFailed, "invalid Agent response")
            .with_detail(format!("{path}: {error}"))
    })?;
    if !status.is_success() || !payload.success {
        let status = axum::http::StatusCode::from_u16(status.as_u16())
            .unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
        let mut error = ApiError::new(
            status,
            payload.error_code.unwrap_or(ErrorCode::AgentRequestFailed),
            payload.message,
        );
        if let Some(message_key) = payload.message_key {
            error = error.with_message_key(message_key);
        }
        return Err(error);
    }

    let data = payload
        .data
        .ok_or_else(|| ApiError::internal("Agent domain response is missing data"))?;
    serde_json::from_value(data).map_err(|error| {
        ApiError::bad_gateway(ErrorCode::AgentRequestFailed, "invalid Agent response")
            .with_detail(format!("{path}: {error}"))
    })
}

fn apply_operation_context(
    request: RequestBuilder,
    actor_kind: &str,
    actor_user_id: Option<i64>,
    actor_name: &str,
    client_ip: Option<&str>,
    trace_id: &str,
) -> RequestBuilder {
    let mut request = request
        .header("x-vust-actor-kind", actor_kind)
        .header("x-vust-actor-name", actor_name)
        .header("x-vust-trace-id", trace_id);
    if let Some(actor_user_id) = actor_user_id {
        request = request.header("x-vust-actor-user-id", actor_user_id);
    }
    match client_ip {
        Some(client_ip) => request.header("x-vust-client-ip", client_ip),
        None => request,
    }
}

fn is_long_running_request(path: &str) -> bool {
    path.contains("/agent/docker/install")
        || path.contains("/agent/docker/suites/install")
        || path.contains("/agent/docker/daemon/settings")
        || path.contains("/agent/docker/system/prune")
        || path
            .split('?')
            .next()
            .is_some_and(|value| value.ends_with("/agent/docker/containers/actions"))
        || is_suite_progress_stream(path)
}

fn is_file_content_stream(path: &str) -> bool {
    path.split('?').next().is_some_and(|value| {
        (value.contains("/agent/files/transfer/") && value.ends_with("/content"))
            || value.ends_with("/agent/docker/images/load")
    })
}

fn is_suite_progress_stream(path: &str) -> bool {
    path.contains("/agent/docker/suite/")
        && path.contains("/proxy/")
        && path
            .split('?')
            .next()
            .is_some_and(|value| value.ends_with("/progress"))
}

async fn decode_json_response<T: DeserializeOwned>(
    method: &'static str,
    path: &str,
    resp: reqwest::Response,
) -> anyhow::Result<T> {
    let status = resp.status();
    let body = resp.text().await?;
    decode_json_body(method, path, status, &body)
}

fn decode_json_body<T: DeserializeOwned>(
    method: &'static str,
    path: &str,
    status: StatusCode,
    body: &str,
) -> anyhow::Result<T> {
    if let Ok(error_response) = serde_json::from_str::<RuntimeErrorResponse>(body)
        && !error_response.success
    {
        let detail = runtime_error_detail(&error_response);
        tracing::warn!(
            method,
            path,
            status = status.as_u16(),
            response_body = %preview_body(body),
            "node runtime returned api error response"
        );
        return Err(anyhow::anyhow!(
            "runtime api error: status={}; message={}; detail={detail}",
            status.as_u16(),
            error_response.message
        ));
    }

    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "runtime api error: status={}; body={}",
            status.as_u16(),
            preview_body(body)
        ));
    }

    match serde_json::from_str::<T>(body) {
        Ok(data) => Ok(data),
        Err(err) => {
            let preview = preview_body(body);
            tracing::error!(
                method,
                path,
                status = status.as_u16(),
                error = %err,
                response_body = %preview,
                "failed to decode node runtime json response"
            );
            Err(anyhow::anyhow!(
                "error decoding response body: {err}; status={}; body={preview}",
                status.as_u16()
            ))
        }
    }
}

fn runtime_error_detail(response: &RuntimeErrorResponse) -> String {
    if let Some(data) = &response.data {
        return match data {
            Value::String(value) => value.clone(),
            other => other.to_string(),
        };
    }
    if let Some(error_code) = &response.error_code {
        return match error_code {
            Value::String(value) => value.clone(),
            other => other.to_string(),
        };
    }
    "empty error detail".to_string()
}

fn preview_body(body: &str) -> String {
    const MAX_CHARS: usize = 2048;
    let mut preview: String = body.chars().take(MAX_CHARS).collect();
    if body.chars().count() > MAX_CHARS {
        preview.push_str("...[truncated]");
    }
    preview
}

async fn reqwest_response_to_axum(resp: reqwest::Response) -> ApiResult<Response> {
    let status = resp.status();
    let mut headers = resp.headers().clone();
    dedupe_response_headers(&mut headers);

    let stream = resp
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));

    let body = Body::from_stream(stream);

    let mut axum_resp = Response::builder().status(status).body(body)?;

    *axum_resp.headers_mut() = headers;

    Ok(axum_resp)
}

fn dedupe_response_headers(headers: &mut HeaderMap) {
    dedupe_vary(headers);

    const MERGE_HEADERS: &[header::HeaderName] = &[
        header::ACCEPT,
        header::ACCEPT_ENCODING,
        header::ACCEPT_LANGUAGE,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::ACCESS_CONTROL_ALLOW_METHODS,
    ];

    for name in MERGE_HEADERS {
        if headers.get(name).is_none() {
            continue;
        }
        let values = headers.get_all(name);
        let mut tokens = std::collections::BTreeSet::new();
        for value in values.iter() {
            if let Ok(text) = value.to_str() {
                for token in text.split(',') {
                    let trimmed = token.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    tokens.insert(trimmed.to_string());
                }
            }
        }
        headers.remove(name);
        if !tokens.is_empty()
            && let Ok(value) =
                header::HeaderValue::from_str(&tokens.into_iter().collect::<Vec<_>>().join(", "))
        {
            headers.insert(name, value);
        }
    }
}

fn dedupe_vary(headers: &mut HeaderMap) {
    if headers.get(header::VARY).is_none() {
        return;
    }
    let values = headers.get_all(header::VARY);

    let mut tokens = std::collections::BTreeSet::new();
    let mut has_wildcard = false;
    for value in values.iter() {
        if let Ok(text) = value.to_str() {
            for token in text.split(',') {
                let trimmed = token.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == "*" {
                    has_wildcard = true;
                    tokens.clear();
                    break;
                }
                tokens.insert(trimmed.to_string());
            }
        }
        if has_wildcard {
            break;
        }
    }

    headers.remove(header::VARY);
    if has_wildcard {
        headers.insert(header::VARY, header::HeaderValue::from_static("*"));
        return;
    }
    if !tokens.is_empty()
        && let Ok(value) =
            header::HeaderValue::from_str(&tokens.into_iter().collect::<Vec<_>>().join(", "))
    {
        headers.insert(header::VARY, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NodeRuntimeClient, StatusCode, apply_operation_context, decode_domain_body,
        decode_json_body, is_file_content_stream, is_long_running_request,
    };
    use crate::models::node_identities::{NodeIdentityRecord, insert_node_identity};
    use crate::models::node_sessions::{NodeSessionRecord, insert_node_session};
    use crate::models::nodes::{NewNodeRecord, insert_node};
    use crate::test_support::setup_test_db;
    use chrono::{Duration, Utc};
    use serde_json::Value;
    use uuid::Uuid;
    use vust_contracts::api::ErrorCode;

    #[test]
    fn system_operation_context_sets_trusted_headers() {
        let request = apply_operation_context(
            reqwest::Client::new().post("http://localhost/test"),
            "system",
            None,
            "image-acquisition",
            None,
            "trace-1",
        )
        .build()
        .unwrap();
        assert_eq!(request.headers()["x-vust-actor-kind"], "system");
        assert_eq!(request.headers()["x-vust-actor-name"], "image-acquisition");
        assert_eq!(request.headers()["x-vust-trace-id"], "trace-1");
        assert!(!request.headers().contains_key("x-vust-client-ip"));
    }

    #[test]
    fn value_response_does_not_hide_runtime_api_error() {
        let result = decode_json_body::<Value>(
            "POST",
            "/api/v1/agent/docker/image-pull-tasks",
            StatusCode::FORBIDDEN,
            r#"{"success":false,"code":403,"message":"access forbidden","errorCode":"AUTH_FORBIDDEN"}"#,
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("status=403"));
        assert!(error.contains("AUTH_FORBIDDEN"));
    }

    #[test]
    fn domain_error_does_not_deserialize_data_as_success_type() {
        let error = decode_domain_body::<Vec<String>>(
            "/api/v1/agent/disks",
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"success":false,"code":503,"message":"disk inventory unavailable","messageKey":"api.errors.DISK_INVENTORY_PARTIAL","errorCode":"DISK_INVENTORY_PARTIAL"}"#,
        )
        .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, ErrorCode::DiskInventoryPartial);
        assert_eq!(error.message, "disk inventory unavailable");
        assert_eq!(
            error.message_key.as_deref(),
            Some("api.errors.DISK_INVENTORY_PARTIAL")
        );
    }

    #[test]
    fn domain_success_deserializes_typed_data() {
        let data = decode_domain_body::<Vec<String>>(
            "/api/v1/agent/example",
            StatusCode::OK,
            r#"{"success":true,"code":200,"message":"ok","data":["value"]}"#,
        )
        .unwrap();

        assert_eq!(data, vec!["value"]);
    }

    #[test]
    fn suite_progress_stream_uses_long_timeout() {
        assert!(is_long_running_request(
            "/api/v1/agent/docker/suite/vust-host-scanner/proxy/main/api/tasks/task_1/progress"
        ));
        assert!(is_long_running_request(
            "/api/v1/agent/docker/suite/vust-host-scanner/proxy/main/api/tasks/task_1/progress?lastEventId=1"
        ));
        assert!(!is_long_running_request(
            "/api/v1/agent/docker/suite/vust-host-scanner/proxy/main/api/tasks/task_1"
        ));
    }

    #[test]
    fn docker_system_prune_uses_long_timeout() {
        assert!(is_long_running_request("/api/v1/agent/docker/system/prune"));
    }

    #[test]
    fn docker_container_batch_actions_use_long_timeout() {
        assert!(is_long_running_request(
            "/api/v1/agent/docker/containers/actions"
        ));
        assert!(is_long_running_request(
            "/api/v1/node/node-1/agent/docker/containers/actions?source=web"
        ));
        assert!(!is_long_running_request(
            "/api/v1/agent/docker/containers/example/stop"
        ));
    }

    #[test]
    fn file_download_uses_streaming_timeout() {
        assert!(is_file_content_stream(
            "/api/v1/agent/files/transfer/transfer-1/content"
        ));
        assert!(is_file_content_stream(
            "/api/v1/agent/files/transfer/transfer-1/content?download=true"
        ));
        assert!(!is_file_content_stream(
            "/api/v1/agent/files/transfer/transfer-1/detail"
        ));
        assert!(is_file_content_stream("/api/v1/agent/docker/images/load"));
        assert!(is_file_content_stream(
            "/api/v1/node/node-1/agent/docker/images/load"
        ));
        assert!(!is_file_content_stream("/api/v1/agent/docker/images"));
    }

    #[tokio::test]
    async fn from_node_route_fails_without_active_session() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        insert_node(
            &pool,
            &NewNodeRecord {
                node_id: node_id.clone(),
                tenant_id: None,
                name: "n1".to_string(),
                normalized_name: "n1".to_string(),
                group_name: "default".to_string(),
                labels: "[]".to_string(),
                description: None,
                desired_role: None,
                schedulable: true,
                metadata: "{}".to_string(),
            },
        )
        .await
        .unwrap();

        let err = NodeRuntimeClient::from_node_route(&pool, Some(&node_id))
            .await
            .expect_err("missing session should fail");
        assert_eq!(err.code, ErrorCode::AgentNotFound);
    }

    #[tokio::test]
    async fn from_node_route_succeeds_with_active_session_endpoint() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        insert_node(
            &pool,
            &NewNodeRecord {
                node_id: node_id.clone(),
                tenant_id: None,
                name: "n2".to_string(),
                normalized_name: "n2".to_string(),
                group_name: "default".to_string(),
                labels: "[]".to_string(),
                description: None,
                desired_role: None,
                schedulable: true,
                metadata: "{}".to_string(),
            },
        )
        .await
        .unwrap();
        insert_node_identity(
            &pool,
            &NodeIdentityRecord {
                identity_id: Uuid::new_v4().to_string(),
                node_id: node_id.clone(),
                agent_id: node_id.clone(),
                certificate_serial_number: None,
                certificate_fingerprint: format!("SHA256:{}", Uuid::new_v4().simple()),
                certificate_status: "active".to_string(),
                public_key_algorithm: "ed25519".to_string(),
                certificate_issued_at: Some(now.to_rfc3339()),
                certificate_expires_at: None,
                rotated_at: None,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();
        let session_id = Uuid::new_v4().to_string();
        insert_node_session(
            &pool,
            &NodeSessionRecord {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
                agent_id: node_id.clone(),
                lease_id: Uuid::new_v4().to_string(),
                advertise_addr: Some("127.0.0.1".to_string()),
                listen_addr: None,
                listen_port: Some(18443),
                server_name: None,
                registered_at: now.to_rfc3339(),
                lease_expires_at: (now + Duration::seconds(60)).to_rfc3339(),
                last_heartbeat_at: Some(now.to_rfc3339()),
                heartbeat_sequence: 1,
                last_seen_at: Some(now.to_rfc3339()),
                status: "active".to_string(),
                close_reason: None,
                closed_at: None,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();
        crate::models::node_sessions::upsert_session_command_access(
            &pool,
            &session_id,
            crate::models::node_sessions::AgentCommandTransport::Https,
            "test-command-credential-which-is-long-enough",
        )
        .await
        .unwrap();

        let client = NodeRuntimeClient::from_node_route(&pool, Some(&node_id)).await;
        assert!(client.is_ok());
    }
}
