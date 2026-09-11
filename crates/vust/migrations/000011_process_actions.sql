-- Master 持久化进程信号幂等事实；进程与投递事实仍由目标 Agent 校验。

CREATE TABLE process_action_requests (
    idempotency_key TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    actor_name TEXT NOT NULL,
    session_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    process_id TEXT NOT NULL,
    signal TEXT NOT NULL CHECK (signal IN ('term', 'kill')),
    status TEXT NOT NULL CHECK (
        status IN ('submitting', 'delivered', 'failed', 'outcome_unknown')
    ),
    pid INTEGER,
    process_name TEXT,
    delivered_at TEXT,
    error_code TEXT,
    error_summary TEXT,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_process_action_requests_target
    ON process_action_requests (node_id, process_id, created_at DESC);
