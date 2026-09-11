//! 套件中心 API：导入、安装、生命周期管理与代理入口。

use crate::api::auth::AuthenticatedAdmin;
use crate::models::NodeRuntimeClient;
use crate::models::desktop_apps::{delete_desktop_apps, hide_suite_desktop_apps};
use crate::models::logging::LogModule;
use crate::models::node_runtime_client::AgentOperationContext;
use crate::models::suites::{
    SuiteAppEntryManifest, SuiteAppEntryRecord, SuiteInstallTaskRecord, SuiteInstanceSummary,
    SuiteManifest, SuitePackageFile, SuitePackageSnapshot, delete_catalog_item, delete_instance,
    delete_instance_app_entries, fetch_catalog_payload, fetch_install_task, fetch_instance,
    fetch_instance_by_suite_and_node, insert_install_task, insert_instance,
    install_task_cancel_requested, list_install_tasks, list_suites, mark_install_task_canceling,
    mark_install_task_recovery_pending, replace_instance_app_entries, suite_has_instances,
    update_install_task, update_instance_status, upsert_catalog_item,
};
use crate::services::logging::{self, OperationEventBuilder};
use crate::state::{AppState, DbPool};
use crate::types::{ApiError, ApiResponse, ApiResult, new_uuid_v7};
use axum::body::Body;
use axum::extract::{
    Multipart, OriginalUri, Path, Query, Request, State, connect_info::ConnectInfo,
};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::read::GzDecoder;
use ring::digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use vust_contracts::types::DockerStatusSummary;

const MAX_SUITE_PACKAGE_BYTES: usize = 50 * 1024 * 1024;
const MAX_SUITE_UNPACKED_BYTES: usize = 50 * 1024 * 1024;
const SUITE_PACKAGE_EXTENSION: &str = ".vsp";
const MIN_SUITE_ICON_SIZE: u32 = 128;
const SUITE_DELETE_BLOCKED_MESSAGE_KEY: &str =
    "app.suiteCenter.messages.deleteBlockedByInstalledInstances";

/// 构建套件中心路由集合。
pub fn suites_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/list", get(list))
        .route("/import", post(import_suite))
        .route("/{suite_id}/install", post(install_suite))
        .route("/{suite_id}", delete(delete_suite))
        .route("/{suite_id}/assets/{*asset_path}", get(read_catalog_asset))
}

/// 构建套件实例路由集合。
pub fn suite_instances_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{instance_id}/enable", post(enable_instance))
        .route("/{instance_id}/disable", post(disable_instance))
        .route("/{instance_id}/uninstall", post(uninstall_instance))
        .route("/{instance_id}/assets/{*asset_path}", get(read_suite_asset))
        .route("/{instance_id}/proxy/{entry_id}/", any(proxy_suite_entry))
        .route(
            "/{instance_id}/proxy/{entry_id}/{*path}",
            any(proxy_suite_entry),
        )
        .route("/{instance_id}/proxy/{entry_id}", any(proxy_suite_entry))
}

/// 构建套件安装任务路由集合。
pub fn suite_install_tasks_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/list", get(list_install_task_progress))
        .route("/{task_id}/progress", get(install_progress))
        .route("/{task_id}/cancel", post(cancel_install))
}

/// 安装套件的请求参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallSuiteRequest {
    pub node_id: Option<String>,
}

/// 查询套件列表的请求参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteListQuery {
    pub node_id: Option<String>,
}

/// 安装任务列表查询参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteInstallTaskListQuery {
    pub node_id: Option<String>,
    #[serde(default)]
    pub active_only: bool,
}

