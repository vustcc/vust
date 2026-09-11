//! 节点运行时服务：处理 enrollment、register、heartbeat 与 deregister。

use crate::models::node_enrollments::{get_enrollment_by_token_hash, mark_enrollment_used};
use crate::models::node_identities::{
    NodeIdentityRecord, get_identity_by_agent_id, upsert_node_identity,
};
use crate::models::node_sessions::{
    NodeSessionRecord, close_active_sessions, close_session_by_id, get_active_session_by_agent_id,
    get_active_session_by_node_id, get_session_by_id, insert_node_session,
    refresh_session_lease_with_sequence,
};
use crate::models::nodes::{NodeStatus, get_node_by_id, update_node_status};
use crate::services::node_state_machine::{
    NodeStateEvent, apply_node_state_transition, transition_to_conflict, transition_to_offline,
    transition_to_online,
};
use crate::state::DbPool;
use crate::types::{ApiError, new_uuid_v7};
use chrono::{Duration, Utc};
use ring::digest;

/// runtime 默认租约 TTL。
pub const DEFAULT_LEASE_TTL_SECONDS: i64 = 30;
/// runtime 默认心跳间隔。
pub const DEFAULT_HEARTBEAT_INTERVAL_SECONDS: i64 = 10;

/// 创建开发期内置的本地节点和 Agent 身份，使其进入统一 runtime session。
pub async fn ensure_local_runtime_identity(pool: &DbPool) -> Result<(), ApiError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO nodes (
            node_id, name, normalized_name, group_name, status, desired_role,
            schedulable, metadata, created_at, updated_at
        ) VALUES ('local', 'Local Node', 'local-node', 'default', 'registered',
                  'agent', 1, '{"systemManaged":true}', ?, ?)
        "#,
    )
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO node_identities (
            identity_id, node_id, agent_id, certificate_fingerprint,
            certificate_status, public_key_algorithm, certificate_issued_at,
            created_at, updated_at
        ) VALUES (?, 'local', 'local', 'local-runtime-agent',
                  'active', 'ed25519', ?, ?, ?)
        "#,
    )
    .bind(new_uuid_v7())
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// enrollment 成功结果。
#[derive(Debug, Clone)]
pub struct RuntimeEnrollResult {
    pub node_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub lease_id: String,
}

/// register 成功结果。
#[derive(Debug, Clone)]
pub struct RuntimeRegisterResult {
    pub node_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub lease_id: String,
    pub session_replaced: bool,
}

/// heartbeat 处理结果。
#[derive(Debug, Clone)]
pub struct RuntimeHeartbeatResult {
    pub lease_id: String,
    pub sequence_ignored: bool,
}

/// rotate-certificate 处理结果。
#[derive(Debug, Clone)]
pub struct RuntimeRotateCertificateResult {
    pub agent_id: String,
    pub old_certificate_retire_after: String,
}

fn token_hash(token: &str) -> String {
    let digest = digest::digest(&digest::SHA256, token.as_bytes());
    hex::encode(digest.as_ref())
}

fn fingerprint_from_payload(agent_id: &str, csr_pem: Option<&str>) -> String {
    let seed = format!("{agent_id}:{}", csr_pem.unwrap_or_default());
    let digest = digest::digest(&digest::SHA256, seed.as_bytes());
    format!("SHA256:{}", hex::encode(digest.as_ref()))
}

fn new_session(
    node_id: &str,
    agent_id: &str,
    advertise_addr: Option<String>,
    listen_port: Option<i64>,
) -> NodeSessionRecord {
    let now = Utc::now();
    NodeSessionRecord {
        session_id: new_uuid_v7(),
        node_id: node_id.to_string(),
        agent_id: agent_id.to_string(),
        lease_id: new_uuid_v7(),
        advertise_addr,
        listen_addr: None,
        listen_port,
        server_name: None,
        registered_at: now.to_rfc3339(),
        lease_expires_at: (now + Duration::seconds(DEFAULT_LEASE_TTL_SECONDS)).to_rfc3339(),
        last_heartbeat_at: Some(now.to_rfc3339()),
        heartbeat_sequence: 0,
        last_seen_at: Some(now.to_rfc3339()),
        status: "active".to_string(),
        close_reason: None,
        closed_at: None,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    }
}

