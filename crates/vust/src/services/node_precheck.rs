//! 节点预检服务：部署前的权限、端口与服务存在性检查。

use crate::config::AgentVersionCompatibilityConfig;
use crate::models::node_identities::get_identity_by_agent_id;
use crate::services::node_target_guard::{
    detect_vust_service, node_conflict_message, open_ssh_session, validate_ssh_auth_material,
};
use crate::state::DbPool;
use crate::types::{ApiError, ApiResult};
use semver::Version;
use serde::Serialize;
use ssh2::Session;
use std::io::Read;

const DEFAULT_SSH_PORT: &str = crate::config::DEFAULT_SSH_PORT;
const DEFAULT_LISTEN_PORT: &str = crate::config::DEFAULT_AGENT_PORT;
const CONTROLLER_VERSION: &str = env!("CARGO_PKG_VERSION");
const REMOTE_AGENT_INSPECT_SCRIPT: &str = r#"sh -c '
home=""
for unit in /etc/systemd/system/vust-agent.service /usr/lib/systemd/system/vust-agent.service /lib/systemd/system/vust-agent.service; do
  if [ -f "$unit" ]; then
    detected=$(grep -h "^Environment=VUST_HOME=" "$unit" 2>/dev/null | tail -n 1 | sed "s/^Environment=VUST_HOME=//")
    if [ -n "$detected" ]; then home="$detected"; fi
  fi
done
if [ -z "$home" ] && [ -f /opt/vust/config/agent.install_dir ]; then
  home=$(cat /opt/vust/config/agent.install_dir 2>/dev/null | head -n 1)
fi
[ -n "$home" ] || home="/opt/vust"
config="$home/config/agent.toml"
[ -f "$config" ] || config="/opt/vust/config/agent.toml"
present="clean"
if [ -f /etc/systemd/system/vust-agent.service ] || [ -f /usr/lib/systemd/system/vust-agent.service ] || [ -f /lib/systemd/system/vust-agent.service ] || [ -x /usr/local/bin/vust-agent ] || [ -x /usr/bin/vust-agent ] || [ -f "$config" ]; then
  present="exists"
fi
read_config() {
  key="$1"
  if [ -f "$config" ]; then
    grep -E "^[[:space:]]*$key[[:space:]]*=" "$config" 2>/dev/null | tail -n 1 | sed -E "s/^[^=]+=//" | sed -E "s/^[[:space:]]*//" | sed -E "s/[[:space:]]*$//" | sed -E "s/^\"//" | sed -E "s/\"$//"
  fi
}
agent_id=$(read_config agentId)
vust_url=$(read_config vustUrl)
role=""
if [ -f "$home/config/node.role" ]; then
  role=$(cat "$home/config/node.role" 2>/dev/null | head -n 1)
fi
version=""
if command -v timeout >/dev/null 2>&1; then
  for bin in /usr/local/bin/vust-agent /usr/bin/vust-agent; do
    if [ -x "$bin" ]; then
      version=$(timeout 2 "$bin" --version 2>/dev/null | head -n 1)
      if [ -n "$version" ]; then break; fi
    fi
  done
fi
printf "present=%s\n" "$present"
printf "agentId=%s\n" "$agent_id"
printf "vustUrl=%s\n" "$vust_url"
printf "installDir=%s\n" "$home"
printf "nodeRole=%s\n" "$role"
printf "version=%s\n" "$version"
'"#;

/// 单项节点预检结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePrecheckDetail {
    pub status: NodePrecheckStatus,
    pub message: String,
}

/// 单项节点预检状态。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodePrecheckStatus {
    Passed,
    Warning,
    Failed,
    Skipped,
}

