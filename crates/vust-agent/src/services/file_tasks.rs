//! 文件后台任务：持久化、幂等、路径互斥、取消和可回滚执行。

use crate::{
    services::{
        file_path_coordinator,
        files::{map_io_error, normalize_absolute_path, revision},
    },
    state::DbPool,
    types::{ApiError, ApiResult},
};
use axum::http::StatusCode;
use chrono::{DateTime, SecondsFormat, Utc};
use once_cell::sync::Lazy;
use sqlx::{Row, Sqlite, Transaction};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vust_contracts::{
    api::ErrorCode,
    files::{
        CreateFileOperationTaskRequest, FileOperation, FileOperationItemResult, FileOperationTask,
        FileTaskItemStatus, FileTaskStage, FileTaskStatus,
    },
};

const MAX_TASK_ITEMS: usize = 500;
const MAX_ERROR_CHARS: usize = 512;

static CREATE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static CANCELLATIONS: Lazy<Mutex<HashMap<String, CancellationToken>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 后台任务保存的可信操作者上下文。
#[derive(Debug, Clone)]
pub struct FileTaskActor {
    pub actor_name: String,
    pub client_ip: Option<String>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone)]
struct TaskItem {
    ordinal: i64,
    source: PathBuf,
    expected_revision: Option<String>,
    target: Option<PathBuf>,
    planned_bytes: u64,
}

#[derive(Debug)]
struct TaskSpec {
    operation: FileOperation,
    recursive: bool,
    items: Vec<TaskItem>,
}

