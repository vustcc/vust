//! 文件任务审计协调：持久化提交身份并补记 Agent 最终执行结果。

use crate::{
    models::{
        logging::{LogModule, PlatformLogLevel},
        node_runtime_client::NodeRuntimeClient,
    },
    services::logging::OperationEventBuilder,
    state::{AppState, DbPool},
    types::ApiResult,
};
use serde_json::json;
use sqlx::Row;
use std::{net::IpAddr, sync::Arc, time::Duration};
use vust_contracts::files::{
    FileOperation, FileOperationTask, FileTaskItemStatus, FileTaskStatus, FileTransfer,
    FileTransferDirection, FileTransferStatus,
};
use vust_contracts::logging::{OperationItemStatus, OperationLogItem, OperationTarget};

const AGENT_BASE_PATH: &str = "/api/v1/agent/files";

/// 新任务的可信审计上下文。
pub struct NewFileTaskAudit<'a> {
    pub task_id: &'a str,
    pub node_id: &'a str,
    pub user_id: i64,
    pub actor_name: &'a str,
    pub client_ip: &'a str,
    pub trace_id: &'a str,
    pub operation: FileOperation,
}

/// 新传输的可信审计上下文。
pub struct NewFileTransferAudit<'a> {
    pub transfer_id: &'a str,
    pub node_id: &'a str,
    pub user_id: i64,
    pub actor_name: &'a str,
    pub client_ip: &'a str,
    pub trace_id: &'a str,
    pub direction: FileTransferDirection,
    pub target_path: &'a str,
}

/// 注册任务审计；失败只记录 runtime error，不改变已成功提交的业务任务。
pub async fn register(pool: &DbPool, audit: NewFileTaskAudit<'_>) {
    if let Err(error) = sqlx::query(
        "INSERT INTO file_task_audits \
         (task_id, node_id, user_id, actor_name, client_ip, trace_id, operation, high_impact) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(node_id, task_id) DO NOTHING",
    )
    .bind(audit.task_id)
    .bind(audit.node_id)
    .bind(audit.user_id)
    .bind(audit.actor_name)
    .bind(audit.client_ip)
    .bind(audit.trace_id)
    .bind(operation_text(audit.operation))
    .bind(audit.operation == FileOperation::Remove)
    .execute(pool)
    .await
    {
        tracing::error!(%error, task_id = audit.task_id, "failed to register file task audit");
    }
}

/// 注册传输审计；失败不改变已建立的传输会话。
pub async fn register_transfer(pool: &DbPool, audit: NewFileTransferAudit<'_>) {
    if let Err(error) = sqlx::query(
        "INSERT INTO file_transfer_audits \
         (transfer_id, node_id, user_id, actor_name, client_ip, trace_id, direction, target_path) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(node_id, transfer_id) DO NOTHING",
    )
    .bind(audit.transfer_id)
    .bind(audit.node_id)
    .bind(audit.user_id)
    .bind(audit.actor_name)
    .bind(audit.client_ip)
    .bind(audit.trace_id)
    .bind(transfer_direction_text(audit.direction))
    .bind(audit.target_path)
    .execute(pool)
    .await
    {
        tracing::error!(%error, transfer_id = audit.transfer_id, "failed to register file transfer audit");
    }
}

/// 启动持久化终态协调器。
pub fn spawn_reconciler(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3));
        loop {
            interval.tick().await;
            if let Err(error) = reconcile_once(&state).await {
                tracing::error!(%error, "failed to reconcile file task audit states");
            }
            if let Err(error) = reconcile_transfers_once(&state).await {
                tracing::error!(%error, "failed to reconcile file transfer audit states");
            }
        }
    });
}

async fn reconcile_transfers_once(state: &AppState) -> ApiResult<()> {
    let rows = sqlx::query(
        "SELECT audit.transfer_id, audit.node_id, audit.user_id, audit.actor_name, \
                audit.client_ip, audit.trace_id, audit.direction, audit.target_path, \
                node.name AS node_name \
         FROM file_transfer_audits audit \
         LEFT JOIN nodes node ON node.node_id = audit.node_id \
         WHERE audit.terminal_logged = 0 ORDER BY audit.created_at ASC LIMIT 100",
    )
    .fetch_all(&state.metadata_db)
    .await?;
    for row in rows {
        let transfer_id: String = row.try_get("transfer_id")?;
        let node_id: String = row.try_get("node_id")?;
        let client =
            match NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&node_id)).await {
                Ok(client) => client,
                Err(_) => continue,
            };
        let transfer: FileTransfer = match client
            .get_domain(&format!("{AGENT_BASE_PATH}/transfer/{transfer_id}/detail"))
            .await
        {
            Ok(transfer) => transfer,
            Err(_) => continue,
        };
        sqlx::query(
            "UPDATE file_transfer_audits SET last_status = ?3 \
             WHERE node_id = ?1 AND transfer_id = ?2",
        )
        .bind(&node_id)
        .bind(&transfer_id)
        .bind(transfer_status_text(transfer.status))
        .execute(&state.metadata_db)
        .await?;
        if !is_transfer_terminal(transfer.status) {
            continue;
        }
        record_transfer_terminal(state, &row, &node_id, &transfer);
        sqlx::query(
            "UPDATE file_transfer_audits SET terminal_logged = 1, finished_at = unixepoch() \
             WHERE node_id = ?1 AND transfer_id = ?2",
        )
        .bind(&node_id)
        .bind(&transfer_id)
        .execute(&state.metadata_db)
        .await?;
    }
    Ok(())
}

