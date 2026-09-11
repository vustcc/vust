//! 脚本库仓储：脚本资产、修订、临时执行与可靠 Agent 上报。

use crate::{
    state::DbPool,
    types::{ApiError, ApiResult, new_uuid_v7},
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, QueryBuilder, Sqlite};
use vust_contracts::{
    api::ErrorCode,
    scripts::{
        AgentScriptRunReport, CreateScriptRequest, ScriptCapabilities, ScriptDetail,
        ScriptExecutionDefaults, ScriptListPage, ScriptOwnership, ScriptOwnershipKind, ScriptRun,
        ScriptRunCapabilities, ScriptRunStatus, ScriptSource, ScriptSummary, UpdateScriptRequest,
    },
};

const ACTIVE_STATUSES: &[&str] = &["queued", "starting", "running", "cancelling"];

/// 可信脚本操作用户。
#[derive(Debug, Clone)]
pub struct ScriptActor {
    pub user_id: i64,
    pub name: String,
    pub client_ip: String,
    pub trace_id: String,
}

/// 脚本列表筛选条件。
pub struct ScriptListFilter<'a> {
    pub keyword: Option<&'a str>,
    pub page: u32,
    pub page_size: u32,
    pub sort_by: &'a str,
    pub sort_order: &'a str,
}

/// Master 脚本持久化行。
#[derive(Debug, Clone, FromRow)]
pub struct ScriptRow {
    pub script_id: String,
    pub name: String,
    pub description: Option<String>,
    pub interactive: bool,
    pub language: String,
    pub content: String,
    pub content_size_bytes: i64,
    pub source_sha256: String,
    pub default_timeout_seconds: i64,
    pub revision: i64,
    pub ownership_kind: String,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    pub manager_path: Option<String>,
    pub updated_by_name: String,
    pub created_at: String,
    pub updated_at: String,
    pub has_active_run: bool,
}

/// Master 脚本运行持久化行。
#[derive(Debug, Clone, FromRow)]
pub struct ScriptRunRow {
    pub run_id: String,
    pub script_id: String,
    pub script_name: String,
    pub script_revision: i64,
    pub source_sha256: String,
    pub source_content: String,
    pub timeout_seconds: i64,
    pub node_id: String,
    pub node_name: String,
    pub status: String,
    pub phase: Option<String>,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub actor_user_id: i64,
    pub actor_name: String,
    pub client_ip: String,
    pub trace_id: String,
    pub terminal_logged_at: Option<String>,
    pub discard_requested: bool,
    pub connection_deadline: Option<String>,
    pub attached_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 关闭临时执行的结果。
pub struct DismissRunResult {
    pub run: Option<ScriptRunRow>,
    pub cancellation_requested: bool,
}

const SCRIPT_SELECT: &str = r#"
SELECT s.script_id, s.name, s.description, s.interactive, s.language, s.content, s.content_size_bytes,
       s.source_sha256, s.default_timeout_seconds, s.revision, s.ownership_kind,
       s.owner_id, s.owner_name, s.manager_path, s.updated_by_name,
       s.created_at, s.updated_at,
       EXISTS(SELECT 1 FROM script_runs ar WHERE ar.script_id = s.script_id
              AND ar.status IN ('queued','starting','running','cancelling')) AS has_active_run
FROM scripts s"#;

const SCRIPT_LIST_SELECT: &str = r#"
SELECT s.script_id, s.name, s.description, s.interactive, s.language, '' AS content, 0 AS content_size_bytes,
       '' AS source_sha256, s.default_timeout_seconds, s.revision, s.ownership_kind,
       s.owner_id, s.owner_name, s.manager_path, s.updated_by_name,
       s.created_at, s.updated_at,
       EXISTS(SELECT 1 FROM script_runs ar WHERE ar.script_id = s.script_id
              AND ar.status IN ('queued','starting','running','cancelling')) AS has_active_run
FROM scripts s"#;

/// 校验并规范化脚本名称。
pub fn normalize_name(name: &str) -> ApiResult<(String, String)> {
    let value = name.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            ErrorCode::ScriptInvalidName,
            "script name is invalid",
        ));
    }
    Ok((value.to_string(), value.to_lowercase()))
}

/// 校验脚本说明、正文和超时。
pub fn validate_fields(description: Option<&str>, content: &str, timeout: u32) -> ApiResult<()> {
    if description.is_some_and(|v| v.chars().count() > 500 || v.chars().any(|c| c == '\0')) {
        return Err(ApiError::bad_request(
            ErrorCode::ScriptInvalidDescription,
            "script description is invalid",
        ));
    }
    let size = content.len();
    if content.trim().is_empty() || size > 262_144 || content.contains('\0') {
        return Err(ApiError::bad_request(
            ErrorCode::ScriptInvalidContent,
            "script content is invalid",
        ));
    }
    if !(1..=86_400).contains(&timeout) {
        return Err(ApiError::bad_request(
            ErrorCode::ScriptInvalidContent,
            "script timeout is invalid",
        ));
    }
    Ok(())
}

