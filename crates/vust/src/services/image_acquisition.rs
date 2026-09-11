//! 节点 Docker 镜像获取编排：目标复用、主控分发与仓库回退。

use crate::models::NodeRuntimeClient;
use crate::models::logging::LogModule;
use crate::services::logging::OperationEventBuilder;
use crate::state::AppState;
use anyhow::{Context, anyhow};
use bollard::Docker;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use uuid::Uuid;
use vust_contracts::logging::OperationOutcome;

const AGENT_AVAILABILITY_PATH: &str = "/api/v1/agent/docker/images/availability";
const AGENT_LOAD_PATH: &str = "/api/v1/agent/docker/images/load";
const AGENT_PULL_TASKS_PATH: &str = "/api/v1/agent/docker/image-pull-tasks";
const AGENT_OPERATION_SOURCE: &str = "image-acquisition";
const TASK_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_DISTRIBUTION_ERROR_CHARS: usize = 512;

/// 镜像任务状态。
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageTaskStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
}

/// 镜像最终来源。
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageSource {
    Target,
    Controller,
    Registry,
}

/// 镜像任务当前阶段。
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageStage {
    Checking,
    Exporting,
    Uploading,
    Loading,
    Pulling,
}

/// Agent 仓库拉取任务返回的单个镜像分层快照。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageLayerProgress {
    pub id: String,
    pub status: Option<String>,
    pub progress_percent: Option<u8>,
}

/// 主控统一镜像任务快照。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageTask {
    pub task_id: String,
    pub node_id: String,
    pub image_ref: String,
    pub status: ImageTaskStatus,
    pub source: Option<ImageSource>,
    pub stage: ImageStage,
    pub progress_percent: u8,
    pub status_text: String,
    pub layers: Vec<ImageLayerProgress>,
    pub controller_error: Option<String>,
    pub registry_error: Option<String>,
    #[serde(skip)]
    actor: Option<ImageTaskActor>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
    #[serde(skip)]
    finished_at: Option<Instant>,
}

/// 镜像后台任务持久到终态日志的可信发起人。
#[derive(Clone, Debug)]
pub struct ImageTaskActor {
    pub user_id: i64,
    pub name: String,
    pub client_ip: IpAddr,
    pub trace_id: String,
}

/// 单个目标节点的镜像分发状态。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDistributionTarget {
    pub node_id: String,
    pub node_name: String,
    pub status: ImageTaskStatus,
    pub source: Option<ImageSource>,
    pub stage: ImageStage,
    pub progress_percent: u8,
    pub error_summary: Option<String>,
}

/// 主控镜像批量分发任务快照。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDistributionTask {
    pub task_id: String,
    pub image_ref: String,
    pub status: ImageTaskStatus,
    pub created_at: i64,
    pub targets: Vec<ImageDistributionTarget>,
}

#[derive(Clone, Debug)]
struct ImageDistributionRecord {
    task_id: String,
    image_ref: String,
    created_at: i64,
    created_instant: Instant,
    targets: Vec<ImageDistributionRecordTarget>,
}

#[derive(Clone, Debug)]
struct ImageDistributionRecordTarget {
    node_id: String,
    node_name: String,
    child_task_id: String,
}

#[derive(Default)]
struct AcquisitionGate {
    running: bool,
    notify: Arc<Notify>,
}

/// 可克隆的镜像获取服务。
#[derive(Clone, Default)]
pub struct ImageAcquisitionService {
    tasks: Arc<Mutex<HashMap<String, ImageTask>>>,
    distributions: Arc<Mutex<HashMap<String, ImageDistributionRecord>>>,
    gates: Arc<AsyncMutex<HashMap<String, AcquisitionGate>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentAvailability {
    image_ref: String,
    available: bool,
}

impl ImageAcquisitionService {
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建后台镜像获取任务。
    pub fn start(
        &self,
        state: Arc<AppState>,
        node_id: String,
        image_ref: String,
        actor: Option<ImageTaskActor>,
    ) -> ImageTask {
        self.cleanup();
        let task = ImageTask {
            task_id: Uuid::new_v4().to_string(),
            node_id,
            image_ref,
            status: ImageTaskStatus::Pending,
            source: None,
            stage: ImageStage::Checking,
            progress_percent: 0,
            status_text: "Waiting to acquire image".to_string(),
            layers: Vec::new(),
            controller_error: None,
            registry_error: None,
            actor,
            cancel: Arc::new(AtomicBool::new(false)),
            finished_at: None,
        };
        self.tasks
            .lock()
            .expect("image task lock poisoned")
            .insert(task.task_id.clone(), task.clone());
        let service = self.clone();
        let task_id = task.task_id.clone();
        tokio::spawn(async move {
            service.run_task(state, &task_id).await;
        });
        task
    }

