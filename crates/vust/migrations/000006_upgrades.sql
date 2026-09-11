-- 在线升级发布、计划、目标、事件和下载令牌。
CREATE TABLE IF NOT EXISTS upgrade_releases (
    release_id TEXT PRIMARY KEY,
    version TEXT NOT NULL UNIQUE,
    tag_name TEXT NOT NULL,
    channel TEXT NOT NULL CHECK (channel IN ('stable', 'prerelease')),
    source TEXT NOT NULL DEFAULT 'github',
    release_url TEXT NOT NULL,
    assets TEXT NOT NULL DEFAULT '[]',
    checksum_status TEXT NOT NULL CHECK (
        checksum_status IN ('unknown', 'missing', 'verified', 'failed')
    ) DEFAULT 'unknown',
    signature_status TEXT NOT NULL CHECK (
        signature_status IN ('unknown', 'missing', 'verified', 'failed')
    ) DEFAULT 'unknown',
    supported_suite_contract_versions TEXT NOT NULL,
    synced_at TEXT NOT NULL,
    published_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_upgrade_releases_channel_version
    ON upgrade_releases (channel, version);

CREATE TRIGGER IF NOT EXISTS set_upgrade_releases_updated_at
AFTER UPDATE ON upgrade_releases FOR EACH ROW
BEGIN
    UPDATE upgrade_releases
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE release_id = OLD.release_id;
END;

CREATE TABLE IF NOT EXISTS upgrade_plans (
    plan_id TEXT PRIMARY KEY,
    target_version TEXT NOT NULL,
    component TEXT NOT NULL CHECK (component IN ('controller', 'agent', 'cluster')),
    scope TEXT NOT NULL DEFAULT '{}',
    strategy TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL CHECK (
        status IN ('draft', 'running', 'paused', 'succeeded', 'failed', 'canceled')
    ) DEFAULT 'draft',
    requested_by_user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    requested_by TEXT NOT NULL,
    client_ip TEXT,
    trace_id TEXT,
    started_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_upgrade_plans_status_created_at
    ON upgrade_plans (status, created_at);

CREATE TRIGGER IF NOT EXISTS set_upgrade_plans_updated_at
AFTER UPDATE ON upgrade_plans FOR EACH ROW
BEGIN
    UPDATE upgrade_plans
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE plan_id = OLD.plan_id;
END;

CREATE TABLE IF NOT EXISTS upgrade_targets (
    target_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    target_type TEXT NOT NULL CHECK (target_type IN ('controller', 'agent')),
    node_id TEXT,
    current_version TEXT,
    target_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'pending',
            'deferred',
            'running',
            'succeeded',
            'failed',
            'rollbacked',
            'canceled'
        )
    ) DEFAULT 'pending',
    sub_status TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    error_detail TEXT,
    started_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (plan_id) REFERENCES upgrade_plans(plan_id) ON DELETE CASCADE,
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_upgrade_targets_plan_status
    ON upgrade_targets (plan_id, status);
CREATE INDEX IF NOT EXISTS idx_upgrade_targets_node_status
    ON upgrade_targets (node_id, status);

CREATE TRIGGER IF NOT EXISTS set_upgrade_targets_updated_at
AFTER UPDATE ON upgrade_targets FOR EACH ROW
BEGIN
    UPDATE upgrade_targets
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE target_id = OLD.target_id;
END;

CREATE TABLE IF NOT EXISTS upgrade_events (
    event_id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    target_id TEXT,
    event_type TEXT NOT NULL,
    message TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (plan_id) REFERENCES upgrade_plans(plan_id) ON DELETE CASCADE,
    FOREIGN KEY (target_id) REFERENCES upgrade_targets(target_id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_upgrade_events_plan_created_at
    ON upgrade_events (plan_id, created_at);

CREATE TABLE IF NOT EXISTS upgrade_artifact_tokens (
    token_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    plan_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    node_id TEXT,
    version TEXT NOT NULL,
    component TEXT NOT NULL CHECK (component IN ('controller', 'agent')),
    target_triple TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (plan_id) REFERENCES upgrade_plans(plan_id) ON DELETE CASCADE,
    FOREIGN KEY (target_id) REFERENCES upgrade_targets(target_id) ON DELETE CASCADE,
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_upgrade_artifact_tokens_hash_expires_at
    ON upgrade_artifact_tokens (token_hash, expires_at);
