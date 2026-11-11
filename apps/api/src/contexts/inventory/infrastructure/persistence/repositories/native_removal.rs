//! Canonical Inventory owner of native-removal inspection, planning and bulk
//! execution.
//!
//! The sync context proves *which* native units it created and may ask for; this
//! module answers *what removing them would do to the native schema*. It never
//! accepts a sync proof type and never grants ownership: its only input is a set
//! of native `(table, id)` units that the caller already authorised.
//!
//! The three rules it enforces are:
//!
//! * every physical effect is read from PostgreSQL's own catalogs, not from a
//!   curated table list, so a new constraint cannot be silently ignored;
//! * every non-FK reference is read from its owner's own discriminator
//!   vocabulary (scope kind, consumer type, target/source kind, selector form),
//!   so an owner row is attributed to this unit only when the owner itself says
//!   it names this unit. A scope column or discriminator value outside those
//!   vocabularies is reported as [`NativeDependencyClass::UnattributedScope`]
//!   instead of being skipped;
//! * an effect may only touch a unit that is itself in the approved set — a
//!   retained child is never rewritten, cascaded or detached to make a delete
//!   succeed, and the approved set is never enlarged. Secret usage bindings are
//!   references like any other: native removal never detaches a secret, and the
//!   interactive path's detach behaviour is untouched;
//! * removal is executed children-first inside one savepoint with the deferred
//!   checks evaluated after the complete set, so a late failure rolls every
//!   native effect back together. Constraints are never disabled.
//!
//! A dependent row is locked and reported by the identity PostgreSQL declares
//! for its table — a single-column primary key, or the single-column unique
//! non-null key of a table that still exposes one — never by an assumed `id`.
//! A junction or binding with a composite key is either sampled by its real key
//! or refused with [`NativeBlocker::UnsupportedEffect`]; it is never silently
//! skipped.
//!
//! The inspector's observation set is derived, never curated: reference columns
//! come from the catalog, taggable/custom-field subjects from their owner
//! functions, and FK children from `pg_constraint`. That observation set is
//! fenced with `SHARE` locks before inspection and re-read afterwards, so a
//! catalogue change during inspection fails closed instead of leaving a new
//! reference form unobserved.
//!
//! Relationship membership and cable attachment removal is a holder `UPDATE`
//! issued by the typed relation owner, never permission to delete the holder.
//! This module models that as [`NativeRelationRemoval`] so the caller can require
//! holder Update authority and preserve local members.
//!
//! The interactive single-row delete path in [`super::delete_guard`] keeps its
//! behaviour and guards; both paths share the catalog reader, the deferred
//! validation and the blocker metadata instead of duplicating them.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::{Column, PgConnection, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::{
    NativeFieldSubject, native_field_subjects,
};
use crate::contexts::inventory::domain::content_type::{
    CONTENT_TYPES, ContentTypeDef, INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES,
};
use crate::contexts::shared::delete_guard::{
    DeleteAction, InboundDeleteEffect, delete_records_guarded_in_savepoint, inbound_delete_effects,
};
use crate::errors::AppError;

use super::delete_guard::inventory_blocker_metadata;

/// The custom-field definition owner table. `InventoryEntity::CustomField` targets
/// it, but it is not an inventory content type: it belongs to the custom-fields
/// catalogue, so the target catalogue alone would refuse it as an unknown table.
const CUSTOM_FIELD_DEFINITION_TABLE: &str = "custom_field";

/// Whether `table` is one of the native unit tables a removal may name: the
/// custom-field definition owner or a registered inventory content type.
fn is_native_unit_table(table: &str) -> bool {
    table == CUSTOM_FIELD_DEFINITION_TABLE
        || CONTENT_TYPES
            .iter()
            .any(|definition| definition.table == table)
}

/// One native record unit the caller has independently approved for removal.
///
/// `table` is a catalogue table name; it is validated against the inventory
/// registry, so a caller cannot name an arbitrary table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApprovedNativeUnit {
    pub table: &'static str,
    pub id: Uuid,
}

/// One relationship unit whose removal is a holder `UPDATE`, not a row delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRelationRemoval {
    pub holder_table: &'static str,
    pub holder_id: Uuid,
    pub kind: NativeRelationKind,
}

/// The typed relationship a source-owned absence asks to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeRelationKind {
    /// One native `tags text[]` membership of `tag_id` on the holder.
    Tag { tag_id: Uuid },
    /// One member of the native junction `table`.
    Junction {
        table: &'static str,
        member_id: Uuid,
    },
    /// One cable attachment edge on the holder, identified by cable and end.
    Cable { cable_id: Uuid, end: CableEndToken },
}

/// The native cable end vocabulary (`dcim_interface.cable_end`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CableEndToken {
    A,
    B,
}

impl CableEndToken {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            _ => Err(AppError::Validation(
                "native_removal: cable end must be A or B".to_string(),
            )),
        }
    }
}

/// Why a native effect is not executable as part of this set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeBlocker {
    /// A surviving child references an approved unit. Removing the parent would
    /// cascade, rewrite or refuse it — none of which this set may do.
    RetainedDependent {
        child_table: String,
        constraint: String,
        action: DeleteAction,
        count: i64,
        sample: Vec<Uuid>,
    },
    /// A typed non-FK reference exists outside the approved set.
    TypedDependency {
        class: NativeDependencyClass,
        table: String,
        count: i64,
        sample: Vec<Uuid>,
    },
    /// The effect form cannot be evaluated (composite reference, unknown action).
    UnsupportedEffect { constraint: String, detail: String },
    /// A relationship update cannot also delete its holder.
    RelationHolderInRemovalSet { table: String },
}

/// Non-FK reference classes this owner knows how to inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NativeDependencyClass {
    ContactAssignment,
    FhrpAssignment,
    ComponentTemplate,
    TagHolder,
    SecretConsumer,
    MonitoringScope,
    PlaybookReference,
    ConfigContextScope,
    ConnectionTarget,
    SessionOrHistory,
    CustomFieldValue,
    /// A native resource scoped to this unit (`ipam_prefix`,
    /// `virtualization_cluster`, `wireless_wirelesslan`, `ipam_vlangroup`): the
    /// dependent is a native inventory row and is retained, never rewritten.
    NativeScopeReference,
    /// A scope reference whose owning context this build cannot name.
    UnattributedScope,
}

impl NativeDependencyClass {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContactAssignment => "contact_assignment",
            Self::FhrpAssignment => "fhrp_assignment",
            Self::ComponentTemplate => "component_template",
            Self::TagHolder => "tag_holder",
            Self::SecretConsumer => "secret_consumer",
            Self::MonitoringScope => "monitoring_scope",
            Self::PlaybookReference => "playbook_reference",
            Self::ConfigContextScope => "config_context_scope",
            Self::ConnectionTarget => "connection_target",
            Self::SessionOrHistory => "session_or_history",
            Self::CustomFieldValue => "custom_field_value",
            Self::NativeScopeReference => "native_scope_reference",
            Self::UnattributedScope => "unattributed_scope",
        }
    }
}

/// One typed non-FK reference found for an approved unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDependency {
    pub class: NativeDependencyClass,
    pub table: String,
    pub count: i64,
    pub sample: Vec<Uuid>,
    pub effect: NativeDependencyEffect,
}

/// Whether a reference makes the approved removal non-executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDependencyEffect {
    /// The owner row is retained and names this unit, so removing the unit would
    /// leave it dangling or force a rewrite. The plan refuses.
    BlocksRemoval,
    /// The owner row records a historical identity and already renders without
    /// the referenced row: the connections owner calls terminal session states
    /// "history, not load", playbook runs copy `target_name`/`target_address`,
    /// and the audit projection falls back to the raw `target_id` text. Keeping
    /// such a row is the owner's normal state, so it has no native effect. This
    /// is not a waiver: the owner's own read path and status vocabulary are the
    /// evidence, and any open row of the same table still blocks.
    RetainedHistory,
}

/// How the plan intends to treat one inbound native effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedEffect {
    /// The child is in the approved set and is deleted explicitly, children
    /// first; PostgreSQL's cascade or rewrite never has to run.
    RemovedWithApprovedUnit { child_table: String, count: i64 },
    /// The child survives and must not be touched.
    Retained(NativeBlocker),
}

/// One unit scheduled for deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedDeletion {
    pub table: &'static str,
    pub id: Uuid,
}

/// The inspection result for one caller-supplied approved set.
#[derive(Debug, Clone)]
pub struct NativeRemovalPlan {
    approved: Vec<ApprovedNativeUnit>,
    /// Children-first deletion order.
    order: Vec<PlannedDeletion>,
    effects: Vec<PlannedEffect>,
    dependencies: Vec<NativeDependency>,
    relations: Vec<NativeRelationRemoval>,
    blockers: Vec<NativeBlocker>,
    /// Child tables that must be fenced so no new reference appears after
    /// inspection.
    fences: Vec<String>,
    /// Already absent rows: the source-owned absence was applied before.
    already_absent: Vec<ApprovedNativeUnit>,
    /// Set when the approved set contains a cycle that only deferred constraints
    /// can absorb.
    deferred_cycle: bool,
}

impl NativeRemovalPlan {
    #[must_use]
    pub fn approved(&self) -> &[ApprovedNativeUnit] {
        &self.approved
    }

    #[must_use]
    pub fn order(&self) -> &[PlannedDeletion] {
        &self.order
    }

    #[must_use]
    pub fn effects(&self) -> &[PlannedEffect] {
        &self.effects
    }

    #[must_use]
    pub fn dependencies(&self) -> &[NativeDependency] {
        &self.dependencies
    }

    #[must_use]
    pub fn relations(&self) -> &[NativeRelationRemoval] {
        &self.relations
    }

    #[must_use]
    pub fn blockers(&self) -> &[NativeBlocker] {
        &self.blockers
    }

    #[must_use]
    pub fn fences(&self) -> &[String] {
        &self.fences
    }

    #[must_use]
    pub fn already_absent(&self) -> &[ApprovedNativeUnit] {
        &self.already_absent
    }

    #[must_use]
    pub fn deferred_cycle(&self) -> bool {
        self.deferred_cycle
    }

    /// A plan is executable only when every effect stays inside the approved set
    /// and every dependency class is covered by a wired owner.
    pub fn ensure_executable(&self) -> Result<(), AppError> {
        if self.blockers.is_empty() {
            return Ok(());
        }
        let detail = self
            .blockers
            .iter()
            .map(describe_blocker)
            .collect::<Vec<_>>()
            .join("; ");
        Err(AppError::Validation(format!(
            "native_removal: plan is not executable: {detail}"
        )))
    }
}

fn describe_blocker(blocker: &NativeBlocker) -> String {
    match blocker {
        NativeBlocker::RetainedDependent {
            child_table,
            action,
            count,
            ..
        } => format!("{count} retained {child_table} row(s) under {action:?}"),
        NativeBlocker::TypedDependency {
            class,
            table,
            count,
            ..
        } => format!(
            "{count} {table} row(s) reference a {} owner",
            class.as_str()
        ),
        NativeBlocker::UnsupportedEffect { constraint, detail } => {
            format!("unevaluable effect {constraint}: {detail}")
        }
        NativeBlocker::RelationHolderInRemovalSet { table } => {
            format!("relationship holder {table} cannot be deleted by a relation removal")
        }
    }
}

/// A native table a canonical scope kind can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeSubject {
    Table(&'static str),
    /// `connection_target` is a device or a virtual machine.
    ConnectionTarget,
    /// The kind names any table registered in [`CONTENT_TYPES`].
    ContentType,
    /// The kind names an owner that is not a native inventory unit, so it can
    /// never reference this unit. It stays in the vocabulary so a row carrying it
    /// is not mistaken for an unknown form.
    NoNativeUnit,
}

