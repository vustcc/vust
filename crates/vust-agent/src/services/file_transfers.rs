//! 文件传输服务：持久化分块上传、Range 下载元数据与原子提交。

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
use ring::digest::{Context, SHA256};
use sqlx::Row;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::{Duration, interval};
use uuid::Uuid;
use vust_contracts::{
    api::ErrorCode,
    files::{CreateFileTransferRequest, FileTransfer, FileTransferDirection, FileTransferStatus},
};

pub const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const RETENTION_SECONDS: i64 = 24 * 60 * 60;
const RETENTION_INTERVAL_SECONDS: u64 = 10 * 60;

/// 按传输 ID 串行化分块写入、提交和取消，Weak 避免终态任务常驻内存。
static TRANSFER_LOCKS: Lazy<StdMutex<HashMap<String, Weak<Mutex<()>>>>> =
    Lazy::new(|| StdMutex::new(HashMap::new()));

/// 返回指定传输的进程内串行锁，并顺带清理已无持有者的弱引用。
fn transfer_lock(transfer_id: &str) -> Arc<Mutex<()>> {
    let mut locks = TRANSFER_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(transfer_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(transfer_id.to_owned(), Arc::downgrade(&lock));
    lock
}

/// 传输保存的可信操作者上下文。
#[derive(Debug, Clone)]
pub struct FileTransferActor {
    pub actor_name: String,
    pub client_ip: Option<String>,
    pub trace_id: Option<String>,
}

/// 启动过期传输清理；状态持久化后再 best-effort 删除上传暂存文件。
pub fn spawn_retention_worker(pool: DbPool) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(RETENTION_INTERVAL_SECONDS));
        loop {
            ticker.tick().await;
            if let Err(error) = expire_all(&pool).await {
                tracing::warn!(%error, "failed to expire file transfers");
            }
        }
    });
}

/// 创建上传或下载传输。
pub async fn create(
    pool: &DbPool,
    node_id: &str,
    actor: FileTransferActor,
    request: CreateFileTransferRequest,
) -> ApiResult<FileTransfer> {
    let path = normalize_absolute_path(&request.path)?;
    if request.overwrite {
        return Err(ApiError::validation(
            "file transfer overwrite is not supported",
        ));
    }
    let id = Uuid::now_v7().to_string();
    let _path_reservation = if request.direction == FileTransferDirection::Upload {
        Some(file_path_coordinator::reserve(pool, vec![path.clone()], None).await?)
    } else {
        None
    };
    let expires_at = now_seconds() + RETENTION_SECONDS;
    let (status, size_bytes, transferred_bytes, revision_value, temporary_path) =
        match request.direction {
            FileTransferDirection::Upload => {
                let size = request
                    .size_bytes
                    .ok_or_else(|| ApiError::validation("upload transfer requires sizeBytes"))?;
                if tokio::fs::symlink_metadata(&path).await.is_ok() {
                    return Err(ApiError::conflict(
                        ErrorCode::FileAlreadyExists,
                        "upload target already exists",
                    ));
                }
                let parent = path
                    .parent()
                    .ok_or_else(|| ApiError::validation("upload target has no parent"))?;
                if !tokio::fs::metadata(parent)
                    .await
                    .map(|value| value.is_dir())
                    .unwrap_or(false)
                {
                    return Err(ApiError::not_found(
                        ErrorCode::FileNotFound,
                        "upload target directory does not exist",
                    ));
                }
                let temporary = parent.join(format!(".vust-upload-{id}"));
                tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .await
                    .map_err(|error| map_io_error(error, "create upload staging file"))?;
                (
                    if size == 0 {
                        FileTransferStatus::Ready
                    } else {
                        FileTransferStatus::Created
                    },
                    size,
                    0,
                    None,
                    Some(temporary),
                )
            }
            FileTransferDirection::Download => {
                let metadata = tokio::fs::symlink_metadata(&path)
                    .await
                    .map_err(|error| map_io_error(error, "read download metadata"))?;
                if !metadata.is_file() {
                    return Err(ApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        ErrorCode::FileTypeUnsupported,
                        "download requires a regular file",
                    ));
                }
                let current_revision = revision(&metadata);
                if request
                    .expected_revision
                    .as_ref()
                    .is_some_and(|value| value != &current_revision)
                {
                    return Err(ApiError::conflict(
                        ErrorCode::FileChanged,
                        "download source changed",
                    ));
                }
                (
                    FileTransferStatus::Created,
                    metadata.len(),
                    0,
                    Some(current_revision),
                    None,
                )
            }
        };
    sqlx::query(
        "INSERT INTO file_transfers \
         (id, node_id, direction, status, path, temporary_path, size_bytes, transferred_bytes, \
          revision, sha256, actor_name, client_ip, trace_id, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )
    .bind(&id)
    .bind(node_id)
    .bind(direction_text(request.direction))
    .bind(status_text(status))
    .bind(path.to_string_lossy().as_ref())
    .bind(
        temporary_path
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned()),
    )
    .bind(size_bytes as i64)
    .bind(transferred_bytes as i64)
    .bind(revision_value)
    .bind(request.sha256.map(|value| value.to_ascii_lowercase()))
    .bind(actor.actor_name)
    .bind(actor.client_ip)
    .bind(actor.trace_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    get(pool, &id).await
}

