use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

static UUID_RE: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
        .ok()
});
static COLOR_RE: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(r"^(#[0-9A-Fa-f]{3}|#[0-9A-Fa-f]{6}|[a-zA-Z][a-zA-Z0-9_-]{0,63})$").ok()
});

const ALLOWED_PROTOCOLS: [&str; 5] = ["ssh", "rdp", "vnc", "app", "cctv"];

const SSH_ALLOWED_FIELDS: [&str; 5] = ["host", "port", "credentials", "label", "color"];
const SSH_CREDENTIAL_FIELDS: [&str; 3] = ["username", "password", "private_key"];
const RDP_ALLOWED_FIELDS: [&str; 6] = ["host", "port", "credentials", "options", "label", "color"];
const RDP_CREDENTIAL_FIELDS: [&str; 2] = ["username", "password"];
const RDP_OPTION_FIELDS: [&str; 8] = [
    "color_depth",
    "width",
    "height",
    "dpi",
    "security",
    "ignore_cert",
    "disable_auth",
    "disable_gfx",
];
const VNC_ALLOWED_FIELDS: [&str; 5] = ["host", "port", "credentials", "label", "color"];
const VNC_CREDENTIAL_FIELDS: [&str; 1] = ["password"];
const APP_ALLOWED_FIELDS: [&str; 4] = ["url", "path", "label", "color"];
const CCTV_ALLOWED_FIELDS: [&str; 5] = ["host", "mgmt_port", "channel", "family", "credentials"];
const CCTV_CREDENTIAL_FIELDS: [&str; 2] = ["username", "password"];

pub const SECRET_FIELDS_SSH: [&str; 2] = ["password", "private_key"];
pub const SECRET_FIELDS_RDP: [&str; 2] = ["username", "password"];
pub const SECRET_FIELDS_VNC: [&str; 1] = ["password"];
pub const SECRET_FIELDS_CCTV: [&str; 1] = ["password"];

/// Canonical, RESERVED set of `{{...}}` placeholders usable in connection-config
/// record params. This is the SINGLE source of truth for the host-variable
/// namespace; the frontend insert menu mirrors this list (see
/// `apps/web/src/features/connections/connection-config/RecordForm.tsx`) and the
/// launch-time resolver in `connection_service/routing.rs` resolves the same
/// dotted paths against the per-launch `{ target: ... }` overlay.
///
/// `primary_ip4`/`primary_ip6` are the canonical endpoint placeholders (also the
/// only ones allowed in the `host` field and in local-command app targets). The
/// `target.*` paths resolve against the authoritative per-launch namespace built
/// by `ConfigContextRepository::load_target_variable_namespace`. A relation
/// object is omitted when its id is null, so an unresolved `target.tenant.name`
/// becomes an empty string rather than a literal placeholder.
///
/// Custom fields are addressed by the dynamic prefix `target.custom_fields.` plus
/// a `[A-Za-z][A-Za-z0-9_]{0,63}` identifier; see
/// [`TARGET_CUSTOM_FIELDS_PREFIX`].
pub const KNOWN_TARGET_VARIABLE_PATHS: &[&str] = &[
    "primary_ip4",
    "primary_ip6",
    "target.name",
    "target.primary_ip4",
    "target.primary_ip6",
    "target.site.name",
    "target.site.slug",
    "target.region.name",
    "target.region.slug",
    "target.tenant.name",
    "target.tenant.slug",
    "target.role.name",
    "target.role.slug",
    "target.platform.name",
    "target.platform.slug",
    "target.device_type.model",
    "target.device_type.slug",
    "target.cluster.name",
];

/// Dynamic prefix for inventory custom fields: any `target.custom_fields.<ident>`
/// where `<ident>` matches `^[a-zA-Z][a-zA-Z0-9_]{0,63}$` is accepted.
pub const TARGET_CUSTOM_FIELDS_PREFIX: &str = "target.custom_fields.";

/// Endpoint placeholders that resolve to the target's primary IP. These are the
/// ONLY placeholders permitted in the `host` field and in local-command app
/// targets (inventory text is hostile input and must not reach a local command).
pub const ENDPOINT_VARIABLE_PATHS: [&str; 2] = ["primary_ip4", "primary_ip6"];

/// Classifies whether an app-launcher template is a web URI (a URL with an
/// allowed http/https-class scheme) or a local command. This is the SINGLE
/// shared classifier used by BOTH write-time validation (this module) and
/// launch-time substitution (`connection_service/routing.rs`); the two paths
/// MUST agree byte-for-byte, so do not fork this logic.
///
/// The template is classified with its `{{...}}` placeholders INTACT (never a
/// substituted string), so an injected space can never re-classify a web URI
/// into a local command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppLauncherKind {
    /// http/https-class web URI: `target.*` placeholders are allowed (and
    /// URL-encoded when substituted at launch).
    WebUri,
    /// Local command run on the operator desktop: only `primary_ip4`/
    /// `primary_ip6` are allowed; `target.*` is forbidden.
    LocalCommand,
}

/// The shared web-URI-vs-local-command classifier. See [`AppLauncherKind`].
pub(crate) fn classify_app_launcher_template(template: &str) -> AppLauncherKind {
    let target = template.trim();
    let Some((scheme, _rest)) = split_app_launcher_uri(target) else {
        return AppLauncherKind::LocalCommand;
    };
    let scheme = scheme.to_ascii_lowercase();
    if is_blocked_app_uri_scheme(&scheme) {
        return AppLauncherKind::LocalCommand;
    }
    // Mirror `validate_app_launcher_target`: a strict-scheme target, or one with
    // no quote/whitespace, is treated as a URI; otherwise it is a local command.
    if strict_app_uri_scheme(&scheme)
        || !target
            .chars()
            .any(|character| character == '"' || character.is_whitespace())
    {
        AppLauncherKind::WebUri
    } else {
        AppLauncherKind::LocalCommand
    }
}

