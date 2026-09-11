//! Docker 容器 API：容器列表、操作与状态查询。

use crate::api::docker::context::DockerOperationContext;
use crate::models::docker;
use crate::state::AppState;
use crate::types::{ApiError, ApiResponse, ApiResult};

use axum::{
    extract::{Json, Path, Query, State},
    response::{IntoResponse, Response},
};

use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerSummary, ContainerTopResponse,
    HostConfig, Mount, MountType, PortBinding, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{self, LogsOptions};
use chrono::{DateTime, Local};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{error, info, warn};
use vust_contracts::api::ErrorCode;

/// 获取所有 Docker 容器（包括已停止的）的摘要信息列表。
pub async fn list_containers(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    info!("Requesting all docker containers");
    let docker = state.docker_client().await?;

    let options = query_parameters::ListContainersOptionsBuilder::new()
        .all(true)
        .build();
    let containers: Vec<ContainerSummary> = docker.list_containers(Some(options)).await?;
    let mut items = containers
        .iter()
        .filter_map(summary_from_container)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(ApiResponse::success_with_raw("Container list loaded", Some(items)).into_response())
}

/// 获取所有由 `docker-compose` 管理的容器列表。
///
/// 通过标签 `com.docker.compose.project` 进行过滤。
pub async fn list_project_containers(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    info!("Requesting docker compose project containers");
    let docker = state.docker_client().await?;

    let mut filters = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec!["com.docker.compose.project".to_string()],
    );
    let project = docker
        .list_containers(Some(
            bollard::query_parameters::ListContainersOptionsBuilder::new()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await?;
    Ok(
        ApiResponse::success_with_raw("Project container list loaded", Some(project))
            .into_response(),
    )
}

/// 创建容器并按需启动。
pub async fn create_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Json(payload): Json<docker::DockerContainerCreateRequest>,
) -> ApiResult<Response> {
    info!("Requesting container create: {}", payload.name);
    let container_name = payload.name.trim().to_string();
    let image_name = payload.image_ref.trim().to_string();
    let result: ApiResult<Response> = async {
        validate_create_request(&payload)?;
        let client = state.docker_client().await?;
        let env = payload
            .environment
            .iter()
            .map(|item| format!("{}={}", item.name, item.value))
            .collect::<Vec<_>>();
        let (exposed_ports, port_bindings) = create_port_bindings(&payload.ports);
        let mounts = payload
            .mounts
            .iter()
            .map(|mount| Mount {
                target: Some(mount.target.trim().to_string()),
                source: Some(mount.source.trim().to_string()),
                typ: Some(match mount.kind {
                    docker::DockerContainerCreateMountKind::Bind => MountType::BIND,
                    docker::DockerContainerCreateMountKind::Volume => MountType::VOLUME,
                }),
                read_only: Some(mount.read_only),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        let host_config = HostConfig {
            port_bindings: if port_bindings.is_empty() {
                None
            } else {
                Some(port_bindings)
            },
            mounts: if mounts.is_empty() {
                None
            } else {
                Some(mounts)
            },
            restart_policy: Some(RestartPolicy {
                name: Some(restart_policy_name(payload.restart_policy)),
                maximum_retry_count: payload.maximum_retry_count,
            }),
            auto_remove: Some(payload.auto_remove),
            network_mode: payload
                .network_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            ..Default::default()
        };

        let config = ContainerCreateBody {
            image: Some(image_name.clone()),
            env: if env.is_empty() { None } else { Some(env) },
            cmd: if payload.command.is_empty() {
                None
            } else {
                Some(payload.command.clone())
            },
            exposed_ports: if exposed_ports.is_empty() {
                None
            } else {
                Some(exposed_ports)
            },
            host_config: Some(host_config),
            ..Default::default()
        };

        let options = query_parameters::CreateContainerOptionsBuilder::new()
            .name(&container_name)
            .build();
        let created = client.create_container(Some(options), config).await?;

        if payload.auto_start
            && let Err(start_error) = client
                .start_container(&created.id, None::<query_parameters::StartContainerOptions>)
                .await
        {
            if let Err(rollback_error) = client
                .remove_container(
                    &created.id,
                    None::<query_parameters::RemoveContainerOptions>,
                )
                .await
            {
                warn!(
                    container_id = %created.id,
                    error = %rollback_error,
                    "Failed to roll back container after automatic start failure"
                );
            }
            return Err(start_error.into());
        }

        let response = docker::DockerContainerCreateResult {
            id: created.id,
            name: container_name.clone(),
            started: payload.auto_start,
            warnings: created.warnings,
        };
        Ok(ApiResponse::success_with_raw("Container created", Some(response)).into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_container_create",
            Some(("container", &container_name)),
            json!({ "name": container_name, "image": image_name }),
            false,
            result,
        )
        .await
}

/// 获取容器详情（inspect）。
pub async fn inspect_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    info!("Requesting container inspect: {}", id);
    let docker = state.docker_client().await?;
    let detail = docker
        .inspect_container(&id, None::<query_parameters::InspectContainerOptions>)
        .await?;
    let detail = detail_from_inspect(&id, &detail);
    Ok(ApiResponse::success_with_raw("Container detail loaded", Some(detail)).into_response())
}

/// 获取容器进程列表 (docker top)。
pub async fn top_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    info!("Requesting container top: {}", id);
    let docker = state.docker_client().await?;
    let options = query_parameters::TopOptionsBuilder::default()
        .ps_args("aux")
        .build();
    let response = normalize_top_result(docker.top_processes(&id, Some(options)).await)?;
    Ok(
        ApiResponse::success_with_raw("Container process list loaded", Some(response))
            .into_response(),
    )
}

/// 将停止容器的 Docker Top 冲突转换为空进程列表。
fn normalize_top_result(
    result: Result<ContainerTopResponse, bollard::errors::Error>,
) -> Result<ContainerTopResponse, bollard::errors::Error> {
    match result {
        Ok(response) => Ok(response),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 409, ..
        }) => Ok(ContainerTopResponse {
            titles: Some(Vec::new()),
            processes: Some(Vec::new()),
        }),
        Err(error) => Err(error),
    }
}

