//! 用户服务：用户初始化、密码校验与业务规则封装。

use crate::models::system_config;
use crate::security::safe_entry_rules::validate_safe_entry_value;
use crate::state::DbPool;
use anyhow::Result;
use bcrypt::{DEFAULT_COST, hash};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

const BOOTSTRAP_SECURITY_FILE: &str = "bootstrap-security.json";

/// 使用 bcrypt 算法计算给定密码的哈希值。
///
/// # 参数
/// - `password`: 需要被哈希的明文密码。
///
/// # 返回
/// - `Result<String, bcrypt::BcryptError>`: 如果成功，返回哈希后的密码字符串；
///   如果失败，返回 bcrypt 库的错误。
///
/// `DEFAULT_COST` 是 bcrypt 的成本因子，代表计算哈希的复杂度。
/// 较高的成本因子可以更好地抵御暴力破解，但也会消耗更多的计算资源。
pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    hash(password, DEFAULT_COST)
}

/// 安装脚本写入的一次性安全初始化载荷。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct BootstrapSecurity {
    username: String,
    password: String,
    safe_entry: String,
    #[serde(default)]
    password_complexity: bool,
}

/// 检查并初始化管理员用户（如果尚不存在）。
///
/// 生产新库必须由安装脚本生成的 bootstrap 文件初始化。开发模式允许
/// 自动创建 `admin/admin`，并关闭安全入口，方便本地直接启动。
pub async fn create_initial_admin_if_not_exists(vust_db: &DbPool) -> Result<()> {
    initialize_admin_if_not_exists(vust_db, &bootstrap_security_path(), cfg!(debug_assertions))
        .await
}

async fn initialize_admin_if_not_exists(
    vust_db: &DbPool,
    bootstrap_path: &Path,
    allow_dev_default: bool,
) -> Result<()> {
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(1) FROM users")
        .fetch_one(vust_db)
        .await?;

    if user_count > 0 {
        if bootstrap_path.exists() {
            fs::remove_file(bootstrap_path).map_err(|err| {
                anyhow::anyhow!(
                    "existing admin user preserved but failed to remove bootstrap security file {}: {}",
                    bootstrap_path.display(),
                    err
                )
            })?;
            info!(
                "Existing admin user and security settings preserved; bootstrap security file removed."
            );
        } else {
            debug!("Admin user already exists. Skipping bootstrap security initialization.");
        }
        return Ok(());
    }

    if !bootstrap_path.exists() {
        if allow_dev_default {
            initialize_admin_from_values(vust_db, "admin", "admin", "", false).await?;
            info!(
                "Initialized development default admin user admin/admin with safe entry disabled."
            );
            return Ok(());
        }
        return Err(anyhow::anyhow!(
            "missing bootstrap security file at {}: No such file or directory",
            bootstrap_path.display()
        ));
    }

    let raw = fs::read_to_string(bootstrap_path).map_err(|err| {
        anyhow::anyhow!(
            "missing bootstrap security file at {}: {}",
            bootstrap_path.display(),
            err
        )
    })?;
    let bootstrap: BootstrapSecurity = serde_json::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("invalid bootstrap security file: {}", err))?;
    validate_bootstrap(&bootstrap)?;

    initialize_admin_from_values(
        vust_db,
        &bootstrap.username,
        &bootstrap.password,
        bootstrap.safe_entry.trim(),
        bootstrap.password_complexity,
    )
    .await?;

    match fs::remove_file(bootstrap_path) {
        Ok(()) => info!(
            "Bootstrap security file consumed and removed: {}",
            bootstrap_path.display()
        ),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "admin initialized but failed to remove bootstrap security file {}: {}",
                bootstrap_path.display(),
                e
            ));
        }
    }

    info!("Successfully initialized bootstrap admin user.");
    Ok(())
}

