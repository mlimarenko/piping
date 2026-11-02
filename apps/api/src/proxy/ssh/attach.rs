use super::*;
/// Enqueues the browser-facing ready control frame only if its lifecycle claim
/// still wins after the WebSocket sink is writable. `start_send` itself is
/// synchronous and runs while the claim is held; flushing happens afterward
/// so the child-exit seal never waits on client I/O.
pub(super) async fn enqueue_browser_ready_frame<S>(
    session: &SshSession,
    ws_sender: &mut S,
) -> Result<bool, S::Error>
where
    S: Sink<axum::extract::ws::Message> + Unpin,
{
    enqueue_browser_ready_frame_after_claim(session, ws_sender, || {}).await
}

/// Internal split point for the non-awaiting enqueue transaction. Production
/// uses a no-op callback; the deterministic regression test starts a separate
/// real `seal_process_exit` caller after `SENDING` but before `start_send`.
/// The callback runs while the synchronous browser-ready gate is held and
/// therefore must never call [`SshSession::seal_process_exit`] on the same
/// thread.
pub(super) async fn enqueue_browser_ready_frame_after_claim<S, H>(
    session: &SshSession,
    ws_sender: &mut S,
    before_start_send: H,
) -> Result<bool, S::Error>
where
    S: Sink<axum::extract::ws::Message> + Unpin,
    H: FnOnce(),
{
    std::future::poll_fn(|cx| Pin::new(&mut *ws_sender).poll_ready(cx)).await?;
    {
        let Some(_ready_claim) = session.claim_browser_ready_enqueue() else {
            return Ok(false);
        };
        if !session.begin_browser_ready_enqueue() {
            session.release_browser_ready_enqueue_claim();
            return Ok(false);
        }
        before_start_send();
        let start_send_result = ws_sender.start_send_unpin(axum::extract::ws::Message::Text(
            format!("{WS_CONTROL_PREFIX}{{\"type\":\"ready\"}}").into(),
        ));
        session.release_browser_ready_enqueue_claim();
        start_send_result?;
    }
    ws_sender.flush().await?;
    Ok(true)
}

/// Buffers client input while the upstream SSH master is still authenticating.
/// Reading the WebSocket at this stage is required to observe a client close;
/// forwarding the bytes would change their meaning if OpenSSH is currently at
/// an authentication prompt. The normal relay drains this queue only after the
/// authenticated-ready transition.
pub(super) fn queue_ssh_input_while_starting(
    pending_input: &mut VecDeque<axum::extract::ws::Message>,
    pending_bytes: &mut usize,
    message: axum::extract::ws::Message,
) -> SshPendingInputResult {
    let payload_size = match &message {
        axum::extract::ws::Message::Close(_) => return SshPendingInputResult::ClientLegDropped,
        axum::extract::ws::Message::Ping(_) | axum::extract::ws::Message::Pong(_) => {
            return SshPendingInputResult::Buffered;
        }
        axum::extract::ws::Message::Text(text) => {
            if parse_ws_control_message(text.as_ref())
                .is_some_and(|control| control.control_type == "detach")
            {
                return SshPendingInputResult::ClientLegDropped;
            }
            text.len()
        }
        axum::extract::ws::Message::Binary(bytes) => bytes.len(),
    };
    if payload_size > SSH_PENDING_INPUT_MAX_BYTES.saturating_sub(*pending_bytes) {
        return SshPendingInputResult::LimitExceeded;
    }
    *pending_bytes = pending_bytes.saturating_add(payload_size);
    pending_input.push_back(message);
    SshPendingInputResult::Buffered
}

/// A receiver subscribed before readiness can contain both the history tail
/// we replay and older chunks already evicted from that bounded history.
/// Once the snapshot reaches `end`, forwarding *any* older live chunk would
/// reorder the PTY stream after the newer replayed tail. Drop every sequence
/// at or before the snapshot end; only later chunks are safe live output.
pub(super) fn ssh_output_precedes_or_was_replayed_from_history(
    seq: u64,
    replayed_history_end: Option<u64>,
) -> bool {
    replayed_history_end.is_some_and(|end| seq <= end)
}

