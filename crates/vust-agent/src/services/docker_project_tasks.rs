//! Docker Compose 项目后台任务持久化、互斥、取消与命令执行。

use crate::api::docker::context::DockerOperationContext;
use crate::models::docker::{
    DockerActivityActorKind, DockerProjectProgressMode, DockerProjectProgressPhase,
    DockerProjectProgressStatus, DockerProjectTask, DockerProjectTaskOperation,
    DockerProjectTaskProgressItem, DockerProjectTaskProgressUpdate, DockerProjectTaskStage,
    DockerProjectTaskStatus,
};
use crate::state::DbPool;
use crate::types::{ApiError, ApiResult};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, OnceCell, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vust_contracts::api::ErrorCode;

const TASK_RETENTION_SECONDS: i64 = 24 * 60 * 60;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const COMPOSE_CAPABILITY_TIMEOUT: Duration = Duration::from_secs(5);
const PROGRESS_PERSIST_INTERVAL: Duration = Duration::from_millis(250);
const MAX_PROGRESS_ITEMS: usize = 200;
const MAX_PROGRESS_LABEL_CHARS: usize = 240;
const MAX_PROGRESS_DETAIL_CHARS: usize = 500;

static CANCELLATIONS: Lazy<Mutex<HashMap<String, CancellationToken>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static CREATE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static JSON_PROGRESS_SUPPORTED: OnceCell<bool> = OnceCell::const_new();
static TASK_EVENT_HUBS: Lazy<Mutex<HashMap<String, DockerProjectTaskEventHub>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const TASK_EVENT_BUFFER: usize = 512;

/// Compose 项目任务事件流中的实时消息。
#[derive(Debug, Clone)]
pub enum DockerProjectTaskEvent {
    Snapshot(DockerProjectTask),
    Progress(DockerProjectTaskProgressUpdate),
    Terminal(DockerProjectTask),
}

struct DockerProjectTaskEventHub {
    sender: broadcast::Sender<DockerProjectTaskEvent>,
    latest_progress: Vec<DockerProjectTaskProgressUpdate>,
}

/// 创建任务时允许保存的非敏感参数。
pub struct NewDockerProjectTask<'a> {
    pub project_name: &'a str,
    pub operation: DockerProjectTaskOperation,
    pub service_name: Option<&'a str>,
    pub replicas: Option<usize>,
    pub pull_images: bool,
}

/// Compose 命令执行结果。
pub enum ComposeCommandResult {
    Succeeded,
    Cancelled,
}

/// Agent 启动时关闭不可恢复的旧任务并清理过期记录。
pub async fn initialize(pool: &DbPool) -> ApiResult<()> {
    let interrupted_task_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM docker_compose_project_tasks \
         WHERE status IN ('queued', 'running') ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    sqlx::query(
        "UPDATE docker_compose_project_tasks \
         SET status = 'failed', stage = 'interrupted', progress_percent = 100, \
             error_summary = 'Agent restarted before the task completed', finished_at = unixepoch() \
         WHERE status IN ('queued', 'running')",
    )
    .execute(pool)
    .await?;
    for task_id in interrupted_task_ids {
        record_terminal_activity(pool, &task_id).await;
    }
    cleanup_expired(pool).await?;
    Ok(())
}

/// 每日清理超过保留期的项目任务。
pub fn spawn_retention_worker(pool: DbPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = cleanup_expired(&pool).await {
                tracing::error!(%error, "failed to clean expired Docker project tasks");
            }
        }
    });
}

/// 创建持久化任务，并为同一项目提供进程内互斥。
pub async fn create(
    pool: &DbPool,
    context: &DockerOperationContext,
    request: NewDockerProjectTask<'_>,
) -> ApiResult<DockerProjectTask> {
    let _guard = CREATE_LOCK.lock().await;
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM docker_compose_project_tasks \
         WHERE project_name = ?1 AND status IN ('queued', 'running')",
    )
    .bind(request.project_name)
    .fetch_one(pool)
    .await?;
    if active > 0 {
        return Err(ApiError::conflict(
            ErrorCode::DockerProjectBusy,
            "Docker project already has an active task",
        ));
    }
    let id = Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO docker_compose_project_tasks (\
            id, project_name, operation, status, stage, progress_percent, \
            service_name, replicas, pull_images, actor_kind, actor_user_id, actor_name, client_ip, trace_id\
         ) VALUES (?1, ?2, ?3, 'queued', 'preparing', 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(&id)
    .bind(request.project_name)
    .bind(request.operation.as_str())
    .bind(request.service_name)
    .bind(request.replicas.map(|value| value as i64))
    .bind(request.pull_images)
    .bind(context.actor_kind.as_str())
    .bind(context.actor_user_id)
    .bind(&context.actor_name)
    .bind(&context.client_ip)
    .bind(&context.trace_id)
    .execute(pool)
    .await?;
    let cancellation = CancellationToken::new();
    CANCELLATIONS.lock().await.insert(id.clone(), cancellation);
    let (sender, _) = broadcast::channel(TASK_EVENT_BUFFER);
    TASK_EVENT_HUBS.lock().await.insert(
        id.clone(),
        DockerProjectTaskEventHub {
            sender,
            latest_progress: Vec::new(),
        },
    );
    get(pool, &id).await
}

/// 返回任务的取消令牌。
pub async fn cancellation(task_id: &str) -> Option<CancellationToken> {
    CANCELLATIONS.lock().await.get(task_id).cloned()
}

