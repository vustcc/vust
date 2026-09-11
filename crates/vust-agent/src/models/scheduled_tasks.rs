//! Agent 计划任务持久化：任务定义、运行状态、有界输出与可靠上报队列。

use crate::state::DbPool;
use chrono::{TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::Serialize;
use sqlx::{FromRow, Row};
use std::str::FromStr;
use vust_contracts::{
    api::ErrorCode,
    scheduled_tasks::{
        AgentScheduledTaskDefinition, AgentScheduledTaskRunReport, ScheduledTaskRun,
        ScheduledTaskRunCapabilities, ScheduledTaskRunOutput, ScheduledTaskRunOutputSummary,
        ScheduledTaskRunStatus, ScheduledTaskTriggerSource,
    },
};

use crate::types::{ApiError, ApiResult};

pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Agent 本地任务定义。
#[derive(Debug, Clone, FromRow)]
pub struct AgentScheduledTask {
    pub task_id: String,
    pub revision: i64,
    pub name: String,
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
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Agent 本地运行记录。
#[derive(Debug, Clone, FromRow)]
pub struct AgentTaskRun {
    pub run_id: String,
    pub task_id: String,
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
    pub cancel_requested: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Agent 批量状态上报中的任务状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskStatus {
    pub task_id: String,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub last_status: Option<ScheduledTaskRunStatus>,
}

/// 后台执行完成时写入的终态。
pub struct FinishTaskRun<'a> {
    pub status: ScheduledTaskRunStatus,
    pub exit_code: Option<i32>,
    pub error_code: Option<&'a str>,
    pub error_summary: Option<&'a str>,
    pub output: &'a [u8],
    pub output_truncated: bool,
}

/// 可靠上报队列条目。
#[derive(Debug, Clone)]
pub struct TaskRunOutboxItem {
    pub run_id: String,
    pub report: AgentScheduledTaskRunReport,
}

/// 校验任务定义并幂等写入本地数据库。
pub async fn upsert_task(
    pool: &DbPool,
    definition: &AgentScheduledTaskDefinition,
) -> ApiResult<AgentScheduledTask> {
    validate_definition(definition)?;
    if let Some(existing) = get_task(pool, &definition.task_id).await?
        && definition.revision < existing.revision
    {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskRevisionConflict,
            "scheduled task revision is older than the deployed revision",
        ));
    }

    let now = now_string();
    let next_run_at = if definition.desired_state
        == vust_contracts::scheduled_tasks::ScheduledTaskDesiredState::Enabled
    {
        compute_next_run_at(&definition.cron_expr, &definition.time_zone, Utc::now())?
    } else {
        None
    };
    let ownership = &definition.ownership;
    sqlx::query(
        r#"
        INSERT INTO scheduled_tasks (
            task_id, revision, name, command, cron_expr, time_zone, desired_state,
            timeout_seconds, prevent_overlap, ownership_kind, owner_id, owner_name,
            manager_path, next_run_at, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)
        ON CONFLICT(task_id) DO UPDATE SET
            revision = excluded.revision,
            name = excluded.name,
            command = excluded.command,
            cron_expr = excluded.cron_expr,
            time_zone = excluded.time_zone,
            desired_state = excluded.desired_state,
            timeout_seconds = excluded.timeout_seconds,
            prevent_overlap = excluded.prevent_overlap,
            ownership_kind = excluded.ownership_kind,
            owner_id = excluded.owner_id,
            owner_name = excluded.owner_name,
            manager_path = excluded.manager_path,
            next_run_at = excluded.next_run_at,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&definition.task_id)
    .bind(definition.revision)
    .bind(definition.name.trim())
    .bind(&definition.command)
    .bind(definition.cron_expr.trim())
    .bind(definition.time_zone.trim())
    .bind(desired_state_text(definition.desired_state))
    .bind(i64::from(definition.timeout_seconds))
    .bind(definition.prevent_overlap)
    .bind(ownership_kind_text(ownership.kind))
    .bind(ownership.owner_id.as_deref())
    .bind(ownership.owner_name.as_deref())
    .bind(ownership.manager_path.as_deref())
    .bind(next_run_at.as_deref())
    .bind(&now)
    .execute(pool)
    .await?;
    record_operation_receipt(
        pool,
        &definition.operation_id,
        &definition.task_id,
        "upsert",
    )
    .await?;
    get_task(pool, &definition.task_id)
        .await?
        .ok_or_else(|| ApiError::internal("scheduled task disappeared after upsert"))
}

