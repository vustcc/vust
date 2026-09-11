//! 类型与错误：统一的 API 响应、错误与通用别名。

pub use crate::errors::{
    AgentClientError, AuthError, ControllerError, NodeError, TaskError, VustConfigError,
};
use uuid::Uuid;
pub use vust_api::error::{ApiError, ApiResult};
pub use vust_api::response::ApiResponse;
pub use vust_contracts::types::agent_socket_path;

/// 生成 UUID v7 字符串，作为运行时主键与链路标识的统一生成入口。
pub fn new_uuid_v7() -> String {
    Uuid::now_v7().to_string()
}
