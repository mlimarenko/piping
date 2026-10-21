//! Permission-matrix scenario suite — confidence that the RBAC evaluation engine
//! decides Allow/Deny correctly across a broad, realistic set of access designs.
//!
//! WHAT THIS TESTS: the pure decision core
//! `evaluate_policies_with_metadata(policies, action, resource_type, resource_id,
//! context)` — deny-override, action×resource-type matching, resource-id glob/
//! regex scoping, and `and`/`or`/equality constraint evaluation against a request
//! context. Each scenario supplies the policy set ALREADY RESOLVED for a subject
//! (as `AccessControlService::resolve_policies_for_principal` would resolve a
//! role's `role:{uuid}` policies) plus the request context, and asserts the
//! decision.
//!
//! WHAT THIS DOES NOT TEST (covered elsewhere):
//! - subject → policy resolution (user→roles→groups→policies): the SQL join in
//!   `list_for_subjects`, exercised by the DB-backed access-control tests;
//! - whether a real enforcement site POPULATES the object attributes a
//!   constraint needs. Every per-target `ConnectionTarget` site resolves the
//!   target's inventory facets and passes them via
//!   `ensure_access_with_attributes` — connect create, session
//!   resume/health/client-credentials and the SSH file tools (those also pass
//!   the session's protocol), monitoring reads, the metric-query live test,
//!   playbook per-host checks, launch-option reads, effective config (with the
//!   requested protocol), and proxy-assignment read/update. Sites for other
//!   resource types still pass no attributes — by design, since the vocabulary
//!   allows no constraint attribute keys for them. The attribute-scoped
//!   scenarios below prove the ENGINE evaluates these correctly GIVEN the
//!   attributes; `enforcement_gaps_documented` pins the fail-closed behavior
//!   at attribute-less sites.
//!
//! The design intent: a single table that reads like a permission spec, so a
//! reviewer can confirm each role's real-world behavior at a glance and a new
//! scenario is one row.

use piping_backend::contexts::access_control::domain::{
    Action, Policy, PolicyDecision, PolicyEffect, ResourceType, evaluate_policies_with_metadata,
};
use serde_json::{Value, json};
use uuid::Uuid;

// ── builders ────────────────────────────────────────────────────────────────

fn policy(
    effect: PolicyEffect,
    actions: Vec<Action>,
    resource_types: Vec<ResourceType>,
    resources: Vec<&str>,
    constraints: Option<Value>,
) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "p".to_string(),
        description: None,
        effect,
        actions,
        resource_types,
        constraints,
        subjects: Vec::new(),
        resources: resources.into_iter().map(str::to_string).collect(),
        enabled: true,
    }
}

fn allow(
    actions: Vec<Action>,
    resource_types: Vec<ResourceType>,
    resources: Vec<&str>,
    constraints: Option<Value>,
) -> Policy {
    policy(
        PolicyEffect::Allow,
        actions,
        resource_types,
        resources,
        constraints,
    )
}

fn deny(
    actions: Vec<Action>,
    resource_types: Vec<ResourceType>,
    resources: Vec<&str>,
    constraints: Option<Value>,
) -> Policy {
    policy(
        PolicyEffect::Deny,
        actions,
        resource_types,
        resources,
        constraints,
    )
}

/// A full-access policy over the whole Action×ResourceType grid (what a super
/// admin role carries; mirrors the bootstrap policy shape).
fn full_access() -> Policy {
    allow(
        Action::ALL.to_vec(),
        ResourceType::ALL.to_vec(),
        vec![],
        None,
    )
}

/// Request context with a `{attributes: {...}}` block (what constraints read).
fn attrs(pairs: &[(&str, &str)]) -> Value {
    let map: serde_json::Map<String, Value> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
        .collect();
    json!({ "attributes": map })
}

struct Case {
    name: &'static str,
    policies: Vec<Policy>,
    action: Action,
    resource_type: ResourceType,
    resource_id: Option<&'static str>,
    context: Value,
    expect: PolicyDecision,
}

fn check(case: &Case) {
    let got = evaluate_policies_with_metadata(
        &case.policies,
        case.action.clone(),
        case.resource_type.clone(),
        case.resource_id,
        &case.context,
    )
    .decision;
    assert_eq!(
        got, case.expect,
        "scenario `{}`: expected {:?}, got {:?}",
        case.name, case.expect, got
    );
}

