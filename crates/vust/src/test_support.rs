//! 测试支持：测试夹具与辅助方法。

use crate::state::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::path::Path;

/// 创建内存数据库并执行迁移，供测试使用。
pub async fn setup_test_db() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let migration_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    sqlx::migrate::Migrator::new(migration_path)
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    pool
}

#[cfg(test)]
mod tests {
    use super::setup_test_db;
    use sqlx::Row;

    /// 校验收敛迁移后的 schema：旧 `agents` 表必须不存在，且新表正常创建。
    #[tokio::test]
    async fn test_converged_migrations_schema() {
        let pool = setup_test_db().await;
        let legacy_exists: Option<String> = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'agents' LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .map(|row| row.get::<String, _>("name"));
        assert!(
            legacy_exists.is_none(),
            "legacy agents table should not exist in converged schema"
        );

        let nodes_exists: Option<String> = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'nodes' LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .map(|row| row.get::<String, _>("name"));
        assert_eq!(nodes_exists.as_deref(), Some("nodes"));

        let sessions_exists: Option<String> = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'node_sessions' LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .map(|row| row.get::<String, _>("name"));
        assert_eq!(sessions_exists.as_deref(), Some("node_sessions"));

        let auth_sessions_exists: Option<String> = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'auth_sessions' LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .map(|row| row.get::<String, _>("name"));
        assert_eq!(auth_sessions_exists.as_deref(), Some("auth_sessions"));
    }
}
