//! Row-level visibility predicate for connection targets.
//!
//! A LIST endpoint that returns connection-target rows (devices / virtual
//! machines) must show a caller only the rows their `Read × CONNECTION_TARGET`
//! policies allow — attribute-constrained visibility of ROWS, not an all-or-
//! nothing gate on the whole list. This module compiles a subject's resolved
//! `Read × CONNECTION_TARGET` policy set into one boolean predicate over a row's
//! inventory facets, with two faithful renderings:
//!
//! * [`VisibilityPredicate::matches`] — pure in-memory evaluation, used to prove
//!   fidelity against the real policy engine and as a fallback.
//! * an SQL rendering (in the inventory infrastructure layer) that walks the
//!   same AST — so the database filters rows exactly as the engine would decide.
//!
//! Semantics mirror [`crate::contexts::access_control::domain::policy_evaluation`]:
//! deny-override, default-deny. Visibility =
//! `OR(allow legs) AND NOT OR(deny legs)`, where each leg is
//! `resource-id match AND constraint match`. Request-scoped conditions
//! (`ip_range` / `time_window`) are pre-evaluated ONCE against the request
//! context (they are identical for every row); object-attribute conditions
//! become per-row column comparisons.
//!
//! Faithfulness note (the reason the two renderings can diverge): a negated
//! equality over a NULL column is three-valued in SQL. The SQL renderer must
//! push negation to the leaves (De Morgan) and emit `col IS DISTINCT FROM $`
//! for a negated equality, never `NOT (col = $)`. This module's `matches` is
//! two-valued and is the reference; the DB-backed fidelity test pins the two
//! together.

use serde_json::Value;

use crate::contexts::access_control::domain::action::Action;
use crate::contexts::access_control::domain::conditions::{
    Condition, ConditionExpression, evaluate_constraints, parse_constraints,
};
use crate::contexts::access_control::domain::policy::{Policy, PolicyEffect};
use crate::contexts::access_control::domain::resource::{
    ResourceType, matches_resource_pattern, resource_pattern_is_valid,
};
use crate::contexts::access_control::domain::vocabulary::{
    ATTR_CLUSTER_ID, ATTR_PLATFORM_ID, ATTR_REGION_ID, ATTR_ROLE_ID, ATTR_SITE_ID, ATTR_STATUS,
    ATTR_TENANT_ID,
};

/// A connection-target facet a policy constraint may scope on. Maps 1:1 to a row
/// column in every read-model (both UNION branches expose these). `protocol` is
/// intentionally NOT a facet: a list row has no protocol, so a protocol-keyed
/// attribute constraint can never match a row (fail-closed, like the per-target
/// enforcement site treats an absent facet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facet {
    RoleId,
    SiteId,
    /// The row's site's direct region — not a physical row column; the SQL
    /// rendering resolves it via a scalar subquery over `dcim_site`.
    RegionId,
    TenantId,
    PlatformId,
    ClusterId,
    Status,
}

impl Facet {
    fn from_attribute_key(key: &str) -> Option<Self> {
        match key {
            ATTR_ROLE_ID => Some(Facet::RoleId),
            ATTR_SITE_ID => Some(Facet::SiteId),
            ATTR_REGION_ID => Some(Facet::RegionId),
            ATTR_TENANT_ID => Some(Facet::TenantId),
            ATTR_PLATFORM_ID => Some(Facet::PlatformId),
            ATTR_CLUSTER_ID => Some(Facet::ClusterId),
            ATTR_STATUS => Some(Facet::Status),
            // `protocol` and any unknown key have no row column → never matches.
            _ => None,
        }
    }
}

/// The facet values of a single row, as the STRING forms a policy constraint
/// compares against (UUIDs as their canonical lowercase string, status as its
/// raw text) — exactly what the enforcement-site attribute map carries, so
/// `matches` reproduces the engine's `==` on the evaluation context.
#[derive(Debug, Clone, Default)]
pub struct RowFacets {
    pub id: String,
    pub role_id: Option<String>,
    pub site_id: Option<String>,
    pub region_id: Option<String>,
    pub tenant_id: Option<String>,
    pub platform_id: Option<String>,
    pub cluster_id: Option<String>,
    pub status: Option<String>,
}

