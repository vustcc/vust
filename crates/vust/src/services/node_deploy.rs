//! 节点部署服务：通过 SSH 下发二进制、配置与 systemd 服务。

use crate::config;
use crate::crypto::decrypt_optional;
use crate::models::node_provisioning::{get_node_provisioning_by_node_id, mark_precheck_validated};
use crate::models::node_sessions::{close_active_sessions, get_active_session_by_node_id};
use crate::runtime_config;
use crate::services::node_enrollment::{issue_enrollment_token, revoke_failed_enrollment};
use crate::services::node_operation_guard::{
    NodeOperation as GuardedNodeOperation, assert_node_operation,
};
use crate::services::node_precheck::inspect_remote_agent;
use crate::services::node_precheck::{NodePrecheckInput, precheck_node};
use crate::services::node_provisioning;
use crate::services::node_state_machine::{
    transition_to_awaiting_registration, transition_to_deploy_failed, transition_to_deploying,
    transition_to_retired,
};
use crate::services::node_target_guard::{
    detect_vust_service, node_conflict_message, open_ssh_session,
};
use crate::services::runtime_metrics;
use crate::state::DbPool;
use crate::types::{ApiError, ApiResult, new_uuid_v7};
use chrono::{Local, Utc};
use serde::Deserialize;
use serde::Serialize;
use ssh2::Session;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration as StdDuration, Instant};

const REGISTRATION_TIMEOUT: StdDuration = StdDuration::from_secs(90);

/// 部署执行器发送给持久任务写入器的进度事件。
#[derive(Debug, Clone)]
pub enum DeployProgressEvent {
    Progress(u32),
    Snapshot {
        logs: Vec<String>,
        progress_percent: Option<u32>,
        is_finished: bool,
        error: Option<String>,
    },
    Append {
        message: String,
        progress_percent: u32,
        is_finished: bool,
        error: Option<String>,
    },
}

/// 单个持久部署任务的进度发送端。
pub type DeployProgressSender = tokio::sync::mpsc::UnboundedSender<DeployProgressEvent>;
type BlockingDeployOutput = (Vec<String>, Option<String>);
type BlockingDeployResult = Result<BlockingDeployOutput, (ApiError, Vec<String>)>;

/// 带进度汇报的 Reader 包装器，用于在 SFTP 传输时统计已发送的字节数与百分比。
pub struct ProgressReader<R, F>
where
    R: Read,
    F: FnMut(u64, u64),
{
    inner: R,
    total: u64,
    current: u64,
    on_progress: F,
}

impl<R, F> ProgressReader<R, F>
where
    R: Read,
    F: FnMut(u64, u64),
{
    pub fn new(inner: R, total: u64, on_progress: F) -> Self {
        Self {
            inner,
            total,
            current: 0,
            on_progress,
        }
    }
}

impl<R, F> Read for ProgressReader<R, F>
where
    R: Read,
    F: FnMut(u64, u64),
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.current += n as u64;
            (self.on_progress)(self.current, self.total);
        }
        Ok(n)
    }
}

const DEFAULT_SSH_PORT: &str = crate::config::DEFAULT_SSH_PORT;
const DEFAULT_LISTEN_PORT: &str = crate::config::DEFAULT_AGENT_PORT;
const DEFAULT_INSTALL_DIR: &str = crate::config::DEFAULT_PRODUCTION_HOME;

/// 节点部署接口请求载荷。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDeployPayload {
    pub listen_addr: Option<String>,
    pub vust_url: Option<String>,
}

/// 远程部署所需的完整参数集合。
#[derive(Debug)]
pub struct NodeDeployInput {
    pub agent_id: String,
    pub enrollment_id: String,
    pub addr: String,
    pub port: String,
    pub user: String,
    pub auth_mode: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub private_key_passphrase: Option<String>,
    pub install_dir: String,
    pub service_port: String,
    pub listen_addr: String,
    pub vust_url: String,
    pub enrollment_token: String,
    pub allow_existing_agent: bool,
}

/// 部署完成后的 API 地址与执行日志。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDeployResult {
    pub enrollment_id: String,
    pub enrollment_token: String,
    pub logs: Vec<String>,
    #[serde(skip)]
    pub rollback_artifact: Option<String>,
}

/// 部署失败时携带错误与过程日志。
#[derive(Debug)]
pub struct NodeDeployError {
    pub error: ApiError,
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum NodeOperation {
    Deploy,
    Upgrade,
    Repair,
}

impl NodeOperation {
    fn deploy_method(self) -> &'static str {
        match self {
            Self::Deploy => "ssh_push",
            Self::Upgrade => "ssh_upgrade",
            Self::Repair => "ssh_repair",
        }
    }

    fn success_status(self) -> &'static str {
        match self {
            Self::Deploy => "prepared",
            Self::Upgrade => "upgraded",
            Self::Repair => "repaired",
        }
    }
}

/// 从数据库读取节点信息并执行远程部署流程。
pub async fn deploy_node(
    pool: &DbPool,
    node_id: &str,
    payload: NodeDeployPayload,
    progress_sender: Option<DeployProgressSender>,
) -> ApiResult<()> {
    operate_node(
        pool,
        node_id,
        payload,
        NodeOperation::Deploy,
        progress_sender,
    )
    .await
}

/// 执行节点升级流程（复用统一部署管线）。
pub async fn upgrade_node(
    pool: &DbPool,
    node_id: &str,
    payload: NodeDeployPayload,
) -> ApiResult<()> {
    operate_node(pool, node_id, payload, NodeOperation::Upgrade, None).await
}

/// 执行节点修复流程（复用统一部署管线）。
pub async fn repair_node(
    pool: &DbPool,
    node_id: &str,
    payload: NodeDeployPayload,
    progress_sender: Option<DeployProgressSender>,
) -> ApiResult<()> {
    operate_node(
        pool,
        node_id,
        payload,
        NodeOperation::Repair,
        progress_sender,
    )
    .await
}

