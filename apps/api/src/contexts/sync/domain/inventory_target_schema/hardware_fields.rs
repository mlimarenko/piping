//! Hardware source fields backed by migrations 0001, 0005 and 0007.
//! Dependencies live in this same catalogue so relations resolve by provider
//! binding, including the three native module-bay-type junctions.

use super::{
    Absent, FieldKind, FieldSpec, InventoryEntity as E, PgType as P, SourceFieldDisposition as D,
};

const fn scalar(entity: E, name: &'static str, sql_type: P, nullable: bool) -> FieldSpec {
    FieldSpec {
        entity,
        name,
        column: name,
        sql_type,
        nullable,
        default: None,
        kind: FieldKind::Scalar,
        choice_normalized: false,
        absent: if nullable {
            Absent::Preserve
        } else {
            Absent::Reject
        },
    }
}

const fn choice(entity: E, name: &'static str, nullable: bool) -> FieldSpec {
    FieldSpec {
        choice_normalized: true,
        ..scalar(entity, name, P::Text, nullable)
    }
}

const fn fk(
    entity: E,
    name: &'static str,
    column: &'static str,
    target: Option<E>,
    nullable: bool,
) -> FieldSpec {
    FieldSpec {
        column,
        kind: FieldKind::ForeignKey { target },
        ..scalar(entity, name, P::Uuid, nullable)
    }
}

const fn tags(entity: E) -> FieldSpec {
    FieldSpec {
        kind: FieldKind::Tags,
        default: Some("ARRAY[]::text[]"),
        absent: Absent::Preserve,
        ..scalar(entity, "tags", P::TextArray, false)
    }
}

const fn bay_types(entity: E, table: &'static str, local_column: &'static str) -> FieldSpec {
    FieldSpec {
        kind: FieldKind::Junction {
            table,
            local_column,
            remote_column: "modulebaytype_id",
            target: Some(E::ModuleBayType),
        },
        absent: Absent::Preserve,
        ..scalar(entity, "module_bay_types", P::UuidArray, false)
    }
}

macro_rules! modular_template_fields {
    ($name:ident, $entity:ident, $($field:expr),* $(,)?) => {
        pub(super) static $name: &[FieldSpec] = &[
            scalar(E::$entity, "name", P::Text, false),
            scalar(E::$entity, "label", P::Text, false),
            scalar(E::$entity, "description", P::Text, false),
            fk(E::$entity, "device_type", "device_type_id", Some(E::DeviceType), true),
            fk(E::$entity, "module_type", "module_type_id", Some(E::ModuleType), true),
            $($field,)*
        ];
    };
}

modular_template_fields!(
    CONSOLE_PORT_TEMPLATE,
    ConsolePortTemplate,
    choice(E::ConsolePortTemplate, "type", true)
);
modular_template_fields!(
    CONSOLE_SERVER_PORT_TEMPLATE,
    ConsoleServerPortTemplate,
    choice(E::ConsoleServerPortTemplate, "type", true)
);
modular_template_fields!(
    POWER_PORT_TEMPLATE,
    PowerPortTemplate,
    choice(E::PowerPortTemplate, "type", true),
    scalar(E::PowerPortTemplate, "maximum_draw", P::Integer, true),
    scalar(E::PowerPortTemplate, "allocated_draw", P::Integer, true)
);
modular_template_fields!(
    POWER_OUTLET_TEMPLATE,
    PowerOutletTemplate,
    choice(E::PowerOutletTemplate, "type", true),
    scalar(E::PowerOutletTemplate, "color", P::Text, false),
    choice(E::PowerOutletTemplate, "feed_leg", true),
    fk(
        E::PowerOutletTemplate,
        "power_port",
        "power_port_id",
        Some(E::PowerPortTemplate),
        true
    )
);
modular_template_fields!(
    INTERFACE_TEMPLATE,
    InterfaceTemplate,
    choice(E::InterfaceTemplate, "type", false),
    scalar(E::InterfaceTemplate, "enabled", P::Bool, false),
    scalar(E::InterfaceTemplate, "mgmt_only", P::Bool, false),
    fk(
        E::InterfaceTemplate,
        "bridge",
        "bridge_id",
        Some(E::InterfaceTemplate),
        true
    ),
    choice(E::InterfaceTemplate, "poe_mode", true),
    choice(E::InterfaceTemplate, "poe_type", true),
    choice(E::InterfaceTemplate, "rf_role", true)
);
modular_template_fields!(
    FRONT_PORT_TEMPLATE,
    FrontPortTemplate,
    choice(E::FrontPortTemplate, "type", false),
    scalar(E::FrontPortTemplate, "color", P::Text, false),
    scalar(E::FrontPortTemplate, "positions", P::SmallInt, false)
);
modular_template_fields!(
    REAR_PORT_TEMPLATE,
    RearPortTemplate,
    choice(E::RearPortTemplate, "type", false),
    scalar(E::RearPortTemplate, "color", P::Text, false),
    scalar(E::RearPortTemplate, "positions", P::SmallInt, false)
);

