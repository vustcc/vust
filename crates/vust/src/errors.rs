//! VUST 应用层与领域错误。

use axum::http::StatusCode;
use thiserror::Error;
use vust_api::error::ApiError;
use vust_contracts::api::ErrorCode;

/// VUST 控制器错误，用于聚合领域错误并在 HTTP 边界转换为 `ApiError`。
#[derive(Debug, Error)]
pub enum ControllerError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error(transparent)]
    Agent(#[from] AgentClientError),
    #[error(transparent)]
    Task(#[from] TaskError),
    #[error(transparent)]
    VustConfig(#[from] VustConfigError),
    #[error("internal error: {0}")]
    Internal(String),
}

/// 认证领域错误。
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("missing credentials")]
    MissingCredentials,
    #[error("wrong credentials")]
    WrongCredentials,
    #[error("invalid token")]
    InvalidToken,
    #[error("token expired")]
    TokenExpired,
    #[error("token creation failed")]
    TokenCreation,
    #[error("invalid password")]
    InvalidPassword,
    #[error("wrong old password")]
    WrongOldPassword,
    #[error("username already exists")]
    UsernameExists,
    #[error("forbidden")]
    Forbidden,
}

/// 节点管理领域错误。
#[derive(Debug, Error)]
pub enum NodeError {
    #[error("node not found")]
    NotFound,
    #[error("invalid node target: {0}")]
    InvalidTarget(String),
    #[error("node unavailable: {0}")]
    Unavailable(String),
    #[error("node already exists")]
    AlreadyExists,
    #[error("node deploy failed: {0}")]
    DeployFailed(String),
    #[error("node precheck failed: {0}")]
    PrecheckFailed(String),
    #[error(transparent)]
    Storage(#[from] sqlx::Error),
}

/// VUST 到 Agent 的客户端错误。
#[derive(Debug, Error)]
pub enum AgentClientError {
    #[error("agent unix socket not found")]
    NotFound,
    #[error("permission denied accessing agent")]
    PermissionDenied,
    #[error("connection to agent timed out")]
    Timeout,
    #[error("agent connection refused")]
    Refused,
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("agent error: {0}")]
    Other(String),
}

/// 任务调度领域错误。
#[derive(Debug, Error)]
pub enum TaskError {
    #[error("task not found")]
    NotFound,
    #[error("invalid schedule: {0}")]
    InvalidSchedule(String),
    #[error("task execution failed: {0}")]
    ExecutionFailed(String),
    #[error(transparent)]
    Storage(#[from] sqlx::Error),
}

/// VUST 配置领域错误。
#[derive(Debug, Error)]
pub enum VustConfigError {
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("port unavailable: {0}")]
    PortUnavailable(String),
    #[error(transparent)]
    Storage(#[from] sqlx::Error),
}

impl From<ControllerError> for ApiError {
    fn from(err: ControllerError) -> Self {
        match err {
            ControllerError::Auth(err) => err.into(),
            ControllerError::Node(err) => err.into(),
            ControllerError::Agent(err) => err.into(),
            ControllerError::Task(err) => err.into(),
            ControllerError::VustConfig(err) => err.into(),
            ControllerError::Internal(detail) => ApiError::internal(detail),
        }
    }
}

impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::MissingCredentials => ApiError::bad_request(
                ErrorCode::AuthMissingCredentials,
                "missing username or password",
            )
            .with_message_key("api.auth.missingCredentials"),
            AuthError::WrongCredentials => ApiError::unauthorized(
                ErrorCode::AuthWrongCredentials,
                "wrong username or password",
            )
            .with_message_key("api.auth.wrongCredentials"),
            AuthError::InvalidToken => {
                ApiError::unauthorized(ErrorCode::AuthInvalidToken, "session expired or invalid")
                    .with_message_key("api.auth.sessionExpired")
            }
            AuthError::TokenExpired => {
                ApiError::unauthorized(ErrorCode::AuthTokenExpired, "token expired")
                    .with_message_key("api.auth.sessionExpired")
            }
            AuthError::TokenCreation => ApiError::internal("token creation failed")
                .with_detail(ErrorCode::AuthTokenCreationFailed.as_str()),
            AuthError::InvalidPassword => {
                ApiError::unauthorized(ErrorCode::AuthInvalidPassword, "invalid password")
                    .with_message_key("api.auth.invalidPassword")
            }
            AuthError::WrongOldPassword => {
                ApiError::bad_request(ErrorCode::AuthWrongOldPassword, "wrong old password")
                    .with_message_key("api.auth.wrongOldPassword")
            }
            AuthError::UsernameExists => {
                ApiError::conflict(ErrorCode::AuthUsernameExists, "username already exists")
                    .with_message_key("api.auth.usernameExists")
            }
            AuthError::Forbidden => ApiError::forbidden(ErrorCode::AuthForbidden, "forbidden")
                .with_message_key("api.auth.forbidden"),
        }
    }
}

