//! 磁盘事实采集：合并块设备、挂载、swap、签名与受管数据卷状态。

use crate::state::DbPool;
use crate::types::ApiError;
use axum::http::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use vust_contracts::api::ErrorCode;
use vust_contracts::disks::{
    DiskCapabilities, DiskDetail, DiskFilesystem, DiskIdentityConfidence, DiskInventory,
    DiskInventoryStatus, DiskNodeRef, DiskOwnershipKind, DiskPartition, DiskPartitionCapabilities,
    DiskReference, DiskSourceState, DiskSourceStatus, DiskSummary, DiskTopologyStatus, DiskUsage,
    ManagedDiskVolume, ManagedVolumeDesiredState, ManagedVolumeRuntimeState,
};

const SYSTEM_MOUNTS: &[&str] = &["/", "/boot", "/boot/efi", "/usr", "/var"];

#[derive(Debug, Deserialize)]
struct LsblkResult {
    #[serde(default)]
    blockdevices: Vec<BlockDevice>,
}

#[derive(Debug, Clone, Deserialize)]
struct BlockDevice {
    path: String,
    kname: String,
    #[serde(rename = "maj:min")]
    major_minor: Option<String>,
    #[serde(rename = "type")]
    device_type: String,
    #[serde(default)]
    size: Option<NumberOrText>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    serial: Option<String>,
    #[serde(default)]
    wwn: Option<String>,
    #[serde(default)]
    tran: Option<String>,
    #[serde(default)]
    rota: Option<bool>,
    #[serde(default)]
    ro: Option<bool>,
    #[serde(default)]
    pttype: Option<String>,
    #[serde(default)]
    fstype: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    partuuid: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    mountpoints: Vec<Option<String>>,
    #[serde(default)]
    children: Vec<BlockDevice>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum NumberOrText {
    Number(u64),
    Text(String),
}

#[derive(Debug, Clone, FromRow)]
struct ManagedVolumeRow {
    volume_id: String,
    disk_id: String,
    partition_id: String,
    filesystem_uuid: String,
    mount_name: Option<String>,
    mount_path: Option<String>,
    desired_state: String,
    runtime_state: String,
}

#[derive(Debug)]
struct Facts {
    swaps: HashSet<String>,
    fstab_specs: HashSet<String>,
    fstab_mounts: HashSet<String>,
    mountinfo: HashMap<String, Vec<String>>,
    by_id: HashMap<PathBuf, String>,
    serial_counts: HashMap<String, usize>,
    trusted_runtime: bool,
    partial: bool,
    sources: Vec<DiskSourceStatus>,
    warnings: Vec<String>,
}

/// 采集磁盘摘要；节点显示信息由 Master 在语义网关覆盖。
pub async fn inventory(pool: &DbPool) -> Result<DiskInventory, ApiError> {
    let managed = managed_volumes(pool).await?;
    tokio::task::spawn_blocking(move || collect_inventory(managed))
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
}

/// 按不透明磁盘 ID 采集详情。
pub async fn detail(pool: &DbPool, disk_id: &str) -> Result<DiskDetail, ApiError> {
    let managed = managed_volumes(pool).await?;
    let disk_id = disk_id.to_string();
    tokio::task::spawn_blocking(move || collect_detail(&disk_id, managed))
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
}

async fn managed_volumes(pool: &DbPool) -> Result<Vec<ManagedVolumeRow>, ApiError> {
    sqlx::query_as::<_, ManagedVolumeRow>(
        "SELECT volume_id, disk_id, partition_id, filesystem_uuid, mount_name, mount_path, desired_state, runtime_state FROM managed_disk_volumes",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| ApiError::database(error.to_string()))
}

fn collect_inventory(managed: Vec<ManagedVolumeRow>) -> Result<DiskInventory, ApiError> {
    let parsed = load_lsblk()?;
    let mut facts = collect_facts(&parsed.blockdevices);
    let mut signatures = HashMap::new();
    for disk in parsed.blockdevices.iter().filter(is_disk) {
        match wipefs_has_signature(&disk.path) {
            Ok(value) => {
                signatures.insert(disk.path.clone(), value);
            }
            Err(summary) => {
                facts.partial = true;
                facts.warnings.push("wipefsUnavailable".to_string());
                facts.sources.push(DiskSourceStatus {
                    source: format!("wipefs:{}", disk.kname),
                    state: DiskSourceState::Failed,
                    error_summary: Some(summary),
                });
            }
        }
    }
    if !facts
        .sources
        .iter()
        .any(|item| item.source.starts_with("wipefs:"))
    {
        facts.sources.push(ready_source("wipefs"));
    }
    let status = inventory_status(&facts);
    let managed_ids = managed
        .iter()
        .map(|item| item.disk_id.as_str())
        .collect::<HashSet<_>>();
    let disks = parsed
        .blockdevices
        .iter()
        .filter(is_disk)
        .map(|disk| {
            let identity = identity(disk, &facts);
            summary(
                disk,
                &facts,
                signatures.get(&disk.path).copied(),
                managed_ids.contains(identity.0.as_str()),
                status,
            )
        })
        .collect();
    Ok(DiskInventory {
        node: DiskNodeRef {
            node_id: String::new(),
            node_name: String::new(),
        },
        status,
        collected_at: chrono::Utc::now().to_rfc3339(),
        source_statuses: facts.sources,
        warnings: facts.warnings,
        disks,
    })
}

fn collect_detail(disk_id: &str, managed: Vec<ManagedVolumeRow>) -> Result<DiskDetail, ApiError> {
    let parsed = load_lsblk()?;
    let facts = collect_facts(&parsed.blockdevices);
    let disk = parsed
        .blockdevices
        .iter()
        .filter(is_disk)
        .find(|disk| identity(disk, &facts).0 == disk_id)
        .ok_or_else(|| ApiError::not_found(ErrorCode::DiskNotFound, "disk not found"))?;
    let signature = wipefs_has_signature(&disk.path).ok();
    let status = inventory_status(&facts);
    let managed_disk = managed.iter().any(|item| item.disk_id == disk_id);
    let summary = summary(disk, &facts, signature, managed_disk, status);
    let system = tree_system(disk, &facts);
    let by_partition = managed
        .into_iter()
        .map(|item| (item.partition_id.clone(), item))
        .collect::<HashMap<_, _>>();
    let partitions = disk
        .children
        .iter()
        .filter(|item| item.device_type == "part")
        .map(|partition| partition_detail(partition, &summary, &facts, system, &by_partition))
        .collect();
    Ok(DiskDetail {
        summary,
        partitions,
        references: references(disk, &facts),
    })
}

fn partition_detail(
    partition: &BlockDevice,
    disk: &DiskSummary,
    facts: &Facts,
    system_disk: bool,
    managed: &HashMap<String, ManagedVolumeRow>,
) -> DiskPartition {
    let partition_id = partition_id(partition);
    let managed_row = managed.get(&partition_id);
    let mounts = device_mounts(partition, facts);
    let system = system_disk || tree_system(partition, facts);
    let external_fstab = matches_fstab(partition, facts) && managed_row.is_none();
    let active_holders = !holders(&partition.kname).is_empty();
    let mut protection_reasons = Vec::new();
    if system {
        protection_reasons.push("systemDevice".to_string());
    }
    if facts.swaps.contains(&partition.path) {
        protection_reasons.push("activeSwap".to_string());
    }
    if !mounts.is_empty() && managed_row.is_none() {
        protection_reasons.push("externalMount".to_string());
    }
    if external_fstab {
        protection_reasons.push("externalFstab".to_string());
    }
    if active_holders {
        protection_reasons.push("activeHolders".to_string());
    }
    let filesystem_type = normalized(&partition.fstype);
    let uuid = normalized(&partition.uuid);
    let facts_ready = inventory_status(facts) == DiskInventoryStatus::Ready;
    let durable_partition = disk.identity_confidence == DiskIdentityConfidence::Durable
        && normalized(&partition.partuuid).is_some();
    let can_format = facts_ready
        && durable_partition
        && !system
        && !facts.swaps.contains(&partition.path)
        && !partition.ro.unwrap_or(false)
        && !active_holders;
    let can_adopt_filesystem = disk.topology_status == DiskTopologyStatus::Simple
        && disk.identity_confidence == DiskIdentityConfidence::Durable
        && facts_ready
        && !system
        && mounts.is_empty()
        && !external_fstab
        && !active_holders
        && filesystem_type.as_deref() == Some("ext4")
        && uuid.is_some()
        && managed_row.is_none();
    let can_adopt_mounted = disk.topology_status == DiskTopologyStatus::Simple
        && disk.identity_confidence == DiskIdentityConfidence::Durable
        && inventory_status(facts) == DiskInventoryStatus::Ready
        && !system
        && mounts.len() == 1
        && managed_mount_name(&mounts[0]).is_some()
        && !active_holders
        && filesystem_type.as_deref() == Some("ext4")
        && uuid.is_some()
        && managed_row.is_none();
    let managed_volume = managed_row.map(volume);
    DiskPartition {
        partition_id,
        device_name: partition.kname.clone(),
        part_uuid: normalized(&partition.partuuid),
        size_bytes: size_bytes(partition.size.as_ref()),
        filesystem: DiskFilesystem {
            filesystem_type: filesystem_type.clone(),
            uuid,
            label: normalized(&partition.label),
        },
        mounts: mounts.clone(),
        usage: mounts.first().and_then(|path| usage(path)),
        ownership: if system {
            DiskOwnershipKind::System
        } else if managed_row.is_some() {
            DiskOwnershipKind::Managed
        } else if filesystem_type.is_some() || !mounts.is_empty() || external_fstab {
            DiskOwnershipKind::External
        } else {
            DiskOwnershipKind::Unknown
        },
        protection_reasons,
        capabilities: DiskPartitionCapabilities {
            can_format,
            can_adopt_filesystem,
            can_adopt_mounted,
            can_mount: managed_volume.as_ref().is_some_and(|item| {
                item.desired_state == ManagedVolumeDesiredState::Unmounted
                    && item.runtime_state == ManagedVolumeRuntimeState::Unmounted
            }),
            can_unmount: managed_volume.as_ref().is_some_and(|item| {
                item.desired_state == ManagedVolumeDesiredState::Mounted
                    && item.runtime_state == ManagedVolumeRuntimeState::Mounted
            }),
            can_change_mount_location: managed_volume.as_ref().is_some_and(|item| {
                item.desired_state == ManagedVolumeDesiredState::Mounted
                    && item.runtime_state == ManagedVolumeRuntimeState::Mounted
            }),
            can_remove_managed_volume: managed_volume.as_ref().is_some_and(|item| {
                item.desired_state == ManagedVolumeDesiredState::Unmounted
                    && item.runtime_state == ManagedVolumeRuntimeState::Unmounted
            }),
        },
        format_confirmation_text: can_format.then(|| {
            let suffix = &disk.fingerprint[disk.fingerprint.len().saturating_sub(8)..];
            format!("FORMAT {} {suffix}", partition.kname)
        }),
        managed_volume,
    }
}

fn summary(
    disk: &BlockDevice,
    facts: &Facts,
    has_signature: Option<bool>,
    managed: bool,
    status: DiskInventoryStatus,
) -> DiskSummary {
    let (disk_id, confidence, source) = identity(disk, facts);
    let fingerprint = fingerprint(disk, &source);
    let system = tree_system(disk, facts);
    let mounted = tree_mounted(disk, facts);
    let swap = tree_swap(disk, &facts.swaps);
    let active_holders = tree_holders(disk);
    let external_fstab = tree_fstab(disk, facts) && !managed;
    let topology_status = topology(disk, has_signature);
    let mut protection_reasons = Vec::new();
    if system {
        protection_reasons.push("systemDevice".to_string());
    }
    if swap {
        protection_reasons.push("activeSwap".to_string());
    }
    if disk.ro.unwrap_or(false) {
        protection_reasons.push("readOnlyDevice".to_string());
    }
    if mounted && !managed {
        protection_reasons.push("externalMount".to_string());
    }
    if external_fstab {
        protection_reasons.push("externalFstab".to_string());
    }
    if active_holders {
        protection_reasons.push("activeHolders".to_string());
    }
    if confidence == DiskIdentityConfidence::Insufficient {
        protection_reasons.push("identityInsufficient".to_string());
    }
    if matches!(
        topology_status,
        DiskTopologyStatus::Complex | DiskTopologyStatus::Unknown
    ) {
        protection_reasons.push("unsupportedTopology".to_string());
    }
    if status != DiskInventoryStatus::Ready {
        protection_reasons.push("inventoryNotReady".to_string());
    }
    let safe = status == DiskInventoryStatus::Ready
        && confidence == DiskIdentityConfidence::Durable
        && !system
        && !swap
        && !active_holders
        && !disk.ro.unwrap_or(false);
    let can_create_partition = safe
        && !mounted
        && !external_fstab
        && topology_status == DiskTopologyStatus::Blank
        && has_signature == Some(false);
    let can_erase = safe && topology_status != DiskTopologyStatus::Unknown;
    let suffix = fingerprint[fingerprint.len().saturating_sub(8)..].to_string();
    DiskSummary {
        disk_id,
        device_name: disk.kname.clone(),
        fingerprint,
        size_bytes: size_bytes(disk.size.as_ref()),
        model: normalized(&disk.model),
        serial: normalized(&disk.serial),
        transport: normalized(&disk.tran),
        media_type: Some(
            if disk.rota.unwrap_or(false) {
                "hdd"
            } else {
                "ssd"
            }
            .to_string(),
        ),
        identity_confidence: confidence,
        topology_status,
        ownership: if system {
            DiskOwnershipKind::System
        } else if managed {
            DiskOwnershipKind::Managed
        } else if topology_status == DiskTopologyStatus::Simple
            || mounted
            || external_fstab
            || has_signature == Some(true)
        {
            DiskOwnershipKind::External
        } else {
            DiskOwnershipKind::Unknown
        },
        protection_reasons,
        capabilities: DiskCapabilities {
            can_create_partition,
            can_erase_and_create_partition: can_erase,
            can_view_detail: true,
        },
        erase_confirmation_text: can_erase.then(|| format!("ERASE {} {}", disk.kname, suffix)),
    }
}

fn managed_mount_name(path: &str) -> Option<String> {
    let root = std::env::var("VUST_MOUNT_ROOT").unwrap_or_else(|_| "/mnt".to_owned());
    let relative = Path::new(path)
        .strip_prefix(root.trim_end_matches('/'))
        .ok()?;
    let mut components = relative.components();
    let name = components.next()?.as_os_str().to_str()?;
    (!name.is_empty() && !name.contains('/') && components.next().is_none())
        .then(|| name.to_owned())
}

fn load_lsblk() -> Result<LsblkResult, ApiError> {
    let output = Command::new("lsblk")
        .args([
            "--bytes", "--json", "--tree", "--output",
            "PATH,KNAME,MAJ:MIN,TYPE,SIZE,MODEL,SERIAL,WWN,TRAN,ROTA,RO,PTTYPE,FSTYPE,UUID,PARTUUID,LABEL,MOUNTPOINTS",
        ])
        .output()
        .map_err(|error| inventory_error(error.to_string()))?;
    if !output.status.success() {
        return Err(inventory_error("lsblk failed"));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| inventory_error(error.to_string()))
}

fn collect_facts(devices: &[BlockDevice]) -> Facts {
    let (swaps, swaps_ok) = read_swaps();
    let (fstab_specs, fstab_mounts, fstab_ok) = read_fstab();
    let (mountinfo, mountinfo_ok) = read_mountinfo();
    let trusted_runtime = trusted_runtime();
    let mut sources = vec![ready_source("lsblk")];
    sources.push(source_result("swaps", swaps_ok));
    sources.push(source_result("fstab", fstab_ok));
    sources.push(source_result("mountInfo", mountinfo_ok));
    sources.push(if trusted_runtime {
        ready_source("runtime")
    } else {
        DiskSourceStatus {
            source: "runtime".to_string(),
            state: DiskSourceState::Unsupported,
            error_summary: Some("disk changes require a trusted Linux host runtime".to_string()),
        }
    });
    Facts {
        swaps,
        fstab_specs,
        fstab_mounts,
        mountinfo,
        by_id: read_by_id(),
        serial_counts: serial_counts(devices),
        trusted_runtime,
        partial: !swaps_ok || !fstab_ok || !mountinfo_ok,
        sources,
        warnings: if trusted_runtime {
            Vec::new()
        } else {
            vec!["unsupportedRuntime".to_string()]
        },
    }
}

fn inventory_status(facts: &Facts) -> DiskInventoryStatus {
    if !facts.trusted_runtime {
        DiskInventoryStatus::ReadOnly
    } else if facts.partial {
        DiskInventoryStatus::Partial
    } else {
        DiskInventoryStatus::Ready
    }
}

fn identity(disk: &BlockDevice, facts: &Facts) -> (String, DiskIdentityConfidence, String) {
    let canonical = fs::canonicalize(&disk.path).unwrap_or_else(|_| PathBuf::from(&disk.path));
    let stable = normalized(&disk.wwn)
        .map(|value| format!("wwn:{value}"))
        .or_else(|| {
            facts
                .by_id
                .get(&canonical)
                .map(|value| format!("by-id:{value}"))
        })
        .or_else(|| {
            let serial = normalized(&disk.serial)?;
            let key = serial_key(disk, &serial);
            (facts.serial_counts.get(&key) == Some(&1)).then(|| format!("serial:{key}"))
        });
    let confidence = if stable.is_some() {
        DiskIdentityConfidence::Durable
    } else {
        DiskIdentityConfidence::Insufficient
    };
    let source = stable.unwrap_or_else(|| format!("path:{}", disk.path));
    (format!("disk_{}", hash(&source)), confidence, source)
}

fn partition_id(partition: &BlockDevice) -> String {
    let value = normalized(&partition.partuuid).unwrap_or_else(|| partition.path.clone());
    format!("part_{}", hash(&value))
}

fn fingerprint(disk: &BlockDevice, source: &str) -> String {
    // 指纹只描述稳定设备身份与容量，不包含会被本操作主动改变的分区、文件系统或挂载状态。
    // 每个阶段仍会重新采集拓扑和引用并独立校验 capability。
    hash(&format!("{source}|{}", size_bytes(disk.size.as_ref())))
}

fn topology(disk: &BlockDevice, signature: Option<bool>) -> DiskTopologyStatus {
    if disk.children.is_empty() && normalized(&disk.pttype).is_none() && signature == Some(false) {
        DiskTopologyStatus::Blank
    } else if disk.children.len() == 1
        && disk.children[0].device_type == "part"
        && disk.children[0].children.is_empty()
    {
        DiskTopologyStatus::Simple
    } else if disk.children.is_empty() && signature.is_none() {
        DiskTopologyStatus::Unknown
    } else {
        DiskTopologyStatus::Complex
    }
}

fn references(device: &BlockDevice, facts: &Facts) -> Vec<DiskReference> {
    let mut result = Vec::new();
    for mount in device_mounts(device, facts) {
        result.push(DiskReference {
            kind: if SYSTEM_MOUNTS.contains(&mount.as_str()) {
                "systemMount"
            } else {
                "mount"
            }
            .to_string(),
            summary: mount,
        });
    }
    if facts.swaps.contains(&device.path) {
        result.push(DiskReference {
            kind: "swap".to_string(),
            summary: device.path.clone(),
        });
    }
    if matches_fstab(device, facts) {
        result.push(DiskReference {
            kind: "fstab".to_string(),
            summary: device.path.clone(),
        });
    }
    result.extend(
        holders(&device.kname)
            .into_iter()
            .map(|item| DiskReference {
                kind: "holder".to_string(),
                summary: item,
            }),
    );
    for child in &device.children {
        result.extend(references(child, facts));
    }
    result
}

fn tree_system(device: &BlockDevice, facts: &Facts) -> bool {
    facts.swaps.contains(&device.path)
        || device_mounts(device, facts)
            .iter()
            .any(|path| SYSTEM_MOUNTS.contains(&path.as_str()))
        || device
            .children
            .iter()
            .any(|child| tree_system(child, facts))
}

fn tree_swap(device: &BlockDevice, swaps: &HashSet<String>) -> bool {
    swaps.contains(&device.path) || device.children.iter().any(|child| tree_swap(child, swaps))
}

fn tree_mounted(device: &BlockDevice, facts: &Facts) -> bool {
    !device_mounts(device, facts).is_empty()
        || device
            .children
            .iter()
            .any(|child| tree_mounted(child, facts))
}

fn tree_holders(device: &BlockDevice) -> bool {
    !holders(&device.kname).is_empty() || device.children.iter().any(tree_holders)
}

fn tree_fstab(device: &BlockDevice, facts: &Facts) -> bool {
    matches_fstab(device, facts) || device.children.iter().any(|child| tree_fstab(child, facts))
}

fn matches_fstab(device: &BlockDevice, facts: &Facts) -> bool {
    facts.fstab_specs.contains(&device.path)
        || normalized(&device.uuid)
            .is_some_and(|uuid| facts.fstab_specs.contains(&format!("UUID={uuid}")))
        || device_mounts(device, facts)
            .iter()
            .any(|path| facts.fstab_mounts.contains(path))
}

fn wipefs_has_signature(path: &str) -> Result<bool, String> {
    let output = Command::new("wipefs")
        .args(["--noheadings", "--output", "TYPE,UUID,LABEL", path])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("wipefs inspection failed".to_string());
    }
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

fn read_swaps() -> (HashSet<String>, bool) {
    fs::read_to_string("/proc/swaps").map_or((HashSet::new(), false), |value| {
        (
            value
                .lines()
                .skip(1)
                .filter_map(|line| line.split_whitespace().next().map(str::to_string))
                .collect(),
            true,
        )
    })
}

fn read_fstab() -> (HashSet<String>, HashSet<String>, bool) {
    let Ok(value) = fs::read_to_string("/etc/fstab") else {
        return (HashSet::new(), HashSet::new(), false);
    };
    let mut specs = HashSet::new();
    let mut mounts = HashSet::new();
    for line in value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() >= 2 {
            specs.insert(fields[0].to_string());
            mounts.insert(fields[1].to_string());
        }
    }
    (specs, mounts, true)
}

