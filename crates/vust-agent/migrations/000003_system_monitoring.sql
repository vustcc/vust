-- Agent 宿主机系统监控设置、历史采样与采集状态。

-- 系统监控设置表：保存历史采集开关与保留天数，固定使用单例记录。
CREATE TABLE IF NOT EXISTS system_monitoring_settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    history_collection_enabled INTEGER NOT NULL CHECK (history_collection_enabled IN (0, 1)),
    retention_days INTEGER NOT NULL CHECK (retention_days IN (1, 3, 7)),
    updated_at INTEGER NOT NULL
);

INSERT OR IGNORE INTO system_monitoring_settings (
    singleton_id,
    history_collection_enabled,
    retention_days,
    updated_at
) VALUES (1, 1, 7, unixepoch());

-- 系统监控历史采样表：保存按分钟持久化的宿主机指标及采集源覆盖情况。
CREATE TABLE IF NOT EXISTS system_monitoring_samples (
    sampled_at INTEGER PRIMARY KEY,
    cpu_percent REAL,
    memory_used_bytes INTEGER,
    memory_total_bytes INTEGER,
    memory_percent REAL,
    load_average_1m REAL,
    load_average_5m REAL,
    load_average_15m REAL,
    disk_read_bytes INTEGER,
    disk_write_bytes INTEGER,
    network_receive_bytes INTEGER,
    network_transmit_bytes INTEGER,
    available_source_count INTEGER NOT NULL CHECK (available_source_count BETWEEN 0 AND 5),
    source_count INTEGER NOT NULL DEFAULT 5 CHECK (source_count = 5)
);

-- 系统监控采集状态表：保存最近采集尝试、成功时间与错误摘要，固定使用单例记录。
CREATE TABLE IF NOT EXISTS system_monitoring_collector_state (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    last_attempted_at INTEGER,
    last_sampled_at INTEGER,
    last_error_summary TEXT
);

INSERT OR IGNORE INTO system_monitoring_collector_state (singleton_id)
VALUES (1);