/// 查询单个任务定义。
pub async fn get_task(pool: &DbPool, task_id: &str) -> ApiResult<Option<AgentScheduledTask>> {
    Ok(
        sqlx::query_as::<_, AgentScheduledTask>("SELECT * FROM scheduled_tasks WHERE task_id = ?")
            .bind(task_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 查询全部本地任务，用于 Master 快照对齐。
pub async fn list_all_tasks(pool: &DbPool) -> ApiResult<Vec<AgentScheduledTask>> {
    Ok(
        sqlx::query_as::<_, AgentScheduledTask>("SELECT * FROM scheduled_tasks ORDER BY task_id")
            .fetch_all(pool)
            .await?,
    )
}

/// 查询当前到期的任务。
pub async fn list_due_tasks(pool: &DbPool, now: &str) -> ApiResult<Vec<AgentScheduledTask>> {
    Ok(sqlx::query_as::<_, AgentScheduledTask>(
        "SELECT * FROM scheduled_tasks WHERE desired_state = 'enabled' AND next_run_at IS NOT NULL AND next_run_at <= ? ORDER BY next_run_at, task_id",
    )
    .bind(now)
    .fetch_all(pool)
    .await?)
}

/// 推进下一次运行时间，避免同一调度点被重复分发。
pub async fn advance_next_run(pool: &DbPool, task: &AgentScheduledTask) -> ApiResult<()> {
    let next = compute_next_run_at(
        &task.cron_expr,
        &task.time_zone,
        Utc::now() + chrono::Duration::seconds(1),
    )?;
    sqlx::query("UPDATE scheduled_tasks SET last_run_at = ?2, next_run_at = ?3, updated_at = ?2 WHERE task_id = ?1")
        .bind(&task.task_id)
        .bind(now_string())
        .bind(next)
        .execute(pool)
        .await?;
    Ok(())
}

/// 删除自定义任务；托管任务必须回到所属模块。
pub async fn delete_task(pool: &DbPool, task_id: &str, operation_id: &str) -> ApiResult<bool> {
    let task = get_task(pool, task_id).await?.ok_or_else(|| {
        ApiError::not_found(ErrorCode::ScheduledTaskNotFound, "scheduled task not found")
    })?;
    if task.ownership_kind != "custom" {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskProtected,
            "managed scheduled task must be removed by its owner module",
        ));
    }
    if has_active_run(pool, task_id).await? {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskInUse,
            "scheduled task has an active run",
        ));
    }
    let result = sqlx::query("DELETE FROM scheduled_tasks WHERE task_id = ?")
        .bind(task_id)
        .execute(pool)
        .await?;
    record_operation_receipt(pool, operation_id, task_id, "remove").await?;
    Ok(result.rows_affected() > 0)
}

