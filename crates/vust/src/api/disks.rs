//! 节点磁盘语义 API：可信清单、幂等操作协调与可恢复查询。

use crate::{
    api::auth::AuthenticatedAdmin,
    models::{
        logging::{LogModule, LogStatus, PlatformLogLevel},
        node_runtime_client::{AgentOperationContext, NodeRuntimeClient},
    },
    services::{
        logging::{self, OperationEventBuilder},
        node_read_model,
    },
    state::AppState,
    types::{ApiError, ApiResponse, ApiResult, new_uuid_v7},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header::HeaderName},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{SecondsFormat, Utc};
use serde_json::json;
use sqlx::Row;
use std::{net::IpAddr, sync::Arc, time::Duration};
use vust_contracts::{
    api::ErrorCode,
    disks::{
        AgentCreateDiskOperationRequest, CreateDiskOperationRequest, DiskDetail, DiskInventory,
        DiskNodeRef, DiskOperation, DiskOperationCapabilities, DiskOperationKind,
        DiskOperationStatus, DiskOperationTarget,
    },
};

static IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

#[derive(Debug, Clone)]
struct DiskActor {
    user_id: i64,
    name: String,
    client_ip: String,
    trace_id: String,
}

/// 构建节点磁盘公共语义路由。
pub fn disk_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{node_id}/disks", get(inventory))
        .route("/{node_id}/disks/{disk_id}", get(detail))
        .route("/{node_id}/disk-operations", post(create_operation))
        .route("/{node_id}/disk-operations/{operation_id}", get(operation))
        .route(
            "/{node_id}/disk-operations/{operation_id}/cancel",
            post(cancel_operation),
        )
}

/// Master 重启后恢复对未终结磁盘操作的跟踪。
pub async fn recover_operations(state: Arc<AppState>) -> ApiResult<()> {
    let rows = sqlx::query(
        "SELECT operation_id, node_id FROM disk_operations WHERE status IN ('queued','validating','preparing','applying','verifying','canceling')",
    )
    .fetch_all(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    for row in rows {
        tokio::spawn(track_operation(
            Arc::clone(&state),
            row.get("node_id"),
            row.get("operation_id"),
        ));
    }
    Ok(())
}

async fn inventory(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
) -> ApiResult<Response> {
    let node = online_node(&state, &node_id).await?;
    let client = runtime_client(&state, &node_id).await?;
    let mut value: DiskInventory = client.get_domain("/api/v1/agent/disks").await?;
    value.node = DiskNodeRef {
        node_id,
        node_name: node.name.clone(),
    };
    for disk in &mut value.disks {
        disk.erase_confirmation_text = disk
            .capabilities
            .can_erase_and_create_partition
            .then(|| erase_confirmation(&node.name, &disk.device_name, &disk.fingerprint));
    }
    Ok(ApiResponse::success_with_raw("Disk inventory loaded", value).into_response())
}

async fn detail(
    State(state): State<Arc<AppState>>,
    Path((node_id, disk_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let node = online_node(&state, &node_id).await?;
    let client = runtime_client(&state, &node_id).await?;
    let mut value: DiskDetail = client
        .get_domain(&format!("/api/v1/agent/disks/{disk_id}"))
        .await?;
    value.summary.erase_confirmation_text = value
        .summary
        .capabilities
        .can_erase_and_create_partition
        .then(|| {
            erase_confirmation(
                &node.name,
                &value.summary.device_name,
                &value.summary.fingerprint,
            )
        });
    Ok(ApiResponse::success_with_raw("Disk detail loaded", value).into_response())
}

async fn create_operation(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Path(node_id): Path<String>,
    Json(request): Json<CreateDiskOperationRequest>,
) -> ApiResult<Response> {
    let actor = actor(&admin, &headers)?;
    let idempotency_key = idempotency_key(&headers)?;
    let request_json =
        serde_json::to_string(&request).map_err(|error| ApiError::internal(error.to_string()))?;
    if let Some((existing, stored_request)) =
        find_idempotent(&state, actor.user_id, &node_id, &idempotency_key).await?
    {
        if stored_request != request_json {
            return Err(ApiError::conflict(
                ErrorCode::DiskOperationConflict,
                "Idempotency-Key was already used for another request",
            ));
        }
        return Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::success_with_raw(
                "Disk operation already accepted",
                existing,
            )),
        )
            .into_response());
    }

    let node = online_node(&state, &node_id).await?;
    let client = runtime_client(&state, &node_id).await?;
    validate_against_current_facts(&client, &node.name, &request).await?;

    let operation_id = new_uuid_v7();
    let now = now();
    sqlx::query(
        r#"INSERT INTO disk_operations (
               operation_id, node_id, idempotency_key, kind, request_json, status, phase,
               target_json, completed_steps, total_steps, actor_user_id, actor_name,
               client_ip, trace_id, created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, 'queued', 'queued', '{}', 0, 5, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&operation_id)
    .bind(&node_id)
    .bind(&idempotency_key)
    .bind(kind_name(request.kind()))
    .bind(request_json)
    .bind(actor.user_id)
    .bind(&actor.name)
    .bind(&actor.client_ip)
    .bind(&actor.trace_id)
    .bind(&now)
    .bind(&now)
    .execute(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;

    let context = AgentOperationContext {
        actor_user_id: actor.user_id,
        actor_name: actor.name.clone(),
        client_ip: actor.client_ip.clone(),
        trace_id: actor.trace_id.clone(),
    };
    let submitted = client
        .post_domain_with_operation_context::<DiskOperation, _>(
            "/api/v1/agent/disks/operations",
            &AgentCreateDiskOperationRequest {
                operation_id: operation_id.clone(),
                node_id: node_id.clone(),
                request,
            },
            &context,
        )
        .await;
    let operation = match submitted {
        Ok(operation) => {
            store_agent_operation(&state, &operation).await?;
            operation
        }
        Err(error) => {
            mark_submission_failed(&state, &operation_id, &error).await?;
            record_submit(&state, &admin, &actor, &node_id, &operation_id, None, true);
            return Err(error);
        }
    };
    record_submit(
        &state,
        &admin,
        &actor,
        &node_id,
        &operation_id,
        Some(&operation.target),
        false,
    );
    tokio::spawn(track_operation(Arc::clone(&state), node_id, operation_id));
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Disk operation accepted",
            operation,
        )),
    )
        .into_response())
}