/// 重命名容器。
pub async fn rename_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
    Json(payload): Json<docker::ContainerRenameRequest>,
) -> ApiResult<Response> {
    info!("Requesting container rename: {} -> {}", id, payload.name);
    let new_name = payload.name.clone();
    let result: ApiResult<Response> = async {
        let docker = state.docker_client().await?;
        let options = query_parameters::RenameContainerOptionsBuilder::new()
            .name(&payload.name)
            .build();
        docker.rename_container(&id, options).await?;
        Ok(ApiResponse::ok("Container renamed").into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_container_rename",
            Some(("container", &id)),
            json!({ "name": id, "newName": new_name }),
            false,
            result,
        )
        .await
}

/// 启动容器。
pub async fn start_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    container_action_response(&state, &context, id, docker::DockerContainerAction::Start).await
}

/// 停止容器。
pub async fn stop_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    container_action_response(&state, &context, id, docker::DockerContainerAction::Stop).await
}

/// 重启容器。
pub async fn restart_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    container_action_response(&state, &context, id, docker::DockerContainerAction::Restart).await
}

/// 暂停容器。
pub async fn pause_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    container_action_response(&state, &context, id, docker::DockerContainerAction::Pause).await
}

/// 恢复容器。
pub async fn unpause_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    container_action_response(&state, &context, id, docker::DockerContainerAction::Unpause).await
}

/// 强制停止容器。
pub async fn kill_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    container_action_response(&state, &context, id, docker::DockerContainerAction::Kill).await
}

/// 删除已停止的容器。
pub async fn remove_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    container_action_response(&state, &context, id, docker::DockerContainerAction::Remove).await
}

/// 批量执行容器生命周期动作并逐项返回结果。
pub async fn batch_container_action(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Json(payload): Json<docker::DockerContainerBatchActionRequest>,
) -> ApiResult<Response> {
    let ids = normalize_batch_ids(payload.ids)?;
    let action = payload.action;
    let executions = stream::iter(ids.into_iter().enumerate().map(|(index, id)| {
        let state = state.clone();
        let context = context.clone();
        async move {
            let execution = execute_container_action(&state, &context, &id, action).await;
            (index, id, execution)
        }
    }))
    .buffer_unordered(5)
    .collect::<Vec<_>>()
    .await;

    let mut items = executions
        .into_iter()
        .map(|(index, id, execution)| {
            let (success, error_code, error_message) = match execution.error {
                Some(error) => (
                    false,
                    Some(error.code.as_str().to_string()),
                    Some(error.message.into_owned()),
                ),
                None => (true, None, None),
            };
            (
                index,
                docker::DockerContainerBatchActionItem {
                    id,
                    name: execution.name,
                    success,
                    state: execution.state,
                    error_code,
                    error_message,
                },
            )
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|(index, _)| *index);
    let result = docker::DockerContainerBatchActionResult {
        items: items.into_iter().map(|(_, item)| item).collect(),
    };

    Ok(
        ApiResponse::success_with_raw("Container batch action completed", Some(result))
            .into_response(),
    )
}

/// 容器 exec 执行命令。
pub async fn exec_container(
    State(state): State<Arc<AppState>>,
    context: DockerOperationContext,
    Path(id): Path<String>,
    Json(payload): Json<docker::ContainerExecRequest>,
) -> ApiResult<Response> {
    info!("Requesting container exec: {}", id);
    let result: ApiResult<Response> = async {
        let docker = state.docker_client().await?;
        let exec = docker
            .create_exec(
                &id,
                CreateExecOptions {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        payload.command,
                    ]),
                    ..Default::default()
                },
            )
            .await?;

        let mut output = String::new();
        if let StartExecResults::Attached {
            output: mut stream, ..
        } = docker.start_exec(&exec.id, None).await?
        {
            while let Some(Ok(log)) = stream.next().await {
                if let Some(line) = format_log_output(log) {
                    output.push_str(&line);
                    output.push('\n');
                }
            }
        }

        let inspect = docker.inspect_exec(&exec.id).await?;
        let result = docker::ContainerExecResult {
            exit_code: inspect.exit_code,
            output,
        };

        Ok(ApiResponse::success_with_raw("Command executed", Some(result)).into_response())
    }
    .await;
    context
        .finish(
            &state.metadata_db,
            "docker_container_exec",
            Some(("container", &id)),
            json!({ "name": id }),
            false,
            result,
        )
        .await
}

/// 查询参数，用于获取容器日志。
#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    /// 获取日志的行数上限，默认 100，最大 500。
    #[serde(default = "default_tail")]
    pub tail: u16,
}

/// `tail` 查询参数的默认值。
fn default_tail() -> u16 {
    100
}

/// 获取指定容器最新的 N 行日志。
///
/// 这是一个 RESTful 端点，用于获取一次性的日志快照，通常在“最新100条”模式下被前端调用。
///
/// # 参数
/// - `state`: 共享的应用状态。
/// - `id`: 从 URL 路径中提取的 Docker 容器 ID。
/// - `query`: URL 查询参数，包含 `tail` 字段，用于指定获取的日志行数。
///
/// # 返回
/// 一个包含日志行数组的 `ApiResponse`。
pub async fn latest_container_logs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<LogsQuery>,
) -> ApiResult<Response> {
    let tail = query.tail.clamp(1, 500); // 限制 tail 的最大值，防止滥用
    let logs = fetch_container_logs(&state, &id, tail).await?;
    Ok(ApiResponse::success_with_raw("Container logs loaded", Some(logs)).into_response())
}

