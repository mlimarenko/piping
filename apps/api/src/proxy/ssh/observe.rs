use super::*;
use axum::extract::ws::{Message, WebSocket};
use std::num::NonZeroU16;

#[derive(Clone)]
struct Frame {
    cols: u16,
    rows: u16,
    bytes: Vec<u8>,
}

fn observer_application_input(message: &Message) -> bool {
    matches!(message, Message::Text(_) | Message::Binary(_))
}

pub(super) struct Observation {
    parser: vt100::Parser,
    frames: watch::Sender<Frame>,
}

impl Observation {
    pub(super) fn new() -> Self {
        let parser = vt100::Parser::default();
        let (frames, _) = watch::channel(Self::frame(&parser));
        Self { parser, frames }
    }

    fn frame(parser: &vt100::Parser) -> Frame {
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        // Rebuild both grids from parsed state. In particular, do not forward
        // raw suffixes: the parser may hold an incomplete escape or UTF-8
        // sequence at the instant a new observer joins.
        let mut bytes = b"\x1bc".to_vec();
        if screen.alternate_screen() {
            let mut scratch = vt100::Parser::new(rows, cols, 0);
            *scratch.screen_mut() = screen.clone();
            scratch.process(b"\x1b[?1049l");
            bytes.extend_from_slice(scratch.screen().state_formatted().as_bytes());
            bytes.extend_from_slice(b"\x1b[?1049h");
        }
        bytes.extend_from_slice(screen.state_formatted().as_bytes());
        Frame {
            cols: cols.get(),
            rows: rows.get(),
            bytes,
        }
    }

    pub(super) fn output(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.frames.send_replace(Self::frame(&self.parser));
    }

    pub(super) fn resize(&mut self, cols: u16, rows: u16) {
        if let (Some(cols), Some(rows)) = (NonZeroU16::new(cols), NonZeroU16::new(rows)) {
            self.parser.set_size(rows, cols);
            self.frames.send_replace(Self::frame(&self.parser));
        }
    }
}

/// Runtime facts only. Absence is unknown, not evidence of termination.
pub(in crate::proxy) async fn runtime_presence(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
) -> Option<crate::integrations::proxy::ProxyRuntimePresence> {
    let generation = state
        .connections
        .read()
        .await
        .get(&connection_id)?
        .generation;
    let session = observer_session(state, connection_id, generation).await?;
    let detached_at = *session.detached_at.lock().await;
    let terminal = session.terminal_state();
    let status = if terminal.is_some() {
        "ended"
    } else if session.process_exit_sealed() {
        // The exit seal precedes final classification and terminal timestamp.
        "unknown"
    } else if !session.is_ready() {
        "connecting"
    } else if detached_at.is_some() {
        "detached"
    } else {
        "live"
    };
    let attached = session.attach_lock.try_lock().is_err();
    let last_activity = *session
        .last_operator_activity_at
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let reconnect_deadline = if status == "detached" && state.ssh_detach_grace_seconds != 0 {
        detached_at
            .and_then(|detached| {
                let elapsed =
                    chrono::Duration::from_std(Instant::now().saturating_duration_since(detached))
                        .ok()?;
                let grace =
                    chrono::Duration::from_std(Duration::from_secs(state.ssh_detach_grace_seconds))
                        .ok()?;
                Utc::now()
                    .checked_sub_signed(elapsed)?
                    .checked_add_signed(grace)
            })
            .map(|deadline| deadline.to_rfc3339())
    } else {
        None
    };
    Some(crate::integrations::proxy::ProxyRuntimePresence {
        state: status.to_string(),
        operator_attached: Some(attached),
        last_operator_activity_at: last_activity.map(|time| time.to_rfc3339()),
        reconnect_deadline,
        ended_at: terminal
            .and_then(|value| value.ended_at)
            .map(|time| time.to_rfc3339()),
        ..Default::default()
    })
}

pub(super) async fn observer_session(
    state: &ProxyState,
    connection_id: Uuid,
    expected_generation: Uuid,
) -> Option<Arc<SshSession>> {
    let connections = state.connections.read().await;
    let entry = connections.get(&connection_id)?;
    if entry.generation != expected_generation || *entry.ended_tx.borrow() {
        return None;
    }
    let session = state
        .ssh_sessions
        .read()
        .await
        .get(&connection_id)
        .cloned()?;
    let generation = *session.generation.lock().unwrap_or_else(|e| e.into_inner());
    (generation == Some(expected_generation)).then_some(session)
}

