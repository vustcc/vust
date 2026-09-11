//! 防火墙规则观察领域共享契约。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 防火墙规则类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallRuleKind {
    Rule,
    Policy,
}

/// 实际执行规则的内核后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallEngine {
    Nftables,
    Iptables,
}

/// 防火墙规则协议族。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallFamily {
    Inet,
    Ipv4,
    Ipv6,
    Bridge,
    Arp,
    Netdev,
    Unknown,
}

/// 规则的主要终结动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallAction {
    Accept,
    Drop,
    Reject,
    Return,
    Jump,
    Goto,
    Log,
    Masquerade,
    Dnat,
    Snat,
    Redirect,
    Queue,
    Continue,
    Other,
}

/// 单个采集来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallSourceKind {
    Nftables,
    IptablesV4,
    IptablesV6,
}

/// 单个采集来源的结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallSourceStatus {
    Loaded,
    Empty,
    Unavailable,
    Failed,
    Timeout,
    Skipped,
}

/// 整体采集质量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallCollectionStatus {
    Complete,
    Partial,
}

/// 可本地化的采集告警码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FirewallWarningCode {
    SourceUnavailable,
    SourceFailed,
    SourceTimeout,
    SourceParsePartial,
    CompatibilityBackendSkipped,
}

/// 规则解析完整度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FirewallRuleParseStatus {
    Complete,
    Partial,
}

/// 防火墙规则的可用能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRuleCapabilities {
    pub can_view_detail: bool,
}

/// 防火墙采集来源状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FirewallCollectionSource {
    #[ts(inline)]
    pub source: FirewallSourceKind,
    #[ts(inline)]
    pub status: FirewallSourceStatus,
    pub rule_count: usize,
}

/// 防火墙采集告警。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FirewallCollectionWarning {
    #[ts(inline)]
    pub code: FirewallWarningCode,
    #[ts(inline)]
    pub source: FirewallSourceKind,
}

/// 防火墙规则采集质量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FirewallCollection {
    #[ts(inline)]
    pub status: FirewallCollectionStatus,
    pub coverage_percent: f64,
    #[ts(inline)]
    pub sources: Vec<FirewallCollectionSource>,
    #[ts(inline)]
    pub warnings: Vec<FirewallCollectionWarning>,
}

/// 防火墙规则列表摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct FirewallRuleSummary {
    pub rule_id: String,
    #[ts(inline)]
    pub kind: FirewallRuleKind,
    #[ts(inline)]
    pub engine: FirewallEngine,
    #[ts(inline)]
    pub family: FirewallFamily,
    pub table: String,
    pub chain: String,
    pub position: usize,
    #[ts(inline)]
    pub action: FirewallAction,
    pub action_target: Option<String>,
    pub protocol: Option<String>,
    pub source_address: Option<String>,
    pub destination_address: Option<String>,
    pub source_ports: Vec<String>,
    pub destination_ports: Vec<String>,
    pub input_interface: Option<String>,
    pub output_interface: Option<String>,
    pub comment: Option<String>,
    #[ts(inline)]
    pub capabilities: FirewallRuleCapabilities,
}

/// 规则匹配条件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRuleMatch {
    pub field: String,
    pub operator: String,
    pub value: String,
}

/// 规则执行效果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct FirewallRuleEffect {
    #[ts(inline)]
    pub action: FirewallAction,
    pub target: Option<String>,
}

/// 防火墙规则详情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "firewall/")]
pub struct FirewallRuleDetail {
    #[ts(inline)]
    pub summary: FirewallRuleSummary,
    #[ts(inline)]
    pub matches: Vec<FirewallRuleMatch>,
    #[ts(inline)]
    pub effects: Vec<FirewallRuleEffect>,
    #[ts(inline)]
    pub parse_status: FirewallRuleParseStatus,
}

/// Master 返回的可信 Node 引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FirewallNodeReference {
    pub node_id: String,
    pub node_name: String,
}

/// Agent 内部使用的防火墙规则分页数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRulePageData {
    pub snapshot_id: String,
    pub collected_at: String,
    pub collection: FirewallCollection,
    pub entries: Vec<FirewallRuleSummary>,
    pub page: usize,
    pub page_size: usize,
    pub available_total: usize,
    pub total: usize,
    pub capabilities: FirewallRuleCapabilities,
}

/// Master 对前端公开的防火墙规则分页数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "firewall/")]
pub struct FirewallRuleListPage {
    #[ts(inline)]
    pub node: FirewallNodeReference,
    pub snapshot_id: String,
    pub collected_at: String,
    #[ts(inline)]
    pub collection: FirewallCollection,
    #[ts(inline)]
    pub entries: Vec<FirewallRuleSummary>,
    pub page: usize,
    pub page_size: usize,
    pub available_total: usize,
    pub total: usize,
    #[ts(inline)]
    pub capabilities: FirewallRuleCapabilities,
}

/// 防火墙列表排序字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FirewallRuleSortBy {
    RuleOrder,
    Action,
    Protocol,
    Source,
    Destination,
}

/// 防火墙列表排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FirewallSortOrder {
    Asc,
    Desc,
}

/// 防火墙规则列表查询。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRuleListQuery {
    pub snapshot_id: Option<String>,
    pub query: Option<String>,
    pub engine: Option<FirewallEngine>,
    pub family: Option<FirewallFamily>,
    pub action: Option<FirewallAction>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub sort_by: Option<FirewallRuleSortBy>,
    pub sort_order: Option<FirewallSortOrder>,
}

/// 防火墙规则详情查询。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRuleDetailQuery {
    pub snapshot_id: String,
}
