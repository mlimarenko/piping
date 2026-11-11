//! Allowlisted audit read projection and its SQL search contract.
use serde_json::{Map, Value, json};

use super::{
    AuditContext, AuditDecision, AuditDelegatedPrincipalKind, AuditEmitterKind, AuditEvent,
    AuditOutcome,
    event_context::{serialized_lifecycle_states, serialized_values},
};

/// Public protocol, bounded counts and allowlisted lifecycle facts only.
pub fn project_metadata(event_type: &str, metadata: &Value) -> Value {
    let mut projected = Map::new();
    let mut omitted = !metadata.is_null();
    if let Some(fields) = metadata.as_object() {
        omitted = false;
        for (key, value) in fields {
            if key == OMITTED_MARKER {
                // Write paths persist this projection, so the read path sees
                // its own marker. Only an explicit boolean is meaningful;
                // anything else stays hidden rather than becoming visible data.
                omitted |= !matches!(value, Value::Bool(false));
                continue;
            }
            match (key.as_str(), value.as_str()) {
                ("audit_context", _) => {}
                ("protocol", Some("ssh" | "rdp" | "vnc")) => {
                    projected.insert(key.clone(), value.clone());
                }
                ("resolved_count", _)
                    if event_type == "secrets.resolve"
                        && value.as_u64().is_some_and(|count| count <= 10_000) =>
                {
                    projected.insert(key.clone(), value.clone());
                }
                ("reason", _) if event_type.starts_with("connection.session.") => {
                    match enum_value(
                        value,
                        &[
                            "client_closed",
                            "detached_alive",
                            "proxy_closed",
                            "proxy_internal_error",
                            "proxy_policy_violation",
                            "proxy_try_again_later",
                            "client_bridge_error",
                            "proxy_bridge_error",
                            "runtime_ended",
                            "user_disconnect",
                            "user_terminate",
                            "automatic_cleanup",
                        ],
                    ) {
                        Some(value) => {
                            projected.insert(key.clone(), value);
                        }
                        None => omitted = true,
                    }
                }
                ("session_owner_id" | "target_id" | "token_id", _)
                    if match key.as_str() {
                        "token_id" => {
                            matches!(event_type, "mcp.tool.invoked" | "mcp.resource.read")
                        }
                        _ => event_type.starts_with("connection.session."),
                    } =>
                {
                    if value
                        .as_str()
                        .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok())
                    {
                        projected.insert(key.clone(), value.clone());
                    } else {
                        omitted = true;
                    }
                }
                (key, _) if event_type.starts_with("playbook.target_set.") => {
                    match target_set_field(key, value) {
                        Some(value) => {
                            projected.insert(key.to_string(), value);
                        }
                        None => omitted = true,
                    }
                }
                _ => omitted = true,
            }
        }
    }
    projected.insert(OMITTED_MARKER.into(), json!(omitted));
    Value::Object(projected)
}

/// The projection's own marker: `true` means at least one stored field was
/// dropped before the value reached a reader.
const OMITTED_MARKER: &str = "_omitted";

/// Allowlisted audit fields for target-set mutations. Only bounded identifiers
/// and selector KEY names are accepted; selector values are never projected.
fn target_set_field(key: &str, value: &Value) -> Option<Value> {
    match key {
        "action" => enum_value(value, &["create", "update", "delete"]),
        "mode" => enum_value(value, &["static", "dynamic"]),
        "changed_selector_keys" | "selector_keys" => selector_key_list(value),
        "mode_changed" | "name_changed" => value.as_bool().map(Value::Bool),
        _ => None,
    }
}

fn enum_value(value: &Value, allowed: &[&str]) -> Option<Value> {
    let raw = value.as_str()?;
    allowed
        .contains(&raw)
        .then(|| Value::String(raw.to_string()))
}

/// At most 64 key names, each a non-empty string of at most 64 bytes without
/// control characters. Values are deliberately not representable here.
const MAX_AUDIT_SELECTOR_KEYS: usize = 64;
const MAX_AUDIT_SELECTOR_KEY_BYTES: usize = 64;

