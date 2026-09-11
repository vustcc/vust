//! 磁盘后台操作：持久化状态机、设备互斥与原子持久挂载。

use crate::{
    services::disk_inventory,
    state::AppState,
    types::{ApiError, ApiResult},
};
use axum::http::StatusCode;
use chrono::{SecondsFormat, Utc};
use sqlx::Row;
use std::{
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{fs, process::Command, time::sleep};
use uuid::Uuid;
use vust_contracts::{
    api::ErrorCode,
    disks::{
        AgentCreateDiskOperationRequest, CreateDiskOperationRequest, DiskOperation,
        DiskOperationCapabilities, DiskOperationKind, DiskOperationStatus, DiskOperationTarget,
    },
};

const FSTAB_PATH: &str = "/etc/fstab";
const MOUNT_ROOT: &str = "/mnt";

#[derive(Debug)]
struct DomainFailure {
    code: ErrorCode,
    summary: String,
    partial: bool,
}

impl DomainFailure {
    fn new(code: ErrorCode, summary: impl Into<String>) -> Self {
        Self {
            code,
            summary: summary.into(),
            partial: false,
        }
    }

    fn partial(code: ErrorCode, summary: impl Into<String>) -> Self {
        Self {
            code,
            summary: summary.into(),
            partial: true,
        }
    }

    /// 标记设备改写开始后的失败，禁止将已产生副作用的操作误报为普通失败。
    fn after_device_change(mut self) -> Self {
        self.partial = true;
        self
    }
}

/// 幂等保存操作，并在后台继续执行。
pub async fn submit(
    state: Arc<AppState>,
    request: AgentCreateDiskOperationRequest,
) -> ApiResult<DiskOperation> {
    validate_request(&request.request)?;
    let agent_node_id = sqlx::query_scalar::<_, Option<String>>(
        "SELECT agent_id FROM agent_identity ORDER BY id LIMIT 1",
    )
    .fetch_optional(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?
    .flatten()
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| "local".to_string());
    if request.node_id != agent_node_id {
        return Err(ApiError::conflict(
            ErrorCode::DiskChanged,
            "disk operation targets another node",
        ));
    }
    if let Some(existing) = find_operation(&state, &request.operation_id).await? {
        let row =
            sqlx::query("SELECT node_id, request_json FROM disk_operations WHERE operation_id = ?")
                .bind(&request.operation_id)
                .fetch_one(&state.metadata_db)
                .await
                .map_err(|error| ApiError::database(error.to_string()))?;
        let submitted = serde_json::to_string(&request.request)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        if row.get::<String, _>("node_id") != request.node_id
            || row.get::<String, _>("request_json") != submitted
        {
            return Err(ApiError::conflict(
                ErrorCode::DiskOperationConflict,
                "operationId was already used for another request",
            ));
        }
        return Ok(existing);
    }

    let lock_key = target_lock_key(&request.request);
    let mount_lock_key = request_mount_name(&request.request);
    let now = now();
    let request_json = serde_json::to_string(&request.request)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let kind = kind_name(request.request.kind());
    let result = sqlx::query(
        r#"INSERT INTO disk_operations (
               operation_id, node_id, kind, request_json, target_lock_key, mount_lock_key, status, phase,
               target_json, completed_steps, total_steps, created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, ?, 'queued', 'queued', '{}', 0, 5, ?, ?)"#,
    )
    .bind(&request.operation_id)
    .bind(&request.node_id)
    .bind(kind)
    .bind(request_json)
    .bind(lock_key)
    .bind(mount_lock_key)
    .bind(&now)
    .bind(&now)
    .execute(&state.metadata_db)
    .await;

    if let Err(error) = result {
        let message = error.to_string();
        if message.contains("idx_disk_operations_active_mount") {
            return Err(ApiError::conflict(
                ErrorCode::DiskMountConflict,
                "another disk operation is already using this mount name",
            ));
        }
        if message.contains("idx_disk_operations_active_target") {
            return Err(ApiError::conflict(
                ErrorCode::DiskOperationConflict,
                "another disk operation is already active for this target",
            ));
        }
        return Err(ApiError::database(message));
    }

    let operation = find_operation(&state, &request.operation_id)
        .await?
        .ok_or_else(|| ApiError::internal("created operation is missing"))?;
    tokio::spawn(run(Arc::clone(&state), request.operation_id));
    Ok(operation)
}

/// 查询 Agent 持久化的操作状态。
pub async fn get(state: &AppState, operation_id: &str) -> ApiResult<DiskOperation> {
    find_operation(state, operation_id).await?.ok_or_else(|| {
        ApiError::not_found(
            ErrorCode::DiskOperationNotFound,
            "disk operation was not found",
        )
    })
}