/// 根据 Master 全量快照删除已经不存在的本地副本。
pub async fn delete_task_for_reconciliation(pool: &DbPool, task_id: &str) -> ApiResult<()> {
    if has_active_run(pool, task_id).await? {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskInUse,
            "scheduled task has an active run",
        ));
    }
    sqlx::query("DELETE FROM scheduled_tasks WHERE task_id = ?")
        .bind(task_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 创建幂等运行记录。
pub async fn create_run(
    pool: &DbPool,
    task: &AgentScheduledTask,
    run_id: &str,
    trigger_source: ScheduledTaskTriggerSource,
) -> ApiResult<(AgentTaskRun, bool)> {
    if let Some(existing) = get_run(pool, run_id).await? {
        if existing.task_id != task.task_id {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskOperationConflict,
                "run id already belongs to another scheduled task",
            ));
        }
        return Ok((existing, false));
    }
    let now = now_string();
    let result = sqlx::query(
        "INSERT INTO task_runs (run_id, task_id, trigger_source, status, phase, queued_at, overlap_guard, created_at, updated_at) VALUES (?, ?, ?, 'queued', 'queued', ?, ?, ?, ?)",
    )
    .bind(run_id)
    .bind(&task.task_id)
    .bind(trigger_source_text(trigger_source))
    .bind(&now)
    .bind(task.prevent_overlap.then_some(task.task_id.as_str()))
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await;
    if let Err(error) = result {
        if let Some(existing) = get_run(pool, run_id).await?
            && existing.task_id == task.task_id
        {
            return Ok((existing, false));
        }
        if matches!(&error, sqlx::Error::Database(database) if database.is_unique_violation()) {
            return Err(ApiError::conflict(
                ErrorCode::ScheduledTaskOperationConflict,
                "scheduled task already has an active run",
            ));
        }
        return Err(error.into());
    }
    let run = get_run(pool, run_id)
        .await?
        .ok_or_else(|| ApiError::internal("scheduled task run disappeared after create"))?;
    Ok((run, true))
}

