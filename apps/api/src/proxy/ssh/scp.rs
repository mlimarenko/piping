use super::*;
pub(super) struct PreparedScpProcess {
    pub(super) command: Command,
    pub(super) launch: SshLaunchConfig,
    pub(super) material_dir: PathBuf,
    pub(super) material_paths: Vec<PathBuf>,
    /// The per-operation `known_hosts` file this process pins, plus its seed.
    pub(super) known_hosts: PathBuf,
    pub(super) seed_lines: Vec<String>,
}

pub(super) enum ScpDirection {
    Upload {
        local_path: PathBuf,
        remote_path: String,
        bytes: u64,
    },
}

pub(super) async fn prepare_scp_process(
    connection_id: Uuid,
    payload: &Value,
    control_path: String,
) -> Result<PreparedScpProcess, AppError> {
    let launch = extract_ssh_launch_config(payload)
        .map_err(|error| AppError::Validation(format!("Invalid SSH payload: {}", error)))?;
    if control_path.trim().is_empty() {
        return Err(AppError::Internal(
            "SSH file upload requires an authenticated ControlMaster session".to_string(),
        ));
    }

    let operation_id = Uuid::now_v7().simple().to_string();
    let material_paths: Vec<PathBuf> = Vec::new();
    let dir = match ensure_ssh_material_dir(ssh_transfer_material_dir(connection_id, &operation_id))
        .await
    {
        Ok(value) => value,
        Err(error) => return Err(error),
    };
    let (known_hosts, seed_lines) = match seed_job_known_hosts(&dir, payload) {
        Ok(seeded) => seeded,
        Err(error) => {
            cleanup_ssh_transfer_material(&dir, &material_paths).await;
            return Err(error);
        }
    };

    // Uploads are mux-only just like downloads. The existing ControlMaster
    // owns target and jump authentication, so this process intentionally has
    // neither an askpass secret nor private-key/jump material for a direct
    // fallback to use.
    let mut command = Command::new("scp");

    let connect_timeout_secs = match super::super::extract_connect_timeout_seconds(payload) {
        Ok(timeout) => timeout.round() as u64,
        Err(error) => {
            cleanup_ssh_transfer_material(&dir, &material_paths).await;
            return Err(AppError::Validation(error.to_string()));
        }
    };

    command
        .kill_on_drop(true)
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .arg("-q")
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg("-o")
        .arg(format!("ConnectTimeout={connect_timeout_secs}"))
        .arg("-P")
        .arg(launch.port.to_string());
    append_pinned_host_key_options(&mut command, &known_hosts);
    append_scp_control_options(&mut command, Some(&control_path));

    Ok(PreparedScpProcess {
        command,
        launch,
        material_dir: dir,
        material_paths,
        known_hosts,
        seed_lines,
    })
}

/// State shared by every phase of one scp transfer, so helpers do not grow
/// past the argument limit and abort logic stays identical across exits.
struct ScpTransferContext {
    material_dir: PathBuf,
    material_paths: Vec<PathBuf>,
    known_hosts: PathBuf,
    seed_lines: Vec<String>,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
    metrics_mode: SshProcessMetricsMode,
    operation: &'static str,
    started_at: Instant,
}

struct ScpProcessOutcome {
    exit_code: Option<i32>,
    success: bool,
    timed_out: bool,
    stderr: AgentSshExecStream,
}

type ScpDrainTask = tokio::task::JoinHandle<Result<AgentSshExecStream, std::io::Error>>;

async fn spawn_scp_process(
    ctx: &ScpTransferContext,
    command: &mut Command,
) -> Result<(tokio::process::Child, ScpDrainTask, ScpDrainTask), AppError> {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Err(abort_scp_transfer(
                &ctx.material_dir,
                &ctx.material_paths,
                &ctx.metrics,
                ctx.metrics_mode,
                AppError::External(format!(
                    "Failed to spawn scp {} process: {error}",
                    ctx.operation
                )),
            )
            .await);
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::External("Failed to capture scp stdout".to_string()));
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::External("Failed to capture scp stderr".to_string()));
    match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => Ok((
            child,
            tokio::spawn(drain_limited_stream(stdout, STDERR_EXCERPT_CHAR_LIMIT)),
            tokio::spawn(drain_limited_stream(stderr, STDERR_EXCERPT_CHAR_LIMIT)),
        )),
        (Err(error), _) | (_, Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(abort_scp_transfer(
                &ctx.material_dir,
                &ctx.material_paths,
                &ctx.metrics,
                ctx.metrics_mode,
                error,
            )
            .await)
        }
    }
}

