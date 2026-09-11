//! API 共享契约：统一响应结构与错误码定义。

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 统一业务错误码，供 vust、agent 与 frontend 对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export_to = "api/")]
pub enum ErrorCode {
    AuthUnauthorized,
    AuthForbidden,
    AuthMissingCredentials,
    AuthWrongCredentials,
    AuthInvalidToken,
    AuthTokenExpired,
    AuthTokenCreationFailed,
    AuthInvalidPassword,
    AuthWrongOldPassword,
    AuthUsernameExists,
    AuthClientVersionMismatch,
    AgentUnavailable,
    AgentNotFound,
    AgentPermissionDenied,
    AgentRefused,
    AgentTimeout,
    AgentRequestFailed,
    NodeNotFound,
    NodeInvalidTarget,
    NodeUnavailable,
    NodeAlreadyExists,
    NodeDeployFailed,
    NodePrecheckFailed,
    NodePrecheckStale,
    NodeProtected,
    NodeStateConflict,
    NodeInUse,
    NodeOperationConflict,
    RuntimeSessionNotFound,
    RuntimeLeaseMismatch,
    RuntimeRegistrationFailed,
    RuntimeCertificateRotationFailed,
    RuntimeLogCursorStale,
    OperationLogNotFound,
    NotificationNotFound,
    NotificationInvalidQuery,
    NotificationInvalidBatch,
    SystemMonitoringInvalidRange,
    SystemMonitoringInvalidSettings,
    SystemMonitoringHistoryBusy,
    SystemMonitoringUnavailable,
    DiskNotFound,
    DiskPartitionNotFound,
    DiskManagedVolumeNotFound,
    DiskIdentityUnavailable,
    DiskChanged,
    DiskProtected,
    DiskInUse,
    DiskNotBlank,
    DiskUnsupportedTopology,
    DiskInvalidMountName,
    DiskMountConflict,
    DiskAlreadyManaged,
    DiskConfirmationInvalid,
    DiskOperationConflict,
    DiskOperationNotFound,
    DiskOperationNotCancellable,
    DiskNodeUnavailable,
    DiskInventoryPartial,
    DiskFstabConflict,
    DiskOperationFailed,
    DockerUnavailable,
    DockerOperationFailed,
    DockerContainerNotFound,
    DockerProjectNotFound,
    DockerProjectAlreadyExists,
    DockerProjectProtected,
    DockerProjectBusy,
    DockerProjectInUse,
    DockerProjectRevisionConflict,
    DockerImageInUse,
    DockerNetworkProtected,
    DockerNetworkInUse,
    DockerVolumeInUse,
    SuiteWorkloadPortUnavailable,
    FileInvalidPath,
    FileNotFound,
    FileAlreadyExists,
    FileMissingName,
    FileUploadInvalid,
    FileOperationFailed,
    FileChanged,
    FilePermissionDenied,
    FileTypeUnsupported,
    FileContentTooLarge,
    FileHardLinkUnsupported,
    FileMetadataPreservationFailed,
    FileOperationConflict,
    FileTaskNotFound,
    FileTaskNotCancellable,
    FileTransferInvalidRange,
    FileTransferExpired,
    FileChecksumMismatch,
    FileStorageExhausted,
    FirewallInvalidQuery,
    FirewallRuleNotFound,
    FirewallSnapshotExpired,
    FirewallRulesetTooLarge,
    FirewallUnsupportedPlatform,
    FirewallUnavailable,
    ProcessInvalidId,
    ProcessNotFound,
    ProcessChanged,
    ProcessOperationConflict,
    ProcessPermissionDenied,
    ProcessConfirmationRequired,
    ProcessConfirmationInvalid,
    ProcessSignalUnavailable,
    ProcessSamplerUnavailable,
    NetworkSamplerUnavailable,
    ScheduledTaskNotFound,
    ScheduledTaskInvalidName,
    ScheduledTaskInvalidCommand,
    ScheduledTaskInvalidSchedule,
    ScheduledTaskInvalidTimeZone,
    ScheduledTaskAlreadyExists,
    ScheduledTaskProtected,
    ScheduledTaskInUse,
    ScheduledTaskOperationConflict,
    ScheduledTaskNodeUnavailable,
    ScheduledTaskRevisionConflict,
    ScheduledTaskRunNotFound,
    ScheduledTaskRunNotCancellable,
    ScheduledTaskMigrationFailed,
    ScriptNotFound,
    ScriptInvalidName,
    ScriptInvalidDescription,
    ScriptInvalidContent,
    ScriptAlreadyExists,
    ScriptProtected,
    ScriptRevisionConflict,
    ScriptInUse,
    ScriptRunNotFound,
    ScriptRunConflict,
    ScriptRunNotCancellable,
    ScriptNodeUnavailable,
    TaskNotFound,
    TaskInvalidSchedule,
    TaskExecutionFailed,
    TaskRevisionConflict,
    VustInvalidConfig,
    VustPortUnavailable,
    ValidationFailed,
    BadRequest,
    InternalServerError,
    DatabaseError,
    ExternalRequestFailed,
}