async fn operate_node(
    pool: &DbPool,
    node_id: &str,
    payload: NodeDeployPayload,
    operation: NodeOperation,
    progress_sender: Option<DeployProgressSender>,
) -> ApiResult<()> {
    let guarded_operation = match operation {
        NodeOperation::Deploy | NodeOperation::Upgrade => GuardedNodeOperation::Deploy,
        NodeOperation::Repair => GuardedNodeOperation::Reprovision,
    };
    assert_node_operation(
        pool,
        node_id,
        guarded_operation,
        matches!(operation, NodeOperation::Deploy | NodeOperation::Repair),
    )
    .await?;
    let record = get_node_provisioning_by_node_id(pool, node_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("node deployment record does not exist".to_string()))?;
    let provisioning_revision = record.revision;
    let agent_id = node_id.to_string();
    let addr = record
        .ssh_addr
        .ok_or_else(|| ApiError::BadRequest("node address must not be empty".to_string()))?;
    let user = record
        .ssh_user
        .ok_or_else(|| ApiError::BadRequest("SSH user must not be empty".to_string()))?;
    let ssh_port = record
        .ssh_port
        .map(|value| value.to_string())
        .unwrap_or_else(|| DEFAULT_SSH_PORT.to_string());
    let service_port = record
        .expected_listen_port
        .map(|value| value.to_string())
        .unwrap_or_else(|| DEFAULT_LISTEN_PORT.to_string());
    let install_dir = if record.install_dir.is_empty() {
        DEFAULT_INSTALL_DIR.to_string()
    } else {
        record.install_dir
    };
    let auth_mode = record
        .ssh_auth_mode
        .unwrap_or_else(|| "password".to_string());
    let vust_url = resolve_vust_url_override(payload.vust_url.as_deref())?;
    let listen_addr = payload
        .listen_addr
        .unwrap_or_else(|| format!("{}:{service_port}", crate::config::DEFAULT_AGENT_LISTEN_IP));

    let password = decrypt_optional(record.ssh_password_ciphertext).map_err(|_| {
        ApiError::Internal("failed to decrypt SSH password; check key configuration".to_string())
    })?;
    let private_key = decrypt_optional(record.ssh_private_key_ciphertext).map_err(|_| {
        ApiError::Internal("failed to decrypt SSH private key; check key configuration".to_string())
    })?;
    let private_key_passphrase = decrypt_optional(record.ssh_private_key_passphrase_ciphertext)
        .map_err(|_| {
            ApiError::Internal(
                "failed to decrypt SSH private key passphrase; check key configuration".to_string(),
            )
        })?;
    let install_parent = Path::new(&install_dir)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("/opt")
        .to_string();
    let expected_node_id = if matches!(operation, NodeOperation::Deploy) {
        None
    } else {
        Some(node_id.to_string())
    };
    let precheck = precheck_node(
        pool,
        NodePrecheckInput {
            addr: addr.clone(),
            port: Some(ssh_port.clone()),
            user: user.clone(),
            auth_mode: Some(auth_mode.clone()),
            password: password.clone(),
            private_key: private_key.clone(),
            private_key_passphrase: private_key_passphrase.clone(),
            service_port: Some(service_port.clone()),
            install_dir: Some(install_parent),
            vust_url: Some(vust_url.clone()),
            expected_node_id,
        },
    )
    .await?;
    if !precheck.passed {
        return Err(ApiError::conflict(
            vust_contracts::api::ErrorCode::NodePrecheckFailed,
            precheck.agent_status.message,
        ));
    }
    let precheck_valid_until = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    if !mark_precheck_validated(pool, node_id, provisioning_revision, &precheck_valid_until).await?
    {
        return Err(ApiError::conflict(
            vust_contracts::api::ErrorCode::NodePrecheckStale,
            "node provisioning changed after precheck",
        ));
    }

    transition_to_deploying(pool, node_id).await?;
    close_active_sessions(pool, node_id, "redeploy_requested", "closed").await?;
    let _ = node_provisioning::record_operation_result(
        pool,
        node_id,
        operation.deploy_method(),
        "running",
        None,
    )
    .await;
    let issued = issue_enrollment_token(pool, &agent_id).await?;

    let input = NodeDeployInput {
        agent_id: agent_id.clone(),
        enrollment_id: issued.enrollment_id.clone(),
        addr: addr.clone(),
        port: ssh_port.clone(),
        user: user.clone(),
        auth_mode: auth_mode.clone(),
        password: password.clone(),
        private_key: private_key.clone(),
        private_key_passphrase: private_key_passphrase.clone(),
        install_dir: install_dir.clone(),
        service_port: service_port.clone(),
        listen_addr,
        vust_url: vust_url.clone(),
        enrollment_token: issued.token.clone(),
        allow_existing_agent: !matches!(operation, NodeOperation::Deploy),
    };

    let registration_started_at = Utc::now();
    let deploy_result = match deploy_node_with(input, progress_sender.clone()).await {
        Ok(result) => result,
        Err(err) => {
            runtime_metrics::record_deploy_result(false);
            let _ = revoke_failed_enrollment(pool, &issued.enrollment_id).await;
            let _ = transition_to_deploy_failed(pool, node_id).await;
            let _ = node_provisioning::record_operation_result(
                pool,
                node_id,
                operation.deploy_method(),
                "failed",
                Some(format!("{:?}", err.error)),
            )
            .await;
            return Err(err.error);
        }
    };
    let post_deploy_result: ApiResult<()> = async {
        node_provisioning::mark_deployed(
            pool,
            &node_provisioning::MarkDeployedInput {
                node_id: node_id.to_string(),
                deploy_method: operation.deploy_method().to_string(),
                result_status: operation.success_status().to_string(),
                ssh_addr: addr.clone(),
                ssh_port: ssh_port.clone(),
                ssh_user: user.clone(),
                auth_mode: auth_mode.clone(),
                install_dir: install_dir.clone(),
                service_port: service_port.clone(),
                vust_url: vust_url.clone(),
            },
        )
        .await?;
        transition_to_awaiting_registration(pool, node_id).await?;
        wait_for_agent_registration(
            pool,
            node_id,
            &issued.enrollment_id,
            registration_started_at,
            progress_sender,
        )
        .await
    }
    .await;
    if let Err(error) = post_deploy_result {
        let _ = rollback_remote_deployment(
            &NodeDeployInput {
                agent_id: agent_id.clone(),
                enrollment_id: issued.enrollment_id.clone(),
                addr: addr.clone(),
                port: ssh_port.clone(),
                user: user.clone(),
                auth_mode: auth_mode.clone(),
                password: password.clone(),
                private_key: private_key.clone(),
                private_key_passphrase: private_key_passphrase.clone(),
                install_dir: install_dir.clone(),
                service_port: service_port.clone(),
                listen_addr: String::new(),
                vust_url: vust_url.clone(),
                enrollment_token: String::new(),
                allow_existing_agent: !matches!(operation, NodeOperation::Deploy),
            },
            deploy_result.rollback_artifact.as_deref(),
        )
        .await;
        runtime_metrics::record_deploy_result(false);
        let _ = revoke_failed_enrollment(pool, &issued.enrollment_id).await;
        let _ = transition_to_deploy_failed(pool, node_id).await;
        let _ = node_provisioning::record_operation_result(
            pool,
            node_id,
            operation.deploy_method(),
            "failed",
            Some(error.message.to_string()),
        )
        .await;
        return Err(error);
    }
    finalize_remote_deployment(
        &NodeDeployInput {
            agent_id,
            enrollment_id: issued.enrollment_id,
            addr,
            port: ssh_port,
            user,
            auth_mode,
            password,
            private_key,
            private_key_passphrase,
            install_dir,
            service_port,
            listen_addr: String::new(),
            vust_url,
            enrollment_token: String::new(),
            allow_existing_agent: !matches!(operation, NodeOperation::Deploy),
        },
        deploy_result.rollback_artifact.as_deref(),
    )
    .await?;
    runtime_metrics::record_deploy_result(true);
    Ok(())
}