/// 追加一个严格连续的上传分块。
pub async fn write_chunk(
    pool: &DbPool,
    transfer_id: &str,
    start: u64,
    end_inclusive: u64,
    total: u64,
    bytes: &[u8],
) -> ApiResult<FileTransfer> {
    let lock = transfer_lock(transfer_id);
    let _guard = lock.lock().await;
    if bytes.is_empty()
        || bytes.len() > MAX_CHUNK_BYTES
        || end_inclusive < start
        || end_inclusive - start + 1 != bytes.len() as u64
    {
        return Err(invalid_range());
    }
    let transfer = get(pool, transfer_id).await?;
    ensure_active(&transfer)?;
    if transfer.direction != FileTransferDirection::Upload
        || transfer.size_bytes != total
        || transfer.transferred_bytes != start
        || end_inclusive >= total
    {
        return Err(invalid_range());
    }
    let temporary = temporary_path(pool, transfer_id).await?;
    let mut file = match tokio::fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .await
    {
        Ok(file) => file,
        Err(error) => {
            return fail_upload_io(
                pool,
                transfer_id,
                &temporary,
                error,
                "open upload staging file",
            )
            .await;
        }
    };
    if let Err(error) = file.seek(std::io::SeekFrom::Start(start)).await {
        return fail_upload_io(
            pool,
            transfer_id,
            &temporary,
            error,
            "seek upload staging file",
        )
        .await;
    }
    if let Err(error) = file.write_all(bytes).await {
        return fail_upload_io(pool, transfer_id, &temporary, error, "write upload chunk").await;
    }
    if let Err(error) = file.sync_data().await {
        return fail_upload_io(pool, transfer_id, &temporary, error, "sync upload chunk").await;
    }
    let transferred = end_inclusive + 1;
    let status = if transferred == total {
        "ready"
    } else {
        "receiving"
    };
    sqlx::query(
        "UPDATE file_transfers SET status = ?2, transferred_bytes = ?3, updated_at = unixepoch() WHERE id = ?1",
    ).bind(transfer_id).bind(status).bind(transferred as i64).execute(pool).await?;
    get(pool, transfer_id).await
}

