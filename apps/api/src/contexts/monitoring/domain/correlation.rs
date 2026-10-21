//! Fleet-vector correlation: demultiplex a Prometheus instant-query vector
//! into per-host single-series envelopes keyed by a configurable correlation
//! label (default `piping_target_id`).

use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

use super::host::DiscoveryHost;
use super::source::ResolvedMetricQuerySource;

/// Default correlation label demultiplexing fleet result vectors into hosts.
///
/// `piping_target_id` is the one correlation label HTTP service discovery
/// always emits (`discovery.rs`), so it works on a default install. The
/// identifier labels (`piping_host_id`, …) stay opt-in behind the SD
/// `identifiers` toggle and are never a required default.
const DEFAULT_CORRELATION_LABEL: &str = "piping_target_id";

/// True when a metric query is fleet-capable (no `$host*` template token).
///
/// List metrics are batched fleet queries; per-host-templated queries cannot
/// be demultiplexed and are handled by the per-host live path instead.
///
/// Evaluate this on the **descriptor template as configured**, before `$host*`
/// substitution. Rendering replaces the token with a concrete host, so a
/// rendered query always looks fleet-capable and would wrongly demultiplex an
/// already-per-host vector.
pub fn is_list_capable_query(descriptor_query: &str) -> bool {
    !descriptor_query.contains("$host")
}