impl ScopeSubject {
    fn names(self, table: &str) -> bool {
        match self {
            Self::Table(candidate) => candidate == table,
            Self::ConnectionTarget => is_connection_target_table(table),
            Self::ContentType => CONTENT_TYPES
                .iter()
                .any(|definition| definition.table == table),
            Self::NoNativeUnit => false,
        }
    }
}

fn is_connection_target_table(table: &str) -> bool {
    matches!(table, "dcim_device" | "virtualization_virtualmachine")
}

/// One owner table and the id/discriminator column pairs that can name this unit.
///
/// `kinds` is the owner's complete discriminator vocabulary. A stored value
/// outside it cannot be attributed, so it is reported as
/// [`NativeDependencyClass::UnattributedScope`] rather than treated as no effect.
struct TypedScopeOwner {
    table: &'static str,
    class: NativeDependencyClass,
    pairs: &'static [(
        &'static str,
        &'static str,
        &'static [(&'static str, ScopeSubject)],
    )],
}

/// `secret_usage_binding.consumer_type` values (`secrets` owner
/// `VALID_SECRET_FIELD_CONSUMER_TYPES` plus the usage-only projections).
const SECRET_CONSUMER_TYPES: &[(&str, ScopeSubject)] = &[
    (
        "connection_scope:connection_target",
        ScopeSubject::ConnectionTarget,
    ),
    ("inventory_custom_field", ScopeSubject::ContentType),
    (
        "inventory_fhrp_group",
        ScopeSubject::Table("ipam_fhrpgroup"),
    ),
    ("connection_config:record", ScopeSubject::NoNativeUnit),
    ("user_connection_credential_set", ScopeSubject::NoNativeUnit),
    (
        "playbook_service_principal:credential",
        ScopeSubject::NoNativeUnit,
    ),
    ("playbook_project", ScopeSubject::NoNativeUnit),
    ("job_template:runtime_env", ScopeSubject::NoNativeUnit),
    ("proxy_node:credential", ScopeSubject::NoNativeUnit),
    ("assistant_instance", ScopeSubject::NoNativeUnit),
    ("oidc_settings", ScopeSubject::NoNativeUnit),
    ("sync_provider", ScopeSubject::NoNativeUnit),
    ("user_auth_password", ScopeSubject::NoNativeUnit),
    ("monitoring_alert_source", ScopeSubject::NoNativeUnit),
    ("monitoring_query_source", ScopeSubject::NoNativeUnit),
];

/// `secret_usage_binding.owner_scope_type` values.
const SECRET_OWNER_SCOPES: &[(&str, ScopeSubject)] = &[
    ("connection_target", ScopeSubject::ConnectionTarget),
    ("inventory_custom_field", ScopeSubject::ContentType),
    (
        "inventory_fhrp_group",
        ScopeSubject::Table("ipam_fhrpgroup"),
    ),
    ("user", ScopeSubject::NoNativeUnit),
    ("user_connection_credential_set", ScopeSubject::NoNativeUnit),
    ("monitoring_alert_source", ScopeSubject::NoNativeUnit),
    ("monitoring_query_source", ScopeSubject::NoNativeUnit),
    ("playbook_project", ScopeSubject::NoNativeUnit),
    ("playbook_job_template", ScopeSubject::NoNativeUnit),
    ("proxy_node", ScopeSubject::NoNativeUnit),
    ("connection_config:record", ScopeSubject::NoNativeUnit),
];

/// `secrets_binding.scope_type` values its resolver understands
/// (`SecretRepository::build_scope_hierarchy`; unknown types are stored as-is,
/// which is why they remain unattributed here).
const SECRET_SCOPE_TYPES: &[(&str, ScopeSubject)] = &[
    ("connection_target", ScopeSubject::ConnectionTarget),
    ("device", ScopeSubject::Table("dcim_device")),
    ("vm", ScopeSubject::Table("virtualization_virtualmachine")),
    ("site", ScopeSubject::Table("dcim_site")),
    ("tenant", ScopeSubject::Table("tenancy_tenant")),
    ("tenant_group", ScopeSubject::Table("tenancy_tenantgroup")),
];

/// `config_context_binding.scope_type` values accepted by
/// `config_context_scope_exists` and the PIP-244 binding constraint.
const CONFIG_CONTEXT_SCOPES: &[(&str, ScopeSubject)] = &[
    ("tenant_group", ScopeSubject::Table("tenancy_tenantgroup")),
    ("tenant", ScopeSubject::Table("tenancy_tenant")),
    ("site", ScopeSubject::Table("dcim_site")),
    ("cluster", ScopeSubject::Table("virtualization_cluster")),
    ("role", ScopeSubject::Table("dcim_devicerole")),
    ("device", ScopeSubject::Table("dcim_device")),
    ("vm", ScopeSubject::Table("virtualization_virtualmachine")),
    ("connection_target", ScopeSubject::ConnectionTarget),
];

/// `connconf_assignment.scope_type` and `connections_proxy_assignment.scope_type`
/// values (`ConnConfScope`, and the proxy owner's identical list).
const CONNECTION_CONFIG_SCOPES: &[(&str, ScopeSubject)] = &[
    ("region", ScopeSubject::Table("dcim_region")),
    ("site_group", ScopeSubject::Table("dcim_sitegroup")),
    ("site", ScopeSubject::Table("dcim_site")),
    ("location", ScopeSubject::Table("dcim_location")),
    ("rack", ScopeSubject::Table("dcim_rack")),
    ("tenant", ScopeSubject::Table("tenancy_tenant")),
    ("cluster", ScopeSubject::Table("virtualization_cluster")),
    ("role", ScopeSubject::Table("dcim_devicerole")),
    ("device_type", ScopeSubject::Table("dcim_devicetype")),
    ("platform", ScopeSubject::Table("dcim_platform")),
    ("device", ScopeSubject::Table("dcim_device")),
    ("vm", ScopeSubject::Table("virtualization_virtualmachine")),
];

/// `playbook_assignment.scope_kind` values (`PlaybookAssignmentScopeKind`).
const PLAYBOOK_ASSIGNMENT_SCOPES: &[(&str, ScopeSubject)] = &[
    ("connection_target", ScopeSubject::ConnectionTarget),
    ("role", ScopeSubject::Table("dcim_devicerole")),
    ("site", ScopeSubject::Table("dcim_site")),
    ("region", ScopeSubject::Table("dcim_region")),
    ("tenant", ScopeSubject::Table("tenancy_tenant")),
    ("cluster", ScopeSubject::Table("virtualization_cluster")),
    ("platform", ScopeSubject::Table("dcim_platform")),
];

/// Monitoring scope vocabulary (`MONITORING_SCOPE_TYPES`); the discovery store
/// resolves all four host kinds against a device or virtual-machine id.
const MONITORING_SCOPES: &[(&str, ScopeSubject)] = &[
    ("site", ScopeSubject::Table("dcim_site")),
    ("tenant", ScopeSubject::Table("tenancy_tenant")),
    ("role", ScopeSubject::Table("dcim_devicerole")),
    ("connection_target", ScopeSubject::ConnectionTarget),
    ("device", ScopeSubject::Table("dcim_device")),
    ("virtual_machine", ScopeSubject::ConnectionTarget),
    ("vm", ScopeSubject::ConnectionTarget),
];

/// Non-FK reference owners with a typed discriminator.
const TYPED_SCOPE_OWNERS: &[TypedScopeOwner] = &[
    TypedScopeOwner {
        table: "secret_usage_binding",
        class: NativeDependencyClass::SecretConsumer,
        pairs: &[
            ("consumer_id", "consumer_type", SECRET_CONSUMER_TYPES),
            ("owner_scope_id", "owner_scope_type", SECRET_OWNER_SCOPES),
        ],
    },
    TypedScopeOwner {
        table: "secrets_binding",
        class: NativeDependencyClass::SecretConsumer,
        pairs: &[("scope_id", "scope_type", SECRET_SCOPE_TYPES)],
    },
    TypedScopeOwner {
        table: "config_context_binding",
        class: NativeDependencyClass::ConfigContextScope,
        pairs: &[("scope_id", "scope_type", CONFIG_CONTEXT_SCOPES)],
    },
    TypedScopeOwner {
        table: "connconf_assignment",
        class: NativeDependencyClass::ConnectionTarget,
        pairs: &[("scope_id", "scope_type", CONNECTION_CONFIG_SCOPES)],
    },
    TypedScopeOwner {
        table: "connections_proxy_assignment",
        class: NativeDependencyClass::ConnectionTarget,
        pairs: &[("scope_id", "scope_type", CONNECTION_CONFIG_SCOPES)],
    },
    TypedScopeOwner {
        table: "playbook_assignment",
        class: NativeDependencyClass::PlaybookReference,
        pairs: &[("scope_id", "scope_kind", PLAYBOOK_ASSIGNMENT_SCOPES)],
    },
    TypedScopeOwner {
        table: "playbook_target_set_member",
        class: NativeDependencyClass::PlaybookReference,
        pairs: &[
            (
                "target_id",
                "target_kind",
                &[("connection_target", ScopeSubject::ConnectionTarget)],
            ),
            (
                "source_id",
                "source_kind",
                &[
                    ("device", ScopeSubject::Table("dcim_device")),
                    (
                        "virtual_machine",
                        ScopeSubject::Table("virtualization_virtualmachine"),
                    ),
                ],
            ),
        ],
    },
    TypedScopeOwner {
        table: "monitoring_profile_assignment",
        class: NativeDependencyClass::MonitoringScope,
        pairs: &[("scope_id", "scope_type", MONITORING_SCOPES)],
    },
    TypedScopeOwner {
        table: "monitoring_alert_source_assignment",
        class: NativeDependencyClass::MonitoringScope,
        pairs: &[("scope_id", "scope_type", MONITORING_SCOPES)],
    },
    TypedScopeOwner {
        table: "monitoring_metric_set_assignment",
        class: NativeDependencyClass::MonitoringScope,
        pairs: &[("scope_id", "scope_type", MONITORING_SCOPES)],
    },
    TypedScopeOwner {
        table: "monitoring_query_source_assignment",
        class: NativeDependencyClass::MonitoringScope,
        pairs: &[("scope_id", "scope_type", MONITORING_SCOPES)],
    },
];

/// Native resources whose own `scope_id` is typed by `scope_type_id`, a content
/// type id (`ipam_prefix`, `ipam_vlangroup`, `virtualization_cluster`,
/// `wireless_wirelesslan`).
const NATIVE_SCOPE_TABLES: &[&str] = &[
    "ipam_prefix",
    "ipam_vlangroup",
    "virtualization_cluster",
    "wireless_wirelesslan",
];

/// Reference columns discovered on tables without a wired inspector. A match is
/// reported as an unattributed scope; the columns are also part of the catalogue
/// snapshot that fences inspection.
const DISCOVERED_REFERENCE_COLUMNS: &[&str] = &[
    "scope_id",
    "consumer_id",
    "owner_scope_id",
    "host_id",
    "target_id",
];

/// Discriminator columns that make a discovered `target_id` a row reference
/// rather than an identity column (`sync_inventory_transfer_target.target_id`
/// is the target instance identity, not a row).
const DISCOVERED_DISCRIMINATOR_COLUMNS: &[&str] = &["target_type", "target_kind", "source_kind"];

