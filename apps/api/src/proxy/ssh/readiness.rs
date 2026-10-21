use super::*;
/// Independent host-reachability watchdog for an interactive SSH session.
///
/// Every `interval`, runs `probe` (a TCP-reachability check that resolves to
/// `true` when the host:port is reachable). Tracks CONSECUTIVE failures; the
/// first time the count reaches `failure_threshold`, invokes `on_upstream_lost`
/// exactly once with a [`SshTerminalState`] carrying
/// [`WS_CLOSE_TARGET_UNAVAILABLE`] / "Upstream connection lost", then returns.
/// Any successful probe resets the counter (so a transient blip never tears down
/// a live session). Returns immediately — WITHOUT firing — when `cancel` signals
/// session end (the relay set the terminal state), so the task never leaks.
///
/// `probe` is injected so the unit tests can feed deterministic
/// success/failure sequences without touching the network.
#[cfg(test)]
pub(super) async fn run_ssh_reachability_watchdog<P, Fut, C, CFut>(
    connection_id: Uuid,
    mut cancel: watch::Receiver<Option<SshTerminalState>>,
    mut probe: P,
    interval: Duration,
    failure_threshold: u32,
    on_upstream_lost: C,
) where
    P: FnMut() -> Fut,
    Fut: Future<Output = bool>,
    C: FnOnce(SshTerminalState) -> CFut,
    CFut: Future<Output = ()>,
{
    // Session may already be terminal (fast startup-then-end) — bail before
    // probing so we never fire against an already-closed session.
    if cancel.borrow().is_some() {
        return;
    }

    let mut ticker = tokio::time::interval(interval);
    // A slow `connect()` (up to the probe timeout) must not let ticks pile up
    // into a burst of overlapping probes; skip missed ticks instead.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // first tick is immediate; skip it so probe 1 is delayed.

    let mut consecutive_failures: u32 = 0;
    loop {
        tokio::select! {
            // Relay ended the session: cancel cleanly without firing.
            changed = cancel.changed() => {
                if changed.is_err() || cancel.borrow().is_some() {
                    return;
                }
            }
            _ = ticker.tick() => {
                if probe().await {
                    consecutive_failures = 0;
                    continue;
                }
                consecutive_failures = consecutive_failures.saturating_add(1);
                if consecutive_failures < failure_threshold {
                    continue;
                }
                log_event(
                    LogLevel::Warning,
                    "proxy.ssh.reachability.lost",
                    EventFields::default()
                        .with_operation("proxy_ssh_reachability_watchdog")
                        .with_resource("proxy_connection", connection_id.to_string())
                        .with_status("upstream_unreachable")
                        .with_context("consecutive_failures", consecutive_failures.to_string()),
                );
                on_upstream_lost(SshTerminalState {
                    close_code: WS_CLOSE_TARGET_UNAVAILABLE,
                    close_reason: "Upstream connection lost".to_string(),
                })
                .await;
                return;
            }
        }
    }
}

/// Identity and metrics context shared by the ControlMaster readiness watcher.
pub(super) struct SshReadinessContext {
    pub(super) connection_id: Uuid,
    pub(super) runtime_session: Arc<SshSession>,
    pub(super) metrics: Arc<RwLock<ConnectionMetricsState>>,
    pub(super) started_at: Instant,
    pub(super) known_hosts: PathBuf,
    pub(super) seed_lines: Vec<String>,
}

/// Waits for OpenSSH to expose the per-session ControlMaster socket, which is
/// the first durable proof that the SSH transport and authentication completed.
/// This is a state-driven watcher with no startup deadline: a pending handshake
/// remains `connecting`, while the session's terminal state reports a failure.
pub(super) async fn run_ssh_control_master_readiness(
    context: SshReadinessContext,
    control_path: String,
) {
    run_ssh_control_master_readiness_with_probe(context, move || {
        let control_path = control_path.clone();
        async move { UnixStream::connect(control_path).await.is_ok() }
    })
    .await;
}