/// 计算正文 SHA-256。
pub fn source_sha256(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// 分页查询自定义脚本。
pub async fn list(pool: &DbPool, filter: &ScriptListFilter<'_>) -> ApiResult<ScriptListPage> {
    let mut count = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(*) FROM scripts s WHERE s.ownership_kind = 'custom'",
    );
    push_script_filter(&mut count, filter.keyword);
    let total: i64 = count.build_query_scalar().fetch_one(pool).await?;

    let mut query = QueryBuilder::<Sqlite>::new(SCRIPT_LIST_SELECT);
    query.push(" WHERE s.ownership_kind = 'custom'");
    push_script_filter(&mut query, filter.keyword);
    let sort = match filter.sort_by {
        "name" => "s.name_key",
        _ => "s.updated_at",
    };
    let order = if filter.sort_order == "asc" {
        "ASC"
    } else {
        "DESC"
    };
    query
        .push(format!(
            " ORDER BY {sort} {order}, s.script_id {order} LIMIT "
        ))
        .push_bind(i64::from(filter.page_size))
        .push(" OFFSET ")
        .push_bind(i64::from((filter.page - 1) * filter.page_size));
    let rows: Vec<ScriptRow> = query.build_query_as().fetch_all(pool).await?;
    Ok(ScriptListPage {
        items: rows.into_iter().map(summary).collect::<ApiResult<_>>()?,
        page: filter.page,
        page_size: filter.page_size,
        total: total.max(0) as u64,
        loaded_at: now(),
    })
}

fn push_script_filter<'a>(query: &mut QueryBuilder<'a, Sqlite>, keyword: Option<&'a str>) {
    if let Some(keyword) = keyword.filter(|v| !v.is_empty()) {
        let pattern = format!("%{}%", keyword.to_lowercase());
        query
            .push(" AND (s.name_key LIKE ")
            .push_bind(pattern.clone())
            .push(" OR lower(COALESCE(s.description, '')) LIKE ")
            .push_bind(pattern)
            .push(")");
    }
}

/// 查询活动脚本详情。
pub async fn detail(pool: &DbPool, script_id: &str) -> ApiResult<ScriptDetail> {
    let row = script_row(pool, script_id)
        .await?
        .ok_or_else(script_not_found)?;
    detail_from_row(row)
}

/// 创建自定义脚本。
pub async fn create(
    pool: &DbPool,
    request: &CreateScriptRequest,
    actor: &ScriptActor,
) -> ApiResult<ScriptDetail> {
    let timeout = request.timeout_seconds.unwrap_or(300);
    let (name, name_key) = normalize_name(&request.name)?;
    validate_fields(request.description.as_deref(), &request.content, timeout)?;
    let id = new_uuid_v7();
    let timestamp = now();
    let result = sqlx::query("INSERT INTO scripts (script_id,name,name_key,description,interactive,language,content,content_size_bytes,source_sha256,default_timeout_seconds,revision,ownership_kind,created_by_user_id,created_by_name,updated_by_user_id,updated_by_name,created_at,updated_at) VALUES (?,?,?,?,?, 'shell', ?,?,?,?,1,'custom',?,?,?,?,?,?)")
        .bind(&id).bind(name).bind(name_key).bind(trim_description(request.description.as_deref()))
        .bind(request.interactive).bind(&request.content).bind(request.content.len() as i64).bind(source_sha256(&request.content))
        .bind(i64::from(timeout)).bind(actor.user_id).bind(&actor.name).bind(actor.user_id)
        .bind(&actor.name).bind(&timestamp).bind(&timestamp).execute(pool).await;
    if let Err(error) = result {
        if is_unique(&error) {
            return Err(ApiError::conflict(
                ErrorCode::ScriptAlreadyExists,
                "script name already exists",
            ));
        }
        return Err(error.into());
    }
    detail(pool, &id).await
}

/// 以 revision 乐观锁更新脚本。
pub async fn update(
    pool: &DbPool,
    script_id: &str,
    request: &UpdateScriptRequest,
    actor: &ScriptActor,
) -> ApiResult<ScriptDetail> {
    let (name, name_key) = normalize_name(&request.name)?;
    validate_fields(
        request.description.as_deref(),
        &request.content,
        request.timeout_seconds,
    )?;
    let result = sqlx::query("UPDATE scripts SET name=?,name_key=?,description=?,interactive=?,content=?,content_size_bytes=?,source_sha256=?,default_timeout_seconds=?,revision=revision+1,updated_by_user_id=?,updated_by_name=?,updated_at=? WHERE script_id=? AND ownership_kind='custom' AND revision=?")
        .bind(name).bind(name_key).bind(trim_description(request.description.as_deref()))
        .bind(request.interactive).bind(&request.content).bind(request.content.len() as i64).bind(source_sha256(&request.content))
        .bind(i64::from(request.timeout_seconds)).bind(actor.user_id).bind(&actor.name).bind(now())
        .bind(script_id).bind(request.expected_revision).execute(pool).await;
    match result {
        Ok(done) if done.rows_affected() == 1 => detail(pool, script_id).await,
        Ok(_) => {
            if script_row(pool, script_id).await?.is_none() {
                Err(script_not_found())
            } else {
                Err(ApiError::conflict(
                    ErrorCode::ScriptRevisionConflict,
                    "script revision changed",
                ))
            }
        }
        Err(error) if is_unique(&error) => Err(ApiError::conflict(
            ErrorCode::ScriptAlreadyExists,
            "script name already exists",
        )),
        Err(error) => Err(error.into()),
    }
}

