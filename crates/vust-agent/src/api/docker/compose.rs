//! Docker Compose 项目查询、详情与配置 API。

use crate::api::docker::context::DockerOperationContext;
use crate::api::docker::images;
use crate::config;
use crate::models::docker;
use crate::services::docker_project_tasks::{self, ComposeCommandResult, NewDockerProjectTask};
use crate::services::docker_projects::{
    self, COMPOSE_FILE_NAME, DockerProjectRecord, MAX_COMPOSE_BYTES,
};
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};
use axum::extract::{Json, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{
    IntoResponse, Response,
    sse::{Event, KeepAlive, Sse},
};
use bollard::models::ContainerSummary;
use bollard::query_parameters;
use futures_util::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::{convert::Infallible, pin::Pin};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use uuid::Uuid;
use vust_contracts::api::ErrorCode;
use vust_contracts::runtime_docker::{RuntimeImageStage, RuntimeImageTask, RuntimeImageTaskStatus};

const COMPOSE_VALIDATION_TIMEOUT: Duration = Duration::from_secs(15);

/// 项目列表筛选参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerProjectListQuery {
    keyword: Option<String>,
    management_kind: Option<docker::DockerProjectManagementKind>,
    runtime_state: Option<docker::DockerProjectRuntimeState>,
    page: Option<usize>,
    page_size: Option<usize>,
}

/// 返回登记项目的分页摘要。
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DockerProjectListQuery>,
) -> ApiResult<Response> {
    let mut items = load_project_summaries(&state).await?;
    if let Some(keyword) = query
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let keyword = keyword.to_lowercase();
        items.retain(|project| {
            project.name.to_lowercase().contains(&keyword)
                || project
                    .management
                    .owner_name
                    .as_deref()
                    .is_some_and(|value| value.to_lowercase().contains(&keyword))
        });
    }
    if let Some(kind) = query.management_kind {
        items.retain(|project| project.management.kind == kind);
    }
    if let Some(runtime_state) = query.runtime_state {
        items.retain(|project| project.runtime_state == runtime_state);
    }
    let total = items.len();
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let items = items.into_iter().skip(offset).take(page_size).collect();
    let response = docker::DockerProjectPage {
        total,
        page,
        page_size,
        items,
    };
    Ok(ApiResponse::success_with_raw("Docker projects loaded", Some(response)).into_response())
}

/// 返回项目详情；容器事实只通过一次带标签过滤的 Docker list 获取。
pub async fn project_detail(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    let record = load_project_record(&state, &name).await?;
    let containers = list_project_containers(&state, &record.name).await?;
    let summary = build_summary(&record, &containers).await;
    let services = build_services(containers);
    let detail = docker::DockerProjectDetail { summary, services };
    Ok(ApiResponse::success_with_raw("Docker project loaded", Some(detail)).into_response())
}

/// 读取用户项目的 Compose 配置。
pub async fn project_configuration(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    let record = load_project_record(&state, &name).await?;
    docker_projects::ensure_mutable(&record)?;
    let compose_file = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
    let compose_yaml = tokio::fs::read_to_string(&compose_file)
        .await
        .map_err(|_| project_not_found(&record.name, "Compose configuration is missing"))?;
    let configuration = docker::DockerProjectConfiguration {
        compose_yaml,
        revision: record.config_revision,
        applied_revision: record.applied_revision,
        state: docker_projects::configuration_state(
            true,
            record.config_revision,
            record.applied_revision,
        ),
    };
    Ok(
        ApiResponse::success_with_raw("Docker project configuration loaded", Some(configuration))
            .into_response(),
    )
}

/// 校验并原子保存 Compose 配置，不触发部署。
pub async fn update_project_configuration(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(name): Path<String>,
    Json(payload): Json<docker::DockerProjectConfigurationUpdateRequest>,
) -> ApiResult<Response> {
    validate_compose_yaml(&payload.compose_yaml).await?;
    let record = load_project_record(&state, &name).await?;
    docker_projects::ensure_mutable(&record)?;
    let expected_revision = payload.expected_revision;
    let next_revision = expected_revision
        .checked_add(1)
        .ok_or_else(|| ApiError::validation("expectedRevision is out of range"))?;
    let result: ApiResult<Response> =
        async {
            if record.config_revision != expected_revision {
                return Err(ApiError::conflict(
                    ErrorCode::DockerProjectRevisionConflict,
                    "Docker project configuration has changed",
                ));
            }

            let compose_file = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
            let previous = tokio::fs::read(&compose_file).await.ok();
            let temporary = compose_file.with_extension("yaml.tmp");
            tokio::fs::write(&temporary, payload.compose_yaml.as_bytes()).await?;
            tokio::fs::rename(&temporary, &compose_file).await?;

            let update = sqlx::query(
                "UPDATE docker_compose_projects SET config_revision = config_revision + 1 \
             WHERE name = ?1 AND config_revision = ?2",
            )
            .bind(&record.name)
            .bind(expected_revision)
            .execute(&state.metadata_db)
            .await;
            let rows_affected = match update {
                Ok(update) => update.rows_affected(),
                Err(error) => {
                    restore_configuration(&compose_file, previous.as_deref()).await;
                    return Err(error.into());
                }
            };
            if rows_affected != 1 {
                restore_configuration(&compose_file, previous.as_deref()).await;
                return Err(ApiError::conflict(
                    ErrorCode::DockerProjectRevisionConflict,
                    "Docker project configuration has changed",
                ));
            }
            let configuration = docker::DockerProjectConfiguration {
                compose_yaml: payload.compose_yaml,
                revision: next_revision,
                applied_revision: record.applied_revision,
                state: docker::DockerProjectConfigurationState::Pending,
            };
            Ok(ApiResponse::success_with_raw(
                "Docker project configuration saved",
                Some(configuration),
            )
            .into_response())
        }
        .await;
    context
        .finish(
            &state.metadata_db,
            "docker_compose_project_configuration_update",
            Some(("composeProject", &record.name)),
            serde_json::json!({
                "projectName": record.name,
                "previousRevision": expected_revision,
                "revision": next_revision,
            }),
            false,
            result,
        )
        .await
}

/// 校验 Compose 配置并返回可直接展示的结果。
pub async fn validate_configuration(
    Json(payload): Json<docker::DockerProjectConfigurationValidateRequest>,
) -> ApiResult<Response> {
    let response = match validate_compose_yaml(&payload.compose_yaml).await {
        Ok(()) => docker::DockerProjectConfigurationValidateResponse {
            valid: true,
            error: None,
        },
        Err(error) if error.status == axum::http::StatusCode::BAD_REQUEST => {
            docker::DockerProjectConfigurationValidateResponse {
                valid: false,
                error: Some(error.message.into_owned()),
            }
        }
        Err(error) => return Err(error),
    };
    Ok(
        ApiResponse::success_with_raw("Compose validation finished", Some(response))
            .into_response(),
    )
}

/// 创建用户 Compose 项目并立即返回后台任务。
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Json(payload): Json<docker::DockerProjectCreateRequest>,
) -> ApiResult<Response> {
    let name = docker_projects::normalize_project_name(&payload.name)?;
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM docker_compose_projects WHERE name = ?1")
            .bind(&name)
            .fetch_one(&state.metadata_db)
            .await?;
    if exists > 0 {
        return Err(ApiError::conflict(
            ErrorCode::DockerProjectAlreadyExists,
            "Docker project already exists",
        ));
    }
    let task = docker_project_tasks::create(
        &state.metadata_db,
        &context,
        NewDockerProjectTask {
            project_name: &name,
            operation: docker::DockerProjectTaskOperation::Create,
            service_name: None,
            replicas: None,
            pull_images: false,
        },
    )
    .await?;
    let task_id = task.id.clone();
    tokio::spawn(run_create_task(
        Arc::clone(&state),
        task_id,
        name,
        payload.compose_yaml,
    ));
    Ok(accepted_task(task))
}

/// 启动项目已有容器。
pub async fn start_project(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    start_lifecycle_task(
        state,
        context,
        name,
        docker::DockerProjectTaskOperation::Start,
    )
    .await
}

/// 停止项目已有容器。
pub async fn stop_project(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    start_lifecycle_task(
        state,
        context,
        name,
        docker::DockerProjectTaskOperation::Stop,
    )
    .await
}