pub(super) static DEVICE_BAY_TEMPLATE: &[FieldSpec] = &[
    scalar(E::DeviceBayTemplate, "name", P::Text, false),
    scalar(E::DeviceBayTemplate, "label", P::Text, false),
    scalar(E::DeviceBayTemplate, "description", P::Text, false),
    FieldSpec {
        default: Some("true"),
        absent: Absent::Preserve,
        ..scalar(E::DeviceBayTemplate, "enabled", P::Bool, false)
    },
    fk(
        E::DeviceBayTemplate,
        "device_type",
        "device_type_id",
        Some(E::DeviceType),
        false,
    ),
];
pub(super) static PORT_TEMPLATE_MAPPING: &[FieldSpec] = &[
    scalar(
        E::PortTemplateMapping,
        "front_port_position",
        P::SmallInt,
        false,
    ),
    scalar(
        E::PortTemplateMapping,
        "rear_port_position",
        P::SmallInt,
        false,
    ),
    fk(
        E::PortTemplateMapping,
        "device_type",
        "device_type_id",
        Some(E::DeviceType),
        true,
    ),
    fk(
        E::PortTemplateMapping,
        "module_type",
        "module_type_id",
        Some(E::ModuleType),
        true,
    ),
    fk(
        E::PortTemplateMapping,
        "front_port",
        "front_port_id",
        Some(E::FrontPortTemplate),
        false,
    ),
    fk(
        E::PortTemplateMapping,
        "rear_port",
        "rear_port_id",
        Some(E::RearPortTemplate),
        false,
    ),
];
pub(super) static INVENTORY_ITEM_TEMPLATE: &[FieldSpec] = &[
    scalar(E::InventoryItemTemplate, "name", P::Text, false),
    scalar(E::InventoryItemTemplate, "label", P::Text, false),
    scalar(E::InventoryItemTemplate, "description", P::Text, false),
    scalar(E::InventoryItemTemplate, "part_id", P::Text, false),
    fk(
        E::InventoryItemTemplate,
        "device_type",
        "device_type_id",
        Some(E::DeviceType),
        false,
    ),
    fk(
        E::InventoryItemTemplate,
        "parent",
        "parent_id",
        Some(E::InventoryItemTemplate),
        true,
    ),
    fk(
        E::InventoryItemTemplate,
        "manufacturer",
        "manufacturer_id",
        Some(E::Manufacturer),
        true,
    ),
    fk(E::InventoryItemTemplate, "role", "role_id", None, true),
    FieldSpec {
        column: "component_id",
        kind: FieldKind::GenericReference {
            type_column: "component_type_id",
            source_type_field: "component_type",
            allowed: &[
                E::ConsolePortTemplate,
                E::ConsoleServerPortTemplate,
                E::PowerPortTemplate,
                E::PowerOutletTemplate,
                E::InterfaceTemplate,
                E::FrontPortTemplate,
                E::RearPortTemplate,
                E::CoolingIntakeTemplate,
                E::CoolingOutflowTemplate,
            ],
        },
        ..scalar(E::InventoryItemTemplate, "component", P::Uuid, true)
    },
];

