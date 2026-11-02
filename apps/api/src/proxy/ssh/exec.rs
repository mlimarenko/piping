use super::*;

struct ExecProcessOutput {
    stdout: AgentSshExecStream,
    stderr: AgentSshExecStream,
    exit_code: Option<i32>,
    success: bool,
    timed_out: bool,
}

pub(super) struct ExecMaterial {
    pub(super) material_dir: PathBuf,
    pub(super) material_paths: Vec<PathBuf>,
    pub(super) known_hosts: PathBuf,
    pub(super) seed_lines: Vec<String>,
    pub(super) private_key_path: Option<String>,
    pub(super) jump_key_paths: Vec<String>,
    pub(super) password: Option<String>,
}

pub(super) struct PreparedExec {
    pub(super) launch: SshLaunchConfig,
    pub(super) material: ExecMaterial,
    pub(super) command: Command,
}

async fn collect_exec_stream(
    task: tokio::task::JoinHandle<Result<AgentSshExecStream, std::io::Error>>,
    stream_name: &str,
) -> Result<AgentSshExecStream, AppError> {
    match task.await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(error)) => Err(AppError::External(format!(
            "Failed to read SSH {stream_name}: {error}"
        ))),
        Err(error) => Err(AppError::External(format!(
            "SSH {stream_name} task failed: {error}"
        ))),
    }
}

async fn wait_exec_child(
    child: &mut tokio::process::Child,
    timeout_duration: Duration,
) -> Result<(Option<i32>, bool, bool), AppError> {
    match timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => Ok((status.code(), status.success(), false)),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(AppError::External(format!(
                "Failed to wait for ssh exec process: {error}"
            )))
        }
        Err(_) => {
            let _ = child.kill().await;
            let status = child.wait().await.ok();
            Ok((status.and_then(|value| value.code()), false, true))
        }
    }
}

async fn execute_ssh_command(
    mut command: Command,
    timeout_duration: Duration,
    max_bytes_per_stream: usize,
    stdin_payload: Option<Vec<u8>>,
) -> Result<ExecProcessOutput, AppError> {
    let mut child = command.spawn().map_err(|error| {
        AppError::External(format!("Failed to spawn ssh exec process: {error}"))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::External("Failed to capture ssh exec stdout".to_string()));
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::External("Failed to capture ssh exec stderr".to_string()));
    let stdin = child.stdin.take();
    let (stdout, stderr) = match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => (stdout, stderr),
        (Err(error), _) | (_, Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
    };
    let stdout_task = tokio::spawn(drain_limited_stream(stdout, max_bytes_per_stream));
    let stderr_task = tokio::spawn(drain_limited_stream(stderr, max_bytes_per_stream));
    // Feed stdin concurrently with the output drains, then close it so the remote
    // command sees EOF. A command that never reads stdin still terminates: the
    // write task is aborted when the child exits or times out below.
    let stdin_task = match (stdin, stdin_payload) {
        (Some(mut pipe), Some(bytes)) => Some(tokio::spawn(async move {
            pipe.write_all(&bytes).await?;
            pipe.shutdown().await
        })),
        _ => None,
    };
    let (exit_code, success, timed_out) = match wait_exec_child(&mut child, timeout_duration).await
    {
        Ok(result) => result,
        Err(error) => {
            stdout_task.abort();
            stderr_task.abort();
            if let Some(task) = stdin_task {
                task.abort();
            }
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(error);
        }
    };
    // The child has exited, so a pending stdin write can only be a broken pipe;
    // it is never a reason to fail an otherwise-completed exec.
    if let Some(task) = stdin_task {
        let _ = task.await;
    }
    let stdout = collect_exec_stream(stdout_task, "stdout").await?;
    let stderr = collect_exec_stream(stderr_task, "stderr").await?;
    Ok(ExecProcessOutput {
        stdout,
        stderr,
        exit_code,
        success,
        timed_out,
    })
}

async fn write_exec_key(
    material_dir: &Path,
    material_paths: &mut Vec<PathBuf>,
    name: String,
    private_key: &str,
) -> Result<String, AppError> {
    let path = write_key_file_excl(material_dir.to_path_buf(), name, private_key).await?;
    material_paths.push(PathBuf::from(&path));
    Ok(path)
}

async fn prepare_target_exec_key(
    material_dir: &Path,
    material_paths: &mut Vec<PathBuf>,
    launch: &SshLaunchConfig,
    operation_id: &str,
) -> Result<Option<String>, AppError> {
    let Some(private_key) = launch.private_key.as_deref() else {
        return Ok(None);
    };
    write_exec_key(
        material_dir,
        material_paths,
        format!("exec-{operation_id}-target.key"),
        private_key,
    )
    .await
    .map(Some)
}

async fn prepare_jump_exec_keys(
    material_dir: &Path,
    material_paths: &mut Vec<PathBuf>,
    launch: &SshLaunchConfig,
    operation_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut paths = Vec::new();
    for (index, hop) in launch.jump.iter().enumerate() {
        if let Some(private_key) = hop.private_key.as_deref() {
            paths.push(
                write_exec_key(
                    material_dir,
                    material_paths,
                    format!("exec-{operation_id}-jump-{index}.key"),
                    private_key,
                )
                .await?,
            );
        }
    }
    Ok(paths)
}