/// 请求在破坏性应用阶段之前取消操作。
pub async fn cancel(state: &AppState, operation_id: &str) -> ApiResult<DiskOperation> {
    let operation = get(state, operation_id).await?;
    if !operation.status.is_cancellable() {
        return Err(ApiError::conflict(
            ErrorCode::DiskOperationNotCancellable,
            "disk operation can no longer be cancelled safely",
        ));
    }
    let result = sqlx::query(
        "UPDATE disk_operations SET cancel_requested = 1, status = 'canceling', phase = 'canceling', updated_at = ? WHERE operation_id = ? AND status IN ('queued','validating','preparing')",
    )
    .bind(now())
    .bind(operation_id)
    .execute(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    if result.rows_affected() == 0 {
        return Err(ApiError::conflict(
            ErrorCode::DiskOperationNotCancellable,
            "disk operation can no longer be cancelled safely",
        ));
    }
    get(state, operation_id).await
}

/// Agent 重启后恢复安全阶段；已进入设备改写阶段的操作转为 partial 等待人工确认。
pub async fn recover(state: &Arc<AppState>) -> ApiResult<()> {
    recover_managed_volumes(state).await?;
    let rows = sqlx::query(
        "SELECT operation_id, status FROM disk_operations WHERE status IN ('queued','validating','preparing','applying','verifying','canceling')",
    )
    .fetch_all(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    for row in rows {
        let operation_id: String = row.get("operation_id");
        let status: String = row.get("status");
        if matches!(status.as_str(), "queued" | "validating" | "preparing") {
            tokio::spawn(run(Arc::clone(state), operation_id));
        } else if status == "canceling" {
            finish(
                state,
                &operation_id,
                DiskOperationStatus::Canceled,
                None,
                None,
                None,
            )
            .await?;
        } else {
            finish(
                state,
                &operation_id,
                DiskOperationStatus::Partial,
                Some(ErrorCode::DiskOperationFailed),
                Some(
                    "Agent restarted after disk changes began; device state requires verification",
                ),
                Some("The operation was not replayed automatically"),
            )
            .await?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ManagedFstabBlock {
    volume_id: String,
    filesystem_uuid: String,
    mount_path: String,
}

/// 从仍然存在的 VUST fstab 标记块恢复因数据库重建丢失的受管卷记录。
async fn recover_managed_volumes(state: &AppState) -> ApiResult<()> {
    let fstab = read_fstab().await.map_err(|failure| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            failure.code,
            failure.summary,
        )
    })?;
    let blocks = managed_fstab_blocks(&fstab);
    if blocks.is_empty() {
        return Ok(());
    }
    let inventory = disk_inventory::inventory(&state.metadata_db).await?;
    for block in blocks {
        let Some(mount_name) = mount_name_from_path(&block.mount_path) else {
            tracing::warn!(mount_path = %block.mount_path, "ignored managed fstab block outside mount root");
            continue;
        };
        let mut recovered = None;
        for disk in &inventory.disks {
            let detail = disk_inventory::detail(&state.metadata_db, &disk.disk_id).await?;
            if let Some(partition) = detail.partitions.iter().find(|partition| {
                partition.filesystem.uuid.as_deref() == Some(block.filesystem_uuid.as_str())
            }) {
                recovered = Some((
                    disk.disk_id.clone(),
                    disk.fingerprint.clone(),
                    partition.partition_id.clone(),
                    partition
                        .mounts
                        .iter()
                        .any(|path| path == &block.mount_path),
                ));
                break;
            }
        }
        let Some((disk_id, fingerprint, partition_id, mounted)) = recovered else {
            tracing::warn!(volume_id = %block.volume_id, "managed fstab volume is not present in disk inventory");
            continue;
        };
        let timestamp = now();
        sqlx::query(
            r#"INSERT OR IGNORE INTO managed_disk_volumes (
                   volume_id, disk_id, disk_fingerprint, partition_id, filesystem_uuid,
                   mount_name, mount_path, desired_state, runtime_state,
                   created_at, updated_at
               ) VALUES (?, ?, ?, ?, ?, ?, ?, 'mounted', ?, ?, ?)"#,
        )
        .bind(&block.volume_id)
        .bind(disk_id)
        .bind(fingerprint)
        .bind(partition_id)
        .bind(&block.filesystem_uuid)
        .bind(mount_name)
        .bind(&block.mount_path)
        .bind(if mounted { "mounted" } else { "unmounted" })
        .bind(&timestamp)
        .bind(&timestamp)
        .execute(&state.metadata_db)
        .await
        .map_err(|error| ApiError::database(error.to_string()))?;
    }
    Ok(())
}

fn managed_fstab_blocks(content: &str) -> Vec<ManagedFstabBlock> {
    const PREFIX: &str = "# BEGIN VUST MANAGED VOLUME ";
    let lines = content.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some(volume_id) = lines[index].strip_prefix(PREFIX) else {
            index += 1;
            continue;
        };
        let end = end_marker(volume_id);
        let mut entry = None;
        let mut found_end = false;
        index += 1;
        while index < lines.len() {
            if lines[index] == end {
                found_end = true;
                break;
            }
            let line = lines[index].trim();
            if !line.is_empty() && !line.starts_with('#') {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                if let (Some(uuid), Some(path)) = (
                    fields.first().and_then(|value| value.strip_prefix("UUID=")),
                    fields.get(1),
                ) {
                    entry = Some(ManagedFstabBlock {
                        volume_id: volume_id.to_owned(),
                        filesystem_uuid: uuid.to_owned(),
                        mount_path: (*path).to_owned(),
                    });
                }
            }
            index += 1;
        }
        if found_end && let Some(entry) = entry {
            blocks.push(entry);
        }
        index += 1;
    }
    blocks
}

async fn run(state: Arc<AppState>, operation_id: String) {
    let result = execute(&state, &operation_id).await;
    if let Err(failure) = result {
        let status = if failure.partial {
            DiskOperationStatus::Partial
        } else {
            DiskOperationStatus::Failed
        };
        if let Err(error) = finish(
            &state,
            &operation_id,
            status,
            Some(failure.code),
            Some(&failure.summary),
            failure
                .partial
                .then_some("Review the current device and fstab state"),
        )
        .await
        {
            tracing::error!(%operation_id, ?error, "failed to persist disk operation failure");
        }
    }
}

async fn execute(state: &AppState, operation_id: &str) -> Result<(), DomainFailure> {
    transition(
        state,
        operation_id,
        DiskOperationStatus::Validating,
        "validating",
        0,
    )
    .await?;
    if cancel_requested(state, operation_id).await? {
        return finish_cancelled(state, operation_id).await;
    }
    let row = sqlx::query("SELECT request_json FROM disk_operations WHERE operation_id = ?")
        .bind(operation_id)
        .fetch_one(&state.metadata_db)
        .await
        .map_err(db_failure)?;
    let request: CreateDiskOperationRequest = serde_json::from_str(row.get("request_json"))
        .map_err(|_| {
            DomainFailure::new(
                ErrorCode::DiskOperationFailed,
                "stored operation is invalid",
            )
        })?;
    if let Some(mount_name) = request_mount_name(&request) {
        ensure_mount_name_available(state, mount_name, request_volume_id(&request)).await?;
    }

    transition(
        state,
        operation_id,
        DiskOperationStatus::Preparing,
        "preparing",
        1,
    )
    .await?;
    if cancel_requested(state, operation_id).await? {
        return finish_cancelled(state, operation_id).await;
    }

    match request {
        CreateDiskOperationRequest::CreatePartition {
            disk_id,
            expected_fingerprint,
        } => {
            create_partition_layout(state, operation_id, &disk_id, &expected_fingerprint, false)
                .await?;
        }
        CreateDiskOperationRequest::EraseAndCreatePartition {
            disk_id,
            expected_fingerprint,
            ..
        } => {
            create_partition_layout(state, operation_id, &disk_id, &expected_fingerprint, true)
                .await?;
        }
        CreateDiskOperationRequest::FormatPartition {
            disk_id,
            partition_id,
            expected_fingerprint,
            confirmation_text,
        } => {
            format_partition(
                state,
                operation_id,
                &disk_id,
                &partition_id,
                &expected_fingerprint,
                &confirmation_text,
            )
            .await?;
        }
        CreateDiskOperationRequest::AdoptFilesystem {
            disk_id,
            partition_id,
            expected_fingerprint,
        } => {
            adopt_filesystem(
                state,
                operation_id,
                &disk_id,
                &partition_id,
                &expected_fingerprint,
            )
            .await?;
        }
        CreateDiskOperationRequest::AdoptMounted {
            disk_id,
            partition_id,
            expected_fingerprint,
        } => {
            adopt_mounted(
                state,
                operation_id,
                &disk_id,
                &partition_id,
                &expected_fingerprint,
            )
            .await?;
        }
        CreateDiskOperationRequest::MountVolume {
            volume_id,
            mount_name,
        } => {
            mount_volume(state, operation_id, &volume_id, &mount_name).await?;
        }
        CreateDiskOperationRequest::UnmountVolume { volume_id } => {
            unmount_volume(state, operation_id, &volume_id).await?;
        }
        CreateDiskOperationRequest::ChangeMountLocation {
            volume_id,
            mount_name,
        } => {
            change_mount_location(state, operation_id, &volume_id, &mount_name).await?;
        }
        CreateDiskOperationRequest::RemoveManagedVolume { volume_id } => {
            remove_managed_volume(state, operation_id, &volume_id).await?;
        }
    }

    transition(
        state,
        operation_id,
        DiskOperationStatus::Verifying,
        "verifying",
        4,
    )
    .await?;
    finish(
        state,
        operation_id,
        DiskOperationStatus::Succeeded,
        None,
        None,
        None,
    )
    .await
    .map_err(api_failure)?;
    remove_fstab_backup(operation_id).await;
    Ok(())
}

async fn create_partition_layout(
    state: &AppState,
    operation_id: &str,
    disk_id: &str,
    expected_fingerprint: &str,
    erase: bool,
) -> Result<(), DomainFailure> {
    let detail = checked_disk(state, disk_id, expected_fingerprint).await?;
    let allowed = if erase {
        detail.summary.capabilities.can_erase_and_create_partition
    } else {
        detail.summary.capabilities.can_create_partition
    };
    if !allowed {
        return Err(DomainFailure::new(
            if erase {
                ErrorCode::DiskProtected
            } else {
                ErrorCode::DiskNotBlank
            },
            "current device facts do not permit this operation",
        ));
    }
    save_target(
        state,
        operation_id,
        DiskOperationTarget {
            disk_id: Some(disk_id.to_owned()),
            device_name: Some(detail.summary.device_name.clone()),
            fingerprint_suffix: Some(fingerprint_suffix(expected_fingerprint)),
            volume_id: None,
            mount_path: None,
        },
    )
    .await?;

    if erase {
        prepare_disk_for_erase(state, operation_id, &detail).await?;
    }

    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "partitioning",
        2,
    )
    .await?;
    async {
        let device = format!("/dev/{}", detail.summary.device_name);
        if erase {
            run_command("wipefs", &["--all", &device]).await?;
            verify_fingerprint(state, disk_id, expected_fingerprint).await?;
        }
        run_command("parted", &["--script", &device, "mklabel", "gpt"]).await?;
        run_command(
            "parted",
            &[
                "--script", &device, "mkpart", "primary", "ext4", "1MiB", "100%",
            ],
        )
        .await?;
        run_command("udevadm", &["settle", "--timeout=15"]).await?;
        wait_for_single_partition(state, disk_id, expected_fingerprint).await?;
        Ok(())
    }
    .await
    .map_err(DomainFailure::after_device_change)
}

