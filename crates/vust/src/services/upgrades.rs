//! 在线升级服务：同步发布版本、创建计划并驱动目标状态。

use crate::models::node_provisioning::get_node_provisioning_by_node_id;
use crate::models::node_runtime_client::NodeRuntimeClient;
use crate::models::node_sessions::get_active_session_by_node_id;
use crate::models::nodes::list_nodes;
use crate::models::upgrades::{
    UpgradeArtifactTokenRecord, UpgradeAsset, UpgradeEventRecord, UpgradePlanRecord,
    UpgradeReleaseRecord, UpgradeTargetRecord, count_targets_by_plan, count_targets_by_plan_status,
    get_artifact_token_by_hash, get_plan, get_release_by_version, insert_artifact_token,
    insert_event, insert_plan, insert_target, list_events_by_plan, list_releases,
    list_schedulable_targets, list_targets_by_plan, mark_target_failed, update_plan_status,
    update_release_suite_contract_versions, update_target_status, update_targets_status,
    upsert_release,
};
use crate::state::DbPool;
use crate::types::{ApiError, ApiResult, new_uuid_v7};
use axum::extract::Multipart;
use chrono::Utc;
use flate2::read::GzDecoder;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use vust_api::response::ApiResponse as RuntimeApiResponse;
use vust_upgrade::{
    COMPONENT_AGENT, COMPONENT_CONTROLLER, component_artifact_names, compute_sha256_hex,
    current_target_triple, extract_named_file_from_tar_gz, normalize_target_triple,
    normalize_version as normalize_semver_version, parse_checksum_text, release_package_names,
    verify_release_signature, verify_sha256,
};

/// 创建升级计划的入参。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradePlanCreatePayload {
    pub target_version: String,
    pub component: Option<String>,
    pub node_ids: Option<Vec<String>>,
    pub include_offline: Option<bool>,
    pub overwrite_same_version: Option<bool>,
    pub max_concurrency: Option<u32>,
    pub failure_threshold_percent: Option<u32>,
}

/// 套件兼容性检测范围。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteCompatibilityCheckPayload {
    pub node_ids: Option<Vec<String>>,
}

/// 已安装套件实例与目标版本的兼容状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteCompatibilityInstance {
    pub instance_id: String,
    pub suite_id: String,
    pub suite_name: String,
    pub node_id: String,
    pub node_name: String,
    pub platform_contract_version: u32,
    pub status: String,
    pub reason: String,
}

/// 目标升级版本与已安装套件的兼容性报告。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteCompatibilityReport {
    pub release_version: String,
    pub supported_suite_contract_versions: Vec<u32>,
    pub compatible: bool,
    pub summary: SuiteCompatibilitySummary,
    pub instances: Vec<SuiteCompatibilityInstance>,
}

/// 套件兼容性检测汇总。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteCompatibilitySummary {
    pub total: usize,
    pub compatible: usize,
    pub incompatible: usize,
}

#[derive(Debug, sqlx::FromRow)]
struct SuiteCompatibilityInstanceRow {
    instance_id: String,
    suite_id: String,
    suite_name: String,
    node_id: String,
    platform_contract_version: i64,
}

/// 升级操作的可信审计上下文。
#[derive(Debug, Clone)]
pub struct UpgradeAuditContext {
    pub user_id: i64,
    pub username: String,
    pub client_ip: String,
    pub trace_id: String,
}

/// 升级计划详情视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradePlanDetail {
    pub plan: UpgradePlanRecord,
    pub targets: Vec<UpgradeTargetRecord>,
    pub events: Vec<UpgradeEventRecord>,
    pub node_names: HashMap<String, String>,
}

/// 发布版本列表视图，包含后端权威升级可用性判断。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeReleaseView {
    #[serde(flatten)]
    pub release: UpgradeReleaseRecord,
    pub upgrade_eligible: bool,
    pub upgrade_block_reason: Option<String>,
}

/// 主控本地缓存后的升级制品。
#[derive(Debug, Clone)]
pub struct CachedUpgradeArtifact {
    pub path: PathBuf,
    pub file_name: String,
    pub sha256: String,
    pub signature: String,
    pub signature_path: PathBuf,
    pub target_triple: String,
}

/// Agent 升级 API 请求。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentUpgradePrepareRequest {
    plan_id: String,
    target_id: String,
    target_version: String,
    asset_url: String,
    sha256: Option<String>,
    signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentUpgradeApplyRequest {
    target_id: String,
}

/// Agent 升级状态响应。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentUpgradeState {
    current_version: String,
    target_id: Option<String>,
    target_version: Option<String>,
    status: String,
    error_detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSystemSummary {
    vust_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    prerelease: bool,
    published_at: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: Option<i64>,
    content_type: Option<String>,
}

/// 查询本地已同步发布版本。
pub async fn list_synced_releases(pool: &DbPool) -> ApiResult<Vec<UpgradeReleaseView>> {
    let releases = list_releases(pool).await?;
    Ok(releases.into_iter().map(build_release_view).collect())
}

/// 检测目标版本与当前已安装套件实例的平台契约兼容性。
pub async fn check_suite_compatibility(
    pool: &DbPool,
    version: &str,
    payload: SuiteCompatibilityCheckPayload,
) -> ApiResult<SuiteCompatibilityReport> {
    let version = normalize_version(version)?;
    let release = get_release_by_version(pool, &version)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("release {version} is not synced")))?;
    let mut supported = serde_json::from_str::<Vec<u32>>(
        &release.supported_suite_contract_versions,
    )
    .map_err(|err| ApiError::Internal(format!("failed to parse release compatibility: {err}")))?;
    if supported.is_empty() && release.source == "github" {
        supported = resolve_github_release_compatibility(pool, &release).await?;
    }
    validate_supported_suite_contract_versions(&supported)?;

    let requested_nodes = payload
        .node_ids
        .map(|values| values.into_iter().collect::<HashSet<_>>());
    let node_names = list_nodes(pool)
        .await?
        .into_iter()
        .map(|node| (node.node_id, node.name))
        .collect::<HashMap<_, _>>();
    let rows = sqlx::query_as::<_, SuiteCompatibilityInstanceRow>(
        r#"
        SELECT
            instance.instance_id,
            instance.suite_id,
            catalog.name AS suite_name,
            instance.node_id,
            instance.platform_contract_version
        FROM suite_instances AS instance
        INNER JOIN suite_catalog_items AS catalog ON catalog.suite_id = instance.suite_id
        WHERE instance.uninstalled_at IS NULL
        ORDER BY catalog.name, instance.node_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    let instances = rows
        .into_iter()
        .filter(|row| {
            requested_nodes
                .as_ref()
                .is_none_or(|node_ids| node_ids.contains(&row.node_id))
        })
        .map(|row| {
            let contract_version = u32::try_from(row.platform_contract_version).unwrap_or(0);
            let compatible = supported.contains(&contract_version);
            SuiteCompatibilityInstance {
                instance_id: row.instance_id,
                suite_id: row.suite_id,
                suite_name: row.suite_name,
                node_name: node_names
                    .get(&row.node_id)
                    .cloned()
                    .unwrap_or_else(|| row.node_id.clone()),
                node_id: row.node_id,
                platform_contract_version: contract_version,
                status: if compatible {
                    "compatible"
                } else {
                    "incompatible"
                }
                .to_string(),
                reason: if compatible {
                    "supported".to_string()
                } else {
                    "contractVersionNotSupported".to_string()
                },
            }
        })
        .collect::<Vec<_>>();
    let compatible_count = instances
        .iter()
        .filter(|instance| instance.status == "compatible")
        .count();
    let incompatible_count = instances.len() - compatible_count;

    Ok(SuiteCompatibilityReport {
        release_version: version,
        supported_suite_contract_versions: supported,
        compatible: incompatible_count == 0,
        summary: SuiteCompatibilitySummary {
            total: instances.len(),
            compatible: compatible_count,
            incompatible: incompatible_count,
        },
        instances,
    })
}

/// 下载并校验 GitHub 完整版本包，解析其签名保护的兼容声明。
async fn resolve_github_release_compatibility(
    pool: &DbPool,
    release: &UpgradeReleaseRecord,
) -> ApiResult<Vec<u32>> {
    let assets = serde_json::from_str::<Vec<UpgradeAsset>>(&release.assets)
        .map_err(|err| ApiError::Internal(format!("failed to parse release assets: {err}")))?;
    let target_triple = current_target_triple();
    let package = select_release_package_asset(&assets, target_triple)?;
    let names = release_package_names(&release.version, target_triple)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let package_bytes = download_bytes(&package.download_url).await?;
    let checksum_text =
        download_text(&signature_or_checksum_url(&assets, &names.checksum)?).await?;
    let expected = parse_checksum_text(&checksum_text, &names.package)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    verify_sha256(&package_bytes, &expected)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let package_signature =
        download_text(&signature_or_checksum_url(&assets, &names.signature)?).await?;
    verify_release_signature(&package_bytes, &package_signature)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    let package_path =
        std::env::temp_dir().join(format!("vust-release-metadata-{}.tar.gz", new_uuid_v7()));
    tokio::fs::write(&package_path, &package_bytes)
        .await
        .map_err(ApiError::Io)?;
    let metadata_result = parse_release_metadata(&package_path);
    let _ = tokio::fs::remove_file(&package_path).await;
    let metadata = metadata_result?;
    let metadata_target = normalize_target_triple(&metadata.target_triple)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    if normalize_version(&metadata.version)? != release.version || metadata_target != target_triple
    {
        return Err(ApiError::BadRequest(
            "release.json does not match the selected release package".to_string(),
        ));
    }

    let supported = metadata.compatibility.supported_suite_contract_versions;
    let serialized = serde_json::to_string(&supported).map_err(|err| {
        ApiError::Internal(format!("failed to serialize release compatibility: {err}"))
    })?;
    update_release_suite_contract_versions(pool, &release.version, &serialized).await?;
    Ok(supported)
}

