//! Agent 脚本运行仓储：幂等终端会话、防重入、取消和可靠上报箱。

use crate::{
    state::DbPool,
    types::{ApiError, ApiResult},
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Row};
use vust_contracts::{
    api::ErrorCode,
    scripts::{AgentScriptRunReport, AgentStartScriptRunRequest, ScriptRunStatus},
};

/// Agent 本地脚本运行行。
#[derive(Debug, Clone, FromRow)]
pub struct ScriptRunRow {
    pub run_id: String,
    pub script_id: String,
    pub script_name: String,
    pub script_revision: i64,
    pub source_sha256: String,
    pub source_content: String,
    pub timeout_seconds: i64,
    pub status: String,
    pub phase: Option<String>,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub cancel_requested: bool,
    pub attached_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 创建幂等运行记录，返回是否首次创建。
pub async fn create(
    pool: &DbPool,
    request: &AgentStartScriptRunRequest,
) -> ApiResult<(ScriptRunRow, bool)> {
    if let Some(row) = get(pool, &request.run_id).await? {
        if row.script_id != request.script_id
            || row.script_revision != request.script_revision
            || row.source_sha256 != request.source_sha256
            || row.timeout_seconds != i64::from(request.timeout_seconds)
        {
            return Err(ApiError::conflict(
                ErrorCode::ScriptRunConflict,
                "script run idempotency payload does not match",
            ));
        }
        return Ok((row, false));
    }
    if request.ownership_kind != vust_contracts::scripts::ScriptOwnershipKind::Custom {
        return Err(ApiError::conflict(
            ErrorCode::ScriptProtected,
            "managed scripts cannot run through the public script library",
        ));
    }
    if request.source_content.trim().is_empty()
        || request.source_content.len() > 262_144
        || request.source_content.contains('\0')
        || !(1..=86_400).contains(&request.timeout_seconds)
    {
        return Err(ApiError::bad_request(
            ErrorCode::ScriptInvalidContent,
            "script run payload is invalid",
        ));
    }
    let digest = format!("{:x}", Sha256::digest(request.source_content.as_bytes()));
    if digest != request.source_sha256 {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "script source digest mismatch",
        ));
    }
    let timestamp = now();
    let result = sqlx::query("INSERT INTO script_runs (run_id,script_id,script_name,script_revision,source_sha256,source_content,timeout_seconds,status,phase,queued_at,overlap_guard,created_at,updated_at) VALUES (?,?,?,?,?,?,?,'queued','queued',?,?,?,?)")
        .bind(&request.run_id).bind(&request.script_id).bind(&request.script_name).bind(request.script_revision)
        .bind(&request.source_sha256).bind(&request.source_content).bind(i64::from(request.timeout_seconds))
        .bind(&timestamp).bind(&request.script_id).bind(&timestamp).bind(&timestamp).execute(pool).await;
    if let Err(error) = result {
        if is_unique(&error) {
            if let Some(row) = get(pool, &request.run_id).await? {
                return Ok((row, false));
            }
            return Err(ApiError::conflict(
                ErrorCode::ScriptRunConflict,
                "script is already running",
            ));
        }
        return Err(error.into());
    }
    Ok((required(pool, &request.run_id).await?, true))
}

/// 查询运行。
pub async fn get(pool: &DbPool, run_id: &str) -> ApiResult<Option<ScriptRunRow>> {
    Ok(sqlx::query_as("SELECT * FROM script_runs WHERE run_id=?")
        .bind(run_id)
        .fetch_optional(pool)
        .await?)
}
/// 查询运行并要求存在。
pub async fn required(pool: &DbPool, run_id: &str) -> ApiResult<ScriptRunRow> {
    get(pool, run_id)
        .await?
        .ok_or_else(|| ApiError::not_found(ErrorCode::ScriptRunNotFound, "script run not found"))
}

/// 将运行置为等待 WebSocket 绑定。
pub async fn mark_awaiting_connection(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE script_runs SET status='starting',phase='awaiting_connection',updated_at=? WHERE run_id=? AND status='queued'")
        .bind(now()).bind(run_id).execute(pool).await?;
    enqueue_report(pool, run_id).await
}

/// 原子绑定终端运行，确保一个运行最多只有一个 WebSocket 会话。
pub async fn attach_terminal(pool: &DbPool, run_id: &str) -> ApiResult<ScriptRunRow> {
    let timestamp = now();
    let result = sqlx::query("UPDATE script_runs SET attached_at=?,phase='awaiting_start',updated_at=? WHERE run_id=? AND status='starting' AND attached_at IS NULL")
        .bind(&timestamp).bind(&timestamp).bind(run_id).execute(pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "script run terminal is already attached or no longer active",
        ));
    }
    required(pool, run_id).await
}

