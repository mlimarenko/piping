use std::fs::File as StdFile;

use crate::errors::AppError;

use super::unix_sys;

pub(crate) fn create_pty_pair() -> Result<(StdFile, StdFile), AppError> {
    unix_sys::openpty_pair_files()
        .map_err(|error| AppError::External(format!("Failed to allocate PTY: {error}")))
}

pub(crate) fn apply_pty_resize(pty_file: &StdFile, cols: u16, rows: u16) -> Result<(), AppError> {
    unix_sys::set_terminal_winsize(pty_file, cols, rows).map_err(|error| {
        AppError::External(format!(
            "Failed to resize PTY to {}x{}: {}",
            cols, rows, error
        ))
    })
}

pub(crate) fn send_sigwinch(child_pid: Option<i32>) {
    let Some(pid) = child_pid else {
        return;
    };
    if pid <= 0 {
        return;
    }

    let _ = unix_sys::kill_process_group_sigwinch(pid);
}
