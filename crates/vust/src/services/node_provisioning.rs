//! 节点部署服务：维护 `node_provisioning` 表。

use crate::crypto::encrypt_optional;
use crate::models::node_api_types::{NodeCreatePayload, NodeUpdatePayload};
use crate::models::node_provisioning::{
    NodeProvisioningRecord, get_node_provisioning_by_node_id, upsert_node_provisioning,
};
use crate::state::DbPool;
use chrono::Utc;
use sqlx::{Executor, Sqlite};

fn provisioning_id(node_id: &str) -> String {
    format!("provisioning-{node_id}")
}

fn map_crypto_err(err: crate::crypto::CryptoError) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(err))
}

fn parse_port(value: Option<&str>) -> Option<i64> {
    value.and_then(|item| item.parse::<i64>().ok())
}

/// 部署完成后需要写入 `node_provisioning` 的摘要信息。
#[derive(Debug, Clone)]
pub struct MarkDeployedInput {
    pub node_id: String,
    pub deploy_method: String,
    pub result_status: String,
    pub ssh_addr: String,
    pub ssh_port: String,
    pub ssh_user: String,
    pub auth_mode: String,
    pub install_dir: String,
    pub service_port: String,
    pub vust_url: String,
}

/// 在部署成功后，将节点部署态写入新表。
pub async fn mark_deployed(pool: &DbPool, input: &MarkDeployedInput) -> sqlx::Result<()> {
    let now = Utc::now().to_rfc3339();
    let existing = get_node_provisioning_by_node_id(pool, &input.node_id).await?;
    let (pwd, key, passphrase) = match existing.as_ref() {
        Some(rec) => (
            rec.ssh_password_ciphertext.clone(),
            rec.ssh_private_key_ciphertext.clone(),
            rec.ssh_private_key_passphrase_ciphertext.clone(),
        ),
        None => (None, None, None),
    };

    let record = NodeProvisioningRecord {
        provisioning_id: provisioning_id(&input.node_id),
        node_id: input.node_id.clone(),
        deploy_method: input.deploy_method.clone(),
        ssh_addr: Some(input.ssh_addr.clone()),
        ssh_port: input.ssh_port.parse::<i64>().ok(),
        ssh_user: Some(input.ssh_user.clone()),
        ssh_auth_mode: Some(input.auth_mode.clone()),
        ssh_password_ciphertext: pwd,
        ssh_private_key_ciphertext: key,
        ssh_private_key_passphrase_ciphertext: passphrase,
        install_dir: input.install_dir.clone(),
        systemd_service_name: "vust-agent.service".to_string(),
        expected_listen_port: input.service_port.parse::<i64>().ok(),
        vust_url: Some(input.vust_url.trim_end_matches('/').to_string()),
        revision: existing.as_ref().map_or(1, |record| record.revision),
        validated_revision: existing
            .as_ref()
            .and_then(|record| record.validated_revision),
        precheck_valid_until: existing
            .as_ref()
            .and_then(|record| record.precheck_valid_until.clone()),
        last_deploy_task_id: None,
        last_deploy_result_status: Some(input.result_status.clone()),
        last_deploy_error_summary: None,
        last_deploy_at: Some(now.clone()),
        created_at: now.clone(),
        updated_at: now,
    };
    upsert_node_provisioning(pool, &record).await
}

/// 记录部署/运维操作结果，供升级、修复、卸载等流程复用。
pub async fn record_operation_result(
    pool: &DbPool,
    node_id: &str,
    deploy_method: &str,
    result_status: &str,
    error_summary: Option<String>,
) -> sqlx::Result<()> {
    let now = Utc::now().to_rfc3339();
    let existing = get_node_provisioning_by_node_id(pool, node_id).await?;
    let mut record = existing.unwrap_or_else(|| NodeProvisioningRecord {
        provisioning_id: provisioning_id(node_id),
        node_id: node_id.to_string(),
        deploy_method: deploy_method.to_string(),
        ssh_addr: None,
        ssh_port: None,
        ssh_user: None,
        ssh_auth_mode: None,
        ssh_password_ciphertext: None,
        ssh_private_key_ciphertext: None,
        ssh_private_key_passphrase_ciphertext: None,
        install_dir: "/opt/vust".to_string(),
        systemd_service_name: "vust-agent.service".to_string(),
        expected_listen_port: None,
        vust_url: None,
        revision: 1,
        validated_revision: None,
        precheck_valid_until: None,
        last_deploy_task_id: None,
        last_deploy_result_status: None,
        last_deploy_error_summary: None,
        last_deploy_at: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    });
    record.deploy_method = deploy_method.to_string();
    record.last_deploy_result_status = Some(result_status.to_string());
    record.last_deploy_error_summary = error_summary;
    record.last_deploy_at = Some(now);
    upsert_node_provisioning(pool, &record).await
}