// Device-role / site / protocol identifiers used across the attribute scenarios.
const RU1: &str = "role-ru1";
const RU2: &str = "role-ru2";
const RU3: &str = "role-ru3";
const RU4: &str = "role-ru4";
const SITE_A: &str = "site-a";
const SITE_B: &str = "site-b";

/// Attribute constraint: object's role_id ∈ {given roles} (an `or` of equalities).
fn role_in(roles: &[&str]) -> Value {
    json!({ "or": roles.iter().map(|r| json!({"attributes": {"role_id": r}})).collect::<Vec<_>>() })
}

// ── the matrix ───────────────────────────────────────────────────────────────

/// Coarse RBAC + subject/resource-id/constraint scenarios the engine supports
/// AND the enforcement sites already feed today. All must pass.
fn expressible_and_wired() -> Vec<Case> {
    let ct = ResourceType::ConnectionTarget;
    vec![
        // ── P1: full access to everything ──
        Case {
            name: "P1 full-access role may update any inventory",
            policies: vec![full_access()],
            action: Action::Update,
            resource_type: ResourceType::Inventory,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "P1 full-access role may manage policies",
            policies: vec![full_access()],
            action: Action::ManagePolicies,
            resource_type: ResourceType::Policy,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        // ── P2: edit all inventory, no admin ──
        Case {
            name: "P2 inventory editor may update inventory",
            policies: vec![allow(
                vec![Action::Read, Action::Create, Action::Update, Action::Delete],
                vec![ResourceType::Inventory],
                vec![],
                None,
            )],
            action: Action::Update,
            resource_type: ResourceType::Inventory,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "P2 inventory editor may NOT manage policies (no admin)",
            policies: vec![allow(
                vec![Action::Read, Action::Create, Action::Update, Action::Delete],
                vec![ResourceType::Inventory],
                vec![],
                None,
            )],
            action: Action::ManagePolicies,
            resource_type: ResourceType::Policy,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
        // ── P5: view + edit inventory, but never connect ──
        Case {
            name: "P5 inventory-only may read inventory",
            policies: vec![allow(
                vec![Action::Read, Action::Update],
                vec![ResourceType::Inventory],
                vec![],
                None,
            )],
            action: Action::Read,
            resource_type: ResourceType::Inventory,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "P5 inventory-only may NOT connect to a target",
            policies: vec![allow(
                vec![Action::Read, Action::Update],
                vec![ResourceType::Inventory],
                vec![],
                None,
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:host-1"),
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
        // ── auditor ──
        Case {
            name: "auditor may view audit, nothing else grants mutate",
            policies: vec![allow(
                vec![Action::Read, Action::ViewAudit],
                vec![ResourceType::AuditEvent],
                vec![],
                None,
            )],
            action: Action::Update,
            resource_type: ResourceType::Inventory,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
        // ── per-target UUID scoping ──
        Case {
            name: "single-host operator may connect to the granted host",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec!["connection_target:host-1"],
                None,
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:host-1"),
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "single-host operator may NOT connect to a different host",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec!["connection_target:host-1"],
                None,
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:host-2"),
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "regex host-set operator may connect to a matching host",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec!["re:connection_target:host-(1|2|3)"],
                None,
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:host-2"),
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        // ── deny-override ──
        Case {
            name: "deny beats a broad allow on a specific host",
            policies: vec![
                allow(vec![Action::Connect], vec![ct.clone()], vec![], None),
                deny(
                    vec![Action::Connect],
                    vec![ct.clone()],
                    vec!["connection_target:jump-box"],
                    None,
                ),
            ],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:jump-box"),
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "default deny when no policy matches",
            policies: vec![allow(
                vec![Action::Read],
                vec![ResourceType::Inventory],
                vec![],
                None,
            )],
            action: Action::Delete,
            resource_type: ResourceType::Playbook,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
        // ── ip / time constraints (fed by the request path today) ──
        Case {
            name: "office-only connect allowed from an in-range ip",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(json!({"ip_range": "10.0.0.0/8"})),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:host-1"),
            context: json!({"ip": "10.4.5.6"}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "office-only connect denied from an out-of-range ip",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(json!({"ip_range": "10.0.0.0/8"})),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:host-1"),
            context: json!({"ip": "203.0.113.9"}),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "maintenance-window allow inside the window",
            policies: vec![allow(
                vec![Action::Execute],
                vec![ResourceType::Playbook],
                vec![],
                Some(
                    json!({"time_window": {"start": "2026-01-01T00:00:00Z", "end": "2030-01-01T00:00:00Z"}}),
                ),
            )],
            action: Action::Execute,
            resource_type: ResourceType::Playbook,
            resource_id: None,
            context: json!({"timestamp": "2026-07-13T12:00:00Z"}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "maintenance-window deny outside the window",
            policies: vec![allow(
                vec![Action::Execute],
                vec![ResourceType::Playbook],
                vec![],
                Some(
                    json!({"time_window": {"start": "2030-01-01T00:00:00Z", "end": "2031-01-01T00:00:00Z"}}),
                ),
            )],
            action: Action::Execute,
            resource_type: ResourceType::Playbook,
            resource_id: None,
            context: json!({"timestamp": "2026-07-13T12:00:00Z"}),
            expect: PolicyDecision::Deny,
        },
        // ── secret vs execute distinction ──
        Case {
            name: "automation runner may EXECUTE a secret (use in a run)",
            policies: vec![allow(
                vec![Action::Execute],
                vec![ResourceType::SecretVariable],
                vec![],
                None,
            )],
            action: Action::Execute,
            resource_type: ResourceType::SecretVariable,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "automation runner may NOT reveal (Secret) a secret value",
            policies: vec![allow(
                vec![Action::Execute],
                vec![ResourceType::SecretVariable],
                vec![],
                None,
            )],
            action: Action::Secret,
            resource_type: ResourceType::SecretVariable,
            resource_id: None,
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
        // ── scoped playbook operator (mirrors the seeded system SP policy) ──
        Case {
            name: "scheduler principal may execute only its bound playbook",
            policies: vec![allow(
                vec![Action::Execute, Action::Read, Action::Create],
                vec![
                    ResourceType::Playbook,
                    ResourceType::PlaybookTargetSet,
                    ResourceType::PlaybookRunGroup,
                ],
                vec!["pb-247", "ts-147", "scheduled"],
                None,
            )],
            action: Action::Execute,
            resource_type: ResourceType::Playbook,
            resource_id: Some("pb-247"),
            context: json!({}),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "scheduler principal may NOT execute a different playbook",
            policies: vec![allow(
                vec![Action::Execute],
                vec![ResourceType::Playbook],
                vec!["pb-247"],
                None,
            )],
            action: Action::Execute,
            resource_type: ResourceType::Playbook,
            resource_id: Some("pb-999"),
            context: json!({}),
            expect: PolicyDecision::Deny,
        },
    ]
}

/// ABAC scenarios — the ENGINE decides these correctly when the object's
/// attributes are in the context, and every
/// per-target ConnectionTarget enforcement site supplies them: connect create
/// and session lifecycle (facets + protocol), monitoring/launch-option reads
/// and playbook per-host checks (facets, no protocol), effective config
/// (facets + requested protocol), proxy-assignment read/update. See
/// `enforcement_gaps_documented()` for the remaining no-attribute boundary.
fn expressible_by_engine_via_attributes() -> Vec<Case> {
    let ct = ResourceType::ConnectionTarget;
    vec![
        // ── P3: like P2 but no access to device-roles RU1/RU2 (deny by role) ──
        Case {
            name: "P3 deny-role: connect to an RU1 device is denied",
            policies: vec![
                allow(vec![Action::Connect], vec![ct.clone()], vec![], None),
                deny(
                    vec![Action::Connect],
                    vec![ct.clone()],
                    vec![],
                    Some(role_in(&[RU1, RU2])),
                ),
            ],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-x"),
            context: attrs(&[("role_id", RU1)]),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "P3 deny-role: connect to an RU5 device is allowed",
            policies: vec![
                allow(vec![Action::Connect], vec![ct.clone()], vec![], None),
                deny(
                    vec![Action::Connect],
                    vec![ct.clone()],
                    vec![],
                    Some(role_in(&[RU1, RU2])),
                ),
            ],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-y"),
            context: attrs(&[("role_id", "role-ru5")]),
            expect: PolicyDecision::Allow,
        },
        // ── P4: only device-role RU3 AND only VNC ──
        Case {
            name: "P4 RU3+VNC: vnc connect to an RU3 device is allowed",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(
                    json!({"and": [{"attributes": {"role_id": RU3}}, {"attributes": {"protocol": "vnc"}}]}),
                ),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:kiosk-1"),
            context: attrs(&[("role_id", RU3), ("protocol", "vnc")]),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "P4 RU3+VNC: ssh connect to the same RU3 device is denied",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(
                    json!({"and": [{"attributes": {"role_id": RU3}}, {"attributes": {"protocol": "vnc"}}]}),
                ),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:kiosk-1"),
            context: attrs(&[("role_id", RU3), ("protocol", "ssh")]),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "P4 RU3+VNC: vnc connect to a non-RU3 device is denied",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(
                    json!({"and": [{"attributes": {"role_id": RU3}}, {"attributes": {"protocol": "vnc"}}]}),
                ),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:server-9"),
            context: attrs(&[("role_id", RU4), ("protocol", "vnc")]),
            expect: PolicyDecision::Deny,
        },
        // ── P6: connect only on sites S1/S2 and only device-roles RU3/RU4 ──
        Case {
            name: "P6 site+role: connect to RU4 on site A is allowed",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(
                    json!({"and": [role_in(&[RU3, RU4]), {"or": [{"attributes": {"site_id": SITE_A}}, {"attributes": {"site_id": SITE_B}}]}]}),
                ),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-a"),
            context: attrs(&[("role_id", RU4), ("site_id", SITE_A)]),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "P6 site+role: connect to RU4 on a non-listed site is denied",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(
                    json!({"and": [role_in(&[RU3, RU4]), {"or": [{"attributes": {"site_id": SITE_A}}, {"attributes": {"site_id": SITE_B}}]}]}),
                ),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-c"),
            context: attrs(&[("role_id", RU4), ("site_id", "site-c")]),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "P6 site+role: connect to RU1 on an allowed site is denied (role not in set)",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(
                    json!({"and": [role_in(&[RU3, RU4]), {"or": [{"attributes": {"site_id": SITE_A}}, {"attributes": {"site_id": SITE_B}}]}]}),
                ),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-d"),
            context: attrs(&[("role_id", RU1), ("site_id", SITE_A)]),
            expect: PolicyDecision::Deny,
        },
        // ── tenant / platform / status scoping (all equality attributes) ──
        Case {
            name: "tenant-scoped connect allowed within the tenant",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"tenant_id": "tenant-acme"}})),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-e"),
            context: attrs(&[("tenant_id", "tenant-acme")]),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "tenant-scoped connect denied for another tenant",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"tenant_id": "tenant-acme"}})),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-f"),
            context: attrs(&[("tenant_id", "tenant-globex")]),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "platform-scoped connect allowed on linux",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"platform": "linux"}})),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-g"),
            context: attrs(&[("platform", "linux")]),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "active-only connect denied for a decommissioned device",
            policies: vec![allow(
                vec![Action::Connect],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"status": "active"}})),
            )],
            action: Action::Connect,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-h"),
            context: attrs(&[("status", "decommissioning")]),
            expect: PolicyDecision::Deny,
        },
        // ── observe(read)-only vs connect on a role ──
        Case {
            name: "read-only-on-RU3 allows monitoring read but the connect policy is separate",
            policies: vec![allow(
                vec![Action::Read],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"role_id": RU3}})),
            )],
            action: Action::Read,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-i"),
            context: attrs(&[("role_id", RU3)]),
            expect: PolicyDecision::Allow,
        },
        // ── monitoring read scoped by site (P2/P5-style): the monitor site
        //    feeds facets without protocol, so site policies apply as-is ──
        Case {
            name: "site-scoped read allows monitoring an in-site host",
            policies: vec![allow(
                vec![Action::Read],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"site_id": SITE_A}})),
            )],
            action: Action::Read,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-j"),
            context: attrs(&[("site_id", SITE_A), ("role_id", RU1)]),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "site-scoped read denies monitoring an out-of-site host",
            policies: vec![allow(
                vec![Action::Read],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"site_id": SITE_A}})),
            )],
            action: Action::Read,
            resource_type: ct.clone(),
            resource_id: Some("connection_target:dev-k"),
            context: attrs(&[("site_id", SITE_B), ("role_id", RU1)]),
            expect: PolicyDecision::Deny,
        },
        // ── playbook per-host check (P6-style): the execute site runs a Read ×
        //    ConnectionTarget check per resolved host with the host's facets
        //    and no protocol; a protocol-constrained policy therefore can never
        //    satisfy a playbook launch (fail-closed), while site+role scoping
        //    gates which hosts a run may touch ──
        Case {
            name: "site+role-scoped read admits a matching host into a playbook run",
            policies: vec![allow(
                vec![Action::Read],
                vec![ct.clone()],
                vec![],
                Some(json!({"and": [role_in(&[RU3, RU4]), {"attributes": {"site_id": SITE_A}}]})),
            )],
            action: Action::Read,
            resource_type: ct.clone(),
            resource_id: Some("host-1"),
            context: attrs(&[("role_id", RU4), ("site_id", SITE_A)]),
            expect: PolicyDecision::Allow,
        },
        Case {
            name: "site+role-scoped read rejects an out-of-scope host from a playbook run",
            policies: vec![allow(
                vec![Action::Read],
                vec![ct.clone()],
                vec![],
                Some(json!({"and": [role_in(&[RU3, RU4]), {"attributes": {"site_id": SITE_A}}]})),
            )],
            action: Action::Read,
            resource_type: ct.clone(),
            resource_id: Some("host-2"),
            context: attrs(&[("role_id", RU1), ("site_id", SITE_A)]),
            expect: PolicyDecision::Deny,
        },
        Case {
            name: "protocol-constrained connect policy grants nothing at a protocol-less site",
            policies: vec![allow(
                vec![Action::Read],
                vec![ct.clone()],
                vec![],
                Some(json!({"attributes": {"protocol": "vnc"}})),
            )],
            action: Action::Read,
            resource_type: ct.clone(),
            // The monitor/execute sites resolve facets but never a protocol, so
            // the attribute map has no `protocol` key and this policy is inert.
            resource_id: Some("connection_target:dev-l"),
            context: attrs(&[("role_id", RU3), ("site_id", SITE_A)]),
            expect: PolicyDecision::Deny,
        },
    ]
}