/// True when `field_path` is an accepted target-variable placeholder: a member
/// of [`KNOWN_TARGET_VARIABLE_PATHS`], or a `target.custom_fields.<ident>` with a
/// valid identifier.
pub(crate) fn is_known_target_variable_path(field_path: &str) -> bool {
    if KNOWN_TARGET_VARIABLE_PATHS.contains(&field_path) {
        return true;
    }
    if let Some(ident) = field_path.strip_prefix(TARGET_CUSTOM_FIELDS_PREFIX) {
        return is_valid_custom_field_ident(ident);
    }
    false
}

/// True when `field_path` is one of the canonical endpoint placeholders
/// (`primary_ip4`/`primary_ip6`).
pub(crate) fn is_endpoint_variable_path(field_path: &str) -> bool {
    ENDPOINT_VARIABLE_PATHS.contains(&field_path)
}

/// Validates an inventory custom-field key: `^[a-zA-Z][a-zA-Z0-9_]{0,63}$`.
///
/// Delegates to the canonical custom-field key validator in
/// `custom_fields::domain::validation` (single source of truth).
pub(crate) fn is_valid_custom_field_ident(ident: &str) -> bool {
    crate::contexts::custom_fields::domain::is_valid_custom_field_key(ident)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionParamValidationError {
    pub field_path: String,
    pub constraint: String,
    pub message: String,
    pub suggestion: Option<String>,
}

pub fn validate_connection_params(data: &Value) -> Result<(), Vec<ConnectionParamValidationError>> {
    let mut errors = Vec::new();

    let Some(piping) = data.get("piping") else {
        return Ok(());
    };
    let Some(connections) = piping.get("connections") else {
        return Ok(());
    };

    let Some(connections_obj) = connections.as_object() else {
        errors.push(ConnectionParamValidationError {
            field_path: "piping.connections".to_string(),
            constraint: "type_object".to_string(),
            message: "piping.connections must be an object".to_string(),
            suggestion: Some("Use object with protocol keys: ssh, rdp, vnc, app, cctv".to_string()),
        });
        return Err(errors);
    };

    for (protocol, protocol_value) in connections_obj {
        if !ALLOWED_PROTOCOLS.contains(&protocol.as_str()) {
            errors.push(ConnectionParamValidationError {
                field_path: format!("piping.connections.{protocol}"),
                constraint: "unknown_protocol".to_string(),
                message: format!("Unsupported protocol key: {protocol}"),
                suggestion: Some("Allowed protocols: ssh, rdp, vnc, app, cctv".to_string()),
            });
            continue;
        }

        let entries = match normalize_entries(protocol, protocol_value, &mut errors) {
            Some(entries) => entries,
            None => continue,
        };

        for (index, entry) in entries {
            match protocol.as_str() {
                "ssh" => validate_ssh_entry(entry, index, &mut errors),
                "rdp" => validate_rdp_entry(entry, index, &mut errors),
                "vnc" => validate_vnc_entry(entry, index, &mut errors),
                "app" => validate_app_entry(entry, index, &mut errors),
                "cctv" => validate_cctv_entry(entry, index, &mut errors),
                _ => {}
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn extract_secret_field_paths(config: &Value) -> Vec<String> {
    let mut out = Vec::new();

    let Some(piping) = config.get("piping") else {
        return out;
    };
    let Some(connections) = piping.get("connections") else {
        return out;
    };
    let Some(connections_obj) = connections.as_object() else {
        return out;
    };

    for (protocol, value) in connections_obj {
        let secret_fields: &[&str] = match protocol.as_str() {
            "ssh" => &SECRET_FIELDS_SSH,
            "rdp" => &SECRET_FIELDS_RDP,
            "vnc" => &SECRET_FIELDS_VNC,
            "cctv" => &SECRET_FIELDS_CCTV,
            _ => continue,
        };

        match value {
            Value::Array(entries) => {
                for (index, entry) in entries.iter().enumerate() {
                    if let Some(obj) = entry.as_object()
                        && let Some(credentials) = obj.get("credentials").and_then(Value::as_object)
                    {
                        for field in secret_fields {
                            if credentials.contains_key(*field) {
                                out.push(format!(
                                    "piping.connections.{protocol}[{index}].credentials.{field}"
                                ));
                            }
                        }
                    }
                }
            }
            Value::Object(obj) => {
                if let Some(credentials) = obj.get("credentials").and_then(Value::as_object) {
                    for field in secret_fields {
                        if credentials.contains_key(*field) {
                            out.push(format!("piping.connections.{protocol}.credentials.{field}"));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    out
}

fn normalize_entries<'a>(
    protocol: &str,
    value: &'a Value,
    errors: &mut Vec<ConnectionParamValidationError>,
) -> Option<Vec<(usize, &'a Value)>> {
    match value {
        Value::Array(entries) => {
            let mut normalized = Vec::with_capacity(entries.len());
            for (index, entry) in entries.iter().enumerate() {
                if !entry.is_object() {
                    errors.push(ConnectionParamValidationError {
                        field_path: format!("piping.connections.{protocol}[{index}]"),
                        constraint: "type_object".to_string(),
                        message: "Protocol entry must be an object".to_string(),
                        suggestion: Some("Use object with protocol-specific fields".to_string()),
                    });
                    continue;
                }
                normalized.push((index, entry));
            }
            Some(normalized)
        }
        Value::Object(_) => Some(vec![(0, value)]),
        _ => {
            errors.push(ConnectionParamValidationError {
                field_path: format!("piping.connections.{protocol}"),
                constraint: "type_object_or_array".to_string(),
                message: "Protocol value must be an object or an array of objects".to_string(),
                suggestion: Some("Use object or array payload for protocol entries".to_string()),
            });
            None
        }
    }
}

fn validate_ssh_entry(
    entry: &Value,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    reject_unknown_fields(entry, "ssh", index, &SSH_ALLOWED_FIELDS, errors);

    validate_required_non_empty_string(entry, "ssh", index, "host", errors);
    validate_port(entry, "ssh", index, errors);
    if let Some(credentials) = validate_required_object(entry, "ssh", index, "credentials", errors)
    {
        reject_unknown_nested_fields(
            credentials,
            "ssh",
            index,
            "credentials",
            &SSH_CREDENTIAL_FIELDS,
            errors,
        );
        validate_required_non_empty_string_in_object(
            credentials,
            "ssh",
            index,
            "credentials",
            "username",
            errors,
        );
        validate_optional_uuid_in_object(
            credentials,
            "ssh",
            index,
            "credentials",
            "password",
            errors,
        );
        validate_optional_uuid_in_object(
            credentials,
            "ssh",
            index,
            "credentials",
            "private_key",
            errors,
        );
    }
    validate_optional_max_len(entry, "ssh", index, "label", 255, errors);
    validate_optional_color(entry, "ssh", index, errors);
}

fn validate_rdp_entry(
    entry: &Value,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    reject_unknown_fields(entry, "rdp", index, &RDP_ALLOWED_FIELDS, errors);

    validate_required_non_empty_string(entry, "rdp", index, "host", errors);
    validate_port(entry, "rdp", index, errors);
    if let Some(credentials) = validate_optional_object(entry, "rdp", index, "credentials", errors)
    {
        reject_unknown_nested_fields(
            credentials,
            "rdp",
            index,
            "credentials",
            &RDP_CREDENTIAL_FIELDS,
            errors,
        );
        validate_optional_uuid_in_object(
            credentials,
            "rdp",
            index,
            "credentials",
            "username",
            errors,
        );
        validate_optional_uuid_in_object(
            credentials,
            "rdp",
            index,
            "credentials",
            "password",
            errors,
        );
    }
    if let Some(options) = validate_optional_object(entry, "rdp", index, "options", errors) {
        reject_unknown_nested_fields(options, "rdp", index, "options", &RDP_OPTION_FIELDS, errors);
        validate_optional_color_depth_in_object(
            options,
            "rdp",
            index,
            "options",
            "color_depth",
            errors,
        );
        let options_path = ValidationPath {
            protocol: "rdp",
            index,
            object_field: Some("options"),
        };
        validate_optional_range_in_object(options, options_path, "width", 1, 7680, errors);
        validate_optional_range_in_object(options, options_path, "height", 1, 4320, errors);
        validate_optional_range_in_object(options, options_path, "dpi", 1, 600, errors);
        validate_optional_enum_in_object(
            options,
            options_path,
            "security",
            &["any", "nla", "tls", "rdp"],
            errors,
        );
        validate_optional_bool_in_object(options, "rdp", index, "options", "ignore_cert", errors);
        validate_optional_bool_in_object(options, "rdp", index, "options", "disable_auth", errors);
        validate_optional_bool_in_object(options, "rdp", index, "options", "disable_gfx", errors);
    }
    validate_optional_max_len(entry, "rdp", index, "label", 255, errors);
    validate_optional_color(entry, "rdp", index, errors);
}

fn validate_vnc_entry(
    entry: &Value,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    reject_unknown_fields(entry, "vnc", index, &VNC_ALLOWED_FIELDS, errors);

    validate_required_non_empty_string(entry, "vnc", index, "host", errors);
    validate_port(entry, "vnc", index, errors);
    if let Some(credentials) = validate_optional_object(entry, "vnc", index, "credentials", errors)
    {
        reject_unknown_nested_fields(
            credentials,
            "vnc",
            index,
            "credentials",
            &VNC_CREDENTIAL_FIELDS,
            errors,
        );
        validate_optional_uuid_in_object(
            credentials,
            "vnc",
            index,
            "credentials",
            "password",
            errors,
        );
    }
    validate_optional_max_len(entry, "vnc", index, "label", 255, errors);
    validate_optional_color(entry, "vnc", index, errors);
}

fn validate_app_entry(
    entry: &Value,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    reject_unknown_fields(entry, "app", index, &APP_ALLOWED_FIELDS, errors);

    validate_required_app_launcher_target(entry, "app", index, errors);
    validate_optional_string(entry, "app", index, "path", errors);
    validate_optional_max_len(entry, "app", index, "label", 255, errors);
    validate_optional_color(entry, "app", index, errors);
}

fn validate_cctv_entry(
    entry: &Value,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    reject_unknown_fields(entry, "cctv", index, &CCTV_ALLOWED_FIELDS, errors);

    let cctv_path = ValidationPath {
        protocol: "cctv",
        index,
        object_field: None,
    };
    validate_optional_non_empty_string(entry, "cctv", index, "host", errors);
    validate_optional_int_range(entry, cctv_path, "mgmt_port", 1, 65535, errors);
    validate_optional_int_range(entry, cctv_path, "channel", 0, 65535, errors);
    validate_optional_string(entry, "cctv", index, "family", errors);
    if let Some(credentials) = validate_required_object(entry, "cctv", index, "credentials", errors)
    {
        reject_unknown_nested_fields(
            credentials,
            "cctv",
            index,
            "credentials",
            &CCTV_CREDENTIAL_FIELDS,
            errors,
        );
        validate_required_non_empty_string_in_object(
            credentials,
            "cctv",
            index,
            "credentials",
            "username",
            errors,
        );
        validate_optional_uuid_in_object(
            credentials,
            "cctv",
            index,
            "credentials",
            "password",
            errors,
        );
    }
}

fn reject_unknown_fields(
    entry: &Value,
    protocol: &str,
    index: usize,
    allowed_fields: &[&str],
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(obj) = entry.as_object() else {
        return;
    };

    let allowed: HashSet<&str> = allowed_fields.iter().copied().collect();
    for key in obj.keys() {
        if !allowed.contains(key.as_str()) {
            errors.push(ConnectionParamValidationError {
                field_path: format!("piping.connections.{protocol}[{index}].{key}"),
                constraint: "unknown_field".to_string(),
                message: format!("Field '{key}' is not allowed for protocol '{protocol}'"),
                suggestion: None,
            });
        }
    }
}

fn nested_field_path(protocol: &str, index: usize, object_field: &str, field: &str) -> String {
    format!("piping.connections.{protocol}[{index}].{object_field}.{field}")
}

/// The location prefix of one connection-parameter field: the protocol entry and
/// its array index, plus the enclosing object field for nested objects. It keeps
/// the field-path formatting in one place across the optional validators.
#[derive(Clone, Copy)]
struct ValidationPath<'a> {
    protocol: &'a str,
    index: usize,
    object_field: Option<&'a str>,
}

impl ValidationPath<'_> {
    /// `piping.connections.<protocol>[<index>][.<object_field>].<field>`,
    /// byte-identical to the inline formatting it replaces.
    fn field_path(&self, field: &str) -> String {
        let protocol = self.protocol;
        let index = self.index;
        match self.object_field {
            Some(object_field) => nested_field_path(protocol, index, object_field, field),
            None => format!("piping.connections.{protocol}[{index}].{field}"),
        }
    }
}

fn validate_required_object<'a>(
    entry: &'a Value,
    protocol: &str,
    index: usize,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) -> Option<&'a Map<String, Value>> {
    let Some(value) = entry.get(field) else {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "required".to_string(),
            message: "Field is required".to_string(),
            suggestion: None,
        });
        return None;
    };
    validate_object_value(value, protocol, index, field, errors)
}

fn validate_optional_object<'a>(
    entry: &'a Value,
    protocol: &str,
    index: usize,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) -> Option<&'a Map<String, Value>> {
    let value = entry.get(field)?;
    if value.is_null() {
        return None;
    }
    validate_object_value(value, protocol, index, field, errors)
}

fn validate_object_value<'a>(
    value: &'a Value,
    protocol: &str,
    index: usize,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) -> Option<&'a Map<String, Value>> {
    let Some(object) = value.as_object() else {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "type_object".to_string(),
            message: "Field must be an object".to_string(),
            suggestion: None,
        });
        return None;
    };
    Some(object)
}