/// 套件安装任务路径参数。
#[derive(Debug, Deserialize)]
pub struct SuiteInstallTaskPath {
    task_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallSuiteRequest {
    #[serde(default)]
    pub remove_data: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSuiteInstallRequest {
    instance_id: String,
    suite_id: String,
    version: String,
    compose_project_name: String,
    compose_file: String,
    runtime_images: Vec<String>,
    #[serde(default)]
    agent_access: Option<crate::models::suites::SuiteAgentAccessManifest>,
    files: Vec<SuitePackageFile>,
    app_entries: Vec<SuiteAppEntryManifest>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSuiteActionRequest {
    suite_id: String,
    suite_instance_id: String,
    compose_project_name: String,
    #[serde(default)]
    remove_data: bool,
}

#[derive(Debug, Deserialize)]
pub struct SuiteCatalogPath {
    suite_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SuiteProxyPath {
    instance_id: String,
    entry_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SuiteAssetPath {
    instance_id: String,
    asset_path: String,
}

#[derive(Debug, Deserialize)]
pub struct SuiteCatalogAssetPath {
    suite_id: String,
    asset_path: String,
}

#[derive(Clone, Debug)]
struct SuiteAuditContext {
    user_id: i64,
    username: String,
    client_ip: IpAddr,
    trace_id: String,
}

impl SuiteAuditContext {
    /// 从请求提取套件审计日志需要的用户、IP 与 trace 上下文。
    fn from_request(admin: &AuthenticatedAdmin, headers: &HeaderMap, conn: SocketAddr) -> Self {
        Self {
            user_id: admin.id,
            username: admin.username.clone(),
            client_ip: extract_client_ip(headers, conn),
            trace_id: logging::resolve_trace_id(headers),
        }
    }

    fn agent_operation_context(&self) -> AgentOperationContext {
        AgentOperationContext {
            actor_user_id: self.user_id,
            actor_name: self.username.clone(),
            client_ip: self.client_ip.to_string(),
            trace_id: self.trace_id.clone(),
        }
    }
}

struct SuitePlatformLog<'a> {
    event: &'a str,
    method: &'a str,
    request_path: &'a str,
    target_type: &'a str,
    target_id: &'a str,
    metadata: serde_json::Value,
    error: Option<&'a str>,
}

/// 根据代理头优先解析客户端 IP。
fn extract_client_ip(headers: &HeaderMap, conn: SocketAddr) -> IpAddr {
    if let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        && let Some(first) = forwarded.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return ip;
    }
    conn.ip()
}

/// 创建套件平台日志的基础记录。
fn suite_platform_log(ctx: &SuiteAuditContext, event: &str) -> OperationEventBuilder {
    OperationEventBuilder::new(&ctx.username, event, ctx.client_ip)
        .user_id(ctx.user_id)
        .module(LogModule::Docker)
        .source("vust_api")
        .trace_id(&ctx.trace_id)
}

/// 写入套件平台事件日志。
fn finish_suite_platform_log(
    state: &Arc<AppState>,
    ctx: &SuiteAuditContext,
    log: SuitePlatformLog<'_>,
) {
    let mut metadata = log.metadata;
    if let Some(error) = log.error {
        if let Some(object) = metadata.as_object_mut() {
            object.insert("error".to_string(), serde_json::json!(error));
        } else {
            metadata = serde_json::json!({ "value": metadata, "error": error });
        }
    }

    let mut entry = suite_platform_log(ctx, log.event)
        .target_type(log.target_type)
        .target_id(log.target_id)
        .request(log.method, log.request_path)
        .metadata(metadata);

    if log.error.is_none() {
        entry = entry.set_success();
    }
    entry.finish(&state.metadata_db);
}

/// 构建套件清单相关日志元数据。
fn suite_manifest_log_metadata(manifest: &SuiteManifest) -> serde_json::Value {
    serde_json::json!({
        "suite_id": manifest.metadata.suite_id,
        "suite_name": manifest.metadata.name,
        "version": manifest.metadata.version,
        "slug": manifest.metadata.slug,
    })
}

/// 构建套件实例相关日志元数据。
fn suite_instance_log_metadata(instance: &SuiteInstanceSummary) -> serde_json::Value {
    serde_json::json!({
        "suite_id": instance.suite_id,
        "version": instance.version,
        "instance_id": instance.instance_id,
        "node_id": instance.node_id,
        "compose_project_name": instance.compose_project_name,
    })
}

/// 返回套件目录和实例列表。
pub async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SuiteListQuery>,
) -> ApiResult<Response> {
    // 套件清单本地化只依赖请求语言，不把语言状态写入数据库。
    let locale = resolve_request_locale(&headers);
    let node_id = query.node_id.as_deref().unwrap_or("local");
    let data = list_suites(&state.metadata_db, locale.as_deref(), Some(node_id)).await?;
    Ok(ApiResponse::success_with_raw("Suites loaded", data).into_response())
}

/// 上传并导入套件包。
pub async fn import_suite(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    let ctx = SuiteAuditContext::from_request(&admin, &headers, conn);
    let result = async {
        let bytes = read_suite_upload(&mut multipart).await?;
        let checksum = hex::encode(digest::digest(&digest::SHA256, &bytes).as_ref());
        let (manifest, package) = parse_suite_package(&bytes)?;
        upsert_catalog_item(&state.metadata_db, &manifest, &package, &checksum).await?;
        Ok::<_, ApiError>((manifest, checksum))
    }
    .await;

    match &result {
        Ok((manifest, checksum)) => finish_suite_platform_log(
            &state,
            &ctx,
            SuitePlatformLog {
                event: "suite_import",
                method: "POST",
                request_path: "/api/v1/suites/import",
                target_type: "suite_catalog",
                target_id: &manifest.metadata.suite_id,
                metadata: {
                    let mut metadata = suite_manifest_log_metadata(manifest);
                    if let Some(object) = metadata.as_object_mut() {
                        object.insert("checksum".to_string(), serde_json::json!(checksum));
                    }
                    metadata
                },
                error: None,
            },
        ),
        Err(err) => finish_suite_platform_log(
            &state,
            &ctx,
            SuitePlatformLog {
                event: "suite_import",
                method: "POST",
                request_path: "/api/v1/suites/import",
                target_type: "suite_catalog",
                target_id: "",
                metadata: serde_json::json!({}),
                error: Some(&err.to_string()),
            },
        ),
    }

    let (manifest, _) = result?;
    Ok(ApiResponse::success_with_raw("Suite imported", manifest).into_response())
}

/// 从套件中心删除未安装的套件目录项。
pub async fn delete_suite(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(path): Path<SuiteCatalogPath>,
) -> ApiResult<Response> {
    let ctx = SuiteAuditContext::from_request(&admin, &headers, conn);
    let result = async {
        if suite_has_instances(&state.metadata_db, &path.suite_id).await? {
            return Err(suite_delete_blocked_by_instances_error());
        }
        let (manifest, _) = fetch_catalog_payload(&state.metadata_db, &path.suite_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest(format!("suite not found: {}", path.suite_id)))?;
        delete_catalog_item(&state.metadata_db, &path.suite_id).await?;
        Ok::<_, ApiError>(manifest)
    }
    .await;
    let metadata = result
        .as_ref()
        .map(suite_manifest_log_metadata)
        .unwrap_or_else(|_| serde_json::json!({ "suite_id": path.suite_id }));
    finish_suite_platform_log(
        &state,
        &ctx,
        SuitePlatformLog {
            event: "suite_delete",
            method: "DELETE",
            request_path: "/api/v1/suites/{suite_id}",
            target_type: "suite_catalog",
            target_id: &path.suite_id,
            metadata,
            error: result.as_ref().err().map(ToString::to_string).as_deref(),
        },
    );
    result?;
    Ok(ApiResponse::ok("Suite deleted").into_response())
}

/// 构建删除套件包时存在任意节点实例的冲突提示。
fn suite_delete_blocked_by_instances_error() -> ApiError {
    ApiError::BadRequest("suite is installed; uninstall all instances before deleting".to_string())
        .with_message_key(SUITE_DELETE_BLOCKED_MESSAGE_KEY)
}

/// 启动套件安装任务；套件安装完成后不自动启用。
pub async fn install_suite(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(suite_id): Path<String>,
    Json(payload): Json<InstallSuiteRequest>,
) -> ApiResult<Response> {
    let ctx = SuiteAuditContext::from_request(&admin, &headers, conn);
    let node_id = payload.node_id.unwrap_or_else(|| "local".to_string());
    let (manifest, package) = match fetch_catalog_payload(&state.metadata_db, &suite_id).await? {
        Some(payload) => payload,
        None => {
            let err = ApiError::BadRequest(format!("suite not found: {suite_id}"));
            finish_suite_platform_log(
                &state,
                &ctx,
                SuitePlatformLog {
                    event: "suite_install_start",
                    method: "POST",
                    request_path: "/api/v1/suites/{suite_id}/install",
                    target_type: "suite_catalog",
                    target_id: &suite_id,
                    metadata: serde_json::json!({
                        "suite_id": suite_id,
                        "node_id": node_id,
                    }),
                    error: Some(&err.to_string()),
                },
            );
            return Err(err);
        }
    };
    if fetch_instance_by_suite_and_node(&state.metadata_db, &manifest.metadata.suite_id, &node_id)
        .await?
        .is_some()
    {
        let err = ApiError::BadRequest(format!(
            "suite is already installed on node {node_id}: {}",
            manifest.metadata.suite_id,
        ));
        let mut metadata = suite_manifest_log_metadata(&manifest);
        if let Some(object) = metadata.as_object_mut() {
            object.insert("node_id".to_string(), serde_json::json!(node_id));
        }
        finish_suite_platform_log(
            &state,
            &ctx,
            SuitePlatformLog {
                event: "suite_install_start",
                method: "POST",
                request_path: "/api/v1/suites/{suite_id}/install",
                target_type: "suite_catalog",
                target_id: &manifest.metadata.suite_id,
                metadata,
                error: Some(&err.to_string()),
            },
        );
        return Err(err);
    }
    if let Err(err) = ensure_suite_node_ready(&state, &node_id).await {
        let mut metadata = suite_manifest_log_metadata(&manifest);
        if let Some(object) = metadata.as_object_mut() {
            object.insert("node_id".to_string(), serde_json::json!(node_id));
        }
        finish_suite_platform_log(
            &state,
            &ctx,
            SuitePlatformLog {
                event: "suite_install_start",
                method: "POST",
                request_path: "/api/v1/suites/{suite_id}/install",
                target_type: "suite_catalog",
                target_id: &manifest.metadata.suite_id,
                metadata,
                error: Some(&err.to_string()),
            },
        );
        return Err(err);
    }
    let instance_id = new_uuid_v7();
    let task_id = new_uuid_v7();
    let compose_project_name = build_compose_project_name(&manifest.metadata.slug);

    let instance = SuiteInstanceSummary {
        instance_id: instance_id.clone(),
        suite_id: manifest.metadata.suite_id.clone(),
        version: manifest.metadata.version.clone(),
        platform_contract_version: i64::from(manifest.compatibility.platform_contract_version),
        node_id: node_id.clone(),
        compose_project_name: compose_project_name.clone(),
        status: "installing".to_string(),
        last_error: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    insert_instance(&state.metadata_db, &instance).await?;

    let request = AgentSuiteInstallRequest {
        instance_id: instance_id.clone(),
        suite_id: manifest.metadata.suite_id.clone(),
        version: manifest.metadata.version.clone(),
        compose_project_name,
        compose_file: manifest.runtime.compose_file.clone(),
        runtime_images: normalize_runtime_images(&manifest.runtime.images),
        agent_access: manifest.runtime.agent.clone(),
        files: package.files,
        app_entries: manifest.app_entries,
    };

    let install_task = SuiteInstallTaskRecord {
        task_id: task_id.clone(),
        instance_id: instance_id.clone(),
        suite_id: manifest.metadata.suite_id.clone(),
        node_id: node_id.clone(),
        progress_percent: 1_i64,
        status: "queued".to_string(),
        current_step: "queued".to_string(),
        current_image: None,
        is_finished: false,
        error: None,
        cancel_requested: false,
        recovery_started_at: None,
        created_at: String::new(),
        updated_at: String::new(),
        finished_at: None,
    };
    if let Err(err) = insert_install_task(&state.metadata_db, &install_task).await {
        let _ = delete_instance(&state.metadata_db, &instance_id).await;
        return Err(err.into());
    }
    let install_task = fetch_install_task(&state.metadata_db, &task_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let state_for_task = Arc::clone(&state);
    let task_id_for_task = task_id.clone();
    let instance_id_for_task = instance_id.clone();
    let node_id_for_task = node_id.clone();
    let ctx_for_task = ctx.clone();
    tokio::spawn(async move {
        run_suite_install_task(
            state_for_task,
            task_id_for_task,
            node_id_for_task,
            instance_id_for_task,
            request,
            ctx_for_task,
        )
        .await;
    });

    finish_suite_platform_log(
        &state,
        &ctx,
        SuitePlatformLog {
            event: "suite_install_start",
            method: "POST",
            request_path: "/api/v1/suites/{suite_id}/install",
            target_type: "suite_instance",
            target_id: &instance_id,
            metadata: serde_json::json!({
                "suite_id": manifest.metadata.suite_id,
                "suite_name": manifest.metadata.name,
                "version": manifest.metadata.version,
                "instance_id": instance_id,
                "node_id": node_id,
                "compose_project_name": instance.compose_project_name,
                "task_id": task_id,
            }),
            error: None,
        },
    );
    Ok(ApiResponse::success_with_raw("Suite install task started", install_task).into_response())
}

/// 查询节点上的套件安装任务，供应用恢复后台进度。
pub async fn list_install_task_progress(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SuiteInstallTaskListQuery>,
) -> ApiResult<Response> {
    let tasks = list_install_tasks(
        &state.metadata_db,
        query.node_id.as_deref(),
        query.active_only,
    )
    .await?;
    Ok(ApiResponse::success_with_raw("Suite install tasks loaded", tasks).into_response())
}

/// 查询套件安装任务的实时进度。
pub async fn install_progress(
    State(state): State<Arc<AppState>>,
    Path(path): Path<SuiteInstallTaskPath>,
) -> ApiResult<Response> {
    let mut progress = fetch_install_task(&state.metadata_db, &path.task_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !progress.is_finished {
        let mut recovered = false;
        if let Ok(client) =
            NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&progress.node_id)).await
        {
            let path = format!(
                "/api/v1/agent/docker/suites/install-progress?instanceId={}",
                progress.instance_id
            );
            if let Ok(agent_response) = client.get_json::<serde_json::Value>(&path).await
                && let Some(agent_progress) =
                    parse_agent_install_progress(&agent_response, &progress)
            {
                progress = agent_progress;
                persist_install_progress(&state.metadata_db, &progress).await?;
                reconcile_recovered_install(&state.metadata_db, &progress).await?;
                recovered = true;
            }
        }

        if !recovered {
            progress = mark_install_task_recovery_pending(&state.metadata_db, &progress.task_id)
                .await?
                .ok_or(ApiError::NotFound)?;
        }

        if !recovered
            && progress
                .recovery_started_at
                .as_deref()
                .is_some_and(is_install_recovery_expired)
        {
            const RECOVERY_ERROR: &str = "Master 重启后两分钟内未能从 Agent 恢复安装任务";
            update_instance_status(
                &state.metadata_db,
                &progress.instance_id,
                "error",
                Some(RECOVERY_ERROR),
            )
            .await?;
            update_install_task(
                &state.metadata_db,
                &progress.task_id,
                u32::try_from(progress.progress_percent).unwrap_or(0),
                "failed",
                "recovery_failed",
                progress.current_image.clone(),
                true,
                Some(RECOVERY_ERROR.to_string()),
            )
            .await?;
            progress = fetch_install_task(&state.metadata_db, &path.task_id)
                .await?
                .ok_or(ApiError::NotFound)?;
        }
    }

    Ok(ApiResponse::success_with_raw("Suite install progress fetched", progress).into_response())
}

/// 取消正在执行的套件安装任务，并尽力清理未完成的实例记录。
pub async fn cancel_install(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(path): Path<SuiteInstallTaskPath>,
) -> ApiResult<Response> {
    let ctx = SuiteAuditContext::from_request(&admin, &headers, conn);
    let progress = mark_install_task_canceling(&state.metadata_db, &path.task_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if progress.is_finished {
        finish_suite_platform_log(
            &state,
            &ctx,
            SuitePlatformLog {
                event: "suite_install_canceled",
                method: "POST",
                request_path: "/api/v1/suite-install-tasks/{task_id}/cancel",
                target_type: "suite_instance",
                target_id: &progress.instance_id,
                metadata: serde_json::json!({
                    "task_id": path.task_id,
                    "instance_id": progress.instance_id,
                    "node_id": progress.node_id,
                    "status": progress.status,
                    "already_finished": true,
                }),
                error: None,
            },
        );
        return Ok(
            ApiResponse::success_with_raw("Suite install already finished", progress)
                .into_response(),
        );
    }

    if let Ok(client) =
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&progress.node_id)).await
    {
        let cancel_path = format!(
            "/api/v1/agent/docker/suites/install-progress/{}/cancel",
            progress.instance_id
        );
        if let Err(err) = client
            .post_json::<serde_json::Value, _>(&cancel_path, &serde_json::json!({}))
            .await
        {
            tracing::warn!(
                task_id = %path.task_id,
                instance_id = %progress.instance_id,
                error = %err,
                "failed to forward suite install cancellation to agent"
            );
        }
    }

    let _ = delete_instance(&state.metadata_db, &progress.instance_id).await;
    update_install_progress(
        &state.metadata_db,
        &path.task_id,
        u32::try_from(progress.progress_percent).unwrap_or(100),
        "canceled",
        "canceled",
        progress.current_image,
        true,
        None,
    )
    .await;
    let canceled = fetch_install_task(&state.metadata_db, &path.task_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    finish_suite_platform_log(
        &state,
        &ctx,
        SuitePlatformLog {
            event: "suite_install_canceled",
            method: "POST",
            request_path: "/api/v1/suite-install-tasks/{task_id}/cancel",
            target_type: "suite_instance",
            target_id: &canceled.instance_id,
            metadata: serde_json::json!({
                "task_id": path.task_id,
                "instance_id": canceled.instance_id,
                "node_id": canceled.node_id,
                "status": canceled.status,
            }),
            error: None,
        },
    );
    Ok(ApiResponse::success_with_raw("Suite install canceled", canceled).into_response())
}

/// 在后台执行套件安装，并把最终状态写入进度会话。
async fn run_suite_install_task(
    state: Arc<AppState>,
    task_id: String,
    node_id: String,
    instance_id: String,
    request: AgentSuiteInstallRequest,
    audit_ctx: SuiteAuditContext,
) {
    if is_install_cancel_requested(&state.metadata_db, &task_id).await {
        let _ = delete_instance(&state.metadata_db, &instance_id).await;
        update_install_progress(
            &state.metadata_db,
            &task_id,
            100,
            "canceled",
            "canceled",
            None,
            true,
            None,
        )
        .await;
        finish_suite_install_task_log(
            &state,
            &audit_ctx,
            SuiteInstallTaskLog {
                event: "suite_install_canceled",
                task_id: &task_id,
                node_id: &node_id,
                instance_id: &instance_id,
                request: &request,
                error: None,
            },
        );
        return;
    }
    update_install_progress(
        &state.metadata_db,
        &task_id,
        5,
        "running",
        "prepare",
        None,
        false,
        None,
    )
    .await;

    if let Err(err) = prewarm_suite_images(Arc::clone(&state), &task_id, &node_id, &request).await {
        let detail = err.to_string();
        let canceled = is_install_cancel_requested(&state.metadata_db, &task_id).await;
        let _ = delete_instance(&state.metadata_db, &instance_id).await;
        update_install_progress(
            &state.metadata_db,
            &task_id,
            if canceled { 100 } else { 40 },
            if canceled { "canceled" } else { "failed" },
            if canceled { "canceled" } else { "failed" },
            None,
            true,
            (!canceled).then_some(detail.clone()),
        )
        .await;
        finish_suite_install_task_log(
            &state,
            &audit_ctx,
            SuiteInstallTaskLog {
                event: if canceled {
                    "suite_install_canceled"
                } else {
                    "suite_install_failed"
                },
                task_id: &task_id,
                node_id: &node_id,
                instance_id: &instance_id,
                request: &request,
                error: (!canceled).then_some(detail.as_str()),
            },
        );
        return;
    }
    update_install_progress(
        &state.metadata_db,
        &task_id,
        40,
        "running",
        "start_services",
        None,
        false,
        None,
    )
    .await;

    let client = match NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&node_id)).await
    {
        Ok(client) => client,
        Err(err) => {
            let detail = err.to_string();
            let _ = delete_instance(&state.metadata_db, &instance_id).await;
            update_install_progress(
                &state.metadata_db,
                &task_id,
                100,
                "failed",
                "failed",
                None,
                true,
                Some(detail.clone()),
            )
            .await;
            finish_suite_install_task_log(
                &state,
                &audit_ctx,
                SuiteInstallTaskLog {
                    event: "suite_install_failed",
                    task_id: &task_id,
                    node_id: &node_id,
                    instance_id: &instance_id,
                    request: &request,
                    error: Some(&detail),
                },
            );
            return;
        }
    };

    let agent_response = match client
        .post_json_with_operation_context::<serde_json::Value, _>(
            "/api/v1/agent/docker/suites/install",
            &request,
            &audit_ctx.agent_operation_context(),
        )
        .await
    {
        Ok(value) => value,
        Err(err) => {
            let detail = err.to_string();
            compensate_failed_install(&client, &request, &instance_id, &audit_ctx).await;
            let _ = delete_instance(&state.metadata_db, &instance_id).await;
            if is_install_cancel_requested(&state.metadata_db, &task_id).await
                || is_install_canceled_error(&detail)
            {
                update_install_progress(
                    &state.metadata_db,
                    &task_id,
                    100,
                    "canceled",
                    "canceled",
                    None,
                    true,
                    None,
                )
                .await;
                finish_suite_install_task_log(
                    &state,
                    &audit_ctx,
                    SuiteInstallTaskLog {
                        event: "suite_install_canceled",
                        task_id: &task_id,
                        node_id: &node_id,
                        instance_id: &instance_id,
                        request: &request,
                        error: None,
                    },
                );
            } else {
                update_install_progress(
                    &state.metadata_db,
                    &task_id,
                    100,
                    "failed",
                    "failed",
                    None,
                    true,
                    Some(detail),
                )
                .await;
                let progress = fetch_install_task(&state.metadata_db, &task_id)
                    .await
                    .ok()
                    .flatten();
                finish_suite_install_task_log(
                    &state,
                    &audit_ctx,
                    SuiteInstallTaskLog {
                        event: "suite_install_failed",
                        task_id: &task_id,
                        node_id: &node_id,
                        instance_id: &instance_id,
                        request: &request,
                        error: progress.as_ref().and_then(|item| item.error.as_deref()),
                    },
                );
            }
            return;
        }
    };
    if is_install_cancel_requested(&state.metadata_db, &task_id).await {
        let _ = delete_instance(&state.metadata_db, &instance_id).await;
        update_install_progress(
            &state.metadata_db,
            &task_id,
            100,
            "canceled",
            "canceled",
            None,
            true,
            None,
        )
        .await;
        finish_suite_install_task_log(
            &state,
            &audit_ctx,
            SuiteInstallTaskLog {
                event: "suite_install_canceled",
                task_id: &task_id,
                node_id: &node_id,
                instance_id: &instance_id,
                request: &request,
                error: None,
            },
        );
        return;
    }
    if let Err(err) = ensure_agent_success(&agent_response) {
        let _ = delete_instance(&state.metadata_db, &instance_id).await;
        if is_install_canceled_error(&err) {
            update_install_progress(
                &state.metadata_db,
                &task_id,
                100,
                "canceled",
                "canceled",
                None,
                true,
                None,
            )
            .await;
            finish_suite_install_task_log(
                &state,
                &audit_ctx,
                SuiteInstallTaskLog {
                    event: "suite_install_canceled",
                    task_id: &task_id,
                    node_id: &node_id,
                    instance_id: &instance_id,
                    request: &request,
                    error: None,
                },
            );
        } else {
            update_install_progress(
                &state.metadata_db,
                &task_id,
                100,
                "failed",
                "failed",
                None,
                true,
                Some(err.clone()),
            )
            .await;
            finish_suite_install_task_log(
                &state,
                &audit_ctx,
                SuiteInstallTaskLog {
                    event: "suite_install_failed",
                    task_id: &task_id,
                    node_id: &node_id,
                    instance_id: &instance_id,
                    request: &request,
                    error: Some(&err),
                },
            );
        }
        return;
    }

    if let Err(err) =
        update_instance_status(&state.metadata_db, &instance_id, "installed", None).await
    {
        let detail = err.to_string();
        update_install_progress(
            &state.metadata_db,
            &task_id,
            100,
            "failed",
            "failed",
            None,
            true,
            Some(detail.clone()),
        )
        .await;
        finish_suite_install_task_log(
            &state,
            &audit_ctx,
            SuiteInstallTaskLog {
                event: "suite_install_failed",
                task_id: &task_id,
                node_id: &node_id,
                instance_id: &instance_id,
                request: &request,
                error: Some(&detail),
            },
        );
        return;
    }

    update_install_progress(
        &state.metadata_db,
        &task_id,
        100,
        "success",
        "completed",
        None,
        true,
        None,
    )
    .await;
    finish_suite_install_task_log(
        &state,
        &audit_ctx,
        SuiteInstallTaskLog {
            event: "suite_install_completed",
            task_id: &task_id,
            node_id: &node_id,
            instance_id: &instance_id,
            request: &request,
            error: None,
        },
    );
}

/// 提取套件声明的静态镜像并通过主控统一镜像任务预热目标节点。
async fn prewarm_suite_images(
    state: Arc<AppState>,
    suite_task_id: &str,
    node_id: &str,
    request: &AgentSuiteInstallRequest,
) -> anyhow::Result<()> {
    let images = extract_suite_images(request)?;
    let count = images.len().max(1);
    for (index, image_ref) in images.iter().enumerate() {
        if is_install_cancel_requested(&state.metadata_db, suite_task_id).await {
            anyhow::bail!("suite install canceled");
        }
        let image_task = state.image_acquisition.start(
            Arc::clone(&state),
            node_id.to_string(),
            image_ref.clone(),
            None,
        );
        loop {
            if is_install_cancel_requested(&state.metadata_db, suite_task_id).await {
                state.image_acquisition.cancel(&image_task.task_id);
                anyhow::bail!("suite install canceled");
            }
            let progress = state
                .image_acquisition
                .get(&image_task.task_id)
                .ok_or_else(|| anyhow::anyhow!("image task disappeared"))?;
            let image_progress = 5
                + (((index * 35) + usize::from(progress.progress_percent) * 35 / 100) / count)
                    as u32;
            let step = match progress.stage {
                crate::services::image_acquisition::ImageStage::Checking => "check_image",
                crate::services::image_acquisition::ImageStage::Exporting
                | crate::services::image_acquisition::ImageStage::Uploading
                | crate::services::image_acquisition::ImageStage::Loading => {
                    "transfer_controller_image"
                }
                crate::services::image_acquisition::ImageStage::Pulling => "pull_registry_image",
            };
            update_install_progress(
                &state.metadata_db,
                suite_task_id,
                image_progress.min(40),
                "running",
                step,
                Some(image_ref.clone()),
                false,
                None,
            )
            .await;
            match progress.status {
                crate::services::image_acquisition::ImageTaskStatus::Success => break,
                crate::services::image_acquisition::ImageTaskStatus::Failed => {
                    anyhow::bail!(progress.status_text)
                }
                crate::services::image_acquisition::ImageTaskStatus::Cancelled => {
                    anyhow::bail!("suite install canceled")
                }
                _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            }
        }
    }
    Ok(())
}

fn extract_suite_images(request: &AgentSuiteInstallRequest) -> anyhow::Result<BTreeSet<String>> {
    let mut images = request
        .runtime_images
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.contains("${"))
        .collect::<BTreeSet<_>>();
    let compose = request
        .files
        .iter()
        .find(|file| file.path == request.compose_file)
        .ok_or_else(|| anyhow::anyhow!("compose file is missing"))?;
    let bytes = STANDARD.decode(&compose.content_base64)?;
    let document: serde_yaml::Value = serde_yaml::from_slice(&bytes)?;
    if let Some(services) = document
        .get("services")
        .and_then(serde_yaml::Value::as_mapping)
    {
        for service in services.values() {
            if let Some(image) = service.get("image").and_then(serde_yaml::Value::as_str) {
                let image = image.trim();
                if !image.is_empty() && !image.contains("${") {
                    images.insert(image.to_string());
                }
            }
        }
    }
    Ok(images)
}