    pub fn get(&self, task_id: &str) -> Option<ImageTask> {
        self.tasks
            .lock()
            .expect("image task lock poisoned")
            .get(task_id)
            .cloned()
    }

    pub fn cancel(&self, task_id: &str) -> Option<ImageTask> {
        let mut tasks = self.tasks.lock().expect("image task lock poisoned");
        let task = tasks.get_mut(task_id)?;
        if matches!(
            task.status,
            ImageTaskStatus::Pending | ImageTaskStatus::Running
        ) {
            task.cancel.store(true, Ordering::Relaxed);
            task.status_text = "Cancelling image acquisition".to_string();
        }
        Some(task.clone())
    }

    /// 在全部目标完成校验后一次性创建批量分发任务。
    pub fn start_distribution(
        &self,
        state: Arc<AppState>,
        image_ref: String,
        targets: Vec<(String, String)>,
        actor: ImageTaskActor,
    ) -> ImageDistributionTask {
        self.cleanup();
        let task_id = Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().timestamp();
        let children = targets
            .into_iter()
            .map(|(node_id, node_name)| {
                let child = self.start(
                    Arc::clone(&state),
                    node_id.clone(),
                    image_ref.clone(),
                    Some(actor.clone()),
                );
                ImageDistributionRecordTarget {
                    node_id,
                    node_name,
                    child_task_id: child.task_id,
                }
            })
            .collect::<Vec<_>>();
        let record = ImageDistributionRecord {
            task_id: task_id.clone(),
            image_ref,
            created_at,
            created_instant: Instant::now(),
            targets: children,
        };
        self.distributions
            .lock()
            .expect("image distribution lock poisoned")
            .insert(task_id.clone(), record);
        self.get_distribution(&task_id)
            .expect("new image distribution task must exist")
    }

    /// 读取批量分发任务的聚合快照。
    pub fn get_distribution(&self, task_id: &str) -> Option<ImageDistributionTask> {
        self.cleanup();
        let record = self
            .distributions
            .lock()
            .expect("image distribution lock poisoned")
            .get(task_id)
            .cloned()?;
        Some(self.distribution_snapshot(&record))
    }

    /// 返回保留期内的批量分发任务，最新任务排在最前。
    pub fn recent_distributions(&self) -> Vec<ImageDistributionTask> {
        self.cleanup();
        let mut records = self
            .distributions
            .lock()
            .expect("image distribution lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.task_id.cmp(&left.task_id))
        });
        records
            .iter()
            .map(|record| self.distribution_snapshot(record))
            .collect()
    }

    /// 请求取消批量任务中尚未结束的全部目标。
    pub fn cancel_distribution(&self, task_id: &str) -> Option<ImageDistributionTask> {
        let child_ids = self
            .distributions
            .lock()
            .expect("image distribution lock poisoned")
            .get(task_id)?
            .targets
            .iter()
            .map(|target| target.child_task_id.clone())
            .collect::<Vec<_>>();
        for child_id in child_ids {
            self.cancel(&child_id);
        }
        self.get_distribution(task_id)
    }

    fn distribution_snapshot(&self, record: &ImageDistributionRecord) -> ImageDistributionTask {
        let tasks = self.tasks.lock().expect("image task lock poisoned");
        let targets = record
            .targets
            .iter()
            .filter_map(|target| {
                let task = tasks.get(&target.child_task_id)?;
                Some(ImageDistributionTarget {
                    node_id: target.node_id.clone(),
                    node_name: target.node_name.clone(),
                    status: task.status,
                    source: task.source,
                    stage: task.stage,
                    progress_percent: task.progress_percent,
                    error_summary: (task.status == ImageTaskStatus::Failed)
                        .then(|| sanitize_distribution_error(&task.status_text)),
                })
            })
            .collect::<Vec<_>>();
        ImageDistributionTask {
            task_id: record.task_id.clone(),
            image_ref: record.image_ref.clone(),
            status: distribution_status(&targets),
            created_at: record.created_at,
            targets,
        }
    }

