//! Docker 镜像获取任务 API。

use crate::api::auth::AuthenticatedAdmin;
use crate::services::{
    image_acquisition::{ImageDistributionTask, ImageTask, ImageTaskActor},
    logging,
};
use crate::state::AppState;
use crate::types::{ApiResponse, ApiResult};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bollard::Docker;
use bollard::query_parameters::ListImagesOptions;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use vust_contracts::api::ErrorCode;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateImageTaskRequest {
    pub node_id: String,
    pub image_ref: String,
    pub source_mode: String,
}

/// 创建主控镜像批量分发任务的请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateImageDistributionTaskRequest {
    pub image_ref: String,
    pub target_node_ids: Vec<String>,
}

/// 主控镜像库使用的稳定摘要。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerImageSummary {
    pub id: String,
    pub tags: Vec<String>,
    pub digests: Vec<String>,
    pub created_at: i64,
    pub size_bytes: i64,
    pub container_count: i64,
    pub dangling: bool,
}

pub fn docker_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/image-tasks", post(create_image_task))
        .route("/image-tasks/{task_id}/progress", get(image_task_progress))
        .route("/image-tasks/{task_id}", delete(cancel_image_task))
        .route(
            "/image-distribution-tasks",
            post(create_image_distribution_task),
        )
        .route(
            "/image-distribution-tasks/recent",
            get(recent_image_distribution_tasks),
        )
        .route(
            "/image-distribution-tasks/{task_id}",
            get(image_distribution_task).delete(cancel_image_distribution_task),
        )
        .route("/controller/images", get(list_controller_images))
}

async fn local_docker() -> ApiResult<Docker> {
    let docker = Docker::connect_with_local_defaults().map_err(|err| {
        vust_api::error::ApiError::bad_gateway(
            vust_contracts::api::ErrorCode::DockerUnavailable,
            format!("Local docker daemon is not available: {err}"),
        )
    })?;
    docker.negotiate_version().await.map_err(|err| {
        vust_api::error::ApiError::bad_gateway(
            vust_contracts::api::ErrorCode::DockerUnavailable,
            format!("Failed to negotiate local Docker API version: {err}"),
        )
    })
}

