//! 节点目标防护服务：用于阻止将已部署主控的机器注册为子节点。

use crate::types::{ApiError, ApiResult};
use ssh2::Session;
use std::collections::HashSet;
use std::fs;
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

const DEFAULT_SSH_PORT: &str = crate::config::DEFAULT_SSH_PORT;
const DETECT_VUST_SERVICE_SCRIPT: &str = r#"sh -c '
if systemctl is-active --quiet vust 2>/dev/null; then
  echo exists
  exit 0
fi

for unit in /etc/systemd/system/vust.service /usr/lib/systemd/system/vust.service /lib/systemd/system/vust.service; do
  if [ -f "$unit" ]; then
    echo exists
    exit 0
  fi
done

if [ -x /usr/local/bin/vust ] || [ -x /usr/bin/vust ]; then
  echo exists
  exit 0
fi

homes="/opt/vust"
for unit in /etc/systemd/system/vust.service /usr/lib/systemd/system/vust.service /lib/systemd/system/vust.service; do
  home=$(grep -h "^Environment=VUST_HOME=" "$unit" 2>/dev/null | tail -n 1 | sed "s/^Environment=VUST_HOME=//")
  if [ -n "$home" ]; then
    homes="$homes $home"
  fi
done

for home in $homes; do
  if [ -f "$home/config/vust.toml" ]; then
    echo exists
    exit 0
  fi
  if [ -f "$home/config/node.role" ] && grep -Eq "^(all|vust)$" "$home/config/node.role" 2>/dev/null; then
    echo exists
    exit 0
  fi
done

echo clean
'"#;

/// 远程主机校验所需的最小 SSH 参数集合。
#[derive(Debug, Clone)]
pub struct NodeTargetGuardInput {
    pub addr: String,
    pub port: Option<String>,
    pub user: String,
    pub auth_mode: Option<String>,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub private_key_passphrase: Option<String>,
}

/// 主控冲突提示文案，供预检、部署、复用注册统一复用。
pub fn node_conflict_message() -> &'static str {
    "Target host already has the vust control service installed; agent deployment or reuse is not allowed"
}

/// 校验节点目标地址，防止将当前 VUST 主控设备添加为子节点。
///
/// 该防护覆盖回环地址、泛监听地址、当前活跃网卡 IP，以及解析到本机地址的主机名。
pub async fn assert_target_not_current_host(addr: &str) -> ApiResult<()> {
    let host = normalize_target_host(addr);
    if host.is_empty() {
        return Err(ApiError::BadRequest(
            "node address must not be empty".to_string(),
        ));
    }

    if is_localhost_name(&host) {
        return Err(current_host_error());
    }

    let local_ips = collect_current_host_ips().await;
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() || ip.is_unspecified() || local_ips.contains(&ip) {
            return Err(current_host_error());
        }
        return Ok(());
    }

    if resolves_to_current_host(&host, local_ips).await {
        return Err(current_host_error());
    }

    Ok(())
}

/// 打开远程 SSH 会话，供多处流程复用。
pub fn open_ssh_session(
    addr: &str,
    port: Option<&str>,
    user: &str,
    auth_mode: Option<&str>,
    password: Option<&str>,
    private_key: Option<&str>,
    private_key_passphrase: Option<&str>,
) -> ApiResult<Session> {
    let ssh_port = port.unwrap_or(DEFAULT_SSH_PORT);
    let auth = validate_ssh_auth_material(auth_mode, password, private_key)?;
    let tcp = TcpStream::connect(format!("{addr}:{ssh_port}"))
        .map_err(|_| ApiError::BadRequest("SSH connection failed".to_string()))?;
    tcp.set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(ApiError::Io)?;
    tcp.set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(ApiError::Io)?;

    let mut session = Session::new().map_err(map_ssh_err)?;
    session.set_tcp_stream(tcp);
    session.handshake().map_err(map_ssh_err)?;

    match auth {
        SshAuthMaterial::Password(password) => {
            session
                .userauth_password(user, password)
                .map_err(map_ssh_err)?;
        }
        SshAuthMaterial::PrivateKey(input) => {
            let key = resolve_private_key_input(Some(input))?;
            session
                .userauth_pubkey_memory(user, None, &key, private_key_passphrase)
                .map_err(map_ssh_err)?;
        }
    }

    if !session.authenticated() {
        return Err(ApiError::BadRequest(
            "SSH authentication failed".to_string(),
        ));
    }

    Ok(session)
}

