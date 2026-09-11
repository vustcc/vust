-- Compose 套件中心：套件目录、实例和应用入口。
CREATE TABLE IF NOT EXISTS suite_catalog_items (
    suite_id TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    name TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT 'uncategorized', -- 套件分类，用于套件中心筛选和展示。
    icon TEXT NOT NULL DEFAULT '', -- 套件图标资产 URL 或包内图标路径。
    manifest_json TEXT NOT NULL,
    package_json TEXT NOT NULL,
    checksum TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'available',
    installed_at TEXT, -- 套件首次安装时间。
    removed_at TEXT, -- 套件从套件中心删除或移除的时间。
    metadata TEXT NOT NULL DEFAULT '{}', -- 套件展示、来源和扩展治理信息。
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER IF NOT EXISTS set_suite_catalog_items_updated_at
AFTER UPDATE ON suite_catalog_items FOR EACH ROW
BEGIN
    UPDATE suite_catalog_items
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE suite_id = OLD.suite_id;
END;

CREATE TABLE IF NOT EXISTS suite_instances (
    instance_id TEXT PRIMARY KEY,
    suite_id TEXT NOT NULL,
    version TEXT NOT NULL,
    platform_contract_version INTEGER NOT NULL CHECK (platform_contract_version >= 1),
    node_id TEXT NOT NULL,
    compose_project_name TEXT NOT NULL,
    status TEXT NOT NULL,
    config_json TEXT NOT NULL DEFAULT '{}',
    last_error TEXT,
    enabled INTEGER NOT NULL DEFAULT 0, -- 套件是否处于启用状态。
    enabled_at TEXT, -- 最近一次启用时间。
    disabled_at TEXT, -- 最近一次停用时间。
    uninstalled_at TEXT, -- 套件卸载时间，保留实例记录时用于生命周期展示。
    last_started_at TEXT, -- 最近一次 Compose 启动时间。
    last_stopped_at TEXT, -- 最近一次 Compose 停止时间。
    metadata TEXT NOT NULL DEFAULT '{}', -- 套件实例运行、桌面恢复和扩展状态信息。
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (suite_id) REFERENCES suite_catalog_items(suite_id) ON DELETE CASCADE
);

CREATE TRIGGER IF NOT EXISTS set_suite_instances_updated_at
AFTER UPDATE ON suite_instances FOR EACH ROW
BEGIN
    UPDATE suite_instances
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE instance_id = OLD.instance_id;
END;

CREATE TABLE IF NOT EXISTS suite_app_entries (
    app_id TEXT PRIMARY KEY,
    suite_instance_id TEXT NOT NULL,
    node_id TEXT NOT NULL DEFAULT 'local',
    app_entry_id TEXT NOT NULL,
    title TEXT NOT NULL,
    icon TEXT NOT NULL,
    entry_type TEXT NOT NULL,
    entry_target TEXT NOT NULL,
    i18n_title_key TEXT, -- 应用入口标题国际化键，存在时优先用于界面渲染。
    default_width INTEGER NOT NULL DEFAULT 1024,
    default_height INTEGER NOT NULL DEFAULT 720,
    min_width INTEGER NOT NULL DEFAULT 860,
    min_height INTEGER NOT NULL DEFAULT 560,
    sort_order INTEGER NOT NULL DEFAULT 10000,
    enabled INTEGER NOT NULL DEFAULT 1,
    metadata TEXT NOT NULL DEFAULT '{}', -- 入口窗口策略、权限和展示扩展信息。
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (suite_instance_id, app_entry_id),
    FOREIGN KEY (suite_instance_id) REFERENCES suite_instances(instance_id) ON DELETE CASCADE
);

CREATE TRIGGER IF NOT EXISTS set_suite_app_entries_updated_at
AFTER UPDATE ON suite_app_entries FOR EACH ROW
BEGIN
    UPDATE suite_app_entries
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE app_id = OLD.app_id;
END;

CREATE UNIQUE INDEX IF NOT EXISTS idx_suite_instances_suite_node
    ON suite_instances (suite_id, node_id);

CREATE INDEX IF NOT EXISTS idx_suite_instances_node
    ON suite_instances (node_id);

CREATE INDEX IF NOT EXISTS idx_suite_app_entries_node
    ON suite_app_entries (node_id, enabled);
