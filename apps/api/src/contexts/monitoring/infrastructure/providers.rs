//! Provider HTTP clients: Prometheus-compatible instant queries,
//! Alertmanager/vmalert active-alert reads, and the Zabbix JSON-RPC family.
//! Persisted domain sources run against an SSRF-pinned `reqwest` client built
//! by [`metric_query_http_client`]. The global settings URL is intentionally
//! unpinned: it comes from trusted deployment configuration (`PIPING_*` or the
//! config file), not from a runtime user request.

use std::time::Duration;

use serde_json::Value;

use super::super::domain::alerts::{AlertProvider, alert_provider_query_url};
use super::super::domain::correlation::scope_prometheus_metric_to_host;
use super::super::domain::host::{
    DiscoveryHost, MonitoringMetricDescriptor, descriptor_query_type,
};
use super::super::domain::source::{PinnedMonitoringResolution, ResolvedMetricQuerySource};
use super::super::domain::templating::render_zabbix_host_selector;

pub fn metric_query_http_client(
    pinned_resolution: Option<&PinnedMonitoringResolution>,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    if let Some(pinned_resolution) = pinned_resolution {
        builder = builder.resolve_to_addrs(&pinned_resolution.host, &pinned_resolution.addrs);
    }
    builder.build().map_err(|error| error.to_string())
}

fn trimmed_token(token: Option<&str>) -> Option<&str> {
    token.map(str::trim).filter(|value| !value.is_empty())
}

fn prometheus_query_url(query_url: &str, metric_query: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(query_url).map_err(|error| error.to_string())?;
    url.query_pairs_mut().append_pair("query", metric_query);
    Ok(url)
}

fn decode_provider_json_response(
    status: reqwest::StatusCode,
    body: &[u8],
    endpoint_name: &str,
) -> Result<Value, String> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("{endpoint_name} returned HTTP {status}"));
    }
    Ok(value)
}

fn active_alert_request_url(
    provider: AlertProvider,
    query_url: &str,
) -> Result<reqwest::Url, String> {
    reqwest::Url::parse(&alert_provider_query_url(provider, query_url))
        .map_err(|error| error.to_string())
}

/// Fetch a host's latest Prometheus-compatible metric and, for a fleet-capable
/// descriptor, reduce the vector to that host.
///
/// `executed_query` is what is sent upstream (the descriptor with `$host*`
/// already substituted); `descriptor_query` is the configured template, which
/// is what decides list capability. They differ exactly when the descriptor is
/// per-host templated, in which case the returned vector is already scoped.
pub async fn fetch_latest_prometheus_metric_for_host(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    executed_query: &str,
    descriptor_query: &str,
    host: Option<&DiscoveryHost>,
) -> Result<Value, String> {
    fetch_latest_prometheus_metric(client, source, executed_query)
        .await
        .map(|value| scope_prometheus_metric_to_host(value, source, host, descriptor_query))
}

pub async fn fetch_latest_prometheus_metric(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    metric_query: &str,
) -> Result<Value, String> {
    let url = prometheus_query_url(&source.query_url, metric_query)?;

    let mut request = client.get(url);
    if let Some(token) = trimmed_token(source.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = request
        .timeout(Duration::from_millis(source.timeout_ms))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|error| error.to_string())?;
    decode_provider_json_response(status, &body, "query endpoint")
}

/// Query a resolved alert source once for its active alerts and return the raw
/// provider JSON. Alertmanager/vmalert use a `GET` against the appended alerts
/// path; Zabbix posts a `problem.get` JSON-RPC envelope to the base URL.
pub async fn fetch_active_alerts(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    provider: AlertProvider,
) -> Result<Value, String> {
    if provider == AlertProvider::Zabbix {
        let params = serde_json::json!({
            "output": ["eventid", "name", "severity", "clock"],
            "selectTags": "extend",
            "recent": false,
            "sortfield": ["eventid"],
            "sortorder": "DESC"
        });
        return zabbix_rpc(client, source, "problem.get", params).await;
    }

    let url = active_alert_request_url(provider, &source.query_url)?;
    let mut request = client.get(url);
    if let Some(token) = trimmed_token(source.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = request
        .timeout(Duration::from_millis(source.timeout_ms))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|error| error.to_string())?;
    decode_provider_json_response(status, &body, "alert endpoint")
}

