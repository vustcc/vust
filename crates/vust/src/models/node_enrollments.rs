//! 节点纳管模型：`node_enrollments` 表的数据结构与基础写入方法。

use crate::state::DbPool;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Executor, FromRow, Sqlite};

/// 节点纳管记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NodeEnrollmentRecord {
    pub enrollment_id: String,
    pub node_id: String,
    pub token_hash: String,
    pub token_status: String,
    pub expires_at: String,
    pub first_used_at: Option<String>,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 新增节点纳管记录。
pub async fn insert_node_enrollment<'e, E>(
    executor: E,
    record: &NodeEnrollmentRecord,
) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        r#"
        INSERT INTO node_enrollments (
            enrollment_id,
            node_id,
            token_hash,
            token_status,
            expires_at,
            first_used_at,
            last_used_at,
            revoked_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.enrollment_id)
    .bind(&record.node_id)
    .bind(&record.token_hash)
    .bind(&record.token_status)
    .bind(&record.expires_at)
    .bind(&record.first_used_at)
    .bind(&record.last_used_at)
    .bind(&record.revoked_at)
    .execute(executor)
    .await?;

    Ok(())
}

/// 通过 token 哈希读取纳管记录。
pub async fn get_enrollment_by_token_hash(
    pool: &DbPool,
    token_hash: &str,
) -> sqlx::Result<Option<NodeEnrollmentRecord>> {
    sqlx::query_as::<_, NodeEnrollmentRecord>(
        r#"
        SELECT
            enrollment_id,
            node_id,
            token_hash,
            token_status,
            expires_at,
            first_used_at,
            last_used_at,
            revoked_at,
            created_at,
            updated_at
        FROM node_enrollments
        WHERE token_hash = ?
        LIMIT 1
        "#,
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
}

/// 标记纳管 token 已使用。
pub async fn mark_enrollment_used(pool: &DbPool, enrollment_id: &str) -> sqlx::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE node_enrollments
        SET
            token_status = 'used',
            first_used_at = COALESCE(first_used_at, ?),
            last_used_at = ?
        WHERE enrollment_id = ?
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(enrollment_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// 撤销尚未被 Agent 使用的纳管令牌。
pub async fn revoke_issued_enrollment(pool: &DbPool, enrollment_id: &str) -> sqlx::Result<bool> {
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        r#"
        UPDATE node_enrollments
        SET
            token_status = 'revoked',
            revoked_at = ?,
            updated_at = ?
        WHERE enrollment_id = ?
          AND token_status = 'issued'
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(enrollment_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() == 1)
}
