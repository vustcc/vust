//! HTTP 边界错误：将应用层错误转换为统一 API 响应。
//!
//! `ApiError` 只保存 HTTP 状态码、跨端错误码与前端消息，不承载领域语义。

use super::response::ApiResponse;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::{borrow::Cow, fmt};
use vust_contracts::api::ErrorCode;

/// HTTP API 统一结果类型。
pub type ApiResult<T> = std::result::Result<T, ApiError>;

fn serialize_status<S>(status: &StatusCode, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_u16(status.as_u16())
}

/// HTTP 边界错误。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiError {
    #[serde(serialize_with = "serialize_status")]
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: Cow<'static, str>,
    pub detail: Option<Cow<'static, str>>,
    #[serde(rename = "messageKey", skip_serializing_if = "Option::is_none")]
    pub message_key: Option<Cow<'static, str>>,
}

impl ApiError {
    /// 创建一个 HTTP 边界错误。
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            detail: None,
            message_key: None,
        }
    }

    /// 附加 i18n 消息 key。
    pub fn with_message_key(mut self, message_key: impl Into<Cow<'static, str>>) -> Self {
        self.message_key = Some(message_key.into());
        self
    }

    /// 附加内部诊断细节；仅记录 5xx 日志，不进入公共响应。
    pub fn with_detail(mut self, detail: impl Into<Cow<'static, str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn bad_request(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn unauthorized(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    pub fn forbidden(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    pub fn not_found(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub fn conflict(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn bad_gateway(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, code, message)
    }

    pub fn gateway_timeout(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, code, message)
    }

    pub fn internal(detail: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalServerError,
            "internal server error",
        )
        .with_detail(detail)
    }

    pub fn database(detail: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::DatabaseError,
            "database error",
        )
        .with_detail(detail)
    }

    pub fn validation(message: impl Into<Cow<'static, str>>) -> Self {
        Self::bad_request(ErrorCode::ValidationFailed, message)
    }

    #[allow(non_upper_case_globals)]
    pub const WrongCredentials: Self = Self {
        status: StatusCode::UNAUTHORIZED,
        code: ErrorCode::AuthWrongCredentials,
        message: Cow::Borrowed("wrong username or password"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const UserNotFound: Self = Self {
        status: StatusCode::UNAUTHORIZED,
        code: ErrorCode::AuthUnauthorized,
        message: Cow::Borrowed("user does not exist"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const MissingCredentials: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: ErrorCode::AuthMissingCredentials,
        message: Cow::Borrowed("username or password is missing"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const TokenCreation: Self = Self {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: ErrorCode::AuthTokenCreationFailed,
        message: Cow::Borrowed("failed to create token"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const InvalidToken: Self = Self {
        status: StatusCode::UNAUTHORIZED,
        code: ErrorCode::AuthInvalidToken,
        message: Cow::Borrowed("invalid token"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const TokenExpired: Self = Self {
        status: StatusCode::UNAUTHORIZED,
        code: ErrorCode::AuthTokenExpired,
        message: Cow::Borrowed("token expired"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const InvalidPassword: Self = Self {
        status: StatusCode::UNAUTHORIZED,
        code: ErrorCode::AuthInvalidPassword,
        message: Cow::Borrowed("invalid password"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const WrongOldPassword: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: ErrorCode::AuthWrongOldPassword,
        message: Cow::Borrowed("old password is incorrect"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const UsernameExists: Self = Self {
        status: StatusCode::CONFLICT,
        code: ErrorCode::AuthUsernameExists,
        message: Cow::Borrowed("username already exists"),
        detail: None,
        message_key: None,
    };

    #[allow(non_snake_case)]
    pub fn ClientVersionMismatch(detail: String) -> Self {
        Self::new(
            StatusCode::UPGRADE_REQUIRED,
            ErrorCode::AuthClientVersionMismatch,
            "client version mismatch",
        )
        .with_detail(detail)
    }

    #[allow(non_snake_case)]
    pub fn BadRequest(message: String) -> Self {
        Self::bad_request(ErrorCode::BadRequest, message)
    }

    #[allow(non_upper_case_globals)]
    pub const NotFound: Self = Self {
        status: StatusCode::NOT_FOUND,
        code: ErrorCode::NodeNotFound,
        message: Cow::Borrowed("resource not found"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const ForbiddenResource: Self = Self {
        status: StatusCode::FORBIDDEN,
        code: ErrorCode::AuthForbidden,
        message: Cow::Borrowed("access to this resource is forbidden"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const ResourceNotFound: Self = Self {
        status: StatusCode::NOT_FOUND,
        code: ErrorCode::FileNotFound,
        message: Cow::Borrowed("resource not found"),
        detail: None,
        message_key: None,
    };

    #[allow(non_upper_case_globals)]
    pub const MissingFileName: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: ErrorCode::FileMissingName,
        message: Cow::Borrowed("uploaded file is missing a file name"),
        detail: None,
        message_key: None,
    };

    #[allow(non_snake_case)]
    pub fn Internal(detail: String) -> Self {
        Self::internal(detail)
    }

    #[allow(non_snake_case)]
    pub fn Io(err: std::io::Error) -> Self {
        Self::bad_request(ErrorCode::FileOperationFailed, "file operation failed")
            .with_detail(err.to_string())
    }

    #[allow(non_snake_case)]
    pub fn Multipart(err: axum::extract::multipart::MultipartError) -> Self {
        Self::bad_request(
            ErrorCode::FileUploadInvalid,
            "failed to parse uploaded data",
        )
        .with_detail(err.to_string())
    }

    #[allow(non_snake_case)]
    pub fn DockerApi(err: bollard::errors::Error) -> Self {
        Self::bad_gateway(ErrorCode::DockerOperationFailed, "Docker operation failed")
            .with_detail(err.to_string())
    }

    #[allow(non_snake_case)]
    pub fn RequestError(err: reqwest::Error) -> Self {
        Self::bad_gateway(ErrorCode::ExternalRequestFailed, "external request failed")
            .with_detail(err.to_string())
    }

    #[allow(non_snake_case)]
    pub fn AxumError(err: axum::http::Error) -> Self {
        Self::internal(err.to_string())
    }

    #[allow(non_snake_case)]
    pub fn Token(err: jsonwebtoken::errors::Error) -> Self {
        Self::bad_request(ErrorCode::AuthInvalidToken, "token error").with_detail(err.to_string())
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code.as_str())
    }
}

impl std::error::Error for ApiError {}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        let msg = err.to_string();
        if msg.contains("UNIQUE constraint failed: nodes.normalized_name") {
            Self::conflict(ErrorCode::NodeAlreadyExists, "node name already exists")
        } else {
            Self::database(msg)
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<axum::extract::multipart::MultipartError> for ApiError {
    fn from(err: axum::extract::multipart::MultipartError) -> Self {
        Self::Multipart(err)
    }
}

impl From<bollard::errors::Error> for ApiError {
    fn from(err: bollard::errors::Error) -> Self {
        Self::DockerApi(err)
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        Self::RequestError(err)
    }
}

impl From<jsonwebtoken::errors::Error> for ApiError {
    fn from(err: jsonwebtoken::errors::Error) -> Self {
        Self::Token(err)
    }
}

impl From<bcrypt::BcryptError> for ApiError {
    fn from(err: bcrypt::BcryptError) -> Self {
        Self::internal(err.to_string())
    }
}

impl From<axum::http::Error> for ApiError {
    fn from(err: axum::http::Error) -> Self {
        Self::AxumError(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!(
                code = self.code.as_str(),
                detail = self.detail.as_deref().unwrap_or(self.code.as_str()),
                "api error"
            );
        }

        let mut resp =
            ApiResponse::<()>::error_with_code(&self.message, self.code, self.status.as_u16());
        if let Some(mk) = self.message_key {
            resp.message_key = Some(mk.into_owned());
        }
        resp.into_response()
    }
}