/// 查询单次运行。
pub async fn get_run(pool: &DbPool, run_id: &str) -> ApiResult<Option<AgentTaskRun>> {
    Ok(
        sqlx::query_as::<_, AgentTaskRun>("SELECT * FROM task_runs WHERE run_id = ?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 分页查询任务运行记录。
pub async fn list_runs(
    pool: &DbPool,
    task_id: &str,
    page: u32,
    page_size: u32,
) -> ApiResult<(Vec<AgentTaskRun>, u64)> {
    let page_size = page_size.clamp(1, 100);
    let page = page.max(1);
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_runs WHERE task_id = ?")
        .bind(task_id)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, AgentTaskRun>(
        "SELECT * FROM task_runs WHERE task_id = ? ORDER BY queued_at DESC, run_id DESC LIMIT ? OFFSET ?",
    )
    .bind(task_id)
    .bind(i64::from(page_size))
    .bind(i64::from((page - 1) * page_size))
    .fetch_all(pool)
    .await?;
    Ok((items, total.max(0) as u64))
}

/// 将运行标记为启动中。
pub async fn mark_run_starting(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    let now = now_string();
    sqlx::query("UPDATE task_runs SET status = 'starting', phase = 'starting', started_at = ?, updated_at = ? WHERE run_id = ? AND status = 'queued'")
        .bind(&now)
        .bind(&now)
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 将运行标记为执行中。
pub async fn mark_run_running(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE task_runs SET status = 'running', phase = 'executing', updated_at = ? WHERE run_id = ? AND status IN ('queued', 'starting')")
        .bind(now_string())
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 请求取消仍处于活动状态的运行。
pub async fn request_cancel(pool: &DbPool, task_id: &str, run_id: &str) -> ApiResult<AgentTaskRun> {
    let run = get_run(pool, run_id).await?.ok_or_else(|| {
        ApiError::not_found(
            ErrorCode::ScheduledTaskRunNotFound,
            "scheduled task run not found",
        )
    })?;
    if run.task_id != task_id {
        return Err(ApiError::not_found(
            ErrorCode::ScheduledTaskRunNotFound,
            "scheduled task run not found",
        ));
    }
    if run_status_from_text(&run.status)?.is_terminal() {
        return Err(ApiError::conflict(
            ErrorCode::ScheduledTaskRunNotCancellable,
            "scheduled task run is already terminal",
        ));
    }
    sqlx::query("UPDATE task_runs SET status = 'cancelling', phase = 'cancelling', cancel_requested = 1, updated_at = ? WHERE run_id = ?")
        .bind(now_string())
        .bind(run_id)
        .execute(pool)
        .await?;
    get_run(pool, run_id)
        .await?
        .ok_or_else(|| ApiError::internal("scheduled task run disappeared after cancellation"))
}

/// 查询运行是否收到取消请求。
pub async fn is_cancel_requested(pool: &DbPool, run_id: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT cancel_requested FROM task_runs WHERE run_id = ?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false),
    )
}

/// 原子保存终态、输出和可靠上报条目。
pub async fn finish_run(
    pool: &DbPool,
    run_id: &str,
    finish: FinishTaskRun<'_>,
) -> ApiResult<AgentTaskRun> {
    let mut tx = pool.begin().await?;
    let now = now_string();
    let output = &finish.output[..finish.output.len().min(MAX_OUTPUT_BYTES)];
    let truncated = finish.output_truncated || finish.output.len() > MAX_OUTPUT_BYTES;
    sqlx::query(
        "UPDATE task_runs SET status = ?, phase = NULL, finished_at = ?, exit_code = ?, error_code = ?, error_summary = ?, output_size_bytes = ?, output_truncated = ?, updated_at = ? WHERE run_id = ?",
    )
    .bind(run_status_text(finish.status))
    .bind(&now)
    .bind(finish.exit_code)
    .bind(finish.error_code)
    .bind(finish.error_summary)
    .bind(output.len() as i64)
    .bind(truncated)
    .bind(&now)
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO task_run_outputs (run_id, content, size_bytes, truncated, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(run_id) DO UPDATE SET content = excluded.content, size_bytes = excluded.size_bytes, truncated = excluded.truncated, updated_at = excluded.updated_at")
        .bind(run_id)
        .bind(output)
        .bind(output.len() as i64)
        .bind(truncated)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    let run = sqlx::query_as::<_, AgentTaskRun>("SELECT * FROM task_runs WHERE run_id = ?")
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
    let report = AgentScheduledTaskRunReport {
        run: run_dto(run.clone())?,
        output_content: Some(String::from_utf8_lossy(output).to_string()),
    };
    let payload = serde_json::to_string(&report).map_err(|error| {
        ApiError::internal(format!("failed to encode task run outbox: {error}"))
    })?;
    sqlx::query("INSERT INTO task_run_outbox (run_id, payload, created_at, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(run_id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at")
        .bind(run_id)
        .bind(payload)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(run)
}

/// 启动时将中断的运行收敛为失败终态并进入上报队列。
pub async fn recover_interrupted_runs(pool: &DbPool) -> ApiResult<()> {
    let run_ids = sqlx::query("SELECT run_id FROM task_runs WHERE status IN ('queued', 'starting', 'running', 'cancelling')")
        .fetch_all(pool)
        .await?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("run_id").ok())
        .collect::<Vec<_>>();
    for run_id in run_ids {
        finish_run(
            pool,
            &run_id,
            FinishTaskRun {
                status: ScheduledTaskRunStatus::Failed,
                exit_code: None,
                error_code: Some("SCHEDULED_TASK_AGENT_RESTARTED"),
                error_summary: Some("Agent restarted while the scheduled task was active"),
                output: &[],
                output_truncated: false,
            },
        )
        .await?;
    }
    Ok(())
}

/// 分页读取单次运行的有界输出。
pub async fn read_output(
    pool: &DbPool,
    run_id: &str,
    offset: u64,
    limit: u32,
) -> ApiResult<ScheduledTaskRunOutput> {
    let row =
        sqlx::query("SELECT content, size_bytes, truncated FROM task_run_outputs WHERE run_id = ?")
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
    let size = content.len() as u64;
    let start = offset.min(size) as usize;
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

/// 返回等待可靠上报的运行终态。
pub async fn list_outbox(pool: &DbPool, limit: i64) -> ApiResult<Vec<TaskRunOutboxItem>> {
    let rows = sqlx::query(
        "SELECT run_id, payload FROM task_run_outbox ORDER BY created_at, run_id LIMIT ?",
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let run_id = row.try_get::<String, _>("run_id")?;
            let payload = row.try_get::<String, _>("payload")?;
            let report = serde_json::from_str(&payload)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
            Ok(TaskRunOutboxItem { run_id, report })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(ApiError::from)
}

/// 确认运行终态已经被 Master 接收。
pub async fn acknowledge_outbox(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM task_run_outbox WHERE run_id = ?")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 记录一次上报失败，供诊断和退避使用。
pub async fn mark_outbox_attempt(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    let now = now_string();
    sqlx::query("UPDATE task_run_outbox SET attempts = attempts + 1, last_attempt_at = ?, updated_at = ? WHERE run_id = ?")
        .bind(&now)
        .bind(&now)
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 批量返回 Agent 实际任务状态。
pub async fn list_all_status(pool: &DbPool) -> ApiResult<Vec<AgentTaskStatus>> {
    let tasks = list_all_tasks(pool).await?;
    let mut statuses = Vec::with_capacity(tasks.len());
    for task in tasks {
        let last_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM task_runs WHERE task_id = ? ORDER BY queued_at DESC, run_id DESC LIMIT 1",
        )
        .bind(&task.task_id)
        .fetch_optional(pool)
        .await?
        .map(|value| run_status_from_text(&value))
        .transpose()?;
        statuses.push(AgentTaskStatus {
            task_id: task.task_id,
            next_run_at: task.next_run_at,
            last_run_at: task.last_run_at,
            last_status,
        });
    }
    Ok(statuses)
}

/// 将数据库运行记录转换为稳定领域 DTO；nodeId 由 Master 会话补齐。
pub fn run_dto(run: AgentTaskRun) -> ApiResult<ScheduledTaskRun> {
    let status = run_status_from_text(&run.status)?;
    Ok(ScheduledTaskRun {
        run_id: run.run_id,
        task_id: run.task_id,
        node_id: String::new(),
        trigger_source: trigger_source_from_text(&run.trigger_source)?,
        status,
        phase: (!status.is_terminal()).then_some(run.phase).flatten(),
        queued_at: run.queued_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        exit_code: run.exit_code,
        error_code: run.error_code,
        error_summary: run.error_summary,
        output: ScheduledTaskRunOutputSummary {
            available: run.output_size_bytes > 0,
            truncated: run.output_truncated,
            size_bytes: run.output_size_bytes.max(0) as u64,
        },
        capabilities: ScheduledTaskRunCapabilities {
            can_cancel: !status.is_terminal(),
        },
    })
}

fn validate_definition(definition: &AgentScheduledTaskDefinition) -> ApiResult<()> {
    let name = definition.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidName,
            "scheduled task name must contain 1 to 80 non-control characters",
        ));
    }
    if definition.command.is_empty()
        || definition.command.len() > 65_536
        || definition.command.contains('\0')
    {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidCommand,
            "scheduled task command is invalid",
        ));
    }
    if !(1..=86_400).contains(&definition.timeout_seconds) {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidCommand,
            "scheduled task timeout must be between 1 and 86400 seconds",
        ));
    }
    validate_schedule(&definition.cron_expr, &definition.time_zone)
}

