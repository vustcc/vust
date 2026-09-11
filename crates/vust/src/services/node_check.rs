//! 节点检查服务：验证 SSH、服务与 API 可用性并写入观测结果。

use crate::crypto::decrypt_optional;
use crate::models::node_provisioning::{NodeProvisioningRecord, get_node_provisioning_by_node_id};
use crate::models::node_runtime_client::NodeRuntimeClient;
use crate::models::node_sessions::resolve_active_session_endpoint;
use crate::services::node_observation;
use crate::services::node_precheck::NodePrecheckStatus;
use crate::services::node_target_guard::open_ssh_session;
use crate::state::DbPool;
use crate::types::{ApiError, ApiResult};
use serde::Serialize;
use ssh2::Session;
use std::io::Read;
use vust_api::response::ApiResponse;
use vust_contracts::types::{DockerStatusSummary, HostSystemSummary};

const DEFAULT_SSH_PORT: &str = crate::config::DEFAULT_SSH_PORT;

/// 单项节点连通性检查结果。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeCheckItem {
    pub status: NodePrecheckStatus,
    pub code: Option<String>,
    pub message: String,
}

/// 节点连通性检查汇总结果。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeCheckResponse {
    pub status: String,
    pub ssh: NodeCheckItem,
    pub service: NodeCheckItem,
    pub api: NodeCheckItem,
}

/// 依次验证 SSH、服务与 API 可用性并记录手动检查结果。
pub async fn check_node_health(pool: &DbPool, node_id: &str) -> ApiResult<NodeCheckResponse> {
    let provisioning = get_node_provisioning_by_node_id(pool, node_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("node deployment record does not exist".to_string()))?;

    let ssh_check = match check_ssh_available(&provisioning).await {
        Ok(()) => NodeCheckItem {
            status: NodePrecheckStatus::Passed,
            code: Some("ssh_ok".to_string()),
            message: "SSH connection is healthy".to_string(),
        },
        Err(err) => {
            let response = NodeCheckResponse {
                status: "OFFLINE".to_string(),
                ssh: NodeCheckItem {
                    status: NodePrecheckStatus::Failed,
                    code: Some("ssh_failed".to_string()),
                    message: format!("{:?}", err),
                },
                service: NodeCheckItem {
                    status: NodePrecheckStatus::Skipped,
                    code: Some("not_executed".to_string()),
                    message: "Not executed".to_string(),
                },
                api: NodeCheckItem {
                    status: NodePrecheckStatus::Skipped,
                    code: Some("not_executed".to_string()),
                    message: "Not executed".to_string(),
                },
            };
            node_observation::record_manual_check(
                pool,
                node_id,
                &response.status,
                NodePrecheckStatus::Failed,
                NodePrecheckStatus::Skipped,
                NodePrecheckStatus::Skipped,
                serde_json::json!(response),
            )
            .await?;
            return Ok(response);
        }
    };

    let service_check = match check_node_service(&provisioning).await {
        Ok(item) => item,
        Err(err) => {
            let response = NodeCheckResponse {
                status: "OFFLINE".to_string(),
                ssh: ssh_check,
                service: NodeCheckItem {
                    status: NodePrecheckStatus::Failed,
                    code: Some("service_error".to_string()),
                    message: format!("{:?}", err),
                },
                api: NodeCheckItem {
                    status: NodePrecheckStatus::Skipped,
                    code: Some("not_executed".to_string()),
                    message: "Not executed".to_string(),
                },
            };
            node_observation::record_manual_check(
                pool,
                node_id,
                &response.status,
                response.ssh.status,
                NodePrecheckStatus::Failed,
                NodePrecheckStatus::Skipped,
                serde_json::json!(response),
            )
            .await?;
            return Ok(response);
        }
    };
    if service_check.status == NodePrecheckStatus::Failed {
        let response = NodeCheckResponse {
            status: "OFFLINE".to_string(),
            ssh: ssh_check,
            service: service_check,
            api: NodeCheckItem {
                status: NodePrecheckStatus::Skipped,
                code: Some("not_executed".to_string()),
                message: "Not executed".to_string(),
            },
        };
        node_observation::record_manual_check(
            pool,
            node_id,
            &response.status,
            response.ssh.status,
            response.service.status,
            NodePrecheckStatus::Skipped,
            serde_json::json!(response),
        )
        .await?;
        return Ok(response);
    }

    let api_check = match check_node_api(pool, node_id, &provisioning).await {
        Ok((_summary, _docker_status)) => NodeCheckItem {
            status: NodePrecheckStatus::Passed,
            code: Some("api_ok".to_string()),
            message: "Node API is available".to_string(),
        },
        Err(err) => NodeCheckItem {
            status: NodePrecheckStatus::Failed,
            code: Some("api_failed".to_string()),
            message: format!("{:?}", err),
        },
    };

    let status = if api_check.status == NodePrecheckStatus::Passed {
        "ONLINE"
    } else {
        "OFFLINE"
    };
    let response = NodeCheckResponse {
        status: status.to_string(),
        ssh: ssh_check,
        service: service_check,
        api: api_check,
    };
    node_observation::record_manual_check(
        pool,
        node_id,
        &response.status,
        response.ssh.status,
        response.service.status,
        response.api.status,
        serde_json::json!(response),
    )
    .await?;
    Ok(response)
}

