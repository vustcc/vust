//! Docker 网络 API：提供规范化列表、详情、校验与受保护的变更操作。

use crate::api::docker::context::DockerOperationContext;
use crate::models::docker::{
    self, DockerNetworkCapabilities, DockerNetworkContainer, DockerNetworkCreateResult,
    DockerNetworkDetail, DockerNetworkIpamConfig, DockerNetworkManagement,
    DockerNetworkManagementKind, DockerNetworkPeer, DockerNetworkSummary,
};
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};

use axum::extract::{Json, Path, State};
use axum::response::{IntoResponse, Response};
use bollard::models::{
    Ipam, IpamConfig, Network, NetworkConnectRequest, NetworkCreateRequest,
    NetworkDisconnectRequest, NetworkInspect,
};
use bollard::query_parameters;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;
use vust_contracts::api::ErrorCode;

/// 返回 Docker 网络摘要；该接口只调用一次 Docker list API。
pub async fn list_networks(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    info!("Requesting network list");
    let docker = state.docker_client().await?;
    let mut networks = docker
        .list_networks(Some(query_parameters::ListNetworksOptions::default()))
        .await?
        .iter()
        .map(summary_from_network)
        .collect::<Vec<_>>();
    networks.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(ApiResponse::success_with_raw("Docker networks loaded", Some(networks)).into_response())
}

/// 创建一个 Bridge 网络，并校验 IPAM 与保留标签。
pub async fn create_network(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Json(payload): Json<docker::DockerNetworkCreateRequest>,
) -> ApiResult<Response> {
    let network_name = payload.name.trim().to_string();
    info!(network = %network_name, "Requesting network create");
    let result: ApiResult<Response> = async {
        let request = build_create_request(payload)?;
        let docker = state.docker_client().await?;
        let created = docker.create_network(request).await?;
        let response = DockerNetworkCreateResult {
            id: created.id,
            warning: non_empty(created.warning),
        };
        Ok(ApiResponse::success_with_raw("Network created", Some(response)).into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_network_create",
            Some(("network", &network_name)),
            json!({ "name": network_name }),
            false,
            result,
        )
        .await
}

/// 获取指定网络的规范化详情。
pub async fn inspect_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    info!(network = %id, "Requesting network inspect");
    let docker = state.docker_client().await?;
    let network = docker
        .inspect_network(
            &id,
            Some(query_parameters::InspectNetworkOptions::default()),
        )
        .await?;
    Ok(
        ApiResponse::success_with_raw("Network detail loaded", Some(detail_from_inspect(&network)))
            .into_response(),
    )
}

