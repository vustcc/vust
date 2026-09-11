//! `vustctl` 命令行工具，用于管理和操作 VUST 服务。
//! 支持服务的停止、重启、状态查询、系统信息展示及服务的卸载等操作。

use anyhow::Result;
use clap::{Parser, Subcommand};
use rusqlite::OpenFlags;
use serde::Deserialize;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const RESERVED_SAFE_ENTRY_PREFIXES: &[&str] = &[
    "api", "assets", "images", "favicon", "static", "public", "health", "metrics", "ws", "wss",
    "robots",
];
const NODE_UNINSTALL_PREVIEW_LIMIT: usize = 5;

/// 卸载守卫展示的非本地节点摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
struct NonLocalNodeSummary {
    name: String,
    status: String,
}

/// 主控数据库中的非本地节点统计结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NonLocalNodeInventory {
    total: i64,
    preview: Vec<NonLocalNodeSummary>,
}

#[derive(Parser)]
#[command(name = "vustctl")]
#[command(version, about = "VUST control tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 停止当前节点的全部服务，或仅停止指定服务
    Stop {
        /// 可选目标服务名称 (vust 或 agent)
        target: Option<String>,
    },
    /// 重启指定的服务 (vust 或 agent)
    Restart {
        /// 目标服务名称 (vust 或 agent)
        target: String,
    },
    /// 显示服务运行状态
    Status,
    /// 显示节点和系统信息
    Info,
    /// 修改管理员密码
    Passwd {
        /// 同时修改用户名
        #[arg(long)]
        username: Option<String>,
        /// 新密码；不传则交互输入
        password: Option<String>,
    },
    /// 修改管理员用户名
    User {
        /// 新用户名
        #[arg(long)]
        username: String,
    },
    /// 管理安全入口
    Entry {
        /// 重新生成安全入口
        #[arg(long)]
        regenerate: bool,
        /// 设置自定义安全入口
        #[arg(long)]
        set: Option<String>,
        /// 关闭安全入口
        #[arg(long)]
        disable: bool,
    },
    /// 卸载服务，支持 --purge 清理数据
    Uninstall {
        /// 是否清理所有相关数据目录
        #[arg(long)]
        purge: bool,
    },
    /// 管理 vustctl 自身
    #[command(name = "self")]
    SelfCmd {
        #[command(subcommand)]
        action: SelfAction,
    },
}

#[derive(Subcommand)]
enum SelfAction {
    /// 卸载 vustctl 自身
    Uninstall,
}

/// 运行时上下文，缓存路径和特权状态。
struct Context {
    /// VUST 主安装目录
    vust_home: PathBuf,
    /// 配置目录
    config_dir: PathBuf,
    /// 数据库目录
    db_dir: PathBuf,
    /// 日志目录
    log_dir: PathBuf,
    /// 运行状态文件目录
    run_dir: PathBuf,
    /// Agent 的 Unix Socket 文件路径
    agent_socket: PathBuf,
    /// 当前是否需要提升权限运行特权指令。
    use_sudo: bool,
}

/// vust 运行时监听配置。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeListenConfig {
    /// 实际监听地址。
    host: String,
    /// 实际监听端口。
    port: u16,
    /// 对外默认访问地址。
    public_host: Option<String>,
}

/// 检查当前运行的用户是否拥有 Root 权限。
fn check_privilege() -> Result<bool> {
    let uid = unsafe { libc::getuid() };
    if uid == 0 {
        return Ok(false);
    }
    eprintln!("vustctl: must run as root");
    std::process::exit(1);
}

/// 动态检测当前可执行文件所在的路径，并推导出默认的 `VUST_HOME`。
fn get_detected_home() -> Result<PathBuf> {
    let current_exe = std::env::current_exe()?;
    let script_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Failed to get parent directory of executable"))?;

    let script_dir_str = script_dir.to_string_lossy();
    if script_dir_str == "/usr/local/bin"
        || script_dir_str == "/usr/bin"
        || script_dir.file_name().is_some_and(|n| n == "deploy")
    {
        Ok(PathBuf::from("/opt/vust"))
    } else {
        Ok(script_dir.to_path_buf())
    }
}

/// 检测外部命令在当前系统环境变量 PATH 中是否存在。
fn command_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 构造服务实际在 Systemd 中注册的名称。
fn build_service_name(target: &str) -> String {
    if target == "vust" {
        target.to_string()
    } else {
        format!("vust-{}", target)
    }
}

/// 解析监听地址，返回 (IP, 端口) 的元组。
fn parse_listen_addr(addr: &str) -> (String, String) {
    if addr.is_empty() {
        return ("".to_string(), "".to_string());
    }
    // IPv6 格式像 [::1]:8080
    if addr.starts_with('[') && addr.rfind(']').is_some() {
        let r_bracket_idx = addr.rfind(']').unwrap();
        let host = &addr[0..=r_bracket_idx];
        let rest = &addr[r_bracket_idx + 1..];
        if let Some(port) = rest.strip_prefix(':') {
            return (host.to_string(), port.to_string());
        }
    }
    // 否则如果是带冒号的形式 host:port
    if let Some(colon_idx) = addr.rfind(':') {
        let host = &addr[0..colon_idx];
        let port = &addr[colon_idx + 1..];
        return (host.to_string(), port.to_string());
    }
    (addr.to_string(), "".to_string())
}

