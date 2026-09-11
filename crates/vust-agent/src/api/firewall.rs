//! 防火墙规则观察内部 API：采集实际生效规则并提供不可变快照查询。

use crate::{
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult},
};
use axum::{
    Router,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{HashMap, VecDeque},
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use uuid::Uuid;
use vust_contracts::{
    api::ErrorCode,
    firewall::{
        FirewallAction, FirewallCollection, FirewallCollectionSource, FirewallCollectionStatus,
        FirewallCollectionWarning, FirewallEngine, FirewallFamily, FirewallRuleCapabilities,
        FirewallRuleDetail, FirewallRuleDetailQuery, FirewallRuleEffect, FirewallRuleKind,
        FirewallRuleListQuery, FirewallRuleMatch, FirewallRulePageData, FirewallRuleParseStatus,
        FirewallRuleSortBy, FirewallRuleSummary, FirewallSortOrder, FirewallSourceKind,
        FirewallSourceStatus, FirewallWarningCode,
    },
};

const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 200;
const MAX_QUERY_LENGTH: usize = 256;
const MAX_RULES: usize = 50_000;
const MAX_SOURCE_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const COLLECTION_TIMEOUT: Duration = Duration::from_secs(12);
const SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_SNAPSHOTS: usize = 8;

static SNAPSHOTS: LazyLock<Mutex<VecDeque<FirewallSnapshot>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

#[derive(Debug, Clone)]
struct FirewallSnapshot {
    snapshot_id: String,
    collected_at: String,
    created_at: Instant,
    collection: FirewallCollection,
    rules: Vec<FirewallRuleDetail>,
}

#[derive(Debug)]
enum CommandFailure {
    Failed,
    Timeout,
    TooLarge,
}

#[derive(Debug)]
struct SourceResult {
    source: FirewallSourceKind,
    status: FirewallSourceStatus,
    rules: Vec<FirewallRuleDetail>,
    parse_partial: bool,
    too_large: bool,
}

/// 注册防火墙规则内部领域路由。
pub fn firewall_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rules/list", get(list_rules))
        .route("/rule/{rule_id}/detail", get(rule_detail))
}

async fn list_rules(Query(query): Query<FirewallRuleListQuery>) -> ApiResult<Response> {
    validate_query(&query)?;
    let snapshot = if let Some(snapshot_id) = query.snapshot_id.as_deref() {
        load_snapshot(snapshot_id)?
    } else {
        let collected = timeout(COLLECTION_TIMEOUT, collect_snapshot())
            .await
            .map_err(|_| firewall_unavailable("firewall collection timed out"))??;
        store_snapshot(collected.clone())?;
        collected
    };
    let page = page_from_snapshot(&snapshot, &query);
    Ok(ApiResponse::success_with_raw("Firewall rules loaded", Some(page)).into_response())
}

async fn rule_detail(
    Path(rule_id): Path<String>,
    Query(query): Query<FirewallRuleDetailQuery>,
) -> ApiResult<Response> {
    validate_rule_id(&rule_id)?;
    validate_snapshot_id(&query.snapshot_id)?;
    let snapshot = load_snapshot(&query.snapshot_id)?;
    let detail = snapshot
        .rules
        .iter()
        .find(|item| item.summary.rule_id == rule_id)
        .cloned()
        .ok_or_else(|| {
            firewall_error(
                StatusCode::NOT_FOUND,
                ErrorCode::FirewallRuleNotFound,
                "firewall rule does not exist in this snapshot",
            )
        })?;
    Ok(ApiResponse::success_with_raw("Firewall rule loaded", Some(detail)).into_response())
}

fn validate_query(query: &FirewallRuleListQuery) -> ApiResult<()> {
    if query.page.is_some_and(|page| page == 0)
        || query
            .page_size
            .is_some_and(|page_size| page_size == 0 || page_size > MAX_PAGE_SIZE)
        || query
            .query
            .as_ref()
            .is_some_and(|value| value.chars().count() > MAX_QUERY_LENGTH)
    {
        return Err(invalid_query("invalid firewall list query"));
    }
    if let Some(snapshot_id) = query.snapshot_id.as_deref() {
        validate_snapshot_id(snapshot_id)?;
    }
    Ok(())
}

fn validate_snapshot_id(snapshot_id: &str) -> ApiResult<()> {
    Uuid::parse_str(snapshot_id)
        .map(|_| ())
        .map_err(|_| invalid_query("snapshotId must be a UUID"))
}

fn validate_rule_id(rule_id: &str) -> ApiResult<()> {
    if rule_id.len() == 64 && rule_id.bytes().all(|value| value.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(invalid_query("invalid firewall rule identity"))
    }
}

fn invalid_query(message: &'static str) -> ApiError {
    firewall_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::FirewallInvalidQuery,
        message,
    )
}

fn firewall_unavailable(message: &'static str) -> ApiError {
    firewall_error(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::FirewallUnavailable,
        message,
    )
}