async fn prepare_disk_for_erase(
    state: &AppState,
    operation_id: &str,
    detail: &vust_contracts::disks::DiskDetail,
) -> Result<(), DomainFailure> {
    transition(
        state,
        operation_id,
        DiskOperationStatus::Preparing,
        "removingReferences",
        2,
    )
    .await?;
    let original = read_fstab().await?;
    let mut rendered = original.clone();
    let managed_rows =
        sqlx::query("SELECT volume_id, mount_path FROM managed_disk_volumes WHERE disk_id = ?")
            .bind(&detail.summary.disk_id)
            .fetch_all(&state.metadata_db)
            .await
            .map_err(db_failure)?;
    for row in &managed_rows {
        let volume_id: String = row.get("volume_id");
        rendered = render_remove(&rendered, &volume_id)?;
    }
    for partition in &detail.partitions {
        if let Some(uuid) = partition.filesystem.uuid.as_deref() {
            let device = format!("/dev/{}", partition.device_name);
            let mount = partition.mounts.first().map(String::as_str).unwrap_or("");
            rendered = render_remove_external_volume_entry(&rendered, &device, uuid, mount);
        }
    }
    if rendered != original {
        commit_fstab_with_backup(operation_id, &original, &rendered).await?;
    }
    let mut unmounted = 0usize;
    for partition in &detail.partitions {
        for mount in &partition.mounts {
            if let Err(error) = run_command("umount", &[mount]).await {
                if unmounted > 0
                    || (rendered != original
                        && commit_fstab_if_revision(&rendered, &original)
                            .await
                            .is_err())
                {
                    return Err(DomainFailure::partial(
                        ErrorCode::DiskInUse,
                        "some disk references were removed but another mount could not be released",
                    ));
                }
                return Err(error);
            }
            unmounted += 1;
            cleanup_mount_directory(state, operation_id, mount).await?;
        }
    }
    for row in &managed_rows {
        if let Some(path) = row.get::<Option<String>, _>("mount_path") {
            cleanup_mount_directory(state, operation_id, &path).await?;
        }
    }
    sqlx::query("DELETE FROM managed_disk_volumes WHERE disk_id = ?")
        .bind(&detail.summary.disk_id)
        .execute(&state.metadata_db)
        .await
        .map_err(db_failure)
        .map_err(DomainFailure::after_device_change)?;
    Ok(())
}

async fn format_partition(
    state: &AppState,
    operation_id: &str,
    disk_id: &str,
    partition_id: &str,
    expected_fingerprint: &str,
    confirmation_text: &str,
) -> Result<(), DomainFailure> {
    let detail = checked_disk(state, disk_id, expected_fingerprint).await?;
    let partition = detail
        .partitions
        .iter()
        .find(|partition| partition.partition_id == partition_id)
        .ok_or_else(|| {
            DomainFailure::new(ErrorCode::DiskPartitionNotFound, "partition was not found")
        })?;
    if !partition.capabilities.can_format {
        return Err(DomainFailure::new(
            ErrorCode::DiskProtected,
            "partition cannot be formatted safely",
        ));
    }
    if partition.format_confirmation_text.as_deref() != Some(confirmation_text) {
        return Err(DomainFailure::new(
            ErrorCode::DiskConfirmationInvalid,
            "format confirmation text does not match current partition facts",
        ));
    }
    save_target(
        state,
        operation_id,
        DiskOperationTarget {
            disk_id: Some(disk_id.to_owned()),
            device_name: Some(partition.device_name.clone()),
            fingerprint_suffix: Some(fingerprint_suffix(expected_fingerprint)),
            volume_id: partition
                .managed_volume
                .as_ref()
                .map(|volume| volume.volume_id.clone()),
            mount_path: partition
                .managed_volume
                .as_ref()
                .and_then(|volume| volume.mount_path.clone()),
        },
    )
    .await?;
    prepare_partition_for_format(state, operation_id, &detail, partition).await?;
    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "formattingPartition",
        3,
    )
    .await?;
    let partition_device = format!("/dev/{}", partition.device_name);
    async {
        run_command("wipefs", &["--all", &partition_device]).await?;
        run_command("udevadm", &["settle", "--timeout=15"]).await?;
        run_command("mkfs.ext4", &["-q", &partition_device]).await?;
        let filesystem_uuid = blkid_value(&partition_device, "UUID").await?;
        register_unmounted_volume(
            state,
            disk_id,
            expected_fingerprint,
            partition_id,
            &filesystem_uuid,
        )
        .await
    }
    .await
    .map_err(DomainFailure::after_device_change)
}

async fn prepare_partition_for_format(
    state: &AppState,
    operation_id: &str,
    detail: &vust_contracts::disks::DiskDetail,
    partition: &vust_contracts::disks::DiskPartition,
) -> Result<(), DomainFailure> {
    transition(
        state,
        operation_id,
        DiskOperationStatus::Preparing,
        "removingPartitionReferences",
        2,
    )
    .await?;
    let original = read_fstab().await?;
    let mut rendered = original.clone();
    if let Some(volume) = &partition.managed_volume {
        rendered = render_remove(&rendered, &volume.volume_id)?;
    }
    if let Some(uuid) = partition.filesystem.uuid.as_deref() {
        let device = format!("/dev/{}", partition.device_name);
        let mount = partition.mounts.first().map(String::as_str).unwrap_or("");
        rendered = render_remove_external_volume_entry(&rendered, &device, uuid, mount);
    }
    if rendered != original {
        commit_fstab_with_backup(operation_id, &original, &rendered).await?;
    }
    for mount in &partition.mounts {
        if let Err(error) = run_command("umount", &[mount]).await {
            if rendered != original {
                let _ = commit_fstab_if_revision(&rendered, &original).await;
            }
            return Err(error);
        }
        cleanup_mount_directory(state, operation_id, mount).await?;
    }
    sqlx::query("DELETE FROM managed_disk_volumes WHERE disk_id = ? AND partition_id = ?")
        .bind(&detail.summary.disk_id)
        .bind(&partition.partition_id)
        .execute(&state.metadata_db)
        .await
        .map_err(db_failure)
        .map_err(DomainFailure::after_device_change)?;
    Ok(())
}

async fn adopt_filesystem(
    state: &AppState,
    operation_id: &str,
    disk_id: &str,
    partition_id: &str,
    expected_fingerprint: &str,
) -> Result<(), DomainFailure> {
    let detail = checked_disk(state, disk_id, expected_fingerprint).await?;
    let partition = detail
        .partitions
        .iter()
        .find(|partition| partition.partition_id == partition_id)
        .ok_or_else(|| {
            DomainFailure::new(ErrorCode::DiskPartitionNotFound, "partition was not found")
        })?;
    if !partition.capabilities.can_adopt_filesystem
        || partition.filesystem.filesystem_type.as_deref() != Some("ext4")
    {
        return Err(DomainFailure::new(
            ErrorCode::DiskProtected,
            "partition cannot be adopted safely",
        ));
    }
    let uuid = partition.filesystem.uuid.as_deref().ok_or_else(|| {
        DomainFailure::new(
            ErrorCode::DiskIdentityUnavailable,
            "filesystem UUID is unavailable",
        )
    })?;
    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "adoptingFilesystem",
        3,
    )
    .await?;
    save_target(
        state,
        operation_id,
        DiskOperationTarget {
            disk_id: Some(disk_id.to_owned()),
            device_name: Some(partition.device_name.clone()),
            fingerprint_suffix: Some(fingerprint_suffix(expected_fingerprint)),
            volume_id: None,
            mount_path: None,
        },
    )
    .await?;
    register_unmounted_volume(state, disk_id, expected_fingerprint, partition_id, uuid).await
}

