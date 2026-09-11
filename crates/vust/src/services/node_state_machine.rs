//! 节点状态机服务：统一管理节点生命周期状态迁移约束。

use crate::models::nodes::{NodeStatus, get_node_by_id, update_node_status};
use crate::state::DbPool;

/// 节点状态变更事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStateEvent {
    StartDeploy,
    DeployFailed,
    DeploymentPrepared,
    RegisterSucceeded,
    DegradeDetected,
    RecoverHealthy,
    LeaseExpired,
    ProbeFailed,
    ConflictDetected,
    RetireRequested,
}

/// 状态迁移错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeStateTransitionError {
    pub from: NodeStatus,
    pub event: NodeStateEvent,
}

impl std::fmt::Display for NodeStateTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid node state transition: {:?} -> {:?}",
            self.from, self.event
        )
    }
}

impl std::error::Error for NodeStateTransitionError {}

/// 根据事件计算节点状态迁移结果。
pub fn apply_node_state_transition(
    current: NodeStatus,
    event: NodeStateEvent,
) -> Result<NodeStatus, NodeStateTransitionError> {
    let next = match (current, event) {
        (NodeStatus::Draft, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::DeployFailed, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::AwaitingRegistration, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::Offline, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::Unreachable, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::Online, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::Degraded, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::Conflict, NodeStateEvent::StartDeploy) => NodeStatus::Deploying,
        (NodeStatus::Deploying, NodeStateEvent::DeployFailed) => NodeStatus::DeployFailed,
        (NodeStatus::AwaitingRegistration, NodeStateEvent::DeployFailed) => {
            NodeStatus::DeployFailed
        }
        (NodeStatus::Deploying, NodeStateEvent::DeploymentPrepared) => {
            NodeStatus::AwaitingRegistration
        }
        (NodeStatus::Conflict, NodeStateEvent::DeploymentPrepared) => {
            NodeStatus::AwaitingRegistration
        }
        (NodeStatus::Online, NodeStateEvent::DeploymentPrepared) => NodeStatus::Online,
        (NodeStatus::Deploying, NodeStateEvent::RegisterSucceeded) => NodeStatus::Online,
        (NodeStatus::AwaitingRegistration, NodeStateEvent::RegisterSucceeded) => NodeStatus::Online,
        (NodeStatus::Registered, NodeStateEvent::RegisterSucceeded) => NodeStatus::Online,
        (NodeStatus::Offline, NodeStateEvent::RegisterSucceeded) => NodeStatus::Online,
        (NodeStatus::Degraded, NodeStateEvent::RegisterSucceeded) => NodeStatus::Online,
        (NodeStatus::Online, NodeStateEvent::RegisterSucceeded) => NodeStatus::Online,
        (NodeStatus::Conflict, NodeStateEvent::RegisterSucceeded) => NodeStatus::Online,
        (NodeStatus::Online, NodeStateEvent::DegradeDetected) => NodeStatus::Degraded,
        (NodeStatus::Degraded, NodeStateEvent::RecoverHealthy) => NodeStatus::Online,
        (NodeStatus::Online, NodeStateEvent::LeaseExpired) => NodeStatus::Offline,
        (NodeStatus::Degraded, NodeStateEvent::LeaseExpired) => NodeStatus::Offline,
        (NodeStatus::Registered, NodeStateEvent::LeaseExpired) => NodeStatus::Offline,
        (NodeStatus::Online, NodeStateEvent::ProbeFailed) => NodeStatus::Unreachable,
        (NodeStatus::Degraded, NodeStateEvent::ProbeFailed) => NodeStatus::Unreachable,
        (_, NodeStateEvent::ConflictDetected) => NodeStatus::Conflict,
        (_, NodeStateEvent::RetireRequested) => NodeStatus::Retired,
        _ => {
            return Err(NodeStateTransitionError {
                from: current,
                event,
            });
        }
    };

    Ok(next)
}

/// 返回节点当前状态是否允许参与代理。
pub fn is_proxyable(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::Online | NodeStatus::Degraded)
}

/// 返回节点当前状态是否允许参与调度。
pub fn is_schedulable(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::Online)
}

async fn transition_node(
    pool: &DbPool,
    node_id: &str,
    event: NodeStateEvent,
) -> sqlx::Result<NodeStatus> {
    let Some(node) = get_node_by_id(pool, node_id).await? else {
        return Err(sqlx::Error::RowNotFound);
    };
    let current = NodeStatus::parse(&node.status)
        .ok_or_else(|| sqlx::Error::Protocol(format!("unknown node status: {}", node.status)))?;
    let next = apply_node_state_transition(current, event).map_err(|err| {
        sqlx::Error::Protocol(format!(
            "invalid node state transition: {:?} -> {:?}",
            err.from, err.event
        ))
    })?;
    update_node_status(pool, node_id, next).await?;
    Ok(next)
}

