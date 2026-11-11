//! Provider-neutral alert normalization: parse Alertmanager/vmalert/Zabbix
//! payloads into `NormalizedAlert`s and correlate them to hosts by a
//! correlation label.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single active alert correlated to a host.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HostAlert {
    pub severity: String,
    pub name: String,
    pub summary: Option<String>,
    pub since: Option<String>,
}

/// A provider-neutral alert before host correlation. `labels` carries the raw
/// alert label set (Alertmanager/vmalert labels, or Zabbix problem tags) so the
/// shared correlation step can key by the configured correlation label.
#[derive(Debug, Clone)]
pub struct NormalizedAlert {
    pub severity: String,
    pub name: String,
    pub summary: Option<String>,
    pub since: Option<String>,
    pub labels: HashMap<String, String>,
}

/// Provider-specific alert source kinds the poller can query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertProvider {
    Alertmanager,
    Vmalert,
    Zabbix,
}

pub fn alert_provider_for_source_type(source_type: &str) -> Option<AlertProvider> {
    match source_type.to_ascii_lowercase().as_str() {
        "alertmanager" => Some(AlertProvider::Alertmanager),
        "vmalert" => Some(AlertProvider::Vmalert),
        "zabbix" => Some(AlertProvider::Zabbix),
        _ => None,
    }
}

/// Read a string label from a JSON label/annotation object, trimming blanks.
fn json_object_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Flatten a JSON object of string values into a label map (non-strings dropped).
fn labels_map_from_object(value: Option<&Value>) -> HashMap<String, String> {
    let Some(Value::Object(map)) = value else {
        return HashMap::new();
    };
    map.iter()
        .filter_map(|(key, value)| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| (key.clone(), value.to_string()))
        })
        .collect()
}

/// Parse an Alertmanager `GET /api/v2/alerts` response into normalized alerts.
///
/// Alertmanager returns a top-level array of alert objects, each with a
/// `labels` map (carrying `alertname`, `severity`, and any correlation labels),
/// an `annotations` map (carrying `summary`/`description`), and a `startsAt`
/// timestamp.
fn parse_alertmanager_alerts(json: &Value) -> Vec<NormalizedAlert> {
    let Some(items) = json.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|alert| {
            let labels = labels_map_from_object(alert.get("labels"));
            let annotations = alert.get("annotations");
            let severity = labels
                .get("severity")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let name = labels
                .get("alertname")
                .cloned()
                .unwrap_or_else(|| "alert".to_string());
            let summary = annotations.and_then(|annotations| {
                json_object_string(annotations, "summary")
                    .or_else(|| json_object_string(annotations, "description"))
            });
            let since = json_object_string(alert, "startsAt");
            NormalizedAlert {
                severity,
                name,
                summary,
                since,
                labels,
            }
        })
        .collect()
}