fn reject_unknown_nested_fields(
    object: &Map<String, Value>,
    protocol: &str,
    index: usize,
    object_field: &str,
    allowed_fields: &[&str],
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let allowed: HashSet<&str> = allowed_fields.iter().copied().collect();
    for key in object.keys() {
        if !allowed.contains(key.as_str()) {
            errors.push(ConnectionParamValidationError {
                field_path: nested_field_path(protocol, index, object_field, key),
                constraint: "unknown_field".to_string(),
                message: format!(
                    "Field '{key}' is not allowed for protocol '{protocol}' {object_field}"
                ),
                suggestion: None,
            });
        }
    }
}

fn validate_required_non_empty_string_in_object(
    object: &Map<String, Value>,
    protocol: &str,
    index: usize,
    object_field: &str,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let path = nested_field_path(protocol, index, object_field, field);
    let Some(value) = object.get(field) else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "required".to_string(),
            message: "Field is required".to_string(),
            suggestion: None,
        });
        return;
    };

    let Some(text) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "type_string".to_string(),
            message: "Field must be a string".to_string(),
            suggestion: None,
        });
        return;
    };

    if text.trim().is_empty() {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "non_empty".to_string(),
            message: "Field cannot be empty".to_string(),
            suggestion: None,
        });
    }
}

