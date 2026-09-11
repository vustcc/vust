-- 进程信号请求事实：用于幂等、终态查询与 Agent 重启后的不确定结果恢复。
CREATE TABLE process_signal_operations (
    idempotency_key TEXT PRIMARY KEY NOT NULL,
    process_id TEXT NOT NULL,
    pid INTEGER,
    process_name TEXT,
    signal TEXT NOT NULL CHECK (signal IN ('term', 'kill')),
    status TEXT NOT NULL CHECK (status IN ('submitting', 'delivered', 'failed', 'outcome_unknown')),
    delivered_at TEXT,
    error_code TEXT,
    error_summary TEXT,
    actor_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