async fn mark_node_online(
    pool: &DbPool,
    node_id: &str,
    update_registered_at: bool,
) -> sqlx::Result<()> {
    let previous = get_node_by_id(pool, node_id).await?;
    transition_to_online(pool, node_id).await?;
    let now = Utc::now().to_rfc3339();
    let registered_at = if update_registered_at {
        Some(now.clone())
    } else {
        None
    };
    sqlx::query(
        r#"
        UPDATE nodes
        SET
            registered_at = COALESCE(?, registered_at),
            last_seen_at = ?
        WHERE node_id = ?
        "#,
    )
    .bind(registered_at)
    .bind(&now)
    .bind(node_id)
    .execute(pool)
    .await?;

    if previous
        .as_ref()
        .is_some_and(|node| node.status == "offline")
    {
        crate::services::logging::OperationEventBuilder::new(
            "system",
            "node_recovered",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        )
        .target_type("node")
        .target_id(node_id)
        .target_display_name(previous.as_ref().map_or(node_id, |node| node.name.as_str()))
        .set_success()
        .metadata(serde_json::json!({
            "nodeId": node_id,
            "nodeName": previous.as_ref().map(|node| node.name.as_str()),
        }))
        .finish(pool);
    }

    Ok(())
}

/// 当节点已无活跃会话时，将其收敛为离线状态。
pub async fn settle_node_after_session_loss(pool: &DbPool, node_id: &str) -> sqlx::Result<()> {
    if crate::models::node_sessions::get_active_session_by_node_id(pool, node_id)
        .await?
        .is_some()
    {
        return Ok(());
    }

    let Some(node) = get_node_by_id(pool, node_id).await? else {
        return Ok(());
    };
    let Some(current) = NodeStatus::parse(&node.status) else {
        return Ok(());
    };
    if apply_node_state_transition(current, NodeStateEvent::LeaseExpired).is_ok() {
        transition_to_offline(pool, node_id).await?;
    } else {
        update_node_status(pool, node_id, NodeStatus::Offline).await?;
    }

    Ok(())
}

/// 首次 enrollment：校验 token、写 identity、创建活跃 session。
pub async fn enroll_node(
    pool: &DbPool,
    enrollment_token: &str,
    advertise_addr: Option<String>,
    listen_port: Option<i64>,
    public_key_algorithm: &str,
    csr_pem: Option<&str>,
) -> Result<RuntimeEnrollResult, ApiError> {
    let token_hash = token_hash(enrollment_token);
    let enrollment = get_enrollment_by_token_hash(pool, &token_hash)
        .await?
        .ok_or_else(|| ApiError::BadRequest("invalid enrollment token".to_string()))?;
    if enrollment.token_status == "revoked" || enrollment.revoked_at.is_some() {
        return Err(ApiError::BadRequest(
            "enrollment token is revoked".to_string(),
        ));
    }
    let expires_at = chrono::DateTime::parse_from_rfc3339(&enrollment.expires_at)
        .map_err(|_| ApiError::BadRequest("invalid enrollment token expiry".to_string()))?
        .with_timezone(&Utc);
    if expires_at <= Utc::now() {
        return Err(ApiError::BadRequest(
            "enrollment token is expired".to_string(),
        ));
    }

    let node_id = enrollment.node_id.clone();
    let agent_id = node_id.clone();
    if enrollment.token_status != "issued" {
        if enrollment.token_status == "used"
            && let Some(active) = get_active_session_by_node_id(pool, &node_id).await?
        {
            return Ok(RuntimeEnrollResult {
                node_id,
                agent_id,
                session_id: active.session_id,
                lease_id: active.lease_id,
            });
        }
        return Err(ApiError::BadRequest(
            "enrollment token is not available".to_string(),
        ));
    }
    if get_active_session_by_node_id(pool, &node_id)
        .await?
        .is_some()
    {
        let _ = transition_to_conflict(pool, &node_id).await;
        return Err(ApiError::BadRequest(
            "runtime session conflict for enrollment".to_string(),
        ));
    }

    let fingerprint = fingerprint_from_payload(&agent_id, csr_pem);
    upsert_node_identity(
        pool,
        &NodeIdentityRecord {
            identity_id: new_uuid_v7(),
            node_id: node_id.clone(),
            agent_id: agent_id.clone(),
            certificate_serial_number: None,
            certificate_fingerprint: fingerprint,
            certificate_status: "active".to_string(),
            public_key_algorithm: public_key_algorithm.to_string(),
            certificate_issued_at: Some(Utc::now().to_rfc3339()),
            certificate_expires_at: None,
            rotated_at: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        },
    )
    .await?;

    let session = new_session(&node_id, &agent_id, advertise_addr, listen_port);
    insert_node_session(pool, &session).await?;
    mark_enrollment_used(pool, &enrollment.enrollment_id).await?;
    mark_node_online(pool, &node_id, true).await?;

    Ok(RuntimeEnrollResult {
        node_id,
        agent_id,
        session_id: session.session_id,
        lease_id: session.lease_id,
    })
}

