//! Docker Compose 套件运行接口：安装、启停、卸载与入口代理。

use crate::api::docker::context::DockerOperationContext;
use crate::api::suite_workloads;
use crate::config;
use crate::services::agent_runtime::CommandTransport;
use crate::state::AppState;
use crate::types::{AgentError, ApiError, ApiResponse, ApiResult};
use axum::body::Body;
use axum::extract::{Json, OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, Method, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bollard::models::NetworkCreateRequest;
use bollard::query_parameters;
use futures_util::StreamExt;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use vust_contracts::api::ErrorCode;
use vust_security::certs::{AGENT_CA_CERT_PEM, issue_client_cert};

const SUITE_NETWORK_NAME: &str = "vust-suite-network";
const COMPOSE_FILE_NAME: &str = "compose.yaml";
const SUITE_METADATA_FILE: &str = "suite-agent.json";
const SUITE_RUNTIME_DIR_NAME: &str = "runtime";
const SUITE_RUNTIME_DESCRIPTOR_FILE: &str = "runtime.json";
const SUITE_RUNTIME_TOKEN_FILE: &str = "access-token";
const SUITE_RUNTIME_OVERRIDE_FILE: &str = "compose.runtime.yaml";
const SUITE_ENTRY_READY_TIMEOUT: Duration = Duration::from_secs(60);
const SUITE_ENTRY_READY_INTERVAL: Duration = Duration::from_secs(1);
const SUITE_ENTRY_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
static SUITE_INSTALL_PROGRESS: LazyLock<Mutex<HashMap<String, SuiteInstallProgress>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 套件包内二进制安全文件。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SuitePackageFile {
    pub path: String,
    pub content_base64: String,
}

/// 套件应用入口。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SuiteAppEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
}

/// 安装套件的请求。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SuiteInstallRequest {
    pub instance_id: String,
    pub suite_id: String,
    pub version: String,
    pub compose_project_name: String,
    pub compose_file: String,
    #[serde(default)]
    pub runtime_images: Vec<String>,
    #[serde(default)]
    pub agent_access: Option<SuiteAgentAccess>,
    pub files: Vec<SuitePackageFile>,
    pub app_entries: Vec<SuiteAppEntry>,
}

/// 套件后端需要挂载 Agent 运行时的服务和能力。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SuiteAgentAccess {
    pub services: Vec<String>,
    pub capabilities: Vec<String>,
}

/// 套件生命周期动作请求。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteActionRequest {
    pub suite_id: String,
    pub suite_instance_id: String,
    pub compose_project_name: String,
    #[serde(default)]
    pub remove_data: bool,
}

/// 查询套件安装进度的请求参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteInstallProgressQuery {
    pub instance_id: String,
}

/// 当前节点上的套件安装进度状态。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteInstallProgress {
    pub instance_id: String,
    pub progress_percent: u32,
    pub status: String,
    pub current_step: String,
    pub current_image: Option<String>,
    pub is_finished: bool,
    pub error: Option<String>,
    pub cancel_requested: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SuiteAgentMetadata {
    pub(crate) instance_id: String,
    pub(crate) suite_id: String,
    version: String,
    compose_project_name: String,
    app_entries: Vec<SuiteAppEntry>,
    #[serde(default)]
    agent_access: Option<SuiteAgentGrant>,
}

/// Agent 持久化的套件实例授权。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SuiteAgentGrant {
    services: Vec<String>,
    capabilities: Vec<String>,
    #[serde(default)]
    runtime_images: Vec<String>,
    token_hash: String,
    enabled: bool,
}

/// 注入套件容器的语言无关 Agent 运行时描述。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SuiteRuntimeDescriptor {
    schema_version: u32,
    platform_version: String,
    suite_id: String,
    instance_id: String,
    endpoint: SuiteRuntimeEndpoint,
    credential: SuiteRuntimeCredential,
    capabilities: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
