-- Agent 身份与本地设置。
CREATE TABLE IF NOT EXISTS agent_identity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mode TEXT NOT NULL,
    listen_addr TEXT,
    agent_ip TEXT,
    agent_id TEXT,
    cert_pem_enc TEXT,
    key_pem_enc TEXT,
    vust_url TEXT,
    enrollment_token TEXT,
    session_id TEXT,
    lease_id TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER IF NOT EXISTS set_agent_identity_updated_at
AFTER UPDATE ON agent_identity FOR EACH ROW
BEGIN
    UPDATE agent_identity
       SET updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
     WHERE id = OLD.id;
END;

CREATE TABLE IF NOT EXISTS agent_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