/// 将泛监听地址转换为可访问的本机地址。
fn normalize_listen_host(host: &str) -> String {
    match host.trim() {
        "" | "*" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_string(),
        value => value
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
    }
}

/// 打印命令行工具用法。
fn print_usage() {
    eprintln!(
        "Usage:\n  vustctl stop [vust|agent]\n  vustctl restart <vust|agent>\n  vustctl status\n  vustctl info\n  vustctl passwd [--username <username>] [password]\n  vustctl user --username <username>\n  vustctl entry [--regenerate|--set <entry>|--disable]\n  vustctl uninstall [--purge]\n  vustctl self uninstall"
    );
}

/// 根据可选目标和本机安装状态选择需要停止的服务。
fn select_stop_targets(
    target: Option<&str>,
    has_vust: bool,
    has_agent: bool,
) -> Result<Vec<&'static str>> {
    match target {
        Some("vust") if has_vust => Ok(vec!["vust"]),
        Some("agent") if has_agent => Ok(vec!["agent"]),
        Some("vust") => Err(anyhow::anyhow!("vust service is not installed")),
        Some("agent") => Err(anyhow::anyhow!("vust-agent service is not installed")),
        Some(_) => Err(anyhow::anyhow!(
            "invalid service target; expected vust or agent"
        )),
        None => {
            let mut targets = Vec::new();
            if has_agent {
                targets.push("agent");
            }
            if has_vust {
                targets.push("vust");
            }
            if targets.is_empty() {
                return Err(anyhow::anyhow!("no VUST services are installed"));
            }
            Ok(targets)
        }
    }
}

/// 根据节点角色选择需要卸载的服务。
fn select_uninstall_targets(role: &str) -> Result<Vec<&'static str>> {
    match role {
        "agent" => Ok(vec!["agent"]),
        "vust" => Ok(vec!["vust"]),
        "all" => Ok(vec!["agent", "vust"]),
        _ => Err(anyhow::anyhow!("no VUST services are installed")),
    }
}

/// 判断卸载目标是否包含主控，并需要执行节点清理预检。
fn uninstall_requires_node_check(targets: &[&str]) -> bool {
    targets.contains(&"vust")
}

/// 查询主控数据库中仍然保留的非本地节点。
fn query_non_local_nodes(conn: &rusqlite::Connection) -> Result<NonLocalNodeInventory> {
    let total = conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE node_id != 'local';",
        [],
        |row| row.get(0),
    )?;
    let mut statement = conn.prepare(
        "SELECT name, status FROM nodes WHERE node_id != 'local' \
         ORDER BY name, node_id LIMIT ?;",
    )?;
    let preview = statement
        .query_map([NODE_UNINSTALL_PREVIEW_LIMIT as i64], |row| {
            Ok(NonLocalNodeSummary {
                name: row.get(0)?,
                status: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(NonLocalNodeInventory { total, preview })
}

/// 生成主控卸载被节点守卫阻止时的用户提示。
fn format_uninstall_blocked_message(inventory: &NonLocalNodeInventory) -> String {
    let node_label = if inventory.total == 1 {
        "node still exists"
    } else {
        "nodes still exist"
    };
    let mut message = format!(
        "uninstall blocked\n\n{} non-local {}:",
        inventory.total, node_label
    );
    for node in &inventory.preview {
        message.push_str(&format!("\n  - {} [{}]", node.name, node.status));
    }
    if inventory.total > inventory.preview.len() as i64 {
        message.push_str(&format!(
            "\n  - ... and {} more",
            inventory.total - inventory.preview.len() as i64
        ));
    }
    message.push_str(
        "\n\nRemove all nodes in Web > Node Management, then retry.\nOtherwise, their Agents will go offline and keep retrying the connection.",
    );
    message
}

/// 按既定顺序执行预检和卸载，并在全部服务卸载成功后统一清理数据。
fn run_uninstall_sequence<V, U, P>(
    targets: &[&str],
    purge: bool,
    mut validate: V,
    mut uninstall: U,
    mut purge_common_dirs: P,
) -> Result<()>
where
    V: FnMut() -> Result<()>,
    U: FnMut(&str) -> Result<()>,
    P: FnMut() -> Result<()>,
{
    validate()?;
    for target in targets {
        uninstall(target)?;
    }
    if purge {
        purge_common_dirs()?;
    }
    Ok(())
}

/// 读取一行交互输入。
fn prompt_line(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim_end_matches(['\r', '\n']).to_string())
}

/// 校验用户名。
fn validate_username_value(username: &str) -> Result<()> {
    let valid = !username.trim().is_empty()
        && username.len() <= 64
        && username
            .chars()
            .enumerate()
            .all(|(idx, ch)| ch.is_ascii_alphanumeric() || ch == '_' || (idx > 0 && ch == '-'));
    if !valid {
        return Err(anyhow::anyhow!(
            "Username must be 1-64 ASCII letters, digits, underscore, or hyphen"
        ));
    }
    Ok(())
}

/// 校验安全入口。
fn validate_safe_entry_value(value: &str) -> Result<()> {
    if !(8..=32).contains(&value.len()) || !value.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err(anyhow::anyhow!(
            "Safe entry must be 8-32 ASCII letters or digits"
        ));
    }
    let lower = value.to_ascii_lowercase();
    if RESERVED_SAFE_ENTRY_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return Err(anyhow::anyhow!(
            "Safe entry must not use a reserved path prefix"
        ));
    }
    Ok(())
}

