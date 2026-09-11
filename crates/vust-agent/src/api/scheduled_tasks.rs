//! Agent 计划任务内部 API：幂等部署、后台运行、取消、状态与有界输出。

use crate::types::{ApiError, ApiResponse, ApiResult};
use crate::{models::scheduled_tasks, services::task_scheduler, state::AppState};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;
use std::sync::Arc;
use vust_contracts::{
    api::ErrorCode,
    scheduled_tasks::{
        AgentScheduledTaskDefinition, AgentStartScheduledTaskRunRequest, ScheduledTaskRunPage,
    },
};

/// 构建只供 Master 使用的计划任务内部路由。
pub fn scheduled_task_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/status", get(list_all_status))
        .route("/{task_id}", put(upsert).delete(remove))
        .route("/{task_id}/status", get(get_status))
        .route("/{task_id}/runs", post(start_run).get(list_runs))
        .route("/{task_id}/runs/{run_id}", get(get_run))
        .route("/{task_id}/runs/{run_id}/output", get(get_output))
        .route("/{task_id}/runs/{run_id}/cancel", post(cancel_run))
}

/// 幂等部署或更新任务定义。
async fn upsert(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(definition): Json<AgentScheduledTaskDefinition>,
) -> ApiResult<Response> {
    if definition.task_id != task_id {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskOperationConflict,
            "scheduled task path id does not match request body",
        ));
    }
    let task = scheduled_tasks::upsert_task(&state.metadata_db, &definition).await?;
    Ok(ApiResponse::success_with_raw(
        "Scheduled task deployed",
        Some(scheduled_tasks::AgentTaskStatus {
            task_id: task.task_id,
            next_run_at: task.next_run_at,
            last_run_at: task.last_run_at,
            last_status: None,
        }),
    )
    .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoveQuery {
    operation_id: String,
}

/// 删除自定义且没有活动运行的任务。
async fn remove(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<RemoveQuery>,
) -> ApiResult<Response> {
    let removed =
        scheduled_tasks::delete_task(&state.metadata_db, &task_id, &query.operation_id).await?;
    Ok(ApiResponse::success_with_raw("Scheduled task removed", Some(removed)).into_response())
}

/// 快速创建后台运行并返回 runId。
async fn start_run(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<AgentStartScheduledTaskRunRequest>,
) -> ApiResult<Response> {
    let run = task_scheduler::submit_run(
        Arc::clone(&state),
        &task_id,
        &request.run_id,
        request.trigger_source,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Scheduled task run accepted",
            run,
        )),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunsQuery {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_page_size")]
    page_size: u32,
}

/// 分页查询任务运行记录。
async fn list_runs(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<RunsQuery>,
) -> ApiResult<Response> {
    ensure_task(&state, &task_id).await?;
    let (records, total) =
        scheduled_tasks::list_runs(&state.metadata_db, &task_id, query.page, query.page_size)
            .await?;
    let items = records
        .into_iter()
        .map(scheduled_tasks::run_dto)
        .collect::<ApiResult<Vec<_>>>()?;
    let page = ScheduledTaskRunPage {
        items,
        page: query.page.max(1),
        page_size: query.page_size.clamp(1, 100),
        total,
        loaded_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
    };
    Ok(ApiResponse::success_with_raw("Scheduled task runs loaded", Some(page)).into_response())
}

/// 查询单次运行详情。
async fn get_run(
    State(state): State<Arc<AppState>>,
    Path((task_id, run_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let run = scheduled_tasks::get_run(&state.metadata_db, &run_id)
        .await?
        .filter(|run| run.task_id == task_id)
        .ok_or_else(run_not_found)?;
    Ok(ApiResponse::success_with_raw(
        "Scheduled task run loaded",
        Some(scheduled_tasks::run_dto(run)?),
    )
    .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutputQuery {
    #[serde(default)]
    offset_bytes: u64,
    #[serde(default = "default_output_limit")]
    limit_bytes: u32,
}

/// 分页读取有界运行输出。
async fn get_output(
    State(state): State<Arc<AppState>>,
    Path((task_id, run_id)): Path<(String, String)>,
    Query(query): Query<OutputQuery>,
) -> ApiResult<Response> {
    let run = scheduled_tasks::get_run(&state.metadata_db, &run_id)
        .await?
        .filter(|run| run.task_id == task_id)
        .ok_or_else(run_not_found)?;
    let output = scheduled_tasks::read_output(
        &state.metadata_db,
        &run.run_id,
        query.offset_bytes,
        query.limit_bytes,
    )
    .await?;
    Ok(
        ApiResponse::success_with_raw("Scheduled task run output loaded", Some(output))
            .into_response(),
    )
}

/// 请求取消活动运行。
async fn cancel_run(
    State(state): State<Arc<AppState>>,
    Path((task_id, run_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let run = scheduled_tasks::request_cancel(&state.metadata_db, &task_id, &run_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Scheduled task run cancellation requested",
            scheduled_tasks::run_dto(run)?,
        )),
    )
        .into_response())
}

/// 查询单个任务的 Agent 实际状态。
async fn get_status(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let status = scheduled_tasks::list_all_status(&state.metadata_db)
        .await?
        .into_iter()
        .find(|item| item.task_id == task_id)
        .ok_or_else(|| {
            ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task not found")
        })?;
    Ok(ApiResponse::success_with_raw("Scheduled task status loaded", Some(status)).into_response())
}

/// 批量查询所有任务状态，供 Master 异步更新读模型。
async fn list_all_status(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let statuses = scheduled_tasks::list_all_status(&state.metadata_db).await?;
    Ok(
        ApiResponse::success_with_raw("Scheduled task statuses loaded", Some(statuses))
            .into_response(),
    )
}

async fn ensure_task(state: &AppState, task_id: &str) -> ApiResult<()> {
    if scheduled_tasks::get_task(&state.metadata_db, task_id)
        .await?
        .is_some()
    {
        Ok(())
    } else {
        Err(ApiError::not_found(
            ErrorCode::ScheduledTaskNotFound,
            "scheduled task not found",
        ))
    }
}

fn run_not_found() -> ApiError {
    ApiError::not_found(
        ErrorCode::ScheduledTaskRunNotFound,
        "scheduled task run not found",
    )
}

const fn default_page() -> u32 {
    1
}
const fn default_page_size() -> u32 {
    50
}
const fn default_output_limit() -> u32 {
    64 * 1024
}