/// Read the configured correlation label for a source, defaulting to
/// `piping_target_id`. Configured via `options.correlation.label`.
pub fn correlation_label_for_source(options: &Value) -> String {
    options
        .get("correlation")
        .and_then(|correlation| correlation.get("label"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| DEFAULT_CORRELATION_LABEL.to_string(), str::to_string)
}

/// Read the configured correlation match strategy for a source, defaulting to
/// `host_id`. Configured via `options.correlation.match`.
pub fn correlation_match_for_source(options: &Value) -> String {
    options
        .get("correlation")
        .and_then(|correlation| correlation.get("match"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| "host_id".to_string(), str::to_string)
}

/// Demultiplex a Prometheus instant-query vector into per-correlation-key
/// single-series envelopes. Each series in `data.result[]` carries a `.metric`
/// label map and a `.value = [ts, "val"]` pair. The returned map is keyed by
/// the correlation label value (e.g. a host id string) → a Prometheus vector
/// envelope holding only that host's series.
///
/// The envelope shape (`{status, data:{resultType, result:[series]}}`) matches
/// exactly what `fetch_latest_prometheus_metric` returns for the per-host live
/// path, so the cached value reuses the frontend `metricDisplay`/
/// `MetricWidgetCard` rendering (which reads `value.data.result[0].value[1]`)
/// unchanged.
fn single_series_vector_envelope(series: Value) -> Value {
    serde_json::json!({
        "status": "success",
        "data": {
            "resultType": "vector",
            "result": [series],
        }
    })
}

fn empty_vector_envelope() -> Value {
    serde_json::json!({
        "status": "success",
        "data": {
            "resultType": "vector",
            "result": [],
        }
    })
}

pub fn demux_vector_by_host_label(prom_json: &Value, label: &str) -> HashMap<String, Value> {
    let mut by_key = HashMap::new();
    let Some(results) = prom_json
        .get("data")
        .and_then(|data| data.get("result"))
        .and_then(Value::as_array)
    else {
        return by_key;
    };

    for series in results {
        let Some(key) = series
            .get("metric")
            .and_then(|metric| metric.get(label))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        // First series for a key wins; later duplicates are ignored. Wrap the
        // single series in a vector envelope mirroring the live-path shape.
        by_key
            .entry(key.to_string())
            .or_insert_with(|| single_series_vector_envelope(series.clone()));
    }

    by_key
}

/// Reduce a fleet instant-query vector to the requesting host's series.
///
/// `descriptor_query` is the configured descriptor template, not the rendered
/// query: it is the only input from which list capability can be decided,
/// because rendering a `$host*` template makes the result look fleet-capable.
/// A per-host-templated descriptor is already scoped, so its vector is
/// returned unchanged.
pub fn scope_prometheus_metric_to_host(
    prom_json: Value,
    source: &ResolvedMetricQuerySource,
    host: Option<&DiscoveryHost>,
    descriptor_query: &str,
) -> Value {
    let Some(host) = host else {
        return prom_json;
    };
    // List capability is decided on the descriptor template, never on the
    // rendered query: a `$host`-templated descriptor is already per-host, so
    // its vector passes through even though rendering removed the token.
    if !is_list_capable_query(descriptor_query) {
        return prom_json;
    }

    let label = correlation_label_for_source(&source.options);
    let match_strategy = correlation_match_for_source(&source.options);
    let Some(correlation_key) = host_correlation_key(host, &match_strategy) else {
        return empty_vector_envelope();
    };

    demux_vector_by_host_label(&prom_json, &label)
        .remove(&correlation_key)
        .unwrap_or_else(empty_vector_envelope)
}

/// Resolve the correlation key for a host given the configured match strategy.
fn host_correlation_key(host: &DiscoveryHost, match_strategy: &str) -> Option<String> {
    match match_strategy {
        "host_address" => {
            let address = host.host_address.trim();
            (!address.is_empty()).then(|| address.to_string())
        }
        "host_name" => {
            let name = host.host_name.trim();
            (!name.is_empty()).then(|| name.to_string())
        }
        _ => Some(host.host_id.to_string()),
    }
}

/// Build a correlation-key → host-id index for a given match strategy.
pub fn build_host_correlation_index(
    hosts: &[DiscoveryHost],
    match_strategy: &str,
) -> HashMap<String, Uuid> {
    let mut index = HashMap::new();
    for host in hosts {
        if let Some(key) = host_correlation_key(host, match_strategy) {
            index.entry(key).or_insert(host.host_id);
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::super::host::sample_host;
    use super::super::source::LatestMetricSourceInfo;
    use super::super::templating::render_host_metric_query;
    use super::*;

    #[test]
    fn demux_vector_maps_each_series_to_its_host() {
        // Synthetic, domain-neutral Prometheus instant-query vector with two
        // series carrying different `piping_host_id` labels.
        let prom_json = serde_json::json!({
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {
                            "__name__": "up",
                            "piping_host_id": "11111111-1111-1111-1111-111111111111"
                        },
                        "value": [1700000000.0, "1"]
                    },
                    {
                        "metric": {
                            "__name__": "up",
                            "piping_host_id": "22222222-2222-2222-2222-222222222222"
                        },
                        "value": [1700000000.0, "0"]
                    }
                ]
            }
        });

        let demuxed = demux_vector_by_host_label(&prom_json, "piping_host_id");

        assert_eq!(demuxed.len(), 2);
        // Each host gets a single-series Prometheus vector envelope, matching
        // the per-host live path so the frontend renders it unchanged.
        let first = demuxed
            .get("11111111-1111-1111-1111-111111111111")
            .expect("first host series present");
        assert_eq!(first["status"], serde_json::json!("success"));
        assert_eq!(
            first["data"]["result"][0]["value"][1],
            serde_json::json!("1")
        );
        assert_eq!(
            first["data"]["result"][0]["metric"]["piping_host_id"],
            serde_json::json!("11111111-1111-1111-1111-111111111111")
        );
        let second = demuxed
            .get("22222222-2222-2222-2222-222222222222")
            .expect("second host series present");
        assert_eq!(
            second["data"]["result"][0]["value"][1],
            serde_json::json!("0")
        );
    }

    #[test]
    fn non_templated_list_query_still_demultiplexes_to_the_requested_host() {
        let prom_json = serde_json::json!({
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {
                            "__name__": "probe_success",
                            "piping_host_id": "11111111-1111-1111-1111-111111111111"
                        },
                        "value": [1700000000.0, "1"]
                    },
                    {
                        "metric": {
                            "__name__": "probe_success",
                            "piping_host_id": "22222222-2222-2222-2222-222222222222"
                        },
                        "value": [1700000000.0, "0"]
                    }
                ]
            }
        });
        let mut host = sample_host();
        host.host_id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let source = ResolvedMetricQuerySource {
            info: LatestMetricSourceInfo {
                name: Some("VM".to_string()),
                source_type: Some("victoriametrics".to_string()),
                configured_from: "monitoring_domain".to_string(),
                query_url: "https://monitoring.example/api/v1/query".to_string(),
                credential_ref: None,
            },
            source_type: "victoriametrics".to_string(),
            query_url: "https://monitoring.example/api/v1/query".to_string(),
            token: None,
            timeout_ms: 5000,
            options: serde_json::json!({
                "correlation": {"label": "piping_host_id", "match": "host_id"}
            }),
            pinned_resolution: None,
        };

        let scoped = scope_prometheus_metric_to_host(
            prom_json,
            &source,
            Some(&host),
            "probe_success{piping_target_id=~\".+\",job=\"blackbox\"}",
        );

        assert_eq!(
            scoped["data"]["result"][0]["metric"]["piping_host_id"],
            serde_json::json!("22222222-2222-2222-2222-222222222222")
        );
        assert_eq!(
            scoped["data"]["result"][0]["value"][1],
            serde_json::json!("0")
        );
    }

    #[test]
    fn scope_prometheus_metric_to_host_returns_empty_vector_when_host_series_missing() {
        let prom_json = serde_json::json!({
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": []
            }
        });
        let source = ResolvedMetricQuerySource {
            info: LatestMetricSourceInfo {
                name: Some("VM".to_string()),
                source_type: Some("victoriametrics".to_string()),
                configured_from: "monitoring_domain".to_string(),
                query_url: "https://monitoring.example/api/v1/query".to_string(),
                credential_ref: None,
            },
            source_type: "victoriametrics".to_string(),
            query_url: "https://monitoring.example/api/v1/query".to_string(),
            token: None,
            timeout_ms: 5000,
            options: serde_json::json!({
                "correlation": {"label": "piping_host_id", "match": "host_id"}
            }),
            pinned_resolution: None,
        };

        let scoped = scope_prometheus_metric_to_host(
            prom_json,
            &source,
            Some(&sample_host()),
            "probe_success{piping_target_id=~\".+\",job=\"blackbox\"}",
        );

        assert_eq!(scoped["data"]["result"], serde_json::json!([]));
    }

    #[test]
    fn templated_descriptor_is_not_demultiplexed_by_a_rendered_query() {
        // A `$host`-templated descriptor is already per-host once rendered.
        // Node-exporter series carry `piping_host_name`/`piping_target_id` but
        // no `piping_host_id`, so demultiplexing the rendered vector by the
        // correlation label would drop the only series and the live path would
        // return empty. List capability is a property of the descriptor
        // template, so the rendered vector must pass through untouched.
        let prom_json = serde_json::json!({
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {
                            "__name__": "node_load1",
                            "piping_host_name": "k8s-01.fsn1.de.piping.space",
                            "job": "node"
                        },
                        "value": [1700000000.0, "0.42"]
                    }
                ]
            }
        });
        let mut host = sample_host();
        host.host_name = "k8s-01.fsn1.de.piping.space".to_string();
        let source = ResolvedMetricQuerySource {
            info: LatestMetricSourceInfo {
                name: Some("VM".to_string()),
                source_type: Some("victoriametrics".to_string()),
                configured_from: "monitoring_domain".to_string(),
                query_url: "https://monitoring.example/api/v1/query".to_string(),
                credential_ref: None,
            },
            source_type: "victoriametrics".to_string(),
            query_url: "https://monitoring.example/api/v1/query".to_string(),
            token: None,
            timeout_ms: 5000,
            options: serde_json::json!({
                "correlation": {"label": "piping_host_id", "match": "host_id"}
            }),
            pinned_resolution: None,
        };

        // The per-host live path sends the rendered query (what the provider
        // sees) but decides list capability from the descriptor template.
        let descriptor_query = "node_load1{piping_host_name=\"$host\",job=\"node\"}";
        let executed_query = render_host_metric_query(descriptor_query, &host);
        assert!(
            !executed_query.contains("$host"),
            "rendering must remove the template token"
        );

        let scoped =
            scope_prometheus_metric_to_host(prom_json, &source, Some(&host), descriptor_query);

        assert_eq!(
            scoped["data"]["result"].as_array().map(Vec::len),
            Some(1),
            "the single per-host series must survive scoping"
        );
        assert_eq!(
            scoped["data"]["result"][0]["metric"]["piping_host_name"],
            serde_json::json!("k8s-01.fsn1.de.piping.space")
        );
    }

    #[test]
    fn demux_vector_drops_series_without_correlation_label() {
        let prom_json = serde_json::json!({
            "status": "success",
            "data": {
                "result": [
                    {
                        "metric": { "__name__": "up", "job": "node" },
                        "value": [1700000000.0, "1"]
                    },
                    {
                        "metric": {
                            "__name__": "up",
                            "piping_host_id": "33333333-3333-3333-3333-333333333333"
                        },
                        "value": [1700000000.0, "1"]
                    }
                ]
            }
        });

        let demuxed = demux_vector_by_host_label(&prom_json, "piping_host_id");

        assert_eq!(demuxed.len(), 1);
        assert!(demuxed.contains_key("33333333-3333-3333-3333-333333333333"));
    }

    #[test]
    fn demux_vector_handles_empty_or_malformed_payload() {
        assert!(demux_vector_by_host_label(&serde_json::json!({}), "piping_host_id").is_empty());
        assert!(
            demux_vector_by_host_label(
                &serde_json::json!({"data": {"result": []}}),
                "piping_host_id"
            )
            .is_empty()
        );
    }

    #[test]
    fn list_capable_query_excludes_host_templates() {
        assert!(is_list_capable_query("up{job=\"node\"}"));
        assert!(is_list_capable_query("sum(node_load1)"));
        assert!(!is_list_capable_query("up{piping_host_name=\"$host\"}"));
        assert!(!is_list_capable_query(
            "node_load1{piping_host_id=\"$host_id\"}"
        ));
        assert!(!is_list_capable_query("up{addr=\"$host_address\"}"));
    }

    #[test]
    fn correlation_label_defaults_to_piping_target_id() {
        assert_eq!(
            correlation_label_for_source(&serde_json::json!({})),
            "piping_target_id"
        );
        assert_eq!(
            correlation_label_for_source(
                &serde_json::json!({"correlation": {"label": "instance"}})
            ),
            "instance"
        );
    }
}
