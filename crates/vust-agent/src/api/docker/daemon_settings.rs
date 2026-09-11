//! Docker daemon 设置接口：管理镜像加速与出站代理配置。

use crate::api::docker::context::DockerOperationContext;
use crate::services::settings as settings_service;
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;

const DOCKER_DAEMON_CONFIG_PATH: &str = "/etc/docker/daemon.json";
const DOCKER_PROXY_ADDRESS_KEY: &str = "docker.proxy.address";
static DAEMON_SETTINGS_LOCK: Mutex<()> = Mutex::const_new(());

/// VUST 管理的 Docker daemon 设置。
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerDaemonSettings {
    pub registry_mirrors: Vec<String>,
    pub proxy: String,
    pub proxy_enabled: bool,
}

/// 读取当前节点的 Docker daemon 设置。
pub async fn get_settings(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let config = read_daemon_config(Path::new(DOCKER_DAEMON_CONFIG_PATH)).await?;
    let (registry_mirrors, active_proxy) = extract_daemon_settings(&config)?;
    let saved_proxy = settings_service::get_string_or_default(
        &state.metadata_db,
        DOCKER_PROXY_ADDRESS_KEY,
        &active_proxy,
    )
    .await
    .map_err(|err| ApiError::Internal(format!("failed to read saved Docker proxy: {err}")))?;
    let proxy_enabled = !active_proxy.is_empty();
    let settings = DockerDaemonSettings {
        registry_mirrors,
        proxy: if proxy_enabled {
            active_proxy
        } else {
            saved_proxy
        },
        proxy_enabled,
    };
    Ok(ApiResponse::success_with_raw("Docker daemon settings loaded", settings).into_response())
}

/// 校验、写入并应用当前节点的 Docker daemon 设置。
pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Json(settings): Json<DockerDaemonSettings>,
) -> ApiResult<Response> {
    let result = update_settings_inner(Arc::clone(&state), settings).await;
    context
        .finish(
            &state.metadata_db,
            "docker_daemon_settings_update",
            Some(("dockerDaemon", "settings")),
            json!({}),
            true,
            result,
        )
        .await
}

async fn update_settings_inner(
    state: Arc<AppState>,
    mut settings: DockerDaemonSettings,
) -> ApiResult<Response> {
    let _guard = DAEMON_SETTINGS_LOCK.lock().await;
    normalize_and_validate_settings(&mut settings)?;

    let config_path = Path::new(DOCKER_DAEMON_CONFIG_PATH);
    let original = read_optional_file(config_path).await?;
    let current = parse_daemon_config(original.as_deref())?;
    let updated = merge_settings(current, &settings);
    let content = serialize_config(&updated)?;
    let staged_path = staged_config_path(config_path, "apply");

    write_config_file(&staged_path, &content).await?;
    if let Err(err) = validate_daemon_config(&staged_path).await {
        remove_file_if_exists(&staged_path).await;
        return Err(err);
    }
    if let Err(err) = tokio::fs::rename(&staged_path, config_path).await {
        remove_file_if_exists(&staged_path).await;
        return Err(ApiError::Internal(format!(
            "failed to replace Docker daemon config: {err}"
        )));
    }

    if let Err(apply_error) = restart_docker().await {
        let rollback_config = restore_original_config(config_path, original.as_deref()).await;
        let rollback_restart = restart_docker().await;
        let rollback_detail = match (rollback_config, rollback_restart) {
            (Ok(()), Ok(())) => "original configuration restored and Docker restarted".to_string(),
            (config_result, restart_result) => format!(
                "rollback config: {}; rollback restart: {}",
                format_result(config_result),
                format_result(restart_result)
            ),
        };
        return Err(ApiError::BadRequest(format!(
            "failed to apply Docker daemon settings: {apply_error}; {rollback_detail}"
        )));
    }

    settings_service::set_string(
        &state.metadata_db,
        DOCKER_PROXY_ADDRESS_KEY,
        &settings.proxy,
    )
    .await
    .map_err(|err| ApiError::Internal(format!("failed to save Docker proxy address: {err}")))?;

    Ok(ApiResponse::success_with_raw("Docker daemon settings applied", settings).into_response())
}

async fn read_daemon_config(path: &Path) -> ApiResult<Value> {
    let content = read_optional_file(path).await?;
    parse_daemon_config(content.as_deref())
}

async fn read_optional_file(path: &Path) -> ApiResult<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ApiError::Internal(format!(
            "failed to read Docker daemon config: {err}"
        ))),
    }
}

