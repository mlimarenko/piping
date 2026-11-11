//! Adapter that runs device-management SSH commands through the proxy.
//!
//! The Linux device driver resolves account-management commands against a
//! connection target, not a live proxy connection. This adapter bridges the
//! [`SshExecPort`] seam to `ConnectionService::resolve_ssh_execution_launch` +
//! `proxy::ssh_exec_payload`: it resolves the target's launch material with the
//! same shared-account path the playbook worker uses, then runs a one-shot exec
//! with no registered connection session.
//!
//! Device management is not user-credential scoped: launch material is resolved
//! with `credential_user_id: None`, so the shared connection-config account
//! applies and no personal credential-set overlay is merged.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use super::ConnectionServiceType;
use crate::config::constants::DEFAULT_PLAYBOOK_RUNTIME_EXEC_MAX_BYTES_PER_STREAM;
use crate::contexts::connections::application::ResolveSshExecutionLaunchInput;
use crate::contexts::device_management::application::ssh_exec::{
    SshExecOutput, SshExecPort, SshExecRequest,
};
use crate::contexts::device_management::domain::DriverError;
use crate::errors::AppError;
use crate::proxy::{AgentSshExecRequest, AgentSshExecResult};

/// Runs one-shot SSH execs on behalf of the device-management Linux driver.
pub(crate) struct ProxyDeviceSshExec {
    connection: Arc<ConnectionServiceType>,
}

impl ProxyDeviceSshExec {
    /// Construct the adapter over the shared connection service.
    pub(crate) fn new(connection: Arc<ConnectionServiceType>) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl SshExecPort for ProxyDeviceSshExec {
    async fn exec(&self, request: SshExecRequest) -> Result<SshExecOutput, DriverError> {
        let launch = self
            .connection
            .resolve_ssh_execution_launch(
                ResolveSshExecutionLaunchInput {
                    // No durable execution owns this command: a per-call id keeps
                    // the materialization/audit record scoped to this exec.
                    execution_id: Uuid::now_v7(),
                    target_id: request.target_id,
                    credential_user_id: None,
                    session_metadata: None,
                    protocol_params: None,
                    requested_proxy_node_id: None,
                },
                None,
                None,
            )
            .await
            .map_err(driver_error_from_app_error)?;
        let result = crate::proxy::ssh_exec_payload(
            launch.proxy_payload,
            AgentSshExecRequest {
                command: request.command,
                timeout: request.timeout,
                max_bytes_per_stream: DEFAULT_PLAYBOOK_RUNTIME_EXEC_MAX_BYTES_PER_STREAM,
                // Never logged or persisted by the exec runtime.
                stdin: request.stdin,
            },
        )
        .await
        .map_err(driver_error_from_app_error)?;
        output_from_exec_result(result)
    }
}

/// Map a connection/proxy [`AppError`] onto the driver's link-fatal taxonomy.
///
/// An unresolvable target is `Unreachable`; malformed launch material and every
/// other failure is a `Transport` fault. Authentication failures stay `Auth`.
/// A remote exit of 255 is not an error here: the driver inspects the result and
/// classifies it.
fn driver_error_from_app_error(error: AppError) -> DriverError {
    match &error {
        AppError::NotFound(_) => DriverError::Unreachable(error.to_string()),
        AppError::Validation(_) | AppError::ValidationJson(_) | AppError::ValidationUuid(_) => {
            DriverError::Transport(error.to_string())
        }
        AppError::Auth(_) | AppError::Forbidden(_) => DriverError::Auth,
        _ => DriverError::Transport(error.to_string()),
    }
}

/// Map a completed proxy exec onto the driver's output, treating a deadline
/// kill as a link-fatal timeout rather than a normal non-zero exit.
fn output_from_exec_result(result: AgentSshExecResult) -> Result<SshExecOutput, DriverError> {
    if result.timed_out {
        return Err(DriverError::Timeout(format!(
            "ssh command exceeded its deadline (exit code {:?})",
            result.exit_code
        )));
    }
    Ok(SshExecOutput {
        stdout: result.stdout.bytes,
        stderr: result.stderr.bytes,
        exit_code: result.exit_code,
        success: result.success,
        timed_out: result.timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::AgentSshExecStream;

    fn exec_result(
        stdout: &[u8],
        stderr: &[u8],
        exit_code: Option<i32>,
        success: bool,
        timed_out: bool,
    ) -> AgentSshExecResult {
        AgentSshExecResult {
            stdout: AgentSshExecStream {
                bytes: stdout.to_vec(),
                bytes_read: stdout.len(),
                truncated: false,
            },
            stderr: AgentSshExecStream {
                bytes: stderr.to_vec(),
                bytes_read: stderr.len(),
                truncated: false,
            },
            exit_code,
            success,
            timed_out,
            duration_ms: 4,
        }
    }

    #[test]
    fn successful_exec_maps_every_field_onto_the_driver_output() {
        let output = output_from_exec_result(exec_result(
            b"alice:x:1000:1000::/home/alice:/bin/bash\n",
            b"",
            Some(0),
            true,
            false,
        ))
        .expect("a non-timeout exec is Ok");
        assert_eq!(output.stdout, b"alice:x:1000:1000::/home/alice:/bin/bash\n");
        assert!(output.stderr.is_empty());
        assert_eq!(output.exit_code, Some(0));
        assert!(output.success);
        assert!(!output.timed_out);
    }

    #[test]
    fn non_zero_exec_is_a_successful_call_with_success_false() {
        let output = output_from_exec_result(exec_result(
            b"",
            b"userdel: user 'alice' does not exist\n",
            Some(6),
            false,
            false,
        ))
        .expect("a non-timeout failure is still an Ok call");
        assert_eq!(output.exit_code, Some(6));
        assert!(!output.success);
        assert!(!output.timed_out);
    }

    #[test]
    fn timed_out_exec_maps_to_driver_timeout() {
        let error = output_from_exec_result(exec_result(b"partial", b"", None, false, true))
            .expect_err("a deadline kill is link-fatal");
        assert!(matches!(error, DriverError::Timeout(_)), "got {error:?}");
    }

    #[test]
    fn app_errors_map_not_found_validation_auth_and_the_rest() {
        assert!(matches!(
            driver_error_from_app_error(AppError::NotFound("target".to_owned())),
            DriverError::Unreachable(_)
        ));
        assert!(matches!(
            driver_error_from_app_error(AppError::Validation("bad launch".to_owned())),
            DriverError::Transport(_)
        ));
        assert!(matches!(
            driver_error_from_app_error(AppError::ValidationUuid(
                "not-a-uuid".parse::<Uuid>().unwrap_err()
            )),
            DriverError::Transport(_)
        ));
        assert!(matches!(
            driver_error_from_app_error(AppError::Auth("denied".to_owned())),
            DriverError::Auth
        ));
        assert!(matches!(
            driver_error_from_app_error(AppError::Forbidden("no".to_owned())),
            DriverError::Auth
        ));
        assert!(matches!(
            driver_error_from_app_error(AppError::Internal("boom".to_owned())),
            DriverError::Transport(_)
        ));
    }
}
