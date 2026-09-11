//! 计划任务公共 API：稳定领域 DTO、可信身份、后台操作与服务端分页。

use crate::{
    api::auth::AuthenticatedAdmin,
    models::{
        logging::{LogModule, LogStatus, PlatformLogLevel},
        nodes,
        task_scheduler::{self, OperationActor, ScheduledTaskRow, TaskListFilter},
    },
    services::{
        logging::{self, OperationEventBuilder},
        task_scheduler as domain, task_sync,
    },
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashSet, net::IpAddr, sync::Arc};
use vust_contracts::{
    api::ErrorCode,
    scheduled_tasks::{
        CreateScheduledTaskBatchRequest, CreateScheduledTaskMigrationRequest,
        CreateScheduledTaskRequest, ScheduledTaskBatch, ScheduledTaskBatchAction,
        ScheduledTaskBatchItem, ScheduledTaskDeployment, ScheduledTaskDeploymentStatus,
        ScheduledTaskDetail, ScheduledTaskExecution, ScheduledTaskLastRun, ScheduledTaskListPage,
        ScheduledTaskNextRun, ScheduledTaskNextRunStatus, ScheduledTaskNode,
        ScheduledTaskOperation, ScheduledTaskRunPage, ScheduledTaskSchedule, ScheduledTaskSummary,
        UpdateScheduledTaskRequest, UpdateScheduledTaskStateRequest,
    },
};

/// 构建计划任务资源路由。
pub fn scheduled_tasks_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{task_id}", get(detail).patch(update).delete(remove))
        .route("/{task_id}/state", patch(update_state))
        .route("/{task_id}/runs", post(start_run).get(list_runs))
        .route("/{task_id}/runs/{run_id}", get(run_detail))
        .route("/{task_id}/runs/{run_id}/output", get(run_output))
        .route("/{task_id}/runs/{run_id}/cancel", post(cancel_run))
        .route("/{task_id}/migrations", post(migrate))
}

/// 构建计划任务后台操作路由。
pub fn scheduled_task_operations_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{operation_id}", get(operation_detail))
        .route("/{operation_id}/cancel", post(cancel_operation))
}

/// 构建批量操作路由。
pub fn scheduled_task_batches_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(create_batch))
        .route("/{batch_id}", get(batch_detail))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    node_id: Option<String>,
    keyword: Option<String>,
    enabled: Option<bool>,
    deployment_status: Option<String>,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_page_size")]
    page_size: u32,
    #[serde(default = "default_sort_by")]
    sort_by: String,
    #[serde(default = "default_sort_order")]
    sort_order: String,
}

async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Response> {
    validate_list_query(&query)?;
    let deployment_status = parse_deployment_status_filter(query.deployment_status.as_deref())?;
    let filter = TaskListFilter {
        node_id: query.node_id.as_deref().filter(|value| !value.is_empty()),
        keyword: query
            .keyword
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        enabled: query.enabled,
        deployment_status,
        page: query.page,
        page_size: query.page_size,
        sort_by: &query.sort_by,
        sort_order: &query.sort_order,
    };
    let (rows, total) = task_scheduler::list_tasks(&state.metadata_db, &filter).await?;
    let items = rows
        .into_iter()
        .map(summary_from_row)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(ApiResponse::success_with_raw(
        "Scheduled tasks loaded",
        Some(ScheduledTaskListPage {
            items,
            page: query.page,
            page_size: query.page_size,
            total,
            loaded_at: now_string(),
        }),
    )
    .into_response())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MutationResponse {
    task: ScheduledTaskSummary,
    operation: ScheduledTaskOperation,
}

async fn create(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(request): Json<CreateScheduledTaskRequest>,
) -> ApiResult<Response> {
    domain::validate_create_request(&request)?;
    ensure_node_exists(&state, &request.node_id).await?;
    let actor = actor_context(&admin, &headers)?;
    let result = task_scheduler::create_task(&state.metadata_db, &request, &actor).await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_create_submitted",
        "POST",
        None,
        false,
        &result,
    );
    let (task, operation) = result?;
    task_sync::trigger_worker();
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::success_with_raw(
            "Scheduled task created",
            MutationResponse {
                task: summary_from_row(task)?,
                operation: task_scheduler::operation_dto(operation)?,
            },
        )),
    )
        .into_response())
}