pub(super) static RACK_GROUP: &[FieldSpec] = &[
    super::owner_field(E::RackGroup),
    scalar(E::RackGroup, "name", P::Text, false),
    scalar(E::RackGroup, "slug", P::Text, false),
    scalar(E::RackGroup, "description", P::Text, false),
    scalar(E::RackGroup, "comments", P::Text, false),
    tags(E::RackGroup),
];
pub(super) static CABLE_BUNDLE: &[FieldSpec] = &[
    super::owner_field(E::CableBundle),
    scalar(E::CableBundle, "name", P::Text, false),
    scalar(E::CableBundle, "description", P::Text, false),
    scalar(E::CableBundle, "comments", P::Text, false),
    tags(E::CableBundle),
];
pub(super) static MODULE_BAY_TYPE: &[FieldSpec] = &[
    super::owner_field(E::ModuleBayType),
    scalar(E::ModuleBayType, "name", P::Text, false),
    scalar(E::ModuleBayType, "slug", P::Text, false),
    scalar(E::ModuleBayType, "color", P::Text, false),
    scalar(E::ModuleBayType, "description", P::Text, false),
    scalar(E::ModuleBayType, "comments", P::Text, false),
    fk(
        E::ModuleBayType,
        "manufacturer",
        "manufacturer_id",
        Some(E::Manufacturer),
        true,
    ),
    tags(E::ModuleBayType),
];
pub(super) static COOLING_SOURCE: &[FieldSpec] = &[
    super::owner_field(E::CoolingSource),
    scalar(E::CoolingSource, "name", P::Text, false),
    choice(E::CoolingSource, "type", false),
    choice(E::CoolingSource, "status", false),
    choice(E::CoolingSource, "fluid_type", true),
    scalar(E::CoolingSource, "cooling_capacity", P::Numeric, true),
    fk(E::CoolingSource, "site", "site_id", Some(E::Site), false),
    fk(
        E::CoolingSource,
        "location",
        "location_id",
        Some(E::Location),
        true,
    ),
    scalar(E::CoolingSource, "description", P::Text, false),
    scalar(E::CoolingSource, "comments", P::Text, false),
    tags(E::CoolingSource),
];
pub(super) static COOLING_FEED: &[FieldSpec] = &[
    super::owner_field(E::CoolingFeed),
    scalar(E::CoolingFeed, "name", P::Text, false),
    choice(E::CoolingFeed, "status", false),
    scalar(E::CoolingFeed, "cooling_capacity", P::Numeric, true),
    scalar(E::CoolingFeed, "max_flow", P::Numeric, true),
    choice(E::CoolingFeed, "max_flow_unit", true),
    fk(
        E::CoolingFeed,
        "cooling_source",
        "cooling_source_id",
        Some(E::CoolingSource),
        false,
    ),
    fk(E::CoolingFeed, "rack", "rack_id", Some(E::Rack), true),
    fk(E::CoolingFeed, "tenant", "tenant_id", Some(E::Tenant), true),
    scalar(E::CoolingFeed, "description", P::Text, false),
    scalar(E::CoolingFeed, "comments", P::Text, false),
    tags(E::CoolingFeed),
];
pub(super) static COOLING_INTAKE: &[FieldSpec] = &[
    super::owner_field(E::CoolingIntake),
    scalar(E::CoolingIntake, "name", P::Text, false),
    scalar(E::CoolingIntake, "label", P::Text, false),
    choice(E::CoolingIntake, "type", true),
    scalar(E::CoolingIntake, "diameter", P::Numeric, true),
    choice(E::CoolingIntake, "diameter_unit", true),
    scalar(E::CoolingIntake, "max_flow", P::Numeric, true),
    choice(E::CoolingIntake, "max_flow_unit", true),
    fk(
        E::CoolingIntake,
        "device",
        "device_id",
        Some(E::Device),
        false,
    ),
    fk(
        E::CoolingIntake,
        "module",
        "module_id",
        Some(E::Module),
        true,
    ),
    fk(
        E::CoolingIntake,
        "cooling_outflow",
        "cooling_outflow_id",
        Some(E::CoolingOutflow),
        true,
    ),
    scalar(E::CoolingIntake, "description", P::Text, false),
    tags(E::CoolingIntake),
];
pub(super) static COOLING_OUTFLOW: &[FieldSpec] = &[
    super::owner_field(E::CoolingOutflow),
    scalar(E::CoolingOutflow, "name", P::Text, false),
    scalar(E::CoolingOutflow, "label", P::Text, false),
    choice(E::CoolingOutflow, "type", true),
    scalar(E::CoolingOutflow, "diameter", P::Numeric, true),
    choice(E::CoolingOutflow, "diameter_unit", true),
    fk(
        E::CoolingOutflow,
        "device",
        "device_id",
        Some(E::Device),
        false,
    ),
    fk(
        E::CoolingOutflow,
        "module",
        "module_id",
        Some(E::Module),
        true,
    ),
    fk(
        E::CoolingOutflow,
        "cooling_intake",
        "cooling_intake_id",
        Some(E::CoolingIntake),
        true,
    ),
    scalar(E::CoolingOutflow, "description", P::Text, false),
    tags(E::CoolingOutflow),
];
pub(super) static COOLING_INTAKE_TEMPLATE: &[FieldSpec] = &[
    scalar(E::CoolingIntakeTemplate, "name", P::Text, false),
    scalar(E::CoolingIntakeTemplate, "label", P::Text, false),
    choice(E::CoolingIntakeTemplate, "type", true),
    scalar(E::CoolingIntakeTemplate, "diameter", P::Numeric, true),
    choice(E::CoolingIntakeTemplate, "diameter_unit", true),
    scalar(E::CoolingIntakeTemplate, "max_flow", P::Numeric, true),
    choice(E::CoolingIntakeTemplate, "max_flow_unit", true),
    fk(
        E::CoolingIntakeTemplate,
        "device_type",
        "device_type_id",
        Some(E::DeviceType),
        true,
    ),
    fk(
        E::CoolingIntakeTemplate,
        "module_type",
        "module_type_id",
        Some(E::ModuleType),
        true,
    ),
    scalar(E::CoolingIntakeTemplate, "description", P::Text, false),
];
pub(super) static COOLING_OUTFLOW_TEMPLATE: &[FieldSpec] = &[
    scalar(E::CoolingOutflowTemplate, "name", P::Text, false),
    scalar(E::CoolingOutflowTemplate, "label", P::Text, false),
    choice(E::CoolingOutflowTemplate, "type", true),
    scalar(E::CoolingOutflowTemplate, "diameter", P::Numeric, true),
    choice(E::CoolingOutflowTemplate, "diameter_unit", true),
    fk(
        E::CoolingOutflowTemplate,
        "device_type",
        "device_type_id",
        Some(E::DeviceType),
        true,
    ),
    fk(
        E::CoolingOutflowTemplate,
        "module_type",
        "module_type_id",
        Some(E::ModuleType),
        true,
    ),
    fk(
        E::CoolingOutflowTemplate,
        "cooling_intake",
        "cooling_intake_id",
        Some(E::CoolingIntakeTemplate),
        true,
    ),
    scalar(E::CoolingOutflowTemplate, "description", P::Text, false),
];