/// Waits for an authenticated ControlMaster probe while honoring the durable
/// child-exit seal. Kept probe-driven so the retry and terminal ordering can be
/// tested without a real SSH server.
pub(super) async fn run_ssh_control_master_readiness_with_probe<P, Fut>(
    context: SshReadinessContext,
    mut control_master_ready: P,
) where
    P: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let SshReadinessContext {
        connection_id,
        runtime_session,
        metrics,
        started_at,
        known_hosts,
        seed_lines,
    } = context;
    let mut terminal_rx = runtime_session.terminal_receiver();
    let mut process_exit_rx = runtime_session.process_exit_receiver();
    if runtime_session.terminal_state().is_some() || runtime_session.process_exit_sealed() {
        return;
    }
    let mut ticker = tokio::time::interval(SSH_CONTROL_MASTER_READY_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = terminal_rx.changed() => {
                if changed.is_err() || terminal_rx.borrow().is_some() {
                    return;
                }
            }
            changed = process_exit_rx.changed() => {
                if changed.is_err() || *process_exit_rx.borrow() {
                    return;
                }
            }
            _ = ticker.tick() => {
                if runtime_session.terminal_state().is_some()
                    || runtime_session.process_exit_sealed()
                {
                    return;
                }
                if try_mark_ssh_session_ready_after_control_master_probe(
                    &runtime_session,
                    &metrics,
                    started_at,
                    control_master_ready().await,
                )
                .await
                {
                    // The handshake is complete and OpenSSH has written any
                    // first-use key into the pinned file; harvest it now.
                    record_harvested_host_keys(&metrics, &known_hosts, &seed_lines).await;
                    log_event(
                        LogLevel::Info,
                        "proxy.ssh.session.ready",
                        EventFields::default()
                            .with_operation("proxy_ssh_control_master_ready")
                            .with_resource("proxy_connection", connection_id.to_string())
                            .with_status("connected"),
                    );
                    return;
                }
            }
        }
    }
}

/// Records readiness after a successful ControlMaster probe only when the
/// child-exit seal and terminal state still permit the transition.
pub(super) async fn try_mark_ssh_session_ready_after_control_master_probe(
    runtime_session: &SshSession,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    started_at: Instant,
    control_master_ready: bool,
) -> bool {
    if !control_master_ready
        || runtime_session.terminal_state().is_some()
        || runtime_session.process_exit_sealed()
    {
        return false;
    }

    runtime_session
        .mark_ready_and_connected(
            metrics,
            Some(i64::try_from(started_at.elapsed().as_millis()).unwrap_or(i64::MAX)),
        )
        .await
}

pub(super) async fn run_ssh_pty_reader(
    connection_id: Uuid,
    mut pty_reader: tokio::fs::File,
    runtime_session: Arc<SshSession>,
    output_tx: broadcast::Sender<SshHistoryChunk>,
    metrics_runtime: Arc<RwLock<ConnectionMetricsState>>,
    output_state: Arc<Mutex<SshOutputState>>,
) -> std::io::Result<()> {
    let mut pty_buf = vec![0u8; SSH_PTY_BUFFER_SIZE];
    loop {
        let read = match pty_reader.read(&mut pty_buf).await {
            Ok(value) => value,
            Err(error) => {
                {
                    let mut state = output_state.lock().await;
                    state.relay_failed = true;
                    state.close_code = WS_CLOSE_INTERNAL_ERROR;
                    state.close_reason = "SSH terminal read failed";
                    state.close_reason_source = "pty_read_error";
                }
                log_error_event(
                    "proxy.ssh.relay.failed",
                    "proxy_ssh_process_to_ws_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("proxy_ssh_process_to_ws")
                        .with_resource("proxy_connection", connection_id.to_string()),
                );
                mark_connection_metrics_error(&metrics_runtime).await;
                return Err(error);
            }
        };
        if read == 0 {
            break;
        }

        let chunk = pty_buf[..read].to_vec();
        let output_text = String::from_utf8_lossy(&chunk);
        {
            let mut state = output_state.lock().await;
            // Keep the first chunk as the general diagnostic, but also retain a
            // chunk carrying a host-key failure marker even when earlier output
            // arrived first: that excerpt is what a mismatch refusal is derived
            // from at finalization.
            let carries_host_key_failure = output_text.contains(SSH_HOST_KEY_CHANGED_MARKER)
                || output_text.contains(SSH_HOST_KEY_VERIFICATION_FAILED_MARKER);
            if state.stderr_excerpt.is_none() || carries_host_key_failure {
                state.stderr_excerpt = normalize_stderr_excerpt(&output_text);
            }
        }

        let history_chunk = runtime_session.history.write().await.push(chunk);
        let _ = output_tx.send(history_chunk);
        record_ssh_pty_output_traffic_if_ready(&runtime_session, &metrics_runtime, read).await;
    }
    Ok(())
}

