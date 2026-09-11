//! 磁盘管理领域契约：可信设备清单、受管数据卷与可恢复操作。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 磁盘清单整体状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum DiskInventoryStatus {
    Ready,
    Partial,
    ReadOnly,
    Unavailable,
}

/// 单个事实来源的采集状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum DiskSourceState {
    Ready,
    Failed,
    Unsupported,
}

/// 设备身份可信度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum DiskIdentityConfidence {
    Durable,
    Insufficient,
}

/// 磁盘拓扑是否满足受管数据卷约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum DiskTopologyStatus {
    Blank,
    Simple,
    Complex,
    Unknown,
}

/// 磁盘或分区的领域归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum DiskOwnershipKind {
    Managed,
    External,
    System,
    Unknown,
}

/// 受管数据卷期望状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum ManagedVolumeDesiredState {
    Mounted,
    Unmounted,
}

/// 受管数据卷实际状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum ManagedVolumeRuntimeState {
    Mounted,
    Unmounted,
    Unavailable,
    Conflict,
}

/// 磁盘操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum DiskOperationKind {
    CreatePartition,
    EraseAndCreatePartition,
    FormatPartition,
    AdoptFilesystem,
    AdoptMounted,
    MountVolume,
    UnmountVolume,
    ChangeMountLocation,
    RemoveManagedVolume,
}

/// 磁盘后台操作状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub enum DiskOperationStatus {
    Queued,
    Validating,
    Preparing,
    Applying,
    Verifying,
    Canceling,
    Succeeded,
    Partial,
    Failed,
    Canceled,
}

impl DiskOperationStatus {
    /// 判断操作是否已进入不可变终态。
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Partial | Self::Failed | Self::Canceled
        )
    }

    /// 判断操作是否仍允许安全取消。
    pub const fn is_cancellable(self) -> bool {
        matches!(self, Self::Queued | Self::Validating | Self::Preparing)
    }
}

/// 节点显示摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskNodeRef {
    pub node_id: String,
    pub node_name: String,
}

/// 事实来源状态。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/", optional_fields)]
pub struct DiskSourceStatus {
    pub source: String,
    pub state: DiskSourceState,
    pub error_summary: Option<String>,
}

/// 磁盘摘要操作能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskCapabilities {
    pub can_create_partition: bool,
    pub can_erase_and_create_partition: bool,
    pub can_view_detail: bool,
}

/// 分区操作能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskPartitionCapabilities {
    pub can_format: bool,
    pub can_adopt_filesystem: bool,
    pub can_adopt_mounted: bool,
    pub can_mount: bool,
    pub can_unmount: bool,
    pub can_change_mount_location: bool,
    pub can_remove_managed_volume: bool,
}

/// 磁盘列表摘要，不包含昂贵的分区使用量详情。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/", optional_fields)]
pub struct DiskSummary {
    pub disk_id: String,
    pub device_name: String,
    pub fingerprint: String,
    pub size_bytes: u64,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub transport: Option<String>,
    pub media_type: Option<String>,
    pub identity_confidence: DiskIdentityConfidence,
    pub topology_status: DiskTopologyStatus,
    pub ownership: DiskOwnershipKind,
    pub protection_reasons: Vec<String>,
    #[ts(inline)]
    pub capabilities: DiskCapabilities,
    pub erase_confirmation_text: Option<String>,
}

/// 文件系统事实。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/", optional_fields)]
pub struct DiskFilesystem {
    pub filesystem_type: Option<String>,
    pub uuid: Option<String>,
    pub label: Option<String>,
}

/// 文件系统空间使用量。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskUsage {
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: f64,
}

/// 受管数据卷。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct ManagedDiskVolume {
    pub volume_id: String,
    pub filesystem_uuid: String,
    pub mount_name: Option<String>,
    pub mount_path: Option<String>,
    pub desired_state: ManagedVolumeDesiredState,
    pub runtime_state: ManagedVolumeRuntimeState,
}

/// 分区详情。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/", optional_fields)]
pub struct DiskPartition {
    pub partition_id: String,
    pub device_name: String,
    pub part_uuid: Option<String>,
    pub size_bytes: u64,
    #[ts(inline)]
    pub filesystem: DiskFilesystem,
    pub mounts: Vec<String>,
    pub usage: Option<DiskUsage>,
    pub ownership: DiskOwnershipKind,
    pub protection_reasons: Vec<String>,
    #[ts(inline)]
    pub capabilities: DiskPartitionCapabilities,
    pub format_confirmation_text: Option<String>,
    pub managed_volume: Option<ManagedDiskVolume>,
}

/// 对危险操作有决策价值的引用摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskReference {
    pub kind: String,
    pub summary: String,
}

/// 磁盘按需详情。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskDetail {
    #[serde(flatten)]
    #[ts(flatten)]
    pub summary: DiskSummary,
    pub partitions: Vec<DiskPartition>,
    pub references: Vec<DiskReference>,
}

/// 节点磁盘清单。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskInventory {
    #[ts(inline)]
    pub node: DiskNodeRef,
    pub status: DiskInventoryStatus,
    pub collected_at: String,
    pub source_statuses: Vec<DiskSourceStatus>,
    pub warnings: Vec<String>,
    pub disks: Vec<DiskSummary>,
}

