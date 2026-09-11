//! 节点任务启动恢复：依据数据库与远端事实收敛或幂等重试中断操作。

use crate::models::node_sessions::get_active_session_by_node_id;
use crate::models::node_tasks::{NodeTaskRecord, list_interrupted_tasks, update_deploy_progress};
use crate::models::nodes::{NodeStatus, get_node_by_id, update_node_status};
use crate::services::node_check::check_node_health;
use crate::services::node_deploy::{
    NodeDeployPayload, deploy_node, recover_interrupted_remote_operation, repair_node,
    uninstall_node,
};
use crate::state::{AppState, DeploySession};
use crate::types::{ApiError, ApiResult};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// 在后台恢复启动时发现的活动节点任务。
pub fn spawn_node_task_recovery(state: Arc<AppState>) {
    tokio::spawn(async move {
        let tasks = match list_interrupted_tasks(&state.metadata_db).await {
            Ok(tasks) => tasks,
            Err(error) => {
                tracing::error!(%error, "failed to load interrupted node tasks");
                return;
            }
        };
        for task in tasks {
            recover_task(&state, &task).await;
        }
    });
}

async fn recover_task(state: &AppState, task: &NodeTaskRecord) {
    persist_recovery_state(
        state,
        task,
        5,
        false,
        "Recovering interrupted node task",
        None,
    )
    .await;
    let result = recover_task_inner(state, task).await;
    let error = result.as_ref().err().map(|error| error.message.to_string());
    persist_recovery_state(
        state,
        task,
        if error.is_some() { 90 } else { 100 },
        true,
        if error.is_some() {
            "Interrupted node task recovery failed"
        } else {
            "Interrupted node task recovered"
        },
        error,
    )
    .await;
}

async fn recover_task_inner(state: &AppState, task: &NodeTaskRecord) -> ApiResult<()> {
    match task.task_type.as_str() {
        "check" => {
            check_node_health(&state.metadata_db, &task.node_id).await?;
            Ok(())
        }
        "uninstall" => {
            if get_node_by_id(&state.metadata_db, &task.node_id)
                .await?
                .is_some_and(|node| node.status == NodeStatus::Retired.as_str())
            {
                return Ok(());
            }
            uninstall_node(&state.metadata_db, &task.node_id, true).await
        }
        "deploy" | "reprovision" => {
            if registration_completed_after_task(state, task).await? {
                return Ok(());
            }
            make_deployment_retryable(state, &task.node_id).await?;
            let reprovision = task.task_type == "reprovision";
            recover_interrupted_remote_operation(&state.metadata_db, &task.node_id, reprovision)
                .await?;
            if reprovision {
                repair_node(
                    &state.metadata_db,
                    &task.node_id,
                    NodeDeployPayload {
                        listen_addr: None,
                        vust_url: None,
                    },
                    None,
                )
                .await
            } else {
                deploy_node(
                    &state.metadata_db,
                    &task.node_id,
                    NodeDeployPayload {
                        listen_addr: None,
                        vust_url: None,
                    },
                    None,
                )
                .await
            }
        }
        _ => Err(ApiError::internal("unsupported interrupted node task type")),
    }
}

async fn registration_completed_after_task(
    state: &AppState,
    task: &NodeTaskRecord,
) -> ApiResult<bool> {
    let Some(session) = get_active_session_by_node_id(&state.metadata_db, &task.node_id).await?
    else {
        return Ok(false);
    };
    let created_at = DateTime::parse_from_rfc3339(&task.created_at)
        .map_err(|_| ApiError::internal("node task has invalid creation time"))?
        .with_timezone(&Utc);
    let registered_at = DateTime::parse_from_rfc3339(&session.registered_at)
        .map_err(|_| ApiError::internal("node session has invalid registration time"))?
        .with_timezone(&Utc);
    let lease_expires_at = DateTime::parse_from_rfc3339(&session.lease_expires_at)
        .map_err(|_| ApiError::internal("node session has invalid lease time"))?
        .with_timezone(&Utc);
    Ok(session.status == "active"
        && session.agent_id == task.node_id
        && registered_at >= created_at
        && lease_expires_at > Utc::now())
}

async fn make_deployment_retryable(state: &AppState, node_id: &str) -> ApiResult<()> {
    let Some(node) = get_node_by_id(&state.metadata_db, node_id).await? else {
        return Err(ApiError::NotFound);
    };
    if matches!(
        NodeStatus::parse(&node.status),
        Some(NodeStatus::Deploying | NodeStatus::AwaitingRegistration)
    ) {
        update_node_status(&state.metadata_db, node_id, NodeStatus::DeployFailed).await?;
    }
    Ok(())
}

async fn persist_recovery_state(
    state: &AppState,
    task: &NodeTaskRecord,
    progress_percent: u32,
    is_finished: bool,
    message: &str,
    error: Option<String>,
) {
    if let Err(persist_error) = update_deploy_progress(
        &state.metadata_db,
        &task.task_id,
        &DeploySession {
            progress_percent,
            logs: vec![message.to_string()],
            is_finished,
            error,
        },
    )
    .await
    {
        tracing::error!(task_id = %task.task_id, %persist_error, "failed to persist node task recovery");
    }
}

#[cfg(test)]
mod tests {
    use super::make_deployment_retryable;
    use crate::models::nodes::{
        NewNodeRecord, NodeStatus, get_node_by_id, insert_node, update_node_status,
    };
    use crate::state::AppState;
    use crate::test_support::setup_test_db;
    use std::sync::Arc;
    use uuid::Uuid;

    #[tokio::test]
    async fn interrupted_deploying_state_becomes_retryable() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        insert_node(
            &pool,
            &NewNodeRecord {
                node_id: node_id.clone(),
                tenant_id: None,
                name: format!("node-{node_id}"),
                normalized_name: format!("node-{node_id}"),
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
        update_node_status(&pool, &node_id, NodeStatus::Deploying)
            .await
            .unwrap();
        let state = AppState {
            server_name: "test".to_string(),
            metadata_db: pool.clone(),
            captcha_service: crate::security::captcha::CaptchaService::default(),
            login_tracker: crate::security::login_tracker::LoginTracker::default(),
            local_node_resource: Arc::new(tokio::sync::Mutex::new(None)),
            image_acquisition: crate::services::image_acquisition::ImageAcquisitionService::new(),
            terminal_tickets: Arc::new(
                crate::services::terminal_ticket::TerminalTicketStore::default(),
            ),
        };

        make_deployment_retryable(&state, &node_id).await.unwrap();
        let node = get_node_by_id(&pool, &node_id).await.unwrap().unwrap();
        assert_eq!(node.status, NodeStatus::DeployFailed.as_str());
    }
}
