//! Provider source resolution: pick the effective query source for a
//! requested source type, resolve its credential, and validate + DNS-pin its
//! URL behind the monitoring SSRF policy.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};

use serde_json::Value;

use crate::config::MonitoringConfig;
use crate::contexts::secrets::application::SecretStore;
use crate::contexts::shared::ssrf::{
    check_ssrf_and_pin, is_cloud_metadata_ip, is_private_or_internal_ip,
};
use crate::errors::AppError;

use super::super::domain::host::{
    MonitoringQuerySourceConfig, is_prometheus_compatible_source_type,
    source_type_matches_descriptor,
};
use super::super::domain::source::{
    LatestMetricSourceInfo, PinnedMonitoringResolution, ResolvedMetricQuerySource,
};

pub async fn resolve_metric_query_source(
    monitoring_config: &MonitoringConfig,
    secret_store: &dyn SecretStore,
    sources: &[MonitoringQuerySourceConfig],
    requested_source_type: &str,
) -> Result<Option<ResolvedMetricQuerySource>, AppError> {
    let selected_source = sources
        .iter()
        .find(|source| source_type_matches_descriptor(&source.source_type, requested_source_type));

    let query_url = selected_source
        .and_then(|source| source.query_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            prometheus_compatible_settings_query_url(monitoring_config, requested_source_type)
        });
    let Some(query_url) = query_url else {
        return Ok(None);
    };
    let uses_persisted_query_url = selected_source
        .and_then(|source| source.query_url.as_deref())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let configured_from = if uses_persisted_query_url {
        "monitoring_domain".to_string()
    } else {
        "global_settings".to_string()
    };

    let source_type = selected_source
        .map(|source| source.source_type.clone())
        .unwrap_or_else(|| {
            if is_prometheus_compatible_source_type(requested_source_type) {
                "prometheus".to_string()
            } else {
                requested_source_type.to_ascii_lowercase()
            }
        });

    // A monitoring source has exactly one credential identity: the canonical
    // named-secret usage binding. Both query and alert sources resolve it from
    // `credential_secret_id`, never from free text.
    let credential_secret_id = selected_source.and_then(|source| source.credential_secret_id);
    let token = if let Some(secret_id) = credential_secret_id {
        Some(secret_store.get_secret(secret_id).await?)
    } else if !uses_persisted_query_url {
        // The deployment-level token belongs only to the deployment-level URL.
        // An unbound persisted source may point to a different endpoint and
        // must never inherit that token.
        prometheus_compatible_settings_query_token(monitoring_config, requested_source_type)
    } else {
        None
    };

    let timeout_ms = selected_source
        .and_then(|source| source.timeout_ms)
        .unwrap_or(monitoring_config.query_timeout_ms)
        .max(100);
    let options = selected_source
        .map(|source| source.options.clone())
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let pinned_resolution = if uses_persisted_query_url {
        // Persisted (admin-created) sources are validated and DNS-pinned. The
        // settings-derived global URL takes the `None` branch below and is not
        // pinned: it is trusted deployment configuration (`PIPING_*` or the
        // config file), not runtime user input, and commonly names an internal
        // monitoring backend.
        Some(pin_monitoring_query_url(&query_url, &options, selected_source.is_some()).await?)
    } else {
        None
    };

    Ok(Some(ResolvedMetricQuerySource {
        info: LatestMetricSourceInfo {
            name: selected_source.and_then(|source| source.name.clone()),
            source_type: Some(source_type.clone()),
            configured_from,
            query_url: redacted_url(&query_url),
            credential_ref: selected_source
                .and_then(|source| source.credential_secret_id)
                .map(|secret_id| secret_id.to_string()),
        },
        source_type,
        query_url,
        token,
        timeout_ms,
        options,
        pinned_resolution,
    }))
}

