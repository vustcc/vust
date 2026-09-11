//! VUST 管理 API：运行时监听地址配置与生效控制。

use crate::api::auth::AuthenticatedAdmin;
use crate::models::logging::LogModule;
use crate::models::node_provisioning::{NodeProvisioningRecord, list_node_provisioning};
use crate::models::node_runtime_client::NodeRuntimeClient;
use crate::models::node_sessions::get_active_session_by_node_id;
use crate::runtime_config::{self, ListenConfig};
use crate::services::logging;
use crate::services::logging::OperationEventBuilder;
use crate::services::node_deploy::detect_lan_ip_sync;
use crate::services::node_provisioning;
use crate::services::runtime_metrics;
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};
use axum::extract::connect_info::ConnectInfo;
use axum::http::HeaderMap;
use axum::{Json, Router, extract::State, response::IntoResponse, routing::get, routing::put};
use serde::Serialize;
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::process::Command;
use tokio::time::{Duration, sleep};
use vust_contracts::vust::{VustNetworkConfig, VustNetworkUpdateResult};

pub fn vust_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/network", get(get_network))
        .route("/network", put(update_network))
        .route("/network-interfaces", get(get_network_interfaces))
        .route("/metrics", get(get_metrics))
}

/// 默认回连地址同步到功能节点后的汇总信息。
#[derive(Debug, Default)]
struct NodeVustUrlSyncSummary {
    attempted: usize,
    succeeded: usize,
    failed: usize,
    skipped_custom: usize,
    skipped_offline: usize,
}

/// 发送给功能节点的主控回连地址更新请求。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VustUrlPayload {
    vust_url: String,
}

/// 默认回连地址同步事件的共享上下文。
struct NodeVustUrlSyncEventContext<'a> {
    username: &'a str,
    client_ip: IpAddr,
    trace_id: &'a str,
    old_default_url: &'a str,
    new_default_url: &'a str,
}

async fn get_network() -> ApiResult<impl IntoResponse> {
    let current = runtime_config::load_or_default();
    let active_public_host = match current.public_host.clone() {
        Some(ref ph) if !ph.trim().is_empty() => Some(ph.clone()),
        _ => {
            if let Ok(env_host) = std::env::var("VUST_PUBLIC_HOST").map(|v| v.trim().to_string())
                && !env_host.is_empty()
            {
                Some(env_host)
            } else {
                detect_lan_ip().await
            }
        }
    };
    Ok(ApiResponse::success_with_raw(
        "VUST network config loaded",
        VustNetworkConfig {
            host: current.host,
            port: current.port,
            public_host: active_public_host,
        },
    )
    .into_response())
}

async fn get_metrics() -> ApiResult<impl IntoResponse> {
    Ok(
        ApiResponse::success_with_raw("Runtime metrics loaded", runtime_metrics::snapshot())
            .into_response(),
    )
}

