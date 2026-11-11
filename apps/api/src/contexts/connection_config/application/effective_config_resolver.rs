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
        merge_resolved_record(
            &mut merged,
            &mut attribution,
            &resolved,
            Some(candidate.scope_type),
            candidate.scope_id,
        );
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

/// Inherited params, their defining records, and the traversed template chain.
#[derive(Debug, Clone)]
pub struct ResolvedRecordParams {
    /// Base values overlaid by each deriving record, with unset fields removed.
    pub params: Value,
    /// The record that supplied each winning leaf, keyed by its dotted path.
    pub attribution: BTreeMap<String, ConnConfSource>,
    /// Record IDs in leaf-to-root order.
    pub chain: Vec<Uuid>,
    // Ordered original patches retain deletion intent when applied over a scope floor.
    layers: Vec<(Value, ConnConfSource)>,
}

/// Resolve one record through the canonical root-to-derived inheritance merge.
/// Callers must authorize every record in `chain` before using the result.
///
/// # Errors
/// Returns an error for a missing base, protocol mismatch, cycle, or excessive depth.
pub fn resolve_record_params(
    record: &ConnConfRecord,
    by_id: &HashMap<Uuid, &ConnConfRecord>,
) -> Result<ResolvedRecordParams, ResolverError> {
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

    let mut merged = Value::Object(Map::new());
    let mut attribution = BTreeMap::new();
    let mut layers = Vec::with_capacity(chain.len());
    for record in chain.iter().rev() {
        let source = ConnConfSource {
            record_id: record.id,
            record_name: record.name.clone(),
            scope_type: None,
            scope_id: None,
        };
        merge_value_annotated(&mut merged, &record.params, "", &source, &mut attribution);
        layers.push((record.params.clone(), source));
    }
    Ok(ResolvedRecordParams {
        params: merged,
        attribution,
        chain: chain.iter().map(|record| record.id).collect(),
        layers,
    })
}

pub(super) fn merge_resolved_record(
    target: &mut Value,
    attribution: &mut BTreeMap<String, ConnConfSource>,
    resolved: &ResolvedRecordParams,
    scope_type: Option<ConnConfScope>,
    scope_id: Option<Uuid>,
) {
    for (patch, source) in &resolved.layers {
        let mut source = source.clone();
        source.scope_type = scope_type;
        source.scope_id = scope_id;
        merge_value_annotated(target, patch, "", &source, attribution);
    }
}