enum SuiteRuntimeEndpoint {
    Unix {
        socket_path: String,
        base_url: String,
    },
    Https {
        base_url: String,
        ca_path: String,
        client_cert_path: String,
        client_key_path: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SuiteRuntimeCredential {
    token_path: String,
}

#[derive(Debug, Deserialize)]
pub struct SuiteProxyPath {
    project: String,
    entry_id: String,
}

/// 取消套件安装任务的路径参数。
#[derive(Debug, Deserialize)]
pub struct SuiteInstallCancelPath {
    instance_id: String,
}

struct SuiteRuntimeLog<'a> {
    event: &'a str,
    target_type: &'a str,
    target_id: &'a str,
    context: &'a DockerOperationContext,
    request_path: &'a str,
    metadata: Value,
    error: Option<&'a str>,
}

/// 写入 Agent 侧套件事件日志。
async fn finish_suite_runtime_log(state: &Arc<AppState>, log: SuiteRuntimeLog<'_>) {
    let context = log.context;
    let event_code = suite_event_code(log.event);
    let high_impact = matches!(event_code, "suite_uninstall" | "suite_install_canceled");
    let _ = log.request_path;
    if let Some(error) = log.error {
        context
            .record_failure(
                &state.metadata_db,
                event_code,
                Some((log.target_type, log.target_id)),
                log.metadata,
                error,
            )
            .await;
    } else {
        context
            .record_success(
                &state.metadata_db,
                event_code,
                Some((log.target_type, log.target_id)),
                log.metadata,
                high_impact,
            )
            .await;
    }
}

fn suite_event_code(event: &str) -> &str {
    match event {
        "suite_runtime_install" => "suite_install",
        "suite_runtime_install_canceled" => "suite_install_canceled",
        "suite_runtime_enable" => "suite_enable",
        "suite_runtime_disable" => "suite_disable",
        "suite_runtime_uninstall" => "suite_uninstall",
        other => other,
    }
}

/// 构建 Agent 侧安装日志的基础元数据。
fn install_log_metadata(payload: &SuiteInstallRequest) -> Value {
    json!({
        "suite_id": payload.suite_id,
        "version": payload.version,
        "instance_id": payload.instance_id,
        "compose_project_name": payload.compose_project_name,
    })
}

/// 构建 Agent 侧生命周期动作日志的基础元数据。
fn action_log_metadata(payload: &SuiteActionRequest) -> Value {
    json!({
        "suite_id": payload.suite_id,
        "suite_instance_id": payload.suite_instance_id,
        "compose_project_name": payload.compose_project_name,
        "remove_data": payload.remove_data,
    })
}

/// 判断错误是否由套件安装取消触发。
fn is_suite_install_canceled_error(message: &str) -> bool {
    message.contains("suite install canceled")
}

/// 安装套件文件、准备镜像并登记 Compose 项目。
pub async fn install_suite(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Json(payload): Json<SuiteInstallRequest>,
) -> ApiResult<Response> {
    validate_id("instance_id", &payload.instance_id)?;
    validate_project_name(&payload.compose_project_name)?;
    validate_agent_access(payload.agent_access.as_ref())?;
    let log_metadata = install_log_metadata(&payload);
    upsert_install_progress(SuiteInstallProgress {
        instance_id: payload.instance_id.clone(),
        progress_percent: 1,
        status: "running".to_string(),
        current_step: "prepare".to_string(),
        current_image: None,
        is_finished: false,
        error: None,
        cancel_requested: false,
    });
    ensure_suite_network(&state).await?;

    let dir = suite_project_dir(&payload.compose_project_name);
    ensure_suite_project_available(&state, &payload.compose_project_name, &dir).await?;
    let result = install_suite_inner(&state, &payload, &dir).await;
    if let Err(err) = result {
        let canceled = is_install_cancel_requested(&payload.instance_id);
        update_install_progress(
            &payload.instance_id,
            100,
            if canceled { "canceled" } else { "failed" },
            if canceled { "canceled" } else { "failed" },
            None,
            true,
            if canceled {
                None
            } else {
                Some(err.to_string())
            },
        );
        rollback_suite_install(&payload.compose_project_name, &dir).await;
        let error = err.to_string();
        let canceled = is_suite_install_canceled_error(&error);
        let mut metadata = log_metadata;
        if let Some(object) = metadata.as_object_mut() {
            object.insert("canceled".to_string(), json!(canceled));
        }
        finish_suite_runtime_log(
            &state,
            SuiteRuntimeLog {
                event: if canceled {
                    "suite_runtime_install_canceled"
                } else {
                    "suite_runtime_install"
                },
                target_type: "suite_instance",
                target_id: &payload.instance_id,
                context: &context,
                request_path: "/api/v1/agent/docker/suites/install",
                metadata,
                error: if canceled { None } else { Some(&error) },
            },
        )
        .await;
        return Err(err);
    }

    update_install_progress(
        &payload.instance_id,
        100,
        "success",
        "completed",
        None,
        true,
        None,
    );
    finish_suite_runtime_log(
        &state,
        SuiteRuntimeLog {
            event: "suite_runtime_install",
            target_type: "suite_instance",
            target_id: &payload.instance_id,
            context: &context,
            request_path: "/api/v1/agent/docker/suites/install",
            metadata: log_metadata,
            error: None,
        },
    )
    .await;
    Ok(ApiResponse::ok("Suite installed").into_response())
}

/// 查询套件安装在当前节点执行面的实时进度。
pub async fn install_progress(
    Query(query): Query<SuiteInstallProgressQuery>,
) -> ApiResult<Response> {
    let sessions = SUITE_INSTALL_PROGRESS.lock().unwrap();
    let progress = sessions
        .get(&query.instance_id)
        .cloned()
        .ok_or(ApiError::NotFound)?;
    Ok(ApiResponse::success_with_raw("Suite install progress fetched", progress).into_response())
}

/// 请求取消当前节点上的套件安装任务。
pub async fn cancel_install(Path(path): Path<SuiteInstallCancelPath>) -> ApiResult<Response> {
    let mut sessions = SUITE_INSTALL_PROGRESS.lock().unwrap();
    let progress = sessions
        .get_mut(&path.instance_id)
        .ok_or(ApiError::NotFound)?;
    progress.cancel_requested = true;
    if !progress.is_finished {
        progress.status = "canceling".to_string();
        progress.current_step = "canceling".to_string();
    }
    Ok(
        ApiResponse::success_with_raw("Suite install cancellation requested", progress.clone())
            .into_response(),
    )
}

/// 新建或覆盖当前节点内存中的套件安装进度。
fn upsert_install_progress(progress: SuiteInstallProgress) {
    let mut sessions = SUITE_INSTALL_PROGRESS.lock().unwrap();
    sessions.insert(progress.instance_id.clone(), progress);
}

/// 更新当前节点内存中的安装进度，并保持百分比单调递增。
fn update_install_progress(
    instance_id: &str,
    progress_percent: u32,
    status: &str,
    current_step: &str,
    current_image: Option<String>,
    is_finished: bool,
    error: Option<String>,
) {
    let mut sessions = SUITE_INSTALL_PROGRESS.lock().unwrap();
    if let Some(progress) = sessions.get_mut(instance_id) {
        progress.progress_percent = progress.progress_percent.max(progress_percent.min(100));
        progress.status = status.to_string();
        progress.current_step = current_step.to_string();
        progress.current_image = current_image;
        progress.is_finished = is_finished;
        progress.error = error;
        if matches!(status, "canceled" | "failed" | "success") {
            progress.cancel_requested = false;
        }
    }
}

/// 判断指定实例的安装任务是否已经收到取消请求。
fn is_install_cancel_requested(instance_id: &str) -> bool {
    let sessions = SUITE_INSTALL_PROGRESS.lock().unwrap();
    sessions
        .get(instance_id)
        .is_some_and(|progress| progress.cancel_requested)
}

/// 执行套件安装事务，只有镜像全部可用后才持久化 Agent 元数据。
async fn install_suite_inner(
    state: &Arc<AppState>,
    payload: &SuiteInstallRequest,
    dir: &FsPath,
) -> ApiResult<()> {
    ensure_install_not_canceled(&payload.instance_id)?;
    tokio::fs::create_dir_all(&dir).await?;

    for file in &payload.files {
        let relative = normalize_relative_path(&file.path)?;
        let target = dir.join(relative);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let content = STANDARD.decode(&file.content_base64).map_err(|err| {
            ApiError::BadRequest(format!(
                "suite file has invalid base64 content: {}: {err}",
                file.path
            ))
        })?;
        tokio::fs::write(target, content).await?;
    }

    let compose_source = dir.join(normalize_relative_path(&payload.compose_file)?);
    if tokio::fs::metadata(&compose_source).await.is_err() {
        return Err(ApiError::BadRequest(format!(
            "compose file does not exist: {}",
            payload.compose_file
        )));
    }
    let compose_target = dir.join(COMPOSE_FILE_NAME);
    if compose_source != compose_target {
        let compose_text = tokio::fs::read_to_string(&compose_source).await?;
        tokio::fs::write(&compose_target, compose_text).await?;
    }
    let agent_access = prepare_suite_runtime_files(state, payload, dir).await?;
    prepare_compose_images(state, payload, &compose_target).await?;
    ensure_install_not_canceled(&payload.instance_id)?;

    let metadata = SuiteAgentMetadata {
        instance_id: payload.instance_id.clone(),
        suite_id: payload.suite_id.clone(),
        version: payload.version.clone(),
        compose_project_name: payload.compose_project_name.clone(),
        app_entries: payload.app_entries.clone(),
        agent_access,
    };
    let metadata_text = serde_json::to_string_pretty(&metadata)
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    tokio::fs::write(dir.join(SUITE_METADATA_FILE), metadata_text).await?;

    let dir_str = dir.to_string_lossy().to_string();
    sqlx::query(
        "INSERT INTO docker_compose_projects (\
            name, compose_dir, management_kind, owner_name, \
            config_revision, applied_revision\
         ) VALUES (?1, ?2, 'suite', ?3, 1, 1)",
    )
    .bind(&payload.compose_project_name)
    .bind(&dir_str)
    .bind(&payload.suite_id)
    .execute(&state.metadata_db)
    .await?;

    Ok(())
}

/// 在安装关键步骤前检查取消状态，已取消时中止安装事务。
fn ensure_install_not_canceled(instance_id: &str) -> ApiResult<()> {
    if is_install_cancel_requested(instance_id) {
        return Err(ApiError::BadRequest("suite install canceled".to_string()));
    }
    Ok(())
}

/// 安装失败时尽力清理尚未登记的套件目录。
async fn rollback_suite_install(project: &str, dir: &FsPath) {
    if tokio::fs::metadata(dir).await.is_ok()
        && let Err(err) = tokio::fs::remove_dir_all(dir).await
    {
        tracing::error!(
            project,
            path = %dir.display(),
            error = %err,
            "failed to rollback suite directory"
        );
    }
}

/// 确保稳定的套件项目名和目录没有被其他 Compose 项目占用。
async fn ensure_suite_project_available(
    state: &Arc<AppState>,
    project: &str,
    dir: &FsPath,
) -> ApiResult<()> {
    let registered = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM docker_compose_projects WHERE name = ?1",
    )
    .bind(project)
    .fetch_one(&state.metadata_db)
    .await?;
    if registered > 0 {
        return Err(ApiError::BadRequest(format!(
            "compose project name is already in use: {project}"
        )));
    }
    if tokio::fs::metadata(dir).await.is_ok() {
        return Err(ApiError::BadRequest(format!(
            "suite compose directory already exists: {}",
            dir.display()
        )));
    }
    Ok(())
}

