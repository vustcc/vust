//! 系统监控 API：提供稳定的实时概览、历史趋势与采集设置。

use axum::{
    Json, Router,
    extract::{FromRequestParts, Query, State, rejection::JsonRejection},
    http::HeaderName,
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::{collections::BTreeMap, sync::Arc};
use vust_contracts::{
    api::ErrorCode,
    monitoring::{
        SystemMonitoringCapabilities, SystemMonitoringCollectionState,
        SystemMonitoringHistorySummary, SystemMonitoringMetrics, SystemMonitoringOverview,
        SystemMonitoringOwnership, SystemMonitoringPageInfo, SystemMonitoringSeriesPage,
        SystemMonitoringSeriesPoint, SystemMonitoringSeriesStatus, SystemMonitoringSettings,
        SystemMonitoringSnapshotStatus, SystemMonitoringSourceState, SystemMonitoringSourceStatus,
    },
};

use crate::{
    services::system_monitoring::{
        HISTORY_SAMPLE_INTERVAL_SECONDS, RawHostMetricSample, STALE_AFTER_SECONDS,
        SystemMonitoringStorageSettings,
    },
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};

const ACTOR_KIND_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-kind");
const ACTOR_NAME_HEADER: HeaderName = HeaderName::from_static("x-vust-actor-name");
const MAX_SERIES_LIMIT: u16 = 500;

/// 由 Master 注入且通过内部链路传递的变更操作上下文。
struct TrustedOperationContext;

impl<S> FromRequestParts<S> for TrustedOperationContext
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let actor_kind = parts
            .headers
            .get(&ACTOR_KIND_HEADER)
            .and_then(|value| value.to_str().ok());
        let actor_name = parts
            .headers
            .get(&ACTOR_NAME_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if matches!(actor_kind, Some("user" | "system")) && actor_name.is_some() {
            return Ok(Self);
        }
        Err(ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "trusted operation context is required",
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemMonitoringSettingsUpdate {
    pub history_collection_enabled: bool,
    pub retention_days: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemMonitoringSeriesQuery {
    #[serde(default = "default_range")]
    pub range: String,
    #[serde(default = "default_sort")]
    pub sort: String,
    #[serde(default = "default_limit")]
    pub limit: u16,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClearHistoryResult {
    cleared_sample_count: u64,
}

#[derive(Debug, Clone, FromRow)]
struct StoredSample {
    sampled_at: i64,
    cpu_percent: Option<f64>,
    memory_used_bytes: Option<i64>,
    memory_total_bytes: Option<i64>,
    memory_percent: Option<f64>,
    load_average_1m: Option<f64>,
    load_average_5m: Option<f64>,
    load_average_15m: Option<f64>,
    disk_read_bytes: Option<i64>,
    disk_write_bytes: Option<i64>,
    network_receive_bytes: Option<i64>,
    network_transmit_bytes: Option<i64>,
    available_source_count: i64,
}

impl StoredSample {
    fn to_raw(&self) -> RawHostMetricSample {
        RawHostMetricSample {
            sampled_at: self.sampled_at,
            cpu_percent: self.cpu_percent,
            memory_used_bytes: as_u64(self.memory_used_bytes),
            memory_total_bytes: as_u64(self.memory_total_bytes),
            memory_percent: self.memory_percent,
            load_average_1m: self.load_average_1m,
            load_average_5m: self.load_average_5m,
            load_average_15m: self.load_average_15m,
            disk_read_bytes: as_u64(self.disk_read_bytes),
            disk_write_bytes: as_u64(self.disk_write_bytes),
            network_receive_bytes: as_u64(self.network_receive_bytes),
            network_transmit_bytes: as_u64(self.network_transmit_bytes),
            source_statuses: Vec::new(),
        }
    }
}

/// 构建 Agent 内部系统监控路由。
pub fn system_monitoring_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/overview", get(overview))
        .route("/series", get(series))
        .route("/settings", get(settings).put(update_settings))
        .route("/history", delete(clear_history))
}

/// 返回缓存中的实时概览，不触发新的宿主机采样。
async fn overview(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let latest = state.system_monitoring.latest_sample.read().await.clone();
    let previous = state.system_monitoring.previous_sample.read().await.clone();
    let storage_settings = *state.system_monitoring.settings.read().await;
    let collection_state = *state.system_monitoring.collection_state.read().await;
    let last_sampled_at = load_last_history_sample(&state).await?;

    let now = Utc::now().timestamp();
    let (snapshot_status, coverage_percent, observed_at, sources, metrics) = match latest {
        Some(sample) => {
            let coverage = f64::from(sample.available_source_count()) / 5.0 * 100.0;
            let status = if now - sample.sampled_at > STALE_AFTER_SECONDS {
                SystemMonitoringSnapshotStatus::Stale
            } else if sample.available_source_count() < 5 {
                SystemMonitoringSnapshotStatus::Partial
            } else {
                SystemMonitoringSnapshotStatus::Fresh
            };
            let metrics = sample.metrics(previous.as_ref());
            (
                status,
                coverage,
                Some(format_epoch(sample.sampled_at)?),
                sample.source_statuses,
                metrics,
            )
        }
        None => (
            SystemMonitoringSnapshotStatus::Unavailable,
            0.0,
            None,
            unavailable_sources(),
            empty_metrics(),
        ),
    };

    let response = SystemMonitoringOverview {
        ownership: SystemMonitoringOwnership::System,
        observed_at,
        snapshot_status,
        coverage_percent,
        sources,
        metrics,
        history: SystemMonitoringHistorySummary {
            state: collection_state,
            sample_interval_seconds: HISTORY_SAMPLE_INTERVAL_SECONDS,
            retention_days: storage_settings.retention_days,
            last_sampled_at: last_sampled_at.map(format_epoch).transpose()?,
        },
        capabilities: capabilities(),
    };
    Ok(
        ApiResponse::success_with_raw("System monitoring overview loaded", Some(response))
            .into_response(),
    )
}

/// 返回固定分辨率并显式包含缺失桶的历史趋势。
async fn series(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SystemMonitoringSeriesQuery>,
) -> ApiResult<Response> {
    let range = parse_range(&query.range)?;
    if query.limit == 0 || query.limit > MAX_SERIES_LIMIT {
        return Err(invalid_range("limit must be between 1 and 500"));
    }
    if query.sort != "asc" && query.sort != "desc" {
        return Err(invalid_range("sort must be asc or desc"));
    }
    let settings = *state.system_monitoring.settings.read().await;
    if range.duration_seconds > i64::from(settings.retention_days) * 86_400 {
        return Err(invalid_range("range exceeds configured retention"));
    }
    let offset = query
        .cursor
        .as_deref()
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| invalid_range("cursor is invalid"))?
        .unwrap_or_default();

    let to = Utc::now().timestamp();
    let from = to - range.duration_seconds;
    let rows = load_series_rows(&state, from, to).await?;
    let mut points = build_series_points(&rows, from, to, range.resolution_seconds)?;
    let expected_point_count = points.len() as u32;
    let actual_point_count = points
        .iter()
        .filter(|point| point.coverage_percent > 0.0)
        .count() as u32;
    let coverage_percent = if expected_point_count == 0 {
        0.0
    } else {
        points
            .iter()
            .map(|point| point.coverage_percent)
            .sum::<f64>()
            / f64::from(expected_point_count)
    };
    if query.sort == "desc" {
        points.reverse();
    }
    if offset > points.len() {
        return Err(invalid_range("cursor is outside the result set"));
    }
    let end = (offset + usize::from(query.limit)).min(points.len());
    let has_more = end < points.len();
    let page_points = points[offset..end].to_vec();
    let collection_state = *state.system_monitoring.collection_state.read().await;
    let newest_sampled_at = rows.last().map(|row| row.sampled_at);
    let series_status = if actual_point_count == 0 {
        SystemMonitoringSeriesStatus::Empty
    } else if collection_state == SystemMonitoringCollectionState::Stopped
        || newest_sampled_at.is_none_or(|sampled_at| to - sampled_at > 120)
    {
        SystemMonitoringSeriesStatus::Stale
    } else if coverage_percent < 100.0 {
        SystemMonitoringSeriesStatus::Partial
    } else {
        SystemMonitoringSeriesStatus::Complete
    };

    let response = SystemMonitoringSeriesPage {
        range: query.range,
        from: format_epoch(from)?,
        to: format_epoch(to)?,
        resolution_seconds: range.resolution_seconds as u32,
        series_status,
        expected_point_count,
        actual_point_count,
        coverage_percent,
        points: page_points,
        page_info: SystemMonitoringPageInfo {
            limit: query.limit,
            has_more,
            next_cursor: has_more.then(|| end.to_string()),
        },
    };
    Ok(
        ApiResponse::success_with_raw("System monitoring series loaded", Some(response))
            .into_response(),
    )
}

/// 返回采集设置和当前存储摘要。
async fn settings(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let response = load_settings_response(&state).await?;
    Ok(
        ApiResponse::success_with_raw("System monitoring settings loaded", Some(response))
            .into_response(),
    )
}

/// 原子更新采集设置，并在缩短保留期时同步清理过期样本。
async fn update_settings(
    State(state): State<Arc<AppState>>,
    _context: TrustedOperationContext,
    payload: Result<Json<SystemMonitoringSettingsUpdate>, JsonRejection>,
) -> ApiResult<Response> {
    let Json(payload) = payload.map_err(|_| {
        ApiError::bad_request(
            ErrorCode::SystemMonitoringInvalidSettings,
            "system monitoring settings payload is invalid",
        )
    })?;
    validate_retention(payload.retention_days)?;
    let _maintenance = state
        .system_monitoring
        .maintenance
        .try_lock()
        .map_err(|_| history_busy())?;
    let now = Utc::now().timestamp();
    let cutoff = now - i64::from(payload.retention_days) * 86_400;
    let mut transaction = state.metadata_db.begin().await.map_err(database_error)?;
    sqlx::query(
        r#"
        UPDATE system_monitoring_settings
        SET history_collection_enabled = ?, retention_days = ?, updated_at = ?
        WHERE singleton_id = 1
        "#,
    )
    .bind(payload.history_collection_enabled)
    .bind(payload.retention_days)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    sqlx::query("DELETE FROM system_monitoring_samples WHERE sampled_at < ?")
        .bind(cutoff)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;

    *state.system_monitoring.settings.write().await = SystemMonitoringStorageSettings {
        history_collection_enabled: payload.history_collection_enabled,
        retention_days: payload.retention_days,
    };
    *state.system_monitoring.collection_state.write().await = if payload.history_collection_enabled
    {
        SystemMonitoringCollectionState::Initializing
    } else {
        SystemMonitoringCollectionState::Stopped
    };
    let response = load_settings_response(&state).await?;
    Ok(
        ApiResponse::success_with_raw("System monitoring settings updated", Some(response))
            .into_response(),
    )
}

/// 清空历史；重复维护请求返回稳定的 409。
async fn clear_history(
    State(state): State<Arc<AppState>>,
    _context: TrustedOperationContext,
) -> ApiResult<Response> {
    let _maintenance = state
        .system_monitoring
        .maintenance
        .try_lock()
        .map_err(|_| history_busy())?;
    let result = sqlx::query("DELETE FROM system_monitoring_samples")
        .execute(&state.metadata_db)
        .await
        .map_err(database_error)?;
    Ok(ApiResponse::success_with_raw(
        "System monitoring history cleared",
        Some(ClearHistoryResult {
            cleared_sample_count: result.rows_affected(),
        }),
    )
    .into_response())
}

async fn load_series_rows(state: &AppState, from: i64, to: i64) -> ApiResult<Vec<StoredSample>> {
    sqlx::query_as::<_, StoredSample>(
        r#"
        SELECT sampled_at, cpu_percent, memory_used_bytes, memory_total_bytes, memory_percent,
               load_average_1m, load_average_5m, load_average_15m,
               disk_read_bytes, disk_write_bytes,
               network_receive_bytes, network_transmit_bytes, available_source_count
        FROM system_monitoring_samples
        WHERE sampled_at >= ? AND sampled_at <= ?
        ORDER BY sampled_at ASC
        "#,
    )
    .bind(from - 150)
    .bind(to)
    .fetch_all(&state.metadata_db)
    .await
    .map_err(database_error)
}

fn build_series_points(
    rows: &[StoredSample],
    from: i64,
    to: i64,
    resolution_seconds: i64,
) -> ApiResult<Vec<SystemMonitoringSeriesPoint>> {
    let expected = ((to - from) / resolution_seconds).max(1) as usize;
    let mut buckets: BTreeMap<usize, Vec<(SystemMonitoringMetrics, f64)>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if row.sampled_at < from {
            continue;
        }
        let bucket = ((row.sampled_at - from) / resolution_seconds) as usize;
        if bucket >= expected {
            continue;
        }
        let previous = index
            .checked_sub(1)
            .and_then(|previous_index| rows.get(previous_index))
            .map(StoredSample::to_raw);
        let raw = row.to_raw();
        buckets.entry(bucket).or_default().push((
            raw.metrics(previous.as_ref()),
            row.available_source_count.clamp(0, 5) as f64 / 5.0 * 100.0,
        ));
    }

    (0..expected)
        .map(|index| {
            let sampled_at = from + index as i64 * resolution_seconds;
            let values = buckets.get(&index).map(Vec::as_slice).unwrap_or_default();
            Ok(SystemMonitoringSeriesPoint {
                sampled_at: format_epoch(sampled_at)?,
                coverage_percent: average(values.iter().map(|(_, coverage)| Some(*coverage)))
                    .unwrap_or(0.0),
                metrics: aggregate_metrics(values),
            })
        })
        .collect()
}

fn aggregate_metrics(values: &[(SystemMonitoringMetrics, f64)]) -> SystemMonitoringMetrics {
    let metric = |read: fn(&SystemMonitoringMetrics) -> Option<f64>| {
        average(values.iter().map(|(metrics, _)| read(metrics)))
    };
    let integer_metric = |read: fn(&SystemMonitoringMetrics) -> Option<u64>| {
        average(
            values
                .iter()
                .map(|(metrics, _)| read(metrics).map(|value| value as f64)),
        )
        .map(|value| value.round() as u64)
    };
    SystemMonitoringMetrics {
        cpu_percent: metric(|value| value.cpu_percent),
        memory_used_bytes: integer_metric(|value| value.memory_used_bytes),
        memory_total_bytes: integer_metric(|value| value.memory_total_bytes),
        memory_percent: metric(|value| value.memory_percent),
        load_average_1m: metric(|value| value.load_average_1m),
        load_average_5m: metric(|value| value.load_average_5m),
        load_average_15m: metric(|value| value.load_average_15m),
        disk_read_bytes_per_second: metric(|value| value.disk_read_bytes_per_second),
        disk_write_bytes_per_second: metric(|value| value.disk_write_bytes_per_second),
        network_receive_bytes_per_second: metric(|value| value.network_receive_bytes_per_second),
        network_transmit_bytes_per_second: metric(|value| value.network_transmit_bytes_per_second),
    }
}

fn average(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let (sum, count) = values
        .flatten()
        .fold((0.0, 0_u32), |(sum, count), value| (sum + value, count + 1));
    (count > 0).then(|| sum / f64::from(count))
}

async fn load_settings_response(state: &AppState) -> ApiResult<SystemMonitoringSettings> {
    let settings = *state.system_monitoring.settings.read().await;
    let summary = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
        "SELECT COUNT(*), MIN(sampled_at), MAX(sampled_at) FROM system_monitoring_samples",
    )
    .fetch_one(&state.metadata_db)
    .await
    .map_err(database_error)?;
    Ok(SystemMonitoringSettings {
        ownership: SystemMonitoringOwnership::System,
        history_collection_enabled: settings.history_collection_enabled,
        history_sample_interval_seconds: HISTORY_SAMPLE_INTERVAL_SECONDS,
        retention_days: settings.retention_days,
        stored_sample_count: summary.0.max(0) as u64,
        oldest_sampled_at: summary.1.map(format_epoch).transpose()?,
        newest_sampled_at: summary.2.map(format_epoch).transpose()?,
        capabilities: capabilities(),
    })
}

async fn load_last_history_sample(state: &AppState) -> ApiResult<Option<i64>> {
    sqlx::query_scalar("SELECT MAX(sampled_at) FROM system_monitoring_samples")
        .fetch_one(&state.metadata_db)
        .await
        .map_err(database_error)
}

#[derive(Clone, Copy)]
struct RangeDefinition {
    duration_seconds: i64,
    resolution_seconds: i64,
}

fn parse_range(value: &str) -> ApiResult<RangeDefinition> {
    let definition = match value {
        "1h" => RangeDefinition {
            duration_seconds: 3_600,
            resolution_seconds: 60,
        },
        "6h" => RangeDefinition {
            duration_seconds: 21_600,
            resolution_seconds: 60,
        },
        "24h" => RangeDefinition {
            duration_seconds: 86_400,
            resolution_seconds: 300,
        },
        "3d" => RangeDefinition {
            duration_seconds: 259_200,
            resolution_seconds: 900,
        },
        "7d" => RangeDefinition {
            duration_seconds: 604_800,
            resolution_seconds: 1_800,
        },
        _ => return Err(invalid_range("range must be one of 1h, 6h, 24h, 3d or 7d")),
    };
    Ok(definition)
}

fn validate_retention(value: u8) -> ApiResult<()> {
    if [1, 3, 7].contains(&value) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            ErrorCode::SystemMonitoringInvalidSettings,
            "retentionDays must be 1, 3 or 7",
        ))
    }
}

