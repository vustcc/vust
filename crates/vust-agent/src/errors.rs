//! Agent 领域错误。

use axum::http::StatusCode;
use thiserror::Error;
use vust_api::error::ApiError;
use vust_contracts::api::ErrorCode;

/// Agent 本地执行与资源访问错误。
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("docker unavailable")]
    DockerUnavailable,
    #[error("docker operation failed: {0}")]
    DockerOperation(String),
    #[error("container not found")]
    DockerContainerNotFound,
    #[error("invalid file path: {0}")]
    FileInvalidPath(String),
    #[error("file not found")]
    FileNotFound,
    #[error("file already exists")]
    FileAlreadyExists,
    #[error("missing file name")]
    FileMissingName,
    #[error("file upload invalid: {0}")]
    FileUploadInvalid(String),
    #[error("file operation failed: {0}")]
    FileOperation(#[from] std::io::Error),
    #[error("process operation failed: {0}")]
    Process(String),
    #[error("system operation failed: {0}")]
    System(String),
    #[error("task execution failed: {0}")]
    Task(String),
    #[error(transparent)]
    Storage(#[from] sqlx::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<AgentError> for ApiError {
    fn from(err: AgentError) -> Self {
        match err {
            AgentError::BadRequest(detail) => ApiError::bad_request(ErrorCode::BadRequest, detail),
            AgentError::DockerUnavailable => ApiError::bad_gateway(
                ErrorCode::DockerUnavailable,
                "Docker service was not detected",
            ),
            AgentError::DockerOperation(detail) => {
                ApiError::bad_gateway(ErrorCode::DockerOperationFailed, "Docker operation failed")
                    .with_detail(detail)
            }
            AgentError::DockerContainerNotFound => ApiError::not_found(
                ErrorCode::DockerContainerNotFound,
                "container does not exist",
            ),
            AgentError::FileInvalidPath(detail) => {
                ApiError::bad_request(ErrorCode::FileInvalidPath, detail)
            }
            AgentError::FileNotFound => {
                ApiError::not_found(ErrorCode::FileNotFound, "file does not exist")
            }
            AgentError::FileAlreadyExists => ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::FileAlreadyExists,
                "file already exists",
            ),
            AgentError::FileMissingName => ApiError::bad_request(
                ErrorCode::FileMissingName,
                "uploaded file is missing a file name",
            ),
            AgentError::FileUploadInvalid(detail) => ApiError::bad_request(
                ErrorCode::FileUploadInvalid,
                "failed to parse uploaded data",
            )
            .with_detail(detail),
            AgentError::FileOperation(err) => {
                ApiError::bad_request(ErrorCode::FileOperationFailed, "file operation failed")
                    .with_detail(err.to_string())
            }
            AgentError::Process(detail) => {
                ApiError::bad_request(ErrorCode::TaskExecutionFailed, "process operation failed")
                    .with_detail(detail)
            }
            AgentError::System(detail) => ApiError::internal(detail),
            AgentError::Task(detail) => {
                ApiError::bad_request(ErrorCode::TaskExecutionFailed, "task execution failed")
                    .with_detail(detail)
            }
            AgentError::Storage(err) => ApiError::database(err.to_string()),
            AgentError::Internal(detail) => ApiError::internal(detail),
        }
    }
}

impl From<bollard::errors::Error> for AgentError {
    fn from(err: bollard::errors::Error) -> Self {
        AgentError::DockerOperation(err.to_string())
    }
}

impl From<axum::extract::multipart::MultipartError> for AgentError {
    fn from(err: axum::extract::multipart::MultipartError) -> Self {
        AgentError::FileUploadInvalid(err.to_string())
    }
}
