//! 文件管理 Agent API：只暴露稳定领域契约并强制可信变更上下文。

use crate::{
    config,
    services::{
        file_tasks::{self, FileTaskActor},
        file_transfers::{self, FileTransferActor},
        files::{self, FileListOptions, FileSortBy, FileSortOrder},
    },
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::{io, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use vust_contracts::files::{
    CreateDirectoryRequest, CreateFileOperationTaskRequest, CreateFileRequest,
    CreateFileTransferRequest, UpdateFileContentRequest,
};

const ACTOR_KIND_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-kind");
const ACTOR_NAME_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-name");
const CLIENT_IP_HEADER: HeaderName = HeaderName::from_static("x-vust-client-ip");
const TRACE_ID_HEADER: HeaderName = HeaderName::from_static("x-vust-trace-id");
const MAX_TEXT_REQUEST_BYTES: usize = files::MAX_TEXT_BYTES as usize * 6 + 64 * 1024;

struct DownloadStreamState {
    file: tokio::fs::File,
    remaining: u64,
    offset: u64,
    finished: bool,
    pool: crate::state::DbPool,
    transfer_id: String,
    complete_download: bool,
}

impl Drop for DownloadStreamState {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let pool = self.pool.clone();
        let transfer_id = self.transfer_id.clone();
        tokio::spawn(async move {
            file_transfers::record_download_failure(&pool, &transfer_id).await;
        });
    }
}

/// Master 注入且由内部传输边界保护的文件操作身份。
#[derive(Debug, Clone)]
pub struct FileOperationContext {
    pub actor_name: String,
    pub client_ip: Option<String>,
    pub trace_id: Option<String>,
}

impl<S> FromRequestParts<S> for FileOperationContext
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or(ApiError::ForbiddenResource)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PathQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    path: String,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_page_size")]
    page_size: u32,
    #[serde(default = "default_sort_by")]
    sort_by: String,
    #[serde(default = "default_sort_order")]
    sort_order: String,
    #[serde(default)]
    show_hidden: bool,
}

async fn home() -> Response {
    ApiResponse::success_with_raw("File home loaded", Some(files::home())).into_response()
}

async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Response> {
    let options = validate_list_query(query)?;
    let page = files::list(&state.metadata_db, options).await?;
    Ok(ApiResponse::success_with_raw("File list loaded", Some(page)).into_response())
}

async fn detail(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> ApiResult<Response> {
    let path = files::normalize_absolute_path(&query.path)?;
    let detail = files::detail(&state.metadata_db, &path).await?;
    Ok(ApiResponse::success_with_raw("File detail loaded", Some(detail)).into_response())
}

async fn read_content(Query(query): Query<PathQuery>) -> ApiResult<Response> {
    let path = files::normalize_absolute_path(&query.path)?;
    let document = files::read_content(&path).await?;
    Ok(ApiResponse::success_with_raw("File content loaded", Some(document)).into_response())
}

async fn create_file(
    State(state): State<Arc<AppState>>,
    context: FileOperationContext,
    Json(request): Json<CreateFileRequest>,
) -> ApiResult<Response> {
    let _ = (&context.actor_name, &context.client_ip, &context.trace_id);
    let path = files::normalize_absolute_path(&request.path)?;
    let detail = files::create_file(
        &state.metadata_db,
        &path,
        request.content.as_deref().unwrap_or_default(),
    )
    .await?;
    Ok(ApiResponse::success_with_raw("File created", Some(detail)).into_response())
}

async fn update_content(
    State(state): State<Arc<AppState>>,
    context: FileOperationContext,
    Json(request): Json<UpdateFileContentRequest>,
) -> ApiResult<Response> {
    let _ = (&context.actor_name, &context.client_ip, &context.trace_id);
    let path = files::normalize_absolute_path(&request.path)?;
    let result = files::update_content(
        &state.metadata_db,
        &path,
        &request.content,
        &request.expected_revision,
    )
    .await?;
    Ok(ApiResponse::success_with_raw("File content updated", Some(result)).into_response())
}

async fn create_directory(
    State(state): State<Arc<AppState>>,
    context: FileOperationContext,
    Json(request): Json<CreateDirectoryRequest>,
) -> ApiResult<Response> {
    let _ = (&context.actor_name, &context.client_ip, &context.trace_id);
    let path = files::normalize_absolute_path(&request.path)?;
    let detail = files::create_directory(&state.metadata_db, &path, request.recursive).await?;
    Ok(ApiResponse::success_with_raw("Directory created", Some(detail)).into_response())
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    context: FileOperationContext,
    Json(request): Json<CreateFileOperationTaskRequest>,
) -> ApiResult<Response> {
    let task = file_tasks::create(
        &state.metadata_db,
        &agent_node_id(),
        FileTaskActor {
            actor_name: context.actor_name,
            client_ip: context.client_ip,
            trace_id: context.trace_id,
        },
        request,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        ApiResponse::success_with_raw("File operation task accepted", Some(task)),
    )
        .into_response())
}

