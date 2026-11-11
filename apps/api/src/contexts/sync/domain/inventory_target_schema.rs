//! Closed, whitelisted schema for the local Piping inventory target (PIP-86).
//!
//! This module is the ONLY place that knows which target entity, table, column,
//! junction and PostgreSQL type a synchronised source field may reach. Every
//! physical identifier in [`crate::contexts::sync::infrastructure::persistence::local_inventory_target`]
//! comes from a [`FieldSpec`] or [`EntityMeta`] here — never from a caller, a
//! mapping document or a provider payload. The table/column set was generated
//! from the migrated PostgreSQL schema (`0001_init.sql` and its forward
//! migrations) and verified field by field against the source
//! census, so an unknown source entity or field is rejected instead of being
//! coerced or dropped.
//!
//! Two invariants hold:
//!
//! * [`InventoryEntity`] is closed over the supported source
//!   resources; [`InventoryEntity::from_source_key`] is the only constructor
//!   from an untrusted string.
//! * [`InventoryField`] is opaque and can only be obtained from an entity's
//!   catalogue ([`InventoryEntity::field`]) — there is no public constructor, so
//!   a caller cannot name a column that the catalogue did not approve.
//!
//! `super::inventory_sync` owns the port traits; this module owns the concrete
//! closed entity/field catalogue they are implemented against.

mod catalog_fields;
mod hardware_fields;

use std::fmt;

/// A typed source-field disposition, never an unconditional string exclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFieldDisposition {
    Derived(&'static str),
    HandledElsewhere(&'static str),
    UnsupportedWhenPopulated(&'static str),
}

/// Source caches are assertions about canonical data, never independent IDs or
/// mutable mapping targets. Preparation validates each admitted assertion.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SourceCacheKind {
    DeviceScope {
        field: &'static str,
        target: InventoryEntity,
    },
    TerminationDevice,
    AbsoluteLength,
    AbsoluteWeight,
    AbsoluteFlow,
    AbsoluteDiameter,
    CablePath,
}

impl SourceFieldDisposition {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Derived(reason)
            | Self::HandledElsewhere(reason)
            | Self::UnsupportedWhenPopulated(reason) => reason,
        }
    }

    pub fn permits_populated(self) -> bool {
        !matches!(self, Self::UnsupportedWhenPopulated(_))
    }
}