pub fn prometheus_compatible_settings_query_url(
    monitoring_config: &MonitoringConfig,
    requested_source_type: &str,
) -> Option<String> {
    if !is_prometheus_compatible_source_type(requested_source_type) {
        return None;
    }
    monitoring_config
        .query_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn prometheus_compatible_settings_query_token(
    monitoring_config: &MonitoringConfig,
    requested_source_type: &str,
) -> Option<String> {
    if !is_prometheus_compatible_source_type(requested_source_type) {
        return None;
    }
    monitoring_config
        .query_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn redacted_url(raw_url: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(raw_url) else {
        return raw_url.to_string();
    };

    if !url.username().is_empty() {
        let _ = url.set_username("[redacted]");
    }
    if url.password().is_some() {
        let _ = url.set_password(Some("[redacted]"));
    }
    url.set_query(None);
    url.set_fragment(None);

    url.to_string()
}

/// Validate a monitoring source URL's shape before it is fetched.
///
/// `allow_private` is the provenance signal: `true` only when the URL comes from
/// an admin-created (`CREATE|MONITORING`-gated) source row, never from a raw
/// request body. A trusted source may point at a private/internal literal IP
/// (e.g. a co-deployed Prometheus on `10.x`), but the cloud-metadata endpoint
/// and the `localhost`/`metadata` hostnames stay blocked for every source.
pub fn validate_monitoring_query_url(raw_url: &str, allow_private: bool) -> Result<(), AppError> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|_| AppError::Validation("query_url must be an absolute URL".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::Validation(
            "query_url scheme must be http or https".to_string(),
        ));
    }

    let host = url
        .host_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Validation("query_url must include a host".to_string()))?;
    let normalized_host = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if matches!(
        normalized_host.as_str(),
        "localhost" | "localhost.localdomain" | "metadata" | "metadata.google.internal"
    ) {
        return Err(AppError::Validation(
            "query_url host is not allowed for monitoring sources".to_string(),
        ));
    }

    if let Ok(ip) = normalized_host.parse::<IpAddr>() {
        let blocked = if allow_private {
            is_cloud_metadata_ip(ip)
        } else {
            is_private_or_internal_ip(ip)
        };
        if blocked {
            return Err(AppError::Validation(
                "query_url IP address is not allowed for monitoring sources".to_string(),
            ));
        }
    }

    Ok(())
}

/// Whether the raw URL's authority contains `@`, including an empty userinfo
/// (`https://@host/api/v1/query`) that `reqwest::Url` reports with neither a
/// username nor a password.
///
/// This mirrors the grammar of the `chk_monitoring_query_source_endpoint_url`
/// CHECK constraint (`^https?:[\\/]*[^\\/?#]*@`): take the text after the
/// scheme, drop leading `/` and `\`, cut at the first `/`, `\`, `?` or `#`,
/// then test for `@`. Without it the validator accepts a URL the database
/// rejects, turning a 400 into a raw constraint error.
fn authority_contains_at(raw_url: &str) -> bool {
    let Some((_scheme, remainder)) = raw_url.split_once(':') else {
        return false;
    };
    let authority = remainder.trim_start_matches(['/', '\\']);
    let end = authority
        .find(['/', '\\', '?', '#'])
        .unwrap_or(authority.len());
    authority[..end].contains('@')
}

