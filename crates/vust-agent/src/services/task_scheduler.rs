//! Agent 计划任务执行器：统一处理定时、手动、取消、超时和有界输出。

use crate::{models::scheduled_tasks, state::AppState};
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};
use vust_contracts::scheduled_tasks::{
    ScheduledTaskRun, ScheduledTaskRunStatus, ScheduledTaskTriggerSource,
};

const STREAM_OUTPUT_LIMIT: usize = scheduled_tasks::MAX_OUTPUT_BYTES / 2;

/// 启动分钟级调度器，并在启动时收敛上一次进程留下的活动记录。
pub fn spawn_scheduler(state: Arc<AppState>) {
    tokio::spawn(async move {
        if let Err(error) = scheduled_tasks::recover_interrupted_runs(&state.metadata_db).await {
            tracing::error!(%error, "failed to recover interrupted scheduled task runs");
        }
        loop {
            let now = chrono::Utc::now();
            let wait = 60 - now.timestamp().rem_euclid(60);
            tokio::time::sleep(Duration::from_secs(wait as u64)).await;
            if let Err(error) = dispatch_due_tasks(Arc::clone(&state)).await {
                tracing::warn!(%error, "scheduled task dispatch failed");
            }
        }
    });
}

/// 创建一次幂等后台运行并立即返回可跟踪 DTO。
pub async fn submit_run(
    state: Arc<AppState>,
    task_id: &str,
    run_id: &str,
    trigger_source: ScheduledTaskTriggerSource,
) -> crate::types::ApiResult<ScheduledTaskRun> {
    let task = scheduled_tasks::get_task(&state.metadata_db, task_id)
        .await?
        .ok_or_else(|| {
            crate::types::ApiError::not_found(
                vust_contracts::api::ErrorCode::ScheduledTaskNotFound,
                "scheduled task not found",
            )
        })?;
    let (run, created) =
        scheduled_tasks::create_run(&state.metadata_db, &task, run_id, trigger_source).await?;
    let dto = scheduled_tasks::run_dto(run)?;
    if !created {
        return Ok(dto);
    }
    let run_id = run_id.to_string();
    tokio::spawn(async move {
        if let Err(error) = execute_run(Arc::clone(&state), task, run_id.clone()).await {
            tracing::error!(%error, %run_id, "scheduled task execution failed unexpectedly");
        }
    });
    Ok(dto)
}