async fn adopt_mounted(
    state: &AppState,
    operation_id: &str,
    disk_id: &str,
    partition_id: &str,
    expected_fingerprint: &str,
) -> Result<(), DomainFailure> {
    let detail = checked_disk(state, disk_id, expected_fingerprint).await?;
    let partition = detail
        .partitions
        .iter()
        .find(|partition| partition.partition_id == partition_id)
        .ok_or_else(|| {
            DomainFailure::new(ErrorCode::DiskPartitionNotFound, "partition was not found")
        })?;
    if !partition.capabilities.can_adopt_mounted
        || partition.filesystem.filesystem_type.as_deref() != Some("ext4")
        || partition.mounts.len() != 1
    {
        return Err(DomainFailure::new(
            ErrorCode::DiskProtected,
            "mounted partition cannot be adopted safely",
        ));
    }
    let filesystem_uuid = partition.filesystem.uuid.as_deref().ok_or_else(|| {
        DomainFailure::new(
            ErrorCode::DiskIdentityUnavailable,
            "filesystem UUID is unavailable",
        )
    })?;
    let mount_path = &partition.mounts[0];
    let mount_name = mount_name_from_path(mount_path).ok_or_else(|| {
        DomainFailure::new(
            ErrorCode::DiskInvalidMountName,
            "mounted partition is outside the managed mount root",
        )
    })?;
    let volume_id = Uuid::now_v7().to_string();
    save_target(
        state,
        operation_id,
        DiskOperationTarget {
            disk_id: Some(disk_id.to_owned()),
            device_name: Some(partition.device_name.clone()),
            fingerprint_suffix: Some(fingerprint_suffix(expected_fingerprint)),
            volume_id: Some(volume_id.clone()),
            mount_path: Some(mount_path.clone()),
        },
    )
    .await?;
    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "adoptingMountedVolume",
        3,
    )
    .await?;
    let original = read_fstab().await?;
    let partition_device = format!("/dev/{}", partition.device_name);
    let without_external = render_remove_external_volume_entry(
        &original,
        &partition_device,
        filesystem_uuid,
        mount_path,
    );
    let rendered = render_add(&without_external, &volume_id, filesystem_uuid, mount_path)?;
    commit_fstab_with_backup(operation_id, &original, &rendered).await?;
    let timestamp = now();
    let insert = sqlx::query(
        r#"INSERT INTO managed_disk_volumes (
               volume_id, disk_id, disk_fingerprint, partition_id, filesystem_uuid,
               mount_name, mount_path, desired_state, runtime_state,
               created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, 'mounted', 'mounted', ?, ?)"#,
    )
    .bind(&volume_id)
    .bind(disk_id)
    .bind(expected_fingerprint)
    .bind(partition_id)
    .bind(filesystem_uuid)
    .bind(mount_name)
    .bind(mount_path)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&state.metadata_db)
    .await;
    if let Err(error) = insert {
        if commit_fstab_if_revision(&rendered, &original)
            .await
            .is_err()
        {
            return Err(DomainFailure::partial(
                ErrorCode::DiskFstabConflict,
                "mounted volume registration failed and fstab rollback could not be verified",
            ));
        }
        return Err(db_failure(error));
    }
    Ok(())
}

/// 本次操作准备的挂载目录，用于只清理本次新建的空目录。
struct PreparedMountPoint {
    path: PathBuf,
    created: bool,
}

impl PreparedMountPoint {
    async fn cleanup(&self) {
        if self.created {
            let _ = fs::remove_dir(&self.path).await;
        }
    }
}

async fn prepare_mount_point(path: &str) -> Result<PreparedMountPoint, DomainFailure> {
    let path = PathBuf::from(path);
    let created = mount_point_creation_needed(path.to_string_lossy().as_ref()).await?;
    if created {
        fs::create_dir(&path).await.map_err(io_failure)?;
    }
    Ok(PreparedMountPoint { path, created })
}

async fn mount_point_creation_needed(path: &str) -> Result<bool, DomainFailure> {
    let path = PathBuf::from(path);
    match fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.file_type().is_dir() => {
            let mut entries = fs::read_dir(&path).await.map_err(io_failure)?;
            if entries.next_entry().await.map_err(io_failure)?.is_some() {
                return Err(DomainFailure::new(
                    ErrorCode::DiskMountConflict,
                    "mount path exists and is not empty",
                ));
            }
            Ok(false)
        }
        Ok(_) => Err(DomainFailure::new(
            ErrorCode::DiskMountConflict,
            "mount path exists but is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(io_failure(error)),
    }
}

async fn register_unmounted_volume(
    state: &AppState,
    disk_id: &str,
    fingerprint: &str,
    partition_id: &str,
    filesystem_uuid: &str,
) -> Result<(), DomainFailure> {
    let volume_id = Uuid::now_v7().to_string();
    let timestamp = now();
    sqlx::query(
        r#"INSERT INTO managed_disk_volumes (
               volume_id, disk_id, disk_fingerprint, partition_id, filesystem_uuid,
               mount_name, mount_path, desired_state, runtime_state,
               created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, NULL, NULL, 'unmounted', 'unmounted', ?, ?)"#,
    )
    .bind(&volume_id)
    .bind(disk_id)
    .bind(fingerprint)
    .bind(partition_id)
    .bind(filesystem_uuid)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&state.metadata_db)
    .await
    .map_err(db_failure)
    .map_err(DomainFailure::after_device_change)?;
    Ok(())
}

async fn mount_volume(
    state: &AppState,
    operation_id: &str,
    volume_id: &str,
    mount_name: &str,
) -> Result<(), DomainFailure> {
    let volume = load_volume(state, volume_id).await?;
    if volume.desired_state != "unmounted" || volume.runtime_state != "unmounted" {
        return Err(DomainFailure::new(
            ErrorCode::DiskOperationConflict,
            "volume is not in an unmounted state",
        ));
    }
    let path = mount_path(mount_name);
    save_target(
        state,
        operation_id,
        DiskOperationTarget {
            disk_id: None,
            device_name: None,
            fingerprint_suffix: None,
            volume_id: Some(volume.volume_id.clone()),
            mount_path: Some(path.clone()),
        },
    )
    .await?;
    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "mountingVolume",
        3,
    )
    .await?;
    let original = read_fstab().await?;
    let rendered = render_add(&original, volume_id, &volume.filesystem_uuid, &path)?;
    let mount_point = prepare_mount_point(&path).await?;
    if let Err(error) = commit_fstab_with_backup(operation_id, &original, &rendered).await {
        mount_point.cleanup().await;
        return Err(error);
    }
    if let Err(error) = run_command("mount", &[&path]).await {
        if commit_fstab_if_revision(&rendered, &original)
            .await
            .is_err()
        {
            return Err(DomainFailure::partial(
                ErrorCode::DiskFstabConflict,
                "mount failed and fstab rollback could not be verified",
            ));
        }
        remove_fstab_backup(operation_id).await;
        mount_point.cleanup().await;
        return Err(error);
    }
    update_volume_location(state, volume_id, Some(mount_name), Some(&path))
        .await
        .map_err(DomainFailure::after_device_change)?;
    update_volume_state(state, volume_id, "mounted", "mounted")
        .await
        .map_err(DomainFailure::after_device_change)
}

