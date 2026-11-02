use std::fs::File as StdFile;
use std::io;

use rustix::fd::OwnedFd;
use rustix::process::{self, Pid, Signal};
use rustix::pty::{self, OpenptFlags};
use rustix::termios::{self, Winsize};
use tokio::process::Command;

#[cfg(not(target_os = "linux"))]
use std::fs::OpenOptions;

pub(super) fn openpty_pair_files() -> io::Result<(StdFile, StdFile)> {
    let master_fd = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    pty::grantpt(&master_fd)?;
    pty::unlockpt(&master_fd)?;

    let slave_fd = openpty_slave_fd(&master_fd)?;

    let master = StdFile::from(master_fd);
    let slave = StdFile::from(slave_fd);
    Ok((master, slave))
}

#[cfg(target_os = "linux")]
fn openpty_slave_fd(master_fd: &OwnedFd) -> io::Result<OwnedFd> {
    pty::ioctl_tiocgptpeer(master_fd, OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(Into::into)
}

#[cfg(not(target_os = "linux"))]
fn openpty_slave_fd(master_fd: &OwnedFd) -> io::Result<OwnedFd> {
    let slave_name = pty::ptsname(master_fd, Vec::new())?;
    let slave_path = slave_name
        .into_string()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Non-UTF-8 PTY path"))?;
    let slave_file = OpenOptions::new().read(true).write(true).open(slave_path)?;
    Ok(slave_file.into())
}

pub(super) fn set_terminal_winsize(pty_file: &StdFile, cols: u16, rows: u16) -> io::Result<()> {
    let winsize = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    termios::tcsetwinsize(pty_file, winsize).map_err(Into::into)
}

pub(super) fn kill_process_group_sigwinch(pid: i32) -> io::Result<()> {
    let Some(pid) = Pid::from_raw(pid) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid process id for SIGWINCH",
        ));
    };
    process::kill_process_group(pid, Signal::WINCH).map_err(Into::into)
}

#[allow(
    unsafe_code,
    reason = "Command::pre_exec is unsafe by contract: the closure runs after fork and must stay async-signal-safe, and the TIOCSCTTY ioctl that makes the child's slave PTY its controlling terminal has no safe wrapper usable at that point"
)]
pub(super) fn configure_child_process_group(command: &mut Command) {
    // The child must own a controlling terminal so SIGWINCH propagates a
    // window-change to the remote shell. At `pre_exec` time the child's stdio
    // (fds 0/1/2) are already dup'd to the slave PTY, so we:
    //   1. start a new session via `setsid()` — this detaches any inherited
    //      controlling tty and makes the child its own session + group leader
    //      (pgid == pid), which keeps `kill_process_group_sigwinch(pid)` valid;
    //   2. acquire the slave PTY (fd 0) as the controlling terminal via
    //      `ioctl(0, TIOCSCTTY, 0)`.
    //
    // The closure runs after fork and must be async-signal-safe: only raw
    // syscalls, no allocation or locking. `setsid()` also subsumes the prior
    // `process_group(0)` semantics (new session => new process group).
    //
    // SAFETY: the closure performs only async-signal-safe syscalls and does not
    // touch any shared state of the parent.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
