use super::*;
pub(super) fn apply_ssh_runtime_resize(runtime: &mut SshRelayRuntime, cols: u16, rows: u16) {
    if let Err(error) = apply_pty_resize(&runtime.slave_pty_resize, cols, rows) {
        log_error_event(
            "proxy.ssh.resize.failed",
            "proxy_ssh_resize_error",
            error.to_string(),
            EventFields::default()
                .with_operation("proxy_ssh_resize")
                .with_resource("proxy_connection", runtime.connection_id.to_string())
                .with_context("cols", cols.to_string())
                .with_context("rows", rows.to_string()),
        );
        return;
    }

    let _ = apply_pty_resize(&runtime.master_pty_resize, cols, rows);
    runtime.resize_applied_count = runtime.resize_applied_count.saturating_add(1);
    let should_log = runtime.resize_applied_count <= RESIZE_LOG_INITIAL_LIMIT
        || runtime
            .resize_applied_count
            .is_multiple_of(RESIZE_LOG_EVERY_NTH_EVENT);
    if should_log {
        log_event(
            LogLevel::Debug,
            "proxy.ssh.resize.applied",
            EventFields::default()
                .with_operation("proxy_ssh_resize")
                .with_resource("proxy_connection", runtime.connection_id.to_string())
                .with_context("cols", cols.to_string())
                .with_context("rows", rows.to_string()),
        );
    }
    send_sigwinch(runtime.child_pid);
}

pub(super) async fn handle_ssh_runtime_input(
    input: Option<SshRuntimeInput>,
    runtime: &mut SshRelayRuntime,
) -> SshRuntimeInputOutcome {
    match input {
        Some(SshRuntimeInput::Bytes(bytes)) => {
            let input_relay_failed = write_ssh_pty_input(
                &mut runtime.pty_writer,
                &bytes,
                runtime.connection_id,
                &runtime.output_state,
            )
            .await
            .is_err();
            if input_relay_failed {
                SshRuntimeInputOutcome::Stop { input_relay_failed }
            } else {
                SshRuntimeInputOutcome::Continue
            }
        }
        Some(SshRuntimeInput::Resize { cols, rows }) => {
            apply_ssh_runtime_resize(runtime, cols, rows);
            SshRuntimeInputOutcome::Continue
        }
        Some(SshRuntimeInput::Shutdown) | None => {
            let mut state = runtime.output_state.lock().await;
            state.close_reason = "SSH session terminated";
            state.close_reason_source = "shutdown";
            SshRuntimeInputOutcome::Stop {
                input_relay_failed: false,
            }
        }
    }
}

pub(super) async fn mark_ssh_pty_reader_finished_if_default(
    output_state: &Arc<Mutex<SshOutputState>>,
) {
    let mut state = output_state.lock().await;
    if state.close_reason_source == "default" {
        state.close_reason = "SSH session ended";
        state.close_reason_source = "pty_reader_finished";
    }
}

pub(super) async fn finish_ssh_pty_reader_task_result(
    result: Result<std::io::Result<()>, tokio::task::JoinError>,
    runtime: &SshRelayRuntime,
) -> bool {
    match result {
        Ok(Ok(())) => {
            mark_ssh_pty_reader_finished_if_default(&runtime.output_state).await;
            false
        }
        // `run_ssh_pty_reader` already records its concrete I/O failure in
        // `output_state` and metrics; it must not be folded into clean EOF.
        Ok(Err(_)) => true,
        Err(error) => {
            log_error_event(
                "proxy.ssh.relay.failed",
                "proxy_ssh_process_to_ws_task_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("proxy_ssh_process_to_ws")
                    .with_resource("proxy_connection", runtime.connection_id.to_string()),
            );
            mark_ssh_pty_reader_task_failure(&runtime.output_state).await;
            true
        }
    }
}

pub(super) async fn complete_ssh_relay_reader_event(
    relay_state: &mut SshRelayState,
    result: Result<std::io::Result<()>, tokio::task::JoinError>,
    runtime: &SshRelayRuntime,
) {
    relay_state.reader_done = true;
    relay_state.input_relay_failed = finish_ssh_pty_reader_task_result(result, runtime).await;
}

