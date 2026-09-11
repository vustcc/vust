//! Agent 数据库维护：统一执行终态历史清理、outbox 积压检查与增量空间回收。

use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use std::time::Duration as StdDuration;
use tracing::{info, warn};

use crate::state::DbPool;

const HISTORY_RETENTION_DAYS: i64 = 30;
const DELIVERED_OUTBOX_RETENTION_DAYS: i64 = 7;
const LARGE_PARENT_BATCH_SIZE: i64 = 100;
const DEFAULT_BATCH_SIZE: i64 = 1_000;
const MAX_BATCHES_PER_TABLE: usize = 10;
const MAX_INCREMENTAL_VACUUM_PAGES: i64 = 1_024;
const OUTBOX_WARNING_AGE_HOURS: i64 = 24;
const OPERATION_OUTBOX_WARNING_COUNT: i64 = 1_000;
const RUN_OUTBOX_WARNING_COUNT: i64 = 100;

const TASK_RUN_CLEANUP: &str = r#"
WITH ranked AS (
    SELECT run_id,
           COALESCE(finished_at, updated_at) AS finished_at,
           ROW_NUMBER() OVER (
               PARTITION BY task_id
               ORDER BY queued_at DESC, run_id DESC
           ) AS retained_rank
    FROM task_runs
    WHERE status IN ('succeeded', 'failed', 'timed_out', 'cancelled')
)
DELETE FROM task_runs
WHERE run_id IN (
    SELECT ranked.run_id
    FROM ranked
    WHERE NOT EXISTS (
        SELECT 1 FROM task_run_outbox outbox
        WHERE outbox.run_id = ranked.run_id
    )
      AND (ranked.finished_at < ?1 OR ranked.retained_rank > 100)
    ORDER BY ranked.finished_at, ranked.run_id
    LIMIT ?2
)"#;

const FILE_TASK_CLEANUP: &str = r#"
DELETE FROM file_operation_tasks
WHERE id IN (
    SELECT id
    FROM file_operation_tasks
    WHERE status IN ('succeeded', 'failed', 'cancelled')
      AND COALESCE(finished_at, created_at) < ?1
    ORDER BY COALESCE(finished_at, created_at), id
    LIMIT ?2
)"#;

const FILE_TRANSFER_CLEANUP: &str = r#"
DELETE FROM file_transfers
WHERE id IN (
    SELECT id
    FROM file_transfers
    WHERE status IN ('completed', 'failed', 'cancelled', 'expired')
      AND updated_at < ?1
    ORDER BY updated_at, id
    LIMIT ?2
)"#;

const DISK_OPERATION_CLEANUP: &str = r#"
DELETE FROM disk_operations
WHERE operation_id IN (
    SELECT operation_id
    FROM disk_operations
    WHERE status IN ('succeeded', 'partial', 'failed', 'canceled')
      AND COALESCE(finished_at, updated_at) < ?1
    ORDER BY COALESCE(finished_at, updated_at), operation_id
    LIMIT ?2
)"#;

const PROCESS_SIGNAL_CLEANUP: &str = r#"
DELETE FROM process_signal_operations
WHERE idempotency_key IN (
    SELECT idempotency_key
    FROM process_signal_operations
    WHERE status IN ('delivered', 'failed', 'outcome_unknown')
      AND updated_at < ?1
    ORDER BY updated_at, idempotency_key
    LIMIT ?2
)"#;

const OPERATION_RECEIPT_CLEANUP: &str = r#"
DELETE FROM scheduled_task_operation_receipts
WHERE operation_id IN (
    SELECT operation_id
    FROM scheduled_task_operation_receipts
    WHERE completed_at < ?1
    ORDER BY completed_at, operation_id
    LIMIT ?2
)"#;

const DELIVERED_OUTBOX_CLEANUP: &str = r#"
DELETE FROM operation_event_outbox
WHERE event_id IN (
    SELECT event_id
    FROM operation_event_outbox
    WHERE delivered_at IS NOT NULL
      AND delivered_at < ?1
    ORDER BY delivered_at, event_id
    LIMIT ?2
)"#;