async fn operation(
    State(state): State<Arc<AppState>>,
    Path((node_id, operation_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let stored = find_operation(&state, &operation_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                ErrorCode::DiskOperationNotFound,
                "disk operation was not found",
            )
        })?;
    if stored.node_id != node_id {
        return Err(ApiError::not_found(
            ErrorCode::DiskOperationNotFound,
            "disk operation was not found",
        ));
    }
    if stored.status.is_terminal() {
        return Ok(ApiResponse::success_with_raw("Disk operation loaded", stored).into_response());
    }
    let current = refresh_operation(&state, &node_id, &operation_id)
        .await
        .unwrap_or(stored);
    Ok(ApiResponse::success_with_raw("Disk operation loaded", current).into_response())
}

async fn cancel_operation(
    State(state): State<Arc<AppState>>,
    admin: AuthenticatedAdmin,
    headers: HeaderMap,
    Path((node_id, operation_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let actor = actor(&admin, &headers)?;
    let operation = find_operation(&state, &operation_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                ErrorCode::DiskOperationNotFound,
                "disk operation was not found",
            )
        })?;
    if operation.node_id != node_id || !operation.capabilities.can_cancel {
        return Err(ApiError::conflict(
            ErrorCode::DiskOperationNotCancellable,
            "disk operation can no longer be cancelled safely",
        ));
    }
    let client = runtime_client(&state, &node_id).await?;
    let context = AgentOperationContext {
        actor_user_id: actor.user_id,
        actor_name: actor.name,
        client_ip: actor.client_ip,
        trace_id: actor.trace_id,
    };
    let value: DiskOperation = client
        .post_domain_with_operation_context(
            &format!("/api/v1/agent/disks/operations/{operation_id}/cancel"),
            &json!({}),
            &context,
        )
        .await?;
    store_agent_operation(&state, &value).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_with_raw(
            "Disk operation cancellation accepted",
            value,
        )),
    )
        .into_response())
}

