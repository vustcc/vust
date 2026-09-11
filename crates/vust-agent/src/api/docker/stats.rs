//! Docker 统计 API：实时与缓存的资源数据输出。

use crate::config;
use crate::models::docker;
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};
use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use bollard::query_parameters;
use chrono::Utc;
use serde::Deserialize;
use sqlx::{FromRow, QueryBuilder, Sqlite};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::info;

/// 资源趋势批量查询允许的最大容器数。
const MAX_HISTORY_CONTAINER_IDS: usize = 7;

/// 查询统计历史的时间窗口配置。
#[derive(Debug, Deserialize)]
pub struct StatsHistoryQuery {
    pub hours: Option<i64>,
}

#[derive(Debug, FromRow)]
struct SummaryRow {
    created_at: i64,
    cpu_core_percent: f64,
    cpu_host_percent: f64,
    memory_working_set_bytes: i64,
    memory_limit_bytes: i64,
    memory_percent: f64,
    running_container_count: i64,
    sampled_container_count: i64,
}

#[derive(Debug, FromRow)]
struct ContainerRow {
    container_id: String,
    created_at: i64,
    cpu_core_percent: f64,
    memory_working_set_bytes: i64,
    memory_limit_bytes: i64,
    memory_percent: f64,
    network_rx_bytes: i64,
    network_tx_bytes: i64,
}

/// 返回全局资源统计的历史趋势数据。
pub async fn history(
    State(state): State<Arc<AppState>>,
    payload: Option<Json<StatsHistoryQuery>>,
) -> ApiResult<Response> {
    let hours = clamp_hours(payload.as_ref().and_then(|value| value.hours));
    let cutoff = Utc::now().timestamp() - hours * 3600;

    let rows = sqlx::query_as::<_, SummaryRow>(
        r#"
        SELECT
            created_at,
            cpu_core_percent,
            cpu_host_percent,
            memory_working_set_bytes,
            memory_limit_bytes,
            memory_percent,
            running_container_count,
            sampled_container_count
        FROM docker_metrics_summary
        WHERE created_at >= ?
        ORDER BY created_at ASC
        "#,
    )
    .bind(cutoff)
    .fetch_all(&state.metadata_db)
    .await?;

    let points = rows.into_iter().map(row_to_summary_point).collect();
    let response = docker::HostResourceUsageHistory { points };
    Ok(
        ApiResponse::success_with_raw("Resource usage history loaded", Some(response))
            .into_response(),
    )
}

/// 返回单个容器的最新资源统计快照。
pub async fn container_summary(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    info!(
        "Requesting container resource usage summary (cache): {}",
        id
    );
    let row = sqlx::query_as::<_, ContainerRow>(
        r#"
        SELECT
            container_id,
            created_at,
            cpu_core_percent,
            memory_working_set_bytes,
            memory_limit_bytes,
            memory_percent,
            network_rx_bytes,
            network_tx_bytes
        FROM docker_metrics_container
        WHERE container_id = ?
        ORDER BY created_at DESC
        LIMIT 2
        "#,
    )
    .bind(&id)
    .fetch_all(&state.metadata_db)
    .await?;

    let summary = row_to_container_summary(&row, Utc::now().timestamp());
    Ok(
        ApiResponse::success_with_raw("Container resource summary loaded", Some(summary))
            .into_response(),
    )
}

/// 返回单个容器在时间窗口内的资源趋势。
pub async fn container_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    payload: Option<Json<StatsHistoryQuery>>,
) -> ApiResult<Response> {
    let hours = clamp_hours(payload.as_ref().and_then(|value| value.hours));
    let cutoff = Utc::now().timestamp() - hours * 3600;

    let rows = sqlx::query_as::<_, ContainerRow>(
        r#"
        SELECT
            container_id,
            created_at,
            cpu_core_percent,
            memory_working_set_bytes,
            memory_limit_bytes,
            memory_percent,
            network_rx_bytes,
            network_tx_bytes
        FROM docker_metrics_container
        WHERE container_id = ?
          AND created_at >= ?
        ORDER BY created_at ASC
        "#,
    )
    .bind(&id)
    .bind(cutoff)
    .fetch_all(&state.metadata_db)
    .await?;

    let points = rows_to_container_points(rows);
    let response = docker::ContainerStatsHistoryAllResponse {
        containers: vec![docker::ContainerStatsHistoryAllItem {
            id: id.clone(),
            name: id,
            points,
        }],
    };
    Ok(
        ApiResponse::success_with_raw("Container resource history loaded", Some(response))
            .into_response(),
    )
}