/// 从 GitHub 同步发布版本元数据。
pub async fn sync_github_releases(pool: &DbPool) -> ApiResult<Vec<UpgradeReleaseView>> {
    let config = &crate::config::get().upgrade;
    let repository = config.release_repository.trim();
    if repository.is_empty() || !repository.contains('/') {
        return Err(ApiError::BadRequest(
            "upgrade.releaseRepository must use owner/repo format".to_string(),
        ));
    }

    let url = format!("https://api.github.com/repos/{repository}/releases");
    let mut request = reqwest::Client::new()
        .get(url)
        .header(reqwest::header::USER_AGENT, "vust-upgrade-manager");
    if let Some(token) = config
        .github_token
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        request = request.bearer_auth(token);
    }

    let releases = request
        .send()
        .await
        .map_err(|err| ApiError::Internal(format!("failed to sync GitHub releases: {err}")))?
        .error_for_status()
        .map_err(|err| ApiError::BadRequest(format!("GitHub release request failed: {err}")))?
        .json::<Vec<GithubRelease>>()
        .await
        .map_err(|err| ApiError::Internal(format!("failed to parse GitHub releases: {err}")))?;

    let synced_at = Utc::now().to_rfc3339();
    for release in releases {
        let version = normalize_version(&release.tag_name)?;
        let channel = if release.prerelease {
            "prerelease"
        } else {
            "stable"
        };
        if config.release_channel == "stable" && channel != "stable" {
            continue;
        }

        let raw_assets = release.assets;
        let assets = collect_release_package_assets(&version, raw_assets);
        let package_assets = assets
            .iter()
            .filter(|asset| asset.target_triple.is_some())
            .collect::<Vec<_>>();
        let checksum_status = aggregate_status(
            !package_assets.is_empty(),
            package_assets.iter().all(|asset| asset.sha256.is_some()),
        );
        let signature_status = aggregate_status(
            !package_assets.is_empty(),
            package_assets
                .iter()
                .all(|asset| asset.signature_status.as_deref() == Some("verified")),
        );
        let supported_suite_contract_versions = get_release_by_version(pool, &version)
            .await?
            .filter(|existing| existing.source == "github")
            .map(|existing| existing.supported_suite_contract_versions)
            .unwrap_or_else(|| "[]".to_string());

        upsert_release(
            pool,
            &UpgradeReleaseRecord {
                release_id: new_uuid_v7(),
                version,
                tag_name: release.tag_name,
                channel: channel.to_string(),
                source: "github".to_string(),
                release_url: release.html_url,
                assets: serde_json::to_string(&assets).unwrap_or_else(|_| "[]".to_string()),
                checksum_status: checksum_status.to_string(),
                signature_status: signature_status.to_string(),
                supported_suite_contract_versions,
                synced_at: synced_at.clone(),
                published_at: release.published_at,
                created_at: synced_at.clone(),
                updated_at: synced_at.clone(),
            },
        )
        .await?;
    }

    list_synced_releases(pool).await
}

/// 为发布版本附加当前主控是否可升级到该版本的判断。
pub fn build_release_view(release: UpgradeReleaseRecord) -> UpgradeReleaseView {
    let current_version = Version::parse(env!("CARGO_PKG_VERSION"));
    let target_version = Version::parse(&release.version);
    let (upgrade_eligible, upgrade_block_reason) = match (target_version, current_version) {
        // 项目初期放宽限制，允许用户选择等于主控版本的包，以供对齐外部节点升级
        (Ok(target), Ok(current)) if target >= current => (true, None),
        (Ok(_), Ok(_)) => (
            false,
            Some("targetVersionMustBeHigherThanCurrent".to_string()),
        ),
        (Err(_), _) => (false, Some("invalidTargetVersion".to_string())),
        (_, Err(_)) => (false, Some("invalidCurrentVersion".to_string())),
    };

    UpgradeReleaseView {
        release,
        upgrade_eligible,
        upgrade_block_reason,
    }
}

/// 创建升级计划并展开目标。
pub async fn create_plan(
    pool: &DbPool,
    payload: UpgradePlanCreatePayload,
    audit: &UpgradeAuditContext,
) -> ApiResult<UpgradePlanDetail> {
    let target_version = normalize_version(&payload.target_version)?;

    let target_semver = Version::parse(&target_version)
        .map_err(|err| ApiError::BadRequest(format!("Invalid target version format: {err}")))?;
    let current_version_str = env!("CARGO_PKG_VERSION");
    let current_semver = Version::parse(current_version_str).map_err(|err| {
        ApiError::Internal(format!(
            "Invalid current control service version format: {err}"
        ))
    })?;

    // 判断是否包含主控升级。若指定了 nodeIds 且其中不含 local 节点，则不升级主控
    let mut include_controller = false;
    if let Some(ref node_ids) = payload.node_ids {
        if node_ids.contains(&"local".to_string()) {
            include_controller = true;
        }
    } else {
        include_controller = true;
    }

    // 自动将主控加入升级：若目标版本高于当前主控运行版本，后台也自动在计划中插入主控升级目标，带上主控升级
    if target_semver > current_semver {
        include_controller = true;
    }

    if include_controller {
        // 包含主控升级时，禁止降级（目标版本必须 >= 当前主控版本）
        if target_semver < current_semver {
            return Err(ApiError::BadRequest(
                "target version must be greater than or equal to current running version when upgrading controller".to_string(),
            )
            .with_message_key("api.upgrades.downgradeForbidden"));
        }
    } else {
        // 不含主控升级时（纯外部节点精细化升级），限制目标版本绝对不能高于当前主控运行版本，保证节点版本不超越主控
        if target_semver > current_semver {
            return Err(ApiError::BadRequest(
                "agent node target version cannot be higher than current controller version"
                    .to_string(),
            )
            .with_message_key("api.upgrades.agentVersionCannotExceedController"));
        }
    }

    if get_release_by_version(pool, &target_version)
        .await?
        .is_none()
    {
        return Err(ApiError::BadRequest(format!(
            "target release {target_version} is not synced"
        )));
    }

    let component = payload
        .component
        .unwrap_or_else(|| "cluster".to_string())
        .to_lowercase();
    if component != "cluster" {
        return Err(ApiError::BadRequest(
            "component must be cluster".to_string(),
        ));
    }
    validate_release_for_plan(pool, &target_version, payload.node_ids.as_deref()).await?;

    let plan_id = new_uuid_v7();
    let include_offline = payload.include_offline.unwrap_or(true);
    let scope = json!({
        "nodeIds": payload.node_ids.clone().unwrap_or_default(),
        "includeOffline": include_offline,
    });
    let strategy = json!({
        "maxConcurrency": payload.max_concurrency.unwrap_or(3).clamp(1, 100),
        "failureThresholdPercent": payload.failure_threshold_percent.unwrap_or(20).clamp(1, 100),
        "overwriteSameVersion": payload.overwrite_same_version.unwrap_or(false),
    });
    let now = Utc::now().to_rfc3339();
    let plan = UpgradePlanRecord {
        plan_id: plan_id.clone(),
        target_version: target_version.clone(),
        component: component.clone(),
        scope: scope.to_string(),
        strategy: strategy.to_string(),
        status: "draft".to_string(),
        requested_by_user_id: audit.user_id,
        requested_by: audit.username.clone(),
        client_ip: Some(audit.client_ip.clone()),
        trace_id: Some(audit.trace_id.clone()),
        started_at: None,
        finished_at: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    insert_plan(pool, &plan).await?;

    if include_controller {
        insert_target(
            pool,
            &UpgradeTargetRecord {
                target_id: new_uuid_v7(),
                plan_id: plan_id.clone(),
                target_type: "controller".to_string(),
                node_id: None,
                current_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                target_version: target_version.clone(),
                status: "pending".to_string(),
                sub_status: None,
                retry_count: 0,
                error_detail: None,
                started_at: None,
                finished_at: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await?;
    }

    create_agent_targets(
        pool,
        &plan_id,
        &target_version,
        payload.node_ids,
        include_offline,
    )
    .await?;

    record_event(
        pool,
        &plan_id,
        None,
        "plan_created",
        "Upgrade plan created",
        json!({}),
    )
    .await?;
    detail(pool, &plan_id).await
}

async fn create_agent_targets(
    pool: &DbPool,
    plan_id: &str,
    target_version: &str,
    node_ids: Option<Vec<String>>,
    include_offline: bool,
) -> ApiResult<()> {
    let selected = node_ids.unwrap_or_default();
    let nodes = list_nodes(pool).await?;
    let now = Utc::now().to_rfc3339();
    for node in nodes {
        if node.node_id == "local" || node.status == "retired" || node.status == "draft" {
            continue;
        }
        if !selected.is_empty() && !selected.iter().any(|item| item == &node.node_id) {
            continue;
        }
        let active = get_active_session_by_node_id(pool, &node.node_id).await?;
        let status = if active.is_some() {
            "pending"
        } else if include_offline {
            "deferred"
        } else {
            continue;
        };
        insert_target(
            pool,
            &UpgradeTargetRecord {
                target_id: new_uuid_v7(),
                plan_id: plan_id.to_string(),
                target_type: "agent".to_string(),
                node_id: Some(node.node_id.clone()),
                current_version: node.metadata.parse::<serde_json::Value>().ok().and_then(
                    |value| {
                        value
                            .get("resource")
                            .and_then(|r| r.get("version"))
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    },
                ),
                target_version: target_version.to_string(),
                status: status.to_string(),
                sub_status: None,
                retry_count: 0,
                error_detail: None,
                started_at: None,
                finished_at: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await?;
    }

    Ok(())
}

async fn validate_release_for_plan(
    pool: &DbPool,
    target_version: &str,
    node_ids: Option<&[String]>,
) -> ApiResult<()> {
    let release = get_release_by_version(pool, target_version)
        .await?
        .ok_or_else(|| {
            ApiError::BadRequest(format!("target release {target_version} is not synced"))
        })?;
    if release.checksum_status != "verified" || release.signature_status != "verified" {
        return Err(ApiError::BadRequest(
            "target release checksum and signature must be verified".to_string(),
        ));
    }
    let assets = serde_json::from_str::<Vec<UpgradeAsset>>(&release.assets)
        .map_err(|err| ApiError::Internal(format!("failed to parse release assets: {err}")))?;
    ensure_release_has_target(&assets, current_target_triple())?;
    let selected = node_ids.unwrap_or_default();
    for node in list_nodes(pool).await? {
        if node.node_id == "local" || node.status == "retired" || node.status == "draft" {
            continue;
        }
        if !selected.is_empty() && !selected.iter().any(|item| item == &node.node_id) {
            continue;
        }
        let target_triple = node_target_triple_from_metadata(&node.metadata)?;
        ensure_release_has_target(&assets, &target_triple)?;
    }
    Ok(())
}

fn ensure_release_has_target(assets: &[UpgradeAsset], target_triple: &str) -> ApiResult<()> {
    if assets.iter().any(|asset| {
        asset.target_triple.as_deref() == Some(target_triple)
            && asset.sha256.is_some()
            && asset.signature_status.as_deref() == Some("verified")
    }) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "release package for targetTriple {target_triple} is not verified"
        )))
    }
}

async fn node_target_triple(pool: &DbPool, node_id: &str) -> ApiResult<String> {
    let node = list_nodes(pool)
        .await?
        .into_iter()
        .find(|node| node.node_id == node_id)
        .ok_or(ApiError::NotFound)?;
    node_target_triple_from_metadata(&node.metadata)
}

fn node_target_triple_from_metadata(metadata: &str) -> ApiResult<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(metadata).unwrap_or_else(|_| json!({}));
    let value = parsed
        .get("targetTriple")
        .or_else(|| parsed.get("target_triple"))
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| current_target_triple());
    normalize_target_triple(value).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// 启动升级计划；执行器未接入前仅进入可调度状态。
