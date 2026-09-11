//! 运行日志 API：按服务、节点和受控文件读取 tracing 日志。

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use vust_api::response::ApiResponse as RuntimeApiResponse;
use vust_contracts::runtime_logs::{
    RuntimeLogAvailability, RuntimeLogFileList, RuntimeLogQuery, RuntimeLogQueryResult,
};

use crate::{
    models::{node_runtime_client::NodeRuntimeClient, nodes::get_node_by_id},
    services::runtime_logs,
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesQuery {
    service: Option<String>,
    node_id: Option<String>,
}

async fn files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilesQuery>,
) -> ApiResult<Response> {
    let result = if query.service.as_deref() == Some("agent") && is_remote(query.node_id.as_deref())
    {
        let node_id = query.node_id.as_deref().unwrap_or_default();
        let node = get_node_by_id(&state.metadata_db, node_id)
            .await?
            .ok_or(ApiError::NotFound)?;
        let node_name = Some(node.name);
        if matches!(node.status.as_str(), "offline" | "unreachable" | "retired") {
            return Ok(ApiResponse::success_with_raw(
                "Runtime log node is offline",
                Some(RuntimeLogFileList {
                    availability: RuntimeLogAvailability::NodeOffline,
                    reason_code: Some("NODE_OFFLINE".to_string()),
                    files: vec![],
                }),
            )
            .into_response());
        }
        let client =
            match NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await {
                Ok(client) => client,
                Err(_) => {
                    return Ok(ApiResponse::success_with_raw(
                        "Runtime log node is offline",
                        Some(RuntimeLogFileList {
                            availability: RuntimeLogAvailability::NodeOffline,
                            reason_code: Some("NODE_UNAVAILABLE".to_string()),
                            files: vec![],
                        }),
                    )
                    .into_response());
                }
            };
        let mut result = match client.get_json("/api/v1/agent/runtime-logs/files").await {
            Ok(response) => unwrap::<RuntimeLogFileList>(response)?,
            Err(_) => RuntimeLogFileList {
                availability: RuntimeLogAvailability::NodeOffline,
                reason_code: Some("NODE_UNAVAILABLE".to_string()),
                files: vec![],
            },
        };
        for file in &mut result.files {
            file.node_id = Some(node_id.to_string());
            file.node_name = node_name.clone();
        }
        result
    } else {
        runtime_logs::list_runtime_log_files(query.service.as_deref()).await?
    };
    Ok(ApiResponse::success_with_raw("Runtime log files loaded", Some(result)).into_response())
}

async fn query(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RuntimeLogQuery>,
) -> ApiResult<Response> {
    let result = if payload.service == "agent" && is_remote(payload.node_id.as_deref()) {
        let client =
            NodeRuntimeClient::from_node_route(&state.metadata_db, payload.node_id.as_deref())
                .await?;
        unwrap::<RuntimeLogQueryResult>(
            client
                .post_json("/api/v1/agent/runtime-logs/query", &payload)
                .await
                .map_err(|err| ApiError::Internal(err.to_string()))?,
        )?
    } else {
        runtime_logs::query_runtime_logs(payload).await?
    };
    Ok(ApiResponse::success_with_raw("Runtime logs loaded", Some(result)).into_response())
}

fn is_remote(node_id: Option<&str>) -> bool {
    matches!(node_id, Some(value) if !value.trim().is_empty() && value != "local")
}
fn unwrap<T>(response: RuntimeApiResponse<T>) -> ApiResult<T> {
    if !response.success {
        return Err(ApiError::BadRequest(response.message));
    }
    response
        .data
        .ok_or_else(|| ApiError::Internal("node runtime log response is empty".to_string()))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/files", get(files))
        .route("/query", post(query))
}
