//! 用户模型：单用户管理员信息的数据结构与数据库访问。

use serde::Serialize;
use sqlx::FromRow;

/// 数据库中的管理员用户模型，用于登录认证和审计关联。
#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    pub username: String,
    /// 存储 bcrypt 哈希后的密码
    pub password_hash: String,
    pub status: String,
    pub password_changed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl User {
    /// 返回管理员账号是否处于可登录状态。
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}
