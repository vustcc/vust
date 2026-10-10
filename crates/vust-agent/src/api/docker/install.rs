//! Docker 安装 API：按官方软件仓库安装 Docker Engine。

use crate::api::docker::context::DockerOperationContext;
use crate::types::{ApiError, ApiResponse, ApiResult};
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, fs, sync::Arc};
use tokio::process::Command;
use vust_contracts::api::ErrorCode;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DockerInstallPayload {
    pub mirror: Option<String>,
    pub timeout_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerInstallResult {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub started_at: i64,
    pub finished_at: i64,
}

#[derive(Debug)]
struct OsRelease {
    id: String,
    version_codename: Option<String>,
    ubuntu_codename: Option<String>,
}

impl OsRelease {
    fn suite(&self) -> Option<&str> {
        self.ubuntu_codename
            .as_deref()
            .or(self.version_codename.as_deref())
    }
}

/// 使用 Docker 官方 apt repository 安装 Docker Engine。
///
/// 此接口不接受前端传入的 shell 脚本；agent 端只执行内置安装流程。
pub async fn install(
    State(state): State<Arc<crate::state::AppState>>,
    context: DockerOperationContext,
    Json(payload): Json<DockerInstallPayload>,
) -> ApiResult<Response> {
    if payload.mirror.as_deref().unwrap_or("official") != "official" {
        return Err(ApiError::BadRequest(
            "only official Docker repository is supported".to_string(),
        ));
    }
    let result: ApiResult<DockerInstallResult> = async {
        let os = read_os_release()?;
        let repo = match os.id.as_str() {
            "ubuntu" => "ubuntu",
            "debian" => "debian",
            other => {
                return Err(ApiError::BadRequest(format!(
                    "unsupported distro for Docker installation: {other}"
                )));
            }
        };
        let suite = os.suite().ok_or_else(|| {
            ApiError::BadRequest("os-release does not contain VERSION_CODENAME".to_string())
        })?;
        let arch = docker_arch()?;
        let timeout_secs = payload.timeout_secs.unwrap_or(600).clamp(60, 1_800) as u64;
        let started_at = chrono::Utc::now().timestamp();
        let script = build_install_script(repo, suite, &arch);

        let output = Command::new("/usr/bin/timeout")
            .arg(format!("{}s", timeout_secs))
            .arg("/bin/bash")
            .arg("-lc")
            .arg(script)
            .output()
            .await
            .map_err(|err| ApiError::Internal(format!("failed to install Docker: {err}")))?;

        let exit_code = output.status.code().unwrap_or(-1) as i64;
        let result = DockerInstallResult {
            exit_code,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            timed_out: exit_code == 124,
            started_at,
            finished_at: chrono::Utc::now().timestamp(),
        };

        Ok(result)
    }
    .await;
    finish_install(&state.metadata_db, &context, result).await
}

/// 根据真实执行结果写入审计，完整保留已结束进程的输出。
async fn finish_install(
    pool: &crate::state::DbPool,
    context: &DockerOperationContext,
    result: ApiResult<DockerInstallResult>,
) -> ApiResult<Response> {
    let target = Some(("dockerEngine", "docker"));
    match result {
        Ok(result) => {
            let parameters = json!({
                "exitCode": result.exit_code,
                "timedOut": result.timed_out,
            });
            if result.exit_code == 0 && !result.timed_out {
                context
                    .record_success(pool, "docker_engine_install", target, parameters, false)
                    .await;
            } else {
                let output = if result.stderr.trim().is_empty() {
                    result.stdout.trim()
                } else {
                    result.stderr.trim()
                };
                context
                    .record_failure(
                        pool,
                        "docker_engine_install",
                        target,
                        parameters,
                        format!(
                            "Docker install exited with code {} (timed_out={}): {}",
                            result.exit_code, result.timed_out, output
                        ),
                    )
                    .await;
            }
            Ok(ApiResponse::success_with_raw("Docker install finished", result).into_response())
        }
        Err(error) => {
            context
                .record_failure(
                    pool,
                    "docker_engine_install",
                    target,
                    json!({}),
                    error.detail.as_deref().unwrap_or(&error.message),
                )
                .await;
            if error.status.is_server_error() {
                Err(ApiError::bad_gateway(
                    ErrorCode::DockerOperationFailed,
                    "Docker installation could not be executed; see operation log for details",
                )
                .with_detail(error.detail.unwrap_or(error.message)))
            } else {
                Err(error)
            }
        }
    }
}

fn read_os_release() -> ApiResult<OsRelease> {
    let content = fs::read_to_string("/etc/os-release")
        .map_err(|err| ApiError::Internal(format!("failed to read /etc/os-release: {err}")))?;
    let mut values = HashMap::new();
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        values.insert(
            key.to_string(),
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string(),
        );
    }
    let id = values
        .get("ID")
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("os-release does not contain ID".to_string()))?;
    Ok(OsRelease {
        id,
        version_codename: values.get("VERSION_CODENAME").cloned(),
        ubuntu_codename: values.get("UBUNTU_CODENAME").cloned(),
    })
}