    /// 同步确保镜像在节点可用，供套件安装流程复用。
    pub async fn acquire(
        &self,
        state: Arc<AppState>,
        node_id: &str,
        image_ref: &str,
    ) -> anyhow::Result<ImageSource> {
        self.acquire_inner(
            state,
            node_id,
            image_ref,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
    }

    async fn run_task(&self, state: Arc<AppState>, task_id: &str) {
        let Some(task) = self.get(task_id) else {
            return;
        };
        self.update(task_id, |task| {
            task.status = ImageTaskStatus::Running;
            task.progress_percent = 2;
        });
        let result = self
            .acquire_inner(
                Arc::clone(&state),
                &task.node_id,
                &task.image_ref,
                Arc::clone(&task.cancel),
                Some(task_id),
            )
            .await;
        self.update(task_id, |task| {
            task.finished_at = Some(Instant::now());
            match result {
                Ok(source) => {
                    task.status = ImageTaskStatus::Success;
                    task.source = Some(source);
                    task.progress_percent = 100;
                    task.status_text = "Image is ready".to_string();
                }
                Err(_) if task.cancel.load(Ordering::Relaxed) => {
                    task.status = ImageTaskStatus::Cancelled;
                    task.status_text = "Image acquisition cancelled".to_string();
                }
                Err(err) => {
                    task.status = ImageTaskStatus::Failed;
                    task.progress_percent = task.progress_percent.min(99);
                    task.status_text = err.to_string();
                }
            }
        });
        if let Some(task) = self.get(task_id)
            && task.actor.is_some()
        {
            let (event, message_key) = image_log_descriptor(&task.status, task.source.as_ref());
            let actor_name = task
                .actor
                .as_ref()
                .map_or("system", |actor| actor.name.as_str());
            let client_ip = task
                .actor
                .as_ref()
                .map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |actor| actor.client_ip);
            let mut log = OperationEventBuilder::new(actor_name, event, client_ip)
                .module(LogModule::Docker)
                .target_type("docker_image")
                .target_id(&task.image_ref)
                .task_id(&task.task_id)
                .metadata(serde_json::json!({
                    "message_key": message_key,
                    "node_id": task.node_id,
                    "image_ref": task.image_ref,
                    "source": task.source,
                    "stage": task.stage,
                    "controller_error": task.controller_error,
                    "registry_error": task.registry_error,
                }));
            if let Some(actor) = task.actor.as_ref() {
                log = log.user_id(actor.user_id).trace_id(&actor.trace_id);
            }
            log = log.outcome(image_log_outcome(task.status));
            log.finish(&state.metadata_db);
        }
    }

    async fn acquire_inner(
        &self,
        state: Arc<AppState>,
        node_id: &str,
        image_ref: &str,
        cancel: Arc<AtomicBool>,
        task_id: Option<&str>,
    ) -> anyhow::Result<ImageSource> {
        let key = format!("{node_id}\0{image_ref}");
        loop {
            let notify = {
                let mut gates = self.gates.lock().await;
                let gate = gates.entry(key.clone()).or_default();
                if gate.running {
                    Some(Arc::clone(&gate.notify))
                } else {
                    gate.running = true;
                    None
                }
            };
            if let Some(notify) = notify {
                notify.notified().await;
                if cancel.load(Ordering::Relaxed) {
                    return Err(anyhow!("image acquisition cancelled"));
                }
                continue;
            }
            break;
        }

        let result = self
            .acquire_exclusive(&state, node_id, image_ref, &cancel, task_id)
            .await;
        let mut gates = self.gates.lock().await;
        if let Some(gate) = gates.remove(&key) {
            gate.notify.notify_waiters();
        }
        result
    }

