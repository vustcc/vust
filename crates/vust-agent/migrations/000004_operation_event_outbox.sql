-- Agent 操作审计持久 outbox；最终结果由 Master 统一查询。
CREATE TABLE operation_event_outbox (
    event_id TEXT PRIMARY KEY,
    event_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    delivered_at TEXT
);

CREATE INDEX idx_operation_event_outbox_pending
    ON operation_event_outbox (next_attempt_at, created_at, event_id)
    WHERE delivered_at IS NULL;

-- 套件代理请求的可信用户上下文；仅允许匹配的套件实例在有效期内引用。
CREATE TABLE suite_operation_contexts (
    context_id TEXT PRIMARY KEY,
    suite_id TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    actor_user_id INTEGER NOT NULL,
    actor_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX idx_suite_operation_contexts_expiry
    ON suite_operation_contexts (expires_at);