/// 校验密码。
fn validate_password(password: &str, enforce_complexity: bool) -> Result<()> {
    if password.is_empty() {
        return Err(anyhow::anyhow!("Password must not be empty"));
    }
    if !enforce_complexity {
        if password.len() < 5 {
            return Err(anyhow::anyhow!(
                "Password length must be at least 5 characters"
            ));
        }
        return Ok(());
    }
    if !(8..=30).contains(&password.len()) {
        return Err(anyhow::anyhow!("Password length must be 8-30 characters"));
    }
    let has_letter = password.chars().any(|ch| ch.is_ascii_alphabetic());
    let has_digit = password.chars().any(|ch| ch.is_ascii_digit());
    let has_special = password.chars().any(|ch| !ch.is_ascii_alphanumeric());
    let count = [has_letter, has_digit, has_special]
        .into_iter()
        .filter(|value| *value)
        .count();
    if count < 2 {
        return Err(anyhow::anyhow!(
            "Password must contain at least two character classes"
        ));
    }
    Ok(())
}

/// 生成随机安全入口。
fn generate_safe_entry() -> Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?
        .read_exact(&mut bytes)
        .map_err(|err| anyhow::anyhow!("Failed to read random bytes: {}", err))?;
    let charset = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    Ok((0..16)
        .map(|idx| charset[bytes[idx] as usize % charset.len()] as char)
        .collect())
}

