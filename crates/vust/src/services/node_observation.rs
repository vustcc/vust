//! 节点观测服务：记录 probe、手动检查与 heartbeat 观测结果。

use crate::models::node_observations::{NodeObservationRecord, insert_node_observation};
use crate::services::node_precheck::NodePrecheckStatus;
use crate::state::DbPool;
use crate::types::new_uuid_v7;
use chrono::Utc;
use serde_json::json;

/// 记录一次手动检查结果。
pub async fn record_manual_check(
    pool: &DbPool,
    node_id: &str,
    status: &str,
    ssh_status: NodePrecheckStatus,
    service_status: NodePrecheckStatus,
    api_status: NodePrecheckStatus,
    detail: serde_json::Value,
) -> sqlx::Result<()> {
    let now = Utc::now().to_rfc3339();
    insert_node_observation(
        pool,
        &NodeObservationRecord {
            observation_id: new_uuid_v7(),
            node_id: node_id.to_string(),
            session_id: None,
            source: "manual_check".to_string(),
            system_snapshot: None,
            docker_snapshot: None,
            probe_result: Some(
                json!({
                    "status": status,
                    "sshStatus": ssh_status,
                    "serviceStatus": service_status,
                    "apiStatus": api_status,
                    "detail": detail,
                })
                .to_string(),
            ),
            observed_at: now,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        },
    )
    .await?;

    sqlx::query(
        r#"
        UPDATE nodes
        SET last_probe_at = ?
        WHERE node_id = ?
        "#,
    )
    .bind(Utc::now().to_rfc3339())
    .bind(node_id)
    .execute(pool)
    .await?;

    Ok(())
}