async fn unmount_volume(
    state: &AppState,
    operation_id: &str,
    volume_id: &str,
) -> Result<(), DomainFailure> {
    let volume = load_volume(state, volume_id).await?;
    if volume.desired_state != "mounted" || volume.runtime_state != "mounted" {
        return Err(DomainFailure::new(
            ErrorCode::DiskOperationConflict,
            "volume is not in a mounted state",
        ));
    }
    let mount_path = volume.mount_path.as_deref().ok_or_else(|| {
        DomainFailure::new(
            ErrorCode::DiskOperationConflict,
            "mounted volume has no assigned mount path",
        )
    })?;
    save_volume_target(state, operation_id, &volume).await?;
    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "unmountingVolume",
        3,
    )
    .await?;
    let original = read_fstab().await?;
    let rendered = render_remove(&original, volume_id)?;
    commit_fstab_with_backup(operation_id, &original, &rendered).await?;
    if let Err(error) = run_command("umount", &[mount_path]).await {
        if commit_fstab_if_revision(&rendered, &original)
            .await
            .is_err()
        {
            return Err(DomainFailure::partial(
                ErrorCode::DiskFstabConflict,
                "unmount failed and fstab rollback could not be verified",
            ));
        }
        remove_fstab_backup(operation_id).await;
        return Err(error);
    }
    update_volume_location(state, volume_id, None, None)
        .await
        .map_err(DomainFailure::after_device_change)?;
    update_volume_state(state, volume_id, "unmounted", "unmounted")
        .await
        .map_err(DomainFailure::after_device_change)?;
    cleanup_mount_directory(state, operation_id, mount_path).await
}

async fn change_mount_location(
    state: &AppState,
    operation_id: &str,
    volume_id: &str,
    mount_name: &str,
) -> Result<(), DomainFailure> {
    let volume = load_volume(state, volume_id).await?;
    if volume
        .mount_name
        .as_deref()
        .is_some_and(|current| current.eq_ignore_ascii_case(mount_name))
    {
        return Err(DomainFailure::new(
            ErrorCode::DiskMountConflict,
            "new mount name matches the current mount name",
        ));
    }
    let new_path = mount_path(mount_name);
    save_target(
        state,
        operation_id,
        DiskOperationTarget {
            disk_id: None,
            device_name: None,
            fingerprint_suffix: None,
            volume_id: Some(volume.volume_id.clone()),
            mount_path: Some(new_path.clone()),
        },
    )
    .await?;
    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "changingMountLocation",
        3,
    )
    .await?;

    if volume.desired_state != "mounted" || volume.runtime_state != "mounted" {
        return Err(DomainFailure::new(
            ErrorCode::DiskOperationConflict,
            "only a mounted volume can change mount location",
        ));
    }
    let old_path = volume.mount_path.as_deref().ok_or_else(|| {
        DomainFailure::new(
            ErrorCode::DiskOperationConflict,
            "mounted volume has no assigned mount path",
        )
    })?;

    let original = read_fstab().await?;
    let rendered = render_add(&original, volume_id, &volume.filesystem_uuid, &new_path)?;
    let new_mount_point = prepare_mount_point(&new_path).await?;
    if let Err(error) = commit_fstab_with_backup(operation_id, &original, &rendered).await {
        new_mount_point.cleanup().await;
        return Err(error);
    }
    if let Err(error) = run_command("umount", &[old_path]).await {
        let rollback = commit_fstab_if_revision(&rendered, &original).await;
        new_mount_point.cleanup().await;
        if rollback.is_err() {
            return Err(DomainFailure::partial(
                ErrorCode::DiskFstabConflict,
                "old mount could not be unmounted and fstab rollback failed",
            ));
        }
        return Err(error);
    }
    if let Err(error) = run_command("mount", &[&new_path]).await {
        if commit_fstab_if_revision(&rendered, &original)
            .await
            .is_err()
            || run_command("mount", &[old_path]).await.is_err()
        {
            return Err(DomainFailure::partial(
                ErrorCode::DiskOperationFailed,
                "new mount failed and the original mount could not be fully restored",
            ));
        }
        new_mount_point.cleanup().await;
        return Err(error);
    }
    update_volume_location(state, volume_id, Some(mount_name), Some(&new_path))
        .await
        .map_err(DomainFailure::after_device_change)?;
    cleanup_mount_directory(state, operation_id, old_path).await
}

async fn remove_managed_volume(
    state: &AppState,
    operation_id: &str,
    volume_id: &str,
) -> Result<(), DomainFailure> {
    let volume = load_volume(state, volume_id).await?;
    if volume.desired_state != "unmounted" || volume.runtime_state != "unmounted" {
        return Err(DomainFailure::new(
            ErrorCode::DiskInUse,
            "volume must be unmounted before it can be removed from management",
        ));
    }
    save_volume_target(state, operation_id, &volume).await?;
    transition(
        state,
        operation_id,
        DiskOperationStatus::Applying,
        "removingManagedVolume",
        3,
    )
    .await?;
    let original = read_fstab().await?;
    let rendered = render_remove(&original, volume_id)?;
    if rendered != original {
        commit_fstab_with_backup(operation_id, &original, &rendered).await?;
    }
    if let Some(path) = volume.mount_path.as_deref() {
        cleanup_mount_directory(state, operation_id, path).await?;
    }
    sqlx::query("DELETE FROM managed_disk_volumes WHERE volume_id = ?")
        .bind(volume_id)
        .execute(&state.metadata_db)
        .await
        .map_err(db_failure)
        .map_err(DomainFailure::after_device_change)?;
    Ok(())
}

#[derive(Debug)]
struct VolumeRow {
    volume_id: String,
    filesystem_uuid: String,
    mount_name: Option<String>,
    mount_path: Option<String>,
    desired_state: String,
    runtime_state: String,
}

async fn load_volume(state: &AppState, volume_id: &str) -> Result<VolumeRow, DomainFailure> {
    let row = sqlx::query(
        "SELECT volume_id, filesystem_uuid, mount_name, mount_path, desired_state, runtime_state FROM managed_disk_volumes WHERE volume_id = ?",
    )
    .bind(volume_id)
    .fetch_optional(&state.metadata_db)
    .await
    .map_err(db_failure)?
    .ok_or_else(|| {
        DomainFailure::new(
            ErrorCode::DiskManagedVolumeNotFound,
            "managed volume was not found",
        )
    })?;
    Ok(VolumeRow {
        volume_id: row.get("volume_id"),
        filesystem_uuid: row.get("filesystem_uuid"),
        mount_name: row.get("mount_name"),
        mount_path: row.get("mount_path"),
        desired_state: row.get("desired_state"),
        runtime_state: row.get("runtime_state"),
    })
}

async fn update_volume_location(
    state: &AppState,
    volume_id: &str,
    mount_name: Option<&str>,
    mount_path: Option<&str>,
) -> Result<(), DomainFailure> {
    sqlx::query(
        "UPDATE managed_disk_volumes SET mount_name = ?, mount_path = ?, updated_at = ? WHERE volume_id = ?",
    )
    .bind(mount_name)
    .bind(mount_path)
    .bind(now())
    .bind(volume_id)
    .execute(&state.metadata_db)
    .await
    .map_err(db_failure)?;
    Ok(())
}

async fn cleanup_mount_directory(
    state: &AppState,
    operation_id: &str,
    path: &str,
) -> Result<(), DomainFailure> {
    if mount_name_from_path(path).is_none() {
        return append_warning(
            state,
            operation_id,
            "old mount path is outside the managed mount root and was preserved",
        )
        .await;
    }
    if let Some(warning) = remove_empty_directory(Path::new(path)).await? {
        append_warning(state, operation_id, &warning).await?;
    }
    Ok(())
}

async fn remove_empty_directory(path: &Path) -> Result<Option<String>, DomainFailure> {
    match fs::remove_dir(path).await {
        Ok(()) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(Some(
            "old mount directory is not empty and was preserved".to_owned(),
        )),
        Err(error) => Err(DomainFailure::partial(
            ErrorCode::DiskOperationFailed,
            format!("VUST-managed mount directory could not be removed: {error}"),
        )),
    }
}

