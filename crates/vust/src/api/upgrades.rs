//! 在线升级 API：发布版本同步、计划创建与计划状态查询。

use crate::api::auth::AuthenticatedAdmin;
use crate::models::logging::{LogModule, LogStatus, PlatformLogLevel};
use crate::services::logging::{self, OperationEventBuilder};
use crate::services::upgrades::{
    self, SuiteCompatibilityCheckPayload, UpgradeAuditContext, UpgradePlanCreatePayload,
};
use crate::state::AppState;
use crate::types::{ApiResponse, ApiResult};
use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::{Value, json};
use std::{net::IpAddr, sync::Arc};

/// 查询已同步的发布版本。
pub async fn list_releases(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let releases = upgrades::list_synced_releases(&state.metadata_db).await?;
    Ok(ApiResponse::success_with_raw("Upgrade releases loaded", Some(releases)).into_response())
}

/// 从 GitHub 同步发布版本。
pub async fn sync_releases(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let result = upgrades::sync_github_releases(&state.metadata_db).await;
    record_upgrade_change(
        &state,
        &admin,
        &headers,
        "upgrade_releases_sync",
        "upgrade_releases",
        "catalog",
        false,
        &result,
        json!({ "releaseCount": result.as_ref().ok().map(Vec::len) }),
    )
    .await?;
    let releases = result?;
    Ok(ApiResponse::success_with_raw("Upgrade releases synced", Some(releases)).into_response())
}

/// 上传本地完整版本包。
pub async fn upload_release(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    let release_res = upgrades::upload_release_package(&state.metadata_db, &mut multipart).await;
    let target_id = release_res
        .as_ref()
        .ok()
        .map(|release| release.version.as_str())
        .unwrap_or("upload");
    record_upgrade_change(
        &state,
        &admin,
        &headers,
        "upgrade_release_upload",
        "upgrade_release",
        target_id,
        false,
        &release_res,
        json!({ "version": release_res.as_ref().ok().map(|release| &release.version) }),
    )
    .await?;
    let release = upgrades::build_release_view(release_res?);
    Ok(ApiResponse::success_with_raw("Upgrade release uploaded", Some(release)).into_response())
}

/// 检测指定升级版本与已安装套件的契约兼容性。
pub async fn check_suite_compatibility(
    State(state): State<Arc<AppState>>,
    Path(version): Path<String>,
    Json(payload): Json<SuiteCompatibilityCheckPayload>,
) -> ApiResult<Response> {
    let report = upgrades::check_suite_compatibility(&state.metadata_db, &version, payload).await?;
    Ok(ApiResponse::success_with_raw("Suite compatibility checked", Some(report)).into_response())
}

/// 创建升级计划。
pub async fn create_plan(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(payload): Json<UpgradePlanCreatePayload>,
) -> ApiResult<Response> {
    let audit = upgrade_audit_context(&admin, &headers)?;
    let detail = upgrades::create_plan(&state.metadata_db, payload, &audit).await?;
    Ok(ApiResponse::success_with_raw("Upgrade plan created", Some(detail)).into_response())
}

/// 启动升级计划。
pub async fn start_plan(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let audit = upgrade_audit_context(&admin, &headers)?;
    let detail = upgrades::start_plan(&state.metadata_db, &plan_id, &audit).await?;
    Ok(ApiResponse::success_with_raw("Upgrade plan started", Some(detail)).into_response())
}

/// 查询升级计划详情。
pub async fn detail(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<String>,
) -> ApiResult<Response> {
    let detail = upgrades::detail(&state.metadata_db, &plan_id).await?;
    Ok(ApiResponse::success_with_raw("Upgrade plan loaded", Some(detail)).into_response())
}

/// 获取最近的升级计划。
pub async fn latest_plan(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let detail = upgrades::get_latest_plan(&state.metadata_db).await?;
    Ok(ApiResponse::success_with_raw("Latest upgrade plan loaded", Some(detail)).into_response())
}