/// 校验大小和可选 SHA-256 后原子提交上传。
pub async fn complete_upload(pool: &DbPool, transfer_id: &str) -> ApiResult<FileTransfer> {
    let lock = transfer_lock(transfer_id);
    let _guard = lock.lock().await;
    let transfer = get(pool, transfer_id).await?;
    if transfer.direction != FileTransferDirection::Upload
        || transfer.status != FileTransferStatus::Ready
        || transfer.transferred_bytes != transfer.size_bytes
    {
        return Err(ApiError::conflict(
            ErrorCode::FileOperationConflict,
            "upload transfer is not ready to complete",
        ));
    }
    let (temporary, expected_sha256) = transfer_storage(pool, transfer_id).await?;
    if let Some(expected) = expected_sha256 {
        let actual = digest_file(&temporary).await?;
        if actual != expected {
            mark_failed(pool, transfer_id, "upload checksum mismatch").await?;
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(ApiError::conflict(
                ErrorCode::FileChecksumMismatch,
                "upload checksum mismatch",
            ));
        }
    }
    let target = PathBuf::from(&transfer.path);
    let _path_reservation =
        file_path_coordinator::reserve(pool, vec![target.clone()], Some(transfer_id)).await?;
    if tokio::fs::symlink_metadata(&target).await.is_ok() {
        mark_failed(pool, transfer_id, "upload target already exists").await?;
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ApiError::conflict(
            ErrorCode::FileAlreadyExists,
            "upload target already exists",
        ));
    }
    if let Err(error) = tokio::fs::rename(&temporary, &target).await {
        let mapped = map_io_error(error, "commit upload");
        mark_failed(pool, transfer_id, &mapped.message).await?;
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(mapped);
    }
    if let Err(error) = sync_parent_directory(target.parent().unwrap_or(Path::new("/"))).await {
        let _ = tokio::fs::rename(&target, &temporary).await;
        let _ = tokio::fs::remove_file(&temporary).await;
        mark_failed(pool, transfer_id, &error.message).await?;
        return Err(error);
    }
    let revision_value = tokio::fs::metadata(&target)
        .await
        .ok()
        .map(|metadata| revision(&metadata));
    sqlx::query(
        "UPDATE file_transfers SET status = 'completed', revision = ?2, updated_at = unixepoch() WHERE id = ?1",
    ).bind(transfer_id).bind(revision_value).execute(pool).await?;
    get(pool, transfer_id).await
}

/// 查询传输。
pub async fn get(pool: &DbPool, transfer_id: &str) -> ApiResult<FileTransfer> {
    expire_if_needed(pool, transfer_id).await?;
    let row = sqlx::query(
        "SELECT id, node_id, direction, status, path, size_bytes, transferred_bytes, revision, \
         error_summary, created_at, updated_at, expires_at FROM file_transfers WHERE id = ?1",
    )
    .bind(transfer_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found(ErrorCode::FileTaskNotFound, "file transfer not found"))?;
    transfer_from_row(&row)
}

/// 返回仍可继续的传输。
pub async fn active(pool: &DbPool) -> ApiResult<Vec<FileTransfer>> {
    expire_all(pool).await?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM file_transfers WHERE status IN ('created', 'receiving', 'ready', 'streaming') ORDER BY created_at",
    ).fetch_all(pool).await?;
    let mut transfers = Vec::with_capacity(ids.len());
    for id in ids {
        transfers.push(get(pool, &id).await?);
    }
    Ok(transfers)
}

/// 取消活动传输并清理上传暂存文件。
pub async fn cancel(pool: &DbPool, transfer_id: &str) -> ApiResult<FileTransfer> {
    let lock = transfer_lock(transfer_id);
    let _guard = lock.lock().await;
    let transfer = get(pool, transfer_id).await?;
    ensure_active(&transfer)?;
    if transfer.direction == FileTransferDirection::Upload
        && let Ok(path) = temporary_path(pool, transfer_id).await
    {
        let _ = tokio::fs::remove_file(path).await;
    }
    sqlx::query(
        "UPDATE file_transfers SET status = 'cancelled', updated_at = unixepoch() WHERE id = ?1",
    )
    .bind(transfer_id)
    .execute(pool)
    .await?;
    get(pool, transfer_id).await
}

