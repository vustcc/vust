-- 文件操作任务和可恢复传输；不保存文件内容、Token 或完整请求体。

CREATE TABLE file_operation_tasks (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    operation TEXT NOT NULL CHECK (operation IN ('copy', 'move', 'remove')),
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'running', 'cancelling', 'succeeded', 'failed', 'cancelled')
    ),
    stage TEXT NOT NULL CHECK (
        stage IN (
            'validating', 'preparing', 'copying', 'moving', 'deleting',
            'rollingBack', 'cleaningUp', 'completed', 'failed', 'cancelled', 'interrupted'
        )
    ),
    progress_percent INTEGER NOT NULL DEFAULT 0 CHECK (
        progress_percent >= 0 AND progress_percent <= 100
    ),
    total_item_count INTEGER NOT NULL,
    completed_item_count INTEGER NOT NULL DEFAULT 0,
    failed_item_count INTEGER NOT NULL DEFAULT 0,
    total_bytes INTEGER NOT NULL DEFAULT 0,
    processed_bytes INTEGER NOT NULL DEFAULT 0,
    target_directory TEXT,
    recursive INTEGER NOT NULL CHECK (recursive IN (0, 1)),
    actor_name TEXT NOT NULL,
    client_ip TEXT,
    trace_id TEXT,
    error_summary TEXT,
    cleanup_warning TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    started_at INTEGER,
    finished_at INTEGER
);

CREATE INDEX idx_file_operation_tasks_active_created
    ON file_operation_tasks (created_at)
    WHERE status IN ('queued', 'running', 'cancelling');

CREATE TABLE file_operation_task_items (
    task_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    source_path TEXT NOT NULL,
    expected_revision TEXT,
    target_path TEXT,
    planned_bytes INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')
    ),
    error_code TEXT,
    error_summary TEXT,
    PRIMARY KEY (task_id, ordinal),
    FOREIGN KEY (task_id) REFERENCES file_operation_tasks(id) ON DELETE CASCADE
);

CREATE TABLE file_transfers (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('upload', 'download')),
    status TEXT NOT NULL CHECK (
        status IN ('created', 'receiving', 'ready', 'streaming', 'completed', 'failed', 'cancelled', 'expired')
    ),
    path TEXT NOT NULL,
    temporary_path TEXT,
    size_bytes INTEGER NOT NULL,
    transferred_bytes INTEGER NOT NULL DEFAULT 0,
    revision TEXT,
    sha256 TEXT,
    actor_name TEXT NOT NULL,
    client_ip TEXT,
    trace_id TEXT,
    error_summary TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL
);

CREATE INDEX idx_file_transfers_active_expires
    ON file_transfers (expires_at)
    WHERE status IN ('created', 'receiving', 'ready', 'streaming');