async fn validate_against_current_facts(
    client: &NodeRuntimeClient,
    node_name: &str,
    request: &CreateDiskOperationRequest,
) -> ApiResult<()> {
    let (disk_id, expected) = match request {
        CreateDiskOperationRequest::CreatePartition {
            disk_id,
            expected_fingerprint,
            ..
        }
        | CreateDiskOperationRequest::EraseAndCreatePartition {
            disk_id,
            expected_fingerprint,
            ..
        }
        | CreateDiskOperationRequest::FormatPartition {
            disk_id,
            expected_fingerprint,
            ..
        }
        | CreateDiskOperationRequest::AdoptFilesystem {
            disk_id,
            expected_fingerprint,
            ..
        }
        | CreateDiskOperationRequest::AdoptMounted {
            disk_id,
            expected_fingerprint,
            ..
        } => (disk_id, expected_fingerprint),
        _ => return Ok(()),
    };
    let detail: DiskDetail = client
        .get_domain(&format!("/api/v1/agent/disks/{disk_id}"))
        .await?;
    if detail.summary.fingerprint != *expected {
        return Err(ApiError::conflict(
            ErrorCode::DiskChanged,
            "disk fingerprint changed since confirmation",
        ));
    }
    match request {
        CreateDiskOperationRequest::CreatePartition { .. }
            if !detail.summary.capabilities.can_create_partition =>
        {
            Err(ApiError::conflict(
                ErrorCode::DiskNotBlank,
                "disk is not blank or is protected",
            ))
        }
        CreateDiskOperationRequest::EraseAndCreatePartition {
            confirmation_text, ..
        } => {
            if !detail.summary.capabilities.can_erase_and_create_partition {
                return Err(ApiError::conflict(
                    ErrorCode::DiskProtected,
                    "disk is protected from destructive operations",
                ));
            }
            let expected_text = erase_confirmation(
                node_name,
                &detail.summary.device_name,
                &detail.summary.fingerprint,
            );
            if confirmation_text != &expected_text {
                return Err(ApiError::bad_request(
                    ErrorCode::DiskConfirmationInvalid,
                    "erase confirmation text does not match current device facts",
                ));
            }
            Ok(())
        }
        CreateDiskOperationRequest::FormatPartition {
            partition_id,
            confirmation_text,
            ..
        } => {
            let Some(partition) = detail
                .partitions
                .iter()
                .find(|partition| partition.partition_id == *partition_id)
            else {
                return Err(ApiError::not_found(
                    ErrorCode::DiskPartitionNotFound,
                    "partition was not found",
                ));
            };
            if !partition.capabilities.can_format {
                return Err(ApiError::conflict(
                    ErrorCode::DiskProtected,
                    "partition is protected from formatting",
                ));
            }
            if partition.format_confirmation_text.as_deref() != Some(confirmation_text) {
                return Err(ApiError::bad_request(
                    ErrorCode::DiskConfirmationInvalid,
                    "format confirmation text does not match current partition facts",
                ));
            }
            Ok(())
        }
        CreateDiskOperationRequest::AdoptFilesystem { partition_id, .. } => {
            let allowed = detail.partitions.iter().any(|partition| {
                partition.partition_id == *partition_id
                    && partition.capabilities.can_adopt_filesystem
            });
            if allowed {
                Ok(())
            } else {
                Err(ApiError::conflict(
                    ErrorCode::DiskProtected,
                    "partition cannot be adopted safely",
                ))
            }
        }
        CreateDiskOperationRequest::AdoptMounted { partition_id, .. } => {
            let allowed = detail.partitions.iter().any(|partition| {
                partition.partition_id == *partition_id && partition.capabilities.can_adopt_mounted
            });
            if allowed {
                Ok(())
            } else {
                Err(ApiError::conflict(
                    ErrorCode::DiskProtected,
                    "mounted partition cannot be adopted safely",
                ))
            }
        }
        _ => Ok(()),
    }
}

async fn track_operation(state: Arc<AppState>, node_id: String, operation_id: String) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        match refresh_operation(&state, &node_id, &operation_id).await {
            Ok(operation) if operation.status.is_terminal() => {
                record_terminal(&state, &operation).await;
                break;
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%operation_id, ?error, "disk operation refresh failed"),
        }
    }
}

async fn refresh_operation(
    state: &AppState,
    node_id: &str,
    operation_id: &str,
) -> ApiResult<DiskOperation> {
    let client = runtime_client(state, node_id).await?;
    let operation: DiskOperation = client
        .get_domain(&format!("/api/v1/agent/disks/operations/{operation_id}"))
        .await?;
    if operation.node_id != node_id || operation.operation_id != operation_id {
        return Err(ApiError::conflict(
            ErrorCode::DiskChanged,
            "agent returned a disk operation for another node",
        ));
    }
    store_agent_operation(state, &operation).await?;
    Ok(operation)
}