async fn reconcile_once(state: &AppState) -> ApiResult<()> {
    let rows = sqlx::query(
        "SELECT audit.task_id, audit.node_id, audit.user_id, audit.actor_name, \
                audit.client_ip, audit.trace_id, audit.operation, audit.high_impact, \
                node.name AS node_name \
         FROM file_task_audits audit \
         LEFT JOIN nodes node ON node.node_id = audit.node_id \
         WHERE audit.terminal_logged = 0 ORDER BY audit.created_at ASC LIMIT 100",
    )
    .fetch_all(&state.metadata_db)
    .await?;
    for row in rows {
        let task_id: String = row.try_get("task_id")?;
        let node_id: String = row.try_get("node_id")?;
        let client =
            match NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&node_id)).await {
                Ok(client) => client,
                Err(_) => continue,
            };
        let task: FileOperationTask = match client
            .get_domain(&format!(
                "{AGENT_BASE_PATH}/operation-task/{task_id}/detail"
            ))
            .await
        {
            Ok(task) => task,
            Err(_) => continue,
        };
        update_last_status(&state.metadata_db, &node_id, &task_id, task.status).await?;
        if !is_terminal(task.status) {
            continue;
        }
        record_terminal(state, &row, &node_id, &task);
        sqlx::query(
            "UPDATE file_task_audits SET terminal_logged = 1, finished_at = unixepoch() \
             WHERE node_id = ?1 AND task_id = ?2",
        )
        .bind(&node_id)
        .bind(&task_id)
        .execute(&state.metadata_db)
        .await?;
    }
    Ok(())
}

async fn update_last_status(
    pool: &DbPool,
    node_id: &str,
    task_id: &str,
    status: FileTaskStatus,
) -> ApiResult<()> {
    sqlx::query("UPDATE file_task_audits SET last_status = ?3 WHERE node_id = ?1 AND task_id = ?2")
        .bind(node_id)
        .bind(task_id)
        .bind(status_text(status))
        .execute(pool)
        .await?;
    Ok(())
}

fn record_terminal(
    state: &AppState,
    row: &sqlx::sqlite::SqliteRow,
    node_id: &str,
    task: &FileOperationTask,
) {
    let client_ip = row
        .try_get::<String, _>("client_ip")
        .ok()
        .and_then(|value| value.parse::<IpAddr>().ok());
    let Some(client_ip) = client_ip else {
        tracing::error!(
            task_id = task.task_id,
            "file task audit has invalid client IP"
        );
        return;
    };
    let high_impact: bool = row.try_get("high_impact").unwrap_or(false);
    let node_name = row.try_get::<String, _>("node_name").ok();
    let level = if task.status == FileTaskStatus::Failed {
        PlatformLogLevel::Error
    } else if high_impact || task.status == FileTaskStatus::Cancelled {
        PlatformLogLevel::Warning
    } else {
        PlatformLogLevel::Info
    };
    OperationEventBuilder::new(
        &row.try_get::<String, _>("actor_name")
            .unwrap_or_else(|_| "unknown".to_string()),
        &format!("file_task_{}", status_text(task.status)),
        client_ip,
    )
    .user_id(row.try_get("user_id").unwrap_or_default())
    .module(LogModule::File)
    .origin_node(node_id, node_name.as_deref())
    .target_type("fileTask")
    .target_id(&task.task_id)
    .trace_id(&row.try_get::<String, _>("trace_id").unwrap_or_default())
    .source("vust_api")
    .request("POST", "/api/v1/node/{node_id}/file-operation-tasks")
    .metadata(json!({
        "nodeId": node_id,
        "operation": operation_text(task.operation),
        "result": status_text(task.status),
        "totalItemCount": task.total_item_count,
        "completedItemCount": task.completed_item_count,
        "failedItemCount": task.failed_item_count,
        "errorSummary": task.error_summary,
        "cleanupWarning": task.cleanup_warning,
    }))
    .items(operation_log_items(task))
    .outcome(file_task_outcome(task))
    .level(level)
    .finish(&state.metadata_db);
}

