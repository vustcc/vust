//! Master 计划任务后台协调器：部署、删除、迁移、运行下发与终态审计。

use crate::{
    models::{
        logging::{LogModule, LogStatus, PlatformLogLevel},
        node_runtime_client::NodeRuntimeClient,
        task_scheduler::{self, ScheduledTaskOperationRow, ScheduledTaskRunRow},
    },
    services::logging::OperationEventBuilder,
    state::AppState,
    types::{ApiError, ApiResult},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    net::IpAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Notify;
use vust_contracts::{
    api::{ApiResponse, ErrorCode},
    scheduled_tasks::{
        AgentScheduledTaskDefinition, AgentStartScheduledTaskRunRequest,
        ScheduledTaskDeploymentStatus, ScheduledTaskDesiredState, ScheduledTaskOperationStatus,
        ScheduledTaskRun,
    },
};

const AGENT_BASE: &str = "/api/v1/agent/scheduled-tasks";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskStatus {
    next_run_at: Option<String>,
}

fn worker_notify() -> &'static Notify {
    static NOTIFY: OnceLock<Notify> = OnceLock::new();
    NOTIFY.get_or_init(Notify::new)
}

/// 唤醒后台协调器立即处理新操作或运行。
pub fn trigger_worker() {
    worker_notify().notify_one();
}

/// 启动持久化操作与运行协调器。
pub fn spawn_sync_queue_worker(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = process_pending(&state).await {
                tracing::error!(%error, "scheduled task background worker failed");
            }
            tokio::select! {
                _ = worker_notify().notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
}

async fn process_pending(state: &AppState) -> ApiResult<()> {
    for operation in task_scheduler::list_pending_operations(&state.metadata_db, 50).await? {
        if operation.cancel_requested && operation.completed_steps == 0 {
            task_scheduler::restore_cancelled_operation(&state.metadata_db, &operation).await?;
            task_scheduler::finish_operation(
                &state.metadata_db,
                &operation.operation_id,
                ScheduledTaskOperationStatus::Cancelled,
                None,
                None,
                None,
            )
            .await?;
            record_terminal(
                state,
                &operation,
                ScheduledTaskOperationStatus::Cancelled,
                None,
            );
            continue;
        }
        let result = process_operation(state, &operation).await;
        if let Err(error) = result {
            if is_node_unavailable(&error) {
                let _ = task_scheduler::update_deployment(
                    &state.metadata_db,
                    &operation.task_id,
                    ScheduledTaskDeploymentStatus::WaitingForNode,
                    None,
                    Some("execution node is currently unavailable"),
                )
                .await;
                continue;
            }
            let _ = task_scheduler::update_deployment(
                &state.metadata_db,
                &operation.task_id,
                ScheduledTaskDeploymentStatus::Failed,
                None,
                Some(&error.message),
            )
            .await;
            task_scheduler::finish_operation(
                &state.metadata_db,
                &operation.operation_id,
                ScheduledTaskOperationStatus::Failed,
                Some(error.code.as_str()),
                Some(&error.message),
                None,
            )
            .await?;
            record_terminal(
                state,
                &operation,
                ScheduledTaskOperationStatus::Failed,
                Some(&error),
            );
        }
    }

    for run in task_scheduler::list_queued_runs(&state.metadata_db, 50).await? {
        if let Err(error) = dispatch_run(state, &run).await {
            let report = vust_contracts::scheduled_tasks::AgentScheduledTaskRunReport {
                run: ScheduledTaskRun {
                    run_id: run.run_id.clone(),
                    task_id: run.task_id.clone(),
                    node_id: run.node_id.clone(),
                    trigger_source: match run.trigger_source.as_str() {
                        "batch" => {
                            vust_contracts::scheduled_tasks::ScheduledTaskTriggerSource::Batch
                        }
                        _ => vust_contracts::scheduled_tasks::ScheduledTaskTriggerSource::Manual,
                    },
                    status: vust_contracts::scheduled_tasks::ScheduledTaskRunStatus::Failed,
                    phase: None,
                    queued_at: run.queued_at.clone(),
                    started_at: None,
                    finished_at: Some(chrono::Utc::now().to_rfc3339()),
                    exit_code: None,
                    error_code: Some(error.code.as_str().to_string()),
                    error_summary: Some(error.message.to_string()),
                    output: Default::default(),
                    capabilities: Default::default(),
                },
                output_content: None,
            };
            task_scheduler::save_run_report(&state.metadata_db, &run.node_id, &report).await?;
        }
    }
    Ok(())
}

async fn process_operation(
    state: &AppState,
    operation: &ScheduledTaskOperationRow,
) -> ApiResult<()> {
    match operation.kind.as_str() {
        "deploy" | "update" | "state_change" => deploy_operation(state, operation).await,
        "remove" => remove_operation(state, operation).await,
        "migrate" => migrate_operation(state, operation).await,
        _ => Err(ApiError::conflict(
            ErrorCode::ScheduledTaskOperationConflict,
            "unsupported scheduled task operation",
        )),
    }
}

async fn deploy_operation(
    state: &AppState,
    operation: &ScheduledTaskOperationRow,
) -> ApiResult<()> {
    let task = task_scheduler::get_task(&state.metadata_db, &operation.task_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task not found")
        })?;
    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "applying",
        0,
    )
    .await?;
    task_scheduler::update_deployment(
        &state.metadata_db,
        &task.task_id,
        ScheduledTaskDeploymentStatus::Applying,
        None,
        None,
    )
    .await?;
    let next_run_at = deploy_definition(
        state,
        &task.node_id,
        task_scheduler::definition_from_row(&task, operation.operation_id.clone())?,
    )
    .await?;
    task_scheduler::update_deployment(
        &state.metadata_db,
        &task.task_id,
        ScheduledTaskDeploymentStatus::Ready,
        next_run_at.as_deref(),
        None,
    )
    .await?;
    task_scheduler::finish_operation(
        &state.metadata_db,
        &operation.operation_id,
        ScheduledTaskOperationStatus::Succeeded,
        None,
        None,
        None,
    )
    .await?;
    record_terminal(
        state,
        operation,
        ScheduledTaskOperationStatus::Succeeded,
        None,
    );
    Ok(())
}