impl NodePrecheckDetail {
    fn passed(message: impl Into<String>) -> Self {
        Self {
            status: NodePrecheckStatus::Passed,
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self {
            status: NodePrecheckStatus::Warning,
            message: message.into(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            status: NodePrecheckStatus::Failed,
            message: message.into(),
        }
    }

    fn skipped(message: impl Into<String>) -> Self {
        Self {
            status: NodePrecheckStatus::Skipped,
            message: message.into(),
        }
    }

    fn is_failed(&self) -> bool {
        self.status == NodePrecheckStatus::Failed
    }
}

/// 节点预检汇总结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePrecheckResponse {
    pub passed: bool,
    pub agent_status: AgentPrecheckStatus,
    pub version_compatibility: VersionCompatibility,
    pub ssh: NodePrecheckDetail,
    pub os: NodePrecheckDetail,
    pub permission: NodePrecheckDetail,
    pub service: NodePrecheckDetail,
    pub systemd: NodePrecheckDetail,
    pub directory: NodePrecheckDetail,
    pub docker: NodePrecheckDetail,
    pub port: NodePrecheckDetail,
    pub callback: NodePrecheckDetail,
}

/// 目标机上已有 agent 的归属与阻断分类。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatusKind {
    Clean,
    CurrentController,
    OtherController,
    ResidualInstall,
    ControllerConflict,
    VersionIncompatible,
}

/// 节点预检中的 agent 归属判断结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPrecheckStatus {
    pub kind: AgentStatusKind,
    pub blocking: bool,
    pub message: String,
    pub required_action: String,
    pub detected_agent_id: Option<String>,
    pub detected_vust_url: Option<String>,
    pub detected_version: Option<String>,
    pub existing_node_id: Option<String>,
    pub install_dir: Option<String>,
    pub node_role: Option<String>,
}

/// 主控与远端 agent 的版本兼容判断。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionCompatibility {
    pub controller_version: String,
    pub agent_version: Option<String>,
    pub compatible: bool,
    pub reason: String,
    pub required_action: String,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteAgentInspection {
    pub present: bool,
    pub agent_id: Option<String>,
    pub vust_url: Option<String>,
    pub install_dir: Option<String>,
    pub node_role: Option<String>,
    pub version: Option<String>,
}

/// 节点预检所需的目标地址与认证配置。
#[derive(Debug, Clone)]
pub struct NodePrecheckInput {
    pub addr: String,
    pub port: Option<String>,
    pub user: String,
    pub auth_mode: Option<String>,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub private_key_passphrase: Option<String>,
    pub service_port: Option<String>,
    pub install_dir: Option<String>,
    pub vust_url: Option<String>,
    pub expected_node_id: Option<String>,
}

struct RemotePrecheckCore {
    ssh: NodePrecheckDetail,
    os: NodePrecheckDetail,
    permission: NodePrecheckDetail,
    service: NodePrecheckDetail,
    systemd: NodePrecheckDetail,
    directory: NodePrecheckDetail,
    docker: NodePrecheckDetail,
    port: NodePrecheckDetail,
    callback: NodePrecheckDetail,
    remote_agent: RemoteAgentInspection,
    controller_conflict: bool,
}

