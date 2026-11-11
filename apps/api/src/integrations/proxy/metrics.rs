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
    /// Runtime ownership evidence, independent of traffic rate and link quality.
    #[serde(default)]
    pub runtime: Option<ProxyRuntimePresence>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, utoipa::ToSchema)]
pub struct ProxyRuntimePresence {
    pub state: String,
    pub generation: Option<String>,
    pub operator_attached: Option<bool>,
    pub observer_count: Option<u32>,
    pub observed_at: Option<String>,
    pub last_operator_activity_at: Option<String>,
    pub reconnect_deadline: Option<String>,
    pub ended_at: Option<String>,
}

impl ProxyRuntimePresence {
    pub fn unknown() -> Self {
        Self {
            state: "unknown".to_string(),
            ..Self::default()
        }
    }
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