pub(super) async fn run_ssh_relay_event_loop(
    runtime: &mut SshRelayRuntime,
    channels: &mut SshRelayChannels,
) -> SshRelayState {
    let mut relay_state = SshRelayState::default();
    loop {
        tokio::select! {
            biased;
            child_exit = &mut channels.child_exit_rx => {
                relay_state.child_exit_observed = true;
                relay_state.child_status = resolve_ssh_child_exit_result(
                    child_exit,
                    &runtime.terminal_session,
                    &runtime.output_state,
                    runtime.connection_id,
                )
                .await;
                return relay_state;
            }
            input = channels.input_rx.recv() => {
                match handle_ssh_runtime_input(input, runtime).await {
                    SshRuntimeInputOutcome::Continue => {}
                    SshRuntimeInputOutcome::Stop { input_relay_failed } => {
                        relay_state.input_relay_failed = input_relay_failed;
                        return relay_state;
                    }
                }
            }
            result = &mut channels.reader_task => {
                complete_ssh_relay_reader_event(&mut relay_state, result, runtime).await;
                return relay_state;
            }
        }
    }
}

pub(super) async fn wait_for_ssh_child_after_relay_event(
    relay_state: &mut SshRelayState,
    runtime: &SshRelayRuntime,
    channels: &mut SshRelayChannels,
) {
    if relay_state.child_exit_observed {
        return;
    }

    if let Some(child_shutdown_tx) = channels.child_shutdown_tx.take() {
        let _ = child_shutdown_tx.send(());
    }
    relay_state.child_status = resolve_ssh_child_exit_result(
        (&mut channels.child_exit_rx).await,
        &runtime.terminal_session,
        &runtime.output_state,
        runtime.connection_id,
    )
    .await;
    relay_state.child_exit_observed = true;
}

pub(super) async fn mark_ssh_pty_reader_drain_timeout(
    runtime: &SshRelayRuntime,
    reader_task: &mut tokio::task::JoinHandle<std::io::Result<()>>,
) {
    // Do not snapshot terminal diagnostics while the reader can still mutate
    // them. Abort and join a hung reader before deriving the final close state.
    reader_task.abort();
    let _ = reader_task.await;
    log_error_event(
        "proxy.ssh.relay.failed",
        "proxy_ssh_process_to_ws_drain_timeout",
        "SSH terminal reader did not finish after child exit".to_string(),
        EventFields::default()
            .with_operation("proxy_ssh_process_to_ws")
            .with_resource("proxy_connection", runtime.connection_id.to_string()),
    );
    let mut state = runtime.output_state.lock().await;
    state.relay_failed = true;
    state.close_code = WS_CLOSE_INTERNAL_ERROR;
    state.close_reason = "SSH terminal reader drain timed out";
    state.close_reason_source = "pty_reader_drain_timeout";
}

pub(super) async fn drain_ssh_pty_reader_after_child_exit(
    relay_state: &mut SshRelayState,
    runtime: &SshRelayRuntime,
    channels: &mut SshRelayChannels,
) {
    if relay_state.reader_done {
        return;
    }

    relay_state.reader_done = true;
    match timeout(Duration::from_millis(500), &mut channels.reader_task).await {
        Ok(result) => {
            relay_state.input_relay_failed |=
                finish_ssh_pty_reader_task_result(result, runtime).await;
        }
        Err(_) => {
            relay_state.input_relay_failed = true;
            mark_ssh_pty_reader_drain_timeout(runtime, &mut channels.reader_task).await;
        }
    }
}

pub(super) fn resolve_ssh_terminal_close(
    authenticated_handshake: bool,
    exit_code: Option<i32>,
    close_code: u16,
    close_reason: String,
    close_reason_source: &'static str,
) -> (u16, String, &'static str) {
    match resolve_ssh_exit_before_or_after_readiness(authenticated_handshake, exit_code, close_code)
    {
        Some((close_code, close_reason, close_reason_source)) => {
            (close_code, close_reason.to_string(), close_reason_source)
        }
        None => (close_code, close_reason, close_reason_source),
    }
}

