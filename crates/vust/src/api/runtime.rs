//! runtime API：节点 enrollment、register、heartbeat 与 deregister。

use crate::models::logging::{LogModule, PlatformLogLevel};
use crate::services::logging::OperationEventBuilder;
use crate::services::node_runtime;
use crate::services::runtime_metrics;
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use vust_contracts::api::ErrorCode;
use vust_contracts::logging::{AgentOperationEvent, AgentOperationEventAck};
use vust_contracts::runtime_docker::{
    RuntimeImageLayerProgress, RuntimeImageSource, RuntimeImageStage, RuntimeImageTask,
    RuntimeImageTaskCreateRequest, RuntimeImageTaskQuery, RuntimeImageTaskStatus,
};
use vust_contracts::terminal::{TerminalTicketConsumeRequest, TerminalTicketConsumeResponse};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeNodePayload {
    pub advertise_addr: Option<String>,
    pub listen_port: Option<i64>,
    pub command_transport: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateRequestPayload {
    pub public_key_algorithm: Option<String>,
    pub csr_pem: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAgentCompatibilityPayload {
    pub agent_version: Option<String>,
    pub runtime_protocol_version: Option<String>,
    pub min_supported_controller_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeControllerCompatibilityResponse {
    pub controller_version: String,
    pub runtime_protocol_version: String,
    pub min_supported_agent_version: String,
    pub compatible: bool,
    pub reason: String,
    pub required_action: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCallbackProbeResponse {
    pub controller_version: String,
    pub runtime_protocol_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollPayload {
    pub enrollment_token: String,
    pub node: RuntimeNodePayload,
    pub certificate_request: Option<CertificateRequestPayload>,
    pub compatibility: Option<RuntimeAgentCompatibilityPayload>,
    pub command_credential: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterPayload {
    pub agent_id: String,
    pub node: RuntimeNodePayload,
    pub compatibility: Option<RuntimeAgentCompatibilityPayload>,
    pub command_credential: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatPayload {
    pub agent_id: String,
    pub session_id: String,
    pub lease_id: String,
    pub sequence: i64,
    pub node: Option<RuntimeNodePayload>,
    pub resource: Option<vust_contracts::types::HostSystemSummary>,
    pub compatibility: Option<RuntimeAgentCompatibilityPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeregisterPayload {
    pub agent_id: String,
    pub session_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationEventReportPayload {
    agent_id: String,
    session_id: String,
    events: Vec<AgentOperationEvent>,
}

/// 校验 Agent 运行时身份、活动状态和租约。
async fn require_runtime_session(
    pool: &crate::state::DbPool,
    agent_id: &str,
    session_id: &str,
) -> ApiResult<crate::models::node_sessions::NodeSessionRecord> {
    crate::models::node_sessions::get_session_by_id(pool, session_id)
        .await?
        .filter(|session| {
            session.agent_id == agent_id && crate::models::node_sessions::has_live_lease(session)
        })
        .ok_or_else(|| {
            ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "runtime session is inactive or expired",
            )
        })
}

/// 校验 Agent 运行时会话并返回会话绑定的节点 ID。
async fn require_runtime_node(
    pool: &crate::state::DbPool,
    agent_id: &str,
    session_id: &str,
) -> ApiResult<String> {
    Ok(require_runtime_session(pool, agent_id, session_id)
        .await?
        .node_id)
}

/// 将 Master 内部镜像任务转换为稳定的运行时契约。
fn runtime_image_task(task: crate::services::image_acquisition::ImageTask) -> RuntimeImageTask {
    use crate::services::image_acquisition::{ImageSource, ImageStage, ImageTaskStatus};
    RuntimeImageTask {
        task_id: task.task_id,
        image_ref: task.image_ref,
        status: match task.status {
            ImageTaskStatus::Pending => RuntimeImageTaskStatus::Pending,
            ImageTaskStatus::Running => RuntimeImageTaskStatus::Running,
            ImageTaskStatus::Success => RuntimeImageTaskStatus::Success,
            ImageTaskStatus::Failed => RuntimeImageTaskStatus::Failed,
            ImageTaskStatus::Cancelled => RuntimeImageTaskStatus::Cancelled,
        },
        source: task.source.map(|source| match source {
            ImageSource::Target => RuntimeImageSource::Target,
            ImageSource::Controller => RuntimeImageSource::Controller,
            ImageSource::Registry => RuntimeImageSource::Registry,
        }),
        stage: match task.stage {
            ImageStage::Checking => RuntimeImageStage::Checking,
            ImageStage::Exporting => RuntimeImageStage::Exporting,
            ImageStage::Uploading => RuntimeImageStage::Uploading,
            ImageStage::Loading => RuntimeImageStage::Loading,
            ImageStage::Pulling => RuntimeImageStage::Pulling,
        },
        progress_percent: task.progress_percent,
        status_text: task.status_text,
        layers: task
            .layers
            .into_iter()
            .map(|layer| RuntimeImageLayerProgress {
                id: layer.id,
                status_text: layer.status.unwrap_or_else(|| "Pulling".to_string()),
                percent: layer.progress_percent,
            })
            .collect(),
    }
}

/// 为当前 Agent 所属节点创建主控优先的镜像获取任务。
async fn create_runtime_image_task(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RuntimeImageTaskCreateRequest>,
) -> ApiResult<Response> {
    let image_ref = payload.image_ref.trim();
    if image_ref.is_empty() || image_ref.chars().count() > 512 {
        return Err(ApiError::bad_request(
            ErrorCode::BadRequest,
            "imageRef must contain 1 to 512 characters",
        ));
    }
    let node_id =
        require_runtime_node(&state.metadata_db, &payload.agent_id, &payload.session_id).await?;
    let task =
        state
            .image_acquisition
            .start(Arc::clone(&state), node_id, image_ref.to_string(), None);
    Ok(
        ApiResponse::success_with_raw("Runtime image task started", Some(runtime_image_task(task)))
            .into_response(),
    )
}

/// 查询当前 Agent 自己创建的镜像获取任务。
async fn runtime_image_task_progress(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<RuntimeImageTaskQuery>,
) -> ApiResult<Response> {
    let node_id =
        require_runtime_node(&state.metadata_db, &query.agent_id, &query.session_id).await?;
    let task = state
        .image_acquisition
        .get(&task_id)
        .filter(|task| task.node_id == node_id)
        .ok_or_else(|| ApiError::not_found(ErrorCode::TaskNotFound, "image task not found"))?;
    Ok(
        ApiResponse::success_with_raw("Runtime image task loaded", Some(runtime_image_task(task)))
            .into_response(),
    )
}

/// 取消当前 Agent 自己创建且仍在运行的镜像获取任务。
async fn cancel_runtime_image_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<RuntimeImageTaskQuery>,
) -> ApiResult<Response> {
    let node_id =
        require_runtime_node(&state.metadata_db, &query.agent_id, &query.session_id).await?;
    let owned = state
        .image_acquisition
        .get(&task_id)
        .is_some_and(|task| task.node_id == node_id);
    if !owned {
        return Err(ApiError::not_found(
            ErrorCode::TaskNotFound,
            "image task not found",
        ));
    }
    let task = state
        .image_acquisition
        .cancel(&task_id)
        .ok_or_else(|| ApiError::not_found(ErrorCode::TaskNotFound, "image task not found"))?;
    Ok(ApiResponse::success_with_raw(
        "Runtime image task cancellation requested",
        Some(runtime_image_task(task)),
    )
    .into_response())
}

/// 接收 Agent 持久 outbox 上报，并以机器会话覆盖来源节点身份。
async fn report_operation_events(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<OperationEventReportPayload>,
) -> ApiResult<Response> {
    if payload.events.is_empty() || payload.events.len() > 100 {
        return Err(ApiError::bad_request(
            ErrorCode::BadRequest,
            "operation event batch must contain 1 to 100 events",
        ));
    }
    let session =
        require_runtime_session(&state.metadata_db, &payload.agent_id, &payload.session_id).await?;
    let node_name = crate::models::nodes::get_node_by_id(&state.metadata_db, &session.node_id)
        .await?
        .map(|node| node.name);
    let mut accepted_event_ids = Vec::with_capacity(payload.events.len());
    for event in payload.events {
        let event_id = event.event_id.clone();
        crate::services::logging::store_agent_event(
            &state.metadata_db,
            &session.node_id,
            node_name.as_deref(),
            event,
        )
        .await?;
        accepted_event_ids.push(event_id);
    }
    Ok(ApiResponse::success_with_raw(
        "Operation events accepted",
        Some(AgentOperationEventAck { accepted_event_ids }),
    )
    .into_response())
}

/// 由 Agent 原子消费 Master 签发的一次性终端票据。
async fn consume_terminal_ticket(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TerminalTicketConsumeRequest>,
) -> ApiResult<Response> {
    let context = state
        .terminal_tickets
        .consume(&payload.ticket, &payload.node_id)
        .ok_or_else(|| {
            ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "terminal ticket is invalid or expired",
            )
        })?;
    Ok(ApiResponse::success_with_raw(
        "Terminal ticket consumed",
        Some(TerminalTicketConsumeResponse {
            actor_name: context.actor_name,
            client_ip: context.client_ip,
            trace_id: context.trace_id,
            node_id: context.node_id,
        }),
    )
    .into_response())
}

/// Runtime 回连探针：用于部署预检验证目标节点能访问主控入口，不产生运行时状态。
pub async fn callback_probe() -> ApiResult<impl IntoResponse> {
    let config = &crate::config::get().agent_version_compatibility;
    Ok(ApiResponse::success_with_raw(
        "Runtime callback probe succeeded",
        Some(RuntimeCallbackProbeResponse {
            controller_version: env!("CARGO_PKG_VERSION").to_string(),
            runtime_protocol_version: config.runtime_protocol_version.clone(),
        }),
    )
    .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateCertificatePayload {
    pub agent_id: String,
    pub session_id: String,
    pub reason: Option<String>,
    pub current_certificate_fingerprint: Option<String>,
    pub certificate_request: Option<CertificateRequestPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDownloadQuery {
    pub token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionResponse {
    pub node_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub lease_id: String,
    pub lease_ttl_seconds: i64,
    pub heartbeat_interval_seconds: i64,
    pub session_replaced: Option<bool>,
    pub controller_compatibility: RuntimeControllerCompatibilityResponse,
}

fn controller_compatibility(
    agent: Option<&RuntimeAgentCompatibilityPayload>,
) -> RuntimeControllerCompatibilityResponse {
    let config = &crate::config::get().agent_version_compatibility;
    let expected_protocol = config.runtime_protocol_version.clone();
    let controller_version = env!("CARGO_PKG_VERSION").to_string();
    let agent_version = agent.and_then(|value| value.agent_version.as_deref());
    let agent_protocol = agent.and_then(|value| value.runtime_protocol_version.as_deref());
    let agent_min_controller_version =
        agent.and_then(|value| value.min_supported_controller_version.as_deref());

    if agent_protocol != Some(expected_protocol.as_str()) {
        return RuntimeControllerCompatibilityResponse {
            controller_version,
            runtime_protocol_version: expected_protocol,
            min_supported_agent_version: config.min_supported_agent_version.clone(),
            compatible: false,
            reason: "Agent runtime protocol is not compatible with this controller".to_string(),
            required_action: "upgrade_agent".to_string(),
        };
    }

    let Some(agent_version) = agent_version else {
        return RuntimeControllerCompatibilityResponse {
            controller_version,
            runtime_protocol_version: expected_protocol,
            min_supported_agent_version: config.min_supported_agent_version.clone(),
            compatible: false,
            reason: "Agent version is missing".to_string(),
            required_action: "upgrade_agent".to_string(),
        };
    };

    let controller = match Version::parse(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(err) => {
            return RuntimeControllerCompatibilityResponse {
                controller_version,
                runtime_protocol_version: expected_protocol,
                min_supported_agent_version: config.min_supported_agent_version.clone(),
                compatible: false,
                reason: format!("Controller version is not valid SemVer: {err}"),
                required_action: "repair_controller".to_string(),
            };
        }
    };
    let Some(agent_min_controller_version) = agent_min_controller_version else {
        return RuntimeControllerCompatibilityResponse {
            controller_version,
            runtime_protocol_version: expected_protocol,
            min_supported_agent_version: config.min_supported_agent_version.clone(),
            compatible: false,
            reason: "Agent minimum supported controller version is missing".to_string(),
            required_action: "upgrade_agent".to_string(),
        };
    };
    let agent_min_controller =
        match Version::parse(agent_min_controller_version.trim_start_matches('v')) {
            Ok(version) => version,
            Err(err) => {
                return RuntimeControllerCompatibilityResponse {
                    controller_version,
                    runtime_protocol_version: expected_protocol,
                    min_supported_agent_version: config.min_supported_agent_version.clone(),
                    compatible: false,
                    reason: format!(
                        "Agent minimum supported controller version is not valid SemVer: {err}"
                    ),
                    required_action: "upgrade_agent".to_string(),
                };
            }
        };
    if controller < agent_min_controller {
        return RuntimeControllerCompatibilityResponse {
            controller_version,
            runtime_protocol_version: expected_protocol,
            min_supported_agent_version: config.min_supported_agent_version.clone(),
            compatible: false,
            reason: "Controller version is older than agent minimum supported controller version"
                .to_string(),
            required_action: "upgrade_controller".to_string(),
        };
    }
    let agent = match Version::parse(agent_version.trim_start_matches('v')) {
        Ok(version) => version,
        Err(err) => {
            return RuntimeControllerCompatibilityResponse {
                controller_version,
                runtime_protocol_version: expected_protocol,
                min_supported_agent_version: config.min_supported_agent_version.clone(),
                compatible: false,
                reason: format!("Agent version is not valid SemVer: {err}"),
                required_action: "upgrade_agent".to_string(),
            };
        }
    };
    let min_agent = match Version::parse(&config.min_supported_agent_version) {
        Ok(version) => version,
        Err(err) => {
            return RuntimeControllerCompatibilityResponse {
                controller_version,
                runtime_protocol_version: expected_protocol,
                min_supported_agent_version: config.min_supported_agent_version.clone(),
                compatible: false,
                reason: format!("Minimum supported agent version is not valid SemVer: {err}"),
                required_action: "repair_controller".to_string(),
            };
        }
    };

    // 项目初期由于在线与局部升级兼容性需要，放宽了零主版本（0.x.x）的兼容限制。
    // 在 zero_major_requires_exact 为 false 时，仅要求首位主版本号一致；待以后项目文档或兼容矩阵明确后再行调整。
    let compatible = agent >= min_agent
        && if controller.major == 0 {
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

    RuntimeControllerCompatibilityResponse {
        controller_version,
        runtime_protocol_version: expected_protocol,
        min_supported_agent_version: config.min_supported_agent_version.clone(),
        compatible,
        reason: if compatible {
            "Agent is compatible with this controller".to_string()
        } else {
            "Agent is not compatible with this controller".to_string()
        },
        required_action: if compatible {
            "none".to_string()
        } else {
            "upgrade_agent".to_string()
        },
    }
}

fn ensure_runtime_agent_compatible(
    agent: Option<&RuntimeAgentCompatibilityPayload>,
) -> Result<RuntimeControllerCompatibilityResponse, ApiError> {
    let compatibility = controller_compatibility(agent);
    if compatibility.compatible {
        Ok(compatibility)
    } else {
        Err(ApiError::BadRequest(compatibility.reason.clone()))
    }
}

pub async fn enroll(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<EnrollPayload>,
) -> ApiResult<impl IntoResponse> {
    let controller_compatibility = ensure_runtime_agent_compatible(payload.compatibility.as_ref())?;
    let command_transport = validate_command_access(
        false,
        &payload.node.command_transport,
        &payload.command_credential,
    )?;
    let mut platform_log = OperationEventBuilder::new("runtime-agent", "runtime_enroll", addr.ip())
        .module(LogModule::System)
        .target_type("node")
        .metadata(json!({ "advertise_addr": payload.node.advertise_addr.clone(), "listen_port": payload.node.listen_port }));
    let result = node_runtime::enroll_node(
        &state.metadata_db,
        &payload.enrollment_token,
        payload.node.advertise_addr,
        payload.node.listen_port,
        payload
            .certificate_request
            .as_ref()
            .and_then(|value| value.public_key_algorithm.as_deref())
            .unwrap_or("ed25519"),
        payload
            .certificate_request
            .as_ref()
            .and_then(|value| value.csr_pem.as_deref()),
    )
    .await;
    let response = match result {
        Ok(result) => {
            store_command_access(
                &state.metadata_db,
                &result.session_id,
                command_transport,
                &payload.command_credential,
            )
            .await?;
            runtime_metrics::record_enroll(true);
            platform_log = platform_log.target_id(&result.node_id).set_success();

            let state_clone = Arc::clone(&state);
            let agent_id = result.agent_id.clone();
            tokio::spawn(async move {
                if let Err(err) =
                    crate::services::task_sync::reconcile_agent_tasks(&state_clone, &agent_id).await
                {
                    tracing::error!(
                        "Failed to reconcile scheduled tasks for agent {}: {}",
                        agent_id,
                        err
                    );
                }
            });

            Ok(ApiResponse::success_with_raw(
                "Node enrolled",
                Some(RuntimeSessionResponse {
                    node_id: result.node_id,
                    agent_id: result.agent_id,
                    session_id: result.session_id,
                    lease_id: result.lease_id,
                    lease_ttl_seconds: node_runtime::DEFAULT_LEASE_TTL_SECONDS,
                    heartbeat_interval_seconds: node_runtime::DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
                    session_replaced: None,
                    controller_compatibility,
                }),
            ))
        }
        Err(err) => {
            runtime_metrics::record_enroll(false);
            platform_log = platform_log.metadata(json!({ "error": err }));
            Err(err)
        }
    };
    platform_log.finish(&state.metadata_db);
    response
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<RegisterPayload>,
) -> ApiResult<impl IntoResponse> {
    let is_local = payload.agent_id == "local";
    let command_transport = validate_command_access(
        is_local,
        &payload.node.command_transport,
        &payload.command_credential,
    )?;
    if is_local {
        if !is_loopback_peer(addr.ip()) {
            return Err(ApiError::forbidden(
                ErrorCode::AuthForbidden,
                "local Agent registration requires a loopback connection",
            ));
        }
        node_runtime::ensure_local_runtime_identity(&state.metadata_db).await?;
    }
    let controller_compatibility = ensure_runtime_agent_compatible(payload.compatibility.as_ref())?;
    let mut platform_log = OperationEventBuilder::new("runtime-agent", "runtime_register", addr.ip())
        .module(LogModule::System)
        .target_type("agent")
        .target_id(&payload.agent_id)
        .metadata(json!({ "advertise_addr": payload.node.advertise_addr.clone(), "listen_port": payload.node.listen_port }));
    let result = node_runtime::register_node(
        &state.metadata_db,
        &payload.agent_id,
        payload.node.advertise_addr,
        payload.node.listen_port,
    )
    .await;
    match result {
        Ok(result) => {
            store_command_access(
                &state.metadata_db,
                &result.session_id,
                command_transport,
                &payload.command_credential,
            )
            .await?;
            runtime_metrics::record_register(true);
            platform_log = platform_log.set_success();
            platform_log.finish_runtime_register_success(&state.metadata_db);

            let state_clone = Arc::clone(&state);
            let agent_id = result.agent_id.clone();
            tokio::spawn(async move {
                if let Err(err) =
                    crate::services::task_sync::reconcile_agent_tasks(&state_clone, &agent_id).await
                {
                    tracing::error!(
                        "Failed to reconcile scheduled tasks for agent {}: {}",
                        agent_id,
                        err
                    );
                }
            });

            Ok(ApiResponse::success_with_raw(
                "Node registered",
                Some(RuntimeSessionResponse {
                    node_id: result.node_id,
                    agent_id: result.agent_id,
                    session_id: result.session_id,
                    lease_id: result.lease_id,
                    lease_ttl_seconds: node_runtime::DEFAULT_LEASE_TTL_SECONDS,
                    heartbeat_interval_seconds: node_runtime::DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
                    session_replaced: Some(result.session_replaced),
                    controller_compatibility,
                }),
            ))
        }
        Err(err) => {
            runtime_metrics::record_register(false);
            platform_log = platform_log.metadata(json!({ "error": err }));
            platform_log.finish_runtime_register_failure(&state.metadata_db);
            Err(err)
        }
    }
}

/// 识别原生回环地址及双栈监听产生的 IPv4-mapped IPv6 回环地址。
fn is_loopback_peer(ip: IpAddr) -> bool {
    ip.is_loopback()
        || matches!(
            ip,
            IpAddr::V6(ipv6)
                if ipv6
                    .to_ipv4_mapped()
                    .is_some_and(|ipv4| ipv4.is_loopback())
        )
}

async fn store_command_access(
    pool: &crate::state::DbPool,
    session_id: &str,
    transport: crate::models::node_sessions::AgentCommandTransport,
    credential: &str,
) -> ApiResult<()> {
    crate::models::node_sessions::upsert_session_command_access(
        pool, session_id, transport, credential,
    )
    .await
    .map_err(|err| ApiError::Internal(err.to_string()))
}

fn validate_command_access(
    is_local: bool,
    transport: &str,
    credential: &str,
) -> ApiResult<crate::models::node_sessions::AgentCommandTransport> {
    if credential.len() != 64 || !credential.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(ApiError::BadRequest(
            "Agent command credential is invalid".to_string(),
        ));
    }
    match (is_local, transport) {
        (true, "uds") => Ok(crate::models::node_sessions::AgentCommandTransport::Uds),
        (false, "https") => Ok(crate::models::node_sessions::AgentCommandTransport::Https),
        _ => Err(ApiError::BadRequest(
            "Agent command transport does not match node topology".to_string(),
        )),
    }
}

pub async fn heartbeat(
    State(state): State<Arc<AppState>>,
    ConnectInfo(_addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<HeartbeatPayload>,
) -> ApiResult<impl IntoResponse> {
    let controller_compatibility = ensure_runtime_agent_compatible(payload.compatibility.as_ref())?;
    let start = Instant::now();
    let result = node_runtime::heartbeat(
        &state.metadata_db,
        &payload.agent_id,
        &payload.session_id,
        &payload.lease_id,
        payload.sequence,
        payload
            .node
            .as_ref()
            .and_then(|node| node.advertise_addr.clone()),
        payload.node.as_ref().and_then(|node| node.listen_port),
        payload.resource,
    )
    .await;
    match result {
        Ok(result) => {
            runtime_metrics::record_heartbeat(true, result.sequence_ignored, start.elapsed());
            Ok(ApiResponse::success_with_raw(
                "Heartbeat accepted",
                Some(json!({
                    "leaseId": result.lease_id,
                    "leaseTtlSeconds": node_runtime::DEFAULT_LEASE_TTL_SECONDS,
                    "heartbeatIntervalSeconds": node_runtime::DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
                    "requireReRegister": false,
                    "requireCertificateRotation": false,
                    "sequenceIgnored": result.sequence_ignored,
                    "controllerCompatibility": controller_compatibility,
                    "commands": [],
                })),
            ))
        }
        Err(err) => {
            runtime_metrics::record_heartbeat(false, false, start.elapsed());
            Err(err)
        }
    }
}

pub async fn deregister(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<DeregisterPayload>,
) -> ApiResult<impl IntoResponse> {
    require_runtime_session(&state.metadata_db, &payload.agent_id, &payload.session_id).await?;
    let mut platform_log =
        OperationEventBuilder::new("runtime-agent", "runtime_deregister", addr.ip())
            .module(LogModule::System)
            .target_type("agent")
            .target_id(&payload.agent_id)
            .metadata(
                json!({ "agent_id": payload.agent_id.clone(), "reason": payload.reason.clone() }),
            );
    let result = node_runtime::deregister(
        &state.metadata_db,
        &payload.session_id,
        payload.reason.as_deref().unwrap_or("shutdown"),
    )
    .await;
    let response = match result {
        Ok(_) => {
            platform_log = platform_log.set_success();
            Ok(ApiResponse::success_with_raw(
                "Node deregistered",
                Some(json!({
                    "sessionClosed": true
                })),
            ))
        }
        Err(err) => {
            platform_log = platform_log.metadata(json!({ "error": err }));
            Err(err)
        }
    };
    platform_log.finish(&state.metadata_db);
    response
}

pub async fn rotate_certificate(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<RotateCertificatePayload>,
) -> ApiResult<impl IntoResponse> {
    require_runtime_session(&state.metadata_db, &payload.agent_id, &payload.session_id).await?;
    let mut platform_log =
        OperationEventBuilder::new("runtime-agent", "runtime_rotate_certificate", addr.ip())
            .module(LogModule::System)
            .target_type("agent")
            .target_id(&payload.agent_id)
            .metadata(json!({
                "agent_id": payload.agent_id.clone(),
                "reason": payload.reason.clone()
            }));
    let result = node_runtime::rotate_certificate(
        &state.metadata_db,
        &payload.agent_id,
        &payload.session_id,
        payload.current_certificate_fingerprint.as_deref(),
        payload
            .certificate_request
            .as_ref()
            .and_then(|value| value.public_key_algorithm.as_deref())
            .unwrap_or("ed25519"),
        payload
            .certificate_request
            .as_ref()
            .and_then(|value| value.csr_pem.as_deref()),
    )
    .await;
    let response = match result {
        Ok(result) => {
            platform_log = platform_log.set_success();
            Ok(ApiResponse::success_with_raw(
                "Certificate rotated",
                Some(json!({
                    "agentId": result.agent_id,
                    "oldCertificateRetireAfter": result.old_certificate_retire_after,
                    "issuedCertificatePem": "PENDING",
                    "coreTrustBundlePem": "PENDING"
                })),
            ))
        }
        Err(err) => {
            platform_log = platform_log.metadata(json!({ "error": err }));
            Err(err)
        }
    };
    platform_log.finish(&state.metadata_db);
    response
}

/// runtime 制品下载：agent 使用短期 token 从主控下载已缓存或待缓存的升级制品。
pub async fn download_upgrade_artifact(
    State(state): State<Arc<AppState>>,
    Path((version, component, target_triple)): Path<(String, String, String)>,
    Query(query): Query<ArtifactDownloadQuery>,
) -> ApiResult<Response> {
    crate::services::upgrades::validate_artifact_download_token(
        &state.metadata_db,
        &query.token,
        &version,
        &component,
        &target_triple,
    )
    .await?;
    let artifact = crate::services::upgrades::ensure_artifact_cached(
        &state.metadata_db,
        &version,
        &component,
        &target_triple,
    )
    .await?;
    let bytes = tokio::fs::read(&artifact.path)
        .await
        .map_err(ApiError::Io)?;
    let response = Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", artifact.file_name),
        )
        .header("x-vust-sha256", artifact.sha256)
        .body(Body::from(bytes))?;
    Ok(response)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportTaskRunsPayload {
    pub agent_id: String,
    pub session_id: String,
    pub runs: Vec<vust_contracts::scheduled_tasks::AgentScheduledTaskRunReport>,
}

pub async fn report_task_runs(
    State(state): State<Arc<AppState>>,
    ConnectInfo(_addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<ReportTaskRunsPayload>,
) -> ApiResult<impl IntoResponse> {
    // 校验 session 是否存在且处于活跃状态
    require_runtime_session(&state.metadata_db, &payload.agent_id, &payload.session_id).await?;

    if payload.runs.is_empty() || payload.runs.len() > 100 {
        return Err(ApiError::bad_request(
            vust_contracts::api::ErrorCode::BadRequest,
            "scheduled task run report must contain 1 to 100 items",
        ));
    }
    for report in &payload.runs {
        crate::models::task_scheduler::save_run_report(
            &state.metadata_db,
            &payload.agent_id,
            report,
        )
        .await?;
        if let Some(audit) = crate::models::task_scheduler::claim_run_terminal_audit(
            &state.metadata_db,
            &report.run.run_id,
        )
        .await?
            && let Ok(client_ip) = audit.client_ip.parse()
        {
            let failed = matches!(
                audit.status,
                vust_contracts::scheduled_tasks::ScheduledTaskRunStatus::Failed
                    | vust_contracts::scheduled_tasks::ScheduledTaskRunStatus::TimedOut
            );
            let target_name =
                crate::models::task_scheduler::get_task(&state.metadata_db, &audit.task_id)
                    .await?
                    .map(|task| task.name);
            OperationEventBuilder::new(
                &audit.actor_name,
                "scheduled_task_run_completed",
                client_ip,
            )
            .user_id(audit.actor_user_id)
            .module(LogModule::System)
            .target_type("scheduled_task")
            .target_id(&audit.task_id)
            .trace_id(&audit.trace_id)
            .outcome(match audit.status {
                vust_contracts::scheduled_tasks::ScheduledTaskRunStatus::Failed => {
                    vust_contracts::logging::OperationOutcome::Failure
                }
                vust_contracts::scheduled_tasks::ScheduledTaskRunStatus::TimedOut => {
                    vust_contracts::logging::OperationOutcome::TimedOut
                }
                vust_contracts::scheduled_tasks::ScheduledTaskRunStatus::Cancelled => {
                    vust_contracts::logging::OperationOutcome::Canceled
                }
                _ => vust_contracts::logging::OperationOutcome::Success,
            })
            .level(if failed {
                PlatformLogLevel::Error
            } else {
                PlatformLogLevel::Info
            })
            .metadata(json!({
                "runId": audit.run_id,
                "targetName": target_name,
                "result": format!("{:?}", audit.status).to_lowercase(),
                "errorCode": audit.error_code,
            }))
            .finish(&state.metadata_db);
        }
    }

    Ok(ApiResponse::ok("Report received"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportScriptRunsPayload {
    pub agent_id: String,
    pub session_id: String,
    pub reports: Vec<vust_contracts::scripts::AgentScriptRunReport>,
}

/// 接收脚本运行可靠上报，并逐条校验运行与当前节点会话的归属。
pub async fn report_script_runs(
    State(state): State<Arc<AppState>>,
    ConnectInfo(_addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<ReportScriptRunsPayload>,
) -> ApiResult<impl IntoResponse> {
    require_runtime_session(&state.metadata_db, &payload.agent_id, &payload.session_id).await?;
    if payload.reports.is_empty() || payload.reports.len() > 100 {
        return Err(ApiError::bad_request(
            ErrorCode::BadRequest,
            "script run report must contain 1 to 100 items",
        ));
    }

    for report in &payload.reports {
        crate::models::scripts::save_report(&state.metadata_db, &payload.agent_id, report).await?;
        if let Some(run) =
            crate::models::scripts::claim_terminal_audit(&state.metadata_db, &report.run_id).await?
        {
            if let Ok(client_ip) = run.client_ip.parse() {
                let failed = matches!(run.status.as_str(), "failed" | "timed_out");
                OperationEventBuilder::new(&run.actor_name, "script_run_completed", client_ip)
                    .user_id(run.actor_user_id)
                    .module(LogModule::System)
                    .target_type("script")
                    .target_id(&run.script_id)
                    .trace_id(&run.trace_id)
                    .outcome(match run.status.as_str() {
                        "failed" => vust_contracts::logging::OperationOutcome::Failure,
                        "timed_out" => vust_contracts::logging::OperationOutcome::TimedOut,
                        "cancelled" => vust_contracts::logging::OperationOutcome::Canceled,
                        _ => vust_contracts::logging::OperationOutcome::Success,
                    })
                    .level(if failed {
                        PlatformLogLevel::Error
                    } else {
                        PlatformLogLevel::Warning
                    })
                    .metadata(json!({
                        "runId": run.run_id,
                        "scriptName": run.script_name,
                        "revision": run.script_revision,
                        "nodeId": run.node_id,
                        "result": run.status,
                        "errorCode": run.error_code,
                    }))
                    .finish(&state.metadata_db);
            }
            crate::models::scripts::cleanup_discarded_run(&state.metadata_db, &report.run_id)
                .await?;
        }
    }

    Ok(ApiResponse::ok("Report received"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TasksSnapshotParams {
    pub session_id: String,
    pub agent_id: String,
}

pub async fn get_tasks_snapshot(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TasksSnapshotParams>,
) -> ApiResult<impl IntoResponse> {
    require_runtime_session(&state.metadata_db, &params.agent_id, &params.session_id).await?;

    let snapshot =
        crate::models::task_scheduler::snapshot(&state.metadata_db, &params.agent_id).await?;

    Ok(ApiResponse::success_with_raw("Snapshot generated", snapshot).into_response())
}

pub fn runtime_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/callback-probe", get(callback_probe))
        .route("/enroll", post(enroll))
        .route("/register", post(register))
        .route("/heartbeat", post(heartbeat))
        .route("/deregister", post(deregister))
        .route("/terminal-tickets/consume", post(consume_terminal_ticket))
        .route("/rotate-certificate", post(rotate_certificate))
        .route("/scheduled-tasks/runs/report", post(report_task_runs))
        .route("/script-runs/report", post(report_script_runs))
        .route("/operation-events/report", post(report_operation_events))
        .route("/docker/image-tasks", post(create_runtime_image_task))
        .route(
            "/docker/image-tasks/{task_id}",
            get(runtime_image_task_progress).delete(cancel_runtime_image_task),
        )
        .route("/scheduled-tasks/snapshot", get(get_tasks_snapshot))
        .route(
            "/upgrades/artifacts/{version}/{component}/{target_triple}/download",
            get(download_upgrade_artifact),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::node_sessions::{NodeSessionRecord, insert_node_session};

    #[test]
    fn command_access_transport_is_bound_to_node_topology() {
        let credential = "ab".repeat(32);
        assert!(validate_command_access(true, "uds", &credential).is_ok());
        assert!(validate_command_access(false, "https", &credential).is_ok());
        assert!(validate_command_access(true, "https", &credential).is_err());
        assert!(validate_command_access(false, "uds", &credential).is_err());
        assert!(validate_command_access(false, "https", "too-short").is_err());
    }

    #[test]
    fn local_registration_accepts_dual_stack_loopback_only() {
        assert!(is_loopback_peer("127.0.0.1".parse().unwrap()));
        assert!(is_loopback_peer("::1".parse().unwrap()));
        assert!(is_loopback_peer("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_loopback_peer("10.0.0.41".parse().unwrap()));
        assert!(!is_loopback_peer("::ffff:10.0.0.41".parse().unwrap()));
    }

    #[tokio::test]
    async fn runtime_image_identity_is_bound_to_active_session_node() {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query(
            "INSERT INTO nodes (node_id, name, normalized_name, status) \
             VALUES ('node-81', '节点 81', '节点 81', 'online')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO node_identities (\
                identity_id, node_id, agent_id, certificate_fingerprint, \
                certificate_status, public_key_algorithm\
             ) VALUES ('identity-81', 'node-81', 'agent-81', 'fingerprint-81', 'active', 'ed25519')",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_node_session(
            &pool,
            &NodeSessionRecord {
                session_id: "session-81".to_string(),
                node_id: "node-81".to_string(),
                agent_id: "agent-81".to_string(),
                lease_id: "lease-81".to_string(),
                advertise_addr: None,
                listen_addr: None,
                listen_port: None,
                server_name: Some("节点 81".to_string()),
                registered_at: "2026-07-31T00:00:00Z".to_string(),
                lease_expires_at: (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339(),
                last_heartbeat_at: None,
                heartbeat_sequence: 0,
                last_seen_at: None,
                status: "active".to_string(),
                close_reason: None,
                closed_at: None,
                created_at: "2026-07-31T00:00:00Z".to_string(),
                updated_at: "2026-07-31T00:00:00Z".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            require_runtime_node(&pool, "agent-81", "session-81")
                .await
                .unwrap(),
            "node-81"
        );
        assert!(
            require_runtime_node(&pool, "other-agent", "session-81")
                .await
                .is_err()
        );
    }
}