async fn initialize_admin_from_values(
    vust_db: &DbPool,
    username: &str,
    password: &str,
    safe_entry: &str,
    password_complexity: bool,
) -> Result<()> {
    let password_hash = match hash_password(password) {
        Ok(hash) => hash,
        Err(e) => {
            error!("Failed to hash admin password: {}", e);
            return Err(anyhow::anyhow!("Failed to hash admin password"));
        }
    };

    let result = sqlx::query(
        r#"
        INSERT INTO users (username, password_hash)
        VALUES (?, ?)
        "#,
    )
    .bind(username)
    .bind(password_hash)
    .execute(vust_db)
    .await;

    if let Err(e) = result {
        error!("Failed to insert bootstrap admin user into DB: {}", e);
        return Err(anyhow::anyhow!("Failed to insert admin user: {}", e));
    }

    system_config::update_security_settings(vust_db, safe_entry, password_complexity).await?;
    Ok(())
}

/// 返回 bootstrap 文件路径。
pub fn bootstrap_security_path() -> PathBuf {
    crate::config::config_dir().join(BOOTSTRAP_SECURITY_FILE)
}

fn validate_bootstrap(bootstrap: &BootstrapSecurity) -> Result<()> {
    let username = bootstrap.username.trim();
    let valid_username = !username.is_empty()
        && username.len() <= 64
        && username
            .chars()
            .enumerate()
            .all(|(idx, ch)| ch.is_ascii_alphanumeric() || ch == '_' || (idx > 0 && ch == '-'));
    if !valid_username {
        return Err(anyhow::anyhow!(
            "bootstrap username must be 1-64 ASCII letters, digits, underscore, or hyphen"
        ));
    }
    if bootstrap.password.is_empty() {
        return Err(anyhow::anyhow!("bootstrap password must not be empty"));
    }
    if bootstrap.password.len() < 5 {
        return Err(anyhow::anyhow!(
            "bootstrap password must be at least 5 characters"
        ));
    }
    let safe_entry = bootstrap.safe_entry.trim();
    validate_safe_entry_value(safe_entry)
        .map_err(|err| anyhow::anyhow!("bootstrap safe entry is invalid: {}", err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BootstrapSecurity, initialize_admin_if_not_exists, validate_bootstrap};
    use crate::models::system_config;
    use crate::test_support::setup_test_db;
    use bcrypt::verify;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn valid_bootstrap() -> BootstrapSecurity {
        BootstrapSecurity {
            username: "vust".to_string(),
            password: "12345".to_string(),
            safe_entry: "Xm9Kp2Qs".to_string(),
            password_complexity: false,
        }
    }

    #[test]
    fn accepts_valid_bootstrap_security_payload() {
        assert!(validate_bootstrap(&valid_bootstrap()).is_ok());
    }

    #[test]
    fn rejects_short_bootstrap_password() {
        let mut bootstrap = valid_bootstrap();
        bootstrap.password = "1234".to_string();
        assert!(validate_bootstrap(&bootstrap).is_err());
    }

    #[test]
    fn rejects_reserved_safe_entry_prefix_in_bootstrap() {
        let mut bootstrap = valid_bootstrap();
        bootstrap.safe_entry = "api123456".to_string();
        assert!(validate_bootstrap(&bootstrap).is_err());
    }

    #[tokio::test]
    async fn debug_default_initializes_admin_without_safe_entry() {
        let pool = setup_test_db().await;
        let missing_bootstrap = unique_bootstrap_path("missing");
        initialize_admin_if_not_exists(&pool, &missing_bootstrap, true)
            .await
            .unwrap();

        let (username, password_hash): (String, String) =
            sqlx::query_as("SELECT username, password_hash FROM users ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(username, "admin");
        assert!(verify("admin", &password_hash).unwrap());
        assert_eq!(
            system_config::get_safe_entry_value(&pool).await.unwrap(),
            ""
        );
        assert!(
            !system_config::password_complexity_enabled(&pool)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn production_missing_bootstrap_is_rejected() {
        let pool = setup_test_db().await;
        let missing_bootstrap = unique_bootstrap_path("prod-missing");
        let err = initialize_admin_if_not_exists(&pool, &missing_bootstrap, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("missing bootstrap security file"));
    }

    #[tokio::test]
    async fn bootstrap_file_overrides_development_default() {
        let pool = setup_test_db().await;
        let bootstrap_path = unique_bootstrap_path("bootstrap");
        fs::create_dir_all(bootstrap_path.parent().unwrap()).unwrap();
        fs::write(
            &bootstrap_path,
            r#"{"username":"vust","password":"12345","safe_entry":"Xm9Kp2Qs","password_complexity":true}"#,
        )
        .unwrap();

        initialize_admin_if_not_exists(&pool, &bootstrap_path, true)
            .await
            .unwrap();

        let (username, password_hash): (String, String) =
            sqlx::query_as("SELECT username, password_hash FROM users ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(username, "vust");
        assert!(verify("12345", &password_hash).unwrap());
        assert_eq!(
            system_config::get_safe_entry_value(&pool).await.unwrap(),
            "Xm9Kp2Qs"
        );
        assert!(
            system_config::password_complexity_enabled(&pool)
                .await
                .unwrap()
        );
        assert!(!bootstrap_path.exists());
    }

    #[tokio::test]
    async fn existing_admin_and_security_are_preserved_when_bootstrap_exists() {
        let pool = setup_test_db().await;
        let initial_bootstrap = unique_bootstrap_path("existing-initial");
        initialize_admin_if_not_exists(&pool, &initial_bootstrap, true)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET username = 'existing-admin' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        system_config::update_security_settings(&pool, "KeepMe123", true)
            .await
            .unwrap();
        let original_password_hash: String =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO auth_sessions (id, session_token_hash, user_id, expires_at) VALUES ('session-1', 'token-1', 1, '2099-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO upgrade_plans (
                plan_id, target_version, component, scope, strategy, status,
                requested_by_user_id, requested_by, trace_id
            ) VALUES ('plan-1', '0.2.0', 'cluster', '{}', '{}', 'succeeded', 1, 'existing-admin', 'trace-1')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let overwrite_bootstrap = unique_bootstrap_path("existing-overwrite");
        fs::create_dir_all(overwrite_bootstrap.parent().unwrap()).unwrap();
        fs::write(
            &overwrite_bootstrap,
            r#"{"username":"replacement","password":"67890","safe_entry":"Replace123","password_complexity":false}"#,
        )
        .unwrap();

        initialize_admin_if_not_exists(&pool, &overwrite_bootstrap, false)
            .await
            .unwrap();

        let (user_id, username, password_hash): (i64, String, String) =
            sqlx::query_as("SELECT id, username, password_hash FROM users")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(user_id, 1);
        assert_eq!(username, "existing-admin");
        assert_eq!(password_hash, original_password_hash);
        assert_eq!(
            system_config::get_safe_entry_value(&pool).await.unwrap(),
            "KeepMe123"
        );
        assert!(
            system_config::password_complexity_enabled(&pool)
                .await
                .unwrap()
        );
        let plan_count: i64 = sqlx::query_scalar("SELECT COUNT(1) FROM upgrade_plans")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(plan_count, 1);
        let session_count: i64 = sqlx::query_scalar("SELECT COUNT(1) FROM auth_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(session_count, 1);
        assert!(!overwrite_bootstrap.exists());
    }

    #[tokio::test]
    async fn existing_user_is_not_reset_without_bootstrap() {
        let pool = setup_test_db().await;
        let missing_bootstrap = unique_bootstrap_path("existing");
        initialize_admin_if_not_exists(&pool, &missing_bootstrap, true)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET username = 'custom' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        initialize_admin_if_not_exists(&pool, &missing_bootstrap, true)
            .await
            .unwrap();

        let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(username, "custom");
    }

    fn unique_bootstrap_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join("vust-user-service-tests")
            .join(format!("{label}-{}-{nanos}.json", std::process::id()))
    }
}
