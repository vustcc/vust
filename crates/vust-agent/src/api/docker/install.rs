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
    let result: ApiResult<Response> = async {
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

        if result.exit_code != 0 {
            return Err(ApiError::Internal(format!(
                "Docker install exited with code {}: {}",
                result.exit_code,
                result.stderr.trim()
            )));
        }
        Ok(ApiResponse::success_with_raw("Docker install finished", Some(result)).into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_engine_install",
            Some(("dockerEngine", "docker")),
            json!({}),
            false,
            result,
        )
        .await
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
    use super::build_install_script;

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
