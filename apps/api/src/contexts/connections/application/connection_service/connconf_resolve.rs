//! Phase-3 connect-path bridge: map a resolved `EffectiveConnConfig` param tree
//! onto the proxy's existing `ConnectionLaunchParams` shape.
//!
//! The connection-config resolver (`connection_config::resolve_effective_config`)
//! produces a nested param tree whose top-level keys (`host`, `port`,
//! `credentials`, `options`, `timeout_secs`, `jump`, `ui`) already match what the
//! downstream `build_proxy_payload` consumes — EXCEPT:
//!
//! * `ui.*` is presentation-only and must be stripped before launch;
//! * `jump` is a bastion chain that the SSH proxy consumes as a sibling field.
//!
//! Secret leaves are stored canonically as bare-UUID strings, the same shape the
//! proxy payload and the secret-resolution fence already expect (a field is
//! marked secret by a `secret_usage_binding` keyed by field path under the
//! `connection_config:record` consumer, not by any inline marker). This module
//! threads the resolver's per-leaf attribution (which record contributed each
//! leaf, at which dotted field path) into the connect-time fence so each secret
//! UUID can be checked against the binding of the record that ACTUALLY produced
//! it — never blanket-trusted. No SQL, no I/O.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::connection_config::{ConnConfScope, ConnConfSource, DeviceScopeIds};

/// Translate a device/VM scope ancestry into the `(scope_type, scope_id)` pairs
/// the resolver matches on, plus the implicit `global` scope. Mirrors the same
/// mapping used by the `connection_config` REST route. Pure, no SQL.
pub(super) fn build_device_scopes(scopes: &DeviceScopeIds) -> Vec<(ConnConfScope, Option<Uuid>)> {
    scopes.to_scope_pairs()
}

/// The (record, dotted field path) that contributed a particular secret-UUID
/// leaf, per the resolver's per-leaf attribution. The connect-time fence checks
/// each candidate against the `secret_usage_binding(consumer_type=
/// 'connection_config:record', consumer_id=record_id, field_key=field_key)` of
/// the record that ACTUALLY produced it — never a blanket trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SecretLeafAttribution {
    pub record_id: Uuid,
    pub field_key: String,
}

/// Outcome of mapping a resolved connection-config param tree to launch params.
pub(super) struct MappedConnConfParams {
    /// Launch-param fields in the shape `build_proxy_payload` consumes
    /// (`host`/`port`/`credentials`/`options`/`timeout_secs`/`jump`). Secret
    /// leaves are canonical bare-UUID strings.
    pub fields: Map<String, Value>,
    /// Per-secret-UUID attribution: each bare-UUID leaf maps to the set of
    /// `(record_id, field_key)` sources that contributed it. The fence resolves
    /// a UUID only if at least one of these records has a matching binding.
    pub secret_attributions: HashMap<Uuid, Vec<SecretLeafAttribution>>,
}