/// 校验下载源 revision 并返回路径和长度。
pub async fn download_source(pool: &DbPool, transfer_id: &str) -> ApiResult<(PathBuf, u64, u64)> {
    let transfer = get(pool, transfer_id).await?;
    ensure_active(&transfer)?;
    if transfer.direction != FileTransferDirection::Download {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::FileTypeUnsupported,
            "transfer is not a download",
        ));
    }
    let path = PathBuf::from(&transfer.path);
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            let mapped = map_io_error(error, "read download metadata");
            mark_failed(pool, transfer_id, &mapped.message).await?;
            return Err(mapped);
        }
    };
    if transfer
        .revision
        .as_ref()
        .is_some_and(|value| value != &revision(&metadata))
    {
        mark_failed(pool, transfer_id, "download source changed").await?;
        return Err(ApiError::conflict(
            ErrorCode::FileChanged,
            "download source changed",
        ));
    }
    sqlx::query(
        "UPDATE file_transfers SET status = 'streaming', updated_at = unixepoch() WHERE id = ?1",
    )
    .bind(transfer_id)
    .execute(pool)
    .await?;
    Ok((path, metadata.len(), transfer.transferred_bytes))
}

/// 更新下载已发送字节和终态。
pub async fn record_download_progress(
    pool: &DbPool,
    transfer_id: &str,
    end_offset: u64,
    complete: bool,
) {
    let status = if complete { "completed" } else { "streaming" };
    let _ = sqlx::query(
        "UPDATE file_transfers SET status = ?2, transferred_bytes = MIN(size_bytes, MAX(transferred_bytes, ?3)), \
         updated_at = unixepoch() WHERE id = ?1",
    ).bind(transfer_id).bind(status).bind(end_offset as i64).execute(pool).await;
}

/// 将下载流读取失败记录为持久终态，错误摘要使用固定脱敏文案。
pub async fn record_download_failure(pool: &DbPool, transfer_id: &str) {
    let _ = mark_failed(pool, transfer_id, "download stream failed").await;
}

async fn fail_upload_io(
    pool: &DbPool,
    transfer_id: &str,
    temporary: &Path,
    error: std::io::Error,
    operation: &'static str,
) -> ApiResult<FileTransfer> {
    let mapped = map_io_error(error, operation);
    mark_failed(pool, transfer_id, &mapped.message).await?;
    let _ = tokio::fs::remove_file(temporary).await;
    Err(mapped)
}

async fn sync_parent_directory(path: &Path) -> ApiResult<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::File::open(path)?.sync_all())
        .await
        .map_err(|_| ApiError::internal("upload directory sync task failed"))?
        .map_err(|error| map_io_error(error, "sync upload target directory"))
}

async fn temporary_path(pool: &DbPool, transfer_id: &str) -> ApiResult<PathBuf> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT temporary_path FROM file_transfers WHERE id = ?1",
    )
    .bind(transfer_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    .map(PathBuf::from)
    .ok_or_else(|| ApiError::internal("upload transfer staging path is missing"))
}

async fn transfer_storage(
    pool: &DbPool,
    transfer_id: &str,
) -> ApiResult<(PathBuf, Option<String>)> {
    let row = sqlx::query("SELECT temporary_path, sha256 FROM file_transfers WHERE id = ?1")
        .bind(transfer_id)
        .fetch_one(pool)
        .await?;
    let path = row
        .try_get::<Option<String>, _>("temporary_path")?
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::internal("upload transfer staging path is missing"))?;
    Ok((path, row.try_get("sha256")?))
}

async fn digest_file(path: &Path) -> ApiResult<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| map_io_error(error, "open upload for checksum"))?;
    let mut context = Context::new(&SHA256);
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| map_io_error(error, "read upload checksum"))?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    Ok(hex::encode(context.finish().as_ref()))
}

async fn mark_failed(pool: &DbPool, transfer_id: &str, error: &str) -> ApiResult<()> {
    sqlx::query("UPDATE file_transfers SET status = 'failed', error_summary = ?2, updated_at = unixepoch() WHERE id = ?1")
        .bind(transfer_id).bind(error.chars().take(512).collect::<String>()).execute(pool).await?;
    Ok(())
}

fn ensure_active(transfer: &FileTransfer) -> ApiResult<()> {
    if transfer.status == FileTransferStatus::Expired {
        return Err(ApiError::conflict(
            ErrorCode::FileTransferExpired,
            "file transfer has expired",
        ));
    }
    if matches!(
        transfer.status,
        FileTransferStatus::Completed
            | FileTransferStatus::Failed
            | FileTransferStatus::Cancelled
            | FileTransferStatus::Expired
    ) {
        return Err(ApiError::conflict(
            ErrorCode::FileOperationConflict,
            "file transfer is already terminal",
        ));
    }
    Ok(())
}

