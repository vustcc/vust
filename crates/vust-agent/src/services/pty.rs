//! Unix PTY 的最小共享基础能力；领域会话策略由各调用方维护。

#[cfg(unix)]
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

#[cfg(unix)]
#[repr(C)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg(unix)]
pub fn open() -> io::Result<(OwnedFd, OwnedFd)> {
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master < 0 {
            return Err(io::Error::last_os_error());
        }
        let master = OwnedFd::from_raw_fd(master);
        if libc::grantpt(master.as_raw_fd()) < 0 || libc::unlockpt(master.as_raw_fd()) < 0 {
            return Err(io::Error::last_os_error());
        }
        let name = libc::ptsname(master.as_raw_fd());
        if name.is_null() {
            return Err(io::Error::last_os_error());
        }
        let slave = libc::open(name, libc::O_RDWR | libc::O_NOCTTY);
        if slave < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((master, OwnedFd::from_raw_fd(slave)))
    }
}

#[cfg(unix)]
pub fn resize(fd: i32, cols: u16, rows: u16) -> io::Result<()> {
    let size = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &size) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub fn signal_group(pid: libc::pid_t, signal: i32) {
    unsafe {
        libc::kill(-pid, signal);
    }
}

#[cfg(unix)]
pub fn is_eof(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::EIO)
}
