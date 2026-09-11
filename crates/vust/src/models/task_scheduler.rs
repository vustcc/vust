//! Master 计划任务仓储：定义、读模型、运行、后台操作与 Agent 快照。

use crate::{
    state::DbPool,
    types::{ApiError, ApiResult},
};
use chrono::Utc;
use sqlx::{FromRow, QueryBuilder, Row, Sqlite};
use vust_contracts::{
    api::ErrorCode,
    scheduled_tasks::{
        AgentScheduledTaskDefinition, AgentScheduledTaskRunReport, CreateScheduledTaskRequest,
        ScheduledTaskBatch, ScheduledTaskBatchAction, ScheduledTaskBatchItem,
        ScheduledTaskDeploymentStatus, ScheduledTaskDesiredState, ScheduledTaskOperation,
        ScheduledTaskOperationKind, ScheduledTaskOperationStatus, ScheduledTaskOwnership,
        ScheduledTaskOwnershipKind, ScheduledTaskRun, ScheduledTaskRunCapabilities,
        ScheduledTaskRunOutput, ScheduledTaskRunOutputSummary, ScheduledTaskRunStatus,
        ScheduledTaskTriggerSource, UpdateScheduledTaskRequest,
    },
};

/// 可信操作发起者上下文，随后台操作持久化。
#[derive(Debug, Clone)]
pub struct OperationActor {
    pub user_id: i64,
    pub name: String,
    pub client_ip: String,
    pub trace_id: String,
}

/// 计划任务列表查询条件。
pub struct TaskListFilter<'a> {
    pub node_id: Option<&'a str>,
    pub keyword: Option<&'a str>,
    pub enabled: Option<bool>,
    pub deployment_status: Option<ScheduledTaskDeploymentStatus>,
    pub page: u32,
    pub page_size: u32,
    pub sort_by: &'a str,
    pub sort_order: &'a str,
}

/// 列表和详情共用的 Master 读模型行。
#[derive(Debug, Clone, FromRow)]
pub struct ScheduledTaskRow {
    pub task_id: String,
    pub name: String,
    pub description: Option<String>,
    pub node_id: String,
    pub node_name: String,
    pub node_status: String,
    pub command: String,
    pub cron_expr: String,
    pub time_zone: String,
    pub desired_state: String,
    pub timeout_seconds: i64,
    pub prevent_overlap: bool,
    pub ownership_kind: String,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    pub manager_path: Option<String>,
    pub created_by_user_id: i64,
    pub created_by_name: String,
    pub revision: i64,
    pub deployment_status: String,
    pub deployment_error_summary: Option<String>,
    pub last_synced_at: Option<String>,
    pub next_run_status: String,
    pub next_run_at: Option<String>,
    pub last_run_id: Option<String>,
    pub last_run_status: Option<String>,
    pub last_run_finished_at: Option<String>,
    pub has_active_run: bool,
    pub has_active_operation: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Master 持久化的运行行。
#[derive(Debug, Clone, FromRow)]
pub struct ScheduledTaskRunRow {
    pub run_id: String,
    pub task_id: String,
    pub node_id: String,
    pub trigger_source: String,
    pub status: String,
    pub phase: Option<String>,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub output_size_bytes: i64,
    pub output_truncated: bool,
    pub actor_user_id: Option<i64>,
    pub actor_name: Option<String>,
    pub client_ip: Option<String>,
    pub trace_id: Option<String>,
    pub terminal_logged_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 后台运行终态日志所需的可信提交上下文。
pub struct ScheduledTaskRunAudit {
    pub run_id: String,
    pub task_id: String,
    pub status: ScheduledTaskRunStatus,
    pub actor_user_id: i64,
    pub actor_name: String,
    pub client_ip: String,
    pub trace_id: String,
    pub error_code: Option<String>,
}

/// Master 持久化的后台操作行。
#[derive(Debug, Clone, FromRow)]
pub struct ScheduledTaskOperationRow {
    pub operation_id: String,
    pub task_id: String,
    pub kind: String,
    pub status: String,
    pub phase: Option<String>,
    pub completed_steps: i64,
    pub total_steps: i64,
    pub source_node_id: Option<String>,
    pub target_node_id: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub warning_summary: Option<String>,
    pub cancel_requested: bool,
    pub attempts: i64,
    pub last_attempt_at: Option<String>,
    pub actor_user_id: i64,
    pub actor_name: String,
    pub client_ip: String,
    pub trace_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

const TASK_SELECT: &str = r#"
    SELECT t.task_id, t.name, t.description, t.node_id,
           CASE WHEN t.node_id = 'local' THEN 'Local Node' ELSE COALESCE(n.name, t.node_id) END AS node_name,
           CASE
               WHEN EXISTS(
                   SELECT 1
                     FROM node_sessions ns
                    WHERE ns.node_id = t.node_id
                      AND ns.status = 'active'
                      AND julianday(ns.lease_expires_at) >= julianday('now')
               ) THEN 'online'
               ELSE 'offline'
           END AS node_status,
           t.command, t.cron_expr, t.time_zone, t.desired_state, t.timeout_seconds,
           t.prevent_overlap, t.ownership_kind, t.owner_id, t.owner_name, t.manager_path,
           t.created_by_user_id, t.created_by_name,
           t.revision, t.deployment_status, t.deployment_error_summary, t.last_synced_at,
           t.next_run_status, t.next_run_at, t.last_run_id,
           lr.status AS last_run_status, lr.finished_at AS last_run_finished_at,
           EXISTS(SELECT 1 FROM scheduled_task_runs ar WHERE ar.task_id = t.task_id
                  AND ar.status IN ('queued','starting','running','cancelling')) AS has_active_run,
           EXISTS(SELECT 1 FROM scheduled_task_operations ao WHERE ao.task_id = t.task_id
                  AND ao.status IN ('queued','running','cancelling')) AS has_active_operation,
           t.created_at, t.updated_at
      FROM scheduled_tasks t
      LEFT JOIN nodes n ON n.node_id = t.node_id
      LEFT JOIN scheduled_task_runs lr ON lr.run_id = t.last_run_id
"#;

/// 服务端过滤、排序和分页的 custom 任务列表。
pub async fn list_tasks(
    pool: &DbPool,
    filter: &TaskListFilter<'_>,
) -> ApiResult<(Vec<ScheduledTaskRow>, u64)> {
    let mut count = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(*) FROM scheduled_tasks t WHERE t.ownership_kind = 'custom' AND t.deleted_at IS NULL",
    );
    push_filters(&mut count, filter);
    let total: i64 = count.build_query_scalar().fetch_one(pool).await?;