/// 非首次 register：要求已有 identity，重建活跃 session。
pub async fn register_node(
    pool: &DbPool,
    agent_id: &str,
    advertise_addr: Option<String>,
    listen_port: Option<i64>,
) -> Result<RuntimeRegisterResult, ApiError> {
    let identity = get_identity_by_agent_id(pool, agent_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("unknown agent identity".to_string()))?;
    let session_replaced = get_active_session_by_agent_id(pool, agent_id)
        .await?
        .is_some();
    close_active_sessions(
        pool,
        &identity.node_id,
        "superseded_by_re_registration",
        "superseded",
    )
    .await?;
    let session = new_session(&identity.node_id, agent_id, advertise_addr, listen_port);
    insert_node_session(pool, &session).await?;
    mark_node_online(pool, &identity.node_id, false).await?;

    Ok(RuntimeRegisterResult {
        node_id: identity.node_id,
        agent_id: agent_id.to_string(),
        session_id: session.session_id,
        lease_id: session.lease_id,
        session_replaced,
    })
}

/// heartbeat：刷新租约与最近可见时间。
#[allow(clippy::too_many_arguments)]
pub async fn heartbeat(
    pool: &DbPool,
    agent_id: &str,
    session_id: &str,
    lease_id: &str,
    sequence: i64,
    advertise_addr: Option<String>,
    listen_port: Option<i64>,
    resource: Option<vust_contracts::types::HostSystemSummary>,
) -> Result<RuntimeHeartbeatResult, ApiError> {
    if sequence <= 0 {
        return Err(ApiError::BadRequest(
            "heartbeat sequence must be positive".to_string(),
        ));
    }
    let session = get_session_by_id(pool, session_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("runtime session not found".to_string()))?;
    if session.status != "active" {
        return Err(ApiError::BadRequest(
            "runtime session is not active".to_string(),
        ));
    }
    if session.agent_id != agent_id {
        return Err(ApiError::BadRequest(
            "runtime identity mismatch".to_string(),
        ));
    }
    if session.lease_id != lease_id {
        return Err(ApiError::BadRequest("runtime lease mismatch".to_string()));
    }

    let applied = refresh_session_lease_with_sequence(
        pool,
        session_id,
        lease_id,
        DEFAULT_LEASE_TTL_SECONDS,
        sequence,
    )
    .await?;
    if !applied {
        return Ok(RuntimeHeartbeatResult {
            lease_id: session.lease_id,
            sequence_ignored: true,
        });
    }
    if advertise_addr.is_some() || listen_port.is_some() {
        sqlx::query(
            r#"
            UPDATE node_sessions
            SET
                advertise_addr = COALESCE(?, advertise_addr),
                listen_port = COALESCE(?, listen_port)
            WHERE session_id = ?
            "#,
        )
        .bind(advertise_addr)
        .bind(listen_port)
        .bind(session_id)
        .execute(pool)
        .await?;
    }

    if let Some(res) = resource {
        let current_metadata_raw: Option<String> =
            sqlx::query_scalar("SELECT metadata FROM nodes WHERE node_id = ?")
                .bind(&session.node_id)
                .fetch_optional(pool)
                .await?;

        let mut meta = match current_metadata_raw
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(serde_json::Value::Object(map)) => serde_json::Value::Object(map),
            _ => serde_json::json!({}),
        };
        if let Some(obj) = meta.as_object_mut() {
            obj.insert(
                "resource".to_string(),
                serde_json::to_value(res).unwrap_or(serde_json::Value::Null),
            );
        }

        sqlx::query(
            r#"
            UPDATE nodes
            SET metadata = ?, last_seen_at = ?
            WHERE node_id = ?
            "#,
        )
        .bind(serde_json::to_string(&meta).unwrap_or_default())
        .bind(Utc::now().to_rfc3339())
        .bind(&session.node_id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            r#"
            UPDATE nodes
            SET last_seen_at = ?
            WHERE node_id = ?
            "#,
        )
        .bind(Utc::now().to_rfc3339())
        .bind(&session.node_id)
        .execute(pool)
        .await?;
    }

    Ok(RuntimeHeartbeatResult {
        lease_id: session.lease_id,
        sequence_ignored: false,
    })
}

