-- 持久化套件安装任务，支持窗口关闭和 Master 重启后的进度恢复。
CREATE TABLE IF NOT EXISTS suite_install_tasks (
    task_id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    suite_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    progress_percent INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    current_step TEXT NOT NULL,
    current_image TEXT,
    is_finished INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    recovery_started_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finished_at TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_suite_install_tasks_active_suite_node
    ON suite_install_tasks (suite_id, node_id)
    WHERE is_finished = 0;

CREATE INDEX IF NOT EXISTS idx_suite_install_tasks_node_status
    ON suite_install_tasks (node_id, is_finished, updated_at DESC);