/// Parse a vmalert `GET /api/v1/alerts` response into normalized alerts.
///
/// vmalert wraps alerts under `data.alerts[]`; each alert carries a `labels`
/// map (with `alertname`/`severity`/correlation labels), an `annotations` map,
/// and an `activeAt` timestamp.
fn parse_vmalert_alerts(json: &Value) -> Vec<NormalizedAlert> {
    let Some(items) = json
        .get("data")
        .and_then(|data| data.get("alerts"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|alert| {
            // Drop alerts still in the `for:` window: vmalert's `/api/v1/alerts`
            // returns `pending` alerts alongside `firing` ones, and a pending
            // alert is not yet a real host problem. When `state` is absent, keep
            // the alert (older vmalert builds omit it).
            if let Some(state) = json_object_string(alert, "state")
                && !state.eq_ignore_ascii_case("firing")
            {
                return None;
            }
            let labels = labels_map_from_object(alert.get("labels"));
            let annotations = alert.get("annotations");
            let severity = labels
                .get("severity")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let name = labels
                .get("alertname")
                .cloned()
                .or_else(|| json_object_string(alert, "name"))
                .unwrap_or_else(|| "alert".to_string());
            let summary = annotations.and_then(|annotations| {
                json_object_string(annotations, "summary")
                    .or_else(|| json_object_string(annotations, "description"))
            });
            let since = json_object_string(alert, "activeAt");
            Some(NormalizedAlert {
                severity,
                name,
                summary,
                since,
                labels,
            })
        })
        .collect()
}

/// Parse a Zabbix `problem.get` JSON-RPC result into normalized alerts.
///
/// Problems carry a numeric `severity`, a `name`, a `clock` epoch, and a
/// `tags[]` array of `{tag, value}` pairs. The tags become the alert `labels`
/// so the same correlation-label step works (the operator is expected to stamp
/// a `piping_host_id` tag on monitored hosts).
fn parse_zabbix_problems(json: &Value) -> Vec<NormalizedAlert> {
    let Some(items) = json.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|problem| {
            let mut labels = HashMap::new();
            if let Some(tags) = problem.get("tags").and_then(Value::as_array) {
                for tag in tags {
                    if let (Some(key), Some(value)) = (
                        json_object_string(tag, "tag"),
                        json_object_string(tag, "value"),
                    ) {
                        labels.entry(key).or_insert(value);
                    }
                }
            }
            let severity = problem
                .get("severity")
                .and_then(|value| match value {
                    Value::String(value) => Some(zabbix_severity_label(value)),
                    Value::Number(value) => Some(zabbix_severity_label(&value.to_string())),
                    _ => None,
                })
                .unwrap_or_else(|| "unknown".to_string());
            let name = json_object_string(problem, "name").unwrap_or_else(|| "problem".to_string());
            let since = json_object_string(problem, "clock");
            NormalizedAlert {
                severity,
                name,
                summary: None,
                since,
                labels,
            }
        })
        .collect()
}

/// Map a Zabbix numeric severity code to a provider-neutral severity word.
fn zabbix_severity_label(code: &str) -> String {
    match code.trim() {
        "5" => "disaster",
        "4" => "high",
        "3" => "average",
        "2" => "warning",
        "1" => "information",
        "0" => "not_classified",
        other => other,
    }
    .to_string()
}

/// Parse a provider response into normalized alerts.
pub fn parse_alerts_for_provider(provider: AlertProvider, json: &Value) -> Vec<NormalizedAlert> {
    match provider {
        AlertProvider::Alertmanager => parse_alertmanager_alerts(json),
        AlertProvider::Vmalert => parse_vmalert_alerts(json),
        AlertProvider::Zabbix => parse_zabbix_problems(json),
    }
}

/// Correlate normalized alerts to hosts by a correlation label, returning a map
/// keyed by the correlation-label value (e.g. a host id string) → that host's
/// alerts. Alerts whose label set lacks the correlation label are dropped.
pub fn correlate_alerts_by_host(
    alerts: &[NormalizedAlert],
    correlation_label: &str,
) -> HashMap<String, Vec<HostAlert>> {
    let mut by_key: HashMap<String, Vec<HostAlert>> = HashMap::new();
    for alert in alerts {
        let Some(key) = alert
            .labels
            .get(correlation_label)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        by_key.entry(key.to_string()).or_default().push(HostAlert {
            severity: alert.severity.clone(),
            name: alert.name.clone(),
            summary: alert.summary.clone(),
            since: alert.since.clone(),
        });
    }
    by_key
}

