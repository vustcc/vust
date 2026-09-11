//! 个人通知中心 API：按可信用户隔离查询，并只允许修改读取与归档状态。

use std::{collections::BTreeMap, convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, patch, post},
};
use chrono::{DateTime, Utc};
use futures_util::{Stream, stream};
use sqlx::{FromRow, QueryBuilder, Sqlite};
use vust_contracts::{
    api::ErrorCode,
    logging::{OperationModule, OperationOutcome, OperationParameterValue},
    notification::{
        NotificationAction, NotificationArchiveScope, NotificationArchiveStateRequest,
        NotificationAttentionLevel, NotificationBatchArchiveStateRequest, NotificationCapabilities,
        NotificationCategory, NotificationCode, NotificationDetail, NotificationPage,
        NotificationQuery, NotificationReadFilter, NotificationReadStateRequest,
        NotificationSource, NotificationSubject, NotificationSummary, NotificationUnreadSummary,
    },
};

use crate::{
    api::auth::AuthenticatedAdmin,
    services::logging,
    state::{AppState, DbPool},
    types::{ApiError, ApiResponse, ApiResult},
};

const NOTIFICATION_RETENTION_DAYS: i64 = 90;
const RETENTION_BATCH_SIZE: i64 = 1_000;

/// 挂载个人通知中心路由；不公开创建、删除或内容更新接口。
pub fn notifications_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/query", post(query_notifications))
        .route("/unread-summary", get(unread_summary))
        .route("/events", get(notification_events))
        .route("/read-all", post(read_all))
        .route("/archive-state", post(batch_archive_state))
        .route("/{notification_id}", get(notification_detail))
        .route("/{notification_id}/read-state", patch(update_read_state))
        .route(
            "/{notification_id}/archive-state",
            patch(update_archive_state),
        )
}

