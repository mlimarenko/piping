//! Optional NetBox `data-flows` plugin source contract (PIP-606).
//!
//! The `data-flows` plugin is not part of core NetBox: an installation may have
//! it, may never have had it, or may lose it. Core inventory must not change in
//! any of those cases. When the plugin *is* installed, its five collections
//! carry the logical flow intent the native catalogue already models, so they
//! are imported through the same read-only pull.
//!
//! This module owns only that optional projection. It installs nothing, writes
//! nothing, and requests no page: it resolves capability from the canonical
//! provider registry and projects already-fetched JSON into [`SourceRecord`]s.
//! The single `pull_only` source runner keeps the only GET path.
//!
//! Four capability states are distinguished and never collapsed:
//!
//! * [`DataflowCapability::Absent`] - the provider schema was read and exposes
//!   none of the five collections.
//! * [`DataflowCapability::Present`] - all five collections exist and their read
//!   serializers match this module's closed field contract exactly.
//! * [`DataflowCapability::Forbidden`] - the schema read was refused by
//!   authorization.
//! * [`DataflowCapability::Failed`] - discovery failed for any other reason.
//!
//! Only `Present` contributes source entities
//! ([`DataflowCapability::pull_entities`]). Every other state contributes no
//! entity and therefore no traversal evidence: a plugin that disappears, or
//! that cannot be read, keeps the data earlier pulls imported and can never
//! look like a completed empty traversal. Omission is not deletion authority;
//! the two-observation absence contract belongs to `sync_inventory_absence`,
//! which only sees the entities the source actually traversed.
//!
//! `Present` is a closed contract. The read serializer's field set must equal
//! the table below, every value is classified, and a populated field the local
//! model cannot represent fails with the exact collection and field in the
//! error code instead of being dropped or turned into an invented local row.
//!
//! Identity is the provider's integer id (`normalize_netbox_id`), as for every
//! other NetBox entity. Relations stay provider integer ids: they reach local
//! UUIDs only through the existing provider binding table, never by copying an
//! upstream id into a local column.
//!
//! Local semantics of the five collections:
//!
//! * `application-roles` -> `inventory_application_role`
//! * `applications` -> `inventory_application`
//! * `dataflow-groups` -> `inventory_dataflow_group`
//! * `dataflows` -> `inventory_dataflow` plus ordered `inventory_dataflow_selector`
//!   rows for `sources`/`destinations`
//! * `objectalias` -> `inventory_address_group`, the reusable named address set,
//!   plus ordered member selector rows; a flow endpoint that names an
//!   `ObjectAlias` is a selector of object type `inventory.addressgroup`.
//!
//! Array order is the only ordering authority, exactly as in the native
//! repository: the projection emits `members` and `sources`/`destinations` in
//! upstream array order and never invents a `position`.

#![allow(
    dead_code,
    reason = "the optional data-flows contract keeps constructors used only by schema variants and by its focused tests; the pull runner consumes the capability and projection paths"
)]

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::contexts::inventory::domain::native::catalog::MAX_ADDRESS_GROUP_MEMBERS;
use crate::contexts::inventory::domain::native::dataflow::{
    DataflowAddressMode, DataflowStatus, DataflowTransport, EndpointType,
};
use crate::contexts::sync::domain::inventory_sync::normalize_netbox_id;
use crate::contexts::sync::domain::{SourceEntity, SourceIdentity, SourceRecord};
use crate::errors::AppError;
use crate::integrations::netbox::NetboxResourceRegistry;

use super::netbox_source_projection::relation_id;

/// One flow side allows 1..=32 selectors, the native selector bound.
const MAX_FLOW_ENDPOINTS: usize = 32;
/// The native port columns hold 1..=256 values in 1..=65535.
const MAX_PORTS: usize = 256;

/// Upstream `status` choice values.
///
/// The native catalogue has no `enabled` flag: `enabled` is imported as
/// `active`, `disabled` as `disabled`, and the local-only `draft` state is never
/// invented from a pull.
const STATUS_VALUES: &[&str] = &["enabled", "disabled"];
/// Upstream `protocol` choice values.
const PROTOCOL_VALUES: &[&str] = &["any", "icmp", "icmpv6", "tcp", "udp", "tcp+udp", "sctp"];
/// The brief alias fields a flow endpoint carries beside its id.
const BRIEF_ALIAS_FIELDS: &[&str] = &["id", "name", "description", "display", "url"];

/// One collection of the optional plugin namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataflowCollection {
    ApplicationRoles,
    Applications,
    DataflowGroups,
    Dataflows,
    ObjectAliases,
}

impl DataflowCollection {
    /// Every collection of the namespace, in manifest order.
    pub(crate) const ALL: [Self; 5] = [
        Self::ApplicationRoles,
        Self::Applications,
        Self::DataflowGroups,
        Self::Dataflows,
        Self::ObjectAliases,
    ];

    /// Canonical source entity key, identical to the registry resource key.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::ApplicationRoles => "data-flows.application-roles",
            Self::Applications => "data-flows.applications",
            Self::DataflowGroups => "data-flows.dataflow-groups",
            Self::Dataflows => "data-flows.dataflows",
            Self::ObjectAliases => "data-flows.objectalias",
        }
    }

    /// Canonical list path the registry descriptor must be bound to.
    pub(crate) const fn list_path(self) -> &'static str {
        match self {
            Self::ApplicationRoles => "api/plugins/data-flows/application-roles/",
            Self::Applications => "api/plugins/data-flows/applications/",
            Self::DataflowGroups => "api/plugins/data-flows/dataflow-groups/",
            Self::Dataflows => "api/plugins/data-flows/dataflows/",
            Self::ObjectAliases => "api/plugins/data-flows/objectalias/",
        }
    }

    pub(crate) fn entity(self) -> Result<SourceEntity, AppError> {
        SourceEntity::new(self.key())
    }

    pub(crate) fn of(entity: &SourceEntity) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|collection| collection.key() == entity.as_str())
    }

    fn contract(self) -> &'static [FieldContract] {
        match self {
            Self::ApplicationRoles => APPLICATION_ROLE_FIELDS,
            Self::Applications => APPLICATION_FIELDS,
            Self::DataflowGroups => DATAFLOW_GROUP_FIELDS,
            Self::Dataflows => DATAFLOW_FIELDS,
            Self::ObjectAliases => OBJECT_ALIAS_FIELDS,
        }
    }
}

