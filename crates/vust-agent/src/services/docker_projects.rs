//! Docker Compose 项目领域事实、状态聚合与管理能力。

use crate::models::docker::{
    DockerProjectCapabilities, DockerProjectConfigurationState, DockerProjectContainerStates,
    DockerProjectManageVia, DockerProjectManagement, DockerProjectManagementKind,
    DockerProjectRuntimeState,
};
use crate::types::{ApiError, ApiResult};
use bollard::models::ContainerSummaryStateEnum;
use vust_contracts::api::ErrorCode;

/// Compose 配置文件名。
pub const COMPOSE_FILE_NAME: &str = "compose.yaml";
/// 上一个已成功应用的配置备份。
pub const APPLIED_COMPOSE_FILE_NAME: &str = ".applied-compose.yaml";
/// 允许保存的最大 Compose 配置大小。
pub const MAX_COMPOSE_BYTES: usize = 1024 * 1024;

/// 数据库中的 Compose 项目事实。
#[derive(Debug, Clone)]
pub struct DockerProjectRecord {
    pub name: String,
    pub compose_dir: String,
    pub management_kind: DockerProjectManagementKind,
    pub owner_name: Option<String>,
    pub config_revision: i64,
    pub applied_revision: Option<i64>,
    pub created_at: i64,
}

/// 校验并规范化用户创建的 Compose 项目名。
pub fn normalize_project_name(value: &str) -> ApiResult<String> {
    let name = value.trim();
    if name.is_empty()
        || name.len() > 63
        || !name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
    {
        return Err(ApiError::validation(
            "project name must match [a-z0-9][a-z0-9_-]{0,62}",
        ));
    }
    Ok(name.to_string())
}

/// 将数据库归属字符串转换为领域枚举。
pub fn parse_management_kind(value: &str) -> ApiResult<DockerProjectManagementKind> {
    match value {
        "custom" => Ok(DockerProjectManagementKind::Custom),
        "suite" => Ok(DockerProjectManagementKind::Suite),
        "system" => Ok(DockerProjectManagementKind::System),
        _ => Err(ApiError::internal("invalid Docker project management kind")),
    }
}

/// 构造项目管理归属。
pub fn management_for(
    kind: DockerProjectManagementKind,
    owner_name: Option<String>,
) -> DockerProjectManagement {
    let (read_only, manage_via) = match kind {
        DockerProjectManagementKind::Custom => (false, DockerProjectManageVia::Projects),
        DockerProjectManagementKind::Suite => (true, DockerProjectManageVia::SuiteCenter),
        DockerProjectManagementKind::System => (true, DockerProjectManageVia::System),
    };
    DockerProjectManagement {
        kind,
        owner_name,
        read_only,
        manage_via,
    }
}

/// 在最终执行端拒绝修改托管项目。
pub fn ensure_mutable(record: &DockerProjectRecord) -> ApiResult<()> {
    if record.management_kind != DockerProjectManagementKind::Custom {
        return Err(ApiError::conflict(
            ErrorCode::DockerProjectProtected,
            "managed Docker projects are read-only in the project module",
        )
        .with_detail(format!(
            "project={} management={}",
            record.name,
            record.management_kind.as_str()
        )));
    }
    Ok(())
}

/// 从容器状态分布计算项目运行状态。
pub fn runtime_state(states: &DockerProjectContainerStates) -> DockerProjectRuntimeState {
    if states.total == 0 || states.exited == states.total {
        return DockerProjectRuntimeState::Stopped;
    }
    if states.running == states.total {
        return DockerProjectRuntimeState::Running;
    }
    if states.running + states.paused + states.restarting + states.exited + states.other
        == states.total
    {
        return DockerProjectRuntimeState::Partial;
    }
    DockerProjectRuntimeState::Unknown
}

/// 累加一个 Docker 容器状态。
pub fn add_container_state(
    states: &mut DockerProjectContainerStates,
    state: Option<ContainerSummaryStateEnum>,
) {
    states.total += 1;
    match state {
        Some(ContainerSummaryStateEnum::RUNNING) => states.running += 1,
        Some(ContainerSummaryStateEnum::PAUSED) => states.paused += 1,
        Some(ContainerSummaryStateEnum::RESTARTING) => states.restarting += 1,
        Some(ContainerSummaryStateEnum::EXITED) => states.exited += 1,
        _ => states.other += 1,
    }
}

/// 根据归属、状态和配置可用性计算能力。
pub fn capabilities_for(
    management_kind: DockerProjectManagementKind,
    runtime: DockerProjectRuntimeState,
    configuration: DockerProjectConfigurationState,
    has_containers: bool,
) -> DockerProjectCapabilities {
    if management_kind != DockerProjectManagementKind::Custom {
        return DockerProjectCapabilities::default();
    }
    let has_config = configuration != DockerProjectConfigurationState::Missing;
    DockerProjectCapabilities {
        can_start: has_config && has_containers && runtime == DockerProjectRuntimeState::Stopped,
        can_stop: matches!(
            runtime,
            DockerProjectRuntimeState::Running | DockerProjectRuntimeState::Partial
        ),
        can_restart: has_config && has_containers,
        can_redeploy: has_config,
        can_edit_configuration: true,
        can_scale: has_config,
        can_remove: true,
    }
}

/// 根据配置文件与版本计算配置状态。
pub fn configuration_state(
    file_exists: bool,
    revision: i64,
    applied_revision: Option<i64>,
) -> DockerProjectConfigurationState {
    if !file_exists {
        DockerProjectConfigurationState::Missing
    } else if applied_revision == Some(revision) {
        DockerProjectConfigurationState::Applied
    } else {
        DockerProjectConfigurationState::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_compose_project_names() {
        assert_eq!(normalize_project_name("demo_1").unwrap(), "demo_1");
        for invalid in ["", "Upper", ".", "a.b", "-demo"] {
            assert!(normalize_project_name(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn classifies_runtime_boundaries() {
        assert_eq!(
            runtime_state(&DockerProjectContainerStates::default()),
            DockerProjectRuntimeState::Stopped
        );
        let running = DockerProjectContainerStates {
            total: 2,
            running: 2,
            ..Default::default()
        };
        assert_eq!(runtime_state(&running), DockerProjectRuntimeState::Running);
        let partial = DockerProjectContainerStates {
            total: 2,
            running: 1,
            exited: 1,
            ..Default::default()
        };
        assert_eq!(runtime_state(&partial), DockerProjectRuntimeState::Partial);
    }

    #[test]
    fn managed_projects_have_no_mutation_capabilities() {
        let capabilities = capabilities_for(
            DockerProjectManagementKind::Suite,
            DockerProjectRuntimeState::Running,
            DockerProjectConfigurationState::Applied,
            true,
        );
        assert_eq!(capabilities, DockerProjectCapabilities::default());
    }
}