async fn append_warning(
    state: &AppState,
    operation_id: &str,
    warning: &str,
) -> Result<(), DomainFailure> {
    sqlx::query(
        "UPDATE disk_operations SET warning_summary = CASE WHEN warning_summary IS NULL OR warning_summary = '' THEN ? ELSE warning_summary || '; ' || ? END, updated_at = ? WHERE operation_id = ?",
    )
    .bind(warning)
    .bind(warning)
    .bind(now())
    .bind(operation_id)
    .execute(&state.metadata_db)
    .await
    .map_err(db_failure)?;
    Ok(())
}

async fn save_volume_target(
    state: &AppState,
    operation_id: &str,
    volume: &VolumeRow,
) -> Result<(), DomainFailure> {
    save_target(
        state,
        operation_id,
        DiskOperationTarget {
            disk_id: None,
            device_name: None,
            fingerprint_suffix: None,
            volume_id: Some(volume.volume_id.clone()),
            mount_path: volume.mount_path.clone(),
        },
    )
    .await
}

async fn update_volume_state(
    state: &AppState,
    volume_id: &str,
    desired: &str,
    runtime: &str,
) -> Result<(), DomainFailure> {
    sqlx::query(
        "UPDATE managed_disk_volumes SET desired_state = ?, runtime_state = ?, updated_at = ? WHERE volume_id = ?",
    )
    .bind(desired)
    .bind(runtime)
    .bind(now())
    .bind(volume_id)
    .execute(&state.metadata_db)
    .await
    .map_err(db_failure)?;
    Ok(())
}

async fn checked_disk(
    state: &AppState,
    disk_id: &str,
    fingerprint: &str,
) -> Result<vust_contracts::disks::DiskDetail, DomainFailure> {
    let detail = disk_inventory::detail(&state.metadata_db, disk_id)
        .await
        .map_err(api_failure)?;
    if detail.summary.fingerprint != fingerprint {
        return Err(DomainFailure::new(
            ErrorCode::DiskChanged,
            "disk fingerprint changed since confirmation",
        ));
    }
    Ok(detail)
}

async fn verify_fingerprint(
    state: &AppState,
    disk_id: &str,
    fingerprint: &str,
) -> Result<(), DomainFailure> {
    checked_disk(state, disk_id, fingerprint).await.map(|_| ())
}

async fn wait_for_single_partition(
    state: &AppState,
    disk_id: &str,
    fingerprint: &str,
) -> Result<vust_contracts::disks::DiskPartition, DomainFailure> {
    for _ in 0..30 {
        let detail = checked_disk(state, disk_id, fingerprint).await?;
        if detail.partitions.len() == 1 {
            return detail.partitions.into_iter().next().ok_or_else(|| {
                DomainFailure::new(ErrorCode::DiskOperationFailed, "partition did not appear")
            });
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(DomainFailure::new(
        ErrorCode::DiskOperationFailed,
        "new partition was not observed before timeout",
    ))
}

async fn blkid_value(device: &str, field: &str) -> Result<String, DomainFailure> {
    let output = Command::new("blkid")
        .args(["-s", field, "-o", "value", device])
        .output()
        .await
        .map_err(io_failure)?;
    if !output.status.success() {
        return Err(DomainFailure::new(
            ErrorCode::DiskOperationFailed,
            "filesystem identity could not be verified",
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return Err(DomainFailure::new(
            ErrorCode::DiskIdentityUnavailable,
            "filesystem UUID is unavailable",
        ));
    }
    Ok(value)
}

async fn run_command(program: &str, args: &[&str]) -> Result<(), DomainFailure> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(io_failure)?;
    if status.success() {
        Ok(())
    } else {
        Err(DomainFailure::new(
            ErrorCode::DiskOperationFailed,
            format!("{program} did not complete successfully"),
        ))
    }
}

async fn read_fstab() -> Result<String, DomainFailure> {
    fs::read_to_string(fstab_path()).await.map_err(io_failure)
}

fn begin_marker(volume_id: &str) -> String {
    format!("# BEGIN VUST MANAGED VOLUME {volume_id}")
}

fn end_marker(volume_id: &str) -> String {
    format!("# END VUST MANAGED VOLUME {volume_id}")
}

fn render_add(
    original: &str,
    volume_id: &str,
    filesystem_uuid: &str,
    mount_path: &str,
) -> Result<String, DomainFailure> {
    let without_existing = render_remove(original, volume_id)?;
    for line in without_existing.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fields = trimmed.split_whitespace().collect::<Vec<_>>();
        if fields.first() == Some(&format!("UUID={filesystem_uuid}").as_str())
            || fields.get(1) == Some(&mount_path)
        {
            return Err(DomainFailure::new(
                ErrorCode::DiskFstabConflict,
                "an external fstab entry already uses this UUID or mount path",
            ));
        }
    }
    let mut output = without_existing.trim_end_matches('\n').to_owned();
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(&begin_marker(volume_id));
    output.push('\n');
    output.push_str(&format!(
        "UUID={filesystem_uuid} {mount_path} ext4 defaults,nofail 0 2\n"
    ));
    output.push_str(&end_marker(volume_id));
    output.push('\n');
    Ok(output)
}

fn render_remove(original: &str, volume_id: &str) -> Result<String, DomainFailure> {
    let begin = begin_marker(volume_id);
    let end = end_marker(volume_id);
    let mut inside = false;
    let mut found_begin = false;
    let mut found_end = false;
    let mut output = String::new();
    for line in original.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == begin {
            if inside {
                return Err(DomainFailure::new(
                    ErrorCode::DiskFstabConflict,
                    "managed fstab markers are malformed",
                ));
            }
            inside = true;
            found_begin = true;
            continue;
        }
        if trimmed == end {
            if !inside {
                return Err(DomainFailure::new(
                    ErrorCode::DiskFstabConflict,
                    "managed fstab markers are malformed",
                ));
            }
            inside = false;
            found_end = true;
            continue;
        }
        if !inside {
            output.push_str(line);
        }
    }
    if inside || found_begin != found_end {
        return Err(DomainFailure::new(
            ErrorCode::DiskFstabConflict,
            "managed fstab markers are incomplete",
        ));
    }
    Ok(output)
}

fn render_remove_external_volume_entry(
    original: &str,
    partition_device: &str,
    filesystem_uuid: &str,
    mount_path: &str,
) -> String {
    let uuid_spec = format!("UUID={filesystem_uuid}");
    original
        .split_inclusive('\n')
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return true;
            }
            let fields = trimmed.split_whitespace().collect::<Vec<_>>();
            fields.first().is_none_or(|spec| {
                *spec != partition_device
                    && *spec != uuid_spec
                    && fields.get(1).is_none_or(|path| *path != mount_path)
            })
        })
        .collect()
}

async fn commit_fstab(content: &str) -> Result<(), DomainFailure> {
    let path = fstab_path();
    commit_fstab_at(&path, content).await
}

async fn commit_fstab_at(path: &Path, content: &str) -> Result<(), DomainFailure> {
    let parent = path.parent().ok_or_else(|| {
        DomainFailure::new(ErrorCode::DiskFstabConflict, "fstab path has no parent")
    })?;
    let metadata = fs::metadata(path).await.map_err(io_failure)?;
    let temporary = parent.join(format!(".vust-fstab-{}", Uuid::now_v7()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temporary).map_err(io_failure)?;
    use std::io::Write;
    file.write_all(content.as_bytes()).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    std::fs::set_permissions(
        &temporary,
        std::fs::Permissions::from_mode(metadata.permissions().mode()),
    )
    .map_err(io_failure)?;
    let ownership = Command::new("chown")
        .arg(format!("--reference={}", path.display()))
        .arg(&temporary)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    let ownership = match ownership {
        Ok(status) => status,
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            return Err(io_failure(error));
        }
    };
    if !ownership.success() {
        let _ = fs::remove_file(&temporary).await;
        return Err(DomainFailure::new(
            ErrorCode::DiskFstabConflict,
            "fstab ownership could not be preserved",
        ));
    }

    let verify = Command::new("findmnt")
        .args(["--verify", "--tab-file"])
        .arg(&temporary)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    let verify = match verify {
        Ok(status) => status,
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            return Err(io_failure(error));
        }
    };
    if !verify.success() {
        let _ = fs::remove_file(&temporary).await;
        return Err(DomainFailure::new(
            ErrorCode::DiskFstabConflict,
            "generated fstab did not pass validation",
        ));
    }
    let directory = std::fs::File::open(parent).map_err(io_failure)?;
    fs::rename(&temporary, path).await.map_err(io_failure)?;
    directory.sync_all().map_err(|error| {
        DomainFailure::partial(
            ErrorCode::DiskFstabConflict,
            format!("fstab was replaced but directory sync failed: {error}"),
        )
    })?;
    Ok(())
}

