//! Pure effective-config resolver for the connection-config context.
//!
//! Implements the algorithm as a single pure function over already-loaded data,
//! so it is fully unit-testable without a database:
//!
//! 1. Pick the candidate assignments whose `(scope_type, scope_id)` matches the
//!    device's scope ancestry (or `global`), joined to enabled records of the
//!    requested protocol.
//! 2. For each candidate record, resolve its own `params` first by deep-merging
//!    its `base_record_id` template chain root → derived (template inheritance).
//! 3. Order candidates by `(scope_specificity, weight, record_id)` ascending,
//!    matching `config_resolver::resolve_config_candidates`.
//! 4. Deep-merge candidate params in that order (most-specific-wins), applying
//!    the `{"unset": true}` sentinel as a key deletion and tracking per-leaf
//!    attribution.
//!
//! The merge engine mirrors `inventory::application::config_resolver::merge_value`
//! (objects merge recursively, scalars/arrays replace) WITHOUT modifying that
//! module; the only addition is the typed `{"unset": true}` clear sentinel.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::connection_config::domain::{
    ConnConfAssignment, ConnConfProtocol, ConnConfRecord, ConnConfScope, ConnConfSource,
    EffectiveConnConfig,
};

/// Max depth of a `base_record_id` template chain before we bail (cycle/abuse
/// guard). Mirrors the jump-chain cap recommended in the design doc.
const MAX_BASE_DEPTH: usize = 8;

/// Errors the pure resolver can raise. Kept local to the application layer; the
/// (later) service maps these onto `AppError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolverError {
    /// A `base_record_id` chain formed a cycle or exceeded `MAX_BASE_DEPTH`.
    BaseChainCycle(Uuid),
    /// A `base_record_id` pointed at a record not present in the input set.
    MissingBaseRecord(Uuid),
    /// A base record's protocol differs from the deriving record's protocol.
    BaseProtocolMismatch {
        record_id: Uuid,
        base_record_id: Uuid,
    },
}

impl std::fmt::Display for ResolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BaseChainCycle(id) => {
                write!(
                    f,
                    "connconf base_record_id chain cycles or is too deep at {id}"
                )
            }
            Self::MissingBaseRecord(id) => {
                write!(f, "connconf base_record_id {id} not found in candidate set")
            }
            Self::BaseProtocolMismatch {
                record_id,
                base_record_id,
            } => write!(
                f,
                "connconf record {record_id} base {base_record_id} has a different protocol"
            ),
        }
    }
}

impl std::error::Error for ResolverError {}

/// Pure inputs for [`resolve_effective_config`]. The caller (repository/service)
/// is responsible for the DB round-trips; the resolver itself touches no SQL.
pub struct ResolveInput<'a> {
    /// The protocol being resolved.
    pub protocol: ConnConfProtocol,
    /// `(scope_type, scope_id)` ancestry for the target device/VM, plus the
    /// implicit `global` scope. Order is irrelevant; the resolver sorts.
    pub device_scopes: &'a [(ConnConfScope, Option<Uuid>)],
    /// Enabled assignments matching the device scopes (already scope-filtered by
    /// the repository; the resolver re-checks against `device_scopes` defensively).
    pub assignments: &'a [ConnConfAssignment],
    /// Every record reachable from the candidate assignments AND their full
    /// `base_record_id` template chains. Keyed lookups are by `id`.
    pub records: &'a [ConnConfRecord],
}

