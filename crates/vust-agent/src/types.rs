//! 类型与错误：统一的 API 响应、错误与通用别名。

pub use crate::errors::AgentError;
pub use vust_api::error::{ApiError, ApiResult};
pub use vust_api::response::ApiResponse;

/// 表示运行模式，用于区分本地与远程连接逻辑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Local,
    Remote,
}

impl AgentMode {
    /// 返回用于配置与序列化的稳定字符串表示。
    pub fn as_str(self) -> &'static str {
        match self {
            AgentMode::Local => "local",
            AgentMode::Remote => "remote",
        }
    }
}

impl std::str::FromStr for AgentMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "local" => Ok(AgentMode::Local),
            "remote" => Ok(AgentMode::Remote),
            other => Err(anyhow::anyhow!("Unknown agent mode: {}", other)),
        }
    }
}