fn validate_optional_uuid_in_object(
    object: &Map<String, Value>,
    protocol: &str,
    index: usize,
    object_field: &str,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = object.get(field) else {
        return;
    };
    if value.is_null() {
        return;
    }
    let path = nested_field_path(protocol, index, object_field, field);
    let Some(text) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "type_string".to_string(),
            message: "Field must be a UUID string".to_string(),
            suggestion: None,
        });
        return;
    };

    if !UUID_RE
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(text))
    {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "uuid".to_string(),
            message: "Field must be a valid UUID".to_string(),
            suggestion: None,
        });
    }
}

fn validate_optional_range_in_object(
    object: &Map<String, Value>,
    path: ValidationPath<'_>,
    field: &str,
    min: i64,
    max: i64,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = object.get(field) else {
        return;
    };
    let field_path = path.field_path(field);
    let Some(number) = value.as_i64() else {
        errors.push(ConnectionParamValidationError {
            field_path,
            constraint: "type_integer".to_string(),
            message: "Field must be an integer".to_string(),
            suggestion: None,
        });
        return;
    };

    if !(min..=max).contains(&number) {
        errors.push(ConnectionParamValidationError {
            field_path,
            constraint: "range".to_string(),
            message: format!("Field must be between {min} and {max}"),
            suggestion: None,
        });
    }
}

