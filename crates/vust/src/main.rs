//! 服务入口：加载配置、初始化依赖并启动 HTTP 服务。

use api::routes::create_router;
use clap::{Parser, Subcommand};
use rustls::crypto::ring::default_provider;
use shadow_rs::{formatcp, shadow};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;
use tokio::signal;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::{fmt::time::ChronoLocal, layer::SubscriberExt, util::SubscriberInitExt};

pub mod acceptor;
pub mod api;
pub mod config;
pub mod crypto;
pub mod db;
pub mod errors;
pub mod models;
pub mod runtime_config;
pub mod security;
pub mod services;
pub mod state;
#[cfg(test)]
mod test_support;
pub mod types;

shadow!(build);

const VERSION_INFO: &str = formatcp!(
    r#"{}
commit_hash: {}
build_time: {}
build_env: {},{}"#,
    build::PKG_VERSION,
    build::SHORT_COMMIT,
    build::BUILD_TIME,
    build::RUST_VERSION,
    build::RUST_CHANNEL
);
const DEFAULT_PRODUCTION_HOME: &str = "/opt/vust";
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser, Debug)]
#[command(name = "VUST", version = VERSION_INFO)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 启动控制台服务
    Start {
        #[arg(short = 'p', long = "port")]
        http_port: Option<u16>,

        /// 服务监听的 IP 地址或主机名。例如：127.0.0.1 或 0.0.0.0。
        #[arg(long)]
        host: Option<String>,
    },
    /// 初始化运行时配置
    InitRuntimeConfig {
        /// 服务监听的 IP 地址或主机名。例如：127.0.0.1 或 0.0.0.0。
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(short = 'p', long = "port", default_value = crate::config::DEFAULT_CONTROLLER_PORT_STR)]
        http_port: u16,
        /// 默认访问主机名或 IP，用于生成节点默认回连地址。
        #[arg(long)]
        public_host: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    load_env();
    let _log_guard = init_logging();
    config::init();
    if let Err(err) = default_provider().install_default() {
        tracing::error!("Failed to install rustls crypto provider: {:?}", err);
        std::process::exit(1);
    }

    let args = Cli::parse();
    tracing::debug!("Successfully parsed command-line arguments: {:?}", args);

    let (listen_host, listen_port, runtime_listen) = match args.command {
        Commands::InitRuntimeConfig {
            host,
            http_port,
            public_host,
        } => {
            let public_host = match normalize_public_host(public_host.as_deref()) {
                Ok(value) => value,
                Err(err) => {
                    tracing::error!("{err}");
                    std::process::exit(1);
                }
            };
            let cfg = runtime_config::ListenConfig {
                host,
                port: http_port,
                public_host,
            };
            if let Err(err) = runtime_config::save(&cfg) {
                tracing::error!("Failed to initialize runtime listen config: {err}");
                std::process::exit(1);
            }
            tracing::info!(
                "Runtime listen config initialized successfully at {}:{}",
                cfg.host,
                cfg.port
            );
            return;
        }
        Commands::Start { http_port, host } => {
            let runtime_listen = runtime_config::load_or_default();
            let listen_host = host.unwrap_or(runtime_listen.host.clone());
            let listen_port = http_port.unwrap_or(runtime_listen.port);
            (listen_host, listen_port, runtime_listen)
        }
    };

    runtime_config::set_active_config(runtime_config::ListenConfig {
        host: listen_host.clone(),
        port: listen_port,
        public_host: runtime_listen.public_host.clone(),
    });

    let (app, db_for_shutdown) = match create_router().await {
        Ok(app) => app,
        Err(err) => {
            let root_cause = err.root_cause();
            if let Some(io_error) = root_cause.downcast_ref::<io::Error>()
                && io_error.kind() == io::ErrorKind::PermissionDenied
            {
                tracing::error!(
                    "Docker API call failed because the process does not have Docker permissions"
                );
                std::process::exit(1);
            }
            tracing::error!("VUST startup failed: {err:#}");
            std::process::exit(1);
        }
    };

    let sans = build_controller_cert_sans(runtime_listen.public_host.as_deref());

    let certs_dir = crate::config::certs_dir();
    let cert_path = certs_dir.join("server.crt");
    let key_path = certs_dir.join("server.key");

    let (cert_pem, key_pem) = if cert_path.exists() && key_path.exists() {
        tracing::info!("Loading existing TLS certificate from {:?}", certs_dir);
        let cert_pem = std::fs::read(&cert_path).expect("Failed to read server certificate");
        let key_pem = std::fs::read(&key_path).expect("Failed to read server private key");
        (cert_pem, key_pem)
    } else {
        tracing::info!("Generating new TLS certificate in {:?}", certs_dir);
        let issued = vust_security::certs::issue_server_cert("vust-controller", &sans)
            .expect("Failed to issue TLS certificate for controller");

        if let Err(err) = std::fs::create_dir_all(&certs_dir) {
            tracing::error!("Failed to create certs directory {:?}: {}", certs_dir, err);
            std::process::exit(1);
        }
        if let Err(err) = std::fs::write(&cert_path, &issued.cert_pem) {
            tracing::error!(
                "Failed to write server certificate to {:?}: {}",
                cert_path,
                err
            );
            std::process::exit(1);
        }
        if let Err(err) = std::fs::write(&key_path, &issued.key_pem) {
            tracing::error!(
                "Failed to write server private key to {:?}: {}",
                key_path,
                err
            );
            std::process::exit(1);
        }
        (issued.cert_pem, issued.key_pem)
    };

    let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem(cert_pem, key_pem)
        .await
        .unwrap();

    use std::net::ToSocketAddrs;
    let addr = format!("{}:{}", listen_host, listen_port)
        .to_socket_addrs()
        .expect("Invalid listen address")
        .next()
        .expect("No socket addresses resolved");

    tracing::info!("Starting HTTPS/HTTP redirect server at {}", addr);
    log_access_urls("https", listen_port);

    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!(
            active_connections = shutdown_handle.connection_count(),
            graceful_timeout_seconds = GRACEFUL_SHUTDOWN_TIMEOUT.as_secs(),
            "Controller server received shutdown signal. Stopping server..."
        );
        shutdown_handle.graceful_shutdown(Some(GRACEFUL_SHUTDOWN_TIMEOUT));
    });

    let listener = match std::net::TcpListener::bind(addr) {
        Ok(l) => l,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::AddrInUse {
                tracing::error!(
                    "Failed to start controller server: Address {} is already in use. Please check if another instance of vust is running or choose a different port.",
                    addr
                );
            } else {
                tracing::error!("Failed to bind to {}: {}", addr, err);
            }
            std::process::exit(1);
        }
    };

    let acceptor = self::acceptor::HttpOrHttpsAcceptor::new(tls_config);
    axum_server::Server::from_tcp(listener)
        .acceptor(acceptor)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();

    tracing::info!("Attempting to close database connection pool...");
    db_for_shutdown.close().await;
    tracing::info!("Database connection pool closed successfully.");
}