/// 等待本次 enrollment 被使用且节点建立有效租约，然后结束现有部署进度。
pub async fn wait_for_agent_registration(
    pool: &DbPool,
    node_id: &str,
    enrollment_id: &str,
    registered_after: chrono::DateTime<Utc>,
    progress_sender: Option<DeployProgressSender>,
) -> ApiResult<()> {
    update_registration_progress(
        &progress_sender,
        98,
        "Waiting for Agent registration and valid lease",
        false,
        None,
    );
    let started = Instant::now();
    loop {
        let enrollment_status = sqlx::query_scalar::<_, String>(
            "SELECT token_status FROM node_enrollments WHERE enrollment_id = ?",
        )
        .bind(enrollment_id)
        .fetch_optional(pool)
        .await?;
        let session = get_active_session_by_node_id(pool, node_id).await?;
        let now = Utc::now();
        if registration_is_complete(
            enrollment_status.as_deref(),
            session.as_ref(),
            node_id,
            registered_after,
            now,
        ) {
            update_registration_progress(
                &progress_sender,
                100,
                "Agent registration completed and lease is active",
                true,
                None,
            );
            return Ok(());
        }
        if started.elapsed() >= REGISTRATION_TIMEOUT {
            let message = "Agent registration timed out before a valid lease was established";
            update_registration_progress(
                &progress_sender,
                98,
                message,
                true,
                Some(message.to_string()),
            );
            return Err(ApiError::conflict(
                vust_contracts::api::ErrorCode::NodeDeployFailed,
                message,
            ));
        }
        tokio::time::sleep(StdDuration::from_secs(1)).await;
    }
}

fn registration_is_complete(
    enrollment_status: Option<&str>,
    session: Option<&crate::models::node_sessions::NodeSessionRecord>,
    node_id: &str,
    registered_after: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
) -> bool {
    enrollment_status == Some("used")
        && session.is_some_and(|session| {
            session.agent_id == node_id
                && chrono::DateTime::parse_from_rfc3339(&session.registered_at)
                    .ok()
                    .is_some_and(|value| value.with_timezone(&Utc) >= registered_after)
                && chrono::DateTime::parse_from_rfc3339(&session.lease_expires_at)
                    .ok()
                    .is_some_and(|value| value.with_timezone(&Utc) > now)
        })
}

fn update_registration_progress(
    progress_sender: &Option<DeployProgressSender>,
    progress_percent: u32,
    message: &str,
    is_finished: bool,
    error: Option<String>,
) {
    if let Some(sender) = progress_sender {
        let _ = sender.send(DeployProgressEvent::Append {
            message: message.to_string(),
            progress_percent,
            is_finished,
            error,
        });
    }
}

/// 退役节点：关闭活跃会话并切换状态。
pub async fn retire_node(pool: &DbPool, node_id: &str) -> ApiResult<()> {
    assert_node_operation(pool, node_id, GuardedNodeOperation::Retire, false).await?;
    close_active_sessions(pool, node_id, "retire_requested", "closed").await?;
    transition_to_retired(pool, node_id).await?;
    sqlx::query(
        r#"
        UPDATE nodes
        SET
            schedulable = 0,
            retired_at = COALESCE(retired_at, ?)
        WHERE node_id = ?
        "#,
    )
    .bind(Utc::now().to_rfc3339())
    .bind(node_id)
    .execute(pool)
    .await?;
    let _ =
        node_provisioning::record_operation_result(pool, node_id, "manual_retire", "retired", None)
            .await;
    Ok(())
}

/// 卸载节点：通过 SSH 清理远程服务与文件后退役。
pub async fn uninstall_node(
    pool: &DbPool,
    node_id: &str,
    ignore_active_task: bool,
) -> ApiResult<()> {
    assert_node_operation(
        pool,
        node_id,
        GuardedNodeOperation::Uninstall,
        ignore_active_task,
    )
    .await?;
    let _ =
        node_provisioning::record_operation_result(pool, node_id, "ssh_uninstall", "running", None)
            .await;
    let record = get_node_provisioning_by_node_id(pool, node_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("node deployment record does not exist".to_string()))?;
    let addr = record
        .ssh_addr
        .ok_or_else(|| ApiError::BadRequest("node address must not be empty".to_string()))?;
    let user = record
        .ssh_user
        .ok_or_else(|| ApiError::BadRequest("SSH user must not be empty".to_string()))?;
    let ssh_port = record
        .ssh_port
        .map(|value| value.to_string())
        .unwrap_or_else(|| DEFAULT_SSH_PORT.to_string());
    let auth_mode = record
        .ssh_auth_mode
        .unwrap_or_else(|| "password".to_string());
    let install_dir = if record.install_dir.is_empty() {
        DEFAULT_INSTALL_DIR.to_string()
    } else {
        record.install_dir
    };
    let password = decrypt_optional(record.ssh_password_ciphertext).map_err(|_| {
        ApiError::Internal("failed to decrypt SSH password; check key configuration".to_string())
    })?;
    let private_key = decrypt_optional(record.ssh_private_key_ciphertext).map_err(|_| {
        ApiError::Internal("failed to decrypt SSH private key; check key configuration".to_string())
    })?;
    let private_key_passphrase = decrypt_optional(record.ssh_private_key_passphrase_ciphertext)
        .map_err(|_| {
            ApiError::Internal(
                "failed to decrypt SSH private key passphrase; check key configuration".to_string(),
            )
        })?;

    let uninstall_result = tokio::task::spawn_blocking(move || -> ApiResult<()> {
        let session = open_ssh_session(
            &addr,
            Some(&ssh_port),
            &user,
            Some(&auth_mode),
            password.as_deref(),
            private_key.as_deref(),
            private_key_passphrase.as_deref(),
        )?;
        let use_sudo = user != "root";
        let install_base = install_dir.trim_end_matches('/').to_string();

        // 判定该目录是否可以被完全删除。如果是系统常用根路径或共享目录，则仅清理子目录以防误删。
        let forbidden_dirs = [
            "",
            "/",
            "/usr",
            "/usr/local",
            "/usr/bin",
            "/usr/sbin",
            "/usr/lib",
            "/usr/share",
            "/usr/include",
            "/var",
            "/var/lib",
            "/var/log",
            "/var/run",
            "/var/tmp",
            "/opt",
            "/opt/bin",
            "/etc",
            "/etc/systemd",
            "/etc/init.d",
            "/home",
            "/root",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",
            "/boot",
            "/dev",
            "/proc",
            "/sys",
            "/tmp",
            "/mnt",
            "/media",
            "/srv",
            "/run",
        ];

        let is_system_dir = forbidden_dirs.contains(&install_base.as_str());

        let cleanup_command = if is_system_dir {
            format!(
                "{sudo}rm -rf {install_base}/agent {install_base}/config {install_base}/database {install_base}/logs {install_base}/log {install_base}/run; \
                {sudo}rmdir {install_base} >/dev/null 2>&1 || true",
                sudo = sudo_prefix(use_sudo),
                install_base = shell_escape(&install_base)
            )
        } else {
            format!(
                "{sudo}rm -rf {install_base}",
                sudo = sudo_prefix(use_sudo),
                install_base = shell_escape(&install_base)
            )
        };

        let command = format!(
            "{sudo}systemctl disable --now vust-agent >/dev/null 2>&1 || true; \
            {sudo}rm -f /etc/systemd/system/vust-agent.service \
            /usr/local/bin/vust-agent /usr/bin/vust-agent /usr/local/bin/vustctl /usr/bin/vustctl; \
            {cleanup_command}; \
            {sudo}systemctl daemon-reload",
            sudo = sudo_prefix(use_sudo),
            cleanup_command = cleanup_command
        );
        run_remote(&session, &command)?;

        let install_residual_check = if is_system_dir {
            format!(
                "[ -e {base}/agent ] || [ -e {base}/config ] || [ -e {base}/database ] || [ -e {base}/logs ] || [ -e {base}/log ] || [ -e {base}/run ]",
                base = shell_escape(&install_base)
            )
        } else {
            format!("[ -e {} ]", shell_escape(&install_base))
        };
        let verify_command = format!(
            "if {sudo}systemctl is-active --quiet vust-agent \
             || [ -e /etc/systemd/system/vust-agent.service ] \
             || [ -e /usr/local/bin/vust-agent ] || [ -L /usr/local/bin/vust-agent ] \
             || [ -e /usr/bin/vust-agent ] || [ -L /usr/bin/vust-agent ] \
             || [ -e /usr/local/bin/vustctl ] || [ -L /usr/local/bin/vustctl ] \
             || [ -e /usr/bin/vustctl ] || [ -L /usr/bin/vustctl ] \
             || {install_residual_check}; then exit 42; fi",
            sudo = sudo_prefix(use_sudo),
        );
        run_remote(&session, &verify_command)
    })
    .await
    .map_err(|err| ApiError::Internal(err.to_string()))?;

    if let Err(err) = uninstall_result {
        runtime_metrics::record_deploy_result(false);
        let _ = node_provisioning::record_operation_result(
            pool,
            node_id,
            "ssh_uninstall",
            "failed",
            Some(format!("{:?}", err)),
        )
        .await;
        return Err(err);
    }

    close_active_sessions(pool, node_id, "uninstalled", "closed").await?;
    let active_session_remains = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM node_sessions WHERE node_id = ? AND status = 'active')",
    )
    .bind(node_id)
    .fetch_one(pool)
    .await?;
    if active_session_remains {
        let error = ApiError::conflict(
            vust_contracts::api::ErrorCode::NodeOperationConflict,
            "active node session remains after uninstall",
        );
        let _ = node_provisioning::record_operation_result(
            pool,
            node_id,
            "ssh_uninstall",
            "failed",
            Some(error.message.to_string()),
        )
        .await;
        return Err(error);
    }
    transition_to_retired(pool, node_id).await?;
    sqlx::query(
        r#"
        UPDATE nodes
        SET
            schedulable = 0,
            retired_at = COALESCE(retired_at, ?)
        WHERE node_id = ?
        "#,
    )
    .bind(Utc::now().to_rfc3339())
    .bind(node_id)
    .execute(pool)
    .await?;
    let _ = node_provisioning::record_operation_result(
        pool,
        node_id,
        "ssh_uninstall",
        "uninstalled",
        None,
    )
    .await;
    runtime_metrics::record_deploy_result(true);
    Ok(())
}

