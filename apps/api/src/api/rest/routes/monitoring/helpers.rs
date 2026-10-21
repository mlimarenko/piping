use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::str::FromStr;

use axum::http::{HeaderMap, header::AUTHORIZATION};

use super::{GroupByField, GroupingMode, TargetSource};
use crate::errors::AppError;

pub(super) fn normalize_metrics_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        "/metrics".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

pub(super) fn append_port(target: &str, port: u16) -> String {
    if target.is_empty() {
        return target.to_string();
    }

    if let Ok(ip) = IpAddr::from_str(target) {
        return match ip {
            IpAddr::V4(_) => format!("{target}:{port}"),
            IpAddr::V6(_) => format!("[{target}]:{port}"),
        };
    }

    if target.starts_with('[') && target.contains("]:") {
        return target.to_string();
    }

    if let Some((_, tail)) = target.rsplit_once(':')
        && tail.chars().all(|char| char.is_ascii_digit())
    {
        return target.to_string();
    }

    format!("{target}:{port}")
}

pub(super) fn deduplicate_targets(targets: &mut Vec<String>) {
    let mut seen = HashSet::new();
    targets.retain(|target| seen.insert(target.clone()));
}

pub(super) fn stable_label_key(labels: &HashMap<String, String>) -> String {
    let mut sorted = labels.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    sorted
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("|")
}

pub(super) fn parse_kind_filter(kind: Option<&str>) -> Result<Option<&'static str>, AppError> {
    let Some(kind) = normalize_string(kind) else {
        return Ok(None);
    };

    match kind.as_str() {
        "all" => Ok(None),
        "device" | "devices" => Ok(Some("device")),
        "virtual_machine" | "virtual-machines" | "virtual_machines" | "vm" | "vms" => {
            Ok(Some("virtual_machine"))
        }
        _ => Err(AppError::Validation(
            "Query parameter `kind` must be one of: all, device, virtual_machine".to_string(),
        )),
    }
}

pub(super) fn parse_target_source(target: Option<&str>) -> Result<TargetSource, AppError> {
    let value = normalize_string(target).unwrap_or_else(|| "address".to_string());
    match value.as_str() {
        "address" | "ip" => Ok(TargetSource::Address),
        "name" | "hostname" => Ok(TargetSource::Name),
        "id" | "uuid" => Ok(TargetSource::Id),
        "interface_addresses" | "interfaces" => Ok(TargetSource::InterfaceAddresses),
        "tunnel_addresses" | "tunnels" => Ok(TargetSource::TunnelAddresses),
        _ => Err(AppError::Validation(
            "Query parameter `target` must be one of: address, name, id, interface_addresses, tunnel_addresses"
                .to_string(),
        )),
    }
}

pub(super) fn parse_grouping_mode(group_by: Option<&str>) -> Result<GroupingMode, AppError> {
    let Some(group_by) = normalize_string(group_by) else {
        return Ok(GroupingMode::PerTarget);
    };

    if group_by == "none" || group_by == "single" || group_by == "all" {
        return Ok(GroupingMode::SingleGroup);
    }

    let fields = parse_group_fields(Some(group_by.as_str()))?;
    if fields.is_empty() {
        Ok(GroupingMode::PerTarget)
    } else {
        Ok(GroupingMode::ByFields)
    }
}

pub(super) fn parse_group_fields(group_by: Option<&str>) -> Result<Vec<GroupByField>, AppError> {
    let Some(group_by) = normalize_string(group_by) else {
        return Ok(Vec::new());
    };
    if group_by == "none" || group_by == "single" || group_by == "all" {
        return Ok(Vec::new());
    }

    group_by
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry {
            "kind" => Ok(GroupByField::Kind),
            "site" => Ok(GroupByField::Site),
            "tenant" => Ok(GroupByField::Tenant),
            "role" => Ok(GroupByField::Role),
            "status" => Ok(GroupByField::Status),
            _ => Err(AppError::Validation(
                "Query parameter `group_by` supports only: kind, site, tenant, role, status"
                    .to_string(),
            )),
        })
        .collect::<Result<Vec<_>, AppError>>()
}

