//! 套件运行时语义操作事件入口：认证实例、收紧字段并写入 durable outbox。

use crate::{
    api::docker::suites,
    services::operation_outbox,
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{Duration, Utc};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;
use vust_contracts::{
    api::ErrorCode,
    logging::{
        AgentOperationEvent, AgentOperationEventAck, OperationActor, OperationActorKind,
        OperationModule, SuiteOperationEventRequest,
    },
};

pub(crate) const SUITE_OPERATION_CONTEXT_HEADER: &str = "x-vust-operation-context";

/// Agent 保存的可信套件请求用户上下文。
struct TrustedSuiteOperationContext {
    actor_user_id: i64,
    actor_name: String,
    client_ip: String,
    trace_id: String,
}

/// 组装套件运行时操作事件路由。
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/suite-runtime/operation-events", post(submit))
}

async fn submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<SuiteOperationEventRequest>,
) -> ApiResult<Response> {
    let principal = authenticate(&headers).await?;
    validate_request(&payload)?;
    let operation_context = match payload.operation_context_id.as_deref() {
        Some(context_id) => Some(
            resolve_operation_context(
                &state.metadata_db,
                &principal.suite_id,
                &principal.instance_id,
                context_id,
            )
            .await?
            .ok_or_else(|| {
                ApiError::forbidden(
                    ErrorCode::AuthForbidden,
                    "suite operation context is invalid or expired",
                )
            })?,
        ),
        None => None,
    };
    let event_id = payload.event_id.clone();
    let event = build_event(&principal, operation_context.as_ref(), payload);
    operation_outbox::enqueue(&state.metadata_db, &event).await?;
    Ok(ApiResponse::success_with_raw(
        "Suite operation event accepted",
        Some(AgentOperationEventAck {
            accepted_event_ids: vec![event_id],
        }),
    )
    .into_response())
}

/// 使用令牌解析出的受信主体覆盖套件不能声明的来源字段。
fn build_event(
    principal: &suites::SuiteRuntimePrincipal,
    operation_context: Option<&TrustedSuiteOperationContext>,
    payload: SuiteOperationEventRequest,
) -> AgentOperationEvent {
    let event_code = format!(
        "suite_{}_{}",
        normalize_segment(&principal.suite_id),
        payload.event_code
    )
    .chars()
    .take(128)
    .collect();
    AgentOperationEvent {
        event_id: payload.event_id,
        occurred_at: Utc::now().to_rfc3339(),
        module: OperationModule::Suites,
        event_code,
        event_label: Some(payload.event_label),
        actor: operation_context.map_or_else(
            || OperationActor {
                kind: OperationActorKind::Suite,
                user_id: None,
                display_name: format!("{}/{}", principal.suite_id, principal.instance_id),
            },
            |context| OperationActor {
                kind: OperationActorKind::User,
                user_id: Some(context.actor_user_id),
                display_name: context.actor_name.clone(),
            },
        ),
        client_ip: operation_context.map(|context| context.client_ip.clone()),
        target: payload.target,
        outcome: payload.outcome,
        impact: payload.impact,
        trace_id: operation_context.map_or_else(
            || Uuid::now_v7().to_string(),
            |context| context.trace_id.clone(),
        ),
        task_id: payload.task_id,
        parameters: payload.parameters,
        error_code: payload.error_code,
        error_summary: payload.error_summary,
    }
}