async fn abort_scp_drain_tasks(stdout_task: ScpDrainTask, stderr_task: ScpDrainTask) {
    stdout_task.abort();
    stderr_task.abort();
    let _ = stdout_task.await;
    let _ = stderr_task.await;
}

/// Flattens one drain task, running the shared abort tail on failure.
async fn collect_scp_drain(
    ctx: &ScpTransferContext,
    joined: Result<Result<AgentSshExecStream, std::io::Error>, tokio::task::JoinError>,
    stream_label: &str,
) -> Result<AgentSshExecStream, AppError> {
    match flatten_scp_drain(joined, stream_label) {
        Ok(stream) => Ok(stream),
        Err(error) => Err(abort_scp_transfer(
            &ctx.material_dir,
            &ctx.material_paths,
            &ctx.metrics,
            ctx.metrics_mode,
            error,
        )
        .await),
    }
}

async fn wait_scp_process(
    ctx: &ScpTransferContext,
    child: &mut tokio::process::Child,
    stdout_task: ScpDrainTask,
    stderr_task: ScpDrainTask,
) -> Result<ScpProcessOutcome, AppError> {
    let (exit_code, success, timed_out) =
        match timeout(SSH_FILE_TRANSFER_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => (status.code(), status.success(), false),
            Ok(Err(error)) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                abort_scp_drain_tasks(stdout_task, stderr_task).await;
                return Err(abort_scp_transfer(
                    &ctx.material_dir,
                    &ctx.material_paths,
                    &ctx.metrics,
                    ctx.metrics_mode,
                    AppError::External(format!(
                        "Failed to wait for scp {} process: {error}",
                        ctx.operation
                    )),
                )
                .await);
            }
            Err(_) => {
                let _ = child.kill().await;
                let status = child.wait().await.ok();
                (status.and_then(|value| value.code()), false, true)
            }
        };

    let _stdout = collect_scp_drain(ctx, stdout_task.await, "stdout").await?;
    let stderr = collect_scp_drain(ctx, stderr_task.await, "stderr").await?;
    Ok(ScpProcessOutcome {
        exit_code,
        success,
        timed_out,
        stderr,
    })
}

async fn fail_scp_transfer(
    ctx: &ScpTransferContext,
    outcome: &ScpProcessOutcome,
    launch: &SshLaunchConfig,
) -> AppError {
    let detail = classify_failed_ssh_host_key_failure(
        outcome.timed_out,
        outcome.success,
        &String::from_utf8_lossy(&outcome.stderr.bytes),
        &launch.host,
        launch.port,
        &ctx.seed_lines,
    )
    .map(|error| error.to_string())
    .unwrap_or_else(|| {
        if outcome.timed_out {
            "transfer timed out".to_string()
        } else {
            ssh_stream_excerpt(&outcome.stderr.bytes)
        }
    });
    abort_scp_transfer(
        &ctx.material_dir,
        &ctx.material_paths,
        &ctx.metrics,
        ctx.metrics_mode,
        AppError::External(format!("SCP {} failed: {detail}", ctx.operation)),
    )
    .await
}

