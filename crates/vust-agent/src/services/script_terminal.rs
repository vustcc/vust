//! 脚本 PTY 会话：一次性启动、原始输入输出与完整进程组回收。

use crate::{models::script_runs, services::pty, state::AppState, types::ApiResult};
use std::{
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    },
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, oneshot},
    time::{Instant, timeout},
};
use vust_contracts::scripts::ScriptRunStatus;

pub const MAX_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_CONTROL_BYTES: usize = 8 * 1024;
const MIN_COLS: u16 = 20;
const MAX_COLS: u16 = 500;
const MIN_ROWS: u16 = 5;
const MAX_ROWS: u16 = 200;
fn script_dir() -> PathBuf {
    crate::config::data_dir().join("script-runs")
}

/// PTY 驱动器事件；输出字节不会写入数据库。
pub enum ScriptTerminalEvent {
    Output(Vec<u8>),
    Exited {
        status: ScriptRunStatus,
        exit_code: Option<i32>,
        ended_at: String,
    },
    Error(String),
}

enum CommandMessage {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close,
}

/// 单次脚本终端会话句柄。
pub struct ScriptTerminalSession {
    commands: mpsc::Sender<CommandMessage>,
}

impl ScriptTerminalSession {
    pub async fn input(&self, data: Vec<u8>) -> Result<(), ()> {
        if data.len() > MAX_INPUT_BYTES {
            return Err(());
        }
        self.commands
            .send(CommandMessage::Input(data))
            .await
            .map_err(|_| ())
    }
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), ()> {
        if !validate_size(cols, rows) {
            return Err(());
        }
        self.commands
            .send(CommandMessage::Resize { cols, rows })
            .await
            .map_err(|_| ())
    }
    pub async fn close(&self) {
        let _ = self.commands.send(CommandMessage::Close).await;
    }
}

pub fn validate_size(cols: u16, rows: u16) -> bool {
    (MIN_COLS..=MAX_COLS).contains(&cols) && (MIN_ROWS..=MAX_ROWS).contains(&rows)
}

