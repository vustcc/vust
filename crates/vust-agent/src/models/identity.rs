//! 身份模型：节点身份与认证信息。

use crate::config::AgentConfig;
use crate::crypto::{CryptoError, decrypt_bytes, encrypt_bytes};
use crate::state::DbPool;
use crate::types::AgentMode;
use sqlx::FromRow;
use vust_security::certs::issue_server_cert;

/// 数据库存储的身份记录（包含加密证书）。
#[derive(Debug, Clone, FromRow)]
pub struct AgentIdentityRecord {
    pub id: i64,
    pub mode: String,
    pub listen_addr: Option<String>,
    pub agent_ip: Option<String>,
    pub agent_id: Option<String>,
    pub vust_url: Option<String>,
    pub enrollment_token: Option<String>,
    pub session_id: Option<String>,
    pub lease_id: Option<String>,
    pub cert_pem_enc: Option<String>,
    pub key_pem_enc: Option<String>,
}

/// 运行时使用的身份配置与解密后的证书。
#[derive(Debug, Clone)]
pub struct AgentIdentity {
    pub mode: AgentMode,
    pub listen_addr: Option<String>,
    pub agent_ip: Option<String>,
    pub agent_id: Option<String>,
    pub vust_url: Option<String>,
    pub enrollment_token: Option<String>,
    pub session_id: Option<String>,
    pub lease_id: Option<String>,
    pub cert_pem: Option<Vec<u8>>,
    pub key_pem: Option<Vec<u8>>,
}