    async fn acquire_exclusive(
        &self,
        state: &Arc<AppState>,
        node_id: &str,
        image_ref: &str,
        cancel: &Arc<AtomicBool>,
        task_id: Option<&str>,
    ) -> anyhow::Result<ImageSource> {
        self.set_stage(task_id, ImageStage::Checking, 5, "Checking target image");
        let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(node_id)).await?;
        let trace_id = task_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if image_available(&client, image_ref).await? {
            return Ok(ImageSource::Target);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("image acquisition cancelled"));
        }

        let controller = local_docker().await;
        if let Ok(docker) = controller
            && docker.inspect_image(image_ref).await.is_ok()
        {
            self.set_stage(
                task_id,
                ImageStage::Exporting,
                10,
                "Exporting controller image",
            );
            let service = self.clone();
            let progress_task_id = task_id.map(ToOwned::to_owned);
            match transfer_controller_image(
                docker,
                &client,
                image_ref,
                Arc::clone(cancel),
                &trace_id,
                move |progress| {
                    service.set_stage(
                        progress_task_id.as_deref(),
                        ImageStage::Uploading,
                        progress,
                        "Transferring controller image",
                    );
                },
            )
            .await
            {
                Ok(()) => {
                    self.set_stage(task_id, ImageStage::Loading, 75, "Image loaded on target");
                    return Ok(ImageSource::Controller);
                }
                Err(err) if cancel.load(Ordering::Relaxed) => return Err(err),
                Err(err) => {
                    tracing::warn!(node_id, image_ref, error = %err, "controller image transfer failed; falling back to registry");
                    self.update_optional(task_id, |task| {
                        task.controller_error = Some(err.to_string())
                    });
                }
            }
        }

        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("image acquisition cancelled"));
        }
        self.set_stage(
            task_id,
            ImageStage::Pulling,
            80,
            "Pulling image from registry",
        );
        match pull_from_registry(
            &client,
            image_ref,
            cancel,
            &trace_id,
            |progress, text, layers| {
                self.update_optional(task_id, |task| {
                    task.progress_percent = (80 + progress / 5).min(99);
                    task.status_text = text;
                    task.layers = layers;
                });
            },
        )
        .await
        {
            Ok(()) => Ok(ImageSource::Registry),
            Err(err) => {
                self.update_optional(task_id, |task| task.registry_error = Some(err.to_string()));
                let controller_error = task_id
                    .and_then(|id| self.get(id))
                    .and_then(|task| task.controller_error);
                Err(match controller_error {
                    Some(first) => {
                        anyhow!("controller transfer failed: {first}; registry pull failed: {err}")
                    }
                    None => anyhow!("registry pull failed: {err}"),
                })
            }
        }
    }

    fn set_stage(&self, task_id: Option<&str>, stage: ImageStage, progress: u8, text: &str) {
        self.update_optional(task_id, |task| {
            task.stage = stage;
            task.progress_percent = task.progress_percent.max(progress);
            task.status_text = text.to_string();
        });
    }
    fn update_optional(&self, task_id: Option<&str>, update: impl FnOnce(&mut ImageTask)) {
        if let Some(id) = task_id {
            self.update(id, update);
        }
    }
    fn update(&self, task_id: &str, update: impl FnOnce(&mut ImageTask)) {
        if let Some(task) = self
            .tasks
            .lock()
            .expect("image task lock poisoned")
            .get_mut(task_id)
        {
            update(task);
        }
    }
    fn cleanup(&self) {
        self.tasks
            .lock()
            .expect("image task lock poisoned")
            .retain(|_, task| {
                task.finished_at
                    .is_none_or(|time| time.elapsed() < TASK_TTL)
            });
        self.distributions
            .lock()
            .expect("image distribution lock poisoned")
            .retain(|_, task| task.created_instant.elapsed() < TASK_TTL);
    }
}

/// 根据全部目标状态计算批量任务终态。
fn distribution_status(targets: &[ImageDistributionTarget]) -> ImageTaskStatus {
    if targets.iter().any(|target| {
        matches!(
            target.status,
            ImageTaskStatus::Pending | ImageTaskStatus::Running
        )
    }) {
        ImageTaskStatus::Running
    } else if targets
        .iter()
        .any(|target| target.status == ImageTaskStatus::Failed)
    {
        ImageTaskStatus::Failed
    } else if targets
        .iter()
        .any(|target| target.status == ImageTaskStatus::Cancelled)
    {
        ImageTaskStatus::Cancelled
    } else {
        ImageTaskStatus::Success
    }
}

/// 截断并脱敏可返回前端的分发错误摘要。
fn sanitize_distribution_error(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .take(MAX_DISTRIBUTION_ERROR_CHARS)
        .collect::<String>();
    for key in ["password", "token", "authorization"] {
        sanitized = redact_error_assignment(&sanitized, key);
    }
    sanitized
        .chars()
        .take(MAX_DISTRIBUTION_ERROR_CHARS)
        .collect()
}