fn selector_key_list(value: &Value) -> Option<Value> {
    let items = value.as_array()?;
    if items.len() > MAX_AUDIT_SELECTOR_KEYS {
        return None;
    }
    let mut keys = Vec::with_capacity(items.len());
    for item in items {
        let key = item.as_str()?;
        if key.is_empty()
            || key.len() > MAX_AUDIT_SELECTOR_KEY_BYTES
            || key.chars().any(char::is_control)
        {
            return None;
        }
        keys.push(Value::String(key.to_string()));
    }
    Some(Value::Array(keys))
}

/// The SQL predicate accepts only the canonical serialized context. Rejecting
/// noncanonical historical values here keeps list, count, sorting and detail in
/// agreement, without trusting raw JSON for filter matches.
pub fn project_context(event: &AuditEvent) -> AuditContext {
    event.audit_context()
}

pub const SEARCHABLE_IDENTITY_EXPRESSIONS: &[&str] = &[
    "event_type",
    "id::text",
    "COALESCE(target_type, '')",
    "COALESCE(actor_id::text, '')",
    "COALESCE(target_id::text, '')",
];

/// Search only display labels accepted by the same canonical context projection.
/// NULL labels and rejected contexts have no searchable display text.
pub fn searchable_display_sql() -> [String; 2] {
    let canonical = canonical_context_sql();
    ["actor", "target"].map(|field| {
        format!("(CASE WHEN {canonical} THEN metadata #>> '{{audit_context,display,{field}}}' ELSE NULL END)")
    })
}

