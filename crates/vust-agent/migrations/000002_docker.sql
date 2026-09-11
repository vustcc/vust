-- Agent Docker 领域数据。

-- Docker Compose 项目注册。
CREATE TABLE IF NOT EXISTS docker_compose_projects (
    name TEXT PRIMARY KEY,
    compose_dir TEXT NOT NULL UNIQUE,
    management_kind TEXT NOT NULL CHECK (management_kind IN ('custom', 'suite', 'system')),
    owner_name TEXT,
    config_revision INTEGER NOT NULL DEFAULT 1 CHECK (config_revision > 0),
    applied_revision INTEGER CHECK (applied_revision IS NULL OR applied_revision > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TRIGGER IF NOT EXISTS set_docker_compose_projects_updated_at
AFTER UPDATE ON docker_compose_projects FOR EACH ROW
BEGIN
    UPDATE docker_compose_projects
       SET updated_at = unixepoch()
     WHERE name = OLD.name;
END;

-- Compose 项目后台任务；不保存 YAML、环境变量或命令等敏感内容。
CREATE TABLE IF NOT EXISTS docker_compose_project_tasks (
    id TEXT PRIMARY KEY,
    project_name TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (
        operation IN ('create', 'start', 'stop', 'restart', 'redeploy', 'scale', 'remove')
    ),
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')
    ),
    stage TEXT NOT NULL CHECK (
        stage IN (
            'validating', 'preparing', 'pulling', 'applying', 'verifying',
            'rolling_back', 'cleaning_up', 'completed', 'cancelled', 'interrupted'
        )
    ),
    progress_percent INTEGER NOT NULL DEFAULT 0 CHECK (
        progress_percent >= 0 AND progress_percent <= 100
    ),
    progress_mode TEXT NOT NULL DEFAULT 'unavailable' CHECK (
        progress_mode IN ('structured', 'text', 'unavailable')
    ),
    progress_items TEXT NOT NULL DEFAULT '[]',
    service_name TEXT,
    replicas INTEGER CHECK (replicas IS NULL OR replicas >= 0),
    pull_images INTEGER NOT NULL DEFAULT 0 CHECK (pull_images IN (0, 1)),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('user', 'system')),
    actor_user_id INTEGER,
    actor_name TEXT NOT NULL,
    client_ip TEXT,
    trace_id TEXT,
    error_summary TEXT,
    cleanup_warning TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    started_at INTEGER,
    finished_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_docker_compose_project_tasks_project_status
    ON docker_compose_project_tasks (project_name, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_docker_compose_project_tasks_created_at
    ON docker_compose_project_tasks (created_at DESC);

-- Docker 宿主机资源汇总缓存。
CREATE TABLE IF NOT EXISTS docker_metrics_summary (
    created_at INTEGER PRIMARY KEY,
    cpu_core_percent REAL NOT NULL,
    cpu_host_percent REAL NOT NULL,
    memory_working_set_bytes INTEGER NOT NULL,
    memory_limit_bytes INTEGER NOT NULL,
    memory_percent REAL NOT NULL,
    running_container_count INTEGER NOT NULL,
    sampled_container_count INTEGER NOT NULL
);

-- Docker 容器资源时间序列；网络字段保存累计计数，由查询层计算速率。
CREATE TABLE IF NOT EXISTS docker_metrics_container (
    container_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    cpu_core_percent REAL NOT NULL,
    memory_working_set_bytes INTEGER NOT NULL,
    memory_limit_bytes INTEGER NOT NULL,
    memory_percent REAL NOT NULL,
    network_rx_bytes INTEGER NOT NULL,
    network_tx_bytes INTEGER NOT NULL,
    PRIMARY KEY (container_id, created_at)
);

CREATE INDEX IF NOT EXISTS idx_docker_metrics_container_created_at
    ON docker_metrics_container (created_at);
