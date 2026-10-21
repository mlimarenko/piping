//! Resolved provider endpoint vocabulary: the connection descriptor produced
//! by source resolution and consumed by every provider HTTP client.

use std::net::SocketAddr;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LatestMetricSourceInfo {
    pub name: Option<String>,
    pub source_type: Option<String>,
    pub configured_from: String,
    pub query_url: String,
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedMetricQuerySource {
    pub source_type: String,
    pub query_url: String,
    pub token: Option<String>,
    pub info: LatestMetricSourceInfo,
    pub timeout_ms: u64,
    pub options: Value,
    pub pinned_resolution: Option<PinnedMonitoringResolution>,
}

#[derive(Debug, Clone)]
pub struct PinnedMonitoringResolution {
    pub host: String,
    pub addrs: Vec<SocketAddr>,
}