async fn update_network(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<SocketAddr>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Json(payload): Json<VustNetworkConfig>,
) -> ApiResult<impl IntoResponse> {
    let client_ip = extract_client_ip(&headers, conn);
    let trace_id = logging::resolve_trace_id(&headers);
    let before = runtime_config::get_active_config();
    let base_platform_log =
        OperationEventBuilder::new(&admin.username, "vust_network_update", client_ip)
            .module(LogModule::System)
            .target_type("vust")
            .target_id("network")
            .trace_id(&trace_id)
            .source("vust_api")
            .request("PUT", "/api/v1/vust/network");

    if let Err(err) = ensure_admin(&admin) {
        base_platform_log
            .metadata(build_platform_log_metadata(
                &before,
                &payload,
                Some(&format!("{err:?}")),
            ))
            .finish(&state.metadata_db);
        return Err(err);
    }
    if let Err(err) = validate_host_and_port(&payload) {
        base_platform_log
            .metadata(build_platform_log_metadata(
                &before,
                &payload,
                Some(&format!("{err:?}")),
            ))
            .finish(&state.metadata_db);
        return Err(err);
    }

    // A) 监听地址防篡改强校验（必须为 :: 或 0.0.0.0 或活跃网卡 IP）
    let active_ips = get_active_system_ips().await;
    if payload.host != "::" && payload.host != "0.0.0.0" && !active_ips.contains(&payload.host) {
        let err = ApiError::BadRequest(
            "The listen address is invalid or its network interface is unavailable".to_string(),
        );
        base_platform_log
            .metadata(build_platform_log_metadata(
                &before,
                &payload,
                Some(&format!("{err:?}")),
            ))
            .finish(&state.metadata_db);
        return Err(err);
    }

    // B) 默认访问地址格式与冲突唯一性校验
    if let Some(ref ph) = payload.public_host {
        let trimmed = ph.trim();
        if !trimmed.is_empty() {
            if trimmed.contains("://") || trimmed.contains('/') || trimmed.contains(':') {
                let err = ApiError::BadRequest("The public host must be a valid IP address or domain without a scheme, path, or port".to_string());
                base_platform_log
                    .metadata(build_platform_log_metadata(
                        &before,
                        &payload,
                        Some(&format!("{err:?}")),
                    ))
                    .finish(&state.metadata_db);
                return Err(err);
            }

            // 校验与被纳管节点 SSH 通信地址冲突
            let duplicate_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM node_provisioning WHERE ssh_addr = ?1)",
            )
            .bind(trimmed)
            .fetch_one(&state.metadata_db)
            .await
            .unwrap_or(false);

            if duplicate_exists {
                let err = ApiError::BadRequest(
                    "The public host must not duplicate an existing node SSH address".to_string(),
                );
                base_platform_log
                    .metadata(build_platform_log_metadata(
                        &before,
                        &payload,
                        Some(&format!("{err:?}")),
                    ))
                    .finish(&state.metadata_db);
                return Err(err);
            }
        }
    }

    let next = ListenConfig {
        host: payload.host.clone(),
        port: payload.port,
        public_host: payload
            .public_host
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    };
    let listen_changed = before.host != next.host || before.port != next.port;
    let port_changed = before.port != next.port;
    let old_default_url = default_vust_url(&before);
    let new_default_url = default_vust_url(&next);
    let default_url_changed = !same_url(&old_default_url, &new_default_url);

    if port_changed && let Err(err) = ensure_listen_available(&next.host, next.port).await {
        base_platform_log
            .metadata(build_platform_log_metadata(
                &before,
                &payload,
                Some(&format!("{err:?}")),
            ))
            .finish(&state.metadata_db);
        return Err(err);
    }

    if let Err(err) = runtime_config::save(&next) {
        let api_err = ApiError::Internal(err.to_string());
        base_platform_log
            .metadata(build_platform_log_metadata(
                &before,
                &payload,
                Some(&format!("{api_err:?}")),
            ))
            .finish(&state.metadata_db);
        return Err(api_err);
    }
    runtime_config::set_active_config(next.clone());

    let sync_summary = if default_url_changed {
        sync_default_vust_url_to_nodes(
            &state,
            &admin.username,
            client_ip,
            &trace_id,
            &old_default_url,
            &new_default_url,
        )
        .await
    } else {
        NodeVustUrlSyncSummary::default()
    };

    let next_url = default_vust_url(&next);
    tracing::info!(
        user = %admin.username,
        from = %format!("{}:{}", before.host, before.port),
        to = %format!("{}:{}", next.host, next.port),
        old_default_url,
        new_default_url,
        node_sync_attempted = sync_summary.attempted,
        node_sync_succeeded = sync_summary.succeeded,
        node_sync_failed = sync_summary.failed,
        trace_id = %trace_id,
        "VUST network config updated"
    );
    if listen_changed {
        tokio::spawn(async {
            sleep(Duration::from_secs(1)).await;
            tracing::info!("Restarting vust to apply listen config");
            if let Err(err) = restart_vust().await {
                tracing::error!("Failed to restart vust: {err}");
            }
        });
    }
    base_platform_log
        .metadata(build_platform_log_metadata_with_sync(
            &before,
            &next,
            None,
            &old_default_url,
            &new_default_url,
            &sync_summary,
        ))
        .set_success()
        .finish(&state.metadata_db);

    Ok(ApiResponse::success_with_raw(
        if listen_changed {
            "VUST network config updated; restart scheduled"
        } else {
            "VUST network config updated"
        },
        VustNetworkUpdateResult {
            host: next.host,
            port: next.port,
            next_url,
        },
    )
    .into_response())
}