/// 重启项目已有容器。
pub async fn restart_project(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    start_lifecycle_task(
        state,
        context,
        name,
        docker::DockerProjectTaskOperation::Restart,
    )
    .await
}

/// 使用当前已保存配置重新部署项目。
pub async fn redeploy_project(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(name): Path<String>,
    Json(payload): Json<docker::DockerProjectDeploymentRequest>,
) -> ApiResult<Response> {
    let record = load_project_record(&state, &name).await?;
    docker_projects::ensure_mutable(&record)?;
    ensure_compose_file(&record).await?;
    let task = docker_project_tasks::create(
        &state.metadata_db,
        &context,
        NewDockerProjectTask {
            project_name: &record.name,
            operation: docker::DockerProjectTaskOperation::Redeploy,
            service_name: None,
            replicas: None,
            pull_images: payload.pull_images,
        },
    )
    .await?;
    tokio::spawn(run_redeploy_task(
        Arc::clone(&state),
        task.id.clone(),
        record,
        payload.pull_images,
    ));
    Ok(accepted_task(task))
}

/// 调整 Compose 服务副本数。
pub async fn scale_project_service(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path((name, service)): Path<(String, String)>,
    Json(payload): Json<docker::DockerProjectScaleRequest>,
) -> ApiResult<Response> {
    let record = load_project_record(&state, &name).await?;
    docker_projects::ensure_mutable(&record)?;
    ensure_compose_file(&record).await?;
    let service = normalize_service_name(&service)?;
    let previous_replicas = count_service_containers(&state, &record.name, &service).await?;
    let task = docker_project_tasks::create(
        &state.metadata_db,
        &context,
        NewDockerProjectTask {
            project_name: &record.name,
            operation: docker::DockerProjectTaskOperation::Scale,
            service_name: Some(&service),
            replicas: Some(payload.replicas),
            pull_images: false,
        },
    )
    .await?;
    tokio::spawn(run_scale_task(
        Arc::clone(&state),
        task.id.clone(),
        record,
        service,
        payload.replicas,
        previous_replicas,
    ));
    Ok(accepted_task(task))
}

/// 删除用户项目部署和受管配置，保留卷、镜像与绑定目录。
pub async fn remove_project(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    let record = load_project_record(&state, &name).await?;
    docker_projects::ensure_mutable(&record)?;
    preflight_remove(&state, &record).await?;
    let task = docker_project_tasks::create(
        &state.metadata_db,
        &context,
        NewDockerProjectTask {
            project_name: &record.name,
            operation: docker::DockerProjectTaskOperation::Remove,
            service_name: None,
            replicas: None,
            pull_images: false,
        },
    )
    .await?;
    tokio::spawn(run_remove_task(Arc::clone(&state), task.id.clone(), record));
    Ok(accepted_task(task))
}

/// 查询单个后台项目操作，供前端跟踪当前操作终态。
pub async fn project_operation(
    State(state): State<Arc<AppState>>,
    Path(operation_id): Path<String>,
) -> ApiResult<Response> {
    let operation = docker_project_tasks::get(&state.metadata_db, &operation_id).await?;
    Ok(
        ApiResponse::success_with_raw("Docker project operation loaded", Some(operation))
            .into_response(),
    )
}

/// 建立 Compose 项目操作的实时事件流，并以持久化快照作为首帧。
pub async fn project_operation_events(
    State(state): State<Arc<AppState>>,
    Path(operation_id): Path<String>,
) -> ApiResult<Response> {
    let subscription = docker_project_tasks::subscribe(&operation_id).await;
    let operation = docker_project_tasks::get(&state.metadata_db, &operation_id).await?;
    let terminal = !matches!(
        operation.status,
        docker::DockerProjectTaskStatus::Queued | docker::DockerProjectTaskStatus::Running
    );
    let initial_event = if terminal {
        sse_json_event("terminal", &operation)
    } else {
        sse_json_event("snapshot", &operation)
    };
    let initial = stream::once(async move { Ok(initial_event) });

    let events: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        if let Some((receiver, replay)) = subscription.filter(|_| !terminal) {
            let replay = stream::iter(
                replay
                    .into_iter()
                    .map(|update| Ok::<_, Infallible>(sse_json_event("progress", &update))),
            );
            let live = stream::unfold(receiver, |mut receiver| async move {
                match receiver.recv().await {
                    Ok(event) => Some((Ok(task_event_to_sse(event)), receiver)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Some((
                        Ok(Event::default().event("resync").data("lagged")),
                        receiver,
                    )),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
                }
            });
            Box::pin(initial.chain(replay).chain(live))
        } else {
            Box::pin(initial)
        };

    Ok(Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response())
}

fn task_event_to_sse(event: docker_project_tasks::DockerProjectTaskEvent) -> Event {
    match event {
        docker_project_tasks::DockerProjectTaskEvent::Snapshot(task) => {
            sse_json_event("snapshot", &task)
        }
        docker_project_tasks::DockerProjectTaskEvent::Progress(update) => {
            sse_json_event("progress", &update)
        }
        docker_project_tasks::DockerProjectTaskEvent::Terminal(task) => {
            sse_json_event("terminal", &task)
        }
    }
}

fn sse_json_event<T: Serialize>(name: &'static str, value: &T) -> Event {
    Event::default()
        .event(name)
        .data(serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()))
}

/// 返回最近提交且仍在执行的创建或重新部署操作。
pub async fn active_deployment(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let deployment = docker_project_tasks::latest_active_deployment(&state.metadata_db).await?;
    Ok(
        ApiResponse::success_with_raw("Active Docker project deployment loaded", deployment)
            .into_response(),
    )
}

async fn start_lifecycle_task(
    state: Arc<AppState>,
    context: DockerOperationContext,
    name: String,
    operation: docker::DockerProjectTaskOperation,
) -> ApiResult<Response> {
    let record = load_project_record(&state, &name).await?;
    docker_projects::ensure_mutable(&record)?;
    ensure_compose_file(&record).await?;
    let task = docker_project_tasks::create(
        &state.metadata_db,
        &context,
        NewDockerProjectTask {
            project_name: &record.name,
            operation,
            service_name: None,
            replicas: None,
            pull_images: false,
        },
    )
    .await?;
    tokio::spawn(run_lifecycle_task(
        Arc::clone(&state),
        task.id.clone(),
        record,
        operation,
    ));
    Ok(accepted_task(task))
}

async fn run_create_task(
    state: Arc<AppState>,
    task_id: String,
    name: String,
    compose_yaml: String,
) {
    let result = async {
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Validating,
            10,
        )
        .await?;
        validate_compose_yaml(&compose_yaml).await?;
        let cancellation = require_cancellation(&task_id).await?;
        let directory = project_directory(&name);
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Preparing,
            25,
        )
        .await?;
        if tokio::fs::metadata(&directory).await.is_ok() {
            return Err(ApiError::conflict(
                ErrorCode::DockerProjectAlreadyExists,
                "Docker project directory already exists",
            ));
        }
        tokio::fs::create_dir_all(&directory).await?;
        let compose_file = directory.join(COMPOSE_FILE_NAME);
        tokio::fs::write(&compose_file, compose_yaml.as_bytes()).await?;
        match crate::services::agent_runtime::load(&state.metadata_db)
            .await
            .map_err(|err| ApiError::Internal(err.to_string()))?
            .image_resolution
        {
            crate::services::agent_runtime::ImageResolutionStrategy::ControllerMediated => {
                prepare_remote_compose_images(
                    &state,
                    &task_id,
                    &name,
                    &compose_file,
                    &cancellation,
                )
                .await?;
            }
            crate::services::agent_runtime::ImageResolutionStrategy::LocalEngine => {
                prepare_local_compose_images(&state, &task_id, &name, &compose_file, &cancellation)
                    .await?;
            }
        }
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Applying,
            55,
        )
        .await?;
        let up_args = compose_up_args(false, true);
        match docker_project_tasks::run_tracked_compose_command(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectProgressPhase::Applying,
            &name,
            &compose_file,
            &up_args,
            &cancellation,
        )
        .await?
        {
            ComposeCommandResult::Cancelled => return Err(TaskRunError::Cancelled.into()),
            ComposeCommandResult::Succeeded => {}
        }
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Verifying,
            90,
        )
        .await?;
        tokio::fs::copy(
            &compose_file,
            directory.join(docker_projects::APPLIED_COMPOSE_FILE_NAME),
        )
        .await?;
        sqlx::query(
            "INSERT INTO docker_compose_projects (\
                name, compose_dir, management_kind, config_revision, applied_revision\
             ) VALUES (?1, ?2, 'custom', 1, 1)",
        )
        .bind(&name)
        .bind(directory.to_string_lossy().as_ref())
        .execute(&state.metadata_db)
        .await?;
        Ok::<(), ApiError>(())
    }
    .await;
    match result {
        Ok(()) => {
            let _ = docker_project_tasks::succeed(&state.metadata_db, &task_id).await;
        }
        Err(error) => {
            let cancelled = error.message.as_ref() == TaskRunError::Cancelled.message();
            let warning = cleanup_failed_create(&name).await;
            if cancelled {
                let _ = docker_project_tasks::cancelled(
                    &state.metadata_db,
                    &task_id,
                    warning.as_deref(),
                )
                .await;
            } else {
                let _ = docker_project_tasks::fail(
                    &state.metadata_db,
                    &task_id,
                    &error.to_string(),
                    warning.as_deref(),
                )
                .await;
            }
        }
    }
}