async fn check_ssh_available(record: &NodeProvisioningRecord) -> ApiResult<()> {
    let addr = record
        .ssh_addr
        .clone()
        .ok_or_else(|| ApiError::BadRequest("node address must not be empty".to_string()))?;
    let user = record
        .ssh_user
        .clone()
        .ok_or_else(|| ApiError::BadRequest("SSH user must not be empty".to_string()))?;
    let port = record
        .ssh_port
        .map(|value| value.to_string())
        .unwrap_or_else(|| DEFAULT_SSH_PORT.to_string());
    let auth_mode = record
        .ssh_auth_mode
        .clone()
        .unwrap_or_else(|| "password".to_string());

    let password = decrypt_optional(record.ssh_password_ciphertext.clone()).map_err(|_| {
        ApiError::Internal("failed to decrypt SSH password; check key configuration".to_string())
    })?;
    let private_key =
        decrypt_optional(record.ssh_private_key_ciphertext.clone()).map_err(|_| {
            ApiError::Internal(
                "failed to decrypt SSH private key; check key configuration".to_string(),
            )
        })?;
    let private_key_passphrase =
        decrypt_optional(record.ssh_private_key_passphrase_ciphertext.clone()).map_err(|_| {
            ApiError::Internal(
                "failed to decrypt SSH private key passphrase; check key configuration".to_string(),
            )
        })?;

    tokio::task::spawn_blocking(move || -> ApiResult<()> {
        open_ssh_session(
            &addr,
            Some(&port),
            &user,
            Some(&auth_mode),
            password.as_deref(),
            private_key.as_deref(),
            private_key_passphrase.as_deref(),
        )
        .map(|_| ())
    })
    .await
    .map_err(|err| ApiError::Internal(err.to_string()))?
}