pub enum SshAuthMaterial<'a> {
    Password(&'a str),
    PrivateKey(&'a str),
}

pub fn validate_ssh_auth_material<'a>(
    auth_mode: Option<&str>,
    password: Option<&'a str>,
    private_key: Option<&'a str>,
) -> ApiResult<SshAuthMaterial<'a>> {
    match auth_mode.map(str::trim).filter(|mode| !mode.is_empty()) {
        Some("key") => {
            let key = private_key.map(str::trim).unwrap_or("");
            if key.is_empty() {
                return Err(ApiError::BadRequest(
                    "SSH private key must not be empty".to_string(),
                ));
            }
            Ok(SshAuthMaterial::PrivateKey(key))
        }
        Some("password") | None => {
            let pwd = password.unwrap_or("");
            if pwd.trim().is_empty() {
                return Err(ApiError::BadRequest(
                    "SSH password must not be empty".to_string(),
                ));
            }
            Ok(SshAuthMaterial::Password(pwd))
        }
        Some(mode) => Err(ApiError::BadRequest(format!(
            "unsupported SSH auth mode: {mode}"
        ))),
    }
}

fn resolve_private_key_input(input: Option<&str>) -> ApiResult<String> {
    let trimmed = input.map(str::trim).unwrap_or("");

    if trimmed.is_empty() {
        return Err(ApiError::BadRequest(
            "SSH private key must not be empty".to_string(),
        ));
    }

    if looks_like_private_key(trimmed) {
        return Ok(trimmed.to_string());
    }

    if looks_like_public_key_content(trimmed) {
        return Err(ApiError::BadRequest(
            "SSH public key content cannot be used for authentication; provide private key content or file path".to_string(),
        ));
    }

    let private_key_path = expand_home(trimmed);

    let key = fs::read_to_string(&private_key_path).map_err(|err| {
        ApiError::BadRequest(format!(
            "failed to read SSH private key: {} ({})",
            private_key_path.display(),
            err
        ))
    })?;

    if !looks_like_private_key(&key) {
        return Err(ApiError::BadRequest(format!(
            "file content is not a valid private key: {}",
            private_key_path.display()
        )));
    }

    Ok(key)
}

fn looks_like_private_key(value: &str) -> bool {
    let val = value.trim();
    val.contains("BEGIN OPENSSH PRIVATE KEY")
        || val.contains("BEGIN RSA PRIVATE KEY")
        || val.contains("BEGIN EC PRIVATE KEY")
        || val.contains("BEGIN DSA PRIVATE KEY")
        || val.contains("BEGIN PRIVATE KEY")
}

fn looks_like_public_key_content(value: &str) -> bool {
    let val = value.trim();
    val.starts_with("ssh-rsa ")
        || val.starts_with("ssh-ed25519 ")
        || val.starts_with("ecdsa-sha2-")
        || val.starts_with("sk-ssh-ed25519@openssh.com ")
        || val.starts_with("sk-ecdsa-sha2-nistp256@openssh.com ")
}

fn expand_home(path: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());

    if path == "~" {
        return PathBuf::from(home);
    }

    if let Some(rest) = path.strip_prefix("~/") {
        return PathBuf::from(home).join(rest);
    }

    PathBuf::from(path)
}

fn normalize_target_host(addr: &str) -> String {
    let trimmed = addr.trim();
    if trimmed.starts_with('[')
        && let Some(end) = trimmed.find(']')
    {
        return trimmed[1..end].trim().to_string();
    }

    if let Some((host, port)) = trimmed.rsplit_once(':')
        && !host.contains(':')
        && port.parse::<u16>().is_ok()
    {
        return host.trim().to_string();
    }

    trimmed.to_string()
}

fn is_localhost_name(host: &str) -> bool {
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    normalized == "localhost" || normalized == "localhost.localdomain"
}