/// 从 Docker 守护进程获取指定容器的日志。
///
/// # 参数
/// - `state`: 应用程序的共享状态。
/// - `container_id`: 目标容器的 ID。
/// - `tail`: 要获取的日志行数。
///
/// # 返回
/// 一个包含格式化后日志行 (`String`) 的 `Vec`。
async fn fetch_container_logs(
    state: &AppState,
    container_id: &str,
    tail: u16,
) -> Result<Vec<String>, ApiError> {
    let docker = state.docker_client().await?;
    let options = LogsOptions {
        stdout: true,
        stderr: true,
        timestamps: true,
        tail: tail.to_string(),
        ..Default::default()
    };

    let log_stream = docker.logs(container_id, Some(options));

    futures_util::pin_mut!(log_stream);
    let mut lines = Vec::new();
    while let Some(item) = log_stream.next().await {
        let log = item.map_err(|error| {
            error!(
                "Failed to fetch container log for {}: {}",
                container_id, error
            );
            ApiError::from(error)
        })?;
        if let Some(line) = format_log_output(log) {
            lines.push(line);
        }
    }

    Ok(lines)
}

/// 格式化 Docker 的 `LogOutput` 为可读的字符串。
///
/// # 处理逻辑
/// 1. 从 `LogOutput` 中提取原始日志字节 (`Bytes`)。
/// 2. 将字节转换为 UTF-8 字符串。
/// 3. 如果日志行以 RFC3339 格式的时间戳开头，则解析该时间戳并将其转换为本地时间，
///    格式化为 `[YYYY/MM/DD HH:MM:SS]`，然后与日志正文拼接。
/// 4. 如果没有时间戳，则直接返回裁剪了前后空白的原始日志行。
///
/// # 参数
/// - `log`: 从 `bollard` 获取的 `LogOutput` 枚举实例。
///
/// # 返回
/// `Option<String>`: 成功格式化则返回 `Some(String)`，如果日志类型不受支持则返回 `None`。
pub(crate) fn format_log_output(log: LogOutput) -> Option<String> {
    let message_bytes = match log {
        LogOutput::StdOut { message } => message,
        LogOutput::StdErr { message } => message,
        _ => return None,
    };

    let line = String::from_utf8_lossy(&message_bytes);

    if let Some(space_index) = line.find(' ') {
        let timestamp_part = &line[..space_index];
        if let Ok(datetime_utc) = timestamp_part.parse::<DateTime<chrono::Utc>>() {
            let local_time: DateTime<Local> = datetime_utc.with_timezone(&Local);
            let message_part = line[space_index + 1..].trim();
            return Some(format!(
                "{} {}",
                local_time.format("[%Y/%m/%d %H:%M:%S]"),
                message_part
            ));
        }
    }
    Some(line.trim().to_string())
}

/// 将 Docker 原始容器摘要转换为稳定领域模型。
fn summary_from_container(container: &ContainerSummary) -> Option<docker::DockerContainerSummary> {
    let id = container.id.clone()?;
    let name = container
        .names
        .as_ref()
        .and_then(|names| names.first())
        .map(|name| name.trim_start_matches('/').to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| id.clone());
    let labels = container.labels.as_ref().cloned().unwrap_or_default();
    let management = classify_management(&labels);
    let state = state_from_value(container.state.as_ref().map(ToString::to_string).as_deref());
    let health = container.health.as_ref().and_then(|health| {
        health
            .status
            .as_ref()
            .map(|status| docker::DockerContainerHealth {
                status: status.to_string(),
                failing_streak: health.failing_streak.unwrap_or_default(),
            })
    });
    let mut ports = container
        .ports
        .as_ref()
        .into_iter()
        .flatten()
        .map(|port| docker::DockerContainerPort {
            host_ip: port.ip.clone().filter(|value| !value.is_empty()),
            host_port: port.public_port,
            container_port: port.private_port,
            protocol: port
                .typ
                .as_ref()
                .map(ToString::to_string)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "tcp".to_string()),
        })
        .collect::<Vec<_>>();
    sort_ports(&mut ports);

    Some(docker::DockerContainerSummary {
        id,
        name,
        image_ref: container.image.clone().unwrap_or_default(),
        image_id: container.image_id.clone().unwrap_or_default(),
        command: container.command.clone().unwrap_or_default(),
        created_at: container.created.unwrap_or_default(),
        state,
        status_text: container.status.clone().unwrap_or_default(),
        health,
        ports,
        capabilities: capabilities_for(state),
        management,
    })
}