async fn check_node_service(record: &NodeProvisioningRecord) -> ApiResult<NodeCheckItem> {
    let addr = record
        .ssh_addr
        .clone()
        .ok_or_else(|| ApiError::BadRequest("node address must not be empty".to_string()))?;
    let user = record
        .ssh_user
        .clone()
        .ok_or_else(|| ApiError::BadRequest("SSH user must not be empty".to_string()))?;
    let port = record
        .ssh_port
        .map(|value| value.to_string())
        .unwrap_or_else(|| DEFAULT_SSH_PORT.to_string());
    let auth_mode = record
        .ssh_auth_mode
        .clone()
        .unwrap_or_else(|| "password".to_string());

    let password = decrypt_optional(record.ssh_password_ciphertext.clone()).map_err(|_| {
        ApiError::Internal("failed to decrypt SSH password; check key configuration".to_string())
    })?;
    let private_key =
        decrypt_optional(record.ssh_private_key_ciphertext.clone()).map_err(|_| {
            ApiError::Internal(
                "failed to decrypt SSH private key; check key configuration".to_string(),
            )
        })?;
    let private_key_passphrase =
        decrypt_optional(record.ssh_private_key_passphrase_ciphertext.clone()).map_err(|_| {
            ApiError::Internal(
                "failed to decrypt SSH private key passphrase; check key configuration".to_string(),
            )
        })?;

    tokio::task::spawn_blocking(move || -> ApiResult<NodeCheckItem> {
        let session = open_ssh_session(
            &addr,
            Some(&port),
            &user,
            Some(&auth_mode),
            password.as_deref(),
            private_key.as_deref(),
            private_key_passphrase.as_deref(),
        )?;

        let output = run_remote_capture(
            &session,
            "sh -c 'if command -v systemctl >/dev/null 2>&1; then \
            if systemctl is-active --quiet vust-agent; then echo active; \
            elif systemctl status vust-agent >/dev/null 2>&1; then echo inactive; \
            else echo missing; fi; \
            else \
            if [ -x /usr/bin/vust-agent ] || [ -x /usr/local/bin/vust-agent ] || [ -f /opt/vust/config/agent.toml ]; then echo inactive; else echo missing; fi; \
            fi'",
        )?;

        match output.trim() {
            "active" => Ok(NodeCheckItem {
                status: NodePrecheckStatus::Passed,
                code: Some("service_running".to_string()),
                message: "Service is running".to_string(),
            }),
            "inactive" => Ok(NodeCheckItem {
                status: NodePrecheckStatus::Failed,
                code: Some("service_stopped".to_string()),
                message: "Service is not running".to_string(),
            }),
            _ => Ok(NodeCheckItem {
                status: NodePrecheckStatus::Failed,
                code: Some("service_missing".to_string()),
                message: "Service does not exist or has been uninstalled".to_string(),
            }),
        }
    })
    .await
    .map_err(|err| ApiError::Internal(err.to_string()))?
}

async fn check_node_api(
    pool: &DbPool,
    node_id: &str,
    provisioning: &NodeProvisioningRecord,
) -> ApiResult<(HostSystemSummary, Option<DockerStatusSummary>)> {
    let client = match resolve_active_session_endpoint(pool, node_id).await? {
        Some(_) => NodeRuntimeClient::from_node_route(pool, Some(node_id)).await?,
        None => {
            let api_url = build_probe_endpoint(provisioning).ok_or_else(|| {
                ApiError::BadRequest("node runtime endpoint must not be empty".to_string())
            })?;
            NodeRuntimeClient::from_api_url(api_url)?
        }
    };
    let response: ApiResponse<HostSystemSummary> = client
        .get_json("/api/v1/agent/system/summary")
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    if !response.success {
        return Err(ApiError::BadRequest(response.message));
    }
    let summary = response
        .data
        .ok_or_else(|| ApiError::BadRequest("Node API returned an empty response".to_string()))?;
    let docker_status = check_node_docker_status(&client).await;
    Ok((summary, docker_status))
}

async fn check_node_docker_status(client: &NodeRuntimeClient) -> Option<DockerStatusSummary> {
    let response: ApiResponse<DockerStatusSummary> =
        client.get_json("/api/v1/agent/docker/status").await.ok()?;
    if !response.success {
        return None;
    }
    response.data
}

fn build_probe_endpoint(record: &NodeProvisioningRecord) -> Option<String> {
    let addr = record.ssh_addr.as_deref()?;
    let port = record.expected_listen_port?;
    Some(format!("https://{}:{}", addr, port))
}

fn map_ssh_err(err: ssh2::Error) -> ApiError {
    ApiError::Internal(err.to_string())
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
