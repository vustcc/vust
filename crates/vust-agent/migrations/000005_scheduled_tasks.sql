-- Agent 计划任务定义：保存 Master 下发的任务修订、调度规则和资源归属。
CREATE TABLE scheduled_tasks (
    task_id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK(revision > 0),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80),
    command TEXT NOT NULL CHECK(length(command) BETWEEN 1 AND 65536),
    cron_expr TEXT NOT NULL,
    time_zone TEXT NOT NULL,
    desired_state TEXT NOT NULL CHECK(desired_state IN ('enabled', 'disabled')),
    timeout_seconds INTEGER NOT NULL CHECK(timeout_seconds BETWEEN 1 AND 86400),
    prevent_overlap INTEGER NOT NULL CHECK(prevent_overlap IN (0, 1)),
    ownership_kind TEXT NOT NULL CHECK(ownership_kind IN ('custom', 'compose', 'suite', 'system')),
    owner_id TEXT,
    owner_name TEXT,
    manager_path TEXT,
    last_run_at TEXT,
    next_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_agent_scheduled_tasks_due
    ON scheduled_tasks(desired_state, next_run_at, task_id);

-- Agent 计划任务运行记录：保存本地进程生命周期、取消请求和防重入占用状态。
CREATE TABLE task_runs (
    run_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    trigger_source TEXT NOT NULL CHECK(trigger_source IN ('schedule', 'manual', 'batch')),
    status TEXT NOT NULL CHECK(status IN (
        'queued', 'starting', 'running', 'cancelling', 'succeeded', 'failed', 'timed_out', 'cancelled'
    )),
    phase TEXT,
    queued_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    exit_code INTEGER,
    error_code TEXT,
    error_summary TEXT,
    output_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK(output_size_bytes >= 0),
    output_truncated INTEGER NOT NULL DEFAULT 0 CHECK(output_truncated IN (0, 1)),
    overlap_guard TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(task_id) REFERENCES scheduled_tasks(task_id) ON DELETE CASCADE
);

CREATE INDEX idx_agent_task_runs_task
    ON task_runs(task_id, queued_at DESC, run_id DESC);
CREATE INDEX idx_agent_task_runs_active
    ON task_runs(status, updated_at)
    WHERE status IN ('queued', 'starting', 'running', 'cancelling');
CREATE UNIQUE INDEX idx_agent_task_runs_overlap_guard
    ON task_runs(overlap_guard)
    WHERE overlap_guard IS NOT NULL AND status IN ('queued', 'starting', 'running', 'cancelling');

-- Agent 计划任务运行输出：以固定上限保存 stdout 和 stderr 的合并输出。
CREATE TABLE task_run_outputs (
    run_id TEXT PRIMARY KEY,
    content BLOB NOT NULL,
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0 AND size_bytes <= 262144),
    truncated INTEGER NOT NULL CHECK(truncated IN (0, 1)),
    updated_at TEXT NOT NULL,
    FOREIGN KEY(run_id) REFERENCES task_runs(run_id) ON DELETE CASCADE
);

-- Agent 运行上报箱：持久化等待 Master 确认的幂等运行状态和结果。
CREATE TABLE task_run_outbox (
    run_id TEXT PRIMARY KEY,
    payload TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
    last_attempt_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(run_id) REFERENCES task_runs(run_id) ON DELETE CASCADE
);

-- Agent 操作回执：记录已处理的 Master operationId，保证重复下发安全幂等。
CREATE TABLE scheduled_task_operation_receipts (
    operation_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    operation_kind TEXT NOT NULL,
    completed_at TEXT NOT NULL
);