/// 创建 PTY 并从权限受限的临时文件执行快照。
pub async fn start(
    state: Arc<AppState>,
    run_id: &str,
    cols: u16,
    rows: u16,
) -> ApiResult<(ScriptTerminalSession, mpsc::Receiver<ScriptTerminalEvent>)> {
    if !validate_size(cols, rows) {
        return Err(crate::types::ApiError::internal("invalid terminal size"));
    }
    let run = script_runs::required(&state.metadata_db, run_id).await?;
    let script_guard = TempScript(write_script(&run.run_id, &run.source_content)?);
    let script_path = script_guard.0.clone();
    let (master_fd, slave_fd) = pty::open()?;
    pty::resize(master_fd.as_raw_fd(), cols, rows)?;
    let slave = File::from(slave_fd);
    let stdin = slave.try_clone()?;
    let stdout = slave.try_clone()?;
    let mut command = Command::new("/bin/bash");
    command
        .args(["--noprofile", "--norc"])
        .arg(&script_path)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(slave))
        .env_clear()
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("HOME", "/tmp")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("TERM", "xterm-256color")
        .kill_on_drop(true);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    script_runs::claim_running(&state.metadata_db, run_id).await?;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Err(error.into()),
    };
    let pid = child
        .id()
        .ok_or_else(|| crate::types::ApiError::internal("script terminal process has no pid"))?
        as libc::pid_t;
    let master = File::from(master_fd);
    let reader = tokio::fs::File::from_std(master.try_clone()?);
    let writer = tokio::fs::File::from_std(master);
    let (exit_tx, exit_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = exit_tx.send(child.wait().await);
    });
    let (command_tx, command_rx) = mpsc::channel(128);
    let (event_tx, event_rx) = mpsc::channel(128);
    let run_id = run_id.to_string();
    tokio::spawn(driver(
        state,
        run_id,
        pid,
        script_guard,
        reader,
        writer,
        command_rx,
        event_tx,
        exit_rx,
        run.timeout_seconds as u64,
    ));
    Ok((
        ScriptTerminalSession {
            commands: command_tx,
        },
        event_rx,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn driver(
    state: Arc<AppState>,
    run_id: String,
    pid: libc::pid_t,
    _script_guard: TempScript,
    mut reader: tokio::fs::File,
    mut writer: tokio::fs::File,
    mut commands: mpsc::Receiver<CommandMessage>,
    events: mpsc::Sender<ScriptTerminalEvent>,
    mut exit_rx: oneshot::Receiver<io::Result<std::process::ExitStatus>>,
    timeout_seconds: u64,
) {
    let deadline = tokio::time::sleep_until(Instant::now() + Duration::from_secs(timeout_seconds));
    tokio::pin!(deadline);
    let mut buffer = [0_u8; 8192];
    let mut pending_output = None;
    let mut forced = None;
    let mut cancel_poll = tokio::time::interval(Duration::from_millis(200));
    cancel_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = loop {
        tokio::select! {
            permit = events.reserve(), if pending_output.is_some() => match permit {
                Ok(permit) => permit.send(ScriptTerminalEvent::Output(pending_output.take().expect("pending output exists"))),
                Err(_) => { forced=Some(ScriptRunStatus::Cancelled); break None; }
            },
            read = reader.read(&mut buffer), if pending_output.is_none() => match read {
                Ok(0) => break timeout(Duration::from_secs(2),&mut exit_rx).await.ok().and_then(Result::ok).and_then(Result::ok),
                Ok(n) => pending_output=Some(buffer[..n].to_vec()),
                Err(e) if pty::is_eof(&e) => break timeout(Duration::from_secs(2),&mut exit_rx).await.ok().and_then(Result::ok).and_then(Result::ok),
                Err(e) => { let _=events.try_send(ScriptTerminalEvent::Error(e.to_string())); forced=Some(ScriptRunStatus::Failed); break None; }
            },
            status = &mut exit_rx => {
                let status=status.ok().and_then(Result::ok);
                if let Some(output)=pending_output.take(){let _=events.send(ScriptTerminalEvent::Output(output)).await;}
                let mut tail=Vec::new();
                let _=timeout(Duration::from_millis(200),reader.read_to_end(&mut tail)).await;
                if !tail.is_empty(){let _=events.send(ScriptTerminalEvent::Output(tail)).await;}
                break status;
            },
            command = commands.recv() => match command { Some(CommandMessage::Input(data)) => if writer.write_all(&data).await.is_err(){ forced=Some(ScriptRunStatus::Failed); break None; }, Some(CommandMessage::Resize{cols,rows}) => { let _=pty::resize(writer.as_raw_fd(),cols,rows); }, Some(CommandMessage::Close)|None => { forced=Some(ScriptRunStatus::Cancelled); break None; } },
            _ = &mut deadline => { forced=Some(ScriptRunStatus::TimedOut); break None; }
            _ = cancel_poll.tick() => if script_runs::cancel_requested(&state.metadata_db,&run_id).await.unwrap_or(true) { forced=Some(ScriptRunStatus::Cancelled); break None; }
        }
    };
    let (status, exit_code) = if let Some(status) = forced {
        if let Some(output) = pending_output.take() {
            let _ = events.try_send(ScriptTerminalEvent::Output(output));
        }
        signal(pid, libc::SIGHUP);
        let waited = timeout(Duration::from_secs(2), &mut exit_rx)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(Result::ok);
        if waited.is_none() {
            signal(pid, libc::SIGKILL);
        }
        (status, waited.and_then(|s| s.code()))
    } else if let Some(exit) = result {
        (
            if exit.success() {
                ScriptRunStatus::Succeeded
            } else {
                ScriptRunStatus::Failed
            },
            exit.code(),
        )
    } else {
        (ScriptRunStatus::Failed, None)
    };
    let (code, summary) = match status {
        ScriptRunStatus::TimedOut => (
            Some("SCRIPT_RUN_TIMED_OUT"),
            Some("script exceeded its timeout"),
        ),
        ScriptRunStatus::Failed => (
            Some("SCRIPT_RUN_EXIT_FAILED"),
            Some("script exited with a non-zero status"),
        ),
        _ => (None, None),
    };
    loop {
        match script_runs::finish(
            &state.metadata_db,
            &run_id,
            status,
            exit_code,
            code,
            summary,
        )
        .await
        {
            Ok(()) => break,
            Err(error) => {
                tracing::warn!(%run_id, %error, "persisting script terminal result will retry");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    let ended_at = chrono::Utc::now().to_rfc3339();
    let _ = events
        .send(ScriptTerminalEvent::Exited {
            status,
            exit_code,
            ended_at,
        })
        .await;
}

fn write_script(run_id: &str, content: &str) -> io::Result<PathBuf> {
    let dir = script_dir();
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{run_id}.sh"));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(content.as_bytes())?;
    Ok(path)
}

struct TempScript(PathBuf);
impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Agent 启动时清理上次进程遗留的脚本终端临时文件。
pub fn cleanup_stale_files() {
    let Ok(entries) = std::fs::read_dir(script_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn signal(pid: libc::pid_t, value: i32) {
    pty::signal_group(pid, value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use vust_contracts::scripts::{AgentStartScriptRunRequest, ScriptOwnershipKind};

    #[tokio::test]
    async fn pty_accepts_multiline_input_and_cleans_restricted_script_file() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        let content = "printf 'Name: '; read name\nprintf 'Hello %s\\n' \"$name\"\n".to_string();
        let request = AgentStartScriptRunRequest {
            run_id: "terminal-pty-test".into(),
            script_id: "script-pty".into(),
            script_name: "PTY".into(),
            script_revision: 1,
            source_sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
            source_content: content,
            timeout_seconds: 10,
            ownership_kind: ScriptOwnershipKind::Custom,
        };
        script_runs::create(&state.metadata_db, &request)
            .await
            .unwrap();
        script_runs::mark_awaiting_connection(&state.metadata_db, &request.run_id)
            .await
            .unwrap();
        script_runs::attach_terminal(&state.metadata_db, &request.run_id)
            .await
            .unwrap();
        let (session, mut events) = start(Arc::clone(&state), &request.run_id, 80, 24)
            .await
            .unwrap();
        let path = script_dir().join(format!("{}.sh", request.run_id));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        session.input(b"Alice\n".to_vec()).await.unwrap();
        let mut output = Vec::new();
        let mut terminal = None;
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(5), events.recv()).await
        {
            match event {
                ScriptTerminalEvent::Output(bytes) => output.extend(bytes),
                ScriptTerminalEvent::Exited { status, .. } => {
                    terminal = Some(status);
                    break;
                }
                ScriptTerminalEvent::Error(error) => panic!("PTY error: {error}"),
            }
        }
        assert!(String::from_utf8_lossy(&output).contains("Hello Alice"));
        assert_eq!(terminal, Some(ScriptRunStatus::Succeeded));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn output_backpressure_does_not_block_total_timeout() {
        let state = Arc::new(crate::test_support::setup_test_state().await);
        let content =
            "while :; do printf '0123456789abcdef0123456789abcdef\\n'; done\n".to_string();
        let request = AgentStartScriptRunRequest {
            run_id: "terminal-backpressure-test".into(),
            script_id: "script-backpressure".into(),
            script_name: "Backpressure".into(),
            script_revision: 1,
            source_sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
            source_content: content,
            timeout_seconds: 1,
            ownership_kind: ScriptOwnershipKind::Custom,
        };
        script_runs::create(&state.metadata_db, &request)
            .await
            .unwrap();
        script_runs::mark_awaiting_connection(&state.metadata_db, &request.run_id)
            .await
            .unwrap();
        script_runs::attach_terminal(&state.metadata_db, &request.run_id)
            .await
            .unwrap();
        let (_session, _events) = start(Arc::clone(&state), &request.run_id, 80, 24)
            .await
            .unwrap();

        let terminal = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let run = script_runs::required(&state.metadata_db, &request.run_id)
                    .await
                    .unwrap();
                if run.status == "timed_out" {
                    break run;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(terminal.error_code.as_deref(), Some("SCRIPT_RUN_TIMED_OUT"));
    }
}