fn docker_arch() -> ApiResult<String> {
    let output = std::process::Command::new("dpkg")
        .arg("--print-architecture")
        .output()
        .map_err(|err| ApiError::Internal(format!("failed to detect dpkg architecture: {err}")))?;
    if !output.status.success() {
        return Err(ApiError::Internal(
            "failed to detect dpkg architecture".to_string(),
        ));
    }
    let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if arch.is_empty() {
        return Err(ApiError::Internal("dpkg architecture is empty".to_string()));
    }
    Ok(arch)
}

fn build_install_script(repo: &str, suite: &str, arch: &str) -> String {
    format!(
        r#"set -euo pipefail
if [ "$(id -u)" -eq 0 ]; then
  SUDO=""
else
  sudo -n true
  SUDO="sudo -n"
fi
echo "[VUST] Installing Docker Engine from Docker official apt repository"
echo "[VUST] Repository: {repo}, suite: {suite}, architecture: {arch}"
export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a
$SUDO env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get update
$SUDO env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get install -y ca-certificates curl
$SUDO install -m 0755 -d /etc/apt/keyrings
$SUDO curl -fsSL https://download.docker.com/linux/{repo}/gpg -o /etc/apt/keyrings/docker.asc
$SUDO chmod a+r /etc/apt/keyrings/docker.asc
cat <<'EOF' | $SUDO tee /etc/apt/sources.list.d/docker.sources >/dev/null
Types: deb
URIs: https://download.docker.com/linux/{repo}
Suites: {suite}
Components: stable
Architectures: {arch}
Signed-By: /etc/apt/keyrings/docker.asc
EOF
$SUDO env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get update
$SUDO env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
if command -v systemctl >/dev/null 2>&1; then
  $SUDO systemctl enable --now docker || $SUDO service docker start
else
  $SUDO service docker start
fi
$SUDO docker version
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use vust_contracts::logging::OperationOutcome;

    /// 使用模拟执行结果验证响应与审计，不执行系统安装命令。
    #[tokio::test]
    async fn completed_execution_preserves_output_and_actual_audit_outcome() {
        let pool = crate::test_support::setup_test_db().await;
        let context = DockerOperationContext::system("test");
        for (exit_code, timed_out, stdout, stderr) in [
            (0, false, "installed", ""),
            (100, false, "apt output", "repository unavailable"),
            (1, false, "failure only in stdout", ""),
            (124, true, "partial output", ""),
        ] {
            let response = finish_install(
                &pool,
                &context,
                Ok(DockerInstallResult {
                    exit_code,
                    timed_out,
                    stdout: stdout.to_string(),
                    stderr: stderr.to_string(),
                    started_at: 1,
                    finished_at: 2,
                }),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["data"]["exitCode"], exit_code);
            assert_eq!(body["data"]["timedOut"], timed_out);
            assert_eq!(body["data"]["stdout"], stdout);
            assert_eq!(body["data"]["stderr"], stderr);
        }
        let events = crate::services::operation_outbox::pending(&pool, 10)
            .await
            .unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].outcome, OperationOutcome::Success);
        assert!(events[0].error_summary.is_none());
        for (event, expected) in events[1..].iter().zip([
            "repository unavailable",
            "failure only in stdout",
            "timed_out=true",
        ]) {
            assert_eq!(event.outcome, OperationOutcome::Failure);
            assert!(event.error_summary.as_deref().unwrap().contains(expected));
            assert!(event.parameters.contains_key("exitCode"));
            assert!(event.parameters.contains_key("timedOut"));
        }
    }

    #[tokio::test]
    async fn execution_error_records_diagnostic_before_http_conversion() {
        let pool = crate::test_support::setup_test_db().await;
        let error = finish_install(
            &pool,
            &DockerOperationContext::system("test"),
            Err(ApiError::internal(
                "failed to install Docker: executable missing",
            )),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::DockerOperationFailed);
        let events = crate::services::operation_outbox::pending(&pool, 10)
            .await
            .unwrap();
        assert_eq!(events[0].outcome, OperationOutcome::Failure);
        assert_eq!(
            events[0].error_summary.as_deref(),
            Some("failed to install Docker: executable missing")
        );
    }

    #[test]
    fn install_script_uses_official_repository_without_remote_shell_script() {
        let script = build_install_script("ubuntu", "noble", "amd64");
        assert!(script.contains("https://download.docker.com/linux/ubuntu"));
        assert!(script.contains("docker-ce docker-ce-cli containerd.io"));
        assert!(!script.contains("get.docker.com"));
        assert!(!script.contains("linuxmirrors.cn"));
        assert!(!script.contains("| bash"));
        assert!(!script.contains("bash <("));
    }
}