/// 在创建进程前原子领取运行状态，取消中的运行不得启动。
pub async fn claim_running(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    let timestamp = now();
    let changed = sqlx::query("UPDATE script_runs SET status='running',phase='running',started_at=COALESCE(started_at,?),updated_at=? WHERE run_id=? AND status='starting' AND attached_at IS NOT NULL AND cancel_requested=0")
        .bind(&timestamp).bind(&timestamp).bind(run_id).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunConflict,
            "script run was cancelled before the process started",
        ));
    }
    enqueue_report(pool, run_id).await
}

/// 请求取消。
pub async fn request_cancel(pool: &DbPool, run_id: &str) -> ApiResult<ScriptRunRow> {
    let row = required(pool, run_id).await?;
    if status_from_text(&row.status)?.is_terminal() {
        return Err(ApiError::conflict(
            ErrorCode::ScriptRunNotCancellable,
            "script run is not cancellable",
        ));
    }
    if row.attached_at.is_none() {
        finish(pool, run_id, ScriptRunStatus::Cancelled, None, None, None).await?;
        return required(pool, run_id).await;
    }
    sqlx::query("UPDATE script_runs SET cancel_requested=1,status='cancelling',phase='cancelling',updated_at=? WHERE run_id=?")
        .bind(now()).bind(run_id).execute(pool).await?;
    enqueue_report(pool, run_id).await?;
    required(pool, run_id).await
}

/// 判断是否收到取消请求。
pub async fn cancel_requested(pool: &DbPool, run_id: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT cancel_requested FROM script_runs WHERE run_id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?,
    )
}

/// 原子保存终态和 outbox。
pub async fn finish(
    pool: &DbPool,
    run_id: &str,
    status: ScriptRunStatus,
    exit_code: Option<i32>,
    error_code: Option<&str>,
    error_summary: Option<&str>,
) -> ApiResult<()> {
    if !status.is_terminal() {
        return Err(ApiError::internal(
            "script run finish requires terminal status",
        ));
    }
    let timestamp = now();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE script_runs SET status=?,phase=NULL,finished_at=?,exit_code=?,error_code=?,error_summary=?,overlap_guard=NULL,updated_at=? WHERE run_id=?")
        .bind(status_text(status)).bind(&timestamp).bind(exit_code).bind(error_code).bind(error_summary)
        .bind(&timestamp).bind(run_id).execute(&mut *tx).await?;
    let report = build_report_tx(&mut tx, run_id).await?;
    sqlx::query("INSERT INTO script_run_outbox (run_id,payload,attempts,created_at,updated_at) VALUES (?,?,0,?,?) ON CONFLICT(run_id) DO UPDATE SET payload=excluded.payload,updated_at=excluded.updated_at")
        .bind(run_id).bind(serde_json::to_string(&report).map_err(|_|ApiError::internal("failed to encode script run report"))?)
        .bind(&timestamp).bind(&timestamp).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Agent 启动时将遗留活动运行收敛为失败终态。
pub async fn recover_interrupted(pool: &DbPool) -> ApiResult<()> {
    let ids:Vec<String>=sqlx::query_scalar("SELECT run_id FROM script_runs WHERE status IN ('queued','starting','running','cancelling')").fetch_all(pool).await?;
    for id in ids {
        finish(
            pool,
            &id,
            ScriptRunStatus::Failed,
            None,
            Some("SCRIPT_RUN_INTERRUPTED"),
            Some("script run was interrupted by agent restart"),
        )
        .await?;
    }
    Ok(())
}

/// 查询待确认上报。
pub async fn pending_reports(pool: &DbPool, limit: u32) -> ApiResult<Vec<AgentScriptRunReport>> {
    let rows = sqlx::query("SELECT payload FROM script_run_outbox ORDER BY created_at LIMIT ?")
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|r| {
            serde_json::from_str(r.try_get("payload")?)
                .map_err(|_| ApiError::internal("invalid script run outbox payload"))
        })
        .collect()
}

