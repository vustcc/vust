//! 节点部署模型：`node_provisioning` 表的数据结构与基础写入方法。

use crate::state::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{Executor, FromRow, Sqlite};

/// 节点部署记录。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NodeProvisioningRecord {
    pub provisioning_id: String,
    pub node_id: String,
    pub deploy_method: String,
    pub ssh_addr: Option<String>,
    pub ssh_port: Option<i64>,
    pub ssh_user: Option<String>,
    pub ssh_auth_mode: Option<String>,
    pub ssh_password_ciphertext: Option<String>,
    pub ssh_private_key_ciphertext: Option<String>,
    pub ssh_private_key_passphrase_ciphertext: Option<String>,
    pub install_dir: String,
    pub systemd_service_name: String,
    pub expected_listen_port: Option<i64>,
    pub vust_url: Option<String>,
    pub revision: i64,
    pub validated_revision: Option<i64>,
    pub precheck_valid_until: Option<String>,
    pub last_deploy_task_id: Option<String>,
    pub last_deploy_result_status: Option<String>,
    pub last_deploy_error_summary: Option<String>,
    pub last_deploy_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 写入或更新节点部署记录。
pub async fn upsert_node_provisioning(
    pool: &DbPool,
    record: &NodeProvisioningRecord,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO node_provisioning (
            provisioning_id,
            node_id,
            deploy_method,
            ssh_addr,
            ssh_port,
            ssh_user,
            ssh_auth_mode,
            ssh_password_ciphertext,
            ssh_private_key_ciphertext,
            ssh_private_key_passphrase_ciphertext,
            install_dir,
            systemd_service_name,
            expected_listen_port,
            vust_url,
            revision,
            validated_revision,
            precheck_valid_until,
            last_deploy_task_id,
            last_deploy_result_status,
            last_deploy_error_summary,
            last_deploy_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(node_id) DO UPDATE SET
            deploy_method = excluded.deploy_method,
            ssh_addr = excluded.ssh_addr,
            ssh_port = excluded.ssh_port,
            ssh_user = excluded.ssh_user,
            ssh_auth_mode = excluded.ssh_auth_mode,
            ssh_password_ciphertext = excluded.ssh_password_ciphertext,
            ssh_private_key_ciphertext = excluded.ssh_private_key_ciphertext,
            ssh_private_key_passphrase_ciphertext = excluded.ssh_private_key_passphrase_ciphertext,
            install_dir = excluded.install_dir,
            systemd_service_name = excluded.systemd_service_name,
            expected_listen_port = excluded.expected_listen_port,
            vust_url = excluded.vust_url,
            revision = excluded.revision,
            validated_revision = excluded.validated_revision,
            precheck_valid_until = excluded.precheck_valid_until,
            last_deploy_task_id = excluded.last_deploy_task_id,
            last_deploy_result_status = excluded.last_deploy_result_status,
            last_deploy_error_summary = excluded.last_deploy_error_summary,
            last_deploy_at = excluded.last_deploy_at
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
    .bind(record.revision)
    .bind(record.validated_revision)
    .bind(&record.precheck_valid_until)
    .bind(&record.last_deploy_task_id)
    .bind(&record.last_deploy_result_status)
    .bind(&record.last_deploy_error_summary)
    .bind(&record.last_deploy_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// 按 `node_id` 读取节点部署记录。
pub async fn get_node_provisioning_by_node_id(
    pool: &DbPool,
    node_id: &str,
) -> sqlx::Result<Option<NodeProvisioningRecord>> {
    sqlx::query_as::<_, NodeProvisioningRecord>(
        r#"
        SELECT
            provisioning_id,
            node_id,
            deploy_method,
            ssh_addr,
            ssh_port,
            ssh_user,
            ssh_auth_mode,
            ssh_password_ciphertext,
            ssh_private_key_ciphertext,
            ssh_private_key_passphrase_ciphertext,
            install_dir,
            systemd_service_name,
            expected_listen_port,
            vust_url,
            revision,
            validated_revision,
            precheck_valid_until,
            last_deploy_task_id,
            last_deploy_result_status,
            last_deploy_error_summary,
            last_deploy_at,
            created_at,
            updated_at
        FROM node_provisioning
        WHERE node_id = ?
        LIMIT 1
        "#,
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await
}

/// 查询所有节点部署记录。
pub async fn list_node_provisioning(pool: &DbPool) -> sqlx::Result<Vec<NodeProvisioningRecord>> {
    sqlx::query_as::<_, NodeProvisioningRecord>(
        r#"
        SELECT
            provisioning_id,
            node_id,
            deploy_method,
            ssh_addr,
            ssh_port,
            ssh_user,
            ssh_auth_mode,
            ssh_password_ciphertext,
            ssh_private_key_ciphertext,
            ssh_private_key_passphrase_ciphertext,
            install_dir,
            systemd_service_name,
            expected_listen_port,
            vust_url,
            revision,
            validated_revision,
            precheck_valid_until,
            last_deploy_task_id,
            last_deploy_result_status,
            last_deploy_error_summary,
            last_deploy_at,
            created_at,
            updated_at
        FROM node_provisioning
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

/// 将预检结果绑定到指定部署配置 revision。
pub async fn mark_precheck_validated<'e, E>(
    executor: E,
    node_id: &str,
    revision: i64,
    valid_until: &str,
) -> sqlx::Result<bool>
where
    E: Executor<'e, Database = Sqlite>,
{
    let result = sqlx::query(
        r#"
        UPDATE node_provisioning
        SET validated_revision = ?, precheck_valid_until = ?
        WHERE node_id = ? AND revision = ?
        "#,
    )
    .bind(revision)
    .bind(valid_until)
    .bind(node_id)
    .bind(revision)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}
