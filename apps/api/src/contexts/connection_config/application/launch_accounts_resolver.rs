//! Pure non-merging launch-account resolver for the connection-config context.
//!
//! This is the sibling of [`super::effective_config_resolver::resolve_effective_config`]
//! described in `com/docs/connection-launch-accounts-design.md` ("Option A").
//! Where the effective-config resolver deep-merges every scope-matched record
//! into ONE config (correct for the effective-config viewer), this resolver
//! keeps distinct credential identities as DISTINCT launchable accounts:
//!
//! 1. Gather the SAME candidate set as the connect path: enabled assignments
//!    matching the device's scopes (or `global`), joined to enabled records of
//!    the requested protocol.
//! 2. Resolve each candidate's OWN params via its `base_record_id` template
//!    chain (reusing `super::effective_config_resolver::resolve_record_params`).
//! 3. Partition candidates: a record is an ACCOUNT when its resolved params
//!    carry a credential identity (`credentials.username` present, non-empty,
//!    and not the `{"unset": true}` sentinel). Records with NO identity are
//!    "shared defaults".
//! 4. Shared defaults deep-merge (most-specific-wins, the same engine as the
//!    effective-config resolver) into a base defaults tree.
//! 5. Each identity account's effective params = `merge(shared_defaults,
//!    account_record_resolved_params)` with the account record winning. Per-leaf
//!    attribution is preserved PER CONTRIBUTING RECORD ID so the later connect
//!    phase's secret fence can attribute each secret leaf to the record that
//!    actually produced it (never blanket-trusted).
//! 6. Accounts are ordered for launch priority: most-specific scope first,
//!    highest assignment weight first, then `record.id` ascending. A record may
//!    be assigned at several scopes; the MOST-SPECIFIC matching assignment
//!    provides the sort key. The order is deterministic and drives both UI
//!    button order and automatic playbook credential selection.
//! 7. Fallback: if there are ZERO identity accounts, emit exactly ONE account
//!    built from the merged effective-config result, so nothing regresses versus
//!    today's single-merged-config connect path.
//!
//! ## entry_id ordering contract
//!
//! The ordered position of each shared account in the returned `Vec` is the
//! `index` used in the launch selector `config:connconf:<proto>:<index>` (see
//! `connections::application::connection_service::entry_selector`). The later
//! connect-path phase MUST rebuild this EXACT same ordering to select an account
//! by index. For identity accounts, index 0 is the highest launch-priority
//! account (`specificity DESC`, `weight DESC`, `record_id ASC`) and is what
//! Ansible/playbook `auto` credential selection uses when no account is selected
//! explicitly. The fallback single account stays at index 0. Do not reorder
//! without updating the connect path.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};
use uuid::Uuid;

use super::effective_config_resolver::{
    ResolveInput, ResolverError, merge_value_annotated, resolve_effective_config,
    resolve_record_params,
};
use crate::contexts::connection_config::domain::{
    ConnConfProtocol, ConnConfRecord, ConnConfScope, ConnConfSource,
};

/// A single resolved launchable account derived from one connection-config
/// record (or the merged fallback). Pure data; the service maps it onto a REST
/// DTO and the connect path maps it onto launch params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAccount {
    /// The contributing record's id. For the merged fallback account this is the
    /// most-specific participating record (the connect-path fence still resolves
    /// secrets via the per-leaf `params_attribution`, not this id).
    pub record_id: Uuid,
    /// The contributing record's name (display fallback for the account label).
    pub record_name: String,
    /// The scope the account's record was matched at (its most-specific matching
    /// assignment). `None` only for the merged fallback account, which has no
    /// single owning scope.
    pub scope_type: Option<ConnConfScope>,
    /// The scope id for [`Self::scope_type`] (`None` for `global` or fallback).
    pub scope_id: Option<Uuid>,
    /// The account's effective params: `merge(shared_defaults, record_params)`
    /// with the record winning. Includes the presentation-only `ui` subtree.
    pub params: Value,
    /// Per-leaf attribution for [`Self::params`], keyed by dotted leaf path. Each
    /// leaf attributes to the record that actually produced it (account record or
    /// a shared-default record), which the connect-time secret fence requires.
    pub attribution: BTreeMap<String, ConnConfSource>,
}