fn zabbix_item_params(
    query_type: &str,
    host_id: &str,
    executed_query: &str,
    monitored_only: bool,
) -> Value {
    let mut params = match query_type {
        "zabbix_itemid" => serde_json::json!({
            "output": ["itemid", "hostid", "name", "key_", "lastvalue", "lastclock", "value_type", "units", "status", "error"],
            "itemids": [executed_query],
            "limit": 1
        }),
        "zabbix_item_name" => serde_json::json!({
            "output": ["itemid", "hostid", "name", "key_", "lastvalue", "lastclock", "value_type", "units", "status", "error"],
            "hostids": [host_id],
            "search": { "name": executed_query },
            "sortfield": "name",
            "limit": 1
        }),
        _ => serde_json::json!({
            "output": ["itemid", "hostid", "name", "key_", "lastvalue", "lastclock", "value_type", "units", "status", "error"],
            "hostids": [host_id],
            "filter": { "key_": [executed_query] },
            "sortfield": "name",
            "limit": 1
        }),
    };
    if monitored_only && let Some(object) = params.as_object_mut() {
        object.insert("monitored".to_string(), Value::Bool(true));
    }
    params
}

fn first_zabbix_item(
    result: &Value,
    executed_query: &str,
    host_name: &str,
) -> Result<Value, String> {
    result
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .ok_or_else(|| {
            format!("Zabbix item `{executed_query}` was not found for host `{host_name}`")
        })
}

/// Resolve a single Zabbix `item.get` row for a descriptor on a host.
///
/// Returns `(item, executed_query, zabbix_host_id)`. Shared by both the
/// instant ([`fetch_latest_zabbix_metric`]) and range (`fetch_zabbix_range`)
/// paths so the item-resolution logic lives in exactly one place.
pub async fn resolve_zabbix_item(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    descriptor: &MonitoringMetricDescriptor,
    host: &DiscoveryHost,
) -> Result<(Value, String, String), String> {
    let host_id = resolve_zabbix_host_id(client, source, host).await?;
    let query_type = descriptor_query_type(descriptor);
    let executed_query = render_zabbix_host_selector(&descriptor.query, host);
    let params = zabbix_item_params(
        query_type,
        &host_id,
        &executed_query,
        json_bool_option(&source.options, &["monitored_only"]).unwrap_or(true),
    );

    let result = zabbix_rpc(client, source, "item.get", params).await?;
    let item = first_zabbix_item(&result, &executed_query, &host.host_name)?;

    Ok((item, executed_query, host_id))
}

fn zabbix_host_lookup_params(filter_key: &str, host_selector: &str, group_ids: &[String]) -> Value {
    let mut params = serde_json::json!({
        "output": ["hostid", "host", "name"],
        "filter": { filter_key: [host_selector] },
        "limit": 1
    });
    if !group_ids.is_empty()
        && let Some(object) = params.as_object_mut()
    {
        object.insert(
            "groupids".to_string(),
            Value::Array(group_ids.iter().cloned().map(Value::String).collect()),
        );
    }
    params
}

pub async fn fetch_latest_zabbix_metric(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    descriptor: &MonitoringMetricDescriptor,
    host: &DiscoveryHost,
) -> Result<(Value, String), String> {
    let (item, executed_query, host_id) =
        resolve_zabbix_item(client, source, descriptor, host).await?;

    Ok((
        serde_json::json!({
            "provider": "zabbix",
            "host": {
                "piping_host_id": host.host_id,
                "piping_host_name": host.host_name,
                "zabbix_hostid": host_id
            },
            "item": item,
            "value": item.get("lastvalue").cloned().unwrap_or(Value::Null),
            "clock": item.get("lastclock").cloned().unwrap_or(Value::Null)
        }),
        executed_query,
    ))
}

