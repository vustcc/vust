//! Agent 在线自升级服务：下载、校验、暂存、替换与回滚本机二进制。

use crate::errors::AgentError;
use crate::types::ApiResult;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use vust_upgrade::{
    extract_named_file_from_tar_gz, verify_release_signature,
    verify_sha256 as verify_upgrade_sha256,
};

const STATE_FILE_NAME: &str = "upgrade-state.json";
const STAGED_BINARY_NAME: &str = "vust-agent.next";
const BACKUP_BINARY_NAME: &str = "vust-agent.prev";

/// Agent 升级准备请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradePrepareRequest {
    pub plan_id: String,
    pub target_id: String,
    pub target_version: String,
    pub asset_url: String,
    pub sha256: Option<String>,
    pub signature: Option<String>,
}

/// Agent 升级应用请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeApplyRequest {
    pub target_id: String,
}

/// Agent 升级回滚请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeRollbackRequest {
    pub target_id: String,
}

/// Agent 本地升级状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpgradeState {
    pub current_version: String,
    pub plan_id: Option<String>,
    pub target_id: Option<String>,
    pub target_version: Option<String>,
    pub status: String,
    pub staged_binary: Option<String>,
    pub backup_binary: Option<String>,
    pub error_detail: Option<String>,
    pub updated_at: String,
}

/// 返回当前升级状态。
pub async fn status() -> ApiResult<AgentUpgradeState> {
    let mut state = load_state().await.unwrap_or_else(|_| default_idle_state());
    state.current_version = env!("CARGO_PKG_VERSION").to_string();
    Ok(state)
}

