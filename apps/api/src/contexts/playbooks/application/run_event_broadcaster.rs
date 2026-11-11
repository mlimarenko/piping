//! In-process fan-out of live playbook-run frames to connected WebSocket
//! subscribers.
//!
//! The run callbacks (`create_run_event` / `complete_run`) persist a row and
//! then `publish` a [`RunStreamFrame`] to the run's channel; the
//! `ws_playbook_run` handler `subscribe`s to that channel and forwards frames to
//! the browser. A channel is created lazily on first use (publish or subscribe)
//! and kept in the [`DashMap`] for the process lifetime — the working set is one
//! sender per *in-flight* run, which is tiny.
//!
//! Single-instance only: the backend that ingests a run's events is the same
//! one that serves its WebSocket, so a process-local broadcast is sufficient. A
//! multi-instance deployment would replace this with Redis pub/sub behind the
//! same `publish`/`subscribe` API.

use dashmap::DashMap;
use serde::Serialize;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::contexts::playbooks::domain::PlaybookRunEvent;

/// Per-run channel capacity. Sized to absorb the in-flight window between a
/// subscriber's replay query and its first live `recv()` (a fast ansible play
/// can emit a burst of `runner_on_*` events). A slow consumer that overflows
/// this gets `RecvError::Lagged` and resyncs by polling the REST history.
const RUN_STREAM_CHANNEL_CAPACITY: usize = 512;

/// A single live frame forwarded to run-stream subscribers, serialized to JSON
/// as a tagged union: `{"type":"event", ...}` or `{"type":"completion", ...}`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunStreamFrame {
    /// A persisted `playbook_run_event` row, mirrored live.
    Event {
        run_group_id: Uuid,
        run_id: Uuid,
        host: Option<String>,
        event_sequence: i64,
        event_kind: String,
        event_type: String,
        message: Option<String>,
        payload: serde_json::Value,
        created_at: chrono::DateTime<chrono::Utc>,
    },
    /// The run reached a terminal status via the completion callback.
    Completion {
        run_id: Uuid,
        status: String,
        exit_code: Option<i32>,
        error_message: Option<String>,
    },
}

impl RunStreamFrame {
    /// Build an `Event` frame from a freshly persisted run event for one
    /// per-run stream channel. Group-level events have `event.run_id = NULL` in
    /// durable storage, but the WebSocket transport remains keyed by a concrete
    /// subscriber run.
    pub fn event_for_run(event: &PlaybookRunEvent, stream_run_id: Uuid) -> Self {
        Self::Event {
            run_group_id: event.run_group_id,
            run_id: stream_run_id,
            host: event.host.clone(),
            event_sequence: event.event_sequence,
            event_kind: event.event_kind.as_str().to_string(),
            event_type: event.event_type.clone(),
            message: event.message.clone(),
            payload: event.payload.clone(),
            created_at: event.created_at,
        }
    }
}

/// Process-local registry of per-run broadcast channels.
#[derive(Default)]
pub struct RunEventBroadcaster {
    channels: DashMap<Uuid, broadcast::Sender<RunStreamFrame>>,
}

impl RunEventBroadcaster {
    /// Construct an empty broadcaster.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the run's sender, creating the channel on first use.
    fn sender_for(&self, run_id: Uuid) -> broadcast::Sender<RunStreamFrame> {
        self.channels
            .entry(run_id)
            .or_insert_with(|| broadcast::channel(RUN_STREAM_CHANNEL_CAPACITY).0)
            .clone()
    }

    /// Publish a frame to the run's subscribers. Never fails: with no
    /// subscribers `send` returns `Err`, which we deliberately ignore — the
    /// frame is durable in the database and replayed on the next connect.
    pub fn publish(&self, run_id: Uuid, frame: RunStreamFrame) {
        let _ = self.sender_for(run_id).send(frame);
    }

    /// Subscribe to the run's live frames, creating the channel on first use.
    /// Callers should subscribe BEFORE replaying history so no frame published
    /// in the replay window is lost (de-dup by `event_sequence`).
    #[must_use]
    pub fn subscribe(&self, run_id: Uuid) -> broadcast::Receiver<RunStreamFrame> {
        self.sender_for(run_id).subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_reaches_a_prior_subscriber() {
        let broadcaster = RunEventBroadcaster::new();
        let run_id = Uuid::now_v7();
        let mut receiver = broadcaster.subscribe(run_id);

        broadcaster.publish(
            run_id,
            RunStreamFrame::Completion {
                run_id,
                status: "succeeded".to_string(),
                exit_code: Some(0),
                error_message: None,
            },
        );

        let frame = receiver.recv().await.expect("frame delivered");
        match frame {
            RunStreamFrame::Completion {
                run_id: got,
                status,
                exit_code,
                error_message,
            } => {
                assert_eq!(got, run_id);
                assert_eq!(status, "succeeded");
                assert_eq!(exit_code, Some(0));
                assert!(error_message.is_none());
            }
            RunStreamFrame::Event { .. } => panic!("expected a completion frame"),
        }
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_a_noop() {
        let broadcaster = RunEventBroadcaster::new();
        let run_id = Uuid::now_v7();
        // No subscriber: publish must not panic and must not fail the caller.
        broadcaster.publish(
            run_id,
            RunStreamFrame::Completion {
                run_id,
                status: "failed".to_string(),
                exit_code: Some(2),
                error_message: Some("boom".to_string()),
            },
        );
    }

    #[test]
    fn event_frame_serializes_with_tagged_type() {
        let frame = RunStreamFrame::Event {
            run_group_id: Uuid::from_u128(3),
            run_id: Uuid::from_u128(1),
            host: Some("h1".to_string()),
            event_sequence: 7,
            event_kind: "stdout".to_string(),
            event_type: "runner_on_ok".to_string(),
            message: Some("ok: [host]".to_string()),
            payload: serde_json::json!({"host": "h1"}),
            created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch"),
        };
        let value = serde_json::to_value(&frame).expect("serialize event frame");
        assert_eq!(value["type"], "event");
        assert_eq!(value["run_group_id"], Uuid::from_u128(3).to_string());
        assert_eq!(value["host"], "h1");
        assert_eq!(value["event_sequence"], 7);
        assert_eq!(value["event_kind"], "stdout");
        assert_eq!(value["event_type"], "runner_on_ok");
        assert_eq!(value["message"], "ok: [host]");
        assert_eq!(value["payload"]["host"], "h1");
    }

    #[test]
    fn completion_frame_serializes_with_tagged_type() {
        let frame = RunStreamFrame::Completion {
            run_id: Uuid::from_u128(2),
            status: "canceled".to_string(),
            exit_code: None,
            error_message: Some("canceled by operator".to_string()),
        };
        let value = serde_json::to_value(&frame).expect("serialize completion frame");
        assert_eq!(value["type"], "completion");
        assert_eq!(value["status"], "canceled");
        assert!(value["exit_code"].is_null());
        assert_eq!(value["error_message"], "canceled by operator");
    }
}