/// Inspect one approved set and decide every native effect.
///
/// Public dependency tables are fenced before inspection, and owning rows are
/// locked `FOR UPDATE` in deterministic order until the transaction ends.
pub async fn plan_native_removal(
    transaction: &mut Transaction<'_, Postgres>,
    approved: &[ApprovedNativeUnit],
    relations: &[NativeRelationRemoval],
) -> Result<NativeRemovalPlan, AppError> {
    let mut approved: Vec<ApprovedNativeUnit> = approved.to_vec();
    approved.sort();
    approved.dedup();
    for unit in &approved {
        if !is_native_unit_table(unit.table) {
            return Err(AppError::Validation(format!(
                "native_removal: {} is not a native inventory table",
                unit.table
            )));
        }
    }
    for relation in relations {
        if !is_native_unit_table(relation.holder_table) {
            return Err(AppError::Validation(format!(
                "native_removal: {} is not a native unit table",
                relation.holder_table
            )));
        }
    }

    let approved_keys: BTreeSet<(String, Uuid)> = approved
        .iter()
        .map(|unit| (unit.table.to_string(), unit.id))
        .collect();

    // Fence exactly what the inspector will read before reading it: catalog
    // reference columns, the tag/custom-field subject tables their owners report,
    // and the scope-fenced native owners. FK children join the set below.
    let observation = observe_reference_catalogue(transaction).await?;
    fence_dependency_tables(transaction, &observation.fence_tables).await?;

    // Lock and confirm the approved rows exist.
    let mut present: Vec<ApprovedNativeUnit> = Vec::with_capacity(approved.len());
    let mut already_absent: Vec<ApprovedNativeUnit> = Vec::new();
    let mut by_table: BTreeMap<&'static str, Vec<Uuid>> = BTreeMap::new();
    for unit in &approved {
        by_table.entry(unit.table).or_default().push(unit.id);
    }
    for (table, ids) in &by_table {
        let mut query = QueryBuilder::<Postgres>::new("SELECT id FROM ");
        query
            .push(*table)
            .push(" WHERE id = ANY(")
            .push_bind(ids.clone())
            .push(") ORDER BY id FOR UPDATE");
        let rows = query.build().fetch_all(&mut **transaction).await?;
        let found: BTreeSet<Uuid> = rows
            .iter()
            .map(|row| row.try_get("id"))
            .collect::<Result<_, _>>()?;
        for id in ids {
            if found.contains(id) {
                present.push(ApprovedNativeUnit { table, id: *id });
            } else {
                already_absent.push(ApprovedNativeUnit { table, id: *id });
            }
        }
    }

    let mut effects: Vec<PlannedEffect> = Vec::new();
    let mut dependencies: Vec<NativeDependency> = Vec::new();
    let mut blockers: Vec<NativeBlocker> = Vec::new();
    let mut fences: BTreeSet<String> = observation.fence_tables.iter().cloned().collect();
    let mut edges: Vec<(ApprovedNativeUnit, ApprovedNativeUnit, bool)> = Vec::new();
    let mut identities = IdentityCatalogue::default();

    let present_ids: BTreeMap<&'static str, Vec<Uuid>> = {
        let mut grouped: BTreeMap<&'static str, Vec<Uuid>> = BTreeMap::new();
        for unit in &present {
            grouped.entry(unit.table).or_default().push(unit.id);
        }
        grouped
    };

    // Discover every inbound FK effect first so its child table is fenced before
    // its rows are read: a child row inserted after inspection must be impossible,
    // not merely unlikely.
    let mut inbound_by_table: BTreeMap<&'static str, Vec<InboundDeleteEffect>> = BTreeMap::new();
    for table in present_ids.keys() {
        let inbound = inbound_delete_effects(&mut **transaction, table).await?;
        for effect in &inbound {
            fences.insert(effect.child_table.clone());
        }
        inbound_by_table.insert(table, inbound);
    }
    let fence_list: Vec<String> = fences.iter().cloned().collect();
    fence_dependency_tables(transaction, &fence_list).await?;
    ensure_catalogue_unchanged(transaction, &observation).await?;

    for (table, inbound) in inbound_by_table {
        let ids = &present_ids[table];
        for effect in &inbound {
            let Some(column) = effect.child_column.as_deref() else {
                blockers.push(NativeBlocker::UnsupportedEffect {
                    constraint: effect.constraint.clone(),
                    detail: "not a single-column reference to the parent id".to_string(),
                });
                continue;
            };
            // A child row is locked and reported by the identity its own table
            // declares. A composite-key table cannot be identified that way, so a
            // surviving row under it blocks instead of being guessed at.
            let child_identity = identities
                .identity(&mut **transaction, &effect.child_table)
                .await?
                .filter(|identity| identity.is_uuid);
            let Some(child_identity) = child_identity else {
                let dependents =
                    count_referencing_children(&mut **transaction, effect, column, ids).await?;
                if dependents > 0 {
                    blockers.push(NativeBlocker::UnsupportedEffect {
                        constraint: effect.constraint.clone(),
                        detail: format!(
                            "{} keeps {dependents} referencing row(s) and declares no \
                             single-column uuid identity to lock or report",
                            effect.child_table
                        ),
                    });
                }
                continue;
            };
            let children = referencing_children(
                &mut **transaction,
                effect,
                column,
                &child_identity.column,
                ids,
            )
            .await?;
            if children.is_empty() {
                continue;
            }
            let mut approved_children = 0_i64;
            let mut retained: Vec<Uuid> = Vec::new();
            for (parent_id, child_id) in &children {
                let child_key = (effect.child_table.clone(), *child_id);
                if approved_keys.contains(&child_key) {
                    approved_children += 1;
                    let parent = present
                        .iter()
                        .find(|unit| unit.table == table && unit.id == *parent_id)
                        .copied()
                        .ok_or_else(|| {
                            AppError::Database(
                                "native_removal: inspected parent is not in the approved set"
                                    .to_string(),
                            )
                        })?;
                    let child = present
                        .iter()
                        .find(|unit| {
                            unit.table == effect.child_table.as_str() && unit.id == *child_id
                        })
                        .copied()
                        .ok_or_else(|| {
                            AppError::Database(
                                "native_removal: inspected child is not a native inventory unit"
                                    .to_string(),
                            )
                        })?;
                    edges.push((child, parent, effect.deferrable));
                } else {
                    retained.push(*child_id);
                }
            }
            // A cascade onto a membership-only table is the parent's own
            // representation (its scope rows), not a separately owned unit: the
            // relation is stored there and the parent carries its identity. Every
            // other surviving child stays retained.
            let membership_representation = effect.action == DeleteAction::Cascade
                && membership_only_table(&mut **transaction, &effect.child_table).await?;
            if membership_representation {
                retained.clear();
                if !children.is_empty() {
                    effects.push(PlannedEffect::RemovedWithApprovedUnit {
                        child_table: effect.child_table.clone(),
                        count: i64::try_from(children.len()).unwrap_or(i64::MAX),
                    });
                }
            }
            if approved_children > 0 {
                effects.push(PlannedEffect::RemovedWithApprovedUnit {
                    child_table: effect.child_table.clone(),
                    count: approved_children,
                });
            }
            if !retained.is_empty() {
                let count = i64::try_from(retained.len())
                    .map_err(|_| AppError::Database("native_removal: count overflow".into()))?;
                let blocker = NativeBlocker::RetainedDependent {
                    child_table: effect.child_table.clone(),
                    constraint: effect.constraint.clone(),
                    action: effect.action,
                    count,
                    sample: retained.iter().take(3).copied().collect(),
                };
                blockers.push(blocker.clone());
                effects.push(PlannedEffect::Retained(blocker));
            }
        }
        for id in ids {
            for dependency in
                typed_dependencies(&mut **transaction, &mut identities, table, *id).await?
            {
                if dependency.effect == NativeDependencyEffect::BlocksRemoval {
                    blockers.push(NativeBlocker::TypedDependency {
                        class: dependency.class,
                        table: dependency.table.clone(),
                        count: dependency.count,
                        sample: dependency.sample.clone(),
                    });
                }
                dependencies.push(dependency);
            }
            if table == CUSTOM_FIELD_DEFINITION_TABLE {
                let subjects = native_field_subjects(transaction).await?;
                for dependency in custom_field_value_dependencies(
                    &mut **transaction,
                    &mut identities,
                    &subjects,
                    *id,
                )
                .await?
                {
                    if dependency.effect == NativeDependencyEffect::BlocksRemoval {
                        blockers.push(NativeBlocker::TypedDependency {
                            class: dependency.class,
                            table: dependency.table.clone(),
                            count: dependency.count,
                            sample: dependency.sample.clone(),
                        });
                    }
                    dependencies.push(dependency);
                }
            }
        }
    }

    let (order, deferred_cycle) = deletion_order(&present, &edges)?;
    for relation in relations {
        if approved_keys.contains(&(relation.holder_table.to_string(), relation.holder_id)) {
            blockers.push(NativeBlocker::RelationHolderInRemovalSet {
                table: relation.holder_table.to_string(),
            });
        }
        let mut query = QueryBuilder::<Postgres>::new("SELECT id FROM ");
        query
            .push(relation.holder_table)
            .push(" WHERE id = ")
            .push_bind(relation.holder_id)
            .push(" FOR UPDATE");
        if query
            .build()
            .fetch_optional(&mut **transaction)
            .await?
            .is_none()
        {
            return Err(AppError::Validation(
                "native_removal: a relationship holder does not exist".to_string(),
            ));
        }
    }

    Ok(NativeRemovalPlan {
        approved: present,
        order,
        effects,
        dependencies,
        relations: relations.to_vec(),
        blockers,
        fences: fences.into_iter().collect(),
        already_absent,
        deferred_cycle,
    })
}

/// Read the surviving or approved children that reference the approved parents.
async fn referencing_children(
    connection: &mut PgConnection,
    effect: &InboundDeleteEffect,
    column: &str,
    identity: &str,
    parent_ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid)>, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query
        .push(qualified(Some("child"), column))
        .push(" AS parent_id, ")
        .push(qualified(Some("child"), identity))
        .push(" AS id FROM ")
        .push(quoted(effect.child_table.as_str()))
        .push(" child WHERE ")
        .push(qualified(Some("child"), column))
        .push(" = ANY(")
        .push_bind(parent_ids.to_vec())
        .push(") ORDER BY ")
        .push(qualified(Some("child"), column))
        .push(", ")
        .push(qualified(Some("child"), identity))
        .push(" FOR UPDATE OF child");
    let rows = query.build().fetch_all(&mut *connection).await?;
    let mut children = Vec::with_capacity(rows.len());
    for row in rows {
        children.push((row.try_get("parent_id")?, row.try_get("id")?));
    }
    Ok(children)
}

/// Count the children of the approved parents when their table declares no
/// single-column identity to lock or report them by.
async fn count_referencing_children(
    connection: &mut PgConnection,
    effect: &InboundDeleteEffect,
    column: &str,
    parent_ids: &[Uuid],
) -> Result<i64, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT count(*) FROM ");
    query
        .push(quoted(effect.child_table.as_str()))
        .push(" child WHERE ")
        .push(qualified(Some("child"), column))
        .push(" = ANY(")
        .push_bind(parent_ids.to_vec())
        .push(")");
    Ok(query
        .build()
        .fetch_one(&mut *connection)
        .await?
        .try_get("count")?)
}

/// Typed inverse references of one native resource.
///
/// Each class is read through the owner that defines it: a discriminator column
/// decides whether a row names this unit, and only the owner's own vocabulary is
/// accepted. A value outside the vocabulary is an unattributed scope, never an
/// assumed no-effect.
pub async fn typed_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &str,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let mut found = Vec::new();
    if let Some(definition) = CONTENT_TYPES
        .iter()
        .find(|definition| definition.table == table)
    {
        found.extend(content_type_dependencies(connection, identities, definition, id).await?);
    }
    if table == "ipam_fhrpgroup" {
        found.extend(fhrp_group_dependencies(connection, identities, id).await?);
    }
    if table == "inventory_tag" {
        found.extend(tag_holders(connection, identities, id).await?);
    }
    found.extend(scoped_dependencies(connection, identities, table, id).await?);
    found.extend(native_scope_dependencies(connection, identities, table, id).await?);
    found.extend(monitoring_host_override_dependencies(connection, identities, table, id).await?);
    found.extend(selector_dependencies(connection, identities, table, id).await?);
    found.extend(history_dependencies(connection, identities, table, id).await?);
    found.extend(discovered_scope_dependencies(connection, identities, table, id).await?);
    Ok(found)
}