/// 将 Docker Inspect 响应转换为稳定领域详情。
fn detail_from_inspect(
    requested_id: &str,
    inspect: &ContainerInspectResponse,
) -> docker::DockerContainerDetail {
    let state = inspect.state.as_ref();
    let config = inspect.config.as_ref();
    let labels = config
        .and_then(|config| config.labels.clone())
        .unwrap_or_default();
    let management = classify_management(&labels);
    let normalized_state = state_from_value(
        state
            .and_then(|state| state.status.as_ref())
            .map(ToString::to_string)
            .as_deref(),
    );
    let health = state
        .and_then(|state| state.health.as_ref())
        .and_then(|health| {
            health
                .status
                .as_ref()
                .map(|status| docker::DockerContainerHealth {
                    status: status.to_string(),
                    failing_streak: health.failing_streak.unwrap_or_default(),
                })
        });
    let id = inspect
        .id
        .clone()
        .unwrap_or_else(|| requested_id.to_string());
    let name = display_container_name(inspect, &id);
    let mut ports = ports_from_inspect(inspect);
    sort_ports(&mut ports);
    let created_at = inspect.created.clone();
    let created_at_epoch = created_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp())
        .unwrap_or_default();
    let summary = docker::DockerContainerSummary {
        id,
        name,
        image_ref: config
            .and_then(|config| config.image.clone())
            .unwrap_or_default(),
        image_id: inspect.image.clone().unwrap_or_default(),
        command: config
            .and_then(|config| config.cmd.as_ref())
            .map(|command| command.join(" "))
            .unwrap_or_default(),
        created_at: created_at_epoch,
        state: normalized_state,
        status_text: state
            .and_then(|state| state.status.as_ref())
            .map(ToString::to_string)
            .unwrap_or_default(),
        health,
        ports,
        capabilities: capabilities_for(normalized_state),
        management,
    };
    let host_config = inspect.host_config.as_ref();
    let restart_policy = host_config.and_then(|config| config.restart_policy.as_ref());
    let mut environment = config
        .and_then(|config| config.env.as_ref())
        .into_iter()
        .flatten()
        .map(|entry| {
            let (name, value) = entry.split_once('=').unwrap_or((entry.as_str(), ""));
            docker::DockerContainerEnvironmentVariable {
                name: name.to_string(),
                value: value.to_string(),
            }
        })
        .collect::<Vec<_>>();
    environment.sort_by(|left, right| left.name.cmp(&right.name));
    let mut mounts = inspect
        .mounts
        .as_ref()
        .into_iter()
        .flatten()
        .map(|mount| docker::DockerContainerMount {
            kind: mount.typ.clone().unwrap_or_default(),
            name: mount.name.clone(),
            source: mount.source.clone().unwrap_or_default(),
            target: mount.destination.clone().unwrap_or_default(),
            read_only: !mount.rw.unwrap_or(true),
            mode: mount.mode.clone().filter(|value| !value.is_empty()),
        })
        .collect::<Vec<_>>();
    mounts.sort_by(|left, right| left.target.cmp(&right.target));
    let mut networks = inspect
        .network_settings
        .as_ref()
        .and_then(|settings| settings.networks.as_ref())
        .into_iter()
        .flat_map(|networks| networks.iter())
        .map(|(name, endpoint)| docker::DockerContainerNetworkEndpoint {
            id: endpoint.network_id.clone().unwrap_or_default(),
            name: name.clone(),
            endpoint_id: endpoint.endpoint_id.clone().unwrap_or_default(),
            mac_address: endpoint.mac_address.clone().unwrap_or_default(),
            ipv4_address: address_with_prefix(
                endpoint.ip_address.as_deref(),
                endpoint.ip_prefix_len,
            ),
            ipv6_address: address_with_prefix(
                endpoint.global_ipv6_address.as_deref(),
                endpoint.global_ipv6_prefix_len,
            ),
            gateway: endpoint.gateway.clone().unwrap_or_default(),
            aliases: endpoint.aliases.clone().unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    networks.sort_by(|left, right| left.name.cmp(&right.name));

    docker::DockerContainerDetail {
        summary,
        created_at,
        started_at: state.and_then(|state| state.started_at.clone()),
        finished_at: state.and_then(|state| state.finished_at.clone()),
        exit_code: state.and_then(|state| state.exit_code),
        error_message: state
            .and_then(|state| state.error.clone())
            .filter(|value| !value.is_empty()),
        oom_killed: state.and_then(|state| state.oom_killed).unwrap_or(false),
        restart_count: inspect.restart_count.unwrap_or_default(),
        restart_policy: docker::DockerContainerRestartPolicy {
            name: restart_policy
                .and_then(|policy| policy.name.as_ref())
                .map(ToString::to_string)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "no".to_string()),
            maximum_retry_count: restart_policy
                .and_then(|policy| policy.maximum_retry_count)
                .unwrap_or_default(),
        },
        entrypoint: config
            .and_then(|config| config.entrypoint.clone())
            .unwrap_or_default(),
        command: config
            .and_then(|config| config.cmd.clone())
            .unwrap_or_default(),
        environment,
        mounts,
        networks,
        labels,
        log_driver: host_config
            .and_then(|config| config.log_config.as_ref())
            .and_then(|config| config.typ.clone())
            .unwrap_or_default(),
    }
}

/// 判断容器由套件、Compose 还是容器模块管理。
pub(crate) fn classify_management(
    labels: &HashMap<String, String>,
) -> docker::DockerContainerManagement {
    let suite_managed = labels.get("vust.owner").map(String::as_str) == Some("suite")
        || labels.contains_key("vust.suite_id")
        || labels.contains_key("vust.workload_id");
    let (kind, owner_name) = if suite_managed {
        let owner = labels
            .get("vust.suite_name")
            .or_else(|| labels.get("vust.suite_id"))
            .or_else(|| labels.get("vust.suite_instance_id"))
            .or_else(|| labels.get("com.docker.compose.project"))
            .cloned();
        (docker::DockerContainerManagementKind::Suite, owner)
    } else if let Some(project) = labels.get("com.docker.compose.project") {
        (
            docker::DockerContainerManagementKind::Compose,
            Some(project.clone()),
        )
    } else {
        (docker::DockerContainerManagementKind::Custom, None)
    };
    docker::DockerContainerManagement { kind, owner_name }
}

