//! Access-control vocabulary registry — the single source of truth for the
//! permission dictionary (Action × ResourceType).
//!
//! Everything that needs to know which permissions exist, which are grantable to
//! a human API token, and which resource types are actually enforced derives
//! from [`resource_governance`] rather than hand-maintaining a parallel list.
//! Historically the same dictionary was hand-copied across the PAT scope catalog
//! (a 106-pair literal), the bootstrap full-access policy, the policy editor, the
//! token picker, i18n label maps and the docs — and it drifted three separate
//! times (the Monitoring `TOKEN_SCOPE_DENIED` incident, the policy-UI resource
//! gap, the docs missing `OBSERVE`).
//!
//! Anti-drift contract:
//! - [`resource_governance`] is a wildcard-free `match` over every
//!   [`ResourceType`] variant, so adding a variant fails to compile until it is
//!   described here (status + PAT-grantable actions).
//! - [`ResourceType`]/[`Action`] derive `strum::EnumIter`; the parity tests below
//!   force `ALL` completeness and `parse` round-trips off that iterator, so a new
//!   variant cannot silently drop out of the capability grid or the wire parser.
//!
//! The two governance axes are independent:
//! - `pat_grantable_actions` — the human-PAT grant surface. Empty means "never
//!   grantable via an API token" (a machine identity such as
//!   [`ResourceType::PlaybookServicePrincipal`] is still fully enforced via
//!   session RBAC — it just cannot be minted through a token, a
//!   privilege-escalation channel we deliberately keep session-only).
//! - `status` — whether a first-party endpoint actually enforces the resource
//!   today. `Enforced` is machine-assertable (a gate can confirm an
//!   `ensure_*_access` fires); `Reserved` is CURATED — declared but not yet
//!   reachable, reviewed by a human rather than proven by a test.
//!
//! # Extension guide — adding a NEW PERMISSIONED ENTITY (ResourceType)
//!
//! The system is designed so a new entity is added by describing it, not by
//! rewriting call sites. Every step below is guarded by a gate that goes red
//! when skipped — do them in order and the gates walk you through:
//!
//! 1. Add the variant to [`ResourceType`] (`domain/resource.rs`).
//!    → the wildcard-free `match` in [`resource_governance`] STOPS COMPILING
//!    until you describe status + PAT-grantable actions here.
//! 2. If the entity needs a new verb, add it to [`Action`] the same way.
//! 3. Regenerate the OpenAPI contract (`REGEN_OPENAPI=1 cargo test
//!    openapi_yaml_matches_apidoc`) and the web schema (`pnpm run codegen`).
//!    → `contract_enums_match_the_rust_vocabulary` (this file) is red until
//!    the contract carries the new enum value.
//! 4. Add i18n label + description (EN + RU) under
//!    `accessControl.policies.{resourceTypes,resourceDescriptions}` in
//!    `apps/web/src/shared/i18n/locales/{en,ru}.ts`.
//!    → the web parity gate `src/shared/i18n/__tests__/vocabulary-coverage.spec.ts`
//!    is red until both locales cover every contract enum value.
//! 5. Regenerate the docs matrix (`REGEN_ACCESS_MATRIX=1 cargo test -p
//!    piping-backend --lib access_matrix`).
//!    → `access_matrix_doc_is_fresh` is red until the matrix matches.
//! 6. Wire the actual enforcement (`ensure_access(action, resource, …)` at the
//!    new endpoints), then flip the registry entry to `Enforced`.
//!
//! Nothing else needs touching: the PAT scope catalog, the bootstrap
//! full-access policy, the policy-editor picker, the token-scope picker and
//! the docs matrix all DERIVE from this registry at runtime or regen time.
//!
//! # Extension guide — adding a NEW OBJECT-ATTRIBUTE KEY (hierarchy facet)
//!
//! Attribute keys are what "scope by site / region / device role" is made of:
//! the policy editor's inventory-hierarchy tabs write `attributes` constraints
//! keyed by these names, the per-target enforcement sites feed the same keys
//! into the evaluation context, and the list read-models compile them into
//! row-visibility SQL. To add one (say `rack_id`), follow the `region_id`
//! commit as the reference implementation:
//!
//! 1. Declare `ATTR_RACK_ID` below and list it in [`constraint_attribute_keys`]
//!    for every resource type whose enforcement site will feed it.
//!    → write-time policy validation and the editor's tab gating derive from
//!    this list; an undeclared key is rejected at policy save.
//! 2. Per-target checks: add the field to `ConnectionTargetFacets`
//!    and populate it (`api/connection_authz_attributes.rs`). If the value is
//!    not a column on the device/VM row itself (like region), resolve it in
//!    `enrich_facets_with_region`-style enrichment so deep-link gates and the
//!    engine see the same value the SQL resolves.
//!    → the struct literal `From` impls STOP COMPILING until the field is set;
//!    `attribute_map_covers_every_declared_connection_target_key` (tests
//!    there) is red if the key never lands in the evaluation map.
//! 3. Row visibility: add a `Facet` variant + `RowFacets` field +
//!    `from_attribute_key` arm (`domain/visibility.rs`), a `VisibilityColumns`
//!    field (`…/repositories/visibility_sql.rs`) and its SQL expression in all
//!    four read-model column sets (connections / devices / VMs / discovery).
//!    A derived value is a scalar subquery — see the `region_id` expressions.
//!    → `RowFacets::facet` and `VisibilityColumns::column` are wildcard-free
//!    matches and the column-set literals are exhaustive struct literals, so
//!    each of these sites STOPS COMPILING until handled;
//!    `every_declared_id_key_compiles_to_a_row_facet` (visibility tests) is
//!    red if the key silently falls out of predicate compilation; extend the
//!    DB fidelity suite (`tests/integration_connection_target_visibility_sql.rs`)
//!    with the new facet's NULL semantics.
//! 4. Frontend: register the key in
//!    `apps/web/src/features/access-control/hierarchyScope.ts` (entity source +
//!    tab definition) and add `constraintKeyLabels.<key>` to both locales.
//!    → `hierarchyScope.spec.ts` is red until the key has an entity source and
//!    RU+EN labels.

