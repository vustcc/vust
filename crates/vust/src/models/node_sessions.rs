//! 节点会话模型：`node_sessions` 表的数据结构与基础仓储方法。

use crate::state::DbPool;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Agent 命令端点的基础设施传输类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommandTransport {
    Uds,
    Https,
}

impl AgentCommandTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uds => "uds",
            Self::Https => "https",
        }
    }
}

/// 活动会话对应的命令端点与解密凭据。
#[derive(Debug, Clone)]
pub struct AgentCommandAccess {
    pub session: NodeSessionRecord,
    pub transport: AgentCommandTransport,
    pub credential: String,
}

/// 判断会话是否仍处于活跃状态且租约尚未过期。
pub fn has_live_lease(session: &NodeSessionRecord) -> bool {
    session.status == "active"
        && chrono::DateTime::parse_from_rfc3339(&session.lease_expires_at)
            .map(|expires_at| expires_at.with_timezone(&Utc) > Utc::now())
            .unwrap_or(false)
}

/// 节点运行时会话记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NodeSessionRecord {
    pub session_id: String,
    pub node_id: String,
    pub agent_id: String,
    pub lease_id: String,
    pub advertise_addr: Option<String>,
    pub listen_addr: Option<String>,
    pub listen_port: Option<i64>,
    pub server_name: Option<String>,
    pub registered_at: String,
    pub lease_expires_at: String,
    pub last_heartbeat_at: Option<String>,
    pub heartbeat_sequence: i64,
    pub last_seen_at: Option<String>,
    pub status: String,
    pub close_reason: Option<String>,
    pub closed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 新增节点会话记录。
