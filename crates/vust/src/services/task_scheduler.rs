//! Master 计划任务领域规则：参数校验、时区调度与能力计算。

use crate::types::{ApiError, ApiResult};
use chrono::{TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::str::FromStr;
use vust_contracts::{
    api::ErrorCode,
    scheduled_tasks::{
        CreateScheduledTaskRequest, ScheduledTaskCapabilities, ScheduledTaskDeploymentStatus,
        ScheduledTaskOwnershipKind, UpdateScheduledTaskRequest,
    },
};

/// 校验创建请求中的所有用户可控字段。
pub fn validate_create_request(request: &CreateScheduledTaskRequest) -> ApiResult<()> {
    validate_common(
        &request.name,
        request.description.as_deref(),
        &request.cron_expr,
        &request.time_zone,
        &request.command,
        request.timeout_seconds,
    )?;
    if request.node_id.trim().is_empty() {
        return Err(ApiError::bad_request(
            ErrorCode::NodeInvalidTarget,
            "scheduled task node id must not be empty",
        ));
    }
    Ok(())
}

/// 校验编辑请求；节点和归属不属于可编辑字段。
pub fn validate_update_request(request: &UpdateScheduledTaskRequest) -> ApiResult<()> {
    validate_common(
        &request.name,
        request.description.as_deref(),
        &request.cron_expr,
        &request.time_zone,
        &request.command,
        request.timeout_seconds,
    )
}

fn validate_common(
    name: &str,
    description: Option<&str>,
    cron_expr: &str,
    time_zone: &str,
    command: &str,
    timeout_seconds: u32,
) -> ApiResult<()> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidName,
            "scheduled task name must contain 1 to 80 non-control characters",
        ));
    }
    if description.is_some_and(|value| value.chars().count() > 500) {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidName,
            "scheduled task description must not exceed 500 characters",
        ));
    }
    if command.is_empty() || command.len() > 65_536 || command.contains('\0') {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidCommand,
            "scheduled task command is invalid",
        ));
    }
    if !(1..=86_400).contains(&timeout_seconds) {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidCommand,
            "scheduled task timeout must be between 1 and 86400 seconds",
        ));
    }
    validate_schedule(cron_expr, time_zone)
}

/// 校验 5 段分钟级 Cron 和 IANA 时区。
pub fn validate_schedule(cron_expr: &str, time_zone: &str) -> ApiResult<()> {
    let parts = cron_expr.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        return Err(ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidSchedule,
            "scheduled task cron expression must contain exactly 5 fields",
        ));
    }
    let _: Tz = time_zone.parse().map_err(|_| {
        ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidTimeZone,
            "scheduled task time zone must be a valid IANA identifier",
        )
    })?;
    Schedule::from_str(&format!("0 {}", parts.join(" "))).map_err(|error| {
        ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidSchedule,
            format!("invalid scheduled task cron expression: {error}"),
        )
    })?;
    Ok(())
}

/// 计算下一次 UTC RFC 3339 执行时间。
pub fn compute_next_run_at(cron_expr: &str, time_zone: &str) -> ApiResult<Option<String>> {
    validate_schedule(cron_expr, time_zone)?;
    let tz: Tz = time_zone.parse().map_err(|_| {
        ApiError::bad_request(
            ErrorCode::ScheduledTaskInvalidTimeZone,
            "scheduled task time zone must be a valid IANA identifier",
        )
    })?;
    let schedule = Schedule::from_str(&format!("0 {cron_expr}"))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let now = Utc::now();
    let mut probe = (now + chrono::Duration::minutes(1))
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .ok_or_else(|| ApiError::internal("failed to normalize scheduled task timestamp"))?;
    for _ in 0..=(36 * 60) {
        if schedule.includes(probe.with_timezone(&tz)) {
            return Ok(Some(probe.to_rfc3339()));
        }
        probe += chrono::Duration::minutes(1);
    }
    let local = tz
        .timestamp_opt(now.timestamp(), 0)
        .single()
        .ok_or_else(|| ApiError::internal("failed to convert scheduled task time zone"))?;
    Ok(schedule
        .after(&local)
        .next()
        .map(|value| value.with_timezone(&Utc).to_rfc3339()))
}

/// 基于归属、部署和运行边界统一计算能力。
pub fn capabilities(
    ownership: ScheduledTaskOwnershipKind,
    deployment: ScheduledTaskDeploymentStatus,
    has_active_run: bool,
    has_active_operation: bool,
) -> ScheduledTaskCapabilities {
    let custom = ownership == ScheduledTaskOwnershipKind::Custom;
    let stable = !matches!(
        deployment,
        ScheduledTaskDeploymentStatus::Deleting | ScheduledTaskDeploymentStatus::Migrating
    ) && !has_active_operation;
    ScheduledTaskCapabilities {
        can_update: custom && stable,
        can_change_state: custom && stable,
        can_run: custom && stable && deployment == ScheduledTaskDeploymentStatus::Ready,
        can_remove: custom && stable && !has_active_run,
        can_migrate: custom && stable && !has_active_run,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_request() -> CreateScheduledTaskRequest {
        CreateScheduledTaskRequest {
            name: "Daily backup".to_string(),
            description: None,
            node_id: "local".to_string(),
            cron_expr: "0 2 * * *".to_string(),
            time_zone: "Asia/Shanghai".to_string(),
            command: "printf ok".to_string(),
            timeout_seconds: 60,
            prevent_overlap: true,
            enabled: true,
        }
    }

    #[test]
    fn managed_tasks_are_read_only() {
        let value = capabilities(
            ScheduledTaskOwnershipKind::Suite,
            ScheduledTaskDeploymentStatus::Ready,
            false,
            false,
        );
        assert!(!value.can_update);
        assert!(!value.can_remove);
    }

    #[test]
    fn active_run_blocks_remove_and_migration() {
        let value = capabilities(
            ScheduledTaskOwnershipKind::Custom,
            ScheduledTaskDeploymentStatus::Ready,
            true,
            false,
        );
        assert!(value.can_run);
        assert!(!value.can_remove);
        assert!(!value.can_migrate);
    }

    #[test]
    fn validates_all_user_controlled_boundaries_with_stable_codes() {
        let mut request = valid_request();
        assert!(validate_create_request(&request).is_ok());

        request.name = "bad\nname".to_string();
        assert_eq!(
            validate_create_request(&request).unwrap_err().code,
            ErrorCode::ScheduledTaskInvalidName
        );
        request = valid_request();
        request.command = "bad\0command".to_string();
        assert_eq!(
            validate_create_request(&request).unwrap_err().code,
            ErrorCode::ScheduledTaskInvalidCommand
        );
        request = valid_request();
        request.timeout_seconds = 0;
        assert_eq!(
            validate_create_request(&request).unwrap_err().code,
            ErrorCode::ScheduledTaskInvalidCommand
        );
        request = valid_request();
        request.cron_expr = "0 0 2 * * *".to_string();
        assert_eq!(
            validate_create_request(&request).unwrap_err().code,
            ErrorCode::ScheduledTaskInvalidSchedule
        );
        request = valid_request();
        request.time_zone = "Invalid/Zone".to_string();
        assert_eq!(
            validate_create_request(&request).unwrap_err().code,
            ErrorCode::ScheduledTaskInvalidTimeZone
        );
    }

    #[test]
    fn active_operation_blocks_every_mutating_capability() {
        let value = capabilities(
            ScheduledTaskOwnershipKind::Custom,
            ScheduledTaskDeploymentStatus::Ready,
            false,
            true,
        );
        assert_eq!(value, ScheduledTaskCapabilities::default());
    }
}