struct SuiteInstallTaskLog<'a> {
    event: &'a str,
    task_id: &'a str,
    node_id: &'a str,
    instance_id: &'a str,
    request: &'a AgentSuiteInstallRequest,
    error: Option<&'a str>,
}

/// 写入主控侧套件安装后台任务终态日志。
fn finish_suite_install_task_log(
    state: &Arc<AppState>,
    ctx: &SuiteAuditContext,
    log: SuiteInstallTaskLog<'_>,
) {
    finish_suite_platform_log(
        state,
        ctx,
        SuitePlatformLog {
            event: log.event,
            method: "POST",
            request_path: "/api/v1/suites/{suite_id}/install",
            target_type: "suite_instance",
            target_id: log.instance_id,
            metadata: serde_json::json!({
                "suite_id": log.request.suite_id,
                "version": log.request.version,
                "instance_id": log.instance_id,
                "node_id": log.node_id,
                "compose_project_name": log.request.compose_project_name,
                "task_id": log.task_id,
            }),
            error: log.error,
        },
    );
}

/// Agent 安装请求发生传输异常时，尽力清理可能已经写入的远端安装内容。
async fn compensate_failed_install(
    client: &NodeRuntimeClient,
    request: &AgentSuiteInstallRequest,
    instance_id: &str,
    context: &SuiteAuditContext,
) {
    let action = AgentSuiteActionRequest {
        suite_id: request.suite_id.clone(),
        suite_instance_id: instance_id.to_string(),
        compose_project_name: request.compose_project_name.clone(),
        remove_data: false,
    };
    let path = format!(
        "/api/v1/agent/docker/suite/{}/uninstall",
        request.compose_project_name
    );
    if let Err(err) = client
        .post_json_with_operation_context::<serde_json::Value, _>(
            &path,
            &action,
            &context.agent_operation_context(),
        )
        .await
    {
        tracing::warn!(
            instance_id,
            error = %err,
            "failed to compensate interrupted suite installation"
        );
    }
}

