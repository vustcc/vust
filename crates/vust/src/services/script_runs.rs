//! Master 脚本运行协调器：准备一次性终端会话并可靠投递取消。

use crate::{
    models::{
        logging::{LogModule, LogStatus, PlatformLogLevel},
        node_runtime_client::NodeRuntimeClient,
        scripts,
    },
    services::logging::OperationEventBuilder,
    state::AppState,
    types::{ApiError, ApiResult},
};
use std::{sync::Arc, time::Duration};
use vust_contracts::{api::ApiResponse, scripts::AgentStartScriptRunRequest};

const AGENT_BASE: &str = "/api/v1/agent/script-runs";

/// 启动单实例维护循环，服务重启后继续处理连接超时和 cancelling 运行。
pub fn spawn_worker(state: Arc<AppState>) {
    let dispatch_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = dispatch_pending(&dispatch_state).await {
                tracing::warn!(%error, "script run dispatch cycle failed");
            }
        }
    });
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = scripts::cleanup(&state.metadata_db).await {
                tracing::warn!(%error, "script run retention cleanup failed");
            }
        }
    });
}

async fn dispatch_pending(state: &AppState) -> ApiResult<()> {
    for run_id in scripts::expire_unattached_terminals(&state.metadata_db).await? {
        let run = scripts::required_run_row(&state.metadata_db, &run_id).await?;
        if run.status != "cancelling" {
            record_terminal(state, &run_id).await?;
        }
    }
    for run in scripts::cancelling_runs(&state.metadata_db).await? {
        if let Err(error) = dispatch_cancel(state, &run).await {
            tracing::warn!(run_id = %run.run_id, node_id = %run.node_id, %error, "script run cancellation delivery will retry");
        } else if run.error_code.as_deref() == Some("SCRIPT_RUN_TERMINAL_ATTACH_TIMEOUT") {
            scripts::finish_terminal_attach_timeout(&state.metadata_db, &run.run_id).await?;
            record_terminal(state, &run.run_id).await?;
        }
    }
    Ok(())
}

async fn record_terminal(state: &AppState, run_id: &str) -> ApiResult<()> {
    let Some(run) = scripts::claim_terminal_audit(&state.metadata_db, run_id).await? else {
        return Ok(());
    };
    if let Ok(client_ip) = run.client_ip.parse() {
        let failed = run.status != "succeeded";
        OperationEventBuilder::new(&run.actor_name, "script_run_completed", client_ip)
            .user_id(run.actor_user_id)
            .module(LogModule::System)
            .target_type("script")
            .target_id(&run.script_id)
            .target_display_name(&run.script_name)
            .task_id(&run.run_id)
            .trace_id(&run.trace_id)
            .status(if failed {
                LogStatus::Failed
            } else {
                LogStatus::Success
            })
            .level(if failed {
                PlatformLogLevel::Error
            } else {
                PlatformLogLevel::Info
            })
            .metadata(serde_json::json!({
                "runId": run.run_id,
                "scriptName": run.script_name,
                "revision": run.script_revision,
                "nodeId": run.node_id,
                "result": run.status,
                "errorCode": run.error_code,
            }))
            .finish(&state.metadata_db);
    } else {
        tracing::error!(%run_id, "trusted script run client IP is invalid");
    }
    scripts::cleanup_discarded_run(&state.metadata_db, run_id).await?;
    Ok(())
}

/// 将网关建连失败收敛为终态，完成审计后立即销毁临时运行。
pub async fn fail_terminal(state: &AppState, run_id: &str, summary: &str) -> ApiResult<()> {
    scripts::fail_dispatch(&state.metadata_db, run_id, summary).await?;
    record_terminal(state, run_id).await
}

/// 将已领取的终端运行快照送达 Agent，但在 WebSocket 绑定前不启动 PTY。
pub async fn prepare_terminal(
    state: &AppState,
    run: &scripts::ScriptRunRow,
) -> ApiResult<NodeRuntimeClient> {
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&run.node_id)).await?;
    let request = AgentStartScriptRunRequest {
        run_id: run.run_id.clone(),
        script_id: run.script_id.clone(),
        script_name: run.script_name.clone(),
        script_revision: run.script_revision,
        source_content: run.source_content.clone(),
        source_sha256: run.source_sha256.clone(),
        timeout_seconds: run.timeout_seconds.clamp(1, 86_400) as u32,
        ownership_kind: vust_contracts::scripts::ScriptOwnershipKind::Custom,
    };
    let response: ApiResponse<serde_json::Value> = client
        .post_json(AGENT_BASE, &request)
        .await
        .map_err(external_error)?;
    if !response.success {
        return Err(ApiError::conflict(
            response
                .error_code
                .unwrap_or(vust_contracts::api::ErrorCode::ScriptRunConflict),
            response.message,
        ));
    }
    Ok(client)
}

/// Agent WebSocket 建立失败后尽力取消已准备的本地运行。
pub async fn cancel_prepared_terminal(client: &NodeRuntimeClient, run_id: &str) {
    let _: Result<ApiResponse<serde_json::Value>, _> = client
        .post_json(
            &format!("{AGENT_BASE}/{run_id}/cancel"),
            &serde_json::json!({}),
        )
        .await;
}

async fn dispatch_cancel(state: &AppState, run: &scripts::ScriptRunRow) -> ApiResult<()> {
    let client = NodeRuntimeClient::from_node_route(&state.metadata_db, Some(&run.node_id)).await?;
    let response: ApiResponse<serde_json::Value> = client
        .post_json(
            &format!("{AGENT_BASE}/{}/cancel", run.run_id),
            &serde_json::json!({}),
        )
        .await
        .map_err(external_error)?;
    if response.success {
        Ok(())
    } else {
        Err(ApiError::conflict(
            response
                .error_code
                .unwrap_or(vust_contracts::api::ErrorCode::ScriptRunNotCancellable),
            response.message,
        ))
    }
}

fn external_error(error: anyhow::Error) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        vust_contracts::api::ErrorCode::ScriptNodeUnavailable,
        format!("script execution node is unavailable: {error}"),
    )
}