async fn resolve_zabbix_host_id(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    host: &DiscoveryHost,
) -> Result<String, String> {
    if let Some(host_id) = json_string_option(&source.options, &["zabbix_hostid", "hostid"]) {
        return Ok(render_zabbix_host_selector(&host_id, host));
    }

    let host_selector = zabbix_host_selector(source, host);
    let group_ids = json_string_array_option(&source.options, &["zabbix_groupids", "groupids"]);
    for filter_key in ["host", "name"] {
        let params = zabbix_host_lookup_params(filter_key, &host_selector, &group_ids);

        let result = zabbix_rpc(client, source, "host.get", params).await?;
        if let Some(host_id) = result
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item.get("hostid"))
            .and_then(Value::as_str)
        {
            return Ok(host_id.to_string());
        }
    }

    Err(format!("Zabbix host `{host_selector}` was not found"))
}

#[derive(Debug, PartialEq)]
struct ZabbixRequestParts {
    payload: Value,
    bearer_token: Option<String>,
}

fn zabbix_request_parts(
    source: &ResolvedMetricQuerySource,
    method: &str,
    params: Value,
) -> ZabbixRequestParts {
    let mut payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });
    let auth_mode =
        json_string_option(&source.options, &["auth_mode"]).unwrap_or_else(|| "bearer".to_string());
    let token = trimmed_token(source.token.as_deref()).map(str::to_string);
    let bearer_token = if auth_mode == "auth_param" {
        if let Some(token) = token.as_ref()
            && let Some(object) = payload.as_object_mut()
        {
            object.insert("auth".to_string(), Value::String(token.clone()));
        }
        None
    } else {
        token
    };
    ZabbixRequestParts {
        payload,
        bearer_token,
    }
}

fn decode_zabbix_response(status: reqwest::StatusCode, body: &[u8]) -> Result<Value, String> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("Zabbix API returned HTTP {status}"));
    }
    if let Some(error) = value.get("error") {
        return Err(format!("Zabbix API error: {error}"));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "Zabbix API response did not include result".to_string())
}

fn zabbix_host_selector(source: &ResolvedMetricQuerySource, host: &DiscoveryHost) -> String {
    if let Some(template) = json_string_option(
        &source.options,
        &["zabbix_host", "host", "host_value", "host_template"],
    ) {
        return render_zabbix_host_selector(&template, host);
    }

    match json_string_option(&source.options, &["host_match"])
        .unwrap_or_else(|| "host_name".to_string())
        .as_str()
    {
        "host_address" | "address" | "ip" => host.host_address.clone(),
        "host_id" | "id" | "uuid" => host.host_id.to_string(),
        _ => host.host_name.clone(),
    }
}

pub async fn zabbix_rpc(
    client: &reqwest::Client,
    source: &ResolvedMetricQuerySource,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let parts = zabbix_request_parts(source, method, params);
    let mut request = client
        .post(&source.query_url)
        .timeout(Duration::from_millis(source.timeout_ms))
        .json(&parts.payload);
    if let Some(token) = parts.bearer_token.as_deref() {
        request = request.bearer_auth(token);
    }

    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.bytes().await.map_err(|error| error.to_string())?;
    decode_zabbix_response(status, &body)
}

fn json_string_option(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|value| match value {
            Value::String(value) => {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
    })
}