pub(super) async fn run_scp_transfer(
    connection_id: Uuid,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
    prepared: PreparedScpProcess,
    direction: ScpDirection,
    metrics_mode: SshProcessMetricsMode,
) -> Result<AgentSshFileTransferResult, AppError> {
    let PreparedScpProcess {
        mut command,
        launch,
        material_dir,
        material_paths,
        known_hosts,
        seed_lines,
    } = prepared;
    let (operation, expected_bytes) = match direction {
        ScpDirection::Upload {
            local_path,
            remote_path,
            bytes,
        } => {
            command
                .arg(local_path)
                .arg(scp_remote_spec(&launch, &remote_path));
            ("upload", bytes)
        }
    };

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    log_event(
        LogLevel::Info,
        "proxy.ssh.file_transfer.start",
        EventFields::default()
            .with_operation(format!("proxy_ssh_file_{operation}"))
            .with_resource("proxy_connection", connection_id.to_string())
            .with_context("host", launch.host.clone())
            .with_context("port", launch.port.to_string())
            .with_context("username", launch.username.clone()),
    );

    if metrics_mode.records_lifecycle() {
        super::super::metrics::mark_connection_metrics_connecting(&metrics).await;
    }
    let ctx = ScpTransferContext {
        material_dir,
        material_paths,
        known_hosts,
        seed_lines,
        metrics,
        metrics_mode,
        operation,
        started_at: Instant::now(),
    };

    let (mut child, stdout_task, stderr_task) = spawn_scp_process(&ctx, &mut command).await?;
    let outcome = wait_scp_process(&ctx, &mut child, stdout_task, stderr_task).await?;

    record_harvested_host_keys(&ctx.metrics, &ctx.known_hosts, &ctx.seed_lines).await;

    if outcome.timed_out || !outcome.success {
        return Err(fail_scp_transfer(&ctx, &outcome, &launch).await);
    }

    cleanup_ssh_transfer_material(&ctx.material_dir, &ctx.material_paths).await;

    if ctx.metrics_mode.records_lifecycle() {
        mark_connection_metrics_disconnected(&ctx.metrics).await;
    }
    let duration_ms = ctx.started_at.elapsed().as_millis();
    log_event(
        LogLevel::Info,
        "proxy.ssh.file_transfer.end",
        EventFields::default()
            .with_operation(format!("proxy_ssh_file_{operation}"))
            .with_resource("proxy_connection", connection_id.to_string())
            .with_status("ok")
            .with_context(
                "exit_code",
                outcome
                    .exit_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "signal_or_unknown".to_string()),
            )
            .with_context("duration_ms", duration_ms.to_string()),
    );

    Ok(AgentSshFileTransferResult {
        bytes: expected_bytes,
        duration_ms,
    })
}

pub(super) fn scp_remote_spec(launch: &SshLaunchConfig, remote_path: &str) -> String {
    format!("{}@{}:{}", launch.username, launch.host, remote_path)
}

pub(super) async fn cleanup_ssh_transfer_material(dir: &Path, paths: &[PathBuf]) {
    for path in paths {
        let _ = tokio::fs::remove_file(path).await;
    }
    // The per-operation directory is owner-only and never shared; it also holds
    // the pinned known_hosts seed, so removing the directory whole is the
    // complete cleanup and never leaves a trust file behind.
    let _ = tokio::fs::remove_dir_all(dir).await;
}

/// Every scp failure path ends the same way: drop the staged transfer material,
/// record the error metric when this mode owns the connection lifecycle, then
/// surface the error unchanged. Keeping that tail here is what stops
/// `run_scp_transfer` from repeating it at each of its exits.
pub(super) async fn abort_scp_transfer(
    material_dir: &Path,
    material_paths: &[PathBuf],
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    metrics_mode: SshProcessMetricsMode,
    error: AppError,
) -> AppError {
    cleanup_ssh_transfer_material(material_dir, material_paths).await;
    if metrics_mode.records_lifecycle() {
        mark_connection_metrics_error(metrics).await;
    }
    error
}

/// Flattens a drain task's join result: the outer error means the task itself
/// failed, the inner one means the stream could not be read.
pub(super) fn flatten_scp_drain(
    joined: Result<Result<AgentSshExecStream, std::io::Error>, tokio::task::JoinError>,
    stream_label: &str,
) -> Result<AgentSshExecStream, AppError> {
    match joined {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(error)) => Err(AppError::External(format!(
            "Failed to read SCP {stream_label}: {error}"
        ))),
        Err(error) => Err(AppError::External(format!(
            "SCP {stream_label} task failed: {error}"
        ))),
    }
}