/// Agent 启动时关闭不可恢复的破坏性任务。
pub async fn initialize(pool: &DbPool) -> ApiResult<()> {
    sqlx::query(
        "UPDATE file_operation_tasks SET status = 'failed', stage = 'interrupted', \
         progress_percent = 100, error_summary = 'Agent restarted before the task completed', \
         finished_at = unixepoch() WHERE status IN ('queued', 'running', 'cancelling')",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// 校验并创建幂等后台任务，然后立即调度执行。
pub async fn create(
    pool: &DbPool,
    node_id: &str,
    actor: FileTaskActor,
    request: CreateFileOperationTaskRequest,
) -> ApiResult<FileOperationTask> {
    validate_request(&request)?;
    let _guard = CREATE_LOCK.lock().await;
    if let Some(existing) = get_by_idempotency_key(pool, &request.idempotency_key).await? {
        return Ok(existing);
    }

    let target_directory = request
        .target_directory
        .as_deref()
        .map(normalize_absolute_path)
        .transpose()?;
    let mut normalized = Vec::with_capacity(request.items.len());
    for (ordinal, item) in request.items.iter().enumerate() {
        let source = normalize_absolute_path(&item.path)?;
        let target = item
            .target_path
            .as_deref()
            .map(normalize_absolute_path)
            .transpose()?
            .or_else(|| {
                target_directory.as_ref().map(|directory| {
                    directory.join(source.file_name().unwrap_or(source.as_os_str()))
                })
            });
        if target
            .as_ref()
            .is_some_and(|target| target == &source || target.starts_with(&source))
        {
            return Err(ApiError::conflict(
                ErrorCode::FileOperationConflict,
                "target path conflicts with the source path",
            ));
        }
        normalized.push(TaskItem {
            ordinal: ordinal as i64,
            source,
            expected_revision: item.expected_revision.clone(),
            target,
            planned_bytes: 0,
        });
    }
    ensure_no_internal_overlap(&normalized)?;
    let reservation_paths = normalized
        .iter()
        .flat_map(|item| std::iter::once(item.source.clone()).chain(item.target.clone()))
        .collect();
    let _path_reservation = file_path_coordinator::reserve(pool, reservation_paths, None).await?;
    ensure_no_active_overlap(pool, &normalized).await?;

    let task_id = Uuid::now_v7().to_string();
    let mut transaction = pool.begin().await?;
    insert_task(
        &mut transaction,
        &task_id,
        node_id,
        &actor,
        &request,
        target_directory.as_deref(),
    )
    .await?;
    for item in &normalized {
        sqlx::query(
            "INSERT INTO file_operation_task_items \
             (task_id, ordinal, source_path, expected_revision, target_path) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&task_id)
        .bind(item.ordinal)
        .bind(item.source.to_string_lossy().as_ref())
        .bind(&item.expected_revision)
        .bind(
            item.target
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        )
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;

    let cancellation = CancellationToken::new();
    CANCELLATIONS
        .lock()
        .await
        .insert(task_id.clone(), cancellation.clone());
    let task_pool = pool.clone();
    let task_id_for_run = task_id.clone();
    tokio::spawn(async move {
        if let Err(error) = run(task_pool.clone(), &task_id_for_run, cancellation).await {
            tracing::error!(task_id = task_id_for_run, %error, "file task runner failed");
            let _ = fail_task(&task_pool, &task_id_for_run, &error.to_string(), None).await;
        }
        CANCELLATIONS.lock().await.remove(&task_id_for_run);
    });
    get(pool, &task_id).await
}

/// 查询单个任务。
pub async fn get(pool: &DbPool, task_id: &str) -> ApiResult<FileOperationTask> {
    let row = sqlx::query(
        "SELECT id, node_id, operation, status, stage, progress_percent, total_item_count, \
         completed_item_count, failed_item_count, total_bytes, processed_bytes, error_summary, \
         cleanup_warning, created_at, started_at, finished_at \
         FROM file_operation_tasks WHERE id = ?1",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(ErrorCode::FileTaskNotFound, "file operation task not found")
    })?;
    task_from_row(pool, &row).await
}

/// 返回当前仍需页面跟踪的任务。
pub async fn active(pool: &DbPool) -> ApiResult<Vec<FileOperationTask>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM file_operation_tasks WHERE status IN ('queued', 'running', 'cancelling') \
         ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    let mut tasks = Vec::with_capacity(ids.len());
    for id in ids {
        tasks.push(get(pool, &id).await?);
    }
    Ok(tasks)
}

/// 请求取消尚未进入清理提交阶段的任务。
pub async fn cancel(pool: &DbPool, task_id: &str) -> ApiResult<FileOperationTask> {
    let task = get(pool, task_id).await?;
    if matches!(
        task.status,
        FileTaskStatus::Succeeded | FileTaskStatus::Failed | FileTaskStatus::Cancelled
    ) || matches!(
        task.stage,
        FileTaskStage::CleaningUp | FileTaskStage::Completed
    ) {
        return Err(ApiError::conflict(
            ErrorCode::FileTaskNotCancellable,
            "file operation task can no longer be cancelled",
        ));
    }
    sqlx::query(
        "UPDATE file_operation_tasks SET status = 'cancelling', stage = 'rollingBack' WHERE id = ?1",
    )
    .bind(task_id)
    .execute(pool)
    .await?;
    if let Some(cancellation) = CANCELLATIONS.lock().await.get(task_id).cloned() {
        cancellation.cancel();
    }
    get(pool, task_id).await
}

async fn run(pool: DbPool, task_id: &str, cancellation: CancellationToken) -> ApiResult<()> {
    let mut spec = load_spec(&pool, task_id).await?;
    set_running(&pool, task_id, FileTaskStage::Validating).await?;
    preflight(&pool, task_id, &mut spec).await?;
    if cancellation.is_cancelled() {
        return cancel_task(&pool, task_id, None).await;
    }

    if spec.operation == FileOperation::Remove {
        return run_remove(&pool, task_id, &spec, &cancellation).await;
    }

    let stage = if spec.operation == FileOperation::Copy {
        FileTaskStage::Copying
    } else {
        FileTaskStage::Moving
    };
    set_running(&pool, task_id, stage).await?;
    let mut committed = Vec::<(PathBuf, PathBuf, FileOperation)>::new();
    for item in &spec.items {
        if cancellation.is_cancelled() {
            let warning = rollback_committed(&committed).await;
            return cancel_task(&pool, task_id, warning.as_deref()).await;
        }
        mark_item_running(&pool, task_id, item.ordinal).await?;
        let source = &item.source;
        let target = item.target.as_ref().expect("validated target");
        let result = if spec.operation == FileOperation::Copy {
            copy_staged(&pool, source, target, task_id, item.ordinal, &cancellation).await
        } else {
            move_path(&pool, source, target, task_id, item.ordinal, &cancellation).await
        };
        match result {
            Ok(bytes_already_reported) => {
                committed.push((source.clone(), target.clone(), spec.operation));
                mark_item_succeeded(
                    &pool,
                    task_id,
                    item.ordinal,
                    if bytes_already_reported {
                        0
                    } else {
                        item.planned_bytes
                    },
                )
                .await?;
            }
            Err(error) => {
                mark_item_failed(
                    &pool,
                    task_id,
                    item.ordinal,
                    error.code.as_str(),
                    &error.message,
                )
                .await?;
                let warning = rollback_committed(&committed).await;
                if cancellation.is_cancelled() {
                    return cancel_task(&pool, task_id, warning.as_deref()).await;
                }
                return fail_task(&pool, task_id, &error.message, warning.as_deref()).await;
            }
        }
    }
    succeed_task(&pool, task_id, None).await
}

async fn run_remove(
    pool: &DbPool,
    task_id: &str,
    spec: &TaskSpec,
    cancellation: &CancellationToken,
) -> ApiResult<()> {
    set_running(pool, task_id, FileTaskStage::Deleting).await?;
    let mut tombstones = Vec::new();
    for item in &spec.items {
        if cancellation.is_cancelled() {
            let warning = rollback_tombstones(&tombstones).await;
            return cancel_task(pool, task_id, warning.as_deref()).await;
        }
        mark_item_running(pool, task_id, item.ordinal).await?;
        let parent = item.source.parent().unwrap_or(Path::new("/"));
        let tombstone = parent.join(format!(".vust-delete-{task_id}-{}", item.ordinal));
        if let Err(error) = tokio::fs::rename(&item.source, &tombstone).await {
            let error = map_io_error(error, "stage file deletion");
            mark_item_failed(
                pool,
                task_id,
                item.ordinal,
                error.code.as_str(),
                &error.message,
            )
            .await?;
            let warning = rollback_tombstones(&tombstones).await;
            return fail_task(pool, task_id, &error.message, warning.as_deref()).await;
        }
        tombstones.push((item.source.clone(), tombstone, item.ordinal));
    }

    set_running(pool, task_id, FileTaskStage::CleaningUp).await?;
    let mut cleanup_warning = None;
    for (_, tombstone, ordinal) in &tombstones {
        let result = remove_entry(tombstone).await;
        match result {
            Ok(()) => {
                let planned_bytes = spec.items[*ordinal as usize].planned_bytes;
                mark_item_succeeded(pool, task_id, *ordinal, planned_bytes).await?;
            }
            Err(error) => {
                cleanup_warning = Some(sanitize_error(&error.message));
                let planned_bytes = spec.items[*ordinal as usize].planned_bytes;
                mark_item_succeeded(pool, task_id, *ordinal, planned_bytes).await?;
            }
        }
    }
    succeed_task(pool, task_id, cleanup_warning.as_deref()).await
}

async fn preflight(pool: &DbPool, task_id: &str, spec: &mut TaskSpec) -> ApiResult<()> {
    let mut total_bytes = 0_u64;
    for item in &mut spec.items {
        let planned_bytes = match preflight_item(spec.operation, spec.recursive, item).await {
            Ok(planned_bytes) => planned_bytes,
            Err(error) => {
                mark_item_failed(
                    pool,
                    task_id,
                    item.ordinal,
                    error.code.as_str(),
                    &error.message,
                )
                .await?;
                return Err(error);
            }
        };
        item.planned_bytes = planned_bytes;
        total_bytes = total_bytes.saturating_add(planned_bytes);
        sqlx::query(
            "UPDATE file_operation_task_items SET planned_bytes = ?3 \
             WHERE task_id = ?1 AND ordinal = ?2",
        )
        .bind(task_id)
        .bind(item.ordinal)
        .bind(planned_bytes as i64)
        .execute(pool)
        .await?;
    }
    sqlx::query("UPDATE file_operation_tasks SET total_bytes = ?2 WHERE id = ?1")
        .bind(task_id)
        .bind(total_bytes as i64)
        .execute(pool)
        .await?;
    Ok(())
}

async fn preflight_item(
    operation: FileOperation,
    recursive: bool,
    item: &TaskItem,
) -> ApiResult<u64> {
    let metadata = tokio::fs::symlink_metadata(&item.source)
        .await
        .map_err(|error| map_io_error(error, "validate task source"))?;
    if item
        .expected_revision
        .as_ref()
        .is_some_and(|value| value != &revision(&metadata))
    {
        return Err(ApiError::conflict(
            ErrorCode::FileChanged,
            "task source changed since it was selected",
        ));
    }
    if operation == FileOperation::Remove && metadata.is_dir() && !recursive {
        let mut reader = tokio::fs::read_dir(&item.source)
            .await
            .map_err(|error| map_io_error(error, "inspect directory"))?;
        if reader
            .next_entry()
            .await
            .map_err(|error| map_io_error(error, "inspect directory"))?
            .is_some()
        {
            return Err(ApiError::conflict(
                ErrorCode::FileOperationConflict,
                "non-empty directory requires recursive removal",
            ));
        }
    }
    if let Some(target) = &item.target {
        if tokio::fs::symlink_metadata(target).await.is_ok() {
            return Err(ApiError::conflict(
                ErrorCode::FileAlreadyExists,
                "task target already exists",
            ));
        }
        let parent = target
            .parent()
            .ok_or_else(|| ApiError::validation("target has no parent"))?;
        if !tokio::fs::metadata(parent)
            .await
            .map(|value| value.is_dir())
            .unwrap_or(false)
        {
            return Err(ApiError::not_found(
                ErrorCode::FileNotFound,
                "task target directory does not exist",
            ));
        }
    }
    scan_entry_bytes(&item.source).await
}

/// 扫描目录的真实普通文件字节数；不跟随符号链接。
async fn scan_entry_bytes(path: &Path) -> ApiResult<u64> {
    let mut total = 0_u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(current) = pending.pop() {
        let metadata = tokio::fs::symlink_metadata(&current)
            .await
            .map_err(|error| map_io_error(error, "scan task source"))?;
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            let mut reader = tokio::fs::read_dir(&current)
                .await
                .map_err(|error| map_io_error(error, "scan task directory"))?;
            while let Some(entry) = reader
                .next_entry()
                .await
                .map_err(|error| map_io_error(error, "scan task directory"))?
            {
                pending.push(entry.path());
            }
        }
    }
    Ok(total)
}

async fn copy_staged(
    pool: &DbPool,
    source: &Path,
    target: &Path,
    task_id: &str,
    ordinal: i64,
    cancellation: &CancellationToken,
) -> ApiResult<bool> {
    let parent = target
        .parent()
        .ok_or_else(|| ApiError::validation("target has no parent"))?;
    let staged = parent.join(format!(".vust-copy-{task_id}-{ordinal}"));
    if let Err(error) = copy_entry(pool, task_id, source, &staged, cancellation).await {
        let _ = remove_entry(&staged).await;
        return Err(error);
    }
    if cancellation.is_cancelled() {
        let _ = remove_entry(&staged).await;
        return Err(ApiError::conflict(
            ErrorCode::FileTaskNotCancellable,
            "file task was cancelled before commit",
        ));
    }
    tokio::fs::rename(&staged, target)
        .await
        .map_err(|error| map_io_error(error, "commit copied path"))?;
    if let Err(error) = sync_directory(parent).await {
        let _ = remove_entry(target).await;
        return Err(error);
    }
    Ok(true)
}

async fn copy_entry(
    pool: &DbPool,
    task_id: &str,
    source: &Path,
    target: &Path,
    cancellation: &CancellationToken,
) -> ApiResult<()> {
    let metadata = tokio::fs::symlink_metadata(source)
        .await
        .map_err(|error| map_io_error(error, "read copy source"))?;
    if metadata.file_type().is_symlink() {
        let link = tokio::fs::read_link(source)
            .await
            .map_err(|error| map_io_error(error, "read symbolic link"))?;
        let target = target.to_path_buf();
        tokio::task::spawn_blocking(move || std::os::unix::fs::symlink(link, target))
            .await
            .map_err(|_| ApiError::internal("symbolic link copy task failed"))?
            .map_err(|error| map_io_error(error, "copy symbolic link"))?;
        return Ok(());
    }
    if metadata.is_file() {
        copy_regular_file(pool, task_id, source, target, cancellation).await?;
        tokio::fs::set_permissions(target, metadata.permissions())
            .await
            .map_err(|error| map_io_error(error, "preserve copied file permissions"))?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::FileTypeUnsupported,
            "special files cannot be copied",
        ));
    }
    tokio::fs::create_dir(target)
        .await
        .map_err(|error| map_io_error(error, "create copied directory"))?;
    let mut pending = vec![(source.to_path_buf(), target.to_path_buf())];
    let mut directory_permissions = vec![(target.to_path_buf(), metadata.permissions())];
    while let Some((source_dir, target_dir)) = pending.pop() {
        let mut reader = tokio::fs::read_dir(&source_dir)
            .await
            .map_err(|error| map_io_error(error, "read copied directory"))?;
        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|error| map_io_error(error, "read copied directory"))?
        {
            if cancellation.is_cancelled() {
                return Err(ApiError::conflict(
                    ErrorCode::FileTaskNotCancellable,
                    "file task was cancelled",
                ));
            }
            let child_source = entry.path();
            let child_target = target_dir.join(entry.file_name());
            let child_metadata = tokio::fs::symlink_metadata(&child_source)
                .await
                .map_err(|error| map_io_error(error, "read copied entry"))?;
            if child_metadata.is_dir() {
                tokio::fs::create_dir(&child_target)
                    .await
                    .map_err(|error| map_io_error(error, "create copied directory"))?;
                directory_permissions.push((child_target.clone(), child_metadata.permissions()));
                pending.push((child_source, child_target));
            } else if child_metadata.is_file() {
                copy_regular_file(pool, task_id, &child_source, &child_target, cancellation)
                    .await?;
                tokio::fs::set_permissions(&child_target, child_metadata.permissions())
                    .await
                    .map_err(|error| map_io_error(error, "preserve copied file permissions"))?;
            } else if child_metadata.file_type().is_symlink() {
                let link = tokio::fs::read_link(&child_source)
                    .await
                    .map_err(|error| map_io_error(error, "read symbolic link"))?;
                tokio::task::spawn_blocking(move || std::os::unix::fs::symlink(link, child_target))
                    .await
                    .map_err(|_| ApiError::internal("symbolic link copy task failed"))?
                    .map_err(|error| map_io_error(error, "copy symbolic link"))?;
            } else {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    ErrorCode::FileTypeUnsupported,
                    "special files cannot be copied",
                ));
            }
        }
    }
    for (directory, permissions) in directory_permissions.into_iter().rev() {
        tokio::fs::set_permissions(directory, permissions)
            .await
            .map_err(|error| map_io_error(error, "preserve copied directory permissions"))?;
    }
    Ok(())
}