/// deregister：关闭活跃会话并标记节点离线。
pub async fn deregister(pool: &DbPool, session_id: &str, reason: &str) -> Result<(), ApiError> {
    let Some(session) = get_session_by_id(pool, session_id).await? else {
        return Ok(());
    };
    if session.status != "active" {
        return Ok(());
    }
    let _ = close_session_by_id(pool, session_id, reason, "closed").await?;
    settle_node_after_session_loss(pool, &session.node_id).await?;
    Ok(())
}

/// rotate-certificate：校验会话与身份后更新证书绑定。
pub async fn rotate_certificate(
    pool: &DbPool,
    agent_id: &str,
    session_id: &str,
    current_certificate_fingerprint: Option<&str>,
    public_key_algorithm: &str,
    csr_pem: Option<&str>,
) -> Result<RuntimeRotateCertificateResult, ApiError> {
    let session = get_session_by_id(pool, session_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("runtime session not found".to_string()))?;
    if session.status != "active" {
        return Err(ApiError::BadRequest(
            "runtime session is not active".to_string(),
        ));
    }
    if session.agent_id != agent_id {
        return Err(ApiError::BadRequest(
            "runtime identity mismatch".to_string(),
        ));
    }
    let identity = get_identity_by_agent_id(pool, agent_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("unknown agent identity".to_string()))?;
    if let Some(expected) = current_certificate_fingerprint
        && expected != identity.certificate_fingerprint
    {
        return Err(ApiError::BadRequest(
            "certificate fingerprint mismatch".to_string(),
        ));
    }

    let now = Utc::now();
    upsert_node_identity(
        pool,
        &NodeIdentityRecord {
            identity_id: identity.identity_id,
            node_id: identity.node_id,
            agent_id: identity.agent_id,
            certificate_serial_number: identity.certificate_serial_number,
            certificate_fingerprint: fingerprint_from_payload(agent_id, csr_pem),
            certificate_status: "active".to_string(),
            public_key_algorithm: public_key_algorithm.to_string(),
            certificate_issued_at: Some(now.to_rfc3339()),
            certificate_expires_at: None,
            rotated_at: Some(now.to_rfc3339()),
            created_at: identity.created_at,
            updated_at: now.to_rfc3339(),
        },
    )
    .await?;

    Ok(RuntimeRotateCertificateResult {
        agent_id: agent_id.to_string(),
        old_certificate_retire_after: (now + Duration::minutes(5)).to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_LEASE_TTL_SECONDS, deregister, enroll_node, heartbeat, register_node,
        rotate_certificate, token_hash,
    };
    use crate::models::node_enrollments::{
        NodeEnrollmentRecord, get_enrollment_by_token_hash, insert_node_enrollment,
    };
    use crate::models::node_identities::get_identity_by_agent_id;
    use crate::models::node_runtime_client::NodeRuntimeClient;
    use crate::models::node_sessions::{
        get_active_session_by_agent_id, get_session_by_id, list_sessions_by_node_id,
        resolve_active_session_endpoint,
    };
    use crate::models::nodes::{
        NewNodeRecord, NodeStatus, get_node_by_id, insert_node, update_node_status,
    };
    use crate::services::node_enrollment::issue_enrollment_token;
    use crate::services::node_session_reaper::reap_expired_sessions;
    use crate::test_support::setup_test_db;
    use chrono::{DateTime, Duration, Utc};
    use uuid::Uuid;

    async fn insert_test_node(pool: &crate::state::DbPool, node_id: &str) {
        insert_node(
            pool,
            &NewNodeRecord {
                node_id: node_id.to_string(),
                tenant_id: None,
                name: format!("node-{node_id}"),
                normalized_name: format!("node-{node_id}"),
                group_name: "default".to_string(),
                labels: "[]".to_string(),
                description: None,
                desired_role: None,
                schedulable: true,
                metadata: "{}".to_string(),
            },
        )
        .await
        .unwrap();
        update_node_status(pool, node_id, NodeStatus::AwaitingRegistration)
            .await
            .unwrap();
    }

    async fn insert_test_enrollment(
        pool: &crate::state::DbPool,
        node_id: &str,
        raw_token: &str,
    ) -> NodeEnrollmentRecord {
        let now = Utc::now();
        let record = NodeEnrollmentRecord {
            enrollment_id: Uuid::new_v4().to_string(),
            node_id: node_id.to_string(),
            token_hash: token_hash(raw_token),
            token_status: "issued".to_string(),
            expires_at: (now + Duration::minutes(10)).to_rfc3339(),
            first_used_at: None,
            last_used_at: None,
            revoked_at: None,
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
        };
        insert_node_enrollment(pool, &record).await.unwrap();
        record
    }

    #[tokio::test]
    async fn enroll_node_creates_identity_session_and_marks_token_used() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "enrollment-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let result = enroll_node(
            &pool,
            token,
            Some("10.0.0.8".to_string()),
            Some(7311),
            "ed25519",
            Some("csr"),
        )
        .await
        .unwrap();

        assert_eq!(result.node_id, node_id);
        assert_eq!(result.agent_id, node_id);

        let identity = get_identity_by_agent_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing identity");
        assert_eq!(identity.certificate_status, "active");

        let session = get_session_by_id(&pool, &result.session_id)
            .await
            .unwrap()
            .expect("missing session");
        assert_eq!(session.status, "active");
        assert_eq!(session.listen_port, Some(7311));

        let enrollment = get_enrollment_by_token_hash(&pool, &token_hash(token))
            .await
            .unwrap()
            .expect("missing enrollment");
        assert_eq!(enrollment.token_status, "used");
        assert!(enrollment.first_used_at.is_some());

        let node = get_node_by_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing node");
        assert_eq!(node.status, NodeStatus::Online.as_str());
        assert!(node.registered_at.is_some());
    }

    #[tokio::test]
    async fn register_node_replaces_same_endpoint_session_epoch() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "register-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let first = enroll_node(
            &pool,
            token,
            Some("10.0.0.9".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();
        let first_heartbeat = heartbeat(
            &pool,
            &node_id,
            &first.session_id,
            &first.lease_id,
            176,
            Some("10.0.0.9".to_string()),
            Some(7311),
            None,
        )
        .await
        .unwrap();
        assert!(!first_heartbeat.sequence_ignored);

        let second = register_node(&pool, &node_id, Some("10.0.0.9".to_string()), Some(7311))
            .await
            .unwrap();

        assert!(second.session_replaced);
        assert_ne!(second.session_id, first.session_id);
        assert_ne!(second.lease_id, first.lease_id);

        let sessions = list_sessions_by_node_id(&pool, &node_id, 10).await.unwrap();
        assert_eq!(sessions.len(), 2);
        let old = get_session_by_id(&pool, &first.session_id)
            .await
            .unwrap()
            .expect("missing old session");
        assert_eq!(old.status, "superseded");
        assert_eq!(
            old.close_reason.as_deref(),
            Some("superseded_by_re_registration")
        );

        let active = get_active_session_by_agent_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing active session");
        assert_eq!(active.session_id, second.session_id);
        assert_eq!(active.advertise_addr.as_deref(), Some("10.0.0.9"));
        assert_eq!(active.listen_port, Some(7311));
        assert_eq!(active.heartbeat_sequence, 0);

        let recovered_heartbeat = heartbeat(
            &pool,
            &node_id,
            &second.session_id,
            &second.lease_id,
            1,
            Some("10.0.0.9".to_string()),
            Some(7311),
            None,
        )
        .await
        .unwrap();
        assert!(!recovered_heartbeat.sequence_ignored);

        let stale_heartbeat = heartbeat(
            &pool,
            &node_id,
            &first.session_id,
            &first.lease_id,
            177,
            Some("10.0.0.9".to_string()),
            Some(7311),
            None,
        )
        .await;
        assert!(stale_heartbeat.is_err());
    }

    #[tokio::test]
    async fn enroll_node_replays_existing_session_when_token_already_used() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "enroll-replay-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let first = enroll_node(
            &pool,
            token,
            Some("10.0.0.21".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();
        let second = enroll_node(
            &pool,
            token,
            Some("10.0.0.99".to_string()),
            Some(1999),
            "ed25519",
            None,
        )
        .await
        .unwrap();

        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.lease_id, second.lease_id);
    }

    #[tokio::test]
    async fn heartbeat_refreshes_lease_expiry() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "heartbeat-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let session = enroll_node(
            &pool,
            token,
            Some("10.0.0.11".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();
        let before = get_session_by_id(&pool, &session.session_id)
            .await
            .unwrap()
            .expect("missing session");

        let result = heartbeat(
            &pool,
            &node_id,
            &session.session_id,
            &session.lease_id,
            1,
            Some("10.0.0.11".to_string()),
            Some(7311),
            None,
        )
        .await
        .unwrap();
        assert!(!result.sequence_ignored);

        let after = get_session_by_id(&pool, &session.session_id)
            .await
            .unwrap()
            .expect("missing refreshed session");
        let before_expiry = DateTime::parse_from_rfc3339(&before.lease_expires_at).unwrap();
        let after_expiry = DateTime::parse_from_rfc3339(&after.lease_expires_at).unwrap();
        assert!(after_expiry >= before_expiry);
        assert_eq!(after.status, "active");
        let before_heartbeat = DateTime::parse_from_rfc3339(
            before
                .last_heartbeat_at
                .as_deref()
                .expect("missing heartbeat"),
        )
        .unwrap();
        let after_heartbeat = DateTime::parse_from_rfc3339(
            after
                .last_heartbeat_at
                .as_deref()
                .expect("missing heartbeat"),
        )
        .unwrap();
        assert!(after_heartbeat >= before_heartbeat);
        assert!((after_expiry - after_heartbeat).num_seconds() >= DEFAULT_LEASE_TTL_SECONDS - 1);

        let ignored = heartbeat(
            &pool,
            &node_id,
            &session.session_id,
            &session.lease_id,
            1,
            Some("10.0.0.11".to_string()),
            Some(7311),
            None,
        )
        .await
        .unwrap();
        assert!(ignored.sequence_ignored);
    }

    #[tokio::test]
    async fn deregister_closes_session_and_marks_node_offline() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "deregister-token";
        insert_test_node(&pool, &node_id).await;
        let _ = insert_test_enrollment(&pool, &node_id, token).await;

        let session = enroll_node(
            &pool,
            token,
            Some("10.0.0.12".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();
        deregister(&pool, &session.session_id, "shutdown")
            .await
            .unwrap();
        deregister(&pool, &session.session_id, "shutdown")
            .await
            .unwrap();

        let stored = get_session_by_id(&pool, &session.session_id)
            .await
            .unwrap()
            .expect("missing stored session");
        assert_eq!(stored.status, "closed");
        assert_eq!(stored.close_reason.as_deref(), Some("shutdown"));

        let node = get_node_by_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing node");
        assert_eq!(node.status, NodeStatus::Offline.as_str());
    }

    #[tokio::test]
    async fn rotate_certificate_updates_identity_fingerprint() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "rotate-cert-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let session = enroll_node(
            &pool,
            token,
            Some("10.0.0.13".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();
        let old = get_identity_by_agent_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing identity");

        let result = rotate_certificate(
            &pool,
            &node_id,
            &session.session_id,
            Some(&old.certificate_fingerprint),
            "ed25519",
            Some("csr-rotated"),
        )
        .await
        .unwrap();

        let new_identity = get_identity_by_agent_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing rotated identity");
        assert_ne!(
            old.certificate_fingerprint,
            new_identity.certificate_fingerprint
        );
        assert!(new_identity.rotated_at.is_some());
        assert_eq!(result.agent_id, node_id);
        assert!(!result.old_certificate_retire_after.is_empty());
    }

    #[tokio::test]
    async fn enroll_node_rejects_conflict_when_issued_token_has_active_session() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "conflict-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;
        let _first = enroll_node(
            &pool,
            token,
            Some("10.0.0.31".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();

        let second_token = format!("conflict-token-{}", Uuid::new_v4().simple());
        insert_test_enrollment(&pool, &node_id, &second_token).await;
        let err = enroll_node(
            &pool,
            &second_token,
            Some("10.0.0.32".to_string()),
            Some(9092),
            "ed25519",
            None,
        )
        .await
        .expect_err("issued token should fail when node already has active session");
        assert!(format!("{err:?}").contains("runtime session conflict for enrollment"));

        let node = get_node_by_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing node");
        assert_eq!(node.status, NodeStatus::Conflict.as_str());
    }

    #[tokio::test]
    async fn e2e_deploy_enroll_heartbeat_and_proxy_route() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        insert_test_node(&pool, &node_id).await;

        // 模拟部署阶段签发 enrollment token。
        let issued = issue_enrollment_token(&pool, &node_id).await.unwrap();
        let enrolled = enroll_node(
            &pool,
            &issued.token,
            Some("10.0.0.41".to_string()),
            Some(18443),
            "ed25519",
            Some("csr-e2e-enroll"),
        )
        .await
        .unwrap();
        let heartbeat_result = heartbeat(
            &pool,
            &node_id,
            &enrolled.session_id,
            &enrolled.lease_id,
            1,
            Some("10.0.0.41".to_string()),
            Some(18443),
            None,
        )
        .await
        .unwrap();
        assert!(!heartbeat_result.sequence_ignored);

        let endpoint = resolve_active_session_endpoint(&pool, &node_id)
            .await
            .unwrap()
            .expect("active session endpoint missing");
        assert_eq!(endpoint, "https://10.0.0.41:18443");
        crate::models::node_sessions::upsert_session_command_access(
            &pool,
            &enrolled.session_id,
            crate::models::node_sessions::AgentCommandTransport::Https,
            "test-command-credential-which-is-long-enough",
        )
        .await
        .unwrap();

        let runtime_client = NodeRuntimeClient::from_node_route(&pool, Some(&node_id)).await;
        assert!(runtime_client.is_ok());
    }

    #[tokio::test]
    async fn e2e_node_restart_and_session_recovery() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "e2e-restart-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let first = enroll_node(
            &pool,
            token,
            Some("10.0.0.51".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();
        let recovered = register_node(&pool, &node_id, Some("10.0.0.52".to_string()), Some(9092))
            .await
            .unwrap();
        assert!(recovered.session_replaced);
        assert_ne!(first.session_id, recovered.session_id);

        let old = get_session_by_id(&pool, &first.session_id)
            .await
            .unwrap()
            .expect("missing old session");
        assert_eq!(old.status, "superseded");
        assert_eq!(
            old.close_reason.as_deref(),
            Some("superseded_by_re_registration")
        );

        let active = get_active_session_by_agent_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing active recovered session");
        assert_eq!(active.session_id, recovered.session_id);
        assert_eq!(active.listen_port, Some(9092));
    }

    #[tokio::test]
    async fn e2e_node_disconnect_then_lease_expired_to_offline() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "e2e-lease-expired-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let session = enroll_node(
            &pool,
            token,
            Some("10.0.0.61".to_string()),
            Some(7311),
            "ed25519",
            None,
        )
        .await
        .unwrap();
        sqlx::query(
            r#"
            UPDATE node_sessions
            SET lease_expires_at = ?
            WHERE session_id = ?
            "#,
        )
        .bind((Utc::now() - Duration::seconds(5)).to_rfc3339())
        .bind(&session.session_id)
        .execute(&pool)
        .await
        .unwrap();

        let reaped = reap_expired_sessions(&pool).await.unwrap();
        assert_eq!(reaped, 1);

        let stored = get_session_by_id(&pool, &session.session_id)
            .await
            .unwrap()
            .expect("missing expired session");
        assert_eq!(stored.status, "expired");
        assert_eq!(stored.close_reason.as_deref(), Some("lease_expired"));

        let node = get_node_by_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing node");
        assert_eq!(node.status, NodeStatus::Offline.as_str());
    }

    #[tokio::test]
    async fn e2e_certificate_rotation() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let token = "e2e-rotate-cert-token";
        insert_test_node(&pool, &node_id).await;
        insert_test_enrollment(&pool, &node_id, token).await;

        let session = enroll_node(
            &pool,
            token,
            Some("10.0.0.71".to_string()),
            Some(7311),
            "ed25519",
            Some("csr-before-rotate"),
        )
        .await
        .unwrap();
        let old = get_identity_by_agent_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing original identity");

        let rotate = rotate_certificate(
            &pool,
            &node_id,
            &session.session_id,
            Some(&old.certificate_fingerprint),
            "ed25519",
            Some("csr-after-rotate"),
        )
        .await
        .unwrap();
        assert_eq!(rotate.agent_id, node_id);
        assert!(!rotate.old_certificate_retire_after.is_empty());

        let new_identity = get_identity_by_agent_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing rotated identity");
        assert_ne!(
            old.certificate_fingerprint,
            new_identity.certificate_fingerprint
        );
        assert!(new_identity.rotated_at.is_some());
    }
}