/// 启用套件实例。
pub async fn enable_suite(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(project): Path<String>,
    Json(payload): Json<SuiteActionRequest>,
) -> ApiResult<Response> {
    validate_suite_project(&project, &payload.compose_project_name)?;
    ensure_suite_network(&state).await?;
    rotate_suite_runtime_token(&project, true).await?;
    let compose_file = suite_project_dir(&project).join(COMPOSE_FILE_NAME);
    let result =
        run_compose_command(&payload.compose_project_name, &compose_file, &["up", "-d"]).await;
    let result = async {
        result?;
        wait_for_suite_entries_ready(&state, &project, &payload.compose_project_name).await
    }
    .await;
    let error = result.as_ref().err().map(ToString::to_string);
    finish_suite_runtime_log(
        &state,
        SuiteRuntimeLog {
            event: "suite_runtime_enable",
            target_type: "compose_project",
            target_id: &payload.compose_project_name,
            context: &context,
            request_path: "/api/v1/agent/docker/suite/{project}/enable",
            metadata: action_log_metadata(&payload),
            error: error.as_deref(),
        },
    )
    .await;
    result?;
    Ok(ApiResponse::ok("Suite enabled").into_response())
}

/// 停用套件实例。
pub async fn disable_suite(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(project): Path<String>,
    Json(payload): Json<SuiteActionRequest>,
) -> ApiResult<Response> {
    validate_suite_project(&project, &payload.compose_project_name)?;
    let compose_file = suite_project_dir(&project).join(COMPOSE_FILE_NAME);
    let result = async {
        rotate_suite_runtime_token(&project, false).await?;
        let docker = state.docker_client().await?;
        suite_workloads::cleanup_suite_workloads_by_instance(&docker, &payload.suite_instance_id)
            .await?;
        run_compose_command(&payload.compose_project_name, &compose_file, &["stop"]).await
    }
    .await;
    let error = result.as_ref().err().map(ToString::to_string);
    finish_suite_runtime_log(
        &state,
        SuiteRuntimeLog {
            event: "suite_runtime_disable",
            target_type: "compose_project",
            target_id: &payload.compose_project_name,
            context: &context,
            request_path: "/api/v1/agent/docker/suite/{project}/disable",
            metadata: action_log_metadata(&payload),
            error: error.as_deref(),
        },
    )
    .await;
    result?;
    Ok(ApiResponse::ok("Suite disabled").into_response())
}