/// Resolve the ordered list of launchable shared accounts for a device +
/// protocol. See the module docs for the algorithm and the entry_id ordering
/// contract. Pure: the caller owns all DB round-trips.
///
/// Returns identity accounts ordered by launch priority
/// `(specificity DESC, weight DESC, record_id ASC)`. When no record yields a
/// credential identity, returns exactly one fallback account built from the
/// merged effective-config result (empty `Vec` only when there are no
/// participating records at all).
pub fn resolve_launch_accounts(
    input: &ResolveInput<'_>,
) -> Result<Vec<LaunchAccount>, ResolverError> {
    let by_id: HashMap<Uuid, &ConnConfRecord> = input.records.iter().map(|r| (r.id, r)).collect();

    let scope_set: HashSet<(ConnConfScope, Option<Uuid>)> =
        input.device_scopes.iter().copied().collect();

    // (1) Gather candidates exactly like the merging resolver, but keep, for each
    // record, its MOST-SPECIFIC matching assignment as the sort/attribution key
    // (a record may be assigned at several scopes).
    let mut best_by_record: HashMap<Uuid, CandidateKey> = HashMap::new();
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
        let key = CandidateKey {
            scope_type: assignment.scope_type,
            scope_id: assignment.scope_id,
            weight: assignment.weight,
        };
        best_by_record
            .entry(record.id)
            .and_modify(|existing| {
                // Most-specific matching assignment wins the sort key: prefer
                // higher specificity, then higher weight (ties resolved by the
                // record id later, which is identical here).
                if key.merge_sort_tuple(record.id) > existing.merge_sort_tuple(record.id) {
                    *existing = key;
                }
            })
            .or_insert(key);
    }

    // Stable merge order over the deduplicated candidates: (specificity, weight,
    // id) ascending. Shared defaults and fallback merged configs must keep this
    // order so stronger records overlay weaker records. Identity-account launch
    // order is applied separately below.
    let mut ordered: Vec<(&ConnConfRecord, CandidateKey)> = best_by_record
        .iter()
        .filter_map(|(record_id, key)| by_id.get(record_id).map(|record| (*record, *key)))
        .collect();
    ordered.sort_by(|(a_rec, a_key), (b_rec, b_key)| {
        a_key
            .merge_sort_tuple(a_rec.id)
            .cmp(&b_key.merge_sort_tuple(b_rec.id))
    });

    if ordered.is_empty() {
        // No participating records at all: no accounts (the service falls back to
        // personal credential sets, matching `resolve_connconf_launch_params`).
        return Ok(Vec::new());
    }

    // (2) Resolve each candidate's own template-chained params.
    let mut resolved: Vec<ResolvedCandidate<'_>> = Vec::with_capacity(ordered.len());
    for (record, key) in &ordered {
        let params = resolve_record_params(record, &by_id)?;
        resolved.push(ResolvedCandidate {
            record,
            key: *key,
            has_identity: has_launch_identity(record.protocol, &params),
            params,
        });
    }

    // (3) Partition into shared defaults vs identity accounts.
    let has_any_identity = resolved.iter().any(|c| c.has_identity);
    if !has_any_identity {
        // (7) Fallback: build exactly ONE account from the merged result so the
        // connect path (which still merges today) is exactly reproduced. The
        // representative record is the most-specific participating record (the
        // last in `ordered`); `ordered` is non-empty here (we returned early
        // above otherwise), so the `else` arm is defensive, not reachable.
        let merged = resolve_effective_config(input)?;
        let Some((fallback_record, _)) = ordered.last() else {
            return Ok(Vec::new());
        };
        return Ok(vec![LaunchAccount {
            record_id: fallback_record.id,
            record_name: fallback_record.name.clone(),
            scope_type: None,
            scope_id: None,
            params: merged.params,
            attribution: merged.attribution,
        }]);
    }

    // (4) Deep-merge shared-default records (no identity) into a base tree, in
    // ascending specificity so the most-specific default wins, with per-leaf
    // attribution to the contributing default record.
    let mut shared_defaults = Value::Object(Map::new());
    let mut shared_attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
    for candidate in resolved.iter().filter(|c| !c.has_identity) {
        let source = ConnConfSource {
            record_id: candidate.record.id,
            record_name: candidate.record.name.clone(),
            scope_type: Some(candidate.key.scope_type),
            scope_id: candidate.key.scope_id,
        };
        merge_value_annotated(
            &mut shared_defaults,
            &candidate.params,
            "",
            &source,
            &mut shared_attribution,
        );
    }

    // (5) For each identity account: effective params = merge(shared_defaults,
    // account_record_params) with the account record winning. Attribution starts
    // from the shared-defaults attribution, then the account overlay overwrites
    // each leaf it contributes — so every leaf attributes to the record that
    // actually produced its winning value.
    let mut accounts: Vec<(LaunchAccount, CandidateKey)> = Vec::new();
    for candidate in resolved.iter().filter(|c| c.has_identity) {
        let mut params = shared_defaults.clone();
        let mut attribution = shared_attribution.clone();
        let source = ConnConfSource {
            record_id: candidate.record.id,
            record_name: candidate.record.name.clone(),
            scope_type: Some(candidate.key.scope_type),
            scope_id: candidate.key.scope_id,
        };
        merge_value_annotated(
            &mut params,
            &candidate.params,
            "",
            &source,
            &mut attribution,
        );
        accounts.push((
            LaunchAccount {
                record_id: candidate.record.id,
                record_name: candidate.record.name.clone(),
                scope_type: Some(candidate.key.scope_type),
                scope_id: candidate.key.scope_id,
                params,
                attribution,
            },
            candidate.key,
        ));
    }

    // (6) Launch choices are presented highest-priority first. This deliberately
    // differs from the merge order above: merge order is ascending so stronger
    // defaults win by overlaying; launch order is descending so index 0 is the
    // best account for UI order and Ansible/playbook `auto` selection.
    accounts.sort_by(|(a_account, a_key), (b_account, b_key)| {
        a_key.cmp_launch_order(a_account.record_id, *b_key, b_account.record_id)
    });

    Ok(accounts
        .into_iter()
        .map(|(account, _key)| account)
        .collect())
}