async fn commit_fstab_with_backup(
    operation_id: &str,
    original: &str,
    replacement: &str,
) -> Result<(), DomainFailure> {
    if read_fstab().await? != original {
        return Err(DomainFailure::new(
            ErrorCode::DiskFstabConflict,
            "fstab changed while the operation was being prepared",
        ));
    }
    let backup = fstab_backup_path(operation_id)?;
    let backup_existed = backup.exists();
    if !backup_existed {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(&backup).map_err(io_failure)?;
        use std::io::Write;
        file.write_all(original.as_bytes()).map_err(io_failure)?;
        file.sync_all().map_err(io_failure)?;
    }
    if let Err(error) = commit_fstab(replacement).await {
        if !backup_existed && !error.partial {
            let _ = fs::remove_file(&backup).await;
        }
        return Err(error);
    }
    Ok(())
}

async fn commit_fstab_if_revision(expected: &str, replacement: &str) -> Result<(), DomainFailure> {
    if read_fstab().await? != expected {
        return Err(DomainFailure::new(
            ErrorCode::DiskFstabConflict,
            "fstab changed before rollback",
        ));
    }
    commit_fstab(replacement).await
}

async fn remove_fstab_backup(operation_id: &str) {
    if let Ok(path) = fstab_backup_path(operation_id) {
        let _ = fs::remove_file(path).await;
    }
}

fn fstab_backup_path(operation_id: &str) -> Result<PathBuf, DomainFailure> {
    let path = fstab_path();
    let parent = path.parent().ok_or_else(|| {
        DomainFailure::new(ErrorCode::DiskFstabConflict, "fstab path has no parent")
    })?;
    Ok(parent.join(format!(".vust-fstab-recovery-{operation_id}")))
}