pub(super) static LOCATION: &[FieldSpec] = &[
    super::owner_field(E::Location),
    scalar(E::Location, "name", P::Text, false),
    scalar(E::Location, "slug", P::Text, false),
    scalar(E::Location, "description", P::Text, false),
    scalar(E::Location, "comments", P::Text, false),
    scalar(E::Location, "facility", P::Text, false),
    choice(E::Location, "status", false),
    fk(E::Location, "parent", "parent_id", Some(E::Location), true),
    fk(E::Location, "site", "site_id", Some(E::Site), false),
    fk(E::Location, "tenant", "tenant_id", Some(E::Tenant), true),
    tags(E::Location),
];
pub(super) static RACK: &[FieldSpec] = &[
    super::owner_field(E::Rack),
    scalar(E::Rack, "name", P::Text, false),
    scalar(E::Rack, "facility_id", P::Text, true),
    choice(E::Rack, "status", false),
    scalar(E::Rack, "serial", P::Text, false),
    scalar(E::Rack, "asset_tag", P::Text, true),
    choice(E::Rack, "form_factor", true),
    FieldSpec {
        choice_normalized: true,
        ..scalar(E::Rack, "width", P::SmallInt, false)
    },
    scalar(E::Rack, "u_height", P::SmallInt, false),
    scalar(E::Rack, "desc_units", P::Bool, false),
    scalar(E::Rack, "starting_unit", P::SmallInt, false),
    scalar(E::Rack, "outer_width", P::SmallInt, true),
    scalar(E::Rack, "outer_depth", P::SmallInt, true),
    scalar(E::Rack, "outer_height", P::SmallInt, true),
    choice(E::Rack, "outer_unit", true),
    scalar(E::Rack, "mounting_depth", P::SmallInt, true),
    scalar(E::Rack, "weight", P::Numeric, true),
    scalar(E::Rack, "max_weight", P::Integer, true),
    choice(E::Rack, "weight_unit", true),
    choice(E::Rack, "airflow", true),
    choice(E::Rack, "cooling_capability", true),
    scalar(E::Rack, "cooling_capacity", P::Numeric, true),
    scalar(E::Rack, "description", P::Text, false),
    scalar(E::Rack, "comments", P::Text, false),
    fk(E::Rack, "site", "site_id", Some(E::Site), false),
    fk(E::Rack, "location", "location_id", Some(E::Location), true),
    fk(E::Rack, "tenant", "tenant_id", Some(E::Tenant), true),
    fk(E::Rack, "group", "group_id", Some(E::RackGroup), true),
    fk(E::Rack, "role", "role_id", None, true),
    fk(E::Rack, "rack_type", "rack_type_id", None, true),
    tags(E::Rack),
];
pub(super) static CABLE: &[FieldSpec] = &[
    super::owner_field(E::Cable),
    choice(E::Cable, "type", true),
    choice(E::Cable, "status", false),
    scalar(E::Cable, "label", P::Text, false),
    scalar(E::Cable, "color", P::Text, false),
    scalar(E::Cable, "length", P::Numeric, true),
    choice(E::Cable, "length_unit", true),
    scalar(E::Cable, "description", P::Text, false),
    scalar(E::Cable, "comments", P::Text, false),
    FieldSpec {
        absent: Absent::EmptyText,
        ..choice(E::Cable, "profile", false)
    },
    fk(E::Cable, "tenant", "tenant_id", Some(E::Tenant), true),
    fk(E::Cable, "bundle", "bundle_id", Some(E::CableBundle), true),
    tags(E::Cable),
];
pub(super) static MODULE_TYPE: &[FieldSpec] = &[
    super::owner_field(E::ModuleType),
    scalar(E::ModuleType, "model", P::Text, false),
    scalar(E::ModuleType, "part_number", P::Text, false),
    scalar(E::ModuleType, "description", P::Text, false),
    scalar(E::ModuleType, "comments", P::Text, false),
    scalar(E::ModuleType, "weight", P::Numeric, true),
    choice(E::ModuleType, "weight_unit", true),
    choice(E::ModuleType, "airflow", true),
    choice(E::ModuleType, "cooling_method", true),
    scalar(E::ModuleType, "end_of_life", P::Date, true),
    FieldSpec {
        column: "attribute_data",
        ..scalar(E::ModuleType, "attributes", P::Jsonb, true)
    },
    fk(
        E::ModuleType,
        "manufacturer",
        "manufacturer_id",
        Some(E::Manufacturer),
        false,
    ),
    fk(
        E::ModuleType,
        "profile",
        "profile_id",
        Some(E::ModuleTypeProfile),
        true,
    ),
    bay_types(
        E::ModuleType,
        "dcim_moduletype_module_bay_types",
        "moduletype_id",
    ),
    tags(E::ModuleType),
];
pub(super) static MODULE_BAY_TEMPLATE: &[FieldSpec] = &[
    FieldSpec {
        default: Some("true"),
        absent: Absent::Preserve,
        ..scalar(E::ModuleBayTemplate, "enabled", P::Bool, false)
    },
    scalar(E::ModuleBayTemplate, "name", P::Text, false),
    scalar(E::ModuleBayTemplate, "label", P::Text, false),
    scalar(E::ModuleBayTemplate, "position", P::Text, false),
    scalar(E::ModuleBayTemplate, "description", P::Text, false),
    fk(
        E::ModuleBayTemplate,
        "device_type",
        "device_type_id",
        Some(E::DeviceType),
        true,
    ),
    fk(
        E::ModuleBayTemplate,
        "module_type",
        "module_type_id",
        Some(E::ModuleType),
        true,
    ),
    bay_types(
        E::ModuleBayTemplate,
        "dcim_modulebaytemplate_module_bay_types",
        "modulebaytemplate_id",
    ),
];
pub(super) static MODULE_BAY: &[FieldSpec] = &[
    super::owner_field(E::ModuleBay),
    FieldSpec {
        default: Some("true"),
        absent: Absent::Preserve,
        ..scalar(E::ModuleBay, "enabled", P::Bool, false)
    },
    scalar(E::ModuleBay, "name", P::Text, false),
    scalar(E::ModuleBay, "label", P::Text, false),
    scalar(E::ModuleBay, "position", P::Text, false),
    scalar(E::ModuleBay, "description", P::Text, false),
    fk(E::ModuleBay, "device", "device_id", Some(E::Device), false),
    fk(E::ModuleBay, "module", "module_id", Some(E::Module), true),
    fk(
        E::ModuleBay,
        "parent",
        "parent_id",
        Some(E::ModuleBay),
        true,
    ),
    bay_types(
        E::ModuleBay,
        "dcim_modulebay_module_bay_types",
        "modulebay_id",
    ),
    tags(E::ModuleBay),
];
pub(super) static MODULE: &[FieldSpec] = &[
    super::owner_field(E::Module),
    scalar(E::Module, "serial", P::Text, false),
    scalar(E::Module, "asset_tag", P::Text, true),
    scalar(E::Module, "description", P::Text, false),
    scalar(E::Module, "comments", P::Text, false),
    scalar(E::Module, "local_context_data", P::Jsonb, true),
    choice(E::Module, "status", false),
    fk(E::Module, "device", "device_id", Some(E::Device), false),
    fk(
        E::Module,
        "module_bay",
        "module_bay_id",
        Some(E::ModuleBay),
        false,
    ),
    fk(
        E::Module,
        "module_type",
        "module_type_id",
        Some(E::ModuleType),
        false,
    ),
    tags(E::Module),
];