async fn store_agent_operation(state: &AppState, operation: &DiskOperation) -> ApiResult<()> {
    sqlx::query(
        r#"UPDATE disk_operations SET status = ?, phase = ?, target_json = ?,
               completed_steps = ?, total_steps = ?, error_code = ?, error_summary = ?,
               warning_summary = ?, updated_at = ?, finished_at = ?
           WHERE operation_id = ? AND node_id = ?"#,
    )
    .bind(status_name(operation.status))
    .bind(if operation.status.is_terminal() {
        None
    } else {
        operation.phase.as_deref()
    })
    .bind(
        serde_json::to_string(&operation.target)
            .map_err(|error| ApiError::internal(error.to_string()))?,
    )
    .bind(operation.completed_steps as i64)
    .bind(operation.total_steps as i64)
    .bind(&operation.error_code)
    .bind(&operation.error_summary)
    .bind(&operation.warning_summary)
    .bind(&operation.updated_at)
    .bind(&operation.finished_at)
    .bind(&operation.operation_id)
    .bind(&operation.node_id)
    .execute(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

async fn mark_submission_failed(
    state: &AppState,
    operation_id: &str,
    error: &ApiError,
) -> ApiResult<()> {
    let timestamp = now();
    sqlx::query(
        "UPDATE disk_operations SET status = 'failed', phase = NULL, error_code = ?, error_summary = ?, updated_at = ?, finished_at = ? WHERE operation_id = ?",
    )
    .bind(error.code.as_str())
    .bind("disk operation could not be submitted to the node")
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(operation_id)
    .execute(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

async fn find_idempotent(
    state: &AppState,
    user_id: i64,
    node_id: &str,
    key: &str,
) -> ApiResult<Option<(DiskOperation, String)>> {
    let row = sqlx::query(
        "SELECT * FROM disk_operations WHERE actor_user_id = ? AND node_id = ? AND idempotency_key = ?",
    )
    .bind(user_id)
    .bind(node_id)
    .bind(key)
    .fetch_optional(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    row.map(|row| {
        let request_json = row.get("request_json");
        operation_from_row(row).map(|operation| (operation, request_json))
    })
    .transpose()
}

async fn find_operation(state: &AppState, operation_id: &str) -> ApiResult<Option<DiskOperation>> {
    let row = sqlx::query("SELECT * FROM disk_operations WHERE operation_id = ?")
        .bind(operation_id)
        .fetch_optional(&state.metadata_db)
        .await
        .map_err(|error| ApiError::database(error.to_string()))?;
    row.map(operation_from_row).transpose()
}

fn operation_from_row(row: sqlx::sqlite::SqliteRow) -> ApiResult<DiskOperation> {
    let status = parse_status(row.get("status"))?;
    Ok(DiskOperation {
        operation_id: row.get("operation_id"),
        node_id: row.get("node_id"),
        kind: parse_kind(row.get("kind"))?,
        status,
        phase: if status.is_terminal() {
            None
        } else {
            row.get("phase")
        },
        target: serde_json::from_str(row.get("target_json")).unwrap_or(DiskOperationTarget {
            disk_id: None,
            device_name: None,
            fingerprint_suffix: None,
            volume_id: None,
            mount_path: None,
        }),
        completed_steps: row.get::<i64, _>("completed_steps").max(0) as u32,
        total_steps: row.get::<i64, _>("total_steps").max(0) as u32,
        error_code: row.get("error_code"),
        error_summary: row.get("error_summary"),
        warning_summary: row.get("warning_summary"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        finished_at: row.get("finished_at"),
        capabilities: DiskOperationCapabilities {
            can_cancel: status.is_cancellable(),
        },
    })
}

async fn online_node(
    state: &AppState,
    node_id: &str,
) -> ApiResult<crate::services::node_read_model::NodeSummaryView> {
    let node = node_read_model::get_node_summary(&state.metadata_db, node_id)
        .await?
        .ok_or_else(|| ApiError::not_found(ErrorCode::NodeNotFound, "node was not found"))?;
    if node.status != "online" {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::DiskNodeUnavailable,
            "disk node is unavailable",
        ));
    }
    Ok(node)
}

async fn runtime_client(state: &AppState, node_id: &str) -> ApiResult<NodeRuntimeClient> {
    NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::DiskNodeUnavailable,
                "disk node is unavailable",
            )
        })
}

fn actor(admin: &AuthenticatedAdmin, headers: &HeaderMap) -> ApiResult<DiskActor> {
    let client_ip = admin.session.client_ip.clone().ok_or_else(|| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session is missing a trusted client IP",
        )
    })?;
    client_ip.parse::<IpAddr>().map_err(|_| {
        ApiError::forbidden(
            ErrorCode::AuthForbidden,
            "authenticated session has an invalid trusted client IP",
        )
    })?;
    Ok(DiskActor {
        user_id: admin.id,
        name: admin.username.clone(),
        client_ip,
        trace_id: logging::resolve_trace_id(headers),
    })
}