async fn remove_operation(
    state: &AppState,
    operation: &ScheduledTaskOperationRow,
) -> ApiResult<()> {
    let task = task_scheduler::get_task(&state.metadata_db, &operation.task_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task not found")
        })?;
    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "removingFromAgent",
        1,
    )
    .await?;
    remove_from_agent(state, &task.node_id, &task.task_id, &operation.operation_id).await?;
    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "finalizing",
        2,
    )
    .await?;
    task_scheduler::hard_delete_task(&state.metadata_db, &task.task_id).await?;
    task_scheduler::finish_operation(
        &state.metadata_db,
        &operation.operation_id,
        ScheduledTaskOperationStatus::Succeeded,
        None,
        None,
        None,
    )
    .await?;
    record_terminal(
        state,
        operation,
        ScheduledTaskOperationStatus::Succeeded,
        None,
    );
    Ok(())
}

async fn migrate_operation(
    state: &AppState,
    operation: &ScheduledTaskOperationRow,
) -> ApiResult<()> {
    let source_node = operation
        .source_node_id
        .as_deref()
        .ok_or_else(|| ApiError::internal("migration source node is missing"))?;
    let target_node = operation
        .target_node_id
        .as_deref()
        .ok_or_else(|| ApiError::internal("migration target node is missing"))?;
    let task = task_scheduler::get_task(&state.metadata_db, &operation.task_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task not found")
        })?;
    let desired_state = task_scheduler::desired_state_from_text(&task.desired_state)?;

    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "pausingSource",
        1,
    )
    .await?;
    let mut disabled = task_scheduler::definition_from_row(&task, operation.operation_id.clone())?;
    disabled.desired_state = ScheduledTaskDesiredState::Disabled;
    deploy_definition(state, source_node, disabled.clone()).await?;

    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "deployingTarget",
        2,
    )
    .await?;
    if let Err(error) = deploy_definition(state, target_node, disabled).await {
        let restore = task_scheduler::definition_from_row(
            &task,
            format!("{}:rollback", operation.operation_id),
        )?;
        let _ = deploy_definition(state, source_node, restore).await;
        return Err(error);
    }

    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "switchingAuthority",
        3,
    )
    .await?;
    task_scheduler::move_task_node(&state.metadata_db, &task.task_id, target_node).await?;
    let mut target_task = task.clone();
    target_task.node_id = target_node.to_string();
    let mut target_definition =
        task_scheduler::definition_from_row(&target_task, operation.operation_id.clone())?;
    target_definition.desired_state = desired_state;

    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "activatingTarget",
        4,
    )
    .await?;
    let next_run_at = deploy_definition(state, target_node, target_definition).await?;
    task_scheduler::update_deployment(
        &state.metadata_db,
        &task.task_id,
        ScheduledTaskDeploymentStatus::Ready,
        next_run_at.as_deref(),
        None,
    )
    .await?;

    task_scheduler::update_operation_progress(
        &state.metadata_db,
        &operation.operation_id,
        "cleaningSource",
        5,
    )
    .await?;
    match remove_from_agent(
        state,
        source_node,
        &task.task_id,
        &format!("{}:cleanup", operation.operation_id),
    )
    .await
    {
        Ok(()) => {
            task_scheduler::finish_operation(
                &state.metadata_db,
                &operation.operation_id,
                ScheduledTaskOperationStatus::Succeeded,
                None,
                None,
                None,
            )
            .await?;
            record_terminal(
                state,
                operation,
                ScheduledTaskOperationStatus::Succeeded,
                None,
            );
        }
        Err(error) => {
            task_scheduler::finish_operation(
                &state.metadata_db, &operation.operation_id, ScheduledTaskOperationStatus::Partial,
                Some(error.code.as_str()), None, Some("task is active on the target node, but the disabled source copy still requires cleanup"),
            ).await?;
            record_terminal(
                state,
                operation,
                ScheduledTaskOperationStatus::Partial,
                Some(&error),
            );
        }
    }
    Ok(())
}