/// 单次数据库维护结果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    /// 本轮删除的终态历史记录总数，不包含级联删除的子记录。
    pub deleted_rows: u64,
    /// 本轮实际回收的 SQLite 空闲页数。
    pub reclaimed_pages: u64,
}

/// outbox 当前积压状态。
#[derive(Debug, PartialEq, Eq)]
struct OutboxBacklog {
    kind: &'static str,
    count: i64,
    oldest_created_at: Option<String>,
    warning_count: i64,
}

impl OutboxBacklog {
    /// 判断积压数量或最旧记录年龄是否超过告警阈值。
    fn requires_warning(&self, now: DateTime<Utc>) -> bool {
        self.count > self.warning_count
            || self
                .oldest_created_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .is_some_and(|value| {
                    value.with_timezone(&Utc) < now - Duration::hours(OUTBOX_WARNING_AGE_HOURS)
                })
    }
}

/// 启动 Agent 数据库维护任务，启动后立即执行并每 24 小时重复。
pub fn spawn_worker(pool: DbPool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(StdDuration::from_secs(24 * 60 * 60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match run_once(&pool).await {
                Ok(report) if report.deleted_rows > 0 || report.reclaimed_pages > 0 => {
                    info!(
                        deleted_rows = report.deleted_rows,
                        reclaimed_pages = report.reclaimed_pages,
                        "Agent database maintenance completed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(%error, "Agent database maintenance failed");
                }
            }
        }
    });
}

/// 使用当前时间执行一轮数据库维护。
pub async fn run_once(pool: &DbPool) -> Result<MaintenanceReport, sqlx::Error> {
    run_at(pool, Utc::now()).await
}

/// 使用指定时间执行一轮可测试的数据库维护。
async fn run_at(pool: &DbPool, now: DateTime<Utc>) -> Result<MaintenanceReport, sqlx::Error> {
    let history_cutoff = (now - Duration::days(HISTORY_RETENTION_DAYS)).to_rfc3339();
    let history_cutoff_unix = (now - Duration::days(HISTORY_RETENTION_DAYS)).timestamp();
    let delivered_cutoff = (now - Duration::days(DELIVERED_OUTBOX_RETENTION_DAYS)).to_rfc3339();

    let mut deleted_rows = 0;
    deleted_rows +=
        delete_text_batches(pool, TASK_RUN_CLEANUP, &history_cutoff, DEFAULT_BATCH_SIZE).await?;
    deleted_rows += delete_integer_batches(
        pool,
        FILE_TASK_CLEANUP,
        history_cutoff_unix,
        LARGE_PARENT_BATCH_SIZE,
    )
    .await?;
    deleted_rows += delete_integer_batches(
        pool,
        FILE_TRANSFER_CLEANUP,
        history_cutoff_unix,
        DEFAULT_BATCH_SIZE,
    )
    .await?;
    deleted_rows += delete_text_batches(
        pool,
        DISK_OPERATION_CLEANUP,
        &history_cutoff,
        DEFAULT_BATCH_SIZE,
    )
    .await?;
    deleted_rows += delete_text_batches(
        pool,
        PROCESS_SIGNAL_CLEANUP,
        &history_cutoff,
        DEFAULT_BATCH_SIZE,
    )
    .await?;
    deleted_rows += delete_text_batches(
        pool,
        OPERATION_RECEIPT_CLEANUP,
        &history_cutoff,
        DEFAULT_BATCH_SIZE,
    )
    .await?;
    deleted_rows += delete_text_batches(
        pool,
        DELIVERED_OUTBOX_CLEANUP,
        &delivered_cutoff,
        DEFAULT_BATCH_SIZE,
    )
    .await?;

    for backlog in inspect_outboxes(pool).await? {
        if backlog.requires_warning(now) {
            warn!(
                outbox = backlog.kind,
                pending_count = backlog.count,
                oldest_created_at = backlog.oldest_created_at.as_deref().unwrap_or("unknown"),
                "Agent durable outbox backlog exceeds its warning threshold"
            );
        }
    }

    let reclaimed_pages = reclaim_unused_pages(pool).await?;
    Ok(MaintenanceReport {
        deleted_rows,
        reclaimed_pages,
    })
}

