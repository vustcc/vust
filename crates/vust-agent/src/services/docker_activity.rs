//! Docker 最终执行审计：仅写入持久 outbox，由 Master 统一查询。

use crate::{
    models::docker::{DockerActivityActorKind, DockerActivityLevel, DockerActivityOutcome},
    state::DbPool,
};
use chrono::Utc;
use serde_json::Value;
use std::collections::BTreeMap;
use vust_contracts::logging::{
    AgentOperationEvent, OperationActor, OperationActorKind, OperationImpact, OperationModule,
    OperationOutcome, OperationParameterValue, OperationTarget,
};

/// 待写入的 Docker 最终操作事件。
#[derive(Debug, Clone)]
pub struct NewDockerActivity {
    pub actor_kind: DockerActivityActorKind,
    pub actor_user_id: Option<i64>,
    pub actor_name: String,
    pub client_ip: Option<String>,
    pub level: DockerActivityLevel,
    pub outcome: DockerActivityOutcome,
    pub event_code: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub message_params: Value,
    pub error_message: Option<String>,
    pub trace_id: Option<String>,
}

/// 持久加入 Agent outbox；不再维护节点本地查询表。
pub async fn record(pool: &DbPool, activity: NewDockerActivity) {
    let parameters = safe_parameters(&activity.message_params);
    let client_ip = activity
        .client_ip
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(64).collect());
    let mut task_id =
        parameter_text(&parameters, "taskId").map(|value| value.chars().take(128).collect());
    let target = match (activity.target_kind, activity.target_id) {
        (Some(kind), Some(id)) if matches!(kind.as_str(), "imagePullTask" | "dockerTask") => {
            task_id = Some(id.chars().take(128).collect());
            None
        }
        (Some(kind), Some(id)) => {
            let display_name = parameter_display_name(&parameters)
                .or_else(|| (!is_opaque_identifier(&id)).then(|| id.chars().take(256).collect()));
            Some(OperationTarget {
                kind: kind.chars().take(64).collect(),
                id: id.chars().take(256).collect(),
                display_name,
                ownership: None,
            })
        }
        _ => None,
    };
    let event = AgentOperationEvent {
        event_id: uuid::Uuid::now_v7().to_string(),
        occurred_at: Utc::now().to_rfc3339(),
        module: if activity.event_code.starts_with("suite_") {
            OperationModule::Suites
        } else {
            OperationModule::Docker
        },
        event_code: activity.event_code.chars().take(128).collect(),
        event_label: None,
        actor: OperationActor {
            kind: match activity.actor_kind {
                DockerActivityActorKind::System => OperationActorKind::System,
                DockerActivityActorKind::User => OperationActorKind::User,
            },
            user_id: activity.actor_user_id,
            display_name: activity.actor_name.trim().chars().take(128).collect(),
        },
        client_ip,
        target,
        outcome: match activity.outcome {
            DockerActivityOutcome::Success => OperationOutcome::Success,
            DockerActivityOutcome::Failure => OperationOutcome::Failure,
            DockerActivityOutcome::Canceled => OperationOutcome::Canceled,
        },
        impact: match activity.level {
            DockerActivityLevel::Info => OperationImpact::Info,
            DockerActivityLevel::Warning => OperationImpact::Warning,
            DockerActivityLevel::Error => OperationImpact::Error,
        },
        trace_id: activity
            .trace_id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
        task_id,
        parameters,
        error_code: None,
        error_summary: activity.error_message.map(|value| sanitize_error(&value)),
    };
    if let Err(error) = crate::services::operation_outbox::enqueue(pool, &event).await {
        tracing::error!(%error, event_id=%event.event_id, "Docker operation audit could not be queued");
    }
}

fn parameter_display_name(
    parameters: &BTreeMap<String, OperationParameterValue>,
) -> Option<String> {
    [
        "targetName",
        "name",
        "imageRef",
        "image",
        "projectName",
        "suiteName",
        "containerName",
        "volumeName",
        "networkName",
    ]
    .into_iter()
    .find_map(|key| match parameters.get(key) {
        Some(OperationParameterValue::String(value)) if !is_opaque_identifier(value) => {
            Some(value.chars().take(256).collect())
        }
        _ => None,
    })
}

fn parameter_text<'a>(
    parameters: &'a BTreeMap<String, OperationParameterValue>,
    key: &str,
) -> Option<&'a str> {
    match parameters.get(key) {
        Some(OperationParameterValue::String(value)) => Some(value),
        _ => None,
    }
}

fn is_opaque_identifier(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
        || (value.len() >= 24 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn safe_parameters(value: &Value) -> BTreeMap<String, OperationParameterValue> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| {
            ![
                "password",
                "token",
                "authorization",
                "secret",
                "command",
                "environment",
            ]
            .iter()
            .any(|item| key.to_ascii_lowercase().contains(item))
        })
        .filter_map(|(key, value)| {
            let value = match value {
                Value::String(value) => {
                    OperationParameterValue::String(value.chars().take(256).collect())
                }
                Value::Number(value) => OperationParameterValue::Number(value.as_f64()?),
                Value::Bool(value) => OperationParameterValue::Boolean(*value),
                _ => return None,
            };
            Some((key.chars().take(64).collect(), value))
        })
        .take(32)
        .collect()
}
fn sanitize_error(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if ["password", "token", "authorization", "secret", "cookie"]
        .iter()
        .any(|key| lower.contains(key))
    {
        "Sensitive error details were redacted".to_string()
    } else {
        value.chars().take(512).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::docker::context::DockerOperationContext;

    #[test]
    fn removes_sensitive_parameters() {
        let values =
            safe_parameters(&serde_json::json!({"name":"demo","token":"secret","list":[1]}));
        assert_eq!(values.len(), 1);
        assert!(values.contains_key("name"));
    }

    #[test]
    fn target_name_never_falls_back_to_opaque_identifier() {
        let parameters = safe_parameters(&serde_json::json!({"name":"web"}));
        assert_eq!(parameter_display_name(&parameters).as_deref(), Some("web"));
        assert!(is_opaque_identifier("019f6f64-cc8d-7a30-9812-845e0f56f185"));
    }

    #[tokio::test]
    async fn canceled_activity_is_persisted_with_safe_parameters() {
        let pool = crate::test_support::setup_test_db().await;
        let context = DockerOperationContext::system("test");
        context
            .record_canceled(
                &pool,
                "docker_image_pull",
                Some(("imagePullTask", "task-1")),
                serde_json::json!({
                    "imageRef": "nginx:latest",
                    "command": "must-not-be-stored",
                }),
            )
            .await;

        let events = crate::services::operation_outbox::pending(&pool, 10)
            .await
            .expect("load outbox");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_code, "docker_image_pull");
        assert_eq!(events[0].outcome, OperationOutcome::Canceled);
        assert_eq!(events[0].task_id.as_deref(), Some("task-1"));
        assert!(events[0].parameters.contains_key("imageRef"));
        assert!(!events[0].parameters.contains_key("command"));
    }

    #[tokio::test]
    async fn suite_activity_is_persisted_in_suites_module() {
        let pool = crate::test_support::setup_test_db().await;
        let context = DockerOperationContext::system("test");
        context
            .record_success(
                &pool,
                "suite_enable",
                Some(("suite_instance", "instance-1")),
                serde_json::json!({"suiteName": "Packet"}),
                false,
            )
            .await;

        let events = crate::services::operation_outbox::pending(&pool, 10)
            .await
            .expect("load outbox");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_code, "suite_enable");
        assert_eq!(events[0].module, OperationModule::Suites);
    }
}