/// The sort/attribution key for a record: its most-specific matching assignment.
#[derive(Debug, Clone, Copy)]
struct CandidateKey {
    scope_type: ConnConfScope,
    scope_id: Option<Uuid>,
    weight: i32,
}

impl CandidateKey {
    /// The ascending merge tuple `(specificity, weight, record_id)`. The record
    /// id is the final tiebreak, matching the effective-config resolver's order.
    fn merge_sort_tuple(self, record_id: Uuid) -> (u8, i32, Uuid) {
        (self.scope_type.specificity(), self.weight, record_id)
    }

    /// Compare two identity accounts in launch order: strongest assignment
    /// first, deterministic record id tiebreak last.
    fn cmp_launch_order(
        self,
        record_id: Uuid,
        other: Self,
        other_record_id: Uuid,
    ) -> std::cmp::Ordering {
        other
            .scope_type
            .specificity()
            .cmp(&self.scope_type.specificity())
            .then_with(|| other.weight.cmp(&self.weight))
            .then_with(|| record_id.cmp(&other_record_id))
    }
}

struct ResolvedCandidate<'a> {
    record: &'a ConnConfRecord,
    key: CandidateKey,
    params: Value,
    has_identity: bool,
}

/// True iff the resolved params represent one launchable choice.
///
/// SSH/RDP/VNC choices are account-shaped and therefore keyed by
/// `credentials.username`. APP choices do not carry credentials; each non-empty
/// launcher URL is its own button and must not be merged with sibling APP
/// records at the same scope.
fn has_launch_identity(protocol: ConnConfProtocol, params: &Value) -> bool {
    if protocol == ConnConfProtocol::App {
        return params
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| !url.trim().is_empty());
    }
    params
        .get("credentials")
        .and_then(Value::as_object)
        .and_then(|creds| creds.get("username"))
        .and_then(Value::as_str)
        .is_some_and(|username| !username.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::connection_config::domain::{ConnConfAssignment, ConnConfProtocol};
    use chrono::Utc;
    use serde_json::json;

    fn record(id: u128, name: &str, base: Option<u128>, params: Value) -> ConnConfRecord {
        record_with_protocol(id, name, ConnConfProtocol::Ssh, base, params)
    }

    fn record_with_protocol(
        id: u128,
        name: &str,
        protocol: ConnConfProtocol,
        base: Option<u128>,
        params: Value,
    ) -> ConnConfRecord {
        ConnConfRecord {
            id: Uuid::from_u128(id),
            name: name.to_string(),
            description: String::new(),
            protocol,
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

    // Ordering determinism + identity-vs-defaults partition + defaults inheritance.
    #[test]
    fn identity_accounts_inherit_shared_defaults_in_order() {
        // A shared default (no username) carried at region: port + timeout.
        let defaults = record(
            1,
            "shared-defaults",
            None,
            json!({ "port": 22, "timeout_secs": 30 }),
        );
        // Two identity accounts at device scope: root and admin.
        let root = record(
            2,
            "root",
            None,
            json!({ "credentials": { "username": "root", "password": "rootpw" } }),
        );
        let admin = record(
            3,
            "admin",
            None,
            json!({ "credentials": { "username": "admin" } }),
        );
        let records = vec![defaults, root, admin];
        let assignments = vec![
            assignment(1, ConnConfScope::Region, Some(REGION), 0),
            assignment(2, ConnConfScope::Device, Some(DEVICE), 0),
            assignment(3, ConnConfScope::Device, Some(DEVICE), 5),
        ];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        let accounts = resolve_launch_accounts(&input).expect("resolve");
        // Two identity accounts (defaults record is not an account).
        assert_eq!(accounts.len(), 2);
        // Deterministic launch order: both at device specificity, admin weight
        // 5 outranks root weight 0, so admin is index 0 for UI and playbook
        // auto credential selection.
        assert_eq!(accounts[0].record_id, Uuid::from_u128(3));
        assert_eq!(accounts[1].record_id, Uuid::from_u128(2));
        // Both accounts inherit the shared defaults (port + timeout).
        assert_eq!(accounts[0].params["port"], json!(22));
        assert_eq!(accounts[0].params["timeout_secs"], json!(30));
        assert_eq!(accounts[1].params["port"], json!(22));
        // Each carries its own identity.
        assert_eq!(
            accounts[0].params["credentials"]["username"],
            json!("admin")
        );
        assert_eq!(accounts[1].params["credentials"]["username"], json!("root"));
        // The default leaves attribute to the defaults record, identity leaves to
        // the account record.
        assert_eq!(
            accounts[0].attribution["port"].record_id,
            Uuid::from_u128(1)
        );
        assert_eq!(
            accounts[1].attribution["credentials.password"].record_id,
            Uuid::from_u128(2)
        );
    }

    // Secret attribution stays per-record even when defaults supply a password and
    // the account overrides it.
    #[test]
    fn secret_attribution_is_per_contributing_record() {
        // Shared default supplies a fallback password at region.
        let defaults = record(
            1,
            "shared",
            None,
            json!({ "credentials": { "password": "shared-secret" } }),
        );
        // Account overrides the password at device scope.
        let root = record(
            2,
            "root",
            None,
            json!({ "credentials": { "username": "root", "password": "own-secret" } }),
        );
        let records = vec![defaults, root];
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

        let accounts = resolve_launch_accounts(&input).expect("resolve");
        assert_eq!(accounts.len(), 1);
        // The account's password wins and attributes to the account record (2),
        // NOT the shared-defaults record (1).
        assert_eq!(
            accounts[0].params["credentials"]["password"],
            json!("own-secret")
        );
        assert_eq!(
            accounts[0].attribution["credentials.password"].record_id,
            Uuid::from_u128(2)
        );
    }

    #[test]
    fn app_url_records_are_separate_launch_choices() {
        let cups = record_with_protocol(
            1,
            "cups",
            ConnConfProtocol::App,
            None,
            json!({ "url": "http://{{primary_ip4}}:631", "ui": { "label": "CUPS" } }),
        );
        let utm = record_with_protocol(
            2,
            "utm",
            ConnConfProtocol::App,
            None,
            json!({ "url": "http://{{primary_ip4}}:8080", "ui": { "label": "УТМ" } }),
        );
        let records = vec![cups, utm];
        let assignments = vec![
            assignment(1, ConnConfScope::Device, Some(DEVICE), 0),
            assignment(2, ConnConfScope::Device, Some(DEVICE), 5),
        ];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::App,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        let accounts = resolve_launch_accounts(&input).expect("resolve");

        assert_eq!(accounts.len(), 2);
        assert_eq!(
            accounts[0].params["url"],
            json!("http://{{primary_ip4}}:8080")
        );
        assert_eq!(
            accounts[1].params["url"],
            json!("http://{{primary_ip4}}:631")
        );
    }

    // Fallback: zero identity accounts -> exactly one merged account.
    #[test]
    fn fallback_to_single_merged_account_when_no_identity() {
        let region = record(1, "region", None, json!({ "port": 22 }));
        let device = record(2, "device", None, json!({ "port": 2222 }));
        let records = vec![region, device];
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

        let accounts = resolve_launch_accounts(&input).expect("resolve");
        assert_eq!(accounts.len(), 1, "exactly one fallback account");
        // Merged result: device port wins, no scope owner on the fallback.
        assert_eq!(accounts[0].params["port"], json!(2222));
        assert!(accounts[0].scope_type.is_none());
    }

    // No participating records -> empty (service falls back to personal sets).
    #[test]
    fn no_records_yields_empty() {
        let records: Vec<ConnConfRecord> = Vec::new();
        let assignments: Vec<ConnConfAssignment> = Vec::new();
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };
        let accounts = resolve_launch_accounts(&input).expect("resolve");
        assert!(accounts.is_empty());
    }

    // A record assigned at MULTIPLE scopes uses its most-specific assignment for
    // the sort key (determinism across duplicate assignments).
    #[test]
    fn most_specific_assignment_drives_ordering() {
        // "a" is assigned at both region and device; device (more specific) is
        // the sort key. "b" only at region. Device-keyed "a" must be the first
        // launch choice even though merge order remains ascending internally.
        let a = record(1, "a", None, json!({ "credentials": { "username": "a" } }));
        let b = record(2, "b", None, json!({ "credentials": { "username": "b" } }));
        let records = vec![a, b];
        let assignments = vec![
            assignment(1, ConnConfScope::Region, Some(REGION), 0),
            assignment(1, ConnConfScope::Device, Some(DEVICE), 0),
            assignment(2, ConnConfScope::Region, Some(REGION), 0),
        ];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        let accounts = resolve_launch_accounts(&input).expect("resolve");
        assert_eq!(accounts.len(), 2);
        // a (device) sorts before b (region) because launch order is highest
        // priority first.
        assert_eq!(accounts[0].record_id, Uuid::from_u128(1));
        assert_eq!(accounts[1].record_id, Uuid::from_u128(2));
        // a appears exactly once despite two assignments.
        let a_count = accounts
            .iter()
            .filter(|acc| acc.record_id == Uuid::from_u128(1))
            .count();
        assert_eq!(a_count, 1);
    }

    // `{"unset": true}` on username does not create a phantom identity account.
    #[test]
    fn unset_username_is_not_an_identity() {
        let rec = record(
            1,
            "no-identity",
            None,
            json!({ "credentials": { "username": { "unset": true } }, "port": 22 }),
        );
        let records = vec![rec];
        let assignments = vec![assignment(1, ConnConfScope::Device, Some(DEVICE), 0)];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };
        let accounts = resolve_launch_accounts(&input).expect("resolve");
        // No identity -> fallback single account (merged), not an identity account.
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].scope_type.is_none(), "fallback account");
    }
}