pub(super) fn configure_child_process_group(command: &mut Command) {
    super::super::unix_sys::configure_child_process_group(command);
}

type SshWsSender =
    futures_util::stream::SplitSink<axum::extract::ws::WebSocket, axum::extract::ws::Message>;
type SshWsReceiver = futures_util::stream::SplitStream<axum::extract::ws::WebSocket>;

async fn send_terminal_close(sender: &mut SshWsSender, terminal: SshTerminalState) {
    let _ = sender
        .send(axum::extract::ws::Message::Close(Some(
            axum::extract::ws::CloseFrame {
                code: terminal.close_code,
                reason: terminal.close_reason.into(),
            },
        )))
        .await;
}

/// Client-side input state for the pre-ready window: the websocket receiver
/// plus the bounded buffer of frames parked while the SSH master authenticates.
struct SshClientInput<'a> {
    receiver: &'a mut SshWsReceiver,
    pending: &'a mut VecDeque<axum::extract::ws::Message>,
    pending_bytes: &'a mut usize,
}

async fn wait_for_ssh_session_ready(
    session: &SshSession,
    sender: &mut SshWsSender,
    input: SshClientInput<'_>,
    ready_rx: &mut watch::Receiver<bool>,
    terminal_rx: &mut watch::Receiver<Option<SshTerminalState>>,
    process_exit_rx: &mut watch::Receiver<bool>,
) -> bool {
    let SshClientInput {
        receiver,
        pending,
        pending_bytes,
    } = input;
    let mut keepalive = tokio::time::interval(SSH_ATTACH_KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    keepalive.tick().await;
    while !session.is_ready() {
        let keep_waiting = tokio::select! {
            changed = ready_rx.changed() => changed.is_ok(),
            changed = terminal_rx.changed() =>
                handle_ready_wait_terminal(sender, terminal_rx, changed.is_ok()).await,
            changed = process_exit_rx.changed() =>
                handle_ready_wait_process_exit(sender, terminal_rx, process_exit_rx, changed.is_ok()).await,
            message = receiver.next() =>
                handle_ready_wait_message(session, sender, pending, pending_bytes, message).await,
            _ = keepalive.tick() => send_ready_wait_keepalive(session, sender).await,
        };
        if !keep_waiting {
            return false;
        }
    }
    true
}

async fn handle_ready_wait_terminal(
    sender: &mut SshWsSender,
    terminal_rx: &mut watch::Receiver<Option<SshTerminalState>>,
    changed_ok: bool,
) -> bool {
    let terminal = if changed_ok {
        terminal_rx.borrow().clone()
    } else {
        None
    }
    .unwrap_or_else(SshTerminalState::session_ended);
    send_terminal_close(sender, terminal).await;
    false
}

async fn handle_ready_wait_process_exit(
    sender: &mut SshWsSender,
    terminal_rx: &mut watch::Receiver<Option<SshTerminalState>>,
    process_exit_rx: &mut watch::Receiver<bool>,
    changed_ok: bool,
) -> bool {
    if !changed_ok || *process_exit_rx.borrow() {
        let terminal = await_terminal_after_lifecycle_seal(terminal_rx).await;
        send_terminal_close(sender, terminal).await;
        return false;
    }
    true
}

async fn handle_ready_wait_message(
    session: &SshSession,
    sender: &mut SshWsSender,
    pending: &mut VecDeque<axum::extract::ws::Message>,
    pending_bytes: &mut usize,
    message: Option<Result<axum::extract::ws::Message, axum::Error>>,
) -> bool {
    let Some(Ok(message)) = message else {
        session.mark_detached().await;
        return false;
    };
    match queue_ssh_input_while_starting(pending, pending_bytes, message) {
        SshPendingInputResult::Buffered => true,
        SshPendingInputResult::ClientLegDropped => {
            session.mark_detached().await;
            false
        }
        SshPendingInputResult::LimitExceeded => {
            session.mark_detached().await;
            send_terminal_close(
                sender,
                SshTerminalState {
                    close_code: WS_CLOSE_INVALID_PAYLOAD,
                    close_reason: "SSH session is not ready".to_string(),
                },
            )
            .await;
            false
        }
    }
}

async fn send_ready_wait_keepalive(session: &SshSession, sender: &mut SshWsSender) -> bool {
    if sender
        .send(axum::extract::ws::Message::Ping(Vec::new().into()))
        .await
        .is_err()
    {
        session.mark_detached().await;
        return false;
    }
    true
}

async fn send_browser_ready(
    session: &SshSession,
    sender: &mut SshWsSender,
    terminal_rx: &mut watch::Receiver<Option<SshTerminalState>>,
) -> bool {
    if let Some(terminal) = session.terminal_state() {
        send_terminal_close(sender, terminal).await;
        return false;
    }
    match enqueue_browser_ready_frame(session, sender).await {
        Ok(true) => true,
        Err(_) => {
            session.mark_detached().await;
            false
        }
        Ok(false) => {
            let terminal = if let Some(terminal) = session.terminal_state() {
                Some(terminal)
            } else if session.ready_enqueue_lifecycle_sealed() {
                Some(await_terminal_after_lifecycle_seal(terminal_rx).await)
            } else {
                None
            };
            if let Some(terminal) = terminal {
                send_terminal_close(sender, terminal).await;
            }
            false
        }
    }
}

async fn replay_ssh_history(
    session: &SshSession,
    sender: &mut SshWsSender,
) -> Result<Option<u64>, ()> {
    let history = session.history_snapshot().await;
    let replayed_history_end = history.last().map(|chunk| chunk.seq);
    for chunk in history {
        if sender
            .send(axum::extract::ws::Message::Binary(chunk.bytes.into()))
            .await
            .is_err()
        {
            session.mark_detached().await;
            return Err(());
        }
    }
    Ok(replayed_history_end)
}

fn spawn_ssh_input_forwarder(
    connection_id: Uuid,
    mut receiver: SshWsReceiver,
    mut pending_input: VecDeque<axum::extract::ws::Message>,
    input_tx: mpsc::Sender<SshRuntimeInput>,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut inbound_frame_count: u64 = 0;
        loop {
            let message = match pending_input.pop_front() {
                Some(message) => Ok(message),
                None => match receiver.next().await {
                    Some(message) => message,
                    None => break,
                },
            };
            let Ok(message) = message else { break };
            inbound_frame_count = inbound_frame_count.saturating_add(1);
            if !forward_ssh_input_message(
                connection_id,
                inbound_frame_count,
                message,
                &input_tx,
                &metrics,
            )
            .await
            {
                break;
            }
        }
    })
}

