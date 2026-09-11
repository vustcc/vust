-- 脚本运行：保存 Master 下发的不可变脚本快照、生命周期和取消请求。
CREATE TABLE script_runs (
    run_id TEXT PRIMARY KEY,
    script_id TEXT NOT NULL,
    script_name TEXT NOT NULL,
    script_revision INTEGER NOT NULL CHECK(script_revision > 0),
    source_sha256 TEXT NOT NULL CHECK(length(source_sha256) = 64),
    source_content TEXT NOT NULL CHECK(length(CAST(source_content AS BLOB)) BETWEEN 1 AND 262144),
    timeout_seconds INTEGER NOT NULL CHECK(timeout_seconds BETWEEN 1 AND 86400),
    status TEXT NOT NULL CHECK(status IN ('queued', 'starting', 'running', 'cancelling', 'succeeded', 'failed', 'timed_out', 'cancelled')),
    phase TEXT,
    queued_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    exit_code INTEGER,
    error_code TEXT,
    error_summary TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0, 1)),
    attached_at TEXT,
    overlap_guard TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_agent_script_runs_overlap_guard
    ON script_runs(overlap_guard)
    WHERE overlap_guard IS NOT NULL AND status IN ('queued', 'starting', 'running', 'cancelling');
CREATE INDEX idx_agent_script_runs_active
    ON script_runs(status, updated_at)
    WHERE status IN ('queued', 'starting', 'running', 'cancelling');

-- 脚本运行上报箱：在 Master 确认前可靠保留最新运行事实。
CREATE TABLE script_run_outbox (
    run_id TEXT PRIMARY KEY,
    payload TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
    last_attempt_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(run_id) REFERENCES script_runs(run_id) ON DELETE CASCADE
);