impl RowFacets {
    fn facet(&self, facet: Facet) -> Option<&str> {
        match facet {
            Facet::RoleId => self.role_id.as_deref(),
            Facet::SiteId => self.site_id.as_deref(),
            Facet::RegionId => self.region_id.as_deref(),
            Facet::TenantId => self.tenant_id.as_deref(),
            Facet::PlatformId => self.platform_id.as_deref(),
            Facet::ClusterId => self.cluster_id.as_deref(),
            Facet::Status => self.status.as_deref(),
        }
    }
}

/// A compiled visibility predicate. Public so the infrastructure SQL renderer can
/// walk it; construct only via [`compile_connection_target_visibility`].
#[derive(Debug, Clone, PartialEq)]
pub enum VisibilityPredicate {
    /// Every row is visible — the caller should skip filtering entirely.
    Always,
    /// No row is visible — the caller should return an empty result (or 403 on a
    /// connection-target-typed endpoint, where zero read grants means no access).
    Never,
    /// `facet == value` (value is the constraint's string form).
    Eq {
        facet: Facet,
        value: String,
    },
    /// The row id matches a policy resource pattern (glob / `re:` / exact),
    /// evaluated by [`matches_resource_pattern`] over the BARE row id.
    IdMatch {
        pattern: String,
    },
    And(Vec<VisibilityPredicate>),
    Or(Vec<VisibilityPredicate>),
    Not(Box<VisibilityPredicate>),
}

impl VisibilityPredicate {
    /// True when the predicate admits every row — the caller can skip filtering.
    pub fn is_always(&self) -> bool {
        matches!(self, VisibilityPredicate::Always)
    }

    /// True when the predicate admits no row — the caller can short-circuit.
    pub fn is_never(&self) -> bool {
        matches!(self, VisibilityPredicate::Never)
    }

    /// Two-valued reference evaluation of the predicate against a row. This is the
    /// authority the SQL rendering must reproduce.
    pub fn matches(&self, row: &RowFacets) -> bool {
        match self {
            VisibilityPredicate::Always => true,
            VisibilityPredicate::Never => false,
            VisibilityPredicate::Eq { facet, value } => row.facet(*facet) == Some(value.as_str()),
            VisibilityPredicate::IdMatch { pattern } => matches_resource_pattern(pattern, &row.id),
            VisibilityPredicate::And(items) => items.iter().all(|item| item.matches(row)),
            VisibilityPredicate::Or(items) => items.iter().any(|item| item.matches(row)),
            VisibilityPredicate::Not(inner) => !inner.matches(row),
        }
    }
}

/// Compile a subject's resolved policies into a connection-target row-visibility
/// predicate. Only `Read × CONNECTION_TARGET` allow/deny policies participate;
/// the caller is responsible for having resolved the policy set for the subject
/// (subject → policy resolution is done in SQL). `context` supplies the request-
/// scoped constraint inputs (`ip` / `timestamp`) used to pre-evaluate
/// `ip_range` / `time_window` conditions.
pub fn compile_connection_target_visibility(
    policies: &[Policy],
    context: &Value,
) -> VisibilityPredicate {
    let mut allow_legs = Vec::new();
    let mut deny_legs = Vec::new();

    for policy in policies {
        if !policy.enabled
            || !policy.actions.contains(&Action::Read)
            || !policy
                .resource_types
                .contains(&ResourceType::ConnectionTarget)
        {
            continue;
        }
        let leg = simplify(VisibilityPredicate::And(vec![
            resource_predicate(&policy.resources),
            constraint_predicate(policy.constraints.as_ref(), context),
        ]));
        match policy.effect {
            PolicyEffect::Allow => allow_legs.push(leg),
            PolicyEffect::Deny => deny_legs.push(leg),
        }
    }

    let allow = simplify(VisibilityPredicate::Or(allow_legs));
    if deny_legs.is_empty() {
        return allow;
    }
    let deny = simplify(VisibilityPredicate::Or(deny_legs));
    simplify(VisibilityPredicate::And(vec![
        allow,
        simplify(VisibilityPredicate::Not(Box::new(deny))),
    ]))
}