/// 删除非系统网络；系统网络和仍连接容器的网络会在 Agent 侧拒绝。
pub async fn remove_network(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    info!(network = %id, "Requesting network remove");
    let mut network_name = id.clone();
    let result: ApiResult<Response> = async {
        let docker = state.docker_client().await?;
        let network = inspect(&docker, &id).await?;
        network_name = display_name(&network.name, &id);
        ensure_not_system_network(&network)?;
        ensure_unused(&network)?;
        docker.remove_network(&id).await?;
        Ok(ApiResponse::ok("Network removed").into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_network_remove",
            Some(("network", &id)),
            json!({ "name": network_name }),
            true,
            result,
        )
        .await
}

/// 将运行中的容器连接到非系统网络。
pub async fn connect_network(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
    Json(payload): Json<docker::NetworkConnectRequest>,
) -> ApiResult<Response> {
    let container_id = payload.container.trim().to_string();
    info!(network = %id, container = %container_id, "Requesting network connect");
    let mut network_name = id.clone();
    let mut container_name = container_id.clone();
    let result: ApiResult<Response> = async {
        if container_id.is_empty() {
            return Err(ApiError::validation("container is required"));
        }
        let docker = state.docker_client().await?;
        let network = inspect(&docker, &id).await?;
        network_name = display_name(&network.name, &id);
        ensure_not_system_network(&network)?;

        let container = docker.inspect_container(&container_id, None).await?;
        container_name = display_name(&container.name, &container_id);
        if !container
            .state
            .and_then(|value| value.running)
            .unwrap_or(false)
        {
            return Err(ApiError::validation(
                "only running containers can be connected to a network",
            ));
        }
        docker
            .connect_network(
                &id,
                NetworkConnectRequest {
                    container: container_id.clone(),
                    endpoint_config: None,
                },
            )
            .await?;
        Ok(ApiResponse::ok("Network connected").into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_network_connect",
            Some(("network", &id)),
            json!({ "name": network_name, "container": container_name }),
            false,
            result,
        )
        .await
}

/// 将容器从非系统网络断开；强制断开会记录为高影响操作。
pub async fn disconnect_network(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
    Json(payload): Json<docker::NetworkDisconnectRequest>,
) -> ApiResult<Response> {
    let container_id = payload.container.trim().to_string();
    let force = payload.force;
    info!(network = %id, container = %container_id, force, "Requesting network disconnect");
    let mut network_name = id.clone();
    let mut container_name = container_id.clone();
    let result: ApiResult<Response> = async {
        if container_id.is_empty() {
            return Err(ApiError::validation("container is required"));
        }
        let docker = state.docker_client().await?;
        let network = inspect(&docker, &id).await?;
        network_name = display_name(&network.name, &id);
        ensure_not_system_network(&network)?;
        if let Some(endpoint) = network
            .containers
            .as_ref()
            .and_then(|containers| containers.get(&container_id))
        {
            container_name = display_name(&endpoint.name, &container_id);
        }
        docker
            .disconnect_network(&id, disconnect_request(&container_id, force))
            .await?;
        Ok(ApiResponse::ok("Network disconnected").into_response())
    }
    .await;
    let (event_code, high_impact) = disconnect_event(force);
    context
        .finish(
            &state.metadata_db,
            event_code,
            Some(("network", &id)),
            json!({ "name": network_name, "container": container_name }),
            high_impact,
            result,
        )
        .await
}

/// 构造断开请求，普通操作显式保持 `force=false`。
fn disconnect_request(container: &str, force: bool) -> NetworkDisconnectRequest {
    NetworkDisconnectRequest {
        container: container.to_string(),
        force: Some(force),
    }
}

/// 返回断开操作的审计事件及高影响标记。
fn disconnect_event(force: bool) -> (&'static str, bool) {
    if force {
        ("docker_network_force_disconnect", true)
    } else {
        ("docker_network_disconnect", false)
    }
}

async fn inspect(docker: &bollard::Docker, id: &str) -> ApiResult<NetworkInspect> {
    Ok(docker
        .inspect_network(id, Some(query_parameters::InspectNetworkOptions::default()))
        .await?)
}

fn summary_from_network(network: &Network) -> DockerNetworkSummary {
    build_summary(
        network.id.as_deref(),
        network.name.as_deref(),
        created_at_millis(network.created.as_ref()),
        network.driver.as_deref(),
        network.scope.as_deref(),
        network.enable_ipv4,
        network.enable_ipv6,
        network.internal,
        network.attachable,
        network.ingress,
        network.config_only,
        network.ipam.as_ref(),
        network.labels.as_ref(),
    )
}

fn summary_from_inspect(network: &NetworkInspect) -> DockerNetworkSummary {
    build_summary(
        network.id.as_deref(),
        network.name.as_deref(),
        created_at_millis(network.created.as_ref()),
        network.driver.as_deref(),
        network.scope.as_deref(),
        network.enable_ipv4,
        network.enable_ipv6,
        network.internal,
        network.attachable,
        network.ingress,
        network.config_only,
        network.ipam.as_ref(),
        network.labels.as_ref(),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_summary(
    id: Option<&str>,
    name: Option<&str>,
    created_at: Option<i64>,
    driver: Option<&str>,
    scope: Option<&str>,
    enable_ipv4: Option<bool>,
    enable_ipv6: Option<bool>,
    internal: Option<bool>,
    attachable: Option<bool>,
    ingress: Option<bool>,
    config_only: Option<bool>,
    ipam: Option<&Ipam>,
    labels: Option<&HashMap<String, String>>,
) -> DockerNetworkSummary {
    let id = id.unwrap_or_default().to_string();
    let name = name
        .filter(|value| !value.is_empty())
        .unwrap_or(&id)
        .to_string();
    let management = classify_management(&name, ingress.unwrap_or(false), labels);
    let mutable = management.kind != DockerNetworkManagementKind::System;
    DockerNetworkSummary {
        id,
        name,
        created_at,
        driver: driver.unwrap_or("unknown").to_string(),
        scope: scope.unwrap_or("unknown").to_string(),
        enable_ipv4: enable_ipv4.unwrap_or(true),
        enable_ipv6: enable_ipv6.unwrap_or(false),
        internal: internal.unwrap_or(false),
        attachable: attachable.unwrap_or(false),
        ingress: ingress.unwrap_or(false),
        config_only: config_only.unwrap_or(false),
        subnets: ipam_configs(ipam)
            .into_iter()
            .filter_map(|config| config.subnet)
            .collect(),
        management,
        capabilities: DockerNetworkCapabilities {
            can_remove: mutable,
            can_manage_connections: mutable,
        },
    }
}

fn detail_from_inspect(network: &NetworkInspect) -> DockerNetworkDetail {
    let mut containers = network
        .containers
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|(id, endpoint)| DockerNetworkContainer {
                    id: id.clone(),
                    name: display_name(&endpoint.name, id),
                    endpoint_id: endpoint.endpoint_id.clone().and_then(non_empty),
                    mac_address: endpoint.mac_address.clone().and_then(non_empty),
                    ipv4_address: endpoint.ipv4_address.clone().and_then(non_empty),
                    ipv6_address: endpoint.ipv6_address.clone().and_then(non_empty),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    containers.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    DockerNetworkDetail {
        summary: summary_from_inspect(network),
        ipam_configs: ipam_configs(network.ipam.as_ref()),
        options: network.options.clone().unwrap_or_default(),
        labels: network.labels.clone().unwrap_or_default(),
        containers,
        peers: network
            .peers
            .as_ref()
            .map(|items| {
                items
                    .iter()
                    .map(|peer| DockerNetworkPeer {
                        name: peer.name.clone().and_then(non_empty),
                        ip: peer.ip.clone().and_then(non_empty),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn ipam_configs(ipam: Option<&Ipam>) -> Vec<DockerNetworkIpamConfig> {
    ipam.and_then(|value| value.config.as_ref())
        .map(|configs| {
            configs
                .iter()
                .map(|config| DockerNetworkIpamConfig {
                    subnet: config.subnet.clone().and_then(non_empty),
                    gateway: config.gateway.clone().and_then(non_empty),
                    ip_range: config.ip_range.clone().and_then(non_empty),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn classify_management(
    name: &str,
    ingress: bool,
    labels: Option<&HashMap<String, String>>,
) -> DockerNetworkManagement {
    let labels = labels.cloned().unwrap_or_default();
    let (kind, owner_name) =
        if ingress || matches!(name, "bridge" | "host" | "none" | "docker_gwbridge") {
            (
                DockerNetworkManagementKind::System,
                Some(if ingress { "Swarm" } else { "Docker" }.to_string()),
            )
        } else if labels.get("vust.owner").map(String::as_str) == Some("suite") {
            (DockerNetworkManagementKind::Suite, Some("VUST".to_string()))
        } else if let Some(project) = labels.get("com.docker.compose.project") {
            (DockerNetworkManagementKind::Compose, Some(project.clone()))
        } else {
            (DockerNetworkManagementKind::Custom, None)
        };
    DockerNetworkManagement { kind, owner_name }
}

/// 拒绝修改 Docker 默认网络和 Swarm Ingress 网络。
fn ensure_not_system_network(network: &NetworkInspect) -> ApiResult<()> {
    let summary = summary_from_inspect(network);
    if summary.management.kind == DockerNetworkManagementKind::System {
        return Err(ApiError::conflict(
            ErrorCode::DockerNetworkProtected,
            "Docker system networks cannot be changed",
        )
        .with_detail(format!(
            "network={} management={:?}",
            summary.name, summary.management.kind
        )));
    }
    Ok(())
}

fn ensure_unused(network: &NetworkInspect) -> ApiResult<()> {
    let Some(containers) = network
        .containers
        .as_ref()
        .filter(|items| !items.is_empty())
    else {
        return Ok(());
    };
    let mut names = containers
        .iter()
        .map(|(id, endpoint)| display_name(&endpoint.name, id))
        .collect::<Vec<_>>();
    names.sort();
    names.truncate(5);
    let summary = names.join(", ");
    Err(ApiError::conflict(
        ErrorCode::DockerNetworkInUse,
        format!("network still has connected containers: {summary}"),
    )
    .with_detail(summary))
}

fn build_create_request(
    payload: docker::DockerNetworkCreateRequest,
) -> ApiResult<NetworkCreateRequest> {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::validation("network name is required"));
    }
    let labels = normalize_map(payload.labels, true)?;
    let options = normalize_map(payload.options, false)?;
    let ipv4 = validate_ipam(payload.ipv4, IpFamily::V4, "ipv4")?;
    let ipv6 = validate_ipam(payload.ipv6, IpFamily::V6, "ipv6")?;
    if !payload.enable_ipv6 && ipv6.is_some() {
        return Err(ApiError::validation(
            "enableIpv6 must be true when IPv6 IPAM is configured",
        ));
    }
    let configs = [ipv4, ipv6].into_iter().flatten().collect::<Vec<_>>();
    Ok(NetworkCreateRequest {
        name,
        driver: Some("bridge".to_string()),
        internal: Some(payload.internal),
        enable_ipv4: Some(true),
        enable_ipv6: Some(payload.enable_ipv6),
        ipam: (!configs.is_empty()).then_some(Ipam {
            config: Some(configs),
            ..Default::default()
        }),
        options,
        labels,
        ..Default::default()
    })
}

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
                "option and label keys cannot be empty",
            ));
        }
        if protect_labels && (key.starts_with("vust.") || key.starts_with("com.docker.compose.")) {
            return Err(ApiError::validation(format!(
                "reserved Docker network label: {key}"
            )));
        }
        if !keys.insert(key.clone()) {
            return Err(ApiError::validation(format!("duplicate key: {key}")));
        }
        result.insert(key, raw_value.trim().to_string());
    }
    Ok((!result.is_empty()).then_some(result))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IpFamily {
    V4,
    V6,
}

#[derive(Clone, Copy)]
struct Cidr {
    address: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn parse(value: &str, family: IpFamily, field: &str) -> ApiResult<Self> {
        let (address, prefix) = value
            .split_once('/')
            .ok_or_else(|| ApiError::validation(format!("{field} must use CIDR notation")))?;
        let address = IpAddr::from_str(address)
            .map_err(|_| ApiError::validation(format!("invalid address in {field}")))?;
        let prefix = prefix
            .parse::<u8>()
            .map_err(|_| ApiError::validation(format!("invalid prefix in {field}")))?;
        let max_prefix = if address.is_ipv4() { 32 } else { 128 };
        if prefix > max_prefix || family_of(address) != family {
            return Err(ApiError::validation(format!(
                "{field} uses the wrong address family or prefix"
            )));
        }
        Ok(Self { address, prefix })
    }

    fn contains(self, address: IpAddr) -> bool {
        match (self.address, address) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                u32::from(network) & mask == u32::from(candidate) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                u128::from(network) & mask == u128::from(candidate) & mask
            }
            _ => false,
        }
    }

    fn contains_cidr(self, other: Self) -> bool {
        other.prefix >= self.prefix && self.contains(other.address)
    }
}

fn validate_ipam(
    config: Option<DockerNetworkIpamConfig>,
    family: IpFamily,
    field: &str,
) -> ApiResult<Option<IpamConfig>> {
    let Some(config) = config else {
        return Ok(None);
    };
    let subnet_text = config
        .subnet
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let gateway_text = config
        .gateway
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let range_text = config
        .ip_range
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if subnet_text.is_none() && (gateway_text.is_some() || range_text.is_some()) {
        return Err(ApiError::validation(format!(
            "{field}.subnet is required when gateway or ipRange is set"
        )));
    }
    let Some(subnet_text) = subnet_text else {
        return Ok(None);
    };
    let subnet = Cidr::parse(subnet_text, family, &format!("{field}.subnet"))?;
    if let Some(gateway) = gateway_text {
        let address = IpAddr::from_str(gateway)
            .map_err(|_| ApiError::validation(format!("invalid {field}.gateway")))?;
        if family_of(address) != family || !subnet.contains(address) {
            return Err(ApiError::validation(format!(
                "{field}.gateway must belong to its subnet"
            )));
        }
    }
    if let Some(range) = range_text {
        let range = Cidr::parse(range, family, &format!("{field}.ipRange"))?;
        if !subnet.contains_cidr(range) {
            return Err(ApiError::validation(format!(
                "{field}.ipRange must be contained by its subnet"
            )));
        }
    }
    Ok(Some(IpamConfig {
        subnet: Some(subnet_text.to_string()),
        gateway: gateway_text.map(ToString::to_string),
        ip_range: range_text.map(ToString::to_string),
        ..Default::default()
    }))
}

fn family_of(address: IpAddr) -> IpFamily {
    if address.is_ipv4() {
        IpFamily::V4
    } else {
        IpFamily::V6
    }
}

fn display_name(name: &Option<String>, fallback: &str) -> String {
    name.as_deref()
        .map(|value| value.trim_start_matches('/').trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn created_at_millis(value: Option<&String>) -> Option<i64> {
    value.and_then(|timestamp| {
        chrono::DateTime::parse_from_rfc3339(timestamp)
            .ok()
            .map(|parsed| parsed.timestamp_millis())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(name: &str, labels: &[(&str, &str)], ingress: bool) -> NetworkInspect {
        NetworkInspect {
            id: Some(format!("id-{name}")),
            name: Some(name.to_string()),
            ingress: Some(ingress),
            labels: Some(
                labels
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn classifies_managed_and_custom_networks() {
        let cases = [
            (
                network(
                    "bridge",
                    &[("com.docker.compose.project", "project")],
                    false,
                ),
                DockerNetworkManagementKind::System,
            ),
            (
                network("ingress", &[("vust.owner", "suite")], true),
                DockerNetworkManagementKind::System,
            ),
            (
                network("docker_gwbridge", &[], false),
                DockerNetworkManagementKind::System,
            ),
            (
                network(
                    "project_default",
                    &[("com.docker.compose.project", "project")],
                    false,
                ),
                DockerNetworkManagementKind::Compose,
            ),
            (
                network("vust-suite-network", &[("vust.owner", "suite")], false),
                DockerNetworkManagementKind::Suite,
            ),
            (
                network("custom", &[], false),
                DockerNetworkManagementKind::Custom,
            ),
        ];
        for (network, expected) in cases {
            let summary = summary_from_inspect(&network);
            assert_eq!(summary.management.kind, expected);
            let mutable = expected != DockerNetworkManagementKind::System;
            assert_eq!(summary.capabilities.can_remove, mutable);
            assert_eq!(summary.capabilities.can_manage_connections, mutable);
        }
    }

    #[test]
    fn protects_only_system_networks() {
        for system_network in [network("bridge", &[], false), network("ingress", &[], true)] {
            let error = ensure_not_system_network(&system_network).unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
            assert_eq!(error.code, ErrorCode::DockerNetworkProtected);
        }
        assert!(
            ensure_not_system_network(&network(
                "project_default",
                &[("com.docker.compose.project", "project")],
                false,
            ))
            .is_ok()
        );
        assert!(
            ensure_not_system_network(&network(
                "vust-suite-network",
                &[("vust.owner", "suite")],
                false,
            ))
            .is_ok()
        );
        assert!(ensure_not_system_network(&network("custom", &[], false)).is_ok());
    }

    #[test]
    fn validates_ipv4_and_ipv6_ipam() {
        let ipv4 = DockerNetworkIpamConfig {
            subnet: Some("172.20.0.0/16".to_string()),
            gateway: Some("172.20.0.1".to_string()),
            ip_range: Some("172.20.10.0/24".to_string()),
        };
        assert!(validate_ipam(Some(ipv4), IpFamily::V4, "ipv4").is_ok());
        let ipv6 = DockerNetworkIpamConfig {
            subnet: Some("fd00:20::/64".to_string()),
            gateway: Some("fd00:20::1".to_string()),
            ip_range: Some("fd00:20::/80".to_string()),
        };
        assert!(validate_ipam(Some(ipv6), IpFamily::V6, "ipv6").is_ok());
    }

    #[test]
    fn rejects_invalid_ipam_boundaries() {
        let missing_subnet = DockerNetworkIpamConfig {
            subnet: None,
            gateway: Some("172.20.0.1".to_string()),
            ip_range: None,
        };
        assert!(validate_ipam(Some(missing_subnet), IpFamily::V4, "ipv4").is_err());
        let outside_gateway = DockerNetworkIpamConfig {
            subnet: Some("172.20.0.0/16".to_string()),
            gateway: Some("172.21.0.1".to_string()),
            ip_range: None,
        };
        assert!(validate_ipam(Some(outside_gateway), IpFamily::V4, "ipv4").is_err());
        let wrong_family = DockerNetworkIpamConfig {
            subnet: Some("fd00::/64".to_string()),
            gateway: None,
            ip_range: None,
        };
        assert!(validate_ipam(Some(wrong_family), IpFamily::V4, "ipv4").is_err());
    }

    #[test]
    fn rejects_reserved_labels() {
        let labels = HashMap::from([("vust.owner".to_string(), "suite".to_string())]);
        let error = normalize_map(Some(labels), true).unwrap_err();
        assert_eq!(error.code, ErrorCode::ValidationFailed);
    }

    #[test]
    fn reports_connected_containers_as_conflict() {
        let mut network = network("custom", &[], false);
        network.containers = Some(HashMap::from([(
            "container-id".to_string(),
            bollard::models::EndpointResource {
                name: Some("web".to_string()),
                ..Default::default()
            },
        )]));
        let error = ensure_unused(&network).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.code, ErrorCode::DockerNetworkInUse);
    }

    #[test]
    fn separates_normal_and_forced_disconnect_semantics() {
        assert_eq!(disconnect_request("container", false).force, Some(false));
        assert_eq!(
            disconnect_event(false),
            ("docker_network_disconnect", false)
        );
        assert_eq!(
            disconnect_event(true),
            ("docker_network_force_disconnect", true)
        );
    }
}
