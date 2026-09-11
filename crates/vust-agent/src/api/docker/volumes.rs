//! Docker 卷 API：提供规范化列表、详情、校验与使用状态保护。

use crate::api::docker::context::DockerOperationContext;
use crate::models::docker::{
    self, DockerVolumeCapabilities, DockerVolumeContainerReference, DockerVolumeDetail,
    DockerVolumeListResponse, DockerVolumeManagement, DockerVolumeManagementKind,
    DockerVolumeSummary,
};
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};

use axum::extract::{Json, Path, State};
use axum::response::{IntoResponse, Response};
use bollard::Docker;
use bollard::models::{ContainerSummary, Volume, VolumeCreateRequest, VolumeListResponse};
use bollard::query_parameters;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::info;
use vust_contracts::api::ErrorCode;

/// 返回规范化的 Docker 卷列表和 daemon 警告。
pub async fn list_volumes(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    info!("Requesting Docker volume list");
    let docker = state.docker_client().await?;
    let response = docker
        .list_volumes(Some(query_parameters::ListVolumesOptions::default()))
        .await?;
    let result = list_response_from_docker(response);
    Ok(ApiResponse::success_with_raw("Docker volumes loaded", Some(result)).into_response())
}

/// 将 Docker 原始列表响应转换为稳定排序的领域响应。
fn list_response_from_docker(response: VolumeListResponse) -> DockerVolumeListResponse {
    let mut items = response
        .volumes
        .unwrap_or_default()
        .iter()
        .map(summary_from_volume)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    DockerVolumeListResponse {
        items,
        warnings: response.warnings.unwrap_or_default(),
    }
}

/// 创建经过校验的 Docker 本地卷。
pub async fn create_volume(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Json(payload): Json<docker::DockerVolumeCreateRequest>,
) -> ApiResult<Response> {
    let volume_name = payload.name.trim().to_string();
    info!(volume = %volume_name, "Requesting Docker volume create");
    let result: ApiResult<Response> = async {
        let request = build_create_request(payload)?;
        let docker = state.docker_client().await?;
        let volume = docker.create_volume(request).await?;
        Ok(ApiResponse::success_with_raw(
            "Docker volume created",
            Some(summary_from_volume(&volume)),
        )
        .into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_volume_create",
            Some(("volume", &volume_name)),
            json!({ "name": volume_name, "management": "custom" }),
            false,
            result,
        )
        .await
}

/// 删除未被容器引用的 Docker 卷。
pub async fn remove_volume(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    info!(volume = %name, "Requesting Docker volume remove");
    let mut management = "unknown";
    let result: ApiResult<Response> = async {
        let docker = state.docker_client().await?;
        let volume = docker.inspect_volume(&name).await?;
        let summary = summary_from_volume(&volume);
        management = summary.management.kind.as_str();
        let references = list_references(&docker, &name).await?;
        ensure_unused(&references)?;
        docker
            .remove_volume(&name, None::<query_parameters::RemoveVolumeOptions>)
            .await?;
        Ok(ApiResponse::ok("Docker volume removed").into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_volume_remove",
            Some(("volume", &name)),
            json!({ "name": name, "management": management }),
            true,
            result,
        )
        .await
}

/// 返回卷元数据及所有引用该卷的容器挂载信息。
pub async fn inspect_volume(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    info!(volume = %name, "Requesting Docker volume inspect");
    let docker = state.docker_client().await?;
    let volume = docker.inspect_volume(&name).await?;
    let referenced_containers = list_references(&docker, &name).await?;
    let detail = detail_from_volume(volume, referenced_containers);
    Ok(ApiResponse::success_with_raw("Docker volume detail loaded", Some(detail)).into_response())
}

/// 将 Docker 原始卷转换为稳定的列表摘要。
fn summary_from_volume(volume: &Volume) -> DockerVolumeSummary {
    let management = classify_management(&volume.labels);
    DockerVolumeSummary {
        name: volume.name.clone(),
        driver: volume.driver.clone(),
        created_at: created_at_seconds(volume.created_at.as_ref()),
        capabilities: DockerVolumeCapabilities { can_remove: true },
        management,
    }
}

/// 将 Docker 原始卷和容器引用转换为稳定详情。
fn detail_from_volume(
    volume: Volume,
    referenced_containers: Vec<DockerVolumeContainerReference>,
) -> DockerVolumeDetail {
    DockerVolumeDetail {
        summary: summary_from_volume(&volume),
        mountpoint: volume.mountpoint,
        scope: volume
            .scope
            .map(|scope| scope.to_string())
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        options: volume.options,
        labels: volume.labels,
        referenced_containers,
    }
}

/// 套件标签优先于 Compose 标签，其他卷作为自定义资源处理。
fn classify_management(labels: &HashMap<String, String>) -> DockerVolumeManagement {
    let (kind, owner_name) = if labels.get("vust.owner").map(String::as_str) == Some("suite") {
        let owner = labels
            .get("vust.suite_name")
            .or_else(|| labels.get("vust.suite_id"))
            .or_else(|| labels.get("com.docker.compose.project"))
            .cloned();
        (DockerVolumeManagementKind::Suite, owner)
    } else if let Some(project) = labels.get("com.docker.compose.project") {
        (DockerVolumeManagementKind::Compose, Some(project.clone()))
    } else {
        (DockerVolumeManagementKind::Custom, None)
    };
    DockerVolumeManagement { kind, owner_name }
}

/// 构造固定使用 local 驱动的创建请求。
fn build_create_request(
    payload: docker::DockerVolumeCreateRequest,
) -> ApiResult<VolumeCreateRequest> {
    let name = payload.name.trim().to_string();
    validate_name(&name)?;
    Ok(VolumeCreateRequest {
        name: Some(name),
        driver: Some("local".to_string()),
        driver_opts: normalize_map(payload.options, false)?,
        labels: normalize_map(payload.labels, true)?,
        ..Default::default()
    })
}

/// 校验 Docker 卷名称，避免依赖 daemon 原始错误文本。
fn validate_name(name: &str) -> ApiResult<()> {
    let valid = (2..=255).contains(&name.len())
        && name
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_alphanumeric())
        && name
            .chars()
            .skip(1)
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '.' | '-'));
    if valid {
        Ok(())
    } else {
        Err(ApiError::validation(
            "volume name must match [a-zA-Z0-9][a-zA-Z0-9_.-]+ and be at most 255 bytes",
        ))
    }
}