/// 根据实时状态计算容器模块允许执行的动作。
fn capabilities_for(state: docker::DockerContainerState) -> docker::DockerContainerCapabilities {
    use docker::DockerContainerState as State;

    docker::DockerContainerCapabilities {
        can_start: matches!(state, State::Created | State::Exited),
        can_stop: matches!(
            state,
            State::Running | State::Paused | State::Restarting | State::Stopping
        ),
        can_restart: matches!(
            state,
            State::Created | State::Running | State::Paused | State::Restarting | State::Exited
        ),
        can_pause: state == State::Running,
        can_unpause: state == State::Paused,
        can_kill: matches!(state, State::Running | State::Restarting),
        can_remove: matches!(state, State::Created | State::Exited | State::Dead),
        can_exec: state == State::Running,
    }
}

/// 一次容器动作的可信目标、状态和错误结果。
struct ContainerActionExecution {
    name: String,
    state: Option<docker::DockerContainerState>,
    error: Option<ApiError>,
}

/// 执行单容器动作并转换为统一 HTTP 响应。
async fn container_action_response(
    state: &Arc<AppState>,
    context: &DockerOperationContext,
    id: String,
    action: docker::DockerContainerAction,
) -> ApiResult<Response> {
    let execution = execute_container_action(state, context, &id, action).await;
    if let Some(error) = execution.error {
        return Err(error);
    }
    Ok(
        ApiResponse::success_with_raw("Container action completed", execution.state)
            .into_response(),
    )
}

/// 使用容器 ID 执行生命周期动作，并统一应用实时状态校验。
async fn execute_container_action(
    state: &Arc<AppState>,
    context: &DockerOperationContext,
    id: &str,
    action: docker::DockerContainerAction,
) -> ContainerActionExecution {
    let mut name = id.to_string();
    let result: ApiResult<Option<docker::DockerContainerState>> = async {
        let client = state.docker_client().await?;
        let inspect = client
            .inspect_container(id, None::<query_parameters::InspectContainerOptions>)
            .await?;
        name = display_container_name(&inspect, id);
        let current_state = state_from_value(
            inspect
                .state
                .as_ref()
                .and_then(|state| state.status.as_ref())
                .map(ToString::to_string)
                .as_deref(),
        );
        let capabilities = capabilities_for(current_state);
        ensure_action_allowed(action, current_state, capabilities, &name)?;

        match action {
            docker::DockerContainerAction::Start => {
                client
                    .start_container(id, None::<query_parameters::StartContainerOptions>)
                    .await?;
            }
            docker::DockerContainerAction::Stop => {
                client
                    .stop_container(id, None::<query_parameters::StopContainerOptions>)
                    .await?;
            }
            docker::DockerContainerAction::Restart => {
                client
                    .restart_container(id, None::<query_parameters::RestartContainerOptions>)
                    .await?;
            }
            docker::DockerContainerAction::Pause => client.pause_container(id).await?,
            docker::DockerContainerAction::Unpause => client.unpause_container(id).await?,
            docker::DockerContainerAction::Kill => {
                client
                    .kill_container(id, None::<query_parameters::KillContainerOptions>)
                    .await?;
            }
            docker::DockerContainerAction::Remove => {
                client
                    .remove_container(id, None::<query_parameters::RemoveContainerOptions>)
                    .await?;
                return Ok(None);
            }
        }

        let next_state = client
            .inspect_container(id, None::<query_parameters::InspectContainerOptions>)
            .await
            .ok()
            .and_then(|inspect| {
                inspect
                    .state
                    .as_ref()
                    .and_then(|state| state.status.as_ref())
                    .map(ToString::to_string)
            })
            .map(|state| state_from_value(Some(&state)));
        Ok(next_state)
    }
    .await;

    let event_code = action_event_code(action);
    let high_impact = matches!(
        action,
        docker::DockerContainerAction::Kill | docker::DockerContainerAction::Remove
    );
    match &result {
        Ok(_) => {
            context
                .record_success(
                    &state.metadata_db,
                    event_code,
                    Some(("container", &name)),
                    json!({ "name": name }),
                    high_impact,
                )
                .await;
        }
        Err(error) => {
            context
                .record_failure(
                    &state.metadata_db,
                    event_code,
                    Some(("container", &name)),
                    json!({ "name": name }),
                    error.to_string(),
                )
                .await;
        }
    }

    ContainerActionExecution {
        name,
        state: result.as_ref().ok().copied().flatten(),
        error: result.err(),
    }
}

/// 校验动作是否与当前容器状态匹配。
fn ensure_action_allowed(
    action: docker::DockerContainerAction,
    state: docker::DockerContainerState,
    capabilities: docker::DockerContainerCapabilities,
    name: &str,
) -> ApiResult<()> {
    let allowed = match action {
        docker::DockerContainerAction::Start => capabilities.can_start,
        docker::DockerContainerAction::Stop => capabilities.can_stop,
        docker::DockerContainerAction::Restart => capabilities.can_restart,
        docker::DockerContainerAction::Pause => capabilities.can_pause,
        docker::DockerContainerAction::Unpause => capabilities.can_unpause,
        docker::DockerContainerAction::Kill => capabilities.can_kill,
        docker::DockerContainerAction::Remove => capabilities.can_remove,
    };
    if allowed {
        return Ok(());
    }
    Err(ApiError::conflict(
        ErrorCode::DockerOperationFailed,
        "container action is not allowed in its current state",
    )
    .with_detail(format!(
        "container={name} state={state:?} action={action:?}"
    )))
}

/// 返回容器动作对应的稳定审计事件代码。
const fn action_event_code(action: docker::DockerContainerAction) -> &'static str {
    match action {
        docker::DockerContainerAction::Start => "docker_container_start",
        docker::DockerContainerAction::Stop => "docker_container_stop",
        docker::DockerContainerAction::Restart => "docker_container_restart",
        docker::DockerContainerAction::Pause => "docker_container_pause",
        docker::DockerContainerAction::Unpause => "docker_container_unpause",
        docker::DockerContainerAction::Kill => "docker_container_kill",
        docker::DockerContainerAction::Remove => "docker_container_remove",
    }
}