/// 批量返回多个容器的历史趋势，并补齐容器名称。
pub async fn container_histories(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<docker::ContainerStatsHistoryQuery>,
) -> ApiResult<Response> {
    info!("Requesting batch container resource usage history (cache)");
    let ids = normalize_stats_ids(payload.ids, MAX_HISTORY_CONTAINER_IDS)?;
    let hours = clamp_hours(payload.hours);
    let cutoff = Utc::now().timestamp() - hours * 3600;

    let docker = state.docker_client().await?;
    let options = query_parameters::ListContainersOptionsBuilder::new()
        .all(true)
        .build();
    let containers = docker.list_containers(Some(options)).await?;
    let mut name_map: HashMap<String, String> = HashMap::new();
    for container in containers {
        if let Some(id) = container.id {
            let name = container
                .names
                .as_ref()
                .and_then(|names| names.first())
                .map(|value| value.trim_start_matches('/').to_string())
                .unwrap_or_else(|| id.clone());
            name_map.insert(id, name);
        }
    }

    let rows = query_history_rows(&state, &ids, cutoff).await?;
    let mut grouped = group_container_rows(rows);
    let mut containers = Vec::with_capacity(ids.len());
    for id in ids {
        let rows = grouped.remove(&id).unwrap_or_default();
        containers.push(docker::ContainerStatsHistoryAllItem {
            name: name_map.get(&id).cloned().unwrap_or_else(|| id.clone()),
            id,
            points: rows_to_container_points(rows),
        });
    }
    containers.sort_by(|a, b| a.name.cmp(&b.name));

    let response = docker::ContainerStatsHistoryAllResponse { containers };
    Ok(
        ApiResponse::success_with_raw("Container resource history loaded", Some(response))
            .into_response(),
    )
}

/// 批量返回多个容器的最新资源统计快照。
pub async fn container_summaries(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<docker::ContainerStatsBatchRequest>,
) -> ApiResult<Response> {
    info!(
        "Requesting batch container resource usage summary (cache): {} ids",
        payload.ids.len()
    );
    let ids = normalize_stats_ids(payload.ids, 50)?;
    let mut summaries: HashMap<String, docker::ContainerResourceUsageSummary> = HashMap::new();
    let rows = query_latest_rows(&state, &ids).await?;
    let mut grouped = group_container_rows(rows);
    let now = Utc::now().timestamp();
    for id in ids {
        let rows = grouped.remove(&id).unwrap_or_default();
        summaries.insert(id, row_to_container_summary(&rows, now));
    }

    let response = docker::ContainerStatsBatchResponse { summaries };
    Ok(
        ApiResponse::success_with_raw("Container resource summary loaded", Some(response))
            .into_response(),
    )
}

/// 使用单次 SQL 查询多个容器的时间窗口数据。
async fn query_history_rows(
    state: &AppState,
    ids: &[String],
    cutoff: i64,
) -> ApiResult<Vec<ContainerRow>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
        SELECT
            container_id,
            created_at,
            cpu_core_percent,
            memory_working_set_bytes,
            memory_limit_bytes,
            memory_percent,
            network_rx_bytes,
            network_tx_bytes
        FROM docker_metrics_container
        WHERE container_id IN (
        "#,
    );
    {
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
    }
    query
        .push(") AND created_at >= ")
        .push_bind(cutoff)
        .push(" ORDER BY container_id ASC, created_at ASC");
    Ok(query
        .build_query_as::<ContainerRow>()
        .fetch_all(&state.metadata_db)
        .await?)
}

/// 使用窗口函数一次读取每个容器最近两个采样点。
async fn query_latest_rows(state: &AppState, ids: &[String]) -> ApiResult<Vec<ContainerRow>> {
    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
        SELECT
            container_id,
            created_at,
            cpu_core_percent,
            memory_working_set_bytes,
            memory_limit_bytes,
            memory_percent,
            network_rx_bytes,
            network_tx_bytes
        FROM (
            SELECT
                container_id,
                created_at,
                cpu_core_percent,
                memory_working_set_bytes,
                memory_limit_bytes,
                memory_percent,
                network_rx_bytes,
                network_tx_bytes,
                ROW_NUMBER() OVER (
                    PARTITION BY container_id ORDER BY created_at DESC
                ) AS sample_rank
            FROM docker_metrics_container
            WHERE container_id IN (
        "#,
    );
    {
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
    }
    query.push(
        r#"
            )
        )
        WHERE sample_rank <= 2
        ORDER BY container_id ASC, created_at DESC
        "#,
    );
    Ok(query
        .build_query_as::<ContainerRow>()
        .fetch_all(&state.metadata_db)
        .await?)
}