async fn detail(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let row = task_scheduler::get_task(&state.metadata_db, &task_id)
        .await?
        .filter(|row| row.ownership_kind == "custom" && row.deployment_status != "deleting")
        .ok_or_else(task_not_found)?;
    let execution = ScheduledTaskExecution {
        kind: "shell".to_string(),
        command: row.command.clone(),
        timeout_seconds: row.timeout_seconds.clamp(1, 86_400) as u32,
        prevent_overlap: row.prevent_overlap,
    };
    Ok(ApiResponse::success_with_raw(
        "Scheduled task loaded",
        Some(ScheduledTaskDetail {
            summary: summary_from_row(row)?,
            execution,
        }),
    )
    .into_response())
}

async fn update(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(request): Json<UpdateScheduledTaskRequest>,
) -> ApiResult<Response> {
    domain::validate_update_request(&request)?;
    let actor = actor_context(&admin, &headers)?;
    let result = task_scheduler::update_task(&state.metadata_db, &task_id, &request, &actor).await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_update_submitted",
        "PATCH",
        Some(&task_id),
        false,
        &result,
    );
    let (task, operation) = result?;
    task_sync::trigger_worker();
    accepted(
        "Scheduled task update accepted",
        MutationResponse {
            task: summary_from_row(task)?,
            operation: task_scheduler::operation_dto(operation)?,
        },
    )
}

async fn update_state(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(request): Json<UpdateScheduledTaskStateRequest>,
) -> ApiResult<Response> {
    let actor = actor_context(&admin, &headers)?;
    let result =
        task_scheduler::update_task_state(&state.metadata_db, &task_id, request.enabled, &actor)
            .await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_state_change_submitted",
        "PATCH",
        Some(&task_id),
        false,
        &result,
    );
    let (task, operation) = result?;
    task_sync::trigger_worker();
    accepted(
        "Scheduled task state change accepted",
        MutationResponse {
            task: summary_from_row(task)?,
            operation: task_scheduler::operation_dto(operation)?,
        },
    )
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let actor = actor_context(&admin, &headers)?;
    let result = task_scheduler::request_remove(&state.metadata_db, &task_id, &actor).await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_remove_submitted",
        "DELETE",
        Some(&task_id),
        true,
        &result,
    );
    let operation = task_scheduler::operation_dto(result?)?;
    task_sync::trigger_worker();
    accepted("Scheduled task removal accepted", operation)
}

async fn start_run(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let actor = actor_context(&admin, &headers)?;
    let result = task_scheduler::create_run(
        &state.metadata_db,
        &task_id,
        vust_contracts::scheduled_tasks::ScheduledTaskTriggerSource::Manual,
        Some(&actor),
    )
    .await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_run_submitted",
        "POST",
        Some(&task_id),
        false,
        &result,
    );
    let run = task_scheduler::run_dto(result?)?;
    task_sync::trigger_worker();
    accepted("Scheduled task run accepted", run)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageQuery {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_page_size")]
    page_size: u32,
}