/// Resolve the effective connection config for a device + protocol.
///
/// See the module docs for the algorithm. Returns the deep-merged param tree
/// with `{"unset": true}` deletions applied, per-leaf attribution, and the
/// ordered resolution chain (least → most specific).
pub fn resolve_effective_config(
    input: &ResolveInput<'_>,
) -> Result<EffectiveConnConfig, ResolverError> {
    let by_id: HashMap<Uuid, &ConnConfRecord> = input.records.iter().map(|r| (r.id, r)).collect();

    let scope_set: HashSet<(ConnConfScope, Option<Uuid>)> =
        input.device_scopes.iter().copied().collect();

    // (1) Pick candidate assignments: enabled, matching a device scope (or
    // global), whose record exists, is enabled, and matches the protocol.
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for assignment in input.assignments {
        if !assignment.enabled {
            continue;
        }
        let matches_scope = assignment.scope_type == ConnConfScope::Global
            || scope_set.contains(&(assignment.scope_type, assignment.scope_id));
        if !matches_scope {
            continue;
        }
        let Some(record) = by_id.get(&assignment.record_id) else {
            continue;
        };
        if !record.enabled || record.protocol != input.protocol {
            continue;
        }
        candidates.push(Candidate {
            record,
            scope_type: assignment.scope_type,
            scope_id: assignment.scope_id,
            weight: assignment.weight,
        });
    }

    // (3) Order by (specificity, weight, record.id) ascending so the most
    // specific candidate is merged last and wins.
    candidates.sort_by(|a, b| {
        (a.scope_type.specificity(), a.weight, a.record.id).cmp(&(
            b.scope_type.specificity(),
            b.weight,
            b.record.id,
        ))
    });

    let mut merged = Value::Object(Map::new());
    let mut attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
    let mut resolution_chain: Vec<Uuid> = Vec::with_capacity(candidates.len());

    for candidate in &candidates {
        // (2) Resolve this record's own params via its template chain first.
        let resolved = resolve_record_params(candidate.record, &by_id)?;
        let source = ConnConfSource {
            record_id: candidate.record.id,
            record_name: candidate.record.name.clone(),
            scope_type: Some(candidate.scope_type),
            scope_id: candidate.scope_id,
        };
        // (4) Deep-merge most-specific-wins with attribution + unset deletion.
        merge_value_annotated(&mut merged, &resolved, "", &source, &mut attribution);
        resolution_chain.push(candidate.record.id);
    }

    Ok(EffectiveConnConfig {
        protocol: input.protocol,
        params: merged,
        attribution,
        resolution_chain,
    })
}

struct Candidate<'a> {
    record: &'a ConnConfRecord,
    scope_type: ConnConfScope,
    scope_id: Option<Uuid>,
    weight: i32,
}

/// Resolve a single record's effective `params` by deep-merging its
/// `base_record_id` chain root → derived (template inheritance). The base's
/// values are the floor; the deriving record overlays on top, and a
/// `{"unset": true}` in the derived record clears a base value.
///
/// Exposed to sibling resolvers in this application module (e.g. the
/// non-merging `launch_accounts_resolver`) so they can resolve each candidate's
/// OWN template-chained params with the exact same semantics as the merging
/// effective-config resolver — without duplicating the chain-walk logic.
pub(super) fn resolve_record_params(
    record: &ConnConfRecord,
    by_id: &HashMap<Uuid, &ConnConfRecord>,
) -> Result<Value, ResolverError> {
    // Walk the base chain leaf → root collecting records, guarding cycles/depth.
    let mut chain: Vec<&ConnConfRecord> = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut current = record;
    loop {
        if !seen.insert(current.id) {
            return Err(ResolverError::BaseChainCycle(current.id));
        }
        if chain.len() >= MAX_BASE_DEPTH {
            return Err(ResolverError::BaseChainCycle(current.id));
        }
        chain.push(current);
        let Some(base_id) = current.base_record_id else {
            break;
        };
        let Some(base) = by_id.get(&base_id) else {
            return Err(ResolverError::MissingBaseRecord(base_id));
        };
        if base.protocol != record.protocol {
            return Err(ResolverError::BaseProtocolMismatch {
                record_id: current.id,
                base_record_id: base_id,
            });
        }
        current = base;
    }

    // Merge root → derived (reverse of the leaf-first collection order). No
    // attribution needed here; template provenance is folded into the record's
    // own attribution when it later participates in scope merge.
    let mut merged = Value::Object(Map::new());
    for record in chain.iter().rev() {
        merge_value_plain(&mut merged, &record.params);
    }
    Ok(merged)
}