use strum::IntoEnumIterator;

use super::action::Action;
use super::resource::ResourceType;

/// Whether a resource type is reachable by a first-party enforcement check today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementStatus {
    /// A first-party endpoint gates on this resource via an `ensure_*_access`
    /// call. Machine-checkable.
    Enforced,
    /// Declared in the vocabulary but not yet reachable by any enforcement check.
    /// Curated (human-reviewed), not test-proven.
    Reserved,
}

/// Governance facts for one resource type. The only constructor is
/// [`resource_governance`]; keeping the struct here (not per call site) is what
/// makes the registry the single source of truth.
#[derive(Debug, Clone, Copy)]
pub struct ResourceGovernance {
    pub status: EnforcementStatus,
    /// Actions a human API token may be granted on this resource, in a stable
    /// display order. Empty = never grantable via PAT.
    pub pat_grantable_actions: &'static [Action],
}

/// The single source of truth for per-resource governance.
///
/// Wildcard-free on purpose: a new [`ResourceType`] variant will not compile
/// until its governance is declared here.
pub fn resource_governance(resource: ResourceType) -> ResourceGovernance {
    use Action::*;
    use EnforcementStatus::*;
    use ResourceType::*;

    match resource {
        ConnectionTarget => ResourceGovernance {
            status: Enforced,
            // `DeviceUsers` is the neutral on-device account plane (video
            // recorders and Linux hosts), governed on the connection target.
            pat_grantable_actions: &[Read, Connect, Observe, Execute, Update, Secret, DeviceUsers],
        },
        ConnectionSession => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        ConnectionFilterPreset => ResourceGovernance {
            status: Enforced,
            // Admin curation of shared presets; personal presets are unscoped.
            pat_grantable_actions: &[Update, Delete],
        },
        User => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        Group => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        Role => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete, ManageRoles],
        },
        Policy => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete, ManagePolicies],
        },
        Inventory => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete, Execute],
        },
        ConfigContext => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        SecretVariable => ResourceGovernance {
            status: Enforced,
            // `Secret` = reveal a value in the UI; `Execute` = export a secret into
            // a playbook run env (gate C3) WITHOUT being able to read it; the CRUD
            // trio manages the named secret identity/metadata (value set/clear stays
            // on the write-only `Secret` route).
            pat_grantable_actions: &[Secret, Update, Read, Create, Delete, Execute],
        },
        AuditEvent => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, ViewAudit],
        },
        Keycloak => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Update],
        },
        Webhook => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Update],
        },
        Assistant => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Update, Execute, ViewAudit],
        },
        AssistantConversation => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[ViewAudit],
        },
        Playbook => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete, Execute],
        },
        PlaybookVersion => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read],
        },
        PlaybookAssignment => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        PlaybookTargetSet => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        PlaybookRunGroup => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create],
        },
        PlaybookRun => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Update],
        },
        PlaybookSchedule => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        PlaybookArtifact => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read],
        },
        // Machine identities: fully enforced via session RBAC, but deliberately
        // NOT grantable through a human PAT — a token must not mint/revoke machine
        // identities (privilege-escalation channel). Locked by the compliance pin
        // `playbooks_service_principal_security`.
        PlaybookServicePrincipal => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[],
        },
        PlaybookExecutionEnvironment => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        PlaybookProject => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        PlaybookJobTemplate => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        License => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Update],
        },
        Settings => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Update],
        },
        Branding => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Update],
        },
        ProxyNode => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        Monitoring => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Create, Update, Delete],
        },
        Backup => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[Read, Update],
        },
        // CCTV permissions are intentionally namespaced on the wire as
        // `cctv:<operation>` and scoped per connection target.
        Cctv => ResourceGovernance {
            status: Enforced,
            pat_grantable_actions: &[CctvView, CctvRecordings, CctvExport, CctvSettings, CctvPtz],
        },
    }
}