async fn list_runs(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Response> {
    validate_page(query.page, query.page_size)?;
    let (rows, total) =
        task_scheduler::list_runs(&state.metadata_db, &task_id, query.page, query.page_size)
            .await?;
    let items = rows
        .into_iter()
        .map(task_scheduler::run_dto)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(ApiResponse::success_with_raw(
        "Scheduled task runs loaded",
        Some(ScheduledTaskRunPage {
            items,
            page: query.page,
            page_size: query.page_size,
            total,
            loaded_at: now_string(),
        }),
    )
    .into_response())
}

async fn run_detail(
    State(state): State<Arc<AppState>>,
    Path((task_id, run_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let run = task_scheduler::run_dto(
        task_scheduler::get_run(&state.metadata_db, &task_id, &run_id).await?,
    )?;
    Ok(ApiResponse::success_with_raw("Scheduled task run loaded", Some(run)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutputQuery {
    #[serde(default)]
    offset_bytes: u64,
    #[serde(default = "default_output_limit")]
    limit_bytes: u32,
}

async fn run_output(
    State(state): State<Arc<AppState>>,
    Path((task_id, run_id)): Path<(String, String)>,
    Query(query): Query<OutputQuery>,
) -> ApiResult<Response> {
    let output = task_scheduler::read_output(
        &state.metadata_db,
        &task_id,
        &run_id,
        query.offset_bytes,
        query.limit_bytes,
    )
    .await?;
    Ok(
        ApiResponse::success_with_raw("Scheduled task run output loaded", Some(output))
            .into_response(),
    )
}

async fn cancel_run(
    State(state): State<Arc<AppState>>,
    Path((task_id, run_id)): Path<(String, String)>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let actor = actor_context(&admin, &headers)?;
    let result = task_sync::cancel_run_on_agent(&state, &task_id, &run_id).await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_run_cancel_submitted",
        "POST",
        Some(&task_id),
        false,
        &result,
    );
    accepted("Scheduled task run cancellation accepted", result?)
}

async fn migrate(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(request): Json<CreateScheduledTaskMigrationRequest>,
) -> ApiResult<Response> {
    ensure_node_exists(&state, &request.target_node_id).await?;
    let actor = actor_context(&admin, &headers)?;
    let result = task_scheduler::request_migration(
        &state.metadata_db,
        &task_id,
        request.target_node_id.trim(),
        &actor,
    )
    .await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_migration_submitted",
        "POST",
        Some(&task_id),
        true,
        &result,
    );
    let operation = task_scheduler::operation_dto(result?)?;
    task_sync::trigger_worker();
    accepted("Scheduled task migration accepted", operation)
}

async fn operation_detail(
    State(state): State<Arc<AppState>>,
    Path(operation_id): Path<String>,
) -> ApiResult<Response> {
    let value = task_scheduler::operation_dto(
        task_scheduler::get_operation(&state.metadata_db, &operation_id).await?,
    )?;
    Ok(
        ApiResponse::success_with_raw("Scheduled task operation loaded", Some(value))
            .into_response(),
    )
}

async fn cancel_operation(
    State(state): State<Arc<AppState>>,
    Path(operation_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let actor = actor_context(&admin, &headers)?;
    let result = task_scheduler::request_operation_cancel(&state.metadata_db, &operation_id).await;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_operation_cancel_submitted",
        "POST",
        None,
        false,
        &result,
    );
    let value = task_scheduler::operation_dto(result?)?;
    task_sync::trigger_worker();
    accepted("Scheduled task operation cancellation accepted", value)
}

async fn create_batch(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(request): Json<CreateScheduledTaskBatchRequest>,
) -> ApiResult<Response> {
    let unique = request
        .task_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if unique.is_empty() || unique.len() != request.task_ids.len() || unique.len() > 100 {
        return Err(ApiError::bad_request(
            ErrorCode::BadRequest,
            "scheduled task batch must contain 1 to 100 unique task ids",
        ));
    }
    let actor = actor_context(&admin, &headers)?;
    let batch_id = uuid::Uuid::now_v7().to_string();
    let mut items = Vec::with_capacity(request.task_ids.len());
    for task_id in &request.task_ids {
        let result: ApiResult<(Option<String>, Option<String>)> = match request.action {
            ScheduledTaskBatchAction::Run => task_scheduler::create_run(
                &state.metadata_db,
                task_id,
                vust_contracts::scheduled_tasks::ScheduledTaskTriggerSource::Batch,
                Some(&actor),
            )
            .await
            .map(|run| (Some(run.run_id), None)),
            ScheduledTaskBatchAction::Enable => {
                task_scheduler::update_task_state(&state.metadata_db, task_id, true, &actor)
                    .await
                    .map(|(_, operation)| (None, Some(operation.operation_id)))
            }
            ScheduledTaskBatchAction::Disable => {
                task_scheduler::update_task_state(&state.metadata_db, task_id, false, &actor)
                    .await
                    .map(|(_, operation)| (None, Some(operation.operation_id)))
            }
            ScheduledTaskBatchAction::Remove => {
                task_scheduler::request_remove(&state.metadata_db, task_id, &actor)
                    .await
                    .map(|operation| (None, Some(operation.operation_id)))
            }
        };
        items.push(match result {
            Ok((run_id, operation_id)) => ScheduledTaskBatchItem {
                task_id: task_id.clone(),
                run_id,
                operation_id,
                error_code: None,
                error_summary: None,
            },
            Err(error) => ScheduledTaskBatchItem {
                task_id: task_id.clone(),
                run_id: None,
                operation_id: None,
                error_code: Some(error.code.as_str().to_string()),
                error_summary: Some(error.message.to_string()),
            },
        });
    }
    let batch = task_scheduler::save_batch(
        &state.metadata_db,
        &batch_id,
        request.action,
        &actor,
        &items,
    )
    .await?;
    record_submit(
        &state,
        &admin,
        &actor,
        "scheduled_task_batch_submitted",
        "POST",
        None,
        matches!(request.action, ScheduledTaskBatchAction::Remove),
        &Ok::<_, ApiError>(&batch),
    );
    task_sync::trigger_worker();
    accepted("Scheduled task batch accepted", batch)
}

async fn batch_detail(
    State(state): State<Arc<AppState>>,
    Path(batch_id): Path<String>,
) -> ApiResult<Response> {
    let value: ScheduledTaskBatch =
        task_scheduler::get_batch(&state.metadata_db, &batch_id).await?;
    Ok(ApiResponse::success_with_raw("Scheduled task batch loaded", Some(value)).into_response())
}

fn summary_from_row(row: ScheduledTaskRow) -> ApiResult<ScheduledTaskSummary> {
    let ownership = task_scheduler::ownership_from_text(&row.ownership_kind)?;
    let deployment_status = task_scheduler::deployment_status_from_text(&row.deployment_status)?;
    let last_run = match (row.last_run_id.clone(), row.last_run_status.as_deref()) {
        (Some(run_id), Some(status)) => Some(ScheduledTaskLastRun {
            run_id,
            status: parse_run_status(status)?,
            finished_at: row.last_run_finished_at.clone(),
        }),
        _ => None,
    };
    Ok(ScheduledTaskSummary {
        task_id: row.task_id,
        name: row.name,
        description: row.description,
        node: ScheduledTaskNode {
            node_id: row.node_id,
            node_name: row.node_name,
        },
        schedule: ScheduledTaskSchedule {
            cron_expr: row.cron_expr.clone(),
            time_zone: row.time_zone.clone(),
            summary: format!("{} ({})", row.cron_expr, row.time_zone),
        },
        desired_state: task_scheduler::desired_state_from_text(&row.desired_state)?,
        deployment: ScheduledTaskDeployment {
            status: deployment_status,
            revision: row.revision,
            last_synced_at: row.last_synced_at,
            error_summary: row.deployment_error_summary,
        },
        next_run: ScheduledTaskNextRun {
            status: parse_next_run_status(&row.next_run_status)?,
            at: row.next_run_at,
        },
        last_run,
        ownership: vust_contracts::scheduled_tasks::ScheduledTaskOwnership {
            kind: ownership,
            owner_id: row.owner_id,
            owner_name: row.owner_name,
            manager_path: row.manager_path,
        },
        capabilities: domain::capabilities(
            ownership,
            deployment_status,
            row.has_active_run,
            row.has_active_operation,
        ),
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn actor_context(admin: &AuthenticatedAdmin, headers: &HeaderMap) -> ApiResult<OperationActor> {
    let client_ip = admin.session.client_ip.clone().ok_or_else(|| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session is missing a trusted client IP",
        )
    })?;
    client_ip.parse::<IpAddr>().map_err(|_| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session has an invalid trusted client IP",
        )
    })?;
    Ok(OperationActor {
        user_id: admin.id,
        name: admin.username.clone(),
        client_ip,
        trace_id: logging::resolve_trace_id(headers),
    })
}

#[allow(clippy::too_many_arguments)]
fn record_submit<T>(
    state: &AppState,
    admin: &AuthenticatedAdmin,
    actor: &OperationActor,
    event: &str,
    method: &str,
    task_id: Option<&str>,
    high_impact: bool,
    result: &ApiResult<T>,
) {
    let Ok(ip) = actor.client_ip.parse::<IpAddr>() else {
        return;
    };
    let failed = result.is_err();
    OperationEventBuilder::new(&admin.username, event, ip).user_id(admin.id).module(LogModule::System)
        .target_type("scheduled_task").target_id(task_id.unwrap_or("batch")).trace_id(&actor.trace_id)
        .request(method, "/api/v1/scheduled-tasks")
        .status(if failed { LogStatus::Failed } else { LogStatus::Success })
        .level(if failed { PlatformLogLevel::Error } else if high_impact { PlatformLogLevel::Warning } else { PlatformLogLevel::Info })
        .metadata(json!({"result": if failed {"failed"} else {"submitted"}, "errorCode": result.as_ref().err().map(|error| error.code.as_str())}))
        .finish(&state.metadata_db);
}

async fn ensure_node_exists(state: &AppState, node_id: &str) -> ApiResult<()> {
    if node_id == "local" {
        return Ok(());
    }
    nodes::get_node_by_id(&state.metadata_db, node_id)
        .await
        .map_err(|error| ApiError::database(error.to_string()))?
        .filter(|node| node.retired_at.is_none())
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found(ErrorCode::NodeNotFound, "execution node not found"))
}

fn validate_list_query(query: &ListQuery) -> ApiResult<()> {
    validate_page(query.page, query.page_size)?;
    if !["name", "nextRunAt", "updatedAt"].contains(&query.sort_by.as_str())
        || !["asc", "desc"].contains(&query.sort_order.as_str())
    {
        return Err(ApiError::bad_request(
            ErrorCode::BadRequest,
            "invalid scheduled task sort query",
        ));
    }
    Ok(())
}

fn validate_page(page: u32, page_size: u32) -> ApiResult<()> {
    if page == 0 || page_size == 0 || page_size > 100 {
        Err(ApiError::bad_request(
            ErrorCode::BadRequest,
            "scheduled task pagination is invalid",
        ))
    } else {
        Ok(())
    }
}

fn parse_deployment_status_filter(
    value: Option<&str>,
) -> ApiResult<Option<ScheduledTaskDeploymentStatus>> {
    value
        .map(|value| {
            serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(|_| {
                ApiError::bad_request(
                    ErrorCode::BadRequest,
                    "invalid scheduled task deployment status filter",
                )
            })
        })
        .transpose()
}

fn parse_next_run_status(value: &str) -> ApiResult<ScheduledTaskNextRunStatus> {
    match value {
        "scheduled" => Ok(ScheduledTaskNextRunStatus::Scheduled),
        "disabled" => Ok(ScheduledTaskNextRunStatus::Disabled),
        "not_deployed" => Ok(ScheduledTaskNextRunStatus::NotDeployed),
        "unavailable" => Ok(ScheduledTaskNextRunStatus::Unavailable),
        _ => Err(ApiError::internal("invalid scheduled task next run status")),
    }
}
fn parse_run_status(
    value: &str,
) -> ApiResult<vust_contracts::scheduled_tasks::ScheduledTaskRunStatus> {
    use vust_contracts::scheduled_tasks::ScheduledTaskRunStatus as S;
    match value {
        "queued" => Ok(S::Queued),
        "starting" => Ok(S::Starting),
        "running" => Ok(S::Running),
        "cancelling" => Ok(S::Cancelling),
        "succeeded" => Ok(S::Succeeded),
        "failed" => Ok(S::Failed),
        "timed_out" => Ok(S::TimedOut),
        "cancelled" => Ok(S::Cancelled),
        _ => Err(ApiError::internal("invalid scheduled task run status")),
    }
}
fn task_not_found() -> ApiError {
    ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task not found")
}
fn accepted<T: Serialize>(message: &str, value: T) -> ApiResult<Response> {
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(message, value)),
    )
        .into_response())
}
fn now_string() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}
const fn default_page() -> u32 {
    1
}
const fn default_page_size() -> u32 {
    50
}
fn default_sort_by() -> String {
    "updatedAt".to_string()
}
fn default_sort_order() -> String {
    "desc".to_string()
}
const fn default_output_limit() -> u32 {
    64 * 1024
}