/// Translate a policy's `resources` list into a row-id predicate. Empty or a `*`
/// entry means "all rows" (the common attribute-scoped case); otherwise the row
/// id must match at least one pattern.
fn resource_predicate(resources: &[String]) -> VisibilityPredicate {
    if resources.is_empty() || resources.iter().any(|pattern| pattern == "*") {
        return VisibilityPredicate::Always;
    }
    VisibilityPredicate::Or(
        resources
            .iter()
            .map(|pattern| {
                // A pattern the engine's matcher cannot compile (a `re:` regex
                // the Rust `regex` crate rejects) matches NOTHING in the engine
                // (`matches_resource_pattern` → `unwrap_or(false)`). Reproduce
                // that as `Never` so the SQL rendering never emits a Postgres
                // regex for a pattern the engine would reject — otherwise the
                // two dialects could disagree and disclose a row the engine
                // denies.
                if resource_pattern_is_valid(pattern) {
                    VisibilityPredicate::IdMatch {
                        pattern: pattern.clone(),
                    }
                } else {
                    VisibilityPredicate::Never
                }
            })
            .collect(),
    )
}

/// Translate a policy's `constraints` JSON into a predicate. Request-scoped
/// conditions are pre-evaluated to a constant; attribute conditions become
/// per-facet equality; unknown leaves are fail-closed to `Never`.
fn constraint_predicate(constraints: Option<&Value>, context: &Value) -> VisibilityPredicate {
    let Some(constraints) = constraints else {
        return VisibilityPredicate::Always;
    };
    walk_condition(&parse_constraints(constraints), context)
}

fn walk_condition(expression: &ConditionExpression, context: &Value) -> VisibilityPredicate {
    match expression {
        ConditionExpression::And(items) => VisibilityPredicate::And(
            items
                .iter()
                .map(|item| walk_condition(item, context))
                .collect(),
        ),
        ConditionExpression::Or(items) => VisibilityPredicate::Or(
            items
                .iter()
                .map(|item| walk_condition(item, context))
                .collect(),
        ),
        ConditionExpression::Condition(Condition::Attributes { attributes }) => {
            VisibilityPredicate::And(
                attributes
                    .iter()
                    .map(|(key, value)| attribute_equality(key, value))
                    .collect(),
            )
        }
        // `ip_range` / `time_window` are identical for every row — fold to a
        // constant using the SAME evaluator the engine uses, so a currently-out-
        // of-window policy contributes nothing (allow) / does not fire (deny).
        ConditionExpression::Condition(
            condition @ (Condition::IpRange { .. } | Condition::TimeWindow { .. }),
        ) => {
            if evaluate_constraints(&ConditionExpression::Condition(condition.clone()), context) {
                VisibilityPredicate::Always
            } else {
                VisibilityPredicate::Never
            }
        }
        // Unknown leaves are rejected at write time; if one is somehow stored,
        // fail closed (grant nothing on an allow leg; De Morgan turns this into
        // "does not fire" on a deny leg).
        ConditionExpression::Condition(Condition::Unknown(_)) => VisibilityPredicate::Never,
    }
}

fn attribute_equality(key: &str, value: &Value) -> VisibilityPredicate {
    match (Facet::from_attribute_key(key), value.as_str()) {
        (Some(facet), Some(value)) => VisibilityPredicate::Eq {
            facet,
            value: value.to_string(),
        },
        // No such row facet (e.g. `protocol`), or a non-string constraint value
        // the engine's exact `==` could never match against a string context
        // value → this leg never matches.
        _ => VisibilityPredicate::Never,
    }
}

/// Fold `Always`/`Never` constants so the common unconstrained-allow case
/// collapses to `Always` (predicate skipped) and an empty allow set to `Never`.
/// Behaviour-preserving with respect to `matches`.
fn simplify(predicate: VisibilityPredicate) -> VisibilityPredicate {
    match predicate {
        VisibilityPredicate::And(items) => simplify_and(items),
        VisibilityPredicate::Or(items) => simplify_or(items),
        VisibilityPredicate::Not(inner) => match simplify(*inner) {
            VisibilityPredicate::Always => VisibilityPredicate::Never,
            VisibilityPredicate::Never => VisibilityPredicate::Always,
            other => VisibilityPredicate::Not(Box::new(other)),
        },
        leaf => leaf,
    }
}