pub async fn start_plan(
    pool: &DbPool,
    plan_id: &str,
    audit: &UpgradeAuditContext,
) -> ApiResult<UpgradePlanDetail> {
    let plan = get_plan(pool, plan_id).await?.ok_or(ApiError::NotFound)?;
    if plan.status != "draft" && plan.status != "paused" {
        return Err(ApiError::BadRequest(
            "only draft or paused upgrade plan can be started".to_string(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    update_plan_audit_context(pool, plan_id, audit).await?;
    update_plan_status(pool, plan_id, "running", Some(&now), None).await?;
    record_event(
        pool,
        plan_id,
        None,
        "plan_started",
        "Upgrade plan started",
        json!({ "executor": "pending-agent-runner" }),
    )
    .await?;
    detail(pool, plan_id).await
}

/// 取消升级计划中尚未执行的目标。
pub async fn cancel_plan(
    pool: &DbPool,
    plan_id: &str,
    audit: &UpgradeAuditContext,
) -> ApiResult<UpgradePlanDetail> {
    let plan = get_plan(pool, plan_id).await?.ok_or(ApiError::NotFound)?;
    if matches!(plan.status.as_str(), "succeeded" | "failed" | "canceled") {
        return Err(ApiError::BadRequest(
            "finished upgrade plan cannot be canceled".to_string(),
        ));
    }
    let affected =
        update_targets_status(pool, plan_id, &["pending", "deferred"], "canceled").await?;
    update_plan_audit_context(pool, plan_id, audit).await?;
    let now = Utc::now().to_rfc3339();
    update_plan_status(pool, plan_id, "canceled", None, Some(&now)).await?;
    // 遵循用户规则：不再于取消计划时自动删除版本包文件，交由用户在升级页面手动按需管理和清理
    record_event(
        pool,
        plan_id,
        None,
        "plan_canceled",
        "Upgrade plan canceled",
        json!({ "canceledTargets": affected }),
    )
    .await?;
    detail(pool, plan_id).await
}

async fn update_plan_audit_context(
    pool: &DbPool,
    plan_id: &str,
    audit: &UpgradeAuditContext,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE upgrade_plans SET requested_by_user_id=?,requested_by=?,client_ip=?,trace_id=? WHERE plan_id=?",
    )
    .bind(audit.user_id)
    .bind(&audit.username)
    .bind(&audit.client_ip)
    .bind(&audit.trace_id)
    .bind(plan_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 启动在线升级后台调度器。
pub fn spawn_upgrade_scheduler(state: Arc<crate::state::AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(err) = run_scheduler_tick(&state.metadata_db).await {
                tracing::warn!("upgrade scheduler tick failed: {:?}", err);
            }
        }
    });
}

async fn run_scheduler_tick(pool: &DbPool) -> ApiResult<()> {
    let controller_not_ready: bool = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM upgrade_targets t
        INNER JOIN upgrade_plans p ON p.plan_id = t.plan_id
        WHERE p.status = 'running'
          AND t.target_type = 'controller'
          AND t.status != 'succeeded'
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0)
        > 0;

    let targets = list_schedulable_targets(pool).await?;
    for target in targets {
        if target.target_type == "controller" {
            if let Err(err) = reconcile_controller_target(pool, &target).await {
                tracing::warn!(
                    target_id = %target.target_id,
                    "controller upgrade target reconcile failed: {:?}",
                    err
                );
                let _ = mark_target_failed(pool, &target.target_id, &format!("{err:?}")).await;
                let _ = record_event(
                    pool,
                    &target.plan_id,
                    Some(&target.target_id),
                    "target_failed",
                    "Controller upgrade target failed",
                    json!({ "error": format!("{err:?}") }),
                )
                .await;
                let _ = refresh_plan_terminal_status(pool, &target.plan_id).await;
            }
            continue;
        }

        if controller_not_ready {
            continue;
        }

        if let Err(err) = reconcile_agent_target(pool, &target).await {
            tracing::warn!(
                target_id = %target.target_id,
                node_id = ?target.node_id,
                "agent upgrade target reconcile failed: {:?}",
                err
            );
            let _ = mark_target_failed(pool, &target.target_id, &format!("{err:?}")).await;
            let _ = record_event(
                pool,
                &target.plan_id,
                Some(&target.target_id),
                "target_failed",
                "Agent upgrade target failed",
                json!({ "error": format!("{err:?}") }),
            )
            .await;
            let _ = refresh_plan_terminal_status(pool, &target.plan_id).await;
        }
    }
    Ok(())
}

async fn reconcile_controller_target(pool: &DbPool, target: &UpgradeTargetRecord) -> ApiResult<()> {
    let overwrite_same_version = target_plan_overwrites_same_version(pool, &target.plan_id).await?;
    if !overwrite_same_version && env!("CARGO_PKG_VERSION") == target.target_version {
        let now = Utc::now().to_rfc3339();
        update_target_status(
            pool,
            &target.target_id,
            "succeeded",
            None,
            None,
            None,
            Some(&now),
        )
        .await?;
        record_event(
            pool,
            &target.plan_id,
            Some(&target.target_id),
            "target_succeeded",
            "Controller upgrade target succeeded (already up-to-date)",
            json!({
                "currentVersion": env!("CARGO_PKG_VERSION"),
                "targetVersion": target.target_version,
                "skipped": true
            }),
        )
        .await?;
        refresh_plan_terminal_status(pool, &target.plan_id).await?;
        return Ok(());
    }
    if target.status == "running" {
        reconcile_running_controller_target(pool, target).await?;
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();
    update_target_status(
        pool,
        &target.target_id,
        "running",
        Some("downloading"),
        None,
        Some(&now),
        None,
    )
    .await?;
    record_event(
        pool,
        &target.plan_id,
        Some(&target.target_id),
        "target_started",
        "Controller upgrade target started",
        json!({}),
    )
    .await?;

    let target_triple = current_target_triple();
    let artifact =
        ensure_artifact_cached(pool, &target.target_version, "controller", target_triple).await?;

    update_target_status(
        pool,
        &target.target_id,
        "running",
        Some("applying"),
        None,
        None,
        None,
    )
    .await?;
    apply_controller_artifact(&artifact.path).await?;

    update_target_status(
        pool,
        &target.target_id,
        "running",
        Some("restarting"),
        None,
        None,
        None,
    )
    .await?;
    record_event(
        pool,
        &target.plan_id,
        Some(&target.target_id),
        "target_apply_submitted",
        "Controller binary replaced and restart submitted",
        json!({ "artifact": artifact.file_name }),
    )
    .await?;
    Ok(())
}

async fn reconcile_running_controller_target(
    pool: &DbPool,
    target: &UpgradeTargetRecord,
) -> ApiResult<()> {
    if !controller_target_restarted_on_target_version(target) {
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();
    update_target_status(
        pool,
        &target.target_id,
        "succeeded",
        None,
        None,
        None,
        Some(&now),
    )
    .await?;
    record_event(
        pool,
        &target.plan_id,
        Some(&target.target_id),
        "target_succeeded",
        "Controller upgrade target succeeded after restart",
        json!({
            "currentVersion": env!("CARGO_PKG_VERSION"),
            "targetVersion": target.target_version,
            "recovered": true
        }),
    )
    .await?;
    refresh_plan_terminal_status(pool, &target.plan_id).await?;
    Ok(())
}

fn controller_target_restarted_on_target_version(target: &UpgradeTargetRecord) -> bool {
    target.target_type == "controller"
        && target.status == "running"
        && matches!(
            target.sub_status.as_deref(),
            Some("restarting" | "restart_scheduled")
        )
        && env!("CARGO_PKG_VERSION") == target.target_version
}

async fn apply_controller_artifact(artifact_path: &PathBuf) -> ApiResult<()> {
    let archive_bytes = tokio::fs::read(artifact_path).await.map_err(ApiError::Io)?;
    let binary = extract_named_file_from_tar_gz(&archive_bytes, "vust")
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let current = std::env::current_exe()
        .map_err(|err| ApiError::Internal(format!("failed to resolve current binary: {err}")))?;
    let backup_dir =
        PathBuf::from(&crate::config::get().upgrade.download_cache_dir).join("controller-backup");
    tokio::fs::create_dir_all(&backup_dir)
        .await
        .map_err(ApiError::Io)?;
    let backup = backup_dir.join("vust.prev");
    if backup.is_file() {
        tokio::fs::remove_file(&backup)
            .await
            .map_err(ApiError::Io)?;
    }
    tokio::fs::copy(&current, &backup)
        .await
        .map_err(ApiError::Io)?;

    let staged = current.with_file_name("vust.next");
    if staged.is_file() {
        tokio::fs::remove_file(&staged)
            .await
            .map_err(ApiError::Io)?;
    }
    let mut staged_file = tokio::fs::File::create(&staged)
        .await
        .map_err(ApiError::Io)?;
    staged_file.write_all(&binary).await.map_err(ApiError::Io)?;
    staged_file.flush().await.map_err(ApiError::Io)?;
    drop(staged_file);

    let mut permissions = tokio::fs::metadata(&staged)
        .await
        .map_err(ApiError::Io)?
        .permissions();
    permissions.set_mode(0o755);
    tokio::fs::set_permissions(&staged, permissions)
        .await
        .map_err(ApiError::Io)?;
    tokio::fs::rename(&staged, &current)
        .await
        .map_err(ApiError::Io)?;

    if should_restart_controller(&current) && crate::config::get().upgrade.controller_auto_restart {
        schedule_controller_restart();
    }
    Ok(())
}

fn should_restart_controller(current: &Path) -> bool {
    std::env::var_os("VUST_HOME").is_some() || current.starts_with("/opt/vust")
}

fn schedule_controller_restart() {
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let result = tokio::process::Command::new("systemctl")
            .args(["restart", "vust"])
            .status()
            .await;
        if let Err(err) = result {
            tracing::error!("failed to restart vust after upgrade: {}", err);
        }
    });
}

async fn reconcile_agent_target(pool: &DbPool, target: &UpgradeTargetRecord) -> ApiResult<()> {
    let node_id = target
        .node_id
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("agent upgrade target has no nodeId".to_string()))?;
    let Some(_) = get_active_session_by_node_id(pool, node_id).await? else {
        if target.status != "deferred" {
            update_target_status(pool, &target.target_id, "deferred", None, None, None, None)
                .await?;
            record_event(
                pool,
                &target.plan_id,
                Some(&target.target_id),
                "target_deferred",
                "Agent node is offline; upgrade deferred",
                json!({ "nodeId": node_id }),
            )
            .await?;
        }
        return Ok(());
    };

    let client = NodeRuntimeClient::from_node_route(pool, Some(node_id)).await?;
    let overwrite_same_version = target_plan_overwrites_same_version(pool, &target.plan_id).await?;

    if !overwrite_same_version
        && target.status == "pending"
        && let Ok(RuntimeApiResponse {
            data: Some(state), ..
        }) = client
            .get_json::<RuntimeApiResponse<AgentUpgradeState>>("/api/v1/agent/upgrade/status")
            .await
        && state.current_version == target.target_version
    {
        let now = Utc::now().to_rfc3339();
        update_target_status(
            pool,
            &target.target_id,
            "succeeded",
            None,
            None,
            None,
            Some(&now),
        )
        .await?;
        record_event(
            pool,
            &target.plan_id,
            Some(&target.target_id),
            "target_succeeded",
            "Agent upgrade target succeeded (already up-to-date)",
            json!({
                "currentVersion": state.current_version,
                "targetVersion": target.target_version,
                "skipped": true
            }),
        )
        .await?;
        refresh_plan_terminal_status(pool, &target.plan_id).await?;
        return Ok(());
    }

    if target.status == "running" {
        reconcile_running_agent_target(pool, target, &client).await?;
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();
    update_target_status(
        pool,
        &target.target_id,
        "running",
        None,
        None,
        Some(&now),
        None,
    )
    .await?;
    record_event(
        pool,
        &target.plan_id,
        Some(&target.target_id),
        "target_started",
        "Agent upgrade target started",
        json!({ "nodeId": node_id }),
    )
    .await?;

    let target_triple = node_target_triple(pool, node_id).await?;
    let artifact =
        ensure_artifact_cached(pool, &target.target_version, "agent", &target_triple).await?;
    let token = issue_artifact_download_token(
        pool,
        &target.plan_id,
        &target.target_id,
        Some(node_id),
        &target.target_version,
        "agent",
        &target_triple,
    )
    .await?;
    let asset_url = build_agent_artifact_url(
        pool,
        node_id,
        &client,
        &target.target_version,
        &target_triple,
        &token,
    )
    .await?;
    let sha256_preview: String = artifact.sha256.chars().take(12).collect();

    tracing::info!(
        plan_id = %target.plan_id,
        target_id = %target.target_id,
        node_id,
        target_version = %target.target_version,
        target_triple,
        asset_url,
        sha256 = %sha256_preview,
        "submitting agent upgrade prepare request"
    );

    let prepare: RuntimeApiResponse<AgentUpgradeState> = match client
        .post_json(
            "/api/v1/agent/upgrade/prepare",
            &AgentUpgradePrepareRequest {
                plan_id: target.plan_id.clone(),
                target_id: target.target_id.clone(),
                target_version: target.target_version.clone(),
                asset_url: asset_url.clone(),
                sha256: Some(artifact.sha256),
                signature: Some(artifact.signature),
            },
        )
        .await
    {
        Ok(response) => response,
        Err(err) => {
            let mut detail = format!("agent prepare request failed: {err}");
            if let Ok(RuntimeApiResponse {
                data: Some(state), ..
            }) = client
                .get_json::<RuntimeApiResponse<AgentUpgradeState>>("/api/v1/agent/upgrade/status")
                .await
                && let Some(agent_error) = state.error_detail
            {
                detail = format!(
                    "{detail}; agentStatus={}; agentError={agent_error}",
                    state.status
                );
            }
            tracing::error!(
                plan_id = %target.plan_id,
                target_id = %target.target_id,
                node_id,
                target_version = %target.target_version,
                target_triple,
                asset_url,
                error = %detail,
                "agent upgrade prepare request failed"
            );
            return Err(ApiError::Internal(detail));
        }
    };
    unwrap_runtime_api_response(prepare)?;
    record_event(
        pool,
        &target.plan_id,
        Some(&target.target_id),
        "target_prepared",
        "Agent upgrade binary prepared",
        json!({ "nodeId": node_id }),
    )
    .await?;

    let apply: RuntimeApiResponse<AgentUpgradeState> = client
        .post_json(
            "/api/v1/agent/upgrade/apply",
            &AgentUpgradeApplyRequest {
                target_id: target.target_id.clone(),
            },
        )
        .await
        .map_err(|err| ApiError::Internal(format!("agent apply request failed: {err}")))?;
    let state = unwrap_runtime_api_response(apply)?;
    record_event(
        pool,
        &target.plan_id,
        Some(&target.target_id),
        "target_apply_submitted",
        "Agent upgrade apply submitted",
        json!({ "nodeId": node_id, "agentStatus": state.status }),
    )
    .await?;
    Ok(())
}

async fn reconcile_running_agent_target(
    pool: &DbPool,
    target: &UpgradeTargetRecord,
    client: &NodeRuntimeClient,
) -> ApiResult<()> {
    let status: RuntimeApiResponse<AgentUpgradeState> = client
        .get_json("/api/v1/agent/upgrade/status")
        .await
        .map_err(|err| ApiError::Internal(format!("agent status request failed: {err}")))?;
    let state = unwrap_runtime_api_response(status)?;
    if state.target_id.as_deref() != Some(&target.target_id)
        || state.target_version.as_deref() != Some(&target.target_version)
    {
        return Err(ApiError::BadRequest(
            "agent upgrade state does not match target scope".to_string(),
        ));
    }
    if state.current_version == target.target_version {
        let now = Utc::now().to_rfc3339();
        update_target_status(
            pool,
            &target.target_id,
            "succeeded",
            None,
            None,
            None,
            Some(&now),
        )
        .await?;
        record_event(
            pool,
            &target.plan_id,
            Some(&target.target_id),
            "target_succeeded",
            "Agent upgrade target succeeded",
            json!({
                "currentVersion": state.current_version,
                "targetVersion": target.target_version
            }),
        )
        .await?;
        refresh_plan_terminal_status(pool, &target.plan_id).await?;
    } else if matches!(state.status.as_str(), "failed" | "rollbacked_no_restart") {
        let detail = state
            .error_detail
            .unwrap_or_else(|| "agent reported upgrade failure".to_string());
        mark_target_failed(pool, &target.target_id, &detail).await?;
        record_event(
            pool,
            &target.plan_id,
            Some(&target.target_id),
            "target_failed",
            "Agent upgrade target failed",
            json!({ "error": detail }),
        )
        .await?;
        refresh_plan_terminal_status(pool, &target.plan_id).await?;
    } else if Some(&state.status) != target.sub_status.as_ref() {
        update_target_status(
            pool,
            &target.target_id,
            "running",
            Some(&state.status),
            None,
            None,
            None,
        )
        .await?;
        record_event(
            pool,
            &target.plan_id,
            Some(&target.target_id),
            "target_progress",
            &format!("Agent upgrade target status: {}", state.status),
            json!({ "status": state.status, "nodeId": target.node_id }),
        )
        .await?;
    }
    Ok(())
}

async fn target_plan_overwrites_same_version(pool: &DbPool, plan_id: &str) -> ApiResult<bool> {
    let Some(plan) = get_plan(pool, plan_id).await? else {
        return Err(ApiError::NotFound);
    };
    let strategy =
        serde_json::from_str::<serde_json::Value>(&plan.strategy).unwrap_or_else(|_| json!({}));
    Ok(strategy
        .get("overwriteSameVersion")
        .and_then(|value| value.as_bool())
        .unwrap_or(false))
}

async fn refresh_plan_terminal_status(pool: &DbPool, plan_id: &str) -> ApiResult<()> {
    let Some(current_plan) = get_plan(pool, plan_id).await? else {
        return Err(ApiError::NotFound);
    };
    if matches!(
        current_plan.status.as_str(),
        "succeeded" | "failed" | "canceled"
    ) {
        return Ok(());
    }
    let total = count_targets_by_plan(pool, plan_id).await?;
    if total == 0 {
        return Ok(());
    }
    let succeeded = count_targets_by_plan_status(pool, plan_id, "succeeded").await?;
    let failed = count_targets_by_plan_status(pool, plan_id, "failed").await?;
    let canceled = count_targets_by_plan_status(pool, plan_id, "canceled").await?;

    let mut terminal = false;
    let mut terminal_status = "";

    if succeeded == total {
        terminal = true;
        terminal_status = "succeeded";
    } else if failed + canceled == total || failed > 0 {
        terminal = true;
        terminal_status = "failed";
    }

    if terminal {
        let now = Utc::now().to_rfc3339();
        update_plan_status(pool, plan_id, terminal_status, None, Some(&now)).await?;
        record_event(
            pool,
            plan_id,
            None,
            &format!("plan_{terminal_status}"),
            &format!("Upgrade plan {terminal_status}"),
            json!({
                "result": terminal_status,
                "targetCount": total,
                "succeededCount": succeeded,
                "failedCount": failed,
                "canceledCount": canceled,
            }),
        )
        .await?;
        // 不自动删除版本包，升级包物理文件由用户在升级页面手动按需管理和清理
    }
    Ok(())
}

fn unwrap_runtime_api_response<T>(response: RuntimeApiResponse<T>) -> ApiResult<T> {
    if !response.success {
        return Err(ApiError::BadRequest(response.message));
    }
    response
        .data
        .ok_or_else(|| ApiError::Internal("agent upgrade response is empty".to_string()))
}

async fn build_agent_artifact_url(
    pool: &DbPool,
    node_id: &str,
    client: &NodeRuntimeClient,
    version: &str,
    target_triple: &str,
    token: &str,
) -> ApiResult<String> {
    let base_url = resolve_agent_artifact_base_url(pool, node_id, client).await?;
    Ok(build_controller_artifact_url(
        &base_url,
        version,
        "agent",
        target_triple,
        token,
    ))
}

async fn resolve_agent_artifact_base_url(
    pool: &DbPool,
    node_id: &str,
    client: &NodeRuntimeClient,
) -> ApiResult<String> {
    let configured = get_node_provisioning_by_node_id(pool, node_id)
        .await?
        .and_then(|record| record.vust_url)
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty());
    if let Some(url) = configured {
        return Ok(url);
    }

    if let Ok(RuntimeApiResponse {
        data: Some(summary),
        ..
    }) = client
        .get_json::<RuntimeApiResponse<AgentSystemSummary>>("/api/v1/agent/system/summary")
        .await
        && let Some(url) = summary
            .vust_url
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
    {
        let _ = crate::services::node_provisioning::update_node_vust_url(pool, node_id, &url).await;
        return Ok(url);
    }

    Ok(crate::services::node_deploy::resolve_vust_url())
}

