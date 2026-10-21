//! Closed, whitelisted schema for the local Piping inventory target (PIP-86).
//!
//! This module is the ONLY place that knows which target entity, table, column,
//! junction and PostgreSQL type a synchronised source field may reach. Every
//! physical identifier in [`crate::contexts::sync::infrastructure::persistence::local_inventory_target`]
//! comes from a [`FieldSpec`] or [`EntityMeta`] here — never from a caller, a
//! mapping document or a provider payload. The table/column set was generated
//! from the migrated PostgreSQL schema (`0001_init.sql` plus the forward
//! migrations `0008`/`0009`) and verified field by field against the source
//! census, so an unknown source entity or field is rejected instead of being
//! coerced or dropped.
//!
//! Two invariants hold:
//!
//! * [`InventoryEntity`] is closed over the twenty-six supported source
//!   resources; [`InventoryEntity::from_source_key`] is the only constructor
//!   from an untrusted string.
//! * [`InventoryField`] is opaque and can only be obtained from an entity's
//!   catalogue ([`InventoryEntity::field`]) — there is no public constructor, so
//!   a caller cannot name a column that the catalogue did not approve.
//!
//! `super::inventory_sync` owns the port traits; this module owns the concrete
//! closed entity/field catalogue they are implemented against.

mod catalog_fields;

use std::fmt;

/// A checked PostgreSQL type the target may bind to. Mirrors the allowlist of
/// column types in the migrated schema; anything outside it is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PgType {
    Text,
    TextArray,
    IntArray,
    UuidArray,
    BigIntArray,
    Bool,
    Integer,
    BigInt,
    SmallInt,
    Numeric,
    Uuid,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Jsonb,
    Json,
    Inet,
    Cidr,
    MacAddr,
    Double,
    Real,
}

/// How a semantically absent (null) source value is encoded for a column.
///
/// Every `NOT NULL`-without-default field names its encoding explicitly in the
/// catalogue: a null source must never silently become `''`/`0`, and a required
/// value must fail instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Absent {
    /// Leave the column out: the DB default applies on insert and the current
    /// value is preserved on update.
    Preserve,
    /// Write the native empty string (optional text with no DB default).
    EmptyText,
    /// Write the native empty array (optional array with no DB default).
    EmptyArray,
    /// Write the native zero (optional numeric with no DB default).
    Zero,
    /// A required value must be present; a null source fails the pull.
    Reject,
}

impl PgType {
    /// Whether the column is a PostgreSQL array.
    #[must_use]
    pub fn is_array(&self) -> bool {
        matches!(
            self,
            Self::TextArray | Self::IntArray | Self::UuidArray | Self::BigIntArray
        )
    }

    /// Whether the type can hold a checked numeric value without loss of the
    /// value's integer part.
    #[must_use]
    pub fn accepts_integer(&self) -> bool {
        matches!(
            self,
            Self::Integer
                | Self::BigInt
                | Self::SmallInt
                | Self::Numeric
                | Self::Double
                | Self::Real
        )
    }
}

/// The target identity of one source resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InventoryEntity {
    DeviceRole,
    DeviceType,
    Device,
    Interface,
    InventoryItem,
    MacAddress,
    Manufacturer,
    ModuleTypeProfile,
    Platform,
    Region,
    SiteGroup,
    Site,
    Tag,
    CustomField,
    IpAddress,
    Prefix,
    Service,
    Vrf,
    Tenant,
    ClusterGroup,
    ClusterType,
    Cluster,
    VmInterface,
    VirtualDisk,
    VirtualMachine,
    /// `extras.tagged-objects`: a tag membership, not an entity row. It is
    /// represented natively on the owning entities' `tags text[]` columns and
    /// never materialises a row of its own.
    TaggedObject,
}

