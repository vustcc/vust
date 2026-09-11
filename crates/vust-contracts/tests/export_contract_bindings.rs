//! 导出前端使用的 TypeScript 契约绑定。

use std::collections::BTreeMap;
use std::path::Path;
use ts_rs::{Config, Dummy, TS};
use vust_contracts::{
    api::{ApiResponse, ErrorCode},
    auth::{AuthBody, AuthSession, AuthUser},
    disks::{
        CreateDiskOperationRequest, DiskDetail, DiskInventory, DiskOperation, ManagedDiskVolume,
    },
    files::{FileDocument, FileSaveResult, UpdateFileContentRequest},
    firewall::{FirewallRuleDetail, FirewallRuleListPage},
    logging::{
        AgentOperationEvent, OperationEventLabel, OperationImpact, OperationLogDetail,
        OperationLogPage, OperationLogQuery, OperationLogSummary, OperationOutcome,
        OperationParameterValue, OperationTarget, SuiteOperationEventRequest,
    },
    monitoring::{SystemMonitoringOverview, SystemMonitoringSeriesPage, SystemMonitoringSettings},
    notification::{
        NotificationAction, NotificationArchiveScope, NotificationArchiveStateRequest,
        NotificationAttentionLevel, NotificationBatchArchiveStateRequest, NotificationCapabilities,
        NotificationCategory, NotificationCode, NotificationDetail, NotificationPage,
        NotificationQuery, NotificationReadFilter, NotificationReadStateRequest,
        NotificationSource, NotificationSubject, NotificationSummary, NotificationUnreadSummary,
    },
    process::{
        NetworkConnectionListPage, ProcessActionRequest, ProcessForceKillConfirmation,
        ProcessListPage, ProcessSignalResult,
    },
    runtime_logs::{
        RuntimeLogFile, RuntimeLogFileList, RuntimeLogLine, RuntimeLogQuery, RuntimeLogQueryResult,
    },
    scheduled_tasks::{
        CreateScheduledTaskBatchRequest, CreateScheduledTaskMigrationRequest,
        CreateScheduledTaskRequest, ScheduledTaskBatch, ScheduledTaskDetail, ScheduledTaskListPage,
        ScheduledTaskOperation, ScheduledTaskRun, ScheduledTaskRunOutput, ScheduledTaskRunPage,
        UpdateScheduledTaskRequest, UpdateScheduledTaskStateRequest,
    },
    scripts::{
        CreateScriptRequest, CreateScriptRunRequest, ScriptDetail, ScriptListPage, ScriptRun,
        ScriptRunTerminalClientMessage, ScriptRunTerminalServerMessage, UpdateScriptRequest,
    },
    terminal::{TerminalAccess, TerminalClientMessage, TerminalServerMessage},
    vust::{VustNetworkConfig, VustNetworkUpdateResult},
};

fn export_type<T: TS + 'static>(cfg: &Config) {
    T::export_all(cfg).expect("failed to export TypeScript binding");
}

fn export_config() -> Config {
    let mut cfg = Config::from_env();
    if std::env::var_os("TS_RS_EXPORT_DIR").is_none() {
        cfg = cfg.with_out_dir(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/ts-rs-bindings/vust-contracts-test"),
        );
    }
    cfg
}

#[test]
fn export_error_code_binding() {
    export_type::<ErrorCode>(&export_config());
}