async fn run_lifecycle_task(
    state: Arc<AppState>,
    task_id: String,
    record: DockerProjectRecord,
    operation: docker::DockerProjectTaskOperation,
) {
    let result = async {
        let cancellation = require_cancellation(&task_id).await?;
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Applying,
            40,
        )
        .await?;
        let action = match operation {
            docker::DockerProjectTaskOperation::Start => "start",
            docker::DockerProjectTaskOperation::Stop => "stop",
            docker::DockerProjectTaskOperation::Restart => "restart",
            _ => return Err(ApiError::internal("invalid lifecycle operation")),
        };
        run_task_command(&record, &[action.to_string()], &cancellation).await?;
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Verifying,
            90,
        )
        .await?;
        Ok::<(), ApiError>(())
    }
    .await;
    finish_simple_task(&state, &task_id, result).await;
}

async fn run_redeploy_task(
    state: Arc<AppState>,
    task_id: String,
    record: DockerProjectRecord,
    pull_images: bool,
) {
    let result = async {
        let cancellation = require_cancellation(&task_id).await?;
        if pull_images {
            docker_project_tasks::update(
                &state.metadata_db,
                &task_id,
                docker::DockerProjectTaskStage::Pulling,
                25,
            )
            .await?;
            let compose_file = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
            let image_refs =
                resolve_compose_images(&record.name, &compose_file, &cancellation).await?;
            let pull_result = docker_project_tasks::run_tracked_compose_command(
                &state.metadata_db,
                &task_id,
                docker::DockerProjectProgressPhase::Pulling,
                &record.name,
                &compose_file,
                &["pull".into()],
                &cancellation,
            )
            .await;
            match pull_result {
                Ok(ComposeCommandResult::Succeeded) => {
                    record_tracked_registry_pull_activities(
                        &state,
                        &task_id,
                        &image_refs,
                        RegistryPullAuditOutcome::Success,
                    )
                    .await;
                }
                Ok(ComposeCommandResult::Cancelled) => {
                    record_tracked_registry_pull_activities(
                        &state,
                        &task_id,
                        &image_refs,
                        RegistryPullAuditOutcome::Cancelled,
                    )
                    .await;
                    return Err(TaskRunError::Cancelled.into());
                }
                Err(error) => {
                    let error_message = error.to_string();
                    record_tracked_registry_pull_activities(
                        &state,
                        &task_id,
                        &image_refs,
                        RegistryPullAuditOutcome::Failure(error_message),
                    )
                    .await;
                    return Err(error);
                }
            }
        } else {
            let compose_file = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
            match crate::services::agent_runtime::load(&state.metadata_db)
                .await
                .map_err(|err| ApiError::Internal(err.to_string()))?
                .image_resolution
            {
                crate::services::agent_runtime::ImageResolutionStrategy::ControllerMediated => {
                    prepare_remote_compose_images(
                        &state,
                        &task_id,
                        &record.name,
                        &compose_file,
                        &cancellation,
                    )
                    .await?;
                }
                crate::services::agent_runtime::ImageResolutionStrategy::LocalEngine => {
                    prepare_local_compose_images(
                        &state,
                        &task_id,
                        &record.name,
                        &compose_file,
                        &cancellation,
                    )
                    .await?;
                }
            }
        }
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Applying,
            55,
        )
        .await?;
        let up_args = compose_up_args(true, true);
        if matches!(
            docker_project_tasks::run_tracked_compose_command(
                &state.metadata_db,
                &task_id,
                docker::DockerProjectProgressPhase::Applying,
                &record.name,
                &PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME),
                &up_args,
                &cancellation,
            )
            .await?,
            ComposeCommandResult::Cancelled
        ) {
            return Err(TaskRunError::Cancelled.into());
        }
        let compose_file = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
        tokio::fs::copy(
            &compose_file,
            PathBuf::from(&record.compose_dir).join(docker_projects::APPLIED_COMPOSE_FILE_NAME),
        )
        .await?;
        sqlx::query(
            "UPDATE docker_compose_projects SET applied_revision = config_revision WHERE name = ?1",
        )
        .bind(&record.name)
        .execute(&state.metadata_db)
        .await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(error) = &result {
        let warning = rollback_deployment(&state, &task_id, &record).await;
        if is_cancelled(error) {
            let _ =
                docker_project_tasks::cancelled(&state.metadata_db, &task_id, warning.as_deref())
                    .await;
        } else {
            let _ = docker_project_tasks::fail(
                &state.metadata_db,
                &task_id,
                &error.to_string(),
                warning.as_deref(),
            )
            .await;
        }
    } else {
        let _ = docker_project_tasks::succeed(&state.metadata_db, &task_id).await;
    }
}

fn compose_up_args(force_recreate: bool, prohibit_pull: bool) -> Vec<String> {
    let mut args = vec!["up".into()];
    if prohibit_pull {
        args.extend(["--pull".into(), "never".into()]);
    }
    args.push("-d".into());
    if force_recreate {
        args.push("--force-recreate".into());
    }
    args.push("--remove-orphans".into());
    args
}