impl ErrorCode {
    /// 返回稳定字符串，便于作为跨端协议字段传输。
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::AuthUnauthorized => "AUTH_UNAUTHORIZED",
            ErrorCode::AuthForbidden => "AUTH_FORBIDDEN",
            ErrorCode::AuthMissingCredentials => "AUTH_MISSING_CREDENTIALS",
            ErrorCode::AuthWrongCredentials => "AUTH_WRONG_CREDENTIALS",
            ErrorCode::AuthInvalidToken => "AUTH_INVALID_TOKEN",
            ErrorCode::AuthTokenExpired => "AUTH_TOKEN_EXPIRED",
            ErrorCode::AuthTokenCreationFailed => "AUTH_TOKEN_CREATION_FAILED",
            ErrorCode::AuthInvalidPassword => "AUTH_INVALID_PASSWORD",
            ErrorCode::AuthWrongOldPassword => "AUTH_WRONG_OLD_PASSWORD",
            ErrorCode::AuthUsernameExists => "AUTH_USERNAME_EXISTS",
            ErrorCode::AuthClientVersionMismatch => "AUTH_CLIENT_VERSION_MISMATCH",
            ErrorCode::AgentUnavailable => "AGENT_UNAVAILABLE",
            ErrorCode::AgentNotFound => "AGENT_NOT_FOUND",
            ErrorCode::AgentPermissionDenied => "AGENT_PERMISSION_DENIED",
            ErrorCode::AgentRefused => "AGENT_REFUSED",
            ErrorCode::AgentTimeout => "AGENT_TIMEOUT",
            ErrorCode::AgentRequestFailed => "AGENT_REQUEST_FAILED",
            ErrorCode::NodeNotFound => "NODE_NOT_FOUND",
            ErrorCode::NodeInvalidTarget => "NODE_INVALID_TARGET",
            ErrorCode::NodeUnavailable => "NODE_UNAVAILABLE",
            ErrorCode::NodeAlreadyExists => "NODE_ALREADY_EXISTS",
            ErrorCode::NodeDeployFailed => "NODE_DEPLOY_FAILED",
            ErrorCode::NodePrecheckFailed => "NODE_PRECHECK_FAILED",
            ErrorCode::NodePrecheckStale => "NODE_PRECHECK_STALE",
            ErrorCode::NodeProtected => "NODE_PROTECTED",
            ErrorCode::NodeStateConflict => "NODE_STATE_CONFLICT",
            ErrorCode::NodeInUse => "NODE_IN_USE",
            ErrorCode::NodeOperationConflict => "NODE_OPERATION_CONFLICT",
            ErrorCode::RuntimeSessionNotFound => "RUNTIME_SESSION_NOT_FOUND",
            ErrorCode::RuntimeLeaseMismatch => "RUNTIME_LEASE_MISMATCH",
            ErrorCode::RuntimeRegistrationFailed => "RUNTIME_REGISTRATION_FAILED",
            ErrorCode::RuntimeCertificateRotationFailed => "RUNTIME_CERTIFICATE_ROTATION_FAILED",
            ErrorCode::RuntimeLogCursorStale => "RUNTIME_LOG_CURSOR_STALE",
            ErrorCode::OperationLogNotFound => "OPERATION_LOG_NOT_FOUND",
            ErrorCode::NotificationNotFound => "NOTIFICATION_NOT_FOUND",
            ErrorCode::NotificationInvalidQuery => "NOTIFICATION_INVALID_QUERY",
            ErrorCode::NotificationInvalidBatch => "NOTIFICATION_INVALID_BATCH",
            ErrorCode::SystemMonitoringInvalidRange => "SYSTEM_MONITORING_INVALID_RANGE",
            ErrorCode::SystemMonitoringInvalidSettings => "SYSTEM_MONITORING_INVALID_SETTINGS",
            ErrorCode::SystemMonitoringHistoryBusy => "SYSTEM_MONITORING_HISTORY_BUSY",
            ErrorCode::SystemMonitoringUnavailable => "SYSTEM_MONITORING_UNAVAILABLE",
            ErrorCode::DiskNotFound => "DISK_NOT_FOUND",
            ErrorCode::DiskPartitionNotFound => "DISK_PARTITION_NOT_FOUND",
            ErrorCode::DiskManagedVolumeNotFound => "DISK_MANAGED_VOLUME_NOT_FOUND",
            ErrorCode::DiskIdentityUnavailable => "DISK_IDENTITY_UNAVAILABLE",
            ErrorCode::DiskChanged => "DISK_CHANGED",
            ErrorCode::DiskProtected => "DISK_PROTECTED",
            ErrorCode::DiskInUse => "DISK_IN_USE",
            ErrorCode::DiskNotBlank => "DISK_NOT_BLANK",
            ErrorCode::DiskUnsupportedTopology => "DISK_UNSUPPORTED_TOPOLOGY",
            ErrorCode::DiskInvalidMountName => "DISK_INVALID_MOUNT_NAME",
            ErrorCode::DiskMountConflict => "DISK_MOUNT_CONFLICT",
            ErrorCode::DiskAlreadyManaged => "DISK_ALREADY_MANAGED",
            ErrorCode::DiskConfirmationInvalid => "DISK_CONFIRMATION_INVALID",
            ErrorCode::DiskOperationConflict => "DISK_OPERATION_CONFLICT",
            ErrorCode::DiskOperationNotFound => "DISK_OPERATION_NOT_FOUND",
            ErrorCode::DiskOperationNotCancellable => "DISK_OPERATION_NOT_CANCELLABLE",
            ErrorCode::DiskNodeUnavailable => "DISK_NODE_UNAVAILABLE",
            ErrorCode::DiskInventoryPartial => "DISK_INVENTORY_PARTIAL",
            ErrorCode::DiskFstabConflict => "DISK_FSTAB_CONFLICT",
            ErrorCode::DiskOperationFailed => "DISK_OPERATION_FAILED",
            ErrorCode::DockerUnavailable => "DOCKER_UNAVAILABLE",
            ErrorCode::DockerOperationFailed => "DOCKER_OPERATION_FAILED",
            ErrorCode::DockerContainerNotFound => "DOCKER_CONTAINER_NOT_FOUND",
            ErrorCode::DockerProjectNotFound => "DOCKER_PROJECT_NOT_FOUND",
            ErrorCode::DockerProjectAlreadyExists => "DOCKER_PROJECT_ALREADY_EXISTS",
            ErrorCode::DockerProjectProtected => "DOCKER_PROJECT_PROTECTED",
            ErrorCode::DockerProjectBusy => "DOCKER_PROJECT_BUSY",
            ErrorCode::DockerProjectInUse => "DOCKER_PROJECT_IN_USE",
            ErrorCode::DockerProjectRevisionConflict => "DOCKER_PROJECT_REVISION_CONFLICT",
            ErrorCode::DockerImageInUse => "DOCKER_IMAGE_IN_USE",
            ErrorCode::DockerNetworkProtected => "DOCKER_NETWORK_PROTECTED",
            ErrorCode::DockerNetworkInUse => "DOCKER_NETWORK_IN_USE",
            ErrorCode::DockerVolumeInUse => "DOCKER_VOLUME_IN_USE",
            ErrorCode::SuiteWorkloadPortUnavailable => "SUITE_WORKLOAD_PORT_UNAVAILABLE",
            ErrorCode::FileInvalidPath => "FILE_INVALID_PATH",
            ErrorCode::FileNotFound => "FILE_NOT_FOUND",
            ErrorCode::FileAlreadyExists => "FILE_ALREADY_EXISTS",
            ErrorCode::FileMissingName => "FILE_MISSING_NAME",
            ErrorCode::FileUploadInvalid => "FILE_UPLOAD_INVALID",
            ErrorCode::FileOperationFailed => "FILE_OPERATION_FAILED",
            ErrorCode::FileChanged => "FILE_CHANGED",
            ErrorCode::FilePermissionDenied => "FILE_PERMISSION_DENIED",
            ErrorCode::FileTypeUnsupported => "FILE_TYPE_UNSUPPORTED",
            ErrorCode::FileContentTooLarge => "FILE_CONTENT_TOO_LARGE",
            ErrorCode::FileHardLinkUnsupported => "FILE_HARD_LINK_UNSUPPORTED",
            ErrorCode::FileMetadataPreservationFailed => "FILE_METADATA_PRESERVATION_FAILED",
            ErrorCode::FileOperationConflict => "FILE_OPERATION_CONFLICT",
            ErrorCode::FileTaskNotFound => "FILE_TASK_NOT_FOUND",
            ErrorCode::FileTaskNotCancellable => "FILE_TASK_NOT_CANCELLABLE",
            ErrorCode::FileTransferInvalidRange => "FILE_TRANSFER_INVALID_RANGE",
            ErrorCode::FileTransferExpired => "FILE_TRANSFER_EXPIRED",
            ErrorCode::FileChecksumMismatch => "FILE_CHECKSUM_MISMATCH",
            ErrorCode::FileStorageExhausted => "FILE_STORAGE_EXHAUSTED",
            ErrorCode::FirewallInvalidQuery => "FIREWALL_INVALID_QUERY",
            ErrorCode::FirewallRuleNotFound => "FIREWALL_RULE_NOT_FOUND",
            ErrorCode::FirewallSnapshotExpired => "FIREWALL_SNAPSHOT_EXPIRED",
            ErrorCode::FirewallRulesetTooLarge => "FIREWALL_RULESET_TOO_LARGE",
            ErrorCode::FirewallUnsupportedPlatform => "FIREWALL_UNSUPPORTED_PLATFORM",
            ErrorCode::FirewallUnavailable => "FIREWALL_UNAVAILABLE",
            ErrorCode::ProcessInvalidId => "PROCESS_INVALID_ID",
            ErrorCode::ProcessNotFound => "PROCESS_NOT_FOUND",
            ErrorCode::ProcessChanged => "PROCESS_CHANGED",
            ErrorCode::ProcessOperationConflict => "PROCESS_OPERATION_CONFLICT",
            ErrorCode::ProcessPermissionDenied => "PROCESS_PERMISSION_DENIED",
            ErrorCode::ProcessConfirmationRequired => "PROCESS_CONFIRMATION_REQUIRED",
            ErrorCode::ProcessConfirmationInvalid => "PROCESS_CONFIRMATION_INVALID",
            ErrorCode::ProcessSignalUnavailable => "PROCESS_SIGNAL_UNAVAILABLE",
            ErrorCode::ProcessSamplerUnavailable => "PROCESS_SAMPLER_UNAVAILABLE",
            ErrorCode::NetworkSamplerUnavailable => "NETWORK_SAMPLER_UNAVAILABLE",
            ErrorCode::ScheduledTaskNotFound => "SCHEDULED_TASK_NOT_FOUND",
            ErrorCode::ScheduledTaskInvalidName => "SCHEDULED_TASK_INVALID_NAME",
            ErrorCode::ScheduledTaskInvalidCommand => "SCHEDULED_TASK_INVALID_COMMAND",
            ErrorCode::ScheduledTaskInvalidSchedule => "SCHEDULED_TASK_INVALID_SCHEDULE",
            ErrorCode::ScheduledTaskInvalidTimeZone => "SCHEDULED_TASK_INVALID_TIME_ZONE",
            ErrorCode::ScheduledTaskAlreadyExists => "SCHEDULED_TASK_ALREADY_EXISTS",
            ErrorCode::ScheduledTaskProtected => "SCHEDULED_TASK_PROTECTED",
            ErrorCode::ScheduledTaskInUse => "SCHEDULED_TASK_IN_USE",
            ErrorCode::ScheduledTaskOperationConflict => "SCHEDULED_TASK_OPERATION_CONFLICT",
            ErrorCode::ScheduledTaskNodeUnavailable => "SCHEDULED_TASK_NODE_UNAVAILABLE",
            ErrorCode::ScheduledTaskRevisionConflict => "SCHEDULED_TASK_REVISION_CONFLICT",
            ErrorCode::ScheduledTaskRunNotFound => "SCHEDULED_TASK_RUN_NOT_FOUND",
            ErrorCode::ScheduledTaskRunNotCancellable => "SCHEDULED_TASK_RUN_NOT_CANCELLABLE",
            ErrorCode::ScheduledTaskMigrationFailed => "SCHEDULED_TASK_MIGRATION_FAILED",
            ErrorCode::ScriptNotFound => "SCRIPT_NOT_FOUND",
            ErrorCode::ScriptInvalidName => "SCRIPT_INVALID_NAME",
            ErrorCode::ScriptInvalidDescription => "SCRIPT_INVALID_DESCRIPTION",
            ErrorCode::ScriptInvalidContent => "SCRIPT_INVALID_CONTENT",
            ErrorCode::ScriptAlreadyExists => "SCRIPT_ALREADY_EXISTS",
            ErrorCode::ScriptProtected => "SCRIPT_PROTECTED",
            ErrorCode::ScriptRevisionConflict => "SCRIPT_REVISION_CONFLICT",
            ErrorCode::ScriptInUse => "SCRIPT_IN_USE",
            ErrorCode::ScriptRunNotFound => "SCRIPT_RUN_NOT_FOUND",
            ErrorCode::ScriptRunConflict => "SCRIPT_RUN_CONFLICT",
            ErrorCode::ScriptRunNotCancellable => "SCRIPT_RUN_NOT_CANCELLABLE",
            ErrorCode::ScriptNodeUnavailable => "SCRIPT_NODE_UNAVAILABLE",
            ErrorCode::TaskNotFound => "TASK_NOT_FOUND",
            ErrorCode::TaskInvalidSchedule => "TASK_INVALID_SCHEDULE",
            ErrorCode::TaskExecutionFailed => "TASK_EXECUTION_FAILED",
            ErrorCode::TaskRevisionConflict => "TASK_REVISION_CONFLICT",
            ErrorCode::VustInvalidConfig => "VUST_INVALID_CONFIG",
            ErrorCode::VustPortUnavailable => "VUST_PORT_UNAVAILABLE",
            ErrorCode::ValidationFailed => "VALIDATION_FAILED",
            ErrorCode::BadRequest => "BAD_REQUEST",
            ErrorCode::InternalServerError => "INTERNAL_SERVER_ERROR",
            ErrorCode::DatabaseError => "DATABASE_ERROR",
            ErrorCode::ExternalRequestFailed => "EXTERNAL_REQUEST_FAILED",
        }
    }
}

