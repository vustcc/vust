//! 宿主机 PTY 会话核心。

use std::time::Duration;
use tokio::sync::mpsc;
use vust_contracts::terminal::{
    TerminalAvailability, TerminalErrorCode, TerminalExitReason, TerminalRuntimeAccess,
    TerminalServerMessage, TerminalShell,
};

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const IDLE_WARNING_AFTER: Duration = Duration::from_secs(25 * 60);
pub const IDLE_WARNING_REMAINING_SECONDS: u64 = 5 * 60;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_CONTROL_BYTES: usize = 8 * 1024;
pub const MIN_COLS: u16 = 20;
pub const MAX_COLS: u16 = 500;
pub const MIN_ROWS: u16 = 5;
pub const MAX_ROWS: u16 = 200;

/// PTY 驱动器向 WebSocket 层发送的事件。
#[derive(Debug)]
pub enum HostTerminalEvent {
    Control(TerminalServerMessage),
    Output(Vec<u8>),
}

#[derive(Debug)]
enum HostTerminalCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close(TerminalExitReason),
}

/// 单个宿主机 PTY 会话的控制句柄。
pub struct HostTerminalSession {
    pub session_id: String,
    pub shell: TerminalShell,
    commands: mpsc::Sender<HostTerminalCommand>,
    task: tokio::task::JoinHandle<()>,
}

impl HostTerminalSession {
    /// 按接收顺序向 PTY 写入原始输入字节。
    pub async fn input(&self, data: Vec<u8>) -> Result<(), TerminalErrorCode> {
        if data.len() > MAX_INPUT_BYTES {
            return Err(TerminalErrorCode::TerminalProtocolViolation);
        }
        self.commands
            .send(HostTerminalCommand::Input(data))
            .await
            .map_err(|_| TerminalErrorCode::TerminalIoFailed)
    }

    /// 串行调整 PTY 尺寸。
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), TerminalErrorCode> {
        validate_size(cols, rows)?;
        self.commands
            .send(HostTerminalCommand::Resize { cols, rows })
            .await
            .map_err(|_| TerminalErrorCode::TerminalIoFailed)
    }

    /// 请求驱动器关闭；调用方应继续消费事件直到收到唯一终态。
    pub async fn request_close(&self, reason: TerminalExitReason) -> Result<(), TerminalErrorCode> {
        self.commands
            .send(HostTerminalCommand::Close(reason))
            .await
            .map_err(|_| TerminalErrorCode::TerminalIoFailed)
    }

    /// 等待驱动器完成子进程回收。
    pub async fn wait(self) {
        let _ = self.task.await;
    }
}

/// 返回 Agent 当前真实的宿主机终端能力。
pub fn runtime_access() -> TerminalRuntimeAccess {
    let shell = resolve_shell();
    TerminalRuntimeAccess {
        availability: if shell.is_some() {
            TerminalAvailability::Available
        } else if cfg!(unix) {
            TerminalAvailability::Unavailable
        } else {
            TerminalAvailability::Unsupported
        },
        shell,
        idle_timeout_seconds: IDLE_TIMEOUT.as_secs(),
        can_start_session: shell.is_some(),
        unavailable_reason: shell
            .is_none()
            .then_some(TerminalErrorCode::TerminalUnavailable),
    }
}

/// 校验前端计算出的终端网格大小。
pub fn validate_size(cols: u16, rows: u16) -> Result<(), TerminalErrorCode> {
    if !(MIN_COLS..=MAX_COLS).contains(&cols) || !(MIN_ROWS..=MAX_ROWS).contains(&rows) {
        return Err(TerminalErrorCode::TerminalInvalidSize);
    }
    Ok(())
}

fn resolve_shell() -> Option<TerminalShell> {
    if std::path::Path::new(TerminalShell::Bash.path()).is_file() {
        return Some(TerminalShell::Bash);
    }
    if std::path::Path::new(TerminalShell::Sh.path()).is_file() {
        return Some(TerminalShell::Sh);
    }
    None
}

/// 创建单一 PTY 会话及其有界事件流。
#[cfg(unix)]
pub async fn start(
    cols: u16,
    rows: u16,
) -> Result<(HostTerminalSession, mpsc::Receiver<HostTerminalEvent>), String> {
    unix::start(cols, rows).await
}

