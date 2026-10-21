//! Redis cache contract between the observability poller (writer) and the
//! host-labels read endpoint (reader): key layout and payload shapes.

use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::errors::AppError;

use super::super::domain::alerts::HostAlert;

/// Redis key prefix for cached per-host list labels.
pub fn host_labels_cache_key(host_id: Uuid) -> String {
    format!("obs:host:{host_id}")
}

/// Redis key prefix for cached per-host active alerts.
pub fn host_alerts_cache_key(host_id: Uuid) -> String {
    format!("obs:alerts:{host_id}")
}

/// A single cached metric value for a host, enriched with the host-specific
/// descriptor display fields so the read endpoint serves a self-contained
/// label. New non-`Option` fields intentionally lack `#[serde(default)]`:
/// pre-rollout cached payloads then fail to deserialize, are surfaced as
/// `stale` by `fetch_host_labels`, and the poller rewrites them within the
/// 3x-interval TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedHostMetric {
    pub query: String,
    pub value: Value,
    pub label: Option<String>,
    pub unit: Option<String>,
    pub value_type: Option<String>,
    pub widget_type: Option<String>,
    pub list_render: String,
    pub options: Value,
    /// Visual grouping copied from the descriptor. `Option`/`#[serde(default)]`
    /// is the rollover-safe pattern: pre-rollout cached payloads omit the key
    /// and deserialize to `None` rather than failing and being flagged stale.
    /// The poller rewrites the field within the TTL.
    #[serde(default)]
    pub category: Option<String>,
    /// Optional backend semantic copied from the descriptor. `Option` keeps
    /// pre-rollout Redis payloads readable while the poller rewrites them.
    #[serde(default)]
    pub semantic: Option<String>,
}

/// The Redis-cached payload for one host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedHostLabels {
    pub metrics: Vec<CachedHostMetric>,
    pub updated_at: String,
}

/// The Redis-cached active-alerts payload for one host (`obs:alerts:{id}`).
/// `HostAlert` is reused verbatim as the cache element and the response shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedHostAlerts {
    pub alerts: Vec<HostAlert>,
    pub updated_at: String,
}

/// One host's cached observability state as read back from Redis.
///
/// `labels` is `None` on a labels-cache miss or an undecodable payload (the
/// read endpoint surfaces those as `stale`); an alerts-cache miss yields an
/// empty list. An undecodable alerts payload is an error because an empty list
/// is authoritative to clients and must only mean that the key is absent.
#[derive(Debug, Clone)]
pub struct CachedHostState {
    pub labels: Option<CachedHostLabels>,
    pub alerts: Vec<HostAlert>,
}

/// Batch-read the cached labels + alerts payloads for `host_ids` in a single
/// `MGET` round trip — the labels keys are concatenated with the alerts keys
/// (labels first) and the combined response is split on return, instead of
/// one `MGET` per cache family (2 round trips -> 1 for any batch size).
///
/// Returns one entry per requested id, in request order.
///
/// # Errors
///
/// Returns the Redis transport error, an invalid alerts-payload error, or an
/// internal error if the `MGET` response is not positionally aligned with the
/// requested keys.
pub async fn read_host_states(
    redis: &mut ConnectionManager,
    host_ids: &[Uuid],
) -> Result<Vec<CachedHostState>, AppError> {
    if host_ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<String> = host_ids
        .iter()
        .map(|host_id| host_labels_cache_key(*host_id))
        .chain(
            host_ids
                .iter()
                .map(|host_id| host_alerts_cache_key(*host_id)),
        )
        .collect();
    let values = crate::storage::cache::redis::cache::mget(redis, &keys).await?;
    if values.len() != keys.len() {
        return Err(AppError::Internal(format!(
            "host cache MGET returned {} values for {} keys",
            values.len(),
            keys.len()
        )));
    }
    let (label_values, alert_values) = values.split_at(host_ids.len());
    label_values
        .iter()
        .zip(alert_values)
        .map(|(label_raw, alert_raw)| {
            Ok(CachedHostState {
                labels: label_raw
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<CachedHostLabels>(raw).ok()),
                alerts: decode_cached_alerts(alert_raw.as_deref())?,
            })
        })
        .collect()
}

fn decode_cached_alerts(raw: Option<&str>) -> Result<Vec<HostAlert>, AppError> {
    match raw {
        Some(raw) => serde_json::from_str::<CachedHostAlerts>(raw)
            .map(|payload| payload.alerts)
            .map_err(|error| {
                AppError::Internal(format!("Invalid host alerts cache payload: {error}"))
            }),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_host_metric_category_round_trips_through_serde() {
        let metric = CachedHostMetric {
            query: "up".to_string(),
            value: Value::Null,
            label: Some("Up".to_string()),
            unit: None,
            value_type: Some("state".to_string()),
            widget_type: Some("badge".to_string()),
            list_render: "auto".to_string(),
            options: Value::Object(serde_json::Map::new()),
            category: Some("System".to_string()),
            semantic: Some("availability".to_string()),
        };
        let json = serde_json::to_string(&metric).expect("serialize");
        let decoded: CachedHostMetric = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.category.as_deref(), Some("System"));
        assert_eq!(decoded.query, "up");
    }

    #[test]
    fn cached_host_metric_category_defaults_to_none_for_pre_rollout_payload() {
        // A payload written by the previous binary (no `category` key). Must
        // deserialize successfully so the read endpoint does NOT flag the
        // cache row as stale before the poller TTL rewrite.
        let pre_rollout = serde_json::json!({
            "query": "up",
            "value": null,
            "label": "Up",
            "unit": null,
            "value_type": "state",
            "widget_type": "badge",
            "list_render": "auto",
            "options": {}
        });
        let decoded: CachedHostMetric =
            serde_json::from_value(pre_rollout).expect("backward-compat decode");
        assert!(decoded.category.is_none());
    }

    #[test]
    fn missing_alerts_are_empty_but_corrupt_alerts_are_an_error() {
        assert!(
            decode_cached_alerts(None)
                .expect("a missing alerts key means no cached alerts")
                .is_empty()
        );
        assert!(
            decode_cached_alerts(Some("not-json")).is_err(),
            "an unreadable alerts value must not become an authoritative empty list"
        );
    }
}