async fn copy_regular_file(
    pool: &DbPool,
    task_id: &str,
    source: &Path,
    target: &Path,
    cancellation: &CancellationToken,
) -> ApiResult<()> {
    let mut source = tokio::fs::File::open(source)
        .await
        .map_err(|error| map_io_error(error, "open copy source"))?;
    let mut target = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .await
        .map_err(|error| map_io_error(error, "create copy target"))?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancellation.is_cancelled() {
            return Err(ApiError::conflict(
                ErrorCode::FileTaskNotCancellable,
                "file task was cancelled",
            ));
        }
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|error| map_io_error(error, "read copy source"))?;
        if read == 0 {
            break;
        }
        target
            .write_all(&buffer[..read])
            .await
            .map_err(|error| map_io_error(error, "write copy target"))?;
        add_processed_bytes(pool, task_id, read as u64).await?;
    }
    target
        .sync_all()
        .await
        .map_err(|error| map_io_error(error, "sync copy target"))
}

async fn move_path(
    pool: &DbPool,
    source: &Path,
    target: &Path,
    task_id: &str,
    ordinal: i64,
    cancellation: &CancellationToken,
) -> ApiResult<bool> {
    match tokio::fs::rename(source, target).await {
        Ok(()) => {
            let target_parent = target.parent().unwrap_or(Path::new("/"));
            if let Err(error) = sync_directory(target_parent).await {
                let _ = tokio::fs::rename(target, source).await;
                return Err(error);
            }
            if source.parent() != Some(target_parent)
                && let Some(source_parent) = source.parent()
                && let Err(error) = sync_directory(source_parent).await
            {
                let _ = tokio::fs::rename(target, source).await;
                return Err(error);
            }
            Ok(false)
        }
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            copy_staged(pool, source, target, task_id, ordinal, cancellation).await?;
            if let Err(error) = remove_entry(source).await {
                let _ = remove_entry(target).await;
                return Err(error);
            }
            Ok(true)
        }
        Err(error) => Err(map_io_error(error, "move path")),
    }
}

