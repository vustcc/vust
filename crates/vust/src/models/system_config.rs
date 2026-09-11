//! 系统配置模型：读写单行全局安全配置。

use crate::state::DbPool;

/// 全局系统配置固定行 ID。
const SYSTEM_CONFIG_ID: i64 = 1;

/// 确保系统配置单行存在。
pub async fn ensure_system_config(pool: &DbPool) -> sqlx::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO system_config (id) VALUES (?1)")
        .bind(SYSTEM_CONFIG_ID)
        .execute(pool)
        .await?;
    Ok(())
}

/// 更新安全入口与密码复杂度配置。
pub async fn update_security_settings(
    pool: &DbPool,
    safe_entry: &str,
    password_complexity: bool,
) -> sqlx::Result<()> {
    ensure_system_config(pool).await?;
    sqlx::query(
        r#"
        UPDATE system_config
           SET safe_entry = ?1,
               password_complexity = ?2
         WHERE id = ?3
        "#,
    )
    .bind(safe_entry)
    .bind(if password_complexity { 1 } else { 0 })
    .bind(SYSTEM_CONFIG_ID)
    .execute(pool)
    .await?;
    Ok(())
}

/// 更新安全入口。
pub async fn update_safe_entry(pool: &DbPool, safe_entry: &str) -> sqlx::Result<()> {
    ensure_system_config(pool).await?;
    sqlx::query(
        r#"
        UPDATE system_config
           SET safe_entry = ?1
         WHERE id = ?2
        "#,
    )
    .bind(safe_entry)
    .bind(SYSTEM_CONFIG_ID)
    .execute(pool)
    .await?;
    Ok(())
}

/// 更新密码复杂度开关。
pub async fn update_password_complexity(pool: &DbPool, enabled: bool) -> sqlx::Result<()> {
    ensure_system_config(pool).await?;
    sqlx::query(
        r#"
        UPDATE system_config
           SET password_complexity = ?1
         WHERE id = ?2
        "#,
    )
    .bind(if enabled { 1 } else { 0 })
    .bind(SYSTEM_CONFIG_ID)
    .execute(pool)
    .await?;
    Ok(())
}

/// 读取安全入口，空字符串按关闭处理。
pub async fn get_safe_entry(pool: &DbPool) -> sqlx::Result<Option<String>> {
    ensure_system_config(pool).await?;
    let value: String = sqlx::query_scalar("SELECT safe_entry FROM system_config WHERE id = ?1")
        .bind(SYSTEM_CONFIG_ID)
        .fetch_one(pool)
        .await?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

/// 读取原始安全入口值。
pub async fn get_safe_entry_value(pool: &DbPool) -> sqlx::Result<String> {
    ensure_system_config(pool).await?;
    sqlx::query_scalar("SELECT safe_entry FROM system_config WHERE id = ?1")
        .bind(SYSTEM_CONFIG_ID)
        .fetch_one(pool)
        .await
}

/// 读取密码复杂度开关。
pub async fn password_complexity_enabled(pool: &DbPool) -> sqlx::Result<bool> {
    ensure_system_config(pool).await?;
    let value: i64 =
        sqlx::query_scalar("SELECT password_complexity FROM system_config WHERE id = ?1")
            .bind(SYSTEM_CONFIG_ID)
            .fetch_one(pool)
            .await?;
    Ok(value != 0)
}