#[test]
fn permission_matrix_coarse_and_wired_scenarios() {
    for case in expressible_and_wired() {
        check(&case);
    }
}

#[test]
fn permission_matrix_attribute_scenarios_the_engine_supports() {
    for case in expressible_by_engine_via_attributes() {
        check(&case);
    }
}

/// Documents — in one enforced place — the boundary between what the ENGINE can
/// decide and what the ENFORCEMENT sites feed. Every
/// per-target `ConnectionTarget` site feeds the object's attributes: connect
/// create, session resume/health/client-credentials and the SSH file tools
/// (with the session's protocol), monitoring reads, the metric-query live
/// test, playbook per-host checks, launch-option reads, effective config
/// (with the requested protocol), and proxy-assignment read/update.
/// The REMAINING boundary this test pins down: an `ensure_access` call that
/// passes no attributes (all sites for OTHER resource types, and type-level
/// checks) keeps attribute-scoped policies fail-closed — such a policy grants
/// nothing there. That is intentional: `constraint_attribute_keys` in the
/// vocabulary returns an empty set for those resource types, so the write-time
/// validator rejects authoring such constraints in the first place. If this
/// assertion ever flips, the engine's no-attribute semantics changed — audit
/// every enforcement site before shipping.
#[test]
fn enforcement_gaps_documented() {
    let ct = ResourceType::ConnectionTarget;
    // An allow policy scoped to protocol=vnc, evaluated with the context an
    // attribute-less `ensure_access` site produces: no `attributes` block.
    let vnc_only = allow(
        vec![Action::Connect],
        vec![ct.clone()],
        vec![],
        Some(json!({"attributes": {"protocol": "vnc"}})),
    );
    let attributeless_site_context = json!({}); // ensure_access passes attributes = None
    let decision = evaluate_policies_with_metadata(
        &[vnc_only],
        Action::Connect,
        ct,
        Some("connection_target:kiosk-1"),
        &attributeless_site_context,
    )
    .decision;
    assert_eq!(
        decision,
        PolicyDecision::Deny,
        "BOUNDARY: an object-attribute constraint must stay fail-closed when an \
         enforcement site supplies no attributes. If this flips to Allow, the \
         engine's no-attribute semantics changed — audit every ensure_access \
         site (all non-ConnectionTarget sites rely on this) before shipping."
    );
}