/// 物理删除没有活动执行的脚本及其临时执行数据。
pub async fn remove(pool: &DbPool, script_id: &str) -> ApiResult<()> {
    let row = script_row(pool, script_id)
        .await?
        .ok_or_else(script_not_found)?;
    if row.ownership_kind != "custom" {
        return Err(ApiError::conflict(
            ErrorCode::ScriptProtected,
            "script is managed",
        ));
    }
    if row.has_active_run {
        return Err(ApiError::conflict(
            ErrorCode::ScriptInUse,
            "script has an active run",
        ));
    }
    sqlx::query("DELETE FROM scripts WHERE script_id=?")
        .bind(script_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 创建或恢复幂等的一次运行。
pub async fn create_run(
    pool: &DbPool,
    script_id: &str,
    node_id: &str,
    node_name: &str,
    timeout_override: Option<u32>,
    idempotency_key: &str,
    actor: &ScriptActor,
) -> ApiResult<(ScriptRunRow, bool)> {
    if let Some(existing) = run_by_idempotency(pool, idempotency_key).await? {
        if existing.script_id != script_id || existing.node_id != node_id {
            return Err(ApiError::conflict(
                ErrorCode::ScriptRunConflict,
                "idempotency key was already used for another script run",
            ));
        }
        return Ok((existing, false));
    }
    let script = script_row(pool, script_id)
        .await?
        .ok_or_else(script_not_found)?;
    let timeout = timeout_override.unwrap_or(script.default_timeout_seconds as u32);
    validate_fields(script.description.as_deref(), &script.content, timeout)?;
    let run_id = new_uuid_v7();
    let timestamp = now();
    let guard = format!("{script_id}:{node_id}");
    let connection_deadline = (Utc::now() + Duration::seconds(30)).to_rfc3339();
    let result = sqlx::query("INSERT INTO script_runs (run_id,idempotency_key,script_id,script_name,script_revision,source_sha256,source_content,timeout_seconds,node_id,node_name,status,phase,queued_at,overlap_guard,actor_user_id,actor_name,client_ip,trace_id,connection_deadline,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,'queued','queued',?,?,?,?,?,?,?,?,?)")
        .bind(&run_id).bind(idempotency_key).bind(script_id).bind(&script.name).bind(script.revision)
        .bind(&script.source_sha256).bind(&script.content).bind(i64::from(timeout)).bind(node_id).bind(node_name)
        .bind(&timestamp).bind(guard).bind(actor.user_id).bind(&actor.name).bind(&actor.client_ip)
        .bind(&actor.trace_id).bind(connection_deadline).bind(&timestamp).bind(&timestamp).execute(pool).await;
    if let Err(error) = result {
        if is_unique(&error) {
            if let Some(existing) = run_by_idempotency(pool, idempotency_key).await? {
                return Ok((existing, false));
            }
            return Err(ApiError::conflict(
                ErrorCode::ScriptRunConflict,
                "script is already running on this node",
            ));
        }
        return Err(error.into());
    }
    Ok((required_run(pool, &run_id).await?, true))
}

/// 关闭并销毁临时执行；活动执行先进入取消流程，终态执行立即删除。
pub async fn dismiss_run(pool: &DbPool, run_id: &str) -> ApiResult<DismissRunResult> {
    let Some(run) = optional_run(pool, run_id).await? else {
        return Ok(DismissRunResult {
            run: None,
            cancellation_requested: false,
        });
    };
    if run.status == "queued" {
        let deleted = sqlx::query("DELETE FROM script_runs WHERE run_id=? AND status='queued'")
            .bind(run_id)
            .execute(pool)
            .await?;
        if deleted.rows_affected() == 1 {
            return Ok(DismissRunResult {
                run: Some(run),
                cancellation_requested: false,
            });
        }
    }
    let Some(run) = optional_run(pool, run_id).await? else {
        return Ok(DismissRunResult {
            run: None,
            cancellation_requested: false,
        });
    };
    if !ACTIVE_STATUSES.contains(&run.status.as_str()) {
        if run.terminal_logged_at.is_some() {
            sqlx::query("DELETE FROM script_runs WHERE run_id=?")
                .bind(run_id)
                .execute(pool)
                .await?;
        } else {
            sqlx::query("UPDATE script_runs SET discard_requested=1,updated_at=? WHERE run_id=?")
                .bind(now())
                .bind(run_id)
                .execute(pool)
                .await?;
        }
        return Ok(DismissRunResult {
            run: None,
            cancellation_requested: false,
        });
    }
    sqlx::query("UPDATE script_runs SET discard_requested=1,status='cancelling',phase='cancelling',updated_at=? WHERE run_id=? AND status IN ('queued','starting','running','cancelling')")
        .bind(now()).bind(run_id).execute(pool).await?;
    Ok(DismissRunResult {
        run: Some(required_run(pool, run_id).await?),
        cancellation_requested: true,
    })
}

/// 获取需要向 Agent 重试取消的运行。
pub async fn cancelling_runs(pool: &DbPool) -> ApiResult<Vec<ScriptRunRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM script_runs WHERE status='cancelling' ORDER BY updated_at LIMIT 100",
    )
    .fetch_all(pool)
    .await?)
}