// ── Constraint attribute vocabulary (ABAC) ──────────────────────────────────
//
// The keys an `attributes` policy constraint may reference, and the resource
// types whose enforcement sites actually populate them. Defining them here (the
// single source of truth) lets: the enforcement site build the same keys, the
// policy write path REJECT an unknown key (so a `role` vs `role_id` typo fails
// loudly instead of silently never matching), and the UI offer the exact set.

pub const ATTR_ROLE_ID: &str = "role_id";
pub const ATTR_SITE_ID: &str = "site_id";
/// The target's site's DIRECT region (`dcim_site.region_id`). Exact-id match:
/// a policy scoped to a parent region does NOT cover sites attached to its
/// child regions — scope on the child region (or on the sites) instead.
pub const ATTR_REGION_ID: &str = "region_id";
pub const ATTR_TENANT_ID: &str = "tenant_id";
pub const ATTR_PLATFORM_ID: &str = "platform_id";
pub const ATTR_CLUSTER_ID: &str = "cluster_id";
pub const ATTR_STATUS: &str = "status";
pub const ATTR_PROTOCOL: &str = "protocol";

/// The object-attribute keys a policy constraint may scope on for a resource
/// type — i.e. the keys the enforcement site for that resource actually feeds
/// into the evaluation context. An empty set means attribute scoping is not
/// wired for that resource yet, so an `attributes` constraint there is rejected
/// at write time rather than silently never matching.
///
/// The wildcard default is safe: a resource type with no declared attribute keys
/// simply cannot carry object-attribute constraints (fail-closed), so a newly
/// added resource does not open a gap by omission — it just cannot be
/// attribute-scoped until its enforcement site is wired and it is listed here.
pub fn constraint_attribute_keys(resource: ResourceType) -> &'static [&'static str] {
    use ResourceType::*;
    match resource {
        // Wired by the connect enforcement site. The CCTV
        // write gate feeds the same connection-target attributes into its
        // evaluation context, so cctv policies may scope on the same keys.
        ConnectionTarget | Cctv => &[
            ATTR_ROLE_ID,
            ATTR_SITE_ID,
            ATTR_REGION_ID,
            ATTR_TENANT_ID,
            ATTR_PLATFORM_ID,
            ATTR_CLUSTER_ID,
            ATTR_STATUS,
            ATTR_PROTOCOL,
        ],
        _ => &[],
    }
}