    let mut query = QueryBuilder::<Sqlite>::new(TASK_SELECT);
    query.push(" WHERE t.ownership_kind = 'custom' AND t.deleted_at IS NULL");
    push_filters(&mut query, filter);
    query.push(" ORDER BY ");
    match filter.sort_by {
        "name" => query.push("t.name_key"),
        "nextRunAt" => query.push("t.next_run_at"),
        _ => query.push("t.updated_at"),
    };
    if filter.sort_order == "asc" {
        query.push(" ASC");
    } else {
        query.push(" DESC");
    }
    query
        .push(", t.task_id ASC LIMIT ")
        .push_bind(i64::from(filter.page_size))
        .push(" OFFSET ")
        .push_bind(i64::from((filter.page - 1) * filter.page_size));
    let rows = query.build_query_as().fetch_all(pool).await?;
    Ok((rows, total.max(0) as u64))
}

fn push_filters<'a>(builder: &mut QueryBuilder<'a, Sqlite>, filter: &TaskListFilter<'a>) {
    if let Some(node_id) = filter.node_id {
        builder.push(" AND t.node_id = ").push_bind(node_id);
    }
    if let Some(keyword) = filter.keyword.filter(|value| !value.is_empty()) {
        let pattern = format!("%{}%", keyword.to_lowercase());
        builder
            .push(" AND (t.name_key LIKE ")
            .push_bind(pattern.clone())
            .push(" OR lower(COALESCE(t.description, '')) LIKE ")
            .push_bind(pattern)
            .push(")");
    }
    if let Some(enabled) = filter.enabled {
        builder
            .push(" AND t.desired_state = ")
            .push_bind(if enabled { "enabled" } else { "disabled" });
    }
    if let Some(status) = filter.deployment_status {
        builder
            .push(" AND t.deployment_status = ")
            .push_bind(deployment_status_text(status));
    }
}

/// 查询任务详情读模型。
pub async fn get_task(pool: &DbPool, task_id: &str) -> ApiResult<Option<ScheduledTaskRow>> {
    let mut query = QueryBuilder::<Sqlite>::new(TASK_SELECT);
    query.push(" WHERE t.task_id = ").push_bind(task_id);
    Ok(query.build_query_as().fetch_optional(pool).await?)
}

/// 创建任务定义和可恢复部署操作。
pub async fn create_task(
    pool: &DbPool,
    request: &CreateScheduledTaskRequest,
    actor: &OperationActor,
) -> ApiResult<(ScheduledTaskRow, ScheduledTaskOperationRow)> {
    let task_id = uuid::Uuid::now_v7().to_string();
    let operation_id = uuid::Uuid::now_v7().to_string();
    let now = now_string();
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"INSERT INTO scheduled_tasks (
            task_id, name, name_key, description, node_id, command, cron_expr, time_zone,
            desired_state, timeout_seconds, prevent_overlap, ownership_kind, revision,
            created_by_user_id, created_by_name, deployment_status, next_run_status, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'custom', 1, ?, ?, 'pending', 'not_deployed', ?, ?)"#,
    )
    .bind(&task_id)
    .bind(request.name.trim())
    .bind(normalize_name(&request.name))
    .bind(normalize_optional(request.description.as_deref()))
    .bind(request.node_id.trim())
    .bind(&request.command)
    .bind(request.cron_expr.trim())
    .bind(request.time_zone.trim())
    .bind(if request.enabled {
        "enabled"
    } else {
        "disabled"
    })
    .bind(i64::from(request.timeout_seconds))
    .bind(request.prevent_overlap)
    .bind(actor.user_id)
    .bind(&actor.name)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    if let Err(error) = result {
        if is_unique_violation(&error) {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskAlreadyExists,
                "a scheduled task with the same name already exists on this node",
            ));
        }
        return Err(error.into());
    }
    insert_operation(
        &mut tx,
        &operation_id,
        &task_id,
        ScheduledTaskOperationKind::Deploy,
        "queued",
        1,
        Some(request.node_id.trim()),
        None,
        actor,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok((
        required_task(pool, &task_id).await?,
        required_operation(pool, &operation_id).await?,
    ))
}

/// 编辑任务并创建部署操作；nodeId 和 ownership 保持不变。
pub async fn update_task(
    pool: &DbPool,
    task_id: &str,
    request: &UpdateScheduledTaskRequest,
    actor: &OperationActor,
) -> ApiResult<(ScheduledTaskRow, ScheduledTaskOperationRow)> {
    ensure_mutable_and_idle(pool, task_id, false).await?;
    let operation_id = uuid::Uuid::now_v7().to_string();
    let now = now_string();
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        "UPDATE scheduled_tasks SET name = ?, name_key = ?, description = ?, command = ?, cron_expr = ?, time_zone = ?, timeout_seconds = ?, prevent_overlap = ?, revision = revision + 1, deployment_status = 'pending', deployment_error_summary = NULL, next_run_status = 'not_deployed', next_run_at = NULL, updated_at = ? WHERE task_id = ?",
    )
    .bind(request.name.trim())
    .bind(normalize_name(&request.name))
    .bind(normalize_optional(request.description.as_deref()))
    .bind(&request.command)
    .bind(request.cron_expr.trim())
    .bind(request.time_zone.trim())
    .bind(i64::from(request.timeout_seconds))
    .bind(request.prevent_overlap)
    .bind(&now)
    .bind(task_id)
    .execute(&mut *tx)
    .await;
    if let Err(error) = result {
        if is_unique_violation(&error) {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskAlreadyExists,
                "a scheduled task with the same name already exists on this node",
            ));
        }
        return Err(error.into());
    }
    let node_id: String =
        sqlx::query_scalar("SELECT node_id FROM scheduled_tasks WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await?;
    insert_operation(
        &mut tx,
        &operation_id,
        task_id,
        ScheduledTaskOperationKind::Update,
        "queued",
        1,
        Some(&node_id),
        None,
        actor,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok((
        required_task(pool, task_id).await?,
        required_operation(pool, &operation_id).await?,
    ))
}

/// 更新期望启停状态并创建部署操作。
pub async fn update_task_state(
    pool: &DbPool,
    task_id: &str,
    enabled: bool,
    actor: &OperationActor,
) -> ApiResult<(ScheduledTaskRow, ScheduledTaskOperationRow)> {
    ensure_mutable_and_idle(pool, task_id, false).await?;
    let operation_id = uuid::Uuid::now_v7().to_string();
    let now = now_string();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE scheduled_tasks SET desired_state = ?, revision = revision + 1, deployment_status = 'pending', deployment_error_summary = NULL, next_run_status = ?, next_run_at = NULL, updated_at = ? WHERE task_id = ?")
        .bind(if enabled { "enabled" } else { "disabled" })
        .bind(if enabled { "not_deployed" } else { "disabled" })
        .bind(&now)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    let node_id: String =
        sqlx::query_scalar("SELECT node_id FROM scheduled_tasks WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await?;
    insert_operation(
        &mut tx,
        &operation_id,
        task_id,
        ScheduledTaskOperationKind::StateChange,
        "queued",
        1,
        Some(&node_id),
        None,
        actor,
        &now,
    )
    .await?;
    tx.commit().await?;
    Ok((
        required_task(pool, task_id).await?,
        required_operation(pool, &operation_id).await?,
    ))
}

/// 创建带 tombstone 的删除操作。
pub async fn request_remove(
    pool: &DbPool,
    task_id: &str,
    actor: &OperationActor,
) -> ApiResult<ScheduledTaskOperationRow> {
    ensure_mutable_and_idle(pool, task_id, true).await?;
    let task = required_task(pool, task_id).await?;
    let operation_id = uuid::Uuid::now_v7().to_string();
    let now = now_string();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE scheduled_tasks SET deleted_at = ?, deployment_status = 'deleting', updated_at = ? WHERE task_id = ?")
        .bind(&now).bind(&now).bind(task_id).execute(&mut *tx).await?;
    insert_operation(
        &mut tx,
        &operation_id,
        task_id,
        ScheduledTaskOperationKind::Remove,
        "queued",
        3,
        Some(&task.node_id),
        None,
        actor,
        &now,
    )
    .await?;
    tx.commit().await?;
    required_operation(pool, &operation_id).await
}

/// 创建独立节点迁移操作。
pub async fn request_migration(
    pool: &DbPool,
    task_id: &str,
    target_node_id: &str,
    actor: &OperationActor,
) -> ApiResult<ScheduledTaskOperationRow> {
    ensure_mutable_and_idle(pool, task_id, true).await?;
    let task = required_task(pool, task_id).await?;
    if task.node_id == target_node_id {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskOperationConflict,
            "scheduled task already belongs to the target node",
        ));
    }
    let operation_id = uuid::Uuid::now_v7().to_string();
    let now = now_string();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE scheduled_tasks SET deployment_status = 'migrating', updated_at = ? WHERE task_id = ?")
        .bind(&now).bind(task_id).execute(&mut *tx).await?;
    insert_operation(
        &mut tx,
        &operation_id,
        task_id,
        ScheduledTaskOperationKind::Migrate,
        "queued",
        6,
        Some(&task.node_id),
        Some(target_node_id),
        actor,
        &now,
    )
    .await?;
    tx.commit().await?;
    required_operation(pool, &operation_id).await
}