fn ensure_admin(admin: &AuthenticatedAdmin) -> ApiResult<()> {
    if admin.id != 1 {
        return Err(ApiError::ForbiddenResource);
    }
    Ok(())
}

fn validate_host_and_port(payload: &VustNetworkConfig) -> ApiResult<()> {
    if payload.host.trim().is_empty() {
        return Err(ApiError::BadRequest("host must not be empty".to_string()));
    }
    if payload.port == 0 {
        return Err(ApiError::BadRequest(
            "port must be in the range 1-65535".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_listen_available(host: &str, port: u16) -> ApiResult<()> {
    let target = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&target)
        .await
        .map_err(|err| ApiError::BadRequest(format!("port unavailable: {target}, {err}")))?;
    drop(listener);
    Ok(())
}

async fn sync_default_vust_url_to_nodes(
    state: &Arc<AppState>,
    username: &str,
    client_ip: IpAddr,
    trace_id: &str,
    old_default_url: &str,
    new_default_url: &str,
) -> NodeVustUrlSyncSummary {
    let mut summary = NodeVustUrlSyncSummary::default();
    let event_context = NodeVustUrlSyncEventContext {
        username,
        client_ip,
        trace_id,
        old_default_url,
        new_default_url,
    };
    let records = match list_node_provisioning(&state.metadata_db).await {
        Ok(records) => records,
        Err(err) => {
            tracing::warn!("failed to list node provisioning for vustUrl sync: {err}");
            return summary;
        }
    };

    for record in records {
        if record.node_id == "local" {
            continue;
        }
        if !should_sync_node_vust_url(record.vust_url.as_deref(), old_default_url) {
            summary.skipped_custom += 1;
            record_node_vust_url_sync_event(state, &event_context, &record, "skipped_custom", None)
                .await;
            continue;
        }
        let Ok(Some(_)) = get_active_session_by_node_id(&state.metadata_db, &record.node_id).await
        else {
            summary.skipped_offline += 1;
            record_node_vust_url_sync_event(
                state,
                &event_context,
                &record,
                "skipped_offline",
                None,
            )
            .await;
            continue;
        };
        summary.attempted += 1;
        match sync_single_node_vust_url(state, &record, new_default_url).await {
            Ok(()) => {
                summary.succeeded += 1;
                record_node_vust_url_sync_event(state, &event_context, &record, "synced", None)
                    .await;
            }
            Err(err) => {
                summary.failed += 1;
                record_node_vust_url_sync_event(
                    state,
                    &event_context,
                    &record,
                    "failed",
                    Some(&format!("{err:?}")),
                )
                .await;
            }
        }
    }
    summary
}

async fn sync_single_node_vust_url(
    state: &Arc<AppState>,
    record: &NodeProvisioningRecord,
    new_default_url: &str,
) -> ApiResult<()> {
    let client =
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&record.node_id)).await?;
    let _: serde_json::Value = client
        .put_json(
            "/api/v1/agent/system/vust-url",
            &VustUrlPayload {
                vust_url: new_default_url.to_string(),
            },
        )
        .await
        .map_err(|err| {
            ApiError::Internal(format!(
                "Failed to sync vustUrl to node {}: {}",
                record.node_id, err
            ))
        })?;
    node_provisioning::update_node_vust_url(&state.metadata_db, &record.node_id, new_default_url)
        .await?;
    Ok(())
}

async fn record_node_vust_url_sync_event(
    state: &Arc<AppState>,
    context: &NodeVustUrlSyncEventContext<'_>,
    record: &NodeProvisioningRecord,
    sync_result: &str,
    error: Option<&str>,
) {
    let status = if error.is_some() {
        crate::models::logging::LogStatus::Failed
    } else {
        crate::models::logging::LogStatus::Success
    };
    OperationEventBuilder::new(context.username, "node_vust_url_sync", context.client_ip)
        .module(LogModule::System)
        .target_type("node")
        .target_id(&record.node_id)
        .trace_id(context.trace_id)
        .source("vust_api")
        .request("PUT", "/api/v1/vust/network")
        .status(status)
        .metadata(json!({
            "nodeId": record.node_id,
            "previousNodeVustUrl": record.vust_url,
            "oldDefaultVustUrl": context.old_default_url,
            "newDefaultVustUrl": context.new_default_url,
            "syncResult": sync_result,
            "error": error
        }))
        .finish(&state.metadata_db);
}

fn should_sync_node_vust_url(current: Option<&str>, old_default_url: &str) -> bool {
    let Some(current) = current.map(normalize_url_for_compare) else {
        return true;
    };
    current.is_empty() || current == normalize_url_for_compare(old_default_url)
}

fn default_vust_url(config: &ListenConfig) -> String {
    if let Ok(public_url) = std::env::var("VUST_PUBLIC_URL") {
        let trimmed = public_url.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }

    let host = config
        .public_host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("VUST_PUBLIC_HOST")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| {
            detect_lan_ip_sync()
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "127.0.0.1".to_string())
        });
    let scheme = std::env::var("VUST_URL_SCHEME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https".to_string());
    format!("{scheme}://{}:{}", host, config.port)
}