async fn prepare_exec_material(
    connection_id: Uuid,
    payload: &Value,
    launch: &SshLaunchConfig,
    mux_only: bool,
    operation_id: &str,
) -> Result<ExecMaterial, AppError> {
    let material_dir =
        ensure_ssh_material_dir(ssh_exec_material_dir(connection_id, operation_id)).await?;
    let mut material_paths = Vec::new();
    let (known_hosts, seed_lines) = match seed_job_known_hosts(&material_dir, payload) {
        Ok(seeded) => seeded,
        Err(error) => {
            cleanup_ssh_transfer_material(&material_dir, &material_paths).await;
            return Err(error);
        }
    };
    let mut private_key_path = None;
    let mut jump_key_paths = Vec::new();
    if !mux_only {
        private_key_path =
            match prepare_target_exec_key(&material_dir, &mut material_paths, launch, operation_id)
                .await
            {
                Ok(path) => path,
                Err(error) => {
                    cleanup_ssh_transfer_material(&material_dir, &material_paths).await;
                    return Err(error);
                }
            };
        jump_key_paths =
            match prepare_jump_exec_keys(&material_dir, &mut material_paths, launch, operation_id)
                .await
            {
                Ok(paths) => paths,
                Err(error) => {
                    cleanup_ssh_transfer_material(&material_dir, &material_paths).await;
                    return Err(error);
                }
            };
    }
    let password = (!mux_only)
        .then_some(launch.password.as_deref())
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    Ok(ExecMaterial {
        material_dir,
        material_paths,
        known_hosts,
        seed_lines,
        private_key_path,
        jump_key_paths,
        password,
    })
}

async fn configure_exec_askpass(
    command: &mut Command,
    material: &mut ExecMaterial,
    operation_id: &str,
) -> Result<(), AppError> {
    let Some(password) = material.password.as_deref() else {
        return Ok(());
    };
    let path = write_askpass_helper_in_dir(
        material.material_dir.clone(),
        format!("exec-{operation_id}-askpass.sh"),
    )
    .await?;
    material.material_paths.push(PathBuf::from(&path));
    command
        .env("PIPING_SSH_ASKPASS_SECRET", password)
        .env("SSH_ASKPASS", &path)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", ":0");
    Ok(())
}

fn append_exec_authentication(
    command: &mut Command,
    launch: &SshLaunchConfig,
    material: &ExecMaterial,
    mux_only: bool,
) {
    if mux_only {
        return;
    }
    if !launch.jump.is_empty() {
        let proxy_jump = launch
            .jump
            .iter()
            .map(SshJumpHop::proxy_jump_spec)
            .collect::<Vec<_>>()
            .join(",");
        command.arg("-J").arg(proxy_jump);
        append_pinned_host_key_options(command, &material.known_hosts);
        for path in &material.jump_key_paths {
            command.arg("-i").arg(path);
        }
    }
    if material.password.is_some() {
        command
            .arg("-o")
            .arg("PreferredAuthentications=password,keyboard-interactive")
            .arg("-o")
            .arg("PubkeyAuthentication=no")
            .arg("-o")
            .arg("NumberOfPasswordPrompts=1");
    }
    if let Some(path) = material.private_key_path.as_deref() {
        command.arg("-i").arg(path);
    }
    append_identities_only_when_keyed(
        command,
        material.private_key_path.is_some() || !material.jump_key_paths.is_empty(),
    );
}