/// 将 Agent 文件任务结果转换为 Master 长期保存的逐项审计事实。
fn operation_log_items(task: &FileOperationTask) -> Vec<OperationLogItem> {
    task.items
        .iter()
        .take(500)
        .enumerate()
        .map(|(index, item)| OperationLogItem {
            sequence: u32::try_from(index + 1).unwrap_or(500),
            source: file_target(&item.path),
            destination: item.target_path.as_deref().map(file_target),
            status: match item.status {
                FileTaskItemStatus::Pending => OperationItemStatus::Pending,
                FileTaskItemStatus::Running => OperationItemStatus::Running,
                FileTaskItemStatus::Succeeded => OperationItemStatus::Succeeded,
                FileTaskItemStatus::Failed => OperationItemStatus::Failed,
                FileTaskItemStatus::Cancelled => OperationItemStatus::Canceled,
            },
            error_code: item.error_code.clone(),
            error_summary: item.error_summary.clone(),
        })
        .collect()
}

fn file_task_outcome(task: &FileOperationTask) -> vust_contracts::logging::OperationOutcome {
    match task.status {
        FileTaskStatus::Cancelled => vust_contracts::logging::OperationOutcome::Canceled,
        FileTaskStatus::Failed if task.completed_item_count > 0 && task.failed_item_count > 0 => {
            vust_contracts::logging::OperationOutcome::Partial
        }
        FileTaskStatus::Failed => vust_contracts::logging::OperationOutcome::Failure,
        _ => vust_contracts::logging::OperationOutcome::Success,
    }
}

fn file_target(path: &str) -> OperationTarget {
    OperationTarget {
        kind: "file".to_string(),
        id: path.to_string(),
        display_name: Some(path.to_string()),
        ownership: None,
    }
}

fn record_transfer_terminal(
    state: &AppState,
    row: &sqlx::sqlite::SqliteRow,
    node_id: &str,
    transfer: &FileTransfer,
) {
    let client_ip = row
        .try_get::<String, _>("client_ip")
        .ok()
        .and_then(|value| value.parse::<IpAddr>().ok());
    let Some(client_ip) = client_ip else {
        tracing::error!(
            transfer_id = transfer.transfer_id,
            "file transfer audit has invalid client IP"
        );
        return;
    };
    let level = match transfer.status {
        FileTransferStatus::Failed => PlatformLogLevel::Error,
        FileTransferStatus::Cancelled | FileTransferStatus::Expired => PlatformLogLevel::Warning,
        _ => PlatformLogLevel::Info,
    };
    let node_name = row.try_get::<String, _>("node_name").ok();
    OperationEventBuilder::new(
        &row.try_get::<String, _>("actor_name")
            .unwrap_or_else(|_| "unknown".to_string()),
        &format!("file_transfer_{}", transfer_status_text(transfer.status)),
        client_ip,
    )
    .user_id(row.try_get("user_id").unwrap_or_default())
    .module(LogModule::File)
    .origin_node(node_id, node_name.as_deref())
    .target_type("fileTransfer")
    .target_id(&transfer.transfer_id)
    .trace_id(&row.try_get::<String, _>("trace_id").unwrap_or_default())
    .source("vust_api")
    .request("POST", "/api/v1/node/{node_id}/file-transfers")
    .metadata(json!({
        "nodeId": node_id,
        "direction": transfer_direction_text(transfer.direction),
        "targetPath": row.try_get::<String, _>("target_path").unwrap_or_default(),
        "result": transfer_status_text(transfer.status),
        "sizeBytes": transfer.size_bytes,
        "transferredBytes": transfer.transferred_bytes,
        "errorSummary": transfer.error_summary,
    }))
    .outcome(match transfer.status {
        FileTransferStatus::Cancelled => vust_contracts::logging::OperationOutcome::Canceled,
        FileTransferStatus::Expired => vust_contracts::logging::OperationOutcome::TimedOut,
        FileTransferStatus::Failed => vust_contracts::logging::OperationOutcome::Failure,
        _ => vust_contracts::logging::OperationOutcome::Success,
    })
    .level(level)
    .finish(&state.metadata_db);
}

fn is_terminal(status: FileTaskStatus) -> bool {
    matches!(
        status,
        FileTaskStatus::Succeeded | FileTaskStatus::Failed | FileTaskStatus::Cancelled
    )
}

fn is_transfer_terminal(status: FileTransferStatus) -> bool {
    matches!(
        status,
        FileTransferStatus::Completed
            | FileTransferStatus::Failed
            | FileTransferStatus::Cancelled
            | FileTransferStatus::Expired
    )
}