fn firewall_error(status: StatusCode, code: ErrorCode, message: &'static str) -> ApiError {
    ApiError::new(status, code, message).with_message_key(format!("api.errors.{}", code.as_str()))
}

fn load_snapshot(snapshot_id: &str) -> ApiResult<FirewallSnapshot> {
    let mut snapshots = SNAPSHOTS
        .lock()
        .map_err(|_| ApiError::internal("firewall snapshot store is unavailable"))?;
    purge_expired(&mut snapshots);
    snapshots
        .iter()
        .find(|snapshot| snapshot.snapshot_id == snapshot_id)
        .cloned()
        .ok_or_else(|| {
            firewall_error(
                StatusCode::GONE,
                ErrorCode::FirewallSnapshotExpired,
                "firewall snapshot is unavailable or expired",
            )
        })
}

fn store_snapshot(snapshot: FirewallSnapshot) -> ApiResult<()> {
    let mut snapshots = SNAPSHOTS
        .lock()
        .map_err(|_| ApiError::internal("firewall snapshot store is unavailable"))?;
    purge_expired(&mut snapshots);
    snapshots.push_front(snapshot);
    snapshots.truncate(MAX_SNAPSHOTS);
    Ok(())
}

fn purge_expired(snapshots: &mut VecDeque<FirewallSnapshot>) {
    snapshots.retain(|snapshot| snapshot.created_at.elapsed() < SNAPSHOT_TTL);
}

async fn collect_snapshot() -> ApiResult<FirewallSnapshot> {
    #[cfg(not(target_os = "linux"))]
    {
        return Err(firewall_error(
            StatusCode::NOT_IMPLEMENTED,
            ErrorCode::FirewallUnsupportedPlatform,
            "firewall rule observation is supported on Linux only",
        ));
    }

    #[cfg(target_os = "linux")]
    {
        let nft_path = find_program("nft");
        let iptables_v4_path = find_program("iptables-save");
        let iptables_v6_path = find_program("ip6tables-save");

        if nft_path.is_none() && iptables_v4_path.is_none() && iptables_v6_path.is_none() {
            return Err(firewall_unavailable(
                "no supported firewall rule reader is available",
            ));
        }

        let mut nft_result = collect_nft(nft_path.as_deref()).await;
        let iptables_uses_nft = if let Some(path) = iptables_v4_path.as_deref() {
            detect_iptables_nft(path).await.unwrap_or(false)
        } else {
            false
        };

        let nft_succeeded = source_succeeded(nft_result.status);
        let v4_result = if nft_succeeded && (iptables_uses_nft || iptables_v4_path.is_none()) {
            skipped_source(FirewallSourceKind::IptablesV4)
        } else {
            collect_iptables(
                FirewallSourceKind::IptablesV4,
                FirewallFamily::Ipv4,
                iptables_v4_path.as_deref(),
            )
            .await
        };
        let v6_result = if nft_succeeded && (iptables_uses_nft || iptables_v6_path.is_none()) {
            skipped_source(FirewallSourceKind::IptablesV6)
        } else {
            collect_iptables(
                FirewallSourceKind::IptablesV6,
                FirewallFamily::Ipv6,
                iptables_v6_path.as_deref(),
            )
            .await
        };
        if nft_path.is_none()
            && (source_succeeded(v4_result.status) || source_succeeded(v6_result.status))
        {
            nft_result = skipped_source(FirewallSourceKind::Nftables);
        }

        let mut source_results = vec![nft_result, v4_result, v6_result];
        if source_results.iter().any(|result| result.too_large) {
            return Err(firewall_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::FirewallRulesetTooLarge,
                "firewall ruleset exceeds the supported size",
            ));
        }
        let succeeded = source_results
            .iter()
            .filter(|result| source_succeeded(result.status))
            .count();
        if succeeded == 0 {
            return Err(firewall_unavailable(
                "firewall rules could not be read from any source",
            ));
        }

        let collection = collection_from_sources(&source_results);
        let mut rules = Vec::new();
        for result in &mut source_results {
            rules.append(&mut result.rules);
        }
        if rules.len() > MAX_RULES {
            return Err(firewall_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::FirewallRulesetTooLarge,
                "firewall ruleset exceeds the supported size",
            ));
        }
        assign_rule_ids(&mut rules)?;
        Ok(FirewallSnapshot {
            snapshot_id: Uuid::new_v4().to_string(),
            collected_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            created_at: Instant::now(),
            collection,
            rules,
        })
    }
}

fn source_succeeded(status: FirewallSourceStatus) -> bool {
    matches!(
        status,
        FirewallSourceStatus::Loaded | FirewallSourceStatus::Empty
    )
}

fn skipped_source(source: FirewallSourceKind) -> SourceResult {
    SourceResult {
        source,
        status: FirewallSourceStatus::Skipped,
        rules: Vec::new(),
        parse_partial: false,
        too_large: false,
    }
}

