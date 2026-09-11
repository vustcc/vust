//! 节点读模型服务：聚合库存、部署、会话与观测视图。

use crate::models::node_observations::{
    NodeObservationRecord, get_latest_node_observation_by_node_id,
    list_node_observations_by_node_id,
};
use crate::models::node_provisioning::get_node_provisioning_by_node_id;
use crate::models::node_sessions::{
    NodeSessionRecord, get_latest_session_by_node_id, list_sessions_by_node_id,
};
use crate::state::DbPool;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::Row;

const HEALTH_FRESHNESS_SECONDS: i64 = 30;

/// 节点列表与详情共用摘要视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSummaryView {
    pub node_id: String,
    pub name: String,
    pub group_name: String,
    pub description: Option<String>,
    pub address: Option<String>,
    pub service_port: Option<String>,
    pub status: String,
    pub lifecycle_status: String,
    pub runtime_status: Option<String>,
    pub health_status: Option<String>,
    pub tags: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub last_seen_at: Option<String>,
}

/// 节点部署视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeProvisioningView {
    pub deploy_method: String,
    pub ssh_addr: Option<String>,
    pub ssh_port: Option<i64>,
    pub ssh_user: Option<String>,
    pub ssh_auth_mode: Option<String>,
    pub install_dir: String,
    pub systemd_service_name: String,
    pub expected_listen_port: Option<i64>,
    pub vust_url: Option<String>,
    pub last_deploy_result_status: Option<String>,
    pub last_deploy_error_summary: Option<String>,
    pub last_deploy_at: Option<String>,
}

/// 节点会话视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSessionView {
    pub session_id: String,
    pub agent_id: String,
    pub advertise_addr: Option<String>,
    pub listen_addr: Option<String>,
    pub listen_port: Option<i64>,
    pub registered_at: String,
    pub lease_expires_at: String,
    pub last_heartbeat_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub status: String,
    pub close_reason: Option<String>,
    pub closed_at: Option<String>,
}

/// 节点观测视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeObservationView {
    pub observation_id: String,
    pub source: String,
    pub system_snapshot: Option<serde_json::Value>,
    pub docker_snapshot: Option<serde_json::Value>,
    pub probe_result: Option<serde_json::Value>,
    pub observed_at: String,
}

/// 节点详情视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDetailView {
    pub node: NodeSummaryView,
    pub provisioning: Option<NodeProvisioningView>,
    pub session: Option<NodeSessionView>,
    pub latest_observation: Option<NodeObservationView>,
}

fn parse_json_value(raw: Option<String>) -> Option<serde_json::Value> {
    raw.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
}

#[derive(Debug, Default)]
struct RuntimeFacts {
    session_status: Option<String>,
    lease_expires_at: Option<String>,
    session_last_seen_at: Option<String>,
    probe_result: Option<String>,
    observed_at: Option<String>,
}

impl RuntimeFacts {
    fn from_records(
        session: Option<&NodeSessionRecord>,
        observation: Option<&NodeObservationRecord>,
    ) -> Self {
        Self {
            session_status: session.map(|item| item.status.clone()),
            lease_expires_at: session.map(|item| item.lease_expires_at.clone()),
            session_last_seen_at: session.and_then(|item| item.last_seen_at.clone()),
            probe_result: observation.and_then(|item| item.probe_result.clone()),
            observed_at: observation.map(|item| item.observed_at.clone()),
        }
    }

    fn from_joined_row(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            session_status: row.try_get("latest_session_status")?,
            lease_expires_at: row.try_get("latest_lease_expires_at")?,
            session_last_seen_at: row.try_get("latest_session_last_seen_at")?,
            probe_result: row.try_get("latest_probe_result")?,
            observed_at: row.try_get("latest_observed_at")?,
        })
    }

    fn has_live_session(&self, now: DateTime<Utc>) -> bool {
        self.session_status.as_deref() == Some("active")
            && self
                .lease_expires_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .is_some_and(|expires_at| expires_at.with_timezone(&Utc) > now)
    }

    fn runtime_status(&self, now: DateTime<Utc>) -> Option<String> {
        match self.session_status.as_deref() {
            Some("active") if !self.has_live_session(now) => Some("expired".to_string()),
            _ => self.session_status.clone(),
        }
    }

    fn fresh_health_status(&self, now: DateTime<Utc>) -> Option<String> {
        let observed_at = self
            .observed_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())?
            .with_timezone(&Utc);
        if now.signed_duration_since(observed_at) > Duration::seconds(HEALTH_FRESHNESS_SECONDS)
            || observed_at > now + Duration::seconds(HEALTH_FRESHNESS_SECONDS)
        {
            return None;
        }
        let probe = parse_json_value(self.probe_result.clone())?;
        probe
            .get("status")
            .and_then(|value| value.as_str())
            .map(|value| value.to_lowercase())
    }
}

