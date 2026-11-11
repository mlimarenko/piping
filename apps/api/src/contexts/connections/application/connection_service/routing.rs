use super::*;
use once_cell::sync::Lazy;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use regex::Regex;

use super::validation::{
    ENDPOINT_REASON_UNRESOLVED, ENDPOINT_REASON_UNSUPPORTED_PLACEHOLDER, endpoint_validation_error,
};
use crate::contexts::config::application::variable_substitution::resolve_field;
use crate::contexts::connections::application::connection_params_schema::{
    AppLauncherKind, classify_app_launcher_template, is_endpoint_variable_path,
    is_known_target_variable_path,
};

const ENDPOINT_PLACEHOLDERS: &[&str] = &["primary_ip4", "primary_ip6"];
static CONFIG_PLACEHOLDER_RE: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"\{\{\s*([a-zA-Z0-9_.]+)\s*\}\}").ok());

/// Percent-encode set for substituting a resolved `target.*` value INTO a web
/// URI: everything outside the URI unreserved set (`A-Z a-z 0-9 - . _ ~`) is
/// escaped. This is deliberately conservative — inventory text is hostile input,
/// so we never let it introduce URI structure (`/`, `?`, `#`, `:`, `@`, `&`, …).
const TARGET_URI_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

fn normalize_host_candidate(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

pub(super) fn is_unresolved_host_template(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("{{") && trimmed.ends_with("}}")
}

fn is_canonical_endpoint_placeholder(field_path: &str) -> bool {
    ENDPOINT_PLACEHOLDERS.contains(&field_path)
}

fn extract_template_field(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let raw = trimmed.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    (!raw.is_empty()).then_some(raw)
}

fn resolve_placeholder_value(context: &Value, field_path: &str) -> Option<String> {
    resolve_field(context, field_path).filter(|value| !value.trim().is_empty())
}

/// A resolved `target.*` (or any non-endpoint) placeholder value that lands in a
/// launch-critical string must never carry control characters — inventory text is
/// hostile input. Reject the launch when it does.
fn reject_control_chars(field_path: &str, value: &str) -> Result<(), AppError> {
    if value.chars().any(char::is_control) {
        return Err(endpoint_validation_error(
            ENDPOINT_REASON_UNSUPPORTED_PLACEHOLDER,
            format!(
                "Resolved value for placeholder '{field_path}' contains control characters and cannot be used in a launch target"
            ),
        ));
    }
    Ok(())
}

/// Resolution result for an unresolved placeholder. A KNOWN, non-endpoint
/// `target.*` path that resolves to nothing becomes an empty string (a relation
/// object is omitted entirely when its id is null, so `{{target.tenant.name}}`
/// must vanish, not stay literal). Every other unresolved placeholder — unknown
/// paths (typos kept visible) and the bare `primary_ip4`/`primary_ip6` endpoints
/// (established literal behavior) — is left literal.
fn unresolved_placeholder_value(field_path: &str, literal: &str) -> String {
    if is_known_target_variable_path(field_path) && !is_endpoint_variable_path(field_path) {
        String::new()
    } else {
        literal.to_string()
    }
}

fn substitute_endpoint_host_placeholder(value: &mut Value, context: &Value) {
    let Value::String(raw) = value else {
        return;
    };
    let Some(field_path) = extract_template_field(raw).filter(|_| is_unresolved_host_template(raw))
    else {
        return;
    };
    if !is_canonical_endpoint_placeholder(field_path) {
        return;
    }
    if let Some(replacement) = resolve_placeholder_value(context, field_path) {
        *raw = replacement;
    }
}

/// Substitutes placeholders in a plain (non-`host`, non-`url`) string field:
/// resolve every known placeholder, leaving unresolved ones literal. Control
/// chars in a resolved non-endpoint value reject the launch.
fn substitute_plain_string(raw: &mut String, context: &Value) -> Result<(), AppError> {
    let Some(pattern) = CONFIG_PLACEHOLDER_RE.as_ref() else {
        return Ok(());
    };
    let mut failure: Option<AppError> = None;
    let replaced = pattern
        .replace_all(raw, |caps: &regex::Captures<'_>| {
            let field_path = caps.get(1).map_or("", |m| m.as_str());
            match resolve_placeholder_value(context, field_path) {
                Some(value) => {
                    if !is_endpoint_variable_path(field_path)
                        && let Err(error) = reject_control_chars(field_path, &value)
                    {
                        failure.get_or_insert(error);
                    }
                    value
                }
                None => {
                    let literal = caps.get(0).map_or("", |matched| matched.as_str());
                    unresolved_placeholder_value(field_path, literal)
                }
            }
        })
        .to_string();
    if let Some(error) = failure {
        return Err(error);
    }
    *raw = replaced;
    Ok(())
}