fn load_env() {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    for dir in cwd.ancestors() {
        let path = dir.join(".env");
        if path.is_file() {
            let _ = dotenvy::from_path(path);
            return;
        }
    }
    let _ = dotenvy::dotenv();
}

/// 打印本机可直接访问的地址，便于在日志中点击打开。
fn log_access_urls(scheme: &str, port: u16) {
    tracing::info!("Access URL: {}://127.0.0.1:{}", scheme, port);
    if let Some(lan_ip) = detect_lan_ipv4() {
        tracing::info!("Access URL: {}://{}:{}", scheme, lan_ip, port);
    }
}

/// 探测当前主网卡 IPv4 地址，用于打印局域网访问地址。
fn detect_lan_ipv4() -> Option<Ipv4Addr> {
    let candidates = [
        (Ipv4Addr::new(223, 5, 5, 5), 53),
        (Ipv4Addr::new(8, 8, 8, 8), 80),
    ];

    for (target_ip, target_port) in candidates {
        let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if socket.connect((target_ip, target_port)).is_err() {
            continue;
        }
        if let Ok(addr) = socket.local_addr()
            && let IpAddr::V4(ip) = addr.ip()
            && !ip.is_loopback()
        {
            return Some(ip);
        }
    }

    None
}

/// 规范化默认访问主机，避免首次安装阶段写入 URL、端口或路径。
fn normalize_public_host(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if trimmed.contains("://") {
        return Err("public-host must be an IP or domain, without scheme".to_string());
    }
    if trimmed.contains('/') {
        return Err("public-host must not include path".to_string());
    }
    if trimmed.contains(':') {
        return Err("public-host must not include port".to_string());
    }
    Ok(Some(trimmed.to_string()))
}

/// 构造主控服务端证书 SAN，覆盖本地访问、主机名和默认回连地址。
fn build_controller_cert_sans(public_host: Option<&str>) -> Vec<String> {
    let mut sans = Vec::new();
    push_unique_san(&mut sans, "127.0.0.1");
    push_unique_san(&mut sans, "::1");
    push_unique_san(&mut sans, "localhost");

    if let Some(hostname) = detect_hostname() {
        push_unique_san(&mut sans, &hostname);
        push_unique_san(&mut sans, &format!("{hostname}.lan"));
    }
    if let Some(public_host) = public_host.map(str::trim).filter(|value| !value.is_empty()) {
        push_unique_san(&mut sans, public_host);
    }
    sans
}

