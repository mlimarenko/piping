use super::*;
pub(in crate::proxy) async fn ssh_session_init_lock(
    state: &ProxyState,
    connection_id: Uuid,
) -> Arc<Mutex<()>> {
    if let Some(existing) = state
        .ssh_session_init_locks
        .read()
        .await
        .get(&connection_id)
        .cloned()
    {
        return existing;
    }

    let mut guard = state.ssh_session_init_locks.write().await;
    guard
        .entry(connection_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Starts the interactive SSH runtime for a connection that has none and owns
/// its lifecycle transition. Callers reach this only through [`get_or_start_ssh_session`], so
/// re-attaching to an existing ready session never rewrites its metrics.
pub(super) async fn start_new_ssh_session_runtime(
    connection_id: Uuid,
    payload: &Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
) -> Result<Arc<SshSession>, AppError> {
    super::super::metrics::mark_connection_metrics_connecting(&metrics).await;
    match start_ssh_session_runtime(connection_id, payload, metrics.clone()).await {
        Ok(session) => Ok(session),
        Err(error) => {
            mark_connection_metrics_error(&metrics).await;
            Err(error)
        }
    }
}

/// Owns the child wait handle for an interactive SSH runtime. After the OS
/// reports completion, the exit seal waits only for an in-flight synchronous
/// browser-ready enqueue, never client I/O; the relay separately drains the
/// PTY and derives the final close code.
pub(super) fn spawn_ssh_child_exit_watcher(
    mut child: tokio::process::Child,
    session: Arc<SshSession>,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> tokio::sync::oneshot::Receiver<std::io::Result<std::process::ExitStatus>> {
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let status = tokio::select! {
            status = child.wait() => status,
            _ = shutdown_rx => {
                let _ = child.kill().await;
                child.wait().await
            }
        };
        session.seal_process_exit();
        // Publish a fail-closed metric before the relay's PTY drain and close
        // classification. The relay replaces it with its precise terminal
        // state (`disconnected` for a clean exit, `error` otherwise) once it
        // has the exit code and diagnostic evidence.
        mark_connection_metrics_error(&metrics).await;
        let _ = exit_tx.send(status);
    });
    exit_rx
}

/// Normalizes an unexpected PTY reader task failure before terminal close
/// classification. Both the main relay select and the bounded post-exit drain
/// use this one path so a `JoinError` can never masquerade as a clean logout.
pub(super) async fn mark_ssh_pty_reader_task_failure(output_state: &Arc<Mutex<SshOutputState>>) {
    mark_ssh_terminal_relay_failure(
        output_state,
        "SSH terminal reader failed",
        "pty_reader_task_error",
    )
    .await;
}

/// Persists a terminal internal-error outcome for a relay failure before the
/// finalizer derives its browser close frame. Metrics alone are insufficient:
/// leaving the default normal close here would show a successful disconnect to
/// the browser despite an I/O or child-lifecycle failure.
pub(super) async fn mark_ssh_terminal_relay_failure(
    output_state: &Arc<Mutex<SshOutputState>>,
    close_reason: &'static str,
    close_reason_source: &'static str,
) {
    let mut state = output_state.lock().await;
    state.relay_failed = true;
    state.close_code = WS_CLOSE_INTERNAL_ERROR;
    state.close_reason = close_reason;
    state.close_reason_source = close_reason_source;
}

/// Consumes the child-watcher result without allowing a failed `wait()` or a
/// dropped watcher channel to collapse into an absent (and therefore normal)
/// exit status. A watcher-channel failure may mean its task died before it
/// published the prompt exit seal, so seal locally as well before finalizing.
pub(super) async fn resolve_ssh_child_exit_result(
    result: Result<
        std::io::Result<std::process::ExitStatus>,
        tokio::sync::oneshot::error::RecvError,
    >,
    session: &SshSession,
    output_state: &Arc<Mutex<SshOutputState>>,
    connection_id: Uuid,
) -> Option<std::process::ExitStatus> {
    match result {
        Ok(Ok(status)) => Some(status),
        Ok(Err(error)) => {
            session.seal_process_exit();
            log_error_event(
                "proxy.ssh.relay.failed",
                "proxy_ssh_child_wait_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("proxy_ssh_child_wait")
                    .with_resource("proxy_connection", connection_id.to_string()),
            );
            mark_ssh_terminal_relay_failure(
                output_state,
                "SSH process wait failed",
                "child_wait_error",
            )
            .await;
            None
        }
        Err(error) => {
            session.seal_process_exit();
            log_error_event(
                "proxy.ssh.relay.failed",
                "proxy_ssh_child_exit_watcher_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("proxy_ssh_child_wait")
                    .with_resource("proxy_connection", connection_id.to_string()),
            );
            mark_ssh_terminal_relay_failure(
                output_state,
                "SSH process exit watcher failed",
                "child_exit_watcher_error",
            )
            .await;
            None
        }
    }
}

/// Writes browser input to the PTY and records an internal terminal failure if
/// the PTY rejects it. Keeping this boundary fallible prevents a write error
/// from updating metrics to error while the final browser close remains 1000.
pub(super) async fn write_ssh_pty_input(
    pty_writer: &mut tokio::fs::File,
    bytes: &[u8],
    connection_id: Uuid,
    output_state: &Arc<Mutex<SshOutputState>>,
) -> std::io::Result<()> {
    // Tokio buffers file writes, so `write_all` alone may report success before
    // the PTY rejects the bytes. Flush at this boundary to surface a failed
    // terminal write while the WebSocket relay can still publish an error.
    let write_result: std::io::Result<()> = async {
        pty_writer.write_all(bytes).await?;
        pty_writer.flush().await
    }
    .await;
    if let Err(error) = write_result {
        log_error_event(
            "proxy.ssh.relay.failed",
            "proxy_ssh_ws_to_process_error",
            error.to_string(),
            EventFields::default()
                .with_operation("proxy_ssh_ws_to_process")
                .with_resource("proxy_connection", connection_id.to_string()),
        );
        mark_ssh_terminal_relay_failure(
            output_state,
            "SSH terminal input write failed",
            "pty_write_error",
        )
        .await;
        return Err(error);
    }
    Ok(())
}

pub(super) fn normalized_ssh_password(launch: &SshLaunchConfig) -> Option<String> {
    launch
        .password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) async fn write_ssh_session_key_files(
    material: &SshSessionMaterial,
    launch: &SshLaunchConfig,
) -> Result<SshSessionKeyPaths, AppError> {
    let target_private_key = match launch.private_key.as_deref() {
        Some(private_key) => Some(write_private_key_file(&material.dir, private_key).await?),
        None => None,
    };

    // OpenSSH offers every supplied identity to each hop in the ProxyJump
    // chain, so writing the hop keys here is sufficient; the `-J` spec carries
    // host/port/user. Empty `jump` leaves this list empty and the chain unset.
    let mut jump_private_keys = Vec::new();
    for (index, hop) in launch.jump.iter().enumerate() {
        let Some(private_key) = hop.private_key.as_deref() else {
            continue;
        };
        jump_private_keys
            .push(write_jump_private_key_file(&material.dir, index, private_key).await?);
    }

    Ok(SshSessionKeyPaths {
        target_private_key,
        jump_private_keys,
    })
}

pub(super) fn append_interactive_ssh_jump_options(
    command: &mut Command,
    launch: &SshLaunchConfig,
    key_paths: &SshSessionKeyPaths,
    known_hosts: &Path,
) {
    if launch.jump.is_empty() {
        return;
    }

    let proxy_jump = launch
        .jump
        .iter()
        .map(SshJumpHop::proxy_jump_spec)
        .collect::<Vec<_>>()
        .join(",");
    command.arg("-J").arg(proxy_jump);
    append_pinned_host_key_options(command, known_hosts);
    for key_path in &key_paths.jump_private_keys {
        command.arg("-i").arg(key_path);
    }
}

pub(super) fn append_interactive_ssh_password_options(
    command: &mut Command,
    password: Option<&str>,
) {
    if password.is_none() {
        return;
    }

    command
        .arg("-o")
        .arg("PreferredAuthentications=password,keyboard-interactive")
        .arg("-o")
        .arg("PubkeyAuthentication=no")
        // Cap password attempts at one so a wrong password fails fast instead
        // of exposing a raw prompt in the terminal. Askpass still covers both
        // password and keyboard-interactive PAM flows.
        .arg("-o")
        .arg("NumberOfPasswordPrompts=1");
}

pub(super) async fn build_interactive_ssh_command(
    material: &SshSessionMaterial,
    launch: &SshLaunchConfig,
    key_paths: &SshSessionKeyPaths,
    password: Option<&str>,
    connect_timeout_secs: u64,
) -> Result<Command, AppError> {
    // Spawn `ssh` directly (never via a `sshpass` wrapper). `sshpass` owns an
    // internal PTY, which breaks the resize propagation to the real SSH child.
    // Password authentication is supplied via SSH_ASKPASS instead.
    let mut command = Command::new("ssh");
    let known_hosts = job_known_hosts_path(&material.dir);
    if let Some(password) = password {
        let askpass_path = write_askpass_helper(material.dir.clone()).await?;
        command
            .env("PIPING_SSH_ASKPASS_SECRET", password)
            .env("SSH_ASKPASS", &askpass_path)
            .env("SSH_ASKPASS_REQUIRE", "force")
            // Some OpenSSH builds require DISPLAY for the forced askpass path;
            // a placeholder is enough and no X server is needed.
            .env("DISPLAY", ":0");
    }
    command
        .env("TERM", "xterm")
        .env("COLORTERM", "truecolor")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .arg("-tt")
        .arg("-M")
        .arg("-S")
        .arg(&material.control_path)
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg("-o")
        .arg(format!("ConnectTimeout={connect_timeout_secs}"))
        // Detect silent half-open drops within roughly 45 seconds rather than
        // hanging the interactive PTY forever.
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-p")
        .arg(launch.port.to_string());
    append_pinned_host_key_options(&mut command, &known_hosts);
    append_ssh_authentication_marker_options(&mut command, &material.authentication_marker);
    append_interactive_ssh_jump_options(&mut command, launch, key_paths, &known_hosts);
    append_interactive_ssh_password_options(&mut command, password);
    if let Some(key_path) = key_paths.target_private_key.as_deref() {
        command.arg("-i").arg(key_path);
    }
    append_identities_only_when_keyed(
        &mut command,
        key_paths.target_private_key.is_some() || !key_paths.jump_private_keys.is_empty(),
    );

    // Pass the username separately and keep the IP-validated host as its own
    // non-option argv element, rather than interpolating `user@host`.
    command.arg("-l").arg(&launch.username).arg(&launch.host);
    Ok(command)
}

pub(super) async fn prepare_interactive_ssh_command(
    material: &SshSessionMaterial,
    launch: &SshLaunchConfig,
    connect_timeout_secs: u64,
) -> Result<PreparedInteractiveSshCommand, AppError> {
    let result: Result<PreparedInteractiveSshCommand, AppError> = async {
        let key_paths = write_ssh_session_key_files(material, launch).await?;
        let password = normalized_ssh_password(launch);
        let command = build_interactive_ssh_command(
            material,
            launch,
            &key_paths,
            password.as_deref(),
            connect_timeout_secs,
        )
        .await?;
        Ok(PreparedInteractiveSshCommand { command, password })
    }
    .await;
    if result.is_err() {
        cleanup_ssh_session_material(material).await;
    }
    result
}

pub(super) fn clone_ssh_pty(pty: &std::fs::File, purpose: &str) -> Result<std::fs::File, AppError> {
    pty.try_clone()
        .map_err(|error| AppError::External(format!("Failed to clone SSH PTY {purpose}: {error}")))
}

pub(super) fn configure_interactive_ssh_pty(
    command: &mut Command,
) -> Result<InteractiveSshPty, AppError> {
    let (master_pty, slave_pty) = create_pty_pair()?;
    let slave_pty_resize = clone_ssh_pty(&slave_pty, "slave for resize")?;
    let slave_pty_in = clone_ssh_pty(&slave_pty, "slave for stdin")?;
    let slave_pty_out = clone_ssh_pty(&slave_pty, "slave for stdout")?;
    command
        .stdin(Stdio::from(slave_pty_in))
        .stdout(Stdio::from(slave_pty_out))
        .stderr(Stdio::from(slave_pty));
    configure_child_process_group(command);
    Ok(InteractiveSshPty {
        master_pty,
        slave_pty_resize,
    })
}

pub(super) async fn prepare_interactive_ssh_pty(
    command: &mut Command,
    material: &SshSessionMaterial,
) -> Result<InteractiveSshPty, AppError> {
    let result = configure_interactive_ssh_pty(command);
    if result.is_err() {
        cleanup_ssh_session_material(material).await;
    }
    result
}

pub(super) fn log_interactive_ssh_session_start(
    connection_id: Uuid,
    launch: &SshLaunchConfig,
    payload: &Value,
    password: Option<&str>,
) {
    let password_set = password.is_some_and(|value| !value.trim().is_empty());
    let password_looks_like_uuid =
        password.is_some_and(|value| Uuid::parse_str(value.trim()).is_ok());
    let username_from_credentials = payload
        .as_object()
        .and_then(|map| map.get("credentials"))
        .and_then(Value::as_object)
        .and_then(|credentials| credentials.get("username"))
        .and_then(Value::as_str)
        .map(|username| !username.trim().is_empty())
        .unwrap_or(false);

    log_event(
        LogLevel::Info,
        "proxy.ssh.session.start",
        EventFields::default()
            .with_operation("proxy_ssh_spawn")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_context("host", launch.host.clone())
            .with_context("port", launch.port.to_string())
            .with_context("username", launch.username.clone())
            .with_context(
                "username_from_config",
                username_from_credentials.to_string(),
            )
            .with_context("password_set", password_set.to_string())
            .with_context(
                "password_unresolved_uuid",
                password_looks_like_uuid.to_string(),
            ),
    );
}

pub(super) async fn spawn_interactive_ssh_child(
    command: &mut Command,
    connection_id: Uuid,
    launch: &SshLaunchConfig,
    payload: &Value,
    password: Option<&str>,
    material: &SshSessionMaterial,
) -> Result<(tokio::process::Child, Option<i32>, Instant), AppError> {
    log_interactive_ssh_session_start(connection_id, launch, payload, password);
    let started_at = Instant::now();
    match command.spawn() {
        Ok(child) => {
            let child_pid = child.id().map(|pid| pid as i32);
            Ok((child, child_pid, started_at))
        }
        Err(error) => {
            cleanup_ssh_session_material(material).await;
            Err(AppError::External(format!(
                "Failed to spawn ssh process: {error}"
            )))
        }
    }
}

pub(super) async fn prepare_interactive_ssh_master_pty(
    master_pty: std::fs::File,
    child: &mut tokio::process::Child,
    material: &SshSessionMaterial,
) -> Result<(std::fs::File, std::fs::File, std::fs::File), AppError> {
    let result = (|| {
        let master_pty_resize = clone_ssh_pty(&master_pty, "master for resize")?;
        let master_pty_reader = clone_ssh_pty(&master_pty, "master for reader")?;
        Ok((master_pty_resize, master_pty_reader, master_pty))
    })();
    if result.is_err() {
        cleanup_spawned_ssh_startup_failure(child, material).await;
    }
    result
}

pub(super) async fn start_ssh_session_runtime(
    connection_id: Uuid,
    payload: &Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
) -> Result<Arc<SshSession>, AppError> {
    let launch = extract_ssh_launch_config(payload)
        .map_err(|error| AppError::Validation(format!("Invalid SSH payload: {}", error)))?;
    // Validate before allocating an owner-only generation directory or writing
    // any credential material. An invalid timeout is a request error, not an
    // SSH runtime that needs teardown.
    let connect_timeout_secs = super::super::extract_connect_timeout_seconds(payload)
        .map_err(|error| AppError::Validation(error.to_string()))?
        .round() as u64;

    let material = create_ssh_session_material().await?;
    let (known_hosts, seed_lines) = match seed_job_known_hosts(&material.dir, payload) {
        Ok(seeded) => seeded,
        Err(error) => {
            cleanup_ssh_session_material(&material).await;
            return Err(error);
        }
    };
    let control_path = material.control_path.clone();
    let PreparedInteractiveSshCommand {
        mut command,
        password,
    } = prepare_interactive_ssh_command(&material, &launch, connect_timeout_secs).await?;
    let InteractiveSshPty {
        master_pty,
        slave_pty_resize,
    } = prepare_interactive_ssh_pty(&mut command, &material).await?;
    let (mut child, child_pid, ssh_started_at) = spawn_interactive_ssh_child(
        &mut command,
        connection_id,
        &launch,
        payload,
        password.as_deref(),
        &material,
    )
    .await?;
    let (master_pty_resize, master_pty_reader, master_pty) =
        prepare_interactive_ssh_master_pty(master_pty, &mut child, &material).await?;
    let pty_reader = tokio::fs::File::from_std(master_pty_reader);
    let pty_writer = tokio::fs::File::from_std(master_pty);

    let (input_tx, input_rx) = mpsc::channel::<SshRuntimeInput>(1024);
    let (output_tx, _) = broadcast::channel::<SshHistoryChunk>(SSH_OUTPUT_BROADCAST_CAPACITY);
    let (terminal_tx, _) = watch::channel(None);
    let (ready_tx, _) = watch::channel(false);
    let (process_exit_tx, _) = watch::channel(false);
    let session = Arc::new(SshSession {
        input_tx,
        output_tx: output_tx.clone(),
        history: RwLock::new(SshHistory::new()),
        attach_lock: Mutex::new(()),
        control_path: Some(control_path.clone()),
        detached_at: Mutex::new(None),
        terminal_tx,
        ready_tx,
        process_exit_tx,
        process_exit_seal: AtomicBool::new(false),
        browser_ready_gate: StdMutex::new(()),
        #[cfg(test)]
        browser_ready_gate_waiter: StdMutex::new(None),
        browser_ready_enqueue_state: AtomicU8::new(BROWSER_READY_ENQUEUE_OPEN),
        lifecycle_transition: Mutex::new(()),
    });
    let runtime_session = session.clone();
    let terminal_session = session.clone();
    let metrics_runtime = metrics.clone();
    let readiness_session = session.clone();
    let readiness_metrics = metrics.clone();
    let (child_shutdown_tx, child_shutdown_rx) = tokio::sync::oneshot::channel();
    let child_exit_rx =
        spawn_ssh_child_exit_watcher(child, session.clone(), metrics.clone(), child_shutdown_rx);
    let output_state = Arc::new(Mutex::new(SshOutputState::default()));
    let reader_output_state = output_state.clone();
    let reader_task = tokio::spawn(run_ssh_pty_reader(
        connection_id,
        pty_reader,
        runtime_session,
        output_tx,
        metrics.clone(),
        reader_output_state,
    ));
    tokio::spawn(run_ssh_control_master_readiness(
        SshReadinessContext {
            connection_id,
            runtime_session: readiness_session,
            metrics: readiness_metrics,
            started_at: ssh_started_at,
            known_hosts,
            seed_lines: seed_lines.clone(),
        },
        control_path,
    ));

    let relay_runtime = SshRelayRuntime {
        connection_id,
        material,
        terminal_session,
        metrics: metrics_runtime,
        child_pid,
        slave_pty_resize,
        master_pty_resize,
        pty_writer,
        output_state,
        resize_applied_count: 0,
        host: launch.host.clone(),
        port: launch.port,
        seed_lines,
    };
    let relay_channels = SshRelayChannels {
        input_rx,
        child_exit_rx,
        child_shutdown_tx: Some(child_shutdown_tx),
        reader_task,
    };
    tokio::spawn(run_interactive_ssh_session_relay(
        relay_runtime,
        relay_channels,
    ));

    Ok(session)
}