/// 根据节点创建载荷直接写入部署记录。
pub async fn create_provisioning_from_payload<'e, E>(
    executor: E,
    node_id: &str,
    payload: &NodeCreatePayload,
) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let now = Utc::now().to_rfc3339();
    let record = NodeProvisioningRecord {
        provisioning_id: provisioning_id(node_id),
        node_id: node_id.to_string(),
        deploy_method: "inventory_create".to_string(),
        ssh_addr: payload.addr.clone(),
        ssh_port: parse_port(payload.port.as_deref()),
        ssh_user: payload.user.clone(),
        ssh_auth_mode: payload.auth_mode.clone(),
        ssh_password_ciphertext: encrypt_optional(payload.pwd.clone()).map_err(map_crypto_err)?,
        ssh_private_key_ciphertext: encrypt_optional(payload.private_key.clone())
            .map_err(map_crypto_err)?,
        ssh_private_key_passphrase_ciphertext: encrypt_optional(
            payload.private_key_passphrase.clone(),
        )
        .map_err(map_crypto_err)?,
        install_dir: {
            let parent = payload
                .install_dir
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("/opt");
            let parent_trimmed = parent.trim_end_matches('/');
            if parent_trimmed.is_empty() {
                "/vust".to_string()
            } else {
                format!("{}/vust", parent_trimmed)
            }
        },
        systemd_service_name: "vust-agent.service".to_string(),
        expected_listen_port: parse_port(payload.service_port.as_deref()),
        vust_url: None,
        revision: 1,
        validated_revision: None,
        precheck_valid_until: None,
        last_deploy_task_id: None,
        last_deploy_result_status: None,
        last_deploy_error_summary: None,
        last_deploy_at: None,
        created_at: now.clone(),
        updated_at: now,
    };
    let query = sqlx::query(
        r#"
        INSERT INTO node_provisioning (
            provisioning_id, node_id, deploy_method, ssh_addr, ssh_port, ssh_user,
            ssh_auth_mode, ssh_password_ciphertext, ssh_private_key_ciphertext,
            ssh_private_key_passphrase_ciphertext, install_dir, systemd_service_name,
            expected_listen_port, vust_url, last_deploy_task_id,
            revision, validated_revision, precheck_valid_until,
            last_deploy_result_status, last_deploy_error_summary, last_deploy_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&record.provisioning_id)
    .bind(&record.node_id)
    .bind(&record.deploy_method)
    .bind(&record.ssh_addr)
    .bind(record.ssh_port)
    .bind(&record.ssh_user)
    .bind(&record.ssh_auth_mode)
    .bind(&record.ssh_password_ciphertext)
    .bind(&record.ssh_private_key_ciphertext)
    .bind(&record.ssh_private_key_passphrase_ciphertext)
    .bind(&record.install_dir)
    .bind(&record.systemd_service_name)
    .bind(record.expected_listen_port)
    .bind(&record.vust_url)
    .bind(&record.last_deploy_task_id)
    .bind(record.revision)
    .bind(record.validated_revision)
    .bind(&record.precheck_valid_until)
    .bind(&record.last_deploy_result_status)
    .bind(&record.last_deploy_error_summary)
    .bind(&record.last_deploy_at);
    query.execute(executor).await?;
    Ok(())
}

/// 根据节点更新载荷直接更新部署记录。
pub async fn update_provisioning_from_payload<'e, E>(
    executor: E,
    node_id: &str,
    payload: &NodeUpdatePayload,
) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let connection_changed = payload.addr.is_some()
        || payload.port.is_some()
        || payload.user.is_some()
        || payload.pwd.is_some()
        || payload.private_key.is_some()
        || payload.private_key_passphrase.is_some()
        || payload.auth_mode.is_some()
        || payload.install_dir.is_some()
        || payload.service_port.is_some()
        || payload.vust_url.is_some();
    let password = encrypt_optional(payload.pwd.clone()).map_err(map_crypto_err)?;
    let private_key = encrypt_optional(payload.private_key.clone()).map_err(map_crypto_err)?;
    let passphrase =
        encrypt_optional(payload.private_key_passphrase.clone()).map_err(map_crypto_err)?;
    let vust_url = payload
        .vust_url
        .as_ref()
        .map(|value| value.trim_end_matches('/').to_string());
    let result = sqlx::query(
        r#"
        UPDATE node_provisioning
        SET
            deploy_method = 'inventory_update',
            ssh_addr = COALESCE(?, ssh_addr),
            ssh_port = COALESCE(?, ssh_port),
            ssh_user = COALESCE(?, ssh_user),
            ssh_auth_mode = COALESCE(?, ssh_auth_mode),
            ssh_password_ciphertext = CASE WHEN ? THEN ? ELSE ssh_password_ciphertext END,
            ssh_private_key_ciphertext = CASE WHEN ? THEN ? ELSE ssh_private_key_ciphertext END,
            ssh_private_key_passphrase_ciphertext = CASE WHEN ? THEN ? ELSE ssh_private_key_passphrase_ciphertext END,
            install_dir = COALESCE(?, install_dir),
            expected_listen_port = COALESCE(?, expected_listen_port),
            vust_url = COALESCE(?, vust_url),
            revision = revision + ?,
            validated_revision = CASE WHEN ? THEN NULL ELSE validated_revision END,
            precheck_valid_until = CASE WHEN ? THEN NULL ELSE precheck_valid_until END
        WHERE node_id = ?
        "#,
    )
    .bind(&payload.addr)
    .bind(parse_port(payload.port.as_deref()))
    .bind(&payload.user)
    .bind(&payload.auth_mode)
    .bind(payload.pwd.is_some())
    .bind(password)
    .bind(payload.private_key.is_some())
    .bind(private_key)
    .bind(payload.private_key_passphrase.is_some())
    .bind(passphrase)
    .bind(&payload.install_dir)
    .bind(parse_port(payload.service_port.as_deref()))
    .bind(vust_url)
    .bind(i64::from(connection_changed))
    .bind(connection_changed)
    .bind(connection_changed)
    .bind(node_id)
    .execute(executor)
    .await?;
    if result.rows_affected() != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