/// Substitutes placeholders in an `app`-launcher `url`/URI string with sink-aware
/// rules:
/// - WEB URI: resolve every known placeholder; URL-encode resolved `target.*`
///   values (raw for `primary_ip4`/`primary_ip6`, since percent-encoding an IPv6
///   literal would mangle its colons). Reject control chars in a resolved
///   `target.*` value.
/// - LOCAL COMMAND: substitute ONLY `primary_ip4`/`primary_ip6`; leave every
///   `{{target.*}}` placeholder literal — inventory text must not reach a local
///   command. Write-time validation already forbids `target.*` here, so this is a
///   defense-in-depth backstop.
fn substitute_app_url(raw: &mut String, context: &Value) -> Result<(), AppError> {
    let Some(pattern) = CONFIG_PLACEHOLDER_RE.as_ref() else {
        return Ok(());
    };
    let kind = classify_app_launcher_template(raw);
    let mut failure: Option<AppError> = None;
    let replaced = pattern
        .replace_all(raw, |caps: &regex::Captures<'_>| {
            let field_path = caps.get(1).map_or("", |m| m.as_str());
            let literal_match = caps.get(0).map_or("", |matched| matched.as_str());
            let is_endpoint = is_endpoint_variable_path(field_path);
            match kind {
                // Local command: leave every non-endpoint `target.*` placeholder
                // literal — inventory text must not reach a desktop command.
                AppLauncherKind::LocalCommand if !is_endpoint => literal_match.to_string(),
                _ => match resolve_placeholder_value(context, field_path) {
                    Some(value) => {
                        if is_endpoint {
                            value
                        } else {
                            if let Err(error) = reject_control_chars(field_path, &value) {
                                failure.get_or_insert(error);
                                return value;
                            }
                            utf8_percent_encode(&value, TARGET_URI_ENCODE_SET).to_string()
                        }
                    }
                    None => unresolved_placeholder_value(field_path, literal_match),
                },
            }
        })
        .to_string();
    if let Some(error) = failure {
        return Err(error);
    }
    *raw = replaced;
    Ok(())
}

