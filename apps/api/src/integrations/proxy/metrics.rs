use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyMetricsResponse {
    pub connection_id: String,
    pub updated_at: String,
    pub overall_status: String,
    pub links: Vec<ProxyLinkMetricsResponse>,
    /// `known_hosts` lines the proxy learned by trust-on-first-use and that the
    /// API persists as pins. Absent on responses from an older proxy, and on a
    /// connection that learned no new keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_host_keys: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProxyMetricsSample {
    pub metrics: ProxyMetricsResponse,
    pub request_latency_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyLinkMetricsResponse {
    pub link_type: String,
    pub status: String,
    pub latency_ms: Option<i64>,
    pub rx_bytes: Option<i64>,
    pub tx_bytes: Option<i64>,
    pub rx_bps: Option<i64>,
    pub tx_bps: Option<i64>,
    pub quality_label: Option<String>,
    pub quality_score: Option<i32>,
}
