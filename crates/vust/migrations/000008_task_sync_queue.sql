-- 计划任务后台操作：保存部署、更新、启停、删除、迁移和批量操作的可恢复状态。
CREATE TABLE scheduled_task_operations (
    operation_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('deploy', 'update', 'state_change', 'remove', 'migrate', 'batch')),
    status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'cancelling', 'succeeded', 'partial', 'failed', 'cancelled')),
    phase TEXT,
    completed_steps INTEGER NOT NULL DEFAULT 0 CHECK(completed_steps >= 0),
    total_steps INTEGER NOT NULL CHECK(total_steps > 0),
    source_node_id TEXT,
    target_node_id TEXT,
    error_code TEXT,
    error_summary TEXT,
    warning_summary TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0, 1)),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
    last_attempt_at TEXT,
    actor_user_id INTEGER NOT NULL,
    actor_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE UNIQUE INDEX idx_scheduled_task_operations_active
    ON scheduled_task_operations(task_id)
    WHERE status IN ('queued', 'running', 'cancelling');
CREATE INDEX idx_scheduled_task_operations_status
    ON scheduled_task_operations(status, updated_at, operation_id);

-- 计划任务批次：保存批量启停、运行或删除请求的总体状态和可信操作人上下文。
CREATE TABLE scheduled_task_batches (
    batch_id TEXT PRIMARY KEY,
    action TEXT NOT NULL CHECK(action IN ('enable', 'disable', 'run', 'remove')),
    status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'cancelling', 'succeeded', 'partial', 'failed', 'cancelled')),
    actor_user_id INTEGER NOT NULL,
    actor_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 计划任务批次明细：关联每个任务对应的运行、后台操作或安全错误摘要。
CREATE TABLE scheduled_task_batch_items (
    batch_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    run_id TEXT,
    operation_id TEXT,
    error_code TEXT,
    error_summary TEXT,
    PRIMARY KEY(batch_id, task_id),
    FOREIGN KEY(batch_id) REFERENCES scheduled_task_batches(batch_id) ON DELETE CASCADE
);
