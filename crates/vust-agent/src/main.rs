//! 服务入口：加载配置、初始化依赖并启动 HTTP 服务。

use crate::models::identity::{
    clear_runtime_session, ensure_identity_certs, load_or_init_identity, update_runtime_session,
};
use crate::state::DbPool;
use crate::types::AgentMode;
use axum_server::tls_rustls::RustlsConfig;
use clap::{Parser, Subcommand};
use ring::rand::{SecureRandom, SystemRandom};
use routes::create_router;
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use semver::Version;
use shadow_rs::{formatcp, shadow};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::{fs, os::unix::fs::PermissionsExt};
use tokio::{net::UnixListener, signal, sync::watch};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::{fmt::time::ChronoLocal, layer::SubscriberExt, util::SubscriberInitExt};
use vust_api::response::ApiResponse;
use vust_contracts::types::agent_socket_path;
use vust_security::certs::AGENT_CA_CERT_PEM;
use vust_security::client::build_tls_client;

pub mod api;
pub mod config;
pub mod crypto;
pub mod db;
pub mod errors;
pub mod models;
pub mod routes;
pub mod services;
pub mod state;
#[cfg(test)]
mod test_support;
pub mod types;

const DEFAULT_PRODUCTION_HOME: &str = "/opt/vust";
const RUNTIME_RETRY_INITIAL_DELAY_SECONDS: u64 = 3;
const RUNTIME_RETRY_MAX_DELAY_SECONDS: u64 = 20;
const RUNTIME_RETRY_JITTER_SECONDS: u64 = 3;
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

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

#[derive(Parser, Debug)]
#[command(name = "VUST Agent", version = VERSION_INFO)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 启动 Agent 服务
    Start,
}