fn build_controller_artifact_url(
    base_url: &str,
    version: &str,
    component: &str,
    target_triple: &str,
    token: &str,
) -> String {
    format!(
        "{}/api/v1/runtime/upgrades/artifacts/{}/{}/{}/download?token={}",
        base_url.trim_end_matches('/'),
        version,
        component,
        target_triple,
        token
    )
}

/// 查询升级计划详情。
pub async fn detail(pool: &DbPool, plan_id: &str) -> ApiResult<UpgradePlanDetail> {
    let plan = get_plan(pool, plan_id).await?.ok_or(ApiError::NotFound)?;
    let targets = list_targets_by_plan(pool, plan_id).await?;
    let events = list_events_by_plan(pool, plan_id).await?;
    let node_names = list_nodes(pool)
        .await?
        .into_iter()
        .map(|node| (node.node_id, node.name))
        .collect();
    Ok(UpgradePlanDetail {
        plan,
        targets,
        events,
        node_names,
    })
}

/// 获取最近创建的升级计划及其详情。
pub async fn get_latest_plan(pool: &DbPool) -> ApiResult<Option<UpgradePlanDetail>> {
    let plan = sqlx::query_as::<_, UpgradePlanRecord>(
        "SELECT * FROM upgrade_plans ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    if let Some(p) = plan {
        let detail = detail(pool, &p.plan_id).await?;
        Ok(Some(detail))
    } else {
        Ok(None)
    }
}