/// 清理、去重并限制批量动作的容器 ID。
fn normalize_batch_ids(ids: Vec<String>) -> ApiResult<Vec<String>> {
    const MAX_BATCH_SIZE: usize = 50;
    if ids.is_empty() {
        return Err(ApiError::validation("container ids must not be empty"));
    }
    if ids.len() > MAX_BATCH_SIZE {
        return Err(ApiError::validation(
            "a container batch action accepts at most 50 ids",
        ));
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(ids.len());
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            return Err(ApiError::validation("container id must not be empty"));
        }
        if seen.insert(id.to_string()) {
            normalized.push(id.to_string());
        }
    }
    Ok(normalized)
}

fn display_container_name(inspect: &ContainerInspectResponse, fallback: &str) -> String {
    inspect
        .name
        .as_deref()
        .map(|name| name.trim_start_matches('/').to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn state_from_value(value: Option<&str>) -> docker::DockerContainerState {
    match value.unwrap_or_default() {
        "created" => docker::DockerContainerState::Created,
        "running" => docker::DockerContainerState::Running,
        "paused" => docker::DockerContainerState::Paused,
        "restarting" => docker::DockerContainerState::Restarting,
        "stopping" => docker::DockerContainerState::Stopping,
        "exited" => docker::DockerContainerState::Exited,
        "removing" => docker::DockerContainerState::Removing,
        "dead" => docker::DockerContainerState::Dead,
        _ => docker::DockerContainerState::Unknown,
    }
}

fn ports_from_inspect(inspect: &ContainerInspectResponse) -> Vec<docker::DockerContainerPort> {
    let Some(port_map) = inspect
        .network_settings
        .as_ref()
        .and_then(|settings| settings.ports.as_ref())
    else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    for (container_port, bindings) in port_map {
        let Some((port, protocol)) = container_port.split_once('/') else {
            continue;
        };
        let Ok(container_port) = port.parse::<u16>() else {
            continue;
        };
        match bindings {
            Some(bindings) if !bindings.is_empty() => {
                ports.extend(bindings.iter().map(|binding| {
                    docker::DockerContainerPort {
                        host_ip: binding.host_ip.clone().filter(|value| !value.is_empty()),
                        host_port: binding
                            .host_port
                            .as_deref()
                            .and_then(|value| value.parse::<u16>().ok()),
                        container_port,
                        protocol: protocol.to_string(),
                    }
                }));
            }
            _ => ports.push(docker::DockerContainerPort {
                host_ip: None,
                host_port: None,
                container_port,
                protocol: protocol.to_string(),
            }),
        }
    }
    ports
}

fn sort_ports(ports: &mut [docker::DockerContainerPort]) {
    ports.sort_by(|left, right| {
        left.container_port
            .cmp(&right.container_port)
            .then_with(|| left.protocol.cmp(&right.protocol))
            .then_with(|| left.host_ip.cmp(&right.host_ip))
            .then_with(|| left.host_port.cmp(&right.host_port))
    });
}

fn address_with_prefix(address: Option<&str>, prefix: Option<i64>) -> String {
    let Some(address) = address.filter(|value| !value.is_empty()) else {
        return String::new();
    };
    match prefix.filter(|value| *value > 0) {
        Some(prefix) => format!("{address}/{prefix}"),
        None => address.to_string(),
    }
}

/// 校验结构化容器创建请求，避免将模糊字符串错误交给 Docker 解释。
fn validate_create_request(payload: &docker::DockerContainerCreateRequest) -> ApiResult<()> {
    let name = payload.name.trim();
    if !valid_docker_resource_name(name) {
        return Err(ApiError::validation("invalid container name"));
    }
    if payload.image_ref.trim().is_empty() || payload.image_ref.contains('\0') {
        return Err(ApiError::validation("image reference must not be empty"));
    }
    if payload
        .command
        .iter()
        .any(|argument| argument.contains('\0'))
    {
        return Err(ApiError::validation(
            "container command contains invalid data",
        ));
    }

    let mut environment_names = HashSet::new();
    for variable in &payload.environment {
        if !valid_environment_name(&variable.name) || variable.value.contains('\0') {
            return Err(ApiError::validation(
                "container environment contains an invalid variable",
            ));
        }
        if !environment_names.insert(variable.name.as_str()) {
            return Err(ApiError::validation(
                "container environment contains duplicate names",
            ));
        }
    }

    let mut port_bindings = HashSet::new();
    for port in &payload.ports {
        if port.container_port == 0 || port.host_port == Some(0) {
            return Err(ApiError::validation(
                "container ports must be greater than zero",
            ));
        }
        let protocol = port.protocol.trim().to_ascii_lowercase();
        if !matches!(protocol.as_str(), "tcp" | "udp" | "sctp") {
            return Err(ApiError::validation("unsupported container port protocol"));
        }
        let host_ip = port
            .host_ip
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if host_ip.is_some_and(|value| value.parse::<std::net::IpAddr>().is_err()) {
            return Err(ApiError::validation("invalid container host ip"));
        }
        if !port_bindings.insert((host_ip, port.host_port, port.container_port, protocol)) {
            return Err(ApiError::validation(
                "container ports contain a duplicate mapping",
            ));
        }
    }

    let mut mount_targets = HashSet::new();
    for mount in &payload.mounts {
        let source = mount.source.trim();
        let target = mount.target.trim();
        if source.is_empty()
            || source.contains('\0')
            || !target.starts_with('/')
            || target == "/"
            || target.contains('\0')
        {
            return Err(ApiError::validation("invalid container mount"));
        }
        if mount.kind == docker::DockerContainerCreateMountKind::Bind && !source.starts_with('/') {
            return Err(ApiError::validation(
                "bind mount source must be an absolute path",
            ));
        }
        if mount.kind == docker::DockerContainerCreateMountKind::Volume
            && !valid_docker_resource_name(source)
        {
            return Err(ApiError::validation("invalid Docker volume name"));
        }
        if !mount_targets.insert(target) {
            return Err(ApiError::validation(
                "container mounts contain duplicate targets",
            ));
        }
    }

    if payload.restart_policy != docker::DockerContainerCreateRestartPolicy::OnFailure
        && payload.maximum_retry_count.is_some()
    {
        return Err(ApiError::validation(
            "maximum retry count requires the on_failure restart policy",
        ));
    }
    if payload.maximum_retry_count.is_some_and(|count| count < 0) {
        return Err(ApiError::validation(
            "maximum retry count must not be negative",
        ));
    }
    if payload.auto_remove
        && payload.restart_policy != docker::DockerContainerCreateRestartPolicy::No
    {
        return Err(ApiError::validation(
            "automatic removal cannot be combined with a restart policy",
        ));
    }
    if payload
        .network_id
        .as_deref()
        .is_some_and(|network| network.trim().is_empty() || network.contains('\0'))
    {
        return Err(ApiError::validation("invalid container network"));
    }
    Ok(())
}

/// 将结构化端口映射转换为 Docker API 参数。
fn create_port_bindings(
    ports: &[docker::DockerContainerCreatePort],
) -> (Vec<String>, bollard::models::PortMap) {
    let mut exposed_ports = Vec::new();
    let mut bindings: bollard::models::PortMap = HashMap::new();
    for port in ports {
        let key = format!(
            "{}/{}",
            port.container_port,
            port.protocol.trim().to_ascii_lowercase()
        );
        if !exposed_ports.contains(&key) {
            exposed_ports.push(key.clone());
        }
        bindings
            .entry(key)
            .or_insert_with(|| Some(Vec::new()))
            .as_mut()
            .expect("port binding entries are always initialized")
            .push(PortBinding {
                host_ip: port
                    .host_ip
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                host_port: port.host_port.map(|value| value.to_string()),
            });
    }
    (exposed_ports, bindings)
}

/// 判断名称是否符合 Docker 常用资源命名规则。
fn valid_docker_resource_name(value: &str) -> bool {
    value.len() <= 255
        && value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
}

/// 判断环境变量名称是否合法。
fn valid_environment_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// 将领域重启策略转换为 Docker API 枚举。
const fn restart_policy_name(
    policy: docker::DockerContainerCreateRestartPolicy,
) -> RestartPolicyNameEnum {
    match policy {
        docker::DockerContainerCreateRestartPolicy::No => RestartPolicyNameEnum::NO,
        docker::DockerContainerCreateRestartPolicy::Always => RestartPolicyNameEnum::ALWAYS,
        docker::DockerContainerCreateRestartPolicy::UnlessStopped => {
            RestartPolicyNameEnum::UNLESS_STOPPED
        }
        docker::DockerContainerCreateRestartPolicy::OnFailure => RestartPolicyNameEnum::ON_FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        capabilities_for, classify_management, detail_from_inspect, ensure_action_allowed,
        normalize_batch_ids, normalize_top_result, state_from_value, summary_from_container,
        validate_create_request,
    };
    use crate::models::docker::{
        DockerContainerAction as Action, DockerContainerCreateMount,
        DockerContainerCreateMountKind, DockerContainerCreatePort, DockerContainerCreateRequest,
        DockerContainerCreateRestartPolicy, DockerContainerEnvironmentVariable,
        DockerContainerManagementKind, DockerContainerState as State,
    };
    use bollard::models::{
        ContainerConfig, ContainerInspectResponse, ContainerState, ContainerSummary,
        ContainerSummaryStateEnum,
    };
    use std::collections::HashMap;
    use vust_contracts::api::ErrorCode;

    fn labels(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn create_request() -> DockerContainerCreateRequest {
        DockerContainerCreateRequest {
            name: "example".to_string(),
            image_ref: "nginx:latest".to_string(),
            command: Vec::new(),
            environment: Vec::new(),
            ports: Vec::new(),
            mounts: Vec::new(),
            restart_policy: DockerContainerCreateRestartPolicy::No,
            maximum_retry_count: None,
            network_id: None,
            auto_remove: false,
            auto_start: true,
        }
    }

    #[test]
    fn suite_management_takes_priority_over_compose() {
        let management = classify_management(&labels(&[
            ("vust.suite_id", "suite-1"),
            ("com.docker.compose.project", "project-1"),
        ]));
        assert_eq!(management.kind, DockerContainerManagementKind::Suite);
        assert_eq!(management.owner_name.as_deref(), Some("suite-1"));

        let compose = classify_management(&labels(&[("com.docker.compose.project", "project-1")]));
        assert_eq!(compose.kind, DockerContainerManagementKind::Compose);
        assert_eq!(compose.owner_name.as_deref(), Some("project-1"));

        let custom = classify_management(&HashMap::new());
        assert_eq!(custom.kind, DockerContainerManagementKind::Custom);
    }

    #[test]
    fn capabilities_follow_container_state_for_every_management_kind() {
        let running = capabilities_for(State::Running);
        assert!(running.can_stop);
        assert!(running.can_restart);
        assert!(running.can_pause);
        assert!(running.can_kill);
        assert!(running.can_exec);
        assert!(!running.can_start);
        assert!(!running.can_remove);

        let paused = capabilities_for(State::Paused);
        assert!(paused.can_stop);
        assert!(paused.can_restart);
        assert!(paused.can_unpause);
        assert!(!paused.can_pause);
        assert!(!paused.can_exec);

        let exited = capabilities_for(State::Exited);
        assert!(exited.can_start);
        assert!(exited.can_restart);
        assert!(exited.can_remove);
    }

    #[test]
    fn normalizes_summary_without_exposing_labels() {
        let source = ContainerSummary {
            id: Some("container-id".to_string()),
            names: Some(vec!["/example".to_string()]),
            image: Some("nginx:latest".to_string()),
            image_id: Some("sha256:image".to_string()),
            command: Some("nginx -g daemon off;".to_string()),
            created: Some(42),
            labels: Some(labels(&[("com.docker.compose.project", "project")])),
            state: Some(ContainerSummaryStateEnum::RUNNING),
            status: Some("Up 10 seconds".to_string()),
            ..Default::default()
        };
        let summary = summary_from_container(&source).expect("summary");
        assert_eq!(summary.id, "container-id");
        assert_eq!(summary.name, "example");
        assert_eq!(summary.state, State::Running);
        assert_eq!(
            summary.management.kind,
            DockerContainerManagementKind::Compose
        );
        assert!(summary.capabilities.can_exec);
        assert!(summary.capabilities.can_restart);
    }

    #[test]
    fn preserves_environment_values_after_first_equals_sign() {
        let inspect = ContainerInspectResponse {
            id: Some("container-id".to_string()),
            name: Some("/example".to_string()),
            created: Some("2026-07-15T08:00:00Z".to_string()),
            state: Some(ContainerState {
                status: Some(bollard::models::ContainerStateStatusEnum::RUNNING),
                ..Default::default()
            }),
            config: Some(ContainerConfig {
                image: Some("example:latest".to_string()),
                env: Some(vec!["TOKEN=part=part".to_string(), "EMPTY=".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let detail = detail_from_inspect("container-id", &inspect);
        assert_eq!(detail.summary.created_at, 1_784_102_400);
        assert_eq!(detail.environment[0].name, "EMPTY");
        assert_eq!(detail.environment[0].value, "");
        assert_eq!(detail.environment[1].name, "TOKEN");
        assert_eq!(detail.environment[1].value, "part=part");
    }

    #[test]
    fn maps_unknown_and_stopping_states_explicitly() {
        assert_eq!(state_from_value(Some("stopping")), State::Stopping);
        assert_eq!(state_from_value(Some("future-state")), State::Unknown);
        assert_eq!(state_from_value(None), State::Unknown);
    }

    #[test]
    fn maps_stopped_container_top_conflict_to_empty_processes() {
        let response =
            normalize_top_result(Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409,
                message: "container is not running".to_string(),
            }))
            .expect("stopped containers have no processes");
        assert_eq!(response.titles, Some(Vec::new()));
        assert_eq!(response.processes, Some(Vec::new()));

        let error = normalize_top_result(Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 500,
            message: "daemon failure".to_string(),
        }))
        .expect_err("other Docker errors must remain visible");
        assert!(matches!(
            error,
            bollard::errors::Error::DockerResponseServerError {
                status_code: 500,
                ..
            }
        ));
    }

    #[test]
    fn rejects_actions_that_do_not_match_container_state() {
        let running = capabilities_for(State::Running);
        ensure_action_allowed(Action::Stop, State::Running, running, "example")
            .expect("running container can stop");
        let error = ensure_action_allowed(Action::Remove, State::Running, running, "example")
            .expect_err("running container cannot be removed");
        assert_eq!(error.code, ErrorCode::DockerOperationFailed);
    }

    #[test]
    fn normalizes_batch_ids_and_enforces_limits() {
        let ids = normalize_batch_ids(vec![
            " first ".to_string(),
            "second".to_string(),
            "first".to_string(),
        ])
        .expect("valid ids");
        assert_eq!(ids, vec!["first", "second"]);
        assert!(normalize_batch_ids(Vec::new()).is_err());
        assert!(normalize_batch_ids(vec!["id".to_string(); 51]).is_err());
        assert!(normalize_batch_ids(vec![" ".to_string()]).is_err());
    }

    #[test]
    fn validates_structured_container_create_fields() {
        let mut request = create_request();
        request.environment = vec![DockerContainerEnvironmentVariable {
            name: "TOKEN".to_string(),
            value: "part=part".to_string(),
        }];
        request.ports = vec![DockerContainerCreatePort {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(8080),
            container_port: 80,
            protocol: "tcp".to_string(),
        }];
        request.mounts = vec![DockerContainerCreateMount {
            kind: DockerContainerCreateMountKind::Volume,
            source: "web_data".to_string(),
            target: "/usr/share/nginx/html".to_string(),
            read_only: true,
        }];
        validate_create_request(&request).expect("valid request");

        request
            .environment
            .push(DockerContainerEnvironmentVariable {
                name: "TOKEN".to_string(),
                value: "duplicate".to_string(),
            });
        assert!(validate_create_request(&request).is_err());
    }

    #[test]
    fn rejects_incompatible_restart_and_mount_options() {
        let mut request = create_request();
        request.auto_remove = true;
        request.restart_policy = DockerContainerCreateRestartPolicy::Always;
        assert!(validate_create_request(&request).is_err());

        request.auto_remove = false;
        request.restart_policy = DockerContainerCreateRestartPolicy::No;
        request.mounts = vec![DockerContainerCreateMount {
            kind: DockerContainerCreateMountKind::Bind,
            source: "relative/path".to_string(),
            target: "/data".to_string(),
            read_only: false,
        }];
        assert!(validate_create_request(&request).is_err());
    }
}