/// 在阻塞线程中执行远程预检，并汇总为统一结果。
pub async fn precheck_node(
    pool: &DbPool,
    input: NodePrecheckInput,
) -> ApiResult<NodePrecheckResponse> {
    let expected_node_id = input.expected_node_id.clone();
    let allow_existing_agent = expected_node_id.is_some();
    let addr = input.addr.clone();
    let port = input
        .port
        .clone()
        .unwrap_or_else(|| DEFAULT_SSH_PORT.to_string());
    let user = input.user.clone();
    let auth_mode = input
        .auth_mode
        .clone()
        .unwrap_or_else(|| "password".to_string());
    let service_port = input
        .service_port
        .clone()
        .unwrap_or_else(|| DEFAULT_LISTEN_PORT.to_string());
    let install_dir = {
        let parent = input
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
    };
    let password = input.password.clone();
    let private_key = input.private_key.clone();
    let private_key_passphrase = input.private_key_passphrase.clone();
    // 接口防御性校验：认证材料缺失属于请求参数错误，不进入预检聚合流程。
    validate_ssh_auth_material(
        Some(&auth_mode),
        password.as_deref(),
        private_key.as_deref(),
    )?;

    let current_controller_url =
        crate::services::node_deploy::resolve_vust_url_override(input.vust_url.as_deref())?;
    let callback_probe_url = format!(
        "{}/api/v1/runtime/callback-probe",
        current_controller_url.trim_end_matches('/')
    );

    let core = tokio::task::spawn_blocking(move || -> ApiResult<RemotePrecheckCore> {
        let session = match open_ssh_session(
            &addr,
            Some(&port),
            &user,
            Some(&auth_mode),
            password.as_deref(),
            private_key.as_deref(),
            private_key_passphrase.as_deref(),
        ) {
            Ok(session) => session,
            Err(err) => {
                let message = ssh_precheck_error_message(&err);
                return Ok(RemotePrecheckCore {
                    ssh: NodePrecheckDetail::failed(message),
                    os: NodePrecheckDetail::skipped("Not executed"),
                    permission: NodePrecheckDetail::skipped("Not executed"),
                    service: NodePrecheckDetail::skipped("Not executed"),
                    systemd: NodePrecheckDetail::skipped("Not executed"),
                    directory: NodePrecheckDetail::skipped("Not executed"),
                    docker: NodePrecheckDetail::skipped("Not executed"),
                    port: NodePrecheckDetail::skipped("Not executed"),
                    callback: NodePrecheckDetail::skipped("Not executed"),
                    remote_agent: RemoteAgentInspection::default(),
                    controller_conflict: false,
                });
            }
        };

        let ssh_detail = NodePrecheckDetail::passed("SSH connection is healthy");
        let callback_detail = check_controller_callback(&session, &callback_probe_url)
            .unwrap_or_else(|err| {
                failed_precheck_detail(&err, "Failed to inspect controller callback")
            });
        if callback_detail.is_failed() {
            return Ok(RemotePrecheckCore {
                ssh: ssh_detail,
                os: NodePrecheckDetail::skipped("Not executed"),
                permission: NodePrecheckDetail::skipped("Not executed"),
                service: NodePrecheckDetail::skipped("Not executed"),
                systemd: NodePrecheckDetail::skipped("Not executed"),
                directory: NodePrecheckDetail::skipped("Not executed"),
                docker: NodePrecheckDetail::skipped("Not executed"),
                port: NodePrecheckDetail::skipped("Not executed"),
                callback: callback_detail,
                remote_agent: RemoteAgentInspection::default(),
                controller_conflict: false,
            });
        }

        let os_detail = check_os(&session)
            .unwrap_or_else(|err| failed_precheck_detail(&err, "Failed to inspect OS"));
        let permission_detail = check_user_permission(&session).unwrap_or_else(|err| {
            failed_precheck_detail(&err, "Failed to inspect user permission")
        });
        let systemd_detail = check_systemd(&session)
            .unwrap_or_else(|err| failed_precheck_detail(&err, "Failed to inspect systemd"));
        let directory_detail =
            check_directory_permission(&session, &install_dir).unwrap_or_else(|err| {
                failed_precheck_detail(&err, "Failed to inspect install directory")
            });
        let docker_detail = check_docker(&session)
            .unwrap_or_else(|err| warning_precheck_detail(&err, "Failed to inspect Docker"));
        let remote_agent_result = inspect_remote_agent(&session);
        let vust_conflict_result = detect_vust_service(&session);
        let (remote_agent, agent_inspection_error) = match remote_agent_result {
            Ok(remote_agent) => (remote_agent, None),
            Err(err) => (RemoteAgentInspection::default(), Some(err)),
        };
        let (vust_conflict, controller_inspection_error) = match vust_conflict_result {
            Ok(vust_conflict) => (vust_conflict, None),
            Err(err) => (false, Some(err)),
        };
        let service_inspection_error = agent_inspection_error.or(controller_inspection_error);
        if let Some(err) = service_inspection_error {
            let service_detail = failed_precheck_detail(&err, "Failed to inspect existing service");
            let port_detail = check_port_available(&session, &service_port, allow_existing_agent)
                .unwrap_or_else(|err| {
                    failed_precheck_detail(&err, "Failed to inspect service port")
                });
            return Ok(RemotePrecheckCore {
                ssh: ssh_detail,
                os: os_detail,
                permission: permission_detail,
                service: service_detail,
                systemd: systemd_detail,
                directory: directory_detail,
                docker: docker_detail,
                port: port_detail,
                callback: callback_detail,
                remote_agent,
                controller_conflict: false,
            });
        }
        if vust_conflict {
            let blocked_detail = NodePrecheckDetail::failed(node_conflict_message());
            let port_detail = check_port_available(&session, &service_port, allow_existing_agent)
                .unwrap_or_else(|err| {
                    failed_precheck_detail(&err, "Failed to inspect service port")
                });
            return Ok(RemotePrecheckCore {
                ssh: ssh_detail,
                os: os_detail,
                permission: permission_detail,
                service: blocked_detail,
                systemd: systemd_detail,
                directory: directory_detail,
                docker: docker_detail,
                port: port_detail,
                callback: callback_detail,
                remote_agent,
                controller_conflict: true,
            });
        }
        let service_detail = if allow_existing_agent && remote_agent.present {
            NodePrecheckDetail::warning("Existing managed vust-agent will be replaced")
        } else {
            check_agent_absence(&remote_agent)
        };
        let port_detail = check_port_available(&session, &service_port, allow_existing_agent)
            .unwrap_or_else(|err| failed_precheck_detail(&err, "Failed to inspect service port"));

        Ok(RemotePrecheckCore {
            ssh: ssh_detail,
            os: os_detail,
            permission: permission_detail,
            service: service_detail,
            systemd: systemd_detail,
            directory: directory_detail,
            docker: docker_detail,
            port: port_detail,
            callback: callback_detail,
            remote_agent,
            controller_conflict: false,
        })
    })
    .await
    .map_err(|err| ApiError::Internal(err.to_string()))??;

    let version_compatibility = decide_version_compatibility(
        core.remote_agent.version.as_deref(),
        &crate::config::get().agent_version_compatibility,
    );
    let agent_status = classify_agent_status(
        pool,
        &core.remote_agent,
        core.controller_conflict,
        &current_controller_url,
        &version_compatibility,
    )
    .await?;
    let expected_existing_agent = expected_node_id.is_some()
        && agent_status.kind == AgentStatusKind::CurrentController
        && agent_status.existing_node_id.as_deref() == expected_node_id.as_deref();
    let passed = !core.ssh.is_failed()
        && !core.os.is_failed()
        && !core.permission.is_failed()
        && !core.service.is_failed()
        && !core.systemd.is_failed()
        && !core.directory.is_failed()
        && !core.port.is_failed()
        && !core.callback.is_failed()
        && (!agent_status.blocking || expected_existing_agent);

    Ok(NodePrecheckResponse {
        passed,
        agent_status,
        version_compatibility,
        ssh: core.ssh,
        os: core.os,
        permission: core.permission,
        service: core.service,
        systemd: core.systemd,
        directory: core.directory,
        docker: core.docker,
        port: core.port,
        callback: core.callback,
    })
}