/// 校验 5 段分钟级 Cron 和 IANA 时区。
pub fn validate_schedule(cron_expr: &str, time_zone: &str) -> ApiResult<()> {
    let parts = cron_expr.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidSchedule,
            "scheduled task cron expression must contain exactly 5 fields",
        ));
    }
    let _: Tz = time_zone.parse().map_err(|_| {
        ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidTimeZone,
            "scheduled task time zone must be a valid IANA identifier",
        )
    })?;
    Schedule::from_str(&format!("0 {}", parts.join(" "))).map_err(|error| {
        ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidSchedule,
            format!("invalid scheduled task cron expression: {error}"),
        )
    })?;
    Ok(())
}

/// 按任务 IANA 时区计算下一次执行时间，并统一返回 UTC RFC 3339。
pub fn compute_next_run_at(
    cron_expr: &str,
    time_zone: &str,
    from: chrono::DateTime<Utc>,
) -> ApiResult<Option<String>> {
    validate_schedule(cron_expr, time_zone)?;
    let tz: Tz = time_zone.parse().map_err(|_| {
        ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidTimeZone,
            "scheduled task time zone must be a valid IANA identifier",
        )
    })?;
    let schedule = Schedule::from_str(&format!("0 {cron_expr}"))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut probe = (from + chrono::Duration::minutes(1))
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .ok_or_else(|| ApiError::internal("failed to normalize scheduled task timestamp"))?;
    for _ in 0..=(36 * 60) {
        if schedule.includes(probe.with_timezone(&tz)) {
            return Ok(Some(probe.to_rfc3339()));
        }
        probe += chrono::Duration::minutes(1);
    }
    let local = tz
        .timestamp_opt(from.timestamp(), 0)
        .single()
        .ok_or_else(|| ApiError::internal("failed to convert scheduled task time zone"))?;
    Ok(schedule
        .after(&local)
        .next()
        .map(|value| value.with_timezone(&Utc).to_rfc3339()))
}