async fn record_ssh_input_traffic(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    payload_size: usize,
) {
    for link_type in [LinkType::ClientProxy, LinkType::ProxyProtocol] {
        record_connection_traffic(metrics, link_type, TrafficDirection::Tx, payload_size).await;
    }
}

async fn forward_ssh_text_input(
    connection_id: Uuid,
    frame_index: u64,
    text: axum::extract::ws::Utf8Bytes,
    input_tx: &mpsc::Sender<SshRuntimeInput>,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) -> bool {
    let text_len = text.len();
    if let Some(control) = parse_ws_control_message(text.as_ref()) {
        log_ssh_inbound_frame(connection_id, frame_index, "text_control", text_len);
        if control.control_type == "detach" {
            return false;
        }
        if let Some((cols, rows)) = control.resize_dimensions() {
            let _ = input_tx.send(SshRuntimeInput::Resize { cols, rows }).await;
            return true;
        }
    }
    log_ssh_inbound_frame(connection_id, frame_index, "text_input", text_len);
    if input_tx
        .send(SshRuntimeInput::Bytes(text.as_bytes().to_vec()))
        .await
        .is_err()
    {
        return false;
    }
    record_ssh_input_traffic(metrics, text_len).await;
    true
}

async fn forward_ssh_binary_input(
    connection_id: Uuid,
    frame_index: u64,
    bytes: axum::body::Bytes,
    input_tx: &mpsc::Sender<SshRuntimeInput>,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) -> bool {
    let payload_size = bytes.len();
    log_ssh_inbound_frame(connection_id, frame_index, "binary_input", payload_size);
    if input_tx
        .send(SshRuntimeInput::Bytes(bytes.to_vec()))
        .await
        .is_err()
    {
        return false;
    }
    record_ssh_input_traffic(metrics, payload_size).await;
    true
}