impl From<NodeError> for ApiError {
    fn from(err: NodeError) -> Self {
        match err {
            NodeError::NotFound => ApiError::not_found(ErrorCode::NodeNotFound, "node not found"),
            NodeError::InvalidTarget(detail) => {
                ApiError::bad_request(ErrorCode::NodeInvalidTarget, detail)
            }
            NodeError::Unavailable(detail) => {
                ApiError::bad_gateway(ErrorCode::NodeUnavailable, "node unavailable")
                    .with_detail(detail)
            }
            NodeError::AlreadyExists => {
                ApiError::conflict(ErrorCode::NodeAlreadyExists, "node already exists")
            }
            NodeError::DeployFailed(detail) => {
                ApiError::bad_gateway(ErrorCode::NodeDeployFailed, "node deploy failed")
                    .with_detail(detail)
            }
            NodeError::PrecheckFailed(detail) => {
                ApiError::bad_request(ErrorCode::NodePrecheckFailed, "node precheck failed")
                    .with_detail(detail)
            }
            NodeError::Storage(err) => ApiError::database(err.to_string()),
        }
    }
}

impl From<AgentClientError> for ApiError {
    fn from(err: AgentClientError) -> Self {
        match err {
            AgentClientError::NotFound => ApiError::not_found(
                ErrorCode::AgentNotFound,
                "target node has no active session",
            ),
            AgentClientError::PermissionDenied => ApiError::forbidden(
                ErrorCode::AgentPermissionDenied,
                "agent runtime access denied",
            ),
            AgentClientError::Timeout => {
                ApiError::gateway_timeout(ErrorCode::AgentTimeout, "Agent timeout")
            }
            AgentClientError::Refused => ApiError::bad_gateway(
                ErrorCode::AgentRefused,
                "Failed to contact the Agent. Please check whether the Agent process is running.",
            ),
            AgentClientError::Request(err) => {
                ApiError::bad_gateway(ErrorCode::AgentRequestFailed, "Agent request failed")
                    .with_detail(err.to_string())
            }
            AgentClientError::Other(detail) => ApiError::new(
                StatusCode::BAD_GATEWAY,
                ErrorCode::AgentUnavailable,
                "Agent Error",
            )
            .with_detail(detail),
        }
    }
}

impl From<TaskError> for ApiError {
    fn from(err: TaskError) -> Self {
        match err {
            TaskError::NotFound => ApiError::not_found(ErrorCode::TaskNotFound, "task not found"),
            TaskError::InvalidSchedule(detail) => {
                ApiError::bad_request(ErrorCode::TaskInvalidSchedule, detail)
            }
            TaskError::ExecutionFailed(detail) => {
                ApiError::bad_gateway(ErrorCode::TaskExecutionFailed, "task execution failed")
                    .with_detail(detail)
            }
            TaskError::Storage(err) => ApiError::database(err.to_string()),
        }
    }
}

impl From<VustConfigError> for ApiError {
    fn from(err: VustConfigError) -> Self {
        match err {
            VustConfigError::Invalid(detail) => {
                ApiError::bad_request(ErrorCode::VustInvalidConfig, detail)
            }
            VustConfigError::PortUnavailable(detail) => {
                ApiError::bad_request(ErrorCode::VustPortUnavailable, detail)
            }
            VustConfigError::Storage(err) => ApiError::database(err.to_string()),
        }
    }
}