/// 为一次可信套件代理请求签发与实例绑定的操作上下文 ID。
pub(crate) async fn issue_operation_context(
    pool: &crate::state::DbPool,
    suite_id: &str,
    instance_id: &str,
    context: &crate::api::docker::context::DockerOperationContext,
) -> ApiResult<String> {
    let now = Utc::now();
    sqlx::query("DELETE FROM suite_operation_contexts WHERE expires_at <= ?")
        .bind(now.to_rfc3339())
        .execute(pool)
        .await?;
    let context_id = Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO suite_operation_contexts (
            context_id, suite_id, instance_id, actor_user_id, actor_name,
            client_ip, trace_id, created_at, expires_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&context_id)
    .bind(suite_id)
    .bind(instance_id)
    .bind(context.actor_user_id.ok_or_else(|| {
        ApiError::forbidden(ErrorCode::AuthForbidden, "suite proxy user id is required")
    })?)
    .bind(&context.actor_name)
    .bind(context.client_ip.as_deref().ok_or_else(|| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "suite proxy client IP is required",
        )
    })?)
    .bind(context.trace_id.as_deref().ok_or_else(|| {
        ApiError::forbidden(ErrorCode::AuthForbidden, "suite proxy trace ID is required")
    })?)
    .bind(now.to_rfc3339())
    .bind((now + Duration::days(7)).to_rfc3339())
    .execute(pool)
    .await?;
    Ok(context_id)
}

async fn resolve_operation_context(
    pool: &crate::state::DbPool,
    suite_id: &str,
    instance_id: &str,
    context_id: &str,
) -> ApiResult<Option<TrustedSuiteOperationContext>> {
    let row = sqlx::query(
        "SELECT actor_user_id, actor_name, client_ip, trace_id
         FROM suite_operation_contexts
         WHERE context_id = ? AND suite_id = ? AND instance_id = ? AND expires_at > ?",
    )
    .bind(context_id)
    .bind(suite_id)
    .bind(instance_id)
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(pool)
    .await?;
    row.map(|row| -> Result<TrustedSuiteOperationContext, sqlx::Error> {
        Ok(TrustedSuiteOperationContext {
            actor_user_id: row.try_get("actor_user_id")?,
            actor_name: row.try_get("actor_name")?,
            client_ip: row.try_get("client_ip")?,
            trace_id: row.try_get("trace_id")?,
        })
    })
    .transpose()
    .map_err(ApiError::from)
}

async fn authenticate(headers: &HeaderMap) -> ApiResult<suites::SuiteRuntimePrincipal> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::forbidden(ErrorCode::AuthForbidden, "suite runtime token is required")
        })?;
    let principal = suites::authenticate_suite_runtime(token).await?;
    ensure_capability(&principal)?;
    Ok(principal)
}

fn ensure_capability(principal: &suites::SuiteRuntimePrincipal) -> ApiResult<()> {
    if !principal
        .capabilities
        .iter()
        .any(|value| value == "operation-logs.write")
    {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "suite operation log capability is not granted",
        ));
    }
    Ok(())
}

fn validate_request(payload: &SuiteOperationEventRequest) -> ApiResult<()> {
    let event_id = Uuid::parse_str(&payload.event_id)
        .map_err(|_| ApiError::BadRequest("eventId must be a UUIDv7".to_string()))?;
    if event_id.get_version_num() != 7 {
        return Err(ApiError::BadRequest("eventId must be a UUIDv7".to_string()));
    }
    if payload
        .operation_context_id
        .as_deref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 128)
    {
        return Err(ApiError::BadRequest(
            "operationContextId must contain 1 to 128 characters".to_string(),
        ));
    }
    let code = payload.event_code.as_bytes();
    if !(3..=64).contains(&code.len())
        || !code[0].is_ascii_lowercase()
        || !code
            .iter()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || *value == b'_')
    {
        return Err(ApiError::BadRequest(
            "eventCode must use lower snake case".to_string(),
        ));
    }
    for label in [&payload.event_label.zh_cn, &payload.event_label.en_us] {
        if label.trim().is_empty() || label.chars().count() > 128 {
            return Err(ApiError::BadRequest(
                "event labels must contain 1 to 128 characters".to_string(),
            ));
        }
    }
    if payload.parameters.len() > 32
        || payload.parameters.keys().any(|key| {
            let key = key.to_ascii_lowercase();
            [
                "password",
                "token",
                "authorization",
                "secret",
                "cookie",
                "command",
                "environment",
            ]
            .iter()
            .any(|value| key.contains(value))
        })
    {
        return Err(ApiError::BadRequest(
            "operation parameters contain unsupported fields".to_string(),
        ));
    }
    Ok(())
}