pub(super) async fn forward_ssh_input_message(
    connection_id: Uuid,
    frame_index: u64,
    message: axum::extract::ws::Message,
    input_tx: &mpsc::Sender<SshRuntimeInput>,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) -> bool {
    match message {
        axum::extract::ws::Message::Text(text) => {
            forward_ssh_text_input(connection_id, frame_index, text, input_tx, metrics).await
        }
        axum::extract::ws::Message::Binary(bytes) => {
            forward_ssh_binary_input(connection_id, frame_index, bytes, input_tx, metrics).await
        }
        axum::extract::ws::Message::Close(_) => false,
        axum::extract::ws::Message::Ping(_) | axum::extract::ws::Message::Pong(_) => true,
    }
}

async fn relay_ssh_session(
    session: &SshSession,
    sender: &mut SshWsSender,
    mut output_rx: broadcast::Receiver<SshHistoryChunk>,
    mut terminal_rx: watch::Receiver<Option<SshTerminalState>>,
    mut input_task: tokio::task::JoinHandle<()>,
    replayed_history_end: Option<u64>,
) {
    let mut keepalive = tokio::time::interval(SSH_ATTACH_KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    keepalive.tick().await;

    let outcome = loop {
        tokio::select! {
            chunk = output_rx.recv() => {
                if let Some(outcome) = handle_relay_output_chunk(sender, chunk, replayed_history_end).await {
                    break outcome;
                }
            }
            _ = &mut input_task => break SshAttachOutcome::ClientLegDropped,
            changed = terminal_rx.changed() => {
                break relay_terminal_outcome(&terminal_rx, changed.is_ok());
            }
            _ = keepalive.tick() => {
                if !send_relay_keepalive(sender).await {
                    break SshAttachOutcome::ClientLegDropped;
                }
            }
        }
    };
    input_task.abort();

    finish_relay_session(session, sender, outcome).await;
}

/// Forwards one output chunk unless it was already covered by the replayed
/// history tail. `Some(outcome)` ends the relay; `None` keeps it running.
async fn handle_relay_output_chunk(
    sender: &mut SshWsSender,
    chunk: Result<SshHistoryChunk, broadcast::error::RecvError>,
    replayed_history_end: Option<u64>,
) -> Option<SshAttachOutcome> {
    match chunk {
        Ok(chunk) => {
            if ssh_output_precedes_or_was_replayed_from_history(chunk.seq, replayed_history_end) {
                return None;
            }
            if sender
                .send(axum::extract::ws::Message::Binary(chunk.bytes.into()))
                .await
                .is_err()
            {
                return Some(SshAttachOutcome::ClientLegDropped);
            }
            None
        }
        Err(broadcast::error::RecvError::Lagged(_)) => None,
        Err(broadcast::error::RecvError::Closed) => Some(SshAttachOutcome::SessionEnded(
            SshTerminalState::session_ended(),
        )),
    }
}

fn relay_terminal_outcome(
    terminal_rx: &watch::Receiver<Option<SshTerminalState>>,
    changed_ok: bool,
) -> SshAttachOutcome {
    let terminal = if changed_ok {
        terminal_rx.borrow().clone()
    } else {
        None
    }
    .unwrap_or_else(SshTerminalState::session_ended);
    SshAttachOutcome::SessionEnded(terminal)
}

async fn send_relay_keepalive(sender: &mut SshWsSender) -> bool {
    sender
        .send(axum::extract::ws::Message::Ping(Vec::new().into()))
        .await
        .is_ok()
}

async fn finish_relay_session(
    session: &SshSession,
    sender: &mut SshWsSender,
    outcome: SshAttachOutcome,
) {
    match outcome {
        SshAttachOutcome::ClientLegDropped => {
            session.mark_detached().await;
            send_terminal_close(
                sender,
                SshTerminalState {
                    close_code: WS_CLOSE_NORMAL,
                    close_reason: WS_REASON_DETACHED_ALIVE.to_string(),
                },
            )
            .await;
        }
        SshAttachOutcome::SessionEnded(terminal) => send_terminal_close(sender, terminal).await,
    }
}

pub(in crate::proxy) async fn proxy_ws_ssh_attachable_loop(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    mut socket: axum::extract::ws::WebSocket,
    payload: &Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
) -> Result<(), AppError> {
    let payload = payload.clone();
    let startup_metrics = metrics.clone();
    let session = match get_or_start_ssh_session(state.clone(), connection_id, move || async move {
        start_new_ssh_session_runtime(connection_id, &payload, startup_metrics).await
    })
    .await
    {
        Ok(session) => session,
        Err(error) => {
            let (code, reason) = classify_ssh_startup_close_frame(&error);
            super::super::send_close_frame(&mut socket, code, reason).await;
            return Err(error);
        }
    };

    let _attach_guard = match session.attach_lock.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            super::super::send_close_frame(&mut socket, 1008, "Connection is already attached")
                .await;
            return Ok(());
        }
    };

    // A client is now attached: clear any pending detach grace timer so the
    // reaper does not GC a session that has just been re-attached.
    session.mark_attached().await;

    let output_rx = session.output_tx.subscribe();
    let mut terminal_rx = session.terminal_receiver();
    let mut ready_rx = session.ready_receiver();
    let mut process_exit_rx = session.process_exit_receiver();
    let (mut ws_sender, mut ws_receiver) = socket.split();
    if let Some(terminal) = session.terminal_state() {
        send_terminal_close(&mut ws_sender, terminal).await;
        return Ok(());
    }
    if session.process_exit_sealed() {
        let terminal = await_terminal_after_lifecycle_seal(&mut terminal_rx).await;
        send_terminal_close(&mut ws_sender, terminal).await;
        return Ok(());
    }

    // Do not let the UI turn this target green merely because `/usr/bin/ssh`
    // was spawned. The session becomes ready only after its ControlMaster
    // socket exists (an authenticated upstream SSH master), or terminates with
    // a truthful error while that handshake is still pending. We still consume
    // the client WebSocket in this state: otherwise a stalled authentication
    // could retain an attach lock forever after the browser disconnects.
    let mut pending_input = VecDeque::new();
    let mut pending_input_bytes = 0;
    if !wait_for_ssh_session_ready(
        &session,
        &mut ws_sender,
        SshClientInput {
            receiver: &mut ws_receiver,
            pending: &mut pending_input,
            pending_bytes: &mut pending_input_bytes,
        },
        &mut ready_rx,
        &mut terminal_rx,
        &mut process_exit_rx,
    )
    .await
    {
        return Ok(());
    }
    // The lifecycle transition lock prevents a terminal state that won the
    // handshake race from being followed by a synthetic ready frame.
    if !send_browser_ready(&session, &mut ws_sender, &mut terminal_rx).await {
        return Ok(());
    }

    // Do not replay PTY bytes before the explicit authenticated-ready frame:
    // the frontend treats terminal payload as usable input, and OpenSSH may
    // emit a banner or error text before authentication completes.
    let Ok(replayed_history_end) = replay_ssh_history(&session, &mut ws_sender).await else {
        return Ok(());
    };

    // Drive client→SSH input in its own task. It completes when the *client*
    // leg ends (clean Close, explicit detach control frame, or a receive
    // error) — i.e. the SSH session itself is still alive and re-attachable.
    // Any input received while the handshake was pending is replayed first,
    // now that it cannot be interpreted as authentication input by OpenSSH.
    let ws_to_ssh = spawn_ssh_input_forwarder(
        connection_id,
        ws_receiver,
        pending_input,
        session.input_tx.clone(),
        metrics.clone(),
    );

    relay_ssh_session(
        &session,
        &mut ws_sender,
        output_rx,
        terminal_rx,
        ws_to_ssh,
        replayed_history_end,
    )
    .await;

    Ok(())
}

