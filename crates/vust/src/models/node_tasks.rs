//! 节点任务模型：持久保存任务状态与脱敏进度快照。

use crate::state::{DbPool, DeploySession};
use crate::types::{ApiError, ApiResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Executor, FromRow, Sqlite};
use vust_contracts::api::ErrorCode;

/// 节点任务记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NodeTaskRecord {
    pub task_id: String,
    pub node_id: String,
    pub task_type: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub status: String,
    pub phase: String,
    pub progress_percent: Option<i64>,
    pub progress_logs: String,
    pub cancellable: bool,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 新建排队中的部署任务；同一节点只允许一个活动任务。
pub async fn create_deploy_task<'e, E>(executor: E, task_id: &str, node_id: &str) -> ApiResult<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    create_node_task(executor, task_id, node_id, "deploy").await
}

/// 新建任意节点操作任务；同一节点只允许一个活动任务。
pub async fn create_node_task<'e, E>(
    executor: E,
    task_id: &str,
    node_id: &str,
    task_type: &str,
) -> ApiResult<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let result = sqlx::query(
        r#"
        INSERT INTO node_tasks (
            task_id, node_id, task_type, status, phase, progress_percent, progress_logs
        ) VALUES (?, ?, ?, 'queued', 'queued', 0, '["Node task created"]')
        "#,
    )
    .bind(task_id)
    .bind(node_id)
    .bind(task_type)
    .execute(executor)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => Err(
            ApiError::conflict(ErrorCode::NodeOperationConflict, "该节点已有正在执行的操作"),
        ),
        Err(error) => Err(error.into()),
    }
}

/// 更新任务的持久进度快照，终态任务不会再被覆盖。
pub async fn update_deploy_progress(
    pool: &DbPool,
    task_id: &str,
    session: &DeploySession,
) -> sqlx::Result<()> {
    let (status, phase, finished_at, result_summary, error_code, error_summary) =
        if session.is_finished {
            if session.error.is_some() {
                (
                    "failed",
                    "failed",
                    Some(Utc::now().to_rfc3339()),
                    None,
                    Some("NODE_OPERATION_FAILED"),
                    Some("节点部署失败"),
                )
            } else {
                (
                    "succeeded",
                    "completed",
                    Some(Utc::now().to_rfc3339()),
                    Some("节点部署完成"),
                    None,
                    None,
                )
            }
        } else {
            ("running", "deploying", None, None, None, None)
        };
    let logs = serde_json::to_string(&sanitize_progress_logs(&session.logs))
        .unwrap_or_else(|_| "[]".to_string());

    sqlx::query(
        r#"
        UPDATE node_tasks
        SET
            status = ?,
            phase = ?,
            progress_percent = ?,
            progress_logs = ?,
            started_at = COALESCE(started_at, ?),
            finished_at = ?,
            result_summary = ?,
            error_code = ?,
            error_summary = ?
        WHERE task_id = ?
          AND status IN ('queued', 'running', 'cancel_requested')
        "#,
    )
    .bind(status)
    .bind(phase)
    .bind(i64::from(session.progress_percent))
    .bind(logs)
    .bind(Utc::now().to_rfc3339())
    .bind(finished_at)
    .bind(result_summary)
    .bind(error_code)
    .bind(error_summary)
    .bind(task_id)
    .execute(pool)
    .await?;

    Ok(())
}

fn sanitize_progress_logs(logs: &[String]) -> Vec<String> {
    logs.iter()
        .map(|line| {
            if let Some((prefix, _)) = line.split_once("Task failed:") {
                format!("{prefix}Task failed")
            } else if let Some((prefix, _)) = line.split_once("Deployment rollback was incomplete:")
            {
                format!("{prefix}Deployment rollback was incomplete")
            } else {
                line.clone()
            }
        })
        .collect()
}