async fn expire_if_needed(pool: &DbPool, transfer_id: &str) -> ApiResult<()> {
    let row = sqlx::query(
        "SELECT temporary_path FROM file_transfers WHERE id = ?1 AND expires_at <= unixepoch() \
         AND status IN ('created', 'receiving', 'ready', 'streaming')",
    )
    .bind(transfer_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let temporary_path = row.try_get::<Option<String>, _>("temporary_path")?;
    sqlx::query(
        "UPDATE file_transfers SET status = 'expired', updated_at = unixepoch() WHERE id = ?1",
    )
    .bind(transfer_id)
    .execute(pool)
    .await?;
    if let Some(path) = temporary_path {
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(())
}

async fn expire_all(pool: &DbPool) -> ApiResult<()> {
    let rows = sqlx::query(
        "SELECT id, temporary_path FROM file_transfers WHERE expires_at <= unixepoch() \
         AND status IN ('created', 'receiving', 'ready', 'streaming')",
    )
    .fetch_all(pool)
    .await?;
    sqlx::query(
        "UPDATE file_transfers SET status = 'expired', updated_at = unixepoch() \
         WHERE expires_at <= unixepoch() AND status IN ('created', 'receiving', 'ready', 'streaming')",
    )
    .execute(pool)
    .await?;
    for row in rows {
        if let Some(path) = row.try_get::<Option<String>, _>("temporary_path")? {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
    Ok(())
}

fn transfer_from_row(row: &sqlx::sqlite::SqliteRow) -> ApiResult<FileTransfer> {
    Ok(FileTransfer {
        transfer_id: row.try_get("id")?,
        node_id: row.try_get("node_id")?,
        direction: parse_direction(row.try_get("direction")?)?,
        status: parse_status(row.try_get("status")?)?,
        path: row.try_get("path")?,
        size_bytes: row.try_get::<i64, _>("size_bytes")? as u64,
        transferred_bytes: row.try_get::<i64, _>("transferred_bytes")? as u64,
        revision: row.try_get("revision")?,
        error_summary: row.try_get("error_summary")?,
        created_at: unix_timestamp(row.try_get("created_at")?),
        updated_at: unix_timestamp(row.try_get("updated_at")?),
        expires_at: unix_timestamp(row.try_get("expires_at")?),
    })
}

fn direction_text(value: FileTransferDirection) -> &'static str {
    match value {
        FileTransferDirection::Upload => "upload",
        FileTransferDirection::Download => "download",
    }
}
fn status_text(value: FileTransferStatus) -> &'static str {
    match value {
        FileTransferStatus::Created => "created",
        FileTransferStatus::Receiving => "receiving",
        FileTransferStatus::Ready => "ready",
        FileTransferStatus::Streaming => "streaming",
        FileTransferStatus::Completed => "completed",
        FileTransferStatus::Failed => "failed",
        FileTransferStatus::Cancelled => "cancelled",
        FileTransferStatus::Expired => "expired",
    }
}
fn parse_direction(value: String) -> ApiResult<FileTransferDirection> {
    match value.as_str() {
        "upload" => Ok(FileTransferDirection::Upload),
        "download" => Ok(FileTransferDirection::Download),
        _ => Err(ApiError::internal("invalid file transfer direction")),
    }
}
fn parse_status(value: String) -> ApiResult<FileTransferStatus> {
    match value.as_str() {
        "created" => Ok(FileTransferStatus::Created),
        "receiving" => Ok(FileTransferStatus::Receiving),
        "ready" => Ok(FileTransferStatus::Ready),
        "streaming" => Ok(FileTransferStatus::Streaming),
        "completed" => Ok(FileTransferStatus::Completed),
        "failed" => Ok(FileTransferStatus::Failed),
        "cancelled" => Ok(FileTransferStatus::Cancelled),
        "expired" => Ok(FileTransferStatus::Expired),
        _ => Err(ApiError::internal("invalid file transfer status")),
    }
}
fn invalid_range() -> ApiError {
    ApiError::new(
        StatusCode::RANGE_NOT_SATISFIABLE,
        ErrorCode::FileTransferInvalidRange,
        "invalid upload content range",
    )
}
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn unix_timestamp(value: i64) -> String {
    DateTime::<Utc>::from_timestamp(value, 0)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        async fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("vust-file-transfer-{label}-{}", Uuid::now_v7()));
            tokio::fs::create_dir(&path).await.unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn actor() -> FileTransferActor {
        FileTransferActor {
            actor_name: "admin".to_string(),
            client_ip: Some("127.0.0.1".to_string()),
            trace_id: Some("trace-transfer".to_string()),
        }
    }

    fn upload_request(path: &Path, size_bytes: u64) -> CreateFileTransferRequest {
        CreateFileTransferRequest {
            direction: FileTransferDirection::Upload,
            path: path.to_string_lossy().into_owned(),
            size_bytes: Some(size_bytes),
            sha256: None,
            expected_revision: None,
            overwrite: false,
        }
    }

    #[test]
    fn chunk_limit_is_eight_mebibytes() {
        assert_eq!(MAX_CHUNK_BYTES, 8 * 1024 * 1024);
    }

    #[tokio::test]
    async fn synchronous_path_reservation_blocks_upload_creation() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("sync-reservation").await;
        let target = root.0.join("payload.bin");
        let _reservation = file_path_coordinator::reserve(&pool, vec![target.clone()], None)
            .await
            .unwrap();

        let error = create(&pool, "local", actor(), upload_request(&target, 4))
            .await
            .unwrap_err();

        assert_eq!(error.code, ErrorCode::FileOperationConflict);
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn upload_rejects_out_of_order_chunks_and_commits_atomically() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("upload").await;
        let target = root.0.join("payload.bin");
        let transfer = create(&pool, "local", actor(), upload_request(&target, 10))
            .await
            .unwrap();
        let error = write_chunk(&pool, &transfer.transfer_id, 5, 9, 10, b"67890")
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::FileTransferInvalidRange);
        write_chunk(&pool, &transfer.transfer_id, 0, 4, 10, b"12345")
            .await
            .unwrap();
        let ready = write_chunk(&pool, &transfer.transfer_id, 5, 9, 10, b"67890")
            .await
            .unwrap();
        assert_eq!(ready.status, FileTransferStatus::Ready);
        assert!(!target.exists());
        let completed = complete_upload(&pool, &transfer.transfer_id).await.unwrap();
        assert_eq!(completed.status, FileTransferStatus::Completed);
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"1234567890");
    }

    #[tokio::test]
    async fn cancellation_wins_before_a_queued_chunk_and_cleans_staging() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("cancel-race").await;
        let target = root.0.join("payload.bin");
        let transfer = create(&pool, "local", actor(), upload_request(&target, 4))
            .await
            .unwrap();
        let staging = temporary_path(&pool, &transfer.transfer_id).await.unwrap();
        let operation_lock = transfer_lock(&transfer.transfer_id);
        let guard = operation_lock.lock().await;

        let cancel_pool = pool.clone();
        let cancel_id = transfer.transfer_id.clone();
        let cancel_task = tokio::spawn(async move { cancel(&cancel_pool, &cancel_id).await });
        tokio::task::yield_now().await;
        let chunk_pool = pool.clone();
        let chunk_id = transfer.transfer_id.clone();
        let chunk_task =
            tokio::spawn(
                async move { write_chunk(&chunk_pool, &chunk_id, 0, 3, 4, b"data").await },
            );
        drop(guard);

        let cancelled = cancel_task.await.unwrap().unwrap();
        assert_eq!(cancelled.status, FileTransferStatus::Cancelled);
        assert!(chunk_task.await.unwrap().is_err());
        assert_eq!(
            get(&pool, &transfer.transfer_id).await.unwrap().status,
            FileTransferStatus::Cancelled
        );
        assert!(!staging.exists());
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn cancellation_wins_before_a_queued_commit() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("cancel-commit-race").await;
        let target = root.0.join("empty.bin");
        let transfer = create(&pool, "local", actor(), upload_request(&target, 0))
            .await
            .unwrap();
        let operation_lock = transfer_lock(&transfer.transfer_id);
        let guard = operation_lock.lock().await;

        let cancel_pool = pool.clone();
        let cancel_id = transfer.transfer_id.clone();
        let cancel_task = tokio::spawn(async move { cancel(&cancel_pool, &cancel_id).await });
        tokio::task::yield_now().await;
        let commit_pool = pool.clone();
        let commit_id = transfer.transfer_id.clone();
        let commit_task =
            tokio::spawn(async move { complete_upload(&commit_pool, &commit_id).await });
        drop(guard);

        assert_eq!(
            cancel_task.await.unwrap().unwrap().status,
            FileTransferStatus::Cancelled
        );
        assert!(commit_task.await.unwrap().is_err());
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn checksum_failure_is_terminal_and_cleans_staging_file() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("checksum").await;
        let target = root.0.join("payload.bin");
        let mut request = upload_request(&target, 4);
        request.sha256 = Some("deadbeef".to_string());
        let transfer = create(&pool, "local", actor(), request).await.unwrap();
        let staging = temporary_path(&pool, &transfer.transfer_id).await.unwrap();
        write_chunk(&pool, &transfer.transfer_id, 0, 3, 4, b"data")
            .await
            .unwrap();
        let error = complete_upload(&pool, &transfer.transfer_id)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::FileChecksumMismatch);
        assert_eq!(
            get(&pool, &transfer.transfer_id).await.unwrap().status,
            FileTransferStatus::Failed
        );
        assert!(!staging.exists());
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn expired_upload_is_distinct_and_removes_staging_file() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("expiry").await;
        let target = root.0.join("payload.bin");
        let transfer = create(&pool, "local", actor(), upload_request(&target, 4))
            .await
            .unwrap();
        let staging = temporary_path(&pool, &transfer.transfer_id).await.unwrap();
        sqlx::query("UPDATE file_transfers SET expires_at = 0 WHERE id = ?1")
            .bind(&transfer.transfer_id)
            .execute(&pool)
            .await
            .unwrap();
        let expired = get(&pool, &transfer.transfer_id).await.unwrap();
        assert_eq!(expired.status, FileTransferStatus::Expired);
        assert!(!staging.exists());
        let error = write_chunk(&pool, &transfer.transfer_id, 0, 3, 4, b"data")
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::FileTransferExpired);
    }

    #[tokio::test]
    async fn zero_byte_upload_can_complete_without_a_chunk() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("empty").await;
        let target = root.0.join("empty.bin");
        let transfer = create(&pool, "local", actor(), upload_request(&target, 0))
            .await
            .unwrap();
        assert_eq!(transfer.status, FileTransferStatus::Ready);
        let completed = complete_upload(&pool, &transfer.transfer_id).await.unwrap();
        assert_eq!(completed.status, FileTransferStatus::Completed);
        assert_eq!(tokio::fs::metadata(target).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn download_source_change_becomes_failed_terminal_state() {
        let pool = crate::test_support::setup_test_db().await;
        let root = TestDirectory::new("download-revision").await;
        let source = root.0.join("source.bin");
        tokio::fs::write(&source, b"first").await.unwrap();
        let transfer = create(
            &pool,
            "local",
            actor(),
            CreateFileTransferRequest {
                direction: FileTransferDirection::Download,
                path: source.to_string_lossy().into_owned(),
                size_bytes: None,
                sha256: None,
                expected_revision: None,
                overwrite: false,
            },
        )
        .await
        .unwrap();
        tokio::fs::write(&source, b"changed-content").await.unwrap();
        let error = download_source(&pool, &transfer.transfer_id)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::FileChanged);
        assert_eq!(
            get(&pool, &transfer.transfer_id).await.unwrap().status,
            FileTransferStatus::Failed
        );
    }
}