pub(super) fn substitute_config_placeholders(
    value: &mut Value,
    context: &Value,
) -> Result<(), AppError> {
    match value {
        Value::String(raw) => substitute_plain_string(raw, context),
        Value::Array(items) => {
            for item in items.iter_mut() {
                substitute_config_placeholders(item, context)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                match key.as_str() {
                    "host" => {
                        substitute_endpoint_host_placeholder(child, context);
                    }
                    "url" => {
                        if let Value::String(raw) = child {
                            substitute_app_url(raw, context)?;
                        } else {
                            substitute_config_placeholders(child, context)?;
                        }
                    }
                    _ => substitute_config_placeholders(child, context)?,
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Resolves the target device's primary IP from a materialized host config.
///
/// Reuses the canonical endpoint placeholder keys (`primary_ip4`, `primary_ip6`)
/// so the connect path can fall back to the device's primary address when a
/// connection-config record omits an explicit host. Returns the IPv4 address
/// when present, otherwise the IPv6 address, otherwise `None`.
pub(super) fn resolve_primary_endpoint(host_config: &Value) -> Option<String> {
    ENDPOINT_PLACEHOLDERS
        .iter()
        .find_map(|field_path| resolve_placeholder_value(host_config, field_path))
        .or_else(|| {
            ENDPOINT_PLACEHOLDERS.iter().find_map(|field_path| {
                resolve_placeholder_value(host_config, &format!("target.{field_path}"))
            })
        })
}

pub(super) fn resolve_proxy_host(explicit_host: Option<String>) -> Result<String, AppError> {
    let explicit = normalize_host_candidate(explicit_host);
    match explicit {
        Some(host) if is_unresolved_host_template(&host) => {
            let field_path = extract_template_field(&host);
            match field_path {
                Some(field_path) if is_canonical_endpoint_placeholder(field_path) => {
                    Err(endpoint_validation_error(
                        ENDPOINT_REASON_UNRESOLVED,
                        format!("Connection host placeholder '{field_path}' could not be resolved"),
                    ))
                }
                Some(field_path) => Err(endpoint_validation_error(
                    ENDPOINT_REASON_UNSUPPORTED_PLACEHOLDER,
                    format!(
                        "Connection host placeholder '{field_path}' is not supported; allowed keys: primary_ip4, primary_ip6"
                    ),
                )),
                None => Err(endpoint_validation_error(
                    ENDPOINT_REASON_UNSUPPORTED_PLACEHOLDER,
                    "Connection host placeholder is not supported; allowed keys: primary_ip4, primary_ip6",
                )),
            }
        }
        Some(host) => Ok(host),
        None => Err(endpoint_validation_error(
            ENDPOINT_REASON_UNRESOLVED,
            "Connection host is required",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context_with_target() -> Value {
        json!({
            "primary_ip4": "10.0.0.5",
            "primary_ip6": "2001:db8::1",
            "target": {
                "name": "host 01",
                "primary_ip4": "10.0.0.5",
                "site": {"name": "dc1", "slug": "dc1"},
                "custom_fields": {"team": "ops"}
            }
        })
    }

    #[test]
    fn host_field_stays_ip_only() {
        // host accepts only canonical endpoint placeholders as a whole-string template.
        let mut value = json!({"host": "{{primary_ip4}}"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        assert_eq!(value["host"], json!("10.0.0.5"));

        // host must NOT resolve target.* — left literal.
        let mut value = json!({"host": "{{target.name}}"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        assert_eq!(value["host"], json!("{{target.name}}"));
    }

    #[test]
    fn web_uri_url_resolves_target_url_encoded() {
        let mut value = json!({"url": "https://portal/{{target.name}}"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        // "host 01" -> the space is percent-encoded, never raw.
        assert_eq!(value["url"], json!("https://portal/host%2001"));
        let url = value["url"].as_str().unwrap();
        assert!(
            !url.contains(' '),
            "resolved web URI must not contain a raw space"
        );
    }

    #[test]
    fn web_uri_url_keeps_endpoint_ip_raw() {
        let mut value = json!({"url": "https://{{primary_ip4}}:8443/app"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        assert_eq!(value["url"], json!("https://10.0.0.5:8443/app"));
    }

    #[test]
    fn web_uri_url_resolves_custom_field() {
        let mut value = json!({"url": "https://portal/?team={{target.custom_fields.team}}"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        assert_eq!(value["url"], json!("https://portal/?team=ops"));
    }

    #[test]
    fn local_command_url_does_not_substitute_target() {
        // No scheme + a space => local command. target.* must be left literal.
        let mut value = json!({"url": "myapp --host {{target.name}} --ip {{primary_ip4}}"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        assert_eq!(
            value["url"],
            json!("myapp --host {{target.name}} --ip 10.0.0.5")
        );
    }

    #[test]
    fn null_relation_resolves_to_empty_not_literal() {
        // target.tenant is omitted entirely (id was null) => resolves to empty string.
        let mut value = json!({"url": "https://portal/{{target.tenant.name}}/end"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        assert_eq!(value["url"], json!("https://portal//end"));
    }

    #[test]
    fn plain_field_resolves_target_unencoded() {
        let mut value = json!({"label": "Host {{target.name}}"});
        substitute_config_placeholders(&mut value, &context_with_target()).unwrap();
        // ui.label is not a URI sink: no percent-encoding.
        assert_eq!(value["label"], json!("Host host 01"));
    }

    #[test]
    fn resolved_target_with_control_char_rejects_launch() {
        let context = json!({
            "target": {"name": "bad\u{0007}name"}
        });
        let mut value = json!({"url": "https://portal/{{target.name}}"});
        let result = substitute_config_placeholders(&mut value, &context);
        assert!(result.is_err());
    }
}
