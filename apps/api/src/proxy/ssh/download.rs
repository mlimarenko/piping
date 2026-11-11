use super::*;
pub(super) struct SshDownloadRequest<'a> {
    pub(super) connection_id: Uuid,
    pub(super) payload: &'a Value,
    pub(super) metrics: Arc<RwLock<ConnectionMetricsState>>,
    pub(super) remote_path: String,
    pub(super) local_path: PathBuf,
    pub(super) max_bytes: u64,
    pub(super) archive: bool,
    pub(super) metrics_mode: SshProcessMetricsMode,
    pub(super) control_path: String,
}

struct DownloadProcess {
    child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr_task: tokio::task::JoinHandle<Result<AgentSshExecStream, std::io::Error>>,
}

struct DownloadProcessOutput {
    bytes: u64,
    exit_code: Option<i32>,
    success: bool,
    timed_out: bool,
    stderr: AgentSshExecStream,
}

pub(super) struct PreparedDownload {
    pub(super) launch: SshLaunchConfig,
    pub(super) material_dir: PathBuf,
    pub(super) material_paths: Vec<PathBuf>,
    pub(super) known_hosts: PathBuf,
    pub(super) seed_lines: Vec<String>,
    pub(super) command: Command,
}

async fn spawn_download_process(mut command: Command) -> Result<DownloadProcess, AppError> {
    let mut child = command.spawn().map_err(|error| {
        AppError::External(format!("Failed to spawn ssh download process: {error}"))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::External("Failed to capture ssh download stdout".to_string()));
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::External("Failed to capture ssh download stderr".to_string()));
    match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => Ok(DownloadProcess {
            child,
            stdout,
            stderr_task: tokio::spawn(drain_limited_stream(stderr, STDERR_EXCERPT_CHAR_LIMIT)),
        }),
        (Err(error), _) | (_, Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(error)
        }
    }
}

async fn write_download_stdout(
    child: &mut tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    local_path: &Path,
    max_bytes: u64,
) -> Result<u64, AppError> {
    match timeout(
        SSH_FILE_TRANSFER_TIMEOUT,
        write_bounded_stream_to_file(stdout, local_path.to_path_buf(), max_bytes),
    )
    .await
    {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(AppError::External(
                "SSH file download timed out".to_string(),
            ))
        }
    }
}

async fn wait_download_child(
    child: &mut tokio::process::Child,
) -> Result<(Option<i32>, bool, bool), AppError> {
    match timeout(SSH_FILE_TRANSFER_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => Ok((status.code(), status.success(), false)),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(AppError::External(format!(
                "Failed to wait for ssh download process: {error}"
            )))
        }
        Err(_) => {
            let _ = child.kill().await;
            let status = child.wait().await.ok();
            Ok((status.and_then(|value| value.code()), false, true))
        }
    }
}

async fn collect_download_stderr(
    stderr_task: tokio::task::JoinHandle<Result<AgentSshExecStream, std::io::Error>>,
) -> Result<AgentSshExecStream, AppError> {
    match stderr_task.await {
        Ok(Ok(stderr)) => Ok(stderr),
        Ok(Err(error)) => Err(AppError::External(format!(
            "Failed to read SSH download stderr: {error}"
        ))),
        Err(error) => Err(AppError::External(format!(
            "SSH download stderr task failed: {error}"
        ))),
    }
}

async fn execute_download_process(
    command: Command,
    local_path: &Path,
    max_bytes: u64,
) -> Result<DownloadProcessOutput, AppError> {
    let DownloadProcess {
        mut child,
        stdout,
        stderr_task,
    } = spawn_download_process(command).await?;
    let bytes = match write_download_stdout(&mut child, stdout, local_path, max_bytes).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = stderr_task.await;
            return Err(error);
        }
    };
    let (exit_code, success, timed_out) = match wait_download_child(&mut child).await {
        Ok(result) => result,
        Err(error) => {
            stderr_task.abort();
            let _ = stderr_task.await;
            return Err(error);
        }
    };
    let stderr = collect_download_stderr(stderr_task).await?;
    Ok(DownloadProcessOutput {
        bytes,
        exit_code,
        success,
        timed_out,
        stderr,
    })
}