pub async fn list_controller_images() -> ApiResult<Response> {
    let mut images = local_docker()
        .await?
        .list_images(Some(ListImagesOptions::default()))
        .await
        .map_err(|err| {
            vust_api::error::ApiError::internal(format!("failed to list local images: {err}"))
        })?
        .into_iter()
        .map(|image| ControllerImageSummary {
            dangling: image.repo_tags.is_empty()
                || image.repo_tags.iter().all(|tag| tag == "<none>:<none>"),
            id: image.id,
            tags: image.repo_tags,
            digests: image.repo_digests,
            created_at: image.created,
            size_bytes: image.size,
            container_count: image.containers.max(0),
        })
        .collect::<Vec<_>>();
    images.sort_by(|left, right| {
        controller_image_sort_key(left)
            .cmp(&controller_image_sort_key(right))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(ApiResponse::success_with_raw("Controller images loaded", Some(images)).into_response())
}

fn controller_image_sort_key(image: &ControllerImageSummary) -> String {
    image
        .tags
        .iter()
        .find(|tag| tag.as_str() != "<none>:<none>")
        .map(|tag| tag.to_lowercase())
        .unwrap_or_else(|| image.id.to_lowercase())
}

/// 校验全部输入后一次性创建镜像批量分发任务。
pub async fn create_image_distribution_task(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateImageDistributionTaskRequest>,
) -> ApiResult<Response> {
    let image_ref = payload.image_ref.trim().to_string();
    if image_ref.is_empty() {
        return Err(vust_api::error::ApiError::validation(
            "imageRef must not be empty",
        ));
    }
    local_docker()
        .await?
        .inspect_image(&image_ref)
        .await
        .map_err(|_| {
            vust_api::error::ApiError::not_found(
                ErrorCode::DockerOperationFailed,
                "controller image does not exist",
            )
        })?;
    let target_node_ids = validate_distribution_targets(payload.target_node_ids)?;
    let mut targets = Vec::with_capacity(target_node_ids.len());
    for node_id in target_node_ids {
        let node = crate::services::node_read_model::get_node_summary(&state.metadata_db, &node_id)
            .await
            .map_err(|err| vust_api::error::ApiError::database(err.to_string()))?
            .ok_or_else(|| {
                vust_api::error::ApiError::not_found(
                    ErrorCode::NodeNotFound,
                    format!("node does not exist: {node_id}"),
                )
            })?;
        if node.status != "online" {
            return Err(vust_api::error::ApiError::conflict(
                ErrorCode::NodeUnavailable,
                format!("node is not online: {node_id}"),
            ));
        }
        targets.push((node.node_id, node.name));
    }
    let task = state.image_acquisition.start_distribution(
        Arc::clone(&state),
        image_ref,
        targets,
        image_actor(&admin, &headers)?,
    );
    Ok(
        ApiResponse::success_with_raw("Image distribution task started", Some(task))
            .into_response(),
    )
}

fn validate_distribution_targets(target_node_ids: Vec<String>) -> ApiResult<Vec<String>> {
    if target_node_ids.is_empty() {
        return Err(vust_api::error::ApiError::validation(
            "targetNodeIds must not be empty",
        ));
    }
    let mut normalized = Vec::with_capacity(target_node_ids.len());
    let mut unique = HashSet::new();
    for raw_id in target_node_ids {
        let node_id = raw_id.trim().to_string();
        if node_id.is_empty() || node_id == "local" {
            return Err(vust_api::error::ApiError::bad_request(
                ErrorCode::NodeInvalidTarget,
                "distribution targets must be non-local nodes",
            ));
        }
        if !unique.insert(node_id.clone()) {
            return Err(vust_api::error::ApiError::validation(format!(
                "duplicate target node: {node_id}"
            )));
        }
        normalized.push(node_id);
    }
    Ok(normalized)
}

pub async fn image_distribution_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let task = find_distribution_task(&state, &task_id)?;
    Ok(ApiResponse::success_with_raw("Image distribution task loaded", Some(task)).into_response())
}

pub async fn recent_image_distribution_tasks(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Response> {
    let tasks = state.image_acquisition.recent_distributions();
    Ok(
        ApiResponse::success_with_raw("Recent image distribution tasks loaded", Some(tasks))
            .into_response(),
    )
}

pub async fn cancel_image_distribution_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let task = state
        .image_acquisition
        .cancel_distribution(&task_id)
        .ok_or_else(|| {
            vust_api::error::ApiError::not_found(
                ErrorCode::TaskNotFound,
                "image distribution task not found",
            )
        })?;
    Ok(
        ApiResponse::success_with_raw("Image distribution cancellation requested", Some(task))
            .into_response(),
    )
}

fn find_distribution_task(state: &AppState, task_id: &str) -> ApiResult<ImageDistributionTask> {
    state
        .image_acquisition
        .get_distribution(task_id)
        .ok_or_else(|| {
            vust_api::error::ApiError::not_found(
                ErrorCode::TaskNotFound,
                "image distribution task not found",
            )
        })
}

pub async fn create_image_task(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateImageTaskRequest>,
) -> ApiResult<Response> {
    let node_id = payload.node_id.trim();
    let image_ref = payload.image_ref.trim();
    if node_id.is_empty() || image_ref.is_empty() {
        return Err(vust_api::error::ApiError::bad_request(
            vust_contracts::api::ErrorCode::BadRequest,
            "nodeId and imageRef must not be empty".to_string(),
        ));
    }
    if payload.source_mode != "controller-first" {
        return Err(vust_api::error::ApiError::bad_request(
            vust_contracts::api::ErrorCode::BadRequest,
            "sourceMode must be controller-first".to_string(),
        ));
    }
    let task = state.image_acquisition.start(
        Arc::clone(&state),
        node_id.to_string(),
        image_ref.to_string(),
        Some(image_actor(&admin, &headers)?),
    );
    Ok(ApiResponse::success_with_raw("Image task started", Some(task)).into_response())
}

fn image_actor(admin: &AuthenticatedAdmin, headers: &HeaderMap) -> ApiResult<ImageTaskActor> {
    let client_ip = admin
        .session
        .client_ip
        .as_deref()
        .ok_or_else(|| {
            vust_api::error::ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "authenticated session is missing a trusted client IP",
            )
        })?
        .parse()
        .map_err(|_| vust_api::error::ApiError::internal("trusted client IP is invalid"))?;
    Ok(ImageTaskActor {
        user_id: admin.id,
        name: admin.username.clone(),
        client_ip,
        trace_id: logging::resolve_trace_id(headers),
    })
}

pub async fn image_task_progress(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let task = find_task(&state, &task_id)?;
    Ok(ApiResponse::success_with_raw("Image task progress loaded", Some(task)).into_response())
}

pub async fn cancel_image_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> ApiResult<Response> {
    let task = state
        .image_acquisition
        .cancel(&task_id)
        .ok_or(vust_api::error::ApiError::NotFound)?;
    Ok(
        ApiResponse::success_with_raw("Image task cancellation requested", Some(task))
            .into_response(),
    )
}

fn find_task(state: &AppState, task_id: &str) -> ApiResult<ImageTask> {
    state
        .image_acquisition
        .get(task_id)
        .ok_or(vust_api::error::ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_targets_reject_local_and_duplicates() {
        assert!(validate_distribution_targets(vec![]).is_err());
        assert!(validate_distribution_targets(vec!["local".to_string()]).is_err());
        assert!(
            validate_distribution_targets(vec!["node-1".to_string(), " node-1 ".to_string()])
                .is_err()
        );
        assert_eq!(
            validate_distribution_targets(vec![" node-1 ".to_string()]).unwrap(),
            vec!["node-1".to_string()]
        );
    }
}