async fn sync_directory(path: &Path) -> ApiResult<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::File::open(path)?.sync_all())
        .await
        .map_err(|_| ApiError::internal("file task directory sync failed"))?
        .map_err(|error| map_io_error(error, "sync file task directory"))
}

async fn add_processed_bytes(pool: &DbPool, task_id: &str, bytes: u64) -> ApiResult<()> {
    sqlx::query(
        "UPDATE file_operation_tasks SET \
         processed_bytes = MIN(total_bytes, processed_bytes + ?2), \
         progress_percent = MIN(99, CASE WHEN total_bytes > 0 \
           THEN ((processed_bytes + ?2) * 100) / total_bytes ELSE progress_percent END) \
         WHERE id = ?1",
    )
    .bind(task_id)
    .bind(bytes as i64)
    .execute(pool)
    .await?;
    Ok(())
}

async fn remove_entry(path: &Path) -> ApiResult<()> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(map_io_error(error, "read removal target")),
    };
    if metadata.is_dir() {
        tokio::fs::remove_dir_all(path).await
    } else {
        tokio::fs::remove_file(path).await
    }
    .map_err(|error| map_io_error(error, "remove path"))
}

async fn rollback_committed(committed: &[(PathBuf, PathBuf, FileOperation)]) -> Option<String> {
    let mut warning = None;
    for (source, target, operation) in committed.iter().rev() {
        let result = if *operation == FileOperation::Copy {
            remove_entry(target).await
        } else {
            tokio::fs::rename(target, source)
                .await
                .map_err(|error| map_io_error(error, "rollback moved path"))
        };
        if let Err(error) = result {
            warning = Some(sanitize_error(&error.message));
        }
    }
    warning
}