/// 统一封装成功状态、消息与可选数据的响应体。
///
/// 用于封装所有 HTTP API 的返回数据，提供统一的跨端格式。
#[derive(Serialize, Deserialize, Debug, TS)]
#[ts(export_to = "api/", optional_fields)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub code: u16,
    pub message: String,
    #[serde(rename = "messageKey", skip_serializing_if = "Option::is_none")]
    pub message_key: Option<String>,
    #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

/// `ApiResponse` 的构建器。
pub struct ApiResponseBuilder<T> {
    success: bool,
    code: u16,
    message: String,
    message_key: Option<String>,
    error_code: Option<ErrorCode>,
    data: Option<T>,
}

impl<T> ApiResponseBuilder<T> {
    /// 创建一个新的 `ApiResponseBuilder`，并带有默认值。
    pub fn new() -> Self {
        Self {
            success: true,
            code: 200,
            message: String::new(),
            message_key: None,
            error_code: None,
            data: None,
        }
    }

    /// 设置 `success` 字段。
    pub fn success(mut self, success: bool) -> Self {
        self.success = success;
        self
    }

    /// 设置 HTTP 状态码。
    pub fn code(mut self, code: u16) -> Self {
        self.code = code;
        self
    }

    /// 设置响应消息。
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// 设置响应 i18n 消息 key。
    pub fn message_key(mut self, message_key: impl Into<String>) -> Self {
        self.message_key = Some(message_key.into());
        self
    }