/// Walk `value` at `path` (resolver dotted-path convention: object keys joined by
/// `.`, array indices as numeric segments) and, for every bare-UUID string leaf,
/// record its `(record_id, field_key)` attribution drawn from `attribution`.
/// The `field_key` stored in `secret_usage_binding` for a connection-config
/// record is the SAME dotted leaf path the resolver emits (see migration
/// `0006_init_connconf_backfill.sql`: `field_key = 'credentials.<k>'`).
fn collect_secret_attributions(
    value: &Value,
    path: &str,
    attribution: &BTreeMap<String, ConnConfSource>,
    out: &mut HashMap<Uuid, Vec<SecretLeafAttribution>>,
) {
    match value {
        Value::String(raw) => {
            let Ok(id) = Uuid::parse_str(raw.trim()) else {
                return;
            };
            // `username` is never a secret field: a UUID-shaped username is a
            // literal value, not a secret reference. Refuse to treat it as a
            // resolvable secret even if a stale binding exists (finding 4b).
            if path == "username" || path.ends_with(".username") {
                return;
            }
            // A UUID leaf is resolvable only when the resolver attributed this
            // exact path to a record; that record's binding is the authority.
            if let Some(source) = attribution.get(path) {
                let entry = SecretLeafAttribution {
                    record_id: source.record_id,
                    field_key: path.to_string(),
                };
                let bucket = out.entry(id).or_default();
                if !bucket.contains(&entry) {
                    bucket.push(entry);
                }
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_secret_attributions(child, &child_path, attribution, out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let child_path = if path.is_empty() {
                    index.to_string()
                } else {
                    format!("{path}.{index}")
                };
                collect_secret_attributions(child, &child_path, attribution, out);
            }
        }
        _ => {}
    }
}

/// Map a resolved effective-config `params` object to launch-param fields.
///
/// Returns `None` when `params` is not an object (no usable config). `ui` is
/// stripped; `jump` (if present) is preserved as a sibling field for the SSH
/// proxy; bare-UUID secret references are paired with the resolver's per-leaf
/// attribution so the connect-time fence can enforce per-record bindings.
pub(super) fn map_effective_params(
    params: &Value,
    attribution: &BTreeMap<String, ConnConfSource>,
) -> Option<MappedConnConfParams> {
    let Value::Object(source) = params else {
        return None;
    };
    let mut fields: Map<String, Value> = source.clone();
    // Presentation-only; never reaches the proxy launch payload.
    fields.remove("ui");

    // Attribution is keyed on the FULL resolved-params path tree (including the
    // stripped `ui`), so walk the original `params` to preserve path alignment.
    let mut secret_attributions = HashMap::new();
    collect_secret_attributions(params, "", attribution, &mut secret_attributions);
    // The stripped `ui` subtree never reaches the proxy and must not contribute
    // resolvable secrets; drop any attributions whose only sources are ui leaves.
    secret_attributions.retain(|_, srcs| {
        srcs.retain(|s| !(s.field_key == "ui" || s.field_key.starts_with("ui.")));
        !srcs.is_empty()
    });

    Some(MappedConnConfParams {
        fields,
        secret_attributions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(record: u128) -> ConnConfSource {
        ConnConfSource {
            record_id: Uuid::from_u128(record),
            record_name: format!("rec-{record}"),
            scope_type: None,
            scope_id: None,
        }
    }

    #[test]
    fn attributes_secret_to_contributing_record_and_strips_ui() {
        let uuid = "11111111-1111-1111-1111-111111111111";
        let params = json!({
            "host": "10.0.0.1",
            "port": 2222,
            "credentials": {
                "username": "root",
                "password": uuid
            },
            "ui": { "label": "prod", "color": "#0f0" }
        });
        let mut attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        attribution.insert("credentials.password".to_string(), source(7));

        let mapped = map_effective_params(&params, &attribution).expect("mapped");
        assert_eq!(mapped.fields["host"], json!("10.0.0.1"));
        assert_eq!(mapped.fields["port"], json!(2222));
        assert_eq!(
            mapped.fields["credentials"]["password"],
            json!(uuid),
            "secret leaf is the canonical bare uuid string"
        );
        assert!(!mapped.fields.contains_key("ui"), "ui must be stripped");
        let attrs = mapped
            .secret_attributions
            .get(&Uuid::parse_str(uuid).unwrap())
            .expect("uuid attributed");
        assert_eq!(
            attrs,
            &vec![SecretLeafAttribution {
                record_id: Uuid::from_u128(7),
                field_key: "credentials.password".to_string(),
            }]
        );
    }

    #[test]
    fn unattributed_uuid_leaf_is_not_resolvable() {
        // A UUID-shaped leaf with no resolver attribution (e.g. an arbitrary
        // string field) must NOT be offered to the fence.
        let uuid = "22222222-2222-2222-2222-222222222222";
        let params = json!({ "host": "10.0.0.1", "credentials": { "username": uuid } });
        let attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        let mapped = map_effective_params(&params, &attribution).expect("mapped");
        assert!(
            mapped.secret_attributions.is_empty(),
            "no attribution -> no resolvable secret"
        );
    }

    #[test]
    fn preserves_jump_chain_field_and_attributes_hop_secret() {
        let key_uuid = "33333333-3333-3333-3333-333333333333";
        let params = json!({
            "host": "10.0.0.1",
            "jump": [ {
                "host": "10.0.0.9",
                "port": 22,
                "credentials": { "username": "jump", "private_key": key_uuid }
            } ]
        });
        let mut attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        attribution.insert("jump.0.credentials.private_key".to_string(), source(9));
        let mapped = map_effective_params(&params, &attribution).expect("mapped");
        assert!(
            mapped.fields.get("jump").is_some(),
            "jump must be preserved"
        );
        let attrs = mapped
            .secret_attributions
            .get(&Uuid::parse_str(key_uuid).unwrap())
            .expect("hop key attributed");
        assert_eq!(attrs[0].field_key, "jump.0.credentials.private_key");
        assert_eq!(attrs[0].record_id, Uuid::from_u128(9));
    }

    #[test]
    fn non_object_params_yield_none() {
        let empty: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        assert!(map_effective_params(&json!(null), &empty).is_none());
        assert!(map_effective_params(&json!("x"), &empty).is_none());
    }
}
