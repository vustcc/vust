//! Docker 系统清理与磁盘使用统计 API。

use crate::api::docker::context::DockerOperationContext;
use crate::models::docker::{DockerDiskUsageCategory, DockerDiskUsageSummary};
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use bollard::Docker;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::process::Command;
use tracing::info;

/// Docker 29.0 从 API 1.52 开始提供聚合磁盘统计；Docker 28.x 的 API 1.48–1.51
/// 仍返回旧版明细结构。提升最低 Docker 版本前必须保留对应兼容分支。
const DOCKER_AGGREGATED_DISK_USAGE_API_MINOR: usize = 52;
const DOCKER_SOCKET_PATH: &str = "/var/run/docker.sock";

/// 获取 Docker 系统磁盘使用统计（镜像、容器、卷、缓存）。
pub async fn system_df(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    info!("Requesting docker system df");
    let docker = state.docker_client().await?;
    let collected_at = Utc::now().timestamp();
    let summary = if docker.client_version().minor_version >= DOCKER_AGGREGATED_DISK_USAGE_API_MINOR
    {
        let df = docker
            .df(None::<bollard::query_parameters::DataUsageOptions>)
            .await?;
        summarize_disk_usage(df, collected_at)
    } else {
        // Bollard 0.21 的公开模型不再包含旧字段，需要直接解析协商版本返回的明细数据。
        let df = load_legacy_system_df(&docker).await?;
        summarize_legacy_disk_usage(df, collected_at)
    };
    Ok(ApiResponse::success_with_raw("Docker system df loaded", Some(summary)).into_response())
}

/// 通过协商后的 Docker API 版本加载旧版磁盘统计响应。
async fn load_legacy_system_df(docker: &Docker) -> ApiResult<LegacySystemDataUsageResponse> {
    let client = reqwest::Client::builder()
        .unix_socket(DOCKER_SOCKET_PATH)
        .build()
        .map_err(|err| ApiError::Internal(format!("Failed to create Docker API client: {err}")))?;
    let url = format!("http://localhost/v{}/system/df", docker.client_version());
    client
        .get(url)
        .send()
        .await
        .map_err(|err| ApiError::Internal(format!("Docker system df request failed: {err}")))?
        .error_for_status()
        .map_err(|err| ApiError::Internal(format!("Docker system df returned an error: {err}")))?
        .json()
        .await
        .map_err(|err| ApiError::Internal(format!("Invalid Docker system df response: {err}")))
}

/// Docker API 1.51 及以下使用的磁盘统计结构。
#[derive(Debug, Deserialize)]
struct LegacySystemDataUsageResponse {
    #[serde(rename = "LayersSize")]
    layers_size: Option<i64>,
    #[serde(rename = "Images")]
    images: Option<Vec<bollard::models::ImageSummary>>,
    #[serde(rename = "Containers")]
    containers: Option<Vec<bollard::models::ContainerSummary>>,
    #[serde(rename = "Volumes")]
    volumes: Option<Vec<bollard::models::Volume>>,
    #[serde(rename = "BuildCache")]
    build_cache: Option<Vec<bollard::models::BuildCache>>,
}

/// 将 Docker Engine 的磁盘统计归一化为稳定领域模型。
fn summarize_disk_usage(
    df: bollard::models::SystemDataUsageResponse,
    collected_at: i64,
) -> DockerDiskUsageSummary {
    let images = df.image_usage.unwrap_or_default();
    let containers = df.container_usage.unwrap_or_default();
    let volumes = df.volume_usage.unwrap_or_default();
    let build_cache = df.build_cache_usage.unwrap_or_default();
    DockerDiskUsageSummary {
        collected_at,
        images: disk_usage_category(
            images.total_count,
            images.active_count,
            images.total_size,
            images.reclaimable,
        ),
        containers: disk_usage_category(
            containers.total_count,
            containers.active_count,
            containers.total_size,
            containers.reclaimable,
        ),
        volumes: disk_usage_category(
            volumes.total_count,
            volumes.active_count,
            volumes.total_size,
            volumes.reclaimable,
        ),
        build_cache: disk_usage_category(
            build_cache.total_count,
            build_cache.active_count,
            build_cache.total_size,
            build_cache.reclaimable,
        ),
    }
}