#[tokio::main]
async fn main() {
    let _args = Cli::parse();

    load_env();
    let _log_guard = init_logging();
    config::init();
    if let Err(err) = default_provider().install_default() {
        tracing::error!("Failed to install rustls crypto provider: {:?}", err);
        std::process::exit(1);
    }

    let (app, pools_for_shutdown, controller_runtime) = match create_router().await {
        Ok(app) => app,
        Err(err) => {
            tracing::error!("VUST Agent startup failed: {err:#}");
            std::process::exit(1);
        }
    };

    let mut identity = match load_or_init_identity(&pools_for_shutdown, config::get()).await {
        Ok(identity) => identity,
        Err(err) => {
            tracing::error!("Failed to load agent identity: {}", err);
            std::process::exit(1);
        }
    };

    let use_uds = identity.mode == AgentMode::Local;

    if identity.mode == AgentMode::Remote && identity.agent_ip.is_none() {
        tracing::warn!("agent_identity.agent_ip is empty; TLS SAN may not match remote address.");
    }

    let (runtime_stop_tx, runtime_stop_rx) = watch::channel(false);
    let mut runtime_stop_rx = Some(runtime_stop_rx);

    if use_uds {
        let socket_path = agent_socket_path();
        if tokio::fs::metadata(&socket_path).await.is_ok() {
            // 尝试连接已存在的套接字
            match tokio::net::UnixStream::connect(&socket_path).await {
                Ok(_) => {
                    // 连接成功，表明另一个实例正在运行
                    tracing::error!(
                        "Failed to start Agent: Another instance is already running and listening on UDS socket at {}.",
                        socket_path.display()
                    );
                    std::process::exit(1);
                }
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    // 连接被拒，说明旧实例已退出，只残留了垃圾文件，可以安全清理
                    tracing::info!(
                        socket_path = %socket_path.display(),
                        "Detected stale UDS socket file (connection refused). Removing it..."
                    );
                    if let Err(err) = tokio::fs::remove_file(&socket_path).await {
                        tracing::error!(
                            "Failed to remove stale socket file at {}: {}",
                            socket_path.display(),
                            err
                        );
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    // 其他系统或权限错误，为避免破坏活跃服务，打印错误并退出
                    tracing::error!(
                        "Failed to check UDS socket status at {}: {}. If you are sure no other instance is running, please remove this file manually.",
                        socket_path.display(),
                        e
                    );
                    std::process::exit(1);
                }
            }
        }

        // 创建目录（如果不存在）
        if let Some(dir) = socket_path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
    }

    if use_uds {
        let socket_path = agent_socket_path();
        let listener = match UnixListener::bind(&socket_path) {
            Ok(l) => l,
            Err(err) => {
                if err.kind() == std::io::ErrorKind::AddrInUse {
                    tracing::error!(
                        "Failed to bind Unix socket at {}: Address already in use. Please check if another instance of vust-agent is running.",
                        socket_path.display()
                    );
                } else {
                    tracing::error!(
                        "Failed to bind Unix socket at {}: {}",
                        socket_path.display(),
                        err
                    );
                }
                std::process::exit(1);
            }
        };

        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .expect("Failed to set socket file permissions");

        tracing::info!(
            "Starting Agent server at {:?}",
            listener.local_addr().unwrap()
        );
        let runtime_handle = tokio::spawn(run_runtime_supervisor(
            pools_for_shutdown.clone(),
            runtime_stop_rx
                .take()
                .expect("runtime supervisor receiver is missing"),
            Arc::clone(&controller_runtime),
        ));
        let runtime_stop_tx_for_signal = runtime_stop_tx.clone();
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(async move {
                shutdown_signal().await;
                let _ = runtime_stop_tx_for_signal.send(true);
                tracing::info!(
                    "vust-agent local listener received shutdown signal. Terminating..."
                );
            })
            .await
            .unwrap();
        wait_for_runtime_supervisor(runtime_handle).await;
        tracing::info!("Attempting to close agent database connection pool...");
        pools_for_shutdown.close().await;
        tracing::info!("Agent database connection pool closed successfully.");
        if tokio::fs::metadata(&socket_path).await.is_ok()
            && let Err(err) = tokio::fs::remove_file(&socket_path).await
        {
            tracing::warn!(error = %err, "failed to remove Agent UDS socket during shutdown");
        }
        return;
    }

    let listen_addr: SocketAddr = identity
        .listen_addr
        .clone()
        .unwrap_or_else(|| self::config::DEFAULT_AGENT_LISTEN_ADDR.to_string())
        .parse()
        .expect("Invalid listen_addr in agent_identity");
    let tls_config = match build_tls_config(&mut identity, &pools_for_shutdown).await {
        Ok(config) => config,
        Err(err) => {
            tracing::error!("Failed to build TLS config: {}", err);
            std::process::exit(1);
        }
    };
    let runtime_handle = tokio::spawn(run_runtime_supervisor(
        pools_for_shutdown.clone(),
        runtime_stop_rx
            .take()
            .expect("runtime supervisor receiver is missing"),
        controller_runtime,
    ));

    tracing::info!("Starting Agent TLS server at {:?}", listen_addr);
    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    let runtime_stop_tx_for_signal = runtime_stop_tx.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!(
            active_connections = shutdown_handle.connection_count(),
            graceful_timeout_seconds = GRACEFUL_SHUTDOWN_TIMEOUT.as_secs(),
            "vust-agent remote listener received shutdown signal. Stopping runtime resources..."
        );
        let _ = runtime_stop_tx_for_signal.send(true);
        shutdown_handle.graceful_shutdown(Some(GRACEFUL_SHUTDOWN_TIMEOUT));
    });

    if let Err(err) = axum_server::bind_rustls(listen_addr, tls_config)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
    {
        if err.kind() == std::io::ErrorKind::AddrInUse {
            tracing::error!(
                "Failed to start Agent server: Address {} is already in use. Please check if another instance of vust-agent is running or choose a different port.",
                listen_addr
            );
        } else {
            tracing::error!("Failed to bind to {}: {}", listen_addr, err);
        }
        std::process::exit(1);
    }

    let _ = runtime_stop_tx.send(true);
    wait_for_runtime_supervisor(runtime_handle).await;

    tracing::info!("Attempting to close agent database connection pool...");
    pools_for_shutdown.close().await;
    tracing::info!("Agent database connection pool closed successfully.");
}