/// The union of attribute keys valid for a policy spanning these resource types.
pub fn allowed_constraint_attribute_keys(
    resource_types: &[ResourceType],
) -> std::collections::HashSet<&'static str> {
    resource_types
        .iter()
        .flat_map(|resource| constraint_attribute_keys(resource.clone()))
        .copied()
        .collect()
}

/// The PAT scope catalog, derived from the registry. Iterates resources in
/// declaration order (`EnumIter`) and expands each resource's grantable actions,
/// so a resource whose governance changes automatically updates the catalog.
pub fn pat_grantable_pairs() -> Vec<(Action, ResourceType)> {
    let mut pairs = Vec::new();
    for resource in ResourceType::iter() {
        let governance = resource_governance(resource.clone());
        for action in governance.pat_grantable_actions {
            pairs.push((action.clone(), resource.clone()));
        }
    }
    pairs
}

/// Canonical PAT scope value. CCTV reverses the generic order intentionally:
/// `cctv:<operation>`; all other resources remain `<action>:<resource>`.
#[must_use]
pub fn pat_scope_wire_value(action: &Action, resource: &ResourceType) -> String {
    if resource == &ResourceType::Cctv && action.is_cctv_permission() {
        format!("cctv:{}", action.as_str())
    } else {
        format!("{}:{}", action.as_str(), resource.as_str())
    }
}