fn parse_health_status_from_facts(facts: &RuntimeFacts, now: DateTime<Utc>) -> Option<String> {
    facts.fresh_health_status(now)
}

fn compute_display_status(
    lifecycle_status: &str,
    facts: &RuntimeFacts,
    now: DateTime<Utc>,
) -> String {
    if matches!(
        lifecycle_status,
        "retired" | "conflict" | "deploying" | "deploy_failed" | "awaiting_registration"
    ) {
        return lifecycle_status.to_string();
    }

    if facts.has_live_session(now) {
        return match parse_health_status_from_facts(facts, now).as_deref() {
            Some("offline" | "degraded" | "unreachable") => "degraded".to_string(),
            _ => "online".to_string(),
        };
    }

    if lifecycle_status == "draft" {
        return "draft".to_string();
    }
    "offline".to_string()
}

fn map_node_summary_row(
    row: &sqlx::sqlite::SqliteRow,
    facts: &RuntimeFacts,
) -> sqlx::Result<NodeSummaryView> {
    let metadata_raw: String = row.try_get("metadata")?;
    let labels_raw: String = row.try_get("labels")?;
    let tags = serde_json::from_str::<Vec<String>>(&labels_raw).unwrap_or_default();
    let address: Option<String> = row.try_get("ssh_addr")?;
    let lifecycle_status: String = row.try_get("status")?;
    let now = Utc::now();
    let runtime_status = facts.runtime_status(now);
    let health_status = parse_health_status_from_facts(facts, now);

    let status = compute_display_status(&lifecycle_status, facts, now);
    let mut metadata = serde_json::from_str::<serde_json::Value>(&metadata_raw).ok();

    if status != "online"
        && status != "degraded"
        && let Some(ref mut meta) = metadata
        && let Some(obj) = meta.as_object_mut()
    {
        obj.remove("resource");
    }

    Ok(NodeSummaryView {
        node_id: row.try_get("node_id")?,
        name: row.try_get("name")?,
        group_name: row.try_get("group_name")?,
        description: row.try_get("description")?,
        address,
        service_port: row
            .try_get::<Option<i64>, _>("expected_listen_port")?
            .map(|value| value.to_string()),
        status,
        lifecycle_status,
        runtime_status,
        health_status,
        tags,
        metadata,
        last_seen_at: facts
            .session_last_seen_at
            .clone()
            .or_else(|| row.try_get("last_seen_at").ok().flatten()),
    })
}

