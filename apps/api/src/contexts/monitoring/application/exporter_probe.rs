//! Bounded metrics-endpoint probes for exporter discovery.
//!
//! A probe is not a port scan: the caller supplies endpoints that are derived
//! from the run's own inventory targets or configured assignments. This module
//! adds the network policy — SSRF pinning from the monitoring source owner, no
//! redirect following, certificate verification, a bounded timeout and body
//! read — and classifies the outcome into the contract vocabulary.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use serde_json::Value;

use crate::errors::AppError;

use super::super::domain::exporter_discovery::{
    ConnectProof, DiscoveryStatus, EndpointIdentity, EndpointObservation, ObservationProvenance,
    reason,
};
use super::exporter_discovery::probe_observation;
use super::source_resolution::pin_monitoring_query_url;
use crate::contexts::monitoring::infrastructure::providers::metric_query_http_client;

/// Upper bound of the response prefix inspected for the metrics exposition
/// format. The full body is never stored.
pub const DISCOVERY_PROBE_MAX_BODY_BYTES: usize = 64 * 1024;
pub const DISCOVERY_PROBE_MIN_TIMEOUT_MS: u64 = 100;
pub const DISCOVERY_PROBE_MAX_TIMEOUT_MS: u64 = 30_000;
pub const DISCOVERY_PROBE_MAX_CONCURRENCY: usize = 16;

/// One endpoint scheduled for probing.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub endpoint: EndpointIdentity,
    pub exporter_type: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProbeSettings {
    pub timeout_ms: u64,
}

impl ProbeSettings {
    pub fn validated(timeout_ms: u64) -> Result<Self, AppError> {
        if !(DISCOVERY_PROBE_MIN_TIMEOUT_MS..=DISCOVERY_PROBE_MAX_TIMEOUT_MS).contains(&timeout_ms)
        {
            return Err(AppError::Validation(
                "discovery probe timeout is out of bounds".to_string(),
            ));
        }
        Ok(Self { timeout_ms })
    }
}

/// Probe every target with bounded concurrency, preserving input order.
pub async fn probe_targets(
    targets: Vec<ProbeTarget>,
    provenance: &ObservationProvenance,
    observed_at: &str,
    settings: ProbeSettings,
    concurrency: usize,
) -> Vec<EndpointObservation> {
    let concurrency = concurrency.clamp(1, DISCOVERY_PROBE_MAX_CONCURRENCY);
    let mut observations = stream::iter(targets)
        .map(|target| {
            let provenance = provenance.clone();
            let observed_at = observed_at.to_string();
            async move { probe_endpoint(&target, &provenance, &observed_at, settings).await }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;
    observations.sort_by(|left, right| left.endpoint.sort_key().cmp(&right.endpoint.sort_key()));
    observations
}

/// Probe one endpoint and classify the result. Never follows a redirect and
/// never records a response body or URL credentials.
pub async fn probe_endpoint(
    target: &ProbeTarget,
    provenance: &ObservationProvenance,
    observed_at: &str,
    settings: ProbeSettings,
) -> EndpointObservation {
    let started = Instant::now();
    let url = target.endpoint.probe_url();
    // Inventory targets and configured assignments are admin-created objects,
    // so their private addresses are allowed; the cloud-metadata endpoint and
    // the `localhost`/`metadata` names stay blocked inside the shared policy.
    let pinned = match pin_monitoring_query_url(&url, &Value::Null, true).await {
        Ok(pinned) => pinned,
        Err(_) => {
            return observation(
                target,
                provenance,
                observed_at,
                DiscoveryStatus::Skipped,
                reason::OUT_OF_SCOPE,
                ConnectProof {
                    connected: false,
                    http_status: None,
                    content_type: None,
                    body_size: None,
                    redirect: None,
                    tls_verified: None,
                    duration_ms: elapsed_ms(started),
                },
            );
        }
    };
    let client = match metric_query_http_client(Some(&pinned)) {
        Ok(client) => client,
        Err(_) => {
            return observation(
                target,
                provenance,
                observed_at,
                DiscoveryStatus::Error,
                reason::INSPECTION_FAILED,
                ConnectProof {
                    connected: false,
                    http_status: None,
                    content_type: None,
                    body_size: None,
                    redirect: None,
                    tls_verified: None,
                    duration_ms: elapsed_ms(started),
                },
            );
        }
    };

    let timeout = Duration::from_millis(settings.timeout_ms);
    let response = match tokio::time::timeout(timeout, client.get(&url).send()).await {
        Err(_) => {
            return observation(
                target,
                provenance,
                observed_at,
                DiscoveryStatus::Unreachable,
                reason::TIMEOUT,
                connect_failure(started),
            );
        }
        Ok(Err(error)) => {
            let (status, reason_code) = classify_request_error(&error);
            return observation(
                target,
                provenance,
                observed_at,
                status,
                reason_code,
                connect_failure(started),
            );
        }
        Ok(Ok(response)) => response,
    };

    let http_status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(200).collect::<String>());

    if response.status().is_redirection() {
        return observation(
            target,
            provenance,
            observed_at,
            DiscoveryStatus::Unverified,
            reason::REDIRECT_NOT_FOLLOWED,
            ConnectProof {
                connected: true,
                http_status: Some(http_status),
                content_type,
                body_size: None,
                redirect: Some("not_followed".to_string()),
                tls_verified: tls_verified(target),
                duration_ms: elapsed_ms(started),
            },
        );
    }

    let (body_size, body) = match tokio::time::timeout(
        timeout,
        read_bounded_body(response, DISCOVERY_PROBE_MAX_BODY_BYTES),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            return observation(
                target,
                provenance,
                observed_at,
                DiscoveryStatus::Unreachable,
                reason::TIMEOUT,
                connect_failure(started),
            );
        }
    };
    let is_metrics = response_is_metrics(content_type.as_deref(), &body);
    let (status, reason_code) = if (200..300).contains(&http_status) && is_metrics {
        (
            DiscoveryStatus::ConfirmedDiscovered,
            reason::METRICS_RESPONSE,
        )
    } else {
        (DiscoveryStatus::Unverified, reason::NOT_METRICS_RESPONSE)
    };
    observation(
        target,
        provenance,
        observed_at,
        status,
        reason_code,
        ConnectProof {
            connected: true,
            http_status: Some(http_status),
            content_type,
            body_size: Some(body_size),
            redirect: None,
            tls_verified: tls_verified(target),
            duration_ms: elapsed_ms(started),
        },
    )
}