async fn rollback_tombstones(tombstones: &[(PathBuf, PathBuf, i64)]) -> Option<String> {
    let mut warning = None;
    for (source, tombstone, _) in tombstones.iter().rev() {
        if let Err(error) = tokio::fs::rename(tombstone, source).await {
            warning = Some(sanitize_error(error.to_string()));
        }
    }
    warning
}

fn validate_request(request: &CreateFileOperationTaskRequest) -> ApiResult<()> {
    if request.items.is_empty() || request.items.len() > MAX_TASK_ITEMS {
        return Err(ApiError::validation(
            "file task must contain 1 to 500 items",
        ));
    }
    if request.idempotency_key.trim().len() < 8 || request.idempotency_key.len() > 128 {
        return Err(ApiError::validation("invalid file task idempotency key"));
    }
    if request.overwrite {
        return Err(ApiError::validation("file task overwrite is not supported"));
    }
    match request.operation {
        FileOperation::Copy | FileOperation::Move
            if request.target_directory.is_none()
                && request.items.iter().any(|item| item.target_path.is_none()) =>
        {
            Err(ApiError::validation(
                "copy and move task items require targetPath or targetDirectory",
            ))
        }
        FileOperation::Remove if request.target_directory.is_some() => Err(ApiError::validation(
            "remove tasks do not accept targetDirectory",
        )),
        _ => Ok(()),
    }
}