/// 确认上报；终态执行同时删除本地运行和 outbox。
pub async fn acknowledge(pool: &DbPool, run_ids: &[String]) -> ApiResult<()> {
    for id in run_ids {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM script_run_outbox WHERE run_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM script_runs WHERE run_id=? AND status IN ('succeeded','failed','timed_out','cancelled')")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}
/// 记录上报重试。
pub async fn mark_attempt(pool: &DbPool, run_ids: &[String]) -> ApiResult<()> {
    for id in run_ids {
        sqlx::query("UPDATE script_run_outbox SET attempts=attempts+1,last_attempt_at=?,updated_at=? WHERE run_id=?").bind(now()).bind(now()).bind(id).execute(pool).await?;
    }
    Ok(())
}

async fn enqueue_report(pool: &DbPool, run_id: &str) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    let report = build_report_tx(&mut tx, run_id).await?;
    let timestamp = now();
    sqlx::query("INSERT INTO script_run_outbox (run_id,payload,attempts,created_at,updated_at) VALUES (?,?,0,?,?) ON CONFLICT(run_id) DO UPDATE SET payload=excluded.payload,updated_at=excluded.updated_at").bind(run_id).bind(serde_json::to_string(&report).map_err(|_|ApiError::internal("failed to encode script run report"))?).bind(&timestamp).bind(&timestamp).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn build_report_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    run_id: &str,
) -> ApiResult<AgentScriptRunReport> {
    let row: ScriptRunRow = sqlx::query_as("SELECT * FROM script_runs WHERE run_id=?")
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(AgentScriptRunReport {
        run_id: row.run_id,
        script_id: row.script_id,
        script_revision: row.script_revision,
        source_sha256: row.source_sha256,
        status: status_from_text(&row.status)?,
        phase: row.phase,
        started_at: row.started_at,
        finished_at: row.finished_at,
        exit_code: row.exit_code,
        error_code: row.error_code,
        error_summary: row.error_summary,
    })
}

fn status_text(v: ScriptRunStatus) -> &'static str {
    match v {
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
fn status_from_text(v: &str) -> ApiResult<ScriptRunStatus> {
    match v {
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
fn is_unique(e: &sqlx::Error) -> bool {
    matches!(e,sqlx::Error::Database(db) if db.is_unique_violation())
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn req(id: &str) -> AgentStartScriptRunRequest {
        let content = "echo ok".to_string();
        AgentStartScriptRunRequest {
            run_id: id.into(),
            script_id: "script-1".into(),
            script_name: "Demo".into(),
            script_revision: 1,
            source_sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
            source_content: content,
            timeout_seconds: 30,
            ownership_kind: vust_contracts::scripts::ScriptOwnershipKind::Custom,
        }
    }
    #[tokio::test]
    async fn run_is_idempotent_and_outbox_is_atomic() {
        let pool = crate::test_support::setup_test_db().await;
        let (_, created) = create(&pool, &req("run-1")).await.unwrap();
        assert!(created);
        let (_, created) = create(&pool, &req("run-1")).await.unwrap();
        assert!(!created);
        finish(
            &pool,
            "run-1",
            ScriptRunStatus::Succeeded,
            Some(0),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(pending_reports(&pool, 10).await.unwrap().len(), 1);
        acknowledge(&pool, &["run-1".into()]).await.unwrap();
        assert!(get(&pool, "run-1").await.unwrap().is_none());
        assert!(pending_reports(&pool, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn repeated_run_id_rejects_a_different_snapshot() {
        let pool = crate::test_support::setup_test_db().await;
        create(&pool, &req("run-1")).await.unwrap();
        let mut changed = req("run-1");
        changed.script_revision = 2;
        assert_eq!(
            create(&pool, &changed).await.unwrap_err().code,
            ErrorCode::ScriptRunConflict
        );
    }

    #[tokio::test]
    async fn terminal_run_attaches_once_and_never_persists_transcript() {
        let pool = crate::test_support::setup_test_db().await;
        let request = req("terminal-run");
        create(&pool, &request).await.unwrap();
        mark_awaiting_connection(&pool, &request.run_id)
            .await
            .unwrap();
        assert!(attach_terminal(&pool, &request.run_id).await.is_ok());
        assert!(attach_terminal(&pool, &request.run_id).await.is_err());
        claim_running(&pool, &request.run_id).await.unwrap();
        finish(
            &pool,
            &request.run_id,
            ScriptRunStatus::Succeeded,
            Some(0),
            None,
            None,
        )
        .await
        .unwrap();
        let report = pending_reports(&pool, 10).await.unwrap().pop().unwrap();
        assert_eq!(report.run_id, request.run_id);
    }

    #[tokio::test]
    async fn cancelling_an_unattached_terminal_immediately_finishes_it() {
        let pool = crate::test_support::setup_test_db().await;
        let request = req("unattached-run");
        create(&pool, &request).await.unwrap();
        mark_awaiting_connection(&pool, &request.run_id)
            .await
            .unwrap();

        let cancelled = request_cancel(&pool, &request.run_id).await.unwrap();

        assert_eq!(cancelled.status, "cancelled");
        let report = pending_reports(&pool, 10).await.unwrap().pop().unwrap();
        assert_eq!(report.status, ScriptRunStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancelled_attached_terminal_cannot_claim_process_start() {
        let pool = crate::test_support::setup_test_db().await;
        let request = req("cancelled-before-start");
        create(&pool, &request).await.unwrap();
        mark_awaiting_connection(&pool, &request.run_id)
            .await
            .unwrap();
        attach_terminal(&pool, &request.run_id).await.unwrap();
        request_cancel(&pool, &request.run_id).await.unwrap();

        assert!(claim_running(&pool, &request.run_id).await.is_err());
    }
}