/// 使用给定参数执行远程部署并返回部署日志。
pub async fn deploy_node_with(
    input: NodeDeployInput,
    progress_sender: Option<DeployProgressSender>,
) -> Result<NodeDeployResult, NodeDeployError> {
    let agent_bin = config::get().agent_binary.clone();
    if !Path::new(&agent_bin).exists() {
        return Err(NodeDeployError {
            error: ApiError::BadRequest(format!("Agent binary does not exist: {}", agent_bin)),
            logs: Vec::new(),
        });
    }
    let vustctl_path = config::get().vustctl_path.clone();
    if !Path::new(&vustctl_path).exists() {
        return Err(NodeDeployError {
            error: ApiError::BadRequest(format!("vustctl script does not exist: {}", vustctl_path)),
            logs: Vec::new(),
        });
    }

    let addr_clone = input.addr.clone();
    let user_clone = input.user.clone();
    let listen_addr_clone = input.listen_addr.clone();
    let install_dir_clone = input.install_dir.clone();
    let agent_bin_clone = agent_bin.clone();
    let vustctl_path_clone = vustctl_path.clone();
    let agent_id_clone = input.agent_id.clone();
    let ssh_port_clone = input.port.clone();
    let vust_url_clone = input.vust_url.clone();
    let enrollment_token_clone = input.enrollment_token.clone();
    let auth_mode = input.auth_mode.clone();
    let password = input.password.clone();
    let private_key = input.private_key.clone();
    let private_key_passphrase = input.private_key_passphrase.clone();
    let allow_existing_agent = input.allow_existing_agent;
    let progress_sender_clone = progress_sender.clone();

    let logs = tokio::task::spawn_blocking(
        move || -> BlockingDeployResult {
            let mut logs = Vec::new();
            let mut rollback_artifact = None;
            let update_status = |logs_ref: &Vec<String>, progress: u32| {
                if let Some(sender) = &progress_sender_clone {
                    let _ = sender.send(DeployProgressEvent::Snapshot {
                        logs: logs_ref.clone(),
                        progress_percent: Some(progress),
                        is_finished: false,
                        error: None,
                    });
                }
            };
            log_line(
                &mut logs,
                &format!("Add node [{addr_clone}] task started [START]"),
            );
            update_status(&logs, 0);
            let session = match open_ssh_session(
                &addr_clone,
                Some(&ssh_port_clone),
                &user_clone,
                Some(&auth_mode),
                password.as_deref(),
                private_key.as_deref(),
                private_key_passphrase.as_deref(),
            ) {
                Ok(session) => session,
                Err(err) => {
                    log_line(&mut logs, &format!("Task failed: {err:?}"));
                    log_line(&mut logs, "[TASK-END]");
                    if let Some(sender) = &progress_sender_clone {
                        let _ = sender.send(DeployProgressEvent::Snapshot {
                            logs: logs.clone(),
                            progress_percent: None,
                            is_finished: true,
                            error: Some(format!("{err:?}")),
                        });
                    }
                    return Err((err, logs));
                }
            };
            let mut remote_mutated = false;
            let result = (|| -> ApiResult<()> {
                // 强前置 Sudo 免密检验防死锁 (Sudo Safe Guard)
                log_line(&mut logs, "Checking passwordless sudo privilege on target node");
                let use_sudo = user_clone != "root";
                if use_sudo {
                    let sudo_check = run_remote_capture(
                        &session,
                        "sh -c 'if sudo -n true >/dev/null 2>&1; then echo sudo; else echo nosudo; fi'",
                    )?;
                    if sudo_check.trim() != "sudo" {
                        return Err(ApiError::BadRequest(
                            "Target user lacks passwordless sudo privilege. Deployment aborted to prevent interactive hang.".to_string(),
                        ));
                    }
                }
                log_line(&mut logs, "Passwordless sudo privilege check passed");
                update_status(&logs, 10);

                if detect_vust_service(&session)? {
                    return Err(ApiError::BadRequest(node_conflict_message().to_string()));
                }
                let remote_agent = inspect_remote_agent(&session)?;
                if remote_agent.present && !allow_existing_agent {
                    return Err(ApiError::BadRequest(
                        "Existing vust-agent detected; new node deployment is blocked"
                            .to_string(),
                    ));
                }
                if allow_existing_agent {
                    let artifact = format!("/tmp/vust-reprovision-{agent_id_clone}");
                    prepare_remote_reprovision_backup(
                        &session,
                        user_clone != "root",
                        &install_dir_clone,
                        &artifact,
                    )?;
                    rollback_artifact = Some(artifact);
                }

                log_line(&mut logs, "Fetching node architecture information");
                let remote_arch = run_remote_capture(&session, "uname -m")?;
                let arch = remote_arch.trim();
                log_line(
                    &mut logs,
                    &format!(
                        "Detected controller architecture: {}; target node architecture: {}",
                        std::env::consts::ARCH,
                        arch
                    ),
                );

                // 强架构限制拦截 (Arch Safe Guard)
                if arch != "x86_64" {
                    return Err(ApiError::BadRequest(format!(
                        "Unsupported CPU architecture: {}. Only x86_64 is supported in the current phase.",
                        arch
                    )));
                }

                log_line(&mut logs, "Node architecture information fetched");
                update_status(&logs, 15);
                log_line(&mut logs, "Preparing node directories");

                if run_remote(&session, "command -v docker >/dev/null 2>&1").is_err() {
                    log_line(&mut logs, "Docker daemon was not detected; installation may be required");
                } else {
                    let docker_daemon_check =
                        format!("{}docker info >/dev/null 2>&1", sudo_prefix(use_sudo));
                    if run_remote(&session, &docker_daemon_check).is_err() {
                        log_line(
                            &mut logs,
                            "Docker daemon is not running or permission was denied; installation or permission changes may be required",
                        );
                    }
                }
                let agent_base = install_dir_clone.trim_end_matches('/').to_string();
                let data_dir = format!("{agent_base}/database");
                let log_dir = format!("{agent_base}/logs");
                let run_dir = format!("{agent_base}/run");
                let config_dir = format!("{agent_base}/config");
                run_remote(
                    &session,
                    &format!(
                        "{}mkdir -p {} {} {} {} /usr/local/bin",
                        sudo_prefix(use_sudo),
                        data_dir,
                        log_dir,
                        run_dir,
                        config_dir
                    ),
                )?;
                remote_mutated = true;
                log_line(&mut logs, "Node directories prepared");
                update_status(&logs, 20);

                let sftp = session.sftp().map_err(map_ssh_err)?;
                let mut remote_file = sftp
                    .create(Path::new("/tmp/vust-agent"))
                    .map_err(map_ssh_err)?;
                let local_file = std::fs::File::open(&agent_bin_clone).map_err(ApiError::Io)?;
                let total_size = local_file.metadata().map_err(ApiError::Io)?.len();
                log_line(&mut logs, "Uploading node binary");
                update_status(&logs, 20);

                let sender_for_progress = progress_sender_clone.clone();
                let mut last_percent = 20u32;
                let mut progress_reader = ProgressReader::new(local_file, total_size, move |current, total| {
                    let percent = (current as f64 / total as f64 * 50.0) as u32;
                    let actual_percent = 20 + percent;
                    if actual_percent > last_percent {
                        last_percent = actual_percent;
                        if let Some(sender) = &sender_for_progress {
                            let _ = sender.send(DeployProgressEvent::Progress(actual_percent));
                        }
                    }
                });

                std::io::copy(&mut progress_reader, &mut remote_file).map_err(ApiError::Io)?;
                remote_file.flush().map_err(ApiError::Io)?;
                drop(remote_file);
                drop(progress_reader);
                log_line(&mut logs, "Node binary uploaded");
                update_status(&logs, 70);

                log_line(&mut logs, "Installing node binary");
                update_status(&logs, 70);
                run_remote(
                    &session,
                    &format!("{}chmod +x /tmp/vust-agent", sudo_prefix(use_sudo)),
                )?;
                run_remote(
                    &session,
                    &format!(
                        "{}mv /tmp/vust-agent /usr/local/bin/vust-agent",
                        sudo_prefix(use_sudo)
                    ),
                )?;
                run_remote(
                    &session,
                    &format!(
                        "{}ln -sf /usr/local/bin/vust-agent /usr/bin/vust-agent",
                        sudo_prefix(use_sudo)
                    ),
                )?;
                log_line(&mut logs, "Node binary installed");
                update_status(&logs, 75);

                let mut vustctl_remote = sftp.create(Path::new("/tmp/vustctl")).map_err(map_ssh_err)?;
                let mut vustctl_local =
                    std::fs::File::open(&vustctl_path_clone).map_err(ApiError::Io)?;
                std::io::copy(&mut vustctl_local, &mut vustctl_remote).map_err(ApiError::Io)?;
                vustctl_remote.flush().map_err(ApiError::Io)?;
                drop(vustctl_remote);
                drop(vustctl_local);
                run_remote(
                    &session,
                    &format!("{}chmod +x /tmp/vustctl", sudo_prefix(use_sudo)),
                )?;
                run_remote(
                    &session,
                    &format!(
                        "{}mv /tmp/vustctl /usr/local/bin/vustctl",
                        sudo_prefix(use_sudo)
                    ),
                )?;
                run_remote(
                    &session,
                    &format!(
                        "{}ln -sf /usr/local/bin/vustctl /usr/bin/vustctl",
                        sudo_prefix(use_sudo)
                    ),
                )?;
                update_status(&logs, 80);

                let agent_config = format!(
                    "mode = \"remote\"\nlistenAddr = \"{}\"\nagentId = \"{}\"\nagentIp = \"{}\"\nvustUrl = \"{}\"\nenrollmentToken = \"{}\"\n",
                    listen_addr_clone,
                    agent_id_clone,
                    addr_clone,
                    vust_url_clone,
                    enrollment_token_clone
                );
                let mut config_file = sftp
                    .create(Path::new("/tmp/vust-agent.toml"))
                    .map_err(map_ssh_err)?;
                config_file
                    .write_all(agent_config.as_bytes())
                    .map_err(ApiError::Io)?;
                config_file.flush().map_err(ApiError::Io)?;
                drop(config_file);
                run_remote(
                    &session,
                    &format!(
                        "{}mv /tmp/vust-agent.toml {}/agent.toml",
                        sudo_prefix(use_sudo),
                        shell_escape(&config_dir)
                    ),
                )?;
                update_status(&logs, 85);
                let mut install_dir_file = sftp
                    .create(Path::new("/tmp/vust-agent.install_dir"))
                    .map_err(map_ssh_err)?;
                install_dir_file
                    .write_all(install_dir_clone.as_bytes())
                    .map_err(ApiError::Io)?;
                install_dir_file.flush().map_err(ApiError::Io)?;
                drop(install_dir_file);
                run_remote(
                    &session,
                    &format!(
                        "{}mv /tmp/vust-agent.install_dir {}/agent.install_dir",
                        sudo_prefix(use_sudo),
                        shell_escape(&config_dir)
                    ),
                )?;
                update_status(&logs, 88);
                run_remote(
                    &session,
                    &format!(
                        "{}sh -c 'printf %s\\\\n agent > {}/node.role'",
                        sudo_prefix(use_sudo),
                        shell_escape(&config_dir)
                    ),
                )?;

                let service_content = include_str!("../../../../deploy/templates/vust-agent.service")
                    .replace("__VUST_HOME__", &agent_base);

                let mut service_file = sftp
                    .create(Path::new("/tmp/vust-agent.service"))
                    .map_err(map_ssh_err)?;
                service_file
                    .write_all(service_content.as_bytes())
                    .map_err(ApiError::Io)?;
                service_file.flush().map_err(ApiError::Io)?;
                drop(service_file);
                run_remote(
                    &session,
                    &format!(
                        "{}mv /tmp/vust-agent.service /etc/systemd/system/vust-agent.service",
                        sudo_prefix(use_sudo)
                    ),
                )?;
                run_remote(
                    &session,
                    &format!("{}systemctl daemon-reload", sudo_prefix(use_sudo)),
                )?;
                update_status(&logs, 92);
                log_line(&mut logs, "Starting service");
                update_status(&logs, 92);
                run_remote(
                    &session,
                    &format!(
                        "{}systemctl enable vust-agent && {}systemctl restart vust-agent",
                        sudo_prefix(use_sudo),
                        sudo_prefix(use_sudo)
                    ),
                )?;
                log_line(&mut logs, "Service started");
                update_status(&logs, 96);
                log_line(&mut logs, &format!("Add node [{addr_clone}] task succeeded"));
                log_line(&mut logs, "[TASK-END]");
                Ok(())
            })();

            match result {
                Ok(()) => {
                    if let Some(sender) = &progress_sender_clone {
                        let _ = sender.send(DeployProgressEvent::Snapshot {
                            logs: logs.clone(),
                            progress_percent: Some(96),
                            is_finished: false,
                            error: None,
                        });
                    }
                    Ok((logs, rollback_artifact))
                }
                Err(err) => {
                    if let Some(artifact) = rollback_artifact.as_deref() {
                        log_line(&mut logs, "Restoring the previous Agent deployment");
                        match restore_remote_reprovision_backup(
                            &session,
                            user_clone != "root",
                            &install_dir_clone,
                            artifact,
                        ) {
                            Ok(()) => log_line(&mut logs, "Previous Agent deployment restored"),
                            Err(cleanup_err) => log_line(
                                &mut logs,
                                &format!("Deployment rollback was incomplete: {cleanup_err:?}"),
                            ),
                        }
                    } else if remote_mutated {
                        log_line(&mut logs, "Rolling back files created by this deployment");
                        match compensate_new_remote_deploy(
                            &session,
                            user_clone != "root",
                            &install_dir_clone,
                        ) {
                            Ok(()) => log_line(&mut logs, "Deployment rollback completed"),
                            Err(cleanup_err) => log_line(
                                &mut logs,
                                &format!("Deployment rollback was incomplete: {cleanup_err:?}"),
                            ),
                        }
                    }
                    log_line(&mut logs, &format!("Task failed: {:?}", err));
                    log_line(&mut logs, "[TASK-END]");
                    if let Some(sender) = &progress_sender_clone {
                        let _ = sender.send(DeployProgressEvent::Snapshot {
                            logs: logs.clone(),
                            progress_percent: None,
                            is_finished: true,
                            error: Some(format!("{err:?}")),
                        });
                    }
                    Err((err, logs))
                }
            }
        })
        .await
        .map_err(|err| NodeDeployError {
            error: ApiError::Internal(err.to_string()),
            logs: Vec::new(),
        })?;

    let (logs, rollback_artifact) = match logs {
        Ok(result) => result,
        Err((error, logs)) => {
            return Err(NodeDeployError { error, logs });
        }
    };

    Ok(NodeDeployResult {
        enrollment_id: input.enrollment_id.clone(),
        enrollment_token: input.enrollment_token.clone(),
        logs,
        rollback_artifact,
    })
}