/// 判断安装任务是否已经收到取消请求。
async fn is_install_cancel_requested(pool: &DbPool, task_id: &str) -> bool {
    install_task_cancel_requested(pool, task_id)
        .await
        .unwrap_or(false)
}

/// 判断 Agent 返回的安装错误是否由用户取消触发。
fn is_install_canceled_error(message: &str) -> bool {
    message.contains("suite install canceled")
}

/// 更新持久化安装任务进度，并保持百分比单调递增。
#[allow(clippy::too_many_arguments)]
async fn update_install_progress(
    pool: &DbPool,
    task_id: &str,
    progress_percent: u32,
    status: &str,
    current_step: &str,
    current_image: Option<String>,
    is_finished: bool,
    error: Option<String>,
) {
    if let Err(err) = update_install_task(
        pool,
        task_id,
        progress_percent,
        status,
        current_step,
        current_image,
        is_finished,
        error,
    )
    .await
    {
        tracing::error!(task_id, error = %err, "failed to persist suite install progress");
    }
}

/// 持久化从 Agent 合并得到的完整进度。
async fn persist_install_progress(
    pool: &DbPool,
    progress: &SuiteInstallTaskRecord,
) -> sqlx::Result<()> {
    update_install_task(
        pool,
        &progress.task_id,
        u32::try_from(progress.progress_percent).unwrap_or(100),
        &progress.status,
        &progress.current_step,
        progress.current_image.clone(),
        progress.is_finished,
        progress.error.clone(),
    )
    .await
}

/// 将 Agent 返回的安装进度合并到主控任务进度中。
fn parse_agent_install_progress(
    response: &serde_json::Value,
    base: &SuiteInstallTaskRecord,
) -> Option<SuiteInstallTaskRecord> {
    let data = response.get("data")?;
    let mut progress = base.clone();
    let agent_percent = data
        .get("progressPercent")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
        .min(100);
    progress.progress_percent = progress
        .progress_percent
        .max(i64::from(40 + agent_percent * 60 / 100));
    progress.status = data
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&progress.status)
        .to_string();
    progress.current_step = data
        .get("currentStep")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&progress.current_step)
        .to_string();
    progress.current_image = data
        .get("currentImage")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    progress.is_finished = data
        .get("isFinished")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(progress.is_finished);
    progress.error = data
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    progress.cancel_requested = data
        .get("cancelRequested")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(progress.cancel_requested);
    Some(progress)
}

/// 判断持久化任务是否已经超过 Agent 恢复宽限期。
fn is_install_recovery_expired(updated_at: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(updated_at)
        .map(|updated_at| {
            chrono::Utc::now()
                .signed_duration_since(updated_at)
                .num_seconds()
                >= 120
        })
        .unwrap_or(false)
}

/// 将 Agent 已结束的安装结果同步回实例状态。
async fn reconcile_recovered_install(
    pool: &DbPool,
    progress: &SuiteInstallTaskRecord,
) -> sqlx::Result<()> {
    if !progress.is_finished {
        return Ok(());
    }
    match progress.status.as_str() {
        "success" => update_instance_status(pool, &progress.instance_id, "installed", None).await,
        "canceled" => delete_instance(pool, &progress.instance_id).await,
        _ => {
            update_instance_status(
                pool,
                &progress.instance_id,
                "error",
                progress.error.as_deref(),
            )
            .await
        }
    }
}

