//! WebSocket 消息：协议结构与序列化/反序列化。

use serde::{Deserialize, Serialize};

/// WebSocket 客户端发送给服务器的消息类型。
///
/// 客户端通过此枚举定义的消息来请求服务器执行特定操作，例如订阅或取消订阅容器日志。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum ClientWsMessage {
    /// 订阅指定容器的实时日志。
    /// 服务器接收到此消息后，会开始向客户端推送该容器的日志流。
    SubscribeLogs { container_id: String },
    /// 取消订阅指定容器的实时日志。
    /// 服务器接收到此消息后，会停止向客户端推送该容器的日志流。
    UnsubscribeLogs { container_id: String },
}

/// 客户端发往终端 WebSocket 通道的消息。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum ContainerTerminalClientMessage {
    /// 启动一个容器终端会话。
    TerminalStart(ContainerTerminalStartPayload),
    /// 向终端会话写入输入数据。
    TerminalInput(ContainerTerminalInputPayload),
    /// 调整终端窗口大小。
    TerminalResize(ContainerTerminalResizePayload),
    /// 关闭终端会话。
    TerminalClose(ContainerTerminalClosePayload),
}

/// 包含日志数据，用于传输日志快照或追加日志行。
#[derive(Debug, Serialize)]
pub struct LogPayload {
    /// 日志所属的容器 ID。
    pub container_id: String,
    /// 日志内容的行数组。
    pub lines: Vec<String>,
}

/// 包含通用消息和其所属容器 ID。
#[derive(Debug, Serialize)]
pub struct MessagePayload {
    /// 消息所属的容器 ID。
    pub container_id: String,
    /// 具体的消息内容。
    pub message: String,
}

/// 终端启动请求载荷。
#[derive(Debug, Deserialize)]
pub struct ContainerTerminalStartPayload {
    /// 目标容器 ID。
    pub container_id: String,
    /// 期望 shell：`bash` 或 `sh`。
    pub shell: String,
    /// 终端列数。
    pub cols: u16,
    /// 终端行数。
    pub rows: u16,
}

/// 终端输入请求载荷。
#[derive(Debug, Deserialize)]
pub struct ContainerTerminalInputPayload {
    /// 终端会话 ID。
    pub session_id: String,
    /// 输入数据，原样转发到容器 stdin。
    pub data: String,
}

/// 终端尺寸变更请求载荷。
#[derive(Debug, Deserialize)]
pub struct ContainerTerminalResizePayload {
    /// 终端会话 ID。
    pub session_id: String,
    /// 终端列数。
    pub cols: u16,
    /// 终端行数。
    pub rows: u16,
}

/// 终端关闭请求载荷。
#[derive(Debug, Deserialize)]
pub struct ContainerTerminalClosePayload {
    /// 终端会话 ID。
    pub session_id: String,
}

/// WebSocket 服务器发送给客户端的消息类型。
///
/// 服务器通过此枚举定义的消息来向客户端发送各种事件，例如日志数据、通知或错误信息。
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
pub enum ServerWsMessage {
    /// 包含容器的初始日志快照，通常在订阅成功后发送。
    Snapshot(LogPayload),
    /// 包含容器新生成的日志行，用于实时追加。
    Append(LogPayload),
    /// 通知客户端指定容器的日志流已结束（例如容器已停止）。
    End(MessagePayload),
    /// 通知客户端在获取或处理指定容器日志时发生了错误。
    Error(MessagePayload),
    /// 心跳消息，用于保持 WebSocket 连接活跃。不包含 payload。
    Heartbeat,
}

/// 服务端发送到终端 WebSocket 通道的消息。
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
pub enum ContainerTerminalServerMessage {
    /// 终端会话启动成功。
    TerminalStarted(ContainerTerminalStartedPayload),
    /// 终端实时输出。
    TerminalOutput(ContainerTerminalOutputPayload),
    /// 终端会话结束。
    TerminalExit(ContainerTerminalExitPayload),
    /// 终端会话错误。
    TerminalError(ContainerTerminalErrorPayload),
}

/// 终端启动成功载荷。
#[derive(Debug, Serialize)]
pub struct ContainerTerminalStartedPayload {
    /// 会话 ID。
    pub session_id: String,
    /// 实际使用的 shell（可能触发 fallback）。
    pub shell: String,
}

/// 终端输出载荷。
#[derive(Debug, Serialize)]
pub struct ContainerTerminalOutputPayload {
    /// 会话 ID。
    pub session_id: String,
    /// 原始终端输出。
    pub data: String,
}

/// 终端退出载荷。
#[derive(Debug, Serialize)]
pub struct ContainerTerminalExitPayload {
    /// 会话 ID。
    pub session_id: String,
    /// 退出码。
    pub exit_code: Option<i64>,
}

/// 终端错误载荷。
#[derive(Debug, Serialize)]
pub struct ContainerTerminalErrorPayload {
    /// 错误消息。
    pub message: String,
}