pub(super) async fn prepare_ssh_download(
    connection_id: Uuid,
    payload: &Value,
    remote_path: &str,
    archive: bool,
    control_path: &str,
) -> Result<PreparedDownload, AppError> {
    let launch = extract_ssh_launch_config(payload)
        .map_err(|error| AppError::Validation(format!("Invalid SSH payload: {error}")))?;
    if control_path.trim().is_empty() {
        return Err(AppError::Internal(
            "SSH file download requires an authenticated ControlMaster session".to_string(),
        ));
    }
    let operation_id = Uuid::now_v7().simple().to_string();
    let material_dir =
        ensure_ssh_material_dir(ssh_transfer_material_dir(connection_id, &operation_id)).await?;
    let material_paths = Vec::new();
    let (known_hosts, seed_lines) = match seed_job_known_hosts(&material_dir, payload) {
        Ok(seeded) => seeded,
        Err(error) => {
            cleanup_ssh_transfer_material(&material_dir, &material_paths).await;
            return Err(error);
        }
    };
    let connect_timeout_secs = match super::super::extract_connect_timeout_seconds(payload) {
        Ok(timeout) => timeout.round() as u64,
        Err(error) => {
            cleanup_ssh_transfer_material(&material_dir, &material_paths).await;
            return Err(AppError::Validation(error.to_string()));
        }
    };
    let remote_command = if archive {
        build_remote_archive_download_command(remote_path)
    } else {
        build_remote_download_command(remote_path)
    };
    let mut command = Command::new("ssh");
    command
        .kill_on_drop(true)
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .arg("-T")
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg("-o")
        .arg(format!("ConnectTimeout={connect_timeout_secs}"))
        .arg("-p")
        .arg(launch.port.to_string());
    append_pinned_host_key_options(&mut command, &known_hosts);
    append_ssh_control_options(&mut command, Some(control_path));
    command
        .arg("-l")
        .arg(&launch.username)
        .arg(&launch.host)
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(PreparedDownload {
        launch,
        material_dir,
        material_paths,
        known_hosts,
        seed_lines,
        command,
    })
}

/// Shared failure tail for a download attempt: drop the staged transfer
/// material and any partial local file, then record the error metric when this
/// mode owns the connection lifecycle.
async fn finish_failed_download(
    material_dir: &Path,
    material_paths: &[PathBuf],
    local_path: &Path,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    metrics_mode: SshProcessMetricsMode,
) {
    cleanup_ssh_transfer_material(material_dir, material_paths).await;
    let _ = tokio::fs::remove_file(local_path).await;
    if metrics_mode.records_lifecycle() {
        mark_connection_metrics_error(metrics).await;
    }
}

