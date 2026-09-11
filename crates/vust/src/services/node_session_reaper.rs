//! 节点会话回收服务：定期清理租约过期的活跃会话，并收敛节点离线状态。

use crate::models::node_sessions::{close_session_by_id, list_expired_active_sessions};
use crate::models::nodes::get_node_by_id;
use crate::services::node_runtime::settle_node_after_session_loss;
use crate::services::runtime_metrics;
use crate::state::{AppState, DbPool};
use std::sync::Arc;
use std::time::Duration;

const SESSION_REAPER_INTERVAL_SECONDS: u64 = 5;

/// 启动后台租约回收任务。
pub fn spawn_session_reaper(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(SESSION_REAPER_INTERVAL_SECONDS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            if let Err(err) = reap_expired_sessions(&state.metadata_db).await {
                tracing::warn!("node session reaper failed: {:?}", err);
            }
        }
    });
}

/// 扫描并回收所有租约已过期的活跃会话。
pub async fn reap_expired_sessions(pool: &DbPool) -> sqlx::Result<usize> {
    let expired_sessions = list_expired_active_sessions(pool).await?;
    let mut reaped = 0usize;

    for session in expired_sessions {
        let rows_affected =
            close_session_by_id(pool, &session.session_id, "lease_expired", "expired").await?;
        if rows_affected == 0 {
            continue;
        }

        reaped += 1;
        let previous = get_node_by_id(pool, &session.node_id).await?;
        settle_node_after_session_loss(pool, &session.node_id).await?;
        if previous
            .as_ref()
            .is_some_and(|node| matches!(node.status.as_str(), "online" | "degraded"))
        {
            crate::services::logging::OperationEventBuilder::new(
                "system",
                "node_offline",
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            )
            .target_type("node")
            .target_id(&session.node_id)
            .target_display_name(
                previous
                    .as_ref()
                    .map_or(session.node_id.as_str(), |node| node.name.as_str()),
            )
            .set_success()
            .metadata(serde_json::json!({
                "nodeId": session.node_id,
                "nodeName": previous.as_ref().map(|node| node.name.as_str()),
                "reason": "leaseExpired",
            }))
            .finish(pool);
        }
    }

    runtime_metrics::record_lease_expired(reaped);
    Ok(reaped)
}

#[cfg(test)]
mod tests {
    use super::reap_expired_sessions;
    use crate::models::node_identities::{NodeIdentityRecord, insert_node_identity};
    use crate::models::node_sessions::{NodeSessionRecord, get_session_by_id, insert_node_session};
    use crate::models::nodes::{
        NewNodeRecord, NodeStatus, get_node_by_id, insert_node, update_node_status,
    };
    use crate::test_support::setup_test_db;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    #[tokio::test]
    async fn reap_expired_session_closes_session_and_marks_node_offline() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let session_id = Uuid::new_v4().to_string();
        let now = Utc::now();

        insert_node(
            &pool,
            &NewNodeRecord {
                node_id: node_id.clone(),
                tenant_id: None,
                name: "test-node".to_string(),
                normalized_name: "test-node".to_string(),
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
        update_node_status(&pool, &node_id, NodeStatus::Online)
            .await
            .unwrap();
        insert_node_identity(
            &pool,
            &NodeIdentityRecord {
                identity_id: Uuid::new_v4().to_string(),
                node_id: node_id.clone(),
                agent_id: node_id.clone(),
                certificate_serial_number: None,
                certificate_fingerprint: format!("SHA256:{}", Uuid::new_v4().simple()),
                certificate_status: "active".to_string(),
                public_key_algorithm: "ed25519".to_string(),
                certificate_issued_at: Some(now.to_rfc3339()),
                certificate_expires_at: None,
                rotated_at: None,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();

        insert_node_session(
            &pool,
            &NodeSessionRecord {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
                agent_id: node_id.clone(),
                lease_id: Uuid::new_v4().to_string(),
                advertise_addr: Some("127.0.0.1".to_string()),
                listen_addr: None,
                listen_port: Some(7311),
                server_name: None,
                registered_at: now.to_rfc3339(),
                lease_expires_at: (now - Duration::seconds(5)).to_rfc3339(),
                last_heartbeat_at: Some((now - Duration::seconds(10)).to_rfc3339()),
                heartbeat_sequence: 1,
                last_seen_at: Some((now - Duration::seconds(10)).to_rfc3339()),
                status: "active".to_string(),
                close_reason: None,
                closed_at: None,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();

        let reaped = reap_expired_sessions(&pool).await.unwrap();
        assert_eq!(reaped, 1);

        let session = get_session_by_id(&pool, &session_id)
            .await
            .unwrap()
            .expect("missing session");
        assert_eq!(session.status, "expired");
        assert_eq!(session.close_reason.as_deref(), Some("lease_expired"));
        assert!(session.closed_at.is_some());

        let node = get_node_by_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("missing node");
        assert_eq!(node.status, NodeStatus::Offline.as_str());
    }
}