/// How a mapped source field reaches the target row.
#[derive(Debug, Clone, Copy)]
pub enum FieldKind {
    /// A scalar column.
    Scalar,
    /// A foreign key column. `target` is `None` when the referenced resource is
    /// outside the supported set (e.g. a config template): the value is bound
    /// only through an existing provider-scoped binding, never by remote id.
    ForeignKey { target: Option<InventoryEntity> },
    /// A polymorphic generic-foreign-key pair: a type column (a canonical
    /// `inventory_content_type` id) plus an id column.
    GenericReference {
        type_column: &'static str,
        /// Source field carrying the `app.model` type token of the reference.
        source_type_field: &'static str,
        allowed: &'static [InventoryEntity],
    },
    /// A many-to-many junction table.
    Junction {
        table: &'static str,
        local_column: &'static str,
        remote_column: &'static str,
        target: Option<InventoryEntity>,
    },
    /// A junction whose remote column holds a canonical `inventory_content_type`
    /// id: values are `app_label.model` tokens resolved against the seeded
    /// content types, never provider ids.
    ContentTypeJunction {
        table: &'static str,
        local_column: &'static str,
        remote_column: &'static str,
    },
    /// A value junction: rows carry scalar value columns instead of a foreign
    /// key. Used for `ipam_service_port_mapping`, whose values are a protocol
    /// and a port parsed from the source mapping.
    ValueJunction {
        table: &'static str,
        local_column: &'static str,
        protocol_column: &'static str,
        port_column: &'static str,
        ordinal_column: &'static str,
    },
    /// Native `text[]` tag membership on the owning table.
    Tags,
}

/// Static metadata for one approved field on one entity.
#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    /// Owning entity.
    pub entity: InventoryEntity,
    /// Canonical source/field name (the key in `SourceRecord::fields`).
    pub name: &'static str,
    /// Physical column, or junction table for a [`FieldKind::Junction`].
    pub column: &'static str,
    /// Checked column type on the owning table.
    pub sql_type: PgType,
    /// Whether the column is nullable.
    pub nullable: bool,
    /// The column's SQL default expression, when it has one.
    pub default: Option<&'static str>,
    /// How the field is written.
    pub kind: FieldKind,
    /// Whether the source value is a NetBox choice object (`{value,label}`)
    /// that must be normalised to its `.value` string.
    pub choice_normalized: bool,
    /// How a null source value is encoded for this column.
    pub absent: Absent,
}

impl FieldSpec {
    /// Whether the field writes through a relation rather than a plain column.
    #[must_use]
    pub fn is_relation(&self) -> bool {
        !matches!(self.kind, FieldKind::Scalar | FieldKind::Tags)
    }
}

/// Static metadata for one supported entity.
#[derive(Debug, Clone, Copy)]
pub struct EntityMeta {
    /// The entity.
    pub entity: InventoryEntity,
    /// Canonical source key, e.g. `dcim.devices`.
    pub source_key: &'static str,
    /// Physical table.
    pub table: &'static str,
    /// Canonical `(app_label, model)` for the generic-FK content type and the
    /// `piping.inventory.<model>` binding key.
    pub content_type: (&'static str, &'static str),
    /// Whether the table carries a native `tags text[]` column.
    pub has_tags: bool,
    /// Whether the table carries a `custom_field_data jsonb` column.
    pub has_custom_field_data: bool,
    /// The approved fields.
    pub fields: &'static [FieldSpec],
}

/// Opaque handle to one approved field.
///
/// The inner reference is private and only [`InventoryEntity::field`] constructs
/// a value, so an [`InventoryField`] provably belongs to the closed catalogue.
#[derive(Clone, Copy)]
pub struct InventoryField(&'static FieldSpec);

impl InventoryField {
    /// Field metadata.
    #[must_use]
    pub fn spec(self) -> &'static FieldSpec {
        self.0
    }

    /// Owning entity.
    #[must_use]
    pub fn entity(self) -> InventoryEntity {
        self.0.entity
    }

    /// Canonical field name.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// Physical column.
    #[must_use]
    pub fn column(self) -> &'static str {
        self.0.column
    }

    /// Checked column type.
    #[must_use]
    pub fn sql_type(self) -> PgType {
        self.0.sql_type
    }

    /// Whether the column is nullable.
    #[must_use]
    pub fn nullable(self) -> bool {
        self.0.nullable
    }

    /// Write kind.
    #[must_use]
    pub fn kind(self) -> FieldKind {
        self.0.kind
    }

    /// Whether the source value is a choice object to normalise.
    #[must_use]
    pub fn choice_normalized(self) -> bool {
        self.0.choice_normalized
    }

    /// How a null source value is encoded for this column.
    #[must_use]
    pub fn absent(self) -> Absent {
        self.0.absent
    }
}