fn observation(
    target: &ProbeTarget,
    provenance: &ObservationProvenance,
    observed_at: &str,
    status: DiscoveryStatus,
    reason_code: &str,
    connect: ConnectProof,
) -> EndpointObservation {
    probe_observation(
        &target.endpoint,
        provenance,
        observed_at,
        status,
        reason_code,
        connect,
        target.exporter_type.clone(),
        target.labels.clone(),
    )
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn connect_failure(started: Instant) -> ConnectProof {
    ConnectProof {
        connected: false,
        http_status: None,
        content_type: None,
        body_size: None,
        redirect: None,
        tls_verified: None,
        duration_ms: elapsed_ms(started),
    }
}

fn tls_verified(target: &ProbeTarget) -> Option<bool> {
    (target.endpoint.scheme == "https").then_some(true)
}

/// Classify a transport error into the contract vocabulary. A TLS handshake
/// failure is `unreachable` with `tls_verification_failed`; a refused connect
/// keeps `connection_refused`; everything else stays a bounded transport error.
fn classify_request_error(error: &reqwest::Error) -> (DiscoveryStatus, &'static str) {
    if error.is_timeout() {
        return (DiscoveryStatus::Unreachable, reason::TIMEOUT);
    }
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("tls") || text.contains("certificate") || text.contains("handshake") {
        return (
            DiscoveryStatus::Unreachable,
            reason::TLS_VERIFICATION_FAILED,
        );
    }
    if error.is_connect() || text.contains("connection refused") || text.contains("connect") {
        return (DiscoveryStatus::Unreachable, reason::CONNECTION_REFUSED);
    }
    (DiscoveryStatus::Unreachable, reason::CONNECT_FAILED)
}

/// Read at most `cap` bytes of the response body. Returns the reported body
/// size (Content-Length when present, otherwise the bytes read) and the prefix.
async fn read_bounded_body(mut response: reqwest::Response, cap: usize) -> (u64, Vec<u8>) {
    let reported = response.content_length();
    let mut body = Vec::with_capacity(cap.min(4096));
    while body.len() < cap {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = cap - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let size = reported.unwrap_or(u64::try_from(body.len()).unwrap_or(u64::MAX));
    (size, body)
}

/// Prometheus text exposition detection: a protocol check on the content type
/// and the exposition preamble, never a natural-language classifier.
#[must_use]
pub fn response_is_metrics(content_type: Option<&str>, body: &[u8]) -> bool {
    if let Some(value) = content_type {
        let value = value.to_ascii_lowercase();
        // A bare `text/plain` also serves health pages; only the Prometheus
        // version parameter or OpenMetrics identifies the exposition format
        // without inspecting the body.
        if value.contains("openmetrics")
            || (value.starts_with("text/plain") && value.contains("version="))
        {
            return true;
        }
    }
    let prefix = String::from_utf8_lossy(&body[..body.len().min(4096)]);
    let prefix = prefix.trim_start();
    prefix.starts_with("# HELP")
        || prefix.starts_with("# TYPE")
        || prefix.lines().any(|line| {
            let line = line.trim_start();
            !line.starts_with('#')
                && !line.is_empty()
                && line
                    .split_whitespace()
                    .next()
                    .is_some_and(|name| name.contains('_'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::monitoring::domain::exporter_discovery::DiscoverySource;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn provenance() -> ObservationProvenance {
        ObservationProvenance {
            run_id: uuid::Uuid::from_u128(10),
            run_group_id: uuid::Uuid::from_u128(11),
            connection_target_id: uuid::Uuid::from_u128(12),
            source_kind: "virtual_machine".to_string(),
            source_id: uuid::Uuid::from_u128(12),
            observed_from: "runner".to_string(),
        }
    }

    fn target(address: &str, port: u16, path: &str) -> ProbeTarget {
        ProbeTarget {
            endpoint: EndpointIdentity::validated(
                uuid::Uuid::from_u128(12),
                address,
                port,
                "http",
                path,
            )
            .unwrap(),
            exporter_type: None,
            labels: BTreeMap::new(),
        }
    }

    async fn serve_once(response: &'static [u8]) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = socket.read(&mut buffer).await;
            let _ = socket.write_all(response).await;
            let _ = socket.shutdown().await;
        });
        port
    }

    #[tokio::test]
    async fn metrics_response_confirms_discovery() {
        let port = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nConnection: close\r\n\r\n# TYPE node_cpu_seconds_total counter\nx 1\n",
        )
        .await;
        let observation = probe_endpoint(
            &target("127.0.0.1", port, "/metrics"),
            &provenance(),
            "2026-11-10T00:00:00Z",
            ProbeSettings::validated(3000).unwrap(),
        )
        .await;
        assert_eq!(observation.source, DiscoverySource::EndpointProbe);
        assert_eq!(observation.status, DiscoveryStatus::ConfirmedDiscovered);
        assert_eq!(observation.reason, reason::METRICS_RESPONSE);
        assert_eq!(
            observation.proof.connect.as_ref().unwrap().http_status,
            Some(200)
        );
    }

    #[tokio::test]
    async fn redirect_is_recorded_and_not_followed() {
        let port = serve_once(
            b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest\r\nConnection: close\r\n\r\n",
        )
        .await;
        let observation = probe_endpoint(
            &target("127.0.0.1", port, "/metrics"),
            &provenance(),
            "2026-11-10T00:00:00Z",
            ProbeSettings::validated(3000).unwrap(),
        )
        .await;
        assert_eq!(observation.status, DiscoveryStatus::Unverified);
        assert_eq!(observation.reason, reason::REDIRECT_NOT_FOLLOWED);
        assert_eq!(
            observation
                .proof
                .connect
                .as_ref()
                .unwrap()
                .redirect
                .as_deref(),
            Some("not_followed")
        );
    }

    #[tokio::test]
    async fn non_metrics_response_stays_unverified() {
        let port = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\nhello",
        )
        .await;
        let observation = probe_endpoint(
            &target("127.0.0.1", port, "/metrics"),
            &provenance(),
            "2026-11-10T00:00:00Z",
            ProbeSettings::validated(3000).unwrap(),
        )
        .await;
        assert_eq!(observation.status, DiscoveryStatus::Unverified);
        assert_eq!(observation.reason, reason::NOT_METRICS_RESPONSE);
    }

    #[tokio::test]
    async fn refused_connection_is_unreachable_not_absent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let observation = probe_endpoint(
            &target("127.0.0.1", port, "/metrics"),
            &provenance(),
            "2026-11-10T00:00:00Z",
            ProbeSettings::validated(500).unwrap(),
        )
        .await;
        assert_eq!(observation.status, DiscoveryStatus::Unreachable);
        assert!(!observation.status.proves_absence());
    }

    #[tokio::test]
    async fn metadata_endpoint_is_out_of_scope() {
        let observation = probe_endpoint(
            &target("169.254.169.254", 80, "/latest"),
            &provenance(),
            "2026-11-10T00:00:00Z",
            ProbeSettings::validated(500).unwrap(),
        )
        .await;
        assert_eq!(observation.status, DiscoveryStatus::Skipped);
        assert_eq!(observation.reason, reason::OUT_OF_SCOPE);
    }

    #[test]
    fn exposition_detection_uses_protocol_shape_only() {
        assert!(response_is_metrics(
            Some("text/plain; version=0.0.4"),
            b"whatever"
        ));
        assert!(response_is_metrics(None, b"# HELP x help\nx 1\n"));
        assert!(response_is_metrics(None, b"node_cpu_seconds_total 1\n"));
        assert!(!response_is_metrics(Some("text/plain"), b"ok"));
        assert!(response_is_metrics(Some("text/plain; version=0.0.4"), b""));
        assert!(!response_is_metrics(Some("text/html"), b"<html></html>"));
        assert!(!response_is_metrics(None, b"just some words\n"));
    }
}