/// Fold an `And`: a `Never` conjunct short-circuits to `Never`, `Always`
/// conjuncts drop out, and the residue collapses when it has 0 or 1 members.
fn simplify_and(items: Vec<VisibilityPredicate>) -> VisibilityPredicate {
    let mut folded = Vec::new();
    for item in items {
        match simplify(item) {
            VisibilityPredicate::Never => return VisibilityPredicate::Never,
            VisibilityPredicate::Always => {}
            other => folded.push(other),
        }
    }
    match folded.len() {
        0 => VisibilityPredicate::Always,
        1 => folded.pop().unwrap_or(VisibilityPredicate::Always),
        _ => VisibilityPredicate::And(folded),
    }
}

/// Fold an `Or`: an `Always` disjunct short-circuits to `Always`, `Never`
/// disjuncts drop out, and the residue collapses when it has 0 or 1 members.
fn simplify_or(items: Vec<VisibilityPredicate>) -> VisibilityPredicate {
    let mut folded = Vec::new();
    for item in items {
        match simplify(item) {
            VisibilityPredicate::Always => return VisibilityPredicate::Always,
            VisibilityPredicate::Never => {}
            other => folded.push(other),
        }
    }
    match folded.len() {
        0 => VisibilityPredicate::Never,
        1 => folded.pop().unwrap_or(VisibilityPredicate::Never),
        _ => VisibilityPredicate::Or(folded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::access_control::domain::policy_evaluation::{
        PolicyDecision, evaluate_policies_with_metadata,
    };
    use serde_json::json;
    use uuid::Uuid;

    fn policy(effect: PolicyEffect, resources: Vec<&str>, constraints: Option<Value>) -> Policy {
        Policy {
            id: Uuid::now_v7(),
            name: "p".to_string(),
            description: None,
            effect,
            actions: vec![Action::Read],
            resource_types: vec![ResourceType::ConnectionTarget],
            constraints,
            subjects: Vec::new(),
            resources: resources.into_iter().map(str::to_string).collect(),
            enabled: true,
        }
    }

    fn request_context() -> Value {
        json!({"ip": "10.1.2.3", "timestamp": "2026-07-13T12:00:00Z"})
    }

    /// Build the per-row evaluation context the engine would see for a row, so we
    /// can compare the compiled predicate against the real engine decision.
    fn row_context(row: &RowFacets) -> Value {
        let mut attributes = serde_json::Map::new();
        let mut put = |key: &str, value: &Option<String>| {
            if let Some(value) = value {
                attributes.insert(key.to_string(), Value::String(value.clone()));
            }
        };
        put(ATTR_ROLE_ID, &row.role_id);
        put(ATTR_SITE_ID, &row.site_id);
        put(ATTR_TENANT_ID, &row.tenant_id);
        put(ATTR_PLATFORM_ID, &row.platform_id);
        put(ATTR_CLUSTER_ID, &row.cluster_id);
        put(ATTR_STATUS, &row.status);
        json!({
            "ip": "10.1.2.3",
            "timestamp": "2026-07-13T12:00:00Z",
            "attributes": attributes,
        })
    }

    fn engine_decision(policies: &[Policy], row: &RowFacets) -> bool {
        evaluate_policies_with_metadata(
            policies,
            Action::Read,
            ResourceType::ConnectionTarget,
            Some(&row.id),
            &row_context(row),
        )
        .decision
            == PolicyDecision::Allow
    }

    fn row(id: &str, site: Option<&str>, role: Option<&str>, status: Option<&str>) -> RowFacets {
        RowFacets {
            id: id.to_string(),
            site_id: site.map(str::to_string),
            role_id: role.map(str::to_string),
            status: status.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn unconstrained_allow_is_always() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["*"], None)];
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        assert!(predicate.is_always());
    }

    #[test]
    fn no_read_grant_is_never() {
        let predicate = compile_connection_target_visibility(&[], &request_context());
        assert!(predicate.is_never());
    }

    #[test]
    fn site_scoped_allow_matches_only_that_site() {
        let policies = vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"attributes": {"site_id": "site-a"}})),
        )];
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        assert!(predicate.matches(&row("d1", Some("site-a"), Some("ru1"), None)));
        assert!(!predicate.matches(&row("d2", Some("site-b"), Some("ru1"), None)));
        // A row with a NULL site never matches a positive site equality.
        assert!(!predicate.matches(&row("d3", None, Some("ru1"), None)));
    }

    #[test]
    fn deny_overrides_allow_and_null_facet_is_not_hidden_by_a_deny() {
        // Unconstrained allow + deny on site-a. The engine hides site-a rows,
        // shows everything else INCLUDING rows whose site is NULL (a positive
        // site equality is false on NULL, so its deny does not fire).
        let policies = vec![
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(
                PolicyEffect::Deny,
                vec!["*"],
                Some(json!({"attributes": {"site_id": "site-a"}})),
            ),
        ];
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        assert!(!predicate.matches(&row("d1", Some("site-a"), None, None)));
        assert!(predicate.matches(&row("d2", Some("site-b"), None, None)));
        // The NULL-site row must remain visible — this is the exact case the SQL
        // `IS DISTINCT FROM` rendering has to preserve.
        assert!(predicate.matches(&row("d3", None, None, None)));
    }

    /// Extension tripwire (see the vocabulary module's extension guide): every
    /// attribute key declared for `CONNECTION_TARGET` must compile to a row
    /// facet — except `protocol`, which is DOCUMENTED as list-inapplicable (a
    /// list row has no protocol; a protocol-keyed constraint fails closed on
    /// lists by design). If this test is red after adding a key, add the
    /// `Facet` variant + `RowFacets` field + `from_attribute_key` arm here and
    /// the `VisibilityColumns` expression in all four read-model column sets.
    #[test]
    fn every_declared_key_compiles_to_a_row_facet_except_protocol() {
        use crate::contexts::access_control::domain::vocabulary::{
            ATTR_PROTOCOL, constraint_attribute_keys,
        };
        for key in constraint_attribute_keys(ResourceType::ConnectionTarget) {
            if *key == ATTR_PROTOCOL {
                assert!(Facet::from_attribute_key(key).is_none());
                continue;
            }
            assert!(
                Facet::from_attribute_key(key).is_some(),
                "declared attribute key `{key}` has no row facet — row-level \
                 visibility would silently never match it; wire it through \
                 Facet/RowFacets/VisibilityColumns (see the extension guide)"
            );
        }
    }

    #[test]
    fn region_scoped_allow_matches_only_rows_with_that_region_facet() {
        let policies = vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"attributes": {"region_id": "region-r1"}})),
        )];
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        let mut in_region = row("d1", Some("site-a"), None, None);
        in_region.region_id = Some("region-r1".to_string());
        assert!(predicate.matches(&in_region));
        // A row whose site has no region (or no site at all) carries no
        // region facet → a positive region equality never matches it.
        assert!(!predicate.matches(&row("d2", Some("site-b"), None, None)));
        assert!(!predicate.matches(&row("d3", None, None, None)));
    }

    #[test]
    fn protocol_keyed_constraint_never_matches_a_list_row() {
        let policies = vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"attributes": {"protocol": "vnc"}})),
        )];
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        assert!(predicate.is_never());
    }

    #[test]
    fn ip_range_out_of_window_drops_the_allow_leg() {
        let policies = vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(
                json!({"and": [{"ip_range": "192.168.0.0/16"}, {"attributes": {"site_id": "site-a"}}]}),
            ),
        )];
        // Request ip 10.1.2.3 is outside 192.168/16 → the AND leg folds to Never.
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        assert!(predicate.is_never());
    }

    #[test]
    fn ip_range_in_window_keeps_the_attribute_leg() {
        let context = json!({"ip": "192.168.5.5", "timestamp": "2026-07-13T12:00:00Z"});
        let policies = vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(
                json!({"and": [{"ip_range": "192.168.0.0/16"}, {"attributes": {"site_id": "site-a"}}]}),
            ),
        )];
        let predicate = compile_connection_target_visibility(&policies, &context);
        assert!(predicate.matches(&RowFacets {
            id: "d1".to_string(),
            site_id: Some("site-a".to_string()),
            ..Default::default()
        }));
    }

    #[test]
    fn re_pattern_the_rust_engine_rejects_is_fail_closed() {
        // Lookahead is unsupported by the Rust `regex` crate, so the engine's
        // matcher returns false for every row. The compiler must reproduce that
        // as `Never` (not emit a Postgres regex that could match), matching the
        // engine and never disclosing a row.
        for bad in ["re:(?=.*c1$).+", "re:(", "re:.*\\1"] {
            let policies = vec![policy(PolicyEffect::Allow, vec![bad], None)];
            let predicate = compile_connection_target_visibility(&policies, &request_context());
            assert!(
                predicate.is_never(),
                "invalid re: pattern {bad:?} must compile to Never (fail-closed)"
            );
            // The engine agrees: this policy grants nothing on any row.
            assert!(!engine_decision(
                &policies,
                &row("d1", Some("site-a"), None, None)
            ));
        }
    }

    #[test]
    fn valid_re_pattern_still_matches() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["re:dev-.*"], None)];
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        assert!(predicate.matches(&row("dev-1", Some("site-a"), None, None)));
        assert!(!predicate.matches(&row("host-1", Some("site-a"), None, None)));
    }

    #[test]
    fn resource_id_scoped_allow_matches_only_that_id() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["dev-1"], None)];
        let predicate = compile_connection_target_visibility(&policies, &request_context());
        assert!(predicate.matches(&row("dev-1", Some("site-a"), None, None)));
        assert!(!predicate.matches(&row("dev-2", Some("site-a"), None, None)));
    }

    /// Fidelity harness: for a matrix of policy sets × rows, the compiled
    /// predicate's `matches` must equal the real engine decision. This is the
    /// two-valued reference the SQL rendering is then tested against separately.
    #[test]
    fn compiled_predicate_matches_engine_across_matrix() {
        let sets: Vec<Vec<Policy>> = vec![
            vec![policy(PolicyEffect::Allow, vec!["*"], None)],
            vec![policy(
                PolicyEffect::Allow,
                vec!["*"],
                Some(json!({"attributes": {"site_id": "site-a"}})),
            )],
            vec![
                policy(PolicyEffect::Allow, vec!["*"], None),
                policy(
                    PolicyEffect::Deny,
                    vec!["*"],
                    Some(
                        json!({"or": [{"attributes": {"role_id": "ru1"}}, {"attributes": {"role_id": "ru2"}}]}),
                    ),
                ),
            ],
            vec![policy(
                PolicyEffect::Allow,
                vec!["*"],
                Some(json!({"and": [
                    {"or": [{"attributes": {"role_id": "ru3"}}, {"attributes": {"role_id": "ru4"}}]},
                    {"or": [{"attributes": {"site_id": "site-a"}}, {"attributes": {"site_id": "site-b"}}]}
                ]})),
            )],
            vec![
                policy(
                    PolicyEffect::Allow,
                    vec!["*"],
                    Some(json!({"attributes": {"site_id": "site-a"}})),
                ),
                policy(
                    PolicyEffect::Deny,
                    vec!["*"],
                    Some(json!({"attributes": {"status": "decommissioning"}})),
                ),
            ],
        ];
        let rows = vec![
            row("d1", Some("site-a"), Some("ru1"), Some("active")),
            row("d2", Some("site-b"), Some("ru3"), Some("active")),
            row("d3", Some("site-a"), Some("ru4"), Some("decommissioning")),
            row("d4", None, None, None),
            row("d5", Some("site-c"), Some("ru2"), Some("active")),
        ];
        for policies in &sets {
            let predicate = compile_connection_target_visibility(policies, &request_context());
            for row in &rows {
                assert_eq!(
                    predicate.matches(row),
                    engine_decision(policies, row),
                    "predicate/engine disagree for row {} under policy set",
                    row.id
                );
            }
        }
    }
}