    /// 设置统一业务错误码。
    pub fn error_code(mut self, error_code: ErrorCode) -> Self {
        self.error_code = Some(error_code);
        self
    }

    /// 设置响应数据。
    pub fn data(mut self, data: T) -> Self {
        self.data = Some(data);
        self
    }

    /// 构建最终的 `ApiResponse<T>` 实例。
    pub fn build(self) -> ApiResponse<T> {
        ApiResponse {
            success: self.success,
            code: self.code,
            message: self.message,
            message_key: self.message_key,
            error_code: self.error_code,
            data: self.data,
        }
    }
}

impl<T> Default for ApiResponseBuilder<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ApiResponse<T> {
    /// 返回一个新的 `ApiResponseBuilder` 实例。
    pub fn builder() -> ApiResponseBuilder<T> {
        ApiResponseBuilder::new()
    }

    /// 快速构建一个失败的 `ApiResponse`。
    pub fn error(message: &str, raw_data: T, code: u16) -> Self {
        ApiResponse {
            success: false,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: Some(raw_data),
            code,
        }
    }

    /// 快速构建一个带业务错误码且不携带 `data` 的失败响应。
    pub fn error_with_code(message: &str, error_code: ErrorCode, code: u16) -> Self {
        ApiResponse {
            success: false,
            message: message.to_string(),
            message_key: None,
            error_code: Some(error_code),
            data: None,
            code,
        }
    }