/// 启用套件实例并注册应用入口。
pub async fn enable_instance(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    Path(instance_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let ctx = SuiteAuditContext::from_request(&admin, &headers, conn);
    let instance = fetch_instance(&state.metadata_db, &instance_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("suite instance not found: {instance_id}")))?;
    validate_instance_transition(&instance, "enable")?;
    update_instance_status(&state.metadata_db, &instance_id, "enabling", None).await?;

    let client =
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&instance.node_id)).await?;
    let request = AgentSuiteActionRequest {
        suite_id: instance.suite_id.clone(),
        suite_instance_id: instance.instance_id.clone(),
        compose_project_name: instance.compose_project_name.clone(),
        remove_data: false,
    };
    let result = async {
        let agent_response = match client
            .post_json_with_operation_context::<serde_json::Value, _>(
                &format!(
                    "/api/v1/agent/docker/suite/{}/enable",
                    instance.compose_project_name
                ),
                &request,
                &ctx.agent_operation_context(),
            )
            .await
        {
            Ok(value) => value,
            Err(err) => {
                let detail = err.to_string();
                update_instance_status(&state.metadata_db, &instance_id, "error", Some(&detail))
                    .await?;
                return Err(ApiError::Internal(detail));
            }
        };
        if let Err(err) = ensure_agent_success(&agent_response) {
            update_instance_status(&state.metadata_db, &instance_id, "error", Some(&err)).await?;
            return Err(ApiError::Internal(err));
        }

        let (manifest, _) = fetch_catalog_payload(&state.metadata_db, &instance.suite_id)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(format!("suite not found: {}", instance.suite_id))
            })?;
        let locale = resolve_request_locale(&headers);
        let entries = build_app_entry_records(
            &instance,
            &manifest.metadata.icon,
            &manifest.app_entries,
            &manifest,
            locale.as_deref(),
        );
        replace_instance_app_entries(&state.metadata_db, &instance_id, &entries).await?;
        update_instance_status(&state.metadata_db, &instance_id, "enabled", None).await?;
        Ok::<_, ApiError>(fetch_instance(&state.metadata_db, &instance_id).await?)
    }
    .await;
    let error = result.as_ref().err().map(ToString::to_string);
    finish_suite_platform_log(
        &state,
        &ctx,
        SuitePlatformLog {
            event: "suite_enable",
            method: "POST",
            request_path: "/api/v1/suite-instances/{instance_id}/enable",
            target_type: "suite_instance",
            target_id: &instance_id,
            metadata: suite_instance_log_metadata(&instance),
            error: error.as_deref(),
        },
    );
    let enabled = result?;
    Ok(ApiResponse::success_with_raw("Suite enabled", enabled).into_response())
}

/// 停用套件实例并隐藏应用入口。
pub async fn disable_instance(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
) -> ApiResult<Response> {
    let ctx = SuiteAuditContext::from_request(&admin, &headers, conn);
    let instance = fetch_instance(&state.metadata_db, &instance_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("suite instance not found: {instance_id}")))?;
    validate_instance_transition(&instance, "disable")?;
    update_instance_status(&state.metadata_db, &instance_id, "disabling", None).await?;

    let client =
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&instance.node_id)).await?;
    let request = AgentSuiteActionRequest {
        suite_id: instance.suite_id.clone(),
        suite_instance_id: instance.instance_id.clone(),
        compose_project_name: instance.compose_project_name.clone(),
        remove_data: false,
    };
    let result = async {
        let agent_response = match client
            .post_json_with_operation_context::<serde_json::Value, _>(
                &format!(
                    "/api/v1/agent/docker/suite/{}/disable",
                    instance.compose_project_name
                ),
                &request,
                &ctx.agent_operation_context(),
            )
            .await
        {
            Ok(value) => value,
            Err(err) => {
                let detail = err.to_string();
                update_instance_status(&state.metadata_db, &instance_id, "error", Some(&detail))
                    .await?;
                return Err(ApiError::Internal(detail));
            }
        };
        if let Err(err) = ensure_agent_success(&agent_response) {
            update_instance_status(&state.metadata_db, &instance_id, "error", Some(&err)).await?;
            return Err(ApiError::Internal(err));
        }

        let (manifest, _) = fetch_catalog_payload(&state.metadata_db, &instance.suite_id)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(format!("suite not found: {}", instance.suite_id))
            })?;
        let app_ids = build_suite_app_ids(&instance_id, &manifest.app_entries);
        hide_suite_desktop_apps(&state.metadata_db, &instance.node_id, &app_ids).await?;
        delete_instance_app_entries(&state.metadata_db, &instance_id).await?;
        update_instance_status(&state.metadata_db, &instance_id, "disabled", None).await?;
        Ok::<_, ApiError>(fetch_instance(&state.metadata_db, &instance_id).await?)
    }
    .await;
    let error = result.as_ref().err().map(ToString::to_string);
    finish_suite_platform_log(
        &state,
        &ctx,
        SuitePlatformLog {
            event: "suite_disable",
            method: "POST",
            request_path: "/api/v1/suite-instances/{instance_id}/disable",
            target_type: "suite_instance",
            target_id: &instance_id,
            metadata: suite_instance_log_metadata(&instance),
            error: error.as_deref(),
        },
    );
    let disabled = result?;
    Ok(ApiResponse::success_with_raw("Suite disabled", disabled).into_response())
}

/// 卸载套件实例。
pub async fn uninstall_instance(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(payload): Json<UninstallSuiteRequest>,
) -> ApiResult<Response> {
    let ctx = SuiteAuditContext::from_request(&admin, &headers, conn);
    let instance = fetch_instance(&state.metadata_db, &instance_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("suite instance not found: {instance_id}")))?;
    validate_instance_transition(&instance, "uninstall")?;
    update_instance_status(&state.metadata_db, &instance_id, "uninstalling", None).await?;
    let client =
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&instance.node_id)).await?;
    let request = AgentSuiteActionRequest {
        suite_id: instance.suite_id.clone(),
        suite_instance_id: instance.instance_id.clone(),
        compose_project_name: instance.compose_project_name.clone(),
        remove_data: payload.remove_data,
    };
    let result = async {
        client
            .post_json_with_operation_context::<serde_json::Value, _>(
                &format!(
                    "/api/v1/agent/docker/suite/{}/disable",
                    instance.compose_project_name
                ),
                &request,
                &ctx.agent_operation_context(),
            )
            .await
            .ok();
        let agent_response = match client
            .post_json_with_operation_context::<serde_json::Value, _>(
                &format!(
                    "/api/v1/agent/docker/suite/{}/uninstall",
                    instance.compose_project_name
                ),
                &request,
                &ctx.agent_operation_context(),
            )
            .await
        {
            Ok(value) => value,
            Err(err) => {
                let detail = err.to_string();
                update_instance_status(&state.metadata_db, &instance_id, "error", Some(&detail))
                    .await?;
                return Err(ApiError::Internal(detail));
            }
        };
        if let Err(err) = ensure_agent_success(&agent_response) {
            update_instance_status(&state.metadata_db, &instance_id, "error", Some(&err)).await?;
            return Err(ApiError::Internal(err));
        }
        if let Some((manifest, _)) =
            fetch_catalog_payload(&state.metadata_db, &instance.suite_id).await?
        {
            let app_ids = build_suite_app_ids(&instance_id, &manifest.app_entries);
            delete_desktop_apps(&state.metadata_db, &instance.node_id, &app_ids).await?;
        }
        delete_instance_app_entries(&state.metadata_db, &instance_id).await?;
        delete_instance(&state.metadata_db, &instance_id).await?;
        Ok::<(), ApiError>(())
    }
    .await;
    let mut metadata = suite_instance_log_metadata(&instance);
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "remove_data".to_string(),
            serde_json::json!(payload.remove_data),
        );
    }
    let error = result.as_ref().err().map(ToString::to_string);
    finish_suite_platform_log(
        &state,
        &ctx,
        SuitePlatformLog {
            event: "suite_uninstall",
            method: "POST",
            request_path: "/api/v1/suite-instances/{instance_id}/uninstall",
            target_type: "suite_instance",
            target_id: &instance_id,
            metadata,
            error: error.as_deref(),
        },
    );
    result?;
    Ok(ApiResponse::ok("Suite uninstalled").into_response())
}

/// 读取套件目录包内的静态资源。
pub async fn read_catalog_asset(
    State(state): State<Arc<AppState>>,
    Path(path): Path<SuiteCatalogAssetPath>,
) -> ApiResult<Response> {
    let (_, package) = fetch_catalog_payload(&state.metadata_db, &path.suite_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("suite not found: {}", path.suite_id)))?;
    let asset_path = normalize_asset_path(&format!("assets/{}", path.asset_path))?;
    respond_suite_asset(&package, &asset_path)
}

/// 读取套件交付包内的静态资源。
pub async fn read_suite_asset(
    State(state): State<Arc<AppState>>,
    Path(path): Path<SuiteAssetPath>,
) -> ApiResult<Response> {
    let instance = fetch_instance(&state.metadata_db, &path.instance_id)
        .await?
        .ok_or_else(|| {
            ApiError::BadRequest(format!("suite instance not found: {}", path.instance_id))
        })?;
    let (_, package) = fetch_catalog_payload(&state.metadata_db, &instance.suite_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("suite not found: {}", instance.suite_id)))?;
    let asset_path = normalize_asset_path(&format!("assets/{}", path.asset_path))?;
    respond_suite_asset(&package, &asset_path)
}

fn respond_suite_asset(package: &SuitePackageSnapshot, asset_path: &str) -> ApiResult<Response> {
    let file = package
        .files
        .iter()
        .find(|file| file.path == asset_path)
        .ok_or_else(|| ApiError::BadRequest(format!("suite asset not found: {asset_path}")))?;
    let content_type = content_type_for_asset(asset_path);
    let content = decode_package_file(file)?;
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(content))
        .map_err(|err| ApiError::Internal(format!("failed to build suite asset response: {err}")))
}