fn json_bool_option(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn json_string_array_option(value: &Value, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| match value {
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|item| match item {
                        Value::String(value) => {
                            let trimmed = value.trim();
                            (!trimmed.is_empty()).then(|| trimmed.to_string())
                        }
                        Value::Number(value) => Some(value.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            Value::String(value) => Some(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::super::super::domain::host::sample_host;
    use super::super::super::domain::source::LatestMetricSourceInfo;
    use super::super::super::domain::templating::render_host_metric_query;
    use super::*;

    fn test_source(
        query_url: String,
        token: Option<&str>,
        options: Value,
    ) -> ResolvedMetricQuerySource {
        ResolvedMetricQuerySource {
            source_type: "zabbix".to_string(),
            info: LatestMetricSourceInfo {
                name: Some("Test provider".to_string()),
                source_type: Some("zabbix".to_string()),
                configured_from: "test".to_string(),
                query_url: query_url.clone(),
                credential_ref: None,
            },
            query_url,
            token: token.map(str::to_string),
            timeout_ms: 5000,
            options,
            pinned_resolution: None,
        }
    }
    #[test]
    fn prometheus_request_preserves_existing_pairs_and_encodes_the_query() {
        let query = "rate(node_cpu_seconds_total{instance=\"db-01:9100\"}[5m])";
        let url = prometheus_query_url("https://metrics.example/api/v1/query?tenant=core", query)
            .expect("valid Prometheus URL");

        assert_eq!(url.path(), "/api/v1/query");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("tenant".into(), "core".into()),
                ("query".into(), query.into())
            ]
        );
        assert_eq!(
            trimmed_token(Some("  prometheus-token\t")),
            Some("prometheus-token")
        );
        assert_eq!(trimmed_token(Some(" \t ")), None);
    }

    #[test]
    fn prometheus_non_success_status_is_reported_instead_of_returning_its_json() {
        let body = serde_json::to_vec(
            &serde_json::json!({ "status": "error", "error": "storage unavailable" }),
        )
        .unwrap();

        let error =
            decode_provider_json_response(StatusCode::SERVICE_UNAVAILABLE, &body, "query endpoint")
                .expect_err("HTTP failure must not be returned as metric data");

        assert_eq!(
            error,
            "query endpoint returned HTTP 503 Service Unavailable"
        );
    }

    #[test]
    fn provider_response_rejects_malformed_json_even_when_http_succeeds() {
        let error = decode_provider_json_response(
            StatusCode::OK,
            br#"{"status":"success","data":]"#,
            "query endpoint",
        )
        .expect_err("malformed provider JSON must not escape as metric data");

        assert!(!error.is_empty());
    }

    #[test]
    fn concurrent_prometheus_url_builds_keep_each_query_isolated() {
        let handles = (0..16)
            .map(|index| {
                std::thread::spawn(move || {
                    let query = format!("up{{shard=\"{index}\"}}");
                    let url = prometheus_query_url(
                        "https://metrics.example/api/v1/query?tenant=core",
                        &query,
                    )
                    .expect("valid Prometheus URL");
                    (query, url)
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            let (query, url) = handle.join().expect("the URL builder thread");
            assert_eq!(
                url.query_pairs().collect::<Vec<_>>(),
                vec![
                    ("tenant".into(), "core".into()),
                    ("query".into(), query.into())
                ]
            );
        }
    }

    /// One-shot HTTP/1.1 server on loopback returning a fixed JSON body, so the
    /// live fetch path can be exercised without a real provider.
    fn spawn_prometheus_query_body(body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}/api/v1/query")
    }

    #[tokio::test]
    async fn per_host_templated_descriptor_passes_through_the_live_fetch() {
        // Caller-wiring guard: the boundary receives the descriptor template
        // separately from the rendered query and must decide list capability
        // from the template. A rendered templated query looks fleet-capable, so
        // scoping it by the correlation label would drop the only series (it
        // carries `piping_host_name`, not `piping_host_id`).
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{"__name__":"node_load1","piping_host_name":"k8s-01.fsn1.de.piping.space","job":"node"},"value":[1700000000.0,"0.42"]}]}}"#;
        let query_url = spawn_prometheus_query_body(body);
        let source = ResolvedMetricQuerySource {
            source_type: "prometheus".to_string(),
            info: LatestMetricSourceInfo {
                name: Some("Test provider".to_string()),
                source_type: Some("prometheus".to_string()),
                configured_from: "test".to_string(),
                query_url: query_url.clone(),
                credential_ref: None,
            },
            query_url,
            token: None,
            timeout_ms: 5000,
            options: serde_json::json!({}),
            pinned_resolution: None,
        };
        let mut host = sample_host();
        host.host_name = "k8s-01.fsn1.de.piping.space".to_string();
        let descriptor_query = "node_load1{piping_host_name=\"$host\",job=\"node\"}";
        let executed_query = render_host_metric_query(descriptor_query, &host);
        let client = metric_query_http_client(None).expect("client");

        let value = fetch_latest_prometheus_metric_for_host(
            &client,
            &source,
            &executed_query,
            descriptor_query,
            Some(&host),
        )
        .await
        .expect("fetch");

        assert_eq!(value["data"]["result"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            value["data"]["result"][0]["metric"]["piping_host_name"],
            serde_json::json!("k8s-01.fsn1.de.piping.space")
        );
    }

    #[test]
    fn alertmanager_url_selects_only_active_unsuppressed_alerts() {
        assert_eq!(
            active_alert_request_url(
                AlertProvider::Alertmanager,
                "https://alerts.example/alertmanager/"
            )
            .expect("valid alert URL")
            .as_str(),
            "https://alerts.example/alertmanager/api/v2/alerts?active=true&silenced=false&inhibited=false" // allow: canonical-vocab
        );
    }

    #[test]
    fn zabbix_auth_param_puts_trimmed_token_only_in_json_rpc_envelope() {
        let source = test_source(
            "https://zabbix.example/api_jsonrpc.php".to_string(),
            Some("  zabbix-token "),
            serde_json::json!({ "auth_mode": "auth_param" }),
        );

        let parts = zabbix_request_parts(
            &source,
            "problem.get",
            serde_json::json!({ "recent": false }),
        );

        assert_eq!(parts.bearer_token, None);
        assert_eq!(
            parts.payload,
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "problem.get",
                "params": { "recent": false },
                "id": 1,
                "auth": "zabbix-token"
            })
        );
    }

    #[test]
    fn zabbix_default_auth_uses_bearer_without_leaking_into_the_payload() {
        let source = test_source(
            "https://zabbix.example/api_jsonrpc.php".to_string(),
            Some(" api-token "),
            Value::Null,
        );

        let parts = zabbix_request_parts(&source, "item.get", serde_json::json!({}));

        assert_eq!(parts.bearer_token.as_deref(), Some("api-token"));
        assert!(parts.payload.get("auth").is_none());
    }

    #[test]
    fn zabbix_json_rpc_error_wins_over_an_http_200_response() {
        let provider_error = serde_json::json!({
            "code": -32602,
            "message": "Invalid params",
            "data": "Unknown output field"
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "error": provider_error,
            "id": 1
        }))
        .unwrap();

        let error = decode_zabbix_response(StatusCode::OK, &body)
            .expect_err("JSON-RPC error must not be treated as a successful empty result");

        assert_eq!(error, format!("Zabbix API error: {provider_error}"));
    }

    #[test]
    fn zabbix_http_failure_wins_over_a_result_body() {
        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "result": [{ "eventid": "73" }],
            "id": 1
        }))
        .unwrap();

        let error =
            decode_zabbix_response(StatusCode::OK, &body).expect("the HTTP 200 result is valid");
        assert_eq!(error, serde_json::json!([{ "eventid": "73" }]));

        let error = decode_zabbix_response(StatusCode::BAD_GATEWAY, &body)
            .expect_err("HTTP failure must not return its JSON-RPC result");
        assert_eq!(error, "Zabbix API returned HTTP 502 Bad Gateway");
    }

    #[test]
    fn zabbix_response_without_result_is_rejected() {
        let error = decode_zabbix_response(StatusCode::OK, br#"{"jsonrpc":"2.0","id":1}"#)
            .expect_err("missing result must be explicit");

        assert_eq!(error, "Zabbix API response did not include result");
    }

    #[test]
    fn zabbix_host_lookup_keeps_group_scope_for_both_selector_fallbacks() {
        let source = test_source(
            "https://zabbix.example/api_jsonrpc.php".to_string(),
            None,
            serde_json::json!({ "zabbix_groupids": ["42", 7, " "] }),
        );
        let group_ids = json_string_array_option(&source.options, &["zabbix_groupids"]);

        assert_eq!(
            zabbix_host_lookup_params("host", "db-01", &group_ids),
            serde_json::json!({
                "output": ["hostid", "host", "name"],
                "filter": { "host": ["db-01"] },
                "groupids": ["42", "7"],
                "limit": 1
            })
        );
        assert_eq!(
            zabbix_host_lookup_params("name", "db-01", &group_ids),
            serde_json::json!({
                "output": ["hostid", "host", "name"],
                "filter": { "name": ["db-01"] },
                "groupids": ["42", "7"],
                "limit": 1
            })
        );
    }

    #[test]
    fn zabbix_item_key_lookup_is_host_scoped_and_monitored_by_default() {
        assert_eq!(
            zabbix_item_params("zabbix_item_key", "9001", "system.cpu.load[db-01]", true,),
            serde_json::json!({
                "output": ["itemid", "hostid", "name", "key_", "lastvalue", "lastclock", "value_type", "units", "status", "error"],
                "hostids": ["9001"],
                "filter": { "key_": ["system.cpu.load[db-01]"] },
                "sortfield": "name",
                "limit": 1,
                "monitored": true
            })
        );
    }

    #[test]
    fn zabbix_itemid_lookup_omits_host_scope_and_can_include_unmonitored_items() {
        assert_eq!(
            zabbix_item_params("zabbix_itemid", "unused-host", "db-01-item", false),
            serde_json::json!({
                "output": ["itemid", "hostid", "name", "key_", "lastvalue", "lastclock", "value_type", "units", "status", "error"],
                "itemids": ["db-01-item"],
                "limit": 1
            })
        );
    }

    #[test]
    fn zabbix_item_name_lookup_uses_search_and_names_the_missing_item() {
        assert_eq!(
            zabbix_item_params("zabbix_item_name", "2718", "CPU utilization on db-01", true,),
            serde_json::json!({
                "output": ["itemid", "hostid", "name", "key_", "lastvalue", "lastclock", "value_type", "units", "status", "error"],
                "hostids": ["2718"],
                "search": { "name": "CPU utilization on db-01" },
                "sortfield": "name",
                "limit": 1,
                "monitored": true
            })
        );

        let error = first_zabbix_item(&serde_json::json!([]), "CPU utilization on db-01", "db-01")
            .expect_err("empty item.get result must identify the failed lookup");
        assert_eq!(
            error,
            "Zabbix item `CPU utilization on db-01` was not found for host `db-01`"
        );
    }

    #[test]
    fn zabbix_host_selector_uses_assignment_options() {
        let source = ResolvedMetricQuerySource {
            source_type: "zabbix".to_string(),
            query_url: "https://zabbix.example/api_jsonrpc.php".to_string(),
            token: Some("token".to_string()),
            info: LatestMetricSourceInfo {
                name: Some("Zabbix".to_string()),
                source_type: Some("zabbix".to_string()),
                configured_from: "monitoring_domain".to_string(),
                query_url: "https://zabbix.example/api_jsonrpc.php".to_string(),
                credential_ref: Some("secret://id".to_string()),
            },
            timeout_ms: 5000,
            options: serde_json::json!({
                "host_match": "host_address",
                "zabbix_groupids": ["42"]
            }),
            pinned_resolution: None,
        };

        assert_eq!(zabbix_host_selector(&source, &sample_host()), "10.0.0.10");
        assert_eq!(
            json_string_array_option(&source.options, &["zabbix_groupids"]),
            vec!["42".to_string()]
        );
    }
}