/// 解析 Compose 展开变量后的仓库镜像引用，排除由本地构建产生镜像的服务。
async fn resolve_compose_images(
    project: &str,
    compose_file: &FsPath,
    cancellation: &tokio_util::sync::CancellationToken,
) -> ApiResult<Vec<String>> {
    let mut command = Command::new("docker");
    command
        .args(["compose", "-f"])
        .arg(compose_file)
        .args(["-p", project, "config", "--format", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let future = command.output();
    tokio::pin!(future);
    let output = tokio::select! {
        _ = cancellation.cancelled() => return Err(TaskRunError::Cancelled.into()),
        result = tokio::time::timeout(COMPOSE_VALIDATION_TIMEOUT, &mut future) => {
            result.map_err(|_| ApiError::validation("Compose image resolution timed out"))??
        }
    };
    if !output.status.success() {
        return Err(ApiError::validation(sanitize_error(
            &String::from_utf8_lossy(&output.stderr),
        )));
    }
    parse_compose_config_images(&output.stdout)
}

#[derive(Debug, Deserialize)]
struct ResolvedComposeConfig {
    #[serde(default)]
    services: HashMap<String, ResolvedComposeService>,
}

#[derive(Debug, Deserialize)]
struct ResolvedComposeService {
    image: Option<String>,
    build: Option<Value>,
}

fn parse_compose_config_images(output: &[u8]) -> ApiResult<Vec<String>> {
    let config: ResolvedComposeConfig = serde_json::from_slice(output)
        .map_err(|error| ApiError::validation(format!("invalid Compose image config: {error}")))?;
    let mut images = config
        .services
        .into_values()
        .filter(|service| service.build.is_none())
        .filter_map(|service| service.image)
        .map(|image| image.trim().to_string())
        .filter(|image| !image.is_empty())
        .collect::<Vec<_>>();
    images.sort();
    images.dedup();
    Ok(images)
}

/// 为本地节点准备缺失镜像，并通过显式仓库拉取保留进度和审计。
async fn prepare_local_compose_images(
    state: &Arc<AppState>,
    task_id: &str,
    project: &str,
    compose_file: &FsPath,
    cancellation: &tokio_util::sync::CancellationToken,
) -> ApiResult<()> {
    let images = resolve_compose_images(project, compose_file, cancellation).await?;
    if images.is_empty() {
        return Ok(());
    }
    let docker = state.docker_client().await?;
    let mut missing = Vec::new();
    for image_ref in images {
        if docker.inspect_image(&image_ref).await.is_err() {
            missing.push(image_ref);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    docker_project_tasks::update(
        &state.metadata_db,
        task_id,
        docker::DockerProjectTaskStage::Pulling,
        25,
    )
    .await?;
    for image_ref in missing {
        if cancellation.is_cancelled() {
            return Err(TaskRunError::Cancelled.into());
        }
        let parent_id = image_progress_id(&image_ref);
        docker_project_tasks::record_progress_item(
            &state.metadata_db,
            task_id,
            docker::DockerProjectTaskProgressItem {
                id: parent_id.clone(),
                parent_id: None,
                phase: docker::DockerProjectProgressPhase::Pulling,
                status: docker::DockerProjectProgressStatus::Working,
                label: format!("Image {image_ref}"),
                action: "Pulling image from registry".to_string(),
                details: None,
                current_bytes: None,
                total_bytes: None,
                percent: None,
            },
        )
        .await?;
        pull_fallback_image(state, task_id, &image_ref, &parent_id, cancellation).await?;
    }
    Ok(())
}

/// 为远程 Agent 的 Compose 部署执行主控优先镜像准备。
async fn prepare_remote_compose_images(
    state: &Arc<AppState>,
    task_id: &str,
    project: &str,
    compose_file: &FsPath,
    cancellation: &tokio_util::sync::CancellationToken,
) -> ApiResult<()> {
    let images = resolve_compose_images(project, compose_file, cancellation).await?;
    if images.is_empty() {
        return Ok(());
    }
    docker_project_tasks::update(
        &state.metadata_db,
        task_id,
        docker::DockerProjectTaskStage::Pulling,
        25,
    )
    .await?;
    for image_ref in images {
        if cancellation.is_cancelled() {
            return Err(TaskRunError::Cancelled.into());
        }
        prepare_remote_image(state, task_id, &image_ref, cancellation).await?;
    }
    Ok(())
}

/// 准备单个镜像；主控运行时不可用时回退到子节点仓库拉取。
async fn prepare_remote_image(
    state: &Arc<AppState>,
    task_id: &str,
    image_ref: &str,
    cancellation: &tokio_util::sync::CancellationToken,
) -> ApiResult<()> {
    let parent_id = image_progress_id(image_ref);
    record_runtime_image_progress(
        state,
        task_id,
        image_ref,
        &parent_id,
        RuntimeImageTask {
            task_id: String::new(),
            image_ref: image_ref.to_string(),
            status: RuntimeImageTaskStatus::Pending,
            source: None,
            stage: RuntimeImageStage::Checking,
            progress_percent: 0,
            status_text: "Checking controller image".to_string(),
            layers: Vec::new(),
        },
    )
    .await;
    let controller_result = state
        .controller_runtime
        .acquire_image(image_ref, cancellation, |snapshot| {
            record_runtime_image_progress(state, task_id, image_ref, &parent_id, snapshot)
        })
        .await;
    match controller_result {
        Ok(_) => return Ok(()),
        Err(_) if cancellation.is_cancelled() => return Err(TaskRunError::Cancelled.into()),
        Err(error) => {
            docker_project_tasks::record_progress_item(
                &state.metadata_db,
                task_id,
                docker::DockerProjectTaskProgressItem {
                    id: parent_id.clone(),
                    parent_id: None,
                    phase: docker::DockerProjectProgressPhase::Pulling,
                    status: docker::DockerProjectProgressStatus::Warning,
                    label: format!("Image {image_ref}"),
                    action: "Falling back to registry".to_string(),
                    details: Some(sanitize_error(&error.to_string())),
                    current_bytes: None,
                    total_bytes: None,
                    percent: None,
                },
            )
            .await?;
        }
    }
    pull_fallback_image(state, task_id, image_ref, &parent_id, cancellation).await
}

/// 将 Master 镜像任务快照映射为 Compose 镜像及分层进度。
async fn record_runtime_image_progress(
    state: &Arc<AppState>,
    task_id: &str,
    image_ref: &str,
    parent_id: &str,
    snapshot: RuntimeImageTask,
) {
    let status = match snapshot.status {
        RuntimeImageTaskStatus::Success => docker::DockerProjectProgressStatus::Done,
        RuntimeImageTaskStatus::Failed | RuntimeImageTaskStatus::Cancelled => {
            docker::DockerProjectProgressStatus::Error
        }
        RuntimeImageTaskStatus::Pending | RuntimeImageTaskStatus::Running => {
            docker::DockerProjectProgressStatus::Working
        }
    };
    let mut items = vec![docker::DockerProjectTaskProgressItem {
        id: parent_id.to_string(),
        parent_id: None,
        phase: docker::DockerProjectProgressPhase::Pulling,
        status,
        label: format!("Image {image_ref}"),
        action: runtime_image_action(snapshot.stage).to_string(),
        details: (status == docker::DockerProjectProgressStatus::Error)
            .then(|| sanitize_error(&snapshot.status_text)),
        current_bytes: None,
        total_bytes: None,
        percent: Some(snapshot.progress_percent),
    }];
    items.extend(snapshot.layers.into_iter().map(|layer| {
        let normalized = layer.status_text.to_ascii_lowercase();
        let status = if normalized.contains("error") {
            docker::DockerProjectProgressStatus::Error
        } else if normalized.contains("complete") || normalized.contains("already exists") {
            docker::DockerProjectProgressStatus::Done
        } else {
            docker::DockerProjectProgressStatus::Working
        };
        docker::DockerProjectTaskProgressItem {
            id: format!("{parent_id}:{}", short_progress_hash(&layer.id)),
            parent_id: Some(parent_id.to_string()),
            phase: docker::DockerProjectProgressPhase::Pulling,
            status,
            label: layer.id,
            action: layer.status_text,
            details: None,
            current_bytes: None,
            total_bytes: None,
            percent: layer.percent,
        }
    }));
    let _ = docker_project_tasks::record_progress_items(&state.metadata_db, task_id, items).await;
}

fn runtime_image_action(stage: RuntimeImageStage) -> &'static str {
    match stage {
        RuntimeImageStage::Checking => "Checking target image",
        RuntimeImageStage::Exporting => "Exporting controller image",
        RuntimeImageStage::Uploading => "Transferring controller image",
        RuntimeImageStage::Loading => "Loading controller image",
        RuntimeImageStage::Pulling => "Pulling image from registry",
    }
}

/// Master 不可用时由子节点直接拉取，并保留 Docker 分层进度。
async fn pull_fallback_image(
    state: &Arc<AppState>,
    task_id: &str,
    image_ref: &str,
    parent_id: &str,
    cancellation: &tokio_util::sync::CancellationToken,
) -> ApiResult<()> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let pull = images::pull_registry_image(
        state,
        image_ref,
        || cancellation.is_cancelled(),
        |event| {
            let _ = sender.send(event);
        },
    );
    tokio::pin!(pull);
    let mut pending = HashMap::new();
    let mut persist_interval = tokio::time::interval(Duration::from_millis(250));
    persist_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    persist_interval.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut pull => break result,
            event = receiver.recv() => {
                if let Some(event) = event {
                    let key = event.id.clone().unwrap_or_else(|| image_ref.to_string());
                    pending.insert(key, event);
                }
            }
            _ = persist_interval.tick() => {
                persist_registry_layer_progress(
                    state, task_id, parent_id, image_ref, &mut pending,
                ).await?;
            }
        }
    };
    while let Ok(event) = receiver.try_recv() {
        let key = event.id.clone().unwrap_or_else(|| image_ref.to_string());
        pending.insert(key, event);
    }
    persist_registry_layer_progress(state, task_id, parent_id, image_ref, &mut pending).await?;
    let error_message = result.as_ref().err().map(ToString::to_string);
    let audit_outcome = match (&result, error_message.as_deref()) {
        (Ok(()), _) => RegistryPullAuditOutcome::Success,
        (Err(_), _) if cancellation.is_cancelled() => RegistryPullAuditOutcome::Cancelled,
        (Err(_), Some(error)) => RegistryPullAuditOutcome::Failure(error.to_string()),
        (Err(_), None) => {
            RegistryPullAuditOutcome::Failure("Registry image pull failed".to_string())
        }
    };
    record_registry_pull_activities(state, task_id, vec![(image_ref.to_string(), audit_outcome)])
        .await;
    match result {
        Ok(()) => {
            docker_project_tasks::record_progress_item(
                &state.metadata_db,
                task_id,
                docker::DockerProjectTaskProgressItem {
                    id: parent_id.to_string(),
                    parent_id: None,
                    phase: docker::DockerProjectProgressPhase::Pulling,
                    status: docker::DockerProjectProgressStatus::Done,
                    label: format!("Image {image_ref}"),
                    action: "Pull complete".to_string(),
                    details: None,
                    current_bytes: None,
                    total_bytes: None,
                    percent: Some(100),
                },
            )
            .await
        }
        Err(_) if cancellation.is_cancelled() => Err(TaskRunError::Cancelled.into()),
        Err(error) => {
            let message = sanitize_error(&error.to_string());
            docker_project_tasks::record_progress_item(
                &state.metadata_db,
                task_id,
                docker::DockerProjectTaskProgressItem {
                    id: parent_id.to_string(),
                    parent_id: None,
                    phase: docker::DockerProjectProgressPhase::Pulling,
                    status: docker::DockerProjectProgressStatus::Error,
                    label: format!("Image {image_ref}"),
                    action: "Pull failed".to_string(),
                    details: Some(message.clone()),
                    current_bytes: None,
                    total_bytes: None,
                    percent: None,
                },
            )
            .await?;
            Err(ApiError::validation(message))
        }
    }
}

