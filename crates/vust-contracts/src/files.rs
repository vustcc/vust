//! 文件管理领域契约：文件元数据、后台操作任务与流式传输。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 文件系统条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// 文件路径所属的 VUST 管理域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileManagementKind {
    Custom,
    Compose,
    Suite,
    System,
}

/// 文件路径的管理归属；该信息只作事实标注，不作为权限边界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/", optional_fields)]
pub struct FileManagement {
    pub kind: FileManagementKind,
    pub owner_name: Option<String>,
    pub manage_via: Option<String>,
}

/// 最终执行端计算的文件操作能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub struct FileCapabilities {
    pub can_open: bool,
    pub can_read: bool,
    pub can_write: bool,
    pub can_create_child: bool,
    pub can_rename: bool,
    pub can_copy: bool,
    pub can_remove: bool,
    pub can_upload: bool,
    pub can_download: bool,
}

/// 文件列表摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/", optional_fields)]
pub struct FileEntrySummary {
    pub name: String,
    pub path: String,
    pub kind: FileEntryKind,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<String>,
    pub created_at: Option<String>,
    pub revision: String,
    pub management: FileManagement,
    pub capabilities: FileCapabilities,
}

/// 文件详情，补充 POSIX 所有者和符号链接信息。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/", optional_fields)]
pub struct FileEntryDetail {
    #[serde(flatten)]
    #[ts(flatten)]
    pub summary: FileEntrySummary,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub user_name: Option<String>,
    pub group_name: Option<String>,
    pub symlink_target: Option<String>,
}

/// 当前目录的真实条目统计。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub struct FileEntryCounts {
    pub file_count: u64,
    pub directory_count: u64,
    pub symlink_count: u64,
    pub other_count: u64,
}

/// 服务端排序和分页后的目录列表。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub struct FileListPage {
    pub path: String,
    pub entries: Vec<FileEntrySummary>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub counts: FileEntryCounts,
    pub loaded_at: String,
}

/// UTF-8 文本文档及乐观并发版本。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/", optional_fields)]
pub struct FileDocument {
    pub path: String,
    pub content: String,
    #[ts(type = "\"utf8\"")]
    pub encoding: String,
    pub size_bytes: u64,
    pub revision: String,
    pub modified_at: Option<String>,
    pub loaded_at: String,
    #[ts(inline)]
    pub capabilities: FileCapabilities,
}

/// 文件保存提交后的持久性状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileSaveDurability {
    Durable,
    Uncertain,
}

/// 原子文件保存结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub struct FileSaveResult {
    #[ts(inline)]
    pub document: FileDocument,
    #[ts(inline)]
    pub durability: FileSaveDurability,
}

/// Agent 有效主目录。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub struct FileHome {
    pub path: String,
}

/// 创建普通文件请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "files/", optional_fields)]
pub struct CreateFileRequest {
    pub path: String,
    pub content: Option<String>,
}

/// 原子更新文本文件请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "files/")]
pub struct UpdateFileContentRequest {
    pub path: String,
    pub content: String,
    pub expected_revision: String,
}

/// 创建目录请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "files/")]
pub struct CreateDirectoryRequest {
    pub path: String,
    pub recursive: bool,
}

/// 文件后台操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileOperation {
    Copy,
    Move,
    Remove,
}

/// 文件后台任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileTaskStatus {
    Queued,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}

/// 文件后台任务阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileTaskStage {
    Validating,
    Preparing,
    Copying,
    Moving,
    Deleting,
    RollingBack,
    CleaningUp,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

/// 后台任务单个条目的执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileTaskItemStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// 后台任务输入条目。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "files/", optional_fields)]
pub struct FileOperationItemRequest {
    pub path: String,
    pub expected_revision: Option<String>,
    pub target_path: Option<String>,
}

/// 创建文件后台任务请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "files/", optional_fields)]
pub struct CreateFileOperationTaskRequest {
    pub operation: FileOperation,
    pub items: Vec<FileOperationItemRequest>,
    pub target_directory: Option<String>,
    pub recursive: bool,
    pub overwrite: bool,
    pub idempotency_key: String,
}

/// 单个后台任务条目的执行结果。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/", optional_fields)]
pub struct FileOperationItemResult {
    pub path: String,
    pub target_path: Option<String>,
    pub status: FileTaskItemStatus,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

/// 文件后台任务领域状态。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/", optional_fields)]
pub struct FileOperationTask {
    pub task_id: String,
    pub node_id: String,
    pub operation: FileOperation,
    pub status: FileTaskStatus,
    pub stage: FileTaskStage,
    pub progress_percent: u8,
    pub total_item_count: u64,
    pub completed_item_count: u64,
    pub failed_item_count: u64,
    pub total_bytes: u64,
    pub processed_bytes: u64,
    pub items: Vec<FileOperationItemResult>,
    pub error_summary: Option<String>,
    pub cleanup_warning: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// 后台任务已接受响应。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub struct FileTaskAccepted {
    pub task_id: String,
}

/// 文件传输方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileTransferDirection {
    Upload,
    Download,
}

/// 文件传输状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/")]
pub enum FileTransferStatus {
    Created,
    Receiving,
    Ready,
    Streaming,
    Completed,
    Failed,
    Cancelled,
    Expired,
}

/// 创建上传或下载传输请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "files/", optional_fields)]
pub struct CreateFileTransferRequest {
    pub direction: FileTransferDirection,
    pub path: String,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub expected_revision: Option<String>,
    pub overwrite: bool,
}

/// 文件传输领域状态。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "files/", optional_fields)]
pub struct FileTransfer {
    pub transfer_id: String,
    pub node_id: String,
    pub direction: FileTransferDirection,
    pub status: FileTransferStatus,
    pub path: String,
    pub size_bytes: u64,
    pub transferred_bytes: u64,
    pub revision: Option<String>,
    pub error_summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: String,
}