async fn collect_nft(path: Option<&FsPath>) -> SourceResult {
    let Some(path) = path else {
        return unavailable_source(FirewallSourceKind::Nftables);
    };
    match run_command(path, &["-j", "-a", "list", "ruleset"]).await {
        Ok(output) => match parse_nft_rules(&output) {
            Ok((rules, partial)) => SourceResult {
                source: FirewallSourceKind::Nftables,
                status: if rules.is_empty() {
                    FirewallSourceStatus::Empty
                } else {
                    FirewallSourceStatus::Loaded
                },
                rules,
                parse_partial: partial,
                too_large: false,
            },
            Err(_) => failed_source(FirewallSourceKind::Nftables, FirewallSourceStatus::Failed),
        },
        Err(CommandFailure::Timeout) => {
            failed_source(FirewallSourceKind::Nftables, FirewallSourceStatus::Timeout)
        }
        Err(CommandFailure::TooLarge) => too_large_source(FirewallSourceKind::Nftables),
        Err(CommandFailure::Failed) => {
            failed_source(FirewallSourceKind::Nftables, FirewallSourceStatus::Failed)
        }
    }
}

async fn collect_iptables(
    source: FirewallSourceKind,
    family: FirewallFamily,
    path: Option<&FsPath>,
) -> SourceResult {
    let Some(path) = path else {
        return unavailable_source(source);
    };
    match run_command(path, &[]).await {
        Ok(output) => {
            let (rules, partial) = parse_iptables_save(&output, family);
            SourceResult {
                source,
                status: if rules.is_empty() {
                    FirewallSourceStatus::Empty
                } else {
                    FirewallSourceStatus::Loaded
                },
                rules,
                parse_partial: partial,
                too_large: false,
            }
        }
        Err(CommandFailure::Timeout) => failed_source(source, FirewallSourceStatus::Timeout),
        Err(CommandFailure::TooLarge) => too_large_source(source),
        Err(CommandFailure::Failed) => failed_source(source, FirewallSourceStatus::Failed),
    }
}

fn unavailable_source(source: FirewallSourceKind) -> SourceResult {
    failed_source(source, FirewallSourceStatus::Unavailable)
}

fn failed_source(source: FirewallSourceKind, status: FirewallSourceStatus) -> SourceResult {
    SourceResult {
        source,
        status,
        rules: Vec::new(),
        parse_partial: false,
        too_large: false,
    }
}

fn too_large_source(source: FirewallSourceKind) -> SourceResult {
    SourceResult {
        source,
        status: FirewallSourceStatus::Failed,
        rules: Vec::new(),
        parse_partial: false,
        too_large: true,
    }
}

async fn detect_iptables_nft(path: &FsPath) -> Option<bool> {
    let output = run_command(path, &["--version"]).await.ok()?;
    Some(output.to_ascii_lowercase().contains("nf_tables"))
}

fn find_program(name: &str) -> Option<PathBuf> {
    ["/usr/sbin", "/usr/bin", "/sbin", "/bin"]
        .iter()
        .map(|directory| FsPath::new(directory).join(name))
        .find(|path| path.is_file())
}

async fn run_command(path: &FsPath, args: &[&str]) -> Result<String, CommandFailure> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| CommandFailure::Failed)?;
    let stdout = child.stdout.take().ok_or(CommandFailure::Failed)?;
    let result = timeout(COMMAND_TIMEOUT, async {
        let mut output = Vec::new();
        stdout
            .take((MAX_SOURCE_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|_| CommandFailure::Failed)?;
        if output.len() > MAX_SOURCE_OUTPUT_BYTES {
            return Err(CommandFailure::TooLarge);
        }
        let status = child.wait().await.map_err(|_| CommandFailure::Failed)?;
        if !status.success() {
            return Err(CommandFailure::Failed);
        }
        String::from_utf8(output).map_err(|_| CommandFailure::Failed)
    })
    .await;
    match result {
        Ok(value) => value,
        Err(_) => {
            let _ = child.kill().await;
            Err(CommandFailure::Timeout)
        }
    }
}

fn collection_from_sources(results: &[SourceResult]) -> FirewallCollection {
    let applicable = results
        .iter()
        .filter(|result| result.status != FirewallSourceStatus::Skipped)
        .count();
    let succeeded = results
        .iter()
        .filter(|result| source_succeeded(result.status))
        .count();
    let warnings = results
        .iter()
        .flat_map(|result| {
            let mut values = Vec::new();
            let code = match result.status {
                FirewallSourceStatus::Unavailable => Some(FirewallWarningCode::SourceUnavailable),
                FirewallSourceStatus::Failed => Some(FirewallWarningCode::SourceFailed),
                FirewallSourceStatus::Timeout => Some(FirewallWarningCode::SourceTimeout),
                FirewallSourceStatus::Skipped => None,
                FirewallSourceStatus::Loaded | FirewallSourceStatus::Empty => None,
            };
            if let Some(code) = code {
                values.push(FirewallCollectionWarning {
                    code,
                    source: result.source,
                });
            }
            if result.parse_partial {
                values.push(FirewallCollectionWarning {
                    code: FirewallWarningCode::SourceParsePartial,
                    source: result.source,
                });
            }
            values
        })
        .collect();
    FirewallCollection {
        status: if succeeded == applicable {
            FirewallCollectionStatus::Complete
        } else {
            FirewallCollectionStatus::Partial
        },
        coverage_percent: if applicable == 0 {
            0.0
        } else {
            (succeeded as f64 / applicable as f64) * 100.0
        },
        sources: results
            .iter()
            .map(|result| FirewallCollectionSource {
                source: result.source,
                status: result.status,
                rule_count: result.rules.len(),
            })
            .collect(),
        warnings,
    }
}