/// 读取 Compose 后台任务保存的可信操作者，供任务内部子操作复用。
pub async fn operation_context(pool: &DbPool, task_id: &str) -> ApiResult<DockerOperationContext> {
    let row = sqlx::query(
        "SELECT actor_kind, actor_user_id, actor_name, client_ip, trace_id \
         FROM docker_compose_project_tasks WHERE id = ?1",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            ErrorCode::TaskNotFound,
            "Docker project task audit context not found",
        )
    })?;
    let actor_kind = match row.try_get::<String, _>("actor_kind")?.as_str() {
        "user" => DockerActivityActorKind::User,
        "system" => DockerActivityActorKind::System,
        _ => return Err(ApiError::internal("invalid Docker project task actor kind")),
    };
    Ok(DockerOperationContext {
        actor_kind,
        actor_user_id: row.try_get("actor_user_id").ok().flatten(),
        actor_name: row
            .try_get("actor_name")
            .unwrap_or_else(|_| "unknown".to_string()),
        client_ip: row.try_get("client_ip").ok().flatten(),
        trace_id: row.try_get("trace_id").ok().flatten(),
    })
}

/// 订阅任务事件，并返回订阅建立前已经产生的最新单项进度。
pub async fn subscribe(
    task_id: &str,
) -> Option<(
    broadcast::Receiver<DockerProjectTaskEvent>,
    Vec<DockerProjectTaskProgressUpdate>,
)> {
    let hubs = TASK_EVENT_HUBS.lock().await;
    let hub = hubs.get(task_id)?;
    Some((hub.sender.subscribe(), hub.latest_progress.clone()))
}

async fn publish_progress(task_id: &str, update: DockerProjectTaskProgressUpdate) {
    let mut hubs = TASK_EVENT_HUBS.lock().await;
    let Some(hub) = hubs.get_mut(task_id) else {
        return;
    };
    if let Some(existing) = hub
        .latest_progress
        .iter_mut()
        .find(|existing| existing.item.id == update.item.id)
    {
        *existing = update.clone();
    } else {
        hub.latest_progress.push(update.clone());
    }
    let _ = hub.sender.send(DockerProjectTaskEvent::Progress(update));
}

/// 合并并持久化由 Compose 外部镜像编排产生的单项进度。
pub async fn record_progress_item(
    pool: &DbPool,
    task_id: &str,
    item: DockerProjectTaskProgressItem,
) -> ApiResult<()> {
    record_progress_items(pool, task_id, vec![item]).await
}

/// 批量合并同一阶段的外部镜像进度，避免每个分层重复读写任务快照。
pub async fn record_progress_items(
    pool: &DbPool,
    task_id: &str,
    incoming: Vec<DockerProjectTaskProgressItem>,
) -> ApiResult<()> {
    let Some(phase) = incoming.first().map(|item| item.phase) else {
        return Ok(());
    };
    let mut items = load_progress_items(pool, task_id).await?;
    let mut changed = Vec::new();
    for item in incoming {
        if item.phase == phase && upsert_progress_item(&mut items, item.clone()) {
            changed.push(item);
        }
    }
    if changed.is_empty() {
        return Ok(());
    }
    let progress_percent = aggregate_progress_percent(&items, phase);
    persist_progress(
        pool,
        task_id,
        DockerProjectProgressMode::Structured,
        &items,
        phase,
    )
    .await?;
    for item in changed {
        publish_progress(
            task_id,
            DockerProjectTaskProgressUpdate {
                progress_mode: DockerProjectProgressMode::Structured,
                progress_percent,
                item,
            },
        )
        .await;
    }
    Ok(())
}

async fn publish_snapshot(pool: &DbPool, task_id: &str, terminal: bool) {
    let Ok(task) = get(pool, task_id).await else {
        return;
    };
    let mut hubs = TASK_EVENT_HUBS.lock().await;
    let Some(hub) = hubs.get(task_id) else {
        return;
    };
    let event = if terminal {
        DockerProjectTaskEvent::Terminal(task)
    } else {
        DockerProjectTaskEvent::Snapshot(task)
    };
    let _ = hub.sender.send(event);
    if terminal {
        hubs.remove(task_id);
    }
}

/// 将任务推进到运行阶段。
pub async fn update(
    pool: &DbPool,
    task_id: &str,
    stage: DockerProjectTaskStage,
    progress_percent: u8,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE docker_compose_project_tasks SET status = 'running', stage = ?2, \
         progress_percent = ?3, started_at = COALESCE(started_at, unixepoch()) WHERE id = ?1",
    )
    .bind(task_id)
    .bind(stage_as_str(stage))
    .bind(i64::from(progress_percent))
    .execute(pool)
    .await?;
    publish_snapshot(pool, task_id, false).await;
    Ok(())
}

/// 将任务标记为成功。
pub async fn succeed(pool: &DbPool, task_id: &str) -> ApiResult<()> {
    finish(pool, task_id, "succeeded", "completed", None, None).await
}

/// 将任务标记为成功，并保留不影响业务结果的清理警告。
pub async fn succeed_with_warning(
    pool: &DbPool,
    task_id: &str,
    cleanup_warning: &str,
) -> ApiResult<()> {
    finish(
        pool,
        task_id,
        "succeeded",
        "completed",
        None,
        Some(cleanup_warning),
    )
    .await
}

/// 将任务标记为失败，并保存截断后的安全摘要。
pub async fn fail(
    pool: &DbPool,
    task_id: &str,
    error: &str,
    cleanup_warning: Option<&str>,
) -> ApiResult<()> {
    finish(
        pool,
        task_id,
        "failed",
        "completed",
        Some(error),
        cleanup_warning,
    )
    .await
}