fn normalize_url_for_compare(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn same_url(left: &str, right: &str) -> bool {
    normalize_url_for_compare(left) == normalize_url_for_compare(right)
}

async fn restart_vust() -> std::io::Result<()> {
    let status = Command::new("systemctl")
        .args(["restart", "vust"])
        .status()
        .await?;
    if status.success() {
        return Ok(());
    }

    let pid = std::process::id().to_string();
    let _ = Command::new("kill").args(["-TERM", &pid]).status().await?;
    Ok(())
}

fn extract_client_ip(headers: &HeaderMap, conn: SocketAddr) -> IpAddr {
    if let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        && let Some(first) = forwarded.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return ip;
    }
    conn.ip()
}

fn build_platform_log_metadata(
    before: &ListenConfig,
    payload: &VustNetworkConfig,
    error: Option<&str>,
) -> serde_json::Value {
    json!({
        "before": {
            "host": before.host.as_str(),
            "port": before.port,
            "publicHost": before.public_host.clone()
        },
        "after": {
            "host": payload.host.as_str(),
            "port": payload.port,
            "publicHost": payload.public_host.clone()
        },
        "error": error
    })
}

fn build_platform_log_metadata_with_sync(
    before: &ListenConfig,
    after: &ListenConfig,
    error: Option<&str>,
    old_default_url: &str,
    new_default_url: &str,
    sync_summary: &NodeVustUrlSyncSummary,
) -> serde_json::Value {
    json!({
        "before": {
            "host": before.host.as_str(),
            "port": before.port,
            "publicHost": before.public_host.clone(),
            "defaultVustUrl": old_default_url,
        },
        "after": {
            "host": after.host.as_str(),
            "port": after.port,
            "publicHost": after.public_host.clone(),
            "defaultVustUrl": new_default_url,
        },
        "nodeVustUrlSync": {
            "attempted": sync_summary.attempted,
            "succeeded": sync_summary.succeeded,
            "failed": sync_summary.failed,
            "skippedCustom": sync_summary.skipped_custom,
            "skippedOffline": sync_summary.skipped_offline,
        },
        "error": error
    })
}

async fn get_network_interfaces(
    State(_state): State<Arc<AppState>>,
    _admin: AuthenticatedAdmin,
) -> ApiResult<impl IntoResponse> {
    let interfaces = get_active_system_interfaces().await;
    let mut response = vec![
        json!({ "name": "any_dual", "ip": "::", "label": "IPv6 & IPv4 wildcard (::)" }),
        json!({ "name": "any_v4", "ip": "0.0.0.0", "label": "IPv4 wildcard (0.0.0.0)" }),
    ];
    for (ifname, ip) in interfaces {
        if ip != "::1" && ip != "127.0.0.1" {
            response.push(json!({
                "name": ifname.clone(),
                "ip": ip.clone(),
                "label": format!("{ifname} ({ip})")
            }));
        }
    }
    Ok(ApiResponse::success_with_raw("System network interfaces loaded", response).into_response())
}

