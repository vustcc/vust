//! 防火墙规则语义网关：校验 Node、转发领域查询并注入可信 Node 信息。

use crate::{
    models::{
        node_runtime_client::NodeRuntimeClient,
        nodes::{self, NodeStatus},
    },
    services::node_state_machine,
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use std::sync::Arc;
use vust_contracts::{
    api::ErrorCode,
    firewall::{
        FirewallAction, FirewallEngine, FirewallFamily, FirewallNodeReference, FirewallRuleDetail,
        FirewallRuleDetailQuery, FirewallRuleListPage, FirewallRuleListQuery, FirewallRulePageData,
        FirewallRuleSortBy, FirewallSortOrder,
    },
};

const AGENT_BASE_PATH: &str = "/api/v1/agent/firewall";

/// 构建 Node-scoped 防火墙规则公共路由。
pub fn firewall_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{node_id}/firewall/rules/list", get(list_rules))
        .route(
            "/{node_id}/firewall/rule/{rule_id}/detail",
            get(rule_detail),
        )
}

async fn list_rules(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    Query(query): Query<FirewallRuleListQuery>,
) -> ApiResult<Response> {
    let (client, node_name) = node_client(&state, &node_id).await?;
    let data: FirewallRulePageData = client.get_domain(&list_query_path(&query)?).await?;
    let page = FirewallRuleListPage {
        node: FirewallNodeReference { node_id, node_name },
        snapshot_id: data.snapshot_id,
        collected_at: data.collected_at,
        collection: data.collection,
        entries: data.entries,
        page: data.page,
        page_size: data.page_size,
        available_total: data.available_total,
        total: data.total,
        capabilities: data.capabilities,
    };
    Ok(ApiResponse::success_with_raw("Firewall rules loaded", Some(page)).into_response())
}

async fn rule_detail(
    State(state): State<Arc<AppState>>,
    Path((node_id, rule_id)): Path<(String, String)>,
    Query(query): Query<FirewallRuleDetailQuery>,
) -> ApiResult<Response> {
    let (client, _) = node_client(&state, &node_id).await?;
    let detail: FirewallRuleDetail = client
        .get_domain(&detail_query_path(&rule_id, &query.snapshot_id)?)
        .await?;
    Ok(ApiResponse::success_with_raw("Firewall rule loaded", Some(detail)).into_response())
}

async fn node_client(state: &AppState, node_id: &str) -> ApiResult<(NodeRuntimeClient, String)> {
    let node_name = if node_id == "local" {
        "Local Node".to_string()
    } else {
        let node = nodes::get_node_by_id(&state.metadata_db, node_id)
            .await
            .map_err(|error| ApiError::database(error.to_string()))?
            .ok_or_else(|| ApiError::not_found(ErrorCode::NodeNotFound, "node does not exist"))?;
        let status = NodeStatus::parse(&node.status)
            .ok_or_else(|| ApiError::internal("node has an invalid lifecycle status"))?;
        if !node_state_machine::is_proxyable(status) {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::NodeUnavailable,
                "node is not available for firewall observation",
            ));
        }
        node.name
    };
    Ok((
        NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?,
        node_name,
    ))
}

fn list_query_path(query: &FirewallRuleListQuery) -> ApiResult<String> {
    let mut pairs = Vec::new();
    push_option(&mut pairs, "snapshotId", query.snapshot_id.clone());
    push_option(&mut pairs, "query", query.query.clone());
    push_option(&mut pairs, "engine", query.engine.map(engine_key));
    push_option(&mut pairs, "family", query.family.map(family_key));
    push_option(&mut pairs, "action", query.action.map(action_key));
    push_option(
        &mut pairs,
        "page",
        query.page.map(|value| value.to_string()),
    );
    push_option(
        &mut pairs,
        "pageSize",
        query.page_size.map(|value| value.to_string()),
    );
    push_option(&mut pairs, "sortBy", query.sort_by.map(sort_key));
    push_option(
        &mut pairs,
        "sortOrder",
        query.sort_order.map(sort_order_key),
    );
    agent_query_path("/rules/list", &pairs)
}