fn page_from_snapshot(
    snapshot: &FirewallSnapshot,
    query: &FirewallRuleListQuery,
) -> FirewallRulePageData {
    let mut entries = snapshot
        .rules
        .iter()
        .map(|detail| detail.summary.clone())
        .filter(|rule| matches_query(rule, query))
        .collect::<Vec<_>>();
    sort_rules(
        &mut entries,
        query.sort_by.unwrap_or(FirewallRuleSortBy::RuleOrder),
        query.sort_order.unwrap_or(FirewallSortOrder::Asc),
    );
    let total = entries.len();
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(DEFAULT_PAGE_SIZE);
    let start = page.saturating_sub(1).saturating_mul(page_size);
    let entries = entries.into_iter().skip(start).take(page_size).collect();
    FirewallRulePageData {
        snapshot_id: snapshot.snapshot_id.clone(),
        collected_at: snapshot.collected_at.clone(),
        collection: snapshot.collection.clone(),
        entries,
        page,
        page_size,
        available_total: snapshot.rules.len(),
        total,
        capabilities: FirewallRuleCapabilities {
            can_view_detail: true,
        },
    }
}

fn matches_query(rule: &FirewallRuleSummary, query: &FirewallRuleListQuery) -> bool {
    if query.engine.is_some_and(|value| value != rule.engine)
        || query.family.is_some_and(|value| value != rule.family)
        || query.action.is_some_and(|value| value != rule.action)
    {
        return false;
    }
    let Some(value) = query
        .query
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return true;
    };
    let needle = value.to_ascii_lowercase();
    [
        Some(rule.table.as_str()),
        Some(rule.chain.as_str()),
        rule.action_target.as_deref(),
        rule.protocol.as_deref(),
        rule.source_address.as_deref(),
        rule.destination_address.as_deref(),
        rule.input_interface.as_deref(),
        rule.output_interface.as_deref(),
        rule.comment.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(rule.source_ports.iter().map(String::as_str))
    .chain(rule.destination_ports.iter().map(String::as_str))
    .any(|candidate| candidate.to_ascii_lowercase().contains(&needle))
}