fn fstab_path() -> PathBuf {
    std::env::var_os("VUST_FSTAB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(FSTAB_PATH))
}

fn mount_path(name: &str) -> String {
    let root = std::env::var("VUST_MOUNT_ROOT").unwrap_or_else(|_| MOUNT_ROOT.to_owned());
    format!("{}/{name}", root.trim_end_matches('/'))
}

fn mount_name_from_path(path: &str) -> Option<String> {
    let root = std::env::var("VUST_MOUNT_ROOT").unwrap_or_else(|_| MOUNT_ROOT.to_owned());
    let relative = Path::new(path)
        .strip_prefix(root.trim_end_matches('/'))
        .ok()?;
    let mut components = relative.components();
    let name = components.next()?.as_os_str().to_str()?;
    (components.next().is_none() && valid_mount_name(name)).then(|| name.to_owned())
}

fn validate_request(request: &CreateDiskOperationRequest) -> ApiResult<()> {
    let mount_name = match request {
        CreateDiskOperationRequest::MountVolume { mount_name, .. }
        | CreateDiskOperationRequest::ChangeMountLocation { mount_name, .. } => {
            Some(mount_name.as_str())
        }
        _ => None,
    };
    if let Some(name) = mount_name
        && !valid_mount_name(name)
    {
        return Err(ApiError::bad_request(
            ErrorCode::DiskInvalidMountName,
            "mountName must match [a-z0-9][a-z0-9_-]{0,47}",
        ));
    }
    Ok(())
}

fn valid_mount_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 48
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn target_lock_key(request: &CreateDiskOperationRequest) -> String {
    match request {
        CreateDiskOperationRequest::CreatePartition { disk_id, .. }
        | CreateDiskOperationRequest::EraseAndCreatePartition { disk_id, .. }
        | CreateDiskOperationRequest::FormatPartition { disk_id, .. }
        | CreateDiskOperationRequest::AdoptFilesystem { disk_id, .. }
        | CreateDiskOperationRequest::AdoptMounted { disk_id, .. } => format!("disk:{disk_id}"),
        CreateDiskOperationRequest::MountVolume { volume_id, .. }
        | CreateDiskOperationRequest::UnmountVolume { volume_id }
        | CreateDiskOperationRequest::ChangeMountLocation { volume_id, .. }
        | CreateDiskOperationRequest::RemoveManagedVolume { volume_id } => {
            format!("volume:{volume_id}")
        }
    }
}

fn request_mount_name(request: &CreateDiskOperationRequest) -> Option<&str> {
    match request {
        CreateDiskOperationRequest::MountVolume { mount_name, .. }
        | CreateDiskOperationRequest::ChangeMountLocation { mount_name, .. } => Some(mount_name),
        _ => None,
    }
}

fn request_volume_id(request: &CreateDiskOperationRequest) -> Option<&str> {
    match request {
        CreateDiskOperationRequest::MountVolume { volume_id, .. }
        | CreateDiskOperationRequest::ChangeMountLocation { volume_id, .. } => Some(volume_id),
        _ => None,
    }
}

async fn ensure_mount_name_available(
    state: &AppState,
    mount_name: &str,
    excluded_volume_id: Option<&str>,
) -> Result<(), DomainFailure> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM managed_disk_volumes WHERE mount_name = ? COLLATE NOCASE AND (? IS NULL OR volume_id <> ?))",
    )
    .bind(mount_name)
    .bind(excluded_volume_id)
    .bind(excluded_volume_id)
    .fetch_one(&state.metadata_db)
    .await
    .map_err(db_failure)?
        != 0;
    if exists {
        return Err(DomainFailure::new(
            ErrorCode::DiskMountConflict,
            "mount name is already managed",
        ));
    }
    let path = mount_path(mount_name);
    let fstab = read_fstab().await?;
    let external_path_exists = fstab.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && trimmed.split_whitespace().nth(1) == Some(path.as_str())
    });
    if external_path_exists {
        return Err(DomainFailure::new(
            ErrorCode::DiskMountConflict,
            "an external fstab entry already uses this mount path",
        ));
    }
    Ok(())
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
    let kind = parse_kind(row.get("kind"))?;
    let target = serde_json::from_str(row.get("target_json")).unwrap_or(DiskOperationTarget {
        disk_id: None,
        device_name: None,
        fingerprint_suffix: None,
        volume_id: None,
        mount_path: None,
    });
    Ok(DiskOperation {
        operation_id: row.get("operation_id"),
        node_id: row.get("node_id"),
        kind,
        status,
        phase: if status.is_terminal() {
            None
        } else {
            row.get("phase")
        },
        target,
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

async fn transition(
    state: &AppState,
    operation_id: &str,
    status: DiskOperationStatus,
    phase: &str,
    completed_steps: i64,
) -> Result<(), DomainFailure> {
    sqlx::query(
        "UPDATE disk_operations SET status = ?, phase = ?, completed_steps = ?, updated_at = ? WHERE operation_id = ? AND cancel_requested = 0 AND status NOT IN ('succeeded','partial','failed','canceled')",
    )
    .bind(status_name(status))
    .bind(phase)
    .bind(completed_steps)
    .bind(now())
    .bind(operation_id)
    .execute(&state.metadata_db)
    .await
    .map_err(db_failure)?;
    Ok(())
}

async fn finish(
    state: &AppState,
    operation_id: &str,
    status: DiskOperationStatus,
    error_code: Option<ErrorCode>,
    error_summary: Option<&str>,
    warning_summary: Option<&str>,
) -> ApiResult<()> {
    let timestamp = now();
    sqlx::query(
        "UPDATE disk_operations SET status = ?, phase = NULL, completed_steps = CASE WHEN ? = 'succeeded' THEN total_steps ELSE completed_steps END, error_code = ?, error_summary = ?, warning_summary = COALESCE(?, warning_summary), updated_at = ?, finished_at = ? WHERE operation_id = ? AND status NOT IN ('succeeded','partial','failed','canceled')",
    )
    .bind(status_name(status))
    .bind(status_name(status))
    .bind(error_code.map(ErrorCode::as_str))
    .bind(error_summary)
    .bind(warning_summary)
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(operation_id)
    .execute(&state.metadata_db)
    .await
    .map_err(|error| ApiError::database(error.to_string()))?;
    Ok(())
}

async fn finish_cancelled(state: &AppState, operation_id: &str) -> Result<(), DomainFailure> {
    finish(
        state,
        operation_id,
        DiskOperationStatus::Canceled,
        None,
        None,
        None,
    )
    .await
    .map_err(api_failure)
}

async fn cancel_requested(state: &AppState, operation_id: &str) -> Result<bool, DomainFailure> {
    sqlx::query_scalar::<_, i64>(
        "SELECT cancel_requested FROM disk_operations WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_one(&state.metadata_db)
    .await
    .map(|value| value != 0)
    .map_err(db_failure)
}

async fn save_target(
    state: &AppState,
    operation_id: &str,
    target: DiskOperationTarget,
) -> Result<(), DomainFailure> {
    let value = serde_json::to_string(&target).map_err(|_| {
        DomainFailure::new(ErrorCode::DiskOperationFailed, "target snapshot is invalid")
    })?;
    sqlx::query(
        "UPDATE disk_operations SET target_json = ?, updated_at = ? WHERE operation_id = ?",
    )
    .bind(value)
    .bind(now())
    .bind(operation_id)
    .execute(&state.metadata_db)
    .await
    .map_err(db_failure)?;
    Ok(())
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

fn fingerprint_suffix(value: &str) -> String {
    value
        .chars()
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn db_failure(error: sqlx::Error) -> DomainFailure {
    DomainFailure::new(
        ErrorCode::DiskOperationFailed,
        format!("database error: {error}"),
    )
}

fn io_failure(error: std::io::Error) -> DomainFailure {
    DomainFailure::new(
        ErrorCode::DiskOperationFailed,
        format!("host operation failed: {error}"),
    )
}

fn api_failure(error: ApiError) -> DomainFailure {
    DomainFailure::new(error.code, error.message)
}

#[allow(dead_code)]
fn service_unavailable(message: &'static str) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::DiskNodeUnavailable,
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DomainFailure, commit_fstab_at, managed_fstab_blocks, prepare_mount_point,
        remove_empty_directory, render_add, render_remove, render_remove_external_volume_entry,
        valid_mount_name,
    };
    use std::path::Path;
    use uuid::Uuid;

    #[test]
    fn mount_name_validation_is_strict() {
        assert!(valid_mount_name("data_01"));
        assert!(!valid_mount_name("Data"));
        assert!(!valid_mount_name("../data"));
        assert!(!valid_mount_name(""));
    }

    #[test]
    fn fstab_update_preserves_external_content() {
        let original = "# external\nUUID=root / ext4 defaults 0 1\n\n";
        let rendered = render_add(original, "volume-1", "data-uuid", "/mnt/data").unwrap();
        assert!(rendered.starts_with(original));
        assert!(rendered.contains("UUID=data-uuid /mnt/data ext4 defaults,nofail 0 2"));
        assert_eq!(render_remove(&rendered, "volume-1").unwrap(), original);
    }

    #[test]
    fn managed_fstab_blocks_require_complete_markers() {
        let valid = "# BEGIN VUST MANAGED VOLUME volume-1\nUUID=fs-1 /mnt/data ext4 defaults,nofail 0 2\n# END VUST MANAGED VOLUME volume-1\n";
        let blocks = managed_fstab_blocks(valid);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].volume_id, "volume-1");
        assert_eq!(blocks[0].filesystem_uuid, "fs-1");
        assert_eq!(blocks[0].mount_path, "/mnt/data");

        let incomplete =
            "# BEGIN VUST MANAGED VOLUME volume-1\nUUID=fs-1 /mnt/data ext4 defaults,nofail 0 2\n";
        assert!(managed_fstab_blocks(incomplete).is_empty());
    }

    #[test]
    fn mounted_adoption_replaces_only_the_matching_external_entry() {
        let original = "# external\nUUID=fs-1 /mnt/data ext4 defaults 0 2\nUUID=keep /mnt/keep ext4 defaults 0 2\n";
        let rendered =
            render_remove_external_volume_entry(original, "/dev/vdb1", "fs-1", "/mnt/data");
        assert!(rendered.contains("# external"));
        assert!(!rendered.contains("UUID=fs-1"));
        assert!(rendered.contains("UUID=keep /mnt/keep"));
    }

    #[tokio::test]
    async fn prepared_mount_target_passes_findmnt_validation() {
        let root = std::env::temp_dir().join(format!("vust-fstab-test-{}", Uuid::now_v7()));
        let fstab = root.join("fstab");
        let mount = root.join("data");
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::write(&fstab, "").await.unwrap();

        let prepared = prepare_mount_point(mount.to_str().unwrap()).await.unwrap();
        let content = format!(
            "none {} tmpfs defaults,nofail 0 0\n",
            mount.to_string_lossy()
        );
        let result = commit_fstab_at(&fstab, &content).await;

        prepared.cleanup().await;
        let _ = tokio::fs::remove_file(&fstab).await;
        let _ = tokio::fs::remove_dir(&root).await;
        assert_commit_succeeded(result, &root);
    }

    #[tokio::test]
    async fn missing_mount_target_is_rejected_without_replacing_fstab() {
        let root = std::env::temp_dir().join(format!("vust-fstab-test-{}", Uuid::now_v7()));
        let fstab = root.join("fstab");
        let mount = root.join("missing");
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::write(&fstab, "# original\n").await.unwrap();
        let content = format!(
            "none {} tmpfs defaults,nofail 0 0\n",
            mount.to_string_lossy()
        );

        let result = commit_fstab_at(&fstab, &content).await;
        let retained = tokio::fs::read_to_string(&fstab).await.unwrap();

        let _ = tokio::fs::remove_file(&fstab).await;
        let _ = tokio::fs::remove_dir(&root).await;
        assert!(result.is_err());
        assert_eq!(retained, "# original\n");
    }

    #[tokio::test]
    async fn mount_target_must_be_empty() {
        let root = std::env::temp_dir().join(format!("vust-mount-test-{}", Uuid::now_v7()));
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::write(root.join("existing-data"), "keep")
            .await
            .unwrap();

        let result = prepare_mount_point(root.to_str().unwrap()).await;

        let _ = tokio::fs::remove_dir_all(&root).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cleanup_removes_an_empty_old_mount_directory() {
        let root = std::env::temp_dir().join(format!("vust-cleanup-test-{}", Uuid::now_v7()));
        tokio::fs::create_dir(&root).await.unwrap();

        let warning = remove_empty_directory(&root).await.unwrap();

        assert!(warning.is_none());
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn cleanup_preserves_non_empty_directory_with_warning() {
        let root = std::env::temp_dir().join(format!("vust-cleanup-test-{}", Uuid::now_v7()));
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::write(root.join("unexpected-data"), "keep")
            .await
            .unwrap();
        let warning = remove_empty_directory(&root).await.unwrap();

        assert!(warning.is_some());
        assert!(root.join("unexpected-data").exists());
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    fn assert_commit_succeeded(result: Result<(), DomainFailure>, root: &Path) {
        if let Err(error) = result {
            panic!(
                "prepared fstab under {} failed validation: {}",
                root.display(),
                error.summary
            );
        }
    }
}