/// 将任务标记为已取消。
pub async fn cancelled(
    pool: &DbPool,
    task_id: &str,
    cleanup_warning: Option<&str>,
) -> ApiResult<()> {
    finish(
        pool,
        task_id,
        "cancelled",
        "cancelled",
        None,
        cleanup_warning,
    )
    .await
}

async fn finish(
    pool: &DbPool,
    task_id: &str,
    status: &str,
    stage: &str,
    error: Option<&str>,
    cleanup_warning: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE docker_compose_project_tasks SET status = ?2, stage = ?3, \
         progress_percent = 100, error_summary = ?4, cleanup_warning = ?5, \
         finished_at = unixepoch() WHERE id = ?1",
    )
    .bind(task_id)
    .bind(status)
    .bind(stage)
    .bind(error.map(sanitize_error))
    .bind(cleanup_warning.map(sanitize_error))
    .execute(pool)
    .await?;
    publish_snapshot(pool, task_id, true).await;
    CANCELLATIONS.lock().await.remove(task_id);
    record_terminal_activity(pool, task_id).await;
    Ok(())
}

async fn record_terminal_activity(pool: &DbPool, task_id: &str) {
    let row = match sqlx::query(
        "SELECT project_name, operation, status, service_name, replicas, pull_images, \
         actor_kind, actor_user_id, actor_name, client_ip, trace_id, error_summary \
         FROM docker_compose_project_tasks WHERE id = ?1",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(%error, task_id, "failed to load Compose task audit context");
            return;
        }
    };
    let context = match operation_context(pool, task_id).await {
        Ok(context) => context,
        Err(error) => {
            tracing::error!(%error, task_id, "failed to load Compose task actor");
            return;
        }
    };
    let project_name: String = match row.try_get("project_name") {
        Ok(value) => value,
        Err(_) => return,
    };
    let operation: String = match row.try_get("operation") {
        Ok(value) => value,
        Err(_) => return,
    };
    let status: String = match row.try_get("status") {
        Ok(value) => value,
        Err(_) => return,
    };
    let params = json!({
        "taskId": task_id,
        "name": project_name,
        "operation": operation,
        "service": row.try_get::<Option<String>, _>("service_name").ok().flatten(),
        "replicas": row.try_get::<Option<i64>, _>("replicas").ok().flatten(),
        "pullImages": row.try_get::<bool, _>("pull_images").unwrap_or(false),
    });
    let event_code = compose_operation_event_code(&operation);
    if status == "failed" {
        let error = row
            .try_get::<Option<String>, _>("error_summary")
            .ok()
            .flatten()
            .unwrap_or_else(|| "Docker project task failed".to_string());
        context
            .record_failure(
                pool,
                event_code,
                Some(("composeProject", &project_name)),
                params,
                error,
            )
            .await;
    } else {
        if status == "cancelled" {
            context
                .record_canceled(
                    pool,
                    event_code,
                    Some(("composeProject", &project_name)),
                    params,
                )
                .await;
        } else {
            context
                .record_success(
                    pool,
                    event_code,
                    Some(("composeProject", &project_name)),
                    params,
                    operation == "redeploy" || operation == "remove",
                )
                .await;
        }
    }
}

/// 将 Compose 任务操作映射为稳定的 Docker 审计事件码。
fn compose_operation_event_code(operation: &str) -> &'static str {
    match operation {
        "create" => "docker_compose_project_create",
        "start" => "docker_compose_project_start",
        "stop" => "docker_compose_project_stop",
        "restart" => "docker_compose_project_restart",
        "redeploy" => "docker_compose_project_redeploy",
        "scale" => "docker_compose_project_scale",
        "remove" => "docker_compose_project_remove",
        _ => "docker_compose_project_operation",
    }
}

/// 查询一个任务。
pub async fn get(pool: &DbPool, task_id: &str) -> ApiResult<DockerProjectTask> {
    let row = sqlx::query(
        "SELECT id, project_name, operation, status, stage, progress_percent, \
         progress_mode, progress_items, service_name, \
         replicas, pull_images, error_summary, cleanup_warning, created_at, started_at, finished_at \
         FROM docker_compose_project_tasks WHERE id = ?1",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found(ErrorCode::TaskNotFound, "Docker project task not found"))?;
    row_to_task(&row)
}