fn sort_rules(
    entries: &mut [FirewallRuleSummary],
    sort_by: FirewallRuleSortBy,
    order: FirewallSortOrder,
) {
    entries.sort_by(|left, right| {
        let ordering = match sort_by {
            FirewallRuleSortBy::RuleOrder => rule_order(left, right),
            FirewallRuleSortBy::Action => left.action.cmp(&right.action),
            FirewallRuleSortBy::Protocol => left.protocol.cmp(&right.protocol),
            FirewallRuleSortBy::Source => left.source_address.cmp(&right.source_address),
            FirewallRuleSortBy::Destination => {
                left.destination_address.cmp(&right.destination_address)
            }
        };
        if order == FirewallSortOrder::Asc {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn rule_order(left: &FirewallRuleSummary, right: &FirewallRuleSummary) -> Ordering {
    left.engine
        .cmp(&right.engine)
        .then_with(|| left.family.cmp(&right.family))
        .then_with(|| left.table.cmp(&right.table))
        .then_with(|| left.chain.cmp(&right.chain))
        .then_with(|| left.position.cmp(&right.position))
}

fn parse_nft_rules(output: &str) -> Result<(Vec<FirewallRuleDetail>, bool), serde_json::Error> {
    let document: Value = serde_json::from_str(output)?;
    let mut rules = Vec::new();
    let mut positions: HashMap<(String, String, String), usize> = HashMap::new();
    let mut partial = false;
    for item in document
        .get("nftables")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(chain) = item.get("chain") {
            if let Some(policy) = chain.get("policy").and_then(Value::as_str) {
                let family = parse_nft_family(chain.get("family").and_then(Value::as_str));
                let table = string_field(chain, "table");
                let chain_name = string_field(chain, "name");
                let action = parse_action(policy);
                rules.push(FirewallRuleDetail {
                    summary: base_summary(
                        FirewallRuleKind::Policy,
                        FirewallEngine::Nftables,
                        family,
                        table,
                        chain_name,
                        0,
                        action,
                    ),
                    matches: Vec::new(),
                    effects: vec![FirewallRuleEffect {
                        action,
                        target: None,
                    }],
                    parse_status: FirewallRuleParseStatus::Complete,
                });
            }
            continue;
        }
        let Some(rule) = item.get("rule") else {
            continue;
        };
        let family = parse_nft_family(rule.get("family").and_then(Value::as_str));
        let table = string_field(rule, "table");
        let chain = string_field(rule, "chain");
        let key = (format!("{family:?}"), table.clone(), chain.clone());
        let position = positions.entry(key).or_default();
        *position += 1;
        let mut detail = FirewallRuleDetail {
            summary: base_summary(
                FirewallRuleKind::Rule,
                FirewallEngine::Nftables,
                family,
                table,
                chain,
                *position,
                FirewallAction::Continue,
            ),
            matches: Vec::new(),
            effects: Vec::new(),
            parse_status: FirewallRuleParseStatus::Complete,
        };
        detail.summary.comment = rule
            .get("comment")
            .and_then(Value::as_str)
            .map(str::to_string);
        for expression in rule
            .get("expr")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(value) = expression.get("match") {
                parse_nft_match(value, &mut detail);
            } else if expression.get("counter").is_some() {
                continue;
            } else if let Some(effect) = parse_nft_effect(expression) {
                if effect.action != FirewallAction::Log {
                    detail.summary.action = effect.action;
                    detail.summary.action_target = effect.target.clone();
                }
                detail.effects.push(effect);
            } else {
                detail.parse_status = FirewallRuleParseStatus::Partial;
                partial = true;
            }
        }
        rules.push(detail);
    }
    Ok((rules, partial))
}

fn parse_nft_match(value: &Value, detail: &mut FirewallRuleDetail) {
    let operator = value
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or("==")
        .to_string();
    let field = nft_field(value.get("left"));
    let match_value = nft_value_string(value.get("right"));
    detail.matches.push(FirewallRuleMatch {
        field: field.clone(),
        operator,
        value: match_value.clone(),
    });
    match field.as_str() {
        "ip.saddr" | "ip6.saddr" => detail.summary.source_address = Some(match_value),
        "ip.daddr" | "ip6.daddr" => detail.summary.destination_address = Some(match_value),
        value if value.ends_with(".sport") => {
            detail.summary.source_ports.push(match_value);
            set_protocol_from_payload(value, detail);
        }
        value if value.ends_with(".dport") => {
            detail.summary.destination_ports.push(match_value);
            set_protocol_from_payload(value, detail);
        }
        "meta.iifname" => detail.summary.input_interface = Some(match_value),
        "meta.oifname" => detail.summary.output_interface = Some(match_value),
        "meta.l4proto" => detail.summary.protocol = Some(match_value),
        value if value.ends_with(".protocol") => {
            detail.summary.protocol = Some(match_value);
        }
        value if value.starts_with("tcp.") => detail.summary.protocol = Some("tcp".to_string()),
        value if value.starts_with("udp.") => detail.summary.protocol = Some("udp".to_string()),
        _ => {}
    }
}

fn set_protocol_from_payload(field: &str, detail: &mut FirewallRuleDetail) {
    if field.starts_with("tcp.") {
        detail.summary.protocol = Some("tcp".to_string());
    } else if field.starts_with("udp.") {
        detail.summary.protocol = Some("udp".to_string());
    }
}

fn parse_nft_effect(expression: &Value) -> Option<FirewallRuleEffect> {
    for (key, action) in [
        ("accept", FirewallAction::Accept),
        ("drop", FirewallAction::Drop),
        ("reject", FirewallAction::Reject),
        ("return", FirewallAction::Return),
        ("jump", FirewallAction::Jump),
        ("goto", FirewallAction::Goto),
        ("log", FirewallAction::Log),
        ("masquerade", FirewallAction::Masquerade),
        ("dnat", FirewallAction::Dnat),
        ("snat", FirewallAction::Snat),
        ("redirect", FirewallAction::Redirect),
        ("queue", FirewallAction::Queue),
    ] {
        if let Some(value) = expression.get(key) {
            let target = value
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    value
                        .get("target")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .or_else(|| {
                    value.get("addr").map(|address| {
                        let address = nft_value_string(Some(address));
                        value
                            .get("port")
                            .map(|port| format!("{address}:{}", nft_value_string(Some(port))))
                            .unwrap_or(address)
                    })
                });
            return Some(FirewallRuleEffect { action, target });
        }
    }
    None
}

fn nft_field(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "unknown".to_string();
    };
    if let Some(payload) = value.get("payload") {
        return format!(
            "{}.{}",
            payload
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("payload"),
            payload
                .get("field")
                .and_then(Value::as_str)
                .unwrap_or("value")
        );
    }
    if let Some(meta) = value.get("meta") {
        return format!(
            "meta.{}",
            meta.get("key").and_then(Value::as_str).unwrap_or("value")
        );
    }
    nft_value_string(Some(value))
}

fn nft_value_string(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Array(values) => values
            .iter()
            .map(|value| nft_value_string(Some(value)))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => {
            if let Some(range) = map.get("range").and_then(Value::as_array) {
                return range
                    .iter()
                    .map(|value| nft_value_string(Some(value)))
                    .collect::<Vec<_>>()
                    .join("-");
            }
            serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
        }
        Value::Null => String::new(),
    }
}