/// 下载并校验目标二进制，暂存到运行目录。
pub async fn prepare(payload: UpgradePrepareRequest) -> ApiResult<AgentUpgradeState> {
    validate_prepare_payload(&payload)?;
    let work_dir = upgrade_work_dir();
    tokio::fs::create_dir_all(&work_dir)
        .await
        .map_err(AgentError::from)?;
    let staged_binary = work_dir.join(STAGED_BINARY_NAME);

    // 初始化状态为下载 0%
    let mut current_state = AgentUpgradeState {
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        plan_id: Some(payload.plan_id.clone()),
        target_id: Some(payload.target_id.clone()),
        target_version: Some(payload.target_version.clone()),
        status: "downloading:0%".to_string(),
        staged_binary: None,
        backup_binary: None,
        error_detail: None,
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    save_state(&current_state).await?;

    let client = vust_security::client::build_mtls_client_with_timeouts(
        "vust-agent",
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(300),
    )
    .map_err(|err| {
        AgentError::Internal(format!(
            "failed to build upgrade asset download client: {err}"
        ))
    });
    let client = match client {
        Ok(client) => client,
        Err(err) => {
            persist_failed_prepare_state(&mut current_state, err.to_string()).await;
            return Err(err.into());
        }
    };

    tracing::info!(
        plan_id = %payload.plan_id,
        target_id = %payload.target_id,
        target_version = %payload.target_version,
        asset_url = %payload.asset_url,
        "starting agent upgrade asset download"
    );

    let response = match client.get(&payload.asset_url).send().await {
        Ok(response) => response,
        Err(err) => {
            let detail = format!("failed to download upgrade asset: {err}");
            tracing::error!(
                plan_id = %payload.plan_id,
                target_id = %payload.target_id,
                asset_url = %payload.asset_url,
                error = %err,
                "failed to send agent upgrade asset download request"
            );
            persist_failed_prepare_state(&mut current_state, detail.clone()).await;
            return Err(AgentError::Internal(detail).into());
        }
    };

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|err| format!("failed to read error response body: {err}"));
        let preview = preview_body(&body);
        tracing::error!(
            plan_id = %payload.plan_id,
            target_id = %payload.target_id,
            asset_url = %payload.asset_url,
            status = status.as_u16(),
            response_body = %preview,
            "agent upgrade asset download returned non-success status"
        );
        let detail = format!(
            "upgrade asset request failed: status {}; body: {preview}",
            status.as_u16()
        );
        persist_failed_prepare_state(&mut current_state, detail.clone()).await;
        return Err(AgentError::BadRequest(detail).into());
    }

    let header_signature = response
        .headers()
        .get("x-vust-signature")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut bytes = Vec::with_capacity(if total_size > 0 {
        total_size as usize
    } else {
        1024 * 1024
    });
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    let mut last_percent = 0;

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(chunk) => chunk,
            Err(err) => {
                let detail = format!("failed to download chunk: {err}");
                tracing::error!(
                    plan_id = %payload.plan_id,
                    target_id = %payload.target_id,
                    asset_url = %payload.asset_url,
                    error = %err,
                    "failed to read agent upgrade asset download chunk"
                );
                persist_failed_prepare_state(&mut current_state, detail.clone()).await;
                return Err(AgentError::Internal(detail).into());
            }
        };
        bytes.extend_from_slice(&chunk);
        downloaded += chunk.len() as u64;
        let percent = (downloaded * 100).checked_div(total_size).unwrap_or(0) as u32;
        if percent > last_percent {
            last_percent = percent;
            current_state.status = format!("downloading:{percent}%");
            current_state.updated_at = chrono::Utc::now().to_rfc3339();
            let _ = save_state(&current_state).await;
        }
    }

    tracing::info!(
        plan_id = %payload.plan_id,
        target_id = %payload.target_id,
        asset_url = %payload.asset_url,
        content_length = total_size,
        downloaded_bytes = bytes.len(),
        "agent upgrade asset download completed"
    );

    if bytes.is_empty() {
        tracing::error!(
            plan_id = %payload.plan_id,
            target_id = %payload.target_id,
            asset_url = %payload.asset_url,
            "agent upgrade asset download returned empty body"
        );
        let detail = "upgrade asset is empty".to_string();
        persist_failed_prepare_state(&mut current_state, detail.clone()).await;
        return Err(AgentError::BadRequest(detail).into());
    }

    current_state.status = "downloaded".to_string();
    current_state.updated_at = chrono::Utc::now().to_rfc3339();
    save_state(&current_state).await?;

    current_state.status = "verifying".to_string();
    current_state.updated_at = chrono::Utc::now().to_rfc3339();
    save_state(&current_state).await?;

    if let Some(expected) = payload.sha256.as_deref()
        && let Err(err) = verify_sha256(&bytes, expected)
    {
        persist_failed_prepare_state(&mut current_state, err.to_string()).await;
        return Err(err.into());
    }
    let signature = match payload
        .signature
        .as_deref()
        .or(header_signature.as_deref())
        .ok_or_else(|| AgentError::BadRequest("upgrade asset signature is required".to_string()))
    {
        Ok(signature) => signature,
        Err(err) => {
            persist_failed_prepare_state(&mut current_state, err.to_string()).await;
            return Err(err.into());
        }
    };
    if let Err(err) = verify_signature(&bytes, signature) {
        persist_failed_prepare_state(&mut current_state, err.to_string()).await;
        return Err(err.into());
    }

    current_state.status = "staging".to_string();
    current_state.updated_at = chrono::Utc::now().to_rfc3339();
    save_state(&current_state).await?;

    let binary = match extract_named_file_from_tar_gz(&bytes, "vust-agent")
        .map_err(|err| AgentError::BadRequest(err.to_string()))
    {
        Ok(binary) => binary,
        Err(err) => {
            persist_failed_prepare_state(&mut current_state, err.to_string()).await;
            return Err(err.into());
        }
    };
    let mut file = match tokio::fs::File::create(&staged_binary).await {
        Ok(file) => file,
        Err(err) => {
            let err = AgentError::from(err);
            persist_failed_prepare_state(&mut current_state, err.to_string()).await;
            return Err(err.into());
        }
    };
    if let Err(err) = file.write_all(&binary).await {
        let err = AgentError::from(err);
        persist_failed_prepare_state(&mut current_state, err.to_string()).await;
        return Err(err.into());
    }
    if let Err(err) = file.flush().await {
        let err = AgentError::from(err);
        persist_failed_prepare_state(&mut current_state, err.to_string()).await;
        return Err(err.into());
    }
    drop(file);
    let mut permissions = match tokio::fs::metadata(&staged_binary).await {
        Ok(metadata) => metadata.permissions(),
        Err(err) => {
            let err = AgentError::from(err);
            persist_failed_prepare_state(&mut current_state, err.to_string()).await;
            return Err(err.into());
        }
    };
    permissions.set_mode(0o755);
    if let Err(err) = tokio::fs::set_permissions(&staged_binary, permissions).await {
        let err = AgentError::from(err);
        persist_failed_prepare_state(&mut current_state, err.to_string()).await;
        return Err(err.into());
    }

    current_state.status = "prepared".to_string();
    current_state.staged_binary = Some(staged_binary.to_string_lossy().into_owned());
    current_state.updated_at = chrono::Utc::now().to_rfc3339();
    save_state(&current_state).await?;
    Ok(current_state)
}

/// 原子替换当前二进制；生产布局下会异步重启 systemd 服务。
pub async fn apply(payload: UpgradeApplyRequest) -> ApiResult<AgentUpgradeState> {
    let mut state = load_state().await?;
    ensure_target_matches(&state, &payload.target_id)?;
    let staged = state
        .staged_binary
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| AgentError::BadRequest("upgrade has not been prepared".to_string()))?;
    if !staged.is_file() {
        return Err(AgentError::BadRequest("staged upgrade binary is missing".to_string()).into());
    }

    let current = std::env::current_exe()
        .map_err(|err| AgentError::Internal(format!("failed to resolve current binary: {err}")))?;
    let backup = upgrade_work_dir().join(BACKUP_BINARY_NAME);
    if backup.exists() {
        tokio::fs::remove_file(&backup)
            .await
            .map_err(AgentError::from)?;
    }
    tokio::fs::copy(&current, &backup)
        .await
        .map_err(AgentError::from)?;
    tokio::fs::rename(&staged, &current)
        .await
        .map_err(AgentError::from)?;

    state.status = if should_restart_after_replace(&current) {
        schedule_agent_restart();
        "restart_scheduled".to_string()
    } else {
        "applied_no_restart".to_string()
    };
    state.backup_binary = Some(backup.to_string_lossy().into_owned());
    state.staged_binary = None;
    state.error_detail = None;
    state.updated_at = chrono::Utc::now().to_rfc3339();
    save_state(&state).await?;
    Ok(state)
}

