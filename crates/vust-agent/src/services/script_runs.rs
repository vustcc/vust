//! Agent 脚本运行服务：接收不可变快照并等待一次性终端连接。

use crate::{models::script_runs, state::AppState, types::ApiResult};
use std::sync::Arc;
use vust_contracts::scripts::AgentStartScriptRunRequest;

/// 幂等保存 Master 下发的脚本快照，并进入等待 WebSocket 连接状态。
pub async fn submit(
    state: Arc<AppState>,
    request: AgentStartScriptRunRequest,
) -> ApiResult<String> {
    let (run, created) = script_runs::create(&state.metadata_db, &request).await?;
    if created {
        script_runs::mark_awaiting_connection(&state.metadata_db, &run.run_id).await?;
    }
    Ok(run.run_id)
}

/// 启动时收敛上一次 Agent 进程遗留的活动运行并清理临时脚本。
pub async fn recover(state: &AppState) -> ApiResult<()> {
    crate::services::script_terminal::cleanup_stale_files();
    script_runs::recover_interrupted(&state.metadata_db).await
}