/// Deep-merge `incoming` into `target`, most-specific-wins. Objects merge
/// recursively; scalars/arrays replace. The `{"unset": true}` sentinel deletes
/// the key from `target`. Mirrors `config_resolver::merge_value` plus the
/// typed clear sentinel.
fn merge_value_plain(target: &mut Value, incoming: &Value) {
    if let (Value::Object(target_map), Value::Object(incoming_map)) = (&mut *target, incoming) {
        for (key, value) in incoming_map {
            if is_unset_sentinel(value) {
                target_map.remove(key);
                continue;
            }
            match target_map.get_mut(key) {
                Some(existing) => merge_value_plain(existing, value),
                None => {
                    target_map.insert(key.clone(), value.clone());
                }
            }
        }
        return;
    }
    // Replacing a non-object target with an unset sentinel clears it to null;
    // at the top level this is a no-op since the root is always an object.
    if !is_unset_sentinel(incoming) {
        *target = incoming.clone();
    }
}

/// Annotated variant: same merge semantics, but records per-leaf attribution and
/// honours `{"unset": true}` by deleting the path (and its stale attribution).
///
/// Exposed to sibling resolvers in this application module so the non-merging
/// account resolver can overlay each account record onto the shared-defaults
/// tree with identical attribution bookkeeping (critical for the per-record
/// secret fence).
pub(super) fn merge_value_annotated(
    target: &mut Value,
    incoming: &Value,
    current_path: &str,
    source: &ConnConfSource,
    attribution: &mut BTreeMap<String, ConnConfSource>,
) {
    if let (Value::Object(target_map), Value::Object(incoming_map)) = (&mut *target, incoming) {
        for (key, value) in incoming_map {
            let child_path = join_path(current_path, key);
            if is_unset_sentinel(value) {
                target_map.remove(key);
                clear_attribution_subtree(&child_path, attribution);
                continue;
            }
            match target_map.get_mut(key) {
                Some(existing) => {
                    merge_value_annotated(existing, value, &child_path, source, attribution);
                }
                None => {
                    target_map.insert(key.clone(), value.clone());
                    assign_attribution_tree(&child_path, value, source, attribution);
                }
            }
        }
        return;
    }
    *target = incoming.clone();
    assign_attribution_tree(current_path, incoming, source, attribution);
}

/// True iff `value` is the typed clear sentinel `{"unset": true}` (and nothing
/// else): a single-key object `unset` = JSON `true`.
fn is_unset_sentinel(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == 1 && map.get("unset") == Some(&Value::Bool(true)))
}

fn join_path(base: &str, segment: &str) -> String {
    if base.is_empty() {
        segment.to_string()
    } else {
        format!("{base}.{segment}")
    }
}

fn assign_attribution_tree(
    path: &str,
    value: &Value,
    source: &ConnConfSource,
    attribution: &mut BTreeMap<String, ConnConfSource>,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = join_path(path, key);
                assign_attribution_tree(&child_path, child, source, attribution);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let child_path = join_path(path, &index.to_string());
                assign_attribution_tree(&child_path, child, source, attribution);
            }
        }
        _ => {
            if !path.is_empty() {
                attribution.insert(path.to_string(), source.clone());
            }
        }
    }
}