fn transfer_direction_text(direction: FileTransferDirection) -> &'static str {
    match direction {
        FileTransferDirection::Upload => "upload",
        FileTransferDirection::Download => "download",
    }
}

fn transfer_status_text(status: FileTransferStatus) -> &'static str {
    match status {
        FileTransferStatus::Created => "created",
        FileTransferStatus::Receiving => "receiving",
        FileTransferStatus::Ready => "ready",
        FileTransferStatus::Streaming => "streaming",
        FileTransferStatus::Completed => "completed",
        FileTransferStatus::Failed => "failed",
        FileTransferStatus::Cancelled => "cancelled",
        FileTransferStatus::Expired => "expired",
    }
}

fn operation_text(operation: FileOperation) -> &'static str {
    match operation {
        FileOperation::Copy => "copy",
        FileOperation::Move => "move",
        FileOperation::Remove => "remove",
    }
}

fn status_text(status: FileTaskStatus) -> &'static str {
    match status {
        FileTaskStatus::Queued => "queued",
        FileTaskStatus::Running => "running",
        FileTaskStatus::Cancelling => "cancelling",
        FileTaskStatus::Succeeded => "succeeded",
        FileTaskStatus::Failed => "failed",
        FileTaskStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vust_contracts::files::{FileOperationItemResult, FileTaskStage};

    #[test]
    fn terminal_states_are_explicit() {
        assert!(is_terminal(FileTaskStatus::Succeeded));
        assert!(is_terminal(FileTaskStatus::Failed));
        assert!(is_terminal(FileTaskStatus::Cancelled));
        assert!(!is_terminal(FileTaskStatus::Running));
        assert!(is_transfer_terminal(FileTransferStatus::Completed));
        assert!(is_transfer_terminal(FileTransferStatus::Expired));
        assert!(!is_transfer_terminal(FileTransferStatus::Streaming));
    }

    #[test]
    fn file_task_items_preserve_sources_destinations_and_terminal_states() {
        let task = FileOperationTask {
            task_id: "task-1".to_string(),
            node_id: "local".to_string(),
            operation: FileOperation::Move,
            status: FileTaskStatus::Failed,
            stage: FileTaskStage::Failed,
            progress_percent: 100,
            total_item_count: 2,
            completed_item_count: 1,
            failed_item_count: 1,
            total_bytes: 0,
            processed_bytes: 0,
            items: vec![
                FileOperationItemResult {
                    path: "/source/a".to_string(),
                    target_path: Some("/target/a".to_string()),
                    status: FileTaskItemStatus::Succeeded,
                    error_code: None,
                    error_summary: None,
                },
                FileOperationItemResult {
                    path: "/source/b".to_string(),
                    target_path: Some("/target/b".to_string()),
                    status: FileTaskItemStatus::Cancelled,
                    error_code: Some("MOVE_CANCELLED".to_string()),
                    error_summary: Some("cancelled safely".to_string()),
                },
            ],
            error_summary: Some("partial failure".to_string()),
            cleanup_warning: None,
            created_at: "2026-07-27T00:00:00Z".to_string(),
            started_at: Some("2026-07-27T00:00:01Z".to_string()),
            finished_at: Some("2026-07-27T00:00:02Z".to_string()),
        };

        let items = operation_log_items(&task);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source.id, "/source/a");
        assert_eq!(
            items[0].destination.as_ref().map(|value| value.id.as_str()),
            Some("/target/a")
        );
        assert_eq!(items[0].status, OperationItemStatus::Succeeded);
        assert_eq!(items[1].status, OperationItemStatus::Canceled);
        assert_eq!(items[1].error_code.as_deref(), Some("MOVE_CANCELLED"));
        assert_eq!(
            file_task_outcome(&task),
            vust_contracts::logging::OperationOutcome::Partial
        );
    }

    #[test]
    fn operation_log_items_are_bounded_to_five_hundred() {
        let item = FileOperationItemResult {
            path: "/source".to_string(),
            target_path: None,
            status: FileTaskItemStatus::Pending,
            error_code: None,
            error_summary: None,
        };
        let task = FileOperationTask {
            task_id: "task-1".to_string(),
            node_id: "local".to_string(),
            operation: FileOperation::Remove,
            status: FileTaskStatus::Cancelled,
            stage: FileTaskStage::Cancelled,
            progress_percent: 0,
            total_item_count: 501,
            completed_item_count: 0,
            failed_item_count: 0,
            total_bytes: 0,
            processed_bytes: 0,
            items: vec![item; 501],
            error_summary: None,
            cleanup_warning: None,
            created_at: "2026-07-27T00:00:00Z".to_string(),
            started_at: None,
            finished_at: Some("2026-07-27T00:00:01Z".to_string()),
        };

        assert_eq!(operation_log_items(&task).len(), 500);
    }
}