    /// 快速构建一个成功的 `ApiResponse`。
    pub fn success(message: &str, raw_data: T, code: u16) -> Self {
        Self {
            success: true,
            code,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: Some(raw_data),
        }
    }

    /// 快速构建一个成功的 `ApiResponse`，使用默认的 `code = 200`。
    pub fn success_with_raw(message: &str, raw_data: T) -> Self {
        Self {
            success: true,
            code: 200,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: Some(raw_data),
        }
    }

    /// 快速构建一个失败的 `ApiResponse`，使用默认的 `code = 400`。
    pub fn error_with_raw(message: &str, raw_data: T) -> Self {
        Self {
            success: false,
            code: 400,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: Some(raw_data),
        }
    }

    /// 设置响应的 HTTP 状态码。
    pub fn set_code(mut self, code: u16) -> Self {
        self.code = code;
        self
    }
}

impl ApiResponse<()> {
    /// 快速构建一个成功的、不带 `data` 的 `ApiResponse`。
    pub fn ok(message: &str) -> ApiResponse<()> {
        ApiResponse {
            success: true,
            code: 200,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: None,
        }
    }

    /// 快速构建一个失败的、不带 `data` 的 `ApiResponse`。
    ///
    /// 注意：此处的 `success` 字段为 `true`，为保持兼容性暂不修改。
    pub fn fail(message: &str) -> ApiResponse<()> {
        ApiResponse {
            success: true,
            code: 400,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: None,
        }
    }