fn idempotency_key(headers: &HeaderMap) -> ApiResult<String> {
    let value = headers
        .get(&IDEMPOTENCY_KEY)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            ErrorCode::BadRequest,
            "a valid Idempotency-Key header is required",
        ));
    }
    Ok(value.to_owned())
}

fn record_submit(
    state: &AppState,
    admin: &AuthenticatedAdmin,
    actor: &DiskActor,
    node_id: &str,
    operation_id: &str,
    target: Option<&vust_contracts::disks::DiskOperationTarget>,
    failed: bool,
) {
    let Ok(ip) = actor.client_ip.parse::<IpAddr>() else {
        return;
    };
    OperationEventBuilder::new(&admin.username, "disk_operation_submitted", ip)
        .user_id(admin.id)
        .module(LogModule::System)
        .target_type("disk_operation")
        .target_id(operation_id)
        .trace_id(&actor.trace_id)
        .request("POST", "/api/v1/node/{nodeId}/disk-operations")
        .status(if failed {
            LogStatus::Failed
        } else {
            LogStatus::Success
        })
        .level(if failed {
            PlatformLogLevel::Error
        } else {
            PlatformLogLevel::Warning
        })
        .metadata(json!({
            "nodeId": node_id,
            "targetName": target.and_then(disk_target_display_name),
            "result": if failed {"failed"} else {"submitted"}
        }))
        .finish(&state.metadata_db);
}

async fn record_terminal(state: &AppState, operation: &DiskOperation) {
    let row = match sqlx::query(
        "SELECT actor_user_id, actor_name, client_ip, trace_id, terminal_logged FROM disk_operations WHERE operation_id = ?",
    )
    .bind(&operation.operation_id)
    .fetch_optional(&state.metadata_db)
    .await
    {
        Ok(Some(row)) if row.get::<i64, _>("terminal_logged") == 0 => row,
        _ => return,
    };
    let client_ip: String = row.get("client_ip");
    let Ok(ip) = client_ip.parse::<IpAddr>() else {
        return;
    };
    let actor_name: String = row.get("actor_name");
    let trace_id: String = row.get("trace_id");
    OperationEventBuilder::new(&actor_name, "disk_operation_finished", ip)
        .user_id(row.get("actor_user_id"))
        .module(LogModule::System)
        .target_type("disk_operation")
        .target_id(&operation.operation_id)
        .trace_id(&trace_id)
        .request("BACKGROUND", "/api/v1/node/{nodeId}/disk-operations")
        .outcome(match operation.status {
            DiskOperationStatus::Partial => vust_contracts::logging::OperationOutcome::Partial,
            DiskOperationStatus::Canceled => vust_contracts::logging::OperationOutcome::Canceled,
            DiskOperationStatus::Failed => vust_contracts::logging::OperationOutcome::Failure,
            _ => vust_contracts::logging::OperationOutcome::Success,
        })
        .level(match operation.status {
            DiskOperationStatus::Failed => PlatformLogLevel::Error,
            DiskOperationStatus::Partial => PlatformLogLevel::Warning,
            DiskOperationStatus::Canceled => PlatformLogLevel::Info,
            _ => PlatformLogLevel::Warning,
        })
        .metadata(json!({
            "nodeId": operation.node_id,
            "targetName": disk_target_display_name(&operation.target),
            "result": status_name(operation.status),
            "errorCode": operation.error_code
        }))
        .finish(&state.metadata_db);
    let _ = sqlx::query("UPDATE disk_operations SET terminal_logged = 1 WHERE operation_id = ?")
        .bind(&operation.operation_id)
        .execute(&state.metadata_db)
        .await;
}