/// 读取节点最近一次部署任务。
pub async fn get_latest_deploy_task(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<Option<NodeTaskRecord>> {
    sqlx::query_as::<_, NodeTaskRecord>(
        r#"
        SELECT
            task_id, node_id, task_type, started_at, finished_at, status, phase,
            progress_percent, progress_logs, cancellable, result_summary,
            error_code, error_summary, created_at, updated_at
        FROM node_tasks
        WHERE node_id = ? AND task_type = 'deploy'
        ORDER BY created_at DESC, task_id DESC
        LIMIT 1
        "#,
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
}

/// 读取控制服务重启时尚未终结的节点任务。
pub async fn list_interrupted_tasks(pool: &DbPool) -> sqlx::Result<Vec<NodeTaskRecord>> {
    sqlx::query_as::<_, NodeTaskRecord>(
        r#"
        SELECT
            task_id, node_id, task_type, started_at, finished_at, status, phase,
            progress_percent, progress_logs, cancellable, result_summary,
            error_code, error_summary, created_at, updated_at
        FROM node_tasks
        WHERE status IN ('queued', 'running', 'cancel_requested')
        ORDER BY created_at ASC, task_id ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

/// 将持久任务转换为现有前端部署进度契约。
pub fn to_deploy_session(task: &NodeTaskRecord) -> DeploySession {
    let logs = serde_json::from_str(&task.progress_logs).unwrap_or_default();
    DeploySession {
        progress_percent: task.progress_percent.unwrap_or(0).clamp(0, 100) as u32,
        logs,
        is_finished: matches!(task.status.as_str(), "succeeded" | "failed" | "canceled"),
        error: task.error_summary.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create_deploy_task, get_latest_deploy_task, list_interrupted_tasks, to_deploy_session,
        update_deploy_progress,
    };
    use crate::models::nodes::{NewNodeRecord, insert_node};
    use crate::state::DeploySession;
    use crate::test_support::setup_test_db;
    use uuid::Uuid;
    use vust_contracts::api::ErrorCode;

    async fn insert_test_node(pool: &crate::state::DbPool) -> String {
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
        node_id
    }

    #[tokio::test]
    async fn enforces_one_active_task_and_terminal_state_is_immutable() {
        let pool = setup_test_db().await;
        let node_id = insert_test_node(&pool).await;
        let task_id = Uuid::new_v4().to_string();
        create_deploy_task(&pool, &task_id, &node_id).await.unwrap();

        let conflict = create_deploy_task(&pool, &Uuid::new_v4().to_string(), &node_id)
            .await
            .unwrap_err();
        assert_eq!(conflict.code, ErrorCode::NodeOperationConflict);

        update_deploy_progress(
            &pool,
            &task_id,
            &DeploySession {
                progress_percent: 42,
                logs: vec!["2026/07/19 Task failed: remote command --secret token".to_string()],
                is_finished: false,
                error: None,
            },
        )
        .await
        .unwrap();
        let running = get_latest_deploy_task(&pool, &node_id)
            .await
            .unwrap()
            .unwrap();
        assert!(!running.progress_logs.contains("remote command"));
        assert!(!running.progress_logs.contains("secret"));
        update_deploy_progress(
            &pool,
            &task_id,
            &DeploySession {
                progress_percent: 100,
                logs: vec!["completed".to_string()],
                is_finished: true,
                error: None,
            },
        )
        .await
        .unwrap();
        update_deploy_progress(
            &pool,
            &task_id,
            &DeploySession {
                progress_percent: 80,
                logs: vec!["late failure".to_string()],
                is_finished: true,
                error: Some("failure".to_string()),
            },
        )
        .await
        .unwrap();

        let stored = get_latest_deploy_task(&pool, &node_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, "succeeded");
        assert_eq!(stored.phase, "completed");
        assert_eq!(stored.progress_percent, Some(100));
        assert_eq!(to_deploy_session(&stored).logs, vec!["completed"]);
    }

    #[tokio::test]
    async fn startup_recovery_lists_active_tasks() {
        let pool = setup_test_db().await;
        let node_id = insert_test_node(&pool).await;
        create_deploy_task(&pool, &Uuid::new_v4().to_string(), &node_id)
            .await
            .unwrap();

        let tasks = list_interrupted_tasks(&pool).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].node_id, node_id);
        assert_eq!(tasks[0].status, "queued");
    }
}
