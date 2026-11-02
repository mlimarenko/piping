use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{ProxyLinkMetricsResponse, ProxyMetricsResponse};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum LinkType {
    ClientProxy,
    ProxyProtocol,
}

impl LinkType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ClientProxy => "client_proxy",
            Self::ProxyProtocol => "proxy_protocol",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TrafficDirection {
    Rx,
    Tx,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ConnectionOverallStatus {
    Connecting,
    Connected,
    Disconnected,
    Error,
}

impl ConnectionOverallStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum LinkStatus {
    Connecting,
    Healthy,
    Degraded,
    Unhealthy,
    Disconnected,
}

impl LinkStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
            Self::Disconnected => "disconnected",
        }
    }
}

#[derive(Debug, Clone)]
struct LinkMetricsState {
    status: LinkStatus,
    latency_ms: Option<i64>,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_bps: Option<u64>,
    tx_bps: Option<u64>,
    quality_label: Option<String>,
    quality_score: Option<i32>,
    last_rate_at_ms: Option<i64>,
    last_rate_rx_bytes: u64,
    last_rate_tx_bytes: u64,
}

impl Default for LinkMetricsState {
    fn default() -> Self {
        Self {
            status: LinkStatus::Disconnected,
            latency_ms: None,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_bps: None,
            tx_bps: None,
            quality_label: None,
            quality_score: None,
            last_rate_at_ms: None,
            last_rate_rx_bytes: 0,
            last_rate_tx_bytes: 0,
        }
    }
}

impl LinkMetricsState {
    fn set_status(&mut self, status: LinkStatus) {
        self.status = status;
        if status == LinkStatus::Disconnected {
            self.rx_bps = None;
            self.tx_bps = None;
            self.last_rate_at_ms = None;
            self.last_rate_rx_bytes = self.rx_bytes;
            self.last_rate_tx_bytes = self.tx_bytes;
        }
    }

    fn mark_connecting(&mut self) {
        if self.status == LinkStatus::Disconnected {
            self.set_status(LinkStatus::Connecting);
        }
    }

    fn set_latency(&mut self, latency_ms: Option<i64>) {
        self.latency_ms = latency_ms;
        let (label, score) = derive_link_quality(latency_ms);
        self.quality_label = label;
        self.quality_score = score;
    }

    fn record_traffic(&mut self, direction: TrafficDirection, bytes: usize, now_ms: i64) {
        if bytes == 0 {
            return;
        }

        if self.status == LinkStatus::Connecting {
            self.set_status(LinkStatus::Healthy);
        }

        match direction {
            TrafficDirection::Rx => {
                self.rx_bytes = self.rx_bytes.saturating_add(bytes as u64);
            }
            TrafficDirection::Tx => {
                self.tx_bytes = self.tx_bytes.saturating_add(bytes as u64);
            }
        }
        self.update_rates(now_ms);
    }

