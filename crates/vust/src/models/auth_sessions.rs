//! Web 控制台认证会话模型：保存单用户管理员的多浏览器服务端会话。

use chrono::{Duration, Utc};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

const SESSION_TTL_HOURS: i64 = 12;
const SESSION_TOKEN_BYTES: usize = 32;

/// 服务端认证会话记录。
#[derive(Debug, Clone, FromRow)]
pub struct AuthSessionRecord {
    pub id: String,
    pub session_token_hash: String,
    pub user_id: i64,
    pub created_at: String,
    pub expires_at: String,
    pub last_seen_at: String,
    pub revoked_at: Option<String>,
    pub revoked_reason: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
}

/// 登录成功后返回给前端的会话摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionSummary {
    pub id: String,
    pub expires_at: String,
}

/// 创建认证会话所需的输入。
#[derive(Debug, Clone)]
pub struct CreateAuthSessionInput {
    pub user_id: i64,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
}

/// 新建认证会话的结果，明文 token 只用于写入 Cookie。
#[derive(Debug, Clone)]
pub struct CreatedAuthSession {
    pub token: String,
    pub record: AuthSessionRecord,
}

/// 生成随机会话 token。
fn generate_session_token() -> anyhow::Result<String> {
    let rng = SystemRandom::new();
    let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
    rng.fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to generate session token"))?;
    Ok(hex::encode(bytes))
}

/// 对明文会话 token 做不可逆哈希。
pub fn hash_session_token(token: &str) -> String {
    let digest = digest::digest(&digest::SHA256, token.as_bytes());
    hex::encode(digest.as_ref())
}

/// 创建一条新的服务端认证会话。
pub async fn create_auth_session(
    pool: &SqlitePool,
    input: CreateAuthSessionInput,
) -> anyhow::Result<CreatedAuthSession> {
    let token = generate_session_token()?;
    let session_token_hash = hash_session_token(&token);
    let id = crate::types::new_uuid_v7();
    let now = Utc::now();
    let expires_at = now + Duration::hours(SESSION_TTL_HOURS);

    sqlx::query(
        r#"
        INSERT INTO auth_sessions (
            id, session_token_hash, user_id, created_at, expires_at,
            last_seen_at, client_ip, user_agent
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
    )
    .bind(&id)
    .bind(&session_token_hash)
    .bind(input.user_id)
    .bind(now.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(input.client_ip)
    .bind(input.user_agent)
    .execute(pool)
    .await?;

    let record = get_auth_session_by_hash(pool, &session_token_hash)
        .await?
        .ok_or_else(|| anyhow::anyhow!("created auth session was not found"))?;
    Ok(CreatedAuthSession { token, record })
}

/// 按 token hash 查询会话。
pub async fn get_auth_session_by_hash(
    pool: &SqlitePool,
    session_token_hash: &str,
) -> sqlx::Result<Option<AuthSessionRecord>> {
    sqlx::query_as::<_, AuthSessionRecord>(
        r#"
        SELECT
            id, session_token_hash, user_id, created_at, expires_at,
            last_seen_at, revoked_at, revoked_reason, client_ip, user_agent
        FROM auth_sessions
        WHERE session_token_hash = ?1
        "#,
    )
    .bind(session_token_hash)
    .fetch_optional(pool)
    .await
}

/// 更新会话最近访问时间。
pub async fn touch_auth_session(pool: &SqlitePool, session_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        UPDATE auth_sessions
        SET last_seen_at = ?2
        WHERE id = ?1
        "#,
    )
    .bind(session_id)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// 撤销指定 token 对应的会话。
pub async fn revoke_auth_session_by_token(
    pool: &SqlitePool,
    token: &str,
    reason: &str,
) -> sqlx::Result<bool> {
    let session_token_hash = hash_session_token(token);
    let result = sqlx::query(
        r#"
        UPDATE auth_sessions
        SET revoked_at = ?2, revoked_reason = ?3
        WHERE session_token_hash = ?1 AND revoked_at IS NULL
        "#,
    )
    .bind(session_token_hash)
    .bind(Utc::now().to_rfc3339())
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 判断会话是否仍在有效期内且未撤销。
pub fn is_session_active(record: &AuthSessionRecord) -> bool {
    if record.revoked_at.is_some() {
        return false;
    }
    record.expires_at > Utc::now().to_rfc3339()
}

impl From<&AuthSessionRecord> for AuthSessionSummary {
    fn from(record: &AuthSessionRecord) -> Self {
        Self {
            id: record.id.clone(),
            expires_at: record.expires_at.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::setup_test_db;

    async fn insert_admin(pool: &SqlitePool) -> i64 {
        let result = sqlx::query(
            r#"
            INSERT INTO users (username, password_hash)
            VALUES ('admin', 'hash')
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
        result.last_insert_rowid()
    }

    #[tokio::test]
    async fn admin_can_have_multiple_active_sessions() {
        let pool = setup_test_db().await;
        let user_id = insert_admin(&pool).await;

        let first = create_auth_session(
            &pool,
            CreateAuthSessionInput {
                user_id,
                client_ip: Some("127.0.0.1".to_string()),
                user_agent: Some("browser-a".to_string()),
            },
        )
        .await
        .unwrap();
        let second = create_auth_session(
            &pool,
            CreateAuthSessionInput {
                user_id,
                client_ip: Some("127.0.0.2".to_string()),
                user_agent: Some("browser-b".to_string()),
            },
        )
        .await
        .unwrap();

        assert_ne!(first.token, second.token);
        assert_ne!(first.record.id, second.record.id);
        assert!(is_session_active(&first.record));
        assert!(is_session_active(&second.record));
    }

    #[tokio::test]
    async fn revoke_one_session_does_not_revoke_other_sessions() {
        let pool = setup_test_db().await;
        let user_id = insert_admin(&pool).await;
        let first = create_auth_session(
            &pool,
            CreateAuthSessionInput {
                user_id,
                client_ip: None,
                user_agent: None,
            },
        )
        .await
        .unwrap();
        let second = create_auth_session(
            &pool,
            CreateAuthSessionInput {
                user_id,
                client_ip: None,
                user_agent: None,
            },
        )
        .await
        .unwrap();

        assert!(
            revoke_auth_session_by_token(&pool, &first.token, "logout")
                .await
                .unwrap()
        );

        let first_reloaded = get_auth_session_by_hash(&pool, &hash_session_token(&first.token))
            .await
            .unwrap()
            .unwrap();
        let second_reloaded = get_auth_session_by_hash(&pool, &hash_session_token(&second.token))
            .await
            .unwrap()
            .unwrap();
        assert!(!is_session_active(&first_reloaded));
        assert!(is_session_active(&second_reloaded));
    }
}