/// Validate the persisted endpoint URL of a monitoring query source.
///
/// Unlike a one-shot metric-test URL, this value is returned by the
/// query-source CRUD API and is retained in the database. Keep every
/// credential-bearing URL component out of that durable, readable record:
/// bearer credentials belong exclusively in the named-secret binding.
pub fn validate_monitoring_query_source_url(raw_url: &str) -> Result<(), AppError> {
    validate_monitoring_query_url(raw_url, false)?;
    let url = reqwest::Url::parse(raw_url)
        .map_err(|_| AppError::Validation("query_url must be an absolute URL".to_string()))?;

    if !url.username().is_empty() || url.password().is_some() || authority_contains_at(raw_url) {
        return Err(AppError::Validation(
            "query_url must not include userinfo; use credential_ref instead".to_string(),
        ));
    }
    if url.query().is_some() {
        return Err(AppError::Validation(
            "query_url must not include query parameters; use credential_ref for credentials"
                .to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(AppError::Validation(
            "query_url must not include a fragment".to_string(),
        ));
    }

    Ok(())
}

/// Resolve and pin a monitoring source URL, enforcing the SSRF guard.
///
/// `allow_private` keys the trusted-source bypass on **provenance**, not on any
/// value inside `options`: it is `true` only when the URL originates from an
/// admin-created source row (see the call sites in `resolve_metric_query_source`
/// and the metric-test handler). A user-supplied ad-hoc URL always arrives with
/// `allow_private = false` and stays fully guarded. Even a trusted source may
/// never reach the cloud-metadata endpoint. `options` still carries the
/// narrower, explicit `allow_private_ips` allowlist (also provenance-gated).
pub async fn pin_monitoring_query_url(
    raw_url: &str,
    options: &Value,
    allow_private: bool,
) -> Result<PinnedMonitoringResolution, AppError> {
    validate_monitoring_query_url(raw_url, allow_private)?;
    let allowed_private_ips = monitoring_query_allowed_private_ips(options);
    if !allow_private && allowed_private_ips.is_empty() {
        let (host, addrs) = check_ssrf_and_pin(raw_url).await?;
        return Ok(PinnedMonitoringResolution { host, addrs });
    }

    let parsed = reqwest::Url::parse(raw_url)
        .map_err(|_| AppError::Validation("query_url must be an absolute URL".to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::Validation("query_url must include a host".to_string()))?
        .to_owned();
    let port = parsed.port_or_known_default().unwrap_or(80);
    let lookup_addr = format!("{host}:{port}");
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_addr)
        .await
        .map_err(|e| AppError::Validation(format!("query_url DNS resolution failed: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(AppError::Validation(
            "query_url DNS resolution returned no addresses".to_string(),
        ));
    }
    for addr in &addrs {
        let ip = addr.ip();
        if !is_private_or_internal_ip(ip) {
            continue;
        }
        // The cloud-metadata endpoint is unconditionally unreachable — neither a
        // trusted (admin-created) source nor an explicit `allow_private_ips`
        // entry may re-open it.
        if is_cloud_metadata_ip(ip) {
            return Err(AppError::Validation(
                "query_url resolves to the cloud-metadata endpoint; request blocked".to_string(),
            ));
        }
        // Admin-enumerated private IP for this source.
        if allowed_private_ips.contains(&ip) {
            continue;
        }
        // A trusted (admin-created) source may reach other private/internal
        // addresses (e.g. a co-deployed Prometheus).
        if allow_private {
            continue;
        }
        return Err(AppError::Validation(
            "query_url resolves to private/internal address outside monitoring source allowlist"
                .to_string(),
        ));
    }
    Ok(PinnedMonitoringResolution { host, addrs })
}

fn monitoring_query_allowed_private_ips(options: &Value) -> HashSet<IpAddr> {
    let mut allowed = HashSet::new();
    collect_ip_options(options.get("allow_private_ips"), &mut allowed);
    collect_ip_options(
        options
            .get("ssrf")
            .and_then(|ssrf| ssrf.get("allow_private_ips")),
        &mut allowed,
    );
    allowed
}

fn collect_ip_options(value: Option<&Value>, allowed: &mut HashSet<IpAddr>) {
    match value {
        Some(Value::String(raw)) => {
            if let Ok(ip) = raw.trim().parse::<IpAddr>() {
                allowed.insert(ip);
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                collect_ip_options(Some(item), allowed);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uuid::Uuid;

    struct RecordingSecretStore {
        reads: Mutex<Vec<Uuid>>,
    }

    impl RecordingSecretStore {
        fn new() -> Self {
            Self {
                reads: Mutex::new(Vec::new()),
            }
        }

        fn reads(&self) -> Vec<Uuid> {
            self.reads.lock().expect("reads lock").clone()
        }
    }

    #[async_trait]
    impl SecretStore for RecordingSecretStore {
        async fn get_secret(&self, secret_id: Uuid) -> Result<String, AppError> {
            self.reads.lock().expect("reads lock").push(secret_id);
            Ok(String::new())
        }

        async fn set_secret(&self, _secret_id: Uuid, _value: &str) -> Result<(), AppError> {
            Err(AppError::Internal("set_secret unused in tests".to_string()))
        }

        async fn delete_secret(&self, _secret_id: Uuid) -> Result<(), AppError> {
            Err(AppError::Internal(
                "delete_secret unused in tests".to_string(),
            ))
        }

        async fn ensure_unique_key(
            &self,
            _scope_type: &str,
            _scope_id: Uuid,
            _key: &str,
        ) -> Result<(), AppError> {
            Err(AppError::Internal(
                "ensure_unique_key unused in tests".to_string(),
            ))
        }
    }

    fn source_with_credential(
        source_type: &str,
        credential_secret_id: Option<Uuid>,
    ) -> MonitoringQuerySourceConfig {
        MonitoringQuerySourceConfig {
            name: Some("test source".to_string()),
            source_type: source_type.to_string(),
            // A literal public IP makes URL pinning deterministic without an
            // outbound request or DNS dependency.
            query_url: Some("http://203.0.113.1:9090/api/v1/query".to_string()),
            credential_secret_id,
            timeout_ms: Some(5000),
            options: Value::Object(serde_json::Map::new()),
        }
    }

    #[tokio::test]
    async fn query_source_resolves_typed_named_secret_binding() {
        let secret_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let store = RecordingSecretStore::new();
        let source = source_with_credential("victoriametrics", Some(secret_id));

        let resolved = resolve_metric_query_source(
            &MonitoringConfig::default(),
            &store,
            &[source],
            "victoriametrics",
        )
        .await
        .expect("the canonical binding must resolve")
        .expect("configured source must resolve");

        assert_eq!(store.reads(), vec![secret_id]);
        assert_eq!(resolved.info.credential_ref, Some(secret_id.to_string()));
    }

    #[tokio::test]
    async fn settings_query_token_is_not_inherited_by_unbound_persisted_query_source() {
        let store = RecordingSecretStore::new();
        let settings_token = format!("fixture-query-token-{}", Uuid::new_v4());
        let monitoring_config = MonitoringConfig {
            query_url: Some("http://127.0.0.1:8428/api/v1/query".to_string()),
            query_token: Some(settings_token.clone()),
            ..MonitoringConfig::default()
        };
        let persisted_source = source_with_credential("victoriametrics", None);

        let persisted = resolve_metric_query_source(
            &monitoring_config,
            &store,
            &[persisted_source],
            "victoriametrics",
        )
        .await
        .expect("unbound persisted source must resolve")
        .expect("persisted source must be selected");
        assert_eq!(persisted.token, None);
        assert!(store.reads().is_empty());

        let settings =
            resolve_metric_query_source(&monitoring_config, &store, &[], "victoriametrics")
                .await
                .expect("settings-derived source must resolve")
                .expect("settings query URL is configured");
        assert_eq!(settings.token.as_deref(), Some(settings_token.as_str()));
        assert_eq!(settings.info.configured_from, "global_settings");
    }

    #[test]
    fn latest_metric_source_redacts_url_credentials() {
        assert_eq!(
            redacted_url("https://user:pass@vm.example/api/v1/query?scope=platform#diagnostic"),
            "https://%5Bredacted%5D:%5Bredacted%5D@vm.example/api/v1/query"
        );
    }

    #[tokio::test]
    async fn query_source_pin_rejects_hostname_resolving_to_private_address() {
        // Untrusted (allow_private=false): a hostname resolving to loopback is
        // an SSRF risk and must be rejected.
        let error = pin_monitoring_query_url(
            "http://ip6-localhost:8428/api/v1/query",
            &Value::Object(serde_json::Map::new()),
            false,
        )
        .await
        .expect_err("hostname resolving to loopback must be rejected");

        assert!(matches!(error, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn query_source_pin_trusted_source_allows_hostname_resolving_to_private_address() {
        // Trusted (allow_private=true, i.e. an admin-created source row): the
        // same private/loopback resolution is permitted so the observability
        // poller can reach a co-deployed monitoring backend.
        let resolved = pin_monitoring_query_url(
            "http://ip6-localhost:8428/api/v1/query",
            &Value::Object(serde_json::Map::new()),
            true,
        )
        .await
        .expect("trusted source resolving to loopback is allowed");
        assert!(!resolved.addrs.is_empty());
    }

    #[test]
    fn query_source_validate_trusted_allows_private_literal_ip_but_not_metadata() {
        // Trusted source may target a private literal IP...
        assert!(validate_monitoring_query_url("http://10.0.0.5:9090/api/v1/query", true).is_ok());
        // ...but never the cloud-metadata endpoint, even when trusted.
        assert!(
            validate_monitoring_query_url("http://169.254.169.254/latest/meta-data", true).is_err()
        );
        // Untrusted: any private literal IP stays blocked.
        assert!(validate_monitoring_query_url("http://10.0.0.5:9090/api/v1/query", false).is_err());
        // localhost hostname is blocked regardless of trust.
        assert!(validate_monitoring_query_url("http://localhost:9090/api/v1/query", true).is_err());
    }

    #[test]
    fn query_source_endpoint_url_rejects_inline_credential_channels() {
        for (url, channel) in [
            (
                "https://operator@monitoring.example/api/v1/query",
                "userinfo",
            ),
            (
                "https:///operator:placeholder@monitoring.example/api/v1/query",
                "userinfo after extra slash",
            ),
            (
                "https:operator:placeholder@monitoring.example/api/v1/query",
                "userinfo without slash",
            ),
            (
                r"https:\\\\operator:placeholder@monitoring.example/api/v1/query",
                "userinfo after backslash",
            ),
            ("https://@monitoring.example/api/v1/query", "empty userinfo"),
            (
                "https://monitoring.example/api/v1/query?scope=platform",
                "query parameters",
            ),
            (
                "https://monitoring.example/api/v1/query#diagnostic",
                "fragment",
            ),
        ] {
            let error = validate_monitoring_query_source_url(url).expect_err(
                "a persisted query-source URL must not carry inline credential channels",
            );
            assert!(
                matches!(error, AppError::Validation(_)),
                "{channel} must be rejected before a source is persisted"
            );
        }

        assert!(
            validate_monitoring_query_source_url("https://monitoring.example/api/v1/query").is_ok(),
            "an endpoint-only query-source URL must remain valid"
        );
    }

    #[test]
    fn query_source_private_allowlist_reads_nested_options() {
        let allowed = monitoring_query_allowed_private_ips(&serde_json::json!({
            "ssrf": {"allow_private_ips": ["10.51.7.116", "not-an-ip"]}
        }));
        assert!(allowed.contains(&"10.51.7.116".parse::<IpAddr>().unwrap()));
        assert_eq!(allowed.len(), 1);
    }

    #[tokio::test]
    async fn pin_monitoring_query_url_rejects_localhost_for_test_endpoint() {
        for url in [
            "http://localhost:9090/api/v1/query",
            "http://127.0.0.1:9090/api/v1/query",
            "http://169.254.169.254/latest/meta-data",
            "file:///etc/passwd",
        ] {
            let err = pin_monitoring_query_url(url, &Value::Object(serde_json::Map::new()), false)
                .await
                .unwrap_err();
            assert!(matches!(err, AppError::Validation(_)), "{url}: {err:?}");
        }
    }
}