/// 规范化选项或标签，并阻止用户伪造系统归属标签。
fn normalize_map(
    values: Option<HashMap<String, String>>,
    protect_labels: bool,
) -> ApiResult<Option<HashMap<String, String>>> {
    let Some(values) = values else {
        return Ok(None);
    };
    let mut result = HashMap::new();
    let mut keys = HashSet::new();
    for (raw_key, raw_value) in values {
        let key = raw_key.trim().to_string();
        if key.is_empty() {
            return Err(ApiError::validation(
                "volume option and label keys cannot be empty",
            ));
        }
        if protect_labels && (key.starts_with("vust.") || key.starts_with("com.docker.compose.")) {
            return Err(ApiError::validation(format!(
                "reserved Docker volume label: {key}"
            )));
        }
        if !keys.insert(key.clone()) {
            return Err(ApiError::validation(format!("duplicate key: {key}")));
        }
        result.insert(key, raw_value.trim().to_string());
    }
    Ok((!result.is_empty()).then_some(result))
}

/// 查询所有引用指定卷的容器，并复用列表返回的挂载信息。
async fn list_references(
    docker: &Docker,
    volume_name: &str,
) -> ApiResult<Vec<DockerVolumeContainerReference>> {
    let filters = HashMap::from([("volume".to_string(), vec![volume_name.to_string()])]);
    let options = query_parameters::ListContainersOptionsBuilder::new()
        .all(true)
        .filters(&filters)
        .build();
    let containers = docker.list_containers(Some(options)).await?;
    Ok(references_from_containers(&containers, volume_name))
}

/// 从容器摘要提取指定卷的挂载目标和读写模式。
fn references_from_containers(
    containers: &[ContainerSummary],
    volume_name: &str,
) -> Vec<DockerVolumeContainerReference> {
    let mut references = containers
        .iter()
        .filter_map(|container| {
            let mount = container
                .mounts
                .as_ref()?
                .iter()
                .find(|mount| mount.name.as_deref() == Some(volume_name))?;
            let id = container.id.clone().unwrap_or_default();
            let name = container
                .names
                .as_ref()
                .and_then(|names| names.first())
                .map(|value| value.trim_start_matches('/').to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| id.chars().take(12).collect());
            Some(DockerVolumeContainerReference {
                id,
                name,
                state: container
                    .state
                    .map(|state| state.to_string())
                    .filter(|state| !state.is_empty())
                    .unwrap_or_else(|| "unknown".to_string()),
                destination: mount.destination.clone().filter(|value| !value.is_empty()),
                read_only: !mount.rw.unwrap_or(true),
            })
        })
        .collect::<Vec<_>>();
    references.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    references
}