/// 卸载套件实例，默认不删除 named volume；用户明确选择删除数据时执行 `down -v`。
pub async fn uninstall_suite(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(project): Path<String>,
    Json(payload): Json<SuiteActionRequest>,
) -> ApiResult<Response> {
    validate_suite_project(&project, &payload.compose_project_name)?;
    let result = async {
        rotate_suite_runtime_token(&project, false).await?;
        let docker = state.docker_client().await?;
        suite_workloads::cleanup_suite_workloads_by_instance(&docker, &payload.suite_instance_id)
            .await?;
        let dir = suite_project_dir(&project);
        let compose_file = dir.join(COMPOSE_FILE_NAME);
        if tokio::fs::metadata(&compose_file).await.is_ok() {
            let args = if payload.remove_data {
                vec!["down", "-v"]
            } else {
                vec!["down"]
            };
            run_compose_command(&payload.compose_project_name, &compose_file, &args).await?;
        }
        sqlx::query("DELETE FROM docker_compose_projects WHERE name = ?1")
            .bind(&payload.compose_project_name)
            .execute(&state.metadata_db)
            .await?;
        if tokio::fs::metadata(&dir).await.is_ok() {
            tokio::fs::remove_dir_all(dir).await?;
        }
        Ok::<(), ApiError>(())
    }
    .await;
    let error = result.as_ref().err().map(ToString::to_string);
    finish_suite_runtime_log(
        &state,
        SuiteRuntimeLog {
            event: "suite_runtime_uninstall",
            target_type: "compose_project",
            target_id: &payload.compose_project_name,
            context: &context,
            request_path: "/api/v1/agent/docker/suite/{project}/uninstall",
            metadata: action_log_metadata(&payload),
            error: error.as_deref(),
        },
    )
    .await;
    result?;
    Ok(ApiResponse::ok("Suite uninstalled").into_response())
}

/// 等待套件 Web 入口端口就绪，避免入口已注册但应用进程尚未监听导致立即打开 502。
async fn wait_for_suite_entries_ready(
    state: &Arc<AppState>,
    project: &str,
    compose_project_name: &str,
) -> ApiResult<()> {
    let metadata = read_suite_metadata(project).await?;
    for entry in metadata
        .app_entries
        .iter()
        .filter(|entry| entry.entry_type == "proxied_web")
    {
        let service = entry
            .service
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("suite entry service is missing".to_string()))?;
        let port = entry
            .port
            .ok_or_else(|| ApiError::BadRequest("suite entry port is missing".to_string()))?;
        wait_for_suite_entry_ready(state, compose_project_name, entry, service, port).await?;
    }
    Ok(())
}

async fn wait_for_suite_entry_ready(
    state: &Arc<AppState>,
    compose_project_name: &str,
    entry: &SuiteAppEntry,
    service: &str,
    port: u16,
) -> ApiResult<()> {
    let deadline = Instant::now() + SUITE_ENTRY_READY_TIMEOUT;

    loop {
        let last_error = match resolve_service_ip(state, compose_project_name, service).await {
            Ok(container_ip) => match container_ip.parse::<IpAddr>() {
                Ok(ip) => {
                    let socket = SocketAddr::new(ip, port);
                    match timeout(SUITE_ENTRY_CONNECT_TIMEOUT, TcpStream::connect(socket)).await {
                        Ok(Ok(stream)) => {
                            drop(stream);
                            tracing::info!(
                                entry_id = %entry.id,
                                service = %service,
                                port = port,
                                compose_project_name = %compose_project_name,
                                "suite entry is ready"
                            );
                            return Ok(());
                        }
                        Ok(Err(err)) => err.to_string(),
                        Err(err) => err.to_string(),
                    }
                }
                Err(err) => {
                    format!("invalid container ip {container_ip}: {err}")
                }
            },
            Err(err) => err.to_string(),
        };

        if Instant::now() >= deadline {
            let detail = format!(
                "suite entry did not become ready within {} seconds: entry_id={}, service={}, port={}, last_error={}",
                SUITE_ENTRY_READY_TIMEOUT.as_secs(),
                entry.id,
                service,
                port,
                last_error
            );
            tracing::error!(
                entry_id = %entry.id,
                service = %service,
                port = port,
                compose_project_name = %compose_project_name,
                error = %last_error,
                "suite entry readiness timed out"
            );
            return Err(ApiError::bad_gateway(
                ErrorCode::ExternalRequestFailed,
                "suite entry is not ready",
            )
            .with_detail(detail));
        }

        sleep(SUITE_ENTRY_READY_INTERVAL).await;
    }
}

/// 代理套件 Web 入口。
pub async fn proxy_suite_entry(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    OriginalUri(uri): OriginalUri,
    Path(path): Path<SuiteProxyPath>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    validate_project_name(&path.project)?;
    validate_id("entry_id", &path.entry_id)?;
    let metadata = read_suite_metadata(&path.project).await?;
    let operation_context_id = if matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        Some(
            crate::api::suite_operation_logs::issue_operation_context(
                &state.metadata_db,
                &metadata.suite_id,
                &metadata.instance_id,
                &context,
            )
            .await?,
        )
    } else {
        None
    };
    let entry = metadata
        .app_entries
        .iter()
        .find(|item| item.id == path.entry_id)
        .ok_or_else(|| {
            ApiError::BadRequest(format!("suite app entry not found: {}", path.entry_id))
        })?;
    if entry.entry_type != "proxied_web" {
        return Err(ApiError::BadRequest(
            "suite entry is not a proxied web entry".to_string(),
        ));
    }
    let service = entry
        .service
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("suite entry service is missing".to_string()))?;
    let port = entry
        .port
        .ok_or_else(|| ApiError::BadRequest("suite entry port is missing".to_string()))?;
    let container_ip = resolve_service_ip(&state, &metadata.compose_project_name, service).await?;
    let suffix = proxy_suffix(
        uri.path_and_query()
            .map(|value| value.as_str())
            .unwrap_or(uri.path()),
        &path.project,
        &path.entry_id,
    );
    let target_url = format!("http://{container_ip}:{port}{suffix}");

    let client = reqwest::Client::new();
    let mut request = client.request(method, target_url.clone());
    for (name, value) in headers.iter() {
        if name.as_str().eq_ignore_ascii_case("host") || name.as_str().starts_with("x-vust-") {
            continue;
        }
        request = request.header(name, value);
    }
    if let Some(operation_context_id) = operation_context_id {
        request = request.header(
            crate::api::suite_operation_logs::SUITE_OPERATION_CONTEXT_HEADER,
            operation_context_id,
        );
    }
    let body_stream = body
        .into_data_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let response = match request
        .body(reqwest::Body::wrap_stream(body_stream))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            let detail = format!(
                "suite entry is not ready: failed to connect to {service}:{port} via {target_url}: {err}"
            );
            tracing::error!(
                error = %err,
                project = %path.project,
                entry_id = %path.entry_id,
                service = %service,
                port = port,
                target_url = %target_url,
                "suite proxy request failed"
            );
            return Err(ApiError::bad_gateway(
                ErrorCode::ExternalRequestFailed,
                "suite entry is not ready",
            )
            .with_detail(detail));
        }
    };

    let status = response.status();
    let mut headers = response.headers().clone();
    strip_hop_by_hop_headers(&mut headers);
    if status.is_server_error() && !is_event_stream_response(&headers) {
        let body_bytes = response.bytes().await.map_err(|err| {
            tracing::error!(
                error = %err,
                status = %status,
                project = %path.project,
                entry_id = %path.entry_id,
                service = %service,
                target_url = %target_url,
                "suite proxy failed to read upstream error body"
            );
            ApiError::from(err)
        })?;
        let body_excerpt = response_body_excerpt(&body_bytes);
        tracing::error!(
            status = %status,
            project = %path.project,
            entry_id = %path.entry_id,
            service = %service,
            target_url = %target_url,
            upstream_body = %body_excerpt,
            "suite proxy upstream returned server error"
        );
        let mut proxied = Response::builder()
            .status(status)
            .body(Body::from(body_bytes))?;
        *proxied.headers_mut() = headers;
        return Ok(proxied);
    }

    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let body = Body::from_stream(stream);
    let mut proxied = Response::builder().status(status).body(body)?;
    *proxied.headers_mut() = headers;
    Ok(proxied)
}