async fn task_detail(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let task = file_tasks::get(&state.metadata_db, &task_id).await?;
    Ok(ApiResponse::success_with_raw("File operation task loaded", Some(task)).into_response())
}

async fn active_tasks(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let tasks = file_tasks::active(&state.metadata_db).await?;
    Ok(
        ApiResponse::success_with_raw("Active file operation tasks loaded", Some(tasks))
            .into_response(),
    )
}

async fn cancel_task(
    State(state): State<Arc<AppState>>,
    _context: FileOperationContext,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let task = file_tasks::cancel(&state.metadata_db, &task_id).await?;
    Ok(
        ApiResponse::success_with_raw("File operation task cancellation requested", Some(task))
            .into_response(),
    )
}

async fn create_transfer(
    State(state): State<Arc<AppState>>,
    context: FileOperationContext,
    Json(request): Json<CreateFileTransferRequest>,
) -> ApiResult<Response> {
    let transfer = file_transfers::create(
        &state.metadata_db,
        &agent_node_id(),
        FileTransferActor {
            actor_name: context.actor_name,
            client_ip: context.client_ip,
            trace_id: context.trace_id,
        },
        request,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success_with_raw("File transfer created", Some(transfer)),
    )
        .into_response())
}

async fn transfer_detail(
    State(state): State<Arc<AppState>>,
    Path(transfer_id): Path<String>,
) -> ApiResult<Response> {
    let transfer = file_transfers::get(&state.metadata_db, &transfer_id).await?;
    Ok(ApiResponse::success_with_raw("File transfer loaded", Some(transfer)).into_response())
}

async fn active_transfers(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let transfers = file_transfers::active(&state.metadata_db).await?;
    Ok(
        ApiResponse::success_with_raw("Active file transfers loaded", Some(transfers))
            .into_response(),
    )
}

async fn upload_chunk(
    State(state): State<Arc<AppState>>,
    _context: FileOperationContext,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> ApiResult<Response> {
    let (start, end, total) = parse_content_range(&headers)?;
    let transfer =
        file_transfers::write_chunk(&state.metadata_db, &transfer_id, start, end, total, &bytes)
            .await?;
    Ok(ApiResponse::success_with_raw("Upload chunk stored", Some(transfer)).into_response())
}

async fn complete_transfer(
    State(state): State<Arc<AppState>>,
    _context: FileOperationContext,
    Path(transfer_id): Path<String>,
) -> ApiResult<Response> {
    let transfer = file_transfers::complete_upload(&state.metadata_db, &transfer_id).await?;
    Ok(ApiResponse::success_with_raw("Upload completed", Some(transfer)).into_response())
}

async fn cancel_transfer(
    State(state): State<Arc<AppState>>,
    _context: FileOperationContext,
    Path(transfer_id): Path<String>,
) -> ApiResult<Response> {
    let transfer = file_transfers::cancel(&state.metadata_db, &transfer_id).await?;
    Ok(ApiResponse::success_with_raw("File transfer cancelled", Some(transfer)).into_response())
}

async fn download_content(
    State(state): State<Arc<AppState>>,
    Path(transfer_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (path, size, previously_transferred) =
        file_transfers::download_source(&state.metadata_db, &transfer_id).await?;
    if size == 0 {
        file_transfers::record_download_progress(&state.metadata_db, &transfer_id, 0, true).await;
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, "0")
            .body(Body::empty())
            .map_err(ApiError::AxumError);
    }
    let (start, end, partial) = parse_download_range(&headers, size)?;
    let length = end - start + 1;
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| files::map_io_error(error, "open download source"))?;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|error| files::map_io_error(error, "seek download source"))?;

    let complete_download = end + 1 == size && (start == 0 || start == previously_transferred);
    let stream = futures_util::stream::unfold(
        DownloadStreamState {
            file,
            remaining: length,
            offset: start,
            finished: false,
            pool: state.metadata_db.clone(),
            transfer_id,
            complete_download,
        },
        |mut stream| async move {
            if stream.finished {
                return None;
            }
            let chunk_size = stream.remaining.min(1024 * 1024) as usize;
            let mut buffer = vec![0_u8; chunk_size];
            match stream.file.read(&mut buffer).await {
                Ok(0) => {
                    file_transfers::record_download_failure(&stream.pool, &stream.transfer_id)
                        .await;
                    stream.finished = true;
                    Some((
                        Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "download source truncated",
                        )),
                        stream,
                    ))
                }
                Ok(read) => {
                    buffer.truncate(read);
                    stream.offset += read as u64;
                    stream.remaining -= read as u64;
                    let (response_finished, transfer_completed) =
                        download_chunk_completion(stream.remaining, stream.complete_download);
                    file_transfers::record_download_progress(
                        &stream.pool,
                        &stream.transfer_id,
                        stream.offset,
                        transfer_completed,
                    )
                    .await;
                    stream.finished = response_finished;
                    Some((Ok(Bytes::from(buffer)), stream))
                }
                Err(error) => {
                    file_transfers::record_download_failure(&stream.pool, &stream.transfer_id)
                        .await;
                    stream.finished = true;
                    Some((Err(error), stream))
                }
            }
        },
    );
    let mut response = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(stream))
        .map_err(ApiError::AxumError)?;
    if partial {
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}"))
                .map_err(|_| ApiError::internal("invalid content range header"))?,
        );
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let safe_name = file_name.replace(['"', '\r', '\n'], "_");
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{safe_name}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

