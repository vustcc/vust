//! 通用配置服务：集中读写 `agent_settings` 键值配置。

use crate::state::DbPool;

/// 读取字符串配置，不存在时返回默认值并自动落库。
pub async fn get_string_or_default(
    pool: &DbPool,
    key: &str,
    default: &str,
) -> anyhow::Result<String> {
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO agent_settings (key, value, updated_at)
        VALUES (?, ?, strftime('%s', 'now'))
        "#,
    )
    .bind(key)
    .bind(default)
    .execute(pool)
    .await?;

    let value = sqlx::query_scalar::<_, String>(
        r#"
        SELECT value
        FROM agent_settings
        WHERE key = ?
        "#,
    )
    .bind(key)
    .fetch_one(pool)
    .await?;

    Ok(value)
}

/// 写入字符串配置，存在则覆盖并更新时间。
pub async fn set_string(pool: &DbPool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO agent_settings (key, value, updated_at)
        VALUES (?, ?, strftime('%s', 'now'))
        ON CONFLICT(key) DO UPDATE SET
            value = excluded.value,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// 读取布尔配置（字符串 `true/false`）。
pub async fn get_bool_or_default(pool: &DbPool, key: &str, default: bool) -> anyhow::Result<bool> {
    let default_text = if default { "true" } else { "false" };
    let value = get_string_or_default(pool, key, default_text).await?;
    Ok(value == "true")
}

/// 写入布尔配置（字符串 `true/false`）。
pub async fn set_bool(pool: &DbPool, key: &str, value: bool) -> anyhow::Result<()> {
    let text = if value { "true" } else { "false" };
    set_string(pool, key, text).await
}