/// 将旧版 Docker Engine 的明细磁盘数据归一化为稳定领域模型。
fn summarize_legacy_disk_usage(
    df: LegacySystemDataUsageResponse,
    collected_at: i64,
) -> DockerDiskUsageSummary {
    let images = df.images.unwrap_or_default();
    let containers = df.containers.unwrap_or_default();
    let volumes = df.volumes.unwrap_or_default();
    let build_cache = df.build_cache.unwrap_or_default();
    DockerDiskUsageSummary {
        collected_at,
        images: DockerDiskUsageCategory {
            total_count: images.len(),
            active_count: images.iter().filter(|image| image.containers > 0).count(),
            size_bytes: positive_bytes(df.layers_size.unwrap_or_default()),
            reclaimable_bytes: images
                .iter()
                .filter(|image| image.containers == 0)
                .map(|image| positive_bytes(image.size.saturating_sub(image.shared_size.max(0))))
                .sum(),
        },
        containers: DockerDiskUsageCategory {
            total_count: containers.len(),
            active_count: containers
                .iter()
                .filter(|container| is_active_container_state(&container.state))
                .count(),
            size_bytes: containers
                .iter()
                .map(|container| positive_bytes(container.size_rw.unwrap_or_default()))
                .sum(),
            reclaimable_bytes: containers
                .iter()
                .filter(|container| !is_active_container_state(&container.state))
                .map(|container| positive_bytes(container.size_rw.unwrap_or_default()))
                .sum(),
        },
        volumes: DockerDiskUsageCategory {
            total_count: volumes.len(),
            active_count: volumes
                .iter()
                .filter(|volume| {
                    volume
                        .usage_data
                        .as_ref()
                        .is_some_and(|usage| usage.ref_count > 0)
                })
                .count(),
            size_bytes: volumes
                .iter()
                .filter_map(|volume| volume.usage_data.as_ref())
                .map(|usage| positive_bytes(usage.size))
                .sum(),
            reclaimable_bytes: volumes
                .iter()
                .filter_map(|volume| volume.usage_data.as_ref())
                .filter(|usage| usage.ref_count == 0)
                .map(|usage| positive_bytes(usage.size))
                .sum(),
        },
        build_cache: DockerDiskUsageCategory {
            total_count: build_cache.len(),
            active_count: build_cache
                .iter()
                .filter(|cache| cache.in_use.unwrap_or(false))
                .count(),
            size_bytes: build_cache
                .iter()
                .map(|cache| positive_bytes(cache.size.unwrap_or_default()))
                .sum(),
            reclaimable_bytes: build_cache
                .iter()
                .filter(|cache| !cache.in_use.unwrap_or(false) && !cache.shared.unwrap_or(false))
                .map(|cache| positive_bytes(cache.size.unwrap_or_default()))
                .sum(),
        },
    }
}

/// 判断容器状态是否应计入 Docker 活跃容器。
fn is_active_container_state(state: &Option<bollard::models::ContainerSummaryStateEnum>) -> bool {
    matches!(
        state,
        Some(
            bollard::models::ContainerSummaryStateEnum::RUNNING
                | bollard::models::ContainerSummaryStateEnum::PAUSED
                | bollard::models::ContainerSummaryStateEnum::RESTARTING
        )
    )
}

/// 将 Docker 返回的单类磁盘汇总转换为稳定领域模型。
fn disk_usage_category(
    total_count: Option<i64>,
    active_count: Option<i64>,
    total_size: Option<i64>,
    reclaimable: Option<i64>,
) -> DockerDiskUsageCategory {
    DockerDiskUsageCategory {
        total_count: positive_count(total_count.unwrap_or_default()),
        active_count: positive_count(active_count.unwrap_or_default()),
        size_bytes: positive_bytes(total_size.unwrap_or_default()),
        reclaimable_bytes: positive_bytes(reclaimable.unwrap_or_default()),
    }
}

/// 将 Docker 可能返回的负数或未知数量归一化为零。
fn positive_count(value: i64) -> usize {
    value.max(0) as usize
}

/// 将 Docker 可能返回的负数或未知容量归一化为零。
fn positive_bytes(value: i64) -> u64 {
    value.max(0) as u64
}