fn capabilities() -> SystemMonitoringCapabilities {
    SystemMonitoringCapabilities {
        can_manage_collection: true,
        can_clear_history: true,
    }
}

fn unavailable_sources() -> Vec<SystemMonitoringSourceStatus> {
    ["cpu", "memory", "load", "diskIo", "networkIo"]
        .into_iter()
        .map(|source| SystemMonitoringSourceStatus {
            source: source.to_string(),
            state: SystemMonitoringSourceState::Unavailable,
        })
        .collect()
}

fn empty_metrics() -> SystemMonitoringMetrics {
    SystemMonitoringMetrics {
        cpu_percent: None,
        memory_used_bytes: None,
        memory_total_bytes: None,
        memory_percent: None,
        load_average_1m: None,
        load_average_5m: None,
        load_average_15m: None,
        disk_read_bytes_per_second: None,
        disk_write_bytes_per_second: None,
        network_receive_bytes_per_second: None,
        network_transmit_bytes_per_second: None,
    }
}

fn format_epoch(value: i64) -> ApiResult<String> {
    chrono::DateTime::from_timestamp(value, 0)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .ok_or_else(|| ApiError::internal("system monitoring timestamp is out of range"))
}

fn as_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn invalid_range(message: &'static str) -> ApiError {
    ApiError::bad_request(ErrorCode::SystemMonitoringInvalidRange, message)
}