fn object(expr: &str, keys: &[&str], inner: &str) -> String {
    let keys = keys
        .iter()
        .map(|key| format!("'{key}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "(CASE WHEN jsonb_typeof({expr}) = 'object' THEN ({expr} ?& ARRAY[{keys}] AND {expr} - ARRAY[{keys}] = '{{}}'::jsonb AND ({inner})) ELSE false END)"
    )
}
fn nullable(expr: &str, inner: String) -> String {
    format!("({expr} = 'null'::jsonb OR ({inner}))")
}
fn enumeration(expr: &str, values: &[String]) -> String {
    let values = values
        .iter()
        .map(|v| {
            format!(
                "'{}'::jsonb",
                serde_json::to_string(v)
                    .expect("audit enum serialization is infallible")
                    .replace('\'', "''")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("({expr} IN ({values}))")
}
fn uuid(expr: &str) -> String {
    format!(
        "(jsonb_typeof({expr}) = 'string' AND {expr} #>> '{{}}' ~ '^[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}$')"
    )
}

/// SQL predicate equivalent to [`project_context`]'s canonical acceptance.
/// All identifiers and values are fixed source literals, never request input.
pub fn canonical_context_sql() -> String {
    let c = "(metadata -> 'audit_context')";
    let field = |name: &str| format!("({c} -> '{name}')");
    let state = field("state");
    let sd = format!("({state} -> 'domain')");
    let sv = format!("({state} -> 'value')");
    let state_cases = serialized_lifecycle_states()
        .into_iter()
        .map(|(domain, values)| {
            format!(
                "({} AND {})",
                enumeration(&sd, std::slice::from_ref(&domain)),
                enumeration(&sv, std::slice::from_ref(&values))
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    let state_valid = nullable(&state, object(&state, &["domain", "value"], &state_cases));
    let principal = field("delegated_principal");
    let principal_valid = nullable(
        &principal,
        object(
            &principal,
            &["kind", "id"],
            &format!(
                "{} AND {}",
                enumeration(
                    &format!("({principal} -> 'kind')"),
                    &serialized_values::<AuditDelegatedPrincipalKind>()
                ),
                uuid(&format!("({principal} -> 'id')"))
            ),
        ),
    );
    let emitter = field("emitter");
    let eid = format!("({emitter} -> 'id')");
    let emitter_valid = nullable(
        &emitter,
        object(
            &emitter,
            &["kind", "id"],
            &format!(
                "{} AND {}",
                enumeration(
                    &format!("({emitter} -> 'kind')"),
                    &serialized_values::<AuditEmitterKind>()
                ),
                nullable(&eid, uuid(&eid))
            ),
        ),
    );
    let correlation = field("correlation");
    let correlation_keys = [
        "request_id",
        "session_id",
        "run_id",
        "run_group_id",
        "causation_id",
    ];
    let correlations = correlation_keys
        .iter()
        .map(|key| {
            let value = format!("({correlation} -> '{key}')");
            nullable(&value, uuid(&value))
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let display = field("display");
    let labels = ["actor", "target"].iter().map(|key| {
        let value = format!("({display} -> '{key}')");
        nullable(&value, format!("(jsonb_typeof({value}) = 'string' AND octet_length({value} #>> '{{}}') BETWEEN 1 AND 256 AND ({value} #>> '{{}}') !~ U&'[\\0001-\\001F\\007F-\\009F]')"))
    }).collect::<Vec<_>>().join(" AND ");
    let decision = field("decision");
    let checks = [
        enumeration(&field("outcome"), &serialized_values::<AuditOutcome>()),
        nullable(
            &decision,
            enumeration(&decision, &serialized_values::<AuditDecision>()),
        ),
        state_valid,
        principal_valid,
        emitter_valid,
        object(&correlation, &correlation_keys, &correlations),
        object(&display, &["actor", "target"], &labels),
    ]
    .join(" AND ");
    object(
        c,
        &[
            "outcome",
            "decision",
            "state",
            "delegated_principal",
            "emitter",
            "correlation",
            "display",
        ],
        &checks,
    )
}

/// SQL expression for the projected outcome, including unknown historical rows.
pub fn outcome_sql() -> String {
    let unknown = serde_json::to_value(AuditOutcome::default())
        .expect("audit outcome serialization is infallible");
    let unknown = unknown
        .as_str()
        .expect("audit outcome must serialize as a string")
        .replace('\'', "''");
    format!(
        "(CASE WHEN {} THEN metadata #>> '{{audit_context,outcome}}' ELSE '{}' END)",
        canonical_context_sql(),
        unknown
    )
}

#[cfg(test)]
mod tests {
    use super::project_metadata;
    use serde_json::json;
    #[test]
    fn projection_keeps_only_valid_protocol_and_never_nested_secrets() {
        assert_eq!(
            project_metadata(
                "other",
                &json!({"protocol":"ssh", "details":{"screen":"screen-canary", "keystroke":"key-canary"}, "credential":"credential-canary"})
            ),
            json!({"protocol":"ssh", "_omitted":true})
        );
        assert_eq!(
            project_metadata("other", &json!({"protocol":{"credential":"canary"}})),
            json!({"_omitted":true})
        );
        assert_eq!(
            project_metadata(
                "secrets.resolve",
                &json!({"resolved_count":3, "keys":["canary"]})
            ),
            json!({"resolved_count":3, "_omitted":true})
        );
        assert_eq!(
            project_metadata("other", &json!({"resolved_count":3})),
            json!({"_omitted":true})
        );
        assert_eq!(
            project_metadata("secrets.resolve", &json!({"resolved_count":10001})),
            json!({"_omitted":true})
        );
    }

    /// Write paths persist the projection, so the read path projects it again.
    /// Its own marker must round-trip instead of becoming a hidden field.
    #[test]
    fn reprojecting_stored_projection_is_idempotent() {
        for (event_type, metadata) in [
            ("other", json!({"protocol": "ssh", "_omitted": false})),
            (
                "secrets.resolve",
                json!({"resolved_count": 3, "_omitted": false}),
            ),
            (
                "playbook.target_set.created",
                json!({
                    "action": "create",
                    "mode": "static",
                    "selector_keys": ["tag_ids"],
                    "_omitted": false
                }),
            ),
            ("other", json!({"_omitted": true})),
            (
                "secrets.resolve",
                json!({"resolved_count": 3, "_omitted": true}),
            ),
        ] {
            assert_eq!(
                project_metadata(event_type, &metadata),
                metadata,
                "{event_type}"
            );
        }
    }

    #[test]
    fn malformed_omission_marker_never_claims_visible_metadata() {
        for marker in [json!("false"), json!(0), json!(null), json!({})] {
            assert_eq!(
                project_metadata("other", &json!({"protocol": "ssh", "_omitted": marker})),
                json!({"protocol": "ssh", "_omitted": true}),
                "{marker}"
            );
        }
    }

    #[test]
    fn transport_details_keep_only_known_reasons_and_identity_ids() {
        let owner = uuid::Uuid::now_v7().to_string();
        assert_eq!(
            project_metadata(
                "connection.session.websocket.disconnected",
                &json!({
                    "reason": "client_closed", "session_owner_id": owner, "target_id": "credential-canary",
                    "screen": "screen-canary",
                })
            ),
            json!({"reason": "client_closed", "session_owner_id": owner, "_omitted": true})
        );
        assert_eq!(
            project_metadata(
                "connection.session.failed",
                &json!({"reason":"credential-canary"})
            ),
            json!({"_omitted":true})
        );
        assert_eq!(
            project_metadata(
                "unrelated",
                &json!({"reason":"client_closed", "session_owner_id":owner})
            ),
            json!({"_omitted":true})
        );
    }

    #[test]
    fn mcp_token_identity_remains_inspectable_without_credential_material() {
        let id = uuid::Uuid::now_v7().to_string();
        for event_type in ["mcp.tool.invoked", "mcp.resource.read"] {
            assert_eq!(
                project_metadata(
                    event_type,
                    &json!({
                        "token_id": id, "token_secret": "credential-canary", "token_hash": "hash-canary",
                    })
                ),
                json!({"token_id":id, "_omitted":true})
            );
        }
        assert_eq!(
            project_metadata("mcp.tool.invoked", &json!({"token_id":"credential-canary"})),
            json!({"_omitted":true})
        );
        assert_eq!(
            project_metadata("unrelated", &json!({"token_id":id})),
            json!({"_omitted":true})
        );
    }

    #[test]
    fn reprojection_never_revives_dropped_secret_fields() {
        let stored = project_metadata(
            "other",
            &json!({"protocol": "ssh", "credential": "credential-canary"}),
        );
        assert_eq!(stored, json!({"protocol": "ssh", "_omitted": true}));
        assert_eq!(
            project_metadata(
                "other",
                &json!({"protocol": "ssh", "_omitted": false, "credential": "credential-canary"})
            ),
            json!({"protocol": "ssh", "_omitted": true})
        );
    }

    #[test]
    fn target_set_projection_keeps_key_names_but_never_selector_values() {
        assert_eq!(
            project_metadata(
                "playbook.target_set.updated",
                &json!({
                    "action": "update",
                    "mode": "dynamic",
                    "changed_selector_keys": ["site", "tenant"],
                    "mode_changed": false,
                    "name_changed": true,
                    "selector": {"site": "secret-looking-value"}
                })
            ),
            json!({
                "action": "update",
                "mode": "dynamic",
                "changed_selector_keys": ["site", "tenant"],
                "mode_changed": false,
                "name_changed": true,
                "_omitted": true
            })
        );
        assert_eq!(
            project_metadata(
                "playbook.target_set.created",
                &json!({"action": "create", "mode": "static", "selector_keys": ["tag_ids"]})
            ),
            json!({
                "action": "create",
                "mode": "static",
                "selector_keys": ["tag_ids"],
                "_omitted": false
            })
        );
        assert_eq!(
            project_metadata(
                "playbook.target_set.updated",
                &json!({"changed_selector_keys": ["bad\nkey"]})
            ),
            json!({"_omitted": true})
        );
    }
}