fn validate_optional_enum_in_object(
    object: &Map<String, Value>,
    path: ValidationPath<'_>,
    field: &str,
    allowed: &[&str],
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = object.get(field) else {
        return;
    };
    let field_path = path.field_path(field);
    let Some(text) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path,
            constraint: "type_string".to_string(),
            message: "Field must be a string".to_string(),
            suggestion: None,
        });
        return;
    };

    if !allowed.contains(&text) {
        errors.push(ConnectionParamValidationError {
            field_path,
            constraint: "enum".to_string(),
            message: format!("Field must be one of: {}", allowed.join(", ")),
            suggestion: None,
        });
    }
}

fn validate_optional_color_depth_in_object(
    object: &Map<String, Value>,
    protocol: &str,
    index: usize,
    object_field: &str,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = object.get(field) else {
        return;
    };
    let Some(depth) = value.as_i64() else {
        errors.push(ConnectionParamValidationError {
            field_path: nested_field_path(protocol, index, object_field, field),
            constraint: "type_integer".to_string(),
            message: "Field must be an integer".to_string(),
            suggestion: None,
        });
        return;
    };

    if !matches!(depth, 8 | 16 | 24 | 32) {
        errors.push(ConnectionParamValidationError {
            field_path: nested_field_path(protocol, index, object_field, field),
            constraint: "enum".to_string(),
            message: "Field must be one of: 8, 16, 24, 32".to_string(),
            suggestion: None,
        });
    }
}

fn validate_optional_bool_in_object(
    object: &Map<String, Value>,
    protocol: &str,
    index: usize,
    object_field: &str,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = object.get(field) else {
        return;
    };
    if !value.is_boolean() {
        errors.push(ConnectionParamValidationError {
            field_path: nested_field_path(protocol, index, object_field, field),
            constraint: "type_boolean".to_string(),
            message: "Field must be a boolean".to_string(),
            suggestion: None,
        });
    }
}

fn validate_required_non_empty_string(
    entry: &Value,
    protocol: &str,
    index: usize,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = entry.get(field) else {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "required".to_string(),
            message: "Field is required".to_string(),
            suggestion: None,
        });
        return;
    };

    let Some(text) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "type_string".to_string(),
            message: "Field must be a string".to_string(),
            suggestion: None,
        });
        return;
    };

    if text.trim().is_empty() {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "non_empty".to_string(),
            message: "Field cannot be empty".to_string(),
            suggestion: None,
        });
    }
}

fn validate_optional_non_empty_string(
    entry: &Value,
    protocol: &str,
    index: usize,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = entry.get(field) else {
        return;
    };
    if value.is_null() {
        return;
    }

    let path = format!("piping.connections.{protocol}[{index}].{field}");
    let Some(text) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "type_string".to_string(),
            message: "Field must be a string".to_string(),
            suggestion: None,
        });
        return;
    };

    if text.trim().is_empty() {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "non_empty".to_string(),
            message: "Field cannot be empty".to_string(),
            suggestion: None,
        });
    }
}

/// Validates an optional top-level integer entry field against an inclusive
/// range. Mirrors [`validate_optional_range_in_object`] for nested objects.
fn validate_optional_int_range(
    entry: &Value,
    path: ValidationPath<'_>,
    field: &str,
    min: i64,
    max: i64,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = entry.get(field) else {
        return;
    };
    if value.is_null() {
        return;
    }

    let field_path = path.field_path(field);
    let Some(number) = value.as_i64() else {
        errors.push(ConnectionParamValidationError {
            field_path,
            constraint: "type_integer".to_string(),
            message: "Field must be an integer".to_string(),
            suggestion: None,
        });
        return;
    };

    if !(min..=max).contains(&number) {
        errors.push(ConnectionParamValidationError {
            field_path,
            constraint: "range".to_string(),
            message: format!("Field must be between {min} and {max}"),
            suggestion: None,
        });
    }
}

fn validate_port(
    entry: &Value,
    protocol: &str,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let path = format!("piping.connections.{protocol}[{index}].port");
    let Some(value) = entry.get("port") else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "required".to_string(),
            message: "Field is required".to_string(),
            suggestion: None,
        });
        return;
    };

    let Some(port) = value.as_i64() else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "type_integer".to_string(),
            message: "Port must be an integer".to_string(),
            suggestion: None,
        });
        return;
    };

    if !(1..=65535).contains(&port) {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "range".to_string(),
            message: "Port must be between 1 and 65535".to_string(),
            suggestion: None,
        });
    }
}

fn validate_optional_max_len(
    entry: &Value,
    protocol: &str,
    index: usize,
    field: &str,
    max_len: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = entry.get(field) else {
        return;
    };

    let Some(text) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "type_string".to_string(),
            message: "Field must be a string".to_string(),
            suggestion: None,
        });
        return;
    };

    if text.len() > max_len {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "max_length".to_string(),
            message: format!("Field must not exceed {max_len} characters"),
            suggestion: None,
        });
    }
}

fn validate_optional_color(
    entry: &Value,
    protocol: &str,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = entry.get("color") else {
        return;
    };

    let Some(color) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].color"),
            constraint: "type_string".to_string(),
            message: "Color must be a string".to_string(),
            suggestion: Some("Use CSS color name or #RRGGBB/#RGB".to_string()),
        });
        return;
    };

    if !COLOR_RE
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(color))
    {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].color"),
            constraint: "css_color".to_string(),
            message: "Color must be a valid CSS color token".to_string(),
            suggestion: Some("Examples: #00ff00, #0f0, green".to_string()),
        });
    }
}