/// 创建一次可恢复的手动或批量运行。
pub async fn create_run(
    pool: &DbPool,
    task_id: &str,
    trigger: ScheduledTaskTriggerSource,
    actor: Option<&OperationActor>,
) -> ApiResult<ScheduledTaskRunRow> {
    let task = ensure_mutable_and_idle(pool, task_id, false).await?;
    if task.deployment_status != "ready" {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskNodeUnavailable,
            "scheduled task is not ready on its execution node",
        ));
    }
    let run_id = uuid::Uuid::now_v7().to_string();
    let now = now_string();
    let actor_user_id = actor.map_or(task.created_by_user_id, |value| value.user_id);
    let actor_name = actor.map_or(task.created_by_name.as_str(), |value| value.name.as_str());
    let client_ip = actor.map_or("127.0.0.1", |value| value.client_ip.as_str());
    let trace_id = actor
        .map(|value| value.trace_id.clone())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let result = sqlx::query("INSERT INTO scheduled_task_runs (run_id, task_id, node_id, trigger_source, status, phase, queued_at, overlap_guard, actor_user_id, actor_name, client_ip, trace_id, created_at, updated_at) VALUES (?, ?, ?, ?, 'queued', 'queued', ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&run_id).bind(task_id).bind(&task.node_id).bind(trigger_source_text(trigger))
        .bind(&now).bind(task.prevent_overlap.then_some(task_id))
        .bind(actor_user_id).bind(actor_name).bind(client_ip).bind(trace_id)
        .bind(&now).bind(&now).execute(pool).await;
    if let Err(error) = result {
        if is_unique_violation(&error) {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskOperationConflict,
                "scheduled task already has an active run",
            ));
        }
        return Err(error.into());
    }
    required_run(pool, &run_id).await
}

/// 原子领取一次带可信用户上下文的运行终态日志，幂等上报不会重复记日志。
pub async fn claim_run_terminal_audit(
    pool: &DbPool,
    run_id: &str,
) -> ApiResult<Option<ScheduledTaskRunAudit>> {
    let row = sqlx::query("UPDATE scheduled_task_runs SET terminal_logged_at = ? WHERE run_id = ? AND terminal_logged_at IS NULL AND actor_user_id IS NOT NULL AND status IN ('succeeded','failed','timed_out','cancelled') RETURNING run_id, task_id, status, actor_user_id, actor_name, client_ip, trace_id, error_code")
        .bind(now_string())
        .bind(run_id)
        .fetch_optional(pool)
        .await?;
    row.map(|value| {
        Ok(ScheduledTaskRunAudit {
            run_id: value.try_get("run_id")?,
            task_id: value.try_get("task_id")?,
            status: run_status_from_text(value.try_get::<String, _>("status")?.as_str())?,
            actor_user_id: value.try_get("actor_user_id")?,
            actor_name: value.try_get("actor_name")?,
            client_ip: value.try_get("client_ip")?,
            trace_id: value.try_get("trace_id")?,
            error_code: value.try_get("error_code")?,
        })
    })
    .transpose()
}

/// 分页读取 Master 运行读模型。
pub async fn list_runs(
    pool: &DbPool,
    task_id: &str,
    page: u32,
    page_size: u32,
) -> ApiResult<(Vec<ScheduledTaskRunRow>, u64)> {
    required_task(pool, task_id).await?;
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_task_runs WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(pool)
            .await?;
    let items = sqlx::query_as::<_, ScheduledTaskRunRow>("SELECT * FROM scheduled_task_runs WHERE task_id = ? ORDER BY queued_at DESC, run_id DESC LIMIT ? OFFSET ?")
        .bind(task_id).bind(i64::from(page_size)).bind(i64::from((page - 1) * page_size))
        .fetch_all(pool).await?;
    Ok((items, total.max(0) as u64))
}

/// 查询单次运行。
pub async fn get_run(pool: &DbPool, task_id: &str, run_id: &str) -> ApiResult<ScheduledTaskRunRow> {
    sqlx::query_as::<_, ScheduledTaskRunRow>(
        "SELECT * FROM scheduled_task_runs WHERE task_id = ? AND run_id = ?",
    )
    .bind(task_id)
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            ErrorCode::ScheduledTaskRunNotFound,
            "scheduled task run not found",
        )
    })
}

/// 读取运行输出分页。
pub async fn read_output(
    pool: &DbPool,
    task_id: &str,
    run_id: &str,
    offset: u64,
    limit: u32,
) -> ApiResult<ScheduledTaskRunOutput> {
    get_run(pool, task_id, run_id).await?;
    let row = sqlx::query(
        "SELECT content, size_bytes, truncated FROM scheduled_task_run_outputs WHERE run_id = ?",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            ErrorCode::ScheduledTaskRunNotFound,
            "scheduled task run output not found",
        )
    })?;
    let content: Vec<u8> = row.try_get("content")?;
    let start = (offset.min(content.len() as u64)) as usize;
    let end = (start + limit.clamp(1, 64 * 1024) as usize).min(content.len());
    Ok(ScheduledTaskRunOutput {
        run_id: run_id.to_string(),
        content: String::from_utf8_lossy(&content[start..end]).to_string(),
        offset_bytes: start as u64,
        next_offset_bytes: (end < content.len()).then_some(end as u64),
        size_bytes: row.try_get::<i64, _>("size_bytes")?.max(0) as u64,
        truncated: row.try_get("truncated")?,
    })
}

