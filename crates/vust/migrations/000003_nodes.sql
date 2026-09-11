-- 节点资产、注册、会话、观测和节点任务。
CREATE TABLE IF NOT EXISTS nodes (
    node_id TEXT PRIMARY KEY,
    tenant_id TEXT,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    group_name TEXT NOT NULL DEFAULT 'default',
    labels TEXT NOT NULL DEFAULT '[]',
    description TEXT,
    status TEXT NOT NULL DEFAULT 'draft' CHECK (
        status IN (
            'draft',
            'deploying',
            'deploy_failed',
            'awaiting_registration',
            'registered',
            'online',
            'degraded',
            'offline',
            'unreachable',
            'conflict',
            'retired'
        )
    ),
    desired_role TEXT,
    schedulable INTEGER NOT NULL DEFAULT 1 CHECK (schedulable IN (0, 1)),
    metadata TEXT NOT NULL DEFAULT '{}',
    registered_at TEXT,
    last_seen_at TEXT,
    last_probe_at TEXT,
    last_deploy_at TEXT,
    retired_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_nodes_normalized_name ON nodes (normalized_name);
CREATE INDEX IF NOT EXISTS idx_nodes_status ON nodes (status);
CREATE INDEX IF NOT EXISTS idx_nodes_group_name ON nodes (group_name);

CREATE TRIGGER IF NOT EXISTS set_nodes_updated_at
AFTER UPDATE ON nodes FOR EACH ROW
BEGIN
    UPDATE nodes
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE node_id = OLD.node_id;
END;

CREATE TABLE IF NOT EXISTS node_provisioning (
    provisioning_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL UNIQUE,
    deploy_method TEXT NOT NULL,
    ssh_addr TEXT,
    ssh_port INTEGER,
    ssh_user TEXT,
    ssh_auth_mode TEXT,
    ssh_password_ciphertext TEXT,
    ssh_private_key_ciphertext TEXT,
    ssh_private_key_passphrase_ciphertext TEXT,
    install_dir TEXT NOT NULL DEFAULT '/opt/vust',
    systemd_service_name TEXT NOT NULL DEFAULT 'vust-agent.service',
    expected_listen_port INTEGER,
    -- 节点实际回连主控使用的 URL，用于多网卡、跨网段和 NAT 场景下生成节点侧制品下载地址。
    vust_url TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    validated_revision INTEGER,
    precheck_valid_until TEXT,
    last_deploy_task_id TEXT,
    last_deploy_result_status TEXT,
    last_deploy_error_summary TEXT,
    last_deploy_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_node_provisioning_last_deploy_at ON node_provisioning (last_deploy_at);

CREATE TRIGGER IF NOT EXISTS set_node_provisioning_updated_at
AFTER UPDATE ON node_provisioning FOR EACH ROW
BEGIN
    UPDATE node_provisioning
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE provisioning_id = OLD.provisioning_id;
END;

CREATE TABLE IF NOT EXISTS node_enrollments (
    enrollment_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    token_status TEXT NOT NULL CHECK (
        token_status IN ('issued', 'used', 'expired', 'revoked')
    ),
    expires_at TEXT NOT NULL,
    first_used_at TEXT,
    last_used_at TEXT,
    revoked_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_node_enrollments_token_hash ON node_enrollments (token_hash);
CREATE INDEX IF NOT EXISTS idx_node_enrollments_node_id ON node_enrollments (node_id);
CREATE INDEX IF NOT EXISTS idx_node_enrollments_status_expires_at ON node_enrollments (token_status, expires_at);

CREATE TRIGGER IF NOT EXISTS set_node_enrollments_updated_at
AFTER UPDATE ON node_enrollments FOR EACH ROW
BEGIN
    UPDATE node_enrollments
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE enrollment_id = OLD.enrollment_id;
END;

CREATE TABLE IF NOT EXISTS node_identities (
    identity_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    certificate_serial_number TEXT,
    certificate_fingerprint TEXT NOT NULL,
    certificate_status TEXT NOT NULL CHECK (
        certificate_status IN ('active', 'rotating', 'revoked', 'expired')
    ),
    public_key_algorithm TEXT NOT NULL,
    certificate_issued_at TEXT,
    certificate_expires_at TEXT,
    rotated_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_node_identities_agent_id ON node_identities (agent_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_node_identities_certificate_fingerprint ON node_identities (certificate_fingerprint);
CREATE INDEX IF NOT EXISTS idx_node_identities_status_expires_at ON node_identities (certificate_status, certificate_expires_at);

CREATE TRIGGER IF NOT EXISTS set_node_identities_updated_at
AFTER UPDATE ON node_identities FOR EACH ROW
BEGIN
    UPDATE node_identities
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE identity_id = OLD.identity_id;
END;

CREATE TABLE IF NOT EXISTS node_sessions (
    session_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    lease_id TEXT NOT NULL,
    advertise_addr TEXT,
    listen_addr TEXT,
    listen_port INTEGER,
    server_name TEXT,
    registered_at TEXT NOT NULL,
    lease_expires_at TEXT NOT NULL,
    last_heartbeat_at TEXT,
    heartbeat_sequence INTEGER NOT NULL DEFAULT 0,
    last_seen_at TEXT,
    status TEXT NOT NULL CHECK (
        status IN ('active', 'draining', 'closed', 'expired', 'superseded')
    ),
    close_reason TEXT,
    closed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE,
    FOREIGN KEY (agent_id) REFERENCES node_identities(agent_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_node_sessions_lease_id ON node_sessions (lease_id);
CREATE INDEX IF NOT EXISTS idx_node_sessions_agent_id ON node_sessions (agent_id);
CREATE INDEX IF NOT EXISTS idx_node_sessions_status_lease_expires_at ON node_sessions (status, lease_expires_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_node_sessions_active_node_id ON node_sessions (node_id) WHERE status = 'active';

CREATE TABLE IF NOT EXISTS node_session_command_access (
    session_id TEXT PRIMARY KEY,
    transport TEXT NOT NULL CHECK (transport IN ('uds', 'https')),
    credential_ciphertext TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (session_id) REFERENCES node_sessions(session_id) ON DELETE CASCADE
);

CREATE TRIGGER IF NOT EXISTS set_node_sessions_updated_at
AFTER UPDATE ON node_sessions FOR EACH ROW
BEGIN
    UPDATE node_sessions
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE session_id = OLD.session_id;
END;

CREATE TABLE IF NOT EXISTS node_observations (
    observation_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    session_id TEXT,
    source TEXT NOT NULL CHECK (
        source IN ('heartbeat', 'probe', 'manual_check')
    ),
    system_snapshot TEXT,
    docker_snapshot TEXT,
    probe_result TEXT,
    observed_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE,
    FOREIGN KEY (session_id) REFERENCES node_sessions(session_id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_node_observations_node_id_observed_at ON node_observations (node_id, observed_at);
CREATE INDEX IF NOT EXISTS idx_node_observations_session_id ON node_observations (session_id);

CREATE TRIGGER IF NOT EXISTS set_node_observations_updated_at
AFTER UPDATE ON node_observations FOR EACH ROW
BEGIN
    UPDATE node_observations
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE observation_id = OLD.observation_id;
END;

CREATE TABLE IF NOT EXISTS node_tasks (
    task_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    task_type TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'running', 'cancel_requested', 'succeeded', 'failed', 'canceled')
    ),
    phase TEXT NOT NULL DEFAULT 'queued',
    progress_percent INTEGER CHECK (
        progress_percent IS NULL OR progress_percent BETWEEN 0 AND 100
    ),
    progress_logs TEXT NOT NULL DEFAULT '[]',
    cancellable INTEGER NOT NULL DEFAULT 0 CHECK (cancellable IN (0, 1)),
    result_summary TEXT,
    error_code TEXT,
    error_summary TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_node_tasks_node_id_created_at ON node_tasks (node_id, created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_node_tasks_one_active_per_node
    ON node_tasks (node_id)
    WHERE status IN ('queued', 'running', 'cancel_requested');

CREATE TRIGGER IF NOT EXISTS set_node_tasks_updated_at
AFTER UPDATE ON node_tasks FOR EACH ROW
BEGIN
    UPDATE node_tasks
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE task_id = OLD.task_id;
END;