/// Typed generic relations registered by the content-type owner: contact
/// assignments and component templates.
async fn content_type_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    definition: &ContentTypeDef,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let mut found = Vec::new();

    // Contact assignments are a typed generic relation, not an FK.
    let identity = identities
        .identity(&mut *connection, "tenancy_contactassignment")
        .await?;
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query
        .push(sample_projection(Some("a"), identity.as_ref()))
        .push(
            ", COUNT(*) OVER() AS total FROM tenancy_contactassignment a \
         JOIN inventory_content_type ct ON ct.id = a.object_type_id \
         WHERE ct.app_label = ",
        );
    query
        .push_bind(definition.app_label)
        .push(" AND ct.model = ")
        .push_bind(definition.model)
        .push(" AND a.object_id = ")
        .push_bind(id)
        .push(sample_order(Some("a"), identity.as_ref()))
        .push(" LIMIT 4");
    if let Some(dependency) = collect_dependency(
        connection,
        NativeDependencyClass::ContactAssignment,
        "tenancy_contactassignment".to_string(),
        NativeDependencyEffect::BlocksRemoval,
        query,
    )
    .await?
    {
        found.push(dependency);
    }

    // Component templates reference their target through a typed pair. The
    // pair is only a reference for the registered component object types.
    let token = format!("{}.{}", definition.app_label, definition.model);
    if INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES.contains(&token.as_str()) {
        let identity = identities
            .identity(&mut *connection, "dcim_inventoryitemtemplate")
            .await?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(sample_projection(Some("t"), identity.as_ref()))
            .push(
                ", COUNT(*) OVER() AS total FROM dcim_inventoryitemtemplate t \
                 JOIN inventory_content_type ct ON ct.id = t.component_type_id \
                 WHERE ct.app_label = ",
            );
        query
            .push_bind(definition.app_label)
            .push(" AND ct.model = ")
            .push_bind(definition.model)
            .push(" AND t.component_id = ")
            .push_bind(id)
            .push(sample_order(Some("t"), identity.as_ref()))
            .push(" LIMIT 4");
        if let Some(dependency) = collect_dependency(
            connection,
            NativeDependencyClass::ComponentTemplate,
            "dcim_inventoryitemtemplate".to_string(),
            NativeDependencyEffect::BlocksRemoval,
            query,
        )
        .await?
        {
            found.push(dependency);
        }
    }
    Ok(found)
}

/// Interfaces and services that name an FHRP group through a typed generic pair.
async fn fhrp_group_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let mut found = Vec::new();
    for (child_table, type_column, object_column) in [
        (
            "ipam_ipaddress",
            "assigned_object_type_id",
            "assigned_object_id",
        ),
        ("ipam_service", "parent_object_type_id", "parent_object_id"),
    ] {
        let identity = identities.identity(&mut *connection, child_table).await?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(sample_projection(Some("child"), identity.as_ref()))
            .push(", COUNT(*) OVER() AS total FROM ")
            .push(child_table)
            .push(" child WHERE ")
            .push(qualified(Some("child"), object_column))
            .push(" = ")
            .push_bind(id)
            .push(" AND ")
            .push(qualified(Some("child"), type_column))
            .push(
                " = (SELECT id FROM inventory_content_type \
                  WHERE app_label = 'ipam' AND model = 'fhrpgroup')",
            )
            .push(sample_order(Some("child"), identity.as_ref()))
            .push(" LIMIT 4");
        if let Some(dependency) = collect_dependency(
            connection,
            NativeDependencyClass::FhrpAssignment,
            child_table.to_string(),
            NativeDependencyEffect::BlocksRemoval,
            query,
        )
        .await?
        {
            found.push(dependency);
        }
    }
    Ok(found)
}

/// Rows of one owner table that name this unit through a typed discriminator.
async fn scoped_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &str,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let mut found = Vec::new();
    for owner in TYPED_SCOPE_OWNERS {
        let matching: Vec<(&str, &str, Vec<String>)> = owner
            .pairs
            .iter()
            .filter_map(|(id_column, kind_column, kinds)| {
                let names: Vec<String> = kinds
                    .iter()
                    .filter(|(_, subject)| subject.names(table))
                    .map(|(kind, _)| (*kind).to_string())
                    .collect();
                (!names.is_empty()).then_some((*id_column, *kind_column, names))
            })
            .collect();
        if !matching.is_empty() {
            let identity = identities.identity(&mut *connection, owner.table).await?;
            let mut query = QueryBuilder::<Postgres>::new("SELECT ");
            query
                .push(sample_projection(None, identity.as_ref()))
                .push(", COUNT(*) OVER() AS total FROM public.");
            push_identifier(&mut query, owner.table);
            query.push(" WHERE ");
            for (index, (id_column, kind_column, kinds)) in matching.iter().enumerate() {
                if index > 0 {
                    query.push(" OR ");
                }
                query.push("(");
                push_identifier(&mut query, id_column);
                query.push(" = ");
                query.push_bind(id);
                query.push(" AND ");
                push_identifier(&mut query, kind_column);
                query.push(" = ANY(");
                query.push_bind(kinds.clone());
                query.push("))");
            }
            query
                .push(sample_order(None, identity.as_ref()))
                .push(" LIMIT 4");
            if let Some(dependency) = collect_dependency(
                connection,
                owner.class,
                owner.table.to_string(),
                NativeDependencyEffect::BlocksRemoval,
                query,
            )
            .await?
            {
                found.push(dependency);
            }
        }

        // A stored discriminator outside the owner's vocabulary cannot be
        // attributed to any table, so it fails closed instead of disappearing.
        for (id_column, kind_column, kinds) in owner.pairs {
            let vocabulary: Vec<String> =
                kinds.iter().map(|(kind, _)| (*kind).to_string()).collect();
            let mut query = QueryBuilder::<Postgres>::new("SELECT DISTINCT ");
            push_identifier(&mut query, kind_column);
            query.push(" AS kind FROM public.");
            push_identifier(&mut query, owner.table);
            query.push(" WHERE ");
            push_identifier(&mut query, id_column);
            query.push(" = ");
            query.push_bind(id);
            query.push(" AND NOT (");
            push_identifier(&mut query, kind_column);
            query.push(" = ANY(");
            query.push_bind(vocabulary);
            query.push(")) ORDER BY kind LIMIT 4");
            let rows = query.build().fetch_all(&mut *connection).await?;
            for row in rows {
                let kind: String = row.try_get("kind")?;
                found.push(unattributed_scope(format!(
                    "{}.{id_column}={kind}",
                    owner.table
                )));
            }
        }
    }
    Ok(found)
}

/// Native rows whose own `scope_id` is typed by `scope_type_id` (a content type).
async fn native_scope_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &str,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let Some(definition) = CONTENT_TYPES
        .iter()
        .find(|definition| definition.table == table)
    else {
        return Ok(Vec::new());
    };
    let content_type: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2",
    )
    .bind(definition.app_label)
    .bind(definition.model)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(content_type) = content_type else {
        // The registry is seeded by the schema; a missing row means the catalogue
        // itself is inconsistent, so no scope row can be attributed.
        return Err(AppError::Database(format!(
            "native_removal: content type {} is not registered",
            definition.model
        )));
    };

    let mut found = Vec::new();
    for scope_table in NATIVE_SCOPE_TABLES {
        let identity = identities.identity(&mut *connection, scope_table).await?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(sample_projection(None, identity.as_ref()))
            .push(", COUNT(*) OVER() AS total FROM public.");
        push_identifier(&mut query, scope_table);
        query.push(" WHERE scope_id = ");
        query.push_bind(id);
        query.push(" AND scope_type_id = ");
        query.push_bind(content_type);
        query
            .push(sample_order(None, identity.as_ref()))
            .push(" LIMIT 4");
        if let Some(dependency) = collect_dependency(
            connection,
            NativeDependencyClass::NativeScopeReference,
            (*scope_table).to_string(),
            NativeDependencyEffect::BlocksRemoval,
            query,
        )
        .await?
        {
            found.push(dependency);
        }

        // An untyped or dangling scope type is not attributable.
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(sample_projection(None, identity.as_ref()))
            .push(", COUNT(*) OVER() AS total FROM public.");
        push_identifier(&mut query, scope_table);
        query.push(" WHERE scope_id = ");
        query.push_bind(id);
        query.push(
            " AND (scope_type_id IS NULL OR NOT EXISTS \
             (SELECT 1 FROM inventory_content_type WHERE id = scope_type_id))",
        );
        query
            .push(sample_order(None, identity.as_ref()))
            .push(" LIMIT 4");
        if let Some(dependency) = collect_dependency(
            connection,
            NativeDependencyClass::UnattributedScope,
            format!("{scope_table}.scope_type_id"),
            NativeDependencyEffect::BlocksRemoval,
            query,
        )
        .await?
        {
            found.push(dependency);
        }
    }
    Ok(found)
}

/// Per-host metric overrides keyed directly by the resolved host id.
async fn monitoring_host_override_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &str,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    if !is_connection_target_table(table) {
        return Ok(Vec::new());
    }
    let identity = identities
        .identity(&mut *connection, "monitoring_metric_query_host_override")
        .await?;
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query.push(sample_projection(None, identity.as_ref())).push(
        ", COUNT(*) OVER() AS total FROM monitoring_metric_query_host_override \
             WHERE host_id = ",
    );
    query
        .push_bind(id)
        .push(sample_order(None, identity.as_ref()))
        .push(" LIMIT 4");
    Ok(collect_dependency(
        connection,
        NativeDependencyClass::MonitoringScope,
        "monitoring_metric_query_host_override".to_string(),
        NativeDependencyEffect::BlocksRemoval,
        query,
    )
    .await?
    .into_iter()
    .collect())
}

/// Dynamic playbook target-set selectors that name this unit by catalog id.
const PLAYBOOK_SELECTOR_KEYS: &[(&str, &str)] = &[
    ("site_ids", "dcim_site"),
    ("region_ids", "dcim_region"),
    ("tenant_ids", "tenancy_tenant"),
    ("role_ids", "dcim_devicerole"),
    ("platform_ids", "dcim_platform"),
    ("cluster_ids", "virtualization_cluster"),
    ("device_type_ids", "dcim_devicetype"),
    ("tag_ids", "inventory_tag"),
];

/// A dynamic selector is a catalog query, so a stored catalog id in it is a
/// reference to that row even though no FK expresses it.
async fn selector_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &str,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let keys: Vec<&str> = PLAYBOOK_SELECTOR_KEYS
        .iter()
        .filter(|(_, target)| *target == table)
        .map(|(key, _)| *key)
        .collect();
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let identity = identities
        .identity(&mut *connection, "playbook_target_set")
        .await?;
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query
        .push(sample_projection(Some("t"), identity.as_ref()))
        .push(", COUNT(*) OVER() AS total FROM playbook_target_set t WHERE ");
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        query.push("EXISTS (SELECT 1 FROM jsonb_array_elements_text(COALESCE(t.selector->'");
        query.push(*key);
        query.push(
            "', '[]'::jsonb)) item \
             WHERE item ~ '^[0-9a-fA-F-]{32,36}$' AND item::uuid = ",
        );
        query.push_bind(id);
        query.push(")");
    }
    query
        .push(sample_order(Some("t"), identity.as_ref()))
        .push(" LIMIT 4");
    Ok(collect_dependency(
        connection,
        NativeDependencyClass::PlaybookReference,
        "playbook_target_set".to_string(),
        NativeDependencyEffect::BlocksRemoval,
        query,
    )
    .await?
    .into_iter()
    .collect())
}