impl Context {
    /// 初始化运行时上下文，加载环境变量并推导目录路径。
    fn init() -> Result<Self> {
        let use_sudo = check_privilege()?;
        let detected_home = get_detected_home()?;

        let vust_home = std::env::var("VUST_HOME")
            .map(PathBuf::from)
            .unwrap_or(detected_home);

        let config_dir = std::env::var("VUST_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| vust_home.join("config"));

        let db_dir = std::env::var("VUST_DB_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| vust_home.join("database"));

        let log_dir = std::env::var("VUST_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| vust_home.join("logs"));

        let run_dir = std::env::var("VUST_RUN_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| vust_home.join("run"));

        let agent_socket = std::env::var("VUST_AGENT_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|_| run_dir.join("vust-agent.sock"));

        Ok(Self {
            vust_home,
            config_dir,
            db_dir,
            log_dir,
            run_dir,
            agent_socket,
            use_sudo,
        })
    }

    /// 执行系统命令，如果 use_sudo 为真，则加上 sudo 前缀。
    fn run_command(&self, cmd: &str, args: &[&str]) -> Result<std::process::ExitStatus> {
        let mut command = if self.use_sudo {
            let mut c = std::process::Command::new("sudo");
            c.arg(cmd);
            c
        } else {
            std::process::Command::new(cmd)
        };
        command.args(args);
        let status = command.status()?;
        Ok(status)
    }

    /// 静默执行系统命令，隐藏 stdout 与 stderr。如果 use_sudo 为真，则加上 sudo 前缀。
    fn run_command_silent(&self, cmd: &str, args: &[&str]) -> Result<std::process::ExitStatus> {
        let mut command = if self.use_sudo {
            let mut c = std::process::Command::new("sudo");
            c.arg(cmd);
            c
        } else {
            std::process::Command::new(cmd)
        };
        command.args(args);
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::null());
        let status = command.status()?;
        Ok(status)
    }

    /// 执行系统命令并捕获标准输出。如果 use_sudo 为真，则加上 sudo 前缀。
    fn run_command_output(&self, cmd: &str, args: &[&str]) -> Result<String> {
        let mut command = if self.use_sudo {
            let mut c = std::process::Command::new("sudo");
            c.arg(cmd);
            c
        } else {
            std::process::Command::new(cmd)
        };
        command.args(args);
        let output = command.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(stdout)
    }

    /// 检查指定服务是否在 systemd 中已安装。
    fn service_installed(&self, target: &str) -> bool {
        let service = build_service_name(target);
        let status = self.run_command_silent(
            "systemctl",
            &["list-unit-files", &format!("{}.service", service)],
        );
        match status {
            Ok(s) => s.success(),
            Err(_) => false,
        }
    }

    /// 检测当前节点的角色 (vust, agent, all, 或 unknown)。
    fn detect_node_role(&self) -> String {
        let role_file = self.config_dir.join("node.role");

        let role_content = if self.use_sudo {
            self.run_command_output("cat", &[&role_file.to_string_lossy()])
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        } else if role_file.exists() {
            std::fs::read_to_string(&role_file)
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        } else {
            "".to_string()
        };

        if role_content == "vust" || role_content == "agent" || role_content == "all" {
            return role_content;
        }

        let has_vust = self.service_installed("vust");
        let has_agent = self.service_installed("agent");

        if has_vust && has_agent {
            "all".to_string()
        } else if has_vust {
            "vust".to_string()
        } else if has_agent {
            "agent".to_string()
        } else {
            "unknown".to_string()
        }
    }

    /// 重启指定的 systemd 服务，并输出服务状态。
    fn restart_service(&self, target: &str) -> Result<()> {
        if target != "vust" && target != "agent" {
            print_usage();
            std::process::exit(1);
        }

        if !command_exists("systemctl") {
            return Err(anyhow::anyhow!("systemctl not found"));
        }

        let service = build_service_name(target);
        self.run_command("systemctl", &["restart", &service])?;
        let status_output =
            self.run_command_output("systemctl", &["status", &service, "--no-pager"])?;
        print!("{}", status_output);
        Ok(())
    }

    /// 停止当前节点的全部 VUST 服务，或仅停止指定服务。
    fn stop_services(&self, target: Option<&str>) -> Result<()> {
        if !command_exists("systemctl") {
            return Err(anyhow::anyhow!("systemctl not found"));
        }

        let targets = select_stop_targets(
            target,
            self.service_installed("vust"),
            self.service_installed("agent"),
        )?;
        for target in targets {
            let service = build_service_name(target);
            self.stop_service_and_verify(&service)?;
            println!("vustctl: {service} stopped");
        }
        Ok(())
    }

    /// 检查指定服务的运行状态并打印。
    fn status_service(&self, target: &str) -> Result<()> {
        if !command_exists("systemctl") {
            return Err(anyhow::anyhow!("systemctl not found"));
        }

        let display_name = if target == "vust" {
            "vust"
        } else {
            "vust-agent"
        };

        if !self.service_installed(target) {
            println!(
                "  \x1b[90m○\x1b[0m  \x1b[1;37m{:<15}\x1b[0m : \x1b[90mNot Installed\x1b[0m",
                display_name
            );
            return Ok(());
        }

        let service = build_service_name(target);
        let status = self.run_command_silent("systemctl", &["is-active", "--quiet", &service])?;
        if status.success() {
            println!(
                "  \x1b[32m●\x1b[0m  \x1b[1;37m{:<15}\x1b[0m : \x1b[32mRunning\x1b[0m",
                display_name
            );
        } else {
            println!(
                "  \x1b[31m●\x1b[0m  \x1b[1;37m{:<15}\x1b[0m : \x1b[31mStopped\x1b[0m",
                display_name
            );
        }
        Ok(())
    }

    /// 展示当前节点和服务的详细信息。
    fn show_info(&self) -> Result<()> {
        let role = self.detect_node_role();
        println!("vustctl: node role: {}", role);
        match role.as_str() {
            "agent" => {
                println!("vustctl: vust listen IP: N/A");
                println!("vustctl: vust listen port: N/A");
                println!("vustctl: admin username: N/A");
            }
            "vust" | "all" => {
                self.show_vust_info()?;
            }
            _ => {
                println!("vustctl: vust listen IP: unknown");
                println!("vustctl: vust listen port: unknown");
                println!("vustctl: admin username: unknown");
            }
        }
        Ok(())
    }

    /// 查询并显示 vust 主服务的配置与运行详情（IP/Port/Admin）。
    fn show_vust_info(&self) -> Result<()> {
        let service = build_service_name("vust");
        let pid_str = self
            .run_command_output("systemctl", &["show", "-p", "MainPID", "--value", &service])
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let mut listen_ip = "unknown".to_string();
        let mut listen_port = "unknown".to_string();
        let runtime_config = self.load_runtime_listen_config();

        if !pid_str.is_empty() && pid_str != "0" && command_exists("ss") {
            let ss_output = self
                .run_command_output("ss", &["-ltnp"])
                .unwrap_or_default();
            let search_pattern = format!("pid={},", pid_str);
            for line in ss_output.lines() {
                if line.contains(&search_pattern) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        let (ip, port) = parse_listen_addr(parts[3]);
                        if !ip.is_empty() {
                            listen_ip = ip;
                        }
                        if !port.is_empty() {
                            listen_port = port;
                        }
                    }
                    break;
                }
            }
        }

        if let Some(config) = runtime_config {
            listen_port = config.port.to_string();
            listen_ip = config
                .public_host
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| normalize_listen_host(&config.host));
        }

        let database_path = self.db_dir.join("vust.db");
        let db_exists = if self.use_sudo {
            let status = self.run_command_silent("test", &["-f", &database_path.to_string_lossy()]);
            status.map(|s| s.success()).unwrap_or(false)
        } else {
            database_path.is_file()
        };

        let admin_username = if !db_exists {
            "unknown (Database not found)".to_string()
        } else {
            match self.get_admin_username_from_db() {
                Ok(name) => name,
                Err(e) => {
                    let err_msg = e.to_string();
                    if err_msg.contains("Permission denied") {
                        "unknown (Permission denied, run as root)".to_string()
                    } else {
                        "unknown (Access failed)".to_string()
                    }
                }
            }
        };
        let safe_entry = self.get_safe_entry_from_db().unwrap_or_default();

        println!("vustctl: vust listen IP: {}", listen_ip);
        println!("vustctl: vust listen port: {}", listen_port);
        println!("vustctl: admin username: {}", admin_username);
        if safe_entry.trim().is_empty() {
            println!("vustctl: safe entry: disabled");
        } else {
            println!("vustctl: safe entry: {}", safe_entry.trim());
            println!(
                "vustctl: panel login URL: https://{}:{}/{}",
                listen_ip,
                listen_port,
                safe_entry.trim()
            );
        }
        Ok(())
    }

    /// 读取运行时监听配置。
    fn load_runtime_listen_config(&self) -> Option<RuntimeListenConfig> {
        let path = self.config_dir.join("runtime-listen.json");
        let raw = if self.use_sudo {
            self.run_command_output("cat", &[&path.to_string_lossy()])
                .ok()?
        } else {
            std::fs::read_to_string(path).ok()?
        };
        serde_json::from_str(&raw).ok()
    }

    /// 安全删除文件或目录（如果存在）。
    fn remove_file_if_exists(&self, path: &str) -> Result<()> {
        let exists = if self.use_sudo {
            let status_e = self.run_command_silent("test", &["-e", path]);
            let status_l = self.run_command_silent("test", &["-L", path]);
            status_e.map(|s| s.success()).unwrap_or(false)
                || status_l.map(|s| s.success()).unwrap_or(false)
        } else {
            let p = Path::new(path);
            p.exists() || p.is_symlink()
        };

        if exists {
            self.run_command_silent("rm", &["-rf", path])?;
        }
        Ok(())
    }

    /// 彻底清理数据目录与缓存。
    fn purge_common_dirs(&self) -> Result<()> {
        self.remove_file_if_exists(&self.db_dir.to_string_lossy())?;
        self.remove_file_if_exists(&self.log_dir.to_string_lossy())?;
        self.remove_file_if_exists(&self.agent_socket.to_string_lossy())?;
        self.remove_file_if_exists(&self.run_dir.to_string_lossy())?;
        self.remove_file_if_exists(&self.vust_home.to_string_lossy())?;
        self.remove_file_if_exists("/etc/vust")?;
        self.remove_file_if_exists("/var/lib/vust")?;
        self.remove_file_if_exists("/var/log/vust")?;
        self.remove_file_if_exists("/run/vust")?;
        Ok(())
    }

    /// 停止服务并确认 systemd 已不再持有活动主进程。
    fn stop_service_and_verify(&self, service: &str) -> Result<()> {
        let stop_status = self.run_command_silent("systemctl", &["stop", service])?;
        let active_status =
            self.run_command_silent("systemctl", &["is-active", "--quiet", service])?;
        let main_pid = self
            .run_command_output("systemctl", &["show", "-p", "MainPID", "--value", service])
            .unwrap_or_default();

        if !service_has_stopped(active_status.success(), &main_pid) {
            return Err(anyhow::anyhow!(
                "failed to stop {service}: service is still active or MainPID is still present"
            ));
        }
        if !stop_status.success() {
            eprintln!(
                "vustctl: warning: systemctl stop {service} returned a failure, but the service is verified stopped"
            );
        }
        Ok(())
    }

    /// 卸载指定的服务。
    fn uninstall_service(&self, target: &str) -> Result<()> {
        if !command_exists("systemctl") {
            return Err(anyhow::anyhow!("systemctl not found"));
        }

        let service = build_service_name(target);
        println!("vustctl: start uninstall {}", target);

        self.stop_service_and_verify(&service)?;
        let _ = self.run_command_silent("systemctl", &["disable", &service]);

        self.remove_file_if_exists(&format!("/etc/systemd/system/{}.service", service))?;
        self.remove_file_if_exists(&format!("/usr/lib/systemd/system/{}.service", service))?;
        self.remove_file_if_exists(&format!("/lib/systemd/system/{}.service", service))?;
        self.remove_file_if_exists(&format!(
            "/etc/systemd/system/multi-user.target.wants/{}.service",
            service
        ))?;

        let _ = self.run_command_silent("systemctl", &["daemon-reload"]);
        let _ = self.run_command_silent("systemctl", &["reset-failed", &service]);

        self.remove_file_if_exists(&format!("/usr/local/bin/{}", service))?;
        self.remove_file_if_exists(&format!("/usr/bin/{}", service))?;

        if target == "agent" {
            self.remove_file_if_exists(&self.config_dir.join("agent.toml").to_string_lossy())?;
            self.remove_file_if_exists(
                &self.config_dir.join("agent.install_dir").to_string_lossy(),
            )?;
            self.remove_file_if_exists(&self.config_dir.join("node.role").to_string_lossy())?;
            self.remove_file_if_exists(&self.vust_home.join("agent").to_string_lossy())?;

            self.remove_file_if_exists(&self.db_dir.join("agent.db").to_string_lossy())?;
            self.remove_file_if_exists(&self.db_dir.join("agent.db-shm").to_string_lossy())?;
            self.remove_file_if_exists(&self.db_dir.join("agent.db-wal").to_string_lossy())?;
        } else {
            self.remove_file_if_exists(&self.config_dir.join("vust.toml").to_string_lossy())?;
        }

        println!("vustctl: {} uninstall completed", target);
        Ok(())
    }

    /// 确认主控卸载前已经在 Web 中清理全部非本地节点。
    fn ensure_uninstall_allowed(&self, targets: &[&str]) -> Result<()> {
        if !uninstall_requires_node_check(targets) {
            return Ok(());
        }

        let database_path = self.db_dir.join("vust.db");
        let database_exists = database_path.try_exists().map_err(|error| {
            anyhow::anyhow!(
                "cannot verify managed nodes from {}: {}",
                database_path.display(),
                error
            )
        })?;
        if !database_exists {
            return Ok(());
        }

        let conn =
            rusqlite::Connection::open_with_flags(&database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "cannot verify managed nodes from {}: {}",
                        database_path.display(),
                        error
                    )
                })?;
        let inventory = query_non_local_nodes(&conn).map_err(|error| {
            anyhow::anyhow!(
                "cannot verify managed nodes from {}: {}",
                database_path.display(),
                error
            )
        })?;
        if inventory.total == 0 {
            return Ok(());
        }