#[derive(Clone)]
enum RegistryPullAuditOutcome {
    Success,
    Cancelled,
    Failure(String),
}

/// 根据 Compose 结构化进度计算每个镜像的真实拉取结果。
async fn record_tracked_registry_pull_activities(
    state: &Arc<AppState>,
    task_id: &str,
    image_refs: &[String],
    fallback: RegistryPullAuditOutcome,
) {
    let items = docker_project_tasks::progress_items(&state.metadata_db, task_id)
        .await
        .unwrap_or_default();
    let outcomes = image_refs
        .iter()
        .map(|image_ref| {
            let outcome = items
                .iter()
                .find(|item| {
                    item.phase == docker::DockerProjectProgressPhase::Pulling
                        && item.parent_id.is_none()
                        && item.label.trim() == format!("Image {image_ref}")
                })
                .map(|item| match item.status {
                    docker::DockerProjectProgressStatus::Done => RegistryPullAuditOutcome::Success,
                    docker::DockerProjectProgressStatus::Error => {
                        RegistryPullAuditOutcome::Failure(
                            item.details
                                .clone()
                                .unwrap_or_else(|| "Registry image pull failed".to_string()),
                        )
                    }
                    docker::DockerProjectProgressStatus::Warning
                    | docker::DockerProjectProgressStatus::Working => fallback.clone(),
                })
                .unwrap_or_else(|| fallback.clone());
            (image_ref.clone(), outcome)
        })
        .collect();
    record_registry_pull_activities(state, task_id, outcomes).await;
}

/// 为 Compose 内部实际执行的仓库拉取记录逐镜像终态。
async fn record_registry_pull_activities(
    state: &Arc<AppState>,
    task_id: &str,
    outcomes: Vec<(String, RegistryPullAuditOutcome)>,
) {
    let context = match docker_project_tasks::operation_context(&state.metadata_db, task_id).await {
        Ok(context) => context,
        Err(error) => {
            tracing::error!(%error, task_id, "failed to load registry pull audit context");
            return;
        }
    };
    for (image_ref, outcome) in outcomes {
        let params = serde_json::json!({
            "taskId": task_id,
            "imageRef": &image_ref,
        });
        match outcome {
            RegistryPullAuditOutcome::Success => {
                context
                    .record_success(
                        &state.metadata_db,
                        "docker_image_pulled_from_registry",
                        Some(("image", &image_ref)),
                        params,
                        false,
                    )
                    .await;
            }
            RegistryPullAuditOutcome::Cancelled => {
                context
                    .record_canceled(
                        &state.metadata_db,
                        "docker_image_pulled_from_registry",
                        Some(("image", &image_ref)),
                        params,
                    )
                    .await;
            }
            RegistryPullAuditOutcome::Failure(error) => {
                context
                    .record_failure(
                        &state.metadata_db,
                        "docker_image_pulled_from_registry",
                        Some(("image", &image_ref)),
                        params,
                        &error,
                    )
                    .await;
            }
        }
    }
}

/// 批量写入每个分层在节流窗口内的最后一条事件。
async fn persist_registry_layer_progress(
    state: &Arc<AppState>,
    task_id: &str,
    parent_id: &str,
    image_ref: &str,
    pending: &mut HashMap<String, bollard::models::CreateImageInfo>,
) -> ApiResult<()> {
    for (_, event) in pending.drain() {
        record_registry_layer_progress(state, task_id, parent_id, image_ref, event).await?;
    }
    Ok(())
}

/// 把 Docker Registry 事件映射为可恢复的 Compose 分层项。
async fn record_registry_layer_progress(
    state: &Arc<AppState>,
    task_id: &str,
    parent_id: &str,
    image_ref: &str,
    event: bollard::models::CreateImageInfo,
) -> ApiResult<()> {
    let action = event.status.unwrap_or_else(|| "Pulling".to_string());
    let layer_id = event.id.unwrap_or_else(|| image_ref.to_string());
    let (current_bytes, total_bytes, percent) = event
        .progress_detail
        .and_then(|progress| progress.current.zip(progress.total))
        .filter(|(_, total)| *total > 0)
        .map(|(current, total)| {
            (
                Some(current),
                Some(total),
                Some(
                    ((current as f64 / total as f64) * 100.0)
                        .round()
                        .clamp(0.0, 100.0) as u8,
                ),
            )
        })
        .unwrap_or((None, None, None));
    let normalized = action.to_ascii_lowercase();
    let status = if normalized.contains("error") {
        docker::DockerProjectProgressStatus::Error
    } else if normalized.contains("complete") || normalized.contains("already exists") {
        docker::DockerProjectProgressStatus::Done
    } else {
        docker::DockerProjectProgressStatus::Working
    };
    docker_project_tasks::record_progress_item(
        &state.metadata_db,
        task_id,
        docker::DockerProjectTaskProgressItem {
            id: format!("{parent_id}:{}", short_progress_hash(&layer_id)),
            parent_id: Some(parent_id.to_string()),
            phase: docker::DockerProjectProgressPhase::Pulling,
            status,
            label: layer_id,
            action,
            details: None,
            current_bytes,
            total_bytes,
            percent,
        },
    )
    .await
}

fn image_progress_id(image_ref: &str) -> String {
    format!("pulling:image:{}", short_progress_hash(image_ref))
}

fn short_progress_hash(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

async fn run_scale_task(
    state: Arc<AppState>,
    task_id: String,
    record: DockerProjectRecord,
    service: String,
    replicas: usize,
    previous_replicas: usize,
) {
    let result = async {
        let cancellation = require_cancellation(&task_id).await?;
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::Applying,
            50,
        )
        .await?;
        run_task_command(
            &record,
            &[
                "up".into(),
                "-d".into(),
                "--scale".into(),
                format!("{service}={replicas}"),
            ],
            &cancellation,
        )
        .await
    }
    .await;
    if let Err(error) = &result {
        let warning = rollback_scale(&state, &task_id, &record, &service, previous_replicas).await;
        if is_cancelled(error) {
            let _ =
                docker_project_tasks::cancelled(&state.metadata_db, &task_id, warning.as_deref())
                    .await;
        } else {
            let _ = docker_project_tasks::fail(
                &state.metadata_db,
                &task_id,
                &error.to_string(),
                warning.as_deref(),
            )
            .await;
        }
    } else {
        let _ = docker_project_tasks::succeed(&state.metadata_db, &task_id).await;
    }
}