/// 将 Master 运行标记为取消中。
pub async fn mark_run_cancelling(
    pool: &DbPool,
    task_id: &str,
    run_id: &str,
) -> ApiResult<ScheduledTaskRunRow> {
    let run = get_run(pool, task_id, run_id).await?;
    if run_status_from_text(&run.status)?.is_terminal() {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskRunNotCancellable,
            "scheduled task run is already terminal",
        ));
    }
    sqlx::query("UPDATE scheduled_task_runs SET status = 'cancelling', phase = 'cancelling', updated_at = ? WHERE run_id = ?")
        .bind(now_string()).bind(run_id).execute(pool).await?;
    required_run(pool, run_id).await
}

/// 返回待后台消费的任务操作。
pub async fn list_pending_operations(
    pool: &DbPool,
    limit: i64,
) -> ApiResult<Vec<ScheduledTaskOperationRow>> {
    Ok(sqlx::query_as::<_, ScheduledTaskOperationRow>("SELECT * FROM scheduled_task_operations WHERE status IN ('queued','running','cancelling') ORDER BY created_at, operation_id LIMIT ?")
        .bind(limit.clamp(1, 100)).fetch_all(pool).await?)
}

/// 返回待下发的手动/批量运行。
pub async fn list_queued_runs(pool: &DbPool, limit: i64) -> ApiResult<Vec<ScheduledTaskRunRow>> {
    Ok(sqlx::query_as::<_, ScheduledTaskRunRow>("SELECT * FROM scheduled_task_runs WHERE status = 'queued' AND trigger_source IN ('manual','batch') ORDER BY queued_at, run_id LIMIT ?")
        .bind(limit.clamp(1, 100)).fetch_all(pool).await?)
}

/// 查询后台操作。
pub async fn get_operation(
    pool: &DbPool,
    operation_id: &str,
) -> ApiResult<ScheduledTaskOperationRow> {
    required_operation(pool, operation_id).await
}

/// 请求取消仍可取消的后台操作。
pub async fn request_operation_cancel(
    pool: &DbPool,
    operation_id: &str,
) -> ApiResult<ScheduledTaskOperationRow> {
    let operation = required_operation(pool, operation_id).await?;
    if operation_status_from_text(&operation.status)?.is_terminal()
        || operation.completed_steps > 0
        || !matches!(operation.kind.as_str(), "remove" | "migrate")
    {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskOperationConflict,
            "scheduled task operation can no longer be cancelled",
        ));
    }
    sqlx::query("UPDATE scheduled_task_operations SET status = 'cancelling', cancel_requested = 1, phase = 'cancelling', updated_at = ? WHERE operation_id = ?")
        .bind(now_string()).bind(operation_id).execute(pool).await?;
    required_operation(pool, operation_id).await
}

