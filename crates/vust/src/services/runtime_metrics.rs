//! 运行时指标服务：聚合注册、心跳、代理与部署链路的内存指标。

use crate::types::ApiError;
use serde::Serialize;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use vust_contracts::api::ErrorCode;

#[derive(Debug, Default)]
struct RuntimeMetrics {
    enroll_success_total: AtomicU64,
    enroll_failure_total: AtomicU64,
    register_success_total: AtomicU64,
    register_failure_total: AtomicU64,
    heartbeat_success_total: AtomicU64,
    heartbeat_failure_total: AtomicU64,
    heartbeat_ignored_total: AtomicU64,
    heartbeat_latency_ms_total: AtomicU64,
    heartbeat_latency_samples_total: AtomicU64,
    lease_expired_total: AtomicU64,
    proxy_http_success_total: AtomicU64,
    proxy_http_failure_total: AtomicU64,
    proxy_http_refused_total: AtomicU64,
    proxy_http_timeout_total: AtomicU64,
    proxy_http_not_found_total: AtomicU64,
    proxy_http_other_total: AtomicU64,
    proxy_ws_success_total: AtomicU64,
    proxy_ws_failure_total: AtomicU64,
    deploy_success_total: AtomicU64,
    deploy_failure_total: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetricsSnapshot {
    pub enroll_success_total: u64,
    pub enroll_failure_total: u64,
    pub register_success_total: u64,
    pub register_failure_total: u64,
    pub heartbeat_success_total: u64,
    pub heartbeat_failure_total: u64,
    pub heartbeat_ignored_total: u64,
    pub heartbeat_latency_avg_ms: f64,
    pub heartbeat_latency_total_ms: u64,
    pub lease_expired_total: u64,
    pub proxy_http_success_total: u64,
    pub proxy_http_failure_total: u64,
    pub proxy_http_refused_total: u64,
    pub proxy_http_timeout_total: u64,
    pub proxy_http_not_found_total: u64,
    pub proxy_http_other_total: u64,
    pub proxy_ws_success_total: u64,
    pub proxy_ws_failure_total: u64,
    pub deploy_success_total: u64,
    pub deploy_failure_total: u64,
}

fn metrics() -> &'static RuntimeMetrics {
    static METRICS: OnceLock<RuntimeMetrics> = OnceLock::new();
    METRICS.get_or_init(RuntimeMetrics::default)
}

fn incr(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

pub fn record_enroll(success: bool) {
    if success {
        incr(&metrics().enroll_success_total);
    } else {
        incr(&metrics().enroll_failure_total);
    }
}

pub fn record_register(success: bool) {
    if success {
        incr(&metrics().register_success_total);
    } else {
        incr(&metrics().register_failure_total);
    }
}

pub fn record_heartbeat(success: bool, sequence_ignored: bool, latency: Duration) {
    if success {
        incr(&metrics().heartbeat_success_total);
    } else {
        incr(&metrics().heartbeat_failure_total);
    }
    if sequence_ignored {
        incr(&metrics().heartbeat_ignored_total);
    }
    metrics()
        .heartbeat_latency_ms_total
        .fetch_add(latency.as_millis() as u64, Ordering::Relaxed);
    metrics()
        .heartbeat_latency_samples_total
        .fetch_add(1, Ordering::Relaxed);
}

pub fn record_lease_expired(count: usize) {
    if count == 0 {
        return;
    }
    metrics()
        .lease_expired_total
        .fetch_add(count as u64, Ordering::Relaxed);
}

pub fn record_proxy_http_success() {
    incr(&metrics().proxy_http_success_total);
}

pub fn record_proxy_http_failure(err: &ApiError) {
    incr(&metrics().proxy_http_failure_total);
    match err.code {
        ErrorCode::AgentRefused => incr(&metrics().proxy_http_refused_total),
        ErrorCode::AgentTimeout => incr(&metrics().proxy_http_timeout_total),
        ErrorCode::AgentNotFound => incr(&metrics().proxy_http_not_found_total),
        _ => incr(&metrics().proxy_http_other_total),
    }
}

pub fn record_proxy_ws(success: bool) {
    if success {
        incr(&metrics().proxy_ws_success_total);
    } else {
        incr(&metrics().proxy_ws_failure_total);
    }
}

pub fn record_deploy_result(success: bool) {
    if success {
        incr(&metrics().deploy_success_total);
    } else {
        incr(&metrics().deploy_failure_total);
    }
}

pub fn snapshot() -> RuntimeMetricsSnapshot {
    let latency_total = metrics().heartbeat_latency_ms_total.load(Ordering::Relaxed);
    let samples = metrics()
        .heartbeat_latency_samples_total
        .load(Ordering::Relaxed);
    RuntimeMetricsSnapshot {
        enroll_success_total: metrics().enroll_success_total.load(Ordering::Relaxed),
        enroll_failure_total: metrics().enroll_failure_total.load(Ordering::Relaxed),
        register_success_total: metrics().register_success_total.load(Ordering::Relaxed),
        register_failure_total: metrics().register_failure_total.load(Ordering::Relaxed),
        heartbeat_success_total: metrics().heartbeat_success_total.load(Ordering::Relaxed),
        heartbeat_failure_total: metrics().heartbeat_failure_total.load(Ordering::Relaxed),
        heartbeat_ignored_total: metrics().heartbeat_ignored_total.load(Ordering::Relaxed),
        heartbeat_latency_avg_ms: if samples == 0 {
            0.0
        } else {
            latency_total as f64 / samples as f64
        },
        heartbeat_latency_total_ms: latency_total,
        lease_expired_total: metrics().lease_expired_total.load(Ordering::Relaxed),
        proxy_http_success_total: metrics().proxy_http_success_total.load(Ordering::Relaxed),
        proxy_http_failure_total: metrics().proxy_http_failure_total.load(Ordering::Relaxed),
        proxy_http_refused_total: metrics().proxy_http_refused_total.load(Ordering::Relaxed),
        proxy_http_timeout_total: metrics().proxy_http_timeout_total.load(Ordering::Relaxed),
        proxy_http_not_found_total: metrics().proxy_http_not_found_total.load(Ordering::Relaxed),
        proxy_http_other_total: metrics().proxy_http_other_total.load(Ordering::Relaxed),
        proxy_ws_success_total: metrics().proxy_ws_success_total.load(Ordering::Relaxed),
        proxy_ws_failure_total: metrics().proxy_ws_failure_total.load(Ordering::Relaxed),
        deploy_success_total: metrics().deploy_success_total.load(Ordering::Relaxed),
        deploy_failure_total: metrics().deploy_failure_total.load(Ordering::Relaxed),
    }
}