fn redact_error_assignment(value: &str, key: &str) -> String {
    let mut output = value.to_string();
    let mut search_from = 0;
    loop {
        let lower = output.to_ascii_lowercase();
        let Some(relative) = lower[search_from..].find(key) else {
            break;
        };
        let start = search_from + relative + key.len();
        let Some(separator) = lower[start..].find(['=', ':']) else {
            break;
        };
        let value_start = start + separator + 1;
        let value_end = lower[value_start..]
            .find([',', '&', ' ', '\n'])
            .map(|offset| value_start + offset)
            .unwrap_or(output.len());
        output.replace_range(value_start..value_end, "[REDACTED]");
        search_from = value_start + "[REDACTED]".len();
        if search_from >= output.len() {
            break;
        }
    }
    output
}

fn image_log_descriptor(
    status: &ImageTaskStatus,
    source: Option<&ImageSource>,
) -> (&'static str, &'static str) {
    match (status, source) {
        (ImageTaskStatus::Success, Some(ImageSource::Target)) => (
            "docker_image_reused_on_target",
            "platformLog.docker.imageAcquisition.targetReused",
        ),
        (ImageTaskStatus::Success, Some(ImageSource::Controller)) => (
            "docker_image_transferred_from_controller",
            "platformLog.docker.imageAcquisition.controllerTransferred",
        ),
        (ImageTaskStatus::Success, Some(ImageSource::Registry)) => (
            "docker_image_pulled_from_registry",
            "platformLog.docker.imageAcquisition.registryPulled",
        ),
        (ImageTaskStatus::Cancelled, _) => (
            "docker_image_acquisition_cancelled",
            "platformLog.docker.imageAcquisition.cancelled",
        ),
        _ => (
            "docker_image_acquisition_failed",
            "platformLog.docker.imageAcquisition.failed",
        ),
    }
}

/// 将镜像任务终态映射为统一操作结果。
const fn image_log_outcome(status: ImageTaskStatus) -> OperationOutcome {
    match status {
        ImageTaskStatus::Success => OperationOutcome::Success,
        ImageTaskStatus::Cancelled => OperationOutcome::Canceled,
        ImageTaskStatus::Pending | ImageTaskStatus::Running | ImageTaskStatus::Failed => {
            OperationOutcome::Failure
        }
    }
}

async fn local_docker() -> anyhow::Result<Docker> {
    let docker =
        Docker::connect_with_local_defaults().context("controller Docker is unavailable")?;
    docker
        .negotiate_version()
        .await
        .context("failed to negotiate controller Docker API version")
}

async fn image_available(client: &NodeRuntimeClient, image_ref: &str) -> anyhow::Result<bool> {
    let response: Value = client
        .post_json(
            AGENT_AVAILABILITY_PATH,
            &serde_json::json!({"images": [image_ref]}),
        )
        .await?;
    let items: Vec<AgentAvailability> =
        serde_json::from_value(response.get("data").cloned().unwrap_or(Value::Null))?;
    Ok(items
        .first()
        .is_some_and(|item| item.image_ref == image_ref && item.available))
}

