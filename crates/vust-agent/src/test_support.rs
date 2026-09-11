//! 测试支持：测试夹具与辅助方法。

use crate::services::{
    process_manager::ProcessManagerRuntime, system_monitoring::SystemMonitoringRuntime, websocket,
};
use crate::state::AppState;
use crate::state::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::path::Path;
use tokio::sync::RwLock;
use vust_contracts::types::DockerServiceStatus;

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

/// 构建带测试数据库与默认状态的应用实例。
pub async fn setup_test_state() -> AppState {
    let metadata_db = setup_test_db().await;
    let system_monitoring = SystemMonitoringRuntime::load(&metadata_db).await.unwrap();
    AppState {
        server_name: "test-agent".to_string(),
        docker: RwLock::new(None),
        docker_status: RwLock::new(DockerServiceStatus::NotInstalled),
        system_monitoring: std::sync::Arc::new(system_monitoring),
        process_manager: std::sync::Arc::new(ProcessManagerRuntime::new()),
        controller_runtime: std::sync::Arc::new(
            crate::services::controller_runtime::ControllerRuntime::new(),
        ),
        metadata_db,
        websocket_sender: websocket::create_channel(),
        running_task_ids: tokio::sync::Mutex::new(std::collections::HashSet::new()),
    }
}