async fn operation_context_layer(mut request: axum::extract::Request, next: Next) -> Response {
    if let Some(context) = parse_context(request.headers()) {
        request.extensions_mut().insert(context);
    }
    next.run(request).await
}

fn parse_context(headers: &HeaderMap) -> Option<FileOperationContext> {
    if header_text(headers, &ACTOR_KIND_HEADER).as_deref() != Some("user") {
        return None;
    }
    Some(FileOperationContext {
        actor_name: header_text(headers, &ACTOR_NAME_HEADER)?,
        client_ip: header_text(headers, &CLIENT_IP_HEADER),
        trace_id: header_text(headers, &TRACE_ID_HEADER),
    })
}

fn header_text(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_content_range(headers: &HeaderMap) -> ApiResult<(u64, u64, u64)> {
    let value = headers
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(invalid_transfer_range)?;
    let value = value
        .strip_prefix("bytes ")
        .ok_or_else(invalid_transfer_range)?;
    let (range, total) = value.split_once('/').ok_or_else(invalid_transfer_range)?;
    let (start, end) = range.split_once('-').ok_or_else(invalid_transfer_range)?;
    Ok((
        start.parse().map_err(|_| invalid_transfer_range())?,
        end.parse().map_err(|_| invalid_transfer_range())?,
        total.parse().map_err(|_| invalid_transfer_range())?,
    ))
}

fn parse_download_range(headers: &HeaderMap, size: u64) -> ApiResult<(u64, u64, bool)> {
    let Some(value) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok((0, size - 1, false));
    };
    let range = value
        .strip_prefix("bytes=")
        .ok_or_else(invalid_transfer_range)?;
    if range.contains(',') {
        return Err(invalid_transfer_range());
    }
    let (start, end) = range.split_once('-').ok_or_else(invalid_transfer_range)?;
    if start.is_empty() {
        let suffix_length: u64 = end.parse().map_err(|_| invalid_transfer_range())?;
        if suffix_length == 0 {
            return Err(invalid_transfer_range());
        }
        return Ok((size.saturating_sub(suffix_length.min(size)), size - 1, true));
    }
    let start: u64 = start.parse().map_err(|_| invalid_transfer_range())?;
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse().map_err(|_| invalid_transfer_range())?
    };
    if start > end || end >= size {
        return Err(invalid_transfer_range());
    }
    Ok((start, end, true))
}

fn invalid_transfer_range() -> ApiError {
    ApiError::new(
        StatusCode::RANGE_NOT_SATISFIABLE,
        vust_contracts::api::ErrorCode::FileTransferInvalidRange,
        "invalid file transfer range",
    )
}