// Unlisted populated fields still block assessment. These are serializer
// metadata, reverse counters or separately validated custom-field values.
macro_rules! metadata {
    ($($extra:expr),* $(,)?) => { &[
    ("id", D::HandledElsewhere("provider-qualified external identity")),
    ("created", D::HandledElsewhere("SourceTimestamps writer")),
    ("last_updated", D::HandledElsewhere("SourceTimestamps writer")),
    ("display", D::Derived("serializer label")),
    ("url", D::Derived("source API URL")),
    ("display_url", D::Derived("source UI URL")),
        $($extra,)*
    ] };
}

macro_rules! object_metadata {
    ($($extra:expr),* $(,)?) => { metadata![
        ("custom_fields", D::HandledElsewhere("validated native custom-field writer")),
        $($extra,)*
    ] };
}

const COMMON: &[(&str, D)] = object_metadata![];

pub(super) fn dispositions(entity: E) -> Option<&'static [(&'static str, D)]> {
    match entity {
        E::TenantGroup => Some(object_metadata![
            (
                "_depth",
                D::Derived("TenantGroup.parent tree depth rebuilt by native tree owner")
            ),
            ("tenant_count", D::Derived("reverse Tenant.group aggregate")),
        ]),
        E::InterfaceTemplate => Some(metadata![
            (
                "parent",
                D::UnsupportedWhenPopulated("interface template parent has no local column")
            ),
            (
                "channels",
                D::UnsupportedWhenPopulated("interface template channels have no local relation")
            ),
            (
                "channel_id",
                D::UnsupportedWhenPopulated("interface template channel has no local column")
            ),
        ]),
        E::FrontPortTemplate => Some(metadata![
            (
                "rear_ports",
                D::HandledElsewhere(
                    "projected into dcim.port-template-mappings records from the canonical embedded relationship"
                )
            ),
            (
                "rear_port",
                D::UnsupportedWhenPopulated(
                    "legacy scalar rear_port is not the canonical 4.7 REST shape; the relationship is projected from rear_ports"
                )
            ),
            (
                "rear_port_position",
                D::UnsupportedWhenPopulated(
                    "legacy scalar rear_port_position is not the canonical 4.7 REST shape; the relationship is projected from rear_ports"
                )
            ),
        ]),
        E::RearPortTemplate => Some(metadata![(
            "front_ports",
            D::HandledElsewhere(
                "reverse view of the dcim.port-template-mappings records projected from dcim.front-port-templates rear_ports"
            )
        ),]),
        E::InventoryItemTemplate => Some(metadata![
            ("_depth", D::Derived("inventory item template parent depth")),
            (
                "component_id",
                D::HandledElsewhere("generic component reference identity")
            ),
            (
                "component_type",
                D::HandledElsewhere("generic component reference type")
            ),
        ]),
        E::PortTemplateMapping
        | E::ConsolePortTemplate
        | E::ConsoleServerPortTemplate
        | E::DeviceBayTemplate
        | E::PowerOutletTemplate
        | E::PowerPortTemplate => Some(metadata![]),
        E::CoolingIntakeTemplate | E::CoolingOutflowTemplate | E::ModuleBayTemplate => {
            Some(metadata![])
        }
        E::RackGroup => Some(object_metadata![(
            "rack_count",
            D::Derived("reverse Rack.group aggregate")
        )]),
        E::CableBundle => Some(object_metadata![(
            "cable_count",
            D::Derived("reverse Cable.bundle aggregate")
        )]),
        E::CoolingSource => Some(object_metadata![(
            "coolingfeed_count",
            D::Derived("reverse CoolingFeed.cooling_source aggregate")
        )]),
        E::Location => Some(object_metadata![
            (
                "_depth",
                D::Derived("Location.parent tree depth rebuilt by native tree owner")
            ),
            ("rack_count", D::Derived("reverse Rack.location aggregate")),
            (
                "device_count",
                D::Derived("reverse Device.location aggregate")
            ),
            (
                "prefix_count",
                D::Derived("reverse Prefix.scope location aggregate")
            ),
        ]),
        E::Rack => Some(object_metadata![
            ("device_count", D::Derived("reverse Device.rack aggregate")),
            (
                "powerfeed_count",
                D::Derived("reverse PowerFeed.rack aggregate")
            ),
        ]),
        E::Cable => Some(object_metadata![
            (
                "a_terminations",
                D::HandledElsewhere(
                    "validated against canonical Interface.cable and cable_end A; partial endpoint snapshots rejected"
                )
            ),
            (
                "b_terminations",
                D::HandledElsewhere(
                    "validated against canonical Interface.cable and cable_end B; partial endpoint snapshots rejected"
                )
            ),
        ]),
        E::ModuleType => Some(object_metadata![
            (
                "module_count",
                D::Derived("reverse Module.module_type installation aggregate")
            ),
            (
                "console_port_template_count",
                D::Derived("reverse ConsolePortTemplate.module_type aggregate")
            ),
            (
                "console_server_port_template_count",
                D::Derived("reverse ConsoleServerPortTemplate.module_type aggregate")
            ),
            (
                "power_port_template_count",
                D::Derived("reverse PowerPortTemplate.module_type aggregate")
            ),
            (
                "power_outlet_template_count",
                D::Derived("reverse PowerOutletTemplate.module_type aggregate")
            ),
            (
                "cooling_intake_template_count",
                D::Derived("reverse CoolingIntakeTemplate.module_type aggregate")
            ),
            (
                "cooling_outflow_template_count",
                D::Derived("reverse CoolingOutflowTemplate.module_type aggregate")
            ),
            (
                "interface_template_count",
                D::Derived("reverse InterfaceTemplate.module_type aggregate")
            ),
            (
                "front_port_template_count",
                D::Derived("reverse FrontPortTemplate.module_type aggregate")
            ),
            (
                "rear_port_template_count",
                D::Derived("reverse RearPortTemplate.module_type aggregate")
            ),
            (
                "module_bay_template_count",
                D::Derived("reverse ModuleBayTemplate.module_type aggregate")
            ),
        ]),
        E::Module => Some(object_metadata![(
            "is_bay_compatible",
            D::Derived(
                "Module.module_type and Module.module_bay module_bay_types compatibility projection"
            )
        )]),
        E::ModuleBay => Some(object_metadata![
            (
                "installed_module",
                D::HandledElsewhere("reverse relation owned by Module.module_bay")
            ),
            ("_occupied", D::Derived("reverse module occupancy")),
            (
                "is_module_compatible",
                D::Derived(
                    "installed Module.module_type and ModuleBay.module_bay_types compatibility projection"
                )
            ),
        ]),
        entity if entity.is_hardware() => Some(COMMON),
        _ => None,
    }
}