/// A checked PostgreSQL type the target may bind to. Mirrors the allowlist of
/// column types in the migrated schema; anything outside it is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PgType {
    Text,
    TextArray,
    SmallIntArray,
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
            Self::TextArray
                | Self::SmallIntArray
                | Self::IntArray
                | Self::UuidArray
                | Self::BigIntArray
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
    RackGroup,
    CableBundle,
    ModuleBayType,
    CoolingSource,
    CoolingFeed,
    CoolingIntake,
    CoolingOutflow,
    CoolingIntakeTemplate,
    CoolingOutflowTemplate,
    Location,
    Rack,
    Cable,
    ModuleType,
    ModuleBayTemplate,
    ModuleBay,
    Module,
    VlanTranslationPolicy,
    OwnerGroup,
    Owner,
    TenantGroup,
    ConsolePortTemplate,
    ConsoleServerPortTemplate,
    DeviceBayTemplate,
    FrontPortTemplate,
    InterfaceTemplate,
    InventoryItemTemplate,
    PowerOutletTemplate,
    PowerPortTemplate,
    RearPortTemplate,
    PortTemplateMapping,
    Vlan,
    Contact,
    ContactGroup,
    ContactRole,
    ContactAssignment,
    /// `data-flows.application-roles`: catalog rows of the optional NetBox
    /// `data-flows` plugin namespace.
    ApplicationRole,
    /// `data-flows.applications`: optional plugin namespace.
    Application,
    /// `data-flows.dataflow-groups`: optional plugin namespace.
    DataflowGroup,
    /// `data-flows.dataflows`: optional plugin namespace.
    Dataflow,
    /// `data-flows.objectalias`: optional plugin namespace. The native
    /// `inventory_address_group` is the reusable named address set.
    AddressGroup,
    /// `extras.tagged-objects`: a tag membership, not an entity row. It is
    /// represented natively on the owning entities' `tags text[]` columns and
    /// never materialises a row of its own.
    TaggedObject,
    /// Source-only cable membership, projected onto the canonical Interface
    /// attachment columns. It never creates a second native endpoint store.
    CableTermination,
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
    /// Native `text[]` tag membership on the owning table.
    Tags,
    /// Ordered typed selector rows in one owner/side table, e.g.
    /// `inventory_dataflow_selector`. Each source entry names its own object
    /// type and provider identity; the row carries the position, the side and
    /// exactly one typed endpoint column.
    SelectorRows {
        table: &'static str,
        /// Column holding the owning row, `dataflow_id` or `group_id`.
        owner_column: &'static str,
        /// Native side value: `source`, `destination` or `member`.
        side: &'static str,
        /// Admitted `(object_type token, endpoint column, target entity)`.
        allowed: &'static [(&'static str, &'static str, InventoryEntity)],
    },
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
        (true, true),
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
    meta(
        InventoryEntity::RackGroup,
        "dcim.rack-groups",
        "dcim_rackgroup",
        ("dcim", "rackgroup"),
        (true, true),
        hardware_fields::RACK_GROUP,
    ),
    meta(
        InventoryEntity::CableBundle,
        "dcim.cable-bundles",
        "dcim_cablebundle",
        ("dcim", "cablebundle"),
        (true, true),
        hardware_fields::CABLE_BUNDLE,
    ),
    meta(
        InventoryEntity::ModuleBayType,
        "dcim.module-bay-types",
        "dcim_modulebaytype",
        ("dcim", "modulebaytype"),
        (true, true),
        hardware_fields::MODULE_BAY_TYPE,
    ),
    meta(
        InventoryEntity::CoolingSource,
        "dcim.cooling-sources",
        "dcim_coolingsource",
        ("dcim", "coolingsource"),
        (true, true),
        hardware_fields::COOLING_SOURCE,
    ),
    meta(
        InventoryEntity::CoolingFeed,
        "dcim.cooling-feeds",
        "dcim_coolingfeed",
        ("dcim", "coolingfeed"),
        (true, true),
        hardware_fields::COOLING_FEED,
    ),
    meta(
        InventoryEntity::CoolingIntake,
        "dcim.cooling-intakes",
        "dcim_coolingintake",
        ("dcim", "coolingintake"),
        (true, true),
        hardware_fields::COOLING_INTAKE,
    ),
    meta(
        InventoryEntity::CoolingOutflow,
        "dcim.cooling-outflows",
        "dcim_coolingoutflow",
        ("dcim", "coolingoutflow"),
        (true, true),
        hardware_fields::COOLING_OUTFLOW,
    ),
    meta(
        InventoryEntity::CoolingIntakeTemplate,
        "dcim.cooling-intake-templates",
        "dcim_coolingintaketemplate",
        ("dcim", "coolingintaketemplate"),
        (false, false),
        hardware_fields::COOLING_INTAKE_TEMPLATE,
    ),
    meta(
        InventoryEntity::CoolingOutflowTemplate,
        "dcim.cooling-outflow-templates",
        "dcim_coolingoutflowtemplate",
        ("dcim", "coolingoutflowtemplate"),
        (false, false),
        hardware_fields::COOLING_OUTFLOW_TEMPLATE,
    ),
    meta(
        InventoryEntity::Location,
        "dcim.locations",
        "dcim_location",
        ("dcim", "location"),
        (true, true),
        hardware_fields::LOCATION,
    ),
    meta(
        InventoryEntity::Rack,
        "dcim.racks",
        "dcim_rack",
        ("dcim", "rack"),
        (true, true),
        hardware_fields::RACK,
    ),
    meta(
        InventoryEntity::Cable,
        "dcim.cables",
        "dcim_cable",
        ("dcim", "cable"),
        (true, true),
        hardware_fields::CABLE,
    ),
    meta(
        InventoryEntity::ModuleType,
        "dcim.module-types",
        "dcim_moduletype",
        ("dcim", "moduletype"),
        (true, true),
        hardware_fields::MODULE_TYPE,
    ),
    meta(
        InventoryEntity::ModuleBayTemplate,
        "dcim.module-bay-templates",
        "dcim_modulebaytemplate",
        ("dcim", "modulebaytemplate"),
        (false, false),
        hardware_fields::MODULE_BAY_TEMPLATE,
    ),
    meta(
        InventoryEntity::ModuleBay,
        "dcim.module-bays",
        "dcim_modulebay",
        ("dcim", "modulebay"),
        (true, true),
        hardware_fields::MODULE_BAY,
    ),
    meta(
        InventoryEntity::Module,
        "dcim.modules",
        "dcim_module",
        ("dcim", "module"),
        (true, true),
        hardware_fields::MODULE,
    ),
    meta(
        InventoryEntity::VlanTranslationPolicy,
        "ipam.vlan-translation-policies",
        "ipam_vlantranslationpolicy",
        ("ipam", "vlantranslationpolicy"),
        (true, true),
        catalog_fields::VLAN_TRANSLATION_POLICY_FIELDS,
    ),
    meta(
        InventoryEntity::OwnerGroup,
        "users.owner-groups",
        "users_ownergroup",
        ("users", "ownergroup"),
        (false, false),
        catalog_fields::OWNER_GROUP_FIELDS,
    ),
    meta(
        InventoryEntity::Owner,
        "users.owners",
        "users_owner",
        ("users", "owner"),
        (false, false),
        catalog_fields::OWNER_FIELDS,
    ),
    meta(
        InventoryEntity::TenantGroup,
        "tenancy.tenant-groups",
        "tenancy_tenantgroup",
        ("tenancy", "tenantgroup"),
        (true, true),
        catalog_fields::TENANT_GROUP_FIELDS,
    ),
    meta(
        InventoryEntity::ConsolePortTemplate,
        "dcim.console-port-templates",
        "dcim_consoleporttemplate",
        ("dcim", "consoleporttemplate"),
        (false, false),
        hardware_fields::CONSOLE_PORT_TEMPLATE,
    ),
    meta(
        InventoryEntity::ConsoleServerPortTemplate,
        "dcim.console-server-port-templates",
        "dcim_consoleserverporttemplate",
        ("dcim", "consoleserverporttemplate"),
        (false, false),
        hardware_fields::CONSOLE_SERVER_PORT_TEMPLATE,
    ),
    meta(
        InventoryEntity::DeviceBayTemplate,
        "dcim.device-bay-templates",
        "dcim_devicebaytemplate",
        ("dcim", "devicebaytemplate"),
        (false, false),
        hardware_fields::DEVICE_BAY_TEMPLATE,
    ),
    meta(
        InventoryEntity::FrontPortTemplate,
        "dcim.front-port-templates",
        "dcim_frontporttemplate",
        ("dcim", "frontporttemplate"),
        (false, false),
        hardware_fields::FRONT_PORT_TEMPLATE,
    ),
    meta(
        InventoryEntity::InterfaceTemplate,
        "dcim.interface-templates",
        "dcim_interfacetemplate",
        ("dcim", "interfacetemplate"),
        (false, false),
        hardware_fields::INTERFACE_TEMPLATE,
    ),
    meta(
        InventoryEntity::InventoryItemTemplate,
        "dcim.inventory-item-templates",
        "dcim_inventoryitemtemplate",
        ("dcim", "inventoryitemtemplate"),
        (false, false),
        hardware_fields::INVENTORY_ITEM_TEMPLATE,
    ),
    meta(
        InventoryEntity::PowerOutletTemplate,
        "dcim.power-outlet-templates",
        "dcim_poweroutlettemplate",
        ("dcim", "poweroutlettemplate"),
        (false, false),
        hardware_fields::POWER_OUTLET_TEMPLATE,
    ),
    meta(
        InventoryEntity::PowerPortTemplate,
        "dcim.power-port-templates",
        "dcim_powerporttemplate",
        ("dcim", "powerporttemplate"),
        (false, false),
        hardware_fields::POWER_PORT_TEMPLATE,
    ),
    meta(
        InventoryEntity::RearPortTemplate,
        "dcim.rear-port-templates",
        "dcim_rearporttemplate",
        ("dcim", "rearporttemplate"),
        (false, false),
        hardware_fields::REAR_PORT_TEMPLATE,
    ),
    meta(
        InventoryEntity::PortTemplateMapping,
        "dcim.port-template-mappings",
        "dcim_porttemplatemapping",
        ("dcim", "porttemplatemapping"),
        (false, false),
        hardware_fields::PORT_TEMPLATE_MAPPING,
    ),
    meta(
        InventoryEntity::Vlan,
        "ipam.vlans",
        "ipam_vlan",
        ("ipam", "vlan"),
        (true, true),
        catalog_fields::VLAN_FIELDS,
    ),
    meta(
        InventoryEntity::Contact,
        "tenancy.contacts",
        "tenancy_contact",
        ("tenancy", "contact"),
        (true, true),
        catalog_fields::CONTACT_FIELDS,
    ),
    meta(
        InventoryEntity::ContactGroup,
        "tenancy.contact-groups",
        "tenancy_contactgroup",
        ("tenancy", "contactgroup"),
        (true, true),
        catalog_fields::CONTACT_GROUP_FIELDS,
    ),
    meta(
        InventoryEntity::ContactRole,
        "tenancy.contact-roles",
        "tenancy_contactrole",
        ("tenancy", "contactrole"),
        (true, true),
        catalog_fields::CONTACT_ROLE_FIELDS,
    ),
    meta(
        InventoryEntity::ContactAssignment,
        "tenancy.contact-assignments",
        "tenancy_contactassignment",
        ("tenancy", "contactassignment"),
        (true, true),
        catalog_fields::CONTACT_ASSIGNMENT_FIELDS,
    ),
    meta(
        InventoryEntity::ApplicationRole,
        "data-flows.application-roles",
        "inventory_application_role",
        ("inventory", "applicationrole"),
        (false, false),
        catalog_fields::APPLICATION_ROLE_FIELDS,
    ),
    meta(
        InventoryEntity::Application,
        "data-flows.applications",
        "inventory_application",
        ("inventory", "application"),
        (true, true),
        catalog_fields::APPLICATION_FIELDS,
    ),
    meta(
        InventoryEntity::DataflowGroup,
        "data-flows.dataflow-groups",
        "inventory_dataflow_group",
        ("inventory", "dataflowgroup"),
        (true, true),
        catalog_fields::DATAFLOW_GROUP_FIELDS,
    ),
    meta(
        InventoryEntity::Dataflow,
        "data-flows.dataflows",
        "inventory_dataflow",
        ("inventory", "dataflow"),
        (true, true),
        catalog_fields::DATAFLOW_FIELDS,
    ),
    meta(
        InventoryEntity::AddressGroup,
        "data-flows.objectalias",
        "inventory_address_group",
        ("inventory", "addressgroup"),
        (true, true),
        catalog_fields::ADDRESS_GROUP_FIELDS,
    ),
];