        Err(anyhow::anyhow!(format_uninstall_blocked_message(
            &inventory
        )))
    }

    /// 卸载 `vustctl` 工具自身。
    fn uninstall_self(&self) -> Result<()> {
        self.remove_file_if_exists("/usr/local/bin/vustctl")?;
        self.remove_file_if_exists("/usr/bin/vustctl")?;
        println!("vustctl: vustctl uninstall completed");
        Ok(())
    }

    /// 打开主控数据库。
    fn open_database(&self) -> Result<rusqlite::Connection> {
        let database_path = self.db_dir.join("vust.db");
        rusqlite::Connection::open(&database_path).map_err(|e| {
            anyhow::anyhow!("Cannot access database {}: {}", database_path.display(), e)
        })
    }

    /// 从数据库中获取管理员用户名。
    fn get_admin_username_from_db(&self) -> Result<String> {
        let conn = self.open_database()?;
        let mut stmt = conn.prepare("SELECT username FROM users ORDER BY id LIMIT 1;")?;
        let username: String = stmt.query_row([], |row| row.get(0))?;
        Ok(username)
    }

    /// 确保系统配置单行存在。
    fn ensure_system_config(&self, conn: &rusqlite::Connection) -> Result<()> {
        conn.execute("INSERT OR IGNORE INTO system_config (id) VALUES (1);", [])?;
        Ok(())
    }

    /// 从数据库中读取安全入口。
    fn get_safe_entry_from_db(&self) -> Result<String> {
        let conn = self.open_database()?;
        self.ensure_system_config(&conn)?;
        let value = conn.query_row(
            "SELECT safe_entry FROM system_config WHERE id = 1;",
            [],
            |row| row.get::<_, String>(0),
        )?;
        Ok(value)
    }

    /// 写入安全入口。
    fn set_safe_entry_in_db(&self, value: &str) -> Result<()> {
        let conn = self.open_database()?;
        self.ensure_system_config(&conn)?;
        conn.execute(
            "UPDATE system_config SET safe_entry = ?1 WHERE id = 1;",
            rusqlite::params![value],
        )?;
        Ok(())
    }

    /// 修改管理员密码，可同时修改用户名。
    fn passwd(&self, username: Option<&str>, new_password: &str) -> Result<()> {
        validate_password(new_password, self.password_complexity_enabled()?)?;
        let password_hash = bcrypt::hash(new_password, bcrypt::DEFAULT_COST)
            .map_err(|e| anyhow::anyhow!("Failed to hash password: {}", e))?;
        let conn = self.open_database()?;
        let affected = if let Some(username) = username {
            validate_username_value(username)?;
            conn.execute(
                "UPDATE users SET username = ?1, password_hash = ?2 WHERE id = (SELECT id FROM users ORDER BY id LIMIT 1);",
                rusqlite::params![username, password_hash],
            )?
        } else {
            conn.execute(
                "UPDATE users SET password_hash = ?1 WHERE id = (SELECT id FROM users ORDER BY id LIMIT 1);",
                rusqlite::params![password_hash],
            )?
        };
        if affected == 0 {
            return Err(anyhow::anyhow!("No admin user found in database."));
        }
        println!("vustctl: password updated");
        Ok(())
    }

    /// 修改管理员用户名。
    fn update_username(&self, username: &str) -> Result<()> {
        validate_username_value(username)?;
        let conn = self.open_database()?;
        let affected = conn.execute(
            "UPDATE users SET username = ?1 WHERE id = (SELECT id FROM users ORDER BY id LIMIT 1);",
            rusqlite::params![username],
        )?;
        if affected == 0 {
            return Err(anyhow::anyhow!("No admin user found in database."));
        }
        println!("vustctl: username updated to '{}'", username);
        Ok(())
    }

    /// 当前密码复杂度开关。
    fn password_complexity_enabled(&self) -> Result<bool> {
        let conn = self.open_database()?;
        self.ensure_system_config(&conn)?;
        let value: i64 = conn.query_row(
            "SELECT password_complexity FROM system_config WHERE id = 1;",
            [],
            |row| row.get(0),
        )?;
        Ok(value != 0)
    }

    /// 查看或修改安全入口。
    fn entry(&self, regenerate: bool, set: Option<String>, disable: bool) -> Result<()> {
        let action_count = [regenerate, set.is_some(), disable]
            .into_iter()
            .filter(|value| *value)
            .count();
        if action_count > 1 {
            return Err(anyhow::anyhow!(
                "Only one of --regenerate, --set, or --disable can be used"
            ));
        }

        if regenerate {
            let value = generate_safe_entry()?;
            self.set_safe_entry_in_db(&value)?;
            println!("vustctl: safe entry updated: {}", value);
            return Ok(());
        }
        if let Some(value) = set {
            validate_safe_entry_value(&value)?;
            self.set_safe_entry_in_db(&value)?;
            println!("vustctl: safe entry updated: {}", value);
            return Ok(());
        }
        if disable {
            self.set_safe_entry_in_db("")?;
            println!("vustctl: safe entry disabled");
            return Ok(());
        }

        let entry = self.get_safe_entry_from_db()?;
        if entry.trim().is_empty() {
            println!("vustctl: safe entry: disabled");
        } else {
            println!("vustctl: safe entry: {}", entry.trim());
        }
        Ok(())
    }
}