fn parse_daemon_config(content: Option<&[u8]>) -> ApiResult<Value> {
    let Some(content) = content else {
        return Ok(Value::Object(Map::new()));
    };
    let value: Value = serde_json::from_slice(content)
        .map_err(|err| ApiError::BadRequest(format!("invalid Docker daemon config: {err}")))?;
    if !value.is_object() {
        return Err(ApiError::BadRequest(
            "Docker daemon config root must be a JSON object".to_string(),
        ));
    }
    Ok(value)
}

fn extract_daemon_settings(config: &Value) -> ApiResult<(Vec<String>, String)> {
    let registry_mirrors = config
        .get("registry-mirrors")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let proxies = config.get("proxies").and_then(Value::as_object);
    let http_proxy = proxy_value(proxies, "http-proxy");
    let https_proxy = proxy_value(proxies, "https-proxy");
    if !http_proxy.is_empty() && !https_proxy.is_empty() && http_proxy != https_proxy {
        return Err(ApiError::BadRequest(
            "Docker daemon HTTP and HTTPS proxy values differ; unify them before using VUST settings"
                .to_string(),
        ));
    }

    Ok((
        registry_mirrors,
        if !http_proxy.is_empty() {
            http_proxy
        } else {
            https_proxy
        },
    ))
}

fn proxy_value(proxies: Option<&Map<String, Value>>, key: &str) -> String {
    proxies
        .and_then(|values| values.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn normalize_and_validate_settings(settings: &mut DockerDaemonSettings) -> ApiResult<()> {
    settings.registry_mirrors = settings
        .registry_mirrors
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    settings.registry_mirrors.sort();
    settings.registry_mirrors.dedup();
    for mirror in &settings.registry_mirrors {
        validate_registry_mirror_url(mirror)?;
    }

    settings.proxy = settings.proxy.trim().to_string();
    if settings.proxy_enabled && settings.proxy.is_empty() {
        return Err(ApiError::BadRequest(
            "proxy address is required when proxy is enabled".to_string(),
        ));
    }
    if settings.proxy_enabled {
        validate_proxy_url(&settings.proxy)?;
    }
    Ok(())
}

fn validate_registry_mirror_url(value: &str) -> ApiResult<()> {
    let url = Url::parse(value)
        .map_err(|err| ApiError::BadRequest(format!("invalid registry mirror URL: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ApiError::BadRequest(
            "registry mirror URL must use http or https and include a host".to_string(),
        ));
    }
    Ok(())
}

fn validate_proxy_url(value: &str) -> ApiResult<()> {
    let url = Url::parse(value).map_err(|_| invalid_proxy_url())?;
    let has_invalid_suffix = (url.path() != "" && url.path() != "/")
        || url.query().is_some()
        || url.fragment().is_some();
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.port().is_none()
        || has_invalid_suffix
    {
        return Err(invalid_proxy_url());
    }
    Ok(())
}

fn invalid_proxy_url() -> ApiError {
    ApiError::BadRequest("invalid proxy address format".to_string())
}

fn merge_settings(mut config: Value, settings: &DockerDaemonSettings) -> Value {
    let root = config
        .as_object_mut()
        .expect("validated daemon config object");
    if settings.registry_mirrors.is_empty() {
        root.remove("registry-mirrors");
    } else {
        root.insert(
            "registry-mirrors".to_string(),
            Value::Array(
                settings
                    .registry_mirrors
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }

    if !settings.proxy_enabled {
        root.remove("proxies");
    } else {
        let mut proxies = Map::new();
        proxies.insert(
            "http-proxy".to_string(),
            Value::String(settings.proxy.clone()),
        );
        proxies.insert(
            "https-proxy".to_string(),
            Value::String(settings.proxy.clone()),
        );
        root.insert("proxies".to_string(), Value::Object(proxies));
    }
    config
}

fn serialize_config(config: &Value) -> ApiResult<Vec<u8>> {
    let mut content = serde_json::to_vec_pretty(config)
        .map_err(|err| ApiError::Internal(format!("failed to serialize daemon config: {err}")))?;
    content.push(b'\n');
    Ok(content)
}

fn staged_config_path(config_path: &Path, purpose: &str) -> PathBuf {
    config_path.with_file_name(format!(
        ".daemon.json.vust-{purpose}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ))
}

async fn write_config_file(path: &Path, content: &[u8]) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|err| {
            ApiError::Internal(format!("failed to create Docker config directory: {err}"))
        })?;
    }
    tokio::fs::write(path, content).await.map_err(|err| {
        ApiError::Internal(format!("failed to write Docker daemon config: {err}"))
    })?;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
        .await
        .map_err(|err| {
            ApiError::Internal(format!(
                "failed to set Docker daemon config permissions: {err}"
            ))
        })
}

async fn validate_daemon_config(path: &Path) -> ApiResult<()> {
    let output = Command::new("dockerd")
        .arg("--validate")
        .arg(format!("--config-file={}", path.display()))
        .output()
        .await
        .map_err(|err| ApiError::Internal(format!("failed to run dockerd validation: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(ApiError::BadRequest(format!(
        "Docker daemon config validation failed: {}",
        command_detail(&output)
    )))
}

async fn restart_docker() -> Result<(), String> {
    run_systemctl(&["restart", "docker"]).await?;
    run_systemctl(&["is-active", "--quiet", "docker"]).await
}

async fn run_systemctl(args: &[&str]) -> Result<(), String> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .await
        .map_err(|err| format!("failed to run systemctl: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(command_detail(&output))
}

async fn restore_original_config(path: &Path, original: Option<&[u8]>) -> Result<(), String> {
    if let Some(content) = original {
        let staged_path = staged_config_path(path, "rollback");
        write_config_file(&staged_path, content)
            .await
            .map_err(|err| err.message.to_string())?;
        tokio::fs::rename(&staged_path, path)
            .await
            .map_err(|err| format!("failed to restore Docker daemon config: {err}"))
    } else {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("failed to remove Docker daemon config: {err}")),
        }
    }
}

async fn remove_file_if_exists(path: &Path) {
    if let Err(err) = tokio::fs::remove_file(path).await
        && err.kind() != ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %err, "failed to remove staged daemon config");
    }
}