fn current_host_error() -> ApiError {
    ApiError::BadRequest("current controller host cannot be added as a node".to_string())
}

async fn collect_current_host_ips() -> HashSet<IpAddr> {
    let mut ips = HashSet::from([
        IpAddr::from([127, 0, 0, 1]),
        IpAddr::from([0, 0, 0, 0]),
        IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]),
        IpAddr::from([0, 0, 0, 0, 0, 0, 0, 0]),
    ]);

    if let Ok(output) = Command::new("ip").args(["-j", "addr"]).output().await
        && output.status.success()
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        && let Some(interfaces) = value.as_array()
    {
        for interface in interfaces {
            let Some(addr_info) = interface.get("addr_info").and_then(|item| item.as_array())
            else {
                continue;
            };
            for addr in addr_info {
                if let Some(local) = addr.get("local").and_then(|item| item.as_str())
                    && let Ok(ip) = local.parse::<IpAddr>()
                {
                    ips.insert(ip);
                }
            }
        }
    }

    ips
}

async fn resolves_to_current_host(host: &str, local_ips: HashSet<IpAddr>) -> bool {
    let host = host.to_string();
    tokio::task::spawn_blocking(move || {
        (host.as_str(), 0)
            .to_socket_addrs()
            .map(|addrs| {
                addrs
                    .map(|addr| addr.ip())
                    .any(|ip| ip.is_loopback() || ip.is_unspecified() || local_ips.contains(&ip))
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// 检查远程是否存在 VUST 主控服务痕迹（运行中、服务文件、二进制或配置文件）。
pub fn detect_vust_service(session: &Session) -> ApiResult<bool> {
    let output = run_remote_capture(session, DETECT_VUST_SERVICE_SCRIPT)?;
    Ok(output.trim() == "exists")
}

/// 供“直接复用注册”等链路调用：在写入节点前阻止主控冲突。
pub async fn assert_target_not_node_host(input: NodeTargetGuardInput) -> ApiResult<()> {
    tokio::task::spawn_blocking(move || -> ApiResult<()> {
        let session = open_ssh_session(
            &input.addr,
            input.port.as_deref(),
            &input.user,
            input.auth_mode.as_deref(),
            input.password.as_deref(),
            input.private_key.as_deref(),
            input.private_key_passphrase.as_deref(),
        )?;

        if detect_vust_service(&session)? {
            return Err(ApiError::BadRequest(node_conflict_message().to_string()));
        }
        Ok(())
    })
    .await
    .map_err(|err| ApiError::Internal(err.to_string()))?
}

fn run_remote_capture(session: &Session, command: &str) -> ApiResult<String> {
    use std::io::Read;

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
    if err
        .to_string()
        .to_ascii_lowercase()
        .contains("authentication failed")
    {
        return ApiError::BadRequest("SSH authentication failed".to_string());
    }
    ApiError::Internal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{SshAuthMaterial, normalize_target_host, validate_ssh_auth_material};

    #[test]
    fn password_mode_rejects_empty_password() {
        let result = validate_ssh_auth_material(Some("password"), Some("  "), None);

        assert!(result.is_err());
    }

    #[test]
    fn key_mode_rejects_empty_private_key() {
        let result = validate_ssh_auth_material(Some("key"), None, Some("  "));

        assert!(result.is_err());
    }

    #[test]
    fn unknown_auth_mode_is_rejected() {
        let result = validate_ssh_auth_material(Some("agent"), Some("secret"), None);

        assert!(result.is_err());
    }

    #[test]
    fn password_mode_preserves_non_empty_password() {
        let result = validate_ssh_auth_material(Some("password"), Some(" secret "), None)
            .expect("password auth should be accepted");

        match result {
            SshAuthMaterial::Password(password) => assert_eq!(password, " secret "),
            SshAuthMaterial::PrivateKey(_) => panic!("expected password auth"),
        }
    }

    #[test]
    fn target_host_normalization_strips_port_and_ipv6_brackets() {
        assert_eq!(normalize_target_host("192.168.1.10:22"), "192.168.1.10");
        assert_eq!(normalize_target_host("[::1]:22"), "::1");
        assert_eq!(normalize_target_host("2001:db8::1"), "2001:db8::1");
    }
}