impl PartialEq for InventoryField {
    fn eq(&self, other: &Self) -> bool {
        self.0.entity == other.0.entity && self.0.name == other.0.name
    }
}

impl Eq for InventoryField {}

impl std::hash::Hash for InventoryField {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.entity.hash(state);
        self.0.name.hash(state);
    }
}

impl PartialOrd for InventoryField {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InventoryField {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.0.entity, self.0.name).cmp(&(other.0.entity, other.0.name))
    }
}

impl fmt::Debug for InventoryField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}::{}", self.0.entity.source_key(), self.0.name)
    }
}

/// The closed entity catalogue. Order is stable but not semantic.
pub const ENTITIES: &[EntityMeta] = &[
    meta(
        InventoryEntity::DeviceRole,
        "dcim.device-roles",
        "dcim_devicerole",
        ("dcim", "devicerole"),
        (true, true),
        catalog_fields::DEVICE_ROLE_FIELDS,
    ),
    meta(
        InventoryEntity::DeviceType,
        "dcim.device-types",
        "dcim_devicetype",
        ("dcim", "devicetype"),
        (true, true),
        catalog_fields::DEVICE_TYPE_FIELDS,
    ),
    meta(
        InventoryEntity::Device,
        "dcim.devices",
        "dcim_device",
        ("dcim", "device"),
        (true, true),
        catalog_fields::DEVICE_FIELDS,
    ),
    meta(
        InventoryEntity::Interface,
        "dcim.interfaces",
        "dcim_interface",
        ("dcim", "interface"),
        (true, true),
        catalog_fields::INTERFACE_FIELDS,
    ),
    meta(
        InventoryEntity::InventoryItem,
        "dcim.inventory-items",
        "dcim_inventoryitem",
        ("dcim", "inventoryitem"),
        (true, true),
        catalog_fields::INVENTORY_ITEM_FIELDS,
    ),
    meta(
        InventoryEntity::MacAddress,
        "dcim.mac-addresses",
        "dcim_macaddress",
        ("dcim", "macaddress"),
        (true, true),
        catalog_fields::MAC_ADDRESS_FIELDS,
    ),
    meta(
        InventoryEntity::Manufacturer,
        "dcim.manufacturers",
        "dcim_manufacturer",
        ("dcim", "manufacturer"),
        (true, true),
        catalog_fields::MANUFACTURER_FIELDS,
    ),
    meta(
        InventoryEntity::ModuleTypeProfile,
        "dcim.module-type-profiles",
        "dcim_moduletypeprofile",
        ("dcim", "moduletypeprofile"),
        (false, true),
        catalog_fields::MODULE_TYPE_PROFILE_FIELDS,
    ),
    meta(
        InventoryEntity::Platform,
        "dcim.platforms",
        "dcim_platform",
        ("dcim", "platform"),
        (true, true),
        catalog_fields::PLATFORM_FIELDS,
    ),
    meta(
        InventoryEntity::Region,
        "dcim.regions",
        "dcim_region",
        ("dcim", "region"),
        (true, true),
        catalog_fields::REGION_FIELDS,
    ),
    meta(
        InventoryEntity::SiteGroup,
        "dcim.site-groups",
        "dcim_sitegroup",
        ("dcim", "sitegroup"),
        (true, true),
        catalog_fields::SITE_GROUP_FIELDS,
    ),
    meta(
        InventoryEntity::Site,
        "dcim.sites",
        "dcim_site",
        ("dcim", "site"),
        (true, true),
        catalog_fields::SITE_FIELDS,
    ),
    meta(
        InventoryEntity::Tag,
        "extras.tags",
        "inventory_tag",
        ("extras", "tag"),
        (false, false),
        catalog_fields::TAG_FIELDS,
    ),
    meta(
        InventoryEntity::CustomField,
        "extras.custom-fields",
        "custom_field",
        ("extras", "customfield"),
        (false, false),
        catalog_fields::CUSTOM_FIELD_FIELDS,
    ),
    meta(
        InventoryEntity::IpAddress,
        "ipam.ip-addresses",
        "ipam_ipaddress",
        ("ipam", "ipaddress"),
        (true, true),
        catalog_fields::IP_ADDRESS_FIELDS,
    ),
    meta(
        InventoryEntity::Prefix,
        "ipam.prefixes",
        "ipam_prefix",
        ("ipam", "prefix"),
        (true, true),
        catalog_fields::PREFIX_FIELDS,
    ),
    meta(
        InventoryEntity::Service,
        "ipam.services",
        "ipam_service",
        ("ipam", "service"),
        (true, true),
        catalog_fields::SERVICE_FIELDS,
    ),
    meta(
        InventoryEntity::Vrf,
        "ipam.vrfs",
        "ipam_vrf",
        ("ipam", "vrf"),
        (true, true),
        catalog_fields::VRF_FIELDS,
    ),
    meta(
        InventoryEntity::Tenant,
        "tenancy.tenants",
        "tenancy_tenant",
        ("tenancy", "tenant"),
        (true, true),
        catalog_fields::TENANT_FIELDS,
    ),
    meta(
        InventoryEntity::ClusterGroup,
        "virtualization.cluster-groups",
        "virtualization_clustergroup",
        ("virtualization", "clustergroup"),
        (true, true),
        catalog_fields::CLUSTER_GROUP_FIELDS,
    ),
    meta(
        InventoryEntity::ClusterType,
        "virtualization.cluster-types",
        "virtualization_clustertype",
        ("virtualization", "clustertype"),
        (true, true),
        catalog_fields::CLUSTER_TYPE_FIELDS,
    ),
    meta(
        InventoryEntity::Cluster,
        "virtualization.clusters",
        "virtualization_cluster",
        ("virtualization", "cluster"),
        (true, true),
        catalog_fields::CLUSTER_FIELDS,
    ),
    meta(
        InventoryEntity::VmInterface,
        "virtualization.interfaces",
        "virtualization_vminterface",
        ("virtualization", "vminterface"),
        (true, true),
        catalog_fields::VM_INTERFACE_FIELDS,
    ),
    meta(
        InventoryEntity::VirtualDisk,
        "virtualization.virtual-disks",
        "virtualization_virtualdisk",
        ("virtualization", "virtualdisk"),
        (true, true),
        catalog_fields::VIRTUAL_DISK_FIELDS,
    ),
    meta(
        InventoryEntity::VirtualMachine,
        "virtualization.virtual-machines",
        "virtualization_virtualmachine",
        ("virtualization", "virtualmachine"),
        (true, true),
        catalog_fields::VIRTUAL_MACHINE_FIELDS,
    ),
];