/// 非 Unix 平台不提供宿主机 PTY。
#[cfg(not(unix))]
pub async fn start(
    _cols: u16,
    _rows: u16,
) -> Result<(HostTerminalSession, mpsc::Receiver<HostTerminalEvent>), String> {
    Err("host terminal is only supported on Unix platforms".to_string())
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::services::pty;
    use chrono::Utc;
    use std::{fs::File, io, os::fd::AsRawFd, process::Stdio};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        process::Command,
        sync::oneshot,
        time::{Instant, sleep_until, timeout},
    };

    pub async fn start(
        cols: u16,
        rows: u16,
    ) -> Result<(HostTerminalSession, mpsc::Receiver<HostTerminalEvent>), String> {
        validate_size(cols, rows).map_err(|_| "invalid terminal size".to_string())?;
        let shell = resolve_shell().ok_or_else(|| "no supported shell is available".to_string())?;
        let (master_fd, slave_fd) = pty::open().map_err(|error| error.to_string())?;
        pty::resize(master_fd.as_raw_fd(), cols, rows).map_err(|error| error.to_string())?;

        let slave = File::from(slave_fd);
        let stdin = slave.try_clone().map_err(|error| error.to_string())?;
        let stdout = slave.try_clone().map_err(|error| error.to_string())?;
        let mut command = Command::new(shell.path());
        command
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(slave))
            .env("TERM", "xterm-256color")
            .kill_on_drop(true);
        if let Some(home) = std::env::var_os("HOME") {
            command.current_dir(home);
        }
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let child_pid = child
            .id()
            .ok_or_else(|| "terminal process did not expose a pid".to_string())?
            as libc::pid_t;
        let master = File::from(master_fd);
        let reader = master.try_clone().map_err(|error| error.to_string())?;
        let reader = tokio::fs::File::from_std(reader);
        let writer = tokio::fs::File::from_std(master);
        let (exit_tx, exit_rx) = oneshot::channel();
        tokio::spawn(async move {
            let _ = exit_tx.send(child.wait().await);
        });

        let session_id = uuid::Uuid::new_v4().to_string();
        let (command_tx, command_rx) = mpsc::channel(128);
        let (event_tx, event_rx) = mpsc::channel(128);
        let driver_session_id = session_id.clone();
        let task = tokio::spawn(async move {
            run_driver(
                driver_session_id,
                child_pid,
                reader,
                writer,
                command_rx,
                event_tx,
                exit_rx,
            )
            .await;
        });
        Ok((
            HostTerminalSession {
                session_id,
                shell,
                commands: command_tx,
                task,
            },
            event_rx,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_driver(
        session_id: String,
        child_pid: libc::pid_t,
        mut reader: tokio::fs::File,
        mut writer: tokio::fs::File,
        mut commands: mpsc::Receiver<HostTerminalCommand>,
        events: mpsc::Sender<HostTerminalEvent>,
        mut exit_rx: oneshot::Receiver<io::Result<std::process::ExitStatus>>,
    ) {
        let mut buffer = [0_u8; 8192];
        let warning = sleep_until(Instant::now() + IDLE_WARNING_AFTER);
        let idle = sleep_until(Instant::now() + IDLE_TIMEOUT);
        tokio::pin!(warning);
        tokio::pin!(idle);
        let mut warning_sent = false;
        let mut exit_code = None;
        let reason = loop {
            tokio::select! {
                read = reader.read(&mut buffer) => match read {
                    Ok(0) => {
                        break TerminalExitReason::ProcessExited;
                    }
                    Ok(count) => {
                        if events.send(HostTerminalEvent::Output(buffer[..count].to_vec())).await.is_err() {
                            break TerminalExitReason::TransportClosed;
                        }
                    }
                    Err(error) if pty::is_eof(&error) => {
                        break TerminalExitReason::ProcessExited;
                    }
                    Err(error) => {
                        let _ = send_error(&events, TerminalErrorCode::TerminalIoFailed, format!("failed to read terminal output: {error}"), false).await;
                        break TerminalExitReason::IoFailed;
                    }
                },
                status = &mut exit_rx => {
                    exit_code = status.ok().and_then(Result::ok).and_then(|status| status.code());
                    break TerminalExitReason::ProcessExited;
                }
                command = commands.recv() => match command {
                    Some(HostTerminalCommand::Input(data)) => {
                        if let Err(error) = writer.write_all(&data).await {
                            let _ = send_error(&events, TerminalErrorCode::TerminalIoFailed, format!("failed to write terminal input: {error}"), false).await;
                            break TerminalExitReason::IoFailed;
                        }
                        warning.as_mut().reset(Instant::now() + IDLE_WARNING_AFTER);
                        idle.as_mut().reset(Instant::now() + IDLE_TIMEOUT);
                        warning_sent = false;
                    }
                    Some(HostTerminalCommand::Resize { cols, rows }) => {
                        if let Err(error) = pty::resize(writer.as_raw_fd(), cols, rows) {
                            let _ = send_error(&events, TerminalErrorCode::TerminalIoFailed, format!("failed to resize terminal: {error}"), true).await;
                        }
                    }
                    Some(HostTerminalCommand::Close(reason)) => break reason,
                    None => break TerminalExitReason::TransportClosed,
                },
                _ = &mut warning, if !warning_sent => {
                    warning_sent = true;
                    let _ = events.send(HostTerminalEvent::Control(TerminalServerMessage::IdleWarning {
                        expires_in_seconds: IDLE_WARNING_REMAINING_SECONDS,
                    })).await;
                }
                _ = &mut idle => break TerminalExitReason::IdleTimeout,
            }
        };

        if reason != TerminalExitReason::ProcessExited {
            pty::signal_group(child_pid, libc::SIGHUP);
            if let Ok(Ok(status)) = timeout(Duration::from_secs(3), &mut exit_rx).await {
                exit_code = status.ok().and_then(|status| status.code());
            } else {
                pty::signal_group(child_pid, libc::SIGKILL);
                if let Ok(Ok(status)) = timeout(Duration::from_secs(3), &mut exit_rx).await {
                    exit_code = status.ok().and_then(|status| status.code());
                }
            }
        } else if exit_code.is_none()
            && let Ok(Ok(status)) = timeout(Duration::from_secs(3), &mut exit_rx).await
        {
            exit_code = status.ok().and_then(|status| status.code());
        }

        let _ = events
            .send(HostTerminalEvent::Control(TerminalServerMessage::Exited {
                session_id,
                exit_code,
                reason,
                ended_at: Utc::now().to_rfc3339(),
            }))
            .await;
    }

    async fn send_error(
        events: &mpsc::Sender<HostTerminalEvent>,
        code: TerminalErrorCode,
        message: String,
        recoverable: bool,
    ) -> Result<(), mpsc::error::SendError<HostTerminalEvent>> {
        events
            .send(HostTerminalEvent::Control(TerminalServerMessage::Error {
                code,
                message,
                recoverable,
            }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_terminal_grid_boundaries() {
        assert!(validate_size(MIN_COLS, MIN_ROWS).is_ok());
        assert!(validate_size(MAX_COLS, MAX_ROWS).is_ok());
        assert_eq!(
            validate_size(MIN_COLS - 1, MIN_ROWS),
            Err(TerminalErrorCode::TerminalInvalidSize)
        );
        assert_eq!(
            validate_size(MAX_COLS, MAX_ROWS + 1),
            Err(TerminalErrorCode::TerminalInvalidSize)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preserves_raw_utf8_and_reports_real_exit_code() {
        let (session, mut events) = start(80, 24).await.expect("PTY should start");
        let session_id = session.session_id.clone();
        session
            .input("printf '中文'; exit 7\n".as_bytes().to_vec())
            .await
            .unwrap();
        let mut output = Vec::new();
        let mut exit = None;
        let mut terminal_error = false;
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(5), events.recv()).await
        {
            match event {
                HostTerminalEvent::Output(bytes) => output.extend(bytes),
                HostTerminalEvent::Control(TerminalServerMessage::Exited {
                    session_id: exited_session,
                    exit_code,
                    reason,
                    ..
                }) => {
                    assert_eq!(exited_session, session_id);
                    assert_eq!(reason, TerminalExitReason::ProcessExited);
                    exit = exit_code;
                    break;
                }
                HostTerminalEvent::Control(TerminalServerMessage::Error { .. }) => {
                    terminal_error = true;
                }
                HostTerminalEvent::Control(_) => {}
            }
        }
        assert!(
            output
                .windows("中文".len())
                .any(|bytes| bytes == "中文".as_bytes())
        );
        assert_eq!(exit, Some(7));
        assert!(!terminal_error);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn explicit_close_emits_one_terminal_event() {
        let (session, mut events) = start(80, 24).await.expect("PTY should start");
        session
            .request_close(TerminalExitReason::UserClosed)
            .await
            .expect("close command should be queued");
        let mut terminal_events = 0;
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(5), events.recv()).await
        {
            if let HostTerminalEvent::Control(TerminalServerMessage::Exited { reason, .. }) = event
            {
                terminal_events += 1;
                assert_eq!(reason, TerminalExitReason::UserClosed);
            }
        }
        assert_eq!(terminal_events, 1);
    }

    #[cfg(unix)]
    #[test]
    fn linux_pty_eio_is_normal_eof() {
        let error = std::io::Error::from_raw_os_error(libc::EIO);
        assert!(crate::services::pty::is_eof(&error));
    }
}
