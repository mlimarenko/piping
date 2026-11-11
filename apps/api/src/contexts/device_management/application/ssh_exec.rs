//! Application port for one-shot SSH command execution.
//!
//! A family whose management plane is a local shell (the Linux host) does not
//! speak a device protocol over a byte transport; it runs commands on the host
//! over SSH. This port is that seam: the `application` layer and the Linux
//! driver depend on [`SshExecPort`], and `infrastructure` adapts it onto the
//! existing proxy SSH exec runtime. Keeping it here means the driver never
//! depends on the proxy module directly, and tests can drive a fake.

use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::contexts::device_management::domain::DriverError;

/// One SSH command to run against a connection target.
///
/// Launch material (host, port, username, login password) is resolved from
/// `target_id` by the [`SshExecPort`] adapter, so this request names the target
/// rather than carrying credentials. `stdin` is the byte payload written to the
/// remote command's standard input before it is closed (e.g. a `chpasswd` line,
/// or a `sudo -S` become password); it is never logged.
#[derive(Clone)]
pub struct SshExecRequest {
    /// The connection target whose launch material this command runs against.
    pub target_id: Uuid,
    /// The remote command line to execute.
    pub command: String,
    /// Bytes written to the remote command's stdin, then EOF. Redacted in logs.
    pub stdin: Option<Vec<u8>>,
    /// Hard deadline for the command.
    pub timeout: Duration,
}

impl std::fmt::Debug for SshExecRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshExecRequest")
            .field("target_id", &self.target_id)
            .field("command", &self.command)
            // Never render the payload: it can carry a plaintext password.
            .field(
                "stdin",
                &self
                    .stdin
                    .as_ref()
                    .map(|bytes| format!("<{} bytes redacted>", bytes.len())),
            )
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// The result of one SSH command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshExecOutput {
    /// Standard output bytes.
    pub stdout: Vec<u8>,
    /// Standard error bytes.
    pub stderr: Vec<u8>,
    /// Process exit code, when the remote command exited normally.
    pub exit_code: Option<i32>,
    /// Whether the remote command exited 0.
    pub success: bool,
    /// Whether the command was killed at its deadline.
    pub timed_out: bool,
}

impl SshExecOutput {
    /// The standard output decoded lossily as UTF-8, for line-oriented parsers.
    #[must_use]
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// The standard error decoded lossily as UTF-8.
    #[must_use]
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// A short, single-line excerpt of standard error for an error message.
    #[must_use]
    pub fn stderr_excerpt(&self) -> String {
        let collapsed = String::from_utf8_lossy(&self.stderr)
            .replace(['\n', '\r', '\t'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        collapsed.chars().take(200).collect()
    }
}

/// Runs one SSH command against a resolved host.
#[async_trait]
pub trait SshExecPort: Send + Sync {
    /// Run `request` to completion, returning its captured output and status.
    ///
    /// # Errors
    ///
    /// Returns a link-fatal [`DriverError`] (`Transport`/`Unreachable`/`Auth`/
    /// `Timeout`) when the SSH link itself could not be established or was lost;
    /// a remote command that runs and exits non-zero is a successful call with
    /// `success == false`, not an error.
    async fn exec(&self, request: SshExecRequest) -> Result<SshExecOutput, DriverError>;
}