/// Business ownership is an inventory relation. Authentication memberships are
/// deliberately not part of this field or its source-qualified identity.
const fn owner_field(entity: InventoryEntity) -> FieldSpec {
    FieldSpec {
        entity,
        name: "owner",
        column: "owner_id",
        sql_type: PgType::Uuid,
        nullable: true,
        default: None,
        kind: FieldKind::ForeignKey {
            target: Some(InventoryEntity::Owner),
        },
        choice_normalized: false,
        absent: Absent::Preserve,
    }
}

/// The membership-only resource, kept out of [`ENTITIES`] so a row-producing
/// lookup never returns it.
pub const TAGGED_OBJECT_SOURCE_KEY: &str = "extras.tagged-objects";
const CABLE_TERMINATION_SOURCE_KEY: &str = "dcim.cable-terminations";

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
    /// Resources whose native invariants are serialized by the hardware graph
    /// owner, before any resource-row or inventory observation lock.
    pub(crate) fn uses_hardware_graph(self) -> bool {
        self.is_hardware()
            || matches!(
                self,
                Self::Device
                    | Self::DeviceType
                    | Self::Interface
                    | Self::InventoryItem
                    | Self::Cable
                    | Self::VmInterface
                    | Self::VlanTranslationPolicy
            )
    }

    /// ModuleBay bookkeeping belongs to its native graph trigger, not the
    /// importer's nested-set rebuild.
    pub(crate) fn is_tree(self) -> bool {
        self != Self::ModuleBay
            && self
                .internal_columns()
                .iter()
                .any(|(column, _, _)| *column == "tree_id")
    }

    /// Nullable inventory references may be withheld only on resources whose
    /// native graph and hierarchy contracts permit that intermediate state.
    pub(crate) fn allows_deferred_references(self) -> bool {
        !self.uses_hardware_graph() && !self.is_tree()
    }

    pub(crate) const fn is_hardware(self) -> bool {
        matches!(
            self,
            Self::RackGroup
                | Self::CableBundle
                | Self::ModuleBayType
                | Self::CoolingSource
                | Self::CoolingFeed
                | Self::CoolingIntake
                | Self::CoolingOutflow
                | Self::CoolingIntakeTemplate
                | Self::CoolingOutflowTemplate
                | Self::Location
                | Self::Rack
                | Self::Cable
                | Self::ModuleType
                | Self::ModuleBayTemplate
                | Self::ModuleBay
                | Self::Module
                | Self::ConsolePortTemplate
                | Self::ConsoleServerPortTemplate
                | Self::DeviceBayTemplate
                | Self::FrontPortTemplate
                | Self::InterfaceTemplate
                | Self::InventoryItemTemplate
                | Self::PowerOutletTemplate
                | Self::PowerPortTemplate
                | Self::RearPortTemplate
                | Self::PortTemplateMapping
        )
    }

    /// Every row-producing entity in the closed catalogue.
    #[must_use = "the iterator performs no work until it is consumed"]
    pub fn all() -> impl Iterator<Item = InventoryEntity> {
        ENTITIES.iter().map(|meta| meta.entity)
    }

    /// Metadata for this entity; `None` for source-only memberships.
    ///
    /// The array index is the enum discriminant: `ENTITIES` is declared in the
    /// same order as [`InventoryEntity`], which the `meta_index_matches_enum`
    /// test pins.
    #[must_use]
    pub fn meta(self) -> Option<&'static EntityMeta> {
        if self.is_membership() {
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
        if key == CABLE_TERMINATION_SOURCE_KEY {
            return Some(Self::CableTermination);
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
            None if self == Self::CableTermination => CABLE_TERMINATION_SOURCE_KEY,
            None => TAGGED_OBJECT_SOURCE_KEY,
        }
    }

    /// Canonical binding target key stored in
    /// `sync_inventory_binding.target_entity`, e.g. `piping.inventory.device`.
    #[must_use]
    pub fn target_key(self) -> String {
        match self.content_type() {
            Some((_, model)) => format!("piping.inventory.{model}"),
            None if self == Self::CableTermination => "piping.inventory.cable_termination".into(),
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
        matches!(self, Self::TaggedObject | Self::CableTermination)
    }

    /// Whether this entity belongs to the optional `data-flows` plugin
    /// namespace.
    ///
    /// The core manifest stays mandatory and complete; these five are imported
    /// only when the provider schema exposes the plugin and they never make a
    /// core pull incomplete.
    #[must_use]
    pub fn is_optional_source(self) -> bool {
        matches!(
            self,
            Self::ApplicationRole
                | Self::Application
                | Self::DataflowGroup
                | Self::Dataflow
                | Self::AddressGroup
        )
    }

    /// Row-producing entities of the mandatory core manifest.
    #[must_use = "the iterator performs no work until it is consumed"]
    pub fn core() -> impl Iterator<Item = InventoryEntity> {
        Self::all().filter(|entity| !entity.is_optional_source())
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

    /// Typed treatment of source fields without a directly mapped target field.
    #[must_use]
    pub fn field_dispositions(self) -> &'static [(&'static str, SourceFieldDisposition)] {
        if self.is_optional_source() {
            // The optional plugin projection emits exactly the approved fields
            // plus these three, which the binding and the custom-field writer
            // own. Every other upstream field is classified by the projection
            // itself and never reaches a source record.
            return &[
                (
                    "created",
                    SourceFieldDisposition::HandledElsewhere(
                        "source timestamp retained in the provider binding",
                    ),
                ),
                (
                    "last_updated",
                    SourceFieldDisposition::HandledElsewhere(
                        "source timestamp retained in the provider binding",
                    ),
                ),
                (
                    "custom_fields",
                    SourceFieldDisposition::HandledElsewhere(
                        "entity custom-field values (jsonb cf writer)",
                    ),
                ),
            ];
        }
        if matches!(
            self,
            Self::Vlan
                | Self::Contact
                | Self::ContactGroup
                | Self::ContactRole
                | Self::ContactAssignment
        ) {
            return &[
                (
                    "id",
                    SourceFieldDisposition::HandledElsewhere("provider external id (binding)"),
                ),
                (
                    "created",
                    SourceFieldDisposition::HandledElsewhere(
                        "source timestamp retained in provider binding",
                    ),
                ),
                (
                    "last_updated",
                    SourceFieldDisposition::HandledElsewhere(
                        "source timestamp retained in provider binding",
                    ),
                ),
                (
                    "custom_fields",
                    SourceFieldDisposition::HandledElsewhere(
                        "entity custom-field values (jsonb cf writer)",
                    ),
                ),
                (
                    "display",
                    SourceFieldDisposition::Derived("read-only derived label"),
                ),
                (
                    "display_url",
                    SourceFieldDisposition::Derived("read-only derived url"),
                ),
                (
                    "url",
                    SourceFieldDisposition::Derived("read-only derived url"),
                ),
            ];
        }
        if self == Self::CableTermination {
            return &[
                (
                    "id",
                    SourceFieldDisposition::HandledElsewhere(
                        "source relation identity in the sealed value census",
                    ),
                ),
                (
                    "cable",
                    SourceFieldDisposition::HandledElsewhere(
                        "typed cable edge checked against the canonical Interface attachment",
                    ),
                ),
                (
                    "cable_end",
                    SourceFieldDisposition::HandledElsewhere(
                        "canonical Interface cable_end and retained side assertion",
                    ),
                ),
                (
                    "termination",
                    SourceFieldDisposition::HandledElsewhere(
                        "typed endpoint edge checked by the Interface graph owner",
                    ),
                ),
                (
                    "termination_type",
                    SourceFieldDisposition::HandledElsewhere(
                        "only the canonical dcim.interface endpoint projection is supported",
                    ),
                ),
                (
                    "connector",
                    SourceFieldDisposition::HandledElsewhere(
                        "lossless canonical Interface cable_connector projection",
                    ),
                ),
                (
                    "positions",
                    SourceFieldDisposition::HandledElsewhere(
                        "lossless ordered Interface cable_positions projection",
                    ),
                ),
                (
                    "created",
                    SourceFieldDisposition::HandledElsewhere(
                        "source relation timestamp retained in the sealed value census",
                    ),
                ),
                (
                    "last_updated",
                    SourceFieldDisposition::HandledElsewhere(
                        "source relation timestamp retained in the sealed value census",
                    ),
                ),
            ];
        }
        if self == Self::OwnerGroup {
            return &[
                (
                    "id",
                    SourceFieldDisposition::HandledElsewhere("provider external id (binding)"),
                ),
                (
                    "url",
                    SourceFieldDisposition::Derived("read-only source API link"),
                ),
                (
                    "display_url",
                    SourceFieldDisposition::Derived("read-only source UI link"),
                ),
                (
                    "display",
                    SourceFieldDisposition::Derived("read-only derived label"),
                ),
                (
                    "member_count",
                    SourceFieldDisposition::Derived("derived owner-group member count"),
                ),
            ];
        }
        if self == Self::Owner {
            return &[
                (
                    "id",
                    SourceFieldDisposition::HandledElsewhere("provider external id (binding)"),
                ),
                (
                    "url",
                    SourceFieldDisposition::Derived("read-only source API link"),
                ),
                (
                    "display_url",
                    SourceFieldDisposition::Derived("read-only source UI link"),
                ),
                (
                    "display",
                    SourceFieldDisposition::Derived("read-only derived label"),
                ),
                (
                    "users",
                    SourceFieldDisposition::UnsupportedWhenPopulated(
                        "authentication user memberships require a separate identity conversion",
                    ),
                ),
                (
                    "user_groups",
                    SourceFieldDisposition::UnsupportedWhenPopulated(
                        "authentication group memberships require a separate identity conversion",
                    ),
                ),
            ];
        }
        if self == Self::VlanTranslationPolicy {
            return &[
                (
                    "id",
                    SourceFieldDisposition::HandledElsewhere("provider external id (binding)"),
                ),
                (
                    "custom_fields",
                    SourceFieldDisposition::HandledElsewhere(
                        "entity custom-field values (jsonb cf writer)",
                    ),
                ),
                (
                    "created",
                    SourceFieldDisposition::Derived("read-only source timestamp"),
                ),
                (
                    "last_updated",
                    SourceFieldDisposition::Derived("read-only source timestamp"),
                ),
                (
                    "display",
                    SourceFieldDisposition::Derived("read-only derived label"),
                ),
                (
                    "display_url",
                    SourceFieldDisposition::Derived("read-only derived url"),
                ),
                (
                    "url",
                    SourceFieldDisposition::Derived("read-only source API link"),
                ),
            ];
        }
        if let Some(fields) = hardware_fields::dispositions(self) {
            return fields;
        }
        if self == Self::TaggedObject {
            return &[
                (
                    "id",
                    SourceFieldDisposition::HandledElsewhere("membership external identity"),
                ),
                (
                    "tag",
                    SourceFieldDisposition::HandledElsewhere(
                        "consumed by native tag membership validation",
                    ),
                ),
                (
                    "object_type",
                    SourceFieldDisposition::HandledElsewhere(
                        "consumed by native tag membership validation",
                    ),
                ),
                (
                    "object_id",
                    SourceFieldDisposition::HandledElsewhere(
                        "consumed by native tag membership validation",
                    ),
                ),
                (
                    "object",
                    SourceFieldDisposition::Derived(
                        "derived nested object representation; identity comes from object_id",
                    ),
                ),
                (
                    "display",
                    SourceFieldDisposition::Derived("read-only derived label"),
                ),
                (
                    "url",
                    SourceFieldDisposition::Derived("read-only source API link"),
                ),
            ];
        }
        catalog_fields::FIELD_DISPOSITIONS_BY_INDEX
            .get(self as usize)
            .copied()
            .unwrap_or(&[])
    }

    /// `UnsupportedWhenPopulated` is a blocker, not permission to discard data.
    /// `None` means approved or unknown; unknown populated fields also block.
    #[must_use]
    pub fn source_field_disposition(self, name: &str) -> Option<SourceFieldDisposition> {
        if self.source_cache_kind(name).is_some() {
            return Some(SourceFieldDisposition::HandledElsewhere(
                "validated against the preserved canonical owner graph or quantity and unit",
            ));
        }
        if self == Self::Prefix && name == "_children" {
            return Some(SourceFieldDisposition::Derived(
                "native immediate-child count recomputed from the prefix and VRF graph",
            ));
        }
        if self == Self::ContactAssignment && matches!(name, "object_type" | "object_id") {
            return Some(SourceFieldDisposition::HandledElsewhere(
                "generic reference companion",
            ));
        }
        self.field_dispositions()
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, reason)| *reason)
    }

    pub(crate) fn source_cache_kind(self, name: &str) -> Option<SourceCacheKind> {
        use SourceCacheKind as C;
        match (self, name) {
            (Self::Cable, "_abs_length") => Some(C::AbsoluteLength),
            (Self::DeviceType | Self::ModuleType | Self::Rack, "_abs_weight") => {
                Some(C::AbsoluteWeight)
            }
            (
                Self::CoolingFeed | Self::CoolingIntake | Self::CoolingIntakeTemplate,
                "_abs_max_flow",
            ) => Some(C::AbsoluteFlow),
            (
                Self::CoolingIntake
                | Self::CoolingIntakeTemplate
                | Self::CoolingOutflow
                | Self::CoolingOutflowTemplate,
                "_abs_diameter",
            ) => Some(C::AbsoluteDiameter),
            (Self::CableTermination, "_device") => Some(C::TerminationDevice),
            (Self::Interface, "_path") => Some(C::CablePath),
            (
                Self::Interface
                | Self::InventoryItem
                | Self::ModuleBay
                | Self::CoolingIntake
                | Self::CoolingOutflow
                | Self::CableTermination,
                "_site",
            ) => Some(C::DeviceScope {
                field: "site",
                target: Self::Site,
            }),
            (
                Self::Interface
                | Self::InventoryItem
                | Self::ModuleBay
                | Self::CoolingIntake
                | Self::CoolingOutflow
                | Self::CableTermination,
                "_rack",
            ) => Some(C::DeviceScope {
                field: "rack",
                target: Self::Rack,
            }),
            (
                Self::Interface
                | Self::InventoryItem
                | Self::ModuleBay
                | Self::CoolingIntake
                | Self::CoolingOutflow
                | Self::CableTermination,
                "_location",
            ) => Some(C::DeviceScope {
                field: "location",
                target: Self::Location,
            }),
            _ => None,
        }
    }

    /// These inherited native columns are consumed by cabling and component
    /// scope queries. Cooling owners instead retain their canonical Device FK.
    pub(crate) fn scope_cache_columns(self) -> &'static [(&'static str, &'static str)] {
        if matches!(
            self,
            Self::Interface | Self::InventoryItem | Self::ModuleBay
        ) {
            &[
                ("_site_id", "site_id"),
                ("_rack_id", "rack_id"),
                ("_location_id", "location_id"),
            ]
        } else {
            &[]
        }
    }

    /// NOT NULL columns without a database default that no mapped field writes.
    /// Each entry is `(column, kind, argument)`; `kind` is `"literal"` (a SQL
    /// expression to bind) or `"copy"` (the value of the approved field
    /// `argument`). The SQL target must synthesise these on insert.
    #[must_use]
    pub fn internal_columns(self) -> &'static [(&'static str, &'static str, &'static str)] {
        if self == Self::VlanTranslationPolicy {
            return &[("custom_field_data", "literal", "'{}'::jsonb")];
        }
        if matches!(self, Self::TenantGroup | Self::ContactGroup) {
            return &[
                ("custom_field_data", "literal", "'{}'::jsonb"),
                ("lft", "literal", "0"),
                ("rght", "literal", "0"),
                ("tree_id", "literal", "0"),
                ("level", "literal", "0"),
            ];
        }
        if matches!(
            self,
            Self::Vlan | Self::Contact | Self::ContactRole | Self::ContactAssignment
        ) {
            return &[("custom_field_data", "literal", "'{}'::jsonb")];
        }
        if let Some(columns) = hardware_fields::internal_columns(self) {
            return columns;
        }
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

    /// The supported source resources, including the hardware graph and its
    /// imported dependency owners.
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
        "dcim.rack-groups",
        "dcim.cable-bundles",
        "dcim.module-bay-types",
        "dcim.cooling-sources",
        "dcim.cooling-feeds",
        "dcim.cooling-intakes",
        "dcim.cooling-outflows",
        "dcim.cooling-intake-templates",
        "dcim.cooling-outflow-templates",
        "dcim.locations",
        "dcim.racks",
        "dcim.cables",
        "dcim.cable-terminations",
        "dcim.module-types",
        "dcim.module-bay-templates",
        "dcim.module-bays",
        "dcim.modules",
        "extras.custom-fields",
        "extras.tagged-objects",
        "extras.tags",
        "ipam.ip-addresses",
        "ipam.prefixes",
        "ipam.services",
        "ipam.vrfs",
        "ipam.vlans",
        "tenancy.contacts",
        "tenancy.contact-groups",
        "tenancy.contact-roles",
        "tenancy.contact-assignments",
        "ipam.vlan-translation-policies",
        "tenancy.tenants",
        "tenancy.tenant-groups",
        "dcim.console-port-templates",
        "dcim.console-server-port-templates",
        "dcim.device-bay-templates",
        "dcim.front-port-templates",
        "dcim.interface-templates",
        "dcim.inventory-item-templates",
        "dcim.power-outlet-templates",
        "dcim.power-port-templates",
        "dcim.rear-port-templates",
        "dcim.port-template-mappings",
        "users.owner-groups",
        "users.owners",
        "virtualization.cluster-groups",
        "virtualization.cluster-types",
        "virtualization.clusters",
        "virtualization.interfaces",
        "virtualization.virtual-disks",
        "virtualization.virtual-machines",
    ];

    #[test]
    fn contact_assignment_targets_match_supported_native_contactable_types() {
        use crate::contexts::inventory::domain::content_type::CONTACT_ASSIGNMENT_OBJECT_TYPES;

        let field = InventoryEntity::ContactAssignment.field("object").unwrap();
        let FieldKind::GenericReference { allowed, .. } = field.kind() else {
            panic!("contact assignment object must be a generic reference");
        };
        let mut expected: Vec<_> = CONTACT_ASSIGNMENT_OBJECT_TYPES
            .iter()
            .filter_map(|token| InventoryEntity::from_content_type_token(token))
            .collect();
        let mut actual = allowed.to_vec();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

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
        assert_eq!(
            CENSUS_SOURCE_KEYS.len(),
            InventoryEntity::core().count() + 2
        );
        assert_eq!(InventoryEntity::all().count(), ENTITIES.len());
    }

    /// The optional plugin namespace, imported only while the provider schema
    /// exposes it. It never joins the mandatory census.
    const OPTIONAL_SOURCE_KEYS: &[&str] = &[
        "data-flows.application-roles",
        "data-flows.applications",
        "data-flows.dataflow-groups",
        "data-flows.dataflows",
        "data-flows.objectalias",
    ];

    #[test]
    fn optional_namespace_is_complete_and_separate_from_the_mandatory_census() {
        let optional: Vec<_> = InventoryEntity::all()
            .filter(|entity| entity.is_optional_source())
            .map(InventoryEntity::source_key)
            .collect();
        assert_eq!(optional, OPTIONAL_SOURCE_KEYS);
        for key in OPTIONAL_SOURCE_KEYS {
            assert!(
                !CENSUS_SOURCE_KEYS.contains(key),
                "{key} must stay outside the mandatory census"
            );
            assert!(
                InventoryEntity::from_source_key(key).is_some(),
                "{key} must resolve to a catalog entity"
            );
        }
        assert_eq!(
            CENSUS_SOURCE_KEYS.len(),
            InventoryEntity::core().count() + 2
        );
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
    fn unavailable_template_fields_never_receive_a_discard_disposition() {
        for (entity, field) in [
            (InventoryEntity::InterfaceTemplate, "parent"),
            (InventoryEntity::InterfaceTemplate, "channels"),
            (InventoryEntity::InterfaceTemplate, "channel_id"),
        ] {
            assert!(matches!(
                entity.source_field_disposition(field),
                Some(SourceFieldDisposition::UnsupportedWhenPopulated(_))
            ));
        }
    }

    #[test]
    fn port_template_relationship_dispositions_track_the_projection() {
        // The REST relationship is projected into dcim.port-template-mappings
        // rows by the source adapter, so the embedded arrays are consumed. The
        // legacy scalar pair has no local target field and stays fail-closed.
        for (entity, field) in [
            (InventoryEntity::FrontPortTemplate, "rear_ports"),
            (InventoryEntity::RearPortTemplate, "front_ports"),
        ] {
            assert!(matches!(
                entity.source_field_disposition(field),
                Some(SourceFieldDisposition::HandledElsewhere(_))
            ));
        }
        for field in ["rear_port", "rear_port_position"] {
            assert!(matches!(
                InventoryEntity::FrontPortTemplate.source_field_disposition(field),
                Some(SourceFieldDisposition::UnsupportedWhenPopulated(_))
            ));
        }
        for field in ["rear_ports", "rear_port", "rear_port_position"] {
            assert!(
                InventoryEntity::FrontPortTemplate.field(field).is_none(),
                "{field} is not a directly mapped front-port target field"
            );
        }
        assert!(
            InventoryEntity::RearPortTemplate
                .field("front_ports")
                .is_none()
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
                    assert!(!allowed.is_empty());
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
            for (field, reason) in meta.entity.field_dispositions() {
                assert!(
                    !reason.reason().is_empty(),
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
                    meta.entity.source_field_disposition(spec.name).is_none(),
                    "{}::{} is both approved and excluded",
                    meta.source_key,
                    spec.name
                );
            }
        }
        // A globally derived field is excluded on every row entity.
        assert!(
            InventoryEntity::Device
                .source_field_disposition("display")
                .is_some()
        );
        assert!(
            InventoryEntity::Interface
                .source_field_disposition("mac_address")
                .is_some()
        );
        // An unknown name is neither approved nor excluded, so the converter
        // blocks it.
        assert!(
            InventoryEntity::Device
                .source_field_disposition("mystery_business_field")
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
                    FieldKind::Junction { .. } | FieldKind::ContentTypeJunction { .. } => continue,
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
        let mappings = InventoryEntity::Service
            .field("port_mappings")
            .expect("port_mappings");
        assert!(!mappings.nullable());
        assert_eq!(mappings.absent(), Absent::Reject);
        let name = InventoryEntity::Manufacturer.field("name").expect("name");
        assert_eq!(name.absent(), Absent::Reject);
    }
}