/// 取消升级计划。
pub async fn cancel_plan(
    State(state): State<Arc<AppState>>,
    Path(plan_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let audit = upgrade_audit_context(&admin, &headers)?;
    let detail = upgrades::cancel_plan(&state.metadata_db, &plan_id, &audit).await?;
    Ok(ApiResponse::success_with_raw("Upgrade plan canceled", Some(detail)).into_response())
}

/// 删除发布版本及其缓存。
pub async fn delete_release(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Path(version): Path<String>,
) -> ApiResult<Response> {
    let res = upgrades::delete_release(&state.metadata_db, &version).await;
    record_upgrade_change(
        &state,
        &admin,
        &headers,
        "upgrade_release_delete",
        "upgrade_release",
        &version,
        true,
        &res,
        json!({ "version": version }),
    )
    .await?;
    res?;
    Ok(ApiResponse::success_with_raw("Upgrade release deleted", Some(())).into_response())
}

fn upgrade_audit_context(
    admin: &AuthenticatedAdmin,
    headers: &HeaderMap,
) -> ApiResult<UpgradeAuditContext> {
    let client_ip = admin.session.client_ip.clone().ok_or_else(|| {
        crate::types::ApiError::forbidden(
            vust_contracts::api::ErrorCode::AuthForbidden,
            "authenticated session is missing a trusted client IP",
        )
    })?;
    client_ip.parse::<IpAddr>().map_err(|_| {
        crate::types::ApiError::forbidden(
            vust_contracts::api::ErrorCode::AuthForbidden,
            "authenticated session has an invalid trusted client IP",
        )
    })?;
    Ok(UpgradeAuditContext {
        user_id: admin.id,
        username: admin.username.clone(),
        client_ip,
        trace_id: logging::resolve_trace_id(headers),
    })
}

#[allow(clippy::too_many_arguments)]
async fn record_upgrade_change<T>(
    state: &AppState,
    admin: &AuthenticatedAdmin,
    headers: &HeaderMap,
    event_code: &str,
    target_kind: &str,
    target_id: &str,
    high_impact: bool,
    result: &ApiResult<T>,
    mut metadata: Value,
) -> ApiResult<()> {
    let audit = upgrade_audit_context(admin, headers)?;
    let failed = result.is_err();
    if let Some(values) = metadata.as_object_mut() {
        values.insert(
            "result".to_string(),
            json!(if failed { "failed" } else { "success" }),
        );
        if let Err(error) = result {
            values.insert("errorCode".to_string(), json!(error.code.as_str()));
        }
    }
    let builder = OperationEventBuilder::new(
        &audit.username,
        event_code,
        audit.client_ip.parse().expect("validated audit IP"),
    )
    .user_id(audit.user_id)
    .module(LogModule::System)
    .target_type(target_kind)
    .target_id(target_id)
    .trace_id(&audit.trace_id)
    .request("POST", "/api/v1/upgrades")
    .status(if failed {
        LogStatus::Failed
    } else {
        LogStatus::Success
    })
    .level(if failed {
        PlatformLogLevel::Error
    } else if high_impact {
        PlatformLogLevel::Warning
    } else {
        PlatformLogLevel::Info
    })
    .metadata(metadata);
    logging::persist_event(builder, &state.metadata_db).await
}

/// 在线升级 API 路由。
pub fn upgrades_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/releases/list", get(list_releases))
        .route("/releases/sync", post(sync_releases))
        .route("/releases/upload", post(upload_release))
        .route("/release/{version}/delete", delete(delete_release))
        .route(
            "/release/{version}/compatibility/check",
            post(check_suite_compatibility),
        )
        .route("/plan/create", post(create_plan))
        .route("/plan/latest", get(latest_plan))
        .route("/plan/{plan_id}/start", post(start_plan))
        .route("/plan/{plan_id}/detail", get(detail))
        .route("/plan/{plan_id}/cancel", post(cancel_plan))
}