/// 由所属用户原子领取终端运行，供 Master WebSocket 网关投递。
pub async fn claim_terminal(
    pool: &DbPool,
    run_id: &str,
    actor_user_id: i64,
) -> ApiResult<ScriptRunRow> {
    let changed=sqlx::query("UPDATE script_runs SET status='starting',phase='awaiting_connection',updated_at=? WHERE run_id=? AND actor_user_id=? AND status='queued' AND connection_deadline>? AND attached_at IS NULL")
        .bind(now()).bind(run_id).bind(actor_user_id).bind(now()).execute(pool).await?;
    if changed.rows_affected() != 1 {
        let row = required_run(pool, run_id).await?;
        let message = if row.actor_user_id != actor_user_id {
            "script run does not belong to the current user"
        } else {
            "script run terminal is already attached, expired, or no longer active"
        };
        return Err(ApiError::conflict(ErrorCode::ScriptRunConflict, message));
    }
    required_run(pool, run_id).await
}

/// 标记 Agent WebSocket 已建立，停止 30 秒连接截止计时。
pub async fn mark_terminal_attached(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    let timestamp = now();
    let changed=sqlx::query("UPDATE script_runs SET attached_at=?,connection_deadline=NULL,phase='awaiting_start',updated_at=? WHERE run_id=? AND status='starting' AND attached_at IS NULL")
        .bind(&timestamp).bind(&timestamp).bind(run_id).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "script run terminal could not be attached",
        ));
    }
    Ok(())
}

/// 将超过连接截止时间的终端运行收敛为取消终态并请求销毁。
pub async fn expire_unattached_terminals(pool: &DbPool) -> ApiResult<Vec<String>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT run_id,status FROM script_runs WHERE attached_at IS NULL AND connection_deadline<=? AND status IN ('queued','starting')")
        .bind(now()).fetch_all(pool).await?;
    for (id, status) in &rows {
        let timestamp = now();
        if status == "starting" {
            sqlx::query("UPDATE script_runs SET status='cancelling',phase='cancelling',error_code='SCRIPT_RUN_TERMINAL_ATTACH_TIMEOUT',error_summary='script run terminal was not connected in time',discard_requested=1,connection_deadline=NULL,updated_at=? WHERE run_id=? AND attached_at IS NULL AND status='starting'")
                .bind(&timestamp).bind(id).execute(pool).await?;
        } else {
            sqlx::query("UPDATE script_runs SET status='cancelled',phase=NULL,finished_at=?,error_code='SCRIPT_RUN_TERMINAL_ATTACH_TIMEOUT',error_summary='script run terminal was not connected in time',discard_requested=1,overlap_guard=NULL,connection_deadline=NULL,updated_at=? WHERE run_id=? AND attached_at IS NULL AND status='queued'")
                .bind(&timestamp).bind(&timestamp).bind(id).execute(pool).await?;
        }
    }
    Ok(rows.into_iter().map(|(id, _)| id).collect())
}

/// Agent 接受未绑定终端的取消后，在 Master 侧形成连接超时终态。
pub async fn finish_terminal_attach_timeout(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    let timestamp = now();
    sqlx::query("UPDATE script_runs SET status='cancelled',phase=NULL,finished_at=?,overlap_guard=NULL,updated_at=? WHERE run_id=? AND status='cancelling' AND attached_at IS NULL AND error_code='SCRIPT_RUN_TERMINAL_ATTACH_TIMEOUT'")
        .bind(&timestamp).bind(&timestamp).bind(run_id).execute(pool).await?;
    Ok(())
}

/// 查询内部运行快照。
pub async fn required_run_row(pool: &DbPool, run_id: &str) -> ApiResult<ScriptRunRow> {
    required_run(pool, run_id).await
}

/// 节点在投递阶段失联时，将尚未启动的运行收敛为可追踪失败终态。
pub async fn fail_dispatch(pool: &DbPool, run_id: &str, summary: &str) -> ApiResult<()> {
    let timestamp = now();
    sqlx::query("UPDATE script_runs SET status=CASE WHEN discard_requested=1 THEN 'cancelled' ELSE 'failed' END,phase=NULL,finished_at=?,error_code=CASE WHEN discard_requested=1 THEN NULL ELSE 'SCRIPT_NODE_UNAVAILABLE' END,error_summary=CASE WHEN discard_requested=1 THEN NULL ELSE ? END,discard_requested=1,overlap_guard=NULL,connection_deadline=NULL,updated_at=? WHERE run_id=? AND status IN ('starting','cancelling')")
        .bind(&timestamp).bind(summary).bind(&timestamp).bind(run_id).execute(pool).await?;
    Ok(())
}