pub(super) fn internal_columns(
    entity: E,
) -> Option<&'static [(&'static str, &'static str, &'static str)]> {
    match entity {
        E::InventoryItemTemplate => Some(&[
            ("lft", "literal", "0"),
            ("rght", "literal", "0"),
            ("tree_id", "literal", "0"),
            ("level", "literal", "0"),
        ]),
        E::InterfaceTemplate => Some(&[("_name", "copy", "name")]),
        E::PortTemplateMapping
        | E::ConsolePortTemplate
        | E::ConsoleServerPortTemplate
        | E::DeviceBayTemplate
        | E::FrontPortTemplate
        | E::PowerOutletTemplate
        | E::PowerPortTemplate
        | E::RearPortTemplate => Some(&[]),
        E::Location | E::ModuleBay => Some(&[
            ("custom_field_data", "literal", "'{}'::jsonb"),
            ("lft", "literal", "0"),
            ("rght", "literal", "0"),
            ("tree_id", "literal", "0"),
            ("level", "literal", "0"),
        ]),
        E::ModuleType => Some(&[
            ("custom_field_data", "literal", "'{}'::jsonb"),
            ("module_count", "literal", "0"),
        ]),
        E::RackGroup
        | E::CableBundle
        | E::ModuleBayType
        | E::CoolingSource
        | E::CoolingFeed
        | E::CoolingIntake
        | E::CoolingOutflow
        | E::Rack
        | E::Cable
        | E::Module => Some(&[("custom_field_data", "literal", "'{}'::jsonb")]),
        E::CoolingIntakeTemplate | E::CoolingOutflowTemplate | E::ModuleBayTemplate => Some(&[]),
        _ => None,
    }
}