/// 清理首次部署过程中创建的远端服务、制品与配置文件。
fn compensate_new_remote_deploy(
    session: &Session,
    use_sudo: bool,
    install_dir: &str,
) -> ApiResult<()> {
    let base = install_dir.trim_end_matches('/');
    let config_dir = format!("{base}/config");
    let command = format!(
        "{sudo}systemctl disable --now vust-agent >/dev/null 2>&1 || true; \
         {sudo}rm -f /etc/systemd/system/vust-agent.service \
         /usr/local/bin/vust-agent /usr/bin/vust-agent \
         /usr/local/bin/vustctl /usr/bin/vustctl \
         /tmp/vust-agent /tmp/vustctl /tmp/vust-agent.toml \
         /tmp/vust-agent.install_dir; \
         {sudo}rm -f {config}/agent.toml {config}/agent.install_dir {config}/node.role; \
         {sudo}systemctl daemon-reload",
        sudo = sudo_prefix(use_sudo),
        config = shell_escape(&config_dir),
    );
    run_remote(session, &command)
}

fn prepare_remote_reprovision_backup(
    session: &Session,
    use_sudo: bool,
    install_dir: &str,
    artifact: &str,
) -> ApiResult<()> {
    let config_dir = format!("{}/config", install_dir.trim_end_matches('/'));
    let command = format!(
        "{sudo}rm -rf {artifact}; {sudo}mkdir -p {artifact}; \
         [ ! -e /usr/local/bin/vust-agent ] || {sudo}cp -a /usr/local/bin/vust-agent {artifact}/vust-agent; \
         [ ! -e /usr/local/bin/vustctl ] || {sudo}cp -a /usr/local/bin/vustctl {artifact}/vustctl; \
         [ ! -e /etc/systemd/system/vust-agent.service ] || {sudo}cp -a /etc/systemd/system/vust-agent.service {artifact}/service; \
         [ ! -e {config}/agent.toml ] || {sudo}cp -a {config}/agent.toml {artifact}/agent.toml; \
         [ ! -e {config}/agent.install_dir ] || {sudo}cp -a {config}/agent.install_dir {artifact}/agent.install_dir; \
         [ ! -e {config}/node.role ] || {sudo}cp -a {config}/node.role {artifact}/node.role; \
         if {sudo}systemctl is-active --quiet vust-agent; then {sudo}touch {artifact}/was-active; fi",
        sudo = sudo_prefix(use_sudo),
        artifact = shell_escape(artifact),
        config = shell_escape(&config_dir),
    );
    run_remote(session, &command)
}