#[cfg(test)]
mod tests {
    use super::{ListQuery, parse_deployment_status_filter, validate_list_query};
    use serde_json::json;
    use vust_contracts::scheduled_tasks::ScheduledTaskDeploymentStatus;

    /// 列表查询必须使用公共契约的 camelCase 部署状态值。
    #[test]
    fn list_query_accepts_waiting_for_node_contract_value() {
        let query: ListQuery = serde_json::from_value(json!({
            "deploymentStatus": "waitingForNode",
            "page": 1,
            "pageSize": 50,
            "sortBy": "updatedAt",
            "sortOrder": "desc"
        }))
        .expect("camelCase deployment status should deserialize");

        validate_list_query(&query).expect("query should be valid");
        assert_eq!(
            parse_deployment_status_filter(query.deployment_status.as_deref()).unwrap(),
            Some(ScheduledTaskDeploymentStatus::WaitingForNode)
        );
    }

    /// 未知部署状态仍由计划任务 API 返回稳定的参数错误。
    #[test]
    fn deployment_status_filter_rejects_unknown_value() {
        let error = parse_deployment_status_filter(Some("unknown"))
            .expect_err("unknown deployment status should be rejected");

        assert_eq!(error.code, vust_contracts::api::ErrorCode::BadRequest);
    }
}