/// 使用备份二进制回滚当前版本；生产布局下会异步重启 systemd 服务。
pub async fn rollback(payload: UpgradeRollbackRequest) -> ApiResult<AgentUpgradeState> {
    let mut state = load_state().await?;
    ensure_target_matches(&state, &payload.target_id)?;
    let backup = state
        .backup_binary
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| upgrade_work_dir().join(BACKUP_BINARY_NAME));
    if !backup.is_file() {
        return Err(AgentError::BadRequest("backup binary is missing".to_string()).into());
    }
    let current = std::env::current_exe()
        .map_err(|err| AgentError::Internal(format!("failed to resolve current binary: {err}")))?;
    tokio::fs::copy(&backup, &current)
        .await
        .map_err(AgentError::from)?;

    state.status = if should_restart_after_replace(&current) {
        schedule_agent_restart();
        "rollback_restart_scheduled".to_string()
    } else {
        "rollbacked_no_restart".to_string()
    };
    state.error_detail = None;
    state.updated_at = chrono::Utc::now().to_rfc3339();
    save_state(&state).await?;
    Ok(state)
}

fn validate_prepare_payload(payload: &UpgradePrepareRequest) -> Result<(), AgentError> {
    if payload.plan_id.trim().is_empty()
        || payload.target_id.trim().is_empty()
        || payload.target_version.trim().is_empty()
    {
        return Err(AgentError::BadRequest(
            "planId, targetId and targetVersion are required".to_string(),
        ));
    }
    if !payload.asset_url.starts_with("https://") {
        return Err(AgentError::BadRequest(
            "upgrade assetUrl must use https".to_string(),
        ));
    }
    Ok(())
}

fn preview_body(body: &str) -> String {
    const MAX_CHARS: usize = 2048;
    let mut preview: String = body.chars().take(MAX_CHARS).collect();
    if body.chars().count() > MAX_CHARS {
        preview.push_str("...[truncated]");
    }
    preview
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), AgentError> {
    verify_upgrade_sha256(bytes, expected)
        .map(|_| ())
        .map_err(|err| AgentError::BadRequest(format!("upgrade asset {err}")))
}

fn verify_signature(bytes: &[u8], signature: &str) -> Result<(), AgentError> {
    verify_release_signature(bytes, signature)
        .map_err(|err| AgentError::BadRequest(format!("upgrade asset {err}")))
}

async fn persist_failed_prepare_state(state: &mut AgentUpgradeState, detail: impl Into<String>) {
    state.status = "failed".to_string();
    state.error_detail = Some(detail.into());
    state.updated_at = chrono::Utc::now().to_rfc3339();
    if let Err(err) = save_state(state).await {
        tracing::error!(
            error = %err,
            "failed to persist agent upgrade prepare failure state"
        );
    }
}

async fn load_state() -> ApiResult<AgentUpgradeState> {
    let raw = tokio::fs::read_to_string(state_path())
        .await
        .map_err(AgentError::from)?;
    serde_json::from_str(&raw)
        .map_err(|err| AgentError::Internal(format!("failed to parse upgrade state: {err}")).into())
}

async fn save_state(state: &AgentUpgradeState) -> ApiResult<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(AgentError::from)?;
    }
    let raw = serde_json::to_vec_pretty(state)
        .map_err(|err| AgentError::Internal(format!("failed to encode upgrade state: {err}")))?;
    tokio::fs::write(path, raw)
        .await
        .map_err(AgentError::from)?;
    Ok(())
}

fn default_idle_state() -> AgentUpgradeState {
    AgentUpgradeState {
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        plan_id: None,
        target_id: None,
        target_version: None,
        status: "idle".to_string(),
        staged_binary: None,
        backup_binary: None,
        error_detail: None,
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn ensure_target_matches(state: &AgentUpgradeState, target_id: &str) -> Result<(), AgentError> {
    if state.target_id.as_deref() != Some(target_id) {
        return Err(AgentError::BadRequest(
            "upgrade targetId does not match prepared state".to_string(),
        ));
    }
    Ok(())
}

fn state_path() -> PathBuf {
    upgrade_work_dir().join(STATE_FILE_NAME)
}

fn upgrade_work_dir() -> PathBuf {
    vust_home().join("run/upgrades")
}

fn vust_home() -> PathBuf {
    std::env::var_os("VUST_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/vust"))
}

fn should_restart_after_replace(current: &Path) -> bool {
    std::env::var_os("VUST_HOME").is_some() || current.starts_with("/opt/vust")
}

fn schedule_agent_restart() {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let result = tokio::process::Command::new("systemctl")
            .args(["restart", "vust-agent"])
            .status()
            .await;
        if let Err(err) = result {
            tracing::error!("failed to restart vust-agent after upgrade: {}", err);
        }
    });
}