fn is_event_stream_response(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        })
}

fn response_body_excerpt(body: &[u8]) -> String {
    const MAX_EXCERPT_LEN: usize = 4096;

    let text = String::from_utf8_lossy(body);
    let excerpt: String = text.chars().take(MAX_EXCERPT_LEN).collect();
    if text.chars().count() > MAX_EXCERPT_LEN {
        format!("{excerpt}...[truncated]")
    } else {
        excerpt
    }
}

fn strip_hop_by_hop_headers(headers: &mut HeaderMap) {
    headers.remove(header::CONNECTION);
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove(header::CONTENT_LENGTH);
    headers.remove(header::UPGRADE);
    headers.remove(header::PROXY_AUTHENTICATE);
    headers.remove(header::PROXY_AUTHORIZATION);
    headers.remove(header::TE);
    headers.remove(header::TRAILER);
    headers.remove(HeaderName::from_static("keep-alive"));
}

async fn ensure_suite_network(state: &Arc<AppState>) -> ApiResult<()> {
    let docker = state.docker_client().await?;
    let labels = suite_network_labels();
    if let Ok(network) = docker
        .inspect_network(
            SUITE_NETWORK_NAME,
            Some(query_parameters::InspectNetworkOptions::default()),
        )
        .await
    {
        let current_labels = network.labels.as_ref();
        let has_required_labels = labels
            .iter()
            .all(|(key, value)| current_labels.and_then(|items| items.get(key)) == Some(value));
        if has_required_labels {
            return Ok(());
        }

        let has_attached_containers = network
            .containers
            .as_ref()
            .is_some_and(|items| !items.is_empty());
        if has_attached_containers {
            tracing::warn!(
                network = SUITE_NETWORK_NAME,
                "suite network exists without standard labels and is currently in use"
            );
            return Ok(());
        }
        docker.remove_network(SUITE_NETWORK_NAME).await?;
    }

    let request = NetworkCreateRequest {
        name: SUITE_NETWORK_NAME.to_string(),
        driver: Some("bridge".to_string()),
        labels: Some(labels),
        ..Default::default()
    };
    docker.create_network(request).await?;
    Ok(())
}

fn suite_network_labels() -> HashMap<String, String> {
    HashMap::from([("vust.owner".to_string(), "suite".to_string())])
}

async fn resolve_service_ip(
    state: &Arc<AppState>,
    project: &str,
    service: &str,
) -> ApiResult<String> {
    let docker = state.docker_client().await?;
    let mut filters = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![
            format!("com.docker.compose.project={project}"),
            format!("com.docker.compose.service={service}"),
        ],
    );
    filters.insert("status".to_string(), vec!["running".to_string()]);
    let options = query_parameters::ListContainersOptionsBuilder::new()
        .all(true)
        .filters(&filters)
        .build();
    let containers = docker.list_containers(Some(options)).await?;
    let container_id = containers
        .first()
        .and_then(|container| container.id.as_deref())
        .ok_or_else(|| ApiError::BadRequest(format!("suite service is not running: {service}")))?;
    let detail = docker
        .inspect_container(
            container_id,
            None::<query_parameters::InspectContainerOptions>,
        )
        .await?;
    let ip = detail
        .network_settings
        .and_then(|settings| settings.networks)
        .and_then(|mut networks| networks.remove(SUITE_NETWORK_NAME))
        .and_then(|endpoint| endpoint.ip_address)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "suite service is not connected to {SUITE_NETWORK_NAME}: {service}"
            ))
        })?;
    Ok(ip)
}

pub(crate) async fn read_suite_metadata(project: &str) -> ApiResult<SuiteAgentMetadata> {
    let text =
        tokio::fs::read_to_string(suite_project_dir(project).join(SUITE_METADATA_FILE)).await?;
    serde_json::from_str::<SuiteAgentMetadata>(&text)
        .map_err(|err| ApiError::BadRequest(format!("invalid suite metadata: {err}")))
}

/// 轮换或撤销套件实例令牌，并原子更新运行时文件和实例元数据。
async fn rotate_suite_runtime_token(project: &str, enabled: bool) -> ApiResult<()> {
    let dir = suite_project_dir(project);
    let metadata_path = dir.join(SUITE_METADATA_FILE);
    let mut metadata = read_suite_metadata(project).await?;
    let Some(grant) = metadata.agent_access.as_mut() else {
        return Ok(());
    };
    let token_path = dir
        .join(SUITE_RUNTIME_DIR_NAME)
        .join(SUITE_RUNTIME_TOKEN_FILE);
    if enabled {
        let token = generate_suite_access_token()?;
        atomic_write(&token_path, token.as_bytes()).await?;
        set_secret_file_permissions(&token_path).await?;
        grant.token_hash = hash_access_token(&token);
        grant.enabled = true;
    } else {
        grant.enabled = false;
        grant.token_hash.clear();
        let encoded = serde_json::to_vec_pretty(&metadata)
            .map_err(|err| ApiError::Internal(format!("failed to encode suite metadata: {err}")))?;
        atomic_write(&metadata_path, &encoded).await?;
        match tokio::fs::remove_file(&token_path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(AgentError::FileOperation(err).into()),
        }
        return Ok(());
    }
    let encoded = serde_json::to_vec_pretty(&metadata)
        .map_err(|err| ApiError::Internal(format!("failed to encode suite metadata: {err}")))?;
    atomic_write(&metadata_path, &encoded).await
}