/// Whether an Action may be paired with this ResourceType.
#[must_use]
pub fn is_valid_permission_pair(action: &Action, resource: &ResourceType) -> bool {
    action.is_cctv_permission() == (resource == &ResourceType::Cctv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The PAT scope catalog exactly as it existed before the registry extraction
    /// (the former `user_api_token_scope_catalog_pairs()` literal). This is a
    /// behavior-preserving pin: the registry-derived catalog MUST reproduce this
    /// set byte-for-byte. Any intentional change to what is grantable belongs in a
    /// separate, reviewed diff that updates this fixture at the same time.
    const PINNED_PAT_CATALOG: &[(Action, ResourceType)] = &[
        (Action::Read, ResourceType::ConnectionTarget),
        (Action::Connect, ResourceType::ConnectionTarget),
        (Action::Observe, ResourceType::ConnectionTarget),
        (Action::Execute, ResourceType::ConnectionTarget),
        (Action::Update, ResourceType::ConnectionTarget),
        (Action::Secret, ResourceType::ConnectionTarget),
        (Action::DeviceUsers, ResourceType::ConnectionTarget),
        (Action::Read, ResourceType::ConnectionSession),
        (Action::Create, ResourceType::ConnectionSession),
        (Action::Update, ResourceType::ConnectionSession),
        (Action::Delete, ResourceType::ConnectionSession),
        (Action::Read, ResourceType::Inventory),
        (Action::Create, ResourceType::Inventory),
        (Action::Update, ResourceType::Inventory),
        (Action::Delete, ResourceType::Inventory),
        (Action::Execute, ResourceType::Inventory),
        (Action::Read, ResourceType::ConfigContext),
        (Action::Create, ResourceType::ConfigContext),
        (Action::Update, ResourceType::ConfigContext),
        (Action::Delete, ResourceType::ConfigContext),
        (Action::Read, ResourceType::User),
        (Action::Create, ResourceType::User),
        (Action::Update, ResourceType::User),
        (Action::Delete, ResourceType::User),
        (Action::Read, ResourceType::Group),
        (Action::Create, ResourceType::Group),
        (Action::Update, ResourceType::Group),
        (Action::Delete, ResourceType::Group),
        (Action::Read, ResourceType::Role),
        (Action::Create, ResourceType::Role),
        (Action::Update, ResourceType::Role),
        (Action::Delete, ResourceType::Role),
        (Action::ManageRoles, ResourceType::Role),
        (Action::Read, ResourceType::Policy),
        (Action::Create, ResourceType::Policy),
        (Action::Update, ResourceType::Policy),
        (Action::Delete, ResourceType::Policy),
        (Action::ManagePolicies, ResourceType::Policy),
        (Action::Read, ResourceType::AuditEvent),
        (Action::ViewAudit, ResourceType::AuditEvent),
        (Action::Read, ResourceType::Keycloak),
        (Action::Update, ResourceType::Keycloak),
        (Action::Secret, ResourceType::SecretVariable),
        (Action::Update, ResourceType::SecretVariable),
        (Action::Read, ResourceType::SecretVariable),
        (Action::Create, ResourceType::SecretVariable),
        (Action::Delete, ResourceType::SecretVariable),
        (Action::Execute, ResourceType::SecretVariable),
        (Action::Read, ResourceType::Webhook),
        (Action::Update, ResourceType::Webhook),
        (Action::Read, ResourceType::Playbook),
        (Action::Create, ResourceType::Playbook),
        (Action::Update, ResourceType::Playbook),
        (Action::Delete, ResourceType::Playbook),
        (Action::Execute, ResourceType::Playbook),
        (Action::Read, ResourceType::PlaybookVersion),
        (Action::Read, ResourceType::PlaybookAssignment),
        (Action::Create, ResourceType::PlaybookAssignment),
        (Action::Update, ResourceType::PlaybookAssignment),
        (Action::Delete, ResourceType::PlaybookAssignment),
        (Action::Read, ResourceType::PlaybookTargetSet),
        (Action::Create, ResourceType::PlaybookTargetSet),
        (Action::Update, ResourceType::PlaybookTargetSet),
        (Action::Delete, ResourceType::PlaybookTargetSet),
        (Action::Read, ResourceType::PlaybookRunGroup),
        (Action::Create, ResourceType::PlaybookRunGroup),
        (Action::Read, ResourceType::PlaybookRun),
        (Action::Update, ResourceType::PlaybookRun),
        (Action::Read, ResourceType::PlaybookSchedule),
        (Action::Create, ResourceType::PlaybookSchedule),
        (Action::Update, ResourceType::PlaybookSchedule),
        (Action::Delete, ResourceType::PlaybookSchedule),
        (Action::Read, ResourceType::PlaybookArtifact),
        (Action::Read, ResourceType::PlaybookExecutionEnvironment),
        (Action::Create, ResourceType::PlaybookExecutionEnvironment),
        (Action::Update, ResourceType::PlaybookExecutionEnvironment),
        (Action::Delete, ResourceType::PlaybookExecutionEnvironment),
        (Action::Read, ResourceType::PlaybookProject),
        (Action::Create, ResourceType::PlaybookProject),
        (Action::Update, ResourceType::PlaybookProject),
        (Action::Delete, ResourceType::PlaybookProject),
        (Action::Read, ResourceType::PlaybookJobTemplate),
        (Action::Create, ResourceType::PlaybookJobTemplate),
        (Action::Update, ResourceType::PlaybookJobTemplate),
        (Action::Delete, ResourceType::PlaybookJobTemplate),
        (Action::Read, ResourceType::Monitoring),
        (Action::Create, ResourceType::Monitoring),
        (Action::Update, ResourceType::Monitoring),
        (Action::Delete, ResourceType::Monitoring),
        (Action::Read, ResourceType::ProxyNode),
        (Action::Create, ResourceType::ProxyNode),
        (Action::Update, ResourceType::ProxyNode),
        (Action::Delete, ResourceType::ProxyNode),
        (Action::Read, ResourceType::Settings),
        (Action::Update, ResourceType::Settings),
        (Action::Update, ResourceType::Branding),
        (Action::Read, ResourceType::License),
        (Action::Update, ResourceType::License),
        (Action::Read, ResourceType::Backup),
        (Action::Update, ResourceType::Backup),
        (Action::Read, ResourceType::Assistant),
        (Action::Update, ResourceType::Assistant),
        (Action::Execute, ResourceType::Assistant),
        (Action::ViewAudit, ResourceType::Assistant),
        (Action::ViewAudit, ResourceType::AssistantConversation),
        (Action::Update, ResourceType::ConnectionFilterPreset),
        (Action::Delete, ResourceType::ConnectionFilterPreset),
        (Action::CctvView, ResourceType::Cctv),
        (Action::CctvRecordings, ResourceType::Cctv),
        (Action::CctvExport, ResourceType::Cctv),
        (Action::CctvSettings, ResourceType::Cctv),
        (Action::CctvPtz, ResourceType::Cctv),
    ];

    #[test]
    fn derived_catalog_reproduces_the_pinned_pat_catalog_exactly() {
        let derived: HashSet<(Action, ResourceType)> = pat_grantable_pairs().into_iter().collect();
        let pinned: HashSet<(Action, ResourceType)> = PINNED_PAT_CATALOG.iter().cloned().collect();
        assert_eq!(
            PINNED_PAT_CATALOG.len(),
            112,
            "the pinned catalog must include the five canonical cctv:* scopes \
             plus the neutral device_users:connection_target"
        );
        assert_eq!(
            derived.len(),
            112,
            "registry derived {} PAT pairs, expected 112 — grantability changed",
            derived.len()
        );
        assert_eq!(
            derived, pinned,
            "registry-derived PAT catalog diverged from the pinned set"
        );
    }

    #[test]
    fn cctv_pat_catalog_uses_only_namespaced_operations() {
        let cctv_actions = pat_grantable_pairs()
            .into_iter()
            .filter_map(|(action, resource)| (resource == ResourceType::Cctv).then_some(action))
            .collect::<Vec<_>>();
        assert_eq!(
            cctv_actions,
            vec![
                Action::CctvView,
                Action::CctvRecordings,
                Action::CctvExport,
                Action::CctvSettings,
                Action::CctvPtz,
            ]
        );
        assert_eq!(
            pat_scope_wire_value(&Action::CctvExport, &ResourceType::Cctv),
            "cctv:export"
        );
        // The neutral device-users grant rides the connection target, not CCTV.
        assert_eq!(
            pat_scope_wire_value(&Action::DeviceUsers, &ResourceType::ConnectionTarget),
            "device_users:connection_target"
        );
        assert!(is_valid_permission_pair(
            &Action::DeviceUsers,
            &ResourceType::ConnectionTarget
        ));
        assert!(!is_valid_permission_pair(
            &Action::CctvExport,
            &ResourceType::ConnectionTarget
        ));
        assert!(!is_valid_permission_pair(
            &Action::Read,
            &ResourceType::Cctv
        ));
    }

    #[test]
    fn no_pat_scope_targets_a_reserved_resource() {
        for (action, resource) in pat_grantable_pairs() {
            assert_eq!(
                resource_governance(resource.clone()).status,
                EnforcementStatus::Enforced,
                "reserved resource {resource:?} must not be PAT-grantable (action {action:?})"
            );
        }
    }

    #[test]
    fn resource_all_matches_enum_iter() {
        assert_eq!(
            ResourceType::ALL.to_vec(),
            ResourceType::iter().collect::<Vec<_>>(),
            "ResourceType::ALL drifted from the enum; regenerate it from the variants"
        );
    }

    #[test]
    fn action_all_matches_enum_iter() {
        assert_eq!(
            Action::ALL.to_vec(),
            Action::iter().collect::<Vec<_>>(),
            "Action::ALL drifted from the enum; regenerate it from the variants"
        );
    }

    #[test]
    fn resource_parse_roundtrips_every_variant() {
        for resource in ResourceType::iter() {
            assert_eq!(
                ResourceType::parse(resource.as_str()),
                Some(resource.clone()),
                "parse() has no arm for {resource:?} (as_str = {})",
                resource.as_str()
            );
        }
    }

    /// Extract a top-level component-schema string enum's values from the
    /// OpenAPI yaml (the `    <Name>:` block's `enum:` list). Whitespace-based,
    /// matching the two-space component indentation utoipa emits.
    fn contract_enum_values(yaml: &str, schema_name: &str) -> Vec<String> {
        let header = format!("\n    {schema_name}:\n");
        let start = yaml
            .find(&header)
            .unwrap_or_else(|| panic!("schema `{schema_name}` not found in contract"));
        let after = &yaml[start + header.len()..];
        let enum_at = after
            .find("      enum:\n")
            .unwrap_or_else(|| panic!("schema `{schema_name}` has no enum block"));
        let mut values = Vec::new();
        for line in after[enum_at + "      enum:\n".len()..].lines() {
            if let Some(value) = line.strip_prefix("      - ") {
                values.push(value.trim().to_string());
            } else {
                break;
            }
        }
        values
    }

    fn wire_form<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_string(value)
            .expect("serialize")
            .trim_matches('"')
            .to_string()
    }

    /// Anti-drift gate: the OpenAPI contract's Action/ResourceType enums must
    /// exactly match the Rust enums. Adding a variant compiles (the registry
    /// match forces that), but if `dump-openapi` was not re-run the contract —
    /// and every consumer generated from it (frontend types, the policy editor,
    /// docs) — silently drifts. This fails loudly until the contract is
    /// regenerated.
    #[test]
    fn contract_enums_match_the_rust_vocabulary() {
        let yaml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/contracts/piping.openapi.yaml"
        ))
        .expect("read openapi contract");

        let contract_resources: HashSet<String> = contract_enum_values(&yaml, "ResourceType")
            .into_iter()
            .collect();
        let rust_resources: HashSet<String> = ResourceType::iter().map(|r| wire_form(&r)).collect();
        assert_eq!(
            contract_resources, rust_resources,
            "ResourceType contract enum drifted from the Rust enum — re-run \
             `cargo run --bin dump-openapi > contracts/piping.openapi.yaml`"
        );

        let contract_actions: HashSet<String> =
            contract_enum_values(&yaml, "Action").into_iter().collect();
        let rust_actions: HashSet<String> = Action::iter().map(|a| wire_form(&a)).collect();
        assert_eq!(
            contract_actions, rust_actions,
            "Action contract enum drifted from the Rust enum — re-run \
             `cargo run --bin dump-openapi > contracts/piping.openapi.yaml`"
        );
    }
}

#[cfg(test)]
mod attribute_vocabulary_tests {
    use super::*;

    #[test]
    fn connection_target_is_attribute_scopable_others_are_not() {
        assert!(constraint_attribute_keys(ResourceType::ConnectionTarget).contains(&ATTR_ROLE_ID));
        assert!(constraint_attribute_keys(ResourceType::ConnectionTarget).contains(&ATTR_PROTOCOL));
        // Not yet wired at other enforcement sites → no attribute scoping.
        assert!(constraint_attribute_keys(ResourceType::Playbook).is_empty());
        assert!(constraint_attribute_keys(ResourceType::Inventory).is_empty());
    }

    #[test]
    fn allowed_keys_is_the_union_over_resource_types() {
        let allowed = allowed_constraint_attribute_keys(&[
            ResourceType::ConnectionTarget,
            ResourceType::Playbook,
        ]);
        assert!(allowed.contains(ATTR_SITE_ID));
        assert!(allowed.contains(ATTR_PROTOCOL));
        let none = allowed_constraint_attribute_keys(&[ResourceType::Playbook]);
        assert!(none.is_empty());
    }
}