/// 从数据库读取身份信息，不存在则初始化。
pub async fn load_or_init_identity(
    pool: &DbPool,
    config: &AgentConfig,
) -> anyhow::Result<AgentIdentity> {
    if let Some(record) = sqlx::query_as::<_, AgentIdentityRecord>(
        r#"SELECT id, mode, listen_addr, agent_ip, agent_id, vust_url, enrollment_token, session_id, lease_id, cert_pem_enc, key_pem_enc
           FROM agent_identity
           ORDER BY id
           LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await?
    {
        let mut new_mode = record.mode.clone();
        let mut new_listen = record.listen_addr.clone();
        let mut new_ip = record.agent_ip.clone();
        let mut new_agent_id = record.agent_id.clone();
        let mut new_vust_url = record.vust_url.clone();
        let mut new_enrollment_token = record.enrollment_token.clone();
        let new_session_id = record.session_id.clone();
        let new_lease_id = record.lease_id.clone();
        let mut reset_certs = false;
        let mut needs_update = false;

        if let Some(config_mode) = config
            .mode
            .as_ref()
            .and_then(|value| value.parse::<AgentMode>().ok())
        {
            let mode_str = config_mode.as_str().to_string();
            if mode_str != record.mode {
                new_mode = mode_str;
                needs_update = true;
            }
        }

        if let Some(config_listen) = config.listen_addr.as_ref()
            && Some(config_listen) != record.listen_addr.as_ref()
        {
            new_listen = Some(config_listen.clone());
            reset_certs = true;
            needs_update = true;
        }

        if let Some(config_ip) = config.agent_ip.as_ref()
            && Some(config_ip) != record.agent_ip.as_ref()
        {
            new_ip = Some(config_ip.clone());
            reset_certs = true;
            needs_update = true;
        }

        if let Some(config_agent_id) = config.agent_id.as_ref()
            && Some(config_agent_id) != record.agent_id.as_ref()
        {
            new_agent_id = Some(config_agent_id.clone());
            needs_update = true;
        }

        if let Some(config_vust_url) = config.vust_url.as_ref()
            && Some(config_vust_url) != record.vust_url.as_ref()
        {
            new_vust_url = Some(config_vust_url.clone());
            needs_update = true;
        }

        if let Some(config_enrollment_token) = config.enrollment_token.as_ref()
            && Some(config_enrollment_token) != record.enrollment_token.as_ref()
        {
            // enrollment token 仅用于首次纳管。若已建立 agent_id/会话历史，不再从配置回灌 token，
            // 避免重启后误走 enroll 分支导致 token 已失效。
            let bootstrap_only = record.agent_id.is_none()
                && record.session_id.is_none()
                && record.lease_id.is_none();
            if bootstrap_only {
                new_enrollment_token = Some(config_enrollment_token.clone());
                needs_update = true;
            }
            // 若不是首次引导（已经建立了有效身份），我们直接忽略配置文件中残留的静态 enrollment_token。
            // 绝不清除本地证书和会话身份，彻底消除因配置残留导致的重启自毁死锁。
        }

        if needs_update {
            let (cert, key) = if reset_certs {
                (None::<String>, None::<String>)
            } else {
                (record.cert_pem_enc.clone(), record.key_pem_enc.clone())
            };
            sqlx::query(
                r#"UPDATE agent_identity
                   SET mode = ?1,
                       listen_addr = ?2,
                       agent_ip = ?3,
                       agent_id = ?4,
                       vust_url = ?5,
                       enrollment_token = ?6,
                       cert_pem_enc = ?7,
                       key_pem_enc = ?8,
                       session_id = ?9,
                       lease_id = ?10
                   WHERE id = ?11"#,
            )
            .bind(new_mode)
            .bind(new_listen)
            .bind(new_ip)
            .bind(new_agent_id)
            .bind(new_vust_url)
            .bind(new_enrollment_token)
            .bind(cert)
            .bind(key)
            .bind(new_session_id)
            .bind(new_lease_id)
            .bind(record.id)
            .execute(pool)
            .await?;

            let updated = sqlx::query_as::<_, AgentIdentityRecord>(
                r#"SELECT id, mode, listen_addr, agent_ip, agent_id, vust_url, enrollment_token, session_id, lease_id, cert_pem_enc, key_pem_enc
                   FROM agent_identity
                   WHERE id = ?1"#,
            )
            .bind(record.id)
            .fetch_one(pool)
            .await?;
            return decode_record(updated);
        }

        return decode_record(record);
    }

    let config_mode = config
        .mode
        .as_ref()
        .and_then(|value| value.parse::<AgentMode>().ok())
        .unwrap_or(AgentMode::Local);
    let config_listen = config.listen_addr.clone();
    let config_agent_ip = config.agent_ip.clone();
    let config_agent_id = config.agent_id.clone();
    let config_vust_url = config.vust_url.clone();
    let config_enrollment_token = config.enrollment_token.clone();

    sqlx::query(
        r#"INSERT INTO agent_identity (mode, listen_addr, agent_ip, agent_id, vust_url, enrollment_token)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
    )
    .bind(config_mode.as_str())
    .bind(config_listen.clone())
    .bind(config_agent_ip.clone())
    .bind(config_agent_id.clone())
    .bind(config_vust_url.clone())
    .bind(config_enrollment_token.clone())
    .execute(pool)
    .await?;

    Ok(AgentIdentity {
        mode: config_mode,
        listen_addr: config_listen,
        agent_ip: config_agent_ip,
        agent_id: config_agent_id,
        vust_url: config_vust_url,
        enrollment_token: config_enrollment_token,
        session_id: None,
        lease_id: None,
        cert_pem: None,
        key_pem: None,
    })
}

/// 加密并写入身份证书到数据库。
pub async fn upsert_identity_certs(
    pool: &DbPool,
    cert_pem: &[u8],
    key_pem: &[u8],
) -> anyhow::Result<()> {
    let cert_enc = encrypt_bytes(cert_pem).map_err(map_crypto_err)?;
    let key_enc = encrypt_bytes(key_pem).map_err(map_crypto_err)?;
    sqlx::query(
        r#"UPDATE agent_identity
           SET cert_pem_enc = ?1, key_pem_enc = ?2
           WHERE id = (SELECT id FROM agent_identity ORDER BY id LIMIT 1)"#,
    )
    .bind(cert_enc)
    .bind(key_enc)
    .execute(pool)
    .await?;
    Ok(())
}

/// 更新 runtime session 字段，并在需要时清空 enrollment token。
pub async fn update_runtime_session(
    pool: &DbPool,
    agent_id: &str,
    session_id: &str,
    lease_id: &str,
    clear_enrollment_token: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"UPDATE agent_identity
           SET agent_id = ?1,
               session_id = ?2,
               lease_id = ?3,
               enrollment_token = CASE WHEN ?4 THEN NULL ELSE enrollment_token END
           WHERE id = (SELECT id FROM agent_identity ORDER BY id LIMIT 1)"#,
    )
    .bind(agent_id)
    .bind(session_id)
    .bind(lease_id)
    .bind(if clear_enrollment_token { 1_i64 } else { 0_i64 })
    .execute(pool)
    .await?;
    Ok(())
}