/// 已通过实例令牌认证的套件运行时主体。
#[derive(Debug, Clone)]
pub(crate) struct SuiteRuntimePrincipal {
    pub(crate) suite_id: String,
    pub(crate) instance_id: String,
    pub(crate) capabilities: Vec<String>,
    pub(crate) runtime_images: Vec<String>,
}

/// 使用套件实例令牌解析授权主体，不暴露持久化令牌摘要。
pub(crate) async fn authenticate_suite_runtime(token: &str) -> ApiResult<SuiteRuntimePrincipal> {
    if token.trim().is_empty() {
        return Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "suite runtime token is required",
        ));
    }
    let token_hash = hash_access_token(token);
    let root = config::compose_root_dir().join("suite");
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "suite runtime token is invalid",
            ));
        }
        Err(err) => return Err(AgentError::FileOperation(err).into()),
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(AgentError::FileOperation)?
    {
        let metadata_path = entry.path().join(SUITE_METADATA_FILE);
        let text = match tokio::fs::read_to_string(&metadata_path).await {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(AgentError::FileOperation(err).into()),
        };
        let metadata = serde_json::from_str::<SuiteAgentMetadata>(&text).map_err(|err| {
            ApiError::BadRequest(format!(
                "invalid suite metadata: {}: {err}",
                metadata_path.display()
            ))
        })?;
        let Some(grant) = metadata.agent_access.as_ref() else {
            continue;
        };
        if grant.enabled && grant.token_hash == token_hash {
            return Ok(SuiteRuntimePrincipal {
                suite_id: metadata.suite_id,
                instance_id: metadata.instance_id,
                capabilities: grant.capabilities.clone(),
                runtime_images: grant.runtime_images.clone(),
            });
        }
    }
    Err(ApiError::forbidden(
        ErrorCode::AuthForbidden,
        "suite runtime token is invalid",
    ))
}

pub(crate) fn suite_project_dir(project: &str) -> PathBuf {
    config::compose_root_dir().join("suite").join(project)
}

async fn prepare_suite_runtime_files(
    state: &Arc<AppState>,
    payload: &SuiteInstallRequest,
    dir: &FsPath,
) -> ApiResult<Option<SuiteAgentGrant>> {
    let Some(access) = payload.agent_access.as_ref() else {
        return Ok(None);
    };
    let runtime_dir = dir.join(SUITE_RUNTIME_DIR_NAME);
    tokio::fs::create_dir_all(&runtime_dir).await?;
    let cert = issue_client_cert(&format!(
        "suite:{}:{}",
        payload.suite_id, payload.instance_id
    ))
    .map_err(|err| ApiError::Internal(format!("failed to issue suite client cert: {err}")))?;
    tokio::fs::write(runtime_dir.join("agent-client.crt"), &cert.cert_pem).await?;
    tokio::fs::write(runtime_dir.join("agent-client.key"), &cert.key_pem).await?;
    tokio::fs::write(runtime_dir.join("agent-ca.crt"), AGENT_CA_CERT_PEM).await?;
    let token = generate_suite_access_token()?;
    atomic_write(
        &runtime_dir.join(SUITE_RUNTIME_TOKEN_FILE),
        token.as_bytes(),
    )
    .await?;
    set_secret_file_permissions(&runtime_dir.join(SUITE_RUNTIME_TOKEN_FILE)).await?;
    write_suite_runtime_descriptor(state, payload, dir, &runtime_dir, access).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let key_path = runtime_dir.join("agent-client.key");
        tokio::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600)).await?;
    }

    Ok(Some(SuiteAgentGrant {
        services: access.services.clone(),
        capabilities: access.capabilities.clone(),
        runtime_images: payload.runtime_images.clone(),
        token_hash: hash_access_token(&token),
        enabled: true,
    }))
}

async fn write_suite_runtime_descriptor(
    state: &Arc<AppState>,
    payload: &SuiteInstallRequest,
    dir: &FsPath,
    runtime_dir: &FsPath,
    access: &SuiteAgentAccess,
) -> ApiResult<()> {
    let runtime = crate::services::agent_runtime::load(&state.metadata_db)
        .await
        .map_err(|err| ApiError::Internal(format!("failed to load agent identity: {err}")))?;
    let endpoint = match runtime.command_transport {
        CommandTransport::Uds => SuiteRuntimeEndpoint::Unix {
            socket_path: "/run/vust-agent.sock".to_string(),
            base_url: "http://local".to_string(),
        },
        CommandTransport::Https => SuiteRuntimeEndpoint::Https {
            base_url: runtime.suite_command_base_url.clone().ok_or_else(|| {
                ApiError::Internal("suite Agent HTTPS endpoint is unavailable".to_string())
            })?,
            ca_path: "/run/vust-agent/agent-ca.crt".to_string(),
            client_cert_path: "/run/vust-agent/agent-client.crt".to_string(),
            client_key_path: "/run/vust-agent/agent-client.key".to_string(),
        },
    };
    let descriptor = SuiteRuntimeDescriptor {
        schema_version: 1,
        platform_version: env!("CARGO_PKG_VERSION").to_string(),
        suite_id: payload.suite_id.clone(),
        instance_id: payload.instance_id.clone(),
        endpoint,
        credential: SuiteRuntimeCredential {
            token_path: "/run/vust-agent/access-token".to_string(),
        },
        capabilities: access.capabilities.clone(),
    };
    let descriptor = serde_json::to_vec_pretty(&descriptor)
        .map_err(|err| ApiError::Internal(format!("failed to encode suite runtime: {err}")))?;
    atomic_write(
        &runtime_dir.join(SUITE_RUNTIME_DESCRIPTOR_FILE),
        &descriptor,
    )
    .await?;
    write_suite_runtime_override(dir, runtime_dir, payload, access, runtime.command_transport)
        .await?;
    Ok(())
}