/// Build the alert query URL for a provider by appending its alerts path to the
/// configured source base URL (trailing slashes are normalized). Zabbix posts
/// JSON-RPC to the base URL itself, so its URL is unchanged.
pub fn alert_provider_query_url(provider: AlertProvider, base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    match provider {
        // Restrict to active, operator-relevant alerts: Alertmanager v2 returns
        // silenced and inhibited alerts by default, which would surface
        // operator-suppressed alerts as firing host badges.
        AlertProvider::Alertmanager => {
            format!("{trimmed}/api/v2/alerts?active=true&silenced=false&inhibited=false") // allow: canonical-vocab
        }
        AlertProvider::Vmalert => format!("{trimmed}/api/v1/alerts"),
        AlertProvider::Zabbix => base_url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_alertmanager_alerts_normalizes_two_alerts() {
        // Synthetic, domain-neutral Alertmanager `GET /api/v2/alerts` response:
        // a top-level array of alert objects with `labels`/`annotations`.
        let json = serde_json::json!([
            {
                "labels": {
                    "alertname": "HighLoad",
                    "severity": "warning",
                    "piping_host_id": "11111111-1111-1111-1111-111111111111"
                },
                "annotations": { "summary": "Load is high" },
                "startsAt": "2024-01-01T00:00:00Z"
            },
            {
                "labels": {
                    "alertname": "DiskFull",
                    "severity": "critical",
                    "piping_host_id": "22222222-2222-2222-2222-222222222222"
                },
                "annotations": { "description": "Disk almost full" },
                "startsAt": "2024-01-02T00:00:00Z"
            }
        ]);

        let alerts = parse_alertmanager_alerts(&json);

        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].name, "HighLoad");
        assert_eq!(alerts[0].severity, "warning");
        assert_eq!(alerts[0].summary.as_deref(), Some("Load is high"));
        assert_eq!(alerts[0].since.as_deref(), Some("2024-01-01T00:00:00Z"));
        assert_eq!(
            alerts[0].labels.get("piping_host_id").map(String::as_str),
            Some("11111111-1111-1111-1111-111111111111")
        );
        // The second alert falls back to `annotations.description` for summary.
        assert_eq!(alerts[1].name, "DiskFull");
        assert_eq!(alerts[1].severity, "critical");
        assert_eq!(alerts[1].summary.as_deref(), Some("Disk almost full"));
    }

    #[test]
    fn parse_alertmanager_alerts_defaults_missing_fields() {
        // An alert with no labels/annotations still normalizes with defaults.
        let json = serde_json::json!([{}]);
        let alerts = parse_alertmanager_alerts(&json);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].name, "alert");
        assert_eq!(alerts[0].severity, "unknown");
        assert!(alerts[0].summary.is_none());
        assert!(alerts[0].since.is_none());
        // A non-array payload yields no alerts (poller never panics on garbage).
        assert!(parse_alertmanager_alerts(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn parse_vmalert_alerts_normalizes_two_alerts() {
        // Synthetic, domain-neutral vmalert `GET /api/v1/alerts` response:
        // alerts wrapped under `data.alerts[]` with `activeAt` timestamps.
        let json = serde_json::json!({
            "status": "success",
            "data": {
                "alerts": [
                    {
                        "state": "firing",
                        "labels": {
                            "alertname": "TargetDown",
                            "severity": "critical",
                            "piping_host_id": "33333333-3333-3333-3333-333333333333"
                        },
                        "annotations": { "summary": "Target is down" },
                        "activeAt": "2024-02-01T00:00:00Z"
                    },
                    {
                        "labels": {
                            "severity": "warning",
                            "piping_host_id": "44444444-4444-4444-4444-444444444444"
                        },
                        "name": "LatencyHigh",
                        "annotations": {},
                        "activeAt": "2024-02-02T00:00:00Z"
                    }
                ]
            }
        });

        let alerts = parse_vmalert_alerts(&json);

        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].name, "TargetDown");
        assert_eq!(alerts[0].severity, "critical");
        assert_eq!(alerts[0].summary.as_deref(), Some("Target is down"));
        assert_eq!(alerts[0].since.as_deref(), Some("2024-02-01T00:00:00Z"));
        // The second alert lacks `labels.alertname`, so the top-level `name`
        // field supplies the alert name.
        assert_eq!(alerts[1].name, "LatencyHigh");
        assert_eq!(
            alerts[1].labels.get("piping_host_id").map(String::as_str),
            Some("44444444-4444-4444-4444-444444444444")
        );
        // A payload without `data.alerts` yields no alerts.
        assert!(parse_vmalert_alerts(&serde_json::json!({"data": {}})).is_empty());
    }

    #[test]
    fn parse_vmalert_alerts_drops_pending_state() {
        // vmalert returns `pending` alerts (the `for:` window not yet elapsed)
        // alongside `firing` ones; only firing alerts are real host problems.
        let json = serde_json::json!({
            "data": {
                "alerts": [
                    {
                        "state": "pending",
                        "labels": { "alertname": "Warming", "severity": "warning" },
                        "activeAt": "2024-03-01T00:00:00Z"
                    },
                    {
                        "state": "firing",
                        "labels": { "alertname": "Firing", "severity": "critical" },
                        "activeAt": "2024-03-02T00:00:00Z"
                    }
                ]
            }
        });

        let alerts = parse_vmalert_alerts(&json);

        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].name, "Firing");
        // An alert with no `state` is kept (older vmalert builds omit it).
        let no_state = serde_json::json!({
            "data": { "alerts": [{ "labels": { "alertname": "SampleAlert" } }] }
        });
        assert_eq!(parse_vmalert_alerts(&no_state).len(), 1);
    }

    #[test]
    fn parse_zabbix_problems_normalizes_tags_and_severity() {
        // Synthetic, domain-neutral Zabbix `problem.get` result: an array of
        // problems with numeric `severity`, `clock`, and a `tags[]` array that
        // flattens into the alert labels (incl. the correlation tag).
        let json = serde_json::json!([
            {
                "name": "Disk space low",
                "severity": "4",
                "clock": "1700000000",
                "tags": [
                    { "tag": "piping_host_id", "value": "55555555-5555-5555-5555-555555555555" },
                    { "tag": "scope", "value": "storage" }
                ]
            },
            {
                "name": "CPU busy",
                "severity": 2,
                "clock": "1700000100",
                "tags": []
            }
        ]);

        let alerts = parse_zabbix_problems(&json);

        assert_eq!(alerts.len(), 2);
        // Numeric code → provider-neutral word, via String and Number forms.
        assert_eq!(alerts[0].name, "Disk space low");
        assert_eq!(alerts[0].severity, "high");
        assert_eq!(alerts[0].since.as_deref(), Some("1700000000"));
        assert_eq!(
            alerts[0].labels.get("piping_host_id").map(String::as_str),
            Some("55555555-5555-5555-5555-555555555555")
        );
        assert_eq!(
            alerts[0].labels.get("scope").map(String::as_str),
            Some("storage")
        );
        assert_eq!(alerts[1].severity, "warning");
        // A non-array payload yields no problems (poller never panics on garbage).
        assert!(parse_zabbix_problems(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn correlate_alerts_by_host_maps_each_alert_to_its_host() {
        let alerts = vec![
            NormalizedAlert {
                severity: "warning".to_string(),
                name: "HighLoad".to_string(),
                summary: Some("Load is high".to_string()),
                since: Some("2024-01-01T00:00:00Z".to_string()),
                labels: HashMap::from([(
                    "piping_host_id".to_string(),
                    "11111111-1111-1111-1111-111111111111".to_string(),
                )]),
            },
            NormalizedAlert {
                severity: "critical".to_string(),
                name: "DiskFull".to_string(),
                summary: None,
                since: None,
                labels: HashMap::from([(
                    "piping_host_id".to_string(),
                    "22222222-2222-2222-2222-222222222222".to_string(),
                )]),
            },
            // No correlation label → dropped.
            NormalizedAlert {
                severity: "info".to_string(),
                name: "Orphan".to_string(),
                summary: None,
                since: None,
                labels: HashMap::from([("job".to_string(), "node".to_string())]),
            },
        ];

        let by_host = correlate_alerts_by_host(&alerts, "piping_host_id");

        assert_eq!(by_host.len(), 2);
        let first = by_host
            .get("11111111-1111-1111-1111-111111111111")
            .expect("first host alerts present");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].name, "HighLoad");
        let second = by_host
            .get("22222222-2222-2222-2222-222222222222")
            .expect("second host alerts present");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].name, "DiskFull");
        // The label-less alert was dropped (not attributed to any host).
        assert!(
            !by_host
                .values()
                .any(|alerts| alerts.iter().any(|alert| alert.name == "Orphan"))
        );
    }
}