fn read_mountinfo() -> (HashMap<String, Vec<String>>, bool) {
    let Ok(value) = fs::read_to_string("/proc/self/mountinfo") else {
        return (HashMap::new(), false);
    };
    let mut mounts = HashMap::<String, Vec<String>>::new();
    for line in value.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 {
            continue;
        }
        mounts
            .entry(fields[2].to_string())
            .or_default()
            .push(decode_mountinfo_path(fields[4]));
    }
    (mounts, true)
}

fn decode_mountinfo_path(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\134", "\\")
}

fn read_by_id() -> HashMap<PathBuf, String> {
    let Ok(entries) = fs::read_dir("/dev/disk/by-id") else {
        return HashMap::new();
    };
    let mut result = HashMap::<PathBuf, String>::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.contains("-part") {
            continue;
        }
        let Ok(target) = fs::canonicalize(&path) else {
            continue;
        };
        result
            .entry(target)
            .and_modify(|current| {
                if name < current.as_str() {
                    *current = name.to_string();
                }
            })
            .or_insert_with(|| name.to_string());
    }
    result
}

fn serial_counts(devices: &[BlockDevice]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for disk in devices.iter().filter(is_disk) {
        if let Some(serial) = normalized(&disk.serial) {
            *counts.entry(serial_key(disk, &serial)).or_insert(0) += 1;
        }
    }
    counts
}