pub(super) async fn run_ssh_download_to_file(
    request: SshDownloadRequest<'_>,
) -> Result<AgentSshFileTransferResult, AppError> {
    let SshDownloadRequest {
        connection_id,
        payload,
        metrics,
        remote_path,
        local_path,
        max_bytes,
        archive,
        metrics_mode,
        control_path,
    } = request;
    let PreparedDownload {
        launch,
        material_dir,
        material_paths,
        known_hosts,
        seed_lines,
        command,
    } = prepare_ssh_download(connection_id, payload, &remote_path, archive, &control_path).await?;

    log_event(
        LogLevel::Info,
        "proxy.ssh.file_download.start",
        EventFields::default()
            .with_operation("proxy_ssh_file_download")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_context("host", launch.host.clone())
            .with_context("port", launch.port.to_string())
            .with_context("username", launch.username.clone())
            .with_context("archive", archive.to_string())
            .with_context("max_bytes", max_bytes.to_string()),
    );

    if metrics_mode.records_lifecycle() {
        super::super::metrics::mark_connection_metrics_connecting(&metrics).await;
    }
    let started_at = Instant::now();
    let DownloadProcessOutput {
        bytes,
        exit_code,
        success,
        timed_out,
        stderr,
    } = match execute_download_process(command, &local_path, max_bytes).await {
        Ok(output) => output,
        Err(error) => {
            finish_failed_download(
                &material_dir,
                &material_paths,
                &local_path,
                &metrics,
                metrics_mode,
            )
            .await;
            return Err(error);
        }
    };

    record_harvested_host_keys(&metrics, &known_hosts, &seed_lines).await;
    cleanup_ssh_transfer_material(&material_dir, &material_paths).await;

    if timed_out || !success {
        finish_failed_download(
            &material_dir,
            &material_paths,
            &local_path,
            &metrics,
            metrics_mode,
        )
        .await;
        let detail = classify_failed_ssh_host_key_failure(
            timed_out,
            success,
            &String::from_utf8_lossy(&stderr.bytes),
            &launch.host,
            launch.port,
            &seed_lines,
        )
        .map(|error| error.to_string())
        .unwrap_or_else(|| {
            if timed_out {
                "download timed out".to_string()
            } else {
                ssh_stream_excerpt(&stderr.bytes)
            }
        });
        return Err(AppError::External(format!(
            "SSH file download failed: {detail}"
        )));
    }

    if metrics_mode.records_lifecycle() {
        mark_connection_metrics_disconnected(&metrics).await;
    }

    let duration_ms = started_at.elapsed().as_millis();
    log_event(
        LogLevel::Info,
        "proxy.ssh.file_download.end",
        EventFields::default()
            .with_operation("proxy_ssh_file_download")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_status("ok")
            .with_context(
                "exit_code",
                exit_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "signal_or_unknown".to_string()),
            )
            .with_context("duration_ms", duration_ms.to_string())
            .with_context("bytes", bytes.to_string()),
    );

    Ok(AgentSshFileTransferResult { bytes, duration_ms })
}

pub(super) async fn drain_limited_stream<R>(
    mut stream: R,
    max_bytes: usize,
) -> Result<AgentSshExecStream, std::io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; SSH_EXEC_BUFFER_SIZE];
    let mut retained = Vec::new();
    let mut bytes_read = 0usize;
    let mut truncated = false;
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(read);
        if retained.len() < max_bytes {
            let remaining = max_bytes.saturating_sub(retained.len());
            let keep = read.min(remaining);
            retained.extend_from_slice(&buffer[..keep]);
            if keep < read {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    Ok(AgentSshExecStream {
        bytes: retained,
        bytes_read,
        truncated,
    })
}

pub(super) async fn write_bounded_stream_to_file<R>(
    mut stream: R,
    local_path: PathBuf,
    max_bytes: u64,
) -> Result<u64, AppError>
where
    R: AsyncRead + Unpin,
{
    let mut file = tokio::fs::File::create(&local_path)
        .await
        .map_err(|error| {
            AppError::External(format!("Failed to create download temp file: {error}"))
        })?;
    let mut buffer = [0_u8; SSH_EXEC_BUFFER_SIZE];
    let mut bytes_written = 0_u64;
    loop {
        let read = stream.read(&mut buffer).await.map_err(|error| {
            AppError::External(format!("Failed to read SSH download stream: {error}"))
        })?;
        if read == 0 {
            break;
        }
        let next = bytes_written.saturating_add(read as u64);
        if next > max_bytes {
            return Err(AppError::PayloadTooLarge(format!(
                "Remote file exceeds configured download limit of {max_bytes} bytes"
            )));
        }
        file.write_all(&buffer[..read]).await.map_err(|error| {
            AppError::External(format!("Failed to write download temp file: {error}"))
        })?;
        bytes_written = next;
    }
    file.flush().await.map_err(|error| {
        AppError::External(format!("Failed to flush download temp file: {error}"))
    })?;
    Ok(bytes_written)
}