/// 建立当前认证用户的通知变化事件流；事件不包含任何通知业务数据。
async fn notification_events(
    admin: AuthenticatedAdmin,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    Sse::new(notification_change_stream(
        logging::subscribe_notification_changes(),
        admin.id,
    ))
    .keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// 将进程内广播过滤为单一用户的通用变化信号；积压时也要求客户端重新校准。
fn notification_change_stream(
    receiver: tokio::sync::broadcast::Receiver<i64>,
    recipient_user_id: i64,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream::unfold(
        (receiver, recipient_user_id),
        |(mut receiver, recipient_user_id)| async move {
            loop {
                match receiver.recv().await {
                    Ok(changed_user_id) if changed_user_id == recipient_user_id => {
                        return Some((
                            Ok(Event::default().data("changed")),
                            (receiver, recipient_user_id),
                        ));
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        return Some((
                            Ok(Event::default().data("changed")),
                            (receiver, recipient_user_id),
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    )
}

/// 查询当前用户的通知摘要。
async fn query_notifications(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
    Json(query): Json<NotificationQuery>,
) -> ApiResult<Response> {
    validate_query(&query)?;
    let offset = i64::from(query.page - 1) * i64::from(query.page_size);

    let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM user_notifications");
    push_query_filters(&mut count, admin.id, &query);
    let total = count
        .build_query_scalar::<i64>()
        .fetch_one(&state.metadata_db)
        .await?;

    let mut select = QueryBuilder::<Sqlite>::new("SELECT * FROM user_notifications");
    push_query_filters(&mut select, admin.id, &query);
    select
        .push(" ORDER BY created_at DESC, notification_id DESC LIMIT ")
        .push_bind(i64::from(query.page_size))
        .push(" OFFSET ")
        .push_bind(offset);
    let items = select
        .build_query_as::<NotificationRow>()
        .fetch_all(&state.metadata_db)
        .await?
        .into_iter()
        .map(NotificationRow::into_summary)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ApiResponse::success_with_raw(
        "Notifications loaded",
        Some(NotificationPage {
            total,
            page: query.page,
            page_size: query.page_size,
            items,
        }),
    )
    .into_response())
}

/// 查询当前用户未归档通知的未读摘要。
async fn unread_summary(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Response> {
    let row = sqlx::query_as::<_, UnreadSummaryRow>(
        r#"
        SELECT COUNT(*) AS unread_count,
               MAX(created_at) AS latest_created_at,
               MAX(state_changed_at) AS latest_state_changed_at
        FROM user_notifications
        WHERE recipient_user_id = ? AND archived_at IS NULL AND read_at IS NULL
        "#,
    )
    .bind(admin.id)
    .fetch_one(&state.metadata_db)
    .await?;
    let version = format!(
        "{}:{}:{}",
        row.unread_count,
        row.latest_created_at.as_deref().unwrap_or("none"),
        row.latest_state_changed_at.as_deref().unwrap_or("none")
    );
    Ok(ApiResponse::success_with_raw(
        "Unread notification summary loaded",
        Some(NotificationUnreadSummary {
            unread_count: row.unread_count,
            latest_created_at: row.latest_created_at,
            version,
        }),
    )
    .into_response())
}

/// 查询当前用户可见的单条通知详情。
async fn notification_detail(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
    Path(notification_id): Path<String>,
) -> ApiResult<Response> {
    let row = sqlx::query_as::<_, NotificationRow>(
        "SELECT * FROM user_notifications WHERE notification_id = ? AND recipient_user_id = ?",
    )
    .bind(notification_id)
    .bind(admin.id)
    .fetch_optional(&state.metadata_db)
    .await?
    .ok_or_else(notification_not_found)?;
    let detail = row.into_detail()?;
    Ok(ApiResponse::success_with_raw("Notification loaded", Some(detail)).into_response())
}

/// 幂等更新单条通知读取状态。
async fn update_read_state(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
    Path(notification_id): Path<String>,
    Json(payload): Json<NotificationReadStateRequest>,
) -> ApiResult<Response> {
    set_notification_read_state(&state.metadata_db, admin.id, &notification_id, payload.read)
        .await?;
    logging::publish_notification_change(admin.id);
    Ok(ApiResponse::ok("Notification read state updated").into_response())
}

/// 将当前用户所有未归档通知标记为已读。
async fn read_all(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Response> {
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query("UPDATE user_notifications SET read_at = ?, state_changed_at = ? WHERE recipient_user_id = ? AND archived_at IS NULL AND read_at IS NULL")
        .bind(&now).bind(&now).bind(admin.id).execute(&state.metadata_db).await?;
    if result.rows_affected() > 0 {
        logging::publish_notification_change(admin.id);
    }
    Ok(ApiResponse::ok("Notifications marked as read").into_response())
}

/// 幂等归档或恢复单条通知。
async fn update_archive_state(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
    Path(notification_id): Path<String>,
    Json(payload): Json<NotificationArchiveStateRequest>,
) -> ApiResult<Response> {
    set_notification_archive_state(
        &state.metadata_db,
        admin.id,
        &notification_id,
        payload.archived,
    )
    .await?;
    logging::publish_notification_change(admin.id);
    Ok(ApiResponse::ok("Notification archive state updated").into_response())
}

/// 按收件人隔离并幂等更新读取状态。
async fn set_notification_read_state(
    pool: &DbPool,
    recipient_user_id: i64,
    notification_id: &str,
    read: bool,
) -> Result<(), ApiError> {
    let now = Utc::now().to_rfc3339();
    let result = if read {
        sqlx::query("UPDATE user_notifications SET read_at = COALESCE(read_at, ?), state_changed_at = ? WHERE notification_id = ? AND recipient_user_id = ?")
            .bind(&now).bind(&now).bind(notification_id).bind(recipient_user_id).execute(pool).await?
    } else {
        sqlx::query("UPDATE user_notifications SET read_at = NULL, state_changed_at = ? WHERE notification_id = ? AND recipient_user_id = ?")
            .bind(&now).bind(notification_id).bind(recipient_user_id).execute(pool).await?
    };
    ensure_visible(result.rows_affected())
}

/// 按收件人隔离并幂等更新归档状态。
async fn set_notification_archive_state(
    pool: &DbPool,
    recipient_user_id: i64,
    notification_id: &str,
    archived: bool,
) -> Result<(), ApiError> {
    let now = Utc::now().to_rfc3339();
    let result = if archived {
        sqlx::query("UPDATE user_notifications SET archived_at = COALESCE(archived_at, ?), state_changed_at = ? WHERE notification_id = ? AND recipient_user_id = ?")
            .bind(&now).bind(&now).bind(notification_id).bind(recipient_user_id).execute(pool).await?
    } else {
        sqlx::query("UPDATE user_notifications SET archived_at = NULL, state_changed_at = ? WHERE notification_id = ? AND recipient_user_id = ?")
            .bind(&now).bind(notification_id).bind(recipient_user_id).execute(pool).await?
    };
    ensure_visible(result.rows_affected())
}

/// 批量归档或恢复当前用户的通知。
async fn batch_archive_state(
    admin: AuthenticatedAdmin,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<NotificationBatchArchiveStateRequest>,
) -> ApiResult<Response> {
    if payload.notification_ids.is_empty() || payload.notification_ids.len() > 100 {
        return Err(ApiError::bad_request(
            ErrorCode::NotificationInvalidBatch,
            "notificationIds must contain between 1 and 100 items",
        ));
    }
    let now = Utc::now().to_rfc3339();
    let mut update = QueryBuilder::<Sqlite>::new("UPDATE user_notifications SET archived_at = ");
    if payload.archived {
        update
            .push("COALESCE(archived_at, ")
            .push_bind(&now)
            .push(")");
    } else {
        update.push("NULL");
    }
    update
        .push(", state_changed_at = ")
        .push_bind(&now)
        .push(" WHERE recipient_user_id = ")
        .push_bind(admin.id)
        .push(" AND notification_id IN (");
    let mut separated = update.separated(",");
    for id in &payload.notification_ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(")");
    let result = update.build().execute(&state.metadata_db).await?;
    if result.rows_affected() > 0 {
        logging::publish_notification_change(admin.id);
    }
    Ok(ApiResponse::ok("Notification archive states updated").into_response())
}

/// 分批清理超过 90 天的个人通知。
pub async fn prune_expired_notifications(pool: &DbPool) -> Result<u64, ApiError> {
    let cutoff = (Utc::now() - chrono::Duration::days(NOTIFICATION_RETENTION_DAYS)).to_rfc3339();
    let result = sqlx::query(
        "DELETE FROM user_notifications WHERE notification_id IN (SELECT notification_id FROM user_notifications WHERE created_at < ? ORDER BY created_at LIMIT ?)",
    )
    .bind(cutoff)
    .bind(RETENTION_BATCH_SIZE)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// 启动每日通知保留清理任务。
pub fn spawn_retention_worker(pool: DbPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;
            loop {
                match prune_expired_notifications(&pool).await {
                    Ok(1_000) => continue,
                    Ok(_) => break,
                    Err(error) => {
                        tracing::error!(%error, "Notification retention cleanup failed");
                        break;
                    }
                }
            }
        }
    });
}

#[derive(Debug, FromRow)]
struct NotificationRow {
    notification_id: String,
    #[allow(dead_code)]
    recipient_user_id: i64,
    operation_event_id: String,
    created_at: String,
    code: String,
    category: String,
    attention_level: String,
    outcome: Option<String>,
    source_module: String,
    source_node_id: Option<String>,
    source_node_name: Option<String>,
    subject_kind: Option<String>,
    subject_id: Option<String>,
    subject_display_name: Option<String>,
    task_id: Option<String>,
    parameters_json: String,
    error_code: Option<String>,
    error_summary: Option<String>,
    trace_id: String,
    read_at: Option<String>,
    archived_at: Option<String>,
    #[allow(dead_code)]
    state_changed_at: String,
}

#[derive(Debug, FromRow)]
struct UnreadSummaryRow {
    unread_count: i64,
    latest_created_at: Option<String>,
    latest_state_changed_at: Option<String>,
}

impl NotificationRow {
    fn into_summary(self) -> Result<NotificationSummary, ApiError> {
        let code = parse_code(&self.code)?;
        let subject = match (self.subject_kind.clone(), self.subject_id.clone()) {
            (Some(kind), Some(id)) => Some(NotificationSubject {
                kind,
                id,
                display_name: self.subject_display_name.clone(),
            }),
            _ => None,
        };
        let parameters = serde_json::from_str::<BTreeMap<String, OperationParameterValue>>(
            &self.parameters_json,
        )
        .unwrap_or_default();
        let action = notification_action(code, &self, subject.as_ref(), &parameters);
        let can_mark_read = self.read_at.is_none();
        let can_mark_unread = self.read_at.is_some();
        let can_archive = self.archived_at.is_none();
        let can_restore = self.archived_at.is_some();
        Ok(NotificationSummary {
            notification_id: self.notification_id,
            created_at: self.created_at,
            code,
            category: parse_category(&self.category)?,
            attention_level: parse_attention_level(&self.attention_level)?,
            outcome: self.outcome.as_deref().map(parse_outcome).transpose()?,
            source: NotificationSource {
                module: parse_module(&self.source_module)?,
                node_id: self.source_node_id,
                node_name: self.source_node_name,
            },
            subject,
            task_id: self.task_id,
            operation_event_id: self.operation_event_id,
            parameters,
            read_at: self.read_at,
            archived_at: self.archived_at,
            action: action.clone(),
            capabilities: NotificationCapabilities {
                can_view_details: true,
                can_mark_read,
                can_mark_unread,
                can_archive,
                can_restore,
                can_open_target: action.is_some(),
            },
        })
    }

    fn into_detail(self) -> Result<NotificationDetail, ApiError> {
        let error_code = self.error_code.clone();
        let error_summary = self.error_summary.clone();
        let trace_id = self.trace_id.clone();
        Ok(NotificationDetail {
            summary: self.into_summary()?,
            error_code,
            error_summary,
            trace_id,
        })
    }
}

fn notification_action(
    code: NotificationCode,
    row: &NotificationRow,
    subject: Option<&NotificationSubject>,
    parameters: &BTreeMap<String, OperationParameterValue>,
) -> Option<NotificationAction> {
    let app_id = match code {
        NotificationCode::NodeDeploymentFinished
        | NotificationCode::NodeOffline
        | NotificationCode::NodeRecovered => "node-manager",
        NotificationCode::SuiteInstallationFinished => "suite-center",
        NotificationCode::ScriptRunFinished => "script-manager",
        NotificationCode::ScheduledTaskOperationFinished
        | NotificationCode::ScheduledTaskRunFinished => "task-scheduler",
        NotificationCode::FileTaskFinished => "operation-log",
        NotificationCode::FileTransferFinished => "file-manager",
        NotificationCode::DiskOperationFinished => "disk-manager",
        NotificationCode::DockerImageTaskFinished
        | NotificationCode::DockerProjectTaskFinished
        | NotificationCode::DockerEngineInstallationFinished => "docker-manager",
        NotificationCode::LoginLockout => "operation-log",
        NotificationCode::UpgradePlanFinished => return None,
    };
    let mut payload = BTreeMap::new();
    if code == NotificationCode::FileTaskFinished {
        payload.insert(
            "eventId".to_string(),
            OperationParameterValue::String(row.operation_event_id.clone()),
        );
        return Some(NotificationAction {
            app_id: app_id.to_string(),
            label_key: "app.notificationCenter.viewOperationLog".to_string(),
            payload,
        });
    }
    let node_id = row
        .source_node_id
        .as_ref()
        .map(|value| OperationParameterValue::String(value.clone()))
        .or_else(|| parameters.get("nodeId").cloned())
        .or_else(|| {
            subject
                .filter(|value| value.kind == "node")
                .map(|value| OperationParameterValue::String(value.id.clone()))
        });
    if let Some(node_id) = node_id {
        payload.insert("nodeId".to_string(), node_id);
    }
    if let Some(subject) = subject {
        payload.insert(
            "subjectId".to_string(),
            OperationParameterValue::String(subject.id.clone()),
        );
    }
    if let Some(task_id) = row.task_id.as_ref() {
        payload.insert(
            "taskId".to_string(),
            OperationParameterValue::String(task_id.clone()),
        );
    }
    Some(NotificationAction {
        app_id: app_id.to_string(),
        label_key: "app.notificationCenter.openTarget".to_string(),
        payload,
    })
}

fn validate_query(query: &NotificationQuery) -> Result<(), ApiError> {
    if query.page == 0 || !matches!(query.page_size, 20 | 50 | 100) {
        return Err(invalid_query("invalid notification pagination"));
    }
    for count in [
        query.categories.as_ref().map_or(0, Vec::len),
        query.attention_levels.as_ref().map_or(0, Vec::len),
        query.modules.as_ref().map_or(0, Vec::len),
        query.codes.as_ref().map_or(0, Vec::len),
    ] {
        if count > 20 {
            return Err(invalid_query("too many notification filters"));
        }
    }
    if query
        .keyword
        .as_ref()
        .is_some_and(|v| v.chars().count() > 100)
    {
        return Err(invalid_query("notification keyword is too long"));
    }
    let from = parse_optional_time(query.created_from.as_deref(), "invalid createdFrom")?;
    let to = parse_optional_time(query.created_to.as_deref(), "invalid createdTo")?;
    if from.zip(to).is_some_and(|(from, to)| from > to) {
        return Err(invalid_query("createdFrom must not exceed createdTo"));
    }
    Ok(())
}

fn push_query_filters<'a>(
    builder: &mut QueryBuilder<'a, Sqlite>,
    recipient_user_id: i64,
    query: &'a NotificationQuery,
) {
    builder
        .push(" WHERE recipient_user_id = ")
        .push_bind(recipient_user_id);
    match query.archive_scope {
        NotificationArchiveScope::Active => builder.push(" AND archived_at IS NULL"),
        NotificationArchiveScope::Archived => builder.push(" AND archived_at IS NOT NULL"),
    };
    match query.read_filter {
        NotificationReadFilter::All => {}
        NotificationReadFilter::Read => {
            builder.push(" AND read_at IS NOT NULL");
        }
        NotificationReadFilter::Unread => {
            builder.push(" AND read_at IS NULL");
        }
    }
    push_list_filter(builder, "category", query.categories.as_deref(), |v| {
        v.as_str()
    });
    push_list_filter(
        builder,
        "attention_level",
        query.attention_levels.as_deref(),
        |v| v.as_str(),
    );
    push_list_filter(builder, "source_module", query.modules.as_deref(), |v| {
        v.as_str()
    });
    push_list_filter(builder, "code", query.codes.as_deref(), |v| v.as_str());
    if let Some(from) = query.created_from.as_ref() {
        let normalized = DateTime::parse_from_rfc3339(from)
            .expect("notification query time was validated")
            .with_timezone(&Utc)
            .to_rfc3339();
        builder.push(" AND created_at >= ").push_bind(normalized);
    }
    if let Some(to) = query.created_to.as_ref() {
        let normalized = DateTime::parse_from_rfc3339(to)
            .expect("notification query time was validated")
            .with_timezone(&Utc)
            .to_rfc3339();
        builder.push(" AND created_at <= ").push_bind(normalized);
    }
    if let Some(keyword) = query
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let escaped = keyword
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let term = format!("%{escaped}%");
        builder
            .push(" AND (code LIKE ")
            .push_bind(term.clone())
            .push(" ESCAPE '\\' OR source_node_name LIKE ")
            .push_bind(term.clone())
            .push(" ESCAPE '\\' OR subject_display_name LIKE ")
            .push_bind(term.clone())
            .push(" ESCAPE '\\' OR parameters_json LIKE ")
            .push_bind(term)
            .push(" ESCAPE '\\')");
    }
}

fn push_list_filter<T>(
    builder: &mut QueryBuilder<'_, Sqlite>,
    field: &'static str,
    values: Option<&[T]>,
    value: impl Fn(&T) -> &'static str,
) {
    let Some(values) = values.filter(|values| !values.is_empty()) else {
        return;
    };
    builder.push(" AND ").push(field).push(" IN (");
    let mut separated = builder.separated(",");
    for item in values {
        separated.push_bind(value(item));
    }
    separated.push_unseparated(")");
}

fn parse_optional_time(
    value: Option<&str>,
    message: &'static str,
) -> Result<Option<DateTime<chrono::FixedOffset>>, ApiError> {
    value
        .map(DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| invalid_query(message))
}

fn invalid_query(message: &'static str) -> ApiError {
    ApiError::bad_request(ErrorCode::NotificationInvalidQuery, message)
}

fn notification_not_found() -> ApiError {
    ApiError::not_found(ErrorCode::NotificationNotFound, "notification not found")
}

fn ensure_visible(rows_affected: u64) -> Result<(), ApiError> {
    (rows_affected > 0)
        .then_some(())
        .ok_or_else(notification_not_found)
}

fn parse_code(value: &str) -> Result<NotificationCode, ApiError> {
    match value {
        "nodeDeploymentFinished" => Ok(NotificationCode::NodeDeploymentFinished),
        "suiteInstallationFinished" => Ok(NotificationCode::SuiteInstallationFinished),
        "scriptRunFinished" => Ok(NotificationCode::ScriptRunFinished),
        "scheduledTaskOperationFinished" => Ok(NotificationCode::ScheduledTaskOperationFinished),
        "scheduledTaskRunFinished" => Ok(NotificationCode::ScheduledTaskRunFinished),
        "fileTaskFinished" => Ok(NotificationCode::FileTaskFinished),
        "fileTransferFinished" => Ok(NotificationCode::FileTransferFinished),
        "diskOperationFinished" => Ok(NotificationCode::DiskOperationFinished),
        "dockerImageTaskFinished" => Ok(NotificationCode::DockerImageTaskFinished),
        "dockerProjectTaskFinished" => Ok(NotificationCode::DockerProjectTaskFinished),
        "dockerEngineInstallationFinished" => {
            Ok(NotificationCode::DockerEngineInstallationFinished)
        }
        "upgradePlanFinished" => Ok(NotificationCode::UpgradePlanFinished),
        "nodeOffline" => Ok(NotificationCode::NodeOffline),
        "nodeRecovered" => Ok(NotificationCode::NodeRecovered),
        "loginLockout" => Ok(NotificationCode::LoginLockout),
        _ => Err(ApiError::internal("invalid notification code")),
    }
}

fn parse_category(value: &str) -> Result<NotificationCategory, ApiError> {
    match value {
        "task" => Ok(NotificationCategory::Task),
        "security" => Ok(NotificationCategory::Security),
        "system" => Ok(NotificationCategory::System),
        _ => Err(ApiError::internal("invalid notification category")),
    }
}

fn parse_attention_level(value: &str) -> Result<NotificationAttentionLevel, ApiError> {
    match value {
        "info" => Ok(NotificationAttentionLevel::Info),
        "warning" => Ok(NotificationAttentionLevel::Warning),
        "critical" => Ok(NotificationAttentionLevel::Critical),
        _ => Err(ApiError::internal("invalid notification attention level")),
    }
}

fn parse_outcome(value: &str) -> Result<OperationOutcome, ApiError> {
    match value {
        "success" => Ok(OperationOutcome::Success),
        "failure" => Ok(OperationOutcome::Failure),
        "partial" => Ok(OperationOutcome::Partial),
        "canceled" => Ok(OperationOutcome::Canceled),
        "timedOut" => Ok(OperationOutcome::TimedOut),
        _ => Err(ApiError::internal("invalid notification outcome")),
    }
}

fn parse_module(value: &str) -> Result<OperationModule, ApiError> {
    match value {
        "auth" => Ok(OperationModule::Auth),
        "nodes" => Ok(OperationModule::Nodes),
        "suites" => Ok(OperationModule::Suites),
        "docker" => Ok(OperationModule::Docker),
        "files" => Ok(OperationModule::Files),
        "processes" => Ok(OperationModule::Processes),
        "disks" => Ok(OperationModule::Disks),
        "monitoring" => Ok(OperationModule::Monitoring),
        "scripts" => Ok(OperationModule::Scripts),
        "scheduledTasks" => Ok(OperationModule::ScheduledTasks),
        "upgrades" => Ok(OperationModule::Upgrades),
        "terminal" => Ok(OperationModule::Terminal),
        "settings" => Ok(OperationModule::Settings),
        _ => Err(ApiError::internal("invalid notification source module")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn query() -> NotificationQuery {
        NotificationQuery {
            page: 1,
            page_size: 20,
            archive_scope: NotificationArchiveScope::Active,
            read_filter: NotificationReadFilter::All,
            categories: None,
            attention_levels: None,
            modules: None,
            codes: None,
            created_from: None,
            created_to: None,
            keyword: None,
        }
    }

    async fn seed_user(pool: &DbPool, id: i64) {
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, 'hash')")
            .bind(id)
            .bind(format!("user-{id}"))
            .execute(pool)
            .await
            .unwrap();
    }

    async fn seed_notification(
        pool: &DbPool,
        recipient_user_id: i64,
        notification_id: &str,
        created_at: &str,
        subject: &str,
        read_at: Option<&str>,
        archived_at: Option<&str>,
    ) {
        let event_id = format!("event-{notification_id}");
        sqlx::query("INSERT INTO operation_logs (event_id, occurred_at, module, event_code, actor_kind, actor_display_name, origin_kind, outcome, impact, trace_id, client_ip) VALUES (?, ?, 'scripts', 'script_run_completed', 'user', 'admin', 'master', 'success', 'info', 'trace', '127.0.0.1')")
            .bind(&event_id)
            .bind(created_at)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_notifications (notification_id, recipient_user_id, operation_event_id, created_at, code, category, attention_level, outcome, source_module, subject_kind, subject_id, subject_display_name, parameters_json, trace_id, read_at, archived_at, state_changed_at) VALUES (?, ?, ?, ?, 'scriptRunFinished', 'task', 'info', 'success', 'scripts', 'script', 'script-1', ?, '{}', 'trace', ?, ?, ?)")
            .bind(notification_id)
            .bind(recipient_user_id)
            .bind(event_id)
            .bind(created_at)
            .bind(subject)
            .bind(read_at)
            .bind(archived_at)
            .bind(created_at)
            .execute(pool)
            .await
            .unwrap();
    }

    #[test]
    fn query_validation_enforces_pagination_keyword_and_time_bounds() {
        let mut value = query();
        value.page_size = 10;
        assert_eq!(
            validate_query(&value).unwrap_err().code,
            ErrorCode::NotificationInvalidQuery
        );
        value.page_size = 20;
        value.keyword = Some("x".repeat(101));
        assert!(validate_query(&value).is_err());
        value.keyword = None;
        value.created_from = Some("2026-07-18T10:00:00Z".to_string());
        value.created_to = Some("2026-07-18T09:00:00Z".to_string());
        assert!(validate_query(&value).is_err());
    }

    #[tokio::test]
    async fn notification_change_stream_only_emits_for_the_target_user() {
        let (sender, receiver) = tokio::sync::broadcast::channel(8);
        let changes = notification_change_stream(receiver, 42);
        tokio::pin!(changes);

        sender.send(7).unwrap();
        sender.send(42).unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), changes.next())
            .await
            .unwrap();
        assert!(event.is_some());
    }

    #[tokio::test]
    async fn query_filters_enforce_user_isolation_and_literal_like_matching() {
        let pool = crate::test_support::setup_test_db().await;
        seed_user(&pool, 1).await;
        seed_user(&pool, 2).await;
        seed_notification(
            &pool,
            1,
            "notification-1",
            "2026-07-18T00:00:00Z",
            "literal %_ value",
            None,
            None,
        )
        .await;
        seed_notification(
            &pool,
            2,
            "notification-2",
            "2026-07-18T00:00:01Z",
            "literal %_ value",
            None,
            None,
        )
        .await;
        let mut value = query();
        value.keyword = Some("%_".to_string());
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM user_notifications");
        push_query_filters(&mut builder, 1, &value);
        let count = builder
            .build_query_scalar::<i64>()
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);

        value.attention_levels = Some(vec![NotificationAttentionLevel::Warning]);
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM user_notifications");
        push_query_filters(&mut builder, 1, &value);
        let count = builder
            .build_query_scalar::<i64>()
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn retention_removes_only_notifications_older_than_ninety_days() {
        let pool = crate::test_support::setup_test_db().await;
        seed_user(&pool, 1).await;
        let old = (Utc::now() - chrono::Duration::days(91)).to_rfc3339();
        let recent = (Utc::now() - chrono::Duration::days(89)).to_rfc3339();
        seed_notification(&pool, 1, "old", &old, "Old", None, None).await;
        seed_notification(&pool, 1, "recent", &recent, "Recent", None, None).await;
        assert_eq!(prune_expired_notifications(&pool).await.unwrap(), 1);
        let remaining =
            sqlx::query_scalar::<_, String>("SELECT notification_id FROM user_notifications")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, vec!["recent".to_string()]);
    }

    #[tokio::test]
    async fn read_archive_and_restore_are_idempotent_and_user_scoped() {
        let pool = crate::test_support::setup_test_db().await;
        seed_user(&pool, 1).await;
        seed_user(&pool, 2).await;
        seed_notification(
            &pool,
            1,
            "notification-state",
            "2026-07-18T00:00:00Z",
            "Script",
            None,
            None,
        )
        .await;

        let hidden = set_notification_read_state(&pool, 2, "notification-state", true)
            .await
            .unwrap_err();
        assert_eq!(hidden.code, ErrorCode::NotificationNotFound);

        set_notification_read_state(&pool, 1, "notification-state", true)
            .await
            .unwrap();
        set_notification_read_state(&pool, 1, "notification-state", true)
            .await
            .unwrap();
        set_notification_archive_state(&pool, 1, "notification-state", true)
            .await
            .unwrap();
        set_notification_archive_state(&pool, 1, "notification-state", false)
            .await
            .unwrap();

        let state = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT read_at, archived_at FROM user_notifications WHERE notification_id = ?",
        )
        .bind("notification-state")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(state.0.is_some());
        assert!(state.1.is_none());

        set_notification_read_state(&pool, 1, "notification-state", false)
            .await
            .unwrap();
        let read_at = sqlx::query_scalar::<_, Option<String>>(
            "SELECT read_at FROM user_notifications WHERE notification_id = ?",
        )
        .bind("notification-state")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(read_at.is_none());

        sqlx::query(
            "UPDATE user_notifications SET parameters_json = '{\"nodeId\":\"node-1\"}' WHERE notification_id = ?",
        )
        .bind("notification-state")
        .execute(&pool)
        .await
        .unwrap();
        let summary = sqlx::query_as::<_, NotificationRow>(
            "SELECT * FROM user_notifications WHERE notification_id = ?",
        )
        .bind("notification-state")
        .fetch_one(&pool)
        .await
        .unwrap()
        .into_summary()
        .unwrap();
        assert_eq!(
            summary
                .action
                .unwrap()
                .payload
                .get("nodeId")
                .and_then(|value| match value {
                    OperationParameterValue::String(value) => Some(value.as_str()),
                    _ => None,
                }),
            Some("node-1")
        );
    }

    #[tokio::test]
    async fn file_task_action_opens_the_matching_operation_log_event() {
        let pool = crate::test_support::setup_test_db().await;
        seed_user(&pool, 1).await;
        seed_notification(
            &pool,
            1,
            "file-task-notification",
            "2026-07-18T00:00:00Z",
            "unused",
            None,
            None,
        )
        .await;
        sqlx::query(
            "UPDATE user_notifications
             SET code = 'fileTaskFinished', source_module = 'files',
                 subject_kind = NULL, subject_id = NULL, subject_display_name = NULL,
                 task_id = 'file-task-1',
                 parameters_json = '{\"totalItemCount\":2,\"completedItemCount\":2,\"failedItemCount\":0}'
             WHERE notification_id = 'file-task-notification'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let summary = sqlx::query_as::<_, NotificationRow>(
            "SELECT * FROM user_notifications WHERE notification_id = 'file-task-notification'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .into_summary()
        .unwrap();
        let action = summary.action.unwrap();

        assert_eq!(action.app_id, "operation-log");
        assert_eq!(
            action.payload.get("eventId").and_then(|value| match value {
                OperationParameterValue::String(value) => Some(value.as_str()),
                _ => None,
            }),
            Some("event-file-task-notification")
        );
        assert_eq!(action.label_key, "app.notificationCenter.viewOperationLog");
    }
}
