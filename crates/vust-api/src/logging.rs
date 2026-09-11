//! 共享的 HTTP 访问日志中间件，支持根据路由规则（如过滤心跳）选择性记录。

use axum::{body::Body, http::Request, middleware::Next, response::Response};

/// 检查请求路径是否是节点心跳。
pub fn is_runtime_heartbeat(path: &str) -> bool {
    path == "/api/v1/runtime/heartbeat"
}

/// 通用的 HTTP 请求/响应日志中间件。
pub async fn http_log_middleware(req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let method = req.method().to_string();
    let is_heartbeat = is_runtime_heartbeat(&path);

    if !is_heartbeat {
        tracing::info!("started {} {}", method, path);
    }
    let start = std::time::Instant::now();
    let res = next.run(req).await;
    let latency = start.elapsed();
    if !is_heartbeat {
        tracing::info!(
            "finished {} {} with {} in {:?}",
            method,
            path,
            res.status().as_u16(),
            latency
        );
    }
    res
}
