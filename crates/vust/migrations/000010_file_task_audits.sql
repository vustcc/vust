-- Master 只保存文件任务审计协调上下文，执行进度事实仍由目标 Agent 持有。

CREATE TABLE file_task_audits (
    task_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    actor_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('copy', 'move', 'remove')),
    high_impact INTEGER NOT NULL CHECK (high_impact IN (0, 1)),
    last_status TEXT NOT NULL DEFAULT 'queued',
    terminal_logged INTEGER NOT NULL DEFAULT 0 CHECK (terminal_logged IN (0, 1)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    finished_at INTEGER,
    PRIMARY KEY (node_id, task_id)
);

CREATE INDEX idx_file_task_audits_pending
    ON file_task_audits (terminal_logged, created_at);

CREATE TABLE file_transfer_audits (
    transfer_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    actor_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('upload', 'download')),
    target_path TEXT NOT NULL,
    last_status TEXT NOT NULL DEFAULT 'created',
    terminal_logged INTEGER NOT NULL DEFAULT 0 CHECK (terminal_logged IN (0, 1)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    finished_at INTEGER,
    PRIMARY KEY (node_id, transfer_id)
);

CREATE INDEX idx_file_transfer_audits_pending
    ON file_transfer_audits (terminal_logged, created_at);