/// 返回最近提交且仍在执行的创建或重新部署操作。
pub async fn latest_active_deployment(pool: &DbPool) -> ApiResult<Option<DockerProjectTask>> {
    let row = sqlx::query(
        "SELECT id, project_name, operation, status, stage, progress_percent, \
         progress_mode, progress_items, service_name, \
         replicas, pull_images, error_summary, cleanup_warning, created_at, started_at, finished_at \
         FROM docker_compose_project_tasks \
         WHERE status IN ('queued', 'running') AND operation IN ('create', 'redeploy') \
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_task).transpose()
}

/// 执行可取消、可超时且不会记录完整命令输出的 Compose 命令。
pub async fn run_compose_command(
    project: &str,
    compose_file: &Path,
    args: &[String],
    cancellation: &CancellationToken,
) -> ApiResult<ComposeCommandResult> {
    let mut command = Command::new("docker");
    command
        .args(["compose", "-f"])
        .arg(compose_file)
        .args(["-p", project])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let future = command.output();
    tokio::pin!(future);
    let output = tokio::select! {
        _ = cancellation.cancelled() => return Ok(ComposeCommandResult::Cancelled),
        result = tokio::time::timeout(COMMAND_TIMEOUT, &mut future) => {
            result.map_err(|_| ApiError::validation("Docker Compose command timed out"))??
        }
    };
    if !output.status.success() {
        return Err(ApiError::validation(sanitize_error(
            &String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(ComposeCommandResult::Succeeded)
}

/// 执行 Compose 命令并把结构化资源事件持久化到任务快照。
pub async fn run_tracked_compose_command(
    pool: &DbPool,
    task_id: &str,
    phase: DockerProjectProgressPhase,
    project: &str,
    compose_file: &Path,
    args: &[String],
    cancellation: &CancellationToken,
) -> ApiResult<ComposeCommandResult> {
    let structured = compose_supports_json_progress().await;
    let mode = if structured {
        DockerProjectProgressMode::Structured
    } else {
        DockerProjectProgressMode::Text
    };
    let mut command = Command::new("docker");
    command.arg("compose").args(["--ansi", "never"]);
    if structured {
        command.args(["--progress", "json"]);
    }
    command
        .args(["-f"])
        .arg(compose_file)
        .args(["-p", project])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| ApiError::internal(format!("failed to start Docker Compose: {error}")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal("failed to capture Docker Compose progress"))?;
    let mut lines = BufReader::new(stderr).lines();
    let timeout = tokio::time::sleep(COMMAND_TIMEOUT);
    tokio::pin!(timeout);
    let mut items = load_progress_items(pool, task_id).await?;
    let mut text_sequence = 0usize;
    let mut progress_dirty = false;
    let mut persist_interval = tokio::time::interval(PROGRESS_PERSIST_INTERVAL);
    persist_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    persist_interval.tick().await;
    let mut error_lines = Vec::new();

    loop {
        let line = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Ok(ComposeCommandResult::Cancelled);
            }
            _ = &mut timeout => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(ApiError::validation("Docker Compose command timed out"));
            }
            _ = persist_interval.tick() => {
                if progress_dirty {
                    persist_progress(pool, task_id, mode, &items, phase).await?;
                    progress_dirty = false;
                }
                continue;
            }
            result = lines.next_line() => result?,
        };
        let Some(line) = line else {
            break;
        };
        let sanitized_line = truncate_chars(&sanitize_error(&line), MAX_PROGRESS_DETAIL_CHARS);
        if !sanitized_line.is_empty() {
            error_lines.push(sanitized_line);
            if error_lines.len() > 20 {
                error_lines.remove(0);
            }
        }
        let changed_item = if structured {
            parse_progress_event(&line, phase).and_then(|item| {
                if upsert_progress_item(&mut items, item.clone()) {
                    Some(item)
                } else {
                    None
                }
            })
        } else {
            text_sequence += 1;
            if upsert_text_progress_item(&mut items, phase, text_sequence, &line) {
                items.last().cloned()
            } else {
                None
            }
        };
        if let Some(item) = changed_item {
            let progress_percent = aggregate_progress_percent(&items, phase);
            publish_progress(
                task_id,
                DockerProjectTaskProgressUpdate {
                    progress_mode: mode,
                    progress_percent,
                    item,
                },
            )
            .await;
            progress_dirty = true;
        }
    }
    let status = child.wait().await?;
    if status.success() {
        for item in items.iter_mut().filter(|item| {
            item.phase == phase && item.status == DockerProjectProgressStatus::Working
        }) {
            item.status = DockerProjectProgressStatus::Done;
        }
    }
    persist_progress(pool, task_id, mode, &items, phase).await?;
    if !status.success() {
        let message = if error_lines.is_empty() {
            "Docker Compose command failed".to_string()
        } else {
            error_lines.join(" ")
        };
        return Err(ApiError::validation(sanitize_error(&message)));
    }
    Ok(ComposeCommandResult::Succeeded)
}

#[derive(Debug, Deserialize)]
struct ComposeProgressEvent {
    id: Option<String>,
    parent_id: Option<String>,
    status: Option<String>,
    text: Option<String>,
    details: Option<String>,
    current: Option<i64>,
    total: Option<i64>,
    percent: Option<i64>,
}

async fn compose_supports_json_progress() -> bool {
    *JSON_PROGRESS_SUPPORTED
        .get_or_init(|| async {
            let output = tokio::time::timeout(
                COMPOSE_CAPABILITY_TIMEOUT,
                Command::new("docker")
                    .args(["compose", "--help"])
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output(),
            )
            .await;
            matches!(
                output,
                Ok(Ok(output))
                    if output.status.success()
                        && String::from_utf8_lossy(&output.stdout).contains("--progress")
                        && String::from_utf8_lossy(&output.stdout).contains("json")
            )
        })
        .await
}

fn parse_progress_event(
    line: &str,
    phase: DockerProjectProgressPhase,
) -> Option<DockerProjectTaskProgressItem> {
    let event: ComposeProgressEvent = serde_json::from_str(line).ok()?;
    let raw_id = event.id?.trim().to_string();
    if raw_id.is_empty() {
        return None;
    }
    let namespace = phase_as_str(phase);
    let id = format!("{namespace}:{raw_id}");
    let parent_id = event
        .parent_id
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("{namespace}:{}", value.trim()));
    let label = truncate_chars(&sanitize_error(&raw_id), MAX_PROGRESS_LABEL_CHARS);
    let action = truncate_chars(
        &sanitize_error(event.text.as_deref().unwrap_or_default()),
        MAX_PROGRESS_LABEL_CHARS,
    );
    let details = event
        .details
        .map(|value| truncate_chars(&sanitize_error(&value), MAX_PROGRESS_DETAIL_CHARS))
        .filter(|value| !value.is_empty());
    let total_bytes = event.total.filter(|value| *value > 0);
    let current_bytes = event
        .current
        .filter(|value| *value >= 0 && total_bytes.is_some());
    let percent = event
        .percent
        .filter(|value| *value >= 0)
        .map(|value| value.clamp(0, 100) as u8)
        .or_else(|| {
            current_bytes
                .zip(total_bytes)
                .map(|(current, total)| ((current.saturating_mul(100) / total).clamp(0, 100)) as u8)
        });
    Some(DockerProjectTaskProgressItem {
        id,
        parent_id,
        phase,
        status: parse_progress_status(event.status.as_deref()),
        label,
        action,
        details,
        current_bytes,
        total_bytes,
        percent,
    })
}

fn parse_progress_status(value: Option<&str>) -> DockerProjectProgressStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "done" => DockerProjectProgressStatus::Done,
        "warning" => DockerProjectProgressStatus::Warning,
        "error" => DockerProjectProgressStatus::Error,
        _ => DockerProjectProgressStatus::Working,
    }
}