fn parse_nft_family(value: Option<&str>) -> FirewallFamily {
    match value {
        Some("inet") => FirewallFamily::Inet,
        Some("ip") => FirewallFamily::Ipv4,
        Some("ip6") => FirewallFamily::Ipv6,
        Some("bridge") => FirewallFamily::Bridge,
        Some("arp") => FirewallFamily::Arp,
        Some("netdev") => FirewallFamily::Netdev,
        _ => FirewallFamily::Unknown,
    }
}

fn parse_iptables_save(output: &str, family: FirewallFamily) -> (Vec<FirewallRuleDetail>, bool) {
    let mut table = "filter".to_string();
    let mut positions: HashMap<String, usize> = HashMap::new();
    let mut rules = Vec::new();
    let mut partial = false;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(value) = line.strip_prefix('*') {
            table = value.to_string();
            positions.clear();
            continue;
        }
        if let Some(value) = line.strip_prefix(':') {
            let tokens = value.split_whitespace().collect::<Vec<_>>();
            if let (Some(chain), Some(policy)) = (tokens.first(), tokens.get(1))
                && *policy != "-"
            {
                let action = parse_action(policy);
                rules.push(FirewallRuleDetail {
                    summary: base_summary(
                        FirewallRuleKind::Policy,
                        FirewallEngine::Iptables,
                        family,
                        table.clone(),
                        (*chain).to_string(),
                        0,
                        action,
                    ),
                    matches: Vec::new(),
                    effects: vec![FirewallRuleEffect {
                        action,
                        target: None,
                    }],
                    parse_status: FirewallRuleParseStatus::Complete,
                });
            }
            continue;
        }
        if !line.starts_with("-A ") {
            continue;
        }
        let tokens = tokenize_iptables(line);
        if tokens.len() < 2 {
            partial = true;
            continue;
        }
        let chain = tokens[1].clone();
        let position = positions.entry(chain.clone()).or_default();
        *position += 1;
        let mut detail = FirewallRuleDetail {
            summary: base_summary(
                FirewallRuleKind::Rule,
                FirewallEngine::Iptables,
                family,
                table.clone(),
                chain,
                *position,
                FirewallAction::Continue,
            ),
            matches: Vec::new(),
            effects: Vec::new(),
            parse_status: FirewallRuleParseStatus::Complete,
        };
        let mut index = 2;
        let mut negated = false;
        while index < tokens.len() {
            let token = tokens[index].as_str();
            let value = tokens.get(index + 1).cloned();
            let mut advance = 1;
            match token {
                "!" => negated = true,
                "-p" | "--protocol" => {
                    set_match(&mut detail, "protocol", value, negated);
                    advance = 2;
                    negated = false;
                }
                "-s" | "--source" => {
                    set_match(&mut detail, "sourceAddress", value, negated);
                    advance = 2;
                    negated = false;
                }
                "-d" | "--destination" => {
                    set_match(&mut detail, "destinationAddress", value, negated);
                    advance = 2;
                    negated = false;
                }
                "--sport" | "--source-port" | "--sports" => {
                    set_match(&mut detail, "sourcePort", value, negated);
                    advance = 2;
                    negated = false;
                }
                "--dport" | "--destination-port" | "--dports" => {
                    set_match(&mut detail, "destinationPort", value, negated);
                    advance = 2;
                    negated = false;
                }
                "-i" | "--in-interface" => {
                    set_match(&mut detail, "inputInterface", value, negated);
                    advance = 2;
                    negated = false;
                }
                "-o" | "--out-interface" => {
                    set_match(&mut detail, "outputInterface", value, negated);
                    advance = 2;
                    negated = false;
                }
                "--comment" => {
                    set_match(&mut detail, "comment", value, negated);
                    advance = 2;
                    negated = false;
                }
                "-j" | "--jump" | "-g" | "--goto" => {
                    if let Some(target) = value {
                        let (action, action_target) = parse_iptables_target(token, &target);
                        detail.summary.action = action;
                        detail.summary.action_target = action_target.clone();
                        detail.effects.push(FirewallRuleEffect {
                            action,
                            target: action_target,
                        });
                    }
                    advance = 2;
                    negated = false;
                }
                "-m" | "--match" | "-t" | "--table" => {
                    advance = 2;
                    negated = false;
                }
                value if value.starts_with('-') => {
                    detail.parse_status = FirewallRuleParseStatus::Partial;
                    partial = true;
                    if tokens
                        .get(index + 1)
                        .is_some_and(|next| !next.starts_with('-'))
                    {
                        advance = 2;
                    }
                    negated = false;
                }
                _ => {}
            }
            index += advance;
        }
        rules.push(detail);
    }
    (rules, partial)
}