/// Session and run records keyed by a target id.
///
/// An open row is a live reference and blocks. A terminal row is history the
/// owner already reads without the target: the connections owner states that
/// terminal session states are "history, not load", a playbook run keeps its own
/// `target_name`/`target_address`, and audit labels fall back to the raw
/// `target_id` when the row is gone. Keeping those rows is the normal state, so
/// they are reported with [`NativeDependencyEffect::RetainedHistory`].
async fn history_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &str,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    // `history_dependency` binds the id and inserts only static owner predicates;
    // no caller-provided SQL reaches the statement.
    let mut found = Vec::new();
    if is_connection_target_table(table) {
        for (table_name, predicate, effect) in [
            (
                "connections_session",
                "target_id = $1 AND ended_at IS NULL AND status::text IN ('pending', 'active')",
                NativeDependencyEffect::BlocksRemoval,
            ),
            (
                "connections_session",
                "target_id = $1 AND NOT (ended_at IS NULL AND status::text IN ('pending', 'active'))",
                NativeDependencyEffect::RetainedHistory,
            ),
            (
                "playbook_run",
                "(target_id = $1 OR source_id = $1) AND status IN ('queued', 'leased', 'dispatching', 'running')",
                NativeDependencyEffect::BlocksRemoval,
            ),
            (
                "playbook_run",
                "(target_id = $1 OR source_id = $1) AND status NOT IN ('queued', 'leased', 'dispatching', 'running')",
                NativeDependencyEffect::RetainedHistory,
            ),
            // A job token is the run's historical capability record; the run row
            // carries the live state.
            (
                "playbook_job_token",
                "(target_id = $1 OR source_id = $1)",
                NativeDependencyEffect::RetainedHistory,
            ),
        ] {
            if let Some(dependency) =
                history_dependency(connection, identities, table_name, id, predicate, effect)
                    .await?
            {
                found.push(dependency);
            }
        }
    }

    let mut audit_kinds: Vec<String> = Vec::new();
    if let Some(definition) = CONTENT_TYPES
        .iter()
        .find(|definition| definition.table == table)
    {
        audit_kinds.push(format!(
            "inventory:{}.{}",
            definition.app_label, definition.model
        ));
    }
    if is_connection_target_table(table) {
        audit_kinds.extend(
            ["device", "virtual_machine", "connection_target"]
                .iter()
                .map(|kind| (*kind).to_string()),
        );
    }
    if !audit_kinds.is_empty() {
        let identity = identities.identity(&mut *connection, "audit_event").await?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query.push(sample_projection(None, identity.as_ref())).push(
            ", COUNT(*) OVER() AS total FROM audit_event \
                 WHERE target_id = ",
        );
        query.push_bind(id).push(" AND target_type = ANY(");
        query.push_bind(audit_kinds);
        query
            .push(")")
            .push(sample_order(None, identity.as_ref()))
            .push(" LIMIT 4");
        if let Some(dependency) = collect_dependency(
            connection,
            NativeDependencyClass::SessionOrHistory,
            "audit_event".to_string(),
            NativeDependencyEffect::RetainedHistory,
            query,
        )
        .await?
        {
            found.push(dependency);
        }
    }
    Ok(found)
}

async fn history_dependency(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &'static str,
    id: Uuid,
    predicate: &'static str,
    effect: NativeDependencyEffect,
) -> Result<Option<NativeDependency>, AppError> {
    // The table is a static owner name and the predicate is static SQL; the id
    // stays bound.
    let identity = identities.identity(&mut *connection, table).await?;
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query
        .push(sample_projection(None, identity.as_ref()))
        .push(", COUNT(*) OVER() AS total FROM public.");
    push_identifier(&mut query, table);
    query
        .push(" WHERE ")
        .push(predicate)
        .push(sample_order(None, identity.as_ref()))
        .push(" LIMIT 4");
    let rows = query.build().bind(id).fetch_all(&mut *connection).await?;
    dependency_from_rows(
        NativeDependencyClass::SessionOrHistory,
        table.to_string(),
        effect,
        rows,
    )
}

/// Count and sample one dependency query that selects the owner's identity as
/// `id` (or `NULL` when its key is not a reportable uuid) and
/// `COUNT(*) OVER() AS total`.
async fn collect_dependency(
    connection: &mut PgConnection,
    class: NativeDependencyClass,
    table: String,
    effect: NativeDependencyEffect,
    mut query: QueryBuilder<Postgres>,
) -> Result<Option<NativeDependency>, AppError> {
    let rows = query.build().fetch_all(&mut *connection).await?;
    dependency_from_rows(class, table, effect, rows)
}

fn dependency_from_rows(
    class: NativeDependencyClass,
    table: String,
    effect: NativeDependencyEffect,
    rows: Vec<sqlx::postgres::PgRow>,
) -> Result<Option<NativeDependency>, AppError> {
    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let count: i64 = first.try_get("total")?;
    let mut sample = Vec::with_capacity(rows.len());
    for row in &rows {
        if let Some(id) = row.try_get::<Option<Uuid>, _>("id")? {
            sample.push(id);
        }
    }
    Ok(Some(NativeDependency {
        class,
        table,
        count,
        sample,
        effect,
    }))
}

fn unattributed_scope(table: String) -> NativeDependency {
    NativeDependency {
        class: NativeDependencyClass::UnattributedScope,
        table,
        count: 1,
        sample: Vec::new(),
        effect: NativeDependencyEffect::BlocksRemoval,
    }
}

fn push_identifier(query: &mut QueryBuilder<Postgres>, identifier: &str) {
    query.push(quoted(identifier));
}

fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// The identity PostgreSQL declares for one table's rows.
///
/// A native owner table is not guaranteed an `id` column: a junction, binding or
/// association table can be keyed by a composite primary key. The identity used
/// here is read from the catalog, never assumed, so locking and sampling address
/// a column the table actually has.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RowIdentity {
    column: String,
    /// Whether the identity is a `uuid`, so a row can be reported as a sample.
    is_uuid: bool,
}

/// Catalog reader for [`RowIdentity`], memoised for one inspection.
#[derive(Debug, Default)]
struct IdentityCatalogue {
    identities: BTreeMap<String, Option<RowIdentity>>,
}

impl IdentityCatalogue {
    async fn identity(
        &mut self,
        connection: &mut PgConnection,
        table: &str,
    ) -> Result<Option<RowIdentity>, AppError> {
        if let Some(known) = self.identities.get(table) {
            return Ok(known.clone());
        }
        let identity = read_row_identity(connection, table).await?;
        self.identities.insert(table.to_string(), identity.clone());
        Ok(identity)
    }
}

/// Read `table`'s identity column from PostgreSQL's own indexes: the
/// single-column primary key, otherwise the single-column unique non-null key
/// the table still declares. A composite key yields `None`.
async fn read_row_identity(
    connection: &mut PgConnection,
    table: &str,
) -> Result<Option<RowIdentity>, AppError> {
    let row = sqlx::query(
        "SELECT attribute.attname AS column_name, \
                attribute.atttypid = 'uuid'::regtype AS is_uuid \
         FROM pg_index definition \
         JOIN pg_attribute attribute ON attribute.attrelid = definition.indrelid \
                                   AND attribute.attnum = definition.indkey[0] \
         WHERE definition.indrelid = to_regclass($1) \
           AND definition.indisunique AND definition.indnkeyatts = 1 \
           AND definition.indpred IS NULL \
           AND attribute.attnum > 0 AND NOT attribute.attisdropped \
           AND (definition.indisprimary OR attribute.attnotnull) \
         ORDER BY definition.indisprimary DESC, attribute.attnum \
         LIMIT 1",
    )
    .bind(table)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(RowIdentity {
        column: row.try_get("column_name")?,
        is_uuid: row.try_get("is_uuid")?,
    }))
}

/// The sample projection of one owner table.
///
/// A `uuid` identity supplies the sample; any other key shape still contributes
/// its exact count, with no invented sample.
fn sample_projection(alias: Option<&str>, identity: Option<&RowIdentity>) -> String {
    match identity {
        Some(identity) if identity.is_uuid => {
            format!("{} AS id", qualified(alias, &identity.column))
        }
        _ => "NULL::uuid AS id".to_string(),
    }
}

/// The matching `ORDER BY` for a sampled owner table, or an empty fragment.
fn sample_order(alias: Option<&str>, identity: Option<&RowIdentity>) -> String {
    identity.as_ref().map_or_else(String::new, |identity| {
        format!(" ORDER BY {}", qualified(alias, &identity.column))
    })
}

fn qualified(alias: Option<&str>, identifier: &str) -> String {
    match alias {
        Some(alias) => format!("{alias}.{}", quoted(identifier)),
        None => quoted(identifier),
    }
}

/// Whether `table` is attributed by a typed inspector above.
fn is_typed_owner_table(table: &str) -> bool {
    TYPED_SCOPE_OWNERS.iter().any(|owner| owner.table == table)
        || NATIVE_SCOPE_TABLES.contains(&table)
        || matches!(
            table,
            "playbook_target_set"
                | "monitoring_metric_query_host_override"
                | "connections_session"
                | "playbook_run"
                | "playbook_job_token"
                | "audit_event"
        )
}

/// Reference columns discovered from the catalog, per public table.
async fn reference_columns(
    connection: &mut PgConnection,
) -> Result<Vec<(String, String)>, AppError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT relation.relname AS table_name, attribute.attname AS column_name \
         FROM pg_class relation \
         JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace \
         JOIN pg_attribute attribute ON attribute.attrelid = relation.oid \
         WHERE namespace.nspname = 'public' AND relation.relkind IN ('r', 'p') \
           AND attribute.attnum > 0 AND NOT attribute.attisdropped \
           AND attribute.attname::text = ANY(",
    );
    query.push_bind(
        DISCOVERED_REFERENCE_COLUMNS
            .iter()
            .map(|column| (*column).to_string())
            .collect::<Vec<String>>(),
    );
    query.push(") ORDER BY 1, 2");
    let rows = query.build().fetch_all(&mut *connection).await?;
    let mut columns = Vec::with_capacity(rows.len());
    for row in rows {
        columns.push((row.try_get("table_name")?, row.try_get("column_name")?));
    }
    Ok(columns)
}

/// The catalogue facts inspection depends on.
struct ReferenceCatalogue {
    /// Tables to fence before reading them.
    fence_tables: Vec<String>,
    /// Sorted `table`/`table.column` facts; any change invalidates inspection.
    identity: Vec<String>,
}