fn history_busy() -> ApiError {
    ApiError::conflict(
        ErrorCode::SystemMonitoringHistoryBusy,
        "system monitoring history maintenance is already running",
    )
}

fn database_error(error: sqlx::Error) -> ApiError {
    ApiError::database(error.to_string())
}

fn default_range() -> String {
    "24h".to_string()
}

fn default_sort() -> String {
    "asc".to_string()
}

fn default_limit() -> u16 {
    MAX_SERIES_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::FromRequestParts;
    use axum::http::{HeaderValue, Request};

    #[test]
    fn range_uses_stable_resolution() {
        assert_eq!(parse_range("1h").unwrap().resolution_seconds, 60);
        assert_eq!(parse_range("24h").unwrap().resolution_seconds, 300);
        assert_eq!(parse_range("7d").unwrap().resolution_seconds, 1_800);
        assert!(parse_range("30d").is_err());
    }

    #[test]
    fn retention_accepts_only_supported_values() {
        for value in [1, 3, 7] {
            assert!(validate_retention(value).is_ok());
        }
        assert!(validate_retention(2).is_err());
    }

    #[test]
    fn settings_payload_rejects_unknown_fields() {
        let payload = serde_json::json!({
            "historyCollectionEnabled": true,
            "retentionDays": 7,
            "force": true,
        });
        assert!(serde_json::from_value::<SystemMonitoringSettingsUpdate>(payload).is_err());
    }

    #[tokio::test]
    async fn mutations_require_trusted_operation_context() {
        let request = Request::builder().body(()).unwrap();
        let (mut parts, _) = request.into_parts();
        assert!(
            TrustedOperationContext::from_request_parts(&mut parts, &())
                .await
                .is_err()
        );

        parts
            .headers
            .insert(&ACTOR_KIND_HEADER, HeaderValue::from_static("user"));
        parts
            .headers
            .insert(&ACTOR_NAME_HEADER, HeaderValue::from_static("admin"));
        assert!(
            TrustedOperationContext::from_request_parts(&mut parts, &())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn settings_update_is_transactional_and_updates_runtime() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        update_settings(
            State(Arc::clone(&state)),
            TrustedOperationContext,
            Ok(Json(SystemMonitoringSettingsUpdate {
                history_collection_enabled: false,
                retention_days: 3,
            })),
        )
        .await
        .unwrap();

        let stored = crate::services::system_monitoring::load_storage_settings(&state.metadata_db)
            .await
            .unwrap();
        assert!(!stored.history_collection_enabled);
        assert_eq!(stored.retention_days, 3);
        assert_eq!(
            *state.system_monitoring.collection_state.read().await,
            SystemMonitoringCollectionState::Stopped
        );
    }

    #[tokio::test]
    async fn overlapping_history_maintenance_returns_conflict() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        let _guard = state.system_monitoring.maintenance.lock().await;
        let error = clear_history(State(Arc::clone(&state)), TrustedOperationContext)
            .await
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.code, ErrorCode::SystemMonitoringHistoryBusy);
    }
}