pub(super) async fn prepare_exec(
    connection_id: Uuid,
    payload: &Value,
    request: &AgentSshExecRequest,
    control_path: Option<&str>,
    mux_only: bool,
) -> Result<PreparedExec, AppError> {
    let launch = extract_ssh_launch_config(payload)
        .map_err(|error| AppError::Validation(format!("Invalid SSH payload: {error}")))?;
    let operation_id = Uuid::now_v7().simple().to_string();
    let mut material =
        prepare_exec_material(connection_id, payload, &launch, mux_only, &operation_id).await?;
    let connect_timeout_secs = match super::super::extract_connect_timeout_seconds(payload) {
        Ok(timeout) => timeout.round() as u64,
        Err(error) => {
            cleanup_ssh_transfer_material(&material.material_dir, &material.material_paths).await;
            return Err(AppError::Validation(error.to_string()));
        }
    };
    let mut command = Command::new("ssh");
    if let Err(error) = configure_exec_askpass(&mut command, &mut material, &operation_id).await {
        cleanup_ssh_transfer_material(&material.material_dir, &material.material_paths).await;
        return Err(error);
    }
    command
        .kill_on_drop(true)
        .env("TERM", "xterm")
        .env("COLORTERM", "truecolor")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg("-o")
        .arg(format!("ConnectTimeout={connect_timeout_secs}"))
        .arg("-p")
        .arg(launch.port.to_string());
    append_pinned_host_key_options(&mut command, &material.known_hosts);
    append_ssh_control_options(&mut command, control_path);
    append_exec_authentication(&mut command, &launch, &material, mux_only);
    command
        .arg("-l")
        .arg(&launch.username)
        .arg(&launch.host)
        .arg(&request.command)
        .stdin(if request.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(PreparedExec {
        launch,
        material,
        command,
    })
}

async fn finish_exec_lifecycle(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    tracks_lifecycle: bool,
    timed_out: bool,
    success: bool,
) {
    if !tracks_lifecycle {
        return;
    }
    if timed_out || !success {
        mark_connection_metrics_error(metrics).await;
    } else {
        mark_connection_metrics_disconnected(metrics).await;
    }
}

pub(super) async fn run_ssh_exec_runtime(
    connection_id: Uuid,
    payload: &Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
    request: AgentSshExecRequest,
    metrics_mode: SshProcessMetricsMode,
    control_path: Option<String>,
) -> Result<AgentSshExecResult, AppError> {
    let mux_only = control_path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty());
    if matches!(metrics_mode, SshProcessMetricsMode::Sidecar) && !mux_only {
        return Err(AppError::Internal(
            "SSH sidecar requires an authenticated ControlMaster session".to_string(),
        ));
    }
    let PreparedExec {
        launch,
        material,
        command,
    } = prepare_exec(
        connection_id,
        payload,
        &request,
        control_path.as_deref(),
        mux_only,
    )
    .await?;
    let ExecMaterial {
        material_dir,
        material_paths,
        known_hosts,
        seed_lines,
        ..
    } = material;

    log_event(
        LogLevel::Info,
        "proxy.ssh.exec.start",
        EventFields::default()
            .with_operation("proxy_ssh_exec_spawn")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_context("host", launch.host.clone())
            .with_context("port", launch.port.to_string())
            .with_context("username", launch.username.clone())
            .with_context("timeout_ms", request.timeout.as_millis().to_string())
            .with_context(
                "max_bytes_per_stream",
                request.max_bytes_per_stream.to_string(),
            ),
    );

    let tracks_lifecycle = metrics_mode.records_lifecycle();
    if tracks_lifecycle {
        super::super::metrics::mark_connection_metrics_connecting(&metrics).await;
    }
    let started_at = Instant::now();
    let max_bytes_per_stream = request.max_bytes_per_stream.max(1);
    let ExecProcessOutput {
        stdout,
        stderr,
        exit_code,
        success,
        timed_out,
    } = match execute_ssh_command(
        command,
        request.timeout,
        max_bytes_per_stream,
        request.stdin,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            cleanup_ssh_transfer_material(&material_dir, &material_paths).await;
            if tracks_lifecycle {
                mark_connection_metrics_error(&metrics).await;
            }
            return Err(error);
        }
    };

    record_harvested_host_keys(&metrics, &known_hosts, &seed_lines).await;
    cleanup_ssh_transfer_material(&material_dir, &material_paths).await;

    if let Some(error) = classify_failed_ssh_host_key_failure(
        timed_out,
        success,
        &String::from_utf8_lossy(&stderr.bytes),
        &launch.host,
        launch.port,
        &seed_lines,
    ) {
        if tracks_lifecycle {
            mark_connection_metrics_error(&metrics).await;
        }
        log_event(
            LogLevel::Error,
            "proxy.ssh.exec.end",
            EventFields::default()
                .with_operation("proxy_ssh_exec_end")
                .with_resource("proxy_connection", connection_id.to_string())
                .with_status("host_key_mismatch")
                .with_context("host", launch.host.clone())
                .with_context("port", launch.port.to_string()),
        );
        return Err(error);
    }

    finish_exec_lifecycle(&metrics, tracks_lifecycle, timed_out, success).await;

    let duration_ms = started_at.elapsed().as_millis();
    log_event(
        LogLevel::Info,
        "proxy.ssh.exec.end",
        EventFields::default()
            .with_operation("proxy_ssh_exec_end")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_status(if success { "ok" } else { "failed" })
            .with_context("timed_out", timed_out.to_string())
            .with_context(
                "exit_code",
                exit_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "signal_or_unknown".to_string()),
            )
            .with_context("duration_ms", duration_ms.to_string())
            .with_context("stdout_bytes", stdout.bytes_read.to_string())
            .with_context("stderr_bytes", stderr.bytes_read.to_string()),
    );

    Ok(AgentSshExecResult {
        stdout,
        stderr,
        exit_code,
        success,
        timed_out,
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_ssh_command_writes_stdin_then_closes_it() {
        // `cat` echoes exactly what it reads and exits on EOF, proving the payload
        // is written to the child's stdin and the pipe is closed afterwards.
        let mut command = Command::new("cat");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = execute_ssh_command(
            command,
            Duration::from_secs(10),
            4096,
            Some(b"name:secret\n".to_vec()),
        )
        .await
        .expect("cat exec should succeed");
        assert!(output.success, "cat exits 0: {:?}", output.exit_code);
        assert_eq!(output.stdout.bytes, b"name:secret\n");
    }
}