pub(super) fn parse_extra_labels(labels: &[String]) -> Result<HashMap<String, String>, AppError> {
    let mut parsed = HashMap::new();
    for raw in labels {
        for item in raw.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let (key, value) = item
                .split_once('=')
                .or_else(|| item.split_once(':'))
                .ok_or_else(|| {
                    AppError::Validation(
                        "Query parameter `label` must be in `key=value` or `key:value` format"
                            .to_string(),
                    )
                })?;
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                return Err(AppError::Validation(
                    "Query parameter `label` must have non-empty key and value".to_string(),
                ));
            }
            if !is_valid_label_name(key) {
                return Err(AppError::Validation(format!(
                    "Invalid label name `{key}`. Prometheus label names must match [a-zA-Z_][a-zA-Z0-9_]*"
                )));
            }
            parsed.insert(key.to_string(), value.to_string());
        }
    }
    Ok(parsed)
}

pub(super) fn is_valid_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(char) if char == '_' || char.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

pub(super) fn normalize_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

pub(super) fn first_non_empty<'a>(
    primary: Option<&'a str>,
    fallback: Option<&'a str>,
) -> Option<&'a str> {
    primary
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()))
}

pub(super) fn is_monitoring_request_authorized(
    headers: &HeaderMap,
    query_token: Option<&str>,
    configured_token: Option<&str>,
) -> bool {
    let expected = expected_monitoring_token(configured_token);
    let Some(expected) = expected else {
        // Fail-closed: with no metrics token configured, deny rather than serving
        // /metrics publicly — an empty-token default must not expose the metrics
        // endpoint to anyone. Mirrors is_service_discovery_request_authorized's
        // no-token behavior.
        return false;
    };

    monitoring_token_matches(headers, query_token, expected)
}

pub(super) fn is_service_discovery_request_authorized(
    headers: &HeaderMap,
    _query_token: Option<&str>,
    configured_token: Option<&str>,
    public_enabled: bool,
) -> bool {
    if public_enabled {
        return true;
    }

    let Some(expected) = expected_monitoring_token(configured_token) else {
        return false;
    };

    monitoring_bearer_token_matches(headers, expected)
}

fn expected_monitoring_token(configured_token: Option<&str>) -> Option<&str> {
    configured_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Compare a presented credential against the expected one without leaking how
/// far the match got. `==` on a secret returns as soon as two bytes differ, so
/// how long the rejection takes tells the caller how much of the prefix was
/// right.
///
/// The length check is deliberately outside the constant-time part: the length
/// of the expected value is not the secret, and `ct_eq` requires equal-length
/// slices anyway.
fn secret_eq(presented: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    presented.len() == expected.len() && bool::from(presented.as_bytes().ct_eq(expected.as_bytes()))
}

fn monitoring_token_matches(
    headers: &HeaderMap,
    query_token: Option<&str>,
    expected: &str,
) -> bool {
    if query_token
        .map(str::trim)
        .is_some_and(|token| secret_eq(token, expected))
    {
        return true;
    }

    if let Some(header_token) = headers
        .get("x-piping-monitoring-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        && secret_eq(header_token, expected)
    {
        return true;
    }

    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_bearer_token)
        .is_some_and(|token| secret_eq(token, expected))
}

fn monitoring_bearer_token_matches(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_bearer_token)
        .is_some_and(|token| secret_eq(token, expected))
}