/// 将套件入口请求代理到目标 Agent。
pub async fn proxy_suite_entry(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    OriginalUri(uri): OriginalUri,
    Path(path): Path<SuiteProxyPath>,
    request: Request,
) -> ApiResult<Response> {
    let (request_parts, body) = request.into_parts();
    let method = request_parts.method;
    let headers = request_parts.headers;
    let audit_context = SuiteAuditContext::from_request(&admin, &headers, conn);
    let instance = fetch_instance(&state.metadata_db, &path.instance_id)
        .await?
        .ok_or_else(|| {
            ApiError::BadRequest(format!("suite instance not found: {}", path.instance_id))
        })?;
    if path.entry_id.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "suite app entry id is required".to_string(),
        ));
    }
    if instance.status != "enabled" {
        return Err(ApiError::BadRequest(
            "suite instance is not enabled".to_string(),
        ));
    }
    let client =
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&instance.node_id)).await?;
    let raw_path = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let public_prefix = format!("/api/v1/suite-instances/{}", path.instance_id);
    let agent_prefix = format!(
        "/api/v1/agent/docker/suite/{}",
        instance.compose_project_name
    );
    let agent_path = raw_path.replacen(&public_prefix, &agent_prefix, 1);
    client
        .forward_streaming_with_operation_context(
            method,
            &agent_path,
            headers,
            body,
            &audit_context.agent_operation_context(),
        )
        .await
}

async fn read_suite_upload(multipart: &mut Multipart) -> ApiResult<Vec<u8>> {
    while let Some(field) = multipart.next_field().await? {
        if field.name() != Some("file") {
            continue;
        }
        let file_name = field.file_name().unwrap_or_default().to_string();
        if !file_name.ends_with(SUITE_PACKAGE_EXTENSION) {
            return Err(ApiError::BadRequest(
                "suite package must be .vsp".to_string(),
            ));
        }
        let bytes = field.bytes().await?;
        if bytes.len() > MAX_SUITE_PACKAGE_BYTES {
            return Err(ApiError::BadRequest(
                "suite package is too large".to_string(),
            ));
        }
        return Ok(bytes.to_vec());
    }
    Err(ApiError::BadRequest(
        "missing suite package file".to_string(),
    ))
}

fn parse_suite_package(bytes: &[u8]) -> ApiResult<(SuiteManifest, SuitePackageSnapshot)> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut files = Vec::new();
    let mut unpacked_bytes = 0usize;
    for entry in archive
        .entries()
        .map_err(|err| ApiError::BadRequest(format!("invalid suite archive: {err}")))?
    {
        let mut entry =
            entry.map_err(|err| ApiError::BadRequest(format!("invalid suite entry: {err}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|err| ApiError::BadRequest(format!("invalid suite path: {err}")))?;
        let path = normalize_package_path(path.as_ref())?;
        let declared_size = usize::try_from(entry.header().size().map_err(|err| {
            ApiError::BadRequest(format!("invalid suite entry size: {path}: {err}"))
        })?)
        .map_err(|_| ApiError::BadRequest(format!("suite entry is too large: {path}")))?;
        unpacked_bytes = unpacked_bytes
            .checked_add(declared_size)
            .ok_or_else(|| ApiError::BadRequest("suite unpacked size exceeds limit".to_string()))?;
        if unpacked_bytes > MAX_SUITE_UNPACKED_BYTES {
            return Err(ApiError::BadRequest(
                "suite unpacked size exceeds limit".to_string(),
            ));
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        files.push(SuitePackageFile {
            path,
            content_base64: STANDARD.encode(content),
        });
    }

    strip_common_package_root(&mut files);

    let manifest_text = files
        .iter()
        .find(|file| file.path == "suite.yaml")
        .ok_or_else(|| ApiError::BadRequest("suite.yaml is missing".to_string()))
        .and_then(decode_package_text)?;
    let manifest = serde_yaml::from_str::<SuiteManifest>(&manifest_text)
        .map_err(|err| ApiError::BadRequest(format!("invalid suite.yaml: {err}")))?;
    let compose_file = manifest.runtime.compose_file.trim();
    let compose = files
        .iter()
        .find(|file| file.path == compose_file)
        .ok_or_else(|| ApiError::BadRequest(format!("compose file is missing: {compose_file}")))?;
    decode_package_text(compose)?;
    validate_manifest(&manifest, &files)?;
    Ok((manifest, SuitePackageSnapshot { files }))
}

fn normalize_package_path(path: &std::path::Path) -> ApiResult<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => {
                parts.push(value.to_string_lossy().to_string());
            }
            std::path::Component::CurDir => {}
            _ => {
                return Err(ApiError::BadRequest(
                    "suite package contains unsafe path".to_string(),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(ApiError::BadRequest(
            "suite package path is empty".to_string(),
        ));
    }
    Ok(parts.join("/"))
}

fn strip_common_package_root(files: &mut [SuitePackageFile]) {
    if files.iter().any(|file| file.path == "suite.yaml") {
        return;
    }
    let Some(first_root) = files
        .first()
        .and_then(|file| file.path.split('/').next())
        .map(str::to_string)
    else {
        return;
    };
    if first_root.is_empty()
        || !files
            .iter()
            .all(|file| file.path.split('/').next() == Some(first_root.as_str()))
    {
        return;
    }
    for file in files {
        if let Some(stripped) = file.path.strip_prefix(&format!("{first_root}/")) {
            file.path = stripped.to_string();
        }
    }
}

fn validate_manifest(manifest: &SuiteManifest, files: &[SuitePackageFile]) -> ApiResult<()> {
    if manifest.api_version != "vust.io/v1alpha1" {
        return Err(ApiError::BadRequest(
            "suite apiVersion must be vust.io/v1alpha1".to_string(),
        ));
    }
    if manifest.kind != "ComposeSuite" {
        return Err(ApiError::BadRequest(
            "suite kind must be ComposeSuite".to_string(),
        ));
    }
    validate_id("suite_id", &manifest.metadata.suite_id)?;
    validate_slug(&manifest.metadata.slug)?;
    if manifest.metadata.version.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "suite version is required".to_string(),
        ));
    }
    if manifest.metadata.name.trim().is_empty() {
        return Err(ApiError::BadRequest("suite name is required".to_string()));
    }
    validate_suite_icon(&manifest.metadata.icon, files, true)?;
    if manifest.runtime.runtime_type != "compose" {
        return Err(ApiError::BadRequest(
            "suite runtime type must be compose".to_string(),
        ));
    }
    validate_min_vust_version(&manifest.metadata.min_vust_version)?;
    validate_platform_contract_version(manifest.compatibility.platform_contract_version)?;
    if manifest.runtime.compose_file != "compose.yaml" {
        return Err(ApiError::BadRequest(
            "suite compose file must be compose.yaml".to_string(),
        ));
    }
    validate_runtime_images(&manifest.runtime.images)?;
    validate_agent_access(manifest, files)?;
    if manifest.app_entries.is_empty() {
        return Err(ApiError::BadRequest(
            "suite must declare at least one app entry".to_string(),
        ));
    }
    for entry in &manifest.app_entries {
        validate_id("app entry id", &entry.id)?;
        if !matches!(entry.entry_type.as_str(), "proxied_web" | "compose_detail") {
            return Err(ApiError::BadRequest(format!(
                "unsupported suite app entry type: {}",
                entry.entry_type
            )));
        }
        if entry.entry_type == "proxied_web" && (entry.service.is_none() || entry.port.is_none()) {
            return Err(ApiError::BadRequest(
                "proxied_web entry must declare service and port".to_string(),
            ));
        }
        if !entry.icon.trim().is_empty() {
            validate_suite_icon(&entry.icon, files, true)?;
        }
    }
    validate_i18n(manifest)?;
    Ok(())
}

fn validate_min_vust_version(value: &str) -> ApiResult<()> {
    let minimum = semver::Version::parse(value).map_err(|err| {
        ApiError::BadRequest(format!("metadata.minVustVersion is invalid: {err}"))
    })?;
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|err| ApiError::Internal(format!("current VUST version is invalid: {err}")))?;
    if current < minimum {
        return Err(ApiError::BadRequest(format!(
            "suite requires VUST {} or newer, current version is {}",
            minimum, current
        )));
    }
    Ok(())
}

fn validate_platform_contract_version(value: u32) -> ApiResult<()> {
    if value == 0 {
        return Err(ApiError::BadRequest(
            "compatibility.platformContractVersion must be a positive integer".to_string(),
        ));
    }
    if !crate::services::upgrades::current_supported_suite_contract_versions()?.contains(&value) {
        return Err(ApiError::BadRequest(format!(
            "suite platform contract version {value} is not supported by this VUST release"
        )));
    }
    Ok(())
}

