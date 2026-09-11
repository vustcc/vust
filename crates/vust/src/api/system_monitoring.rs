//! 系统监控语义网关：绑定节点、认证会话、能力与最终操作日志。

use axum::{
    Json, Router,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{net::IpAddr, sync::Arc};
use vust_contracts::monitoring::{
    SystemMonitoringOverview, SystemMonitoringSeriesPage, SystemMonitoringSettings,
};

use crate::{
    api::auth::AuthenticatedAdmin,
    models::{
        logging::{LogModule, LogStatus, PlatformLogLevel},
        node_runtime_client::{AgentOperationContext, NodeRuntimeClient},
        nodes,
    },
    services::logging::{self, OperationEventBuilder},
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};

const AGENT_BASE_PATH: &str = "/api/v1/agent/system-monitoring";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeriesQuery {
    #[serde(default = "default_range")]
    range: String,
    #[serde(default = "default_sort")]
    sort: String,
    #[serde(default = "default_limit")]
    limit: u16,
    cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsUpdate {
    history_collection_enabled: bool,
    retention_days: u8,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClearHistoryResult {
    cleared_sample_count: u64,
}

/// 构建单节点系统监控路由。
pub fn system_monitoring_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{node_id}/system-monitoring/overview", get(overview))
        .route("/{node_id}/system-monitoring/series", get(series))
        .route(
            "/{node_id}/system-monitoring/settings",
            get(settings).put(update_settings),
        )
        .route(
            "/{node_id}/system-monitoring/history",
            delete(clear_history),
        )
}

async fn overview(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
) -> ApiResult<Response> {
    let client = node_client(&state, &node_id).await?;
    let mut data: SystemMonitoringOverview = client
        .get_domain(&format!("{AGENT_BASE_PATH}/overview"))
        .await?;
    apply_capabilities(
        &mut data.capabilities.can_manage_collection,
        &mut data.capabilities.can_clear_history,
    );
    Ok(
        ApiResponse::success_with_raw("System monitoring overview loaded", Some(data))
            .into_response(),
    )
}

async fn series(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    Query(query): Query<SeriesQuery>,
) -> ApiResult<Response> {
    validate_series_query(&query)?;
    let client = node_client(&state, &node_id).await?;
    let cursor = query
        .cursor
        .as_ref()
        .map(|value| format!("&cursor={value}"))
        .unwrap_or_default();
    let path = format!(
        "{AGENT_BASE_PATH}/series?range={}&sort={}&limit={}{}",
        query.range, query.sort, query.limit, cursor
    );
    let data: SystemMonitoringSeriesPage = client.get_domain(&path).await?;
    Ok(
        ApiResponse::success_with_raw("System monitoring series loaded", Some(data))
            .into_response(),
    )
}

async fn settings(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
) -> ApiResult<Response> {
    let client = node_client(&state, &node_id).await?;
    let mut data: SystemMonitoringSettings = client
        .get_domain(&format!("{AGENT_BASE_PATH}/settings"))
        .await?;
    apply_capabilities(
        &mut data.capabilities.can_manage_collection,
        &mut data.capabilities.can_clear_history,
    );
    Ok(
        ApiResponse::success_with_raw("System monitoring settings loaded", Some(data))
            .into_response(),
    )
}

async fn update_settings(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    payload: Result<Json<SettingsUpdate>, JsonRejection>,
) -> ApiResult<Response> {
    let Json(payload) = payload.map_err(|_| {
        ApiError::bad_request(
            vust_contracts::api::ErrorCode::SystemMonitoringInvalidSettings,
            "system monitoring settings payload is invalid",
        )
    })?;
    let (client, node_name) = node_client_and_name(&state, &node_id).await?;
    let previous: SystemMonitoringSettings = client
        .get_domain(&format!("{AGENT_BASE_PATH}/settings"))
        .await?;
    let trace_id = logging::resolve_trace_id(&headers);
    let operation_context = operation_context(&admin, &trace_id)?;
    let result = client
        .put_domain_with_operation_context::<SystemMonitoringSettings, _>(
            &format!("{AGENT_BASE_PATH}/settings"),
            &payload,
            &operation_context,
        )
        .await;
    let high_impact =
        !payload.history_collection_enabled || payload.retention_days < previous.retention_days;
    record_operation(
        &state,
        &admin,
        &node_id,
        &node_name,
        &trace_id,
        "system_monitoring_settings_update",
        "PUT",
        high_impact,
        &result,
        json!({
            "historyCollectionEnabled": payload.history_collection_enabled,
            "retentionDays": payload.retention_days,
        }),
    );
    let mut data = result?;
    apply_capabilities(
        &mut data.capabilities.can_manage_collection,
        &mut data.capabilities.can_clear_history,
    );
    Ok(
        ApiResponse::success_with_raw("System monitoring settings updated", Some(data))
            .into_response(),
    )
}

async fn clear_history(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (client, node_name) = node_client_and_name(&state, &node_id).await?;
    let trace_id = logging::resolve_trace_id(&headers);
    let operation_context = operation_context(&admin, &trace_id)?;
    let result = client
        .delete_domain_with_operation_context::<ClearHistoryResult>(
            &format!("{AGENT_BASE_PATH}/history"),
            &operation_context,
        )
        .await;
    record_operation(
        &state,
        &admin,
        &node_id,
        &node_name,
        &trace_id,
        "system_monitoring_history_clear",
        "DELETE",
        true,
        &result,
        json!({}),
    );
    let data = result?;
    Ok(
        ApiResponse::success_with_raw("System monitoring history cleared", Some(data))
            .into_response(),
    )
}

async fn node_client(state: &AppState, node_id: &str) -> ApiResult<NodeRuntimeClient> {
    node_client_and_name(state, node_id)
        .await
        .map(|(client, _)| client)
}

async fn node_client_and_name(
    state: &AppState,
    node_id: &str,
) -> ApiResult<(NodeRuntimeClient, String)> {
    let node_name = if node_id == "local" {
        "Local Node".to_string()
    } else {
        nodes::get_node_by_id(&state.metadata_db, node_id)
            .await
            .map_err(|error| ApiError::database(error.to_string()))?
            .ok_or(ApiError::NotFound)?
            .name
    };
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?;
    Ok((client, node_name))
}

fn operation_context(
    admin: &AuthenticatedAdmin,
    trace_id: &str,
) -> ApiResult<AgentOperationContext> {
    let client_ip = admin.session.client_ip.clone().ok_or_else(|| {
        ApiError::forbidden(
            vust_contracts::api::ErrorCode::AuthForbidden,
            "authenticated session is missing a trusted client IP",
        )
    })?;
    Ok(AgentOperationContext {
        actor_user_id: admin.id,
        actor_name: admin.username.clone(),
        client_ip,
        trace_id: trace_id.to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
fn record_operation<T>(
    state: &AppState,
    admin: &AuthenticatedAdmin,
    node_id: &str,
    node_name: &str,
    trace_id: &str,
    event: &str,
    method: &str,
    high_impact: bool,
    result: &ApiResult<T>,
    mut metadata: serde_json::Value,
) {
    let Some(client_ip) = admin
        .session
        .client_ip
        .as_deref()
        .and_then(|value| value.parse::<IpAddr>().ok())
    else {
        tracing::error!(
            event,
            "Trusted session IP is invalid; platform operation log skipped"
        );
        return;
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert("nodeName".to_string(), json!(node_name));
        object.insert(
            "result".to_string(),
            json!(if result.is_ok() { "success" } else { "failed" }),
        );
        if let Err(error) = result {
            object.insert("errorCode".to_string(), json!(error.code.as_str()));
        }
    }
    let (status, level) = if result.is_err() {
        (LogStatus::Failed, PlatformLogLevel::Error)
    } else if high_impact {
        (LogStatus::Success, PlatformLogLevel::Warning)
    } else {
        (LogStatus::Success, PlatformLogLevel::Info)
    };
    OperationEventBuilder::new(&admin.username, event, client_ip)
        .user_id(admin.id)
        .module(LogModule::System)
        .target_type("node")
        .target_id(node_id)
        .trace_id(trace_id)
        .request(method, &format!("/api/v1/node/{node_id}/system-monitoring"))
        .metadata(metadata)
        .status(status)
        .level(level)
        .finish(&state.metadata_db);
}

fn apply_capabilities(can_manage: &mut bool, can_clear: &mut bool) {
    *can_manage = true;
    *can_clear = true;
}

fn validate_series_query(query: &SeriesQuery) -> ApiResult<()> {
    if !["1h", "6h", "24h", "3d", "7d"].contains(&query.range.as_str())
        || !["asc", "desc"].contains(&query.sort.as_str())
        || query.limit == 0
        || query.limit > 500
        || query
            .cursor
            .as_ref()
            .is_some_and(|value| value.parse::<usize>().is_err())
    {
        return Err(ApiError::bad_request(
            vust_contracts::api::ErrorCode::SystemMonitoringInvalidRange,
            "invalid system monitoring series query",
        ));
    }
    Ok(())
}

fn default_range() -> String {
    "24h".to_string()
}

fn default_sort() -> String {
    "asc".to_string()
}

fn default_limit() -> u16 {
    500
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_or_unbounded_series_query() {
        let invalid = SeriesQuery {
            range: "24h&cursor=forged".to_string(),
            sort: "asc".to_string(),
            limit: 500,
            cursor: None,
        };
        assert!(validate_series_query(&invalid).is_err());
    }
}