fn ssh_precheck_error_message(err: &ApiError) -> String {
    let message = err.message.as_ref();
    if message == "internal server error" {
        "SSH session failed".to_string()
    } else {
        message.to_string()
    }
}

fn failed_precheck_detail(err: &ApiError, fallback: &'static str) -> NodePrecheckDetail {
    let message = err.message.as_ref();
    if message == "internal server error" {
        NodePrecheckDetail::failed(fallback)
    } else {
        NodePrecheckDetail::failed(message)
    }
}

fn warning_precheck_detail(err: &ApiError, fallback: &'static str) -> NodePrecheckDetail {
    let message = err.message.as_ref();
    if message == "internal server error" {
        NodePrecheckDetail::warning(fallback)
    } else {
        NodePrecheckDetail::warning(message)
    }
}

fn check_os(session: &Session) -> ApiResult<NodePrecheckDetail> {
    let output = match run_remote_capture(
        session,
        "sh -c 'if [ -f /etc/os-release ]; then cat /etc/os-release; \
        elif [ -f /etc/redhat-release ]; then cat /etc/redhat-release; \
        else uname -sr; fi'",
    ) {
        Ok(out) => out,
        Err(err) => {
            return Ok(failed_precheck_detail(&err, "Failed to read OS release"));
        }
    };

    let mut pretty_name = String::new();
    let mut id = String::new();

    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("PRETTY_NAME=") {
            pretty_name = line
                .trim_start_matches("PRETTY_NAME=")
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        } else if line.starts_with("ID=") {
            id = line
                .trim_start_matches("ID=")
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        }
    }

    if pretty_name.is_empty()
        && let Some(first_line) = output.lines().next()
    {
        pretty_name = first_line.trim().to_string();
    }

    if pretty_name.is_empty() {
        pretty_name = "Unknown Linux".to_string();
    }

    let message = if !id.is_empty() {
        format!("{} ({})", pretty_name, id)
    } else {
        pretty_name
    };

    Ok(NodePrecheckDetail::passed(message))
}

