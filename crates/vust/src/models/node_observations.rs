//! 节点观测模型：`node_observations` 表的数据结构与基础写入方法。

use crate::state::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 节点观测记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NodeObservationRecord {
    pub observation_id: String,
    pub node_id: String,
    pub session_id: Option<String>,
    pub source: String,
    pub system_snapshot: Option<String>,
    pub docker_snapshot: Option<String>,
    pub probe_result: Option<String>,
    pub observed_at: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 写入节点观测记录。
pub async fn insert_node_observation(
    pool: &DbPool,
    record: &NodeObservationRecord,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO node_observations (
            observation_id,
            node_id,
            session_id,
            source,
            system_snapshot,
            docker_snapshot,
            probe_result,
            observed_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.observation_id)
    .bind(&record.node_id)
    .bind(&record.session_id)
    .bind(&record.source)
    .bind(&record.system_snapshot)
    .bind(&record.docker_snapshot)
    .bind(&record.probe_result)
    .bind(&record.observed_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 按 `node_id` 读取最近一条节点观测记录。
pub async fn get_latest_node_observation_by_node_id(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<Option<NodeObservationRecord>> {
    sqlx::query_as::<_, NodeObservationRecord>(
        r#"
        SELECT
            observation_id,
            node_id,
            session_id,
            source,
            system_snapshot,
            docker_snapshot,
            probe_result,
            observed_at,
            created_at,
            updated_at
        FROM node_observations
        WHERE node_id = ?
        ORDER BY observed_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
}

/// 按 `node_id` 读取最近若干条节点观测记录。
pub async fn list_node_observations_by_node_id(
    pool: &DbPool,
    node_id: &str,
    limit: i64,
) -> sqlx::Result<Vec<NodeObservationRecord>> {
    sqlx::query_as::<_, NodeObservationRecord>(
        r#"
        SELECT
            observation_id,
            node_id,
            session_id,
            source,
            system_snapshot,
            docker_snapshot,
            probe_result,
            observed_at,
            created_at,
            updated_at
        FROM node_observations
        WHERE node_id = ?
        ORDER BY observed_at DESC, created_at DESC
        LIMIT ?
        "#,
    )
    .bind(node_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}