/// 保存并校验 Agent 运行上报。
pub async fn save_report(
    pool: &DbPool,
    session_node_id: &str,
    report: &AgentScriptRunReport,
) -> ApiResult<()> {
    let Some(current) = optional_run(pool, &report.run_id).await? else {
        // Master 可能已处理并销毁要求关闭的终态执行；Agent 重试时仍需得到幂等确认。
        return Ok(());
    };
    if current.node_id != session_node_id || current.script_id != report.script_id {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "script run does not belong to this node",
        ));
    }
    if current.script_revision != report.script_revision
        || current.source_sha256 != report.source_sha256
    {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "script run report snapshot does not match",
        ));
    }
    validate_report(report)?;
    let current_status = run_status_from_text(&current.status)?;
    if current_status.is_terminal() {
        return Ok(());
    }
    if !transition_allowed(current_status, report.status) {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "invalid script run status transition",
        ));
    }
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE script_runs SET status=?,phase=?,started_at=COALESCE(?,started_at),finished_at=?,exit_code=?,error_code=?,error_summary=?,discard_requested=CASE WHEN ? IN ('succeeded','failed','timed_out','cancelled') THEN 1 ELSE discard_requested END,overlap_guard=CASE WHEN ? IN ('succeeded','failed','timed_out','cancelled') THEN NULL ELSE overlap_guard END,connection_deadline=CASE WHEN ? IN ('running','succeeded','failed','timed_out','cancelled') THEN NULL ELSE connection_deadline END,updated_at=? WHERE run_id=?")
        .bind(run_status_text(report.status)).bind(if report.status.is_terminal(){None}else{report.phase.as_deref()})
        .bind(&report.started_at).bind(&report.finished_at).bind(report.exit_code).bind(&report.error_code)
        .bind(&report.error_summary)
        .bind(run_status_text(report.status)).bind(run_status_text(report.status)).bind(run_status_text(report.status)).bind(now()).bind(&report.run_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

fn validate_report(report: &AgentScriptRunReport) -> ApiResult<()> {
    if report
        .error_summary
        .as_ref()
        .is_some_and(|value| value.len() > 500)
        || report
            .error_code
            .as_ref()
            .is_some_and(|value| value.len() > 80)
    {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "script run report contains invalid summary fields",
        ));
    }
    Ok(())
}

/// 原子领取一次终态操作日志。
pub async fn claim_terminal_audit(pool: &DbPool, run_id: &str) -> ApiResult<Option<ScriptRunRow>> {
    let timestamp = now();
    let done = sqlx::query("UPDATE script_runs SET terminal_logged_at=? WHERE run_id=? AND terminal_logged_at IS NULL AND status IN ('succeeded','failed','timed_out','cancelled')")
        .bind(&timestamp).bind(run_id).execute(pool).await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    Ok(Some(required_run(pool, run_id).await?))
}

/// 清理已经进入终态且明确要求销毁的临时执行。
pub async fn cleanup(pool: &DbPool) -> ApiResult<()> {
    sqlx::query("DELETE FROM script_runs WHERE discard_requested=1 AND terminal_logged_at IS NOT NULL AND status IN ('succeeded','failed','timed_out','cancelled')")
        .execute(pool).await?;
    Ok(())
}

/// 在终态操作审计完成后删除明确要求销毁的执行。
pub async fn cleanup_discarded_run(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM script_runs WHERE run_id=? AND discard_requested=1 AND terminal_logged_at IS NOT NULL AND status IN ('succeeded','failed','timed_out','cancelled')")
        .bind(run_id).execute(pool).await?;
    Ok(())
}

async fn script_row(pool: &DbPool, script_id: &str) -> ApiResult<Option<ScriptRow>> {
    let mut query = QueryBuilder::<Sqlite>::new(SCRIPT_SELECT);
    query.push(" WHERE s.script_id=").push_bind(script_id);
    Ok(query.build_query_as().fetch_optional(pool).await?)
}

async fn required_run(pool: &DbPool, run_id: &str) -> ApiResult<ScriptRunRow> {
    optional_run(pool, run_id)
        .await?
        .ok_or_else(|| ApiError::not_found(ErrorCode::ScriptRunNotFound, "script run not found"))
}

async fn optional_run(pool: &DbPool, run_id: &str) -> ApiResult<Option<ScriptRunRow>> {
    Ok(sqlx::query_as("SELECT * FROM script_runs WHERE run_id=?")
        .bind(run_id)
        .fetch_optional(pool)
        .await?)
}

async fn run_by_idempotency(pool: &DbPool, key: &str) -> ApiResult<Option<ScriptRunRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM script_runs WHERE idempotency_key=?")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}

fn summary(row: ScriptRow) -> ApiResult<ScriptSummary> {
    let ownership = ownership_from_text(&row.ownership_kind)?;
    Ok(ScriptSummary {
        script_id: row.script_id,
        name: row.name,
        description: row.description,
        interactive: row.interactive,
        language: row.language,
        revision: row.revision,
        ownership: ScriptOwnership {
            kind: ownership,
            owner_id: row.owner_id,
            owner_name: row.owner_name,
            manager_path: row.manager_path,
        },
        capabilities: capabilities(ownership, row.has_active_run),
        created_at: row.created_at,
        updated_at: row.updated_at,
        updated_by: row.updated_by_name,
    })
}

