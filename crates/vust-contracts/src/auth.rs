//! Web 控制台认证共享契约。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 当前管理员摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "auth/")]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
}

/// 当前服务端会话摘要。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "auth/")]
pub struct AuthSession {
    pub id: String,
    pub expires_at: String,
}

/// 登录与当前会话查询接口的返回载荷。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "auth/")]
pub struct AuthBody {
    pub user: AuthUser,
    pub session: AuthSession,
}

impl AuthBody {
    /// 构建登录或当前会话接口的响应载荷。
    pub fn new(
        user_id: i64,
        username: impl Into<String>,
        session_id: impl Into<String>,
        expires_at: impl Into<String>,
    ) -> Self {
        Self {
            user: AuthUser {
                id: user_id,
                username: username.into(),
            },
            session: AuthSession {
                id: session_id.into(),
                expires_at: expires_at.into(),
            },
        }
    }
}