fn upsert_progress_item(
    items: &mut Vec<DockerProjectTaskProgressItem>,
    item: DockerProjectTaskProgressItem,
) -> bool {
    if let Some(existing) = items.iter_mut().find(|existing| existing.id == item.id) {
        if existing == &item {
            return false;
        }
        *existing = item;
        return true;
    }
    items.push(item);
    if items.len() > MAX_PROGRESS_ITEMS {
        items.remove(0);
    }
    true
}

fn upsert_text_progress_item(
    items: &mut Vec<DockerProjectTaskProgressItem>,
    phase: DockerProjectProgressPhase,
    sequence: usize,
    line: &str,
) -> bool {
    let label = truncate_chars(&sanitize_error(line), MAX_PROGRESS_DETAIL_CHARS);
    if label.is_empty() {
        return false;
    }
    upsert_progress_item(
        items,
        DockerProjectTaskProgressItem {
            id: format!("{}:text:{sequence}", phase_as_str(phase)),
            parent_id: None,
            phase,
            status: if label.to_ascii_lowercase().contains("error") {
                DockerProjectProgressStatus::Error
            } else {
                DockerProjectProgressStatus::Working
            },
            label,
            action: String::new(),
            details: None,
            current_bytes: None,
            total_bytes: None,
            percent: None,
        },
    )
}

async fn load_progress_items(
    pool: &DbPool,
    task_id: &str,
) -> ApiResult<Vec<DockerProjectTaskProgressItem>> {
    let value: String =
        sqlx::query_scalar("SELECT progress_items FROM docker_compose_project_tasks WHERE id = ?1")
            .bind(task_id)
            .fetch_one(pool)
            .await?;
    Ok(serde_json::from_str(&value).unwrap_or_default())
}

/// 读取任务当前持久化的进度项，供终态子操作审计使用。
pub async fn progress_items(
    pool: &DbPool,
    task_id: &str,
) -> ApiResult<Vec<DockerProjectTaskProgressItem>> {
    load_progress_items(pool, task_id).await
}

async fn persist_progress(
    pool: &DbPool,
    task_id: &str,
    mode: DockerProjectProgressMode,
    items: &[DockerProjectTaskProgressItem],
    phase: DockerProjectProgressPhase,
) -> ApiResult<()> {
    let progress_percent = aggregate_progress_percent(items, phase);
    let progress_items = serde_json::to_string(items).map_err(|error| {
        ApiError::internal(format!("failed to serialize Compose progress: {error}"))
    })?;
    sqlx::query(
        "UPDATE docker_compose_project_tasks \
         SET progress_mode = ?2, progress_items = ?3, \
             progress_percent = MAX(progress_percent, ?4) WHERE id = ?1",
    )
    .bind(task_id)
    .bind(progress_mode_as_str(mode))
    .bind(progress_items)
    .bind(i64::from(progress_percent))
    .execute(pool)
    .await?;
    Ok(())
}

fn aggregate_progress_percent(
    items: &[DockerProjectTaskProgressItem],
    phase: DockerProjectProgressPhase,
) -> u8 {
    let parent_ids = items
        .iter()
        .filter_map(|item| item.parent_id.as_deref())
        .collect::<std::collections::HashSet<_>>();
    let leaves = items
        .iter()
        .filter(|item| item.phase == phase && !parent_ids.contains(item.id.as_str()))
        .collect::<Vec<_>>();
    let (weighted_current, weighted_total) = leaves
        .iter()
        .filter_map(|item| item.current_bytes.zip(item.total_bytes))
        .fold(
            (0_i64, 0_i64),
            |(current, total), (item_current, item_total)| {
                (
                    current.saturating_add(item_current.min(item_total)),
                    total.saturating_add(item_total),
                )
            },
        );
    let detail_percent = if weighted_total > 0 {
        (weighted_current.saturating_mul(100) / weighted_total).clamp(0, 100) as u8
    } else {
        let values = leaves
            .iter()
            .filter_map(|item| item.percent)
            .collect::<Vec<_>>();
        if values.is_empty() {
            0
        } else {
            values.iter().map(|value| u32::from(*value)).sum::<u32>() as u8 / values.len() as u8
        }
    };
    match phase {
        DockerProjectProgressPhase::Pulling => 25 + ((u16::from(detail_percent) * 29) / 100) as u8,
        DockerProjectProgressPhase::Applying => 55 + ((u16::from(detail_percent) * 34) / 100) as u8,
    }
}

const fn phase_as_str(phase: DockerProjectProgressPhase) -> &'static str {
    match phase {
        DockerProjectProgressPhase::Pulling => "pulling",
        DockerProjectProgressPhase::Applying => "applying",
    }
}