/// 持久化可恢复的批量操作及逐项结果。
pub async fn save_batch(
    pool: &DbPool,
    batch_id: &str,
    action: ScheduledTaskBatchAction,
    actor: &OperationActor,
    items: &[ScheduledTaskBatchItem],
) -> ApiResult<ScheduledTaskBatch> {
    let now = now_string();
    let status = if items.iter().all(|item| item.error_code.is_none()) {
        "running"
    } else if items.iter().all(|item| item.error_code.is_some()) {
        "failed"
    } else {
        "partial"
    };
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO scheduled_task_batches (batch_id, action, status, actor_user_id, actor_name, client_ip, trace_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(batch_id).bind(batch_action_text(action)).bind(status).bind(actor.user_id)
        .bind(&actor.name).bind(&actor.client_ip).bind(&actor.trace_id).bind(&now).bind(&now)
        .execute(&mut *tx).await?;
    for item in items {
        sqlx::query("INSERT INTO scheduled_task_batch_items (batch_id, task_id, run_id, operation_id, error_code, error_summary) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(batch_id).bind(&item.task_id).bind(item.run_id.as_deref()).bind(item.operation_id.as_deref())
            .bind(item.error_code.as_deref()).bind(item.error_summary.as_deref()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    get_batch(pool, batch_id).await
}

/// 查询批量操作并根据子任务实时收敛状态。
pub async fn get_batch(pool: &DbPool, batch_id: &str) -> ApiResult<ScheduledTaskBatch> {
    let row = sqlx::query("SELECT action, status, created_at, updated_at FROM scheduled_task_batches WHERE batch_id = ?")
        .bind(batch_id).fetch_optional(pool).await?
        .ok_or_else(|| ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task batch not found"))?;
    let items = sqlx::query("SELECT task_id, run_id, operation_id, error_code, error_summary FROM scheduled_task_batch_items WHERE batch_id = ? ORDER BY task_id")
        .bind(batch_id).fetch_all(pool).await?.into_iter().map(|item| ScheduledTaskBatchItem {
            task_id: item.get("task_id"), run_id: item.get("run_id"), operation_id: item.get("operation_id"),
            error_code: item.get("error_code"), error_summary: item.get("error_summary"),
        }).collect::<Vec<_>>();
    let mut status = operation_status_from_text(row.get::<String, _>("status").as_str())?;
    if status == ScheduledTaskOperationStatus::Running {
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_task_batch_items bi LEFT JOIN scheduled_task_operations o ON o.operation_id = bi.operation_id LEFT JOIN scheduled_task_runs r ON r.run_id = bi.run_id WHERE bi.batch_id = ? AND (o.status IN ('queued','running','cancelling') OR r.status IN ('queued','starting','running','cancelling'))")
            .bind(batch_id).fetch_one(pool).await?;
        if active == 0 {
            status = ScheduledTaskOperationStatus::Succeeded;
            sqlx::query("UPDATE scheduled_task_batches SET status = 'succeeded', updated_at = ? WHERE batch_id = ?")
                .bind(now_string()).bind(batch_id).execute(pool).await?;
        }
    }
    Ok(ScheduledTaskBatch {
        batch_id: batch_id.to_string(),
        action: batch_action_from_text(row.get::<String, _>("action").as_str())?,
        status,
        items,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

/// 更新后台操作进度。
pub async fn update_operation_progress(
    pool: &DbPool,
    operation_id: &str,
    phase: &str,
    completed_steps: i64,
) -> ApiResult<()> {
    let now = now_string();
    sqlx::query("UPDATE scheduled_task_operations SET status = 'running', phase = ?, completed_steps = ?, attempts = attempts + 1, last_attempt_at = ?, updated_at = ? WHERE operation_id = ?")
        .bind(phase).bind(completed_steps).bind(&now).bind(&now).bind(operation_id).execute(pool).await?;
    Ok(())
}

/// 收敛后台操作终态。
pub async fn finish_operation(
    pool: &DbPool,
    operation_id: &str,
    status: ScheduledTaskOperationStatus,
    error_code: Option<&str>,
    error_summary: Option<&str>,
    warning_summary: Option<&str>,
) -> ApiResult<()> {
    let now = now_string();
    sqlx::query("UPDATE scheduled_task_operations SET status = ?, phase = NULL, completed_steps = CASE WHEN ? IN ('succeeded','partial') THEN total_steps ELSE completed_steps END, error_code = ?, error_summary = ?, warning_summary = ?, updated_at = ?, finished_at = ? WHERE operation_id = ?")
        .bind(operation_status_text(status)).bind(operation_status_text(status)).bind(error_code).bind(error_summary).bind(warning_summary).bind(&now).bind(&now).bind(operation_id)
        .execute(pool).await?;
    Ok(())
}

/// 更新任务部署读模型。
pub async fn update_deployment(
    pool: &DbPool,
    task_id: &str,
    status: ScheduledTaskDeploymentStatus,
    next_run_at: Option<&str>,
    error_summary: Option<&str>,
) -> ApiResult<()> {
    let next_status = match status {
        ScheduledTaskDeploymentStatus::Ready if next_run_at.is_some() => "scheduled",
        ScheduledTaskDeploymentStatus::Ready => "disabled",
        ScheduledTaskDeploymentStatus::WaitingForNode => "unavailable",
        _ => "not_deployed",
    };
    let synced_at = (status == ScheduledTaskDeploymentStatus::Ready).then(now_string);
    sqlx::query("UPDATE scheduled_tasks SET deployment_status = ?, deployment_error_summary = ?, last_synced_at = COALESCE(?, last_synced_at), next_run_status = ?, next_run_at = ?, updated_at = ? WHERE task_id = ?")
        .bind(deployment_status_text(status)).bind(error_summary).bind(synced_at).bind(next_status).bind(next_run_at).bind(now_string()).bind(task_id)
        .execute(pool).await?;
    Ok(())
}

/// 在迁移切换点原子更新执行节点。
pub async fn move_task_node(pool: &DbPool, task_id: &str, node_id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE scheduled_tasks SET node_id = ?, updated_at = ? WHERE task_id = ?")
        .bind(node_id)
        .bind(now_string())
        .bind(task_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Agent 确认删除后清理 Master 定义。
pub async fn hard_delete_task(pool: &DbPool, task_id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM scheduled_tasks WHERE task_id = ?")
        .bind(task_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 将 Agent 终态幂等写入 Master 读模型并校验节点归属。
pub async fn save_run_report(
    pool: &DbPool,
    node_id: &str,
    report: &AgentScheduledTaskRunReport,
) -> ApiResult<()> {
    let task = required_task(pool, &report.run.task_id).await?;
    if task.node_id != node_id || report.run.node_id != node_id {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskOperationConflict,
            "scheduled task run report node does not own this task",
        ));
    }
    let run = &report.run;
    if let Some(existing) = sqlx::query_as::<_, ScheduledTaskRunRow>(
        "SELECT * FROM scheduled_task_runs WHERE run_id = ?",
    )
    .bind(&run.run_id)
    .fetch_optional(pool)
    .await?
    {
        if existing.task_id != run.task_id || existing.node_id != node_id {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskOperationConflict,
                "scheduled task run report does not match the existing run",
            ));
        }
        let previous = run_status_from_text(&existing.status)?;
        if !is_valid_run_transition(previous, run.status) {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskOperationConflict,
                "scheduled task run report contains an invalid status transition",
            ));
        }
    }
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO scheduled_task_runs (run_id, task_id, node_id, trigger_source, status, phase, queued_at, started_at, finished_at, exit_code, error_code, error_summary, output_size_bytes, output_truncated, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(run_id) DO UPDATE SET status = excluded.status, phase = excluded.phase, started_at = excluded.started_at, finished_at = excluded.finished_at, exit_code = excluded.exit_code, error_code = excluded.error_code, error_summary = excluded.error_summary, output_size_bytes = excluded.output_size_bytes, output_truncated = excluded.output_truncated, updated_at = excluded.updated_at")
        .bind(&run.run_id).bind(&run.task_id).bind(node_id).bind(trigger_source_text(run.trigger_source))
        .bind(run_status_text(run.status)).bind(run.phase.as_deref()).bind(&run.queued_at).bind(run.started_at.as_deref()).bind(run.finished_at.as_deref())
        .bind(run.exit_code).bind(run.error_code.as_deref()).bind(run.error_summary.as_deref()).bind(run.output.size_bytes as i64).bind(run.output.truncated)
        .bind(&run.queued_at).bind(now_string()).execute(&mut *tx).await?;
    if let Some(output) = &report.output_content {
        let bytes = output.as_bytes();
        let capped = &bytes[..bytes.len().min(256 * 1024)];
        sqlx::query("INSERT INTO scheduled_task_run_outputs (run_id, content, size_bytes, truncated, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(run_id) DO UPDATE SET content = excluded.content, size_bytes = excluded.size_bytes, truncated = excluded.truncated, updated_at = excluded.updated_at")
            .bind(&run.run_id).bind(capped).bind(capped.len() as i64).bind(run.output.truncated || bytes.len() > capped.len()).bind(now_string())
            .execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE scheduled_tasks SET last_run_id = ?, updated_at = ? WHERE task_id = ?")
        .bind(&run.run_id)
        .bind(now_string())
        .bind(&run.task_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// 生成指定节点的 Master 权威任务快照。
pub async fn snapshot(
    pool: &DbPool,
    node_id: &str,
) -> ApiResult<Vec<AgentScheduledTaskDefinition>> {
    let rows = sqlx::query_as::<_, ScheduledTaskRow>(&format!(
        "{TASK_SELECT} WHERE t.node_id = ? AND t.deleted_at IS NULL ORDER BY t.task_id"
    ))
    .bind(node_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| definition_from_row(&row, format!("snapshot:{}:{}", row.task_id, row.revision)))
        .collect()
}

/// 将运行行转换为公共 DTO。
pub fn run_dto(row: ScheduledTaskRunRow) -> ApiResult<ScheduledTaskRun> {
    let status = run_status_from_text(&row.status)?;
    Ok(ScheduledTaskRun {
        run_id: row.run_id,
        task_id: row.task_id,
        node_id: row.node_id,
        trigger_source: trigger_source_from_text(&row.trigger_source)?,
        status,
        phase: (!status.is_terminal()).then_some(row.phase).flatten(),
        queued_at: row.queued_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        exit_code: row.exit_code,
        error_code: row.error_code,
        error_summary: row.error_summary,
        output: ScheduledTaskRunOutputSummary {
            available: row.output_size_bytes > 0,
            truncated: row.output_truncated,
            size_bytes: row.output_size_bytes.max(0) as u64,
        },
        capabilities: ScheduledTaskRunCapabilities {
            can_cancel: !status.is_terminal(),
        },
    })
}

/// 将后台操作行转换为公共 DTO。
pub fn operation_dto(row: ScheduledTaskOperationRow) -> ApiResult<ScheduledTaskOperation> {
    let status = operation_status_from_text(&row.status)?;
    let kind = operation_kind_from_text(&row.kind)?;
    Ok(ScheduledTaskOperation {
        operation_id: row.operation_id,
        task_id: row.task_id,
        kind,
        status,
        phase: (!status.is_terminal()).then_some(row.phase).flatten(),
        completed_steps: row.completed_steps.max(0) as u32,
        total_steps: row.total_steps.max(0) as u32,
        error_code: row.error_code,
        error_summary: row.error_summary,
        warning_summary: row.warning_summary,
        can_cancel: !status.is_terminal()
            && row.completed_steps == 0
            && matches!(
                kind,
                ScheduledTaskOperationKind::Remove | ScheduledTaskOperationKind::Migrate
            ),
        created_at: row.created_at,
        updated_at: row.updated_at,
        finished_at: row.finished_at,
    })
}

/// 在 Agent 尚未执行任何步骤时恢复删除或迁移操作修改的读模型状态。
pub async fn restore_cancelled_operation(
    pool: &DbPool,
    operation: &ScheduledTaskOperationRow,
) -> ApiResult<()> {
    match operation.kind.as_str() {
        "remove" => {
            sqlx::query("UPDATE scheduled_tasks SET deleted_at = NULL, deployment_status = 'ready', deployment_error_summary = NULL, updated_at = ? WHERE task_id = ?")
                .bind(now_string())
                .bind(&operation.task_id)
                .execute(pool)
                .await?;
        }
        "migrate" => {
            sqlx::query("UPDATE scheduled_tasks SET deployment_status = 'ready', deployment_error_summary = NULL, updated_at = ? WHERE task_id = ?")
                .bind(now_string())
                .bind(&operation.task_id)
                .execute(pool)
                .await?;
        }
        _ => {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskOperationConflict,
                "scheduled task operation does not support cancellation",
            ));
        }
    }
    Ok(())
}

/// 构造 Agent 部署定义。
pub fn definition_from_row(
    row: &ScheduledTaskRow,
    operation_id: String,
) -> ApiResult<AgentScheduledTaskDefinition> {
    Ok(AgentScheduledTaskDefinition {
        operation_id,
        task_id: row.task_id.clone(),
        revision: row.revision,
        name: row.name.clone(),
        command: row.command.clone(),
        cron_expr: row.cron_expr.clone(),
        time_zone: row.time_zone.clone(),
        desired_state: desired_state_from_text(&row.desired_state)?,
        timeout_seconds: row.timeout_seconds.clamp(1, 86_400) as u32,
        prevent_overlap: row.prevent_overlap,
        ownership: ScheduledTaskOwnership {
            kind: ownership_from_text(&row.ownership_kind)?,
            owner_id: row.owner_id.clone(),
            owner_name: row.owner_name.clone(),
            manager_path: row.manager_path.clone(),
        },
    })
}

async fn ensure_mutable_and_idle(
    pool: &DbPool,
    task_id: &str,
    require_no_run: bool,
) -> ApiResult<ScheduledTaskRow> {
    let task = required_task(pool, task_id).await?;
    if task.ownership_kind != "custom" {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskProtected,
            "managed scheduled task must be changed by its owner module",
        ));
    }
    if task.has_active_operation {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskOperationConflict,
            "scheduled task already has an active operation",
        ));
    }
    if require_no_run && task.has_active_run {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskInUse,
            "scheduled task has an active run",
        ));
    }
    Ok(task)
}

async fn required_task(pool: &DbPool, task_id: &str) -> ApiResult<ScheduledTaskRow> {
    get_task(pool, task_id).await?.ok_or_else(|| {
        ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task not found")
    })
}

async fn required_run(pool: &DbPool, run_id: &str) -> ApiResult<ScheduledTaskRunRow> {
    sqlx::query_as::<_, ScheduledTaskRunRow>("SELECT * FROM scheduled_task_runs WHERE run_id = ?")
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                ErrorCode::ScheduledTaskRunNotFound,
                "scheduled task run not found",
            )
        })
}