/// How one upstream field is treated.
#[derive(Debug, Clone, Copy)]
enum FieldShape {
    /// The provider identity of the record.
    Identity,
    /// A non-null text value copied under its canonical name.
    Imported,
    /// The `custom_fields` object copied to the native JSON column.
    CustomFields,
    /// A nullable relation, normalised to its provider integer id.
    Relation,
    /// A `NestedTag` array normalised to `[{"id": n}]` provider identities.
    TagIds,
    /// `{value,label}` reduced to the local status vocabulary.
    StatusChoice,
    /// `{value,label}` reduced to the local transport vocabulary.
    ProtocolChoice,
    /// `created`/`last_updated`, stored as provider observations only.
    Observation,
    /// A read-only server-derived value; never imported and never trusted as
    /// local state, because the effective status, effective tags and tree depth
    /// are derived locally from the owning hierarchy.
    Derived,
    /// `slug`: the native catalog has no column and identifies these records by
    /// name within their scope, so the value is validated and not stored.
    Slug,
    /// A field the native entity has no column for at all. An empty value is
    /// accepted, a populated one fails: the plugin reports intent the native
    /// model cannot hold.
    NotStored,
    /// A nullable integer array checked against the native port bounds.
    Ports,
    /// A `BriefObjectAlias` array projected to ordered alias selectors.
    Selectors,
    /// One of the three `ObjectAlias` address arrays, folded into `members`.
    Members(EndpointType),
}

/// One classified upstream field.
#[derive(Debug, Clone, Copy)]
struct FieldContract {
    name: &'static str,
    shape: FieldShape,
    /// Whether a canonical read may spell this field `null`.
    nullable: bool,
}

const fn field(name: &'static str, shape: FieldShape, nullable: bool) -> FieldContract {
    FieldContract {
        name,
        shape,
        nullable,
    }
}

const APPLICATION_ROLE_FIELDS: &[FieldContract] = &[
    field("id", FieldShape::Identity, false),
    field("url", FieldShape::Derived, false),
    field("display", FieldShape::Derived, false),
    field("name", FieldShape::Imported, false),
    field("slug", FieldShape::Slug, false),
    field("description", FieldShape::Imported, false),
    field("comments", FieldShape::Imported, false),
    // The native role owns no tag, custom-field or tenant column.
    field("tags", FieldShape::NotStored, false),
    field("custom_fields", FieldShape::NotStored, false),
    field("created", FieldShape::Observation, true),
    field("last_updated", FieldShape::Observation, true),
    field("owner", FieldShape::Relation, true),
    field("application_count", FieldShape::Derived, false),
];

const APPLICATION_FIELDS: &[FieldContract] = &[
    field("id", FieldShape::Identity, false),
    field("url", FieldShape::Derived, false),
    field("display", FieldShape::Derived, false),
    field("name", FieldShape::Imported, false),
    field("role", FieldShape::Relation, true),
    field("description", FieldShape::Imported, false),
    field("comments", FieldShape::Imported, false),
    field("tags", FieldShape::TagIds, false),
    field("custom_fields", FieldShape::CustomFields, false),
    field("created", FieldShape::Observation, true),
    field("last_updated", FieldShape::Observation, true),
    field("tenant", FieldShape::Relation, true),
    field("dataflow_count", FieldShape::Derived, false),
    field("owner", FieldShape::Relation, true),
];

const DATAFLOW_GROUP_FIELDS: &[FieldContract] = &[
    field("id", FieldShape::Identity, false),
    field("url", FieldShape::Derived, false),
    field("display", FieldShape::Derived, false),
    field("name", FieldShape::Imported, false),
    field("slug", FieldShape::Slug, false),
    field("parent", FieldShape::Relation, true),
    field("description", FieldShape::Imported, false),
    field("comments", FieldShape::Imported, false),
    field("status", FieldShape::StatusChoice, false),
    field("inherited_status", FieldShape::Derived, false),
    field("tags", FieldShape::TagIds, false),
    field("inherited_tags", FieldShape::Derived, false),
    field("created", FieldShape::Observation, true),
    field("last_updated", FieldShape::Observation, true),
    field("custom_fields", FieldShape::CustomFields, false),
    field("tenant", FieldShape::Relation, true),
    field("application", FieldShape::Relation, true),
    field("_depth", FieldShape::Derived, false),
    field("owner", FieldShape::Relation, true),
];

const DATAFLOW_FIELDS: &[FieldContract] = &[
    field("id", FieldShape::Identity, false),
    field("url", FieldShape::Derived, false),
    field("display", FieldShape::Derived, false),
    field("name", FieldShape::Imported, false),
    field("application", FieldShape::Relation, true),
    field("description", FieldShape::Imported, false),
    field("comments", FieldShape::Imported, false),
    field("status", FieldShape::StatusChoice, false),
    field("inherited_status", FieldShape::Derived, false),
    field("group", FieldShape::Relation, true),
    field("protocol", FieldShape::ProtocolChoice, false),
    field("source_ports", FieldShape::Ports, true),
    field("destination_ports", FieldShape::Ports, true),
    field("sources", FieldShape::Selectors, false),
    field("destinations", FieldShape::Selectors, false),
    field("tags", FieldShape::TagIds, false),
    field("inherited_tags", FieldShape::Derived, false),
    field("created", FieldShape::Observation, true),
    field("last_updated", FieldShape::Observation, true),
    field("custom_fields", FieldShape::CustomFields, false),
    field("tenant", FieldShape::Relation, true),
    field("owner", FieldShape::Relation, true),
];

const OBJECT_ALIAS_FIELDS: &[FieldContract] = &[
    field("id", FieldShape::Identity, false),
    field("url", FieldShape::Derived, false),
    field("display", FieldShape::Derived, false),
    field("name", FieldShape::Imported, false),
    field("description", FieldShape::Imported, false),
    field("comments", FieldShape::Imported, false),
    field("prefixes", FieldShape::Members(EndpointType::Prefix), false),
    field(
        "ip_ranges",
        FieldShape::Members(EndpointType::IpRange),
        false,
    ),
    field(
        "ip_addresses",
        FieldShape::Members(EndpointType::IpAddress),
        false,
    ),
    field("tags", FieldShape::TagIds, false),
    field("owner", FieldShape::Relation, true),
];

/// Optional namespace capability of one provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataflowCapability {
    /// The provider schema was read and exposes no plugin collection.
    Absent,
    /// Every collection exists with the exact shape this module projects.
    Present,
    /// The provider schema was refused by authorization.
    Forbidden,
    /// Discovery failed for any reason other than authorization.
    Failed,
}

