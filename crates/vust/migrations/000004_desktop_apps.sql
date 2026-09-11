-- 桌面应用注册表与用户桌面配置。
CREATE TABLE IF NOT EXISTS apps (
    app_id TEXT PRIMARY KEY,
    i18n_title_key TEXT NOT NULL,
    icon TEXT NOT NULL,
    default_width INTEGER NOT NULL,
    default_height INTEGER NOT NULL,
    min_width INTEGER NOT NULL,
    min_height INTEGER NOT NULL,
    hidden INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0,
    app_type TEXT NOT NULL DEFAULT 'builtin', -- 应用来源类型，例如内置应用、套件应用或系统入口。
    source_id TEXT, -- 来源标识，套件应用可记录 suite_id 或实例 ID。
    metadata TEXT NOT NULL DEFAULT '{}', -- 应用窗口、权限和展示策略的扩展配置。
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER IF NOT EXISTS set_apps_updated_at
AFTER UPDATE ON apps FOR EACH ROW
BEGIN
    UPDATE apps
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE app_id = OLD.app_id;
END;

CREATE TABLE IF NOT EXISTS desktop_apps (
    node_id TEXT NOT NULL DEFAULT 'local',
    app_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    visible INTEGER NOT NULL DEFAULT 1,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (node_id, app_id)
);

CREATE INDEX IF NOT EXISTS idx_desktop_apps_node_sort
    ON desktop_apps (node_id, sort_order, app_id);

CREATE TRIGGER IF NOT EXISTS set_desktop_apps_updated_at
AFTER UPDATE ON desktop_apps FOR EACH ROW
BEGIN
    UPDATE desktop_apps
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE node_id = OLD.node_id
       AND app_id = OLD.app_id;
END;

INSERT OR IGNORE INTO apps (
    app_id,
    i18n_title_key,
    icon,
    default_width,
    default_height,
    min_width,
    min_height,
    hidden,
    sort_order
)
VALUES
    ('suite-center', 'app.suiteCenter.appName', '/images/apps/suite-center.png', 1120, 760, 900, 620, 0, 10),
    ('node-manager', 'app.nodes.appName', '/images/apps/node-manager.png', 1024, 720, 860, 560, 0, 20),
    ('docker-manager', 'app.docker.appName', '/images/apps/docker-manager.png', 1024, 720, 860, 560, 0, 30),
    ('system-monitor', 'app.systemMonitor.appName', '/images/apps/system-monitor.png', 1024, 720, 860, 560, 0, 40),
    ('terminal', 'app.terminal.appName', '/images/apps/terminal.png', 1024, 720, 860, 560, 0, 50),
    ('file-manager', 'app.fileManager.appName', '/images/apps/file-manager.png', 1024, 720, 860, 560, 0, 60),
    ('file-editor', 'app.fileEditor.appName', '/images/apps/file-editor.png', 1024, 720, 860, 560, 0, 70),
    ('process-manager', 'app.processManager.appName', '/images/apps/process-manager.png', 1024, 720, 860, 560, 0, 80),
    ('disk-manager', 'app.diskManager.appName', '/images/apps/disk-manager.png', 1024, 720, 860, 560, 0, 90),
    ('firewall-manager', 'app.firewallManager.appName', '/images/apps/firewall-manager.png', 1024, 720, 860, 560, 0, 100),
    ('task-scheduler', 'app.taskScheduler.appName', '/images/apps/task-scheduler.png', 1024, 720, 860, 560, 0, 110),
    ('script-manager', 'app.scriptManager.appName', '/images/apps/script-manager.png', 1024, 720, 860, 560, 0, 120),
    ('operation-log', 'app.operationLog.appName', '/images/apps/platform-log.png', 1024, 720, 860, 560, 0, 130),
    ('runtime-log', 'app.runtimeLog.appName', '/images/apps/runtime-log.png', 1024, 720, 860, 560, 0, 140),
    ('notification-center', 'app.notificationCenter.appName', '/images/apps/notification-center.png', 1024, 720, 860, 560, 0, 150),
    ('settings', 'app.settings.appName', '/images/apps/settings.png', 1024, 720, 860, 560, 0, 160);