/// 为声明 Agent 能力的服务生成节点拓扑专属 Compose override。
async fn write_suite_runtime_override(
    dir: &FsPath,
    runtime_dir: &FsPath,
    payload: &SuiteInstallRequest,
    access: &SuiteAgentAccess,
    transport: CommandTransport,
) -> ApiResult<()> {
    let runtime_mount = serde_json::to_string(&format!(
        "{}:/run/vust-agent:ro",
        runtime_dir.to_string_lossy()
    ))
    .map_err(|err| ApiError::Internal(err.to_string()))?;
    let socket_mount = serde_json::to_string(&format!(
        "{}:/run/vust-agent.sock",
        vust_contracts::types::agent_socket_path().to_string_lossy()
    ))
    .map_err(|err| ApiError::Internal(err.to_string()))?;
    let mut output = String::from("services:\n");
    for service in &access.services {
        output.push_str(&format!(
            "  {service}:\n    environment:\n      VUST_SUITE_ID: {}\n      VUST_SUITE_INSTANCE_ID: {}\n      VUST_AGENT_RUNTIME: /run/vust-agent/runtime.json\n    volumes:\n      - {runtime_mount}\n",
            payload.suite_id, payload.instance_id
        ));
        match transport {
            CommandTransport::Uds => {
                output.push_str(&format!("      - {socket_mount}\n"));
            }
            CommandTransport::Https => {
                output
                    .push_str("    extra_hosts:\n      - \"host.docker.internal:host-gateway\"\n");
            }
        }
    }
    atomic_write(&dir.join(SUITE_RUNTIME_OVERRIDE_FILE), output.as_bytes()).await
}

fn validate_agent_access(access: Option<&SuiteAgentAccess>) -> ApiResult<()> {
    let Some(access) = access else {
        return Ok(());
    };
    if access.services.is_empty() || access.capabilities.is_empty() {
        return Err(ApiError::BadRequest(
            "suite Agent access must declare services and capabilities".to_string(),
        ));
    }
    for service in &access.services {
        validate_id("suite Agent service", service)?;
    }
    for capability in &access.capabilities {
        if !matches!(
            capability.as_str(),
            "workloads.manage" | "captures.manage" | "operation-logs.write"
        ) {
            return Err(ApiError::BadRequest(format!(
                "unsupported suite Agent capability: {capability}"
            )));
        }
    }
    Ok(())
}