async fn required_operation(
    pool: &DbPool,
    operation_id: &str,
) -> ApiResult<ScheduledTaskOperationRow> {
    sqlx::query_as::<_, ScheduledTaskOperationRow>(
        "SELECT * FROM scheduled_task_operations WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            ErrorCode::ScheduledTaskNotFound,
            "scheduled task operation not found",
        )
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_operation(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    operation_id: &str,
    task_id: &str,
    kind: ScheduledTaskOperationKind,
    status: &str,
    total_steps: i64,
    source_node_id: Option<&str>,
    target_node_id: Option<&str>,
    actor: &OperationActor,
    now: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO scheduled_task_operations (operation_id, task_id, kind, status, phase, total_steps, source_node_id, target_node_id, actor_user_id, actor_name, client_ip, trace_id, created_at, updated_at) VALUES (?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(operation_id).bind(task_id).bind(operation_kind_text(kind)).bind(status).bind(total_steps)
        .bind(source_node_id).bind(target_node_id).bind(actor.user_id).bind(&actor.name).bind(&actor.client_ip).bind(&actor.trace_id).bind(now).bind(now)
        .execute(&mut **tx).await?;
    Ok(())
}

fn normalize_name(value: &str) -> String {
    value.trim().to_lowercase()
}
fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}
fn now_string() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}
fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.is_unique_violation())
}

