use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

const RELAY_HEALTH_ENTRY_TTL_MS: i64 = 6 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RelayHealthState {
    Connecting,
    Ok,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone)]
pub struct RelayLegHealthSnapshot {
    pub state: RelayHealthState,
    pub updated_at: DateTime<Utc>,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub rx_bps: Option<i64>,
    pub tx_bps: Option<i64>,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone)]
struct RelayLegHealthState {
    state: RelayHealthState,
    updated_at: DateTime<Utc>,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_bps: Option<u64>,
    tx_bps: Option<u64>,
    last_rate_at_ms: Option<i64>,
    last_rate_rx_bytes: u64,
    last_rate_tx_bytes: u64,
    reason_code: Option<String>,
}

impl Default for RelayLegHealthState {
    fn default() -> Self {
        Self {
            state: RelayHealthState::Connecting,
            updated_at: Utc::now(),
            rx_bytes: 0,
            tx_bytes: 0,
            rx_bps: None,
            tx_bps: None,
            last_rate_at_ms: None,
            last_rate_rx_bytes: 0,
            last_rate_tx_bytes: 0,
            reason_code: None,
        }
    }
}

impl RelayLegHealthState {
    fn set_state(&mut self, state: RelayHealthState, reason_code: Option<String>) {
        self.state = state;
        self.updated_at = Utc::now();
        self.reason_code = reason_code;
        if matches!(
            state,
            RelayHealthState::Disconnected | RelayHealthState::Failed
        ) {
            self.rx_bps = None;
            self.tx_bps = None;
            self.last_rate_at_ms = None;
            self.last_rate_rx_bytes = self.rx_bytes;
            self.last_rate_tx_bytes = self.tx_bytes;
        }
    }

    fn record_traffic(&mut self, direction: RelayTrafficDirection, bytes: usize) {
        if bytes == 0 {
            return;
        }
        match direction {
            RelayTrafficDirection::ClientToBackend => {
                self.rx_bytes = self.rx_bytes.saturating_add(bytes as u64);
            }
            RelayTrafficDirection::BackendToClient => {
                self.tx_bytes = self.tx_bytes.saturating_add(bytes as u64);
            }
        }
        if !matches!(self.state, RelayHealthState::Failed) {
            self.state = RelayHealthState::Ok;
            self.reason_code = None;
        }
        self.updated_at = Utc::now();
        self.update_rates(self.updated_at.timestamp_millis());
    }

    fn update_rates(&mut self, now_ms: i64) {
        let Some(last_ms) = self.last_rate_at_ms else {
            self.last_rate_at_ms = Some(now_ms);
            self.last_rate_rx_bytes = self.rx_bytes;
            self.last_rate_tx_bytes = self.tx_bytes;
            return;
        };
        let elapsed_ms = now_ms - last_ms;
        if elapsed_ms < crate::config::PROXY_METRICS_RATE_MIN_INTERVAL_MS || elapsed_ms <= 0 {
            return;
        }

        let elapsed_seconds = elapsed_ms as f64 / 1_000.0;
        let rx_delta = self.rx_bytes.saturating_sub(self.last_rate_rx_bytes);
        let tx_delta = self.tx_bytes.saturating_sub(self.last_rate_tx_bytes);

        self.rx_bps = Some((rx_delta as f64 / elapsed_seconds).round() as u64);
        self.tx_bps = Some((tx_delta as f64 / elapsed_seconds).round() as u64);
        self.last_rate_at_ms = Some(now_ms);
        self.last_rate_rx_bytes = self.rx_bytes;
        self.last_rate_tx_bytes = self.tx_bytes;
    }