const fn progress_mode_as_str(mode: DockerProjectProgressMode) -> &'static str {
    match mode {
        DockerProjectProgressMode::Structured => "structured",
        DockerProjectProgressMode::Text => "text",
        DockerProjectProgressMode::Unavailable => "unavailable",
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// 清理超过保留期的终态任务。
pub async fn cleanup_expired(pool: &DbPool) -> ApiResult<()> {
    sqlx::query(
        "DELETE FROM docker_compose_project_tasks WHERE status IN ('succeeded', 'failed', 'cancelled') \
         AND created_at < unixepoch() - ?1",
    )
    .bind(TASK_RETENTION_SECONDS)
    .execute(pool)
    .await?;
    Ok(())
}

fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> ApiResult<DockerProjectTask> {
    let operation: String = row.try_get("operation")?;
    let status: String = row.try_get("status")?;
    let stage: String = row.try_get("stage")?;
    Ok(DockerProjectTask {
        id: row.try_get("id")?,
        project_name: row.try_get("project_name")?,
        operation: parse_operation(&operation)?,
        status: parse_status(&status)?,
        stage: parse_stage(&stage)?,
        progress_percent: row.try_get::<i64, _>("progress_percent")? as u8,
        progress_mode: parse_progress_mode(
            &row.try_get::<String, _>("progress_mode")
                .unwrap_or_else(|_| "unavailable".to_string()),
        ),
        progress_items: row
            .try_get::<String, _>("progress_items")
            .ok()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default(),
        service_name: row.try_get("service_name")?,
        replicas: row
            .try_get::<Option<i64>, _>("replicas")?
            .map(|value| value as usize),
        pull_images: row.try_get("pull_images")?,
        error_summary: row.try_get("error_summary")?,
        cleanup_warning: row.try_get("cleanup_warning")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
    })
}

fn parse_progress_mode(value: &str) -> DockerProjectProgressMode {
    match value {
        "structured" => DockerProjectProgressMode::Structured,
        "text" => DockerProjectProgressMode::Text,
        _ => DockerProjectProgressMode::Unavailable,
    }
}

fn parse_operation(value: &str) -> ApiResult<DockerProjectTaskOperation> {
    match value {
        "create" => Ok(DockerProjectTaskOperation::Create),
        "start" => Ok(DockerProjectTaskOperation::Start),
        "stop" => Ok(DockerProjectTaskOperation::Stop),
        "restart" => Ok(DockerProjectTaskOperation::Restart),
        "redeploy" => Ok(DockerProjectTaskOperation::Redeploy),
        "scale" => Ok(DockerProjectTaskOperation::Scale),
        "remove" => Ok(DockerProjectTaskOperation::Remove),
        _ => Err(ApiError::internal("invalid Docker project task operation")),
    }
}

fn parse_status(value: &str) -> ApiResult<DockerProjectTaskStatus> {
    match value {
        "queued" => Ok(DockerProjectTaskStatus::Queued),
        "running" => Ok(DockerProjectTaskStatus::Running),
        "succeeded" => Ok(DockerProjectTaskStatus::Succeeded),
        "failed" => Ok(DockerProjectTaskStatus::Failed),
        "cancelled" => Ok(DockerProjectTaskStatus::Cancelled),
        _ => Err(ApiError::internal("invalid Docker project task status")),
    }
}

fn parse_stage(value: &str) -> ApiResult<DockerProjectTaskStage> {
    match value {
        "validating" => Ok(DockerProjectTaskStage::Validating),
        "preparing" => Ok(DockerProjectTaskStage::Preparing),
        "pulling" => Ok(DockerProjectTaskStage::Pulling),
        "applying" => Ok(DockerProjectTaskStage::Applying),
        "verifying" => Ok(DockerProjectTaskStage::Verifying),
        "rolling_back" => Ok(DockerProjectTaskStage::RollingBack),
        "cleaning_up" => Ok(DockerProjectTaskStage::CleaningUp),
        "completed" => Ok(DockerProjectTaskStage::Completed),
        "cancelled" => Ok(DockerProjectTaskStage::Cancelled),
        "interrupted" => Ok(DockerProjectTaskStage::Interrupted),
        _ => Err(ApiError::internal("invalid Docker project task stage")),
    }
}

const fn stage_as_str(stage: DockerProjectTaskStage) -> &'static str {
    match stage {
        DockerProjectTaskStage::Validating => "validating",
        DockerProjectTaskStage::Preparing => "preparing",
        DockerProjectTaskStage::Pulling => "pulling",
        DockerProjectTaskStage::Applying => "applying",
        DockerProjectTaskStage::Verifying => "verifying",
        DockerProjectTaskStage::RollingBack => "rolling_back",
        DockerProjectTaskStage::CleaningUp => "cleaning_up",
        DockerProjectTaskStage::Completed => "completed",
        DockerProjectTaskStage::Cancelled => "cancelled",
        DockerProjectTaskStage::Interrupted => "interrupted",
    }
}

fn sanitize_error(value: &str) -> String {
    let mut sanitized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(1000)
        .collect::<String>();
    for key in ["password", "token", "authorization"] {
        sanitized = redact_assignment(&sanitized, key);
    }
    sanitized.chars().take(1000).collect()
}

