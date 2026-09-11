//! 节点操作守卫：在 API 与最终执行服务中统一校验归属、状态和活动任务。

use crate::models::nodes::{NodeStatus, get_node_by_id};
use crate::state::DbPool;
use crate::types::{ApiError, ApiResult};
use vust_contracts::api::ErrorCode;

/// 需要服务端授权的节点操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeOperation {
    Update,
    Check,
    Deploy,
    Reprovision,
    Retire,
    Uninstall,
}

impl NodeOperation {
    fn allows_status(self, status: NodeStatus) -> bool {
        match self {
            Self::Update | Self::Reprovision => {
                !matches!(status, NodeStatus::Deploying | NodeStatus::Retired)
            }
            Self::Check => !matches!(
                status,
                NodeStatus::Draft | NodeStatus::Deploying | NodeStatus::Retired
            ),
            Self::Deploy => matches!(
                status,
                NodeStatus::Draft
                    | NodeStatus::DeployFailed
                    | NodeStatus::AwaitingRegistration
                    | NodeStatus::Online
                    | NodeStatus::Degraded
                    | NodeStatus::Offline
                    | NodeStatus::Unreachable
                    | NodeStatus::Conflict
            ),
            Self::Retire | Self::Uninstall => !matches!(
                status,
                NodeStatus::Draft | NodeStatus::Deploying | NodeStatus::Retired
            ),
        }
    }
}

/// 校验节点操作；部署执行阶段可忽略由本次请求创建的唯一活动部署任务。
pub async fn assert_node_operation(
    pool: &DbPool,
    node_id: &str,
    operation: NodeOperation,
    ignore_active_deploy_task: bool,
) -> ApiResult<()> {
    if node_id == "local" {
        return Err(ApiError::forbidden(
            ErrorCode::NodeProtected,
            "Local Node is system managed and cannot perform this operation",
        ));
    }
    let node = get_node_by_id(pool, node_id)
        .await?
        .ok_or_else(|| ApiError::not_found(ErrorCode::NodeNotFound, "node not found"))?;
    let status = NodeStatus::parse(&node.status)
        .ok_or_else(|| ApiError::internal("node has an invalid persisted status"))?;
    if !operation.allows_status(status) {
        return Err(ApiError::conflict(
            ErrorCode::NodeStateConflict,
            format!(
                "node operation {:?} is not allowed while status is {}",
                operation, node.status
            ),
        ));
    }

    if matches!(operation, NodeOperation::Retire | NodeOperation::Uninstall) {
        let reference_count = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT
                (SELECT COUNT(*) FROM scheduled_tasks
                  WHERE node_id = ? AND deleted_at IS NULL)
              + (SELECT COUNT(*) FROM suite_instances
                  WHERE node_id = ? AND uninstalled_at IS NULL)
              + (SELECT COUNT(*) FROM upgrade_targets
                  WHERE node_id = ? AND status IN ('pending', 'deferred', 'running'))
            "#,
        )
        .bind(node_id)
        .bind(node_id)
        .bind(node_id)
        .fetch_one(pool)
        .await?;
        if reference_count > 0 {
            return Err(ApiError::conflict(
                ErrorCode::NodeInUse,
                format!("node has {reference_count} active resource references"),
            ));
        }
    }

    if !ignore_active_deploy_task {
        let has_active_task = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM node_tasks
                WHERE node_id = ?
                  AND status IN ('queued', 'running', 'cancel_requested')
            )
            "#,
        )
        .bind(node_id)
        .fetch_one(pool)
        .await?;
        if has_active_task {
            return Err(ApiError::conflict(
                ErrorCode::NodeOperationConflict,
                "该节点已有正在执行的操作",
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{NodeOperation, assert_node_operation};
    use crate::models::node_tasks::create_deploy_task;
    use crate::models::nodes::{NewNodeRecord, NodeStatus, insert_node, update_node_status};
    use crate::test_support::setup_test_db;
    use uuid::Uuid;
    use vust_contracts::api::ErrorCode;

    async fn insert_remote(pool: &crate::state::DbPool, status: NodeStatus) -> String {
        let node_id = Uuid::new_v4().to_string();
        insert_node(
            pool,
            &NewNodeRecord {
                node_id: node_id.clone(),
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
        update_node_status(pool, &node_id, status).await.unwrap();
        node_id
    }

    #[tokio::test]
    async fn local_node_is_protected_for_every_operation() {
        let pool = setup_test_db().await;
        for operation in [
            NodeOperation::Update,
            NodeOperation::Check,
            NodeOperation::Deploy,
            NodeOperation::Reprovision,
            NodeOperation::Retire,
            NodeOperation::Uninstall,
        ] {
            let error = assert_node_operation(&pool, "local", operation, false)
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::NodeProtected);
        }
    }

    #[tokio::test]
    async fn state_and_active_task_are_enforced() {
        let pool = setup_test_db().await;
        let draft = insert_remote(&pool, NodeStatus::Draft).await;
        let error = assert_node_operation(&pool, &draft, NodeOperation::Retire, false)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NodeStateConflict);
        assert_node_operation(&pool, &draft, NodeOperation::Deploy, false)
            .await
            .unwrap();

        create_deploy_task(&pool, &Uuid::new_v4().to_string(), &draft)
            .await
            .unwrap();
        let error = assert_node_operation(&pool, &draft, NodeOperation::Update, false)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NodeOperationConflict);
        assert_node_operation(&pool, &draft, NodeOperation::Deploy, true)
            .await
            .unwrap();
    }
}