/// 按容器 ID 聚合已按时间排序的采样行。
fn group_container_rows(rows: Vec<ContainerRow>) -> HashMap<String, Vec<ContainerRow>> {
    let mut grouped: HashMap<String, Vec<ContainerRow>> = HashMap::new();
    for row in rows {
        grouped
            .entry(row.container_id.clone())
            .or_default()
            .push(row);
    }
    grouped
}

/// 清理并限制统计查询的容器 ID。
fn normalize_stats_ids(ids: Vec<String>, limit: usize) -> ApiResult<Vec<String>> {
    if ids.is_empty() {
        return Err(ApiError::validation("container ids must not be empty"));
    }
    if ids.len() > limit {
        return Err(ApiError::validation(format!(
            "at most {limit} containers can be queried at once"
        )));
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(ids.len());
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            return Err(ApiError::validation("container id must not be empty"));
        }
        if seen.insert(id.to_string()) {
            normalized.push(id.to_string());
        }
    }
    Ok(normalized)
}

fn clamp_hours(value: Option<i64>) -> i64 {
    let max_hours = config::stats_retention_hours() as i64;
    let hours = value.unwrap_or(max_hours).max(1);
    hours.min(max_hours)
}

fn row_to_container_summary(
    rows: &[ContainerRow],
    now: i64,
) -> docker::ContainerResourceUsageSummary {
    let Some(current) = rows.first() else {
        return unavailable_container_summary();
    };
    let previous = rows.get(1);
    let (network_rx_bytes_per_second, network_tx_bytes_per_second) =
        network_rates(previous, current);
    let stale_after = config::stats_sample_interval().as_secs() as i64 * 2;
    let status = if now - current.created_at > stale_after {
        docker::ResourceSampleStatus::Stale
    } else {
        docker::ResourceSampleStatus::Fresh
    };
    docker::ContainerResourceUsageSummary {
        status,
        collected_at: Some(current.created_at),
        cpu_core_percent: current.cpu_core_percent,
        memory_working_set_bytes: current.memory_working_set_bytes.max(0) as u64,
        memory_limit_bytes: current.memory_limit_bytes.max(0) as u64,
        memory_percent: current.memory_percent,
        network_rx_bytes_per_second,
        network_tx_bytes_per_second,
    }
}

/// 构造没有任何可用采样的容器资源状态。
fn unavailable_container_summary() -> docker::ContainerResourceUsageSummary {
    docker::ContainerResourceUsageSummary {
        status: docker::ResourceSampleStatus::Unavailable,
        collected_at: None,
        cpu_core_percent: 0.0,
        memory_working_set_bytes: 0,
        memory_limit_bytes: 0,
        memory_percent: 0.0,
        network_rx_bytes_per_second: None,
        network_tx_bytes_per_second: None,
    }
}

fn row_to_summary_point(row: SummaryRow) -> docker::HostResourceUsagePoint {
    docker::HostResourceUsagePoint {
        timestamp: row.created_at,
        cpu_host_percent: row.cpu_host_percent,
        cpu_core_percent: row.cpu_core_percent,
        memory_working_set_bytes: row.memory_working_set_bytes as u64,
        memory_limit_bytes: row.memory_limit_bytes as u64,
        memory_percent: row.memory_percent,
        running_container_count: row.running_container_count as usize,
        sampled_container_count: row.sampled_container_count as usize,
    }
}

fn rows_to_container_points(rows: Vec<ContainerRow>) -> Vec<docker::ContainerResourceUsagePoint> {
    let mut previous: Option<&ContainerRow> = None;
    let mut points = Vec::with_capacity(rows.len());
    for row in &rows {
        let (network_rx_bytes_per_second, network_tx_bytes_per_second) =
            network_rates(previous, row);
        points.push(docker::ContainerResourceUsagePoint {
            timestamp: row.created_at,
            cpu_core_percent: row.cpu_core_percent,
            memory_working_set_bytes: row.memory_working_set_bytes as u64,
            memory_limit_bytes: row.memory_limit_bytes as u64,
            memory_percent: row.memory_percent,
            network_rx_bytes_per_second,
            network_tx_bytes_per_second,
        });
        previous = Some(row);
    }
    points
}