#[test]
fn export_contract_bindings() {
    let cfg = export_config();

    export_type::<ErrorCode>(&cfg);
    export_type::<ApiResponse<Dummy>>(&cfg);
    export_type::<AuthUser>(&cfg);
    export_type::<AuthSession>(&cfg);
    export_type::<AuthBody>(&cfg);
    export_type::<DiskInventory>(&cfg);
    export_type::<DiskDetail>(&cfg);
    export_type::<ManagedDiskVolume>(&cfg);
    export_type::<CreateDiskOperationRequest>(&cfg);
    export_type::<DiskOperation>(&cfg);
    export_type::<FileDocument>(&cfg);
    export_type::<FileSaveResult>(&cfg);
    export_type::<UpdateFileContentRequest>(&cfg);
    export_type::<FirewallRuleListPage>(&cfg);
    export_type::<FirewallRuleDetail>(&cfg);
    export_type::<VustNetworkConfig>(&cfg);
    export_type::<VustNetworkUpdateResult>(&cfg);
    export_type::<OperationLogSummary>(&cfg);
    export_type::<OperationLogDetail>(&cfg);
    export_type::<OperationLogQuery>(&cfg);
    export_type::<OperationLogPage>(&cfg);
    export_type::<OperationEventLabel>(&cfg);
    export_type::<AgentOperationEvent>(&cfg);
    export_type::<SuiteOperationEventRequest>(&cfg);
    export_type::<SystemMonitoringOverview>(&cfg);
    export_type::<SystemMonitoringSeriesPage>(&cfg);
    export_type::<SystemMonitoringSettings>(&cfg);
    export_type::<NotificationCode>(&cfg);
    export_type::<NotificationCategory>(&cfg);
    export_type::<NotificationAttentionLevel>(&cfg);
    export_type::<NotificationArchiveScope>(&cfg);
    export_type::<NotificationReadFilter>(&cfg);
    export_type::<NotificationSource>(&cfg);
    export_type::<NotificationSubject>(&cfg);
    export_type::<NotificationAction>(&cfg);
    export_type::<NotificationCapabilities>(&cfg);
    export_type::<NotificationSummary>(&cfg);
    export_type::<NotificationDetail>(&cfg);
    export_type::<NotificationQuery>(&cfg);
    export_type::<NotificationPage>(&cfg);
    export_type::<NotificationUnreadSummary>(&cfg);
    export_type::<NotificationReadStateRequest>(&cfg);
    export_type::<NotificationArchiveStateRequest>(&cfg);
    export_type::<NotificationBatchArchiveStateRequest>(&cfg);
    export_type::<ProcessListPage>(&cfg);
    export_type::<NetworkConnectionListPage>(&cfg);
    export_type::<ProcessActionRequest>(&cfg);
    export_type::<ProcessForceKillConfirmation>(&cfg);
    export_type::<ProcessSignalResult>(&cfg);
    export_type::<RuntimeLogFile>(&cfg);
    export_type::<RuntimeLogFileList>(&cfg);
    export_type::<RuntimeLogQuery>(&cfg);
    export_type::<RuntimeLogLine>(&cfg);
    export_type::<RuntimeLogQueryResult>(&cfg);
    export_type::<ScheduledTaskListPage>(&cfg);
    export_type::<ScheduledTaskDetail>(&cfg);
    export_type::<CreateScheduledTaskRequest>(&cfg);
    export_type::<UpdateScheduledTaskRequest>(&cfg);
    export_type::<UpdateScheduledTaskStateRequest>(&cfg);
    export_type::<CreateScheduledTaskMigrationRequest>(&cfg);
    export_type::<CreateScheduledTaskBatchRequest>(&cfg);
    export_type::<ScheduledTaskRun>(&cfg);
    export_type::<ScheduledTaskRunPage>(&cfg);
    export_type::<ScheduledTaskRunOutput>(&cfg);
    export_type::<ScheduledTaskOperation>(&cfg);
    export_type::<ScheduledTaskBatch>(&cfg);
    export_type::<ScriptListPage>(&cfg);
    export_type::<ScriptDetail>(&cfg);
    export_type::<CreateScriptRequest>(&cfg);
    export_type::<UpdateScriptRequest>(&cfg);
    export_type::<CreateScriptRunRequest>(&cfg);
    export_type::<ScriptRun>(&cfg);
    export_type::<ScriptRunTerminalClientMessage>(&cfg);
    export_type::<ScriptRunTerminalServerMessage>(&cfg);
    export_type::<TerminalAccess>(&cfg);
    export_type::<TerminalClientMessage>(&cfg);
    export_type::<TerminalServerMessage>(&cfg);
}

#[test]
fn suite_operation_event_matches_runtime_sdk_fixture() {
    let request = SuiteOperationEventRequest {
        event_id: "0198f021-7f1a-7000-8000-000000000001".to_string(),
        operation_context_id: Some("0198f021-7f1a-7000-8000-000000000010".to_string()),
        event_code: "scan_submitted".to_string(),
        event_label: OperationEventLabel {
            zh_cn: "提交扫描".to_string(),
            en_us: "Submit scan".to_string(),
        },
        outcome: OperationOutcome::Success,
        impact: OperationImpact::Info,
        target: Some(OperationTarget {
            kind: "scan_task".to_string(),
            id: "task-1".to_string(),
            display_name: Some("192.0.2.0/24".to_string()),
            ownership: None,
        }),
        task_id: Some("task-1".to_string()),
        parameters: BTreeMap::from([(
            "hostCount".to_string(),
            OperationParameterValue::Number(254.0),
        )]),
        error_code: None,
        error_summary: None,
    };
    let actual = serde_json::to_value(request).unwrap();
    let expected: serde_json::Value = serde_json::from_str(
        r#"{
          "eventId": "0198f021-7f1a-7000-8000-000000000001",
          "operationContextId": "0198f021-7f1a-7000-8000-000000000010",
          "eventCode": "scan_submitted",
          "eventLabel": { "zhCn": "提交扫描", "enUs": "Submit scan" },
          "outcome": "success",
          "impact": "info",
          "target": { "kind": "scan_task", "id": "task-1", "displayName": "192.0.2.0/24", "ownership": null },
          "taskId": "task-1",
          "parameters": { "hostCount": 254.0 },
          "errorCode": null,
          "errorSummary": null
        }"#,
    )
    .unwrap();
    assert_eq!(actual, expected);
}