fn check_user_permission(session: &Session) -> ApiResult<NodePrecheckDetail> {
    let output = run_remote_capture(
        session,
        "sh -c 'if [ \"$(id -u)\" -eq 0 ]; then echo root; \
        elif sudo -n true >/dev/null 2>&1; then echo sudo; \
        else echo nosudo; fi'",
    )?;
    match output.trim() {
        "root" => Ok(NodePrecheckDetail {
            status: NodePrecheckStatus::Passed,
            message: "Root user".to_string(),
        }),
        "sudo" => Ok(NodePrecheckDetail::passed("Sudo permission is available")),
        _ => Ok(NodePrecheckDetail::failed(
            "User has no sudo permission or requires interaction",
        )),
    }
}

fn check_agent_absence(remote_agent: &RemoteAgentInspection) -> NodePrecheckDetail {
    if remote_agent.present {
        NodePrecheckDetail::failed("Existing vust-agent detected; new node deployment is blocked")
    } else {
        NodePrecheckDetail::passed("No existing vust-agent detected")
    }
}

fn check_port_available(
    session: &Session,
    service_port: &str,
    allow_occupied: bool,
) -> ApiResult<NodePrecheckDetail> {
    let command = format!(
        "sh -c 'if command -v ss >/dev/null 2>&1; then \
        if ss -ltn | grep -q \":{port} \"; then echo busy; else echo free; fi; \
        elif command -v netstat >/dev/null 2>&1; then \
        if netstat -ltn | grep -q \":{port} \"; then echo busy; else echo free; fi; \
        else echo unknown; fi'",
        port = service_port
    );
    let output = run_remote_capture(session, &command)?;
    match output.trim() {
        "free" => Ok(NodePrecheckDetail {
            status: NodePrecheckStatus::Passed,
            message: "Port is available".to_string(),
        }),
        "busy" if allow_occupied => Ok(NodePrecheckDetail::warning(
            "Port is occupied by an existing service",
        )),
        "busy" => Ok(NodePrecheckDetail::failed("Port is occupied")),
        _ => Ok(NodePrecheckDetail::warning(
            "Port inspection tool was not detected; check skipped",
        )),
    }
}

fn check_systemd(session: &Session) -> ApiResult<NodePrecheckDetail> {
    let output = run_remote_capture(
        session,
        "sh -c 'if command -v systemctl >/dev/null 2>&1; then echo ok; else echo missing; fi'",
    )?;
    match output.trim() {
        "ok" => Ok(NodePrecheckDetail {
            status: NodePrecheckStatus::Passed,
            message: "Systemd is available".to_string(),
        }),
        _ => Ok(NodePrecheckDetail::failed("Systemd is unavailable")),
    }
}