fn push_unique_san(sans: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() || sans.iter().any(|item| item == trimmed) {
        return;
    }
    sans.push(trimmed.to_string());
}

fn detect_hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

/// 为应用程序设置日志记录基础设施。
fn init_logging() -> Option<WorkerGuard> {
    let file_writer = init_file_log_writer("vust");

    let registry = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // axum logs rejections from built-in extractors with the `axum::rejection`
                // target, at `TRACE` level. `axum::rejection=trace` enables showing those events
                format!(
                    "{}=info,tower_http=info,axum::rejection=trace",
                    env!("CARGO_CRATE_NAME")
                )
                .into()
            }),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.3f".to_string())),
        );

    if let Some((writer, guard)) = file_writer {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_timer(ChronoLocal::new("%Y-%m-%dT%H:%M:%S%.3f%:z".to_string())),
            )
            .init();
        Some(guard)
    } else {
        registry.init();
        None
    }
}

fn runtime_log_dir(service: &str) -> std::path::PathBuf {
    std::env::var_os("VUST_LOG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("VUST_DATA_DIR")
                .map(|value| std::path::PathBuf::from(value).join("logs"))
        })
        .unwrap_or_else(default_log_root)
        .join(service)
}

fn default_log_root() -> std::path::PathBuf {
    if cfg!(debug_assertions) {
        workspace_dev_dir().join("logs")
    } else {
        production_home().join("logs")
    }
}

fn production_home() -> std::path::PathBuf {
    std::env::var_os("VUST_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_PRODUCTION_HOME))
}

fn workspace_dev_dir() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    for ancestor in cwd.ancestors() {
        if ancestor.join("crates").is_dir() && ancestor.join("frontend").is_dir() {
            return ancestor.join(".vust");
        }
    }
    std::path::PathBuf::from(".vust")
}

fn init_file_log_writer(service: &str) -> Option<(NonBlocking, WorkerGuard)> {
    let dir = runtime_log_dir(service);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            log_dir = %dir.display(),
            error = %err,
            "Failed to create runtime log directory; file logging disabled"
        );
        return None;
    }

    Some(tracing_appender::non_blocking(
        tracing_appender::rolling::daily(dir, format!("{service}.log")),
    ))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Received termination signal shutting down");
}

#[test]
fn parse_vust_cli_args() {
    let args = Cli::try_parse_from(["app", "start", "-p", "80", "--host", "192.168.1.1"])
        .expect("failed to parse cli args");

    match args.command {
        Commands::Start { http_port, host } => {
            assert_eq!(http_port, Some(80));
            assert_eq!(host.as_deref(), Some("192.168.1.1"));
        }
        _ => panic!("expected Start command"),
    }
}

#[test]
fn parse_vust_cli_init_command() {
    let args = Cli::try_parse_from([
        "app",
        "init-runtime-config",
        "--host",
        "0.0.0.0",
        "-p",
        "9000",
        "--public-host",
        "controller.example.com",
    ])
    .expect("failed to parse init command");

    match args.command {
        Commands::InitRuntimeConfig {
            host,
            http_port,
            public_host,
        } => {
            assert_eq!(host, "0.0.0.0");
            assert_eq!(http_port, 9000);
            assert_eq!(public_host.as_deref(), Some("controller.example.com"));
        }
        _ => panic!("expected InitRuntimeConfig command"),
    }
}

#[test]
fn normalize_public_host_accepts_ip_or_domain() {
    assert_eq!(
        normalize_public_host(Some(" 192.168.1.1 ")).unwrap(),
        Some("192.168.1.1".to_string())
    );
    assert_eq!(
        normalize_public_host(Some("controller.example.com")).unwrap(),
        Some("controller.example.com".to_string())
    );
    assert_eq!(normalize_public_host(Some("")).unwrap(), None);
}

#[test]
fn normalize_public_host_rejects_url_port_or_path() {
    assert!(normalize_public_host(Some("https://controller.example.com")).is_err());
    assert!(normalize_public_host(Some("controller.example.com:7310")).is_err());
    assert!(normalize_public_host(Some("controller.example.com/path")).is_err());
}

#[test]
fn build_controller_cert_sans_contains_fixed_local_names() {
    let sans = build_controller_cert_sans(None);
    assert!(sans.contains(&"127.0.0.1".to_string()));
    assert!(sans.contains(&"::1".to_string()));
    assert!(sans.contains(&"localhost".to_string()));
}

#[test]
fn build_controller_cert_sans_adds_public_host_once() {
    let sans = build_controller_cert_sans(Some("127.0.0.1"));
    let count = sans
        .iter()
        .filter(|item| item.as_str() == "127.0.0.1")
        .count();
    assert_eq!(count, 1);

    let sans = build_controller_cert_sans(Some("controller.example.com"));
    assert!(sans.contains(&"controller.example.com".to_string()));
}