fn restore_remote_reprovision_backup(
    session: &Session,
    use_sudo: bool,
    install_dir: &str,
    artifact: &str,
) -> ApiResult<()> {
    let config_dir = format!("{}/config", install_dir.trim_end_matches('/'));
    let command = format!(
        "{sudo}systemctl stop vust-agent >/dev/null 2>&1 || true; \
         if [ -e {artifact}/vust-agent ]; then {sudo}cp -a {artifact}/vust-agent /usr/local/bin/vust-agent; else {sudo}rm -f /usr/local/bin/vust-agent /usr/bin/vust-agent; fi; \
         if [ -e {artifact}/vustctl ]; then {sudo}cp -a {artifact}/vustctl /usr/local/bin/vustctl; else {sudo}rm -f /usr/local/bin/vustctl /usr/bin/vustctl; fi; \
         if [ -e {artifact}/service ]; then {sudo}cp -a {artifact}/service /etc/systemd/system/vust-agent.service; else {sudo}rm -f /etc/systemd/system/vust-agent.service; fi; \
         {sudo}mkdir -p {config}; \
         for name in agent.toml agent.install_dir node.role; do if [ -e {artifact}/$name ]; then {sudo}cp -a {artifact}/$name {config}/$name; else {sudo}rm -f {config}/$name; fi; done; \
         if [ -e /usr/local/bin/vust-agent ]; then {sudo}ln -sf /usr/local/bin/vust-agent /usr/bin/vust-agent; else {sudo}rm -f /usr/bin/vust-agent; fi; \
         if [ -e /usr/local/bin/vustctl ]; then {sudo}ln -sf /usr/local/bin/vustctl /usr/bin/vustctl; else {sudo}rm -f /usr/bin/vustctl; fi; \
         {sudo}systemctl daemon-reload; \
         if [ -e {artifact}/was-active ]; then {sudo}systemctl enable vust-agent >/dev/null 2>&1; {sudo}systemctl restart vust-agent; fi; \
         {sudo}rm -rf {artifact}",
        sudo = sudo_prefix(use_sudo),
        artifact = shell_escape(artifact),
        config = shell_escape(&config_dir),
    );
    run_remote(session, &command)
}