fn generate_suite_access_token() -> ApiResult<String> {
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ApiError::Internal("failed to generate suite access token".to_string()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn hash_access_token(token: &str) -> String {
    hex::encode(digest::digest(&digest::SHA256, token.as_bytes()).as_ref())
}

async fn atomic_write(path: &FsPath, content: &[u8]) -> ApiResult<()> {
    let temporary = path.with_extension(format!("tmp-{}", Uuid::now_v7()));
    tokio::fs::write(&temporary, content).await?;
    tokio::fs::rename(&temporary, path).await?;
    Ok(())
}

/// 限制令牌和私钥仅允许宿主机属主读取。
async fn set_secret_file_permissions(path: &FsPath) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

fn proxy_suffix(path_with_query: &str, project: &str, entry_id: &str) -> String {
    let prefix = format!("/api/v1/agent/docker/suite/{project}/proxy/{entry_id}");
    let suffix = path_with_query.strip_prefix(&prefix).unwrap_or("/");
    if suffix.is_empty() {
        "/".to_string()
    } else {
        suffix.to_string()
    }
}

fn normalize_relative_path(path: &str) -> ApiResult<PathBuf> {
    let p = FsPath::new(path);
    let mut normalized = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::CurDir => {}
            _ => {
                return Err(ApiError::BadRequest(
                    "suite file path must be relative and safe".to_string(),
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(ApiError::BadRequest("suite file path is empty".to_string()));
    }
    Ok(normalized)
}

fn validate_id(label: &str, value: &str) -> ApiResult<()> {
    if value.trim().is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ApiError::BadRequest(format!(
            "{label} may only contain letters, digits, hyphen, underscore, and dot"
        )));
    }
    Ok(())
}

fn validate_project_name(value: &str) -> ApiResult<()> {
    validate_id("compose_project_name", value)
}

fn validate_suite_project(path_project: &str, payload_project: &str) -> ApiResult<()> {
    validate_project_name(path_project)?;
    validate_project_name(payload_project)?;
    if path_project != payload_project {
        return Err(ApiError::BadRequest(
            "suite compose project does not match request path".to_string(),
        ));
    }
    Ok(())
}

/// 解析 Compose 中的镜像，并确保每个镜像在目标节点本地可用。
async fn prepare_compose_images(
    state: &Arc<AppState>,
    payload: &SuiteInstallRequest,
    compose_file: &FsPath,
) -> ApiResult<()> {
    update_install_progress(
        &payload.instance_id,
        10,
        "running",
        "resolve_images",
        None,
        false,
        None,
    );
    let output = Command::new("docker")
        .args(["compose", "-f"])
        .arg(compose_file)
        .args(["-p", &payload.compose_project_name, "config", "--images"])
        .output()
        .await?;
    if !output.status.success() {
        let detail = command_error_detail(&output);
        tracing::error!(
            project = %payload.compose_project_name,
            compose_file = %compose_file.display(),
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "failed to resolve suite compose images"
        );
        return Err(ApiError::BadRequest(format!(
            "invalid suite compose configuration: {detail}"
        )));
    }

    let images = collect_install_images(&output.stdout, &payload.runtime_images);
    if images.is_empty() {
        return Err(ApiError::BadRequest(
            "suite compose configuration must declare at least one image".to_string(),
        ));
    }

    let images: Vec<String> = images.into_iter().collect();
    let image_count = images.len().max(1);
    for (index, image) in images.iter().enumerate() {
        ensure_install_not_canceled(&payload.instance_id)?;
        ensure_image_available(state, payload, image, index, image_count).await?;
    }
    Ok(())
}

/// 解析并去重 `docker compose config --images` 的输出。
fn parse_compose_images(output: &[u8]) -> BTreeSet<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .filter(|image| !image.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn collect_install_images(
    compose_images_output: &[u8],
    runtime_images: &[String],
) -> BTreeSet<String> {
    let mut images = parse_compose_images(compose_images_output);
    images.extend(
        runtime_images
            .iter()
            .map(|image| image.trim())
            .filter(|image| !image.is_empty())
            .map(ToOwned::to_owned),
    );
    images
}

/// 本地镜像存在时直接复用，否则从镜像仓库拉取固定版本镜像。
async fn ensure_image_available(
    state: &Arc<AppState>,
    payload: &SuiteInstallRequest,
    image: &str,
    image_index: usize,
    image_count: usize,
) -> ApiResult<()> {
    ensure_install_not_canceled(&payload.instance_id)?;
    let base_progress = image_progress(image_index, image_count, 0);
    update_install_progress(
        &payload.instance_id,
        base_progress,
        "running",
        "check_image",
        Some(image.to_string()),
        false,
        None,
    );
    let inspect = Command::new("docker")
        .args(["image", "inspect", image])
        .output()
        .await?;
    if inspect.status.success() {
        update_install_progress(
            &payload.instance_id,
            image_progress(image_index, image_count, 100),
            "running",
            "image_ready",
            Some(image.to_string()),
            false,
            None,
        );
        return Ok(());
    }

    ensure_install_not_canceled(&payload.instance_id)?;
    pull_image_with_progress(state, payload, image, image_index, image_count).await
}

/// 通过 Docker API 拉取镜像，并把 pull stream 转换为套件安装进度。
async fn pull_image_with_progress(
    state: &Arc<AppState>,
    payload: &SuiteInstallRequest,
    image: &str,
    image_index: usize,
    image_count: usize,
) -> ApiResult<()> {
    update_install_progress(
        &payload.instance_id,
        image_progress(image_index, image_count, 5),
        "running",
        "pull_image",
        Some(image.to_string()),
        false,
        None,
    );

    let result = crate::api::docker::images::pull_registry_image(
        state,
        image,
        || is_install_cancel_requested(&payload.instance_id),
        |info| {
            if let Some(detail) = info.progress_detail
                && let (Some(current), Some(total)) = (detail.current, detail.total)
                && total > 0
            {
                let layer_percent = ((current as f64 / total as f64) * 100.0).round() as u32;
                update_install_progress(
                    &payload.instance_id,
                    image_progress(image_index, image_count, layer_percent),
                    "running",
                    "pull_image",
                    Some(image.to_string()),
                    false,
                    None,
                );
            }
        },
    )
    .await
    .map_err(|err| ApiError::BadRequest(format!("suite image `{image}` pull failed: {err}")));

    result?;

    update_install_progress(
        &payload.instance_id,
        image_progress(image_index, image_count, 100),
        "running",
        "image_ready",
        Some(image.to_string()),
        false,
        None,
    );
    Ok(())
}

/// 将单个镜像的拉取百分比映射到整个安装任务的进度区间。
fn image_progress(image_index: usize, image_count: usize, image_percent: u32) -> u32 {
    let span = 75.0 / image_count as f64;
    let value = 15.0 + span * image_index as f64 + span * (image_percent.min(100) as f64 / 100.0);
    value.round().clamp(15.0, 90.0) as u32
}

/// 提取 Docker 命令的有效错误文本。
fn command_error_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if !detail.is_empty() {
        return detail.to_string();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = stdout.trim();
    if !detail.is_empty() {
        return detail.to_string();
    }
    match output.status.code() {
        Some(code) => format!("docker command exited with code {code}"),
        None => "docker command terminated without an exit code".to_string(),
    }
}

async fn run_compose_command(project: &str, compose_file: &FsPath, args: &[&str]) -> ApiResult<()> {
    if tokio::fs::metadata(compose_file).await.is_err() {
        return Err(ApiError::BadRequest(format!(
            "compose file does not exist: {}",
            compose_file.display()
        )));
    }
    let mut command = Command::new("docker");
    command.args(["compose", "-f"]).arg(compose_file);
    let runtime_override = compose_file.with_file_name(SUITE_RUNTIME_OVERRIDE_FILE);
    if tokio::fs::metadata(&runtime_override).await.is_ok() {
        command.arg("-f").arg(&runtime_override);
    }
    let output = command.args(["-p", project]).args(args).output().await?;

    if !output.status.success() {
        let detail = command_error_detail(&output);
        tracing::error!(
            project,
            compose_file = %compose_file.display(),
            args = ?args,
            status = ?output.status.code(),
            stdout = %String::from_utf8_lossy(&output.stdout).trim(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "suite compose command failed"
        );
        return Err(ApiError::BadRequest(format!(
            "suite compose command failed: {detail}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{collect_install_images, parse_compose_images, suite_event_code};

    #[test]
    fn suite_lifecycle_events_use_registered_operation_codes() {
        assert_eq!(suite_event_code("suite_runtime_install"), "suite_install");
        assert_eq!(suite_event_code("suite_runtime_enable"), "suite_enable");
        assert_eq!(
            suite_event_code("suite_runtime_install_canceled"),
            "suite_install_canceled"
        );
        assert_eq!(suite_event_code("suite_runtime_disable"), "suite_disable");
        assert_eq!(
            suite_event_code("suite_runtime_uninstall"),
            "suite_uninstall"
        );
    }

    #[test]
    fn parse_compose_images_removes_blank_lines_and_duplicates() {
        let images = parse_compose_images(
            b"docker.io/library/nginx:1.27-alpine\n\n custom/app:0.1.0-alpha.1 \ncustom/app:0.1.0-alpha.1\n",
        );

        assert_eq!(
            images.into_iter().collect::<Vec<_>>(),
            vec![
                "custom/app:0.1.0-alpha.1".to_string(),
                "docker.io/library/nginx:1.27-alpine".to_string(),
            ]
        );
    }

    #[test]
    fn collect_install_images_merges_compose_and_runtime_images() {
        let images = collect_install_images(
            b"vustcc/protocol-simulation:0.1.0-alpha.1\n",
            &[
                " vustcc/protocol-simulation-engine:0.1.0-alpha.1 ".to_string(),
                "vustcc/protocol-simulation:0.1.0-alpha.1".to_string(),
                "".to_string(),
            ],
        );

        assert_eq!(
            images.into_iter().collect::<Vec<_>>(),
            vec![
                "vustcc/protocol-simulation-engine:0.1.0-alpha.1".to_string(),
                "vustcc/protocol-simulation:0.1.0-alpha.1".to_string(),
            ]
        );
    }
}