impl DataflowCapability {
    /// Reporting token; safe for job statistics.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Present => "present",
            Self::Forbidden => "forbidden",
            Self::Failed => "failed",
        }
    }

    pub(crate) const fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }

    /// Source entities this pull may request.
    ///
    /// Empty unless `Present`: an absent or unreadable plugin contributes no
    /// namespace, so no later stage can read omission as an empty traversal.
    pub(crate) fn pull_entities(self) -> Result<Vec<SourceEntity>, AppError> {
        if !self.is_present() {
            return Ok(Vec::new());
        }
        optional_source_entities()
    }

    /// Whether a completed pull of this namespace may state absences at all.
    ///
    /// Only a fully declared and readable plugin may; every other state retains
    /// what earlier pulls imported.
    pub(crate) const fn contributes_absence_evidence(self) -> bool {
        self.is_present()
    }
}

/// Every source entity of the optional namespace, for mapping validation.
pub(crate) fn optional_source_entities() -> Result<Vec<SourceEntity>, AppError> {
    DataflowCollection::ALL
        .into_iter()
        .map(DataflowCollection::entity)
        .collect()
}

/// Classify a provider schema read failure.
///
/// Only a typed authorization outcome is `Forbidden`; a status the client
/// collapses into an untyped external error stays `Failed`. Guessing the state
/// from a message would turn an unreadable plugin into a claimed absence.
pub(crate) fn capability_from_discovery_error(error: &AppError) -> DataflowCapability {
    match error {
        AppError::Auth(_) | AppError::Forbidden(_) => DataflowCapability::Forbidden,
        _ => DataflowCapability::Failed,
    }
}

/// Resolve the optional capability from the canonical provider registry.
///
/// `Absent` is a positive result: the registry was built from a read schema and
/// exposes no collection of the namespace. A partially exposed namespace, or a
/// serializer whose field set, identity type or choice vocabulary differs from
/// the contract, is an unsupported shape and fails with the exact field.
pub(crate) fn resolve_capability(
    registry: &NetboxResourceRegistry,
) -> Result<DataflowCapability, AppError> {
    let exposed: Vec<DataflowCollection> = DataflowCollection::ALL
        .into_iter()
        .filter(|collection| registry.has_resource(collection.key()))
        .collect();
    if exposed.is_empty() {
        return Ok(DataflowCapability::Absent);
    }
    if exposed.len() != DataflowCollection::ALL.len() {
        let missing: Vec<&str> = DataflowCollection::ALL
            .into_iter()
            .filter(|collection| !exposed.contains(collection))
            .map(DataflowCollection::key)
            .collect();
        return Err(AppError::Validation(format!(
            "netbox_dataflow:capability.partial:{}",
            missing.join(",")
        )));
    }

    for collection in DataflowCollection::ALL {
        let descriptor = registry.get_descriptor(collection.key()).ok_or_else(|| {
            AppError::Validation(format!(
                "netbox_dataflow:{}.descriptor:missing",
                collection.key()
            ))
        })?;
        if descriptor.list_path != collection.list_path() || !descriptor.supports_operation("list")
        {
            return Err(AppError::Validation(format!(
                "netbox_dataflow:{}.descriptor:not_the_dataflow_collection",
                collection.key()
            )));
        }
        let properties = registry
            .response_schema_properties(collection.key())
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "netbox_dataflow:{}.read_serializer:missing",
                    collection.key()
                ))
            })?;
        validate_contract(collection, properties)?;
    }
    Ok(DataflowCapability::Present)
}