/// 等待 supervisor 完成注销和本地会话清理。
async fn wait_for_runtime_supervisor(handle: tokio::task::JoinHandle<()>) {
    match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, handle).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::warn!(error = %err, "runtime supervisor task failed"),
        Err(_) => {
            tracing::warn!("runtime supervisor did not finish graceful deregistration in time")
        }
    }
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

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollRequest {
    enrollment_token: String,
    node: RuntimeNode,
    certificate_request: CertificateRequest,
    compatibility: RuntimeAgentCompatibility,
    command_credential: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    agent_id: String,
    node: RuntimeNode,
    compatibility: RuntimeAgentCompatibility,
    command_credential: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatRequest {
    agent_id: String,
    session_id: String,
    lease_id: String,
    sequence: i64,
    node: RuntimeNode,
    resource: Option<vust_contracts::types::HostSystemSummary>,
    compatibility: RuntimeAgentCompatibility,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeNode {
    advertise_addr: Option<String>,
    listen_port: Option<i64>,
    command_transport: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CertificateRequest {
    public_key_algorithm: String,
    csr_pem: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeAgentCompatibility {
    agent_version: String,
    runtime_protocol_version: String,
    min_supported_controller_version: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeControllerCompatibility {
    controller_version: String,
    runtime_protocol_version: String,
    min_supported_agent_version: String,
    compatible: bool,
    reason: String,
    required_action: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSessionResponse {
    agent_id: String,
    session_id: String,
    lease_id: String,
    heartbeat_interval_seconds: i64,
    controller_compatibility: RuntimeControllerCompatibility,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeregisterRequest {
    agent_id: String,
    session_id: String,
    reason: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RotateCertificateRequest {
    agent_id: String,
    session_id: String,
    reason: String,
    current_certificate_fingerprint: Option<String>,
    certificate_request: CertificateRequest,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationEventReportRequest {
    agent_id: String,
    session_id: String,
    events: Vec<vust_contracts::logging::AgentOperationEvent>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatResponse {
    lease_id: String,
    lease_ttl_seconds: i64,
    heartbeat_interval_seconds: i64,
    require_re_register: bool,
    require_certificate_rotation: bool,
    sequence_ignored: Option<bool>,
    controller_compatibility: RuntimeControllerCompatibility,
}

/// 拒绝未推进会话序号的心跳，避免等待旧租约自然过期后才重新注册。
fn ensure_heartbeat_sequence_applied(sequence_ignored: Option<bool>) -> anyhow::Result<()> {
    if sequence_ignored.unwrap_or(false) {
        anyhow::bail!("controller ignored heartbeat sequence; runtime re-registration is required");
    }
    Ok(())
}

/// 保留运行时接口的状态码与受限响应正文，便于定位会话拒绝原因。
async fn require_runtime_response(
    response: reqwest::Response,
    operation: &str,
) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let detail = response.text().await.unwrap_or_default();
    let detail = detail.trim().chars().take(1024).collect::<String>();
    if detail.is_empty() {
        anyhow::bail!("{operation} rejected with HTTP status {status}");
    }
    anyhow::bail!("{operation} rejected with HTTP status {status}: {detail}");
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RotateCertificateResponse {
    agent_id: String,
}

#[derive(Debug, Clone)]
struct RuntimeSessionState {
    vust_url: String,
    agent_id: String,
    session_id: String,
    lease_id: String,
    heartbeat_interval_seconds: u64,
    heartbeat_sequence: i64,
    advertise_addr: Option<String>,
    listen_port: Option<i64>,
}

async fn run_runtime_supervisor(
    pool: DbPool,
    mut stop_rx: watch::Receiver<bool>,
    controller_runtime: Arc<services::controller_runtime::ControllerRuntime>,
) {
    let mut retry_delay_seconds = RUNTIME_RETRY_INITIAL_DELAY_SECONDS;
    loop {
        if *stop_rx.borrow() {
            return;
        }

        match establish_runtime_session(&pool).await {
            Ok(session) => {
                retry_delay_seconds = RUNTIME_RETRY_INITIAL_DELAY_SECONDS;
                controller_runtime
                    .set_session(&session.vust_url, &session.agent_id, &session.session_id)
                    .await;
                let result = maintain_runtime_session(&pool, &mut stop_rx, session).await;
                controller_runtime.clear_session().await;
                if let Err(err) = result {
                    tracing::warn!("Runtime session dropped: {}", err);
                    let _ = clear_runtime_session(&pool).await;
                } else {
                    return;
                }
            }
            Err(err) => {
                tracing::warn!("Runtime session bootstrap failed: {}", err);
            }
        }

        let wait = tokio::time::sleep(std::time::Duration::from_secs(
            retry_delay_seconds + runtime_retry_jitter_seconds(),
        ));
        tokio::pin!(wait);
        tokio::select! {
            _ = &mut wait => {}
            changed = stop_rx.changed() => {
                if changed.is_ok() && *stop_rx.borrow() {
                    return;
                }
            }
        }
        retry_delay_seconds = (retry_delay_seconds * 2).min(RUNTIME_RETRY_MAX_DELAY_SECONDS);
    }
}

fn runtime_retry_jitter_seconds() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or_default();
    nanos % (RUNTIME_RETRY_JITTER_SECONDS + 1)
}

fn runtime_agent_compatibility() -> RuntimeAgentCompatibility {
    let config = &config::get().controller_compatibility;
    RuntimeAgentCompatibility {
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        runtime_protocol_version: config.runtime_protocol_version.clone(),
        min_supported_controller_version: config.min_supported_controller_version.clone(),
    }
}

fn ensure_controller_compatible(
    compatibility: &RuntimeControllerCompatibility,
) -> anyhow::Result<()> {
    let config = &config::get().controller_compatibility;
    if compatibility.runtime_protocol_version != config.runtime_protocol_version {
        return Err(anyhow::anyhow!(
            "controller runtime protocol mismatch: expected {}, got {}",
            config.runtime_protocol_version,
            compatibility.runtime_protocol_version
        ));
    }
    if !compatibility.compatible {
        return Err(anyhow::anyhow!(
            "controller rejected agent compatibility: {} ({})",
            compatibility.reason,
            compatibility.required_action
        ));
    }

    let controller = Version::parse(compatibility.controller_version.trim_start_matches('v'))
        .map_err(|err| anyhow::anyhow!("controller version is not valid SemVer: {err}"))?;
    let min_controller =
        Version::parse(&config.min_supported_controller_version).map_err(|err| {
            anyhow::anyhow!("minimum supported controller version is not valid SemVer: {err}")
        })?;
    if controller < min_controller {
        return Err(anyhow::anyhow!(
            "controller version {} is older than minimum supported {}",
            controller,
            min_controller
        ));
    }
    let agent = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|err| anyhow::anyhow!("agent version is not valid SemVer: {err}"))?;
    let min_agent = Version::parse(&compatibility.min_supported_agent_version).map_err(|err| {
        anyhow::anyhow!("controller minimum agent version is not valid SemVer: {err}")
    })?;
    if agent < min_agent {
        return Err(anyhow::anyhow!(
            "agent version {} is older than controller minimum supported {}",
            agent,
            min_agent
        ));
    }

    // 项目初期由于在线与局部升级兼容性需要，放宽了零主版本（0.x.x）的兼容限制。
    // 在 zero_major_requires_exact 为 false 时，仅要求首位主版本号一致；待以后项目文档或兼容矩阵明确后再行调整。
    let compatible = if agent.major == 0 {
        if config.zero_major_requires_exact {
            agent.major == controller.major
                && agent.minor == controller.minor
                && agent.patch == controller.patch
                && (!config.zero_major_requires_prerelease_match || agent.pre == controller.pre)
        } else {
            agent.major == controller.major
        }
    } else {
        !config.stable_requires_same_major || agent.major == controller.major
    };
    if !compatible {
        return Err(anyhow::anyhow!(
            "controller version {} is not compatible with agent version {}",
            controller,
            agent
        ));
    }

    Ok(())
}

async fn establish_runtime_session(pool: &DbPool) -> anyhow::Result<RuntimeSessionState> {
    let identity = load_or_init_identity(pool, config::get()).await?;
    let vust_url = match identity.mode {
        AgentMode::Local => config::local_controller_url()?,
        AgentMode::Remote => identity
            .vust_url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing vust_url"))?,
    };
    let client = build_tls_client()?;
    let node = build_runtime_node(&identity);
    let compatibility = runtime_agent_compatibility();
    let clear_enrollment_token = identity.enrollment_token.is_some();
    let command_credential = generate_command_credential()?;

    let session = if identity.mode == AgentMode::Remote
        && let Some(token) = identity.enrollment_token.clone()
    {
        let response = client
            .post(format!(
                "{}/api/v1/runtime/enroll",
                vust_url.trim_end_matches('/')
            ))
            .json(&EnrollRequest {
                enrollment_token: token,
                node: node.clone(),
                certificate_request: CertificateRequest {
                    public_key_algorithm: "ed25519".to_string(),
                    csr_pem: None,
                },
                compatibility: compatibility.clone(),
                command_credential: command_credential.clone(),
            })
            .send()
            .await?
            .error_for_status()?;
        let payload = response
            .json::<ApiResponse<RuntimeSessionResponse>>()
            .await?;
        if !payload.success {
            return Err(anyhow::anyhow!(payload.message));
        }
        payload
            .data
            .ok_or_else(|| anyhow::anyhow!("missing enroll response data"))?
    } else {
        let agent_id = match identity.mode {
            AgentMode::Local => "local".to_string(),
            AgentMode::Remote => identity
                .agent_id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing agent_id"))?,
        };
        let response = client
            .post(format!(
                "{}/api/v1/runtime/register",
                vust_url.trim_end_matches('/')
            ))
            .json(&RegisterRequest {
                agent_id,
                node: node.clone(),
                compatibility: compatibility.clone(),
                command_credential: command_credential.clone(),
            })
            .send()
            .await?
            .error_for_status()?;
        let payload = response
            .json::<ApiResponse<RuntimeSessionResponse>>()
            .await?;
        if !payload.success {
            return Err(anyhow::anyhow!(payload.message));
        }
        payload
            .data
            .ok_or_else(|| anyhow::anyhow!("missing register response data"))?
    };
    ensure_controller_compatible(&session.controller_compatibility)?;
    services::settings::set_string(
        pool,
        "runtime.command_credential_hash",
        &hash_command_credential(&command_credential),
    )
    .await?;

    update_runtime_session(
        pool,
        &session.agent_id,
        &session.session_id,
        &session.lease_id,
        clear_enrollment_token,
    )
    .await?;

    Ok(RuntimeSessionState {
        vust_url,
        agent_id: session.agent_id,
        session_id: session.session_id,
        lease_id: session.lease_id,
        heartbeat_interval_seconds: session.heartbeat_interval_seconds.max(1) as u64,
        heartbeat_sequence: 1,
        advertise_addr: node.advertise_addr,
        listen_port: node.listen_port,
    })
}

fn generate_command_credential() -> anyhow::Result<String> {
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to generate Agent command credential"))?;
    Ok(hex::encode(bytes))
}

fn hash_command_credential(credential: &str) -> String {
    hex::encode(ring::digest::digest(
        &ring::digest::SHA256,
        credential.as_bytes(),
    ))
}

async fn report_task_run_to_controller(
    client: &reqwest::Client,
    session: &RuntimeSessionState,
    mut report: vust_contracts::scheduled_tasks::AgentScheduledTaskRunReport,
) -> anyhow::Result<()> {
    report.run.node_id.clone_from(&session.agent_id);
    let payload = serde_json::json!({
        "agentId": session.agent_id,
        "sessionId": session.session_id,
        "runs": vec![report],
    });

    let url = format!(
        "{}/api/v1/runtime/scheduled-tasks/runs/report",
        session.vust_url.trim_end_matches('/')
    );
    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .await?
        .error_for_status()?;
    let api_resp = response.json::<ApiResponse<()>>().await?;
    if !api_resp.success {
        return Err(anyhow::anyhow!(
            "controller returned failure: {}",
            api_resp.message
        ));
    }
    Ok(())
}

/// 将脚本运行 outbox 批量上报给 Master，成功响应即作为确认。
async fn report_script_runs_to_controller(
    client: &reqwest::Client,
    session: &RuntimeSessionState,
    reports: Vec<vust_contracts::scripts::AgentScriptRunReport>,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "agentId": session.agent_id,
        "sessionId": session.session_id,
        "reports": reports,
    });
    let response = client
        .post(format!(
            "{}/api/v1/runtime/script-runs/report",
            session.vust_url.trim_end_matches('/')
        ))
        .json(&payload)
        .send()
        .await?
        .error_for_status()?;
    let body = response.json::<ApiResponse<()>>().await?;
    if !body.success {
        return Err(anyhow::anyhow!(
            "controller returned failure: {}",
            body.message
        ));
    }
    Ok(())
}

async fn pull_and_sync_tasks(
    pool: &DbPool,
    client: &reqwest::Client,
    session: &RuntimeSessionState,
) -> anyhow::Result<()> {
    let url = format!(
        "{}/api/v1/runtime/scheduled-tasks/snapshot?agentId={}&sessionId={}",
        session.vust_url.trim_end_matches('/'),
        session.agent_id,
        session.session_id
    );
    let resp = client.get(url).send().await?.error_for_status()?;
    let payload = resp
        .json::<ApiResponse<Vec<vust_contracts::scheduled_tasks::AgentScheduledTaskDefinition>>>()
        .await?;
    if !payload.success {
        return Err(anyhow::anyhow!(
            "snapshot query failed: {}",
            payload.message
        ));
    }
    let remote_tasks = payload.data.unwrap_or_default();

    let local_tasks = models::scheduled_tasks::list_all_tasks(pool).await?;
    let mut local_map: std::collections::HashMap<
        String,
        models::scheduled_tasks::AgentScheduledTask,
    > = local_tasks
        .into_iter()
        .map(|task| (task.task_id.clone(), task))
        .collect();

    for remote in remote_tasks {
        let upsert_needed = if let Some(local) = local_map.remove(&remote.task_id) {
            local.revision != remote.revision
        } else {
            true
        };

        if upsert_needed
            && let Err(error) = models::scheduled_tasks::upsert_task(pool, &remote).await
        {
            tracing::warn!(
                task_id = %remote.task_id,
                %error,
                "failed to align scheduled task from Master snapshot"
            );
        }
    }

    for (task_id, _) in local_map {
        if let Err(error) =
            models::scheduled_tasks::delete_task_for_reconciliation(pool, &task_id).await
        {
            tracing::warn!(%task_id, %error, "failed to clean up obsolete scheduled task");
        }
    }

    Ok(())
}

async fn maintain_runtime_session(
    pool: &DbPool,
    stop_rx: &mut watch::Receiver<bool>,
    mut session: RuntimeSessionState,
) -> anyhow::Result<()> {
    let client = build_tls_client()?;
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
        session.heartbeat_interval_seconds,
    ));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // `interval` 首次 tick 会立即返回，先消费掉，避免重连后马上连续发送 heartbeat。
    ticker.tick().await;

    // 首次上线/重连时立即发起一次主动拉取对齐
    if let Err(err) = pull_and_sync_tasks(pool, &client, &session).await {
        tracing::warn!("Initial pull-based sync failed: {:?}", err);
    }

    let mut pull_sync_ticker = tokio::time::interval(std::time::Duration::from_secs(300));
    pull_sync_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // 消费首次 tick
    pull_sync_ticker.tick().await;

    let mut task_report_ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    task_report_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut script_report_ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    script_report_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut operation_event_ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    operation_event_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = operation_event_ticker.tick() => {
                let events = services::operation_outbox::pending(pool, 100).await?;
                if !events.is_empty() {
                    let event_ids = events.iter().map(|event| event.event_id.clone()).collect::<Vec<_>>();
                    let response = client.post(format!("{}/api/v1/runtime/operation-events/report", session.vust_url.trim_end_matches('/')))
                        .json(&OperationEventReportRequest { agent_id: session.agent_id.clone(), session_id: session.session_id.clone(), events }).send().await;
                    match response {
                        Ok(response) if response.status().is_success() => {
                            let payload = response.json::<ApiResponse<vust_contracts::logging::AgentOperationEventAck>>().await?;
                            let accepted = payload.data.map(|value| value.accepted_event_ids).unwrap_or_default();
                            services::operation_outbox::acknowledge(pool, &accepted).await?;
                        }
                        Ok(response) => {
                            services::operation_outbox::mark_failed(pool, &event_ids).await?;
                            tracing::warn!(status=%response.status(), "operation audit report rejected; durable retry retained");
                        }
                        Err(error) => {
                            services::operation_outbox::mark_failed(pool, &event_ids).await?;
                            tracing::warn!(%error, "operation audit report failed; durable retry retained");
                        }
                    }
                }
            }
            _ = pull_sync_ticker.tick() => {
                if let Err(err) = pull_and_sync_tasks(pool, &client, &session).await {
                    tracing::warn!("Scheduled pull-based sync failed: {:?}", err);
                }
            }
            _ = task_report_ticker.tick() => {
                for item in models::scheduled_tasks::list_outbox(pool, 20).await? {
                    match report_task_run_to_controller(&client, &session, item.report).await {
                        Ok(()) => models::scheduled_tasks::acknowledge_outbox(pool, &item.run_id).await?,
                        Err(error) => {
                            models::scheduled_tasks::mark_outbox_attempt(pool, &item.run_id).await?;
                            tracing::warn!(%error, run_id = %item.run_id, "failed to report scheduled task run; durable retry retained");
                            break;
                        }
                    }
                }
            }
            _ = script_report_ticker.tick() => {
                let reports = models::script_runs::pending_reports(pool, 20).await?;
                if !reports.is_empty() {
                    let run_ids = reports.iter().map(|report| report.run_id.clone()).collect::<Vec<_>>();
                    match report_script_runs_to_controller(&client, &session, reports).await {
                        Ok(()) => models::script_runs::acknowledge(pool, &run_ids).await?,
                        Err(error) => {
                            models::script_runs::mark_attempt(pool, &run_ids).await?;
                            tracing::warn!(%error, "failed to report script runs; durable retry retained");
                        }
                    }
                }
            }
            _ = ticker.tick() => {
                let resource = tokio::task::spawn_blocking(services::system_metrics::collect_snapshot)
                    .await
                    .ok()
                    .and_then(|r| r.ok());

                let response = client
                    .post(format!("{}/api/v1/runtime/heartbeat", session.vust_url.trim_end_matches('/')))
                    .json(&HeartbeatRequest {
                        agent_id: session.agent_id.clone(),
                        session_id: session.session_id.clone(),
                        lease_id: session.lease_id.clone(),
                        sequence: session.heartbeat_sequence,
                        node: RuntimeNode {
                            advertise_addr: session.advertise_addr.clone(),
                            listen_port: session.listen_port,
                            command_transport: if session.agent_id == "local" {
                                "uds".to_string()
                            } else {
                                "https".to_string()
                            },
                        },
                        resource,
                        compatibility: runtime_agent_compatibility(),
                    })
                    .send()
                    .await?;
                let response = require_runtime_response(response, "runtime heartbeat").await?;
                let payload = response
                    .json::<ApiResponse<HeartbeatResponse>>()
                    .await?;
                if !payload.success {
                    return Err(anyhow::anyhow!(payload.message));
                }
                let heartbeat = payload
                    .data
                    .ok_or_else(|| anyhow::anyhow!("missing heartbeat response data"))?;
                if heartbeat.require_certificate_rotation {
                    rotate_runtime_certificate(&client, &session).await?;
                }
                ensure_controller_compatible(&heartbeat.controller_compatibility)?;
                if heartbeat.require_re_register {
                    return Err(anyhow::anyhow!("vust requested re-register"));
                }
                ensure_heartbeat_sequence_applied(heartbeat.sequence_ignored)?;
                let _ = heartbeat.lease_ttl_seconds;
                session.lease_id = heartbeat.lease_id;
                session.heartbeat_interval_seconds = heartbeat.heartbeat_interval_seconds.max(1) as u64;
                session.heartbeat_sequence = session.heartbeat_sequence.saturating_add(1);
                update_runtime_session(
                    pool,
                    &session.agent_id,
                    &session.session_id,
                    &session.lease_id,
                    false,
                )
                .await?;
                ticker = tokio::time::interval(std::time::Duration::from_secs(
                    session.heartbeat_interval_seconds,
                ));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // 重建 interval 后同样先消费首次即时 tick，维持稳定发送间隔。
                ticker.tick().await;
            }
            changed = stop_rx.changed() => {
                if changed.is_ok() && *stop_rx.borrow() {
                    let _ = deregister_runtime_session(&client, &session).await;
                    let _ = clear_runtime_session(pool).await;
                    return Ok(());
                }
                if changed.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

async fn deregister_runtime_session(
    client: &reqwest::Client,
    session: &RuntimeSessionState,
) -> anyhow::Result<()> {
    let response = client
        .post(format!(
            "{}/api/v1/runtime/deregister",
            session.vust_url.trim_end_matches('/')
        ))
        .json(&DeregisterRequest {
            agent_id: session.agent_id.clone(),
            session_id: session.session_id.clone(),
            reason: "shutdown".to_string(),
        })
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "deregister failed with status {}",
            response.status()
        ));
    }

    Ok(())
}

async fn rotate_runtime_certificate(
    client: &reqwest::Client,
    session: &RuntimeSessionState,
) -> anyhow::Result<()> {
    let csr = format!("rotate-{}", chrono::Utc::now().timestamp_millis());
    let response = client
        .post(format!(
            "{}/api/v1/runtime/rotate-certificate",
            session.vust_url.trim_end_matches('/')
        ))
        .json(&RotateCertificateRequest {
            agent_id: session.agent_id.clone(),
            session_id: session.session_id.clone(),
            reason: "requested_by_vust".to_string(),
            current_certificate_fingerprint: None,
            certificate_request: CertificateRequest {
                public_key_algorithm: "ed25519".to_string(),
                csr_pem: Some(csr),
            },
        })
        .send()
        .await?
        .error_for_status()?;
    let payload = response
        .json::<ApiResponse<RotateCertificateResponse>>()
        .await?;
    if !payload.success {
        return Err(anyhow::anyhow!(payload.message));
    }
    let rotated = payload
        .data
        .ok_or_else(|| anyhow::anyhow!("missing rotate-certificate response data"))?;
    if rotated.agent_id != session.agent_id {
        return Err(anyhow::anyhow!(
            "rotate-certificate response agent mismatch"
        ));
    }
    Ok(())
}

fn build_runtime_node(identity: &crate::models::identity::AgentIdentity) -> RuntimeNode {
    match identity.mode {
        AgentMode::Local => RuntimeNode {
            advertise_addr: None,
            listen_port: None,
            command_transport: "uds".to_string(),
        },
        AgentMode::Remote => RuntimeNode {
            advertise_addr: identity.agent_ip.clone(),
            listen_port: identity
                .listen_addr
                .as_deref()
                .and_then(|value| value.rsplit(':').next())
                .and_then(|value| value.parse::<i64>().ok()),
            command_transport: "https".to_string(),
        },
    }
}

/// 为应用程序设置日志记录基础设施。
fn init_logging() -> Option<WorkerGuard> {
    let file_writer = init_file_log_writer("agent");

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
    if std::env::var_os("VUST_HOME").is_some() {
        return production_home().join("logs");
    }
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

async fn build_tls_config(
    identity: &mut crate::models::identity::AgentIdentity,
    pool: &DbPool,
) -> anyhow::Result<RustlsConfig> {
    let sans = derive_sans(identity);
    ensure_identity_certs(pool, identity, &sans).await?;

    let cert_pem = identity
        .cert_pem
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing agent certificate"))?;
    let key_pem = identity
        .key_pem
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing agent private key"))?;

    let mut chain_pem = Vec::new();
    chain_pem.extend_from_slice(cert_pem);
    chain_pem.extend_from_slice(AGENT_CA_CERT_PEM);

    let certs = load_certs_from_bytes(&chain_pem)?;
    let key = load_private_key_from_bytes(key_pem)?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in load_certs_from_bytes(AGENT_CA_CERT_PEM)? {
        roots.add(cert)?;
    }

    let client_auth = WebPkiClientVerifier::builder(roots.into())
        .build()
        .map_err(|err| anyhow::anyhow!("Failed to build client verifier: {}", err))?;
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(client_auth)
        .with_single_cert(certs, key)?;

    Ok(RustlsConfig::from_config(std::sync::Arc::new(config)))
}

fn derive_sans(identity: &crate::models::identity::AgentIdentity) -> Vec<String> {
    let mut sans = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "host.docker.internal".to_string(),
    ];
    if let Some(ip) = identity.agent_ip.as_ref() {
        if !sans.contains(ip) {
            sans.push(ip.clone());
        }
        return sans;
    }
    if let Some(listen) = identity.listen_addr.as_ref()
        && let Some((host, _)) = listen.split_once(':')
        && host != "0.0.0.0"
        && host != "127.0.0.1"
    {
        let host = host.trim_matches(['[', ']']);
        if !sans.iter().any(|value| value == host) {
            sans.push(host.to_string());
        }
    }
    sans
}

fn load_certs_from_bytes(data: &[u8]) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let mut reader = std::io::Cursor::new(data);
    Ok(rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?)
}

fn load_private_key_from_bytes(data: &[u8]) -> anyhow::Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::Cursor::new(data);
    rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| anyhow::anyhow!("No private key found in embedded key"))
}

#[cfg(test)]
mod runtime_session_tests {
    use super::ensure_heartbeat_sequence_applied;

    #[test]
    fn ignored_heartbeat_sequence_requires_re_registration() {
        assert!(ensure_heartbeat_sequence_applied(Some(true)).is_err());
        assert!(ensure_heartbeat_sequence_applied(Some(false)).is_ok());
        assert!(ensure_heartbeat_sequence_applied(None).is_ok());
    }
}
