//! Agent 统一运行时上下文：将节点拓扑差异限制在基础设施层。

use crate::config;
use crate::models::identity::load_or_init_identity;
use crate::state::DbPool;
use crate::types::AgentMode;

/// Master 调用当前 Agent 的命令传输。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTransport {
    Uds,
    Https,
}

/// Compose 镜像准备所需的基础设施策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageResolutionStrategy {
    LocalEngine,
    ControllerMediated,
}

/// 业务模块使用的统一 Agent 运行时事实。
#[derive(Debug, Clone)]
pub struct AgentRuntimeContext {
    pub node_id: String,
    pub controller_url: String,
    pub command_transport: CommandTransport,
    pub suite_command_base_url: Option<String>,
    pub image_resolution: ImageResolutionStrategy,
}

/// 从持久身份和运行配置构造统一上下文。
pub async fn load(pool: &DbPool) -> anyhow::Result<AgentRuntimeContext> {
    let identity = load_or_init_identity(pool, config::get()).await?;
    match identity.mode {
        AgentMode::Local => Ok(AgentRuntimeContext {
            node_id: "local".to_string(),
            controller_url: config::local_controller_url()?,
            command_transport: CommandTransport::Uds,
            suite_command_base_url: None,
            image_resolution: ImageResolutionStrategy::LocalEngine,
        }),
        AgentMode::Remote => Ok(AgentRuntimeContext {
            node_id: identity
                .agent_id
                .ok_or_else(|| anyhow::anyhow!("Agent node identity is unavailable"))?,
            controller_url: identity
                .vust_url
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("Controller URL is unavailable"))?,
            command_transport: CommandTransport::Https,
            suite_command_base_url: Some(format!(
                "https://host.docker.internal:{}",
                identity
                    .listen_addr
                    .as_deref()
                    .unwrap_or(config::DEFAULT_AGENT_LISTEN_ADDR)
                    .rsplit(':')
                    .next()
                    .and_then(|value| value.parse::<u16>().ok())
                    .unwrap_or(7311)
            )),
            image_resolution: ImageResolutionStrategy::ControllerMediated,
        }),
    }
}