fn extract_bearer_token(header_value: &str) -> Option<&str> {
    let (scheme, token) = header_value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = token.trim();
        if token.is_empty() { None } else { Some(token) }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{
        append_port, extract_bearer_token, is_monitoring_request_authorized,
        is_service_discovery_request_authorized, is_valid_label_name, parse_extra_labels,
        parse_kind_filter, parse_target_source, secret_eq,
    };
    use crate::api::rest::routes::monitoring::TargetSource;

    #[test]
    fn secret_eq_accepts_only_exact_matches() {
        assert!(secret_eq("token-value", "token-value"));
        // Same length, different content.
        assert!(!secret_eq("token-valuX", "token-value"));
        // Prefix of the expected value (the short-circuit `==` failure mode).
        assert!(!secret_eq("token-val", "token-value"));
        assert!(!secret_eq("", "token-value"));
    }

    #[test]
    fn append_port_wraps_ipv6() {
        assert_eq!(append_port("2001:db8::1", 9100), "[2001:db8::1]:9100");
    }

    #[test]
    fn append_port_keeps_existing_port() {
        assert_eq!(append_port("10.0.0.10:9200", 9100), "10.0.0.10:9200");
    }

    #[test]
    fn parse_kind_filter_understands_synonyms() {
        assert_eq!(parse_kind_filter(Some("all")).expect("kind"), None);
        assert_eq!(
            parse_kind_filter(Some("devices")).expect("kind"),
            Some("device")
        );
        assert_eq!(
            parse_kind_filter(Some("vms")).expect("kind"),
            Some("virtual_machine")
        );
    }

    #[test]
    fn parse_target_source_understands_synonyms() {
        assert_eq!(
            parse_target_source(Some("hostname")).expect("target"),
            TargetSource::Name
        );
        assert_eq!(
            parse_target_source(Some("uuid")).expect("target"),
            TargetSource::Id
        );
        assert_eq!(
            parse_target_source(Some("interfaces")).expect("target"),
            TargetSource::InterfaceAddresses
        );
        assert_eq!(
            parse_target_source(Some("tunnel_addresses")).expect("target"),
            TargetSource::TunnelAddresses
        );
        assert_eq!(
            parse_target_source(Some("tunnels")).expect("target"),
            TargetSource::TunnelAddresses
        );
    }

    #[test]
    fn parse_extra_labels_accepts_equals_colon_and_csv() {
        let parsed = parse_extra_labels(&["env=prod,team:platform".to_string()]).expect("labels");
        assert_eq!(parsed.get("env").map(String::as_str), Some("prod"));
        assert_eq!(parsed.get("team").map(String::as_str), Some("platform"));
    }

    #[test]
    fn label_name_validation_matches_prometheus_shape() {
        assert!(is_valid_label_name("service_name"));
        assert!(is_valid_label_name("_meta"));
        assert!(!is_valid_label_name("9bad"));
        assert!(!is_valid_label_name("bad-name"));
    }

    #[test]
    fn bearer_token_parser_accepts_case_insensitive_scheme() {
        assert_eq!(extract_bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(extract_bearer_token("bearer abc"), Some("abc"));
        assert_eq!(extract_bearer_token("Basic abc"), None);
    }

    #[test]
    fn monitoring_request_authorizes_query_header_and_bearer_tokens() {
        let empty = HeaderMap::new();
        // Fail-closed: with no metrics token configured the endpoint is NOT
        // public (see is_monitoring_request_authorized; changed in b965f755).
        assert!(!is_monitoring_request_authorized(&empty, None, None));
        assert!(is_monitoring_request_authorized(
            &empty,
            Some("token"),
            Some("token")
        ));
        assert!(!is_monitoring_request_authorized(
            &empty,
            Some("bad"),
            Some("token")
        ));

        let mut header = HeaderMap::new();
        header.insert(
            "x-piping-monitoring-token",
            HeaderValue::from_static("token"),
        );
        assert!(is_monitoring_request_authorized(
            &header,
            None,
            Some("token")
        ));

        let mut bearer = HeaderMap::new();
        bearer.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token"),
        );
        assert!(is_monitoring_request_authorized(
            &bearer,
            None,
            Some("token")
        ));
    }

    #[test]
    fn service_discovery_request_requires_token_unless_public_enabled() {
        let empty = HeaderMap::new();
        assert!(!is_service_discovery_request_authorized(
            &empty, None, None, false
        ));
        assert!(is_service_discovery_request_authorized(
            &empty, None, None, true
        ));

        let mut bearer = HeaderMap::new();
        bearer.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token"),
        );
        assert!(is_service_discovery_request_authorized(
            &bearer,
            None,
            Some("token"),
            false
        ));
        assert!(!is_service_discovery_request_authorized(
            &empty,
            Some("token"),
            Some("token"),
            false
        ));
    }
}
