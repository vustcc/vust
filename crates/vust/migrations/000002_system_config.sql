-- 系统安全配置。单行宽表，id 固定为 1。
CREATE TABLE IF NOT EXISTS system_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),

    -- 当前安全入口路径，空字符串表示关闭。
    safe_entry TEXT NOT NULL DEFAULT '',
    -- 是否启用密码复杂度校验：0 关闭，1 开启。
    password_complexity INTEGER NOT NULL DEFAULT 0,

    -- 授权访问 IP 或 CIDR 列表，JSON 数组。
    allowed_ips TEXT NOT NULL DEFAULT '[]',
    -- 是否启用域名绑定：0 关闭，1 开启。
    domain_binding_enabled INTEGER NOT NULL DEFAULT 0,
    -- 允许访问服务的绑定域名列表，JSON 数组。
    bound_domains TEXT NOT NULL DEFAULT '[]',

    -- 密码过期天数，0 表示不过期。
    password_expires_days INTEGER NOT NULL DEFAULT 0,
    -- 触发登录锁定的失败次数。
    login_lockout_threshold INTEGER NOT NULL DEFAULT 5,
    -- 登录锁定时长，单位分钟。
    login_lockout_minutes INTEGER NOT NULL DEFAULT 15,
    -- 是否启用验证码功能：0 关闭，1 开启。
    captcha_enabled INTEGER NOT NULL DEFAULT 1,

    -- 两步验证总开关：0 关闭，1 开启。
    two_factor_enabled INTEGER NOT NULL DEFAULT 0,
    -- 启用的两步验证方式列表，JSON 数组。
    two_factor_methods TEXT NOT NULL DEFAULT '[]',
    -- TOTP 发行方名称。
    totp_issuer TEXT NOT NULL DEFAULT 'VUST',

    -- 通行密钥登录开关：0 关闭，1 开启。
    passkey_enabled INTEGER NOT NULL DEFAULT 0,
    -- 通行密钥策略配置，JSON 对象。
    passkey_policy TEXT NOT NULL DEFAULT '{}',

    -- API 接口访问总开关：0 关闭，1 开启。
    api_access_enabled INTEGER NOT NULL DEFAULT 0,
    -- API Token 认证开关：0 关闭，1 开启。
    api_token_enabled INTEGER NOT NULL DEFAULT 0,
    -- API Token 默认过期天数，0 表示不过期。
    api_token_expires_days INTEGER NOT NULL DEFAULT 0,
    -- API CORS 授权来源列表，JSON 数组。
    api_allowed_origins TEXT NOT NULL DEFAULT '[]',
    -- API 限流策略配置，JSON 对象。
    api_rate_limit TEXT NOT NULL DEFAULT '{}',

    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

INSERT OR IGNORE INTO system_config (id) VALUES (1);

CREATE TRIGGER IF NOT EXISTS set_system_config_updated_at
AFTER UPDATE ON system_config FOR EACH ROW
BEGIN
    UPDATE system_config
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE id = OLD.id;
END;