/// Annotated variant: same merge semantics, but records per-leaf attribution and
/// honours `{"unset": true}` by deleting the path (and its stale attribution).
///
/// Exposed to sibling resolvers in this application module so the non-merging
/// account resolver can overlay each account record onto the shared-defaults
/// tree with identical attribution bookkeeping (critical for the per-record
/// secret fence).
fn merge_value_annotated(
    target: &mut Value,
    incoming: &Value,
    current_path: &str,
    source: &ConnConfSource,
    attribution: &mut BTreeMap<String, ConnConfSource>,
) {
    if incoming.is_object() && !target.is_object() {
        clear_attribution_subtree(current_path, attribution);
        *target = Value::Object(Map::new());
    }
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
                    if value.is_object() {
                        let mut child = Value::Object(Map::new());
                        merge_value_annotated(&mut child, value, &child_path, source, attribution);
                        target_map.insert(key.clone(), child);
                    } else {
                        target_map.insert(key.clone(), value.clone());
                        assign_attribution_tree(&child_path, value, source, attribution);
                    }
                }
            }
        }
        return;
    }
    clear_attribution_subtree(current_path, attribution);
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

    #[test]
    fn inherited_new_and_replaced_objects_clear_nested_unsets() {
        for base_credentials in [None, Some(json!("not-an-object"))] {
            let mut base = record(10, "base", None, json!({"host": "192.0.2.1"}));
            if let Some(value) = base_credentials {
                base.params["credentials"] = value;
            }
            let child = record(
                11,
                "child",
                Some(10),
                json!({"credentials": {
                    "username": "operator", "private_key": "key", "password": {"unset": true}
                }}),
            );
            let records = [base, child];
            let by_id = records.iter().map(|record| (record.id, record)).collect();
            let resolved = resolve_record_params(&records[1], &by_id).unwrap();
            assert_eq!(resolved.params["credentials"]["private_key"], json!("key"));
            assert!(resolved.params["credentials"].get("password").is_none());
            assert_eq!(
                resolved.attribution["credentials.private_key"].record_id,
                records[1].id
            );
            assert!(
                !resolved
                    .attribution
                    .keys()
                    .any(|path| path.starts_with("credentials.password"))
            );
        }
    }

    #[test]
    fn inherited_unset_clears_scope_floor_for_effective_and_launch_accounts() {
        let floor = record(
            1,
            "floor",
            None,
            json!({"credentials": {"password": "floor-secret"}}),
        );
        let base = record(10, "base", None, json!({"host": "192.0.2.1"}));
        let child = record(
            11,
            "child",
            Some(10),
            json!({"credentials": {
                "username": "operator", "private_key": "child-key", "password": {"unset": true}
            }}),
        );
        let records = [floor, base, child];
        let assignments = [
            assignment(1, ConnConfScope::Global, None, 0),
            assignment(11, ConnConfScope::Device, Some(DEVICE), 0),
        ];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };
        let effective = resolve_effective_config(&input).unwrap();
        assert!(effective.params["credentials"].get("password").is_none());
        assert!(!effective.attribution.contains_key("credentials.password"));
        assert_eq!(
            effective.attribution["credentials.private_key"].record_id,
            records[2].id
        );
        let accounts =
            super::super::launch_accounts_resolver::resolve_launch_accounts(&input).unwrap();
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].params["credentials"].get("password").is_none());
        assert!(!accounts[0].attribution.contains_key("credentials.password"));
        assert_eq!(
            accounts[0].attribution["credentials.private_key"].record_id,
            records[2].id
        );
    }

    #[test]
    fn inherited_secret_attribution_survives_record_effective_and_launch_resolution() {
        let base = record(
            10,
            "base",
            None,
            json!({"credentials": {"username": "root", "private_key": "base-key", "password": "base-password"}}),
        );
        let child = record(
            11,
            "child",
            Some(10),
            json!({"credentials": {"username": "child", "password": {"unset": true}}}),
        );
        let records = vec![base, child];
        let by_id = records.iter().map(|record| (record.id, record)).collect();
        let resolved = resolve_record_params(&records[1], &by_id).unwrap();
        assert_eq!(
            resolved.chain,
            vec![Uuid::from_u128(11), Uuid::from_u128(10)]
        );
        assert_eq!(
            resolved.attribution["credentials.private_key"].record_id,
            Uuid::from_u128(10)
        );
        assert_eq!(
            resolved.attribution["credentials.username"].record_id,
            Uuid::from_u128(11)
        );
        assert!(resolved.params["credentials"].get("password").is_none());
        assert!(!resolved.attribution.contains_key("credentials.password"));
        let scopes = device_scopes();
        let assignments = vec![assignment(11, ConnConfScope::Device, Some(DEVICE), 0)];
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };
        let effective = resolve_effective_config(&input).unwrap();
        assert_eq!(
            effective.attribution["credentials.private_key"].record_id,
            Uuid::from_u128(10)
        );
        let accounts =
            super::super::launch_accounts_resolver::resolve_launch_accounts(&input).unwrap();
        assert_eq!(
            accounts[0].attribution["credentials.private_key"].record_id,
            Uuid::from_u128(10)
        );
        let mut overridden = records[1].clone();
        overridden.params["credentials"]["private_key"] = json!("child-key");
        let by_id = [(records[0].id, &records[0]), (overridden.id, &overridden)]
            .into_iter()
            .collect();
        let resolved = resolve_record_params(&overridden, &by_id).unwrap();
        assert_eq!(
            resolved.attribution["credentials.private_key"].record_id,
            overridden.id
        );
        overridden.params["credentials"]["private_key"] = json!({"unset": true});
        let by_id = [(records[0].id, &records[0]), (overridden.id, &overridden)]
            .into_iter()
            .collect();
        let resolved = resolve_record_params(&overridden, &by_id).unwrap();
        assert!(resolved.params["credentials"].get("private_key").is_none());
        assert!(!resolved.attribution.contains_key("credentials.private_key"));
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
        // Attribution names the record that supplied each winning leaf.
        assert_eq!(effective.attribution["port"].record_id, Uuid::from_u128(10));
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