/// 生成 agent 从主控下载制品的短期令牌。
pub async fn issue_artifact_download_token(
    pool: &DbPool,
    plan_id: &str,
    target_id: &str,
    node_id: Option<&str>,
    version: &str,
    component: &str,
    target_triple: &str,
) -> ApiResult<String> {
    if !matches!(component, "controller" | "agent") {
        return Err(ApiError::BadRequest(
            "component must be controller or agent".to_string(),
        ));
    }
    let raw = new_uuid_v7();
    let now = Utc::now();
    insert_artifact_token(
        pool,
        &UpgradeArtifactTokenRecord {
            token_id: new_uuid_v7(),
            token_hash: token_hash(&raw),
            plan_id: plan_id.to_string(),
            target_id: target_id.to_string(),
            node_id: node_id.map(str::to_string),
            version: normalize_version(version)?,
            component: component.to_string(),
            target_triple: target_triple.to_string(),
            expires_at: (now + chrono::Duration::minutes(30)).to_rfc3339(),
            used_at: None,
            created_at: now.to_rfc3339(),
        },
    )
    .await?;
    Ok(raw)
}

/// 校验下载令牌，确保 agent 只能下载主控授权的制品。
pub async fn validate_artifact_download_token(
    pool: &DbPool,
    raw_token: &str,
    version: &str,
    component: &str,
    target_triple: &str,
) -> ApiResult<()> {
    let token = get_artifact_token_by_hash(pool, &token_hash(raw_token))
        .await?
        .ok_or_else(|| ApiError::BadRequest("invalid artifact download token".to_string()))?;
    let expires_at = chrono::DateTime::parse_from_rfc3339(&token.expires_at)
        .map_err(|_| ApiError::BadRequest("invalid artifact token expiry".to_string()))?
        .with_timezone(&Utc);
    if expires_at <= Utc::now() {
        return Err(ApiError::BadRequest(
            "artifact download token is expired".to_string(),
        ));
    }
    if token.version != normalize_version(version)?
        || token.component != component
        || token.target_triple != target_triple
    {
        return Err(ApiError::BadRequest(
            "artifact download token scope mismatch".to_string(),
        ));
    }
    Ok(())
}

