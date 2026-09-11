//! Docker 统计采样服务：定时采样并写入缓存。

use crate::config;
use crate::models::docker::{
    ContainerResourceUsageSample, HostResourceUsageSummary, ResourceSampleStatus,
};
use crate::state::AppState;
use bollard::models::ContainerSummaryStateEnum;
use bollard::query_parameters;
use chrono::Utc;
use futures_util::stream::{self, StreamExt};
use sqlx::FromRow;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::time::{Duration, timeout};
use tracing::{debug, warn};

const CONTAINER_STATS_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, FromRow)]
struct LatestSummaryRow {
    created_at: i64,
    cpu_core_percent: f64,
    cpu_host_percent: f64,
    memory_working_set_bytes: i64,
    memory_limit_bytes: i64,
    memory_percent: f64,
    running_container_count: i64,
    sampled_container_count: i64,
}

/// 启动后台采样任务，定期收集并清理 Docker 统计数据。
pub fn spawn_stats_collector(state: Arc<AppState>) {
    let interval = config::stats_sample_interval();
    let interval_secs = interval.as_secs().max(1);
    let cleanup_every = (300 / interval_secs).max(1);

    tokio::spawn(async move {
        debug!(
            "Docker resource usage sampling interval set to {:?} seconds",
            interval
        );
        let mut ticker = tokio::time::interval(interval);
        let mut cleanup_ticks = 0_u64;

        loop {
            ticker.tick().await;

            if let Err(err) = collect_and_store_stats(&state).await {
                warn!("Docker stats sampling failed: {}", err);
            }

            cleanup_ticks += 1;
            if cleanup_ticks >= cleanup_every {
                cleanup_ticks = 0;
                if let Err(err) = cleanup_old_stats(&state).await {
                    warn!("Docker stats cleanup failed: {}", err);
                }
            }
        }
    });
}

/// 从缓存读取最近一次宿主机 Docker 资源汇总，并标注新鲜度。
pub async fn load_latest_summary(state: &AppState) -> anyhow::Result<HostResourceUsageSummary> {
    let row = sqlx::query_as::<_, LatestSummaryRow>(
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
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.metadata_db)
    .await?;

    let Some(row) = row else {
        return Ok(HostResourceUsageSummary {
            status: ResourceSampleStatus::Unavailable,
            collected_at: None,
            running_container_count: 0,
            sampled_container_count: 0,
            cpu_host_percent: 0.0,
            cpu_core_percent: 0.0,
            memory_working_set_bytes: 0,
            memory_limit_bytes: 0,
            memory_percent: 0.0,
        });
    };

    let status = resolve_sample_status(
        Utc::now().timestamp(),
        row.created_at,
        row.running_container_count,
        row.sampled_container_count,
    );
    Ok(HostResourceUsageSummary {
        status,
        collected_at: Some(row.created_at),
        running_container_count: row.running_container_count.max(0) as usize,
        sampled_container_count: row.sampled_container_count.max(0) as usize,
        cpu_host_percent: row.cpu_host_percent,
        cpu_core_percent: row.cpu_core_percent,
        memory_working_set_bytes: row.memory_working_set_bytes.max(0) as u64,
        memory_limit_bytes: row.memory_limit_bytes.max(0) as u64,
        memory_percent: row.memory_percent,
    })
}

/// 根据采样时间与覆盖率判定资源数据状态。
fn resolve_sample_status(
    now: i64,
    collected_at: i64,
    running_container_count: i64,
    sampled_container_count: i64,
) -> ResourceSampleStatus {
    let stale_after = config::stats_sample_interval().as_secs() as i64 * 2;
    if now - collected_at > stale_after {
        ResourceSampleStatus::Stale
    } else if sampled_container_count < running_container_count {
        ResourceSampleStatus::Partial
    } else {
        ResourceSampleStatus::Fresh
    }
}