fn detail_query_path(rule_id: &str, snapshot_id: &str) -> ApiResult<String> {
    if rule_id.len() != 64 || !rule_id.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(invalid_query("invalid firewall rule identity"));
    }
    if uuid::Uuid::parse_str(snapshot_id).is_err() {
        return Err(invalid_query("snapshotId must be a UUID"));
    }
    agent_query_path(
        &format!("/rule/{rule_id}/detail"),
        &[("snapshotId", snapshot_id.to_string())],
    )
}

fn invalid_query(message: &'static str) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::FirewallInvalidQuery,
        message,
    )
    .with_message_key("api.errors.FIREWALL_INVALID_QUERY")
}

fn push_option(pairs: &mut Vec<(&'static str, String)>, key: &'static str, value: Option<String>) {
    if let Some(value) = value {
        pairs.push((key, value));
    }
}

fn agent_query_path(suffix: &str, pairs: &[(&str, String)]) -> ApiResult<String> {
    let mut url = reqwest::Url::parse(&format!("http://agent{AGENT_BASE_PATH}{suffix}"))
        .map_err(|_| ApiError::internal("failed to build Agent firewall URL"))?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
    }
    let query = url
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    Ok(format!("{}{query}", url.path()))
}

fn engine_key(value: FirewallEngine) -> String {
    match value {
        FirewallEngine::Nftables => "nftables",
        FirewallEngine::Iptables => "iptables",
    }
    .to_string()
}

fn family_key(value: FirewallFamily) -> String {
    match value {
        FirewallFamily::Inet => "inet",
        FirewallFamily::Ipv4 => "ipv4",
        FirewallFamily::Ipv6 => "ipv6",
        FirewallFamily::Bridge => "bridge",
        FirewallFamily::Arp => "arp",
        FirewallFamily::Netdev => "netdev",
        FirewallFamily::Unknown => "unknown",
    }
    .to_string()
}

fn action_key(value: FirewallAction) -> String {
    match value {
        FirewallAction::Accept => "accept",
        FirewallAction::Drop => "drop",
        FirewallAction::Reject => "reject",
        FirewallAction::Return => "return",
        FirewallAction::Jump => "jump",
        FirewallAction::Goto => "goto",
        FirewallAction::Log => "log",
        FirewallAction::Masquerade => "masquerade",
        FirewallAction::Dnat => "dnat",
        FirewallAction::Snat => "snat",
        FirewallAction::Redirect => "redirect",
        FirewallAction::Queue => "queue",
        FirewallAction::Continue => "continue",
        FirewallAction::Other => "other",
    }
    .to_string()
}

fn sort_key(value: FirewallRuleSortBy) -> String {
    match value {
        FirewallRuleSortBy::RuleOrder => "ruleOrder",
        FirewallRuleSortBy::Action => "action",
        FirewallRuleSortBy::Protocol => "protocol",
        FirewallRuleSortBy::Source => "source",
        FirewallRuleSortBy::Destination => "destination",
    }
    .to_string()
}

fn sort_order_key(value: FirewallSortOrder) -> String {
    match value {
        FirewallSortOrder::Asc => "asc",
        FirewallSortOrder::Desc => "desc",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_query_is_encoded_for_agent() {
        let path = list_query_path(&FirewallRuleListQuery {
            query: Some("tcp 22".to_string()),
            family: Some(FirewallFamily::Ipv4),
            page: Some(2),
            page_size: Some(50),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            path,
            "/api/v1/agent/firewall/rules/list?query=tcp+22&family=ipv4&page=2&pageSize=50"
        );
    }

    #[test]
    fn detail_query_requires_stable_ids() {
        let snapshot_id = uuid::Uuid::new_v4().to_string();
        let rule_id = "a".repeat(64);
        let path = detail_query_path(&rule_id, &snapshot_id).unwrap();
        assert!(path.contains(&format!("/rule/{rule_id}/detail")));
        let error = detail_query_path("bad", &snapshot_id).unwrap_err();
        assert_eq!(error.code, ErrorCode::FirewallInvalidQuery);
        assert_eq!(
            error.message_key.as_deref(),
            Some("api.errors.FIREWALL_INVALID_QUERY")
        );
    }
}