/// 校验套件 Agent 能力及其目标 Compose 服务。
fn validate_agent_access(manifest: &SuiteManifest, files: &[SuitePackageFile]) -> ApiResult<()> {
    let Some(access) = manifest.runtime.agent.as_ref() else {
        return Ok(());
    };
    if access.services.is_empty() {
        return Err(ApiError::BadRequest(
            "runtime.agent.services must not be empty".to_string(),
        ));
    }
    if access.capabilities.is_empty() {
        return Err(ApiError::BadRequest(
            "runtime.agent.capabilities must not be empty".to_string(),
        ));
    }
    const ALLOWED_CAPABILITIES: [&str; 3] = [
        "workloads.manage",
        "captures.manage",
        "operation-logs.write",
    ];
    let mut capabilities = BTreeSet::new();
    for capability in &access.capabilities {
        let capability = capability.trim();
        if !ALLOWED_CAPABILITIES.contains(&capability) {
            return Err(ApiError::BadRequest(format!(
                "unsupported runtime.agent capability: {capability}"
            )));
        }
        if !capabilities.insert(capability) {
            return Err(ApiError::BadRequest(format!(
                "duplicate runtime.agent capability: {capability}"
            )));
        }
    }

    let compose = files
        .iter()
        .find(|file| file.path == manifest.runtime.compose_file)
        .ok_or_else(|| ApiError::BadRequest("suite compose file is missing".to_string()))
        .and_then(decode_package_text)?;
    let compose: serde_yaml::Value = serde_yaml::from_str(&compose)
        .map_err(|err| ApiError::BadRequest(format!("invalid compose file: {err}")))?;
    let services = compose
        .get("services")
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or_else(|| ApiError::BadRequest("compose services are missing".to_string()))?;
    let mut declared_services = BTreeSet::new();
    for service in &access.services {
        let service = service.trim();
        validate_id("runtime.agent service", service)?;
        if !declared_services.insert(service) {
            return Err(ApiError::BadRequest(format!(
                "duplicate runtime.agent service: {service}"
            )));
        }
        if !services.contains_key(serde_yaml::Value::String(service.to_string())) {
            return Err(ApiError::BadRequest(format!(
                "runtime.agent service does not exist in compose: {service}"
            )));
        }
    }
    Ok(())
}

fn normalize_runtime_images(images: &[String]) -> Vec<String> {
    images
        .iter()
        .map(|image| image.trim())
        .filter(|image| !image.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn validate_runtime_images(images: &[String]) -> ApiResult<()> {
    for image in images {
        if image.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "suite runtime image must not be empty".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_suite_icon(
    icon_path: &str,
    files: &[SuitePackageFile],
    require_png_dimensions: bool,
) -> ApiResult<()> {
    let icon_path = normalize_asset_path(icon_path.trim())?;
    if !icon_path.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '-' | '_')
    }) {
        return Err(ApiError::BadRequest(format!(
            "suite icon path contains unsupported characters: {icon_path}"
        )));
    }
    let extension = std::path::Path::new(&icon_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| ApiError::BadRequest("suite icon extension is required".to_string()))?;
    if !matches!(extension.as_str(), "png" | "webp" | "svg") {
        return Err(ApiError::BadRequest(format!(
            "unsupported suite icon format: {icon_path}"
        )));
    }
    let file = files
        .iter()
        .find(|file| file.path == icon_path)
        .ok_or_else(|| ApiError::BadRequest(format!("suite icon is missing: {icon_path}")))?;
    let content = decode_package_file(file)?;
    if extension == "png" && require_png_dimensions {
        validate_png_icon(&content, &icon_path)?;
    }
    Ok(())
}

fn validate_png_icon(content: &[u8], path: &str) -> ApiResult<()> {
    let decoder = png::Decoder::new(Cursor::new(content));
    let mut reader = decoder.read_info().map_err(|err| {
        ApiError::BadRequest(format!("suite icon is not a valid PNG: {path}: {err}"))
    })?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).map_err(|err| {
        ApiError::BadRequest(format!("suite icon is not a valid PNG: {path}: {err}"))
    })?;
    let width = info.width;
    let height = info.height;
    if width != height {
        return Err(ApiError::BadRequest(format!(
            "suite PNG icon must be square: {path}"
        )));
    }
    if width < MIN_SUITE_ICON_SIZE {
        return Err(ApiError::BadRequest(format!(
            "suite PNG icon must be at least {MIN_SUITE_ICON_SIZE}x{MIN_SUITE_ICON_SIZE}: {path}"
        )));
    }
    Ok(())
}

fn decode_package_file(file: &SuitePackageFile) -> ApiResult<Vec<u8>> {
    STANDARD.decode(&file.content_base64).map_err(|err| {
        ApiError::BadRequest(format!(
            "suite file has invalid base64 content: {}: {err}",
            file.path
        ))
    })
}

fn decode_package_text(file: &SuitePackageFile) -> ApiResult<String> {
    let bytes = STANDARD.decode(&file.content_base64).map_err(|err| {
        ApiError::BadRequest(format!(
            "suite file has invalid base64 content: {}: {err}",
            file.path
        ))
    })?;
    String::from_utf8(bytes)
        .map_err(|_| ApiError::BadRequest(format!("suite file must be UTF-8 text: {}", file.path)))
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

fn validate_slug(slug: &str) -> ApiResult<()> {
    if slug.trim().is_empty()
        || slug.starts_with('-')
        || slug.ends_with('-')
        || !slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ApiError::BadRequest(
            "suite slug may only contain lowercase letters, digits, and hyphen".to_string(),
        ));
    }
    Ok(())
}

fn validate_i18n(manifest: &SuiteManifest) -> ApiResult<()> {
    let Some(i18n) = &manifest.i18n else {
        return Ok(());
    };
    validate_locale_tag("defaultLocale", &i18n.default_locale)?;
    // i18n 只能覆盖已声明应用入口，避免打包后出现无法生效的翻译配置。
    let entry_ids = manifest
        .app_entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for (locale, override_value) in &i18n.locales {
        validate_locale_tag("locale", locale)?;
        for entry_id in override_value.app_entries.keys() {
            if !entry_ids.contains(entry_id.as_str()) {
                return Err(ApiError::BadRequest(format!(
                    "suite i18n references unknown app entry: {entry_id}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_locale_tag(label: &str, value: &str) -> ApiResult<()> {
    if value.trim().is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::BadRequest(format!(
            "suite i18n {label} may only contain letters, digits, and hyphen"
        )));
    }
    Ok(())
}

fn build_compose_project_name(slug: &str) -> String {
    format!("vust-{slug}")
}

fn build_app_entry_records(
    instance: &SuiteInstanceSummary,
    suite_icon: &str,
    entries: &[SuiteAppEntryManifest],
    manifest: &SuiteManifest,
    locale: Option<&str>,
) -> Vec<SuiteAppEntryRecord> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let width = entry
                .window
                .as_ref()
                .and_then(|window| window.width)
                .unwrap_or(entry.default_width);
            let height = entry
                .window
                .as_ref()
                .and_then(|window| window.height)
                .unwrap_or(entry.default_height);
            let min_width = entry
                .window
                .as_ref()
                .and_then(|window| window.min_width)
                .unwrap_or(entry.min_width);
            let min_height = entry
                .window
                .as_ref()
                .and_then(|window| window.min_height)
                .unwrap_or(entry.min_height);
            let app_id = format!("suite:{}:{}", instance.instance_id, entry.id);
            let entry_target = if entry.entry_type == "compose_detail" {
                instance.compose_project_name.clone()
            } else {
                build_suite_proxy_target(&instance.instance_id, &entry.id, entry.path.as_deref())
            };
            SuiteAppEntryRecord {
                app_id,
                suite_instance_id: instance.instance_id.clone(),
                node_id: instance.node_id.clone(),
                app_entry_id: entry.id.clone(),
                title: manifest.localized_app_entry_title(entry, locale),
                icon: resolve_suite_icon(instance, suite_icon, entry),
                entry_type: entry.entry_type.clone(),
                entry_target,
                default_width: width,
                default_height: height,
                min_width,
                min_height,
                sort_order: 10_000 + index as i64,
                enabled: 1,
            }
        })
        .collect()
}

fn resolve_request_locale(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())?;
    // 当前只使用首选语言；质量权重不影响 VUST 目前的 zh-CN/en-US 双语选择。
    raw.split(',')
        .filter_map(|part| part.trim().split(';').next())
        .map(str::trim)
        .find(|part| !part.is_empty())
        .map(|part| part.replace('_', "-"))
}

fn build_suite_app_ids(instance_id: &str, entries: &[SuiteAppEntryManifest]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| format!("suite:{instance_id}:{}", entry.id))
        .collect()
}

fn resolve_suite_icon(
    instance: &SuiteInstanceSummary,
    suite_icon: &str,
    entry: &SuiteAppEntryManifest,
) -> String {
    let icon = if entry.icon.trim().is_empty() {
        suite_icon
    } else {
        entry.icon.trim()
    };
    build_suite_asset_url(&instance.instance_id, icon)
}

fn build_suite_asset_url(instance_id: &str, asset_path: &str) -> String {
    format!("/api/v1/suite-instances/{instance_id}/{asset_path}")
}

fn normalize_asset_path(asset_path: &str) -> ApiResult<String> {
    let path = normalize_package_path(std::path::Path::new(asset_path))?;
    if !path.starts_with("assets/") {
        return Err(ApiError::BadRequest(
            "suite asset path must be under assets/".to_string(),
        ));
    }
    Ok(path)
}