fn check_directory_permission(
    session: &Session,
    install_dir: &str,
) -> ApiResult<NodePrecheckDetail> {
    let command = format!(
        "sh -c 'mkdir -p {dir} >/dev/null 2>&1 && [ -w {dir} ] && echo ok || echo denied'",
        dir = shell_escape(install_dir)
    );
    let output = run_remote_capture(session, &command)?;
    match output.trim() {
        "ok" => Ok(NodePrecheckDetail {
            status: NodePrecheckStatus::Passed,
            message: "Install directory is writable".to_string(),
        }),
        _ => Ok(NodePrecheckDetail::failed(
            "Install directory is not writable",
        )),
    }
}

fn check_docker(session: &Session) -> ApiResult<NodePrecheckDetail> {
    let output = run_remote_capture(
        session,
        "sh -c 'if ! command -v docker >/dev/null 2>&1; then echo missing; elif docker info >/dev/null 2>&1; then echo running; else echo no_daemon; fi'",
    )?;
    match output.trim() {
        "running" => Ok(NodePrecheckDetail {
            status: NodePrecheckStatus::Passed,
            message: "Docker is available".to_string(),
        }),
        "no_daemon" => Ok(NodePrecheckDetail::warning("Docker daemon is unavailable")),
        _ => Ok(NodePrecheckDetail::warning("Docker is not installed")),
    }
}

fn check_controller_callback(
    session: &Session,
    callback_probe_url: &str,
) -> ApiResult<NodePrecheckDetail> {
    let command = format!(
        "sh -c 'url={url}; \
        if command -v curl >/dev/null 2>&1; then \
          curl -k --connect-timeout 5 --max-time 10 -fsS -o /dev/null \"$url\" >/dev/null 2>&1 && echo ok || echo failed; \
        elif command -v wget >/dev/null 2>&1; then \
          wget --no-check-certificate -q --timeout=10 --tries=1 --spider \"$url\" >/dev/null 2>&1 && echo ok || echo failed; \
        else \
          echo missing; \
        fi'",
        url = shell_escape(callback_probe_url)
    );
    let output = run_remote_capture(session, &command)?;
    match output.trim() {
        "ok" => Ok(NodePrecheckDetail::passed(
            "Controller callback URL is reachable from target node",
        )),
        "missing" => Ok(NodePrecheckDetail::failed(
            "Callback probe requires curl or wget on target node",
        )),
        _ => Ok(NodePrecheckDetail::failed(
            "Controller callback URL is not reachable from target node",
        )),
    }
}

pub fn inspect_remote_agent(session: &Session) -> ApiResult<RemoteAgentInspection> {
    parse_remote_agent_inspection(&run_remote_capture(session, REMOTE_AGENT_INSPECT_SCRIPT)?)
}

fn parse_remote_agent_inspection(output: &str) -> ApiResult<RemoteAgentInspection> {
    let mut inspection = RemoteAgentInspection::default();
    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = normalize_optional(value);
        match key {
            "present" => inspection.present = value.as_deref() == Some("exists"),
            "agentId" => inspection.agent_id = value,
            "vustUrl" => inspection.vust_url = value,
            "installDir" => inspection.install_dir = value,
            "nodeRole" => inspection.node_role = value,
            "version" => inspection.version = value.map(normalize_agent_version),
            _ => {}
        }
    }
    Ok(inspection)
}

fn normalize_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_agent_version(value: String) -> String {
    value
        .split_whitespace()
        .last()
        .unwrap_or(value.as_str())
        .trim_start_matches('v')
        .to_string()
}