/// 注册失败后重新连接远端并回滚本次部署。
pub async fn rollback_remote_deployment(
    input: &NodeDeployInput,
    rollback_artifact: Option<&str>,
) -> ApiResult<()> {
    let addr = input.addr.clone();
    let port = input.port.clone();
    let user = input.user.clone();
    let auth_mode = input.auth_mode.clone();
    let password = input.password.clone();
    let private_key = input.private_key.clone();
    let passphrase = input.private_key_passphrase.clone();
    let install_dir = input.install_dir.clone();
    let artifact = rollback_artifact.map(str::to_string);
    tokio::task::spawn_blocking(move || {
        let session = open_ssh_session(
            &addr,
            Some(&port),
            &user,
            Some(&auth_mode),
            password.as_deref(),
            private_key.as_deref(),
            passphrase.as_deref(),
        )?;
        if let Some(artifact) = artifact {
            restore_remote_reprovision_backup(&session, user != "root", &install_dir, &artifact)
        } else {
            compensate_new_remote_deploy(&session, user != "root", &install_dir)
        }
    })
    .await
    .map_err(|error| ApiError::Internal(error.to_string()))?
}

/// 注册成功后删除重新部署使用的临时备份。
pub async fn finalize_remote_deployment(
    input: &NodeDeployInput,
    rollback_artifact: Option<&str>,
) -> ApiResult<()> {
    let Some(artifact) = rollback_artifact.map(str::to_string) else {
        return Ok(());
    };
    let addr = input.addr.clone();
    let port = input.port.clone();
    let user = input.user.clone();
    let auth_mode = input.auth_mode.clone();
    let password = input.password.clone();
    let private_key = input.private_key.clone();
    let passphrase = input.private_key_passphrase.clone();
    tokio::task::spawn_blocking(move || {
        let session = open_ssh_session(
            &addr,
            Some(&port),
            &user,
            Some(&auth_mode),
            password.as_deref(),
            private_key.as_deref(),
            passphrase.as_deref(),
        )?;
        run_remote(
            &session,
            &format!(
                "{}rm -rf {}",
                sudo_prefix(user != "root"),
                shell_escape(&artifact)
            ),
        )
    })
    .await
    .map_err(|error| ApiError::Internal(error.to_string()))?
}

/// 启动恢复前清理首次部署残留，或恢复重新部署留下的原版本备份。
pub async fn recover_interrupted_remote_operation(
    pool: &DbPool,
    node_id: &str,
    reprovision: bool,
) -> ApiResult<()> {
    let record = get_node_provisioning_by_node_id(pool, node_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("node deployment record does not exist".to_string()))?;
    let addr = record
        .ssh_addr
        .ok_or_else(|| ApiError::BadRequest("node address must not be empty".to_string()))?;
    let port = record
        .ssh_port
        .map(|value| value.to_string())
        .unwrap_or_else(|| DEFAULT_SSH_PORT.to_string());
    let user = record
        .ssh_user
        .ok_or_else(|| ApiError::BadRequest("SSH user must not be empty".to_string()))?;
    let auth_mode = record
        .ssh_auth_mode
        .unwrap_or_else(|| "password".to_string());
    let password = decrypt_optional(record.ssh_password_ciphertext)
        .map_err(|_| ApiError::Internal("failed to decrypt SSH password".to_string()))?;
    let private_key = decrypt_optional(record.ssh_private_key_ciphertext)
        .map_err(|_| ApiError::Internal("failed to decrypt SSH private key".to_string()))?;
    let passphrase = decrypt_optional(record.ssh_private_key_passphrase_ciphertext)
        .map_err(|_| ApiError::Internal("failed to decrypt SSH key passphrase".to_string()))?;
    let install_dir = record.install_dir;
    let node_id = node_id.to_string();
    tokio::task::spawn_blocking(move || {
        let session = open_ssh_session(
            &addr,
            Some(&port),
            &user,
            Some(&auth_mode),
            password.as_deref(),
            private_key.as_deref(),
            passphrase.as_deref(),
        )?;
        if reprovision {
            let artifact = format!("/tmp/vust-reprovision-{node_id}");
            let exists = run_remote_capture(
                &session,
                &format!(
                    "if [ -d {} ]; then echo yes; else echo no; fi",
                    shell_escape(&artifact)
                ),
            )?;
            if exists.trim() == "yes" {
                restore_remote_reprovision_backup(
                    &session,
                    user != "root",
                    &install_dir,
                    &artifact,
                )?;
            }
            return Ok(());
        }

        let remote_agent = inspect_remote_agent(&session)?;
        if remote_agent
            .agent_id
            .as_deref()
            .is_some_and(|agent_id| agent_id != node_id)
        {
            return Err(ApiError::conflict(
                vust_contracts::api::ErrorCode::NodeInvalidTarget,
                "interrupted deployment target now belongs to another node",
            ));
        }
        compensate_new_remote_deploy(&session, user != "root", &install_dir)
    })
    .await
    .map_err(|error| ApiError::Internal(error.to_string()))?
}

/// 生成新的节点标识符。
pub fn generate_node_id() -> ApiResult<String> {
    Ok(new_uuid_v7())
}

pub fn resolve_vust_url() -> String {
    if let Ok(public_url) = std::env::var("VUST_PUBLIC_URL") {
        let trimmed = public_url.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    let listen = runtime_config::get_active_config();
    let host = match listen.host.as_str() {
        "0.0.0.0" | "::" => {
            if let Some(ref ph) = listen.public_host {
                let trimmed = ph.trim().to_string();
                if !trimmed.is_empty() {
                    trimmed
                } else {
                    std::env::var("VUST_PUBLIC_HOST")
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .or_else(detect_lan_ip_sync)
                        .unwrap_or_else(|| "127.0.0.1".to_string())
                }
            } else {
                std::env::var("VUST_PUBLIC_HOST")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .or_else(detect_lan_ip_sync)
                    .unwrap_or_else(|| "127.0.0.1".to_string())
            }
        }
        value => value.to_string(),
    };
    let scheme = std::env::var("VUST_URL_SCHEME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https".to_string());
    format!("{scheme}://{}:{}", host, listen.port)
}

/// 解析单次部署回连地址覆盖值，并强制生产回连入口使用 HTTPS。
pub fn resolve_vust_url_override(value: Option<&str>) -> ApiResult<String> {
    let resolved = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(resolve_vust_url);
    let parsed = reqwest::Url::parse(&resolved)
        .map_err(|_| ApiError::BadRequest("vustUrl must be a valid HTTPS URL".to_string()))?;
    if parsed.scheme() != "https" {
        return Err(ApiError::BadRequest("vustUrl must use HTTPS".to_string()));
    }
    if parsed.host_str().is_none() {
        return Err(ApiError::BadRequest(
            "vustUrl must include a host".to_string(),
        ));
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ApiError::BadRequest(
            "vustUrl must not include path, query, or fragment".to_string(),
        ));
    }
    Ok(resolved)
}