async fn deploy_definition(
    state: &AppState,
    node_id: &str,
    definition: AgentScheduledTaskDefinition,
) -> ApiResult<Option<String>> {
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?;
    let path = format!("{AGENT_BASE}/{}", definition.task_id);
    let response: ApiResponse<AgentTaskStatus> = client
        .put_json(&path, &definition)
        .await
        .map_err(external_error)?;
    if !response.success {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            response
                .error_code
                .unwrap_or(ErrorCode::ScheduledTaskOperationConflict),
            response.message,
        ));
    }
    Ok(response.data.and_then(|value| value.next_run_at))
}

async fn remove_from_agent(
    state: &AppState,
    node_id: &str,
    task_id: &str,
    operation_id: &str,
) -> ApiResult<()> {
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?;
    let path = format!("{AGENT_BASE}/{task_id}?operationId={operation_id}");
    let response: ApiResponse<bool> = client.delete_json(&path).await.map_err(external_error)?;
    if response.success {
        Ok(())
    } else {
        Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            response
                .error_code
                .unwrap_or(ErrorCode::ScheduledTaskOperationConflict),
            response.message,
        ))
    }
}

async fn dispatch_run(state: &AppState, run: &ScheduledTaskRunRow) -> ApiResult<()> {
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&run.node_id)).await?;
    let request = AgentStartScheduledTaskRunRequest {
        operation_id: uuid::Uuid::now_v7().to_string(),
        run_id: run.run_id.clone(),
        trigger_source: match run.trigger_source.as_str() {
            "batch" => vust_contracts::scheduled_tasks::ScheduledTaskTriggerSource::Batch,
            _ => vust_contracts::scheduled_tasks::ScheduledTaskTriggerSource::Manual,
        },
    };
    let path = format!("{AGENT_BASE}/{}/runs", run.task_id);
    let response: ApiResponse<ScheduledTaskRun> = client
        .post_json(&path, &request)
        .await
        .map_err(external_error)?;
    if !response.success {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            response
                .error_code
                .unwrap_or(ErrorCode::ScheduledTaskOperationConflict),
            response.message,
        ));
    }
    if let Some(mut agent_run) = response.data {
        agent_run.node_id.clone_from(&run.node_id);
        task_scheduler::save_run_report(
            &state.metadata_db,
            &run.node_id,
            &vust_contracts::scheduled_tasks::AgentScheduledTaskRunReport {
                run: agent_run,
                output_content: None,
            },
        )
        .await?;
    }
    Ok(())
}