fn redact_assignment(value: &str, key: &str) -> String {
    let mut output = value.to_string();
    let mut search_from = 0;
    loop {
        let lower = output.to_ascii_lowercase();
        let Some(relative) = lower[search_from..].find(key) else {
            break;
        };
        let start = search_from + relative + key.len();
        let Some(separator) = lower[start..].find(['=', ':']) else {
            break;
        };
        let value_start = start + separator + 1;
        let value_end = lower[value_start..]
            .find([',', '&', ' '])
            .map(|offset| value_start + offset)
            .unwrap_or(output.len());
        output.replace_range(value_start..value_end, "[REDACTED]");
        search_from = value_start + "[REDACTED]".len();
        if search_from >= output.len() {
            break;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespaced_structured_progress() {
        let item = parse_progress_event(
            r#"{"id":"sha256:abc","parent_id":"Image nginx","status":"Working","text":"Downloading","details":"layer","current":512,"total":1024,"percent":50}"#,
            DockerProjectProgressPhase::Pulling,
        )
        .expect("progress event");
        assert_eq!(item.id, "pulling:sha256:abc");
        assert_eq!(item.parent_id.as_deref(), Some("pulling:Image nginx"));
        assert_eq!(item.status, DockerProjectProgressStatus::Working);
        assert_eq!(item.current_bytes, Some(512));
        assert_eq!(item.total_bytes, Some(1024));
        assert_eq!(item.percent, Some(50));
    }

    #[test]
    fn ignores_malformed_or_identity_less_progress() {
        assert!(parse_progress_event("not-json", DockerProjectProgressPhase::Applying).is_none());
        assert!(
            parse_progress_event(
                r#"{"status":"Working","text":"Pulling"}"#,
                DockerProjectProgressPhase::Pulling,
            )
            .is_none()
        );
    }

    #[test]
    fn updates_existing_item_without_cross_phase_collision() {
        let mut items = Vec::new();
        let pulling = parse_progress_event(
            r#"{"id":"Image nginx","status":"Working","text":"Pulling","percent":10}"#,
            DockerProjectProgressPhase::Pulling,
        )
        .expect("pull event");
        let applying = parse_progress_event(
            r#"{"id":"Image nginx","status":"Done","text":"Pulled","percent":100}"#,
            DockerProjectProgressPhase::Applying,
        )
        .expect("apply event");
        assert!(upsert_progress_item(&mut items, pulling.clone()));
        let mut updated = pulling;
        updated.percent = Some(80);
        assert!(upsert_progress_item(&mut items, updated));
        assert!(upsert_progress_item(&mut items, applying));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].percent, Some(80));
    }

    #[test]
    fn aggregates_leaf_byte_progress_into_stage_range() {
        let items = vec![
            DockerProjectTaskProgressItem {
                id: "pulling:image".into(),
                parent_id: None,
                phase: DockerProjectProgressPhase::Pulling,
                status: DockerProjectProgressStatus::Working,
                label: "Image nginx".into(),
                action: "Pulling".into(),
                details: None,
                current_bytes: None,
                total_bytes: None,
                percent: None,
            },
            DockerProjectTaskProgressItem {
                id: "pulling:layer".into(),
                parent_id: Some("pulling:image".into()),
                phase: DockerProjectProgressPhase::Pulling,
                status: DockerProjectProgressStatus::Working,
                label: "layer".into(),
                action: "Downloading".into(),
                details: None,
                current_bytes: Some(50),
                total_bytes: Some(100),
                percent: Some(50),
            },
        ];
        assert_eq!(
            aggregate_progress_percent(&items, DockerProjectProgressPhase::Pulling),
            39
        );
    }

    #[test]
    fn text_fallback_is_bounded_and_sanitized() {
        let mut items = Vec::new();
        assert!(upsert_text_progress_item(
            &mut items,
            DockerProjectProgressPhase::Applying,
            1,
            "token=secret Creating container"
        ));
        assert_eq!(items.len(), 1);
        assert!(!items[0].label.contains("secret"));
        assert_eq!(items[0].percent, None);
    }

    #[tokio::test]
    async fn persists_and_restores_progress_snapshot() {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query(
            "INSERT INTO docker_compose_project_tasks (\
                id, project_name, operation, status, stage, progress_percent, pull_images, \
                actor_kind, actor_name\
             ) VALUES ('task-progress', 'demo', 'redeploy', 'running', 'pulling', 25, 1, \
                'system', 'test')",
        )
        .execute(&pool)
        .await
        .expect("insert task");
        let item = parse_progress_event(
            r#"{"id":"Image nginx","status":"Working","text":"Pulling","percent":50}"#,
            DockerProjectProgressPhase::Pulling,
        )
        .expect("progress event");

        persist_progress(
            &pool,
            "task-progress",
            DockerProjectProgressMode::Structured,
            &[item],
            DockerProjectProgressPhase::Pulling,
        )
        .await
        .expect("persist progress");

        let task = get(&pool, "task-progress").await.expect("load task");
        assert_eq!(task.progress_mode, DockerProjectProgressMode::Structured);
        assert_eq!(task.progress_items.len(), 1);
        assert_eq!(task.progress_items[0].label, "Image nginx");
        assert_eq!(task.progress_percent, 39);
    }

    #[tokio::test]
    async fn external_controller_progress_is_persisted_and_published() {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query(
            "INSERT INTO docker_compose_project_tasks (\
                id, project_name, operation, status, stage, progress_percent, pull_images, \
                actor_kind, actor_name\
             ) VALUES ('controller-progress', 'demo', 'create', 'running', 'pulling', 25, 0, \
                'system', 'test')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (sender, _) = broadcast::channel(TASK_EVENT_BUFFER);
        TASK_EVENT_HUBS.lock().await.insert(
            "controller-progress".to_string(),
            DockerProjectTaskEventHub {
                sender,
                latest_progress: Vec::new(),
            },
        );
        let item = DockerProjectTaskProgressItem {
            id: "pulling:image:nginx".to_string(),
            parent_id: None,
            phase: DockerProjectProgressPhase::Pulling,
            status: DockerProjectProgressStatus::Working,
            label: "Image nginx:latest".to_string(),
            action: "Transferring controller image".to_string(),
            details: None,
            current_bytes: None,
            total_bytes: None,
            percent: Some(42),
        };

        record_progress_item(&pool, "controller-progress", item.clone())
            .await
            .unwrap();

        let restored = get(&pool, "controller-progress").await.unwrap();
        assert_eq!(
            restored.progress_mode,
            DockerProjectProgressMode::Structured
        );
        assert_eq!(restored.progress_items, vec![item.clone()]);
        let hubs = TASK_EVENT_HUBS.lock().await;
        assert_eq!(
            hubs.get("controller-progress")
                .unwrap()
                .latest_progress
                .last()
                .unwrap()
                .item,
            item
        );
    }

    #[tokio::test]
    async fn startup_records_interrupted_compose_task_terminal_event() {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query(
            "INSERT INTO docker_compose_project_tasks (\
                id, project_name, operation, status, stage, progress_percent, pull_images, \
                actor_kind, actor_name\
             ) VALUES ('task-interrupted', 'demo', 'redeploy', 'running', 'applying', 70, 1, \
                'system', 'test')",
        )
        .execute(&pool)
        .await
        .expect("insert task");

        initialize(&pool).await.expect("recover tasks");

        let task = get(&pool, "task-interrupted").await.expect("load task");
        assert_eq!(task.status, DockerProjectTaskStatus::Failed);
        assert_eq!(task.stage, DockerProjectTaskStage::Interrupted);
        let events = crate::services::operation_outbox::pending(&pool, 10)
            .await
            .expect("load outbox");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_code, "docker_compose_project_redeploy");
        assert_eq!(
            events[0].outcome,
            vust_contracts::logging::OperationOutcome::Failure
        );
        assert_eq!(events[0].task_id.as_deref(), Some("task-interrupted"));
    }

    #[tokio::test]
    async fn exposes_working_layer_snapshot_before_pull_finishes() {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query(
            "INSERT INTO docker_compose_project_tasks (\
                id, project_name, operation, status, stage, progress_percent, pull_images, \
                actor_kind, actor_name\
             ) VALUES ('task-working-layer', 'demo', 'redeploy', 'running', 'pulling', 25, 1, \
                'system', 'test')",
        )
        .execute(&pool)
        .await
        .expect("insert task");
        let parent = parse_progress_event(
            r#"{"id":"Image nginx","status":"Working","text":"Pulling"}"#,
            DockerProjectProgressPhase::Pulling,
        )
        .expect("parent event");
        let layer = parse_progress_event(
            r#"{"id":"sha256:abc","parent_id":"Image nginx","status":"Working","text":"Downloading","current":25,"total":100,"percent":25}"#,
            DockerProjectProgressPhase::Pulling,
        )
        .expect("layer event");

        persist_progress(
            &pool,
            "task-working-layer",
            DockerProjectProgressMode::Structured,
            &[parent, layer],
            DockerProjectProgressPhase::Pulling,
        )
        .await
        .expect("persist progress");

        let task = get(&pool, "task-working-layer").await.expect("load task");
        assert_eq!(task.status, DockerProjectTaskStatus::Running);
        assert_eq!(task.progress_items.len(), 2);
        assert_eq!(
            task.progress_items[1].parent_id.as_deref(),
            Some("pulling:Image nginx")
        );
        assert_eq!(
            task.progress_items[1].status,
            DockerProjectProgressStatus::Working
        );
        assert_eq!(task.progress_items[1].percent, Some(25));
    }

    #[tokio::test]
    async fn broadcasts_working_layer_before_database_flush() {
        let task_id = "task-live-layer";
        let (sender, _) = broadcast::channel(TASK_EVENT_BUFFER);
        TASK_EVENT_HUBS.lock().await.insert(
            task_id.to_string(),
            DockerProjectTaskEventHub {
                sender,
                latest_progress: Vec::new(),
            },
        );
        let (mut receiver, replay) = subscribe(task_id).await.expect("subscribe task");
        assert!(replay.is_empty());
        let item = parse_progress_event(
            r#"{"id":"sha256:abc","parent_id":"Image nginx","status":"Working","text":"Downloading","current":25,"total":100,"percent":25}"#,
            DockerProjectProgressPhase::Pulling,
        )
        .expect("layer event");

        publish_progress(
            task_id,
            DockerProjectTaskProgressUpdate {
                progress_mode: DockerProjectProgressMode::Structured,
                progress_percent: 32,
                item: item.clone(),
            },
        )
        .await;

        let event = receiver.recv().await.expect("live progress");
        let DockerProjectTaskEvent::Progress(update) = event else {
            panic!("expected progress event");
        };
        assert_eq!(update.item, item);
        assert_eq!(update.progress_percent, 32);
        let (_, replay) = subscribe(task_id).await.expect("resubscribe task");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].item.status, DockerProjectProgressStatus::Working);
        TASK_EVENT_HUBS.lock().await.remove(task_id);
    }

    #[test]
    fn sanitizes_and_limits_command_errors() {
        let source = format!("  token=secret password:guess\n{}", "x".repeat(2000));
        let result = sanitize_error(&source);
        assert!(!result.contains('\n'));
        assert!(!result.contains("secret"));
        assert!(!result.contains("guess"));
        assert_eq!(result.chars().count(), 1000);
    }
}