fn validate_optional_string(
    entry: &Value,
    protocol: &str,
    index: usize,
    field: &str,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let Some(value) = entry.get(field) else {
        return;
    };

    if !value.is_string() {
        errors.push(ConnectionParamValidationError {
            field_path: format!("piping.connections.{protocol}[{index}].{field}"),
            constraint: "type_string".to_string(),
            message: "Field must be a string".to_string(),
            suggestion: None,
        });
    }
}

fn validate_required_app_launcher_target(
    entry: &Value,
    protocol: &str,
    index: usize,
    errors: &mut Vec<ConnectionParamValidationError>,
) {
    let path = format!("piping.connections.{protocol}[{index}].url");
    let Some(value) = entry.get("url") else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "required".to_string(),
            message: "Field is required".to_string(),
            suggestion: None,
        });
        return;
    };

    let Some(url_text) = value.as_str() else {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "type_string".to_string(),
            message: "APP launcher target must be a string".to_string(),
            suggestion: None,
        });
        return;
    };

    let validation_target = match app_launcher_template_for_validation(url_text) {
        Ok(value) => value,
        Err(message) => {
            errors.push(ConnectionParamValidationError {
                field_path: path,
                constraint: "app_launcher_template".to_string(),
                message,
                suggestion: Some(known_placeholder_suggestion()),
            });
            return;
        }
    };

    if let Err(message) = validate_app_launcher_target(&validation_target) {
        errors.push(ConnectionParamValidationError {
            field_path: path,
            constraint: "app_launcher_target".to_string(),
            message,
            suggestion: Some(
                "Examples: https://{{primary_ip4}}:8443, mailto:ops@example.com, notepad, \"C:\\Program Files\\Vendor\\App.exe\" --host {{primary_ip4}}".to_string(),
            ),
        });
    }
}

/// Human-readable list of the canonical placeholder set, for error suggestions.
fn known_placeholder_suggestion() -> String {
    format!(
        "Allowed placeholders in web-link targets: {}, {}<key>. \
         Local commands accept only {{{{primary_ip4}}}}, {{{{primary_ip6}}}}.",
        KNOWN_TARGET_VARIABLE_PATHS
            .iter()
            .map(|path| format!("{{{{{path}}}}}"))
            .collect::<Vec<_>>()
            .join(", "),
        format_args!("{{{{{TARGET_CUSTOM_FIELDS_PREFIX}"),
    )
}

/// Substitutes the known placeholders in an app-launcher template with safe dummy
/// values so the resulting URI/command can be validated. Rejects any placeholder
/// not allowed for the template's sink:
/// - a WEB URI accepts any placeholder in [`KNOWN_TARGET_VARIABLE_PATHS`] or
///   matching the `target.custom_fields.<ident>` prefix;
/// - a LOCAL COMMAND accepts ONLY `primary_ip4`/`primary_ip6` — `target.*` is
///   rejected because inventory text must never reach a local command.
fn app_launcher_template_for_validation(value: &str) -> Result<String, String> {
    let kind = classify_app_launcher_template(value);
    let mut output = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let placeholder_rest = &rest[start + 2..];
        let Some(end) = placeholder_rest.find("}}") else {
            return Err("APP launcher template contains an unterminated placeholder".to_string());
        };
        let placeholder = placeholder_rest[..end].trim();
        let replacement = match kind {
            AppLauncherKind::LocalCommand => match placeholder {
                "primary_ip4" => "127.0.0.1",
                "primary_ip6" => "[2001:db8::1]",
                _ if is_known_target_variable_path(placeholder) => {
                    return Err(
                        "host-variable placeholders are only allowed in web-link app targets, \
                         not local commands"
                            .to_string(),
                    );
                }
                _ => {
                    return Err(format!(
                        "Unsupported APP launcher placeholder: {placeholder}"
                    ));
                }
            },
            AppLauncherKind::WebUri => match placeholder {
                "primary_ip4" => "127.0.0.1",
                "primary_ip6" => "[2001:db8::1]",
                _ if is_known_target_variable_path(placeholder) => "x",
                _ => {
                    return Err(format!(
                        "Unsupported APP launcher placeholder: {placeholder}"
                    ));
                }
            },
        };
        output.push_str(replacement);
        rest = &placeholder_rest[end + 2..];
    }

    if rest.contains("}}") {
        return Err("APP launcher template contains an unopened placeholder".to_string());
    }
    output.push_str(rest);
    Ok(output)
}

fn validate_app_launcher_target(value: &str) -> Result<(), String> {
    let target = value.trim();
    if target.is_empty() {
        return Err("APP launcher target cannot be empty".to_string());
    }
    if target.chars().any(char::is_control) {
        return Err("APP launcher target must not contain control characters".to_string());
    }

    let Some((scheme, _rest)) = split_app_launcher_uri(target) else {
        return Ok(());
    };
    let scheme = scheme.to_ascii_lowercase();
    if is_blocked_app_uri_scheme(&scheme) {
        return Err("in-page URI schemes are not valid APP launcher targets".to_string());
    }
    if strict_app_uri_scheme(&scheme)
        || !target
            .chars()
            .any(|character| character == '"' || character.is_whitespace())
    {
        validate_app_launcher_uri(target)?;
    }

    Ok(())
}

fn validate_app_launcher_uri(value: &str) -> Result<(), String> {
    if value
        .chars()
        .any(|character| character == '"' || character.is_whitespace() || character.is_control())
    {
        return Err(
            "APP launcher URI must not contain quotes, whitespace, or control characters"
                .to_string(),
        );
    }
    if !has_valid_percent_encoding(value) {
        return Err("APP launcher URI contains invalid percent encoding".to_string());
    }
    let (scheme, rest) = split_app_launcher_uri(value)
        .ok_or_else(|| "APP launcher URI must include a scheme".to_string())?;
    let scheme = scheme.to_ascii_lowercase();
    if is_blocked_app_uri_scheme(&scheme) {
        return Err("in-page URI schemes are not valid APP launcher targets".to_string());
    }
    if matches!(scheme.as_str(), "http" | "https") {
        validate_web_launcher_uri(value)?;
    }
    if matches!(
        scheme.as_str(),
        "microsoft-edge" | "microsoft-edge-holographic"
    ) {
        validate_web_launcher_uri(rest)?;
    }
    Ok(())
}

