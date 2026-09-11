//! Docker 操作上下文：解析 Master 注入的可信操作者信息并构造日志记录。

use crate::models::docker::{DockerActivityActorKind, DockerActivityLevel, DockerActivityOutcome};
use crate::services::docker_activity::{self, NewDockerActivity};
use crate::state::DbPool;
use crate::types::ApiError;
use axum::{
    extract::FromRequestParts,
    extract::Request,
    http::{HeaderMap, HeaderName},
    middleware::Next,
    response::Response,
};
use serde_json::Value;

const ACTOR_KIND_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-kind");
const ACTOR_USER_ID_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-user-id");
const ACTOR_NAME_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-name");
const CLIENT_IP_HEADER: HeaderName = HeaderName::from_static("x-vust-client-ip");
const TRACE_ID_HEADER: HeaderName = HeaderName::from_static("x-vust-trace-id");

/// 已由 Master 验证并注入的 Docker 操作身份。
#[derive(Debug, Clone)]
pub struct DockerOperationContext {
    pub actor_kind: DockerActivityActorKind,
    pub actor_user_id: Option<i64>,
    pub actor_name: String,
    pub client_ip: Option<String>,
    pub trace_id: Option<String>,
}

impl<S> FromRequestParts<S> for DockerOperationContext
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or(ApiError::ForbiddenResource)
    }
}

impl DockerOperationContext {
    /// 从 Master 注入的可信内部请求头建立操作上下文。
    pub fn from_trusted_headers(headers: &HeaderMap) -> Option<Self> {
        parse_context(headers)
    }

    /// 创建供后台任务使用的系统身份。
    pub fn system(source: impl Into<String>) -> Self {
        Self {
            actor_kind: DockerActivityActorKind::System,
            actor_user_id: None,
            actor_name: source.into(),
            client_ip: None,
            trace_id: None,
        }
    }

    /// 判断请求是否由 Master 镜像获取编排发起。
    pub fn is_image_acquisition(&self) -> bool {
        self.actor_kind == DockerActivityActorKind::System && self.actor_name == "image-acquisition"
    }

    /// 隐藏内部服务名，使用稳定的系统操作者写入用户可见审计。
    pub fn into_public_system_actor(mut self) -> Self {
        if self.actor_kind == DockerActivityActorKind::System {
            self.actor_name = "system".to_string();
        }
        self
    }

    /// 写入成功操作；高影响操作使用警告级别。
    pub async fn record_success(
        &self,
        pool: &DbPool,
        event_code: &str,
        target: Option<(&str, &str)>,
        message_params: Value,
        high_impact: bool,
    ) {
        docker_activity::record(
            pool,
            self.activity(
                event_code,
                target,
                message_params,
                if high_impact {
                    DockerActivityLevel::Warning
                } else {
                    DockerActivityLevel::Info
                },
                DockerActivityOutcome::Success,
                None,
            ),
        )
        .await;
    }

    /// 写入失败操作并保存脱敏后的错误摘要。
    pub async fn record_failure(
        &self,
        pool: &DbPool,
        event_code: &str,
        target: Option<(&str, &str)>,
        message_params: Value,
        error_message: impl Into<String>,
    ) {
        docker_activity::record(
            pool,
            self.activity(
                event_code,
                target,
                message_params,
                DockerActivityLevel::Error,
                DockerActivityOutcome::Failure,
                Some(error_message.into()),
            ),
        )
        .await;
    }

    /// 写入已取消操作，供后台任务终态使用。
    pub async fn record_canceled(
        &self,
        pool: &DbPool,
        event_code: &str,
        target: Option<(&str, &str)>,
        message_params: Value,
    ) {
        docker_activity::record(
            pool,
            self.activity(
                event_code,
                target,
                message_params,
                DockerActivityLevel::Warning,
                DockerActivityOutcome::Canceled,
                None,
            ),
        )
        .await;
    }

    /// 根据操作结果写入成功或失败日志，并原样返回业务结果。
    pub async fn finish<T, E: std::fmt::Display>(
        &self,
        pool: &DbPool,
        event_code: &str,
        target: Option<(&str, &str)>,
        message_params: Value,
        high_impact: bool,
        result: Result<T, E>,
    ) -> Result<T, E> {
        match &result {
            Ok(_) => {
                self.record_success(pool, event_code, target, message_params, high_impact)
                    .await;
            }
            Err(error) => {
                self.record_failure(pool, event_code, target, message_params, error.to_string())
                    .await;
            }
        }
        result
    }

    fn activity(
        &self,
        event_code: &str,
        target: Option<(&str, &str)>,
        message_params: Value,
        level: DockerActivityLevel,
        outcome: DockerActivityOutcome,
        error_message: Option<String>,
    ) -> NewDockerActivity {
        NewDockerActivity {
            actor_kind: self.actor_kind,
            actor_user_id: self.actor_user_id,
            actor_name: self.actor_name.clone(),
            client_ip: self.client_ip.clone(),
            level,
            outcome,
            event_code: event_code.to_string(),
            target_kind: target.map(|(kind, _)| kind.to_string()),
            target_id: target.map(|(_, id)| id.to_string()),
            message_params,
            error_message,
            trace_id: self.trace_id.clone(),
        }
    }
}

/// 从可信内部请求头建立 Docker 操作上下文。
pub async fn operation_context_layer(mut request: Request, next: Next) -> Response {
    if let Some(context) = DockerOperationContext::from_trusted_headers(request.headers()) {
        request.extensions_mut().insert(context);
    }
    next.run(request).await
}

fn parse_context(headers: &HeaderMap) -> Option<DockerOperationContext> {
    let actor_name = header_text(headers, &ACTOR_NAME_HEADER)?;
    let actor_kind = match header_text(headers, &ACTOR_KIND_HEADER).as_deref() {
        Some("system") => DockerActivityActorKind::System,
        Some("user") => DockerActivityActorKind::User,
        _ => return None,
    };
    let actor_user_id = match actor_kind {
        DockerActivityActorKind::User => Some(
            header_text(headers, &ACTOR_USER_ID_HEADER)?
                .parse::<i64>()
                .ok()
                .filter(|value| *value > 0)?,
        ),
        DockerActivityActorKind::System => None,
    };
    Some(DockerOperationContext {
        actor_kind,
        actor_user_id,
        actor_name,
        client_ip: header_text(headers, &CLIENT_IP_HEADER),
        trace_id: header_text(headers, &TRACE_ID_HEADER),
    })
}

fn header_text(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn requires_complete_trusted_actor_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(&ACTOR_KIND_HEADER, HeaderValue::from_static("user"));
        assert!(parse_context(&headers).is_none());

        headers.insert(&ACTOR_NAME_HEADER, HeaderValue::from_static("admin"));
        headers.insert(&ACTOR_USER_ID_HEADER, HeaderValue::from_static("1"));
        let context = parse_context(&headers).unwrap();
        assert_eq!(context.actor_kind, DockerActivityActorKind::User);
        assert_eq!(context.actor_user_id, Some(1));
        assert_eq!(context.actor_name, "admin");
    }

    #[test]
    fn image_acquisition_actor_is_hidden_from_public_audit() {
        let context = DockerOperationContext {
            actor_kind: DockerActivityActorKind::System,
            actor_user_id: None,
            actor_name: "image-acquisition".to_string(),
            client_ip: None,
            trace_id: None,
        };

        assert!(context.is_image_acquisition());
        let context = context.into_public_system_actor();
        assert_eq!(context.actor_name, "system");
        assert!(!context.is_image_acquisition());
    }
}
