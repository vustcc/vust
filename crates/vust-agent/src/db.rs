//! 数据库：连接池初始化与迁移执行。

use crate::config;
use crate::state::DbPool;
use anyhow::{Context, Result};
use sqlx::migrate::MigrateError;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::path::Path;
use std::str::FromStr;

/// 建立并初始化 `agent` 服务的数据库连接池。
///
/// 此函数在 `agent` 服务启动时调用，负责：
/// 1. 根据运行环境（`debug` 或 `release`）确定 `agent.db` 数据库文件的路径。
/// 2. 确保数据库文件所在的目录存在。
/// 3. 使用一组优化的 SQLite 连接选项 (`sqlite_options`) 创建一个 `SqlitePool`。
/// 4. 在连接池创建后，自动运行位于 `crates/agent/migrations` 目录下的数据库迁移脚本。
///
/// # 返回
/// 一个初始化完成并已运行迁移的 `DbPool` (`sqlx::Pool<Sqlite>`)。
pub async fn establish_connection() -> Result<DbPool> {
    let db_path = config::data_dir().join("agent.db");

    if let Some(parent) = Path::new(&db_path).parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create Agent database directory '{}'.",
                parent.display()
            )
        })?;
    }

    let database_url = format!("sqlite:{}", db_path.to_string_lossy());

    let pool = SqlitePoolOptions::new()
        .max_connections(1) // SQLite 在 WAL 模式下通常单连接性能最佳
        .connect_with(sqlite_options(&database_url)?)
        .await
        .with_context(|| format!("Failed to open Agent database at '{}'.", db_path.display()))?;

    tracing::info!("Running agent database migrations...");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|error| anyhow::anyhow!(migration_failure_message(&db_path, &error)))?;
    tracing::info!("Agent database migrations completed...");

    Ok(pool)
}

/// 将迁移错误转换为可操作的启动提示。
fn migration_failure_message(path: &Path, error: &MigrateError) -> String {
    let database = path.display();
    match error {
        MigrateError::VersionMissing(version) => format!(
            "Agent database migration history is incompatible with this build: database '{database}' contains removed migration version {version}. Back up the database, remove it, and restart VUST Agent to create a clean database."
        ),
        MigrateError::VersionMismatch(version) => format!(
            "Agent database migration history is incompatible with this build: migration version {version} in database '{database}' differs from the current migration file. Back up the database, remove it, and restart VUST Agent to create a clean database."
        ),
        _ => format!(
            "Failed to migrate Agent database '{database}': {error}. Check database permissions, available disk space, and migration files, then restart VUST Agent."
        ),
    }
}

/// 为 SQLite 连接提供一组优化的默认选项。
///
/// 这组选项旨在提高并发性能和数据安全性，适用于服务端应用。
///
/// # 配置选项
/// - **`journal_mode(SqliteJournalMode::Wal)`**: 启用预写日志（Write-Ahead Logging）模式。
///   这是 SQLite 在高并发读写场景下的首选模式，能显著提高性能。
/// - **`synchronous(SqliteSynchronous::Normal)`**: 设置同步等级为 "Normal"。
///   在 WAL 模式下，这是性能和安全性之间的良好平衡。
/// - **`foreign_keys(true)`**: 强制执行外键约束，保证数据完整性。
/// - **`auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Incremental)`**: 启用增量 vacuum，
///   有助于在删除数据后自动回收数据库空间。
/// - **`busy_timeout(std::time::Duration::from_secs(5))`**: 当数据库被锁定时，
///   设置一个 5 秒的等待超时，以减少因并发写入导致的 "database is locked" 错误。
///
/// # 参数
/// - `path`: 数据库文件的路径，用于构建连接选项。
///
/// # 返回
/// 一个配置好的 `SqliteConnectOptions` 实例。
fn sqlite_options(path: &str) -> Result<SqliteConnectOptions> {
    Ok(SqliteConnectOptions::from_str(&format!("sqlite:{path}"))
        .with_context(|| format!("Invalid Agent SQLite database path '{path}'."))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Incremental)
        .busy_timeout(std::time::Duration::from_secs(5)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_failure_explains_removed_version() {
        let message = migration_failure_message(
            Path::new("/var/lib/vust/agent.db"),
            &MigrateError::VersionMissing(10),
        );

        assert!(message.contains("removed migration version 10"));
        assert!(message.contains("Back up the database"));
        assert!(message.contains("/var/lib/vust/agent.db"));
    }

    #[test]
    fn migration_failure_explains_changed_version() {
        let message = migration_failure_message(
            Path::new("/var/lib/vust/agent.db"),
            &MigrateError::VersionMismatch(2),
        );

        assert!(message.contains("migration version 2"));
        assert!(message.contains("differs from the current migration file"));
    }
}