pub(super) async fn resolve_ssh_terminal_finalization(
    relay_state: SshRelayState,
    runtime: &SshRelayRuntime,
) -> SshTerminalFinalization {
    // `ExitStatus` is `Copy`, so reading `.code()` leaves the status available
    // for final event enrichment.
    let exit_code = relay_state.child_status.and_then(|status| status.code());
    let state = runtime.output_state.lock().await;
    let close_code = state.close_code;
    let close_reason = state.close_reason.to_string();
    let close_reason_source = state.close_reason_source;
    let relay_failed = relay_state.input_relay_failed || state.relay_failed;
    let stderr_excerpt = state.stderr_excerpt.clone();
    drop(state);

    // PTY text is never terminal-classification evidence. Authentication must
    // be proven by the ready transition or durable LocalCommand marker.
    let authenticated_handshake =
        ssh_session_authenticated_handshake(&runtime.terminal_session, &runtime.material).await;
    let (close_code, close_reason, close_reason_source) = resolve_ssh_terminal_close(
        authenticated_handshake,
        exit_code,
        close_code,
        close_reason,
        close_reason_source,
    );

    // A changed host key must reach the user as a named refusal with the repair,
    // not as a generic "connection ended before readiness". Only an
    // unauthenticated handshake can be a host-key refusal, so benign program
    // output that happens to contain the marker after a real login never
    // rewrites the terminal reason.
    let (close_code, close_reason, close_reason_source) = if !authenticated_handshake {
        match stderr_excerpt.as_deref().and_then(|stderr| {
            classify_ssh_host_key_failure(stderr, &runtime.host, runtime.port, &runtime.seed_lines)
        }) {
            Some(error) => (
                WS_CLOSE_INVALID_PAYLOAD,
                error.to_string(),
                "host_key_mismatch",
            ),
            None => (close_code, close_reason, close_reason_source),
        }
    } else {
        (close_code, close_reason, close_reason_source)
    };

    SshTerminalFinalization {
        child_status: relay_state.child_status,
        close_code,
        close_reason,
        close_reason_source,
        relay_failed,
        stderr_excerpt,
    }
}

pub(super) fn ssh_session_end_event_fields(
    connection_id: Uuid,
    terminal: &SshTerminalFinalization,
) -> EventFields {
    let mut fields = EventFields::default()
        .with_operation("proxy_ssh_session_end")
        .with_resource("proxy_connection", connection_id.to_string())
        .with_status("closed")
        .with_context("close_code", terminal.close_code.to_string())
        .with_context("close_reason", terminal.close_reason.clone())
        .with_context("reason_source", terminal.close_reason_source.to_string());
    if let Some(status) = terminal.child_status {
        let process_exit_code = status
            .code()
            .map_or_else(|| "signal_or_unknown".to_string(), |code| code.to_string());
        fields = fields.with_context("process_exit_code", process_exit_code);
    }
    if let Some(stderr) = terminal.stderr_excerpt.as_deref() {
        fields = fields.with_context("stderr_excerpt", stderr.to_string());
    }
    fields
}

pub(super) fn ssh_terminal_requires_error_metrics(relay_failed: bool, close_code: u16) -> bool {
    relay_failed
        || matches!(
            close_code,
            WS_CLOSE_INVALID_PAYLOAD | WS_CLOSE_INTERNAL_ERROR | WS_CLOSE_TARGET_UNAVAILABLE
        )
}

pub(super) async fn finalize_ssh_session_resources(
    runtime: &SshRelayRuntime,
    terminal: &SshTerminalFinalization,
) {
    if ssh_terminal_requires_error_metrics(terminal.relay_failed, terminal.close_code) {
        mark_connection_metrics_error(&runtime.metrics).await;
    } else {
        mark_connection_metrics_disconnected(&runtime.metrics).await;
    }
    cleanup_ssh_session_material(&runtime.material).await;
}

pub(super) async fn publish_ssh_session_terminal_finalization(
    runtime: &SshRelayRuntime,
    terminal: SshTerminalFinalization,
) {
    let fields = ssh_session_end_event_fields(runtime.connection_id, &terminal);
    publish_ssh_terminal_after_finalization(
        &runtime.terminal_session,
        finalize_ssh_session_resources(runtime, &terminal),
        terminal.close_code,
        terminal.close_reason.clone(),
    )
    .await;
    log_event(LogLevel::Info, "proxy.ssh.session.end", fields);
}

pub(super) async fn run_interactive_ssh_session_relay(
    mut runtime: SshRelayRuntime,
    mut channels: SshRelayChannels,
) {
    let mut relay_state = run_ssh_relay_event_loop(&mut runtime, &mut channels).await;
    wait_for_ssh_child_after_relay_event(&mut relay_state, &runtime, &mut channels).await;
    drain_ssh_pty_reader_after_child_exit(&mut relay_state, &runtime, &mut channels).await;
    let terminal = resolve_ssh_terminal_finalization(relay_state, &runtime).await;
    publish_ssh_session_terminal_finalization(&runtime, terminal).await;
}