pub fn ownership_from_text(value: &str) -> ApiResult<ScheduledTaskOwnershipKind> {
    match value {
        "custom" => Ok(ScheduledTaskOwnershipKind::Custom),
        "compose" => Ok(ScheduledTaskOwnershipKind::Compose),
        "suite" => Ok(ScheduledTaskOwnershipKind::Suite),
        "system" => Ok(ScheduledTaskOwnershipKind::System),
        _ => Err(ApiError::internal("invalid scheduled task ownership")),
    }
}
pub fn deployment_status_from_text(value: &str) -> ApiResult<ScheduledTaskDeploymentStatus> {
    match value {
        "pending" => Ok(ScheduledTaskDeploymentStatus::Pending),
        "applying" => Ok(ScheduledTaskDeploymentStatus::Applying),
        "ready" => Ok(ScheduledTaskDeploymentStatus::Ready),
        "waiting_for_node" => Ok(ScheduledTaskDeploymentStatus::WaitingForNode),
        "failed" => Ok(ScheduledTaskDeploymentStatus::Failed),
        "deleting" => Ok(ScheduledTaskDeploymentStatus::Deleting),
        "migrating" => Ok(ScheduledTaskDeploymentStatus::Migrating),
        _ => Err(ApiError::internal(
            "invalid scheduled task deployment status",
        )),
    }
}
pub fn desired_state_from_text(value: &str) -> ApiResult<ScheduledTaskDesiredState> {
    match value {
        "enabled" => Ok(ScheduledTaskDesiredState::Enabled),
        "disabled" => Ok(ScheduledTaskDesiredState::Disabled),
        _ => Err(ApiError::internal("invalid scheduled task desired state")),
    }
}
fn deployment_status_text(value: ScheduledTaskDeploymentStatus) -> &'static str {
    match value {
        ScheduledTaskDeploymentStatus::Pending => "pending",
        ScheduledTaskDeploymentStatus::Applying => "applying",
        ScheduledTaskDeploymentStatus::Ready => "ready",
        ScheduledTaskDeploymentStatus::WaitingForNode => "waiting_for_node",
        ScheduledTaskDeploymentStatus::Failed => "failed",
        ScheduledTaskDeploymentStatus::Deleting => "deleting",
        ScheduledTaskDeploymentStatus::Migrating => "migrating",
    }
}
fn trigger_source_text(value: ScheduledTaskTriggerSource) -> &'static str {
    match value {
        ScheduledTaskTriggerSource::Schedule => "schedule",
        ScheduledTaskTriggerSource::Manual => "manual",
        ScheduledTaskTriggerSource::Batch => "batch",
    }
}
fn trigger_source_from_text(value: &str) -> ApiResult<ScheduledTaskTriggerSource> {
    match value {
        "schedule" => Ok(ScheduledTaskTriggerSource::Schedule),
        "manual" => Ok(ScheduledTaskTriggerSource::Manual),
        "batch" => Ok(ScheduledTaskTriggerSource::Batch),
        _ => Err(ApiError::internal("invalid scheduled task trigger source")),
    }
}
fn run_status_text(value: ScheduledTaskRunStatus) -> &'static str {
    match value {
        ScheduledTaskRunStatus::Queued => "queued",
        ScheduledTaskRunStatus::Starting => "starting",
        ScheduledTaskRunStatus::Running => "running",
        ScheduledTaskRunStatus::Cancelling => "cancelling",
        ScheduledTaskRunStatus::Succeeded => "succeeded",
        ScheduledTaskRunStatus::Failed => "failed",
        ScheduledTaskRunStatus::TimedOut => "timed_out",
        ScheduledTaskRunStatus::Cancelled => "cancelled",
    }
}
fn run_status_from_text(value: &str) -> ApiResult<ScheduledTaskRunStatus> {
    match value {
        "queued" => Ok(ScheduledTaskRunStatus::Queued),
        "starting" => Ok(ScheduledTaskRunStatus::Starting),
        "running" => Ok(ScheduledTaskRunStatus::Running),
        "cancelling" => Ok(ScheduledTaskRunStatus::Cancelling),
        "succeeded" => Ok(ScheduledTaskRunStatus::Succeeded),
        "failed" => Ok(ScheduledTaskRunStatus::Failed),
        "timed_out" => Ok(ScheduledTaskRunStatus::TimedOut),
        "cancelled" => Ok(ScheduledTaskRunStatus::Cancelled),
        _ => Err(ApiError::internal("invalid scheduled task run status")),
    }
}