fn normalize_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use vust_contracts::logging::{
        OperationEventLabel, OperationImpact, OperationOutcome, OperationParameterValue,
    };

    fn request() -> SuiteOperationEventRequest {
        SuiteOperationEventRequest {
            event_id: Uuid::now_v7().to_string(),
            operation_context_id: None,
            event_code: "capture_started".to_string(),
            event_label: OperationEventLabel {
                zh_cn: "开始抓包".to_string(),
                en_us: "Start capture".to_string(),
            },
            outcome: OperationOutcome::Success,
            impact: OperationImpact::Info,
            target: None,
            task_id: None,
            parameters: BTreeMap::new(),
            error_code: None,
            error_summary: None,
        }
    }

    #[test]
    fn validates_semantic_event_and_rejects_sensitive_parameters() {
        let mut payload = request();
        assert!(validate_request(&payload).is_ok());
        payload.parameters.insert(
            "accessToken".to_string(),
            OperationParameterValue::String("secret".to_string()),
        );
        assert!(validate_request(&payload).is_err());
    }

    #[test]
    fn trusted_principal_controls_actor_module_and_source() {
        let principal = suites::SuiteRuntimePrincipal {
            suite_id: "vust.packet".to_string(),
            instance_id: "instance-7".to_string(),
            capabilities: vec!["operation-logs.write".to_string()],
            runtime_images: Vec::new(),
        };
        let event = build_event(&principal, None, request());
        assert_eq!(event.module, OperationModule::Suites);
        assert_eq!(event.actor.kind, OperationActorKind::Suite);
        assert_eq!(event.actor.display_name, "vust.packet/instance-7");
        assert_eq!(event.event_code, "suite_vust_packet_capture_started");
        assert!(event.client_ip.is_none());
    }

    #[test]
    fn capability_is_required() {
        let principal = suites::SuiteRuntimePrincipal {
            suite_id: "vust.packet".to_string(),
            instance_id: "instance-7".to_string(),
            capabilities: vec!["captures.manage".to_string()],
            runtime_images: Vec::new(),
        };
        assert!(ensure_capability(&principal).is_err());
    }

    #[tokio::test]
    async fn trusted_context_restores_user_ip_and_trace_for_matching_suite() {
        let pool = crate::test_support::setup_test_db().await;
        let proxy_context = crate::api::docker::context::DockerOperationContext {
            actor_kind: crate::models::docker::DockerActivityActorKind::User,
            actor_user_id: Some(7),
            actor_name: "admin".to_string(),
            client_ip: Some("::ffff:10.0.0.41".to_string()),
            trace_id: Some("trace-7".to_string()),
        };
        let context_id =
            issue_operation_context(&pool, "vust.packet", "instance-7", &proxy_context)
                .await
                .unwrap();
        let trusted = resolve_operation_context(&pool, "vust.packet", "instance-7", &context_id)
            .await
            .unwrap()
            .unwrap();
        let principal = suites::SuiteRuntimePrincipal {
            suite_id: "vust.packet".to_string(),
            instance_id: "instance-7".to_string(),
            capabilities: vec!["operation-logs.write".to_string()],
            runtime_images: Vec::new(),
        };
        let event = build_event(&principal, Some(&trusted), request());
        assert_eq!(event.actor.kind, OperationActorKind::User);
        assert_eq!(event.actor.user_id, Some(7));
        assert_eq!(event.actor.display_name, "admin");
        assert_eq!(event.client_ip.as_deref(), Some("::ffff:10.0.0.41"));
        assert_eq!(event.trace_id, "trace-7");

        assert!(
            resolve_operation_context(&pool, "vust.packet", "forged-instance", &context_id,)
                .await
                .unwrap()
                .is_none()
        );
    }
}
