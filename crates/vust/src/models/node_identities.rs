//! 节点身份模型：`node_identities` 表的数据结构与基础写入方法。

use crate::state::DbPool;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 节点身份记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NodeIdentityRecord {
    pub identity_id: String,
    pub node_id: String,
    pub agent_id: String,
    pub certificate_serial_number: Option<String>,
    pub certificate_fingerprint: String,
    pub certificate_status: String,
    pub public_key_algorithm: String,
    pub certificate_issued_at: Option<String>,
    pub certificate_expires_at: Option<String>,
    pub rotated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 写入节点身份记录。
pub async fn insert_node_identity(pool: &DbPool, record: &NodeIdentityRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO node_identities (
            identity_id,
            node_id,
            agent_id,
            certificate_serial_number,
            certificate_fingerprint,
            certificate_status,
            public_key_algorithm,
            certificate_issued_at,
            certificate_expires_at,
            rotated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.identity_id)
    .bind(&record.node_id)
    .bind(&record.agent_id)
    .bind(&record.certificate_serial_number)
    .bind(&record.certificate_fingerprint)
    .bind(&record.certificate_status)
    .bind(&record.public_key_algorithm)
    .bind(&record.certificate_issued_at)
    .bind(&record.certificate_expires_at)
    .bind(&record.rotated_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 通过 `agent_id` 查询节点身份记录。
pub async fn get_identity_by_agent_id(
    pool: &DbPool,
    agent_id: &str,
) -> sqlx::Result<Option<NodeIdentityRecord>> {
    sqlx::query_as::<_, NodeIdentityRecord>(
        r#"
        SELECT
            identity_id,
            node_id,
            agent_id,
            certificate_serial_number,
            certificate_fingerprint,
            certificate_status,
            public_key_algorithm,
            certificate_issued_at,
            certificate_expires_at,
            rotated_at,
            created_at,
            updated_at
        FROM node_identities
        WHERE agent_id = ?
        LIMIT 1
        "#,
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
}

/// 写入或更新节点身份记录。
pub async fn upsert_node_identity(pool: &DbPool, record: &NodeIdentityRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO node_identities (
            identity_id,
            node_id,
            agent_id,
            certificate_serial_number,
            certificate_fingerprint,
            certificate_status,
            public_key_algorithm,
            certificate_issued_at,
            certificate_expires_at,
            rotated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(agent_id) DO UPDATE SET
            certificate_serial_number = excluded.certificate_serial_number,
            certificate_fingerprint = excluded.certificate_fingerprint,
            certificate_status = excluded.certificate_status,
            public_key_algorithm = excluded.public_key_algorithm,
            certificate_issued_at = excluded.certificate_issued_at,
            certificate_expires_at = excluded.certificate_expires_at,
            rotated_at = excluded.rotated_at,
            updated_at = ?
        "#,
    )
    .bind(&record.identity_id)
    .bind(&record.node_id)
    .bind(&record.agent_id)
    .bind(&record.certificate_serial_number)
    .bind(&record.certificate_fingerprint)
    .bind(&record.certificate_status)
    .bind(&record.public_key_algorithm)
    .bind(&record.certificate_issued_at)
    .bind(&record.certificate_expires_at)
    .bind(&record.rotated_at)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;

    Ok(())
}
