//! 节点库存模型：`nodes` 表的数据结构与基础仓储方法。

use crate::state::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 节点生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Draft,
    Deploying,
    DeployFailed,
    AwaitingRegistration,
    Registered,
    Online,
    Degraded,
    Offline,
    Unreachable,
    Conflict,
    Retired,
}

impl NodeStatus {
    /// 返回数据库持久化使用的状态字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Deploying => "deploying",
            Self::DeployFailed => "deploy_failed",
            Self::AwaitingRegistration => "awaiting_registration",
            Self::Registered => "registered",
            Self::Online => "online",
            Self::Degraded => "degraded",
            Self::Offline => "offline",
            Self::Unreachable => "unreachable",
            Self::Conflict => "conflict",
            Self::Retired => "retired",
        }
    }

    /// 从数据库状态字符串解析节点状态。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "deploying" => Some(Self::Deploying),
            "deploy_failed" => Some(Self::DeployFailed),
            "awaiting_registration" => Some(Self::AwaitingRegistration),
            "registered" => Some(Self::Registered),
            "online" => Some(Self::Online),
            "degraded" => Some(Self::Degraded),
            "offline" => Some(Self::Offline),
            "unreachable" => Some(Self::Unreachable),
            "conflict" => Some(Self::Conflict),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }
}

/// 节点主记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NodeRecord {
    pub node_id: String,
    pub tenant_id: Option<String>,
    pub name: String,
    pub normalized_name: String,
    pub group_name: String,
    pub labels: String,
    pub description: Option<String>,
    pub status: String,
    pub desired_role: Option<String>,
    pub schedulable: i64,
    pub metadata: String,
    pub registered_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub last_probe_at: Option<String>,
    pub last_deploy_at: Option<String>,
    pub retired_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 创建节点时使用的基础载荷。
#[derive(Debug, Clone)]
pub struct NewNodeRecord {
    pub node_id: String,
    pub tenant_id: Option<String>,
    pub name: String,
    pub normalized_name: String,
    pub group_name: String,
    pub labels: String,
    pub description: Option<String>,
    pub desired_role: Option<String>,
    pub schedulable: bool,
    pub metadata: String,
}

/// 查询所有节点主记录。
pub async fn list_nodes(pool: &DbPool) -> sqlx::Result<Vec<NodeRecord>> {
    sqlx::query_as::<_, NodeRecord>(
        r#"
        SELECT
            node_id,
            tenant_id,
            name,
            normalized_name,
            group_name,
            labels,
            description,
            status,
            desired_role,
            schedulable,
            metadata,
            registered_at,
            last_seen_at,
            last_probe_at,
            last_deploy_at,
            retired_at,
            created_at,
            updated_at
        FROM nodes
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await
}

/// 按 `node_id` 查询单个节点主记录。
pub async fn get_node_by_id(pool: &DbPool, node_id: &str) -> sqlx::Result<Option<NodeRecord>> {
    sqlx::query_as::<_, NodeRecord>(
        r#"
        SELECT
            node_id,
            tenant_id,
            name,
            normalized_name,
            group_name,
            labels,
            description,
            status,
            desired_role,
            schedulable,
            metadata,
            registered_at,
            last_seen_at,
            last_probe_at,
            last_deploy_at,
            retired_at,
            created_at,
            updated_at
        FROM nodes
        WHERE node_id = ?
        "#,
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
}

/// 创建节点主记录。
pub async fn insert_node(pool: &DbPool, payload: &NewNodeRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO nodes (
            node_id,
            tenant_id,
            name,
            normalized_name,
            group_name,
            labels,
            description,
            status,
            desired_role,
            schedulable,
            metadata
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&payload.node_id)
    .bind(&payload.tenant_id)
    .bind(&payload.name)
    .bind(&payload.normalized_name)
    .bind(&payload.group_name)
    .bind(&payload.labels)
    .bind(&payload.description)
    .bind(NodeStatus::Draft.as_str())
    .bind(&payload.desired_role)
    .bind(if payload.schedulable { 1_i64 } else { 0_i64 })
    .bind(&payload.metadata)
    .execute(pool)
    .await?;

    Ok(())
}

/// 更新节点主状态。
pub async fn update_node_status(
    pool: &DbPool,
    node_id: &str,
    status: NodeStatus,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        UPDATE nodes
        SET status = ?
        WHERE node_id = ?
        "#,
    )
    .bind(status.as_str())
    .bind(node_id)
    .execute(pool)
    .await?;

    Ok(())
}