/// 确保主控本地已缓存指定升级制品；缓存缺失时由主控从 GitHub 下载。
pub async fn ensure_artifact_cached(
    pool: &DbPool,
    version: &str,
    component: &str,
    target_triple: &str,
) -> ApiResult<CachedUpgradeArtifact> {
    let version = normalize_version(version)?;
    if !matches!(component, COMPONENT_CONTROLLER | COMPONENT_AGENT) {
        return Err(ApiError::BadRequest(
            "component must be controller or agent".to_string(),
        ));
    }
    let target_triple = normalize_target_triple(target_triple)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let release = get_release_by_version(pool, &version)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("release {version} is not synced")))?;
    let assets = serde_json::from_str::<Vec<UpgradeAsset>>(&release.assets)
        .map_err(|err| ApiError::Internal(format!("failed to parse release assets: {err}")))?;

    if !component_artifact_path(&version, component, &target_triple).is_file()
        && release.source == "github"
    {
        cache_github_release_package(&version, &target_triple, &assets).await?;
    }

    let names = component_artifact_names(component, &target_triple)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let path = component_artifact_path(&version, component, &target_triple);
    let checksum_path = path.with_file_name(&names.checksum);
    let signature_path = path.with_file_name(&names.signature);
    let bytes = tokio::fs::read(&path).await.map_err(ApiError::Io)?;
    let checksum_text = tokio::fs::read_to_string(&checksum_path)
        .await
        .map_err(ApiError::Io)?;
    let expected = parse_checksum_text(&checksum_text, &names.artifact)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let sha256 =
        verify_sha256(&bytes, &expected).map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let signature = tokio::fs::read_to_string(&signature_path)
        .await
        .map_err(ApiError::Io)?;
    verify_release_signature(&bytes, &signature)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    Ok(CachedUpgradeArtifact {
        path,
        file_name: names.artifact,
        sha256,
        signature,
        signature_path,
        target_triple,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseMetadata {
    version: String,
    channel: String,
    #[serde(rename = "targetTriple")]
    target_triple: String,
    #[serde(rename = "publishedAt")]
    published_at: Option<String>,
    compatibility: ReleaseCompatibilityMetadata,
}

/// 升级版本声明支持的套件平台契约版本。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseCompatibilityMetadata {
    supported_suite_contract_versions: Vec<u32>,
}

/// 读取当前构建版本的套件平台契约支持范围。
pub(crate) fn current_supported_suite_contract_versions() -> ApiResult<Vec<u32>> {
    let compatibility: ReleaseCompatibilityMetadata = serde_json::from_str(include_str!(
        "../../../../release-compatibility.json"
    ))
    .map_err(|err| ApiError::Internal(format!("invalid release compatibility config: {err}")))?;
    validate_supported_suite_contract_versions(&compatibility.supported_suite_contract_versions)?;
    Ok(compatibility.supported_suite_contract_versions)
}

/// 校验发布版本的套件平台契约支持范围。
fn validate_supported_suite_contract_versions(versions: &[u32]) -> ApiResult<()> {
    if versions.is_empty() || versions.contains(&0) {
        return Err(ApiError::BadRequest(
            "supportedSuiteContractVersions must contain positive integers".to_string(),
        ));
    }
    let unique = versions.iter().copied().collect::<HashSet<_>>();
    if unique.len() != versions.len() {
        return Err(ApiError::BadRequest(
            "supportedSuiteContractVersions must not contain duplicates".to_string(),
        ));
    }
    Ok(())
}

fn parse_release_metadata(package_path: &std::path::Path) -> ApiResult<ReleaseMetadata> {
    let file = std::fs::File::open(package_path)
        .map_err(|err| ApiError::BadRequest(format!("Failed to open package file: {err}")))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|err| {
        ApiError::BadRequest(format!(
            "Failed to unpack tar archive (invalid gzip or corrupted format): {err}"
        ))
    })?;
    let mut metadata_content = None;
    let mut metadata_signature = None;
    for entry in entries {
        let mut entry = entry
            .map_err(|err| ApiError::BadRequest(format!("Failed to read archive entry: {err}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|err| ApiError::BadRequest(format!("Failed to read entry path: {err}")))?;
        let file_name = path.file_name().and_then(|value| value.to_str());
        if file_name == Some("release.json") {
            let mut content = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut content).map_err(|err| {
                ApiError::BadRequest(format!("Failed to read release.json content: {err}"))
            })?;
            metadata_content = Some(content);
        } else if file_name == Some("release.json.sig") {
            let mut signature = String::new();
            std::io::Read::read_to_string(&mut entry, &mut signature).map_err(|err| {
                ApiError::BadRequest(format!("Failed to read release.json signature: {err}"))
            })?;
            metadata_signature = Some(signature);
        }
    }
    let content = metadata_content
        .ok_or_else(|| ApiError::BadRequest("release.json not found in package".to_string()))?;
    let signature = metadata_signature.ok_or_else(|| {
        ApiError::BadRequest("release.json signature not found in package".to_string())
    })?;
    verify_release_signature(&content, &signature)
        .map_err(|err| ApiError::BadRequest(format!("release.json signature is invalid: {err}")))?;
    let metadata: ReleaseMetadata = serde_json::from_slice(&content).map_err(|err| {
        ApiError::BadRequest(format!(
            "Invalid release.json metadata (JSON format error): {err}"
        ))
    })?;
    validate_supported_suite_contract_versions(
        &metadata.compatibility.supported_suite_contract_versions,
    )?;
    Ok(metadata)
}

/// 上传完整版本包并写入发布版本记录。
pub async fn upload_release_package(
    pool: &DbPool,
    multipart: &mut Multipart,
) -> ApiResult<UpgradeReleaseRecord> {
    let temp_path = std::env::temp_dir().join(format!("vust-upload-{}.tar.gz", new_uuid_v7()));
    let mut has_file = false;

    while let Some(mut field) = multipart.next_field().await.map_err(ApiError::from)? {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            let file_name = field.file_name().unwrap_or_default().to_lowercase();
            if !file_name.ends_with(".tar.gz") && !file_name.ends_with(".tgz") {
                return Err(ApiError::BadRequest(
                    "Invalid package format. Only .tar.gz or .tgz files are supported.".to_string(),
                )
                .with_message_key("api.upgrades.invalidFormat"));
            }
            has_file = true;
            let mut output = tokio::fs::File::create(&temp_path)
                .await
                .map_err(ApiError::Io)?;
            while let Some(chunk) = field.chunk().await.map_err(ApiError::from)? {
                output.write_all(&chunk).await.map_err(ApiError::Io)?;
            }
            output.flush().await.map_err(ApiError::Io)?;
        }
    }

    if !has_file {
        return Err(ApiError::BadRequest("upload file is required".to_string()));
    }

    // 从大包内部解析出 release.json 元数据
    let metadata = parse_release_metadata(&temp_path).map_err(|err| {
        tracing::error!("Failed to parse release metadata: {:?}", err);
        ApiError::BadRequest(
            "Upgrade package verification failed. Please upload a valid official upgrade package."
                .to_string(),
        )
        .with_message_key("api.upgrades.verificationFailed")
    })?;
    let version = normalize_semver_version(&metadata.version).map_err(|err| {
        tracing::error!(
            "Failed to normalize SemVer version {}: {:?}",
            metadata.version,
            err
        );
        ApiError::BadRequest(
            "Upgrade package verification failed. Please upload a valid official upgrade package."
                .to_string(),
        )
        .with_message_key("api.upgrades.verificationFailed")
    })?;

    let target_semver = Version::parse(&version).map_err(|err| {
        let _ = std::fs::remove_file(&temp_path);
        tracing::error!("Failed to parse target version {}: {:?}", version, err);
        ApiError::BadRequest(
            "Upgrade package verification failed. Please upload a valid official upgrade package."
                .to_string(),
        )
        .with_message_key("api.upgrades.verificationFailed")
    })?;
    let current_version_str = env!("CARGO_PKG_VERSION");
    let current_semver = Version::parse(current_version_str).map_err(|err| {
        let _ = std::fs::remove_file(&temp_path);
        tracing::error!("Failed to parse current version: {:?}", err);
        ApiError::Internal(format!(
            "Invalid current control service version format: {err}"
        ))
    })?;

    // 允许上传与当前主控同版本（等于）的升级包，以便补充缺失的同版本包供其他节点对齐升级；仅禁止上传低于当前主控的版本（降级）
    if target_semver < current_semver {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(ApiError::BadRequest(
            "target version must be greater than or equal to current running version".to_string(),
        )
        .with_message_key("api.upgrades.downgradeForbidden"));
    }

    let channel = metadata.channel.trim().to_lowercase();
    if !matches!(channel.as_str(), "stable" | "prerelease") {
        let _ = tokio::fs::remove_file(&temp_path).await;
        tracing::error!("Invalid release channel: {}", channel);
        return Err(ApiError::BadRequest(
            "Upgrade package verification failed. Please upload a valid official upgrade package."
                .to_string(),
        )
        .with_message_key("api.upgrades.verificationFailed"));
    }
    let target_triple = normalize_target_triple(&metadata.target_triple).map_err(|err| {
        tracing::error!(
            "Failed to normalize target triple {}: {:?}",
            metadata.target_triple,
            err
        );
        ApiError::BadRequest(
            "Upgrade package verification failed. Please upload a valid official upgrade package."
                .to_string(),
        )
        .with_message_key("api.upgrades.verificationFailed")
    })?;
    let package_names = release_package_names(&version, &target_triple).map_err(|err| {
        tracing::error!("Failed to generate package names: {:?}", err);
        ApiError::BadRequest(
            "Upgrade package verification failed. Please upload a valid official upgrade package."
                .to_string(),
        )
        .with_message_key("api.upgrades.verificationFailed")
    })?;

    let bytes = tokio::fs::read(&temp_path).await.map_err(ApiError::Io)?;
    let sha256 = compute_sha256_hex(&bytes);

    // 解包并在内部依次校验其子组件包的签名
    if let Err(err) = unpack_and_cache_release_package(&version, &target_triple, &temp_path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        tracing::error!("Failed to unpack and verify release package: {:?}", err);
        return Err(ApiError::BadRequest(
            "Upgrade package verification failed. Please upload a valid official upgrade package."
                .to_string(),
        )
        .with_message_key("api.upgrades.verificationFailed"));
    }
    let _ = tokio::fs::remove_file(&temp_path).await;

    let mut assets = if let Some(existing) = get_release_by_version(pool, &version).await? {
        if existing.source == "github" {
            return Err(ApiError::BadRequest(
                "uploaded release conflicts with GitHub release source".to_string(),
            ));
        }
        let existing_supported = serde_json::from_str::<Vec<u32>>(
            &existing.supported_suite_contract_versions,
        )
        .map_err(|err| {
            ApiError::Internal(format!(
                "failed to parse existing release compatibility: {err}"
            ))
        })?;
        if existing_supported != metadata.compatibility.supported_suite_contract_versions {
            return Err(ApiError::BadRequest(
                "packages for the same release must declare identical suite contract versions"
                    .to_string(),
            ));
        }
        serde_json::from_str::<Vec<UpgradeAsset>>(&existing.assets).unwrap_or_default()
    } else {
        Vec::new()
    };
    assets.retain(|asset| asset.target_triple.as_deref() != Some(&target_triple));
    assets.push(UpgradeAsset {
        name: package_names.package.clone(),
        download_url: format!(
            "upload://{version}/{target_triple}/{}",
            package_names.package
        ),
        size: Some(bytes.len() as i64),
        content_type: Some("application/gzip".to_string()),
        component: None,
        target_triple: Some(target_triple.clone()),
        sha256: Some(sha256),
        signature_name: None, // 大包没有外部 signature 文件
        signature_status: Some("verified".to_string()),
    });

    let now = Utc::now().to_rfc3339();
    let record = UpgradeReleaseRecord {
        release_id: new_uuid_v7(),
        version: version.clone(),
        tag_name: version.clone(),
        channel,
        source: "upload".to_string(),
        release_url: format!("upload://{}", new_uuid_v7()),
        assets: serde_json::to_string(&assets).unwrap_or_else(|_| "[]".to_string()),
        checksum_status: "verified".to_string(),
        signature_status: "verified".to_string(),
        supported_suite_contract_versions: serde_json::to_string(
            &metadata.compatibility.supported_suite_contract_versions,
        )
        .map_err(|err| {
            ApiError::Internal(format!("failed to serialize release compatibility: {err}"))
        })?,
        synced_at: now.clone(),
        published_at: metadata.published_at.clone().or_else(|| Some(now.clone())),
        created_at: now.clone(),
        updated_at: now,
    };
    upsert_release(pool, &record).await?;
    get_release_by_version(pool, &version)
        .await?
        .ok_or_else(|| ApiError::Internal("uploaded release was not persisted".to_string()))
}

fn collect_release_package_assets(version: &str, assets: Vec<GithubAsset>) -> Vec<UpgradeAsset> {
    let mut by_name = assets
        .into_iter()
        .map(|asset| (asset.name.clone(), asset))
        .collect::<HashMap<_, _>>();
    let mut collected = Vec::new();
    for target_triple in ["linux-x86_64", "linux-aarch64"] {
        let Ok(names) = release_package_names(version, target_triple) else {
            continue;
        };
        if let Some(package) = by_name.remove(&names.package) {
            let checksum_exists = by_name.contains_key(&names.checksum);
            let signature_exists = by_name.contains_key(&names.signature);
            collected.push(UpgradeAsset {
                name: package.name,
                download_url: package.browser_download_url,
                size: package.size,
                content_type: package.content_type,
                component: None,
                target_triple: Some(target_triple.to_string()),
                sha256: checksum_exists.then(|| names.checksum.clone()),
                signature_name: signature_exists.then(|| names.signature.clone()),
                signature_status: Some(
                    if signature_exists {
                        "verified"
                    } else {
                        "missing"
                    }
                    .to_string(),
                ),
            });
            if let Some(checksum) = by_name.remove(&names.checksum) {
                collected.push(UpgradeAsset {
                    name: checksum.name,
                    download_url: checksum.browser_download_url,
                    size: checksum.size,
                    content_type: checksum.content_type,
                    component: None,
                    target_triple: None,
                    sha256: None,
                    signature_name: None,
                    signature_status: None,
                });
            }
            if let Some(signature) = by_name.remove(&names.signature) {
                collected.push(UpgradeAsset {
                    name: signature.name,
                    download_url: signature.browser_download_url,
                    size: signature.size,
                    content_type: signature.content_type,
                    component: None,
                    target_triple: None,
                    sha256: None,
                    signature_name: None,
                    signature_status: None,
                });
            }
        }
    }
    collected
}

fn aggregate_status(has_assets: bool, all_verified: bool) -> &'static str {
    if !has_assets {
        "missing"
    } else if all_verified {
        "verified"
    } else {
        "missing"
    }
}