fn disk_target_display_name(target: &vust_contracts::disks::DiskOperationTarget) -> Option<&str> {
    target
        .mount_path
        .as_deref()
        .or(target.device_name.as_deref())
        .or(target.volume_id.as_deref())
        .or(target.disk_id.as_deref())
}

fn erase_confirmation(node_name: &str, device_name: &str, fingerprint: &str) -> String {
    format!(
        "ERASE {node_name} {device_name} {}",
        fingerprint
            .chars()
            .rev()
            .take(8)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    )
}

fn parse_kind(value: &str) -> ApiResult<DiskOperationKind> {
    match value {
        "createPartition" => Ok(DiskOperationKind::CreatePartition),
        "eraseAndCreatePartition" => Ok(DiskOperationKind::EraseAndCreatePartition),
        "formatPartition" => Ok(DiskOperationKind::FormatPartition),
        "adoptFilesystem" => Ok(DiskOperationKind::AdoptFilesystem),
        "adoptMounted" => Ok(DiskOperationKind::AdoptMounted),
        "mountVolume" => Ok(DiskOperationKind::MountVolume),
        "unmountVolume" => Ok(DiskOperationKind::UnmountVolume),
        "changeMountLocation" => Ok(DiskOperationKind::ChangeMountLocation),
        "removeManagedVolume" => Ok(DiskOperationKind::RemoveManagedVolume),
        _ => Err(ApiError::internal("invalid disk operation kind")),
    }
}

fn parse_status(value: &str) -> ApiResult<DiskOperationStatus> {
    match value {
        "queued" => Ok(DiskOperationStatus::Queued),
        "validating" => Ok(DiskOperationStatus::Validating),
        "preparing" => Ok(DiskOperationStatus::Preparing),
        "applying" => Ok(DiskOperationStatus::Applying),
        "verifying" => Ok(DiskOperationStatus::Verifying),
        "canceling" => Ok(DiskOperationStatus::Canceling),
        "succeeded" => Ok(DiskOperationStatus::Succeeded),
        "partial" => Ok(DiskOperationStatus::Partial),
        "failed" => Ok(DiskOperationStatus::Failed),
        "canceled" => Ok(DiskOperationStatus::Canceled),
        _ => Err(ApiError::internal("invalid disk operation status")),
    }
}

const fn kind_name(kind: DiskOperationKind) -> &'static str {
    match kind {
        DiskOperationKind::CreatePartition => "createPartition",
        DiskOperationKind::EraseAndCreatePartition => "eraseAndCreatePartition",
        DiskOperationKind::FormatPartition => "formatPartition",
        DiskOperationKind::AdoptFilesystem => "adoptFilesystem",
        DiskOperationKind::AdoptMounted => "adoptMounted",
        DiskOperationKind::MountVolume => "mountVolume",
        DiskOperationKind::UnmountVolume => "unmountVolume",
        DiskOperationKind::ChangeMountLocation => "changeMountLocation",
        DiskOperationKind::RemoveManagedVolume => "removeManagedVolume",
    }
}

const fn status_name(status: DiskOperationStatus) -> &'static str {
    match status {
        DiskOperationStatus::Queued => "queued",
        DiskOperationStatus::Validating => "validating",
        DiskOperationStatus::Preparing => "preparing",
        DiskOperationStatus::Applying => "applying",
        DiskOperationStatus::Verifying => "verifying",
        DiskOperationStatus::Canceling => "canceling",
        DiskOperationStatus::Succeeded => "succeeded",
        DiskOperationStatus::Partial => "partial",
        DiskOperationStatus::Failed => "failed",
        DiskOperationStatus::Canceled => "canceled",
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::{erase_confirmation, parse_status};
    use vust_contracts::disks::DiskOperationStatus;

    #[test]
    fn erase_confirmation_binds_node_device_and_fingerprint() {
        assert_eq!(
            erase_confirmation("node-a", "sdb", "0123456789abcdef"),
            "ERASE node-a sdb 89abcdef"
        );
    }

    #[test]
    fn terminal_status_is_parsed_without_phase_semantics() {
        let status = parse_status("partial").unwrap();
        assert_eq!(status, DiskOperationStatus::Partial);
        assert!(status.is_terminal());
        assert!(!status.is_cancellable());
    }
}