/// 创建磁盘后台操作的请求。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "disks/")]
pub enum CreateDiskOperationRequest {
    CreatePartition {
        #[serde(rename = "diskId")]
        #[ts(rename = "diskId")]
        disk_id: String,
        #[serde(rename = "expectedFingerprint")]
        #[ts(rename = "expectedFingerprint")]
        expected_fingerprint: String,
    },
    EraseAndCreatePartition {
        #[serde(rename = "diskId")]
        #[ts(rename = "diskId")]
        disk_id: String,
        #[serde(rename = "expectedFingerprint")]
        #[ts(rename = "expectedFingerprint")]
        expected_fingerprint: String,
        #[serde(rename = "confirmationText")]
        #[ts(rename = "confirmationText")]
        confirmation_text: String,
    },
    FormatPartition {
        #[serde(rename = "diskId")]
        #[ts(rename = "diskId")]
        disk_id: String,
        #[serde(rename = "partitionId")]
        #[ts(rename = "partitionId")]
        partition_id: String,
        #[serde(rename = "expectedFingerprint")]
        #[ts(rename = "expectedFingerprint")]
        expected_fingerprint: String,
        #[serde(rename = "confirmationText")]
        #[ts(rename = "confirmationText")]
        confirmation_text: String,
    },
    AdoptFilesystem {
        #[serde(rename = "diskId")]
        #[ts(rename = "diskId")]
        disk_id: String,
        #[serde(rename = "partitionId")]
        #[ts(rename = "partitionId")]
        partition_id: String,
        #[serde(rename = "expectedFingerprint")]
        #[ts(rename = "expectedFingerprint")]
        expected_fingerprint: String,
    },
    AdoptMounted {
        #[serde(rename = "diskId")]
        #[ts(rename = "diskId")]
        disk_id: String,
        #[serde(rename = "partitionId")]
        #[ts(rename = "partitionId")]
        partition_id: String,
        #[serde(rename = "expectedFingerprint")]
        #[ts(rename = "expectedFingerprint")]
        expected_fingerprint: String,
    },
    MountVolume {
        #[serde(rename = "volumeId")]
        #[ts(rename = "volumeId")]
        volume_id: String,
        #[serde(rename = "mountName")]
        #[ts(rename = "mountName")]
        mount_name: String,
    },
    UnmountVolume {
        #[serde(rename = "volumeId")]
        #[ts(rename = "volumeId")]
        volume_id: String,
    },
    ChangeMountLocation {
        #[serde(rename = "volumeId")]
        #[ts(rename = "volumeId")]
        volume_id: String,
        #[serde(rename = "mountName")]
        #[ts(rename = "mountName")]
        mount_name: String,
    },
    RemoveManagedVolume {
        #[serde(rename = "volumeId")]
        #[ts(rename = "volumeId")]
        volume_id: String,
    },
}

impl CreateDiskOperationRequest {
    /// 返回请求对应的稳定操作类型。
    pub const fn kind(&self) -> DiskOperationKind {
        match self {
            Self::CreatePartition { .. } => DiskOperationKind::CreatePartition,
            Self::EraseAndCreatePartition { .. } => DiskOperationKind::EraseAndCreatePartition,
            Self::FormatPartition { .. } => DiskOperationKind::FormatPartition,
            Self::AdoptFilesystem { .. } => DiskOperationKind::AdoptFilesystem,
            Self::AdoptMounted { .. } => DiskOperationKind::AdoptMounted,
            Self::MountVolume { .. } => DiskOperationKind::MountVolume,
            Self::UnmountVolume { .. } => DiskOperationKind::UnmountVolume,
            Self::ChangeMountLocation { .. } => DiskOperationKind::ChangeMountLocation,
            Self::RemoveManagedVolume { .. } => DiskOperationKind::RemoveManagedVolume,
        }
    }
}

/// 操作目标快照。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/", optional_fields)]
pub struct DiskOperationTarget {
    pub disk_id: Option<String>,
    pub device_name: Option<String>,
    pub fingerprint_suffix: Option<String>,
    pub volume_id: Option<String>,
    pub mount_path: Option<String>,
}

/// 后台操作能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/")]
pub struct DiskOperationCapabilities {
    pub can_cancel: bool,
}

/// 可恢复跟踪的磁盘后台操作。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "disks/", optional_fields)]
pub struct DiskOperation {
    pub operation_id: String,
    pub node_id: String,
    pub kind: DiskOperationKind,
    pub status: DiskOperationStatus,
    pub phase: Option<String>,
    #[ts(inline)]
    pub target: DiskOperationTarget,
    pub completed_steps: u32,
    pub total_steps: u32,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub warning_summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    #[ts(inline)]
    pub capabilities: DiskOperationCapabilities,
}

/// Master 向 Agent 提交的可信磁盘操作。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCreateDiskOperationRequest {
    pub operation_id: String,
    pub node_id: String,
    pub request: CreateDiskOperationRequest,
}
