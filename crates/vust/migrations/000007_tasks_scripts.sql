-- 计划任务定义：保存 Master 管理的任务配置、资源归属和部署读模型。
CREATE TABLE scheduled_tasks (
    task_id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80),
    name_key TEXT NOT NULL,
    description TEXT CHECK(description IS NULL OR length(description) <= 500),
    node_id TEXT NOT NULL,
    command TEXT NOT NULL CHECK(length(command) BETWEEN 1 AND 65536),
    cron_expr TEXT NOT NULL,
    time_zone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
    desired_state TEXT NOT NULL CHECK(desired_state IN ('enabled', 'disabled')),
    timeout_seconds INTEGER NOT NULL CHECK(timeout_seconds BETWEEN 1 AND 86400),
    prevent_overlap INTEGER NOT NULL CHECK(prevent_overlap IN (0, 1)),
    ownership_kind TEXT NOT NULL CHECK(ownership_kind IN ('custom', 'compose', 'suite', 'system')),
    owner_id TEXT,
    owner_name TEXT,
    manager_path TEXT,
    created_by_user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_by_name TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    deployment_status TEXT NOT NULL CHECK(deployment_status IN (
        'pending', 'applying', 'ready', 'waiting_for_node', 'failed', 'deleting', 'migrating'
    )),
    deployment_error_summary TEXT,
    last_synced_at TEXT,
    next_run_status TEXT NOT NULL CHECK(next_run_status IN (
        'scheduled', 'disabled', 'not_deployed', 'unavailable'
    )),
    next_run_at TEXT,
    last_run_id TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(node_id, name_key)
);

CREATE INDEX idx_scheduled_tasks_custom_list
    ON scheduled_tasks(ownership_kind, deleted_at, node_id, updated_at DESC, task_id);
CREATE INDEX idx_scheduled_tasks_deployment
    ON scheduled_tasks(deployment_status, updated_at, task_id);

-- 计划任务运行记录：保存每次触发的生命周期、可信操作人上下文和输出摘要。
CREATE TABLE scheduled_task_runs (
    run_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    trigger_source TEXT NOT NULL CHECK(trigger_source IN ('schedule', 'manual', 'batch')),
    status TEXT NOT NULL CHECK(status IN (
        'queued', 'starting', 'running', 'cancelling', 'succeeded', 'failed', 'timed_out', 'cancelled'
    )),
    phase TEXT,
    queued_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    exit_code INTEGER,
    error_code TEXT,
    error_summary TEXT,
    output_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK(output_size_bytes >= 0),
    output_truncated INTEGER NOT NULL DEFAULT 0 CHECK(output_truncated IN (0, 1)),
    overlap_guard TEXT,
    actor_user_id INTEGER,
    actor_name TEXT,
    client_ip TEXT,
    trace_id TEXT,
    terminal_logged_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(task_id) REFERENCES scheduled_tasks(task_id) ON DELETE CASCADE
);

CREATE INDEX idx_scheduled_task_runs_task
    ON scheduled_task_runs(task_id, queued_at DESC, run_id DESC);
CREATE INDEX idx_scheduled_task_runs_active
    ON scheduled_task_runs(status, updated_at) WHERE status IN ('queued', 'starting', 'running', 'cancelling');
CREATE UNIQUE INDEX idx_scheduled_task_runs_overlap_guard
    ON scheduled_task_runs(overlap_guard)
    WHERE overlap_guard IS NOT NULL AND status IN ('queued', 'starting', 'running', 'cancelling');

-- 计划任务运行输出：保存 Master 可分页读取的有界命令输出。
CREATE TABLE scheduled_task_run_outputs (
    run_id TEXT PRIMARY KEY,
    content BLOB NOT NULL,
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0 AND size_bytes <= 262144),
    truncated INTEGER NOT NULL CHECK(truncated IN (0, 1)),
    updated_at TEXT NOT NULL,
    FOREIGN KEY(run_id) REFERENCES scheduled_task_runs(run_id) ON DELETE CASCADE
);

-- 脚本库：保存 Master 管理的脚本正文、修订、资源归属和可信变更用户。
CREATE TABLE scripts (
    script_id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80),
    name_key TEXT NOT NULL,
    description TEXT CHECK(description IS NULL OR length(description) <= 500),
    interactive INTEGER NOT NULL CHECK(interactive IN (0, 1)),
    language TEXT NOT NULL DEFAULT 'shell' CHECK(language = 'shell'),
    content TEXT NOT NULL CHECK(length(CAST(content AS BLOB)) BETWEEN 1 AND 262144),
    content_size_bytes INTEGER NOT NULL CHECK(content_size_bytes BETWEEN 1 AND 262144),
    source_sha256 TEXT NOT NULL CHECK(length(source_sha256) = 64),
    default_timeout_seconds INTEGER NOT NULL DEFAULT 300 CHECK(default_timeout_seconds BETWEEN 1 AND 86400),
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    ownership_kind TEXT NOT NULL DEFAULT 'custom' CHECK(ownership_kind IN ('custom', 'compose', 'suite', 'system')),
    owner_id TEXT,
    owner_name TEXT,
    manager_path TEXT,
    created_by_user_id INTEGER NOT NULL,
    created_by_name TEXT NOT NULL,
    updated_by_user_id INTEGER NOT NULL,
    updated_by_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_scripts_name_key ON scripts(name_key);
CREATE INDEX idx_scripts_public_list
    ON scripts(ownership_kind, updated_at DESC, script_id);

-- 脚本运行：保存 Master 接受的幂等请求、确定脚本快照、节点和可信用户上下文。
CREATE TABLE script_runs (
    run_id TEXT PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    script_id TEXT NOT NULL,
    script_name TEXT NOT NULL,
    script_revision INTEGER NOT NULL CHECK(script_revision > 0),
    source_sha256 TEXT NOT NULL CHECK(length(source_sha256) = 64),
    source_content TEXT NOT NULL CHECK(length(CAST(source_content AS BLOB)) BETWEEN 1 AND 262144),
    timeout_seconds INTEGER NOT NULL CHECK(timeout_seconds BETWEEN 1 AND 86400),
    node_id TEXT NOT NULL,
    node_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'starting', 'running', 'cancelling', 'succeeded', 'failed', 'timed_out', 'cancelled')),
    phase TEXT,
    queued_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    exit_code INTEGER,
    error_code TEXT,
    error_summary TEXT,
    overlap_guard TEXT,
    actor_user_id INTEGER NOT NULL,
    actor_name TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    terminal_logged_at TEXT,
    discard_requested INTEGER NOT NULL DEFAULT 0 CHECK(discard_requested IN (0, 1)),
    connection_deadline TEXT,
    attached_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(script_id) REFERENCES scripts(script_id) ON DELETE CASCADE
);

CREATE INDEX idx_script_runs_active
    ON script_runs(status, updated_at) WHERE status IN ('queued', 'starting', 'running', 'cancelling');
CREATE UNIQUE INDEX idx_script_runs_overlap_guard
    ON script_runs(overlap_guard)
    WHERE overlap_guard IS NOT NULL AND status IN ('queued', 'starting', 'running', 'cancelling');