fn log_line(logs: &mut Vec<String>, message: &str) {
    let timestamp = Local::now().format("%Y/%m/%d %H:%M:%S");
    logs.push(format!("{} {}", timestamp, message));
}

fn run_remote_capture(session: &Session, command: &str) -> ApiResult<String> {
    let mut channel = session.channel_session().map_err(map_ssh_err)?;
    channel.exec(command).map_err(map_ssh_err)?;
    let mut output = String::new();
    let _ = channel.read_to_string(&mut output);
    let mut err_output = String::new();
    let _ = channel.stderr().read_to_string(&mut err_output);
    channel.wait_eof().map_err(map_ssh_err)?;
    channel.close().map_err(map_ssh_err)?;
    channel.wait_close().map_err(map_ssh_err)?;
    let exit = channel.exit_status().map_err(map_ssh_err)?;
    if exit != 0 {
        return Err(ApiError::Internal(format!(
            "remote command failed: {} (exit={}) stdout={} stderr={}",
            command,
            exit,
            output.trim(),
            err_output.trim()
        )));
    }
    Ok(output)
}

fn run_remote(session: &Session, command: &str) -> ApiResult<()> {
    let mut channel = session.channel_session().map_err(map_ssh_err)?;
    channel.exec(command).map_err(map_ssh_err)?;
    let mut output = String::new();
    let _ = channel.read_to_string(&mut output);
    let mut err_output = String::new();
    let _ = channel.stderr().read_to_string(&mut err_output);
    channel.wait_eof().map_err(map_ssh_err)?;
    channel.close().map_err(map_ssh_err)?;
    channel.wait_close().map_err(map_ssh_err)?;
    let exit = channel.exit_status().map_err(map_ssh_err)?;
    if exit != 0 {
        return Err(ApiError::Internal(format!(
            "remote command failed: {} (exit={}) stdout={} stderr={}",
            command,
            exit,
            output.trim(),
            err_output.trim()
        )));
    }
    Ok(())
}

fn sudo_prefix(use_sudo: bool) -> &'static str {
    if use_sudo { "sudo " } else { "" }
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn map_ssh_err(err: ssh2::Error) -> ApiError {
    ApiError::Internal(err.to_string())
}

pub fn detect_lan_ip_sync() -> Option<String> {
    if let Ok(output) = std::process::Command::new("ip")
        .args(["-j", "addr"])
        .output()
        && output.status.success()
        && let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        && let Some(arr) = val.as_array()
    {
        let mut candidates = Vec::new();
        for interface in arr {
            let ifname = interface
                .get("ifname")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if ifname.starts_with("lo")
                || ifname.starts_with("docker")
                || ifname.starts_with("br-")
                || ifname.starts_with("veth")
            {
                continue;
            }
            if let Some(addr_info) = interface.get("addr_info").and_then(|a| a.as_array()) {
                for addr in addr_info {
                    if let Some(family) = addr.get("family").and_then(|f| f.as_str())
                        && family == "inet"
                        && let Some(local) = addr.get("local").and_then(|l| l.as_str())
                    {
                        let ip = local.trim().to_string();
                        if is_private_v4(&ip) {
                            if ifname.starts_with("en")
                                || ifname.starts_with("eth")
                                || ifname.starts_with("wl")
                            {
                                return Some(ip);
                            }
                            candidates.push(ip);
                        }
                    }
                }
            }
        }
        if !candidates.is_empty() {
            return Some(candidates[0].clone());
        }
    }
    None
}

fn is_private_v4(ip: &str) -> bool {
    if let Ok(ip_addr) = ip.parse::<std::net::Ipv4Addr>() {
        let octets = ip_addr.octets();
        match octets[0] {
            10 => true,
            172 => octets[1] >= 16 && octets[1] <= 31,
            192 => octets[0] == 192 && octets[1] == 168,
            _ => false,
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{generate_node_id, registration_is_complete, resolve_vust_url_override};
    use crate::models::node_sessions::NodeSessionRecord;
    use chrono::{Duration, Utc};

    #[test]
    fn generate_node_id_uses_uuid_v7() {
        let node_id = generate_node_id().expect("should generate node id");
        let parsed = uuid::Uuid::parse_str(&node_id).expect("node id should be valid uuid");
        assert_eq!(parsed.get_version_num(), 7);
    }

    #[test]
    fn vust_url_override_requires_https() {
        let err = resolve_vust_url_override(Some("http://controller.example.com:7310"))
            .expect_err("http callback URL should be rejected");
        assert_eq!(err.message.as_ref(), "vustUrl must use HTTPS");
    }

    #[test]
    fn vust_url_override_rejects_path_query_and_fragment() {
        for value in [
            "https://controller.example.com:9443/api/v1",
            "https://controller.example.com:9443?token=abc",
            "https://controller.example.com:9443#agent",
        ] {
            let err = resolve_vust_url_override(Some(value))
                .expect_err("callback URL must be a base URL");
            assert_eq!(
                err.message.as_ref(),
                "vustUrl must not include path, query, or fragment"
            );
        }
    }

    #[test]
    fn vust_url_override_trims_trailing_slash() {
        let resolved = resolve_vust_url_override(Some("https://controller.example.com:9443/"))
            .expect("valid callback URL should be accepted");
        assert_eq!(resolved, "https://controller.example.com:9443");
    }

    #[test]
    fn registration_requires_used_enrollment_and_fresh_matching_lease() {
        let now = Utc::now();
        let registered_after = now - Duration::seconds(5);
        let session = NodeSessionRecord {
            session_id: "session-1".to_string(),
            node_id: "node-1".to_string(),
            agent_id: "node-1".to_string(),
            lease_id: "lease-1".to_string(),
            advertise_addr: None,
            listen_addr: None,
            listen_port: None,
            server_name: None,
            registered_at: now.to_rfc3339(),
            lease_expires_at: (now + Duration::seconds(20)).to_rfc3339(),
            last_heartbeat_at: None,
            heartbeat_sequence: 0,
            last_seen_at: None,
            status: "active".to_string(),
            close_reason: None,
            closed_at: None,
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
        };
        assert!(registration_is_complete(
            Some("used"),
            Some(&session),
            "node-1",
            registered_after,
            now,
        ));
        assert!(!registration_is_complete(
            Some("issued"),
            Some(&session),
            "node-1",
            registered_after,
            now,
        ));
        assert!(!registration_is_complete(
            Some("used"),
            Some(&session),
            "other-node",
            registered_after,
            now,
        ));
    }
}