fn split_app_launcher_uri(value: &str) -> Option<(&str, &str)> {
    if looks_like_windows_drive_path(value) {
        return None;
    }
    let (scheme, rest) = value.split_once(':')?;
    is_valid_uri_scheme(scheme).then_some((scheme, rest))
}

fn is_valid_uri_scheme(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }

    chars.all(|character| {
        character.is_ascii_alphanumeric()
            || character == '+'
            || character == '-'
            || character == '.'
    })
}

fn looks_like_windows_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn strict_app_uri_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "file" | "http" | "https" | "mailto" | "microsoft-edge" | "microsoft-edge-holographic"
    )
}

fn is_blocked_app_uri_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "about" | "blob" | "data" | "intent" | "javascript" | "vbscript"
    )
}

fn has_valid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

fn validate_web_launcher_uri(value: &str) -> Result<(), String> {
    let url = Url::parse(value)
        .map_err(|_| "APP launcher URI must contain a valid http(s) URL".to_string())?;
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err("APP launcher URI must contain http or https".to_string()),
    }
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err("APP launcher URI must include a host".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("APP launcher URI must not include credentials".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{extract_secret_field_paths, validate_connection_params};

    #[test]
    fn allows_missing_connections_envelope() {
        let payload = json!({"other": true});
        assert!(validate_connection_params(&payload).is_ok());
    }

    #[test]
    fn allows_empty_protocol_array() {
        let payload = json!({
            "piping": {
                "connections": {
                    "ssh": []
                }
            }
        });

        assert!(validate_connection_params(&payload).is_ok());
    }

    #[test]
    fn rejects_unknown_protocol() {
        let payload = json!({
            "piping": {
                "connections": {
                    "telnet": []
                }
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "unknown_protocol")
        );
    }

    #[test]
    fn rejects_non_object_connections_envelope() {
        let payload = json!({
            "piping": {
                "connections": "bad"
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert_eq!(errors[0].field_path, "piping.connections");
    }

    #[test]
    fn validates_ssh_entry_constraints() {
        let payload = json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": "",
                        "port": 70000,
                        "credentials": {
                            "username": "",
                            "password": "not-uuid",
                            "private_key": "not-uuid"
                        },
                        "unknown": true
                    }]
                }
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert!(errors.iter().any(|error| error.constraint == "non_empty"));
        assert!(errors.iter().any(|error| error.constraint == "range"));
        assert!(errors.iter().any(|error| error.constraint == "uuid"));
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "unknown_field")
        );
    }

    #[test]
    fn accepts_null_secret_fields_as_unset() {
        let payload = json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": "1.2.3.4",
                        "port": 22,
                        "credentials": {
                            "username": "root",
                            "password": null,
                            "private_key": null
                        }
                    }],
                    "rdp": [{
                        "host": "server.local",
                        "port": 3389,
                        "credentials": {
                            "username": null,
                            "password": null
                        }
                    }],
                    "vnc": [{
                        "host": "server.local",
                        "port": 5900,
                        "credentials": {
                            "password": null
                        }
                    }]
                }
            }
        });

        validate_connection_params(&payload).expect("null secret fields must be accepted");
    }

    #[test]
    fn validates_rdp_constraints() {
        let payload = json!({
            "piping": {
                "connections": {
                    "rdp": [{
                        "host": "server.local",
                        "port": 3389,
                        "options": {
                            "color_depth": 99,
                            "width": 0,
                            "height": 9999,
                            "dpi": 0,
                            "security": "bogus",
                            "ignore_cert": "yes"
                        }
                    }]
                }
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.field_path.ends_with("color_depth"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.field_path.ends_with("width"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.field_path.ends_with("height"))
        );
        assert!(errors.iter().any(|error| error.field_path.ends_with("dpi")));
        assert!(
            errors
                .iter()
                .any(|error| error.field_path.ends_with("security"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.field_path.ends_with("ignore_cert"))
        );
    }

    #[test]
    fn validates_app_launcher_target_and_rejects_web_alias() {
        let valid_payload = json!({
            "piping": {
                "connections": {
                    "app": [
                        {"url": "http://{{primary_ip4}}:631"},
                        {"url": "microsoft-edge:http://{{primary_ip4}}"},
                        {"url": "mailto:ops@example.com"},
                        {"url": "notepad"},
                        {"url": "calc.exe"},
                        {"url": "\"C:\\Program Files\\Vendor\\App.exe\" --host {{primary_ip4}}"},
                        {"url": "/opt/vendor/client --target {{primary_ip4}}"}
                    ]
                }
            }
        });
        validate_connection_params(&valid_payload).expect("launcher targets must validate");

        let payload = json!({
            "piping": {
                "connections": {
                    "app": [{"url": "javascript:alert(1)"}],
                    "web": [{}]
                }
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "app_launcher_target")
        );
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "unknown_protocol")
        );
    }

    #[test]
    fn rejects_app_launcher_bad_templates_and_invalid_strict_uris() {
        let payload = json!({
            "piping": {
                "connections": {
                    "app": [
                        {"url": "https://{{unknown}}:631"},
                        {"url": "https://example.com/a b"},
                        {"url": "microsoft-edge:file:///etc/passwd"},
                        {"url": "notepad\u{0000}"}
                    ]
                }
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "app_launcher_template")
        );
        assert!(
            errors
                .iter()
                .filter(|error| error.constraint == "app_launcher_target")
                .count()
                >= 3
        );
    }

    #[test]
    fn extracts_secret_field_paths() {
        let payload = json!({
            "piping": {
                "connections": {
                    "ssh": [{"credentials": {"password": "x", "private_key": "y"}}],
                    "rdp": [{"credentials": {"username": "x", "password": "y"}}],
                    "vnc": [{"credentials": {"password": "z"}}],
                    "app": [{"url": "https://example.com"}]
                }
            }
        });

        let mut fields = extract_secret_field_paths(&payload);
        fields.sort();

        assert_eq!(
            fields,
            vec![
                "piping.connections.rdp[0].credentials.password",
                "piping.connections.rdp[0].credentials.username",
                "piping.connections.ssh[0].credentials.password",
                "piping.connections.ssh[0].credentials.private_key",
                "piping.connections.vnc[0].credentials.password",
            ]
        );
    }

    #[test]
    fn accepts_valid_video_entry() {
        let payload = json!({
            "piping": {
                "connections": {
                    "cctv": [{
                        "host": "10.0.0.5",
                        "mgmt_port": 34567,
                        "channel": 1,
                        "family": "dvrip_sofia",
                        "credentials": {
                            "username": "admin",
                            "password": "0191b2c3-4d5e-7f80-9a1b-2c3d4e5f6071"
                        }
                    }]
                }
            }
        });

        validate_connection_params(&payload).expect("valid cctv entry must be accepted");
    }

    #[test]
    fn accepts_minimal_video_entry_with_credentials_only() {
        let payload = json!({
            "piping": {
                "connections": {
                    "cctv": [{
                        "credentials": { "username": "admin" }
                    }]
                }
            }
        });

        validate_connection_params(&payload)
            .expect("cctv entry with only required credentials must be accepted");
    }

    #[test]
    fn rejects_video_unknown_field_and_bad_port() {
        let payload = json!({
            "piping": {
                "connections": {
                    "cctv": [{
                        "host": "10.0.0.5",
                        "mgmt_port": 70000,
                        "channel": 700000,
                        "credentials": {
                            "username": "admin",
                            "token": "nope"
                        },
                        "unknown": true
                    }]
                }
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "unknown_field"
                    && error.field_path.ends_with(".unknown")),
            "unknown top-level field must be rejected: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "range" && error.field_path.ends_with("mgmt_port")),
            "out-of-range mgmt_port must be rejected: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "range" && error.field_path.ends_with("channel")),
            "out-of-range channel must be rejected: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "unknown_field"
                    && error.field_path.ends_with("credentials.token")),
            "unknown credential field must be rejected: {errors:?}"
        );
    }

    #[test]
    fn rejects_video_entry_missing_credentials() {
        let payload = json!({
            "piping": {
                "connections": {
                    "cctv": [{ "host": "10.0.0.5" }]
                }
            }
        });

        let errors = validate_connection_params(&payload).expect_err("must fail");
        assert!(
            errors.iter().any(|error| error.constraint == "required"
                && error.field_path.ends_with(".credentials")),
            "missing credentials must be rejected: {errors:?}"
        );
    }

    #[test]
    fn video_registers_credentials_password_secret() {
        let payload = json!({
            "piping": {
                "connections": {
                    "cctv": [{
                        "credentials": { "username": "admin", "password": "ref" }
                    }]
                }
            }
        });

        let fields = extract_secret_field_paths(&payload);
        assert_eq!(
            fields,
            vec!["piping.connections.cctv[0].credentials.password"]
        );
    }

    fn app_payload(url: &str) -> serde_json::Value {
        json!({
            "piping": {
                "connections": {
                    "app": [{ "url": url }]
                }
            }
        })
    }

    fn app_url_errors(url: &str) -> Vec<super::ConnectionParamValidationError> {
        match validate_connection_params(&app_payload(url)) {
            Ok(()) => Vec::new(),
            Err(errors) => errors,
        }
    }

    #[test]
    fn web_uri_url_accepts_known_target_placeholders() {
        assert!(app_url_errors("https://{{target.name}}.portal/{{target.site.slug}}").is_empty());
        assert!(app_url_errors("https://portal/{{target.custom_fields.team}}").is_empty());
        assert!(app_url_errors("https://{{primary_ip4}}:8443/app").is_empty());
    }

    #[test]
    fn web_uri_url_rejects_unknown_placeholder() {
        let errors = app_url_errors("https://portal/{{target.bogus}}");
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "app_launcher_template"),
            "unknown placeholder must be rejected: {errors:?}"
        );
    }

    #[test]
    fn local_command_url_rejects_target_placeholders() {
        let errors = app_url_errors("myapp --host {{target.name}}");
        assert!(
            errors
                .iter()
                .any(|error| error.constraint == "app_launcher_template"
                    && error.message.contains("web-link app targets")),
            "local command must reject target.*: {errors:?}"
        );
    }

    #[test]
    fn local_command_url_still_accepts_primary_ip() {
        assert!(app_url_errors("myapp --host {{primary_ip4}}").is_empty());
    }

    #[test]
    fn classifier_distinguishes_web_uri_from_local_command() {
        use super::{AppLauncherKind, classify_app_launcher_template};
        assert_eq!(
            classify_app_launcher_template("https://{{target.name}}/x"),
            AppLauncherKind::WebUri
        );
        assert_eq!(
            classify_app_launcher_template("myapp --host {{primary_ip4}}"),
            AppLauncherKind::LocalCommand
        );
    }
}
