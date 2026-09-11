//! 在线升级模型：发布版本、升级计划、目标与事件的数据库访问。

use crate::state::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 升级制品摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeAsset {
    pub name: String,
    pub download_url: String,
    pub size: Option<i64>,
    pub content_type: Option<String>,
    pub component: Option<String>,
    pub target_triple: Option<String>,
    pub sha256: Option<String>,
    pub signature_name: Option<String>,
    pub signature_status: Option<String>,
}

/// 已同步的升级发布版本。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeReleaseRecord {
    pub release_id: String,
    pub version: String,
    pub tag_name: String,
    pub channel: String,
    pub source: String,
    pub release_url: String,
    pub assets: String,
    pub checksum_status: String,
    pub signature_status: String,
    pub supported_suite_contract_versions: String,
    pub synced_at: String,
    pub published_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 升级计划记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UpgradePlanRecord {
    pub plan_id: String,
    pub target_version: String,
    pub component: String,
    pub scope: String,
    pub strategy: String,
    pub status: String,
    pub requested_by_user_id: i64,
    pub requested_by: String,
    pub client_ip: Option<String>,
    pub trace_id: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 升级目标记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeTargetRecord {
    pub target_id: String,
    pub plan_id: String,
    pub target_type: String,
    pub node_id: Option<String>,
    pub current_version: Option<String>,
    pub target_version: String,
    pub status: String,
    pub sub_status: Option<String>,
    pub retry_count: i64,
    pub error_detail: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 升级事件记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeEventRecord {
    pub event_id: String,
    pub plan_id: String,
    pub target_id: Option<String>,
    pub event_type: String,
    pub message: String,
    pub metadata: String,
    pub created_at: String,
}

/// 升级制品下载令牌。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeArtifactTokenRecord {
    pub token_id: String,
    pub token_hash: String,
    pub plan_id: String,
    pub target_id: String,
    pub node_id: Option<String>,
    pub version: String,
    pub component: String,
    pub target_triple: String,
    pub expires_at: String,
    pub used_at: Option<String>,
    pub created_at: String,
}

/// 新增或更新发布版本。
pub async fn upsert_release(pool: &DbPool, record: &UpgradeReleaseRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO upgrade_releases (
            release_id,
            version,
            tag_name,
            channel,
            source,
            release_url,
            assets,
            checksum_status,
            signature_status,
            supported_suite_contract_versions,
            synced_at,
            published_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(version) DO UPDATE SET
            tag_name = excluded.tag_name,
            channel = excluded.channel,
            source = excluded.source,
            release_url = excluded.release_url,
            assets = excluded.assets,
            checksum_status = excluded.checksum_status,
            signature_status = excluded.signature_status,
            supported_suite_contract_versions = excluded.supported_suite_contract_versions,
            synced_at = excluded.synced_at,
            published_at = excluded.published_at
        "#,
    )
    .bind(&record.release_id)
    .bind(&record.version)
    .bind(&record.tag_name)
    .bind(&record.channel)
    .bind(&record.source)
    .bind(&record.release_url)
    .bind(&record.assets)
    .bind(&record.checksum_status)
    .bind(&record.signature_status)
    .bind(&record.supported_suite_contract_versions)
    .bind(&record.synced_at)
    .bind(&record.published_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 查询发布版本列表。
pub async fn list_releases(pool: &DbPool) -> sqlx::Result<Vec<UpgradeReleaseRecord>> {
    sqlx::query_as::<_, UpgradeReleaseRecord>(
        r#"
        SELECT
            release_id,
            version,
            tag_name,
            channel,
            source,
            release_url,
            assets,
            checksum_status,
            signature_status,
            supported_suite_contract_versions,
            synced_at,
            published_at,
            created_at,
            updated_at
        FROM upgrade_releases
        ORDER BY published_at DESC, synced_at DESC
        "#,
    )
    .fetch_all(pool)
    .await
}

/// 根据版本查询发布版本。
pub async fn get_release_by_version(
    pool: &DbPool,
    version: &str,
) -> sqlx::Result<Option<UpgradeReleaseRecord>> {
    sqlx::query_as::<_, UpgradeReleaseRecord>(
        r#"
        SELECT
            release_id,
            version,
            tag_name,
            channel,
            source,
            release_url,
            assets,
            checksum_status,
            signature_status,
            supported_suite_contract_versions,
            synced_at,
            published_at,
            created_at,
            updated_at
        FROM upgrade_releases
        WHERE version = ?
        "#,
    )
    .bind(version)
    .fetch_optional(pool)
    .await
}

/// 更新发布版本声明支持的套件平台契约版本。
pub async fn update_release_suite_contract_versions(
    pool: &DbPool,
    version: &str,
    supported_versions: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE upgrade_releases SET supported_suite_contract_versions = ? WHERE version = ?",
    )
    .bind(supported_versions)
    .bind(version)
    .execute(pool)
    .await?;
    Ok(())
}

/// 根据版本删除同步的发布版本。
pub async fn delete_release_by_version(pool: &DbPool, version: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM upgrade_releases WHERE version = ?")
        .bind(version)
        .execute(pool)
        .await?;
    Ok(())
}

/// 写入升级计划。
pub async fn insert_plan(pool: &DbPool, record: &UpgradePlanRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO upgrade_plans (
            plan_id,
            target_version,
            component,
            scope,
            strategy,
            status,
            requested_by_user_id,
            requested_by,
            client_ip,
            trace_id,
            started_at,
            finished_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.plan_id)
    .bind(&record.target_version)
    .bind(&record.component)
    .bind(&record.scope)
    .bind(&record.strategy)
    .bind(&record.status)
    .bind(record.requested_by_user_id)
    .bind(&record.requested_by)
    .bind(&record.client_ip)
    .bind(&record.trace_id)
    .bind(&record.started_at)
    .bind(&record.finished_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 查询升级计划。
pub async fn get_plan(pool: &DbPool, plan_id: &str) -> sqlx::Result<Option<UpgradePlanRecord>> {
    sqlx::query_as::<_, UpgradePlanRecord>(
        r#"
        SELECT
            plan_id,
            target_version,
            component,
            scope,
            strategy,
            status,
            requested_by_user_id,
            requested_by,
            client_ip,
            trace_id,
            started_at,
            finished_at,
            created_at,
            updated_at
        FROM upgrade_plans
        WHERE plan_id = ?
        "#,
    )
    .bind(plan_id)
    .fetch_optional(pool)
    .await
}

/// 更新升级计划状态。
pub async fn update_plan_status(
    pool: &DbPool,
    plan_id: &str,
    status: &str,
    started_at: Option<&str>,
    finished_at: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        UPDATE upgrade_plans
        SET
            status = ?,
            started_at = COALESCE(?, started_at),
            finished_at = COALESCE(?, finished_at)
        WHERE plan_id = ?
        "#,
    )
    .bind(status)
    .bind(started_at)
    .bind(finished_at)
    .bind(plan_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// 写入升级目标。
pub async fn insert_target(pool: &DbPool, record: &UpgradeTargetRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO upgrade_targets (
            target_id,
            plan_id,
            target_type,
            node_id,
            current_version,
            target_version,
            status,
            sub_status,
            retry_count,
            error_detail,
            started_at,
            finished_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.target_id)
    .bind(&record.plan_id)
    .bind(&record.target_type)
    .bind(&record.node_id)
    .bind(&record.current_version)
    .bind(&record.target_version)
    .bind(&record.status)
    .bind(&record.sub_status)
    .bind(record.retry_count)
    .bind(&record.error_detail)
    .bind(&record.started_at)
    .bind(&record.finished_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 查询计划下的目标。
pub async fn list_targets_by_plan(
    pool: &DbPool,
    plan_id: &str,
) -> sqlx::Result<Vec<UpgradeTargetRecord>> {
    sqlx::query_as::<_, UpgradeTargetRecord>(
        r#"
        SELECT
            target_id,
            plan_id,
            target_type,
            node_id,
            current_version,
            target_version,
            status,
            sub_status,
            retry_count,
            error_detail,
            started_at,
            finished_at,
            created_at,
            updated_at
        FROM upgrade_targets
        WHERE plan_id = ?
        ORDER BY created_at ASC
        "#,
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
}

/// 查询可被调度器推进的升级目标。
pub async fn list_schedulable_targets(pool: &DbPool) -> sqlx::Result<Vec<UpgradeTargetRecord>> {
    sqlx::query_as::<_, UpgradeTargetRecord>(
        r#"
        SELECT
            t.target_id,
            t.plan_id,
            t.target_type,
            t.node_id,
            t.current_version,
            t.target_version,
            t.status,
            t.sub_status,
            t.retry_count,
            t.error_detail,
            t.started_at,
            t.finished_at,
            t.created_at,
            t.updated_at
        FROM upgrade_targets t
        INNER JOIN upgrade_plans p
          ON p.plan_id = t.plan_id
        WHERE p.status = 'running'
          AND t.status IN ('pending', 'deferred', 'running')
        ORDER BY t.created_at ASC
        LIMIT 50
        "#,
    )
    .fetch_all(pool)
    .await
}

/// 更新单个升级目标状态。
pub async fn update_target_status(
    pool: &DbPool,
    target_id: &str,
    status: &str,
    sub_status: Option<&str>,
    error_detail: Option<&str>,
    started_at: Option<&str>,
    finished_at: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        UPDATE upgrade_targets
        SET
            status = ?,
            sub_status = ?,
            error_detail = ?,
            started_at = COALESCE(?, started_at),
            finished_at = COALESCE(?, finished_at)
        WHERE target_id = ?
        "#,
    )
    .bind(status)
    .bind(sub_status)
    .bind(error_detail)
    .bind(started_at)
    .bind(finished_at)
    .bind(target_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 增加目标重试次数并记录错误。
pub async fn mark_target_failed(
    pool: &DbPool,
    target_id: &str,
    error_detail: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        UPDATE upgrade_targets
        SET
            status = 'failed',
            sub_status = NULL,
            retry_count = retry_count + 1,
            error_detail = ?,
            finished_at = ?
        WHERE target_id = ?
        "#,
    )
    .bind(error_detail)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(target_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 统计计划内各目标状态数量。
pub async fn count_targets_by_plan_status(
    pool: &DbPool,
    plan_id: &str,
    status: &str,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM upgrade_targets
        WHERE plan_id = ?
          AND status = ?
        "#,
    )
    .bind(plan_id)
    .bind(status)
    .fetch_one(pool)
    .await
}

/// 统计计划目标总数。
pub async fn count_targets_by_plan(pool: &DbPool, plan_id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM upgrade_targets
        WHERE plan_id = ?
        "#,
    )
    .bind(plan_id)
    .fetch_one(pool)
    .await
}

/// 批量更新计划内未开始目标状态。
pub async fn update_targets_status(
    pool: &DbPool,
    plan_id: &str,
    from_status: &[&str],
    to_status: &str,
) -> sqlx::Result<u64> {
    let mut affected = 0_u64;
    for status in from_status {
        let result = sqlx::query(
            r#"
            UPDATE upgrade_targets
            SET status = ?
            WHERE plan_id = ?
              AND status = ?
            "#,
        )
        .bind(to_status)
        .bind(plan_id)
        .bind(status)
        .execute(pool)
        .await?;
        affected += result.rows_affected();
    }
    Ok(affected)
}

/// 写入升级事件。
pub async fn insert_event(pool: &DbPool, record: &UpgradeEventRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO upgrade_events (
            event_id,
            plan_id,
            target_id,
            event_type,
            message,
            metadata
        ) VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.event_id)
    .bind(&record.plan_id)
    .bind(&record.target_id)
    .bind(&record.event_type)
    .bind(&record.message)
    .bind(&record.metadata)
    .execute(pool)
    .await?;

    Ok(())
}

/// 查询计划事件。
pub async fn list_events_by_plan(
    pool: &DbPool,
    plan_id: &str,
) -> sqlx::Result<Vec<UpgradeEventRecord>> {
    sqlx::query_as::<_, UpgradeEventRecord>(
        r#"
        SELECT
            event_id,
            plan_id,
            target_id,
            event_type,
            message,
            metadata,
            created_at
        FROM upgrade_events
        WHERE plan_id = ?
        ORDER BY created_at ASC
        "#,
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
}

/// 写入升级制品下载令牌。
pub async fn insert_artifact_token(
    pool: &DbPool,
    record: &UpgradeArtifactTokenRecord,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO upgrade_artifact_tokens (
            token_id,
            token_hash,
            plan_id,
            target_id,
            node_id,
            version,
            component,
            target_triple,
            expires_at,
            used_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.token_id)
    .bind(&record.token_hash)
    .bind(&record.plan_id)
    .bind(&record.target_id)
    .bind(&record.node_id)
    .bind(&record.version)
    .bind(&record.component)
    .bind(&record.target_triple)
    .bind(&record.expires_at)
    .bind(&record.used_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 通过 token hash 查询制品下载令牌。
pub async fn get_artifact_token_by_hash(
    pool: &DbPool,
    token_hash: &str,
) -> sqlx::Result<Option<UpgradeArtifactTokenRecord>> {
    sqlx::query_as::<_, UpgradeArtifactTokenRecord>(
        r#"
        SELECT
            token_id,
            token_hash,
            plan_id,
            target_id,
            node_id,
            version,
            component,
            target_triple,
            expires_at,
            used_at,
            created_at
        FROM upgrade_artifact_tokens
        WHERE token_hash = ?
        "#,
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
}