async fn ensure_no_active_overlap(pool: &DbPool, items: &[TaskItem]) -> ApiResult<()> {
    let rows = sqlx::query(
        "SELECT i.source_path, i.target_path FROM file_operation_task_items i \
         JOIN file_operation_tasks t ON t.id = i.task_id \
         WHERE t.status IN ('queued', 'running', 'cancelling')",
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let existing_source = PathBuf::from(row.try_get::<String, _>("source_path")?);
        let existing_target = row
            .try_get::<Option<String>, _>("target_path")?
            .map(PathBuf::from);
        for item in items {
            let candidates = [Some(&item.source), item.target.as_ref()];
            for candidate in candidates.into_iter().flatten() {
                if paths_overlap(candidate, &existing_source)
                    || existing_target
                        .as_ref()
                        .is_some_and(|path| paths_overlap(candidate, path))
                {
                    return Err(ApiError::conflict(
                        ErrorCode::FileOperationConflict,
                        "another file task already owns an overlapping path",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn ensure_no_internal_overlap(items: &[TaskItem]) -> ApiResult<()> {
    for (index, item) in items.iter().enumerate() {
        for other in &items[index + 1..] {
            for left in [Some(&item.source), item.target.as_ref()]
                .into_iter()
                .flatten()
            {
                for right in [Some(&other.source), other.target.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    if paths_overlap(left, right) {
                        return Err(ApiError::conflict(
                            ErrorCode::FileOperationConflict,
                            "file task contains overlapping source or target paths",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

async fn insert_task(
    transaction: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    node_id: &str,
    actor: &FileTaskActor,
    request: &CreateFileOperationTaskRequest,
    target_directory: Option<&Path>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO file_operation_tasks \
         (id, node_id, idempotency_key, operation, status, stage, total_item_count, \
          target_directory, recursive, actor_name, client_ip, trace_id) \
         VALUES (?1, ?2, ?3, ?4, 'queued', 'validating', ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(task_id)
    .bind(node_id)
    .bind(request.idempotency_key.trim())
    .bind(operation_text(request.operation))
    .bind(request.items.len() as i64)
    .bind(target_directory.map(|path| path.to_string_lossy().into_owned()))
    .bind(request.recursive)
    .bind(&actor.actor_name)
    .bind(&actor.client_ip)
    .bind(&actor.trace_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn get_by_idempotency_key(pool: &DbPool, key: &str) -> ApiResult<Option<FileOperationTask>> {
    let id: Option<String> =
        sqlx::query_scalar("SELECT id FROM file_operation_tasks WHERE idempotency_key = ?1")
            .bind(key.trim())
            .fetch_optional(pool)
            .await?;
    match id {
        Some(id) => Ok(Some(get(pool, &id).await?)),
        None => Ok(None),
    }
}

async fn load_spec(pool: &DbPool, task_id: &str) -> ApiResult<TaskSpec> {
    let row = sqlx::query("SELECT operation, recursive FROM file_operation_tasks WHERE id = ?1")
        .bind(task_id)
        .fetch_one(pool)
        .await?;
    let operation = parse_operation(row.try_get("operation")?)?;
    let recursive = row.try_get("recursive")?;
    let rows = sqlx::query(
        "SELECT ordinal, source_path, expected_revision, target_path, planned_bytes \
         FROM file_operation_task_items WHERE task_id = ?1 ORDER BY ordinal",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;
    let items = rows
        .into_iter()
        .map(|row| {
            Ok(TaskItem {
                ordinal: row.try_get("ordinal")?,
                source: PathBuf::from(row.try_get::<String, _>("source_path")?),
                expected_revision: row.try_get("expected_revision")?,
                target: row
                    .try_get::<Option<String>, _>("target_path")?
                    .map(PathBuf::from),
                planned_bytes: row.try_get::<i64, _>("planned_bytes")? as u64,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(TaskSpec {
        operation,
        recursive,
        items,
    })
}

async fn set_running(pool: &DbPool, task_id: &str, stage: FileTaskStage) -> ApiResult<()> {
    sqlx::query(
        "UPDATE file_operation_tasks SET status = 'running', stage = ?2, \
         started_at = COALESCE(started_at, unixepoch()) WHERE id = ?1",
    )
    .bind(task_id)
    .bind(stage_text(stage))
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_item_running(pool: &DbPool, task_id: &str, ordinal: i64) -> ApiResult<()> {
    sqlx::query("UPDATE file_operation_task_items SET status = 'running' WHERE task_id = ?1 AND ordinal = ?2")
        .bind(task_id).bind(ordinal).execute(pool).await?;
    Ok(())
}

async fn mark_item_succeeded(
    pool: &DbPool,
    task_id: &str,
    ordinal: i64,
    processed_bytes: u64,
) -> ApiResult<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query("UPDATE file_operation_task_items SET status = 'succeeded' WHERE task_id = ?1 AND ordinal = ?2")
        .bind(task_id).bind(ordinal).execute(&mut *transaction).await?;
    sqlx::query(
        "UPDATE file_operation_tasks SET completed_item_count = completed_item_count + 1, \
         processed_bytes = MIN(total_bytes, processed_bytes + ?2), \
         progress_percent = MIN(99, CASE WHEN total_bytes > 0 \
           THEN ((processed_bytes + ?2) * 100) / total_bytes \
           ELSE ((completed_item_count + 1) * 100) / total_item_count END) WHERE id = ?1",
    )
    .bind(task_id)
    .bind(processed_bytes as i64)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn mark_item_failed(
    pool: &DbPool,
    task_id: &str,
    ordinal: i64,
    code: &str,
    summary: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE file_operation_task_items SET status = 'failed', error_code = ?3, error_summary = ?4 \
         WHERE task_id = ?1 AND ordinal = ?2",
    )
    .bind(task_id).bind(ordinal).bind(code).bind(sanitize_error(summary)).execute(pool).await?;
    Ok(())
}

async fn succeed_task(pool: &DbPool, task_id: &str, warning: Option<&str>) -> ApiResult<()> {
    finish_task(pool, task_id, "succeeded", "completed", None, warning).await
}

async fn fail_task(
    pool: &DbPool,
    task_id: &str,
    error: &str,
    warning: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE file_operation_task_items SET status = 'cancelled' \
         WHERE task_id = ?1 AND status IN ('pending', 'running')",
    )
    .bind(task_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE file_operation_tasks SET failed_item_count = failed_item_count + 1 WHERE id = ?1",
    )
    .bind(task_id)
    .execute(pool)
    .await?;
    finish_task(pool, task_id, "failed", "failed", Some(error), warning).await
}

async fn cancel_task(pool: &DbPool, task_id: &str, warning: Option<&str>) -> ApiResult<()> {
    sqlx::query("UPDATE file_operation_task_items SET status = 'cancelled' WHERE task_id = ?1 AND status IN ('pending', 'running')")
        .bind(task_id).execute(pool).await?;
    finish_task(pool, task_id, "cancelled", "cancelled", None, warning).await
}

async fn finish_task(
    pool: &DbPool,
    task_id: &str,
    status: &str,
    stage: &str,
    error: Option<&str>,
    warning: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE file_operation_tasks SET status = ?2, stage = ?3, progress_percent = 100, \
         error_summary = ?4, cleanup_warning = ?5, finished_at = unixepoch() WHERE id = ?1",
    )
    .bind(task_id)
    .bind(status)
    .bind(stage)
    .bind(error.map(sanitize_error))
    .bind(warning.map(sanitize_error))
    .execute(pool)
    .await?;
    Ok(())
}

async fn task_from_row(
    pool: &DbPool,
    row: &sqlx::sqlite::SqliteRow,
) -> ApiResult<FileOperationTask> {
    let task_id: String = row.try_get("id")?;
    let item_rows = sqlx::query(
        "SELECT source_path, target_path, status, error_code, error_summary \
         FROM file_operation_task_items WHERE task_id = ?1 ORDER BY ordinal",
    )
    .bind(&task_id)
    .fetch_all(pool)
    .await?;
    let items = item_rows
        .into_iter()
        .map(|item| {
            let status: String = item.try_get("status").unwrap_or_default();
            FileOperationItemResult {
                path: item.try_get("source_path").unwrap_or_default(),
                target_path: item.try_get("target_path").ok().flatten(),
                status: parse_item_status(&status).unwrap_or(FileTaskItemStatus::Failed),
                error_code: item.try_get("error_code").ok().flatten(),
                error_summary: item.try_get("error_summary").ok().flatten(),
            }
        })
        .collect();
    Ok(FileOperationTask {
        task_id,
        node_id: row.try_get("node_id")?,
        operation: parse_operation(row.try_get("operation")?)?,
        status: parse_status(row.try_get("status")?)?,
        stage: parse_stage(row.try_get("stage")?)?,
        progress_percent: row.try_get::<i64, _>("progress_percent")? as u8,
        total_item_count: row.try_get::<i64, _>("total_item_count")? as u64,
        completed_item_count: row.try_get::<i64, _>("completed_item_count")? as u64,
        failed_item_count: row.try_get::<i64, _>("failed_item_count")? as u64,
        total_bytes: row.try_get::<i64, _>("total_bytes")? as u64,
        processed_bytes: row.try_get::<i64, _>("processed_bytes")? as u64,
        items,
        error_summary: row.try_get("error_summary")?,
        cleanup_warning: row.try_get("cleanup_warning")?,
        created_at: unix_timestamp(row.try_get("created_at")?),
        started_at: row
            .try_get::<Option<i64>, _>("started_at")?
            .map(unix_timestamp),
        finished_at: row
            .try_get::<Option<i64>, _>("finished_at")?
            .map(unix_timestamp),
    })
}

fn operation_text(value: FileOperation) -> &'static str {
    match value {
        FileOperation::Copy => "copy",
        FileOperation::Move => "move",
        FileOperation::Remove => "remove",
    }
}

fn parse_operation(value: String) -> ApiResult<FileOperation> {
    match value.as_str() {
        "copy" => Ok(FileOperation::Copy),
        "move" => Ok(FileOperation::Move),
        "remove" => Ok(FileOperation::Remove),
        _ => Err(ApiError::internal("invalid file task operation")),
    }
}

fn parse_status(value: String) -> ApiResult<FileTaskStatus> {
    match value.as_str() {
        "queued" => Ok(FileTaskStatus::Queued),
        "running" => Ok(FileTaskStatus::Running),
        "cancelling" => Ok(FileTaskStatus::Cancelling),
        "succeeded" => Ok(FileTaskStatus::Succeeded),
        "failed" => Ok(FileTaskStatus::Failed),
        "cancelled" => Ok(FileTaskStatus::Cancelled),
        _ => Err(ApiError::internal("invalid file task status")),
    }
}

fn parse_item_status(value: &str) -> ApiResult<FileTaskItemStatus> {
    match value {
        "pending" => Ok(FileTaskItemStatus::Pending),
        "running" => Ok(FileTaskItemStatus::Running),
        "succeeded" => Ok(FileTaskItemStatus::Succeeded),
        "failed" => Ok(FileTaskItemStatus::Failed),
        "cancelled" => Ok(FileTaskItemStatus::Cancelled),
        _ => Err(ApiError::internal("invalid file task item status")),
    }
}

fn stage_text(value: FileTaskStage) -> &'static str {
    match value {
        FileTaskStage::Validating => "validating",
        FileTaskStage::Preparing => "preparing",
        FileTaskStage::Copying => "copying",
        FileTaskStage::Moving => "moving",
        FileTaskStage::Deleting => "deleting",
        FileTaskStage::RollingBack => "rollingBack",
        FileTaskStage::CleaningUp => "cleaningUp",
        FileTaskStage::Completed => "completed",
        FileTaskStage::Failed => "failed",
        FileTaskStage::Cancelled => "cancelled",
        FileTaskStage::Interrupted => "interrupted",
    }
}

fn parse_stage(value: String) -> ApiResult<FileTaskStage> {
    match value.as_str() {
        "validating" => Ok(FileTaskStage::Validating),
        "preparing" => Ok(FileTaskStage::Preparing),
        "copying" => Ok(FileTaskStage::Copying),
        "moving" => Ok(FileTaskStage::Moving),
        "deleting" => Ok(FileTaskStage::Deleting),
        "rollingBack" => Ok(FileTaskStage::RollingBack),
        "cleaningUp" => Ok(FileTaskStage::CleaningUp),
        "completed" => Ok(FileTaskStage::Completed),
        "failed" => Ok(FileTaskStage::Failed),
        "cancelled" => Ok(FileTaskStage::Cancelled),
        "interrupted" => Ok(FileTaskStage::Interrupted),
        _ => Err(ApiError::internal("invalid file task stage")),
    }
}

fn unix_timestamp(value: i64) -> String {
    DateTime::<Utc>::from_timestamp(value, 0)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn sanitize_error(value: impl AsRef<str>) -> String {
    value.as_ref().chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vust_contracts::files::{CreateFileOperationTaskRequest, FileOperationItemRequest};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        async fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("vust-file-task-{label}-{}", Uuid::now_v7()));
            tokio::fs::create_dir(&path).await.unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn actor() -> FileTaskActor {
        FileTaskActor {
            actor_name: "admin".to_string(),
            client_ip: Some("127.0.0.1".to_string()),
            trace_id: Some("trace-test".to_string()),
        }
    }

    async fn wait_for_terminal(pool: &DbPool, task_id: &str) -> FileOperationTask {
        for _ in 0..500 {
            let task = get(pool, task_id).await.unwrap();
            if matches!(
                task.status,
                FileTaskStatus::Succeeded | FileTaskStatus::Failed | FileTaskStatus::Cancelled
            ) {
                return task;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("file task did not reach a terminal state");
    }

    #[test]
    fn overlapping_paths_include_descendants() {
        assert!(paths_overlap(Path::new("/tmp/a"), Path::new("/tmp/a/b")));
        assert!(!paths_overlap(Path::new("/tmp/a"), Path::new("/tmp/ab")));
        let items = vec![
            TaskItem {
                ordinal: 0,
                source: PathBuf::from("/tmp/a"),
                expected_revision: None,
                target: None,
                planned_bytes: 0,
            },
            TaskItem {
                ordinal: 1,
                source: PathBuf::from("/tmp/a/child"),
                expected_revision: None,
                target: None,
                planned_bytes: 0,
            },
        ];
        assert!(ensure_no_internal_overlap(&items).is_err());
    }

    #[tokio::test]
    async fn synchronous_path_reservation_blocks_task_submission() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("sync-reservation").await;
        let source = root.0.join("source.txt");
        tokio::fs::write(&source, b"content").await.unwrap();
        let _reservation = file_path_coordinator::reserve(&pool, vec![source.clone()], None)
            .await
            .unwrap();

        let error = create(
            &pool,
            "local",
            actor(),
            CreateFileOperationTaskRequest {
                operation: FileOperation::Remove,
                items: vec![FileOperationItemRequest {
                    path: source.to_string_lossy().into_owned(),
                    expected_revision: None,
                    target_path: None,
                }],
                target_directory: None,
                recursive: false,
                overwrite: false,
                idempotency_key: format!("reserved-{}", Uuid::now_v7()),
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::FileOperationConflict);
        assert!(source.exists());
    }

    #[tokio::test]
    async fn directory_copy_is_idempotent_and_reports_real_bytes() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("copy").await;
        let source = root.0.join("source");
        let target = root.0.join("target");
        tokio::fs::create_dir(&source).await.unwrap();
        tokio::fs::write(source.join("first.txt"), b"1234")
            .await
            .unwrap();
        tokio::fs::write(source.join("second.txt"), b"567")
            .await
            .unwrap();
        std::os::unix::fs::symlink("first.txt", source.join("first-link")).unwrap();
        let request = CreateFileOperationTaskRequest {
            operation: FileOperation::Copy,
            items: vec![FileOperationItemRequest {
                path: source.to_string_lossy().into_owned(),
                expected_revision: None,
                target_path: Some(target.to_string_lossy().into_owned()),
            }],
            target_directory: None,
            recursive: true,
            overwrite: false,
            idempotency_key: format!("copy-{}", Uuid::now_v7()),
        };

        let first = create(&pool, "local", actor(), request.clone())
            .await
            .unwrap();
        let repeated = create(&pool, "local", actor(), request).await.unwrap();
        assert_eq!(first.task_id, repeated.task_id);
        let task = wait_for_terminal(&pool, &first.task_id).await;
        assert_eq!(task.status, FileTaskStatus::Succeeded);
        assert_eq!(task.total_bytes, 7);
        assert_eq!(task.processed_bytes, 7);
        assert_eq!(task.completed_item_count, 1);
        assert_eq!(
            tokio::fs::read(target.join("first.txt")).await.unwrap(),
            b"1234"
        );
        assert!(
            tokio::fs::symlink_metadata(target.join("first-link"))
                .await
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn restart_marks_inflight_tasks_interrupted() {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query(
            "INSERT INTO file_operation_tasks \
             (id, node_id, idempotency_key, operation, status, stage, total_item_count, recursive, actor_name) \
             VALUES ('task-interrupted', 'local', 'restart-test', 'remove', 'running', 'deleting', 1, 1, 'admin')",
        )
        .execute(&pool)
        .await
        .unwrap();
        initialize(&pool).await.unwrap();
        let task = get(&pool, "task-interrupted").await.unwrap();
        assert_eq!(task.status, FileTaskStatus::Failed);
        assert_eq!(task.stage, FileTaskStage::Interrupted);
        assert_eq!(task.progress_percent, 100);
    }

    #[tokio::test]
    async fn preflight_failure_has_stable_item_result() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("preflight").await;
        let source = root.0.join("non-empty");
        tokio::fs::create_dir(&source).await.unwrap();
        tokio::fs::write(source.join("child"), b"data")
            .await
            .unwrap();
        let task = create(
            &pool,
            "local",
            actor(),
            CreateFileOperationTaskRequest {
                operation: FileOperation::Remove,
                items: vec![FileOperationItemRequest {
                    path: source.to_string_lossy().into_owned(),
                    expected_revision: None,
                    target_path: None,
                }],
                target_directory: None,
                recursive: false,
                overwrite: false,
                idempotency_key: format!("remove-{}", Uuid::now_v7()),
            },
        )
        .await
        .unwrap();
        let terminal = wait_for_terminal(&pool, &task.task_id).await;
        assert_eq!(terminal.status, FileTaskStatus::Failed);
        assert_eq!(terminal.failed_item_count, 1);
        assert_eq!(terminal.items[0].status, FileTaskItemStatus::Failed);
        assert_eq!(
            terminal.items[0].error_code.as_deref(),
            Some(ErrorCode::FileOperationConflict.as_str())
        );
        assert!(source.exists());
    }
}