async fn run_remove_task(state: Arc<AppState>, task_id: String, record: DockerProjectRecord) {
    let result = async {
        let cancellation = require_cancellation(&task_id).await?;
        docker_project_tasks::update(
            &state.metadata_db,
            &task_id,
            docker::DockerProjectTaskStage::CleaningUp,
            25,
        )
        .await?;
        let directory = PathBuf::from(&record.compose_dir);
        let compose_file = directory.join(COMPOSE_FILE_NAME);
        if tokio::fs::metadata(&compose_file).await.is_ok() {
            run_task_command(
                &record,
                &["down".into(), "--remove-orphans".into()],
                &cancellation,
            )
            .await?;
        } else {
            remove_stopped_project_containers(&state, &record.name).await?;
        }
        let trash = directory.with_extension(format!("removing-{}", Uuid::new_v4()));
        if tokio::fs::metadata(&directory).await.is_ok() {
            tokio::fs::rename(&directory, &trash).await?;
        }
        let delete = sqlx::query("DELETE FROM docker_compose_projects WHERE name = ?1")
            .bind(&record.name)
            .execute(&state.metadata_db)
            .await;
        if let Err(error) = delete {
            if tokio::fs::metadata(&trash).await.is_ok() {
                let _ = tokio::fs::rename(&trash, &directory).await;
            }
            return Err(error.into());
        }
        let cleanup_warning = if tokio::fs::metadata(&trash).await.is_ok() {
            tokio::fs::remove_dir_all(&trash)
                .await
                .err()
                .map(|error| format!("managed project directory cleanup failed: {error}"))
        } else {
            None
        };
        Ok::<Option<String>, ApiError>(cleanup_warning)
    }
    .await;
    match result {
        Ok(Some(warning)) => {
            let _ =
                docker_project_tasks::succeed_with_warning(&state.metadata_db, &task_id, &warning)
                    .await;
        }
        Ok(None) => {
            let _ = docker_project_tasks::succeed(&state.metadata_db, &task_id).await;
        }
        Err(error) if is_cancelled(&error) => {
            let _ = docker_project_tasks::cancelled(&state.metadata_db, &task_id, None).await;
        }
        Err(error) => {
            let _ =
                docker_project_tasks::fail(&state.metadata_db, &task_id, &error.to_string(), None)
                    .await;
        }
    }
}

async fn run_task_command(
    record: &DockerProjectRecord,
    args: &[String],
    cancellation: &tokio_util::sync::CancellationToken,
) -> ApiResult<()> {
    let compose_file = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
    match docker_project_tasks::run_compose_command(&record.name, &compose_file, args, cancellation)
        .await?
    {
        ComposeCommandResult::Succeeded => Ok(()),
        ComposeCommandResult::Cancelled => Err(TaskRunError::Cancelled.into()),
    }
}

async fn finish_simple_task(state: &Arc<AppState>, task_id: &str, result: ApiResult<()>) {
    match result {
        Ok(()) => {
            let _ = docker_project_tasks::succeed(&state.metadata_db, task_id).await;
        }
        Err(error) if is_cancelled(&error) => {
            let _ = docker_project_tasks::cancelled(&state.metadata_db, task_id, None).await;
        }
        Err(error) => {
            let _ =
                docker_project_tasks::fail(&state.metadata_db, task_id, &error.to_string(), None)
                    .await;
        }
    }
}

async fn rollback_deployment(
    state: &Arc<AppState>,
    task_id: &str,
    record: &DockerProjectRecord,
) -> Option<String> {
    let backup =
        PathBuf::from(&record.compose_dir).join(docker_projects::APPLIED_COMPOSE_FILE_NAME);
    if tokio::fs::metadata(&backup).await.is_err() {
        return None;
    }
    let _ = docker_project_tasks::update(
        &state.metadata_db,
        task_id,
        docker::DockerProjectTaskStage::RollingBack,
        85,
    )
    .await;
    let cancellation = tokio_util::sync::CancellationToken::new();
    docker_project_tasks::run_compose_command(
        &record.name,
        &backup,
        &["up".into(), "-d".into(), "--remove-orphans".into()],
        &cancellation,
    )
    .await
    .err()
    .map(|error| format!("deployment rollback failed: {error}"))
}

async fn rollback_scale(
    state: &Arc<AppState>,
    task_id: &str,
    record: &DockerProjectRecord,
    service: &str,
    replicas: usize,
) -> Option<String> {
    let _ = docker_project_tasks::update(
        &state.metadata_db,
        task_id,
        docker::DockerProjectTaskStage::RollingBack,
        85,
    )
    .await;
    let cancellation = tokio_util::sync::CancellationToken::new();
    run_task_command(
        record,
        &[
            "up".into(),
            "-d".into(),
            "--scale".into(),
            format!("{service}={replicas}"),
        ],
        &cancellation,
    )
    .await
    .err()
    .map(|error| format!("scale rollback failed: {error}"))
}

async fn cleanup_failed_create(name: &str) -> Option<String> {
    let directory = project_directory(name);
    let compose_file = directory.join(COMPOSE_FILE_NAME);
    let cancellation = tokio_util::sync::CancellationToken::new();
    let command_warning = if tokio::fs::metadata(&compose_file).await.is_ok() {
        docker_project_tasks::run_compose_command(
            name,
            &compose_file,
            &["down".into(), "--remove-orphans".into()],
            &cancellation,
        )
        .await
        .err()
        .map(|error| error.to_string())
    } else {
        None
    };
    let directory_warning = tokio::fs::remove_dir_all(&directory)
        .await
        .err()
        .filter(|_| directory.exists())
        .map(|error| error.to_string());
    command_warning.or(directory_warning)
}

async fn preflight_remove(state: &Arc<AppState>, record: &DockerProjectRecord) -> ApiResult<()> {
    let compose_file = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
    if tokio::fs::metadata(&compose_file).await.is_ok() {
        return Ok(());
    }
    let containers = list_project_containers(state, &record.name).await?;
    let active = containers
        .iter()
        .filter(|container| {
            !matches!(
                container.state,
                Some(bollard::models::ContainerSummaryStateEnum::EXITED)
                    | Some(bollard::models::ContainerSummaryStateEnum::CREATED)
                    | Some(bollard::models::ContainerSummaryStateEnum::DEAD)
            )
        })
        .collect::<Vec<_>>();
    if active.is_empty() {
        return Ok(());
    }
    let names = active
        .iter()
        .take(5)
        .filter_map(|container| container.names.as_ref().and_then(|names| names.first()))
        .map(|name| name.trim_start_matches('/'))
        .collect::<Vec<_>>()
        .join(", ");
    Err(ApiError::conflict(
        ErrorCode::DockerProjectInUse,
        format!("project has {} active container(s): {names}", active.len()),
    ))
}

async fn remove_stopped_project_containers(state: &Arc<AppState>, project: &str) -> ApiResult<()> {
    let docker = state.docker_client().await?;
    for container in list_project_containers(state, project).await? {
        if let Some(id) = container.id {
            docker
                .remove_container(
                    &id,
                    Some(
                        query_parameters::RemoveContainerOptionsBuilder::new()
                            .force(false)
                            .build(),
                    ),
                )
                .await?;
        }
    }
    Ok(())
}

async fn count_service_containers(
    state: &Arc<AppState>,
    project: &str,
    service: &str,
) -> ApiResult<usize> {
    Ok(list_project_containers(state, project)
        .await?
        .into_iter()
        .filter(|container| {
            container
                .labels
                .as_ref()
                .and_then(|labels| labels.get("com.docker.compose.service"))
                .is_some_and(|value| value == service)
        })
        .count())
}

async fn ensure_compose_file(record: &DockerProjectRecord) -> ApiResult<PathBuf> {
    let path = PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME);
    if tokio::fs::metadata(&path).await.is_err() {
        return Err(project_not_found(
            &record.name,
            "Compose configuration is missing",
        ));
    }
    Ok(path)
}

fn normalize_service_name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(ApiError::validation("invalid Compose service name"));
    }
    Ok(value.to_string())
}

async fn require_cancellation(task_id: &str) -> ApiResult<tokio_util::sync::CancellationToken> {
    docker_project_tasks::cancellation(task_id)
        .await
        .ok_or_else(|| {
            ApiError::not_found(ErrorCode::TaskNotFound, "task cancellation state missing")
        })
}

fn accepted_task(task: docker::DockerProjectTask) -> Response {
    let mut response =
        ApiResponse::success_with_raw("Docker project task accepted", Some(task)).into_response();
    *response.status_mut() = StatusCode::ACCEPTED;
    response
}

enum TaskRunError {
    Cancelled,
}

impl TaskRunError {
    const fn message(&self) -> &'static str {
        match self {
            Self::Cancelled => "Docker project task cancelled",
        }
    }
}

