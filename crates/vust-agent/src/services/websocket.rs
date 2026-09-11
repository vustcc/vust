//! WebSocket 服务：消息转发与连接管理逻辑。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// WebSocket 推送事件的基础结构，包含事件类型、可序列化的 Payload 以及时间戳。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebsocketEvent {
    pub event_type: String,
    pub payload: Value,
    pub timestamp: DateTime<Utc>,
}

impl WebsocketEvent {
    /// 创建一条新的事件，自动注入当前 UTC 时间。
    pub fn new(event_type: impl Into<String>, payload: Value) -> Self {
        WebsocketEvent {
            event_type: event_type.into(),
            payload,
            timestamp: Utc::now(),
        }
    }
}

/// 为 WebSocket 客户端提供的事件广播通道。
pub fn create_channel() -> broadcast::Sender<WebsocketEvent> {
    broadcast::channel(512).0
}