async fn cache_github_release_package(
    version: &str,
    target_triple: &str,
    assets: &[UpgradeAsset],
) -> ApiResult<()> {
    let package = select_release_package_asset(assets, target_triple)?;
    let names = release_package_names(version, target_triple)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let package_bytes = download_bytes(&package.download_url).await?;
    let checksum_text = download_text(&signature_or_checksum_url(assets, &names.checksum)?).await?;
    let expected = parse_checksum_text(&checksum_text, &names.package)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    verify_sha256(&package_bytes, &expected)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let signature = download_text(&signature_or_checksum_url(assets, &names.signature)?).await?;
    verify_release_signature(&package_bytes, &signature)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let package_path = std::env::temp_dir().join(format!("vust-github-{}.tar.gz", new_uuid_v7()));
    tokio::fs::write(&package_path, &package_bytes)
        .await
        .map_err(ApiError::Io)?;
    unpack_and_cache_release_package(version, target_triple, &package_path).await?;
    let _ = tokio::fs::remove_file(&package_path).await;
    Ok(())
}

async fn unpack_and_cache_release_package(
    version: &str,
    target_triple: &str,
    package_path: &Path,
) -> ApiResult<()> {
    let version = version.to_string();
    let target_triple = target_triple.to_string();
    let package_path = package_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        unpack_and_cache_release_package_blocking(&version, &target_triple, &package_path)
    })
    .await
    .map_err(|err| ApiError::Internal(format!("failed to join unpack task: {err}")))?
}

fn unpack_and_cache_release_package_blocking(
    version: &str,
    target_triple: &str,
    package_path: &Path,
) -> ApiResult<()> {
    let file = std::fs::File::open(package_path)
        .map_err(|err| ApiError::BadRequest(format!("Failed to open package cache file: {err}")))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|err| {
        ApiError::BadRequest(format!("Failed to parse release archive content: {err}"))
    })?;
    let mut files = HashMap::<String, Vec<u8>>::new();
    for entry in entries {
        let mut entry = entry.map_err(|err| {
            ApiError::BadRequest(format!("Failed to read release archive file entry: {err}"))
        })?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|err| ApiError::BadRequest(format!("Failed to read entry path: {err}")))?;
        let Some(name) = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|err| {
            ApiError::BadRequest(format!("Failed to extract file {name} from archive: {err}"))
        })?;
        files.insert(name, bytes);
    }
    for component in [COMPONENT_CONTROLLER, COMPONENT_AGENT] {
        let names = component_artifact_names(component, target_triple)
            .map_err(|err| ApiError::BadRequest(err.to_string()))?;
        let artifact = files.get(&names.artifact).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Invalid upgrade package: missing core component file ({})",
                names.artifact
            ))
        })?;
        let checksum_text = std::str::from_utf8(files.get(&names.checksum).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Invalid upgrade package: missing checksum descriptor ({})",
                names.checksum
            ))
        })?)
        .map_err(|err| ApiError::BadRequest(format!("Invalid UTF-8 checksum: {err}")))?;
        let expected = parse_checksum_text(checksum_text, &names.artifact)
            .map_err(|err| ApiError::BadRequest(err.to_string()))?;
        verify_sha256(artifact, &expected).map_err(|err| {
            ApiError::BadRequest(format!(
                "Component checksum mismatch for {}: {err}",
                names.artifact
            ))
        })?;
        let signature = std::str::from_utf8(files.get(&names.signature).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Invalid upgrade package: missing signature descriptor ({})",
                names.signature
            ))
        })?)
        .map_err(|err| ApiError::BadRequest(format!("Invalid UTF-8 signature: {err}")))?;
        verify_release_signature(artifact, signature).map_err(|err| {
            ApiError::BadRequest(format!(
                "Component signature verification failed for {}: {err}",
                names.artifact
            ))
        })?;
        let dir = artifact_cache_dir(version, component, target_triple);
        std::fs::create_dir_all(&dir).map_err(ApiError::Io)?;
        std::fs::write(dir.join(&names.artifact), artifact).map_err(ApiError::Io)?;
        std::fs::write(dir.join(&names.checksum), checksum_text).map_err(ApiError::Io)?;
        std::fs::write(dir.join(&names.signature), signature).map_err(ApiError::Io)?;
    }
    Ok(())
}

fn select_release_package_asset<'a>(
    assets: &'a [UpgradeAsset],
    target_triple: &str,
) -> ApiResult<&'a UpgradeAsset> {
    assets
        .iter()
        .find(|asset| asset.target_triple.as_deref() == Some(target_triple))
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "release package for targetTriple {target_triple} was not found"
            ))
        })
}

fn signature_or_checksum_url(assets: &[UpgradeAsset], name: &str) -> ApiResult<String> {
    assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.download_url.clone())
        .ok_or_else(|| ApiError::BadRequest(format!("release asset {name} was not found")))
}

async fn download_bytes(url: &str) -> ApiResult<Vec<u8>> {
    let bytes = reqwest::get(url)
        .await
        .map_err(|err| ApiError::Internal(format!("failed to download asset: {err}")))?
        .error_for_status()
        .map_err(|err| ApiError::BadRequest(format!("asset download failed: {err}")))?
        .bytes()
        .await
        .map_err(|err| ApiError::Internal(format!("failed to read asset: {err}")))?;
    Ok(bytes.to_vec())
}

async fn download_text(url: &str) -> ApiResult<String> {
    reqwest::get(url)
        .await
        .map_err(|err| ApiError::Internal(format!("failed to download asset: {err}")))?
        .error_for_status()
        .map_err(|err| ApiError::BadRequest(format!("asset download failed: {err}")))?
        .text()
        .await
        .map_err(|err| ApiError::Internal(format!("failed to read asset: {err}")))
}

fn component_artifact_path(version: &str, component: &str, target_triple: &str) -> PathBuf {
    let names = component_artifact_names(component, target_triple)
        .expect("validated component and targetTriple should build artifact names");
    artifact_cache_dir(version, component, target_triple).join(names.artifact)
}