/// 根据相邻累计计数计算网络速率，重置或长间断返回空值。
fn network_rates(
    previous: Option<&ContainerRow>,
    current: &ContainerRow,
) -> (Option<f64>, Option<f64>) {
    let max_gap_seconds = (config::stats_sample_interval().as_secs() as i64 * 5) / 2;
    previous
        .filter(|previous| {
            let gap = current.created_at - previous.created_at;
            gap > 0
                && gap <= max_gap_seconds
                && current.network_rx_bytes >= previous.network_rx_bytes
                && current.network_tx_bytes >= previous.network_tx_bytes
        })
        .map(|previous| {
            let elapsed = (current.created_at - previous.created_at) as f64;
            (
                Some((current.network_rx_bytes - previous.network_rx_bytes) as f64 / elapsed),
                Some((current.network_tx_bytes - previous.network_tx_bytes) as f64 / elapsed),
            )
        })
        .unwrap_or((None, None))
}

#[cfg(test)]
mod tests {
    use super::{
        ContainerRow, MAX_HISTORY_CONTAINER_IDS, normalize_stats_ids, row_to_container_summary,
        rows_to_container_points,
    };
    use crate::config;
    use crate::models::docker::ResourceSampleStatus;

    fn row(created_at: i64, rx: i64, tx: i64) -> ContainerRow {
        ContainerRow {
            container_id: "container".to_string(),
            created_at,
            cpu_core_percent: 120.0,
            memory_working_set_bytes: 1024,
            memory_limit_bytes: 4096,
            memory_percent: 25.0,
            network_rx_bytes: rx,
            network_tx_bytes: tx,
        }
    }

    #[test]
    fn network_rate_handles_normal_counter_growth() {
        let points = rows_to_container_points(vec![row(100, 100, 200), row(160, 700, 500)]);
        assert_eq!(points[0].network_rx_bytes_per_second, None);
        assert_eq!(points[1].network_rx_bytes_per_second, Some(10.0));
        assert_eq!(points[1].network_tx_bytes_per_second, Some(5.0));
    }

    #[test]
    fn network_rate_ignores_counter_reset_and_long_gap() {
        let points = rows_to_container_points(vec![
            row(100, 1000, 1000),
            row(160, 100, 100),
            row(400, 500, 500),
        ]);
        assert_eq!(points[1].network_rx_bytes_per_second, None);
        assert_eq!(points[2].network_rx_bytes_per_second, None);
    }

    #[test]
    fn latest_summary_exposes_rate_and_freshness() {
        let interval = config::stats_sample_interval().as_secs() as i64;
        let now = 10_000;
        let rows = vec![row(now - 1, 700, 500), row(now - 61, 100, 200)];
        let summary = row_to_container_summary(&rows, now);
        assert_eq!(summary.status, ResourceSampleStatus::Fresh);
        assert_eq!(summary.network_rx_bytes_per_second, Some(10.0));
        assert_eq!(summary.network_tx_bytes_per_second, Some(5.0));

        let stale = row_to_container_summary(&[row(now - interval * 3, 100, 100)], now);
        assert_eq!(stale.status, ResourceSampleStatus::Stale);
        let unavailable = row_to_container_summary(&[], now);
        assert_eq!(unavailable.status, ResourceSampleStatus::Unavailable);
    }

    #[test]
    fn stats_query_ids_are_stable_and_bounded() {
        let ids = normalize_stats_ids(
            vec![
                " first ".to_string(),
                "second".to_string(),
                "first".to_string(),
            ],
            MAX_HISTORY_CONTAINER_IDS,
        )
        .expect("ids");
        assert_eq!(ids, vec!["first", "second"]);
        assert!(normalize_stats_ids(Vec::new(), MAX_HISTORY_CONTAINER_IDS).is_err());
        assert!(
            normalize_stats_ids(
                (1..=MAX_HISTORY_CONTAINER_IDS)
                    .map(|index| format!("id-{index}"))
                    .collect(),
                MAX_HISTORY_CONTAINER_IDS,
            )
            .is_ok()
        );
        assert!(
            normalize_stats_ids(
                (1..=MAX_HISTORY_CONTAINER_IDS + 1)
                    .map(|index| format!("id-{index}"))
                    .collect(),
                MAX_HISTORY_CONTAINER_IDS,
            )
            .is_err()
        );
    }
}