async fn get_active_system_interfaces() -> Vec<(String, String)> {
    let mut result = Vec::new();
    if let Ok(output) = Command::new("ip").args(["-j", "addr"]).output().await
        && output.status.success()
        && let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        && let Some(arr) = val.as_array()
    {
        for interface in arr {
            let ifname = interface
                .get("ifname")
                .and_then(|n| n.as_str())
                .unwrap_or("unknown")
                .to_string();
            if let Some(addr_info) = interface.get("addr_info").and_then(|a| a.as_array()) {
                for addr in addr_info {
                    if let Some(family) = addr.get("family").and_then(|f| f.as_str())
                        && family == "inet"
                        && let Some(local) = addr.get("local").and_then(|l| l.as_str())
                    {
                        let trimmed = local.trim().to_string();
                        if !trimmed.is_empty() {
                            result.push((ifname.clone(), trimmed));
                        }
                    }
                }
            }
        }
    }
    if result.is_empty() {
        result.push(("lo".to_string(), "127.0.0.1".to_string()));
        result.push(("lo".to_string(), "::1".to_string()));
    }
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result.dedup_by(|a, b| a.1 == b.1);
    result
}

async fn get_active_system_ips() -> Vec<String> {
    get_active_system_interfaces()
        .await
        .into_iter()
        .map(|(_, ip)| ip)
        .collect()
}

pub async fn detect_lan_ip() -> Option<String> {
    if let Ok(output) = Command::new("ip").args(["-j", "addr"]).output().await
        && output.status.success()
        && let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        && let Some(arr) = val.as_array()
    {
        let mut candidates = Vec::new();
        for interface in arr {
            let ifname = interface
                .get("ifname")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if ifname.starts_with("lo")
                || ifname.starts_with("docker")
                || ifname.starts_with("br-")
                || ifname.starts_with("veth")
            {
                continue;
            }
            if let Some(addr_info) = interface.get("addr_info").and_then(|a| a.as_array()) {
                for addr in addr_info {
                    if let Some(family) = addr.get("family").and_then(|f| f.as_str())
                        && family == "inet"
                        && let Some(local) = addr.get("local").and_then(|l| l.as_str())
                    {
                        let ip = local.trim().to_string();
                        if is_private_v4(&ip) {
                            if ifname.starts_with("en")
                                || ifname.starts_with("eth")
                                || ifname.starts_with("wl")
                            {
                                return Some(ip);
                            }
                            candidates.push(ip);
                        }
                    }
                }
            }
        }
        if !candidates.is_empty() {
            return Some(candidates[0].clone());
        }
    }
    None
}

fn is_private_v4(ip: &str) -> bool {
    if let Ok(ip_addr) = ip.parse::<std::net::Ipv4Addr>() {
        let octets = ip_addr.octets();
        match octets[0] {
            10 => true,
            172 => octets[1] >= 16 && octets[1] <= 31,
            192 => octets[0] == 192 && octets[1] == 168,
            _ => false,
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::should_sync_node_vust_url;

    #[test]
    fn should_sync_node_vust_url_accepts_empty_or_old_default() {
        assert!(should_sync_node_vust_url(
            None,
            "https://10.121.111.124:7310"
        ));
        assert!(should_sync_node_vust_url(
            Some(""),
            "https://10.121.111.124:7310"
        ));
        assert!(should_sync_node_vust_url(
            Some("https://10.121.111.124:7310/"),
            "https://10.121.111.124:7310"
        ));
    }

    #[test]
    fn should_sync_node_vust_url_skips_custom_callback_url() {
        assert!(!should_sync_node_vust_url(
            Some("https://openwrt.example.net:19090"),
            "https://10.121.111.124:7310"
        ));
    }
}