/// Reaps SSH sessions whose client leg has been detached longer than `grace`.
///
/// Walks the registry, collecting candidates whose detach timestamp is older
/// than the grace window, then — immediately before each removal — re-reads the
/// detach timestamp to guard against a TOCTOU race where a client re-attaches
/// (clearing `detached_at`) between the candidate scan and the removal call.
/// Only sessions still expired at the second check are shut down and removed.
/// Returns the number of sessions actually reaped so callers and tests can
/// observe the effect.
pub(in crate::proxy) async fn reap_detached_ssh_sessions(
    state: &Arc<ProxyState>,
    grace: Duration,
) -> usize {
    let now = Instant::now();
    let candidates: Vec<(Uuid, Arc<SshSession>)> = {
        let guard = state.ssh_sessions.read().await;
        guard
            .iter()
            .map(|(id, session)| (*id, session.clone()))
            .collect()
    };

    let mut expired = Vec::new();
    for (connection_id, session) in candidates {
        if let Some(detached_for) = session.detached_for(now).await
            && detached_for >= grace
        {
            expired.push((connection_id, session));
        }
    }

    let mut reaped = 0;
    for (connection_id, session) in expired {
        // Re-check under a fresh timestamp: a client may have called
        // mark_attached() between the candidate scan above and now, clearing
        // detached_at. If that happened, skip removal to avoid killing a
        // freshly re-attached live session.
        if session
            .detached_for(Instant::now())
            .await
            .is_none_or(|d| d < grace)
        {
            continue;
        }
        if super::super::router::remove_connection_state(state.clone(), connection_id).await {
            reaped += 1;
            log_event(
                LogLevel::Info,
                "proxy.ssh.session.reaped",
                EventFields::default()
                    .with_operation("proxy_ssh_detach_reaper")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_status("reaped"),
            );
        }
    }
    reaped
}

/// Returns an authenticated, live ControlMaster path for file sidecars.
/// Sidecars are never allowed to receive `None`: OpenSSH/Scp could then launch
/// a new direct connection instead of multiplexing through the interactive
/// session that authorized the file operation.
pub(super) async fn active_ssh_control_path(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
) -> Result<String, AppError> {
    let session = {
        let guard = state.ssh_sessions.read().await;
        guard.get(&connection_id).cloned()
    }
    .ok_or_else(|| AppError::Conflict("SSH ControlMaster session is not active".to_string()))?;

    if session.terminal_state().is_some() {
        return Err(AppError::Conflict(
            "SSH ControlMaster session has ended".to_string(),
        ));
    }
    if session.ready_enqueue_lifecycle_sealed() {
        return Err(AppError::Conflict(
            "SSH ControlMaster session is ending".to_string(),
        ));
    }
    if !session.is_ready() {
        return Err(AppError::Conflict(
            "SSH ControlMaster session is not ready".to_string(),
        ));
    }

    session
        .control_path()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| {
            AppError::Internal("Ready SSH ControlMaster session has no control path".to_string())
        })
}