    fn update_rates(&mut self, now_ms: i64) {
        let Some(last_ms) = self.last_rate_at_ms else {
            self.last_rate_at_ms = Some(now_ms);
            self.last_rate_rx_bytes = self.rx_bytes;
            self.last_rate_tx_bytes = self.tx_bytes;
            return;
        };

        let elapsed_ms = now_ms - last_ms;
        if elapsed_ms < crate::config::PROXY_METRICS_RATE_MIN_INTERVAL_MS {
            return;
        }
        if elapsed_ms <= 0 {
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

    fn as_response(&self, link_type: LinkType) -> ProxyLinkMetricsResponse {
        ProxyLinkMetricsResponse {
            link_type: link_type.as_str().to_string(),
            status: self.status.as_str().to_string(),
            latency_ms: self.latency_ms,
            rx_bytes: Some(to_i64_saturated(self.rx_bytes)),
            tx_bytes: Some(to_i64_saturated(self.tx_bytes)),
            rx_bps: self.rx_bps.map(to_i64_saturated),
            tx_bps: self.tx_bps.map(to_i64_saturated),
            quality_label: self.quality_label.clone(),
            quality_score: self.quality_score,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectionMetricsState {
    overall_status: ConnectionOverallStatus,
    updated_at: DateTime<Utc>,
    client_proxy: LinkMetricsState,
    proxy_protocol: LinkMetricsState,
    /// Host key lines this connection observed that were not part of the seed.
    /// Deduplicated and sorted so a poller persists a stable set regardless of
    /// how many TOFU observations the relay recorded.
    observed_host_keys: Vec<String>,
}

impl Default for ConnectionMetricsState {
    fn default() -> Self {
        Self {
            overall_status: ConnectionOverallStatus::Disconnected,
            updated_at: Utc::now(),
            client_proxy: LinkMetricsState::default(),
            proxy_protocol: LinkMetricsState::default(),
            observed_host_keys: Vec::new(),
        }
    }
}

impl ConnectionMetricsState {
    pub(crate) fn mark_connecting(&mut self) {
        self.client_proxy.mark_connecting();
        self.proxy_protocol.mark_connecting();
        self.recompute_overall_status();
        self.updated_at = Utc::now();
    }

    pub(crate) fn mark_connected(&mut self, target_latency_ms: Option<i64>) {
        self.client_proxy.set_status(LinkStatus::Healthy);
        self.proxy_protocol.set_status(LinkStatus::Healthy);
        self.proxy_protocol.set_latency(target_latency_ms);
        self.recompute_overall_status();
        self.updated_at = Utc::now();
    }

    pub(crate) fn mark_proxy_unhealthy(&mut self, latency_ms: Option<i64>) {
        self.client_proxy.set_status(LinkStatus::Degraded);
        self.proxy_protocol.set_status(LinkStatus::Unhealthy);
        self.proxy_protocol.set_latency(latency_ms);
        self.recompute_overall_status();
        self.updated_at = Utc::now();
    }

    pub(crate) fn mark_error(&mut self) {
        self.client_proxy.set_status(LinkStatus::Unhealthy);
        self.proxy_protocol.set_status(LinkStatus::Unhealthy);
        self.recompute_overall_status();
        self.updated_at = Utc::now();
    }

    pub(crate) fn mark_disconnected(&mut self) {
        self.client_proxy.set_status(LinkStatus::Disconnected);
        self.proxy_protocol.set_status(LinkStatus::Disconnected);
        self.recompute_overall_status();
        self.updated_at = Utc::now();
    }

    pub(crate) fn record_traffic(
        &mut self,
        link_type: LinkType,
        direction: TrafficDirection,
        bytes: usize,
    ) {
        let now_ms = Utc::now().timestamp_millis();
        match link_type {
            LinkType::ClientProxy => self.client_proxy.record_traffic(direction, bytes, now_ms),
            LinkType::ProxyProtocol => self.proxy_protocol.record_traffic(direction, bytes, now_ms),
        }
        self.recompute_overall_status();
        self.updated_at = Utc::now();
    }

    /// Merge newly observed host key lines into the connection's set. The set
    /// is deduplicated and sorted so repeated observations and differing
    /// arrival orders produce one stable response.
    pub(crate) fn record_observed_host_keys(&mut self, lines: &[String]) {
        for line in lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            self.observed_host_keys.push(trimmed.to_string());
        }
        self.observed_host_keys.sort();
        self.observed_host_keys.dedup();
        self.updated_at = Utc::now();
    }

    pub(crate) fn as_response(&self, connection_id: Uuid) -> ProxyMetricsResponse {
        ProxyMetricsResponse {
            connection_id: connection_id.to_string(),
            updated_at: self.updated_at.to_rfc3339(),
            overall_status: self.overall_status.as_str().to_string(),
            links: vec![
                self.client_proxy.as_response(LinkType::ClientProxy),
                self.proxy_protocol.as_response(LinkType::ProxyProtocol),
            ],
            observed_host_keys: self.observed_host_keys.clone(),
        }
    }

    fn recompute_overall_status(&mut self) {
        let statuses = [self.client_proxy.status, self.proxy_protocol.status];
        self.overall_status = if statuses
            .iter()
            .all(|status| *status == LinkStatus::Disconnected)
        {
            ConnectionOverallStatus::Disconnected
        } else if statuses.contains(&LinkStatus::Unhealthy) {
            ConnectionOverallStatus::Error
        } else if statuses.contains(&LinkStatus::Connecting)
            || statuses.contains(&LinkStatus::Degraded)
        {
            ConnectionOverallStatus::Connecting
        } else {
            ConnectionOverallStatus::Connected
        };
    }
}

pub(crate) fn derive_link_quality(latency_ms: Option<i64>) -> (Option<String>, Option<i32>) {
    let Some(latency) = latency_ms else {
        return (None, None);
    };
    if latency <= 0 {
        return (Some("good".to_string()), Some(100));
    }
    if latency <= crate::config::PROXY_QUALITY_GOOD_LATENCY_MS {
        return (Some("good".to_string()), Some(100));
    }
    if latency <= crate::config::PROXY_QUALITY_WARNING_LATENCY_MS {
        return (Some("warning".to_string()), Some(75));
    }
    (Some("poor".to_string()), Some(40))
}

fn to_i64_saturated(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

pub(crate) async fn mark_connection_metrics_connecting(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) {
    let mut guard = metrics.write().await;
    guard.mark_connecting();
}

pub(crate) async fn mark_connection_metrics_connected(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    target_latency_ms: Option<i64>,
) {
    let mut guard = metrics.write().await;
    guard.mark_connected(target_latency_ms);
}

pub(crate) async fn mark_connection_metrics_proxy_unhealthy(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    target_latency_ms: Option<i64>,
) {
    let mut guard = metrics.write().await;
    guard.mark_proxy_unhealthy(target_latency_ms);
}

pub(crate) async fn mark_connection_metrics_error(metrics: &Arc<RwLock<ConnectionMetricsState>>) {
    let mut guard = metrics.write().await;
    guard.mark_error();
}

pub(crate) async fn mark_connection_metrics_disconnected(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) {
    let mut guard = metrics.write().await;
    guard.mark_disconnected();
}

pub(crate) async fn record_connection_traffic(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    link_type: LinkType,
    direction: TrafficDirection,
    bytes: usize,
) {
    if bytes == 0 {
        return;
    }
    let mut guard = metrics.write().await;
    guard.record_traffic(link_type, direction, bytes);
}

/// Record host key lines the proxy learned by trust-on-first-use. Empty input
/// is a no-op so a connection with no first-contact key stays untouched.
pub(crate) async fn record_connection_observed_host_keys(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    lines: &[String],
) {
    if lines.is_empty() {
        return;
    }
    let mut guard = metrics.write().await;
    guard.record_observed_host_keys(lines);
}

#[cfg(test)]
mod tests {
    use super::{ConnectionMetricsState, LinkType, TrafficDirection};
    use uuid::Uuid;

    fn link_status(response: &super::ProxyMetricsResponse, link_type: &str) -> String {
        response
            .links
            .iter()
            .find(|link| link.link_type == link_type)
            .expect("link exists")
            .status
            .clone()
    }

    #[test]
    fn mark_connecting_sets_disconnected_links_to_connecting() {
        let mut state = ConnectionMetricsState::default();

        state.mark_connecting();

        let response = state.as_response(Uuid::now_v7());
        assert_eq!(link_status(&response, "client_proxy"), "connecting");
        assert_eq!(link_status(&response, "proxy_protocol"), "connecting");
        assert_eq!(response.overall_status, "connecting");
    }

    #[test]
    fn mark_connecting_does_not_downgrade_connected_links_on_reattach() {
        let mut state = ConnectionMetricsState::default();
        state.mark_connecting();
        state.mark_connected(Some(42));

        state.mark_connecting();

        let response = state.as_response(Uuid::now_v7());
        assert_eq!(link_status(&response, "client_proxy"), "healthy");
        assert_eq!(link_status(&response, "proxy_protocol"), "healthy");
        assert_eq!(response.overall_status, "connected");
        assert_eq!(response.links[1].latency_ms, Some(42));
    }

    #[test]
    fn mark_connecting_does_not_mask_degraded_or_unhealthy_links() {
        let mut state = ConnectionMetricsState::default();
        state.mark_proxy_unhealthy(Some(512));

        state.mark_connecting();

        let response = state.as_response(Uuid::now_v7());
        assert_eq!(link_status(&response, "client_proxy"), "degraded");
        assert_eq!(link_status(&response, "proxy_protocol"), "unhealthy");
        assert_eq!(response.overall_status, "error");
    }

    #[test]
    fn traffic_promotes_connecting_link_to_healthy() {
        let mut state = ConnectionMetricsState::default();
        state.mark_connecting();

        state.record_traffic(LinkType::ProxyProtocol, TrafficDirection::Rx, 64);

        let response = state.as_response(Uuid::now_v7());
        assert_eq!(link_status(&response, "client_proxy"), "connecting");
        assert_eq!(link_status(&response, "proxy_protocol"), "healthy");
        assert_eq!(response.overall_status, "connecting");
        assert_eq!(response.links[1].rx_bytes, Some(64));
    }

    #[test]
    fn traffic_on_both_links_completes_connecting_metrics() {
        let mut state = ConnectionMetricsState::default();
        state.mark_connecting();

        state.record_traffic(LinkType::ClientProxy, TrafficDirection::Tx, 32);
        state.record_traffic(LinkType::ProxyProtocol, TrafficDirection::Rx, 64);

        let response = state.as_response(Uuid::now_v7());
        assert_eq!(link_status(&response, "client_proxy"), "healthy");
        assert_eq!(link_status(&response, "proxy_protocol"), "healthy");
        assert_eq!(response.overall_status, "connected");
    }

    #[test]
    fn observed_host_keys_merge_and_report() {
        let mut state = ConnectionMetricsState::default();
        state.record_observed_host_keys(&[
            "b.example ssh-ed25519 BBBB".to_string(),
            "a.example ssh-ed25519 AAAA".to_string(),
        ]);
        state.record_observed_host_keys(&[
            "a.example ssh-ed25519 AAAA".to_string(),
            "c.example ssh-ed25519 CCCC".to_string(),
        ]);

        let response = state.as_response(Uuid::now_v7());
        assert_eq!(
            response.observed_host_keys,
            vec![
                "a.example ssh-ed25519 AAAA".to_string(),
                "b.example ssh-ed25519 BBBB".to_string(),
                "c.example ssh-ed25519 CCCC".to_string(),
            ],
            "observations must merge, deduplicate and sort"
        );

        state.record_observed_host_keys(&[]);
        assert_eq!(
            state.as_response(Uuid::now_v7()).observed_host_keys,
            response.observed_host_keys,
            "an empty merge must not change the reported set"
        );
    }
}