    /// 快速构建一个成功的、不带 `data` 的 `ApiResponse`，并指定状态码。
    pub fn success_msg(message: &str, code: u16) -> ApiResponse<()> {
        ApiResponse {
            success: true,
            code,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: None,
        }
    }

    /// 快速构建一个失败的、不带 `data` 的 `ApiResponse`，并指定状态码。
    pub fn error_msg(message: &str, code: u16) -> ApiResponse<()> {
        ApiResponse {
            success: false,
            code,
            message: message.to_string(),
            message_key: None,
            error_code: None,
            data: None,
        }
    }
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    /// 将统一响应封装为 Axum JSON 响应。
    fn into_response(self) -> Response {
        let status_code = match StatusCode::from_u16(self.code) {
            Ok(code) => code,
            Err(_) => {
                if self.success {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_REQUEST
                }
            }
        };
        (status_code, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coded_error_does_not_duplicate_error_code_in_data() {
        let response = ApiResponse::<serde_json::Value>::error_with_code(
            "unavailable",
            ErrorCode::FirewallUnavailable,
            503,
        );

        assert!(!response.success);
        assert_eq!(response.error_code, Some(ErrorCode::FirewallUnavailable));
        assert!(response.data.is_none());
        let serialized = serde_json::to_value(&response).unwrap();
        assert!(serialized.get("data").is_none());
    }
}