fn is_valid_run_transition(previous: ScheduledTaskRunStatus, next: ScheduledTaskRunStatus) -> bool {
    use ScheduledTaskRunStatus::{
        Cancelled, Cancelling, Failed, Queued, Running, Starting, Succeeded, TimedOut,
    };
    if previous == next {
        return true;
    }
    match previous {
        Queued => matches!(
            next,
            Starting | Running | Cancelling | Succeeded | Failed | TimedOut | Cancelled
        ),
        Starting => matches!(
            next,
            Running | Cancelling | Succeeded | Failed | TimedOut | Cancelled
        ),
        Running => matches!(next, Cancelling | Succeeded | Failed | TimedOut | Cancelled),
        Cancelling => matches!(next, Succeeded | Failed | TimedOut | Cancelled),
        Succeeded | Failed | TimedOut | Cancelled => false,
    }
}
fn operation_kind_text(value: ScheduledTaskOperationKind) -> &'static str {
    match value {
        ScheduledTaskOperationKind::Deploy => "deploy",
        ScheduledTaskOperationKind::Update => "update",
        ScheduledTaskOperationKind::StateChange => "state_change",
        ScheduledTaskOperationKind::Remove => "remove",
        ScheduledTaskOperationKind::Migrate => "migrate",
        ScheduledTaskOperationKind::Batch => "batch",
    }
}
fn operation_kind_from_text(value: &str) -> ApiResult<ScheduledTaskOperationKind> {
    match value {
        "deploy" => Ok(ScheduledTaskOperationKind::Deploy),
        "update" => Ok(ScheduledTaskOperationKind::Update),
        "state_change" => Ok(ScheduledTaskOperationKind::StateChange),
        "remove" => Ok(ScheduledTaskOperationKind::Remove),
        "migrate" => Ok(ScheduledTaskOperationKind::Migrate),
        "batch" => Ok(ScheduledTaskOperationKind::Batch),
        _ => Err(ApiError::internal("invalid scheduled task operation kind")),
    }
}
fn operation_status_text(value: ScheduledTaskOperationStatus) -> &'static str {
    match value {
        ScheduledTaskOperationStatus::Queued => "queued",
        ScheduledTaskOperationStatus::Running => "running",
        ScheduledTaskOperationStatus::Cancelling => "cancelling",
        ScheduledTaskOperationStatus::Succeeded => "succeeded",
        ScheduledTaskOperationStatus::Partial => "partial",
        ScheduledTaskOperationStatus::Failed => "failed",
        ScheduledTaskOperationStatus::Cancelled => "cancelled",
    }
}
fn operation_status_from_text(value: &str) -> ApiResult<ScheduledTaskOperationStatus> {
    match value {
        "queued" => Ok(ScheduledTaskOperationStatus::Queued),
        "running" => Ok(ScheduledTaskOperationStatus::Running),
        "cancelling" => Ok(ScheduledTaskOperationStatus::Cancelling),
        "succeeded" => Ok(ScheduledTaskOperationStatus::Succeeded),
        "partial" => Ok(ScheduledTaskOperationStatus::Partial),
        "failed" => Ok(ScheduledTaskOperationStatus::Failed),
        "cancelled" => Ok(ScheduledTaskOperationStatus::Cancelled),
        _ => Err(ApiError::internal(
            "invalid scheduled task operation status",
        )),
    }
}
fn batch_action_text(value: ScheduledTaskBatchAction) -> &'static str {
    match value {
        ScheduledTaskBatchAction::Enable => "enable",
        ScheduledTaskBatchAction::Disable => "disable",
        ScheduledTaskBatchAction::Run => "run",
        ScheduledTaskBatchAction::Remove => "remove",
    }
}
fn batch_action_from_text(value: &str) -> ApiResult<ScheduledTaskBatchAction> {
    match value {
        "enable" => Ok(ScheduledTaskBatchAction::Enable),
        "disable" => Ok(ScheduledTaskBatchAction::Disable),
        "run" => Ok(ScheduledTaskBatchAction::Run),
        "remove" => Ok(ScheduledTaskBatchAction::Remove),
        _ => Err(ApiError::internal("invalid scheduled task batch action")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vust_contracts::scheduled_tasks::{
        CreateScheduledTaskRequest, ScheduledTaskRunOutputSummary,
    };

    async fn setup_pool() -> DbPool {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'tester', 'hash')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn actor() -> OperationActor {
        OperationActor {
            user_id: 1,
            name: "tester".to_string(),
            client_ip: "127.0.0.1".to_string(),
            trace_id: "trace-task-test".to_string(),
        }
    }

    fn request(name: &str) -> CreateScheduledTaskRequest {
        CreateScheduledTaskRequest {
            name: name.to_string(),
            description: Some("test task".to_string()),
            node_id: "local".to_string(),
            cron_expr: "*/5 * * * *".to_string(),
            time_zone: "Asia/Shanghai".to_string(),
            command: "printf ok".to_string(),
            timeout_seconds: 30,
            prevent_overlap: true,
            enabled: true,
        }
    }

    async fn ready_task(pool: &DbPool, name: &str) -> ScheduledTaskRow {
        let (task, operation) = create_task(pool, &request(name), &actor()).await.unwrap();
        finish_operation(
            pool,
            &operation.operation_id,
            ScheduledTaskOperationStatus::Succeeded,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        update_deployment(
            pool,
            &task.task_id,
            ScheduledTaskDeploymentStatus::Ready,
            Some("2026-07-17T00:00:00Z"),
            None,
        )
        .await
        .unwrap();
        required_task(pool, &task.task_id).await.unwrap()
    }

    #[tokio::test]
    async fn list_is_custom_only_paginated_and_name_is_case_insensitive_unique() {
        let pool = setup_pool().await;
        let (first, _) = create_task(&pool, &request("Alpha"), &actor())
            .await
            .unwrap();
        create_task(&pool, &request("Beta"), &actor())
            .await
            .unwrap();
        sqlx::query("UPDATE scheduled_tasks SET ownership_kind = 'suite' WHERE task_id = ?")
            .bind(&first.task_id)
            .execute(&pool)
            .await
            .unwrap();

        let filter = TaskListFilter {
            node_id: Some("local"),
            keyword: Some("task"),
            enabled: Some(true),
            deployment_status: Some(ScheduledTaskDeploymentStatus::Pending),
            page: 1,
            page_size: 1,
            sort_by: "name",
            sort_order: "asc",
        };
        let (items, total) = list_tasks(&pool, &filter).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].name, "Beta");

        let error = create_task(&pool, &request(" beta "), &actor())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ScheduledTaskAlreadyExists);
    }

    #[tokio::test]
    async fn overlap_guard_and_remove_protection_cover_manual_runs() {
        let pool = setup_pool().await;
        let task = ready_task(&pool, "Overlap").await;
        create_run(
            &pool,
            &task.task_id,
            ScheduledTaskTriggerSource::Manual,
            None,
        )
        .await
        .unwrap();
        let overlap = create_run(
            &pool,
            &task.task_id,
            ScheduledTaskTriggerSource::Batch,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(overlap.code, ErrorCode::ScheduledTaskOperationConflict);
        let remove = request_remove(&pool, &task.task_id, &actor())
            .await
            .unwrap_err();
        assert_eq!(remove.code, ErrorCode::ScheduledTaskInUse);
    }

    #[tokio::test]
    async fn agent_report_enforces_node_and_monotonic_status() {
        let pool = setup_pool().await;
        let task = ready_task(&pool, "Report").await;
        let queued = create_run(
            &pool,
            &task.task_id,
            ScheduledTaskTriggerSource::Manual,
            Some(&actor()),
        )
        .await
        .unwrap();
        let report = |status, node_id: &str| AgentScheduledTaskRunReport {
            run: ScheduledTaskRun {
                run_id: queued.run_id.clone(),
                task_id: task.task_id.clone(),
                node_id: node_id.to_string(),
                trigger_source: ScheduledTaskTriggerSource::Manual,
                status,
                phase: None,
                queued_at: queued.queued_at.clone(),
                started_at: Some(queued.queued_at.clone()),
                finished_at: status.is_terminal().then(now_string),
                exit_code: status.is_terminal().then_some(0),
                error_code: None,
                error_summary: None,
                output: ScheduledTaskRunOutputSummary::default(),
                capabilities: ScheduledTaskRunCapabilities::default(),
            },
            output_content: None,
        };

        let wrong_node = save_run_report(
            &pool,
            "local",
            &report(ScheduledTaskRunStatus::Running, "other-node"),
        )
        .await
        .unwrap_err();
        assert_eq!(wrong_node.code, ErrorCode::ScheduledTaskOperationConflict);
        save_run_report(
            &pool,
            "local",
            &report(ScheduledTaskRunStatus::Running, "local"),
        )
        .await
        .unwrap();
        save_run_report(
            &pool,
            "local",
            &report(ScheduledTaskRunStatus::Succeeded, "local"),
        )
        .await
        .unwrap();
        let audit = claim_run_terminal_audit(&pool, &queued.run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(audit.actor_name, "tester");
        assert!(
            claim_run_terminal_audit(&pool, &queued.run_id)
                .await
                .unwrap()
                .is_none()
        );
        let regression = save_run_report(
            &pool,
            "local",
            &report(ScheduledTaskRunStatus::Running, "local"),
        )
        .await
        .unwrap_err();
        assert_eq!(regression.code, ErrorCode::ScheduledTaskOperationConflict);
    }

    #[tokio::test]
    async fn queued_remove_cancellation_restores_tombstone() {
        let pool = setup_pool().await;
        let task = ready_task(&pool, "Cancelable Remove").await;
        let remove = request_remove(&pool, &task.task_id, &actor())
            .await
            .unwrap();
        let dto = operation_dto(remove.clone()).unwrap();
        assert!(dto.can_cancel);
        let cancelling = request_operation_cancel(&pool, &remove.operation_id)
            .await
            .unwrap();
        restore_cancelled_operation(&pool, &cancelling)
            .await
            .unwrap();
        finish_operation(
            &pool,
            &remove.operation_id,
            ScheduledTaskOperationStatus::Cancelled,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let restored = required_task(&pool, &task.task_id).await.unwrap();
        assert_eq!(restored.deployment_status, "ready");
        let deleted_at: Option<String> =
            sqlx::query_scalar("SELECT deleted_at FROM scheduled_tasks WHERE task_id = ?")
                .bind(&task.task_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(deleted_at.is_none());
    }
}