fn content_type_for_asset(asset_path: &str) -> &'static str {
    match std::path::Path::new(asset_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn build_suite_proxy_target(instance_id: &str, entry_id: &str, entry_path: Option<&str>) -> String {
    let base = format!("/api/v1/suite-instances/{instance_id}/proxy/{entry_id}");
    let Some(path) = entry_path.map(str::trim).filter(|value| !value.is_empty()) else {
        return format!("{base}/");
    };
    if path == "/" {
        return format!("{base}/");
    }
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// 校验实例生命周期操作是否允许从当前状态进入。
fn validate_instance_transition(instance: &SuiteInstanceSummary, action: &str) -> ApiResult<()> {
    let allowed = match action {
        "enable" => matches!(instance.status.as_str(), "installed" | "disabled" | "error"),
        "disable" => matches!(instance.status.as_str(), "enabled" | "error"),
        "uninstall" => matches!(
            instance.status.as_str(),
            "installed" | "enabled" | "disabled" | "error"
        ),
        _ => false,
    };
    if allowed {
        return Ok(());
    }
    Err(ApiError::BadRequest(format!(
        "cannot {action} suite instance {} while status is {}",
        instance.instance_id, instance.status
    )))
}

/// 安装前确认目标节点和 Docker 运行时可用。
async fn ensure_suite_node_ready(state: &Arc<AppState>, node_id: &str) -> ApiResult<()> {
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?;
    let response: ApiResponse<DockerStatusSummary> = client
        .get_json("/api/v1/agent/docker/status")
        .await
        .map_err(|err| {
            ApiError::BadRequest(format!(
                "failed to check docker status on node {node_id}: {err}"
            ))
        })?;
    if !response.success {
        let message = if response.message.trim().is_empty() {
            format!("docker status check failed on node {node_id}")
        } else {
            response.message
        };
        return Err(ApiError::BadRequest(message));
    }
    let status = response.data.ok_or_else(|| {
        ApiError::BadRequest(format!(
            "docker status check returned empty data on node {node_id}"
        ))
    })?;
    if !status.docker_available {
        return Err(ApiError::BadRequest(format!(
            "docker is not available on node {node_id}: {:?}",
            status.docker_status
        )));
    }
    Ok(())
}

fn ensure_agent_success(value: &serde_json::Value) -> Result<(), String> {
    if value
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(());
    }
    let message = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("agent operation failed");
    let detail = match value.get("data") {
        Some(serde_json::Value::String(detail)) => detail.clone(),
        Some(detail) if !detail.is_null() => detail.to_string(),
        _ => String::new(),
    };
    if detail.is_empty() || detail == message {
        Err(message.to_string())
    } else {
        Err(format!("{message}: {detail}"))
    }
}

#[cfg(test)]
mod suite_asset_tests {
    use super::*;
    use png::{BitDepth, ColorType, Encoder};

    fn png_image(width: u32, height: u32) -> Vec<u8> {
        let mut content = Vec::new();
        {
            let mut encoder = Encoder::new(&mut content, width, height);
            encoder.set_color(ColorType::Rgba);
            encoder.set_depth(BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let pixels = vec![0; width as usize * height as usize * 4];
            writer.write_image_data(&pixels).unwrap();
        }
        content
    }

    #[test]
    fn png_icon_requires_square_minimum_dimensions() {
        assert!(validate_png_icon(&png_image(256, 256), "assets/suite-icon.png").is_ok());
        assert!(validate_png_icon(&png_image(256, 128), "assets/suite-icon.png").is_err());
        assert!(validate_png_icon(&png_image(64, 64), "assets/suite-icon.png").is_err());
        assert!(validate_png_icon(b"not-png", "assets/suite-icon.png").is_err());
    }

    #[test]
    fn suite_asset_content_types_cover_supported_images() {
        assert_eq!(content_type_for_asset("assets/suite-icon.png"), "image/png");
        assert_eq!(
            content_type_for_asset("assets/suite-icon.webp"),
            "image/webp"
        );
        assert_eq!(
            content_type_for_asset("assets/suite-icon.svg"),
            "image/svg+xml; charset=utf-8"
        );
        assert_eq!(
            content_type_for_asset("assets/readme.txt"),
            "application/octet-stream"
        );
    }

    #[test]
    fn package_file_base64_round_trips_binary_content() {
        let expected = vec![0, 159, 146, 150, 255];
        let file = SuitePackageFile {
            path: "assets/suite-icon.png".to_string(),
            content_base64: STANDARD.encode(&expected),
        };
        assert_eq!(decode_package_file(&file).unwrap(), expected);
    }

    #[test]
    fn suite_platform_contract_requires_supported_positive_version() {
        assert!(validate_platform_contract_version(1).is_ok());
        assert!(validate_platform_contract_version(0).is_err());
        assert!(validate_platform_contract_version(2).is_err());
    }

    #[test]
    fn app_entry_inherits_suite_icon() {
        let instance = SuiteInstanceSummary {
            instance_id: "instance-1".to_string(),
            suite_id: "vust.example".to_string(),
            version: "0.1.0-alpha.1".to_string(),
            platform_contract_version: 1,
            node_id: "local".to_string(),
            compose_project_name: "vust-example".to_string(),
            status: "enabled".to_string(),
            last_error: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let entry = SuiteAppEntryManifest {
            id: "main".to_string(),
            title: "Example".to_string(),
            icon: String::new(),
            entry_type: "proxied_web".to_string(),
            service: Some("web".to_string()),
            port: Some(8080),
            path: Some("/".to_string()),
            target: None,
            default_width: 1024,
            default_height: 720,
            min_width: 860,
            min_height: 560,
            window: None,
        };
        assert_eq!(
            resolve_suite_icon(&instance, "assets/suite-icon.png", &entry),
            "/api/v1/suite-instances/instance-1/assets/suite-icon.png"
        );
    }

    #[test]
    fn runtime_images_are_optional_but_entries_must_not_be_empty() {
        assert!(validate_runtime_images(&[]).is_ok());
        assert!(
            validate_runtime_images(&[
                " vustcc/protocol-simulation-engine:0.1.0-alpha.1 ".to_string()
            ])
            .is_ok()
        );
        assert!(validate_runtime_images(&[" ".to_string()]).is_err());
        assert_eq!(
            normalize_runtime_images(&[
                " vustcc/protocol-simulation-engine:0.1.0-alpha.1 ".to_string(),
                "".to_string(),
            ]),
            vec!["vustcc/protocol-simulation-engine:0.1.0-alpha.1".to_string()]
        );
    }

    #[test]
    fn suite_minimum_version_is_required_and_cannot_exceed_current_build() {
        assert!(validate_min_vust_version(env!("CARGO_PKG_VERSION")).is_ok());
        assert!(validate_min_vust_version("not-semver").is_err());
        assert!(validate_min_vust_version("999.0.0").is_err());
    }

    #[test]
    fn agent_access_requires_known_capabilities_and_existing_services() {
        let manifest = serde_yaml::from_str::<SuiteManifest>(
            r#"
apiVersion: vust.io/v1alpha1
kind: ComposeSuite
metadata:
  suiteId: suite.example
  slug: example
  version: 1.0.0
  minVustVersion: 0.1.0-alpha.1
  name: Example
  icon: assets/icon.png
compatibility:
  platformContractVersion: 1
runtime:
  type: compose
  composeFile: compose.yaml
  agent:
    services: [api]
    capabilities: [workloads.manage, captures.manage]
appEntries: []
"#,
        )
        .unwrap();
        let files = vec![SuitePackageFile {
            path: "compose.yaml".to_string(),
            content_base64: STANDARD.encode("services:\n  api:\n    image: example/api:1\n"),
        }];
        assert!(validate_agent_access(&manifest, &files).is_ok());

        let mut unknown_capability = manifest.clone();
        unknown_capability
            .runtime
            .agent
            .as_mut()
            .unwrap()
            .capabilities = vec!["docker.admin".to_string()];
        assert!(validate_agent_access(&unknown_capability, &files).is_err());

        let mut unknown_service = manifest;
        unknown_service.runtime.agent.as_mut().unwrap().services = vec!["missing".to_string()];
        assert!(validate_agent_access(&unknown_service, &files).is_err());
    }

    #[test]
    fn suite_images_merge_compose_and_runtime_images() {
        let compose = r#"services:
  api:
    image: example/api:1.0
  dynamic:
    image: ${DYNAMIC_IMAGE}
"#;
        let request = AgentSuiteInstallRequest {
            instance_id: "instance".to_string(),
            suite_id: "suite".to_string(),
            version: "1.0.0".to_string(),
            compose_project_name: "suite-project".to_string(),
            compose_file: "compose.yaml".to_string(),
            runtime_images: vec![
                "example/engine:1.0".to_string(),
                "example/api:1.0".to_string(),
            ],
            agent_access: None,
            files: vec![SuitePackageFile {
                path: "compose.yaml".to_string(),
                content_base64: STANDARD.encode(compose),
            }],
            app_entries: Vec::new(),
        };
        assert_eq!(
            extract_suite_images(&request).unwrap(),
            BTreeSet::from([
                "example/api:1.0".to_string(),
                "example/engine:1.0".to_string()
            ])
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SUITE_DELETE_BLOCKED_MESSAGE_KEY, build_compose_project_name, build_suite_proxy_target,
        suite_delete_blocked_by_instances_error,
    };

    #[test]
    fn compose_project_name_uses_slug_and_does_not_include_instance_id() {
        assert_eq!(
            build_compose_project_name("host-scanner"),
            "vust-host-scanner"
        );
    }

    #[test]
    fn suite_proxy_target_keeps_web_entry_as_directory() {
        assert_eq!(
            build_suite_proxy_target("instance-1", "main", Some("/")),
            "/api/v1/suite-instances/instance-1/proxy/main/"
        );
        assert_eq!(
            build_suite_proxy_target("instance-1", "main", None),
            "/api/v1/suite-instances/instance-1/proxy/main/"
        );
        assert_eq!(
            build_suite_proxy_target("instance-1", "main", Some("/console")),
            "/api/v1/suite-instances/instance-1/proxy/main/console"
        );
    }

    #[test]
    fn delete_suite_blocked_error_has_message_key() {
        let err = suite_delete_blocked_by_instances_error();
        assert_eq!(
            err.message_key.as_deref(),
            Some(SUITE_DELETE_BLOCKED_MESSAGE_KEY)
        );
        assert_eq!(
            err.message,
            "suite is installed; uninstall all instances before deleting"
        );
    }
}