async fn record_event(
    pool: &DbPool,
    plan_id: &str,
    target_id: Option<&str>,
    event_type: &str,
    message: &str,
    metadata: serde_json::Value,
) -> ApiResult<()> {
    insert_event(
        pool,
        &UpgradeEventRecord {
            event_id: new_uuid_v7(),
            plan_id: plan_id.to_string(),
            target_id: target_id.map(str::to_string),
            event_type: event_type.to_string(),
            message: message.to_string(),
            metadata: metadata.to_string(),
            created_at: Utc::now().to_rfc3339(),
        },
    )
    .await?;

    if !matches!(
        event_type,
        "plan_created" | "plan_started" | "plan_canceled" | "plan_succeeded" | "plan_failed"
    ) {
        return Ok(());
    }

    let Some(plan) = get_plan(pool, plan_id).await.ok().flatten() else {
        tracing::error!(plan_id, event_type, "upgrade audit context is unavailable");
        return Ok(());
    };
    let Some(client_ip) = plan
        .client_ip
        .as_deref()
        .and_then(|value| value.parse::<std::net::IpAddr>().ok())
    else {
        tracing::error!(
            plan_id,
            event_type,
            "upgrade audit client IP is unavailable"
        );
        return Ok(());
    };
    let status = if event_type.contains("failed")
        || event_type.contains("error")
        || message.to_lowercase().contains("failed")
        || message.to_lowercase().contains("error")
    {
        crate::models::logging::LogStatus::Failed
    } else {
        crate::models::logging::LogStatus::Success
    };
    let outcome = if event_type == "plan_canceled" {
        vust_contracts::logging::OperationOutcome::Canceled
    } else if event_type == "plan_failed"
        && metadata
            .get("succeededCount")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default()
            > 0
    {
        vust_contracts::logging::OperationOutcome::Partial
    } else if status == crate::models::logging::LogStatus::Failed {
        vust_contracts::logging::OperationOutcome::Failure
    } else {
        vust_contracts::logging::OperationOutcome::Success
    };

    let mut log_meta = serde_json::Map::new();
    log_meta.insert(
        "message".to_string(),
        serde_json::Value::String(message.to_string()),
    );
    log_meta.insert(
        "planId".to_string(),
        serde_json::Value::String(plan_id.to_string()),
    );
    if let Some(tid) = target_id {
        log_meta.insert(
            "targetId".to_string(),
            serde_json::Value::String(tid.to_string()),
        );
    }
    if let serde_json::Value::Object(map) = metadata {
        for (k, v) in map {
            log_meta.insert(k, v);
        }
    }

    let mut operation = crate::services::logging::OperationEventBuilder::new(
        &plan.requested_by,
        &format!("upgrade_{}", event_type),
        client_ip,
    )
    .user_id(plan.requested_by_user_id);
    if let Some(trace_id) = plan.trace_id.as_deref() {
        operation = operation.trace_id(trace_id);
    }
    crate::services::logging::persist_event(
        operation
            .module(crate::models::logging::LogModule::System)
            .target_type("upgrade_plan")
            .target_id(plan_id)
            .outcome(outcome)
            .metadata(serde_json::Value::Object(log_meta)),
        pool,
    )
    .await?;

    Ok(())
}

fn artifact_cache_dir(version: &str, component: &str, target_triple: &str) -> PathBuf {
    PathBuf::from(&crate::config::get().upgrade.download_cache_dir)
        .join(version)
        .join(component)
        .join(target_triple)
}

fn normalize_version(value: &str) -> ApiResult<String> {
    normalize_semver_version(value).map_err(|err| ApiError::BadRequest(err.to_string()))
}

fn token_hash(token: &str) -> String {
    compute_sha256_hex(token.as_bytes())
}

/// 删除版本包及其在主控本地的二进制文件缓存目录。
pub async fn delete_release(pool: &DbPool, version: &str) -> ApiResult<()> {
    crate::models::upgrades::delete_release_by_version(pool, version)
        .await
        .map_err(ApiError::from)?;

    let cache_dir = PathBuf::from(&crate::config::get().upgrade.download_cache_dir).join(version);
    if cache_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&cache_dir).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::upgrades::{
        get_plan, insert_plan, insert_target, list_events_by_plan, list_targets_by_plan,
    };

    fn upgrade_plan(plan_id: &str, target_version: &str) -> UpgradePlanRecord {
        let now = Utc::now().to_rfc3339();
        UpgradePlanRecord {
            plan_id: plan_id.to_string(),
            target_version: target_version.to_string(),
            component: "cluster".to_string(),
            scope: "{}".to_string(),
            strategy: json!({ "overwriteSameVersion": true }).to_string(),
            status: "running".to_string(),
            requested_by_user_id: 1,
            requested_by: "tester".to_string(),
            client_ip: Some("127.0.0.1".to_string()),
            trace_id: Some("upgrade-test-trace".to_string()),
            started_at: Some(now.clone()),
            finished_at: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    fn controller_target(
        plan_id: &str,
        target_id: &str,
        status: &str,
        sub_status: Option<&str>,
        target_version: &str,
    ) -> UpgradeTargetRecord {
        let now = Utc::now().to_rfc3339();
        UpgradeTargetRecord {
            target_id: target_id.to_string(),
            plan_id: plan_id.to_string(),
            target_type: "controller".to_string(),
            node_id: None,
            current_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            target_version: target_version.to_string(),
            status: status.to_string(),
            sub_status: sub_status.map(str::to_string),
            retry_count: 0,
            error_detail: None,
            started_at: Some(now.clone()),
            finished_at: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[test]
    fn current_release_starts_with_suite_contract_version_one() {
        assert_eq!(
            current_supported_suite_contract_versions().unwrap(),
            vec![1]
        );
        assert!(validate_supported_suite_contract_versions(&[1]).is_ok());
        assert!(validate_supported_suite_contract_versions(&[]).is_err());
        assert!(validate_supported_suite_contract_versions(&[0]).is_err());
        assert!(validate_supported_suite_contract_versions(&[1, 1]).is_err());
    }

    #[tokio::test]
    async fn reports_installed_suite_contract_incompatibility_and_honors_node_scope() {
        let pool = crate::test_support::setup_test_db().await;
        let now = Utc::now().to_rfc3339();
        upsert_release(
            &pool,
            &UpgradeReleaseRecord {
                release_id: "release-contract-test".to_string(),
                version: "9.0.0".to_string(),
                tag_name: "v9.0.0".to_string(),
                channel: "stable".to_string(),
                source: "upload".to_string(),
                release_url: "upload://contract-test".to_string(),
                assets: "[]".to_string(),
                checksum_status: "verified".to_string(),
                signature_status: "verified".to_string(),
                supported_suite_contract_versions: "[1]".to_string(),
                synced_at: now.clone(),
                published_at: Some(now.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO suite_catalog_items (
                suite_id, version, name, manifest_json, package_json, checksum
            ) VALUES ('suite-contract-test', '1.0.0', '契约测试套件', '{}', '{}', 'checksum')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        for (instance_id, node_id, contract_version) in [
            ("instance-compatible", "local", 1),
            ("instance-incompatible", "node-2", 2),
        ] {
            sqlx::query(
                r#"
                INSERT INTO suite_instances (
                    instance_id, suite_id, version, node_id, compose_project_name, status,
                    platform_contract_version
                ) VALUES (?, 'suite-contract-test', '1.0.0', ?, ?, 'installed', ?)
                "#,
            )
            .bind(instance_id)
            .bind(node_id)
            .bind(format!("contract-test-{node_id}"))
            .bind(contract_version)
            .execute(&pool)
            .await
            .unwrap();
        }

        let report =
            check_suite_compatibility(&pool, "9.0.0", SuiteCompatibilityCheckPayload::default())
                .await
                .unwrap();
        assert!(!report.compatible);
        assert_eq!(report.summary.total, 2);
        assert_eq!(report.summary.incompatible, 1);
        assert_eq!(report.instances[1].reason, "contractVersionNotSupported");

        let local_report = check_suite_compatibility(
            &pool,
            "9.0.0",
            SuiteCompatibilityCheckPayload {
                node_ids: Some(vec!["local".to_string()]),
            },
        )
        .await
        .unwrap();
        assert!(local_report.compatible);
        assert_eq!(local_report.summary.total, 1);
    }

    #[tokio::test]
    async fn running_controller_target_recovers_after_restart() {
        let pool = crate::test_support::setup_test_db().await;
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'tester', 'hash')")
            .execute(&pool)
            .await
            .unwrap();
        let plan_id = "plan-controller-restart";
        let target_id = "target-controller-restart";
        let target_version = env!("CARGO_PKG_VERSION");
        let plan = upgrade_plan(plan_id, target_version);
        let target = controller_target(
            plan_id,
            target_id,
            "running",
            Some("restarting"),
            target_version,
        );

        insert_plan(&pool, &plan).await.unwrap();
        insert_target(&pool, &target).await.unwrap();

        reconcile_controller_target(&pool, &target).await.unwrap();

        let targets = list_targets_by_plan(&pool, plan_id).await.unwrap();
        let recovered = targets
            .iter()
            .find(|item| item.target_id == target_id)
            .expect("controller target should exist");
        assert_eq!(recovered.status, "succeeded");
        assert!(recovered.sub_status.is_none());
        assert!(recovered.finished_at.is_some());

        let updated_plan = get_plan(&pool, plan_id).await.unwrap().unwrap();
        assert_eq!(updated_plan.status, "succeeded");
        assert!(updated_plan.finished_at.is_some());

        let events = list_events_by_plan(&pool, plan_id).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "target_succeeded")
        );
    }

    #[test]
    fn controller_recovery_requires_restart_sub_status_and_matching_version() {
        let target_version = env!("CARGO_PKG_VERSION");
        let recovering = controller_target(
            "plan",
            "target",
            "running",
            Some("restarting"),
            target_version,
        );
        assert!(controller_target_restarted_on_target_version(&recovering));

        let applying = controller_target(
            "plan",
            "target",
            "running",
            Some("applying"),
            target_version,
        );
        assert!(!controller_target_restarted_on_target_version(&applying));

        let other_version =
            controller_target("plan", "target", "running", Some("restarting"), "0.0.0");
        assert!(!controller_target_restarted_on_target_version(
            &other_version
        ));
    }
}