fn validate_contract(
    collection: DataflowCollection,
    properties: &Map<String, Value>,
) -> Result<(), AppError> {
    for name in properties.keys() {
        if !collection
            .contract()
            .iter()
            .any(|field| field.name == name.as_str())
        {
            return Err(reject(
                collection,
                name.as_str(),
                "unclassified_upstream_field",
            ));
        }
    }
    for field in collection.contract() {
        let property = properties
            .get(field.name)
            .ok_or_else(|| reject(collection, field.name, "missing_upstream_field"))?;
        match field.shape {
            FieldShape::Identity => {
                if property.get("type").and_then(Value::as_str) != Some("integer") {
                    return Err(reject(collection, field.name, "identity_type"));
                }
            }
            FieldShape::StatusChoice => {
                require_choice(collection, field.name, property, STATUS_VALUES)?;
            }
            FieldShape::ProtocolChoice => {
                require_choice(collection, field.name, property, PROTOCOL_VALUES)?;
            }
            FieldShape::TagIds
            | FieldShape::Ports
            | FieldShape::Selectors
            | FieldShape::Members(_) => {
                if property.get("type").and_then(Value::as_str) != Some("array") {
                    return Err(reject(collection, field.name, "array_type"));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn require_choice(
    collection: DataflowCollection,
    name: &str,
    property: &Value,
    expected: &[&str],
) -> Result<(), AppError> {
    let mut actual: Vec<&str> = property
        .get("properties")
        .and_then(|properties| properties.get("value"))
        .and_then(|value| value.get("enum"))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .ok_or_else(|| reject(collection, name, "choice_shape"))?;
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    if actual != expected {
        return Err(reject(collection, name, "choice_drift"));
    }
    Ok(())
}

/// Project one fetched page of the optional namespace.
///
/// Every collection here is a direct list resource: one record per upstream row,
/// so the shared page guard keeps its existing meaning and
/// `page_cursor(offset, rows_read, count)` stays the only pagination authority.
/// A page whose declared count is not reachable by the rows read is an error,
/// not an empty result.
pub(crate) fn project_page(
    provider_id: Uuid,
    entity: &SourceEntity,
    values: &[Value],
) -> Result<Vec<SourceRecord>, AppError> {
    let collection = DataflowCollection::of(entity).ok_or_else(|| {
        AppError::Validation(format!(
            "netbox_dataflow:{}:not_a_dataflow_entity",
            entity.as_str()
        ))
    })?;
    let mut records = Vec::with_capacity(values.len());
    for value in values {
        records.push(project_record(provider_id, collection, value)?);
    }
    Ok(records)
}

fn project_record(
    provider_id: Uuid,
    collection: DataflowCollection,
    value: &Value,
) -> Result<SourceRecord, AppError> {
    let record = value
        .as_object()
        .ok_or_else(|| reject(collection, "record", "shape"))?;
    for name in record.keys() {
        if !collection
            .contract()
            .iter()
            .any(|field| field.name == name.as_str())
        {
            return Err(reject(
                collection,
                name.as_str(),
                "unclassified_upstream_field",
            ));
        }
    }

    let mut fields = Map::new();
    let mut members: Vec<Value> = Vec::new();
    let mut external_id = None;
    for field in collection.contract() {
        let raw = record
            .get(field.name)
            .ok_or_else(|| reject(collection, field.name, "missing_upstream_field"))?;
        match field.shape {
            FieldShape::Identity => {
                external_id = Some(
                    normalize_netbox_id(raw)
                        .map_err(|_| reject(collection, field.name, "identity"))?,
                );
            }
            FieldShape::Imported => {
                if raw.as_str().is_none() {
                    return Err(reject(collection, field.name, "text_type"));
                }
                fields.insert(field.name.to_string(), raw.clone());
            }
            FieldShape::CustomFields => {
                if !raw.is_object() {
                    return Err(reject(collection, field.name, "custom_fields_type"));
                }
                fields.insert(field.name.to_string(), raw.clone());
            }
            FieldShape::Relation => {
                let value = provider_relation(collection, field, raw)?;
                fields.insert(field.name.to_string(), value);
            }
            FieldShape::TagIds => {
                let value = tag_ids(collection, field.name, raw)?;
                fields.insert("tags".to_string(), value);
            }
            FieldShape::StatusChoice => {
                let value = local_status(collection, field.name, raw)?;
                fields.insert("status".to_string(), Value::String(value));
            }
            FieldShape::ProtocolChoice => {
                let value = local_protocol(collection, field.name, raw)?;
                fields.insert("protocol".to_string(), Value::String(value));
            }
            FieldShape::Observation => {
                if !raw.is_null() && raw.as_str().is_none() {
                    return Err(reject(collection, field.name, "observation_type"));
                }
                fields.insert(field.name.to_string(), raw.clone());
            }
            FieldShape::Derived => {}
            FieldShape::Slug => {
                if raw.as_str().is_none_or(str::is_empty) {
                    return Err(reject(collection, field.name, "slug_shape"));
                }
            }
            FieldShape::NotStored => require_empty(collection, field, raw)?,
            FieldShape::Ports => {
                let value = ports(collection, field.name, raw)?;
                fields.insert(field.name.to_string(), value);
            }
            FieldShape::Selectors => {
                let value = selectors(collection, field.name, raw)?;
                fields.insert(field.name.to_string(), value);
            }
            FieldShape::Members(object_type) => {
                collect_members(collection, field.name, raw, object_type, &mut members)?;
            }
        }
    }

    match collection {
        DataflowCollection::ObjectAliases => {
            // A group with no members is valid upstream; only the upper bound is
            // a projection limit.
            if members.len() > MAX_ADDRESS_GROUP_MEMBERS {
                return Err(reject(collection, "members", "member_count"));
            }
            fields.insert("members".to_string(), Value::Array(members));
        }
        DataflowCollection::Dataflows => reject_ports_on_icmp(collection, &fields)?,
        _ => {}
    }

    let external_id = external_id.ok_or_else(|| reject(collection, "id", "identity"))?;
    let identity = SourceIdentity::new(provider_id, collection.entity()?, external_id)?;
    Ok(SourceRecord::new(identity, fields))
}

fn reject(collection: DataflowCollection, field: &str, reason: &str) -> AppError {
    AppError::Validation(format!(
        "netbox_dataflow:{}.{field}:{reason}",
        collection.key()
    ))
}

fn provider_relation(
    collection: DataflowCollection,
    field: &FieldContract,
    raw: &Value,
) -> Result<Value, AppError> {
    if raw.is_null() {
        return if field.nullable {
            Ok(Value::Null)
        } else {
            Err(reject(collection, field.name, "null_relation"))
        };
    }
    relation_id(Some(raw), &format!("{}.{}", collection.key(), field.name))
}

fn tag_ids(collection: DataflowCollection, name: &str, raw: &Value) -> Result<Value, AppError> {
    let items = raw
        .as_array()
        .ok_or_else(|| reject(collection, name, "array_type"))?;
    let mut seen = BTreeSet::new();
    let mut tags = Vec::with_capacity(items.len());
    for item in items {
        let id = relation_id(Some(item), &format!("{}.{name}", collection.key()))?;
        if !seen.insert(id.to_string()) {
            return Err(reject(collection, name, "duplicate_identity"));
        }
        tags.push(json!({ "id": id }));
    }
    Ok(Value::Array(tags))
}

fn choice_value<'a>(
    collection: DataflowCollection,
    name: &str,
    raw: &'a Value,
    expected: &[&str],
) -> Result<&'a str, AppError> {
    let value = raw
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| reject(collection, name, "choice_shape"))?;
    if !expected.contains(&value) {
        return Err(reject(collection, name, "choice_value"));
    }
    Ok(value)
}

fn local_status(
    collection: DataflowCollection,
    name: &str,
    raw: &Value,
) -> Result<String, AppError> {
    let status = match choice_value(collection, name, raw, STATUS_VALUES)? {
        "enabled" => DataflowStatus::Active,
        "disabled" => DataflowStatus::Disabled,
        _ => return Err(reject(collection, name, "choice_value")),
    };
    Ok(status.as_str().to_string())
}

fn local_protocol(
    collection: DataflowCollection,
    name: &str,
    raw: &Value,
) -> Result<String, AppError> {
    let transport = match choice_value(collection, name, raw, PROTOCOL_VALUES)? {
        "any" => DataflowTransport::Any,
        "icmp" => DataflowTransport::Icmp,
        "icmpv6" => DataflowTransport::Icmpv6,
        "tcp" => DataflowTransport::Tcp,
        "udp" => DataflowTransport::Udp,
        "tcp+udp" => DataflowTransport::TcpUdp,
        "sctp" => DataflowTransport::Sctp,
        _ => return Err(reject(collection, name, "choice_value")),
    };
    Ok(transport.as_str().to_string())
}

fn require_empty(
    collection: DataflowCollection,
    field: &FieldContract,
    raw: &Value,
) -> Result<(), AppError> {
    let empty = raw.is_null()
        || raw.as_array().is_some_and(Vec::is_empty)
        || raw.as_object().is_some_and(Map::is_empty);
    if empty {
        Ok(())
    } else {
        Err(reject(collection, field.name, "not_representable"))
    }
}

fn ports(collection: DataflowCollection, name: &str, raw: &Value) -> Result<Value, AppError> {
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let items = raw
        .as_array()
        .ok_or_else(|| reject(collection, name, "array_type"))?;
    // NetBox serializes "no ports" as either null or an empty array; both mean
    // the same flow, so an empty list is imported as an empty list.
    if items.len() > MAX_PORTS {
        return Err(reject(collection, name, "port_count"));
    }
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let port = item
            .as_i64()
            .filter(|port| (1..=65535).contains(port))
            .ok_or_else(|| reject(collection, name, "port_value"))?;
        values.push(port);
    }
    values.sort_unstable();
    values.dedup();
    Ok(Value::Array(values.into_iter().map(Value::from).collect()))
}

fn selectors(collection: DataflowCollection, name: &str, raw: &Value) -> Result<Value, AppError> {
    let items = raw
        .as_array()
        .ok_or_else(|| reject(collection, name, "array_type"))?;
    // An empty endpoint side is a real flow with no aliases on that side; it is
    // imported as an empty selector list rather than failing the whole plugin.
    if items.len() > MAX_FLOW_ENDPOINTS {
        return Err(reject(collection, name, "endpoint_count"));
    }
    let mut selectors = Vec::with_capacity(items.len());
    for item in items {
        let brief = item
            .as_object()
            .ok_or_else(|| reject(collection, name, "endpoint_shape"))?;
        for key in brief.keys() {
            if !BRIEF_ALIAS_FIELDS.contains(&key.as_str()) {
                return Err(reject(collection, name, "endpoint_field"));
            }
        }
        let alias = relation_id(brief.get("id"), &format!("{}.{name}", collection.key()))?;
        selectors.push(json!({
            "object_type": EndpointType::AddressGroup.as_str(),
            "object_id": alias,
            "address_mode": DataflowAddressMode::Default.as_str(),
        }));
    }
    Ok(Value::Array(selectors))
}

fn collect_members(
    collection: DataflowCollection,
    name: &str,
    raw: &Value,
    object_type: EndpointType,
    members: &mut Vec<Value>,
) -> Result<(), AppError> {
    let items = raw
        .as_array()
        .ok_or_else(|| reject(collection, name, "array_type"))?;
    for item in items {
        let object_id = relation_id(Some(item), &format!("{}.{name}", collection.key()))?;
        members.push(json!({
            "object_type": object_type.as_str(),
            "object_id": object_id,
        }));
    }
    Ok(())
}

fn reject_ports_on_icmp(
    collection: DataflowCollection,
    fields: &Map<String, Value>,
) -> Result<(), AppError> {
    let icmp = fields
        .get("protocol")
        .and_then(Value::as_str)
        .is_some_and(|protocol| {
            protocol == DataflowTransport::Icmp.as_str()
                || protocol == DataflowTransport::Icmpv6.as_str()
        });
    if !icmp {
        return Ok(());
    }
    for name in ["source_ports", "destination_ports"] {
        if fields.get(name).is_some_and(|value| !value.is_null()) {
            return Err(reject(collection, name, "ports_on_icmp"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn choice(values: Value) -> Value {
        json!({
            "type": "object",
            "properties": {
                "value": {"type": "string", "enum": values},
                "label": {"type": "string"},
            },
        })
    }

    fn relation() -> Value {
        json!({"nullable": true, "allOf": [{"type": "object"}]})
    }

    fn array() -> Value {
        json!({"type": "array", "items": {"type": "object"}})
    }

    fn timestamp() -> Value {
        json!({"type": "string", "format": "date-time", "nullable": true, "readOnly": true})
    }

    /// Minimal redacted read serializers, taken from the actual NetBox 4.7
    /// `/api/plugins/data-flows/` responses.
    fn serializer(collection: DataflowCollection) -> Value {
        match collection {
            DataflowCollection::ApplicationRoles => json!({
                "id": {"type": "integer", "readOnly": true},
                "url": {"type": "string", "readOnly": true},
                "display": {"type": "string", "readOnly": true},
                "name": {"type": "string", "maxLength": 100},
                "slug": {"type": "string", "maxLength": 100},
                "description": {"type": "string", "maxLength": 200},
                "comments": {"type": "string"},
                "tags": array(),
                "custom_fields": {"type": "object"},
                "created": timestamp(),
                "last_updated": timestamp(),
                "owner": relation(),
                "application_count": {"type": "integer", "readOnly": true},
            }),
            DataflowCollection::Applications => json!({
                "id": {"type": "integer", "readOnly": true},
                "url": {"type": "string", "readOnly": true},
                "display": {"type": "string", "readOnly": true},
                "name": {"type": "string", "maxLength": 100},
                "role": relation(),
                "description": {"type": "string", "maxLength": 200},
                "comments": {"type": "string"},
                "tags": array(),
                "custom_fields": {"type": "object"},
                "created": timestamp(),
                "last_updated": timestamp(),
                "tenant": relation(),
                "dataflow_count": {"type": "integer", "readOnly": true},
                "owner": relation(),
            }),
            DataflowCollection::DataflowGroups => json!({
                "id": {"type": "integer", "readOnly": true},
                "url": {"type": "string", "readOnly": true},
                "display": {"type": "string", "readOnly": true},
                "name": {"type": "string", "maxLength": 100},
                "slug": {"type": "string", "maxLength": 100},
                "parent": relation(),
                "description": {"type": "string", "maxLength": 200},
                "comments": {"type": "string"},
                "status": choice(json!(["enabled", "disabled"])),
                "inherited_status": choice(json!(["enabled", "disabled", "disabled-parent"])),
                "tags": array(),
                "inherited_tags": {"type": "array", "readOnly": true},
                "created": timestamp(),
                "last_updated": timestamp(),
                "custom_fields": {"type": "object"},
                "tenant": relation(),
                "application": relation(),
                "_depth": {"type": "integer", "readOnly": true},
                "owner": relation(),
            }),
            DataflowCollection::Dataflows => json!({
                "id": {"type": "integer", "readOnly": true},
                "url": {"type": "string", "readOnly": true},
                "display": {"type": "string", "readOnly": true},
                "name": {"type": "string", "maxLength": 200},
                "application": relation(),
                "description": {"type": "string", "maxLength": 500},
                "comments": {"type": "string"},
                "status": choice(json!(["enabled", "disabled"])),
                "inherited_status": choice(json!(["enabled", "disabled", "disabled-parent"])),
                "group": relation(),
                "protocol": choice(json!([
                    "any", "icmp", "icmpv6", "tcp", "udp", "tcp+udp", "sctp"
                ])),
                "source_ports": {"type": "array", "nullable": true, "items": {"type": "integer"}},
                "destination_ports": {
                    "type": "array", "nullable": true, "items": {"type": "integer"}
                },
                "sources": array(),
                "destinations": array(),
                "tags": array(),
                "inherited_tags": {"type": "array", "readOnly": true},
                "created": timestamp(),
                "last_updated": timestamp(),
                "custom_fields": {"type": "object"},
                "tenant": relation(),
                "owner": relation(),
            }),
            DataflowCollection::ObjectAliases => json!({
                "id": {"type": "integer", "readOnly": true},
                "url": {"type": "string", "readOnly": true},
                "display": {"type": "string", "readOnly": true},
                "name": {"type": "string", "maxLength": 100},
                "description": {"type": "string", "maxLength": 200},
                "comments": {"type": "string"},
                "prefixes": array(),
                "ip_ranges": array(),
                "ip_addresses": array(),
                "tags": array(),
                "owner": relation(),
            }),
        }
    }

    /// Read serializer component the provider's `/api/schema/` publishes for the
    /// collection's detail response. NetBox names these after the model, not
    /// after the collection.
    fn read_component(collection: DataflowCollection) -> &'static str {
        match collection {
            DataflowCollection::ApplicationRoles => "ApplicationRole",
            DataflowCollection::Applications => "Application",
            DataflowCollection::DataflowGroups => "DataFlowGroup",
            DataflowCollection::Dataflows => "DataFlow",
            DataflowCollection::ObjectAliases => "ObjectAlias",
        }
    }

    /// A provider schema with the core `dcim.sites` collection and exactly the
    /// requested plugin collections.
    ///
    /// Each plugin collection mirrors the live `/api/schema/` shape: the list
    /// operation at `/api/plugins/data-flows/<collection>/`, a detail operation
    /// whose `200` response references the read serializer component, and that
    /// component as an object schema carrying `properties`. The registry derives
    /// the exposed read shape from the detail operation only, so the list
    /// response body stays out of the fixture.
    fn schema(collections: &[DataflowCollection]) -> Value {
        let site = json!({
            "get": {"responses": {"200": {"content": {"application/json": {
                "schema": {"$ref": "#/components/schemas/Site"}
            }}}}}
        });
        let mut paths = Map::new();
        paths.insert("/api/dcim/sites/".to_string(), site.clone());
        paths.insert("/api/dcim/sites/{id}/".to_string(), site);
        let mut components = Map::new();
        components.insert(
            "Site".to_string(),
            json!({"type": "object", "properties": {
                "id": {"type": "integer"}, "name": {"type": "string"}
            }}),
        );
        for collection in collections {
            let suffix = collection
                .key()
                .split('.')
                .nth(1)
                .expect("collection suffix");
            let operation = suffix.replace('-', "_");
            let component = read_component(*collection);
            paths.insert(
                format!("/api/plugins/data-flows/{suffix}/"),
                json!({"get": {
                    "operationId": format!("plugins_data_flows_{operation}_list")
                }}),
            );
            paths.insert(
                format!("/api/plugins/data-flows/{suffix}/{{id}}/"),
                json!({"get": {
                    "operationId": format!("plugins_data_flows_{operation}_retrieve"),
                    "responses": {"200": {"content": {"application/json": {
                        "schema": {"$ref": format!("#/components/schemas/{component}")}
                    }}}}
                }}),
            );
            components.insert(
                component.to_string(),
                json!({"type": "object", "properties": serializer(*collection)}),
            );
        }
        json!({"paths": paths, "components": {"schemas": components}})
    }

    fn registry(collections: &[DataflowCollection]) -> NetboxResourceRegistry {
        NetboxResourceRegistry::from_schema(schema(collections)).expect("provider registry")
    }

    fn canonical(collection: DataflowCollection) -> Value {
        match collection {
            DataflowCollection::ApplicationRoles => json!({
                "id": 4,
                "url": "http://127.0.0.1:8000/api/plugins/data-flows/application-roles/4/",
                "display": "Edge",
                "name": "Edge",
                "slug": "edge",
                "description": "",
                "comments": "",
                "tags": [],
                "custom_fields": {},
                "created": "2026-01-02T03:04:05Z",
                "last_updated": null,
                "owner": null,
                "application_count": 0,
            }),
            DataflowCollection::Applications => json!({
                "id": 5,
                "url": "http://127.0.0.1:8000/api/plugins/data-flows/applications/5/",
                "display": "Payments",
                "name": "Payments",
                "role": {"id": 4, "display": "Edge", "name": "Edge", "slug": "edge"},
                "description": "",
                "comments": "",
                "tags": [{"id": 2, "name": "prod"}],
                "custom_fields": {"tier": "gold"},
                "created": null,
                "last_updated": null,
                "tenant": null,
                "dataflow_count": 1,
                "owner": null,
            }),
            DataflowCollection::DataflowGroups => json!({
                "id": 6,
                "url": "http://127.0.0.1:8000/api/plugins/data-flows/dataflow-groups/6/",
                "display": "Frontend",
                "name": "Frontend",
                "slug": "frontend",
                "parent": null,
                "description": "",
                "comments": "",
                "status": {"value": "disabled", "label": "Disabled"},
                "inherited_status": {"value": "disabled", "label": "Disabled"},
                "tags": [],
                "inherited_tags": [],
                "created": null,
                "last_updated": null,
                "custom_fields": {},
                "tenant": null,
                "application": {"id": 5, "display": "Payments", "name": "Payments"},
                "_depth": 0,
                "owner": null,
            }),
            DataflowCollection::Dataflows => json!({
                "id": 7,
                "url": "http://127.0.0.1:8000/api/plugins/data-flows/dataflows/7/",
                "display": "Web",
                "name": "Web",
                "application": {"id": 5, "display": "Payments", "name": "Payments"},
                "description": "",
                "comments": "",
                "status": {"value": "enabled", "label": "Enabled"},
                "inherited_status": {"value": "enabled", "label": "Enabled"},
                "group": {"id": 6, "display": "Frontend", "name": "Frontend"},
                "protocol": {"value": "tcp+udp", "label": "TCP+UDP"},
                "source_ports": null,
                "destination_ports": [443, 80],
                "sources": [{
                    "id": 8,
                    "url": "http://127.0.0.1:8000/api/plugins/data-flows/objectalias/8/",
                    "display": "clients",
                    "name": "clients",
                    "description": "",
                }],
                "destinations": [{
                    "id": 9,
                    "url": "http://127.0.0.1:8000/api/plugins/data-flows/objectalias/9/",
                    "display": "servers",
                    "name": "servers",
                    "description": "",
                }],
                "tags": [{"id": 2, "name": "prod"}],
                "inherited_tags": [],
                "created": null,
                "last_updated": null,
                "custom_fields": {},
                "tenant": null,
                "owner": null,
            }),
            DataflowCollection::ObjectAliases => json!({
                "id": 8,
                "url": "http://127.0.0.1:8000/api/plugins/data-flows/objectalias/8/",
                "display": "clients",
                "name": "clients",
                "description": "",
                "comments": "",
                "prefixes": [{"id": 11, "prefix": "10.0.0.0/24"}],
                "ip_ranges": [],
                "ip_addresses": [{"id": 12, "address": "10.0.0.5/24"}],
                "tags": [],
                "owner": null,
            }),
        }
    }

    fn with(collection: DataflowCollection, overrides: Value) -> Value {
        let mut record = canonical(collection);
        let object = record.as_object_mut().expect("record object");
        for (key, value) in overrides.as_object().expect("overrides").into_iter() {
            object.insert(key.clone(), value.clone());
        }
        record
    }

    fn without(collection: DataflowCollection, key: &str) -> Value {
        let mut record = canonical(collection);
        record.as_object_mut().expect("record object").remove(key);
        record
    }

    fn fields(collection: DataflowCollection, record: Value) -> Map<String, Value> {
        let entity = collection.entity().expect("canonical entity");
        let records = project_page(Uuid::from_u128(7), &entity, &[record]).expect("projected");
        assert_eq!(records.len(), 1, "one record per upstream row");
        assert_eq!(records[0].identity.entity, entity);
        records[0].fields.clone()
    }

    fn rejection(collection: DataflowCollection, record: Value) -> String {
        let entity = collection.entity().expect("canonical entity");
        match project_page(Uuid::from_u128(7), &entity, &[record]) {
            Ok(records) => panic!("expected a rejection, got {} records", records.len()),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn absent_plugin_keeps_the_core_catalogue_and_claims_no_namespace() {
        let registry = registry(&[]);
        assert!(
            registry.has_resource("dcim.sites"),
            "core resources are unaffected"
        );
        let capability = resolve_capability(&registry).expect("resolved capability");
        assert_eq!(capability, DataflowCapability::Absent);
        assert_eq!(capability.as_str(), "absent");
        assert!(capability.pull_entities().expect("entities").is_empty());
        assert!(!capability.contributes_absence_evidence());
        assert!(!registry.has_resource("data-flows.dataflows"));
    }

    #[test]
    fn complete_plugin_is_present_and_projects_typed_identities_and_relations() {
        let registry = registry(&DataflowCollection::ALL);
        let capability = resolve_capability(&registry).expect("resolved capability");
        assert_eq!(capability, DataflowCapability::Present);
        assert!(capability.contributes_absence_evidence());
        let entities = capability.pull_entities().expect("entities");
        let keys: Vec<&str> = entities.iter().map(SourceEntity::as_str).collect();
        assert_eq!(
            keys,
            [
                "data-flows.application-roles",
                "data-flows.applications",
                "data-flows.dataflow-groups",
                "data-flows.dataflows",
                "data-flows.objectalias",
            ]
        );

        let role = fields(
            DataflowCollection::ApplicationRoles,
            canonical(DataflowCollection::ApplicationRoles),
        );
        assert_eq!(role.get("name"), Some(&json!("Edge")));
        assert_eq!(role.get("created"), Some(&json!("2026-01-02T03:04:05Z")));
        assert!(!role.contains_key("slug"), "slug has no native column");
        assert!(!role.contains_key("application_count"));

        let application = fields(
            DataflowCollection::Applications,
            canonical(DataflowCollection::Applications),
        );
        assert_eq!(application.get("role"), Some(&json!(4)));
        assert_eq!(application.get("tags"), Some(&json!([{"id": 2}])));
        assert_eq!(
            application.get("custom_fields"),
            Some(&json!({"tier": "gold"}))
        );

        let group = fields(
            DataflowCollection::DataflowGroups,
            canonical(DataflowCollection::DataflowGroups),
        );
        assert_eq!(group.get("status"), Some(&json!("disabled")));
        assert_eq!(group.get("parent"), Some(&Value::Null));
        assert_eq!(group.get("application"), Some(&json!(5)));
        assert!(!group.contains_key("_depth"), "tree depth is local state");
        assert!(!group.contains_key("inherited_status"));

        let flow = fields(
            DataflowCollection::Dataflows,
            canonical(DataflowCollection::Dataflows),
        );
        assert_eq!(flow.get("status"), Some(&json!("active")));
        assert_eq!(flow.get("protocol"), Some(&json!("tcp_udp")));
        assert_eq!(flow.get("destination_ports"), Some(&json!([80, 443])));
        assert_eq!(
            flow.get("sources"),
            Some(&json!([{
                "object_type": "inventory.addressgroup",
                "object_id": 8,
                "address_mode": "default",
            }]))
        );
        assert_eq!(flow.get("group"), Some(&json!(6)));

        let alias = fields(
            DataflowCollection::ObjectAliases,
            canonical(DataflowCollection::ObjectAliases),
        );
        assert_eq!(
            alias.get("members"),
            Some(&json!([
                {"object_type": "ipam.prefix", "object_id": 11},
                {"object_type": "ipam.ipaddress", "object_id": 12},
            ]))
        );
        assert_eq!(alias.get("description"), Some(&json!("")));

        let entity = DataflowCollection::ObjectAliases.entity().expect("entity");
        let records = project_page(Uuid::from_u128(7), &entity, &[])
            .expect("an empty page of an approved entity is empty");
        assert!(records.is_empty());
    }

    #[test]
    fn disappearing_or_unreadable_plugin_never_becomes_absence_evidence() {
        // The plugin was there and is gone: the namespace contributes nothing.
        let withdrawn = resolve_capability(&registry(&[])).expect("resolved capability");
        assert_eq!(withdrawn, DataflowCapability::Absent);
        assert!(withdrawn.pull_entities().expect("entities").is_empty());
        assert!(!withdrawn.contributes_absence_evidence());

        // A refused schema read is not an empty traversal.
        let forbidden = capability_from_discovery_error(&AppError::Forbidden("token".into()));
        assert_eq!(forbidden, DataflowCapability::Forbidden);
        assert!(!forbidden.is_present());
        assert!(forbidden.pull_entities().expect("entities").is_empty());
        assert!(!forbidden.contributes_absence_evidence());

        // An untyped client failure is not silently promoted to `Absent`.
        let failed = capability_from_discovery_error(&AppError::External(
            "NetBox returned status 500".into(),
        ));
        assert_eq!(failed, DataflowCapability::Failed);
        assert!(!failed.contributes_absence_evidence());

        // Three of five collections is an unsupported shape, not a smaller universe.
        let partial = resolve_capability(&registry(&[
            DataflowCollection::ApplicationRoles,
            DataflowCollection::Applications,
            DataflowCollection::Dataflows,
        ]))
        .expect_err("a partial namespace must fail");
        let message = partial.to_string();
        assert!(message.contains("capability.partial"), "{message}");
        assert!(message.contains("data-flows.dataflow-groups"), "{message}");
        assert!(message.contains("data-flows.objectalias"), "{message}");

        // Zero rows read against a declared count is a pagination contradiction.
        assert!(
            super::super::netbox_source_projection::page_cursor(0, 0, 5).is_err(),
            "a partial read must fail instead of importing an empty universe"
        );
        assert_eq!(
            super::super::netbox_source_projection::page_cursor(0, 0, 0).expect("exhausted"),
            None
        );
    }

    #[test]
    fn unsupported_shapes_fail_closed_with_the_exact_field() {
        let role_tags = rejection(
            DataflowCollection::ApplicationRoles,
            with(
                DataflowCollection::ApplicationRoles,
                json!({"tags": [{"id": 2, "name": "prod"}]}),
            ),
        );
        assert!(
            role_tags.contains("data-flows.application-roles.tags:not_representable"),
            "{role_tags}"
        );

        let role_fields = rejection(
            DataflowCollection::ApplicationRoles,
            with(
                DataflowCollection::ApplicationRoles,
                json!({"custom_fields": {"tier": "gold"}}),
            ),
        );
        assert!(
            role_fields.contains("data-flows.application-roles.custom_fields:not_representable"),
            "{role_fields}"
        );

        let unknown = rejection(
            DataflowCollection::Dataflows,
            with(DataflowCollection::Dataflows, json!({"widgets": "x"})),
        );
        assert!(
            unknown.contains("data-flows.dataflows.widgets:unclassified_upstream_field"),
            "{unknown}"
        );

        let missing = rejection(
            DataflowCollection::Dataflows,
            without(DataflowCollection::Dataflows, "sources"),
        );
        assert!(
            missing.contains("data-flows.dataflows.sources:missing_upstream_field"),
            "{missing}"
        );

        let empty_side = fields(
            DataflowCollection::Dataflows,
            with(DataflowCollection::Dataflows, json!({"sources": []})),
        );
        assert_eq!(empty_side.get("sources"), Some(&json!([])));

        let zero_port = rejection(
            DataflowCollection::Dataflows,
            with(
                DataflowCollection::Dataflows,
                json!({"destination_ports": [0, 443]}),
            ),
        );
        assert!(
            zero_port.contains("data-flows.dataflows.destination_ports:port_value"),
            "{zero_port}"
        );

        let mut icmp = canonical(DataflowCollection::Dataflows);
        icmp["protocol"] = json!({"value": "icmp", "label": "ICMPv4"});
        icmp["source_ports"] = json!([80]);
        let icmp_ports = rejection(DataflowCollection::Dataflows, icmp);
        assert!(
            icmp_ports.contains("data-flows.dataflows.source_ports:ports_on_icmp"),
            "{icmp_ports}"
        );

        let draft = rejection(
            DataflowCollection::Dataflows,
            with(
                DataflowCollection::Dataflows,
                json!({"status": {"value": "draft", "label": "Draft"}}),
            ),
        );
        assert!(
            draft.contains("data-flows.dataflows.status:choice_value"),
            "{draft}"
        );

        let empty_alias = fields(
            DataflowCollection::ObjectAliases,
            with(
                DataflowCollection::ObjectAliases,
                json!({"prefixes": [], "ip_ranges": [], "ip_addresses": []}),
            ),
        );
        assert_eq!(empty_alias.get("members"), Some(&json!([])));

        let mut too_many = canonical(DataflowCollection::ObjectAliases);
        too_many["prefixes"] = Value::Array(
            (1..=(MAX_ADDRESS_GROUP_MEMBERS + 1))
                .map(|id| json!({"id": id, "prefix": "10.0.0.0/24"}))
                .collect(),
        );
        let oversized = rejection(DataflowCollection::ObjectAliases, too_many);
        assert!(
            oversized.contains("data-flows.objectalias.members:member_count"),
            "{oversized}"
        );
    }

    #[test]
    fn an_empty_port_list_is_imported_as_an_empty_list() {
        let mut flow = canonical(DataflowCollection::Dataflows);
        flow["source_ports"] = json!([]);
        let fields = fields(DataflowCollection::Dataflows, flow);
        assert_eq!(fields.get("source_ports"), Some(&json!([])));
    }

    #[test]
    fn a_drifted_plugin_serializer_is_not_present() {
        let mut drifted = schema(&DataflowCollection::ALL);
        let flow = read_component(DataflowCollection::Dataflows);
        let protocol = &mut drifted["components"]["schemas"][flow]["properties"]["protocol"]["properties"]
            ["value"]["enum"];
        let values = protocol.as_array_mut().expect("protocol choices");
        values.retain(|value| value.as_str() != Some("sctp"));
        let registry = NetboxResourceRegistry::from_schema(drifted).expect("provider registry");
        let error = resolve_capability(&registry).expect_err("a drifted vocabulary must fail");
        assert!(
            error
                .to_string()
                .contains("data-flows.dataflows.protocol:choice_drift"),
            "{error}"
        );

        let mut extended = schema(&DataflowCollection::ALL);
        let alias = read_component(DataflowCollection::ObjectAliases);
        extended["components"]["schemas"][alias]["properties"]["widgets"] =
            json!({"type": "string"});
        let registry = NetboxResourceRegistry::from_schema(extended).expect("provider registry");
        let error = resolve_capability(&registry).expect_err("an unknown field must fail");
        assert!(
            error
                .to_string()
                .contains("data-flows.objectalias.widgets:unclassified_upstream_field"),
            "{error}"
        );
    }
}