/// 拒绝删除仍被容器引用的卷，并提供安全的容器名称摘要。
fn ensure_unused(references: &[DockerVolumeContainerReference]) -> ApiResult<()> {
    if references.is_empty() {
        return Ok(());
    }
    let names = references
        .iter()
        .take(5)
        .map(|reference| reference.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(ApiError::conflict(
        ErrorCode::DockerVolumeInUse,
        format!(
            "volume is referenced by {} container(s): {names}",
            references.len()
        ),
    )
    .with_detail(names))
}

/// 将 Docker RFC 3339 时间转换为 Unix 秒。
fn created_at_seconds(value: Option<&String>) -> Option<i64> {
    value.and_then(|timestamp| {
        chrono::DateTime::parse_from_rfc3339(timestamp)
            .ok()
            .map(|parsed| parsed.timestamp())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{ContainerSummaryStateEnum, MountPoint};

    fn volume(name: &str, labels: &[(&str, &str)]) -> Volume {
        Volume {
            name: name.to_string(),
            driver: "local".to_string(),
            mountpoint: format!("/var/lib/docker/volumes/{name}/_data"),
            labels: labels
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn classifies_suite_compose_and_custom_volumes() {
        let suite = summary_from_volume(&volume(
            "suite-data",
            &[
                ("vust.owner", "suite"),
                ("com.docker.compose.project", "suite-project"),
            ],
        ));
        let compose = summary_from_volume(&volume(
            "project-data",
            &[("com.docker.compose.project", "project")],
        ));
        let custom = summary_from_volume(&volume("custom-data", &[]));
        assert_eq!(suite.management.kind, DockerVolumeManagementKind::Suite);
        assert_eq!(
            suite.management.owner_name.as_deref(),
            Some("suite-project")
        );
        assert!(suite.capabilities.can_remove);
        assert_eq!(compose.management.kind, DockerVolumeManagementKind::Compose);
        assert!(compose.capabilities.can_remove);
        assert_eq!(custom.management.kind, DockerVolumeManagementKind::Custom);
        assert!(custom.capabilities.can_remove);
    }

    #[test]
    fn normalizes_list_order_and_preserves_warnings() {
        let response = list_response_from_docker(VolumeListResponse {
            volumes: Some(vec![volume("zeta", &[]), volume("Alpha", &[])]),
            warnings: Some(vec!["plugin warning".to_string()]),
        });
        assert_eq!(response.items[0].name, "Alpha");
        assert_eq!(response.items[1].name, "zeta");
        assert_eq!(response.warnings, vec!["plugin warning"]);
    }

    #[test]
    fn validates_names_and_reserved_labels() {
        assert!(validate_name("data-01").is_ok());
        assert!(validate_name("x").is_err());
        assert!(validate_name("bad/name").is_err());
        let labels = HashMap::from([("vust.owner".to_string(), "suite".to_string())]);
        assert!(normalize_map(Some(labels), true).is_err());
    }

    #[test]
    fn detects_trimmed_duplicate_keys() {
        let options = HashMap::from([
            ("type".to_string(), "none".to_string()),
            (" type ".to_string(), "nfs".to_string()),
        ]);
        assert!(normalize_map(Some(options), false).is_err());
    }

    #[test]
    fn extracts_and_sorts_container_references() {
        let containers = vec![
            ContainerSummary {
                id: Some("b-id".to_string()),
                names: Some(vec!["/zeta".to_string()]),
                state: Some(ContainerSummaryStateEnum::EXITED),
                mounts: Some(vec![MountPoint {
                    name: Some("data".to_string()),
                    destination: Some("/var/lib/data".to_string()),
                    rw: Some(false),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ContainerSummary {
                id: Some("a-id".to_string()),
                names: Some(vec!["/alpha".to_string()]),
                state: Some(ContainerSummaryStateEnum::RUNNING),
                mounts: Some(vec![MountPoint {
                    name: Some("data".to_string()),
                    destination: Some("/data".to_string()),
                    rw: Some(true),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ];
        let references = references_from_containers(&containers, "data");
        assert_eq!(references[0].name, "alpha");
        assert_eq!(references[0].destination.as_deref(), Some("/data"));
        assert!(!references[0].read_only);
        assert_eq!(references[1].state, "exited");
        assert!(references[1].read_only);
    }

    #[test]
    fn reports_referenced_volume_as_conflict() {
        let references = vec![DockerVolumeContainerReference {
            id: "id".to_string(),
            name: "database".to_string(),
            state: "running".to_string(),
            destination: Some("/data".to_string()),
            read_only: false,
        }];
        let error = ensure_unused(&references).unwrap_err();
        assert_eq!(error.code, ErrorCode::DockerVolumeInUse);
        assert!(error.to_string().contains("database"));
    }
}