/// 对 RFC 3339 时间字段执行有界分批删除。
async fn delete_text_batches(
    pool: &DbPool,
    statement: &str,
    cutoff: &str,
    batch_size: i64,
) -> Result<u64, sqlx::Error> {
    let mut deleted = 0;
    for _ in 0..MAX_BATCHES_PER_TABLE {
        let affected = sqlx::query(statement)
            .bind(cutoff)
            .bind(batch_size)
            .execute(pool)
            .await?
            .rows_affected();
        deleted += affected;
        if affected < batch_size as u64 {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(deleted)
}

/// 对 Unix 秒时间字段执行有界分批删除。
async fn delete_integer_batches(
    pool: &DbPool,
    statement: &str,
    cutoff: i64,
    batch_size: i64,
) -> Result<u64, sqlx::Error> {
    let mut deleted = 0;
    for _ in 0..MAX_BATCHES_PER_TABLE {
        let affected = sqlx::query(statement)
            .bind(cutoff)
            .bind(batch_size)
            .execute(pool)
            .await?
            .rows_affected();
        deleted += affected;
        if affected < batch_size as u64 {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(deleted)
}

/// 读取三类可靠 outbox 的待确认数量和最早创建时间。
async fn inspect_outboxes(pool: &DbPool) -> Result<Vec<OutboxBacklog>, sqlx::Error> {
    let definitions = [
        (
            "operation_event_outbox",
            "SELECT COUNT(*), MIN(created_at) FROM operation_event_outbox WHERE delivered_at IS NULL",
            OPERATION_OUTBOX_WARNING_COUNT,
        ),
        (
            "task_run_outbox",
            "SELECT COUNT(*), MIN(created_at) FROM task_run_outbox",
            RUN_OUTBOX_WARNING_COUNT,
        ),
        (
            "script_run_outbox",
            "SELECT COUNT(*), MIN(created_at) FROM script_run_outbox",
            RUN_OUTBOX_WARNING_COUNT,
        ),
    ];
    let mut backlogs = Vec::with_capacity(definitions.len());
    for (kind, statement, warning_count) in definitions {
        let row = sqlx::query(statement).fetch_one(pool).await?;
        backlogs.push(OutboxBacklog {
            kind,
            count: row.try_get(0)?,
            oldest_created_at: row.try_get(1)?,
            warning_count,
        });
    }
    Ok(backlogs)
}

/// 在增量 vacuum 模式下有界回收数据库尾部空闲页。
async fn reclaim_unused_pages(pool: &DbPool) -> Result<u64, sqlx::Error> {
    let auto_vacuum = sqlx::query_scalar::<_, i64>("PRAGMA auto_vacuum")
        .fetch_one(pool)
        .await?;
    if auto_vacuum != 2 {
        warn!(
            auto_vacuum,
            "Agent database is not using incremental auto-vacuum; online page reclamation skipped"
        );
        return Ok(0);
    }
    let free_pages = sqlx::query_scalar::<_, i64>("PRAGMA freelist_count")
        .fetch_one(pool)
        .await?;
    let pages_to_reclaim = free_pages.clamp(0, MAX_INCREMENTAL_VACUUM_PAGES);
    if pages_to_reclaim == 0 {
        return Ok(0);
    }
    sqlx::query(&format!("PRAGMA incremental_vacuum({pages_to_reclaim})"))
        .execute(pool)
        .await?;
    let remaining_pages = sqlx::query_scalar::<_, i64>("PRAGMA freelist_count")
        .fetch_one(pool)
        .await?;
    Ok(free_pages.saturating_sub(remaining_pages) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::setup_test_db;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use uuid::Uuid;

    #[test]
    fn outbox_warning_uses_count_and_age_thresholds() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            OutboxBacklog {
                kind: "test",
                count: 101,
                oldest_created_at: Some(now.to_rfc3339()),
                warning_count: 100,
            }
            .requires_warning(now)
        );
        assert!(
            OutboxBacklog {
                kind: "test",
                count: 1,
                oldest_created_at: Some((now - Duration::hours(25)).to_rfc3339()),
                warning_count: 100,
            }
            .requires_warning(now)
        );
        assert!(
            !OutboxBacklog {
                kind: "test",
                count: 100,
                oldest_created_at: Some((now - Duration::hours(24)).to_rfc3339()),
                warning_count: 100,
            }
            .requires_warning(now)
        );
    }

    #[tokio::test]
    async fn pending_operation_outbox_is_never_deleted() {
        let pool = setup_test_db().await;
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO operation_event_outbox \
             (event_id,event_json,created_at,next_attempt_at,delivered_at) \
             VALUES ('pending','{}',?1,?1,NULL),('delivered','{}',?1,?1,?2)",
        )
        .bind((now - Duration::days(60)).to_rfc3339())
        .bind((now - Duration::days(8)).to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        run_at(&pool, now).await.unwrap();

        let ids = sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM operation_event_outbox ORDER BY event_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(ids, vec!["pending"]);
    }

    #[tokio::test]
    async fn cleanup_cascades_children_and_protects_outboxes() {
        let pool = setup_test_db().await;
        let now = Utc::now();
        let old = (now - Duration::days(31)).to_rfc3339();
        insert_task_definition(&pool, "task-1", &old).await;
        insert_task_run(&pool, "old-task-run", "task-1", "succeeded", &old).await;
        insert_task_run(&pool, "pending-task-run", "task-1", "failed", &old).await;
        sqlx::query(
            "INSERT INTO task_run_outputs (run_id,content,size_bytes,truncated,updated_at) \
             VALUES ('old-task-run','x',1,0,?1),('pending-task-run','x',1,0,?1)",
        )
        .bind(&old)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_run_outbox \
             (run_id,payload,created_at,updated_at) VALUES ('pending-task-run','{}',?1,?1)",
        )
        .bind(&old)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_operation_tasks \
             (id,node_id,idempotency_key,operation,status,stage,total_item_count,recursive,actor_name,created_at,finished_at) \
             VALUES ('old-file','node','file-key-old','remove','succeeded','completed',1,0,'admin',0,0), \
                    ('active-file','node','file-key-active','remove','running','deleting',1,0,'admin',0,NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO file_operation_task_items \
             (task_id,ordinal,source_path,status) VALUES \
             ('old-file',0,'/tmp/old','succeeded'),('active-file',0,'/tmp/active','running')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_at(&pool, now).await.unwrap();

        let file_ids =
            sqlx::query_scalar::<_, String>("SELECT id FROM file_operation_tasks ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(file_ids, vec!["active-file"]);
        let item_ids: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_operation_task_items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(item_ids, 1);
        let task_run_ids =
            sqlx::query_scalar::<_, String>("SELECT run_id FROM task_runs ORDER BY run_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(task_run_ids, vec!["pending-task-run"]);
        let task_outputs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_run_outputs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(task_outputs, 1);
    }

    #[tokio::test]
    async fn keeps_only_one_hundred_scheduled_task_runs_per_definition() {
        let pool = setup_test_db().await;
        let now = Utc::now();
        let recent = (now - Duration::days(1)).to_rfc3339();
        insert_task_definition(&pool, "task-limit", &recent).await;
        for sequence in 0..101 {
            insert_task_run(
                &pool,
                &format!("task-run-{sequence:03}"),
                "task-limit",
                "succeeded",
                &recent,
            )
            .await;
        }

        run_at(&pool, now).await.unwrap();

        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_runs WHERE task_id='task-limit'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, 100);
    }

    #[tokio::test]
    async fn terminal_history_respects_the_thirty_day_boundary() {
        let pool = setup_test_db().await;
        let now = DateTime::parse_from_rfc3339("2026-07-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let old = (now - Duration::days(31)).to_rfc3339();
        let boundary = (now - Duration::days(30)).to_rfc3339();
        let old_unix = (now - Duration::days(31)).timestamp();
        let boundary_unix = (now - Duration::days(30)).timestamp();

        sqlx::query(
            "INSERT INTO file_transfers \
             (id,node_id,direction,status,path,size_bytes,actor_name,created_at,updated_at,expires_at) \
             VALUES ('old-transfer','node','download','completed','/tmp/old',1,'admin',?1,?1,?1), \
                    ('boundary-transfer','node','download','completed','/tmp/new',1,'admin',?2,?2,?2), \
                    ('active-transfer','node','upload','receiving','/tmp/active',1,'admin',?1,?1,?1)",
        )
        .bind(old_unix)
        .bind(boundary_unix)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO disk_operations \
             (operation_id,node_id,kind,request_json,target_lock_key,status,created_at,updated_at,finished_at) \
             VALUES ('old-disk','node','formatPartition','{}','disk-old','succeeded',?1,?1,?1), \
                    ('boundary-disk','node','formatPartition','{}','disk-boundary','failed',?2,?2,?2), \
                    ('active-disk','node','formatPartition','{}','disk-active','queued',?1,?1,NULL)",
        )
        .bind(&old)
        .bind(&boundary)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO process_signal_operations \
             (idempotency_key,process_id,signal,status,actor_name,client_ip,trace_id,created_at,updated_at) \
             VALUES ('old-signal','process-old','term','delivered','admin','127.0.0.1','trace-old',?1,?1), \
                    ('boundary-signal','process-new','term','failed','admin','127.0.0.1','trace-new',?2,?2), \
                    ('active-signal','process-active','term','submitting','admin','127.0.0.1','trace-active',?1,?1)",
        )
        .bind(&old)
        .bind(&boundary)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO scheduled_task_operation_receipts \
             (operation_id,task_id,operation_kind,completed_at) \
             VALUES ('old-receipt','task','update',?1),('boundary-receipt','task','update',?2)",
        )
        .bind(&old)
        .bind(&boundary)
        .execute(&pool)
        .await
        .unwrap();

        run_at(&pool, now).await.unwrap();

        let transfers =
            sqlx::query_scalar::<_, String>("SELECT id FROM file_transfers ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(transfers, vec!["active-transfer", "boundary-transfer"]);
        let disks =
            sqlx::query_scalar::<_, String>("SELECT operation_id FROM disk_operations ORDER BY 1")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(disks, vec!["active-disk", "boundary-disk"]);
        let signals = sqlx::query_scalar::<_, String>(
            "SELECT idempotency_key FROM process_signal_operations ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(signals, vec!["active-signal", "boundary-signal"]);
        let receipts = sqlx::query_scalar::<_, String>(
            "SELECT operation_id FROM scheduled_task_operation_receipts ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(receipts, vec!["boundary-receipt"]);
    }

    #[tokio::test]
    async fn each_table_stops_after_ten_batches() {
        let pool = setup_test_db().await;
        sqlx::query(
            "CREATE TABLE maintenance_batch_fixture (\
             id INTEGER PRIMARY KEY, created_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for first_id in (0..1_005).step_by(100) {
            let last_id = (first_id + 100).min(1_005);
            let mut builder =
                sqlx::QueryBuilder::new("INSERT INTO maintenance_batch_fixture (id,created_at) ");
            builder.push_values(first_id..last_id, |mut row, id| {
                row.push_bind(id).push_bind(0_i64);
            });
            builder.build().execute(&pool).await.unwrap();
        }

        let deleted = delete_integer_batches(
            &pool,
            "DELETE FROM maintenance_batch_fixture WHERE id IN (\
             SELECT id FROM maintenance_batch_fixture WHERE created_at < ?1 LIMIT ?2)",
            1,
            100,
        )
        .await
        .unwrap();

        assert_eq!(deleted, 1_000);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM maintenance_batch_fixture")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 5);
    }

    #[tokio::test]
    async fn incremental_vacuum_is_bounded_on_file_database() {
        let path =
            std::env::temp_dir().join(format!("vust-agent-maintenance-{}.db", Uuid::new_v4()));
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("PRAGMA auto_vacuum=INCREMENTAL")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("VACUUM").execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE payloads (id INTEGER PRIMARY KEY, payload BLOB NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        for _ in 0..2_000 {
            sqlx::query("INSERT INTO payloads(payload) VALUES (zeroblob(4096))")
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM payloads")
            .execute(&pool)
            .await
            .unwrap();
        let before: i64 = sqlx::query_scalar("PRAGMA freelist_count")
            .fetch_one(&pool)
            .await
            .unwrap();

        let reclaimed = reclaim_unused_pages(&pool).await.unwrap();
        let after: i64 = sqlx::query_scalar("PRAGMA freelist_count")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert!(before > 0);
        assert_eq!(reclaimed, before.saturating_sub(after) as u64);
        assert!(reclaimed <= MAX_INCREMENTAL_VACUUM_PAGES as u64);
        pool.close().await;
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn migrations_create_only_the_expected_maintenance_indexes() {
        let pool = setup_test_db().await;
        let indexes = sqlx::query_as::<_, (String, String)>(
            "SELECT tbl_name,name FROM sqlite_master WHERE type='index'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let names = indexes
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>();
        assert!(!names.contains(&"idx_system_monitoring_samples_sampled_at"));
        assert!(!names.contains(&"idx_file_operation_task_items_source"));
        assert!(!names.contains(&"idx_process_signal_operations_process"));

        for (query, expected_index) in [
            (
                "SELECT event_id,event_json FROM operation_event_outbox \
                 WHERE delivered_at IS NULL AND next_attempt_at <= '2026-07-01T00:00:00Z' \
                 ORDER BY created_at,event_id LIMIT 100",
                "idx_operation_event_outbox_pending",
            ),
            (
                "SELECT id FROM file_operation_tasks \
                 WHERE status IN ('queued','running','cancelling') ORDER BY created_at",
                "idx_file_operation_tasks_active_created",
            ),
            (
                "SELECT id FROM file_transfers WHERE expires_at <= unixepoch() \
                 AND status IN ('created','receiving','ready','streaming')",
                "idx_file_transfers_active_expires",
            ),
            (
                "SELECT operation_id,status FROM disk_operations \
                 WHERE status IN ('queued','validating','preparing','applying','verifying','canceling')",
                "idx_disk_operations_active",
            ),
        ] {
            let details = explain_query_plan(&pool, query).await;
            assert!(
                details.iter().any(|detail| detail.contains(expected_index)),
                "{query} 未使用 {expected_index}：{details:?}"
            );
        }
    }

    /// 返回 SQLite 查询计划的详情列。
    async fn explain_query_plan(pool: &DbPool, query: &str) -> Vec<String> {
        sqlx::query(&format!("EXPLAIN QUERY PLAN {query}"))
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get("detail"))
            .collect()
    }

    /// 插入最小计划任务定义夹具。
    async fn insert_task_definition(pool: &DbPool, task_id: &str, timestamp: &str) {
        sqlx::query(
            "INSERT INTO scheduled_tasks \
             (task_id,revision,name,command,cron_expr,time_zone,desired_state,timeout_seconds,\
              prevent_overlap,ownership_kind,created_at,updated_at) \
             VALUES (?1,1,'task','echo test','* * * * *','UTC','enabled',60,0,'custom',?2,?2)",
        )
        .bind(task_id)
        .bind(timestamp)
        .execute(pool)
        .await
        .unwrap();
    }

    /// 插入最小计划任务运行夹具。
    async fn insert_task_run(
        pool: &DbPool,
        run_id: &str,
        task_id: &str,
        status: &str,
        timestamp: &str,
    ) {
        sqlx::query(
            "INSERT INTO task_runs \
             (run_id,task_id,trigger_source,status,queued_at,finished_at,created_at,updated_at) \
             VALUES (?1,?2,'schedule',?3,?4,?5,?4,?4)",
        )
        .bind(run_id)
        .bind(task_id)
        .bind(status)
        .bind(timestamp)
        .bind(
            if matches!(status, "succeeded" | "failed" | "timed_out" | "cancelled") {
                Some(timestamp)
            } else {
                None
            },
        )
        .execute(pool)
        .await
        .unwrap();
    }
}