/// 切换到部署中。
pub async fn transition_to_deploying(pool: &DbPool, node_id: &str) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::StartDeploy).await
}

/// 切换到部署失败。
pub async fn transition_to_deploy_failed(pool: &DbPool, node_id: &str) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::DeployFailed).await
}

/// 切换到等待注册。
pub async fn transition_to_awaiting_registration(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::DeploymentPrepared).await
}

/// 切换到在线。
pub async fn transition_to_online(pool: &DbPool, node_id: &str) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::RegisterSucceeded).await
}

/// 切换到降级。
pub async fn transition_to_degraded(pool: &DbPool, node_id: &str) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::DegradeDetected).await
}

/// 切换到离线。
pub async fn transition_to_offline(pool: &DbPool, node_id: &str) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::LeaseExpired).await
}

/// 切换到冲突。
pub async fn transition_to_conflict(pool: &DbPool, node_id: &str) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::ConflictDetected).await
}

/// 切换到退役。
pub async fn transition_to_retired(pool: &DbPool, node_id: &str) -> sqlx::Result<NodeStatus> {
    transition_node(pool, node_id, NodeStateEvent::RetireRequested).await
}

#[cfg(test)]
mod tests {
    use super::{
        NodeStateEvent, apply_node_state_transition, is_proxyable, is_schedulable,
        transition_to_conflict, transition_to_deploying, transition_to_online,
    };
    use crate::models::nodes::{NewNodeRecord, NodeStatus, get_node_by_id, insert_node};
    use crate::test_support::setup_test_db;
    use uuid::Uuid;

    #[test]
    fn state_transition_matrix_covers_vust_paths() {
        assert_eq!(
            apply_node_state_transition(NodeStatus::Draft, NodeStateEvent::StartDeploy).unwrap(),
            NodeStatus::Deploying
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Deploying, NodeStateEvent::DeployFailed)
                .unwrap(),
            NodeStatus::DeployFailed
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Deploying, NodeStateEvent::DeploymentPrepared)
                .unwrap(),
            NodeStatus::AwaitingRegistration
        );
        assert_eq!(
            apply_node_state_transition(
                NodeStatus::AwaitingRegistration,
                NodeStateEvent::RegisterSucceeded
            )
            .unwrap(),
            NodeStatus::Online
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Online, NodeStateEvent::DegradeDetected)
                .unwrap(),
            NodeStatus::Degraded
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Degraded, NodeStateEvent::RecoverHealthy)
                .unwrap(),
            NodeStatus::Online
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Online, NodeStateEvent::LeaseExpired).unwrap(),
            NodeStatus::Offline
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Online, NodeStateEvent::ProbeFailed).unwrap(),
            NodeStatus::Unreachable
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Online, NodeStateEvent::ConflictDetected)
                .unwrap(),
            NodeStatus::Conflict
        );
        assert_eq!(
            apply_node_state_transition(NodeStatus::Online, NodeStateEvent::RetireRequested)
                .unwrap(),
            NodeStatus::Retired
        );
    }

    #[test]
    fn state_transition_rejects_invalid_path() {
        let err = apply_node_state_transition(NodeStatus::Draft, NodeStateEvent::RegisterSucceeded)
            .expect_err("draft should not jump to register succeeded directly");
        assert_eq!(err.from, NodeStatus::Draft);
        assert_eq!(err.event, NodeStateEvent::RegisterSucceeded);
    }

    #[test]
    fn proxyable_and_schedulable_status_sets_are_expected() {
        assert!(is_proxyable(NodeStatus::Online));
        assert!(is_proxyable(NodeStatus::Degraded));
        assert!(!is_proxyable(NodeStatus::Offline));

        assert!(is_schedulable(NodeStatus::Online));
        assert!(!is_schedulable(NodeStatus::Degraded));
        assert!(!is_schedulable(NodeStatus::Offline));
    }

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
    }

    #[tokio::test]
    async fn transition_helpers_update_persisted_status() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        insert_test_node(&pool, &node_id).await;

        let deploying = transition_to_deploying(&pool, &node_id).await.unwrap();
        assert_eq!(deploying, NodeStatus::Deploying);
        let online = transition_to_online(&pool, &node_id).await.unwrap();
        assert_eq!(online, NodeStatus::Online);
        let conflict = transition_to_conflict(&pool, &node_id).await.unwrap();
        assert_eq!(conflict, NodeStatus::Conflict);

        let node = get_node_by_id(&pool, &node_id)
            .await
            .unwrap()
            .expect("node must exist");
        assert_eq!(node.status, NodeStatus::Conflict.as_str());
    }
}