/// 向 Agent 转发取消请求并更新 Master 运行读模型。
pub async fn cancel_run_on_agent(
    state: &AppState,
    task_id: &str,
    run_id: &str,
) -> ApiResult<ScheduledTaskRun> {
    let run = task_scheduler::mark_run_cancelling(&state.metadata_db, task_id, run_id).await?;
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&run.node_id)).await?;
    let path = format!("{AGENT_BASE}/{task_id}/runs/{run_id}/cancel");
    let response: ApiResponse<ScheduledTaskRun> = client
        .post_json(&path, &json!({}))
        .await
        .map_err(external_error)?;
    if !response.success {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            response
                .error_code
                .unwrap_or(ErrorCode::ScheduledTaskRunNotCancellable),
            response.message,
        ));
    }
    let mut value = response
        .data
        .ok_or_else(|| ApiError::internal("Agent returned no scheduled task run"))?;
    value.node_id = run.node_id;
    task_scheduler::save_run_report(
        &state.metadata_db,
        &value.node_id.clone(),
        &vust_contracts::scheduled_tasks::AgentScheduledTaskRunReport {
            run: value.clone(),
            output_content: None,
        },
    )
    .await?;
    Ok(value)
}

fn is_node_unavailable(error: &ApiError) -> bool {
    matches!(
        error.code,
        ErrorCode::AgentUnavailable
            | ErrorCode::AgentNotFound
            | ErrorCode::AgentTimeout
            | ErrorCode::ExternalRequestFailed
    )
}

fn external_error(error: anyhow::Error) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        ErrorCode::ExternalRequestFailed,
        error.to_string(),
    )
}

/// 节点上线后唤醒持久化协调器；Agent 同时会主动拉取完整快照。
pub async fn reconcile_agent_tasks(_state: &AppState, _agent_id: &str) -> anyhow::Result<()> {
    trigger_worker();
    Ok(())
}

fn record_terminal(
    state: &AppState,
    operation: &ScheduledTaskOperationRow,
    status: ScheduledTaskOperationStatus,
    error: Option<&ApiError>,
) {
    let Ok(client_ip) = operation.client_ip.parse::<IpAddr>() else {
        tracing::error!(operation_id = %operation.operation_id, "trusted scheduled task operation IP is invalid");
        return;
    };
    let failed = status == ScheduledTaskOperationStatus::Failed;
    let high_impact = matches!(operation.kind.as_str(), "remove" | "migrate");
    OperationEventBuilder::new(&operation.actor_name, &format!("scheduled_task_{}_completed", operation.kind), client_ip)
        .user_id(operation.actor_user_id)
        .module(LogModule::System)
        .target_type("scheduled_task")
        .target_id(&operation.task_id)
        .trace_id(&operation.trace_id)
        .status(if failed { LogStatus::Failed } else { LogStatus::Success })
        .level(if failed { PlatformLogLevel::Error } else if high_impact { PlatformLogLevel::Warning } else { PlatformLogLevel::Info })
        .metadata(json!({
            "operationId": operation.operation_id,
            "result": if failed { "failed" } else if status == ScheduledTaskOperationStatus::Partial { "partial" } else if status == ScheduledTaskOperationStatus::Cancelled { "cancelled" } else { "success" },
            "errorCode": error.map(|value| value.code.as_str()),
        }))
        .finish(&state.metadata_db);
}