/// 同步节点本地配置中的主控回连地址。
pub async fn update_node_vust_url(
    pool: &DbPool,
    node_id: &str,
    vust_url: &str,
) -> sqlx::Result<()> {
    let trimmed = vust_url.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE node_provisioning
        SET
            vust_url = ?,
            revision = revision + 1,
            validated_revision = NULL,
            precheck_valid_until = NULL
        WHERE node_id = ?
        "#,
    )
    .bind(trimmed)
    .bind(node_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{create_provisioning_from_payload, update_provisioning_from_payload};
    use crate::models::node_api_types::{NodeCreatePayload, NodeUpdatePayload};
    use crate::models::node_provisioning::{
        get_node_provisioning_by_node_id, mark_precheck_validated,
    };
    use crate::models::nodes::{NodeStatus, get_node_by_id};
    use crate::services::node_inventory::create_node_from_payload;
    use crate::test_support::setup_test_db;
    use uuid::Uuid;

    fn valid_payload() -> NodeCreatePayload {
        NodeCreatePayload {
            agent_id: None,
            name: Some("worker-transaction".to_string()),
            group_id: Some("default".to_string()),
            description: None,
            addr: Some("192.0.2.10".to_string()),
            port: Some("22".to_string()),
            user: Some("root".to_string()),
            pwd: None,
            private_key: Some("~/.ssh/id_ed25519".to_string()),
            private_key_passphrase: None,
            auth_mode: Some("key".to_string()),
            status: None,
            version: None,
            tags: Some(vec!["lab".to_string()]),
            metadata: None,
            install_dir: Some("/opt".to_string()),
            service_port: Some("7311".to_string()),
            sync_enabled: None,
        }
    }

    #[tokio::test]
    async fn failed_provisioning_write_rolls_back_node_creation() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let payload = valid_payload();
        let mut transaction = pool.begin().await.unwrap();
        create_node_from_payload(&mut *transaction, &node_id, &payload, NodeStatus::Draft)
            .await
            .unwrap();
        create_provisioning_from_payload(&mut *transaction, &node_id, &payload)
            .await
            .unwrap();
        assert!(
            create_provisioning_from_payload(&mut *transaction, &node_id, &payload)
                .await
                .is_err()
        );
        transaction.rollback().await.unwrap();

        assert!(get_node_by_id(&pool, &node_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn connection_update_increments_revision_and_invalidates_precheck() {
        let pool = setup_test_db().await;
        let node_id = Uuid::new_v4().to_string();
        let payload = valid_payload();
        let mut transaction = pool.begin().await.unwrap();
        create_node_from_payload(&mut *transaction, &node_id, &payload, NodeStatus::Draft)
            .await
            .unwrap();
        create_provisioning_from_payload(&mut *transaction, &node_id, &payload)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        assert!(
            mark_precheck_validated(&pool, &node_id, 1, "2099-01-01T00:00:00Z")
                .await
                .unwrap()
        );

        update_provisioning_from_payload(
            &pool,
            &node_id,
            &NodeUpdatePayload {
                agent_id: None,
                name: None,
                group_id: None,
                description: None,
                addr: None,
                port: None,
                user: None,
                pwd: None,
                private_key: None,
                private_key_passphrase: None,
                auth_mode: None,
                status: None,
                version: None,
                tags: None,
                metadata: None,
                install_dir: None,
                service_port: Some("7312".to_string()),
                sync_enabled: None,
                vust_url: None,
            },
        )
        .await
        .unwrap();

        let record = get_node_provisioning_by_node_id(&pool, &node_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 2);
        assert_eq!(record.validated_revision, None);
        assert_eq!(record.precheck_valid_until, None);
    }
}