async fn classify_agent_status(
    pool: &DbPool,
    remote_agent: &RemoteAgentInspection,
    controller_conflict: bool,
    current_controller_url: &str,
    version_compatibility: &VersionCompatibility,
) -> ApiResult<AgentPrecheckStatus> {
    if controller_conflict {
        return Ok(build_agent_status(
            AgentStatusKind::ControllerConflict,
            true,
            node_conflict_message(),
            "abort",
            remote_agent,
            None,
        ));
    }
    if !remote_agent.present {
        return Ok(build_agent_status(
            AgentStatusKind::Clean,
            false,
            "Target host is clean and can be deployed",
            "deploy",
            remote_agent,
            None,
        ));
    }

    let Some(agent_id) = remote_agent.agent_id.as_deref() else {
        return Ok(build_agent_status(
            AgentStatusKind::ResidualInstall,
            true,
            "Existing vust-agent installation has no agent identity; clean or uninstall it before adding a node",
            "uninstall",
            remote_agent,
            None,
        ));
    };
    let Some(vust_url) = remote_agent.vust_url.as_deref() else {
        return Ok(build_agent_status(
            AgentStatusKind::ResidualInstall,
            true,
            "Existing vust-agent installation has no controller URL; clean or uninstall it before adding a node",
            "uninstall",
            remote_agent,
            None,
        ));
    };

    if normalize_controller_url(vust_url) != normalize_controller_url(current_controller_url) {
        return Ok(build_agent_status(
            AgentStatusKind::OtherController,
            true,
            "Existing vust-agent belongs to another controller; takeover is not supported",
            "abort",
            remote_agent,
            None,
        ));
    }

    let identity = get_identity_by_agent_id(pool, agent_id).await?;
    let Some(identity) = identity else {
        return Ok(build_agent_status(
            AgentStatusKind::ResidualInstall,
            true,
            "Existing vust-agent points to this controller but has no matching identity record; repair or uninstall it before adding a node",
            "repair",
            remote_agent,
            None,
        ));
    };

    if !version_compatibility.compatible {
        return Ok(build_agent_status(
            AgentStatusKind::VersionIncompatible,
            true,
            &version_compatibility.reason,
            &version_compatibility.required_action,
            remote_agent,
            Some(identity.node_id),
        ));
    }

    Ok(build_agent_status(
        AgentStatusKind::CurrentController,
        true,
        "Existing vust-agent is already managed by this controller; use the existing node instead of adding a duplicate",
        "view_existing_node",
        remote_agent,
        Some(identity.node_id),
    ))
}

fn build_agent_status(
    kind: AgentStatusKind,
    blocking: bool,
    message: &str,
    required_action: &str,
    remote_agent: &RemoteAgentInspection,
    existing_node_id: Option<String>,
) -> AgentPrecheckStatus {
    AgentPrecheckStatus {
        kind,
        blocking,
        message: message.to_string(),
        required_action: required_action.to_string(),
        detected_agent_id: remote_agent.agent_id.clone(),
        detected_vust_url: remote_agent.vust_url.clone(),
        detected_version: remote_agent.version.clone(),
        existing_node_id,
        install_dir: remote_agent.install_dir.clone(),
        node_role: remote_agent.node_role.clone(),
    }
}