/// Records PTY output only after the interactive runtime completed the
/// authenticated ControlMaster transition. Before then, OpenSSH diagnostic
/// text is useful history but is not proof of a usable target connection.
pub(super) async fn record_ssh_pty_output_traffic_if_ready(
    runtime_session: &SshSession,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    bytes: usize,
) {
    if !runtime_session.can_emit_browser_ready() {
        return;
    }
    for link_type in [LinkType::ProxyProtocol, LinkType::ClientProxy] {
        record_connection_traffic(metrics, link_type, TrafficDirection::Rx, bytes).await;
    }
}

/// Returns whether the interactive runtime proved an authenticated upstream
/// connection before it terminated. The published `ready` flag is the normal
/// live source. For a short-lived master that appears and exits between its
/// socket polls, OpenSSH's post-connect `LocalCommand` writes a marker inside
/// this generation's private material directory before cleanup begins.
///
/// Deliberately do not inspect `control_path` metadata here: a pathname is a
/// sampled filesystem artifact, not historical authentication evidence.
pub(super) async fn ssh_session_authenticated_handshake(
    session: &SshSession,
    material: &SshSessionMaterial,
) -> bool {
    if session.is_ready() {
        return true;
    }
    tokio::fs::try_exists(&material.authentication_marker)
        .await
        .unwrap_or(false)
}

/// Maps only process-exit facts to a browser terminal outcome. PTY output is
/// intentionally excluded: a remote forced command can emit strings such as
/// "Permission denied" after a healthy authenticated handshake.
///
/// An unconfirmed non-success exit (including a signal with no numeric code)
/// is terminal but ambiguous. Treat it as target unavailable instead of
/// calling it an authentication failure or a normal close. Once authentication
/// was confirmed, OpenSSH's 255 exit remains the specific upstream-loss
/// signal; other nonzero codes can be remote-command outcomes and stay normal.
pub(super) fn resolve_ssh_exit_before_or_after_readiness(
    authenticated_handshake: bool,
    exit_code: Option<i32>,
    current_close_code: u16,
) -> Option<(u16, &'static str, &'static str)> {
    if current_close_code != WS_CLOSE_NORMAL || exit_code == Some(0) {
        return None;
    }
    if !authenticated_handshake {
        return Some((
            WS_CLOSE_TARGET_UNAVAILABLE,
            "SSH connection ended before readiness was confirmed",
            "unconfirmed_unsuccessful_exit",
        ));
    }
    if exit_code == Some(255) {
        return Some((
            WS_CLOSE_TARGET_UNAVAILABLE,
            "Upstream connection lost",
            "exit_255_upstream_lost",
        ));
    }
    None
}

#[cfg(test)]
pub(super) async fn probe_ssh_target_reachability(
    host: &str,
    port: u16,
    timeout_secs: u64,
) -> Result<(), AppError> {
    let deadline = Duration::from_secs(timeout_secs);

    // Phase 1: TCP connect
    let mut stream = match timeout(deadline, TcpStream::connect((host, port))).await {
        Ok(Ok(stream)) => {
            super::super::configure_interactive_tcp_stream(&stream);
            stream
        }
        Ok(Err(error)) => {
            return Err(AppError::External(format!(
                "SSH target {}:{} is unreachable: {}",
                host, port, error
            )));
        }
        Err(_) => {
            return Err(AppError::External(format!(
                "SSH target {}:{} connection timed out after {}s",
                host, port, timeout_secs
            )));
        }
    };

    // Phase 2: Read SSH version banner (RFC 4253 §4.2).
    // The server MUST send a line starting with "SSH-" before the key exchange.
    let banner_timeout = Duration::from_secs(timeout_secs.min(3));
    let mut buf = [0u8; 256];
    match timeout(banner_timeout, stream.read(&mut buf)).await {
        Ok(Ok(n)) if n >= b"SSH-".len() && buf[..n].starts_with(b"SSH-") => Ok(()),
        Ok(Ok(0)) => Err(AppError::External(format!(
            "SSH target {}:{} closed connection without sending banner",
            host, port
        ))),
        Ok(Ok(_)) => Err(AppError::External(format!(
            "SSH target {}:{} did not send a valid SSH banner",
            host, port
        ))),
        Ok(Err(error)) => Err(AppError::External(format!(
            "SSH target {}:{} banner read failed: {}",
            host, port, error
        ))),
        Err(_) => Err(AppError::External(format!(
            "SSH target {}:{} banner exchange timed out after {}s",
            host, port, timeout_secs
        ))),
    }
}