impl From<TaskRunError> for ApiError {
    fn from(value: TaskRunError) -> Self {
        ApiError::bad_request(ErrorCode::TaskExecutionFailed, value.message())
    }
}

fn is_cancelled(error: &ApiError) -> bool {
    error.message.as_ref() == TaskRunError::Cancelled.message()
}

/// 汇总所有登记项目，供项目列表与 Docker 概览共用。
pub(crate) async fn load_project_summaries(
    state: &Arc<AppState>,
) -> ApiResult<Vec<docker::DockerProjectSummary>> {
    let records = load_project_records(state).await?;
    let docker = state.docker_client().await?;
    let containers = docker
        .list_containers(Some(
            query_parameters::ListContainersOptionsBuilder::new()
                .all(true)
                .build(),
        ))
        .await?;
    let registered = records
        .iter()
        .map(|record| record.name.as_str())
        .collect::<HashSet<_>>();
    let mut grouped: HashMap<String, Vec<ContainerSummary>> = HashMap::new();
    for container in containers {
        let project = container
            .labels
            .as_ref()
            .and_then(|labels| labels.get("com.docker.compose.project"));
        if let Some(project) = project.filter(|project| registered.contains(project.as_str())) {
            grouped.entry(project.clone()).or_default().push(container);
        }
    }
    let mut summaries = Vec::with_capacity(records.len());
    for record in records {
        let containers = grouped.remove(&record.name).unwrap_or_default();
        summaries.push(build_summary(&record, &containers).await);
    }
    summaries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(summaries)
}

async fn build_summary(
    record: &DockerProjectRecord,
    containers: &[ContainerSummary],
) -> docker::DockerProjectSummary {
    let mut states = docker::DockerProjectContainerStates::default();
    let mut services = HashSet::new();
    for container in containers {
        docker_projects::add_container_state(&mut states, container.state);
        if let Some(service) = container
            .labels
            .as_ref()
            .and_then(|labels| labels.get("com.docker.compose.service"))
        {
            services.insert(service.clone());
        }
    }
    let file_exists =
        tokio::fs::metadata(PathBuf::from(&record.compose_dir).join(COMPOSE_FILE_NAME))
            .await
            .is_ok();
    let configuration_state = docker_projects::configuration_state(
        file_exists,
        record.config_revision,
        record.applied_revision,
    );
    let runtime_state = docker_projects::runtime_state(&states);
    docker::DockerProjectSummary {
        name: record.name.clone(),
        created_at: record.created_at,
        runtime_state,
        configuration_state,
        service_count: services.len(),
        management: docker_projects::management_for(
            record.management_kind,
            record.owner_name.clone(),
        ),
        capabilities: docker_projects::capabilities_for(
            record.management_kind,
            runtime_state,
            configuration_state,
            states.total > 0,
        ),
        container_states: states,
    }
}

fn build_services(containers: Vec<ContainerSummary>) -> Vec<docker::DockerProjectServiceSummary> {
    let mut grouped: HashMap<String, Vec<ContainerSummary>> = HashMap::new();
    for container in containers {
        let service = container
            .labels
            .as_ref()
            .and_then(|labels| labels.get("com.docker.compose.service"))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        grouped.entry(service).or_default().push(container);
    }
    let mut services = grouped
        .into_iter()
        .map(|(name, containers)| {
            let mut states = docker::DockerProjectContainerStates::default();
            let mut image = None;
            let containers = containers
                .into_iter()
                .filter_map(|container| {
                    docker_projects::add_container_state(&mut states, container.state);
                    if image.is_none() {
                        image.clone_from(&container.image);
                    }
                    normalize_container(container)
                })
                .collect::<Vec<_>>();
            docker::DockerProjectServiceSummary {
                name,
                image,
                runtime_state: docker_projects::runtime_state(&states),
                container_states: states,
                containers,
            }
        })
        .collect::<Vec<_>>();
    services.sort_by_key(|service| service.name.to_lowercase());
    services
}