async fn has_active_run(pool: &DbPool, task_id: &str) -> ApiResult<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_runs WHERE task_id = ? AND status IN ('queued', 'starting', 'running', 'cancelling')",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

async fn record_operation_receipt(
    pool: &DbPool,
    operation_id: &str,
    task_id: &str,
    kind: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT OR IGNORE INTO scheduled_task_operation_receipts (operation_id, task_id, operation_kind, completed_at) VALUES (?, ?, ?, ?)")
        .bind(operation_id)
        .bind(task_id)
        .bind(kind)
        .bind(now_string())
        .execute(pool)
        .await?;
    Ok(())
}

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn desired_state_text(
    value: vust_contracts::scheduled_tasks::ScheduledTaskDesiredState,
) -> &'static str {
    match value {
        vust_contracts::scheduled_tasks::ScheduledTaskDesiredState::Enabled => "enabled",
        vust_contracts::scheduled_tasks::ScheduledTaskDesiredState::Disabled => "disabled",
    }
}

fn ownership_kind_text(
    value: vust_contracts::scheduled_tasks::ScheduledTaskOwnershipKind,
) -> &'static str {
    use vust_contracts::scheduled_tasks::ScheduledTaskOwnershipKind;
    match value {
        ScheduledTaskOwnershipKind::Custom => "custom",
        ScheduledTaskOwnershipKind::Compose => "compose",
        ScheduledTaskOwnershipKind::Suite => "suite",
        ScheduledTaskOwnershipKind::System => "system",
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

#[cfg(test)]
mod tests {
    use super::*;
    use vust_contracts::scheduled_tasks::{
        ScheduledTaskDesiredState, ScheduledTaskOwnership, ScheduledTaskOwnershipKind,
    };

    fn definition(task_id: &str) -> AgentScheduledTaskDefinition {
        AgentScheduledTaskDefinition {
            operation_id: format!("operation-{task_id}"),
            task_id: task_id.to_string(),
            revision: 1,
            name: format!("Task {task_id}"),
            command: "printf ok".to_string(),
            cron_expr: "*/5 * * * *".to_string(),
            time_zone: "Asia/Shanghai".to_string(),
            desired_state: ScheduledTaskDesiredState::Disabled,
            timeout_seconds: 30,
            prevent_overlap: true,
            ownership: ScheduledTaskOwnership {
                kind: ScheduledTaskOwnershipKind::Custom,
                owner_id: None,
                owner_name: None,
                manager_path: None,
            },
        }
    }

    #[test]
    fn validates_minute_cron_and_iana_time_zone() {
        assert!(validate_schedule("*/5 * * * *", "Asia/Shanghai").is_ok());
        assert!(validate_schedule("0 */5 * * * *", "Asia/Shanghai").is_err());
        assert!(validate_schedule("*/5 * * * *", "Invalid/Zone").is_err());
    }

    #[test]
    fn computes_next_run_in_task_time_zone() {
        let from = chrono::DateTime::parse_from_rfc3339("2026-07-16T15:59:30Z")
            .unwrap()
            .with_timezone(&Utc);
        let next = compute_next_run_at("0 0 * * *", "Asia/Shanghai", from).unwrap();
        assert_eq!(next.as_deref(), Some("2026-07-16T16:00:00+00:00"));
    }

    #[test]
    fn daylight_saving_transitions_keep_real_utc_instants() {
        let spring = chrono::DateTime::parse_from_rfc3339("2026-03-08T06:59:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let spring_next = compute_next_run_at("30 2 * * *", "America/New_York", spring).unwrap();
        assert_eq!(spring_next.as_deref(), Some("2026-03-09T06:30:00+00:00"));

        let fall = chrono::DateTime::parse_from_rfc3339("2026-11-01T04:59:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let fall_next = compute_next_run_at("30 1 * * *", "America/New_York", fall).unwrap();
        assert_eq!(fall_next.as_deref(), Some("2026-11-01T05:30:00+00:00"));
    }

    #[test]
    fn terminal_status_has_no_active_phase() {
        assert!(ScheduledTaskRunStatus::Succeeded.is_terminal());
        assert!(!ScheduledTaskRunStatus::Running.is_terminal());
    }

    #[tokio::test]
    async fn run_id_is_idempotent_and_overlap_guard_is_atomic() {
        let pool = crate::test_support::setup_test_db().await;
        let task = upsert_task(&pool, &definition("task-1")).await.unwrap();
        let (_, created) = create_run(&pool, &task, "run-1", ScheduledTaskTriggerSource::Manual)
            .await
            .unwrap();
        assert!(created);
        let (_, created_again) =
            create_run(&pool, &task, "run-1", ScheduledTaskTriggerSource::Manual)
                .await
                .unwrap();
        assert!(!created_again);

        let error = create_run(&pool, &task, "run-2", ScheduledTaskTriggerSource::Batch)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ScheduledTaskOperationConflict);
    }

    #[tokio::test]
    async fn finish_persists_bounded_output_and_outbox_atomically() {
        let pool = crate::test_support::setup_test_db().await;
        let task = upsert_task(&pool, &definition("task-2")).await.unwrap();
        create_run(
            &pool,
            &task,
            "run-output",
            ScheduledTaskTriggerSource::Manual,
        )
        .await
        .unwrap();
        let output = vec![b'x'; MAX_OUTPUT_BYTES + 100];
        let finished = finish_run(
            &pool,
            "run-output",
            FinishTaskRun {
                status: ScheduledTaskRunStatus::Succeeded,
                exit_code: Some(0),
                error_code: None,
                error_summary: None,
                output: &output,
                output_truncated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(finished.output_size_bytes as usize, MAX_OUTPUT_BYTES);
        assert!(finished.output_truncated);
        let outbox = list_outbox(&pool, 10).await.unwrap();
        assert_eq!(outbox.len(), 1);
        assert!(outbox[0].report.run.status.is_terminal());
        assert_eq!(
            outbox[0].report.run.output.size_bytes as usize,
            MAX_OUTPUT_BYTES
        );
        assert!(outbox[0].report.run.output.truncated);
    }

    #[tokio::test]
    async fn recovery_and_managed_resource_protection_are_persistent() {
        let pool = crate::test_support::setup_test_db().await;
        let mut managed = definition("task-managed");
        managed.ownership.kind = ScheduledTaskOwnershipKind::Suite;
        let task = upsert_task(&pool, &managed).await.unwrap();
        let error = delete_task(&pool, &task.task_id, "remove-managed")
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ScheduledTaskProtected);

        create_run(
            &pool,
            &task,
            "run-recover",
            ScheduledTaskTriggerSource::Schedule,
        )
        .await
        .unwrap();
        recover_interrupted_runs(&pool).await.unwrap();
        let recovered = get_run(&pool, "run-recover").await.unwrap().unwrap();
        assert_eq!(recovered.status, "failed");
        assert_eq!(
            recovered.error_code.as_deref(),
            Some("SCHEDULED_TASK_AGENT_RESTARTED")
        );
        assert_eq!(list_outbox(&pool, 10).await.unwrap().len(), 1);
    }
}