/// 根据 systemd 活动状态和主进程号判断服务是否已经完全停止。
fn service_has_stopped(is_active: bool, main_pid: &str) -> bool {
    !is_active && matches!(main_pid.trim(), "" | "0")
}

/// 分发并执行命令行指令。
fn run_app(cli: Cli, context: Context) -> Result<()> {
    match cli.command {
        Commands::Stop { target } => {
            context.stop_services(target.as_deref())?;
        }
        Commands::Restart { target } => {
            context.restart_service(&target)?;
        }
        Commands::Status => {
            println!("\x1b[1;36mVust Service Status\x1b[0m");
            println!("\x1b[90m----------------------------------------\x1b[0m");
            let role = context.detect_node_role();
            match role.as_str() {
                "agent" => {
                    context.status_service("agent")?;
                }
                "vust" => {
                    context.status_service("vust")?;
                }
                _ => {
                    context.status_service("vust")?;
                    context.status_service("agent")?;
                }
            }
            println!("\x1b[90m----------------------------------------\x1b[0m");
        }
        Commands::Info => {
            context.show_info()?;
        }
        Commands::Passwd { username, password } => {
            let password = match password {
                Some(value) => value,
                None => {
                    let first = prompt_line("New password: ")?;
                    let second = prompt_line("Confirm password: ")?;
                    if first != second {
                        return Err(anyhow::anyhow!("Password confirmation does not match"));
                    }
                    first
                }
            };
            context.passwd(username.as_deref(), &password)?;
        }
        Commands::User { username } => {
            context.update_username(&username)?;
        }
        Commands::Entry {
            regenerate,
            set,
            disable,
        } => {
            context.entry(regenerate, set, disable)?;
        }
        Commands::Uninstall { purge } => {
            let role = context.detect_node_role();
            let targets = select_uninstall_targets(&role)?;
            run_uninstall_sequence(
                &targets,
                purge,
                || context.ensure_uninstall_allowed(&targets),
                |target| context.uninstall_service(target),
                || context.purge_common_dirs(),
            )?;
        }
        Commands::SelfCmd { action } => match action {
            SelfAction::Uninstall => {
                context.uninstall_self()?;
            }
        },
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => match e.kind() {
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                e.exit();
            }
            _ => {
                print_usage();
                std::process::exit(1);
            }
        },
    };

    let context = match Context::init() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("vustctl error: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = run_app(cli, context) {
        eprintln!("vustctl: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        Cli, Commands, NonLocalNodeInventory, NonLocalNodeSummary,
        format_uninstall_blocked_message, query_non_local_nodes, run_uninstall_sequence,
        select_stop_targets, select_uninstall_targets, service_has_stopped,
        uninstall_requires_node_check, validate_password, validate_safe_entry_value,
    };

    #[test]
    fn stop_command_accepts_optional_target() {
        let all = Cli::try_parse_from(["vustctl", "stop"]).unwrap();
        assert!(matches!(all.command, Commands::Stop { target: None }));

        let agent = Cli::try_parse_from(["vustctl", "stop", "agent"]).unwrap();
        assert!(matches!(
            agent.command,
            Commands::Stop {
                target: Some(ref target)
            } if target == "agent"
        ));
    }

    #[test]
    fn stop_target_selection_prefers_agent_before_vust() {
        assert_eq!(
            select_stop_targets(None, true, true).unwrap(),
            vec!["agent", "vust"]
        );
        assert_eq!(
            select_stop_targets(Some("vust"), true, true).unwrap(),
            vec!["vust"]
        );
        assert!(select_stop_targets(Some("unknown"), true, true).is_err());
        assert!(select_stop_targets(Some("agent"), true, false).is_err());
        assert!(select_stop_targets(None, false, false).is_err());
    }

    #[test]
    fn uninstall_target_selection_follows_node_role() {
        assert_eq!(select_uninstall_targets("agent").unwrap(), vec!["agent"]);
        assert_eq!(select_uninstall_targets("vust").unwrap(), vec!["vust"]);
        assert_eq!(
            select_uninstall_targets("all").unwrap(),
            vec!["agent", "vust"]
        );
        assert!(select_uninstall_targets("unknown").is_err());
    }

    #[test]
    fn uninstall_sequence_runs_selected_targets_and_purges_once() {
        let targets = select_uninstall_targets("all").unwrap();
        let mut uninstalled = Vec::new();
        let mut purge_count = 0;

        run_uninstall_sequence(
            &targets,
            true,
            || Ok(()),
            |target| {
                uninstalled.push(target.to_string());
                Ok(())
            },
            || {
                purge_count += 1;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(uninstalled, vec!["agent", "vust"]);
        assert_eq!(purge_count, 1);
    }

    #[test]
    fn agent_uninstall_sequence_does_not_include_vust_or_purge_by_default() {
        let targets = select_uninstall_targets("agent").unwrap();
        let mut uninstalled = Vec::new();
        let mut purge_count = 0;

        run_uninstall_sequence(
            &targets,
            false,
            || Ok(()),
            |target| {
                uninstalled.push(target.to_string());
                Ok(())
            },
            || {
                purge_count += 1;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(uninstalled, vec!["agent"]);
        assert_eq!(purge_count, 0);
    }

    #[test]
    fn uninstall_sequence_skips_purge_when_service_uninstall_fails() {
        let targets = select_uninstall_targets("all").unwrap();
        let mut purge_count = 0;

        let result = run_uninstall_sequence(
            &targets,
            true,
            || Ok(()),
            |target| {
                if target == "vust" {
                    return Err(anyhow::anyhow!("uninstall failed"));
                }
                Ok(())
            },
            || {
                purge_count += 1;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert_eq!(purge_count, 0);
    }

    #[test]
    fn only_master_uninstall_targets_require_node_check() {
        assert!(!uninstall_requires_node_check(&["agent"]));
        assert!(uninstall_requires_node_check(&["vust"]));
        assert!(uninstall_requires_node_check(&["agent", "vust"]));
    }

    #[test]
    fn node_query_excludes_local_and_includes_every_other_status() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE nodes (node_id TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL);\
             INSERT INTO nodes VALUES ('local', 'Local Node', 'online');\
             INSERT INTO nodes VALUES ('draft-node', 'Draft Node', 'draft');\
             INSERT INTO nodes VALUES ('retired-node', 'Retired Node', 'retired');",
        )
        .unwrap();

        let inventory = query_non_local_nodes(&conn).unwrap();

        assert_eq!(inventory.total, 2);
        assert_eq!(inventory.preview.len(), 2);
        assert_eq!(inventory.preview[0].name, "Draft Node");
        assert_eq!(inventory.preview[1].name, "Retired Node");
    }

    #[test]
    fn node_query_allows_inventory_with_only_local_node() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE nodes (node_id TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL);\
             INSERT INTO nodes VALUES ('local', 'Local Node', 'online');",
        )
        .unwrap();

        let inventory = query_non_local_nodes(&conn).unwrap();

        assert_eq!(inventory.total, 0);
        assert!(inventory.preview.is_empty());
    }

    #[test]
    fn node_query_fails_when_inventory_cannot_be_verified() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        assert!(query_non_local_nodes(&conn).is_err());
    }

    #[test]
    fn uninstall_blocked_message_omits_node_id_and_uses_readable_layout() {
        let inventory = NonLocalNodeInventory {
            total: 1,
            preview: vec![NonLocalNodeSummary {
                name: "7".to_string(),
                status: "online".to_string(),
            }],
        };

        let message = format_uninstall_blocked_message(&inventory);

        assert!(message.starts_with("uninstall blocked\n\n1 non-local node still exists:"));
        assert!(message.contains("\n  - 7 [online]\n\n"));
    }

    #[test]
    fn uninstall_preflight_failure_happens_before_service_changes() {
        let mut uninstall_count = 0;
        let mut purge_count = 0;

        let result = run_uninstall_sequence(
            &["agent", "vust"],
            false,
            || Err(anyhow::anyhow!("managed nodes still exist")),
            |_| {
                uninstall_count += 1;
                Ok(())
            },
            || {
                purge_count += 1;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert_eq!(uninstall_count, 0);
        assert_eq!(purge_count, 0);
    }

    #[test]
    fn service_stop_verification_requires_inactive_state_and_zero_pid() {
        assert!(service_has_stopped(false, "0\n"));
        assert!(service_has_stopped(false, ""));
        assert!(!service_has_stopped(true, "0"));
        assert!(!service_has_stopped(false, "7310"));
    }

    #[test]
    fn validates_safe_entry_format_and_reserved_prefixes() {
        assert!(validate_safe_entry_value("Xm9Kp2Qs").is_ok());
        assert!(validate_safe_entry_value("abc1234").is_err());
        assert!(validate_safe_entry_value("abc1234!").is_err());
        assert!(validate_safe_entry_value("api123456").is_err());
        assert!(validate_safe_entry_value("AssetsLogin").is_err());
    }

    #[test]
    fn validates_password_minimum_length_without_complexity() {
        assert!(validate_password("", false).is_err());
        assert!(validate_password("1234", false).is_err());
        assert!(validate_password("12345", false).is_ok());
    }

    #[test]
    fn validates_password_complexity_when_enabled() {
        assert!(validate_password("abcdefg1", true).is_ok());
        assert!(validate_password("abcdefgh", true).is_err());
        assert!(validate_password("abc1", true).is_err());
    }
}