fn set_match(detail: &mut FirewallRuleDetail, field: &str, value: Option<String>, negated: bool) {
    let Some(value) = value else {
        detail.parse_status = FirewallRuleParseStatus::Partial;
        return;
    };
    detail.matches.push(FirewallRuleMatch {
        field: field.to_string(),
        operator: if negated { "notEquals" } else { "equals" }.to_string(),
        value: value.clone(),
    });
    match field {
        "protocol" => detail.summary.protocol = Some(value),
        "sourceAddress" => detail.summary.source_address = Some(value),
        "destinationAddress" => detail.summary.destination_address = Some(value),
        "sourcePort" => detail.summary.source_ports.push(value),
        "destinationPort" => detail.summary.destination_ports.push(value),
        "inputInterface" => detail.summary.input_interface = Some(value),
        "outputInterface" => detail.summary.output_interface = Some(value),
        "comment" => detail.summary.comment = Some(value),
        _ => {}
    }
}

fn parse_iptables_target(token: &str, target: &str) -> (FirewallAction, Option<String>) {
    let normalized = target.to_ascii_lowercase();
    let action = parse_action(&normalized);
    let target = if matches!(token, "-g" | "--goto") || action == FirewallAction::Other {
        Some(target.to_string())
    } else {
        None
    };
    let action = if matches!(token, "-g" | "--goto") {
        FirewallAction::Goto
    } else if target.is_some() && action == FirewallAction::Other {
        FirewallAction::Jump
    } else {
        action
    };
    (action, target)
}