async fn collect_and_store_stats(state: &AppState) -> anyhow::Result<()> {
    let docker = match state.docker_client().await {
        Ok(client) => client,
        Err(_) => return Ok(()),
    };

    let (summary, samples) = collect_samples_and_summary(&docker).await?;
    let timestamp = Utc::now().timestamp();
    let mut tx = state.metadata_db.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO docker_metrics_summary (
            created_at,
            cpu_core_percent,
            cpu_host_percent,
            memory_working_set_bytes,
            memory_limit_bytes,
            memory_percent,
            running_container_count,
            sampled_container_count
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(timestamp)
    .bind(summary.cpu_core_percent)
    .bind(summary.cpu_host_percent)
    .bind(summary.memory_working_set_bytes as i64)
    .bind(summary.memory_limit_bytes as i64)
    .bind(summary.memory_percent)
    .bind(summary.running_container_count as i64)
    .bind(summary.sampled_container_count as i64)
    .execute(&mut *tx)
    .await?;

    if !samples.is_empty() {
        for (id, sample) in samples {
            sqlx::query(
                r#"
                INSERT INTO docker_metrics_container (
                    container_id,
                    created_at,
                    cpu_core_percent,
                    memory_working_set_bytes,
                    memory_limit_bytes,
                    memory_percent,
                    network_rx_bytes,
                    network_tx_bytes
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(id)
            .bind(timestamp)
            .bind(sample.cpu_core_percent)
            .bind(sample.memory_working_set_bytes as i64)
            .bind(sample.memory_limit_bytes as i64)
            .bind(sample.memory_percent)
            .bind(sample.network_rx_bytes as i64)
            .bind(sample.network_tx_bytes as i64)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    debug!(
        "Docker stats sampled: containers={}, timestamp={}",
        summary.sampled_container_count, timestamp
    );
    Ok(())
}

async fn collect_samples_and_summary(
    docker: &bollard::Docker,
) -> anyhow::Result<(
    HostResourceUsageSummary,
    Vec<(String, ContainerResourceUsageSample)>,
)> {
    let running_ids = list_running_container_ids(docker).await?;
    let attempts = stream::iter(running_ids)
        .map(|id| async {
            let summary = fetch_container_stats_snapshot(docker, &id).await;
            (id, summary)
        })
        .buffer_unordered(6)
        .collect::<Vec<_>>()
        .await;
    let mut samples = Vec::with_capacity(attempts.len());
    let mut failed_ids = Vec::new();
    for (id, sample) in attempts {
        match sample {
            Some(sample) => samples.push((id, sample)),
            None => failed_ids.push(id),
        }
    }

    let sampled_container_count = samples.len();
    let running_container_count = if failed_ids.is_empty() {
        sampled_container_count
    } else {
        let refreshed_running_ids = match list_running_container_ids(docker).await {
            Ok(ids) => Some(ids.into_iter().collect::<HashSet<_>>()),
            Err(error) => {
                warn!(
                    error = %error,
                    failed_container_count = failed_ids.len(),
                    "Docker stats failure state recheck failed; preserving partial sample status"
                );
                None
            }
        };
        resolve_running_container_count(
            sampled_container_count,
            &failed_ids,
            refreshed_running_ids.as_ref(),
        )
    };

    if running_container_count < sampled_container_count + failed_ids.len() {
        debug!(
            sampled_container_count,
            failed_container_count = failed_ids.len(),
            running_container_count,
            "Docker stats failures caused by container lifecycle changes were excluded"
        );
    }

    let mut cpu_core_percent = 0.0_f64;
    let mut memory_working_set_bytes = 0_u64;
    let mut memory_limit_bytes = 0_u64;

    for (_, sample) in &samples {
        cpu_core_percent += sample.cpu_core_percent;
        memory_working_set_bytes += sample.memory_working_set_bytes;
        memory_limit_bytes = memory_limit_bytes.max(sample.memory_limit_bytes);
    }

    let mut online_cpus = 1_u64;
    if let Ok(info) = docker.info().await {
        if let Some(mem_total) = info.mem_total
            && mem_total > 0
        {
            memory_limit_bytes = mem_total as u64;
        }
        if let Some(ncpu) = info.ncpu
            && ncpu > 0
        {
            online_cpus = ncpu as u64;
        }
    }

    let memory_percent = if memory_limit_bytes > 0 {
        ((memory_working_set_bytes as f64 / memory_limit_bytes as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let cpu_host_percent = normalize_cpu_host_percent(cpu_core_percent, online_cpus);
    let status = if sampled_container_count < running_container_count {
        ResourceSampleStatus::Partial
    } else {
        ResourceSampleStatus::Fresh
    };

    let summary = HostResourceUsageSummary {
        status,
        collected_at: Some(Utc::now().timestamp()),
        running_container_count,
        sampled_container_count,
        cpu_host_percent,
        cpu_core_percent,
        memory_working_set_bytes,
        memory_limit_bytes,
        memory_percent,
    };

    Ok((summary, samples))
}

/// 返回当前处于运行状态且具备有效 ID 的容器列表。
async fn list_running_container_ids(docker: &bollard::Docker) -> anyhow::Result<Vec<String>> {
    let options = query_parameters::ListContainersOptionsBuilder::new()
        .all(true)
        .build();
    let containers = docker.list_containers(Some(options)).await?;
    Ok(containers
        .into_iter()
        .filter(|container| container.state == Some(ContainerSummaryStateEnum::RUNNING))
        .filter_map(|container| container.id)
        .collect())
}

/// 根据失败容器的最新运行状态修正本次采样分母。
fn resolve_running_container_count(
    sampled_container_count: usize,
    failed_ids: &[String],
    refreshed_running_ids: Option<&HashSet<String>>,
) -> usize {
    let still_running_failure_count =
        refreshed_running_ids.map_or(failed_ids.len(), |running_ids| {
            failed_ids
                .iter()
                .filter(|id| running_ids.contains(id.as_str()))
                .count()
        });
    sampled_container_count + still_running_failure_count
}

async fn cleanup_old_stats(state: &AppState) -> anyhow::Result<()> {
    let retention_hours = config::stats_retention_hours();
    let cutoff = Utc::now().timestamp() - (retention_hours as i64 * 3600);

    let mut tx = state.metadata_db.begin().await?;
    sqlx::query("DELETE FROM docker_metrics_summary WHERE created_at < ?")
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM docker_metrics_container WHERE created_at < ?")
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn fetch_container_stats_snapshot(
    docker: &bollard::Docker,
    id: &str,
) -> Option<ContainerResourceUsageSample> {
    let stats_options = container_stats_options();
    let mut stream = docker.stats(id, Some(stats_options));
    let sample = timeout(CONTAINER_STATS_TIMEOUT, stream.next())
        .await
        .ok()??
        .ok()?;

    let cpu_core_percent = calculate_cpu_percent(&sample);

    let (mut memory_working_set_bytes, mut memory_limit_bytes) = (0_u64, 0_u64);
    if let Some(memory_stats) = &sample.memory_stats {
        if let Some(usage) = memory_stats.usage {
            let reclaimable_cache = memory_stats
                .stats
                .as_ref()
                .and_then(|stats| {
                    stats
                        .get("total_inactive_file")
                        .or_else(|| stats.get("inactive_file"))
                        .or_else(|| stats.get("cache"))
                })
                .copied()
                .unwrap_or(0);
            memory_working_set_bytes = usage.saturating_sub(reclaimable_cache);
        }
        if let Some(limit) = memory_stats.limit {
            memory_limit_bytes = limit;
        }
    }

    let mut network_rx_bytes = 0_u64;
    let mut network_tx_bytes = 0_u64;
    if let Some(networks) = sample.networks {
        for network in networks.values() {
            if let Some(rx) = network.rx_bytes {
                network_rx_bytes += rx;
            }
            if let Some(tx) = network.tx_bytes {
                network_tx_bytes += tx;
            }
        }
    }

    let memory_percent = if memory_limit_bytes > 0 {
        ((memory_working_set_bytes as f64 / memory_limit_bytes as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    Some(ContainerResourceUsageSample {
        cpu_core_percent,
        memory_working_set_bytes,
        memory_limit_bytes,
        memory_percent,
        network_rx_bytes,
        network_tx_bytes,
    })
}

/// 构造一次非流式双周期 Docker 统计请求。
fn container_stats_options() -> query_parameters::StatsOptions {
    query_parameters::StatsOptionsBuilder::new()
        .stream(false)
        .one_shot(false)
        .build()
}

/// 将 Docker 多核 CPU 百分比归一化为宿主机总容量占比。
fn normalize_cpu_host_percent(cpu_core_percent: f64, online_cpus: u64) -> f64 {
    (cpu_core_percent / online_cpus.max(1) as f64).clamp(0.0, 100.0)
}

fn calculate_cpu_percent(stats: &bollard::models::ContainerStatsResponse) -> f64 {
    let Some(cpu_stats) = &stats.cpu_stats else {
        return 0.0;
    };
    let Some(precpu_stats) = &stats.precpu_stats else {
        return 0.0;
    };

    let cpu_total = cpu_stats
        .cpu_usage
        .as_ref()
        .and_then(|usage| usage.total_usage)
        .unwrap_or(0);
    let precpu_total = precpu_stats
        .cpu_usage
        .as_ref()
        .and_then(|usage| usage.total_usage)
        .unwrap_or(0);
    let cpu_delta = cpu_total.saturating_sub(precpu_total);

    let system_total = cpu_stats.system_cpu_usage.unwrap_or(0);
    let presystem_total = precpu_stats.system_cpu_usage.unwrap_or(0);
    let system_delta = system_total.saturating_sub(presystem_total);

    if system_delta == 0 || cpu_delta == 0 {
        return 0.0;
    }

    let online_cpus = cpu_stats
        .online_cpus
        .filter(|value| *value > 0)
        .or_else(|| {
            cpu_stats
                .cpu_usage
                .as_ref()
                .and_then(|usage| usage.percpu_usage.as_ref())
                .map(|usage| usage.len() as u32)
                .filter(|value| *value > 0)
        })
        .unwrap_or(1) as f64;
    (cpu_delta as f64 / system_delta as f64) * online_cpus * 100.0
}

#[cfg(test)]
mod tests {
    use super::{
        calculate_cpu_percent, cleanup_old_stats, container_stats_options,
        normalize_cpu_host_percent, resolve_running_container_count, resolve_sample_status,
    };
    use crate::config;
    use crate::models::docker::ResourceSampleStatus;
    use crate::test_support::setup_test_state;
    use bollard::models::{ContainerCpuStats, ContainerCpuUsage, ContainerStatsResponse};
    use chrono::Utc;
    use std::collections::HashSet;

    fn make_stats(
        cpu_total: u64,
        precpu_total: u64,
        system_total: u64,
        presystem_total: u64,
    ) -> ContainerStatsResponse {
        ContainerStatsResponse {
            os_type: None,
            name: None,
            id: None,
            read: None,
            preread: None,
            pids_stats: None,
            blkio_stats: None,
            num_procs: None,
            storage_stats: None,
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(cpu_total),
                    percpu_usage: None,
                    usage_in_kernelmode: None,
                    usage_in_usermode: None,
                }),
                system_cpu_usage: Some(system_total),
                online_cpus: Some(2),
                throttling_data: None,
            }),
            precpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(precpu_total),
                    percpu_usage: None,
                    usage_in_kernelmode: None,
                    usage_in_usermode: None,
                }),
                system_cpu_usage: Some(presystem_total),
                online_cpus: Some(2),
                throttling_data: None,
            }),
            memory_stats: None,
            networks: None,
        }
    }

    #[test]
    fn calculate_cpu_percent_handles_zero_delta() {
        let stats = make_stats(100, 100, 200, 200);
        assert_eq!(calculate_cpu_percent(&stats), 0.0);
    }

    #[test]
    fn calculate_cpu_percent_computes_ratio() {
        let stats = make_stats(200, 100, 300, 100);
        let percent = calculate_cpu_percent(&stats);
        assert!((percent - 100.0).abs() < 0.0001);
    }

    #[test]
    fn calculate_cpu_percent_falls_back_to_per_cpu_usage_count() {
        let mut stats = make_stats(200, 100, 300, 100);
        let cpu_stats = stats.cpu_stats.as_mut().unwrap();
        cpu_stats.online_cpus = None;
        cpu_stats.cpu_usage.as_mut().unwrap().percpu_usage = Some(vec![50, 50, 50, 50]);

        let percent = calculate_cpu_percent(&stats);

        assert!((percent - 200.0).abs() < 0.0001);
    }

    #[test]
    fn container_stats_waits_for_two_non_streaming_samples() {
        let options = container_stats_options();
        assert!(!options.stream);
        assert!(!options.one_shot);
    }

    #[test]
    fn normalize_cpu_percent_uses_host_capacity() {
        assert_eq!(normalize_cpu_host_percent(400.0, 8), 50.0);
        assert_eq!(normalize_cpu_host_percent(900.0, 8), 100.0);
    }

    #[test]
    fn sample_status_distinguishes_partial_stale_and_idle() {
        assert_eq!(
            resolve_sample_status(100, 90, 2, 1),
            ResourceSampleStatus::Partial
        );
        assert_eq!(
            resolve_sample_status(1000, 0, 2, 2),
            ResourceSampleStatus::Stale
        );
        assert_eq!(
            resolve_sample_status(100, 90, 0, 0),
            ResourceSampleStatus::Fresh
        );
    }

    #[test]
    fn running_count_excludes_failed_containers_that_stopped() {
        let failed_ids = vec!["stopped".to_string()];
        let running_ids = HashSet::new();

        assert_eq!(
            resolve_running_container_count(3, &failed_ids, Some(&running_ids)),
            3
        );
        assert_eq!(
            resolve_sample_status(100, 90, 3, 3),
            ResourceSampleStatus::Fresh
        );
    }

    #[test]
    fn running_count_keeps_failed_containers_that_are_still_running() {
        let failed_ids = vec!["running".to_string()];
        let running_ids = HashSet::from(["running".to_string()]);

        assert_eq!(
            resolve_running_container_count(3, &failed_ids, Some(&running_ids)),
            4
        );
        assert_eq!(
            resolve_sample_status(100, 90, 4, 3),
            ResourceSampleStatus::Partial
        );
    }

    #[test]
    fn running_count_only_keeps_failed_containers_still_running() {
        let failed_ids = vec![
            "running".to_string(),
            "paused".to_string(),
            "removed".to_string(),
        ];
        let running_ids = HashSet::from(["running".to_string()]);

        assert_eq!(
            resolve_running_container_count(2, &failed_ids, Some(&running_ids)),
            3
        );
    }

    #[test]
    fn running_count_preserves_failures_when_recheck_is_unavailable() {
        let failed_ids = vec!["first".to_string(), "second".to_string()];

        assert_eq!(resolve_running_container_count(2, &failed_ids, None), 4);
    }

    #[test]
    fn running_count_matches_samples_when_nothing_failed() {
        assert_eq!(resolve_running_container_count(4, &[], None), 4);
    }

    #[tokio::test]
    async fn cleanup_old_stats_removes_expired_rows() {
        let state = setup_test_state().await;
        let now = Utc::now().timestamp();
        let retention_hours = config::stats_retention_hours() as i64;
        let old = now - (retention_hours + 1) * 3600;
        let recent = now - 3600;

        sqlx::query(
            r#"
            INSERT INTO docker_metrics_summary (
                created_at, cpu_core_percent, cpu_host_percent, memory_working_set_bytes,
                memory_limit_bytes, memory_percent, running_container_count, sampled_container_count
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(old)
        .bind(1.0)
        .bind(1.0)
        .bind(10_i64)
        .bind(20_i64)
        .bind(50.0)
        .bind(1_i64)
        .bind(1_i64)
        .execute(&state.metadata_db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO docker_metrics_summary (
                created_at, cpu_core_percent, cpu_host_percent, memory_working_set_bytes,
                memory_limit_bytes, memory_percent, running_container_count, sampled_container_count
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(recent)
        .bind(2.0)
        .bind(2.0)
        .bind(11_i64)
        .bind(21_i64)
        .bind(52.0)
        .bind(2_i64)
        .bind(2_i64)
        .execute(&state.metadata_db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO docker_metrics_container (
                container_id, created_at, cpu_core_percent, memory_working_set_bytes, memory_limit_bytes,
                memory_percent, network_rx_bytes, network_tx_bytes
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("abc")
        .bind(old)
        .bind(1.0)
        .bind(10_i64)
        .bind(20_i64)
        .bind(50.0)
        .bind(5_i64)
        .bind(6_i64)
        .execute(&state.metadata_db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO docker_metrics_container (
                container_id, created_at, cpu_core_percent, memory_working_set_bytes, memory_limit_bytes,
                memory_percent, network_rx_bytes, network_tx_bytes
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("abc")
        .bind(recent)
        .bind(2.0)
        .bind(11_i64)
        .bind(21_i64)
        .bind(52.0)
        .bind(7_i64)
        .bind(8_i64)
        .execute(&state.metadata_db)
        .await
        .unwrap();

        cleanup_old_stats(&state).await.unwrap();

        let summary_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM docker_metrics_summary")
            .fetch_one(&state.metadata_db)
            .await
            .unwrap();
        let container_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM docker_metrics_container")
                .fetch_one(&state.metadata_db)
                .await
                .unwrap();

        assert_eq!(summary_count, 1);
        assert_eq!(container_count, 1);
    }
}