    fn snapshot(&self) -> RelayLegHealthSnapshot {
        RelayLegHealthSnapshot {
            state: self.state,
            updated_at: self.updated_at,
            rx_bytes: to_i64_saturated(self.rx_bytes),
            tx_bytes: to_i64_saturated(self.tx_bytes),
            rx_bps: self.rx_bps.map(to_i64_saturated),
            tx_bps: self.tx_bps.map(to_i64_saturated),
            reason_code: self.reason_code.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RelayTrafficDirection {
    ClientToBackend,
    BackendToClient,
}

#[derive(Debug, Default)]
pub struct SessionRelayHealthRegistry {
    sessions: RwLock<HashMap<Uuid, RelayLegHealthState>>,
}

impl SessionRelayHealthRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn mark_connecting(&self, session_id: Uuid) {
        let mut sessions = self.sessions.write().await;
        prune_expired_locked(&mut sessions, Utc::now().timestamp_millis());
        sessions
            .entry(session_id)
            .or_default()
            .set_state(RelayHealthState::Connecting, None);
    }

    pub async fn mark_open(&self, session_id: Uuid) {
        let mut sessions = self.sessions.write().await;
        prune_expired_locked(&mut sessions, Utc::now().timestamp_millis());
        sessions
            .entry(session_id)
            .or_default()
            .set_state(RelayHealthState::Ok, None);
    }

    pub async fn record_client_to_backend(&self, session_id: Uuid, bytes: usize) {
        self.record_traffic(session_id, RelayTrafficDirection::ClientToBackend, bytes)
            .await;
    }

    pub async fn record_backend_to_client(&self, session_id: Uuid, bytes: usize) {
        self.record_traffic(session_id, RelayTrafficDirection::BackendToClient, bytes)
            .await;
    }

    pub async fn mark_disconnected(&self, session_id: Uuid, reason_code: &str) {
        let mut sessions = self.sessions.write().await;
        prune_expired_locked(&mut sessions, Utc::now().timestamp_millis());
        sessions.entry(session_id).or_default().set_state(
            RelayHealthState::Disconnected,
            Some(reason_code.to_string()),
        );
    }

    pub async fn mark_failed(&self, session_id: Uuid, reason_code: &str) {
        let mut sessions = self.sessions.write().await;
        prune_expired_locked(&mut sessions, Utc::now().timestamp_millis());
        sessions
            .entry(session_id)
            .or_default()
            .set_state(RelayHealthState::Failed, Some(reason_code.to_string()));
    }

    pub async fn snapshot(&self, session_id: Uuid) -> Option<RelayLegHealthSnapshot> {
        let mut sessions = self.sessions.write().await;
        prune_expired_locked(&mut sessions, Utc::now().timestamp_millis());
        sessions.get(&session_id).map(RelayLegHealthState::snapshot)
    }

    pub async fn remove(&self, session_id: Uuid) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(&session_id);
    }

    async fn record_traffic(
        &self,
        session_id: Uuid,
        direction: RelayTrafficDirection,
        bytes: usize,
    ) {
        if bytes == 0 {
            return;
        }
        let mut sessions = self.sessions.write().await;
        prune_expired_locked(&mut sessions, Utc::now().timestamp_millis());
        sessions
            .entry(session_id)
            .or_default()
            .record_traffic(direction, bytes);
    }
}

pub type SharedSessionRelayHealthRegistry = Arc<SessionRelayHealthRegistry>;

fn to_i64_saturated(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

fn prune_expired_locked(sessions: &mut HashMap<Uuid, RelayLegHealthState>, now_ms: i64) {
    sessions.retain(|_, state| {
        now_ms.saturating_sub(state.updated_at.timestamp_millis()) <= RELAY_HEALTH_ENTRY_TTL_MS
    });
}

#[cfg(test)]
mod tests {
    use super::{
        RELAY_HEALTH_ENTRY_TTL_MS, RelayHealthState, RelayLegHealthState,
        SessionRelayHealthRegistry,
    };
    use chrono::{Duration, Utc};

    #[tokio::test]
    async fn relay_registry_tracks_client_backend_bytes() {
        let registry = SessionRelayHealthRegistry::new();
        let session_id = uuid::Uuid::now_v7();

        registry.mark_connecting(session_id).await;
        registry.record_client_to_backend(session_id, 7).await;
        registry.record_backend_to_client(session_id, 11).await;

        let snapshot = registry.snapshot(session_id).await.unwrap();
        assert_eq!(snapshot.state, RelayHealthState::Ok);
        assert_eq!(snapshot.rx_bytes, 7);
        assert_eq!(snapshot.tx_bytes, 11);
        assert!(snapshot.reason_code.is_none());
    }

    #[tokio::test]
    async fn relay_registry_disconnect_clears_rates_and_reason() {
        let registry = SessionRelayHealthRegistry::new();
        let session_id = uuid::Uuid::now_v7();

        registry.mark_open(session_id).await;
        registry
            .mark_disconnected(session_id, "client_closed")
            .await;

        let snapshot = registry.snapshot(session_id).await.unwrap();
        assert_eq!(snapshot.state, RelayHealthState::Disconnected);
        assert_eq!(snapshot.reason_code.as_deref(), Some("client_closed"));
        assert!(snapshot.rx_bps.is_none());
        assert!(snapshot.tx_bps.is_none());
    }

    #[tokio::test]
    async fn relay_registry_prunes_stale_entries() {
        let registry = SessionRelayHealthRegistry::new();
        let stale_session_id = uuid::Uuid::now_v7();
        let fresh_session_id = uuid::Uuid::now_v7();

        {
            let mut sessions = registry.sessions.write().await;
            sessions.insert(
                stale_session_id,
                RelayLegHealthState {
                    updated_at: Utc::now()
                        - Duration::milliseconds(RELAY_HEALTH_ENTRY_TTL_MS + 1_000),
                    ..RelayLegHealthState::default()
                },
            );
            sessions.insert(fresh_session_id, RelayLegHealthState::default());
        }

        registry.mark_open(fresh_session_id).await;

        assert!(registry.snapshot(stale_session_id).await.is_none());
        assert!(registry.snapshot(fresh_session_id).await.is_some());
    }

    #[tokio::test]
    async fn relay_registry_removes_session_on_request() {
        let registry = SessionRelayHealthRegistry::new();
        let session_id = uuid::Uuid::now_v7();

        registry.mark_open(session_id).await;
        registry.remove(session_id).await;

        assert!(registry.snapshot(session_id).await.is_none());
    }
}