pub async fn insert_node_session(pool: &DbPool, record: &NodeSessionRecord) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO node_sessions (
            session_id,
            node_id,
            agent_id,
            lease_id,
            advertise_addr,
            listen_addr,
            listen_port,
            server_name,
            registered_at,
            lease_expires_at,
            last_heartbeat_at,
            heartbeat_sequence,
            last_seen_at,
            status,
            close_reason,
            closed_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.session_id)
    .bind(&record.node_id)
    .bind(&record.agent_id)
    .bind(&record.lease_id)
    .bind(&record.advertise_addr)
    .bind(&record.listen_addr)
    .bind(record.listen_port)
    .bind(&record.server_name)
    .bind(&record.registered_at)
    .bind(&record.lease_expires_at)
    .bind(&record.last_heartbeat_at)
    .bind(record.heartbeat_sequence)
    .bind(&record.last_seen_at)
    .bind(&record.status)
    .bind(&record.close_reason)
    .bind(&record.closed_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 查询节点的活跃会话。
pub async fn get_active_session_by_node_id(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<Option<NodeSessionRecord>> {
    sqlx::query_as::<_, NodeSessionRecord>(
        r#"
        SELECT
            session_id,
            node_id,
            agent_id,
            lease_id,
            advertise_addr,
            listen_addr,
            listen_port,
            server_name,
            registered_at,
            lease_expires_at,
            last_heartbeat_at,
            heartbeat_sequence,
            last_seen_at,
            status,
            close_reason,
            closed_at,
            created_at,
            updated_at
        FROM node_sessions
        WHERE node_id = ?
          AND status = 'active'
        LIMIT 1
        "#,
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
}

/// 查询节点最近一条会话记录。
pub async fn get_latest_session_by_node_id(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<Option<NodeSessionRecord>> {
    sqlx::query_as::<_, NodeSessionRecord>(
        r#"
        SELECT
            session_id,
            node_id,
            agent_id,
            lease_id,
            advertise_addr,
            listen_addr,
            listen_port,
            server_name,
            registered_at,
            lease_expires_at,
            last_heartbeat_at,
            heartbeat_sequence,
            last_seen_at,
            status,
            close_reason,
            closed_at,
            created_at,
            updated_at
        FROM node_sessions
        WHERE node_id = ?
        ORDER BY registered_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
}

/// 查询节点最近若干条会话记录。
pub async fn list_sessions_by_node_id(
    pool: &DbPool,
    node_id: &str,
    limit: i64,
) -> sqlx::Result<Vec<NodeSessionRecord>> {
    sqlx::query_as::<_, NodeSessionRecord>(
        r#"
        SELECT
            session_id,
            node_id,
            agent_id,
            lease_id,
            advertise_addr,
            listen_addr,
            listen_port,
            server_name,
            registered_at,
            lease_expires_at,
            last_heartbeat_at,
            heartbeat_sequence,
            last_seen_at,
            status,
            close_reason,
            closed_at,
            created_at,
            updated_at
        FROM node_sessions
        WHERE node_id = ?
        ORDER BY registered_at DESC, created_at DESC
        LIMIT ?
        "#,
    )
    .bind(node_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// 通过 `agent_id` 查询活跃会话。
pub async fn get_active_session_by_agent_id(
    pool: &DbPool,
    agent_id: &str,
) -> sqlx::Result<Option<NodeSessionRecord>> {
    sqlx::query_as::<_, NodeSessionRecord>(
        r#"
        SELECT
            session_id,
            node_id,
            agent_id,
            lease_id,
            advertise_addr,
            listen_addr,
            listen_port,
            server_name,
            registered_at,
            lease_expires_at,
            last_heartbeat_at,
            heartbeat_sequence,
            last_seen_at,
            status,
            close_reason,
            closed_at,
            created_at,
            updated_at
        FROM node_sessions
        WHERE agent_id = ?
          AND status = 'active'
        LIMIT 1
        "#,
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
}

/// 通过节点或运行时身份解析活跃会话的 TLS 端点。
pub async fn resolve_active_session_endpoint(
    pool: &DbPool,
    target: &str,
) -> sqlx::Result<Option<String>> {
    let by_agent = get_active_session_by_agent_id(pool, target).await?;
    let session = match by_agent {
        Some(session) => Some(session),
        None => get_active_session_by_node_id(pool, target).await?,
    };
    Ok(
        session.and_then(|record| match (record.advertise_addr, record.listen_port) {
            (Some(addr), Some(port)) => Some(format!("https://{}:{}", addr, port)),
            _ => None,
        }),
    )
}

/// 保存会话命令端点和加密凭据。
pub async fn upsert_session_command_access(
    pool: &DbPool,
    session_id: &str,
    transport: AgentCommandTransport,
    credential: &str,
) -> anyhow::Result<()> {
    let ciphertext = crate::crypto::encrypt_bytes(credential.as_bytes())
        .map_err(|err| anyhow::anyhow!("failed to encrypt Agent command credential: {err}"))?;
    sqlx::query(
        r#"
        INSERT INTO node_session_command_access (session_id, transport, credential_ciphertext)
        VALUES (?, ?, ?)
        ON CONFLICT(session_id) DO UPDATE SET
            transport = excluded.transport,
            credential_ciphertext = excluded.credential_ciphertext,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        "#,
    )
    .bind(session_id)
    .bind(transport.as_str())
    .bind(ciphertext)
    .execute(pool)
    .await?;
    Ok(())
}

/// 解析目标节点的活动命令端点和凭据。
pub async fn resolve_active_command_access(
    pool: &DbPool,
    target: &str,
) -> anyhow::Result<Option<AgentCommandAccess>> {
    let session = match get_active_session_by_agent_id(pool, target).await? {
        Some(session) => session,
        None => match get_active_session_by_node_id(pool, target).await? {
            Some(session) => session,
            None => return Ok(None),
        },
    };
    if !has_live_lease(&session) {
        return Ok(None);
    }
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT transport, credential_ciphertext FROM node_session_command_access WHERE session_id = ?",
    )
    .bind(&session.session_id)
    .fetch_optional(pool)
    .await?;
    let Some((transport, ciphertext)) = row else {
        return Ok(None);
    };
    let transport = match transport.as_str() {
        "uds" => AgentCommandTransport::Uds,
        "https" => AgentCommandTransport::Https,
        other => anyhow::bail!("invalid Agent command transport: {other}"),
    };
    let credential = crate::crypto::decrypt_bytes(&ciphertext)
        .map_err(|err| anyhow::anyhow!("failed to decrypt Agent command credential: {err}"))
        .and_then(|bytes| String::from_utf8(bytes).map_err(anyhow::Error::from))?;
    Ok(Some(AgentCommandAccess {
        session,
        transport,
        credential,
    }))
}

/// 通过 `session_id` 读取会话。
pub async fn get_session_by_id(
    pool: &DbPool,
    session_id: &str,
) -> sqlx::Result<Option<NodeSessionRecord>> {
    sqlx::query_as::<_, NodeSessionRecord>(
        r#"
        SELECT
            session_id,
            node_id,
            agent_id,
            lease_id,
            advertise_addr,
            listen_addr,
            listen_port,
            server_name,
            registered_at,
            lease_expires_at,
            last_heartbeat_at,
            heartbeat_sequence,
            last_seen_at,
            status,
            close_reason,
            closed_at,
            created_at,
            updated_at
        FROM node_sessions
        WHERE session_id = ?
        LIMIT 1
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
}

/// 查询所有已过期但仍处于活跃状态的会话。
pub async fn list_expired_active_sessions(pool: &DbPool) -> sqlx::Result<Vec<NodeSessionRecord>> {
    let now = Utc::now().to_rfc3339();
    sqlx::query_as::<_, NodeSessionRecord>(
        r#"
        SELECT
            session_id,
            node_id,
            agent_id,
            lease_id,
            advertise_addr,
            listen_addr,
            listen_port,
            server_name,
            registered_at,
            lease_expires_at,
            last_heartbeat_at,
            heartbeat_sequence,
            last_seen_at,
            status,
            close_reason,
            closed_at,
            created_at,
            updated_at
        FROM node_sessions
        WHERE status = 'active'
          AND lease_expires_at < ?
        ORDER BY lease_expires_at ASC
        "#,
    )
    .bind(now)
    .fetch_all(pool)
    .await
}

/// 关闭节点当前活跃会话。
pub async fn close_active_sessions(
    pool: &DbPool,
    node_id: &str,
    close_reason: &str,
    next_status: &str,
) -> sqlx::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE node_sessions
        SET
            status = ?,
            close_reason = ?,
            closed_at = ?
        WHERE node_id = ?
          AND status = 'active'
        "#,
    )
    .bind(next_status)
    .bind(close_reason)
    .bind(&now)
    .bind(node_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// 按 `session_id` 关闭会话；如果会话已不是活跃态则不再改写。
pub async fn close_session_by_id(
    pool: &DbPool,
    session_id: &str,
    close_reason: &str,
    next_status: &str,
) -> sqlx::Result<u64> {
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        r#"
        UPDATE node_sessions
        SET
            status = ?,
            close_reason = ?,
            closed_at = ?
        WHERE session_id = ?
          AND status = 'active'
        "#,
    )
    .bind(next_status)
    .bind(close_reason)
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// 根据 heartbeat 与 sequence 刷新租约。
pub async fn refresh_session_lease_with_sequence(
    pool: &DbPool,
    session_id: &str,
    lease_id: &str,
    lease_ttl_seconds: i64,
    sequence: i64,
) -> sqlx::Result<bool> {
    let now = Utc::now();
    let result = sqlx::query(
        r#"
        UPDATE node_sessions
        SET
            lease_id = ?,
            lease_expires_at = ?,
            last_heartbeat_at = ?,
            heartbeat_sequence = ?,
            last_seen_at = ?,
            status = 'active'
        WHERE session_id = ?
          AND heartbeat_sequence < ?
        "#,
    )
    .bind(lease_id)
    .bind((now + Duration::seconds(lease_ttl_seconds)).to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(sequence)
    .bind(now.to_rfc3339())
    .bind(session_id)
    .bind(sequence)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// 兼容模式：根据 heartbeat 刷新租约，不校验 sequence。
pub async fn refresh_session_lease(
    pool: &DbPool,
    session_id: &str,
    lease_id: &str,
    lease_ttl_seconds: i64,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        r#"
        UPDATE node_sessions
        SET
            lease_id = ?,
            lease_expires_at = ?,
            last_heartbeat_at = ?,
            heartbeat_sequence = heartbeat_sequence + 1,
            last_seen_at = ?,
            status = 'active'
        WHERE session_id = ?
        "#,
    )
    .bind(lease_id)
    .bind((now + Duration::seconds(lease_ttl_seconds)).to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(session_id)
    .execute(pool)
    .await?;

    Ok(())
}