/// 判断当前响应与完整下载传输是否在本数据块后完成。
fn download_chunk_completion(remaining: u64, complete_download: bool) -> (bool, bool) {
    let response_finished = remaining == 0;
    (response_finished, response_finished && complete_download)
}

fn agent_node_id() -> String {
    config::get()
        .agent_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "local".to_string())
}

fn validate_list_query(query: ListQuery) -> ApiResult<FileListOptions> {
    if query.page == 0 || query.page_size == 0 || query.page_size > files::MAX_PAGE_SIZE {
        return Err(ApiError::validation("invalid file list pagination"));
    }
    let sort_by = match query.sort_by.as_str() {
        "name" => FileSortBy::Name,
        "modifiedAt" => FileSortBy::ModifiedAt,
        "sizeBytes" => FileSortBy::SizeBytes,
        _ => return Err(ApiError::validation("invalid file list sort field")),
    };
    let sort_order = match query.sort_order.as_str() {
        "asc" => FileSortOrder::Asc,
        "desc" => FileSortOrder::Desc,
        _ => return Err(ApiError::validation("invalid file list sort order")),
    };
    Ok(FileListOptions {
        path: files::normalize_absolute_path(&query.path)?,
        page: query.page,
        page_size: query.page_size,
        sort_by,
        sort_order,
        show_hidden: query.show_hidden,
    })
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    files::DEFAULT_PAGE_SIZE
}

fn default_sort_by() -> String {
    "name".to_string()
}

fn default_sort_order() -> String {
    "asc".to_string()
}

/// 构建仅供 Master 访问的文件领域路由。
pub fn fs_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/home", get(home))
        .route("/list", get(list))
        .route("/entry/detail", get(detail))
        .route(
            "/content",
            get(read_content)
                .put(update_content)
                .layer(DefaultBodyLimit::max(MAX_TEXT_REQUEST_BYTES)),
        )
        .route(
            "/entries",
            post(create_file).layer(DefaultBodyLimit::max(MAX_TEXT_REQUEST_BYTES)),
        )
        .route("/directories", post(create_directory))
        .route("/operation-tasks", post(create_task))
        .route("/operation-tasks/active", get(active_tasks))
        .route("/operation-task/{task_id}/detail", get(task_detail))
        .route("/operation-task/{task_id}/cancel", post(cancel_task))
        .route("/transfers", post(create_transfer))
        .route("/transfers/active", get(active_transfers))
        .route("/transfer/{transfer_id}/detail", get(transfer_detail))
        .route(
            "/transfer/{transfer_id}/chunk",
            axum::routing::put(upload_chunk)
                .layer(DefaultBodyLimit::max(file_transfers::MAX_CHUNK_BYTES)),
        )
        .route("/transfer/{transfer_id}/complete", post(complete_transfer))
        .route("/transfer/{transfer_id}/cancel", post(cancel_transfer))
        .route("/transfer/{transfer_id}/content", get(download_content))
        .layer(middleware::from_fn(operation_context_layer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn mutations_require_complete_trusted_context() {
        let mut headers = HeaderMap::new();
        headers.insert(&ACTOR_KIND_HEADER, HeaderValue::from_static("user"));
        assert!(parse_context(&headers).is_none());
        headers.insert(&ACTOR_NAME_HEADER, HeaderValue::from_static("admin"));
        assert_eq!(parse_context(&headers).unwrap().actor_name, "admin");
    }

    #[test]
    fn list_query_rejects_unbounded_page_size() {
        let query = ListQuery {
            path: "/".to_string(),
            page: 1,
            page_size: 501,
            sort_by: "name".to_string(),
            sort_order: "asc".to_string(),
            show_hidden: false,
        };
        assert!(validate_list_query(query).is_err());
    }

    #[test]
    fn download_range_supports_open_and_suffix_forms() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=10-"));
        assert_eq!(parse_download_range(&headers, 100).unwrap(), (10, 99, true));
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=-20"));
        assert_eq!(parse_download_range(&headers, 100).unwrap(), (80, 99, true));
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=-0"));
        assert!(parse_download_range(&headers, 100).is_err());
    }

    #[test]
    fn final_download_chunk_finishes_response_before_body_drop() {
        assert_eq!(download_chunk_completion(0, true), (true, true));
        assert_eq!(download_chunk_completion(0, false), (true, false));
        assert_eq!(download_chunk_completion(1, true), (false, false));
    }
}