/// The membership-only resource, kept out of [`ENTITIES`] so a row-producing
/// lookup never returns it.
pub const TAGGED_OBJECT_SOURCE_KEY: &str = "extras.tagged-objects";

/// Constructor used only by the constant catalogue above.
///
/// The two optional native columns are grouped as
/// `(has_tags, has_custom_field_data)` so the constructor stays within
/// clippy's argument limit.
const fn meta(
    entity: InventoryEntity,
    source_key: &'static str,
    table: &'static str,
    content_type: (&'static str, &'static str),
    native_columns: (bool, bool),
    fields: &'static [FieldSpec],
) -> EntityMeta {
    let (has_tags, has_custom_field_data) = native_columns;
    EntityMeta {
        entity,
        source_key,
        table,
        content_type,
        has_tags,
        has_custom_field_data,
        fields,
    }
}

impl InventoryEntity {
    /// Every row-producing entity in the closed catalogue.
    #[must_use = "the iterator performs no work until it is consumed"]
    pub fn all() -> impl Iterator<Item = InventoryEntity> {
        ENTITIES.iter().map(|meta| meta.entity)
    }

    /// Metadata for this entity; `None` for the membership-only resource.
    ///
    /// The array index is the enum discriminant: `ENTITIES` is declared in the
    /// same order as [`InventoryEntity`], which the `meta_index_matches_enum`
    /// test pins.
    #[must_use]
    pub fn meta(self) -> Option<&'static EntityMeta> {
        if self == Self::TaggedObject {
            return None;
        }
        ENTITIES.get(self as usize)
    }

    /// Resolve an untrusted `app.resource` source key. Unknown, mis-cased or
    /// SQL-like keys return `None`; there is no fallback.
    #[must_use]
    pub fn from_source_key(key: &str) -> Option<Self> {
        if key == TAGGED_OBJECT_SOURCE_KEY {
            return Some(Self::TaggedObject);
        }
        ENTITIES
            .iter()
            .find(|meta| meta.source_key == key)
            .map(|meta| meta.entity)
    }

    /// Canonical source key, e.g. `dcim.devices`.
    #[must_use]
    pub fn source_key(self) -> &'static str {
        match self.meta() {
            Some(meta) => meta.source_key,
            None => TAGGED_OBJECT_SOURCE_KEY,
        }
    }

    /// Canonical binding target key stored in
    /// `sync_inventory_binding.target_entity`, e.g. `piping.inventory.device`.
    #[must_use]
    pub fn target_key(self) -> String {
        match self.content_type() {
            Some((_, model)) => format!("piping.inventory.{model}"),
            None => "piping.inventory.tagged_object".to_string(),
        }
    }

    /// Physical table, when this entity owns one.
    #[must_use]
    pub fn table(self) -> Option<&'static str> {
        self.meta().map(|meta| meta.table)
    }

    /// Canonical `(app_label, model)` for the generic-FK content type.
    #[must_use]
    pub fn content_type(self) -> Option<(&'static str, &'static str)> {
        self.meta().map(|meta| meta.content_type)
    }

    /// Resolve an `(app_label, model)` content type to the entity that owns it.
    #[must_use]
    pub fn from_content_type(app_label: &str, model: &str) -> Option<Self> {
        ENTITIES
            .iter()
            .find(|meta| meta.content_type == (app_label, model))
            .map(|meta| meta.entity)
    }

    /// Resolve an untrusted `app_label.model` content-type token.
    #[must_use]
    pub fn from_content_type_token(token: &str) -> Option<Self> {
        let (app_label, model) = token.split_once('.')?;
        if model.contains('.') {
            return None;
        }
        Self::from_content_type(app_label, model)
    }

    /// Whether the table carries a native `tags text[]` column.
    #[must_use]
    pub fn has_tags(self) -> bool {
        self.meta().is_some_and(|meta| meta.has_tags)
    }

    /// Whether the table carries `custom_field_data jsonb`.
    #[must_use]
    pub fn has_custom_field_data(self) -> bool {
        self.meta().is_some_and(|meta| meta.has_custom_field_data)
    }

    /// Whether the resource is a membership represented on other rows.
    #[must_use]
    pub fn is_membership(self) -> bool {
        self == Self::TaggedObject
    }

    /// The approved fields on this entity.
    #[must_use]
    pub fn fields(self) -> &'static [FieldSpec] {
        match self.meta() {
            Some(meta) => meta.fields,
            None => &[],
        }
    }

    /// Resolve an untrusted field name against the catalogue. Returns `None` for
    /// any name the catalogue did not approve, including SQL fragments.
    #[must_use]
    pub fn field(self, name: &str) -> Option<InventoryField> {
        self.fields()
            .iter()
            .find(|spec| spec.name == name)
            .map(InventoryField)
    }

    /// Iterate the approved fields of this entity.
    #[must_use = "the iterator performs no work until it is consumed"]
    pub fn field_iter(self) -> impl Iterator<Item = InventoryField> {
        self.fields().iter().map(InventoryField)
    }

    /// Every source field deliberately not persisted, with its reason.
    #[must_use]
    pub fn excluded_fields(self) -> &'static [(&'static str, &'static str)] {
        if self == Self::TaggedObject {
            return &[
                ("id", "membership external identity"),
                ("tag", "consumed by native tag membership validation"),
                (
                    "object_type",
                    "consumed by native tag membership validation",
                ),
                ("object_id", "consumed by native tag membership validation"),
                (
                    "object",
                    "derived nested object representation; identity comes from object_id",
                ),
                ("display", "read-only derived label"),
                ("url", "read-only source API link"),
            ];
        }
        catalog_fields::EXCLUDED_BY_INDEX
            .get(self as usize)
            .copied()
            .unwrap_or(&[])
    }

    /// Reason a populated-or-derived source field is deliberately not
    /// persisted. `None` means the field is approved or unknown; an unknown
    /// field (neither approved nor excluded) must block a pull rather than be
    /// dropped.
    #[must_use]
    pub fn excluded_source_field(self, name: &str) -> Option<&'static str> {
        self.excluded_fields()
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, reason)| *reason)
    }

    /// NOT NULL columns without a database default that no mapped field writes.
    /// Each entry is `(column, kind, argument)`; `kind` is `"literal"` (a SQL
    /// expression to bind) or `"copy"` (the value of the approved field
    /// `argument`). The SQL target must synthesise these on insert.
    #[must_use]
    pub fn internal_columns(self) -> &'static [(&'static str, &'static str, &'static str)] {
        if self == Self::TaggedObject {
            return &[];
        }
        catalog_fields::INTERNAL_COLUMNS_BY_INDEX
            .get(self as usize)
            .copied()
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact source resources of the PIP-86 census. This is the consumer
    /// contract: the catalogue must cover every one and add none.
    const CENSUS_SOURCE_KEYS: &[&str] = &[
        "dcim.device-roles",
        "dcim.device-types",
        "dcim.devices",
        "dcim.interfaces",
        "dcim.inventory-items",
        "dcim.mac-addresses",
        "dcim.manufacturers",
        "dcim.module-type-profiles",
        "dcim.platforms",
        "dcim.regions",
        "dcim.site-groups",
        "dcim.sites",
        "extras.custom-fields",
        "extras.tagged-objects",
        "extras.tags",
        "ipam.ip-addresses",
        "ipam.prefixes",
        "ipam.services",
        "ipam.vrfs",
        "tenancy.tenants",
        "virtualization.cluster-groups",
        "virtualization.cluster-types",
        "virtualization.clusters",
        "virtualization.interfaces",
        "virtualization.virtual-disks",
        "virtualization.virtual-machines",
    ];

    #[test]
    fn catalogue_covers_exactly_the_census_resources() {
        let mut resolved: Vec<&str> = CENSUS_SOURCE_KEYS
            .iter()
            .map(|key| {
                InventoryEntity::from_source_key(key)
                    .unwrap_or_else(|| panic!("unmapped census resource {key}"))
                    .source_key()
            })
            .collect();
        resolved.sort_unstable();
        let mut expected = CENSUS_SOURCE_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(resolved, expected);
        assert_eq!(CENSUS_SOURCE_KEYS.len(), 26);
        // Twenty-five row entities plus the membership-only tagged-objects.
        assert_eq!(ENTITIES.len(), 25);
    }

    #[test]
    fn source_key_lookup_rejects_unknown_and_sql_like_keys() {
        assert!(InventoryEntity::from_source_key("dcim.devices; DROP TABLE dcim_device").is_none());
        assert!(InventoryEntity::from_source_key("dcim.devices ").is_none());
        assert!(InventoryEntity::from_source_key("DCIM.DEVICES").is_none());
        assert!(InventoryEntity::from_source_key("../etc/passwd").is_none());
        assert!(InventoryEntity::from_source_key("").is_none());
        assert_eq!(
            InventoryEntity::from_source_key("dcim.devices"),
            Some(InventoryEntity::Device)
        );
        assert_eq!(
            InventoryEntity::from_source_key("extras.tagged-objects"),
            Some(InventoryEntity::TaggedObject)
        );
        assert!(
            InventoryEntity::from_source_key("extras.tagged-objects")
                .is_some_and(InventoryEntity::is_membership)
        );
    }

    #[test]
    fn field_lookup_rejects_unknown_and_sql_like_names() {
        let device = InventoryEntity::Device;
        assert!(device.field("name; DROP TABLE dcim_device").is_none());
        assert!(device.field("name\" OR 1=1").is_none());
        assert!(device.field("not_a_field").is_none());
        assert!(device.field("").is_none());
        let name = device.field("name").expect("name is approved");
        assert_eq!(name.column(), "name");
        assert_eq!(name.entity(), InventoryEntity::Device);
        assert_eq!(name.sql_type(), PgType::Text);
        assert!(name.nullable(), "native devices may be unnamed");
        assert!(
            !InventoryEntity::Manufacturer
                .field("name")
                .expect("name")
                .nullable()
        );
    }

    fn ident_ok(value: &str) -> bool {
        let mut chars = value.chars();
        matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first == '_')
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }

    #[test]
    fn every_field_is_a_well_formed_identifier() {
        for meta in ENTITIES {
            let table = meta.table;
            assert!(ident_ok(table), "table {table}");
            for spec in meta.fields {
                assert!(ident_ok(spec.column), "column {}", spec.column);
                assert!(ident_ok(spec.name), "field {}", spec.name);
                assert_relation_identifiers(spec.kind);
            }
        }
    }

    fn assert_relation_identifiers(kind: FieldKind) {
        match kind {
            FieldKind::Junction {
                table: junction,
                local_column,
                remote_column,
                ..
            }
            | FieldKind::ContentTypeJunction {
                table: junction,
                local_column,
                remote_column,
            } => {
                assert!(ident_ok(junction), "junction {junction}");
                assert!(ident_ok(local_column), "junction local {local_column}");
                assert!(ident_ok(remote_column), "junction remote {remote_column}");
            }
            FieldKind::ValueJunction {
                table: junction,
                local_column,
                protocol_column,
                port_column,
                ordinal_column,
            } => {
                assert!(ident_ok(junction), "value junction {junction}");
                assert!(
                    ident_ok(local_column),
                    "value junction local {local_column}"
                );
                assert!(
                    ident_ok(protocol_column),
                    "value junction protocol {protocol_column}"
                );
                assert!(ident_ok(port_column), "value junction port {port_column}");
                assert!(
                    ident_ok(ordinal_column),
                    "value junction ordinal {ordinal_column}"
                );
            }
            FieldKind::GenericReference { type_column, .. } => {
                assert!(ident_ok(type_column), "gfk type column {type_column}");
            }
            _ => {}
        }
    }

    #[test]
    fn field_names_are_unique_per_entity() {
        for meta in ENTITIES {
            let mut names: Vec<&str> = meta.fields.iter().map(|spec| spec.name).collect();
            let total = names.len();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), total, "duplicate field on {}", meta.source_key);
        }
    }

    #[test]
    fn inventory_field_ordering_is_stable_and_consistent() {
        let a = InventoryEntity::Device.field("name").expect("name");
        let b = InventoryEntity::Device.field("role").expect("role");
        assert_eq!(a, a);
        assert_ne!(a, b);
        assert_eq!(a.cmp(&b), a.name().cmp(b.name()));
        // Copy semantics: obtaining the same field twice yields equal handles.
        let again = InventoryEntity::Device.field("name").expect("name");
        assert_eq!(a, again);
    }

    #[test]
    fn tags_are_modelled_for_every_taggable_entity() {
        for meta in ENTITIES {
            if !meta.has_tags {
                assert!(
                    meta.entity.field("tags").is_none(),
                    "{} declares no native tags column",
                    meta.source_key
                );
                continue;
            }
            let tags = meta
                .entity
                .field("tags")
                .unwrap_or_else(|| panic!("{} has no tags field", meta.source_key));
            assert_eq!(tags.column(), "tags");
            assert_eq!(tags.sql_type(), PgType::TextArray);
            assert!(matches!(tags.kind(), FieldKind::Tags));
            assert!(meta.has_tags, "{} lost its tags flag", meta.source_key);
        }
    }

    #[test]
    fn generic_references_name_their_allowed_targets() {
        for meta in ENTITIES {
            for spec in meta.fields {
                if let FieldKind::GenericReference {
                    type_column,
                    source_type_field,
                    allowed,
                } = spec.kind
                {
                    if allowed.is_empty() {
                        // Component types are outside this closed source slice;
                        // populated references must fail, not resolve by raw id.
                        assert_eq!(meta.entity, InventoryEntity::InventoryItem);
                        assert_eq!(spec.name, "component");
                        assert!(spec.nullable);
                    }
                    assert!(type_column.ends_with("_type_id"));
                    assert!(!source_type_field.is_empty());
                }
            }
        }
    }

    #[test]
    fn external_references_are_never_resolved_by_remote_id() {
        // A foreign key to an out-of-scope entity has no local target. A null
        // source leaves the column untouched; a populated source reference
        // cannot be resolved and the converter fails the pull rather than
        // dropping it or guessing a remote id.
        for meta in ENTITIES {
            for spec in meta.fields {
                if let FieldKind::ForeignKey { target: None } = spec.kind {
                    assert!(spec.column.ends_with("_id"), "{}", spec.column);
                }
            }
        }
    }

    #[test]
    fn membership_entity_owns_no_table_or_fields() {
        assert_eq!(InventoryEntity::TaggedObject.table(), None);
        assert!(InventoryEntity::TaggedObject.is_membership());
        assert!(InventoryEntity::TaggedObject.fields().is_empty());
        assert!(InventoryEntity::TaggedObject.field("tags").is_none());
    }

    #[test]
    fn pg_type_array_and_integer_helpers() {
        assert!(PgType::TextArray.is_array());
        assert!(PgType::Integer.accepts_integer());
        assert!(PgType::Numeric.accepts_integer());
        assert!(!PgType::Text.accepts_integer());
        assert!(!PgType::Bool.accepts_integer());
    }

    #[test]
    fn meta_index_matches_enum_discriminant() {
        for (index, meta) in ENTITIES.iter().enumerate() {
            assert_eq!(meta.entity as usize, index, "{}", meta.source_key);
            assert_eq!(
                meta.entity.meta().map(|entry| entry.source_key),
                Some(meta.source_key)
            );
        }
        assert!(InventoryEntity::TaggedObject.meta().is_none());
        assert_eq!(InventoryEntity::TaggedObject as usize, ENTITIES.len());
    }

    #[test]
    fn binding_target_key_is_canonical_and_membership_key_is_distinct() {
        assert_eq!(
            InventoryEntity::Device.target_key(),
            "piping.inventory.device"
        );
        assert_eq!(
            InventoryEntity::VmInterface.target_key(),
            "piping.inventory.vminterface"
        );
        assert_eq!(
            InventoryEntity::TaggedObject.target_key(),
            "piping.inventory.tagged_object"
        );
    }

    #[test]
    fn approved_and_excluded_field_sets_are_disjoint() {
        for meta in ENTITIES {
            for (field, reason) in meta.entity.excluded_fields() {
                assert!(
                    !reason.is_empty(),
                    "{}::{field} has no reason",
                    meta.source_key
                );
                assert!(
                    meta.entity.field(field).is_none(),
                    "{}::{field} is both approved and excluded",
                    meta.source_key
                );
            }
            for spec in meta.fields {
                assert!(
                    meta.entity.excluded_source_field(spec.name).is_none(),
                    "{}::{} is both approved and excluded",
                    meta.source_key,
                    spec.name
                );
            }
        }
        // A globally derived field is excluded on every row entity.
        assert!(
            InventoryEntity::Device
                .excluded_source_field("display")
                .is_some()
        );
        assert!(
            InventoryEntity::Interface
                .excluded_source_field("mac_address")
                .is_some()
        );
        // An unknown name is neither approved nor excluded, so the converter
        // blocks it.
        assert!(
            InventoryEntity::Device
                .excluded_source_field("mystery_business_field")
                .is_none()
        );
        assert!(
            InventoryEntity::Device
                .field("mystery_business_field")
                .is_none()
        );
    }
    #[test]
    fn not_null_no_default_fields_have_explicit_absent_encoding() {
        for meta in ENTITIES {
            for spec in meta.fields {
                if spec.nullable || spec.default.is_some() {
                    continue;
                }
                // Relation-only fields (junctions) have no bound column.
                match spec.kind {
                    FieldKind::Junction { .. }
                    | FieldKind::ContentTypeJunction { .. }
                    | FieldKind::ValueJunction { .. } => continue,
                    _ => {}
                }
                assert_ne!(
                    spec.absent,
                    Absent::Preserve,
                    "{}::{} must declare how a null source is encoded",
                    meta.source_key,
                    spec.name
                );
            }
        }
        // The census-proven cases: native empty encodings for optional
        // columns, hard rejection for a required one.
        let front = InventoryEntity::DeviceType
            .field("front_image")
            .expect("front_image");
        assert_eq!(front.absent(), Absent::EmptyText);
        let protocol = InventoryEntity::Service
            .field("protocol")
            .expect("protocol");
        assert!(protocol.nullable());
        assert_eq!(protocol.absent(), Absent::Preserve);
        let ports = InventoryEntity::Service.field("ports").expect("ports");
        assert!(ports.nullable());
        assert_eq!(ports.absent(), Absent::Preserve);
        let name = InventoryEntity::Manufacturer.field("name").expect("name");
        assert_eq!(name.absent(), Absent::Reject);
    }
}
