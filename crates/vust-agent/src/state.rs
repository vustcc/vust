//! 应用状态：共享依赖（数据库、Docker 客户端等）聚合。

use crate::services::controller_runtime::ControllerRuntime;
use crate::services::process_manager::ProcessManagerRuntime;
use crate::services::system_monitoring::SystemMonitoringRuntime;
use crate::services::websocket;
use crate::types::{ApiError, ApiResult};
use bollard::Docker;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
pub use vust_contracts::DbPool;
use vust_contracts::types::DockerServiceStatus;

/// `agent` 服务的共享状态，通过 `axum::extract::State` 在所有 `handler` 之间共享。
///
/// `AppState` 实例在服务启动时创建，并使用 `Arc` 进行包裹，以实现在多线程环境下的安全共享。
///
/// # 字段
/// - `server_name`: 服务器的名称，用于日志和标识。
/// - `docker`: 一个可选的 `bollard::Docker` 客户端实例，用于与本地 Docker 守护进程交互。
/// - `metadata_db`: `agent` 服务自身的数据库连接池，用于存储其内部数据（如日志）。
/// - `websocket_sender`: 一个 `tokio::sync::broadcast::Sender`，用于向所有连接的
///   WebSocket 客户端广播通用事件。虽然目前日志订阅已改为独立任务，但此广播通道
///   仍可用于未来的全局通知功能。
pub struct AppState {
    pub server_name: String,
    pub docker: RwLock<Option<Arc<Docker>>>,
    pub docker_status: RwLock<DockerServiceStatus>,
    pub system_monitoring: Arc<SystemMonitoringRuntime>,
    pub process_manager: Arc<ProcessManagerRuntime>,
    pub controller_runtime: Arc<ControllerRuntime>,
    pub metadata_db: DbPool,
    pub websocket_sender: tokio::sync::broadcast::Sender<websocket::WebsocketEvent>,
    pub running_task_ids: tokio::sync::Mutex<std::collections::HashSet<i64>>,
}

impl AppState {
    /// 探测 Docker 守护进程并返回可用客户端与服务状态。
    pub async fn init_docker_state() -> (Option<Arc<Docker>>, DockerServiceStatus) {
        let socket_path = Path::new("/var/run/docker.sock");
        let docker_bin_exists = Path::new("/usr/bin/docker").exists()
            || Path::new("/usr/local/bin/docker").exists()
            || Path::new("/bin/docker").exists();
        if !socket_path.exists() {
            return (
                None,
                if docker_bin_exists {
                    DockerServiceStatus::NotRunning
                } else {
                    DockerServiceStatus::NotInstalled
                },
            );
        }
        // Bollard 0.21 默认使用 API 1.53。必须先协商版本，否则 Docker 28.x
        // 和最高仅支持 API 1.52 的 Docker 29.0/29.1 会在初始化阶段被误判为不可用。
        match Docker::connect_with_socket_defaults() {
            Ok(client) => match client.negotiate_version().await {
                Ok(client) => match client.info().await {
                    Ok(_) => (Some(Arc::new(client)), DockerServiceStatus::Available),
                    Err(err) => {
                        let status = map_docker_error(&err.to_string());
                        tracing::warn!(error = %err, "Docker is not ready");
                        (None, status)
                    }
                },
                Err(err) => {
                    let status = map_docker_error(&err.to_string());
                    tracing::warn!(error = %err, "Docker API version negotiation failed");
                    (None, status)
                }
            },
            Err(err) => {
                let status = map_docker_error(&err.to_string());
                tracing::warn!(error = %err, "Docker daemon unavailable");
                (None, status)
            }
        }
    }

    /// 重新探测 Docker 并刷新共享状态中的客户端与状态标记。
    pub async fn refresh_docker_state(&self) -> DockerServiceStatus {
        let (client, status) = Self::init_docker_state().await;
        let mut docker_guard = self.docker.write().await;
        *docker_guard = client;
        let mut status_guard = self.docker_status.write().await;
        *status_guard = status.clone();
        status
    }

    /// 获取可用的 Docker 客户端，不可用时返回业务错误。
    pub async fn docker_client(&self) -> ApiResult<Arc<Docker>> {
        if let Some(client) = self.docker.read().await.clone() {
            return Ok(client);
        }
        let status = self.refresh_docker_state().await;
        if status == DockerServiceStatus::Available
            && let Some(client) = self.docker.read().await.clone()
        {
            return Ok(client);
        }
        Err(ApiError::BadRequest(
            "Docker daemon unavailable".to_string(),
        ))
    }

    /// 刷新状态并判断 Docker 是否可用。
    pub async fn docker_available(&self) -> bool {
        self.refresh_docker_state().await == DockerServiceStatus::Available
    }

    /// 刷新并返回当前 Docker 服务状态。
    pub async fn docker_status(&self) -> DockerServiceStatus {
        self.refresh_docker_state().await
    }
}

fn map_docker_error(message: &str) -> DockerServiceStatus {
    let lower = message.to_lowercase();
    if lower.contains("permission denied") || lower.contains("os error 13") {
        return DockerServiceStatus::PermissionDenied;
    }
    if lower.contains("connection refused")
        || lower.contains("is the docker daemon running")
        || lower.contains("connection error")
    {
        return DockerServiceStatus::NotRunning;
    }
    DockerServiceStatus::Unknown
}
