-- 磁盘后台操作读模型：保存用户提交、可信身份上下文和最终结果，供页面恢复与平台日志使用。
CREATE TABLE disk_operations (
    operation_id TEXT PRIMARY KEY, -- UUIDv7 操作标识。
    node_id TEXT NOT NULL, -- 操作绑定的节点标识。
    idempotency_key TEXT NOT NULL, -- 客户端幂等键。
    kind TEXT NOT NULL CHECK (kind IN ('createPartition', 'eraseAndCreatePartition', 'formatPartition', 'adoptFilesystem', 'adoptMounted', 'mountVolume', 'unmountVolume', 'changeMountLocation', 'removeManagedVolume')), -- 操作类型。
    request_json TEXT NOT NULL, -- 已校验的领域请求；不得包含完整 fstab 或命令输出。
    status TEXT NOT NULL CHECK (status IN ('queued', 'validating', 'preparing', 'applying', 'verifying', 'canceling', 'succeeded', 'partial', 'failed', 'canceled')), -- 当前状态。
    phase TEXT, -- 非终态阶段；终态必须为空。
    target_json TEXT NOT NULL DEFAULT '{}', -- 安全的目标摘要。
    completed_steps INTEGER NOT NULL DEFAULT 0 CHECK (completed_steps >= 0), -- 已完成步骤数。
    total_steps INTEGER NOT NULL DEFAULT 0 CHECK (total_steps >= 0), -- 总步骤数。
    error_code TEXT, -- 稳定领域错误码。
    error_summary TEXT, -- 不含敏感信息的错误摘要。
    warning_summary TEXT, -- partial 或非阻塞问题摘要。
    actor_user_id INTEGER NOT NULL, -- 提交用户 ID。
    actor_name TEXT NOT NULL, -- 提交时可信用户名快照。
    client_ip TEXT NOT NULL, -- 认证会话中的可信客户端 IP。
    trace_id TEXT NOT NULL, -- 提交请求 trace ID。
    terminal_logged INTEGER NOT NULL DEFAULT 0 CHECK (terminal_logged IN (0, 1)), -- 是否已写入终态平台日志。
    created_at TEXT NOT NULL, -- RFC 3339 UTC 创建时间。
    updated_at TEXT NOT NULL, -- RFC 3339 UTC 更新时间。
    finished_at TEXT -- RFC 3339 UTC 终态时间。
);

-- 同一用户在同一节点重复提交相同幂等键时返回原操作。
CREATE UNIQUE INDEX idx_disk_operations_idempotency
    ON disk_operations(actor_user_id, node_id, idempotency_key);

-- 后台协调器按状态和更新时间恢复活动操作。
CREATE INDEX idx_disk_operations_active
    ON disk_operations(status, updated_at, operation_id);

-- 页面按节点查询最近操作。
CREATE INDEX idx_disk_operations_node_created
    ON disk_operations(node_id, created_at DESC, operation_id DESC);