async fn observe_reference_catalogue(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<ReferenceCatalogue, AppError> {
    let reference = reference_columns(&mut **transaction).await?;
    let tag_subjects: Vec<String> =
        sqlx::query_scalar("SELECT table_name FROM inventory_tag_subjects()")
            .fetch_all(&mut **transaction)
            .await?;
    let field_subjects = native_field_subjects(transaction).await?;

    let mut fence: BTreeSet<String> = reference.iter().map(|(table, _)| table.clone()).collect();
    fence.extend(tag_subjects);
    fence.extend(
        field_subjects
            .iter()
            .map(|subject| subject.table_name.clone()),
    );
    fence.extend(NATIVE_SCOPE_TABLES.iter().map(|table| (*table).to_string()));
    // A dynamic target set keeps its references in `selector` JSON, not in a
    // reference column.
    fence.insert("playbook_target_set".to_string());

    let mut identity: Vec<String> = fence.iter().cloned().collect();
    identity.extend(
        reference
            .iter()
            .map(|(table, column)| format!("{table}.{column}")),
    );
    identity.sort();
    Ok(ReferenceCatalogue {
        fence_tables: fence.into_iter().collect(),
        identity,
    })
}

/// Fail closed when the catalogue changed between observation and row reads.
async fn ensure_catalogue_unchanged(
    transaction: &mut Transaction<'_, Postgres>,
    observed: &ReferenceCatalogue,
) -> Result<(), AppError> {
    let current = observe_reference_catalogue(transaction).await?;
    if current.identity == observed.identity {
        return Ok(());
    }
    Err(AppError::Conflict(
        "native_removal:catalogue_changed".to_string(),
    ))
}

/// Fail-closed fallback for a reference column on a table without a wired
/// inspector.
///
/// The owners above attribute their rows by discriminator. Everything else that
/// stores one of the canonical reference columns is reported as an unattributed
/// scope when its value equals this unit id, so a new reference form cannot
/// silently look like no effect.
async fn discovered_scope_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    table: &str,
    id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    if !is_native_unit_table(table) {
        return Ok(Vec::new());
    }
    let reference = reference_columns(connection).await?;
    let mut found = Vec::new();
    for (owner_table, column) in reference {
        if is_typed_owner_table(&owner_table) {
            continue;
        }
        if column == "target_id" && !has_reference_discriminator(connection, &owner_table).await? {
            // An undiscriminated `target_id` can be an instance identity
            // (`sync_inventory_transfer_target`), not a row reference.
            continue;
        }
        let identity = identities.identity(&mut *connection, &owner_table).await?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(sample_projection(None, identity.as_ref()))
            .push(", COUNT(*) OVER() AS total FROM public.");
        push_identifier(&mut query, &owner_table);
        query.push(" WHERE ");
        push_identifier(&mut query, &column);
        query.push(" = ");
        query.push_bind(id);
        query
            .push(sample_order(None, identity.as_ref()))
            .push(" LIMIT 4");
        if let Some(dependency) = collect_dependency(
            connection,
            NativeDependencyClass::UnattributedScope,
            format!("{owner_table}.{column}"),
            NativeDependencyEffect::BlocksRemoval,
            query,
        )
        .await?
        {
            found.push(dependency);
        }
    }
    Ok(found)
}

/// Whether `table` also carries a discriminator for its discovered `target_id`.
async fn has_reference_discriminator(
    connection: &mut PgConnection,
    table: &str,
) -> Result<bool, AppError> {
    let discriminators: Vec<String> = DISCOVERED_DISCRIMINATOR_COLUMNS
        .iter()
        .map(|column| (*column).to_string())
        .collect();
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_class relation \
         JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace \
         JOIN pg_attribute attribute ON attribute.attrelid = relation.oid \
         WHERE namespace.nspname = 'public' AND relation.relname = $1 \
           AND attribute.attnum > 0 AND NOT attribute.attisdropped \
           AND attribute.attname::text = ANY($2))",
    )
    .bind(table)
    .bind(discriminators)
    .fetch_one(&mut *connection)
    .await?;
    Ok(exists)
}

async fn tag_holders(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    tag_id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let mut found = Vec::new();
    let subjects: Vec<String> =
        sqlx::query_scalar("SELECT table_name FROM inventory_tag_subjects()")
            .fetch_all(&mut *connection)
            .await?;
    let mut total = 0_i64;
    let mut samples: Vec<Uuid> = Vec::new();
    for subject in subjects {
        let identity = identities.identity(&mut *connection, &subject).await?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(sample_projection(Some("owner"), identity.as_ref()))
            .push(", COUNT(*) OVER() AS total FROM public.");
        push_identifier(&mut query, &subject);
        query
            .push(" owner WHERE owner.tags @> ARRAY[")
            .push_bind(tag_id.to_string())
            .push("]::text[]")
            .push(sample_order(Some("owner"), identity.as_ref()))
            .push(" LIMIT 4");
        let rows = query.build().fetch_all(&mut *connection).await?;
        for row in rows {
            total += row.try_get::<i64, _>("total")?;
            if samples.len() < 4 {
                if let Some(sample) = row.try_get::<Option<Uuid>, _>("id")? {
                    samples.push(sample);
                }
            }
        }
    }
    if total > 0 {
        found.push(NativeDependency {
            class: NativeDependencyClass::TagHolder,
            table: "inventory_tag_subject".to_string(),
            count: total,
            sample: samples,
            effect: NativeDependencyEffect::BlocksRemoval,
        });
    }
    Ok(found)
}

/// Order the approved units children-first.
///
/// A cycle is absorbable only when every edge in it is deferrable; otherwise the
/// plan fails closed instead of disabling a constraint.
fn deletion_order(
    present: &[ApprovedNativeUnit],
    edges: &[(ApprovedNativeUnit, ApprovedNativeUnit, bool)],
) -> Result<(Vec<PlannedDeletion>, bool), AppError> {
    let mut children_of: BTreeMap<ApprovedNativeUnit, BTreeSet<ApprovedNativeUnit>> =
        BTreeMap::new();
    for unit in present {
        children_of.entry(*unit).or_default();
    }
    for (child, parent, _) in edges {
        children_of.entry(*parent).or_default().insert(*child);
    }

    // Repeatable removal of units whose children are already ordered.
    let mut order: Vec<PlannedDeletion> = Vec::with_capacity(present.len());
    let mut placed: BTreeSet<ApprovedNativeUnit> = BTreeSet::new();
    loop {
        let ready: Vec<ApprovedNativeUnit> = present
            .iter()
            .filter(|unit| !placed.contains(unit))
            .filter(|unit| {
                children_of
                    .get(unit)
                    .is_none_or(|children| children.iter().all(|child| placed.contains(child)))
            })
            .copied()
            .collect();
        if ready.is_empty() {
            break;
        }
        for unit in ready {
            placed.insert(unit);
            order.push(PlannedDeletion {
                table: unit.table,
                id: unit.id,
            });
        }
    }

    if placed.len() == present.len() {
        return Ok((order, false));
    }

    // A cycle remains. It is only legal when every edge still unplaced is
    // deferrable, because then the deferred check after the complete set is what
    // proves consistency.
    let unplaced: BTreeSet<ApprovedNativeUnit> = present
        .iter()
        .filter(|unit| !placed.contains(unit))
        .copied()
        .collect();
    let blocking: Vec<&(ApprovedNativeUnit, ApprovedNativeUnit, bool)> = edges
        .iter()
        .filter(|(child, parent, _)| unplaced.contains(child) && unplaced.contains(parent))
        .collect();
    if blocking.is_empty() || blocking.iter().any(|(_, _, deferrable)| !deferrable) {
        let mut tables: Vec<String> = unplaced.iter().map(|unit| unit.table.to_string()).collect();
        tables.sort();
        tables.dedup();
        return Err(AppError::Validation(format!(
            "native_removal: non-deferrable reference cycle among {tables:?}"
        )));
    }
    let mut rest: Vec<PlannedDeletion> = unplaced
        .iter()
        .map(|unit| PlannedDeletion {
            table: unit.table,
            id: unit.id,
        })
        .collect();
    rest.sort_by(|left, right| (left.table, left.id).cmp(&(right.table, right.id)));
    order.extend(rest);
    Ok((order, true))
}

/// Whether `table` is a pure membership: only uuid columns, of which a unique
/// pair is the identity. Such a table stores a relation, never payload, so a
/// cascade onto it is the parent's own representation.
async fn membership_only_table(
    connection: &mut PgConnection,
    table: &str,
) -> Result<bool, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT count(*) FILTER (WHERE attribute.atttypid <> 'uuid'::regtype) AS non_uuid,
               count(*) AS total
        FROM pg_attribute attribute
        WHERE attribute.attrelid = to_regclass($1)
          AND attribute.attnum > 0
          AND NOT attribute.attisdropped
        "#,
    )
    .bind(table)
    .fetch_one(&mut *connection)
    .await?;
    let non_uuid: i64 = rows.try_get("non_uuid")?;
    let total: i64 = rows.try_get("total")?;
    if non_uuid != 0 || !(2..=3).contains(&total) {
        return Ok(false);
    }
    let unique_pair: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint WHERE conrelid = to_regclass($1)          AND contype IN ('p', 'u') AND array_length(conkey, 1) = 2",
    )
    .bind(table)
    .fetch_one(&mut *connection)
    .await?;
    Ok(unique_pair > 0)
}

/// Stored values of one custom-field definition across every native subject.
///
/// The subject list comes from the custom-fields owner, and the probe reads the
/// definition's own JSON key: a stored value is local data, so it blocks instead
/// of being dropped with the definition.
async fn custom_field_value_dependencies(
    connection: &mut PgConnection,
    identities: &mut IdentityCatalogue,
    subjects: &[NativeFieldSubject],
    definition_id: Uuid,
) -> Result<Vec<NativeDependency>, AppError> {
    let key: Option<String> = sqlx::query_scalar("SELECT name FROM custom_field WHERE id = $1")
        .bind(definition_id)
        .fetch_optional(&mut *connection)
        .await?;
    let Some(key) = key else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for subject in subjects {
        let identity = identities
            .identity(&mut *connection, &subject.table_name)
            .await?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(sample_projection(Some("owner"), identity.as_ref()))
            .push(", COUNT(*) OVER() AS total FROM public.");
        push_identifier(&mut query, &subject.table_name);
        query
            .push(" owner WHERE owner.custom_field_data ? ")
            .push_bind(key.clone())
            .push(" AND jsonb_typeof(owner.custom_field_data -> ")
            .push_bind(key.clone())
            .push(") <> 'null'")
            .push(sample_order(Some("owner"), identity.as_ref()))
            .push(" LIMIT 4");
        let rows = query.build().fetch_all(&mut *connection).await?;
        if rows.is_empty() {
            continue;
        }
        let count: i64 = rows[0].try_get("total")?;
        found.push(NativeDependency {
            class: NativeDependencyClass::CustomFieldValue,
            table: subject.table_name.clone(),
            count,
            sample: rows
                .iter()
                .map(|row| row.try_get::<Option<Uuid>, _>("id"))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect(),
            effect: NativeDependencyEffect::BlocksRemoval,
        });
    }
    Ok(found)
}

/// Derived state that is stale after a removal and must be repaired by its owner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DerivedStateRepair {
    /// Device owners whose component counters changed.
    pub devices: Vec<Uuid>,
    /// Virtual-machine owners whose component counters changed.
    pub virtual_machines: Vec<Uuid>,
    /// MPTT trees by owning table whose ordering changed.
    pub trees: Vec<(&'static str, i32)>,
    /// Prefix owners whose descendant state changed.
    pub prefixes: Vec<Uuid>,
}

/// Collect the repair work for deleted *and* upserted owners.
///
/// Every owner is inspected before its row is gone, so the request names the
/// derived state that must be recomputed instead of leaving it silently stale.
pub async fn plan_derived_state_repair(
    connection: &mut PgConnection,
    deleted: &[ApprovedNativeUnit],
    upserted: &[ApprovedNativeUnit],
) -> Result<DerivedStateRepair, AppError> {
    let mut repair = DerivedStateRepair::default();
    for unit in deleted.iter().chain(upserted.iter()) {
        let mut query = QueryBuilder::<Postgres>::new("SELECT * FROM ");
        query
            .push(unit.table)
            .push(" WHERE id = ")
            .push_bind(unit.id);
        let Some(row) = query.build().fetch_optional(&mut *connection).await? else {
            continue;
        };
        let columns: Vec<&str> = row.columns().iter().map(|column| column.name()).collect();
        if columns.contains(&"device_id") {
            if let Ok(device) = row.try_get::<Option<Uuid>, _>("device_id") {
                if let Some(device) = device {
                    repair.devices.push(device);
                }
            }
        }
        if columns.contains(&"virtual_machine_id") {
            if let Ok(machine) = row.try_get::<Option<Uuid>, _>("virtual_machine_id") {
                if let Some(machine) = machine {
                    repair.virtual_machines.push(machine);
                }
            }
        }
        if columns.contains(&"tree_id") {
            if let Ok(tree) = row.try_get::<Option<i32>, _>("tree_id") {
                if let Some(tree) = tree {
                    repair.trees.push((unit.table, tree));
                }
            }
        }
        if unit.table == "ipam_prefix" && columns.contains(&"parent_id") {
            repair.prefixes.push(unit.id);
        }
    }
    repair.devices.sort_unstable();
    repair.devices.dedup();
    repair.virtual_machines.sort_unstable();
    repair.virtual_machines.dedup();
    repair.trees.sort_unstable();
    repair.trees.dedup();
    repair.prefixes.sort_unstable();
    repair.prefixes.dedup();
    Ok(repair)
}