fn serial_key(disk: &BlockDevice, serial: &str) -> String {
    format!("{}:{serial}", normalized(&disk.model).unwrap_or_default())
}

fn trusted_runtime() -> bool {
    if std::env::var_os("WSL_INTEROP").is_some()
        || std::env::var_os("WSL_DISTRO_NAME").is_some()
        || Path::new("/.dockerenv").exists()
    {
        return false;
    }
    let text = format!(
        "{} {}",
        fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default(),
        fs::read_to_string("/proc/1/cgroup").unwrap_or_default()
    )
    .to_ascii_lowercase();
    ![
        "microsoft",
        "-wsl",
        "docker",
        "containerd",
        "kubepods",
        "lxc",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn holders(kname: &str) -> Vec<String> {
    fs::read_dir(format!("/sys/class/block/{kname}/holders")).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .collect()
        },
    )
}

fn mountpoints(device: &BlockDevice) -> Vec<String> {
    device
        .mountpoints
        .iter()
        .filter_map(|item| item.as_deref())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn device_mounts(device: &BlockDevice, facts: &Facts) -> Vec<String> {
    let mut result = mountpoints(device);
    if let Some(major_minor) = device.major_minor.as_deref()
        && let Some(paths) = facts.mountinfo.get(major_minor)
    {
        result.extend(paths.iter().cloned());
    }
    result.sort();
    result.dedup();
    result
}

fn volume(row: &ManagedVolumeRow) -> ManagedDiskVolume {
    ManagedDiskVolume {
        volume_id: row.volume_id.clone(),
        filesystem_uuid: row.filesystem_uuid.clone(),
        mount_name: row.mount_name.clone(),
        mount_path: row.mount_path.clone(),
        desired_state: if row.desired_state == "mounted" {
            ManagedVolumeDesiredState::Mounted
        } else {
            ManagedVolumeDesiredState::Unmounted
        },
        runtime_state: match row.runtime_state.as_str() {
            "mounted" => ManagedVolumeRuntimeState::Mounted,
            "unmounted" => ManagedVolumeRuntimeState::Unmounted,
            "conflict" => ManagedVolumeRuntimeState::Conflict,
            _ => ManagedVolumeRuntimeState::Unavailable,
        },
    }
}

fn usage(path: &str) -> Option<DiskUsage> {
    let path = CString::new(path).ok()?;
    let mut value = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and statvfs initializes value on success.
    if unsafe { libc::statvfs(path.as_ptr(), value.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: successful statvfs initialized the structure.
    let value = unsafe { value.assume_init() };
    let block_size = value.f_frsize;
    let total = value.f_blocks.saturating_mul(block_size);
    let available = value.f_bavail.saturating_mul(block_size);
    let used = total.saturating_sub(value.f_bfree.saturating_mul(block_size));
    let denominator = used.saturating_add(available);
    Some(DiskUsage {
        used_bytes: used,
        available_bytes: available,
        usage_percent: if denominator == 0 {
            0.0
        } else {
            used as f64 * 100.0 / denominator as f64
        },
    })
}

fn normalized(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
}

fn size_bytes(value: Option<&NumberOrText>) -> u64 {
    match value {
        Some(NumberOrText::Number(value)) => *value,
        Some(NumberOrText::Text(value)) => value.trim().parse().unwrap_or(0),
        None => 0,
    }
}

fn is_disk(device: &&BlockDevice) -> bool {
    device.device_type == "disk"
}

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))[..32].to_string()
}