fn detail_from_row(row: ScriptRow) -> ApiResult<ScriptDetail> {
    let source = ScriptSource {
        content: row.content.clone(),
        size_bytes: row.content_size_bytes.max(0) as u64,
        sha256: row.source_sha256.clone(),
    };
    let defaults = ScriptExecutionDefaults {
        timeout_seconds: row.default_timeout_seconds.clamp(1, 86_400) as u32,
    };
    Ok(ScriptDetail {
        summary: summary(row)?,
        source,
        execution_defaults: defaults,
    })
}

/// 将持久化行转换为公共运行 DTO。
pub fn run_dto(row: ScriptRunRow) -> ApiResult<ScriptRun> {
    let status = run_status_from_text(&row.status)?;
    Ok(ScriptRun {
        run_id: row.run_id,
        script_id: row.script_id,
        script_name: row.script_name,
        script_revision: row.script_revision,
        source_sha256: row.source_sha256,
        node_id: row.node_id,
        node_name: row.node_name,
        status,
        phase: if status.is_terminal() {
            None
        } else {
            row.phase
        },
        queued_at: row.queued_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        exit_code: row.exit_code,
        error_code: row.error_code,
        error_summary: row.error_summary,
        capabilities: ScriptRunCapabilities {
            can_cancel: !status.is_terminal(),
        },
    })
}

/// 统一计算脚本能力。
pub fn capabilities(kind: ScriptOwnershipKind, active_run: bool) -> ScriptCapabilities {
    let custom = kind == ScriptOwnershipKind::Custom;
    ScriptCapabilities {
        can_update: custom,
        can_remove: custom && !active_run,
        can_clone: custom,
        can_run: custom,
    }
}