fn normalize_container(
    container: ContainerSummary,
) -> Option<docker::DockerProjectContainerSummary> {
    let id = container.id?;
    let name = container
        .names
        .as_ref()
        .and_then(|names| names.first())
        .map(|name| name.trim_start_matches('/').to_string())
        .unwrap_or_else(|| id.chars().take(12).collect());
    let mut ip_addresses = container
        .network_settings
        .as_ref()
        .and_then(|settings| settings.networks.as_ref())
        .into_iter()
        .flat_map(|networks| networks.values())
        .flat_map(|endpoint| {
            [
                endpoint.ip_address.clone(),
                endpoint.global_ipv6_address.clone(),
            ]
        })
        .flatten()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    ip_addresses.sort();
    ip_addresses.dedup();
    let mut published_ports = container
        .ports
        .unwrap_or_default()
        .into_iter()
        .filter_map(|port| {
            port.public_port.map(|public| {
                format!(
                    "{}:{public}->{}{suffix}",
                    port.ip.unwrap_or_else(|| "0.0.0.0".to_string()),
                    port.private_port,
                    suffix = port
                        .typ
                        .map(|value| format!("/{value}"))
                        .unwrap_or_default()
                )
            })
        })
        .collect::<Vec<_>>();
    published_ports.sort();
    Some(docker::DockerProjectContainerSummary {
        id,
        name,
        state: container
            .state
            .map(|state| state.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        ip_addresses,
        published_ports,
    })
}

pub(crate) async fn load_project_record(
    state: &Arc<AppState>,
    name: &str,
) -> ApiResult<DockerProjectRecord> {
    let name = docker_projects::normalize_project_name(name)?;
    let row = sqlx::query(
        "SELECT name, compose_dir, management_kind, owner_name, \
         config_revision, applied_revision, created_at \
         FROM docker_compose_projects WHERE name = ?1",
    )
    .bind(&name)
    .fetch_optional(&state.metadata_db)
    .await?
    .ok_or_else(|| project_not_found(&name, "Docker project not found"))?;
    row_to_record(&row)
}

async fn load_project_records(state: &Arc<AppState>) -> ApiResult<Vec<DockerProjectRecord>> {
    let rows = sqlx::query(
        "SELECT name, compose_dir, management_kind, owner_name, \
         config_revision, applied_revision, created_at FROM docker_compose_projects",
    )
    .fetch_all(&state.metadata_db)
    .await?;
    rows.iter().map(row_to_record).collect()
}

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> ApiResult<DockerProjectRecord> {
    let management: String = row.try_get("management_kind")?;
    Ok(DockerProjectRecord {
        name: row.try_get("name")?,
        compose_dir: row.try_get("compose_dir")?,
        management_kind: docker_projects::parse_management_kind(&management)?,
        owner_name: row.try_get("owner_name")?,
        config_revision: row.try_get("config_revision")?,
        applied_revision: row.try_get("applied_revision")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn list_project_containers(
    state: &Arc<AppState>,
    project: &str,
) -> ApiResult<Vec<ContainerSummary>> {
    let mut filters = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![format!("com.docker.compose.project={project}")],
    );
    let options = query_parameters::ListContainersOptionsBuilder::new()
        .all(true)
        .filters(&filters)
        .build();
    Ok(state
        .docker_client()
        .await?
        .list_containers(Some(options))
        .await?)
}

pub(crate) fn project_directory(name: &str) -> PathBuf {
    config::compose_root_dir().join("docker").join(name)
}

pub(crate) async fn validate_compose_yaml(compose_yaml: &str) -> ApiResult<()> {
    let compose_yaml = compose_yaml.trim();
    if compose_yaml.is_empty() {
        return Err(ApiError::validation("Compose configuration is required"));
    }
    if compose_yaml.len() > MAX_COMPOSE_BYTES {
        return Err(ApiError::validation(
            "Compose configuration must not exceed 1 MiB",
        ));
    }
    let mut child = Command::new("docker")
        .args(["compose", "-f", "-", "config", "--format", "json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| ApiError::internal(format!("failed to start Docker Compose: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(compose_yaml.as_bytes()).await?;
    }
    let output = tokio::time::timeout(COMPOSE_VALIDATION_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ApiError::validation("Compose validation timed out"))??;
    if !output.status.success() {
        return Err(ApiError::validation(sanitize_error(
            &String::from_utf8_lossy(&output.stderr),
        )));
    }
    let normalized: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| ApiError::validation(sanitize_error(&error.to_string())))?;
    reject_reserved_labels(&normalized)?;
    Ok(())
}

fn reject_reserved_labels(value: &Value) -> ApiResult<()> {
    match value {
        Value::Object(mapping) => {
            for (key, value) in mapping {
                if key == "labels" {
                    inspect_labels(value)?;
                }
                reject_reserved_labels(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_reserved_labels(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn inspect_labels(value: &Value) -> ApiResult<()> {
    let keys = match value {
        Value::Object(mapping) => mapping.keys().cloned().collect::<Vec<_>>(),
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|value| {
                value
                    .split_once('=')
                    .map_or(Some(value), |(key, _)| Some(key))
            })
            .map(str::to_string)
            .collect::<Vec<_>>(),
        _ => return Err(ApiError::validation("Compose labels must be a map or list")),
    };
    if keys.iter().any(|key| {
        let key = key.trim();
        key.starts_with("vust.") || key.starts_with("com.docker.compose.")
    }) {
        return Err(ApiError::validation(
            "reserved vust.* and com.docker.compose.* labels are not allowed",
        ));
    }
    Ok(())
}

fn sanitize_error(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(1000).collect()
}

async fn restore_configuration(path: &FsPath, previous: Option<&[u8]>) {
    let result = match previous {
        Some(contents) => tokio::fs::write(path, contents).await,
        None => tokio::fs::remove_file(path).await,
    };
    if let Err(error) = result {
        tracing::error!(path = %path.display(), %error, "failed to restore Compose configuration");
    }
}

fn project_not_found(name: &str, message: &'static str) -> ApiError {
    ApiError::not_found(ErrorCode::DockerProjectNotFound, message)
        .with_detail(format!("project={name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved_labels_in_map_and_list_forms() {
        for json in [
            r#"{"services":{"app":{"labels":{"vust.owner":"fake"}}}}"#,
            r#"{"services":{"app":{"labels":["com.docker.compose.project=fake"]}}}"#,
        ] {
            let parsed: Value = serde_json::from_str(json).unwrap();
            assert!(reject_reserved_labels(&parsed).is_err());
        }
    }

    #[test]
    fn remote_compose_up_never_pulls_implicitly() {
        assert_eq!(
            compose_up_args(false, true),
            vec!["up", "--pull", "never", "-d", "--remove-orphans"]
        );
        assert_eq!(
            compose_up_args(true, true),
            vec![
                "up",
                "--pull",
                "never",
                "-d",
                "--force-recreate",
                "--remove-orphans"
            ]
        );
        assert!(!compose_up_args(false, false).contains(&"--pull".to_string()));
    }

    #[test]
    fn compose_config_images_exclude_build_only_services_and_deduplicate() {
        assert_eq!(
            parse_compose_config_images(
                br#"{
                    "services": {
                        "build-only": {"build": {"context": "/tmp/app"}},
                        "tagged-build": {
                            "image": "local/app:latest",
                            "build": {"context": "/tmp/tagged"}
                        },
                        "web": {"image": " nginx:latest "},
                        "worker": {"image": "redis:7"},
                        "cache": {"image": "redis:7"}
                    }
                }"#
            )
            .expect("parse images"),
            vec!["nginx:latest", "redis:7"]
        );
    }

    #[tokio::test]
    async fn runtime_registry_layers_are_persisted_as_compose_children() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        sqlx::query(
            "INSERT INTO docker_compose_project_tasks (\
                id, project_name, operation, status, stage, progress_percent, pull_images, \
                actor_kind, actor_name, trace_id\
             ) VALUES ('runtime-layers', 'demo', 'create', 'running', 'pulling', 25, 1, \
                'system', 'system', 'runtime-layer-trace')",
        )
        .execute(&state.metadata_db)
        .await
        .expect("insert task");
        let parent_id = image_progress_id("nginx:latest");

        record_runtime_image_progress(
            &state,
            "runtime-layers",
            "nginx:latest",
            &parent_id,
            RuntimeImageTask {
                task_id: "image-task".to_string(),
                image_ref: "nginx:latest".to_string(),
                status: RuntimeImageTaskStatus::Running,
                source: None,
                stage: RuntimeImageStage::Pulling,
                progress_percent: 48,
                status_text: "Pulling image from registry".to_string(),
                layers: vec![vust_contracts::runtime_docker::RuntimeImageLayerProgress {
                    id: "sha256:abc".to_string(),
                    status_text: "Downloading".to_string(),
                    percent: Some(42),
                }],
            },
        )
        .await;

        let items = docker_project_tasks::progress_items(&state.metadata_db, "runtime-layers")
            .await
            .expect("load progress");
        assert_eq!(items.len(), 2);
        let child = items
            .iter()
            .find(|item| item.parent_id.as_deref() == Some(parent_id.as_str()))
            .expect("layer child");
        assert_eq!(child.label, "sha256:abc");
        assert_eq!(child.percent, Some(42));
        assert_eq!(child.status, docker::DockerProjectProgressStatus::Working);
    }

    #[tokio::test]
    async fn compose_registry_pull_uses_task_actor_and_image_target() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        sqlx::query(
            "INSERT INTO docker_compose_project_tasks (\
                id, project_name, operation, status, stage, progress_percent, pull_images, \
                actor_kind, actor_user_id, actor_name, client_ip, trace_id\
             ) VALUES ('registry-audit', 'demo', 'redeploy', 'running', 'pulling', 25, 1, \
                'user', 7, 'admin', '10.0.0.7', 'compose-trace')",
        )
        .execute(&state.metadata_db)
        .await
        .expect("insert task");

        record_registry_pull_activities(
            &state,
            "registry-audit",
            vec![(
                "nginx:latest".to_string(),
                RegistryPullAuditOutcome::Success,
            )],
        )
        .await;

        let events = crate::services::operation_outbox::pending(&state.metadata_db, 10)
            .await
            .expect("load outbox");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_code, "docker_image_pulled_from_registry");
        assert_eq!(events[0].actor.display_name, "admin");
        assert_eq!(events[0].task_id.as_deref(), Some("registry-audit"));
        assert_eq!(
            events[0].target.as_ref().map(|target| target.id.as_str()),
            Some("nginx:latest")
        );
    }

    #[tokio::test]
    async fn compose_registry_pull_records_each_image_result_from_progress() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        sqlx::query(
            "INSERT INTO docker_compose_project_tasks (\
                id, project_name, operation, status, stage, progress_percent, pull_images, \
                actor_kind, actor_name, trace_id\
             ) VALUES ('partial-registry-audit', 'demo', 'redeploy', 'running', 'pulling', 25, 1, \
                'system', 'scheduler', 'partial-compose-trace')",
        )
        .execute(&state.metadata_db)
        .await
        .expect("insert task");
        for (image_ref, status, details) in [
            (
                "nginx:latest",
                docker::DockerProjectProgressStatus::Done,
                None,
            ),
            (
                "redis:7",
                docker::DockerProjectProgressStatus::Error,
                Some("registry denied".to_string()),
            ),
        ] {
            docker_project_tasks::record_progress_item(
                &state.metadata_db,
                "partial-registry-audit",
                docker::DockerProjectTaskProgressItem {
                    id: format!("pulling:Image {image_ref}"),
                    parent_id: None,
                    phase: docker::DockerProjectProgressPhase::Pulling,
                    status,
                    label: format!("Image {image_ref}"),
                    action: "Pull complete".to_string(),
                    details,
                    current_bytes: None,
                    total_bytes: None,
                    percent: Some(100),
                },
            )
            .await
            .expect("record image progress");
        }

        record_tracked_registry_pull_activities(
            &state,
            "partial-registry-audit",
            &["nginx:latest".to_string(), "redis:7".to_string()],
            RegistryPullAuditOutcome::Failure("compose pull failed".to_string()),
        )
        .await;

        let events = crate::services::operation_outbox::pending(&state.metadata_db, 10)
            .await
            .expect("load outbox");
        let outcome_for = |image_ref: &str| {
            events
                .iter()
                .find(|event| {
                    event
                        .target
                        .as_ref()
                        .is_some_and(|target| target.id == image_ref)
                })
                .map(|event| event.outcome)
        };
        assert_eq!(
            outcome_for("nginx:latest"),
            Some(vust_contracts::logging::OperationOutcome::Success)
        );
        assert_eq!(
            outcome_for("redis:7"),
            Some(vust_contracts::logging::OperationOutcome::Failure)
        );
    }
}