async fn fetch_summary_components(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<(
    Option<NodeSummaryView>,
    Option<NodeSessionRecord>,
    Option<NodeObservationRecord>,
)> {
    let row = sqlx::query(
        r#"
        SELECT
            n.node_id,
            n.name,
            n.group_name,
            n.description,
            n.status,
            n.labels,
            n.metadata,
            n.last_seen_at,
            p.ssh_addr,
            p.ssh_port,
            p.expected_listen_port
        FROM nodes n
        LEFT JOIN node_provisioning p
          ON p.node_id = n.node_id
        WHERE n.node_id = ?
        "#,
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    let session = get_latest_session_by_node_id(pool, node_id).await?;
    let observation = get_latest_node_observation_by_node_id(pool, node_id).await?;
    let summary = match row {
        Some(row) => {
            let facts = RuntimeFacts::from_records(session.as_ref(), observation.as_ref());
            Some(map_node_summary_row(&row, &facts)?)
        }
        None => None,
    };
    Ok((summary, session, observation))
}

/// 读取节点列表聚合视图。
pub async fn list_node_summaries(pool: &DbPool) -> sqlx::Result<Vec<NodeSummaryView>> {
    let rows = sqlx::query(
        r#"
        WITH latest_sessions AS (
            SELECT
                node_id,
                status,
                lease_expires_at,
                last_seen_at,
                ROW_NUMBER() OVER (
                    PARTITION BY node_id ORDER BY registered_at DESC, created_at DESC
                ) AS row_number
            FROM node_sessions
        ),
        latest_observations AS (
            SELECT
                node_id,
                probe_result,
                observed_at,
                ROW_NUMBER() OVER (
                    PARTITION BY node_id ORDER BY observed_at DESC, created_at DESC
                ) AS row_number
            FROM node_observations
        )
        SELECT
            n.node_id,
            n.name,
            n.group_name,
            n.description,
            n.status,
            n.labels,
            n.metadata,
            n.last_seen_at,
            p.ssh_addr,
            p.ssh_port,
            p.expected_listen_port,
            s.status AS latest_session_status,
            s.lease_expires_at AS latest_lease_expires_at,
            s.last_seen_at AS latest_session_last_seen_at,
            o.probe_result AS latest_probe_result,
            o.observed_at AS latest_observed_at
        FROM nodes n
        LEFT JOIN node_provisioning p
          ON p.node_id = n.node_id
        LEFT JOIN latest_sessions s
          ON s.node_id = n.node_id AND s.row_number = 1
        LEFT JOIN latest_observations o
          ON o.node_id = n.node_id AND o.row_number = 1
        ORDER BY n.created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let facts = RuntimeFacts::from_joined_row(&row)?;
            map_node_summary_row(&row, &facts)
        })
        .collect()
}

/// 读取单个节点摘要视图。
pub async fn get_node_summary(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<Option<NodeSummaryView>> {
    if node_id == "local" {
        let session =
            crate::models::node_sessions::get_active_session_by_node_id(pool, "local").await?;
        let session = session.filter(crate::models::node_sessions::has_live_lease);
        let local_status = if session.is_some() {
            "online".to_string()
        } else {
            "offline".to_string()
        };
        return Ok(Some(NodeSummaryView {
            node_id: "local".to_string(),
            name: "Local Node".to_string(),
            group_name: "default".to_string(),
            description: Some("Local Controller".to_string()),
            address: Some("127.0.0.1".to_string()),
            service_port: None,
            status: local_status.clone(),
            lifecycle_status: local_status.clone(),
            runtime_status: session.as_ref().map(|value| value.status.clone()),
            health_status: Some(local_status.clone()),
            tags: vec!["local".to_string()],
            metadata: None,
            last_seen_at: session.and_then(|value| value.last_seen_at),
        }));
    }
    Ok(fetch_summary_components(pool, node_id).await?.0)
}

/// 聚合单个节点详情视图。
pub async fn get_node_detail(pool: &DbPool, node_id: &str) -> sqlx::Result<Option<NodeDetailView>> {
    let (node, session_record, observation_record) =
        fetch_summary_components(pool, node_id).await?;
    let Some(node) = node else {
        return Ok(None);
    };

    let provisioning = get_node_provisioning_by_node_id(pool, node_id)
        .await?
        .map(|record| NodeProvisioningView {
            deploy_method: record.deploy_method,
            ssh_addr: record.ssh_addr,
            ssh_port: record.ssh_port,
            ssh_user: record.ssh_user,
            ssh_auth_mode: record.ssh_auth_mode,
            install_dir: record.install_dir,
            systemd_service_name: record.systemd_service_name,
            expected_listen_port: record.expected_listen_port,
            vust_url: record.vust_url,
            last_deploy_result_status: record.last_deploy_result_status,
            last_deploy_error_summary: record.last_deploy_error_summary,
            last_deploy_at: record.last_deploy_at,
        });

    let session = session_record.map(|record| NodeSessionView {
        session_id: record.session_id,
        agent_id: record.agent_id,
        advertise_addr: record.advertise_addr,
        listen_addr: record.listen_addr,
        listen_port: record.listen_port,
        registered_at: record.registered_at,
        lease_expires_at: record.lease_expires_at,
        last_heartbeat_at: record.last_heartbeat_at,
        last_seen_at: record.last_seen_at,
        status: record.status,
        close_reason: record.close_reason,
        closed_at: record.closed_at,
    });

    let is_active = node.status == "online" || node.status == "degraded";
    let latest_observation = if is_active {
        observation_record.map(|record| NodeObservationView {
            observation_id: record.observation_id,
            source: record.source,
            system_snapshot: parse_json_value(record.system_snapshot),
            docker_snapshot: parse_json_value(record.docker_snapshot),
            probe_result: parse_json_value(record.probe_result),
            observed_at: record.observed_at,
        })
    } else {
        None
    };

    Ok(Some(NodeDetailView {
        node,
        provisioning,
        session,
        latest_observation,
    }))
}

/// 读取节点最近若干条观测记录。
pub async fn list_node_observation_views(
    pool: &DbPool,
    node_id: &str,
    limit: i64,
) -> sqlx::Result<Vec<NodeObservationView>> {
    let rows = list_node_observations_by_node_id(pool, node_id, limit).await?;
    Ok(rows
        .into_iter()
        .map(|record| NodeObservationView {
            observation_id: record.observation_id,
            source: record.source,
            system_snapshot: parse_json_value(record.system_snapshot),
            docker_snapshot: parse_json_value(record.docker_snapshot),
            probe_result: parse_json_value(record.probe_result),
            observed_at: record.observed_at,
        })
        .collect())
}

/// 读取节点最近若干条会话记录。
pub async fn list_node_session_views(
    pool: &DbPool,
    node_id: &str,
    limit: i64,
) -> sqlx::Result<Vec<NodeSessionView>> {
    let rows = list_sessions_by_node_id(pool, node_id, limit).await?;
    Ok(rows
        .into_iter()
        .map(|record| NodeSessionView {
            session_id: record.session_id,
            agent_id: record.agent_id,
            advertise_addr: record.advertise_addr,
            listen_addr: record.listen_addr,
            listen_port: record.listen_port,
            registered_at: record.registered_at,
            lease_expires_at: record.lease_expires_at,
            last_heartbeat_at: record.last_heartbeat_at,
            last_seen_at: record.last_seen_at,
            status: record.status,
            close_reason: record.close_reason,
            closed_at: record.closed_at,
        })
        .collect())
}

pub fn spawn_local_node_monitor(state: std::sync::Arc<crate::state::AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;

            let client =
                match crate::models::node_runtime_client::NodeRuntimeClient::from_node_route(
                    &state.metadata_db,
                    None,
                )
                .await
                {
                    Ok(c) => c,
                    Err(_) => {
                        let mut cache = state.local_node_resource.lock().await;
                        *cache = None;
                        continue;
                    }
                };

            let resp: Result<serde_json::Value, _> =
                client.get_json("/api/v1/agent/system/summary").await;

            match resp {
                Ok(val) => {
                    if val.get("success").and_then(|v| v.as_bool()) == Some(true) {
                        if let Some(data) = val.get("data") {
                            let mut cache = state.local_node_resource.lock().await;
                            *cache = Some(data.clone());
                        }
                    } else {
                        let mut cache = state.local_node_resource.lock().await;
                        *cache = None;
                    }
                }
                Err(_) => {
                    let mut cache = state.local_node_resource.lock().await;
                    *cache = None;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        RuntimeFacts, compute_display_status, get_node_detail, list_node_observation_views,
        list_node_session_views, list_node_summaries,
    };
    use crate::models::node_identities::{NodeIdentityRecord, insert_node_identity};
    use crate::models::node_observations::{NodeObservationRecord, insert_node_observation};
    use crate::models::node_provisioning::{NodeProvisioningRecord, upsert_node_provisioning};
    use crate::models::node_sessions::{NodeSessionRecord, insert_node_session};
    use crate::models::nodes::{NewNodeRecord, NodeStatus, insert_node, update_node_status};
    use crate::test_support::setup_test_db;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    async fn insert_test_node_with_provisioning(
        pool: &crate::state::DbPool,
        node_id: &str,
    ) -> String {
        insert_node(
            pool,
            &NewNodeRecord {
                node_id: node_id.to_string(),
                tenant_id: None,
                name: format!("node-{node_id}"),
                normalized_name: format!("node-{node_id}"),
                group_name: "default".to_string(),
                labels: r#"["prod","gpu"]"#.to_string(),
                description: Some("test node".to_string()),
                desired_role: None,
                schedulable: true,
                metadata: r#"{"rack":"r1"}"#.to_string(),
            },
        )
        .await
        .unwrap();
        update_node_status(pool, node_id, NodeStatus::Online)
            .await
            .unwrap();

        let provisioning_id = Uuid::new_v4().to_string();
        upsert_node_provisioning(
            pool,
            &NodeProvisioningRecord {
                provisioning_id,
                node_id: node_id.to_string(),
                deploy_method: "ssh_push".to_string(),
                ssh_addr: Some("10.8.0.11".to_string()),
                ssh_port: Some(22),
                ssh_user: Some("root".to_string()),
                ssh_auth_mode: Some("password".to_string()),
                ssh_password_ciphertext: None,
                ssh_private_key_ciphertext: None,
                ssh_private_key_passphrase_ciphertext: None,
                install_dir: crate::config::DEFAULT_PRODUCTION_HOME.to_string(),
                systemd_service_name: "vust-agent.service".to_string(),
                expected_listen_port: Some(7311),
                vust_url: Some("https://controller.example.com:7310".to_string()),
                revision: 1,
                validated_revision: None,
                precheck_valid_until: None,
                last_deploy_task_id: None,
                last_deploy_result_status: Some("prepared".to_string()),
                last_deploy_error_summary: None,
                last_deploy_at: Some(Utc::now().to_rfc3339()),
                created_at: Utc::now().to_rfc3339(),
                updated_at: Utc::now().to_rfc3339(),
            },
        )
        .await
        .unwrap();
        insert_node_identity(
            pool,
            &NodeIdentityRecord {
                identity_id: Uuid::new_v4().to_string(),
                node_id: node_id.to_string(),
                agent_id: node_id.to_string(),
                certificate_serial_number: None,
                certificate_fingerprint: format!("SHA256:{}", Uuid::new_v4().simple()),
                certificate_status: "active".to_string(),
                public_key_algorithm: "ed25519".to_string(),
                certificate_issued_at: Some(Utc::now().to_rfc3339()),
                certificate_expires_at: None,
                rotated_at: None,
                created_at: Utc::now().to_rfc3339(),
                updated_at: Utc::now().to_rfc3339(),
            },
        )
        .await
        .unwrap();

        node_id.to_string()
    }

    #[test]
    fn lifecycle_and_freshness_control_display_state() {
        let now = Utc::now();
        let live = RuntimeFacts {
            session_status: Some("active".to_string()),
            lease_expires_at: Some((now + Duration::minutes(1)).to_rfc3339()),
            session_last_seen_at: Some(now.to_rfc3339()),
            probe_result: Some(r#"{"status":"degraded"}"#.to_string()),
            observed_at: Some(now.to_rfc3339()),
        };
        assert_eq!(compute_display_status("retired", &live, now), "retired");
        assert_eq!(compute_display_status("conflict", &live, now), "conflict");
        assert_eq!(compute_display_status("online", &live, now), "degraded");

        let stale = RuntimeFacts {
            observed_at: Some((now - Duration::seconds(31)).to_rfc3339()),
            ..live
        };
        assert_eq!(stale.fresh_health_status(now), None);
        assert_eq!(compute_display_status("online", &stale, now), "online");

        let expired = RuntimeFacts {
            lease_expires_at: Some((now - Duration::seconds(1)).to_rfc3339()),
            ..stale
        };
        assert_eq!(expired.runtime_status(now).as_deref(), Some("expired"));
        assert_eq!(compute_display_status("online", &expired, now), "offline");
    }

    #[tokio::test]
    async fn list_summaries_joins_latest_runtime_facts() {
        let pool = setup_test_db().await;
        let first = Uuid::new_v4().to_string();
        let second = Uuid::new_v4().to_string();
        insert_test_node_with_provisioning(&pool, &first).await;
        insert_test_node_with_provisioning(&pool, &second).await;

        let items = list_node_summaries(&pool).await.unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| item.node_id == first));
        assert!(items.iter().any(|item| item.node_id == second));
        assert!(items.iter().all(|item| item.status == "offline"));
    }

    #[tokio::test]
    async fn list_history_views_return_recent_records() {
        let pool = setup_test_db().await;
        let node_id = insert_test_node_with_provisioning(&pool, &Uuid::new_v4().to_string()).await;
        let now = Utc::now();

        let old_session_id = Uuid::new_v4().to_string();
        insert_node_session(
            &pool,
            &NodeSessionRecord {
                session_id: old_session_id.clone(),
                node_id: node_id.clone(),
                agent_id: node_id.clone(),
                lease_id: Uuid::new_v4().to_string(),
                advertise_addr: Some("10.8.0.11".to_string()),
                listen_addr: None,
                listen_port: Some(7311),
                server_name: None,
                registered_at: (now - Duration::minutes(5)).to_rfc3339(),
                lease_expires_at: (now + Duration::minutes(1)).to_rfc3339(),
                last_heartbeat_at: Some((now - Duration::minutes(4)).to_rfc3339()),
                heartbeat_sequence: 1,
                last_seen_at: Some((now - Duration::minutes(4)).to_rfc3339()),
                status: "superseded".to_string(),
                close_reason: Some("restart".to_string()),
                closed_at: Some((now - Duration::minutes(3)).to_rfc3339()),
                created_at: (now - Duration::minutes(5)).to_rfc3339(),
                updated_at: (now - Duration::minutes(3)).to_rfc3339(),
            },
        )
        .await
        .unwrap();

        let active_session_id = Uuid::new_v4().to_string();
        insert_node_session(
            &pool,
            &NodeSessionRecord {
                session_id: active_session_id.clone(),
                node_id: node_id.clone(),
                agent_id: node_id.clone(),
                lease_id: Uuid::new_v4().to_string(),
                advertise_addr: Some("10.8.0.12".to_string()),
                listen_addr: None,
                listen_port: Some(9443),
                server_name: None,
                registered_at: now.to_rfc3339(),
                lease_expires_at: (now + Duration::minutes(5)).to_rfc3339(),
                last_heartbeat_at: Some(now.to_rfc3339()),
                heartbeat_sequence: 2,
                last_seen_at: Some(now.to_rfc3339()),
                status: "active".to_string(),
                close_reason: None,
                closed_at: None,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();

        insert_node_observation(
            &pool,
            &NodeObservationRecord {
                observation_id: Uuid::new_v4().to_string(),
                node_id: node_id.clone(),
                session_id: Some(active_session_id.clone()),
                source: "heartbeat".to_string(),
                system_snapshot: None,
                docker_snapshot: None,
                probe_result: Some(r#"{"status":"degraded"}"#.to_string()),
                observed_at: now.to_rfc3339(),
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();

        let sessions = list_node_session_views(&pool, &node_id, 10).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, active_session_id);
        assert_eq!(sessions[1].session_id, old_session_id);

        let observations = list_node_observation_views(&pool, &node_id, 10)
            .await
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].source, "heartbeat");
    }

    #[tokio::test]
    async fn get_node_detail_aggregates_provisioning_session_and_observation() {
        let pool = setup_test_db().await;
        let node_id = insert_test_node_with_provisioning(&pool, &Uuid::new_v4().to_string()).await;
        let now = Utc::now();
        let session_id = Uuid::new_v4().to_string();

        insert_node_session(
            &pool,
            &NodeSessionRecord {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
                agent_id: node_id.clone(),
                lease_id: Uuid::new_v4().to_string(),
                advertise_addr: Some("10.8.0.21".to_string()),
                listen_addr: None,
                listen_port: Some(7311),
                server_name: None,
                registered_at: now.to_rfc3339(),
                lease_expires_at: (now + Duration::minutes(5)).to_rfc3339(),
                last_heartbeat_at: Some(now.to_rfc3339()),
                heartbeat_sequence: 3,
                last_seen_at: Some(now.to_rfc3339()),
                status: "active".to_string(),
                close_reason: None,
                closed_at: None,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();
        insert_node_observation(
            &pool,
            &NodeObservationRecord {
                observation_id: Uuid::new_v4().to_string(),
                node_id: node_id.clone(),
                session_id: Some(session_id),
                source: "probe".to_string(),
                system_snapshot: Some(r#"{"cpu":12}"#.to_string()),
                docker_snapshot: None,
                probe_result: Some(r#"{"status":"online"}"#.to_string()),
                observed_at: now.to_rfc3339(),
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            },
        )
        .await
        .unwrap();

        let detail = get_node_detail(&pool, &node_id)
            .await
            .unwrap()
            .expect("detail should exist");
        assert_eq!(detail.node.node_id, node_id);
        assert_eq!(detail.node.status, "online");
        assert_eq!(detail.node.lifecycle_status, "online");
        assert_eq!(detail.node.health_status.as_deref(), Some("online"));
        assert_eq!(
            detail.node.tags,
            vec!["prod".to_string(), "gpu".to_string()]
        );
        assert_eq!(detail.node.address.as_deref(), Some("10.8.0.11"));

        let provisioning = detail.provisioning.expect("provisioning should exist");
        assert_eq!(provisioning.deploy_method, "ssh_push");
        assert_eq!(provisioning.expected_listen_port, Some(7311));

        let session = detail.session.expect("session should exist");
        assert_eq!(session.status, "active");
        assert_eq!(session.listen_port, Some(7311));

        let observation = detail
            .latest_observation
            .expect("latest observation should exist");
        assert_eq!(observation.source, "probe");
        assert_eq!(
            observation
                .probe_result
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(|value| value.as_str()),
            Some("online")
        );
    }
}