fn ownership_from_text(value: &str) -> ApiResult<ScriptOwnershipKind> {
    match value {
        "custom" => Ok(ScriptOwnershipKind::Custom),
        "compose" => Ok(ScriptOwnershipKind::Compose),
        "suite" => Ok(ScriptOwnershipKind::Suite),
        "system" => Ok(ScriptOwnershipKind::System),
        _ => Err(ApiError::internal("invalid script ownership")),
    }
}
fn run_status_text(value: ScriptRunStatus) -> &'static str {
    match value {
        ScriptRunStatus::Queued => "queued",
        ScriptRunStatus::Starting => "starting",
        ScriptRunStatus::Running => "running",
        ScriptRunStatus::Cancelling => "cancelling",
        ScriptRunStatus::Succeeded => "succeeded",
        ScriptRunStatus::Failed => "failed",
        ScriptRunStatus::TimedOut => "timed_out",
        ScriptRunStatus::Cancelled => "cancelled",
    }
}
pub fn run_status_from_text(value: &str) -> ApiResult<ScriptRunStatus> {
    match value {
        "queued" => Ok(ScriptRunStatus::Queued),
        "starting" => Ok(ScriptRunStatus::Starting),
        "running" => Ok(ScriptRunStatus::Running),
        "cancelling" => Ok(ScriptRunStatus::Cancelling),
        "succeeded" => Ok(ScriptRunStatus::Succeeded),
        "failed" => Ok(ScriptRunStatus::Failed),
        "timed_out" => Ok(ScriptRunStatus::TimedOut),
        "cancelled" => Ok(ScriptRunStatus::Cancelled),
        _ => Err(ApiError::internal("invalid script run status")),
    }
}
fn transition_allowed(from: ScriptRunStatus, to: ScriptRunStatus) -> bool {
    from == to
        || matches!(
            (from, to),
            (
                ScriptRunStatus::Queued,
                ScriptRunStatus::Starting
                    | ScriptRunStatus::Running
                    | ScriptRunStatus::Cancelling
                    | ScriptRunStatus::Succeeded
                    | ScriptRunStatus::Failed
                    | ScriptRunStatus::TimedOut
                    | ScriptRunStatus::Cancelled
            ) | (
                ScriptRunStatus::Starting,
                ScriptRunStatus::Running
                    | ScriptRunStatus::Cancelling
                    | ScriptRunStatus::Succeeded
                    | ScriptRunStatus::Failed
                    | ScriptRunStatus::TimedOut
                    | ScriptRunStatus::Cancelled
            ) | (
                ScriptRunStatus::Running,
                ScriptRunStatus::Cancelling
                    | ScriptRunStatus::Succeeded
                    | ScriptRunStatus::Failed
                    | ScriptRunStatus::TimedOut
                    | ScriptRunStatus::Cancelled
            ) | (
                ScriptRunStatus::Cancelling,
                ScriptRunStatus::Succeeded
                    | ScriptRunStatus::Failed
                    | ScriptRunStatus::TimedOut
                    | ScriptRunStatus::Cancelled
            )
        )
}
fn trim_description(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}
fn script_not_found() -> ApiError {
    ApiError::not_found(ErrorCode::ScriptNotFound, "script not found")
}
fn is_unique(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.is_unique_violation())
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> ScriptActor {
        ScriptActor {
            user_id: 1,
            name: "tester".into(),
            client_ip: "127.0.0.1".into(),
            trace_id: "trace".into(),
        }
    }
    fn request(name: &str) -> CreateScriptRequest {
        CreateScriptRequest {
            name: name.into(),
            description: Some("demo".into()),
            interactive: false,
            content: "echo ok".into(),
            timeout_seconds: Some(30),
        }
    }

    #[tokio::test]
    async fn script_crud_is_revisioned_unique_and_hard_deleted() {
        let pool = crate::test_support::setup_test_db().await;
        let first = create(&pool, &request("Alpha"), &actor()).await.unwrap();
        assert!(create(&pool, &request(" alpha "), &actor()).await.is_err());
        let updated = update(
            &pool,
            &first.summary.script_id,
            &UpdateScriptRequest {
                expected_revision: 1,
                name: "Beta".into(),
                description: None,
                interactive: true,
                content: "echo changed".into(),
                timeout_seconds: 40,
            },
            &actor(),
        )
        .await
        .unwrap();
        assert_eq!(updated.summary.revision, 2);
        assert!(updated.summary.interactive);
        assert!(updated.summary.capabilities.can_run);
        assert!(
            update(
                &pool,
                &first.summary.script_id,
                &UpdateScriptRequest {
                    expected_revision: 1,
                    name: "Late".into(),
                    description: None,
                    interactive: false,
                    content: "echo late".into(),
                    timeout_seconds: 40
                },
                &actor()
            )
            .await
            .is_err()
        );
        remove(&pool, &first.summary.script_id).await.unwrap();
        assert!(detail(&pool, &first.summary.script_id).await.is_err());
    }

    #[tokio::test]
    async fn run_is_idempotent_and_blocks_overlap_and_delete() {
        let pool = crate::test_support::setup_test_db().await;
        let script = create(&pool, &request("Run"), &actor()).await.unwrap();
        let (run, created) = create_run(
            &pool,
            &script.summary.script_id,
            "local",
            "local",
            None,
            "key-1",
            &actor(),
        )
        .await
        .unwrap();
        assert!(created);
        let (same, created) = create_run(
            &pool,
            &script.summary.script_id,
            "local",
            "local",
            None,
            "key-1",
            &actor(),
        )
        .await
        .unwrap();
        assert!(!created);
        assert_eq!(same.run_id, run.run_id);
        assert!(
            create_run(
                &pool,
                &script.summary.script_id,
                "local",
                "local",
                None,
                "key-2",
                &actor()
            )
            .await
            .is_err()
        );
        assert!(remove(&pool, &script.summary.script_id).await.is_err());
    }

    #[tokio::test]
    async fn every_script_run_requires_one_terminal_connection() {
        let pool = crate::test_support::setup_test_db().await;
        let mut create_request = request("Interactive");
        create_request.interactive = true;
        let script = create(&pool, &create_request, &actor()).await.unwrap();
        let (run, created) = create_run(
            &pool,
            &script.summary.script_id,
            "local",
            "local",
            None,
            "interactive-key",
            &actor(),
        )
        .await
        .unwrap();
        assert!(created);
        assert!(run.connection_deadline.is_some());
        let claimed = claim_terminal(&pool, &run.run_id, actor().user_id)
            .await
            .unwrap();
        assert_eq!(claimed.phase.as_deref(), Some("awaiting_connection"));
        assert!(
            claim_terminal(&pool, &run.run_id, actor().user_id)
                .await
                .is_err()
        );
        mark_terminal_attached(&pool, &run.run_id).await.unwrap();
        assert!(
            required_run_row(&pool, &run.run_id)
                .await
                .unwrap()
                .attached_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn terminal_connection_deadline_cancels_queued_and_prepared_runs() {
        let pool = crate::test_support::setup_test_db().await;
        let queued_script = create(&pool, &request("Queued timeout"), &actor())
            .await
            .unwrap();
        let prepared_script = create(&pool, &request("Prepared timeout"), &actor())
            .await
            .unwrap();
        let (queued, _) = create_run(
            &pool,
            &queued_script.summary.script_id,
            "local",
            "Local Node",
            None,
            "queued-timeout",
            &actor(),
        )
        .await
        .unwrap();
        let (prepared, _) = create_run(
            &pool,
            &prepared_script.summary.script_id,
            "local",
            "Local Node",
            None,
            "prepared-timeout",
            &actor(),
        )
        .await
        .unwrap();
        claim_terminal(&pool, &prepared.run_id, actor().user_id)
            .await
            .unwrap();
        sqlx::query("UPDATE script_runs SET connection_deadline='2000-01-01T00:00:00Z'")
            .execute(&pool)
            .await
            .unwrap();

        let expired = expire_unattached_terminals(&pool).await.unwrap();

        assert_eq!(expired.len(), 2);
        assert_eq!(
            required_run(&pool, &queued.run_id).await.unwrap().status,
            "cancelled"
        );
        assert_eq!(
            required_run(&pool, &prepared.run_id).await.unwrap().status,
            "cancelling"
        );
        finish_terminal_attach_timeout(&pool, &prepared.run_id)
            .await
            .unwrap();
        assert_eq!(
            required_run(&pool, &prepared.run_id).await.unwrap().status,
            "cancelled"
        );
    }

    #[tokio::test]
    async fn terminal_gateway_failure_is_audited_before_cleanup() {
        let pool = crate::test_support::setup_test_db().await;
        let script = create(&pool, &request("Gateway failure"), &actor())
            .await
            .unwrap();
        let (run, _) = create_run(
            &pool,
            &script.summary.script_id,
            "local",
            "Local Node",
            None,
            "gateway-failure",
            &actor(),
        )
        .await
        .unwrap();
        claim_terminal(&pool, &run.run_id, actor().user_id)
            .await
            .unwrap();

        fail_dispatch(&pool, &run.run_id, "gateway unavailable")
            .await
            .unwrap();

        let failed = required_run(&pool, &run.run_id).await.unwrap();
        assert_eq!(failed.status, "failed");
        assert!(failed.discard_requested);
        assert!(
            claim_terminal_audit(&pool, &run.run_id)
                .await
                .unwrap()
                .is_some()
        );
        cleanup_discarded_run(&pool, &run.run_id).await.unwrap();
        assert!(required_run(&pool, &run.run_id).await.is_err());
    }

    #[tokio::test]
    async fn dismissed_execution_is_cancelled_and_destroyed_after_terminal_audit() {
        let pool = crate::test_support::setup_test_db().await;
        let script = create(&pool, &request("Dismiss"), &actor()).await.unwrap();
        let (run_row, _) = create_run(
            &pool,
            &script.summary.script_id,
            "local",
            "Local Node",
            None,
            "dismiss-key",
            &actor(),
        )
        .await
        .unwrap();
        claim_terminal(&pool, &run_row.run_id, actor().user_id)
            .await
            .unwrap();
        let dismissed = dismiss_run(&pool, &run_row.run_id).await.unwrap();
        assert!(dismissed.cancellation_requested);
        assert!(dismissed.run.unwrap().discard_requested);

        let terminal_report = AgentScriptRunReport {
            run_id: run_row.run_id.clone(),
            script_id: run_row.script_id,
            script_revision: run_row.script_revision,
            source_sha256: run_row.source_sha256,
            status: ScriptRunStatus::Cancelled,
            phase: None,
            started_at: None,
            finished_at: Some(now()),
            exit_code: None,
            error_code: None,
            error_summary: None,
        };
        save_report(&pool, "local", &terminal_report).await.unwrap();
        assert!(
            claim_terminal_audit(&pool, &run_row.run_id)
                .await
                .unwrap()
                .is_some()
        );
        cleanup_discarded_run(&pool, &run_row.run_id).await.unwrap();
        assert!(required_run_row(&pool, &run_row.run_id).await.is_err());
        save_report(&pool, "local", &terminal_report).await.unwrap();
        assert!(
            !dismiss_run(&pool, &run_row.run_id)
                .await
                .unwrap()
                .cancellation_requested
        );
    }

    #[tokio::test]
    async fn queued_execution_is_destroyed_before_dispatch() {
        let pool = crate::test_support::setup_test_db().await;
        let script = create(&pool, &request("Queued"), &actor()).await.unwrap();
        let (run_row, _) = create_run(
            &pool,
            &script.summary.script_id,
            "local",
            "Local Node",
            None,
            "queued-key",
            &actor(),
        )
        .await
        .unwrap();
        let dismissed = dismiss_run(&pool, &run_row.run_id).await.unwrap();
        assert!(!dismissed.cancellation_requested);
        assert!(dismissed.run.is_some());
        assert!(required_run_row(&pool, &run_row.run_id).await.is_err());
    }

    #[tokio::test]
    async fn agent_report_validates_snapshot_and_terminal_monotonicity() {
        let pool = crate::test_support::setup_test_db().await;
        let script = create(&pool, &request("Report"), &actor()).await.unwrap();
        let (run_row, _) = create_run(
            &pool,
            &script.summary.script_id,
            "local",
            "Local Node",
            None,
            "report-key",
            &actor(),
        )
        .await
        .unwrap();
        let report = AgentScriptRunReport {
            run_id: run_row.run_id.clone(),
            script_id: run_row.script_id.clone(),
            script_revision: run_row.script_revision,
            source_sha256: run_row.source_sha256.clone(),
            status: ScriptRunStatus::Succeeded,
            phase: None,
            started_at: Some(now()),
            finished_at: Some(now()),
            exit_code: Some(0),
            error_code: None,
            error_summary: None,
        };
        let mut invalid = report.clone();
        invalid.source_sha256 = "0".repeat(64);
        assert!(save_report(&pool, "local", &invalid).await.is_err());
        save_report(&pool, "local", &report).await.unwrap();
        assert!(
            required_run_row(&pool, &run_row.run_id)
                .await
                .unwrap()
                .discard_requested
        );
        let mut stale = report;
        stale.status = ScriptRunStatus::Running;
        stale.finished_at = None;
        save_report(&pool, "local", &stale).await.unwrap();
        assert_eq!(
            required_run_row(&pool, &run_row.run_id)
                .await
                .unwrap()
                .status,
            "succeeded"
        );
        assert!(
            !dismiss_run(&pool, &run_row.run_id)
                .await
                .unwrap()
                .cancellation_requested
        );
        assert!(
            claim_terminal_audit(&pool, &run_row.run_id)
                .await
                .unwrap()
                .is_some()
        );
        cleanup_discarded_run(&pool, &run_row.run_id).await.unwrap();
        assert!(required_run_row(&pool, &run_row.run_id).await.is_err());
    }
}