/// 执行一键清理（清理停止的容器、未使用的网络和挂起镜像）。
pub async fn system_prune(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
) -> ApiResult<Response> {
    info!("Requesting docker system prune");
    let result: ApiResult<Response> = async {
        let output = Command::new("docker")
            .args(["system", "prune", "-f"])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ApiError::Internal(format!(
                "Docker system prune failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(
            ApiResponse::success_with_raw("Docker system prune completed", Some(stdout))
                .into_response(),
        )
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_system_prune",
            Some(("system", "docker")),
            json!({}),
            true,
            result,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::{LegacySystemDataUsageResponse, summarize_disk_usage, summarize_legacy_disk_usage};
    use bollard::models::{
        BuildCache, BuildCacheDiskUsage, ContainerSummary, ContainerSummaryStateEnum,
        ContainersDiskUsage, ImageSummary, ImagesDiskUsage, SystemDataUsageResponse,
        VolumesDiskUsage,
    };

    #[test]
    fn disk_usage_uses_docker_aggregated_values() {
        let summary = summarize_disk_usage(
            SystemDataUsageResponse {
                image_usage: Some(ImagesDiskUsage {
                    total_count: Some(12),
                    active_count: Some(1),
                    total_size: Some(3_852_273_592),
                    reclaimable: Some(3_140_794_611),
                    items: None,
                }),
                container_usage: Some(ContainersDiskUsage {
                    total_count: Some(4),
                    active_count: Some(3),
                    total_size: Some(100),
                    reclaimable: Some(40),
                    items: None,
                }),
                volume_usage: Some(VolumesDiskUsage {
                    total_count: Some(3),
                    active_count: Some(1),
                    total_size: Some(147_456),
                    reclaimable: Some(73_728),
                    items: None,
                }),
                build_cache_usage: Some(BuildCacheDiskUsage {
                    total_count: Some(217),
                    active_count: Some(9),
                    total_size: Some(25_369_267_364),
                    reclaimable: Some(0),
                    items: None,
                }),
            },
            123,
        );

        assert_eq!(summary.collected_at, 123);
        assert_eq!(summary.images.total_count, 12);
        assert_eq!(summary.images.active_count, 1);
        assert_eq!(summary.images.size_bytes, 3_852_273_592);
        assert_eq!(summary.images.reclaimable_bytes, 3_140_794_611);
        assert_eq!(summary.containers.active_count, 3);
        assert_eq!(summary.containers.size_bytes, 100);
        assert_eq!(summary.containers.reclaimable_bytes, 40);
        assert_eq!(summary.volumes.reclaimable_bytes, 73_728);
        assert_eq!(summary.build_cache.size_bytes, 25_369_267_364);
        assert_eq!(summary.build_cache.reclaimable_bytes, 0);
    }

    #[test]
    fn legacy_disk_usage_preserves_docker_28_semantics() {
        let summary = summarize_legacy_disk_usage(
            LegacySystemDataUsageResponse {
                layers_size: Some(300),
                images: Some(vec![
                    ImageSummary {
                        containers: 0,
                        size: 250,
                        shared_size: 100,
                        ..Default::default()
                    },
                    ImageSummary {
                        containers: 1,
                        size: 200,
                        shared_size: 100,
                        ..Default::default()
                    },
                ]),
                containers: Some(vec![
                    container(ContainerSummaryStateEnum::RUNNING, 10),
                    container(ContainerSummaryStateEnum::PAUSED, 20),
                    container(ContainerSummaryStateEnum::RESTARTING, 30),
                    container(ContainerSummaryStateEnum::EXITED, 40),
                ]),
                volumes: None,
                build_cache: Some(vec![
                    build_cache(false, false, 100),
                    build_cache(false, true, 200),
                    build_cache(true, false, 300),
                ]),
            },
            456,
        );

        assert_eq!(summary.collected_at, 456);
        assert_eq!(summary.images.total_count, 2);
        assert_eq!(summary.images.active_count, 1);
        assert_eq!(summary.images.size_bytes, 300);
        assert_eq!(summary.images.reclaimable_bytes, 150);
        assert_eq!(summary.containers.active_count, 3);
        assert_eq!(summary.containers.size_bytes, 100);
        assert_eq!(summary.containers.reclaimable_bytes, 40);
        assert_eq!(summary.build_cache.size_bytes, 600);
        assert_eq!(summary.build_cache.reclaimable_bytes, 100);
    }

    fn container(state: ContainerSummaryStateEnum, size_rw: i64) -> ContainerSummary {
        ContainerSummary {
            state: Some(state),
            size_rw: Some(size_rw),
            ..Default::default()
        }
    }

    fn build_cache(in_use: bool, shared: bool, size: i64) -> BuildCache {
        BuildCache {
            in_use: Some(in_use),
            shared: Some(shared),
            size: Some(size),
            ..Default::default()
        }
    }
}
