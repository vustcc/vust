-- VUST 受管数据卷：仅记录由本模块创建或显式接管的 ext4 文件系统。
CREATE TABLE managed_disk_volumes (
    volume_id TEXT PRIMARY KEY, -- UUIDv7 数据卷标识。
    disk_id TEXT NOT NULL, -- 提交时不透明磁盘标识。
    disk_fingerprint TEXT NOT NULL, -- 最近确认的稳定设备指纹。
    partition_id TEXT NOT NULL, -- PARTUUID 派生的分区标识。
    filesystem_uuid TEXT NOT NULL UNIQUE, -- ext4 文件系统 UUID。
    mount_name TEXT COLLATE NOCASE UNIQUE, -- 已分配的 /mnt 下一级目录名称；未挂载时为空。
    mount_path TEXT UNIQUE, -- 已分配的规范化绝对挂载路径；未挂载时为空。
    desired_state TEXT NOT NULL CHECK (desired_state IN ('mounted', 'unmounted')), -- 期望挂载状态。
    runtime_state TEXT NOT NULL CHECK (runtime_state IN ('mounted', 'unmounted', 'unavailable', 'conflict')), -- 最近观测到的实际状态。
    created_at TEXT NOT NULL, -- RFC 3339 UTC 创建时间。
    updated_at TEXT NOT NULL -- RFC 3339 UTC 更新时间。
);

-- Agent 磁盘操作事实：保存执行阶段、设备锁和重启恢复检查点。
CREATE TABLE disk_operations (
    operation_id TEXT PRIMARY KEY, -- Master 生成的 UUIDv7 操作标识。
    node_id TEXT NOT NULL, -- 操作声明的节点标识。
    kind TEXT NOT NULL CHECK (kind IN ('createPartition', 'eraseAndCreatePartition', 'formatPartition', 'adoptFilesystem', 'adoptMounted', 'mountVolume', 'unmountVolume', 'changeMountLocation', 'removeManagedVolume')), -- 操作类型。
    request_json TEXT NOT NULL, -- 已校验领域请求。
    target_lock_key TEXT NOT NULL, -- 磁盘或数据卷的互斥锁键。
    mount_lock_key TEXT, -- 新建或接管时不区分大小写的挂载名称互斥键。
    status TEXT NOT NULL CHECK (status IN ('queued', 'validating', 'preparing', 'applying', 'verifying', 'canceling', 'succeeded', 'partial', 'failed', 'canceled')), -- 当前状态。
    phase TEXT, -- 非终态阶段。
    target_json TEXT NOT NULL DEFAULT '{}', -- 安全目标摘要。
    checkpoint_json TEXT NOT NULL DEFAULT '{}', -- 重启恢复所需的最小检查点。
    completed_steps INTEGER NOT NULL DEFAULT 0 CHECK (completed_steps >= 0), -- 已完成步骤数。
    total_steps INTEGER NOT NULL DEFAULT 0 CHECK (total_steps >= 0), -- 总步骤数。
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)), -- 安全取消请求。
    error_code TEXT, -- 稳定领域错误码。
    error_summary TEXT, -- 不含命令输出的错误摘要。
    warning_summary TEXT, -- partial 或恢复提示。
    created_at TEXT NOT NULL, -- RFC 3339 UTC 创建时间。
    updated_at TEXT NOT NULL, -- RFC 3339 UTC 更新时间。
    finished_at TEXT -- RFC 3339 UTC 终态时间。
);

-- 同一目标只允许一个非终态操作，防止并发破坏块设备。
CREATE UNIQUE INDEX idx_disk_operations_active_target
    ON disk_operations(target_lock_key)
    WHERE status IN ('queued', 'validating', 'preparing', 'applying', 'verifying', 'canceling');

-- 不同磁盘并发创建时也不能竞争同一挂载名称。
CREATE UNIQUE INDEX idx_disk_operations_active_mount
    ON disk_operations(mount_lock_key COLLATE NOCASE)
    WHERE mount_lock_key IS NOT NULL
      AND status IN ('queued', 'validating', 'preparing', 'applying', 'verifying', 'canceling');

-- Agent 重启时按状态恢复未完成操作。
CREATE INDEX idx_disk_operations_active
    ON disk_operations(status, updated_at, operation_id)
    WHERE status IN ('queued', 'validating', 'preparing', 'applying', 'verifying', 'canceling');