async fn dispatch_due_tasks(state: Arc<AppState>) -> crate::types::ApiResult<()> {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    for task in scheduled_tasks::list_due_tasks(&state.metadata_db, &now).await? {
        scheduled_tasks::advance_next_run(&state.metadata_db, &task).await?;
        let run_id = uuid::Uuid::now_v7().to_string();
        match scheduled_tasks::create_run(
            &state.metadata_db,
            &task,
            &run_id,
            ScheduledTaskTriggerSource::Schedule,
        )
        .await
        {
            Ok((_, true)) => {
                let task_state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(error) = execute_run(task_state, task, run_id.clone()).await {
                        tracing::error!(%error, %run_id, "scheduled task execution failed unexpectedly");
                    }
                });
            }
            Ok((_, false)) => {}
            Err(error)
                if error.code == vust_contracts::api::ErrorCode::ScheduledTaskOperationConflict =>
            {
                tracing::info!(task_id = %task.task_id, "scheduled task trigger skipped because overlap prevention is active");
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn execute_run(
    state: Arc<AppState>,
    task: scheduled_tasks::AgentScheduledTask,
    run_id: String,
) -> crate::types::ApiResult<()> {
    scheduled_tasks::mark_run_starting(&state.metadata_db, &run_id).await?;
    if scheduled_tasks::is_cancel_requested(&state.metadata_db, &run_id).await? {
        finish_cancelled(&state, &run_id, &[]).await?;
        return Ok(());
    }

    let mut command = Command::new("/bin/bash");
    command
        .arg("-lc")
        .arg(&task.command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            scheduled_tasks::finish_run(
                &state.metadata_db,
                &run_id,
                scheduled_tasks::FinishTaskRun {
                    status: ScheduledTaskRunStatus::Failed,
                    exit_code: None,
                    error_code: Some("SCHEDULED_TASK_EXECUTION_FAILED"),
                    error_summary: Some(&format!("failed to start scheduled task: {error}")),
                    output: &[],
                    output_truncated: false,
                },
            )
            .await?;
            return Ok(());
        }
    };
    let pid = child.id().map(|value| value as libc::pid_t);
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader =
        tokio::spawn(async move { read_bounded(stdout, STREAM_OUTPUT_LIMIT).await });
    let stderr_reader =
        tokio::spawn(async move { read_bounded(stderr, STREAM_OUTPUT_LIMIT).await });
    scheduled_tasks::mark_run_running(&state.metadata_db, &run_id).await?;

    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(task.timeout_seconds.clamp(1, 86_400) as u64);
    let mut poll = tokio::time::interval(Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let outcome = loop {
        tokio::select! {
            status = child.wait() => break RunOutcome::Exited(status),
            _ = tokio::time::sleep_until(deadline) => {
                terminate_process_group(pid).await;
                let status = child.wait().await;
                break RunOutcome::TimedOut(status);
            }
            _ = poll.tick() => {
                if scheduled_tasks::is_cancel_requested(&state.metadata_db, &run_id).await? {
                    terminate_process_group(pid).await;
                    let status = child.wait().await;
                    break RunOutcome::Cancelled(status);
                }
            }
        }
    };

    let stdout = stdout_reader.await.unwrap_or_else(|_| (Vec::new(), true));
    let stderr = stderr_reader.await.unwrap_or_else(|_| (Vec::new(), true));
    let (output, truncated) = merge_output(stdout, stderr);
    let (status, exit_code, error_code, error_summary) = match outcome {
        RunOutcome::Exited(Ok(exit)) if exit.success() => {
            (ScheduledTaskRunStatus::Succeeded, exit.code(), None, None)
        }
        RunOutcome::Exited(Ok(exit)) => (
            ScheduledTaskRunStatus::Failed,
            exit.code(),
            Some("SCHEDULED_TASK_EXECUTION_FAILED"),
            Some("scheduled task exited with a non-zero status"),
        ),
        RunOutcome::Exited(Err(_)) => (
            ScheduledTaskRunStatus::Failed,
            None,
            Some("SCHEDULED_TASK_EXECUTION_FAILED"),
            Some("failed to wait for the scheduled task process"),
        ),
        RunOutcome::TimedOut(result) => (
            ScheduledTaskRunStatus::TimedOut,
            result.ok().and_then(|value| value.code()),
            Some("SCHEDULED_TASK_EXECUTION_TIMEOUT"),
            Some("scheduled task exceeded its timeout"),
        ),
        RunOutcome::Cancelled(result) => (
            ScheduledTaskRunStatus::Cancelled,
            result.ok().and_then(|value| value.code()),
            None,
            None,
        ),
    };
    scheduled_tasks::finish_run(
        &state.metadata_db,
        &run_id,
        scheduled_tasks::FinishTaskRun {
            status,
            exit_code,
            error_code,
            error_summary,
            output: &output,
            output_truncated: truncated,
        },
    )
    .await?;
    Ok(())
}

async fn finish_cancelled(
    state: &AppState,
    run_id: &str,
    output: &[u8],
) -> crate::types::ApiResult<()> {
    scheduled_tasks::finish_run(
        &state.metadata_db,
        run_id,
        scheduled_tasks::FinishTaskRun {
            status: ScheduledTaskRunStatus::Cancelled,
            exit_code: None,
            error_code: None,
            error_summary: None,
            output,
            output_truncated: false,
        },
    )
    .await?;
    Ok(())
}

async fn read_bounded<R>(reader: Option<R>, limit: usize) -> (Vec<u8>, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return (Vec::new(), false);
    };
    let mut kept = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let remaining = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..read.min(remaining)]);
                truncated |= read > remaining;
            }
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    (kept, truncated)
}

fn merge_output(stdout: (Vec<u8>, bool), stderr: (Vec<u8>, bool)) -> (Vec<u8>, bool) {
    let mut merged = Vec::new();
    if !stdout.0.is_empty() {
        merged.extend_from_slice(b"[stdout]\n");
        merged.extend_from_slice(&stdout.0);
    }
    if !stderr.0.is_empty() {
        if !merged.is_empty() {
            merged.extend_from_slice(b"\n\n");
        }
        merged.extend_from_slice(b"[stderr]\n");
        merged.extend_from_slice(&stderr.0);
    }
    let oversized = merged.len() > scheduled_tasks::MAX_OUTPUT_BYTES;
    merged.truncate(scheduled_tasks::MAX_OUTPUT_BYTES);
    (merged, stdout.1 || stderr.1 || oversized)
}

async fn terminate_process_group(pid: Option<libc::pid_t>) {
    let Some(pid) = pid else { return };
    // SAFETY: pid 来自刚创建的子进程，负 pid 仅向该子进程组发送信号。
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    // SAFETY: 同上；进程已退出时 kill 返回 ESRCH，不影响状态收敛。
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

enum RunOutcome {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut(std::io::Result<std::process::ExitStatus>),
    Cancelled(std::io::Result<std::process::ExitStatus>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_reader_drains_and_truncates() {
        let data = [b'x'; 32];
        let (kept, truncated) = read_bounded(Some(&data[..]), 8).await;
        assert_eq!(kept.len(), 8);
        assert!(truncated);
    }

    #[test]
    fn merged_output_never_exceeds_limit() {
        let data = vec![b'x'; scheduled_tasks::MAX_OUTPUT_BYTES];
        let (merged, truncated) = merge_output((data.clone(), false), (data, false));
        assert_eq!(merged.len(), scheduled_tasks::MAX_OUTPUT_BYTES);
        assert!(truncated);
    }
}