async fn transfer_controller_image(
    docker: Docker,
    client: &NodeRuntimeClient,
    image_ref: &str,
    cancel: Arc<AtomicBool>,
    trace_id: &str,
    progress: impl Fn(u8) + Send + Sync + 'static,
) -> anyhow::Result<()> {
    let stream = docker
        .export_image(image_ref)
        .scan(0_u64, move |sent, chunk| {
            let cancel = Arc::clone(&cancel);
            let result = if cancel.load(Ordering::Relaxed) {
                Err(std::io::Error::other("image acquisition cancelled"))
            } else {
                chunk
                    .inspect(|bytes| {
                        *sent += bytes.len() as u64;
                        progress((15 + ((*sent / (1024 * 1024)).min(45)) as u8).min(60));
                    })
                    .map_err(std::io::Error::other)
            };
            futures_util::future::ready(Some(result))
        });
    let part = reqwest::multipart::Part::stream(reqwest::Body::wrap_stream(stream))
        .file_name("image.tar")
        .mime_str("application/x-tar")?;
    let request = client
        .client
        .post(client.build_uri(AGENT_LOAD_PATH))
        .header("x-vust-image-ref", image_ref)
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .timeout(Duration::from_secs(600));
    let response = client
        .with_system_operation_context(request, AGENT_OPERATION_SOURCE, trace_id)
        .send()
        .await?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(anyhow!(
            "Agent image load failed: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ))
    }
}

async fn pull_from_registry(
    client: &NodeRuntimeClient,
    image_ref: &str,
    cancel: &Arc<AtomicBool>,
    trace_id: &str,
    progress: impl Fn(u8, String, Vec<ImageLayerProgress>),
) -> anyhow::Result<()> {
    let response: Value = client
        .post_json_with_system_context(
            AGENT_PULL_TASKS_PATH,
            &serde_json::json!({"imageName": image_ref}),
            AGENT_OPERATION_SOURCE,
            trace_id,
        )
        .await?;
    let task_id = response
        .pointer("/data/taskId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Agent did not return image pull task id"))?
        .to_string();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _: Value = client
                .delete_json_with_system_context(
                    &format!("{AGENT_PULL_TASKS_PATH}/{task_id}"),
                    AGENT_OPERATION_SOURCE,
                    trace_id,
                )
                .await?;
            return Err(anyhow!("image acquisition cancelled"));
        }
        let value: Value = client
            .get_json(&format!("{AGENT_PULL_TASKS_PATH}/{task_id}/progress"))
            .await?;
        let data = value
            .get("data")
            .ok_or_else(|| anyhow!("Agent image pull progress missing data"))?;
        let percent = data
            .get("progressPercent")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u8;
        let text = data
            .get("statusText")
            .and_then(Value::as_str)
            .unwrap_or("Pulling image from registry")
            .to_string();
        let layers = parse_agent_pull_layers(data)?;
        progress(percent, text, layers);
        match data.get("status").and_then(Value::as_str) {
            Some("success") => return Ok(()),
            Some("failed") => {
                return Err(anyhow!(
                    data.get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("Agent registry pull failed")
                        .to_string()
                ));
            }
            Some("cancelled") => return Err(anyhow!("Agent registry pull cancelled")),
            _ => tokio::time::sleep(Duration::from_millis(300)).await,
        }
    }
}

fn parse_agent_pull_layers(data: &Value) -> anyhow::Result<Vec<ImageLayerProgress>> {
    serde_json::from_value(
        data.get("layers")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .context("Agent image pull layers are invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_source_uses_explicit_transfer_event() {
        assert_eq!(
            image_log_descriptor(&ImageTaskStatus::Success, Some(&ImageSource::Controller)),
            (
                "docker_image_transferred_from_controller",
                "platformLog.docker.imageAcquisition.controllerTransferred"
            )
        );
    }

    #[test]
    fn image_task_terminal_status_maps_to_operation_outcome() {
        assert_eq!(
            image_log_outcome(ImageTaskStatus::Success),
            OperationOutcome::Success
        );
        assert_eq!(
            image_log_outcome(ImageTaskStatus::Cancelled),
            OperationOutcome::Canceled
        );
        assert_eq!(
            image_log_outcome(ImageTaskStatus::Failed),
            OperationOutcome::Failure
        );
    }

    #[test]
    fn agent_registry_layers_preserve_status_and_percent() {
        let layers = parse_agent_pull_layers(&serde_json::json!({
            "layers": [{
                "id": "sha256:abc",
                "status": "Downloading",
                "progressPercent": 42
            }]
        }))
        .expect("parse layers");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].id, "sha256:abc");
        assert_eq!(layers[0].status.as_deref(), Some("Downloading"));
        assert_eq!(layers[0].progress_percent, Some(42));
    }

    #[test]
    fn distribution_terminal_status_has_stable_priority() {
        let target = |status| ImageDistributionTarget {
            node_id: "node".to_string(),
            node_name: "Node".to_string(),
            status,
            source: None,
            stage: ImageStage::Checking,
            progress_percent: 0,
            error_summary: None,
        };

        assert_eq!(
            distribution_status(&[target(ImageTaskStatus::Success)]),
            ImageTaskStatus::Success
        );
        assert_eq!(
            distribution_status(&[
                target(ImageTaskStatus::Success),
                target(ImageTaskStatus::Failed)
            ]),
            ImageTaskStatus::Failed
        );
        assert_eq!(
            distribution_status(&[
                target(ImageTaskStatus::Cancelled),
                target(ImageTaskStatus::Running)
            ]),
            ImageTaskStatus::Running
        );
    }

    #[test]
    fn distribution_error_summary_is_redacted_and_limited() {
        let value = format!("token=secret password:guess {}", "x".repeat(600));
        let sanitized = sanitize_distribution_error(&value);

        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("guess"));
        assert!(sanitized.chars().count() <= MAX_DISTRIBUTION_ERROR_CHARS);
    }
}