fn inventory_error(detail: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::DiskInventoryPartial,
        "disk inventory unavailable",
    )
    .with_detail(detail.into())
}

fn ready_source(source: &str) -> DiskSourceStatus {
    DiskSourceStatus {
        source: source.to_string(),
        state: DiskSourceState::Ready,
        error_summary: None,
    }
}

fn source_result(source: &str, ready: bool) -> DiskSourceStatus {
    if ready {
        ready_source(source)
    } else {
        DiskSourceStatus {
            source: source.to_string(),
            state: DiskSourceState::Failed,
            error_summary: Some(format!("failed to read {source}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockDevice, Facts, NumberOrText, partition_detail, summary, topology};
    use std::collections::{HashMap, HashSet};
    use vust_contracts::disks::{DiskInventoryStatus, DiskTopologyStatus};

    fn test_disk() -> BlockDevice {
        BlockDevice {
            path: "/dev/vdb".into(),
            kname: "vdb".into(),
            major_minor: Some("252:16".into()),
            device_type: "disk".into(),
            size: Some(NumberOrText::Number(1024)),
            model: Some("Virtual".into()),
            serial: Some("stable-1".into()),
            wwn: None,
            tran: Some("virtio".into()),
            rota: Some(false),
            ro: Some(false),
            pttype: None,
            fstype: None,
            uuid: None,
            partuuid: None,
            label: None,
            mountpoints: Vec::new(),
            children: Vec::new(),
        }
    }

    fn facts() -> Facts {
        Facts {
            swaps: HashSet::new(),
            fstab_specs: HashSet::new(),
            fstab_mounts: HashSet::new(),
            mountinfo: HashMap::new(),
            by_id: HashMap::new(),
            serial_counts: HashMap::from([("Virtual:stable-1".to_string(), 1)]),
            trusted_runtime: true,
            partial: false,
            sources: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn test_partition() -> BlockDevice {
        BlockDevice {
            path: "/dev/vdb1".into(),
            kname: "vdb1".into(),
            major_minor: Some("252:17".into()),
            device_type: "part".into(),
            size: Some(NumberOrText::Number(1000)),
            model: None,
            serial: None,
            wwn: None,
            tran: None,
            rota: Some(false),
            ro: Some(false),
            pttype: None,
            fstype: Some("ext4".into()),
            uuid: Some("filesystem-1".into()),
            partuuid: Some("partition-1".into()),
            label: None,
            mountpoints: Vec::new(),
            children: Vec::new(),
        }
    }

    #[test]
    fn blank_disk_can_initialize_only_with_complete_inventory() {
        let disk = test_disk();
        assert_eq!(topology(&disk, Some(false)), DiskTopologyStatus::Blank);
        assert!(
            summary(
                &disk,
                &facts(),
                Some(false),
                false,
                DiskInventoryStatus::Ready
            )
            .capabilities
            .can_create_partition
        );
        assert!(
            !summary(
                &disk,
                &facts(),
                Some(false),
                false,
                DiskInventoryStatus::Partial
            )
            .capabilities
            .can_create_partition
        );
    }

    #[test]
    fn external_mount_requires_explicit_erase_but_does_not_hide_it() {
        let disk = test_disk();
        let mut facts = facts();
        facts
            .mountinfo
            .insert("252:16".to_string(), vec!["/srv/bind-data".to_string()]);
        let summary = summary(
            &disk,
            &facts,
            Some(false),
            false,
            DiskInventoryStatus::Ready,
        );
        assert!(!summary.capabilities.can_create_partition);
        assert!(summary.capabilities.can_erase_and_create_partition);
        assert!(
            summary
                .protection_reasons
                .contains(&"externalMount".to_string())
        );
    }

    #[test]
    fn system_mount_still_closes_all_destructive_capabilities() {
        let disk = test_disk();
        let mut facts = facts();
        facts
            .mountinfo
            .insert("252:16".to_string(), vec!["/".to_string()]);
        let summary = summary(
            &disk,
            &facts,
            Some(false),
            false,
            DiskInventoryStatus::Ready,
        );
        assert!(!summary.capabilities.can_create_partition);
        assert!(!summary.capabilities.can_erase_and_create_partition);
        assert!(
            summary
                .protection_reasons
                .contains(&"systemDevice".to_string())
        );
    }

    #[test]
    fn ordinary_partition_can_be_formatted_without_being_mounted() {
        let mut disk = test_disk();
        disk.children = vec![test_partition()];
        let facts = facts();
        let summary = summary(&disk, &facts, Some(true), false, DiskInventoryStatus::Ready);
        let partition =
            partition_detail(&disk.children[0], &summary, &facts, false, &HashMap::new());

        assert!(partition.capabilities.can_format);
        assert!(partition.capabilities.can_adopt_filesystem);
        assert!(partition.format_confirmation_text.is_some());
        assert!(partition.mounts.is_empty());
    }
}