fn normalize_controller_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn decide_version_compatibility(
    agent_version: Option<&str>,
    config: &AgentVersionCompatibilityConfig,
) -> VersionCompatibility {
    let controller_version = CONTROLLER_VERSION.to_string();
    let Some(agent_version) = agent_version.filter(|value| !value.trim().is_empty()) else {
        return VersionCompatibility {
            controller_version,
            agent_version: None,
            compatible: false,
            reason: "Existing agent version cannot be detected".to_string(),
            required_action: "repair".to_string(),
        };
    };
    let normalized_agent_version = normalize_agent_version(agent_version.to_string());
    let controller = match Version::parse(CONTROLLER_VERSION) {
        Ok(version) => version,
        Err(err) if config.require_semver => {
            return VersionCompatibility {
                controller_version,
                agent_version: Some(normalized_agent_version),
                compatible: false,
                reason: format!("Controller version is not valid SemVer: {err}"),
                required_action: "repair".to_string(),
            };
        }
        Err(_) => {
            return VersionCompatibility {
                controller_version,
                agent_version: Some(normalized_agent_version),
                compatible: false,
                reason: "Controller version is not SemVer; compatibility cannot be evaluated"
                    .to_string(),
                required_action: "repair".to_string(),
            };
        }
    };
    let agent = match Version::parse(&normalized_agent_version) {
        Ok(version) => version,
        Err(err) if config.require_semver => {
            return VersionCompatibility {
                controller_version,
                agent_version: Some(normalized_agent_version),
                compatible: false,
                reason: format!("Agent version is not valid SemVer: {err}"),
                required_action: "repair".to_string(),
            };
        }
        Err(_) => {
            return VersionCompatibility {
                controller_version,
                agent_version: Some(normalized_agent_version),
                compatible: false,
                reason: "Agent version is not SemVer; compatibility cannot be evaluated"
                    .to_string(),
                required_action: "repair".to_string(),
            };
        }
    };

    // 项目初期由于在线与局部升级兼容性需要，放宽了零主版本（0.x.x）的兼容限制。
    // 在 zero_major_requires_exact 为 false 时，仅要求首位主版本号一致；待以后项目文档或兼容矩阵明确后再行调整。
    let compatible = if controller.major == 0 {
        if config.zero_major_requires_exact {
            controller.major == agent.major
                && controller.minor == agent.minor
                && controller.patch == agent.patch
                && (!config.zero_major_requires_prerelease_match || controller.pre == agent.pre)
        } else {
            controller.major == agent.major
        }
    } else {
        (!config.stable_requires_same_major || controller.major == agent.major)
            && (!config.stable_disallow_agent_newer_than_controller || agent <= controller)
    };
    VersionCompatibility {
        controller_version,
        agent_version: Some(normalized_agent_version),
        compatible,
        reason: if compatible {
            "Agent version is compatible with this controller".to_string()
        } else {
            "Agent version is not compatible with this controller".to_string()
        },
        required_action: if compatible {
            "none".to_string()
        } else {
            "upgrade".to_string()
        },
    }
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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

fn map_ssh_err(err: ssh2::Error) -> ApiError {
    ApiError::Internal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        decide_version_compatibility, parse_remote_agent_inspection, ssh_precheck_error_message,
    };
    use crate::config::AgentVersionCompatibilityConfig;
    use crate::types::ApiError;

    #[test]
    fn parse_remote_agent_inspection_extracts_identity_fields() {
        let parsed = parse_remote_agent_inspection(
            "present=exists\nagentId=node-1\nvustUrl=https://controller:9443/\ninstallDir=/opt/vust\nnodeRole=agent\nversion=vust-agent 0.1.0-alpha.1\n",
        )
        .expect("inspection should parse");

        assert!(parsed.present);
        assert_eq!(parsed.agent_id.as_deref(), Some("node-1"));
        assert_eq!(parsed.vust_url.as_deref(), Some("https://controller:9443/"));
        assert_eq!(parsed.version.as_deref(), Some("0.1.0-alpha.1"));
    }

    #[test]
    fn current_zero_version_requires_exact_prerelease_match() {
        let config = AgentVersionCompatibilityConfig {
            zero_major_requires_exact: true,
            zero_major_requires_prerelease_match: true,
            ..Default::default()
        };
        let compatible = decide_version_compatibility(Some(env!("CARGO_PKG_VERSION")), &config);
        assert!(compatible.compatible);

        let incompatible = decide_version_compatibility(Some("0.1.0"), &config);
        assert!(!incompatible.compatible);
        assert_eq!(incompatible.required_action, "upgrade");
    }

    #[test]
    fn ssh_precheck_error_uses_public_message() {
        let err = ApiError::BadRequest("SSH authentication failed".to_string());
        assert_eq!(
            ssh_precheck_error_message(&err),
            "SSH authentication failed"
        );
    }

    #[test]
    fn ssh_precheck_error_hides_internal_detail() {
        let err = ApiError::Internal("[Session(-18)] Authentication failed".to_string());
        assert_eq!(ssh_precheck_error_message(&err), "SSH session failed");
    }
}