fn command_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        return stderr.trim().to_string();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        return stdout.trim().to_string();
    }
    match output.status.code() {
        Some(code) => format!("command exited with code {code}"),
        None => "command terminated without an exit code".to_string(),
    }
}

fn format_result(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "succeeded".to_string(),
        Err(err) => err,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DockerDaemonSettings, extract_daemon_settings, merge_settings,
        normalize_and_validate_settings,
    };
    use serde_json::json;

    #[test]
    fn merge_settings_preserves_unmanaged_fields() {
        let config = json!({
            "data-root": "/var/lib/docker-custom",
            "registry-mirrors": ["https://old.example.com"]
        });
        let settings = DockerDaemonSettings {
            registry_mirrors: vec!["https://mirror.example.com".to_string()],
            proxy: "http://user:pass@10.0.0.1:7890".to_string(),
            proxy_enabled: true,
        };

        let merged = merge_settings(config, &settings);

        assert_eq!(merged["data-root"], "/var/lib/docker-custom");
        assert_eq!(
            merged["registry-mirrors"],
            json!(["https://mirror.example.com"])
        );
        assert_eq!(
            merged["proxies"]["http-proxy"],
            "http://user:pass@10.0.0.1:7890"
        );
        assert_eq!(
            merged["proxies"]["https-proxy"],
            "http://user:pass@10.0.0.1:7890"
        );
    }

    #[test]
    fn merge_settings_removes_managed_empty_fields() {
        let config = json!({
            "debug": true,
            "registry-mirrors": ["https://old.example.com"],
            "proxies": {"http-proxy": "http://proxy.example.com"}
        });

        let merged = merge_settings(config, &DockerDaemonSettings::default());

        assert_eq!(merged["debug"], true);
        assert!(merged.get("registry-mirrors").is_none());
        assert!(merged.get("proxies").is_none());
    }

    #[test]
    fn extract_settings_rejects_different_proxy_values() {
        let config = json!({
            "proxies": {
                "http-proxy": "http://proxy-a.example.com",
                "https-proxy": "http://proxy-b.example.com"
            }
        });

        assert!(extract_daemon_settings(&config).is_err());
    }

    #[test]
    fn disabled_proxy_keeps_address_but_removes_daemon_proxy() {
        let config = json!({
            "proxies": {"http-proxy": "http://old.example.com:7890"}
        });
        let settings = DockerDaemonSettings {
            registry_mirrors: Vec::new(),
            proxy: "http://user:pass@127.0.0.1:7890".to_string(),
            proxy_enabled: false,
        };

        let merged = merge_settings(config, &settings);

        assert!(merged.get("proxies").is_none());
        assert_eq!(settings.proxy, "http://user:pass@127.0.0.1:7890");
    }

    #[test]
    fn malformed_proxy_returns_actionable_error() {
        let mut settings = DockerDaemonSettings {
            registry_mirrors: Vec::new(),
            proxy: "http://gwj@1321:10.0.0.254:7890".to_string(),
            proxy_enabled: true,
        };

        let error = normalize_and_validate_settings(&mut settings).unwrap_err();

        assert_eq!(error.message, "invalid proxy address format");
    }
}
