//! Agent 与 Master 之间的 Docker 运行时任务契约。

use serde::{Deserialize, Serialize};

/// Agent 请求 Master 为当前节点准备镜像。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeImageTaskCreateRequest {
    pub agent_id: String,
    pub session_id: String,
    pub image_ref: String,
}

/// Agent 查询或取消运行时镜像任务时携带的会话身份。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeImageTaskQuery {
    pub agent_id: String,
    pub session_id: String,
}

/// 运行时镜像任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeImageTaskStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
}

/// 镜像最终来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeImageSource {
    Target,
    Controller,
    Registry,
}

/// 镜像获取任务当前阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeImageStage {
    Checking,
    Exporting,
    Uploading,
    Loading,
    Pulling,
}

/// 子节点仓库拉取过程中单个镜像分层的进度。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeImageLayerProgress {
    pub id: String,
    pub status_text: String,
    pub percent: Option<u8>,
}

/// Master 返回给 Agent 的镜像任务快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeImageTask {
    pub task_id: String,
    pub image_ref: String,
    pub status: RuntimeImageTaskStatus,
    pub source: Option<RuntimeImageSource>,
    pub stage: RuntimeImageStage,
    pub progress_percent: u8,
    pub status_text: String,
    pub layers: Vec<RuntimeImageLayerProgress>,
}