/// Execute an executable plan: fence, then delete children-first in one savepoint.
///
/// The fence is a `SHARE` lock on every child table the inspection saw, so a
/// concurrent writer cannot add a new local reference between inspection and
/// deletion.
pub async fn execute_native_removal(
    transaction: &mut Transaction<'_, Postgres>,
    plan: &NativeRemovalPlan,
) -> Result<u64, AppError> {
    plan.ensure_executable()?;
    if plan.order.is_empty() {
        return Ok(0);
    }
    fence_dependency_tables(transaction, plan.fences()).await?;
    let deletions: Vec<(String, Uuid)> = plan
        .order()
        .iter()
        .map(|unit| (unit.table.to_string(), unit.id))
        .collect();
    delete_records_guarded_in_savepoint(transaction, &deletions, inventory_blocker_metadata).await
}

/// Block concurrent writers on the dependent tables seen during inspection.
async fn fence_dependency_tables(
    transaction: &mut Transaction<'_, Postgres>,
    tables: &[String],
) -> Result<(), AppError> {
    let mut names: Vec<&str> = tables.iter().map(String::as_str).collect();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::<Postgres>::new("LOCK TABLE ");
    let mut first = true;
    for name in &names {
        if !first {
            query.push(", ");
        }
        first = false;
        query
            .push("public.\"")
            .push(name.replace('"', "\"\""))
            .push("\"");
    }
    query.push(" IN SHARE MODE");
    query.build().execute(&mut **transaction).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(table: &'static str, id: u128) -> ApprovedNativeUnit {
        ApprovedNativeUnit {
            table,
            id: Uuid::from_u128(id),
        }
    }

    #[test]
    fn a_child_is_ordered_before_its_parent() {
        let parent = unit("dcim_device", 1);
        let child = unit("dcim_interface", 2);
        let (order, deferred) =
            deletion_order(&[parent, child], &[(child, parent, false)]).expect("order");
        assert!(!deferred);
        assert_eq!(order[0].id, child.id);
        assert_eq!(order[1].id, parent.id);
    }

    #[test]
    fn a_cycle_is_absorbed_only_when_every_edge_is_deferrable() {
        let left = unit("dcim_device", 1);
        let right = unit("dcim_rack", 2);
        let deferrable =
            deletion_order(&[left, right], &[(left, right, true), (right, left, true)])
                .expect("deferrable cycle");
        assert!(deferrable.1);
        assert_eq!(deferrable.0.len(), 2);
        assert!(
            deletion_order(&[left, right], &[(left, right, true), (right, left, false)]).is_err()
        );
    }

    #[test]
    fn a_cable_relation_removal_names_its_end() {
        assert_eq!(CableEndToken::parse("A").expect("end").as_str(), "A");
        assert!(CableEndToken::parse("C").is_err());
        let relation = NativeRelationRemoval {
            holder_table: "dcim_interface",
            holder_id: Uuid::from_u128(3),
            kind: NativeRelationKind::Cable {
                cable_id: Uuid::from_u128(4),
                end: CableEndToken::B,
            },
        };
        assert!(matches!(
            relation.kind,
            NativeRelationKind::Cable {
                end: CableEndToken::B,
                ..
            }
        ));
    }

    #[test]
    fn a_scope_kind_only_names_the_table_its_owner_declares() {
        assert!(ScopeSubject::ConnectionTarget.names("dcim_device"));
        assert!(ScopeSubject::ConnectionTarget.names("virtualization_virtualmachine"));
        assert!(!ScopeSubject::ConnectionTarget.names("dcim_site"));
        assert!(ScopeSubject::Table("dcim_site").names("dcim_site"));
        assert!(!ScopeSubject::Table("dcim_site").names("dcim_region"));
        assert!(ScopeSubject::ContentType.names("dcim_interface"));
        assert!(!ScopeSubject::ContentType.names("custom_field"));
        assert!(!ScopeSubject::NoNativeUnit.names("dcim_device"));
        assert!(is_typed_owner_table("secret_usage_binding"));
        assert!(is_typed_owner_table("config_context_binding"));
        assert!(!is_typed_owner_table("removal_probe_scope"));
    }

    async fn seed_device_graph(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> (Uuid, Uuid, Uuid) {
        let manufacturer = Uuid::new_v4();
        let role = Uuid::new_v4();
        let site = Uuid::new_v4();
        let device_type = Uuid::new_v4();
        let device = Uuid::new_v4();
        let interface = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO dcim_manufacturer (id, name, slug, description, comments, custom_field_data) \
             VALUES ($1, 'removal manufacturer', 'removal-manufacturer', '', '', '{}')",
        )
        .bind(manufacturer)
        .execute(&mut **tx)
        .await
        .expect("manufacturer");
        sqlx::query(
            "INSERT INTO dcim_devicerole (id, name, slug, color, vm_role, description, level, lft, \
             rght, tree_id, comments, custom_field_data) \
             VALUES ($1, 'removal role', 'removal-role', 'ffffff', false, '', 0, 0, 0, 0, '', '{}')",
        )
        .bind(role)
        .execute(&mut **tx)
        .await
        .expect("role");
        sqlx::query(
            "INSERT INTO dcim_site (id, name, slug, status, facility, description, \
             physical_address, shipping_address, comments, custom_field_data) \
             VALUES ($1, 'removal site', 'removal-site', 'active', '', '', '', '', '', '{}')",
        )
        .bind(site)
        .execute(&mut **tx)
        .await
        .expect("site");
        sqlx::query(
            "INSERT INTO dcim_devicetype (id, model, slug, part_number, u_height, is_full_depth, \
             front_image, rear_image, comments, manufacturer_id, description, \
             console_port_template_count, console_server_port_template_count, \
             power_port_template_count, power_outlet_template_count, interface_template_count, \
             front_port_template_count, rear_port_template_count, device_bay_template_count, \
             module_bay_template_count, inventory_item_template_count, exclude_from_utilization, \
             device_count, custom_field_data) \
             VALUES ($1, 'removal-model', 'removal-model', '', 1, false, '', '', '', $2, '', \
             0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, 0, '{}')",
        )
        .bind(device_type)
        .bind(manufacturer)
        .execute(&mut **tx)
        .await
        .expect("device type");
        sqlx::query(
            "INSERT INTO dcim_device (id, name, serial, status, comments, role_id, device_type_id, \
             site_id, description, console_port_count, console_server_port_count, power_port_count, \
             power_outlet_count, interface_count, front_port_count, rear_port_count, \
             device_bay_count, module_bay_count, inventory_item_count, custom_field_data) \
             VALUES ($1, 'removal-device', '', 'active', '', $2, $3, $4, '', \
             0, 0, 0, 0, 0, 0, 0, 0, 0, 0, '{}')",
        )
        .bind(device)
        .bind(role)
        .bind(device_type)
        .bind(site)
        .execute(&mut **tx)
        .await
        .expect("device");
        sqlx::query(
            "INSERT INTO dcim_interface (id, device_id, name, label, description, mark_connected, \
             enabled, _name, type, mgmt_only, custom_field_data) \
             VALUES ($1, $2, 'eth0', '', '', false, true, 'eth0', '1000base-t', false, '{}')",
        )
        .bind(interface)
        .bind(device)
        .execute(&mut **tx)
        .await
        .expect("interface");
        (site, device, interface)
    }

    async fn seed_user(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Uuid {
        let user = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO identity_user (id, username, email, status, auth_source) \
             VALUES ($1, 'removal-user', 'removal@example.test', 'active', 'local')",
        )
        .bind(user)
        .execute(&mut **tx)
        .await
        .expect("user");
        user
    }

    fn assert_typed_blocker(plan: &NativeRemovalPlan, class: NativeDependencyClass, table: &str) {
        assert!(
            plan.blockers().iter().any(|blocker| matches!(
                blocker,
                NativeBlocker::TypedDependency { class: actual, table: blocked, count, .. }
                    if *actual == class && blocked == table && *count >= 1
            )),
            "expected a {class:?} blocker for {table}, got {:?}",
            plan.blockers()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_dependency_free_device_interface_and_field_set_is_executable(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (_, device, interface) = seed_device_graph(&mut tx).await;
        let field = Uuid::new_v4();
        sqlx::query("INSERT INTO custom_field (id, name) VALUES ($1, 'unused_removal_field')")
            .bind(field)
            .execute(&mut *tx)
            .await
            .expect("field");

        let plan = plan_native_removal(
            &mut tx,
            &[
                ApprovedNativeUnit {
                    table: "dcim_device",
                    id: device,
                },
                ApprovedNativeUnit {
                    table: "dcim_interface",
                    id: interface,
                },
                ApprovedNativeUnit {
                    table: "custom_field",
                    id: field,
                },
            ],
            &[],
        )
        .await
        .expect("plan");

        assert!(
            plan.blockers().is_empty(),
            "unexpected blockers: {:?}",
            plan.blockers()
        );
        plan.ensure_executable().expect("executable plan");
        assert_eq!(plan.order().len(), 3);
        assert!(plan.effects().iter().any(|effect| matches!(
            effect,
            PlannedEffect::RemovedWithApprovedUnit { child_table, .. } if child_table == "dcim_interface"
        )));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn an_eligible_dependent_set_is_removed_children_first(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (_, device, interface) = seed_device_graph(&mut tx).await;
        let plan = plan_native_removal(
            &mut tx,
            &[
                ApprovedNativeUnit {
                    table: "dcim_device",
                    id: device,
                },
                ApprovedNativeUnit {
                    table: "dcim_interface",
                    id: interface,
                },
            ],
            &[],
        )
        .await
        .expect("plan");
        plan.ensure_executable().expect("executable plan");

        execute_native_removal(&mut tx, &plan)
            .await
            .expect("execute");
        let remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM dcim_interface WHERE id = $1")
                .bind(interface)
                .fetch_one(&mut *tx)
                .await
                .expect("interface count");
        assert_eq!(remaining, 0);
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_device WHERE id = $1")
            .bind(device)
            .fetch_one(&mut *tx)
            .await
            .expect("device count");
        assert_eq!(remaining, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_retained_dependent_blocks_and_leaves_every_row_unchanged(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (_, device, interface) = seed_device_graph(&mut tx).await;
        let plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("plan");
        assert!(plan.blockers().iter().any(|blocker| matches!(
            blocker,
            NativeBlocker::RetainedDependent { child_table, count, .. }
                if child_table == "dcim_interface" && *count == 1
        )));
        plan.ensure_executable().expect_err("retained child blocks");

        execute_native_removal(&mut tx, &plan)
            .await
            .expect_err("execution refuses");
        let device_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_device WHERE id = $1")
            .bind(device)
            .fetch_one(&mut *tx)
            .await
            .expect("device count");
        let interface_rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM dcim_interface WHERE id = $1")
                .bind(interface)
                .fetch_one(&mut *tx)
                .await
                .expect("interface count");
        assert_eq!((device_rows, interface_rows), (1, 1));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_secret_consumer_blocks_without_detaching_the_binding(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (_, device, _) = seed_device_graph(&mut tx).await;
        let secret = Uuid::new_v4();
        let binding = Uuid::new_v4();
        sqlx::query("INSERT INTO secrets_variable (id, key, kind) VALUES ($1, 'removal', 'named')")
            .bind(secret)
            .execute(&mut *tx)
            .await
            .expect("secret");
        sqlx::query(
            "INSERT INTO secret_usage_binding (id, consumer_type, consumer_id, field_key, \
             owner_scope_type, owner_scope_id, secret_id) \
             VALUES ($1, 'connection_scope:connection_target', $2, 'ssh_password', \
             'connection_target', $2, $3)",
        )
        .bind(binding)
        .bind(device)
        .bind(secret)
        .execute(&mut *tx)
        .await
        .expect("binding");

        let plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("plan");
        assert_typed_blocker(
            &plan,
            NativeDependencyClass::SecretConsumer,
            "secret_usage_binding",
        );
        execute_native_removal(&mut tx, &plan)
            .await
            .expect_err("secret consumer blocks");
        let bindings: i64 =
            sqlx::query_scalar("SELECT count(*) FROM secret_usage_binding WHERE id = $1")
                .bind(binding)
                .fetch_one(&mut *tx)
                .await
                .expect("binding count");
        assert_eq!(bindings, 1, "native removal never detaches a secret");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn monitoring_scopes_block_through_their_typed_columns(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (site, device, _) = seed_device_graph(&mut tx).await;
        let profile = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO monitoring_profile (id, name, slug) VALUES ($1, 'p', 'removal-p')",
        )
        .bind(profile)
        .execute(&mut *tx)
        .await
        .expect("profile");
        sqlx::query(
            "INSERT INTO monitoring_profile_assignment (id, profile_id, scope_type, scope_id) \
             VALUES ($1, $2, 'site', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(profile)
        .bind(site)
        .execute(&mut *tx)
        .await
        .expect("assignment");
        let metric_query = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO monitoring_metric_query (id, name, slug, query) \
             VALUES ($1, 'removal query', 'removal-query', 'up')",
        )
        .bind(metric_query)
        .execute(&mut *tx)
        .await
        .expect("metric query");
        sqlx::query(
            "INSERT INTO monitoring_metric_query_host_override (id, metric_query_id, host_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(Uuid::new_v4())
        .bind(metric_query)
        .bind(device)
        .execute(&mut *tx)
        .await
        .expect("host override");

        let site_plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_site",
                id: site,
            }],
            &[],
        )
        .await
        .expect("site plan");
        assert_typed_blocker(
            &site_plan,
            NativeDependencyClass::MonitoringScope,
            "monitoring_profile_assignment",
        );

        let device_plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("device plan");
        assert_typed_blocker(
            &device_plan,
            NativeDependencyClass::MonitoringScope,
            "monitoring_metric_query_host_override",
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn playbook_selectors_and_static_members_block(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (site, device, _) = seed_device_graph(&mut tx).await;
        sqlx::query(
            "INSERT INTO playbook_target_set (id, name, mode, selector) \
             VALUES ($1, 'removal set', 'dynamic', jsonb_build_object('site_ids', jsonb_build_array($2::text)))",
        )
        .bind(Uuid::new_v4())
        .bind(site)
        .execute(&mut *tx)
        .await
        .expect("target set");
        let target_set = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO playbook_target_set (id, name, mode, selector) \
             VALUES ($1, 'removal static set', 'static', '{}')",
        )
        .bind(target_set)
        .execute(&mut *tx)
        .await
        .expect("static set");
        sqlx::query(
            "INSERT INTO playbook_target_set_member \
             (target_set_id, target_kind, target_id, source_kind, source_id) \
             VALUES ($1, 'connection_target', $2, 'device', $2)",
        )
        .bind(target_set)
        .bind(device)
        .execute(&mut *tx)
        .await
        .expect("member");

        let site_plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_site",
                id: site,
            }],
            &[],
        )
        .await
        .expect("site plan");
        assert_typed_blocker(
            &site_plan,
            NativeDependencyClass::PlaybookReference,
            "playbook_target_set",
        );

        let device_plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("device plan");
        assert_typed_blocker(
            &device_plan,
            NativeDependencyClass::PlaybookReference,
            "playbook_target_set_member",
        );
        // The member row is identified by its whole membership key, not by an
        // `id`: the effect is counted and blocks, and no sample key is invented
        // for a column the table does not have.
        let member_dependencies: Vec<&NativeDependency> = device_plan
            .dependencies()
            .iter()
            .filter(|dependency| dependency.table == "playbook_target_set_member")
            .collect();
        assert!(!member_dependencies.is_empty(), "member effect is reported");
        for dependency in member_dependencies {
            assert_eq!(dependency.count, 1, "{dependency:?}");
            assert!(dependency.sample.is_empty(), "{dependency:?}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_config_context_binding_blocks(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (_, device, _) = seed_device_graph(&mut tx).await;
        let context = Uuid::new_v4();
        sqlx::query("INSERT INTO config_context (id, name, data) VALUES ($1, 'removal', '{}')")
            .bind(context)
            .execute(&mut *tx)
            .await
            .expect("context");
        sqlx::query(
            "INSERT INTO config_context_binding (id, context_id, scope_type, scope_id) \
             VALUES ($1, $2, 'device', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(context)
        .bind(device)
        .execute(&mut *tx)
        .await
        .expect("binding");

        let plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("plan");
        assert_typed_blocker(
            &plan,
            NativeDependencyClass::ConfigContextScope,
            "config_context_binding",
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn connection_config_and_proxy_scopes_block(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (_, device, _) = seed_device_graph(&mut tx).await;
        let record = Uuid::new_v4();
        let proxy = Uuid::new_v4();
        sqlx::query("INSERT INTO connconf_record (id, name, protocol) VALUES ($1, 'r', 'ssh')")
            .bind(record)
            .execute(&mut *tx)
            .await
            .expect("record");
        sqlx::query(
            "INSERT INTO connconf_assignment (id, record_id, scope_type, scope_id) \
             VALUES ($1, $2, 'device', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(record)
        .bind(device)
        .execute(&mut *tx)
        .await
        .expect("assignment");
        sqlx::query(
            "INSERT INTO connections_proxy_node (id, name, slug, base_url) \
             VALUES ($1, 'removal proxy', 'removal-proxy', 'https://proxy.example.test')",
        )
        .bind(proxy)
        .execute(&mut *tx)
        .await
        .expect("proxy");
        sqlx::query(
            "INSERT INTO connections_proxy_assignment (id, proxy_node_id, scope_type, scope_id) \
             VALUES ($1, $2, 'device', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(proxy)
        .bind(device)
        .execute(&mut *tx)
        .await
        .expect("proxy assignment");

        let plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("plan");
        assert_typed_blocker(
            &plan,
            NativeDependencyClass::ConnectionTarget,
            "connconf_assignment",
        );
        assert_typed_blocker(
            &plan,
            NativeDependencyClass::ConnectionTarget,
            "connections_proxy_assignment",
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_live_session_blocks_and_terminal_history_does_not(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let (_, device, _) = seed_device_graph(&mut tx).await;
        let user = seed_user(&mut tx).await;
        let session = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO connections_session (id, target_id, user_id, protocol, status) \
             VALUES ($1, $2, $3, 'ssh', 'active')",
        )
        .bind(session)
        .bind(device)
        .bind(user)
        .execute(&mut *tx)
        .await
        .expect("session");

        let plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("plan");
        assert_typed_blocker(
            &plan,
            NativeDependencyClass::SessionOrHistory,
            "connections_session",
        );

        sqlx::query(
            "UPDATE connections_session SET status = 'terminated', ended_at = now() WHERE id = $1",
        )
        .bind(session)
        .execute(&mut *tx)
        .await
        .expect("terminate session");

        let plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "dcim_device",
                id: device,
            }],
            &[],
        )
        .await
        .expect("plan");
        assert!(
            !plan.blockers().iter().any(|blocker| matches!(
                blocker,
                NativeBlocker::TypedDependency {
                    class: NativeDependencyClass::SessionOrHistory,
                    ..
                }
            )),
            "terminal session history is not a live reference"
        );
        assert!(plan.dependencies().iter().any(|dependency| matches!(
            dependency,
            NativeDependency { class: NativeDependencyClass::SessionOrHistory, table, count: 1, effect: NativeDependencyEffect::RetainedHistory, .. }
                if table.as_str() == "connections_session"
        )));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn an_unknown_scope_column_stays_unattributed(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let field = Uuid::new_v4();
        let site = Uuid::new_v4();
        sqlx::query("INSERT INTO custom_field (id, name) VALUES ($1, 'removal_probe')")
            .bind(field)
            .execute(&mut *tx)
            .await
            .expect("field");
        sqlx::query(
            "INSERT INTO dcim_site (id, name, slug, status, facility, description, \
             physical_address, shipping_address, comments, custom_field_data) \
             VALUES ($1, 'removal probe', 'removal-probe', 'active', '', '', '', '', '', \
             '{\"removal_probe\":\"local\"}')",
        )
        .bind(site)
        .execute(&mut *tx)
        .await
        .expect("site");
        sqlx::query("CREATE TABLE public.removal_probe_scope (id uuid PRIMARY KEY, scope_id uuid)")
            .execute(&mut *tx)
            .await
            .expect("scope table");
        sqlx::query("INSERT INTO public.removal_probe_scope VALUES ($1, $2)")
            .bind(Uuid::new_v4())
            .bind(site)
            .execute(&mut *tx)
            .await
            .expect("scope");

        let plan = plan_native_removal(
            &mut tx,
            &[
                ApprovedNativeUnit {
                    table: "dcim_site",
                    id: site,
                },
                ApprovedNativeUnit {
                    table: "custom_field",
                    id: field,
                },
            ],
            &[],
        )
        .await
        .expect("plan");
        assert_typed_blocker(&plan, NativeDependencyClass::CustomFieldValue, "dcim_site");
        assert_typed_blocker(
            &plan,
            NativeDependencyClass::UnattributedScope,
            "removal_probe_scope.scope_id",
        );
        plan.ensure_executable().expect_err("both references block");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_composite_key_child_blocks_instead_of_inventing_an_identity(pool: sqlx::PgPool) {
        let mut tx = pool.begin().await.expect("transaction");
        let field = Uuid::new_v4();
        sqlx::query("INSERT INTO custom_field (id, name) VALUES ($1, 'composite_probe')")
            .bind(field)
            .execute(&mut *tx)
            .await
            .expect("field");
        let content_type: Uuid =
            sqlx::query_scalar("SELECT id FROM inventory_content_type LIMIT 1")
                .fetch_one(&mut *tx)
                .await
                .expect("content type");
        // `custom_field_object_type` is keyed by its whole pair and has no `id`.
        sqlx::query(
            "INSERT INTO custom_field_object_type (custom_field_id, content_type_id) \
             VALUES ($1, $2)",
        )
        .bind(field)
        .bind(content_type)
        .execute(&mut *tx)
        .await
        .expect("object type");

        let plan = plan_native_removal(
            &mut tx,
            &[ApprovedNativeUnit {
                table: "custom_field",
                id: field,
            }],
            &[],
        )
        .await
        .expect("plan");
        assert!(
            plan.blockers().iter().any(|blocker| matches!(
                blocker,
                NativeBlocker::UnsupportedEffect { constraint, detail }
                    if constraint == "custom_field_object_type_field_fkey"
                        && detail.contains("custom_field_object_type")
                        && detail.contains("keeps 1 referencing")
            )),
            "unexpected blockers: {:?}",
            plan.blockers()
        );
        plan.ensure_executable()
            .expect_err("unidentifiable child blocks");
        execute_native_removal(&mut tx, &plan)
            .await
            .expect_err("execution refuses");
        let fields: i64 = sqlx::query_scalar("SELECT count(*) FROM custom_field WHERE id = $1")
            .bind(field)
            .fetch_one(&mut *tx)
            .await
            .expect("field count");
        assert_eq!(fields, 1, "the definition is left untouched");
    }
}