/// 清空当前 runtime session 字段。
pub async fn clear_runtime_session(pool: &DbPool) -> anyhow::Result<()> {
    sqlx::query(
        r#"UPDATE agent_identity
           SET session_id = NULL, lease_id = NULL
           WHERE id = (SELECT id FROM agent_identity ORDER BY id LIMIT 1)"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// 确保证书存在，不足时签发并保存。
pub async fn ensure_identity_certs(
    pool: &DbPool,
    identity: &mut AgentIdentity,
    sans: &[String],
) -> anyhow::Result<()> {
    if identity.cert_pem.is_some() && identity.key_pem.is_some() {
        return Ok(());
    }

    let issued = issue_server_cert("vust-agent", sans)?;
    upsert_identity_certs(pool, &issued.cert_pem, &issued.key_pem).await?;
    identity.cert_pem = Some(issued.cert_pem);
    identity.key_pem = Some(issued.key_pem);
    Ok(())
}

fn decode_record(record: AgentIdentityRecord) -> anyhow::Result<AgentIdentity> {
    let mode: AgentMode = record.mode.parse()?;
    let cert_pem = match record.cert_pem_enc {
        Some(value) => Some(decrypt_bytes(&value).map_err(map_crypto_err)?),
        None => None,
    };
    let key_pem = match record.key_pem_enc {
        Some(value) => Some(decrypt_bytes(&value).map_err(map_crypto_err)?),
        None => None,
    };

    Ok(AgentIdentity {
        mode,
        listen_addr: record.listen_addr,
        agent_ip: record.agent_ip,
        agent_id: record.agent_id,
        vust_url: record.vust_url,
        enrollment_token: record.enrollment_token,
        session_id: record.session_id,
        lease_id: record.lease_id,
        cert_pem,
        key_pem,
    })
}

fn map_crypto_err(err: CryptoError) -> anyhow::Error {
    anyhow::anyhow!("crypto error: {:?}", err)
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_identity_certs, load_or_init_identity, update_runtime_session, upsert_identity_certs,
    };
    use crate::config::AgentConfig;
    use crate::test_support::setup_test_db;
    use crate::types::AgentMode;

    #[tokio::test]
    async fn init_identity_inserts_default_row() {
        let pool = setup_test_db().await;
        let config = AgentConfig::default();

        let identity = load_or_init_identity(&pool, &config).await.unwrap();
        assert_eq!(identity.mode, AgentMode::Local);
        assert!(identity.agent_id.is_none());

        let identity_again = load_or_init_identity(&pool, &config).await.unwrap();
        assert_eq!(identity_again.mode, AgentMode::Local);
    }

    #[tokio::test]
    async fn upsert_identity_certs_roundtrip() {
        let pool = setup_test_db().await;
        let config = AgentConfig::default();
        let _ = load_or_init_identity(&pool, &config).await.unwrap();

        let cert = b"fake-cert";
        let key = b"fake-key";
        upsert_identity_certs(&pool, cert, key).await.unwrap();

        let identity = load_or_init_identity(&pool, &config).await.unwrap();
        assert_eq!(identity.cert_pem.as_deref(), Some(cert.as_slice()));
        assert_eq!(identity.key_pem.as_deref(), Some(key.as_slice()));
    }

    #[tokio::test]
    async fn ensure_identity_certs_generates_when_missing() {
        let pool = setup_test_db().await;
        let config = AgentConfig::default();
        let mut identity = load_or_init_identity(&pool, &config).await.unwrap();

        ensure_identity_certs(&pool, &mut identity, &[String::from("127.0.0.1")])
            .await
            .unwrap();
        assert!(identity.cert_pem.as_ref().is_some());
        assert!(identity.key_pem.as_ref().is_some());
    }

    #[tokio::test]
    async fn restart_does_not_rehydrate_enrollment_token_after_runtime_initialized() {
        let pool = setup_test_db().await;
        let config = AgentConfig {
            mode: Some("remote".to_string()),
            listen_addr: Some("[::]:7311".to_string()),
            agent_ip: Some("10.0.0.81".to_string()),
            agent_id: None,
            vust_url: Some("http://10.0.0.43:7310".to_string()),
            enrollment_token: Some("ldt_token_bootstrap".to_string()),
            compose_root_dir: None,
            stats_sample_interval_secs: None,
            stats_retention_hours: None,
            ..AgentConfig::default()
        };

        let _ = load_or_init_identity(&pool, &config).await.unwrap();
        update_runtime_session(&pool, "node-1", "session-1", "lease-1", true)
            .await
            .unwrap();

        let reloaded = load_or_init_identity(&pool, &config).await.unwrap();
        assert_eq!(reloaded.agent_id.as_deref(), Some("node-1"));
        assert!(
            reloaded.enrollment_token.is_none(),
            "enrollment token should not be written back from config after runtime initialized"
        );
    }
}