fn tokenize_iptables(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn parse_action(value: &str) -> FirewallAction {
    match value.to_ascii_lowercase().as_str() {
        "accept" => FirewallAction::Accept,
        "drop" => FirewallAction::Drop,
        "reject" => FirewallAction::Reject,
        "return" => FirewallAction::Return,
        "jump" => FirewallAction::Jump,
        "goto" => FirewallAction::Goto,
        "log" => FirewallAction::Log,
        "masquerade" => FirewallAction::Masquerade,
        "dnat" => FirewallAction::Dnat,
        "snat" => FirewallAction::Snat,
        "redirect" => FirewallAction::Redirect,
        "queue" => FirewallAction::Queue,
        "continue" => FirewallAction::Continue,
        _ => FirewallAction::Other,
    }
}

fn base_summary(
    kind: FirewallRuleKind,
    engine: FirewallEngine,
    family: FirewallFamily,
    table: String,
    chain: String,
    position: usize,
    action: FirewallAction,
) -> FirewallRuleSummary {
    FirewallRuleSummary {
        rule_id: String::new(),
        kind,
        engine,
        family,
        table,
        chain,
        position,
        action,
        action_target: None,
        protocol: None,
        source_address: None,
        destination_address: None,
        source_ports: Vec::new(),
        destination_ports: Vec::new(),
        input_interface: None,
        output_interface: None,
        comment: None,
        capabilities: FirewallRuleCapabilities {
            can_view_detail: true,
        },
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn assign_rule_ids(rules: &mut [FirewallRuleDetail]) -> ApiResult<()> {
    let mut occurrences = HashMap::<String, usize>::new();
    for detail in rules {
        let mut identity = detail.clone();
        identity.summary.rule_id.clear();
        identity.summary.position = 0;
        let canonical = serde_json::to_vec(&identity)
            .map_err(|_| ApiError::internal("failed to identify firewall rule"))?;
        let base = hex::encode(Sha256::digest(&canonical));
        let occurrence = occurrences.entry(base.clone()).or_default();
        let rule_id = Sha256::digest(format!("{base}:{occurrence}").as_bytes());
        detail.summary.rule_id = hex::encode(rule_id);
        *occurrence += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firewall_errors_include_message_key_and_error_code() {
        let error = firewall_unavailable("unavailable");

        assert_eq!(error.code, ErrorCode::FirewallUnavailable);
        assert_eq!(
            error.message_key.as_deref(),
            Some("api.errors.FIREWALL_UNAVAILABLE")
        );
    }

    #[test]
    fn parses_nft_json_rules_and_chain_policy() {
        let input = r#"{"nftables":[
          {"chain":{"family":"inet","table":"filter","name":"input","policy":"drop"}},
          {"rule":{"family":"inet","table":"filter","chain":"input","expr":[
            {"match":{"op":"==","left":{"payload":{"protocol":"tcp","field":"dport"}},"right":22}},
            {"accept":null}
          ]}}
        ]}"#;
        let (rules, partial) = parse_nft_rules(input).unwrap();
        assert!(!partial);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].summary.kind, FirewallRuleKind::Policy);
        assert_eq!(rules[0].summary.action, FirewallAction::Drop);
        assert_eq!(rules[1].summary.destination_ports, vec!["22"]);
        assert_eq!(rules[1].summary.protocol.as_deref(), Some("tcp"));
    }

    #[test]
    fn parses_iptables_save_policies_and_rule_fields() {
        let input = r#"*filter
:INPUT DROP [0:0]
-A INPUT -s 10.0.0.0/8 -p tcp --dport 22 -m comment --comment "admin access" -j ACCEPT
COMMIT"#;
        let (rules, _) = parse_iptables_save(input, FirewallFamily::Ipv4);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].summary.action, FirewallAction::Drop);
        assert_eq!(
            rules[1].summary.source_address.as_deref(),
            Some("10.0.0.0/8")
        );
        assert_eq!(rules[1].summary.destination_ports, vec!["22"]);
        assert_eq!(rules[1].summary.comment.as_deref(), Some("admin access"));
        assert_eq!(rules[1].summary.action, FirewallAction::Accept);
    }

    #[test]
    fn parses_iptables_negation_without_losing_following_fields() {
        let input = r#"*filter
:INPUT ACCEPT [0:0]
-A INPUT ! -s 10.0.0.0/8 -p tcp --syn --dport 443 -j DROP
COMMIT"#;
        let (rules, partial) = parse_iptables_save(input, FirewallFamily::Ipv4);
        let rule = &rules[1];
        assert!(partial);
        assert_eq!(rule.matches[0].operator, "notEquals");
        assert_eq!(rule.summary.source_address.as_deref(), Some("10.0.0.0/8"));
        assert_eq!(rule.summary.destination_ports, vec!["443"]);
        assert_eq!(rule.summary.action, FirewallAction::Drop);
    }

    #[test]
    fn collection_preserves_per_source_rule_counts() {
        let detail = FirewallRuleDetail {
            summary: base_summary(
                FirewallRuleKind::Rule,
                FirewallEngine::Nftables,
                FirewallFamily::Inet,
                "filter".to_string(),
                "input".to_string(),
                1,
                FirewallAction::Accept,
            ),
            matches: Vec::new(),
            effects: Vec::new(),
            parse_status: FirewallRuleParseStatus::Complete,
        };
        let collection = collection_from_sources(&[SourceResult {
            source: FirewallSourceKind::Nftables,
            status: FirewallSourceStatus::Loaded,
            rules: vec![detail],
            parse_partial: false,
            too_large: false,
        }]);
        assert_eq!(collection.sources[0].rule_count, 1);
        assert_eq!(collection.coverage_percent, 100.0);
    }

    #[test]
    fn pagination_and_filters_use_the_same_snapshot() {
        let rules = (0..3)
            .map(|position| FirewallRuleDetail {
                summary: base_summary(
                    FirewallRuleKind::Rule,
                    FirewallEngine::Nftables,
                    FirewallFamily::Inet,
                    "filter".to_string(),
                    "input".to_string(),
                    position,
                    if position == 1 {
                        FirewallAction::Drop
                    } else {
                        FirewallAction::Accept
                    },
                ),
                matches: Vec::new(),
                effects: Vec::new(),
                parse_status: FirewallRuleParseStatus::Complete,
            })
            .collect();
        let snapshot = FirewallSnapshot {
            snapshot_id: Uuid::new_v4().to_string(),
            collected_at: Utc::now().to_rfc3339(),
            created_at: Instant::now(),
            collection: FirewallCollection {
                status: FirewallCollectionStatus::Complete,
                coverage_percent: 100.0,
                sources: Vec::new(),
                warnings: Vec::new(),
            },
            rules,
        };
        let page = page_from_snapshot(
            &snapshot,
            &FirewallRuleListQuery {
                action: Some(FirewallAction::Accept),
                page: Some(1),
                page_size: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(page.total, 2);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.snapshot_id, snapshot.snapshot_id);
    }

    #[test]
    fn stable_ids_ignore_rule_position() {
        let mut left = vec![FirewallRuleDetail {
            summary: base_summary(
                FirewallRuleKind::Rule,
                FirewallEngine::Iptables,
                FirewallFamily::Ipv4,
                "filter".to_string(),
                "INPUT".to_string(),
                1,
                FirewallAction::Accept,
            ),
            matches: Vec::new(),
            effects: Vec::new(),
            parse_status: FirewallRuleParseStatus::Complete,
        }];
        let mut right = left.clone();
        right[0].summary.position = 9;
        assign_rule_ids(&mut left).unwrap();
        assign_rule_ids(&mut right).unwrap();
        assert_eq!(left[0].summary.rule_id, right[0].summary.rule_id);
    }

    #[test]
    fn duplicate_rules_receive_distinct_stable_ids() {
        let detail = FirewallRuleDetail {
            summary: base_summary(
                FirewallRuleKind::Rule,
                FirewallEngine::Nftables,
                FirewallFamily::Inet,
                "filter".to_string(),
                "input".to_string(),
                1,
                FirewallAction::Accept,
            ),
            matches: Vec::new(),
            effects: Vec::new(),
            parse_status: FirewallRuleParseStatus::Complete,
        };
        let mut first_snapshot = vec![detail.clone(), detail.clone()];
        let mut second_snapshot = vec![detail.clone(), detail];

        assign_rule_ids(&mut first_snapshot).unwrap();
        assign_rule_ids(&mut second_snapshot).unwrap();

        assert_ne!(
            first_snapshot[0].summary.rule_id,
            first_snapshot[1].summary.rule_id
        );
        assert_eq!(
            first_snapshot[0].summary.rule_id,
            second_snapshot[0].summary.rule_id
        );
        assert_eq!(
            first_snapshot[1].summary.rule_id,
            second_snapshot[1].summary.rule_id
        );
    }
}