pub(in crate::proxy) async fn proxy_ws_ssh_observe_loop(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    mut socket: WebSocket,
    expected_generation: Uuid,
    deadline: Instant,
) -> Result<(), AppError> {
    use super::super::observation::send_observer_lease_close;

    let resolved = tokio::time::timeout_at(deadline, async {
        let session = observer_session(&state, connection_id, expected_generation).await?;
        if !session.is_ready()
            || session.process_exit_sealed()
            || session.terminal_state().is_some()
        {
            return None;
        }
        let frames = session.observation.lock().await.frames.subscribe();
        Some((session, frames))
    })
    .await;
    let (session, mut frames) = match resolved {
        Ok(Some(resolved)) => resolved,
        Ok(None) => {
            super::super::send_close_frame(
                &mut socket,
                WS_CLOSE_TARGET_UNAVAILABLE,
                "SSH session unavailable",
            )
            .await;
            return Ok(());
        }
        Err(_) => {
            send_observer_lease_close(&mut socket).await;
            return Ok(());
        }
    };
    let mut terminal = session.terminal_receiver();
    let mut exit = session.process_exit_receiver();
    let (mut sender, mut receiver) = socket.split();
    let mut current = Some(frames.borrow_and_update().clone());
    let relay = async {
        loop {
            if session.process_exit_sealed() || session.terminal_state().is_some() {
                break;
            }
            if let Some(frame) = current.take() {
                let size = format!(
                    "{WS_CONTROL_PREFIX}{{\"type\":\"observer_resize\",\"cols\":{},\"rows\":{}}}",
                    frame.cols, frame.rows
                );
                if sender.send(Message::Text(size.into())).await.is_err()
                    || sender
                        .send(Message::Binary(frame.bytes.into()))
                        .await
                        .is_err()
                {
                    return Ok(());
                }
            }
            tokio::select! {
                changed = frames.changed() => {
                    if changed.is_err() { break; }
                    current = Some(frames.borrow_and_update().clone());
                }
                _ = terminal.changed() => break,
                _ = exit.changed() => break,
                message = receiver.next() => match message {
                    Some(Ok(Message::Ping(bytes))) => { if sender.send(Message::Pong(bytes)).await.is_err() { return Ok(()); } }
                    Some(Ok(Message::Pong(_))) => {},
                    Some(Ok(message)) if observer_application_input(&message) => {
                        let _ = sender.send(Message::Close(Some(axum::extract::ws::CloseFrame { code: WS_CLOSE_INVALID_PAYLOAD, reason: "Observer input forbidden".into() }))).await;
                        return Ok(());
                    }
                    _ => return Ok(()),
                }
            }
        }
        let _ = sender.send(Message::Close(None)).await;
        Ok(())
    };
    match tokio::time::timeout_at(deadline, relay).await {
        Ok(result) => result,
        Err(_) => {
            send_observer_lease_close(&mut sender).await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_restores_primary_after_alternate_exit_and_split_sequences() {
        let mut observation = Observation::new();
        observation.output(b"primary\x1b[?1049h\x1b[?25lalt\x1b[");
        let before = observation.frames.borrow().clone();
        let mut viewer = vt100::Parser::default();
        viewer.process(&before.bytes);
        assert!(viewer.screen().alternate_screen());
        assert!(viewer.screen().hide_cursor());
        assert_eq!(
            viewer.screen().contents(),
            observation.parser.screen().contents()
        );
        observation.output(b"2Jafter");
        let after = observation.frames.borrow().clone();
        viewer.process(&after.bytes);
        assert_eq!(
            viewer.screen().contents(),
            observation.parser.screen().contents()
        );
        viewer.process(b"\x1b[?1049l");
        assert!(viewer.screen().contents().contains("primary"));
    }

    #[test]
    fn observer_rejects_all_application_frames() {
        for payload in [
            "a",
            "__piping_ws__:{\"type\":\"resize\"}",
            "\u{1b}]52;c;YQ==\u{7}",
            "",
        ] {
            assert!(observer_application_input(&Message::Text(payload.into())));
        }
        assert!(observer_application_input(&Message::Binary(
            vec![0, 27, 91].into()
        )));
        assert!(observer_application_input(&Message::Binary(
            Vec::new().into()
        )));
        assert!(!observer_application_input(&Message::Ping(
            Vec::new().into()
        )));
        assert!(!observer_application_input(&Message::Pong(
            Vec::new().into()
        )));
        assert!(!observer_application_input(&Message::Close(None)));
    }

    #[test]
    fn operator_resize_rejects_unbounded_terminal_grids() {
        for (cols, rows) in [(0, 24), (80, 0), (501, 24), (80, 301)] {
            let control = WsControlMessage {
                control_type: "resize".into(),
                cols: Some(cols),
                rows: Some(rows),
            };
            assert_eq!(control.resize_dimensions(), None);
        }
    }

    #[test]
    fn independent_observers_receive_latest_complete_state_after_resize() {
        let mut observation = Observation::new();
        let mut first = observation.frames.subscribe();
        observation.output(b"first");
        let mut second = observation.frames.subscribe();
        observation.resize(100, 40);
        observation.output(b" second");
        for receiver in [&mut first, &mut second] {
            let frame = receiver.borrow_and_update().clone();
            assert_eq!((frame.cols, frame.rows), (100, 40));
            let mut viewer = vt100::Parser::new(
                NonZeroU16::new(frame.rows).unwrap(),
                NonZeroU16::new(frame.cols).unwrap(),
                0,
            );
            viewer.process(&frame.bytes);
            assert_eq!(
                viewer.screen().contents(),
                observation.parser.screen().contents()
            );
        }
        let reconnect = observation.frames.subscribe();
        assert_eq!(reconnect.borrow().bytes, observation.frames.borrow().bytes);
    }
}