/// Remove attribution entries for `path` and any deeper leaves beneath it.
fn clear_attribution_subtree(path: &str, attribution: &mut BTreeMap<String, ConnConfSource>) {
    let prefix = format!("{path}.");
    attribution.retain(|key, _| key != path && !key.starts_with(&prefix));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn record(id: u128, name: &str, base: Option<u128>, params: Value) -> ConnConfRecord {
        ConnConfRecord {
            id: Uuid::from_u128(id),
            name: name.to_string(),
            description: String::new(),
            protocol: ConnConfProtocol::Ssh,
            base_record_id: base.map(Uuid::from_u128),
            enabled: true,
            owner_user_id: None,
            params,
            custom_field_data: Value::Object(Map::new()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn assignment(
        record_id: u128,
        scope: ConnConfScope,
        scope_id: Option<u128>,
        weight: i32,
    ) -> ConnConfAssignment {
        ConnConfAssignment {
            id: Uuid::from_u128(record_id + 9_000_000),
            record_id: Uuid::from_u128(record_id),
            scope_type: scope,
            scope_id: scope_id.map(Uuid::from_u128),
            weight,
            enabled: true,
            created_at: Utc::now(),
        }
    }

    const REGION: u128 = 100;
    const DEVICE: u128 = 200;

    fn device_scopes() -> Vec<(ConnConfScope, Option<Uuid>)> {
        vec![
            (ConnConfScope::Global, None),
            (ConnConfScope::Region, Some(Uuid::from_u128(REGION))),
            (ConnConfScope::Device, Some(Uuid::from_u128(DEVICE))),
        ]
    }

    // (a) Scope precedence: a value set at region is overridden at device.
    #[test]
    fn device_scope_overrides_region() {
        let region_rec = record(1, "region", None, json!({ "port": 22, "timeout_secs": 30 }));
        let device_rec = record(2, "device", None, json!({ "port": 2222 }));
        let records = vec![region_rec, device_rec];
        let assignments = vec![
            assignment(1, ConnConfScope::Region, Some(REGION), 0),
            assignment(2, ConnConfScope::Device, Some(DEVICE), 0),
        ];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        let effective = resolve_effective_config(&input).expect("resolve");
        // device wins for port, region's timeout survives (not overridden).
        assert_eq!(effective.params["port"], json!(2222));
        assert_eq!(effective.params["timeout_secs"], json!(30));
        assert_eq!(effective.attribution["port"].record_id, Uuid::from_u128(2));
        assert_eq!(
            effective.attribution["timeout_secs"].record_id,
            Uuid::from_u128(1)
        );
        assert_eq!(
            effective.resolution_chain,
            vec![Uuid::from_u128(1), Uuid::from_u128(2)]
        );
    }

    // (b) `{"unset": true}` at a deeper scope deletes an inherited key.
    #[test]
    fn unset_sentinel_deletes_inherited_key() {
        let region_rec = record(
            1,
            "region",
            None,
            json!({ "credentials": { "username": "root", "password": "p" } }),
        );
        // device clears the inherited password but keeps username.
        let device_rec = record(
            2,
            "device",
            None,
            json!({ "credentials": { "password": { "unset": true } } }),
        );
        let records = vec![region_rec, device_rec];
        let assignments = vec![
            assignment(1, ConnConfScope::Region, Some(REGION), 0),
            assignment(2, ConnConfScope::Device, Some(DEVICE), 0),
        ];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        let effective = resolve_effective_config(&input).expect("resolve");
        assert_eq!(effective.params["credentials"]["username"], json!("root"));
        assert!(
            effective.params["credentials"].get("password").is_none(),
            "password should be deleted by the unset sentinel"
        );
        assert!(
            !effective.attribution.contains_key("credentials.password"),
            "stale attribution for the deleted leaf must be cleared"
        );
    }

    // (c) `base_record_id` template overlay: base supplies defaults, the
    // deriving record overrides selectively, then scope merge applies on top.
    #[test]
    fn base_record_template_overlay() {
        // base "corp-ssh": port + username defaults.
        let base = record(
            10,
            "corp-ssh",
            None,
            json!({ "port": 22, "credentials": { "username": "corp" } }),
        );
        // derived "prod-ssh" extends base, overrides username only.
        let derived = record(
            11,
            "prod-ssh",
            Some(10),
            json!({ "credentials": { "username": "prod" } }),
        );
        let records = vec![base, derived];
        // Only the derived record is assigned to a scope; the base is pulled in
        // purely as a template.
        let assignments = vec![assignment(11, ConnConfScope::Device, Some(DEVICE), 0)];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        let effective = resolve_effective_config(&input).expect("resolve");
        // port inherited from base, username overridden by derived.
        assert_eq!(effective.params["port"], json!(22));
        assert_eq!(effective.params["credentials"]["username"], json!("prod"));
        // attribution credits the participating (derived) record for both leaves.
        assert_eq!(effective.attribution["port"].record_id, Uuid::from_u128(11));
        assert_eq!(
            effective.attribution["credentials.username"].record_id,
            Uuid::from_u128(11)
        );
    }

    #[test]
    fn base_chain_cycle_is_rejected() {
        let a = record(1, "a", Some(2), json!({}));
        let b = record(2, "b", Some(1), json!({}));
        let records = vec![a, b];
        let assignments = vec![assignment(1, ConnConfScope::Device, Some(DEVICE), 0)];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        assert!(matches!(
            resolve_effective_config(&input),
            Err(ResolverError::BaseChainCycle(_))
        ));
    }
}
