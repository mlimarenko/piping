//! Closed field tables for the original local inventory target resources.
//!
//! Every row is verified against the migrated PostgreSQL schema
//! (`0001_init.sql` and its forward migrations); no caller may name a table
//! or column that is not listed here. Hardware owners added after the original
//! census live in the sibling `hardware_fields` catalogue.

use super::{Absent, FieldKind, FieldSpec, InventoryEntity, PgType, SourceFieldDisposition};

macro_rules! field {
    ($entity:ident, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve) => {
        FieldSpec {
            entity: InventoryEntity::$entity,
            name: "tags",
            column: "tags",
            sql_type: PgType::TextArray,
            nullable: false,
            default: Some("ARRAY[]::text[]"),
            kind: FieldKind::Tags,
            choice_normalized: false,
            absent: Absent::Preserve,
        }
    };
    ($entity:ident, $name:literal, $column:literal, $type:ident, $nullable:expr, $kind:expr, $choice:expr, $absent:ident) => {
        FieldSpec {
            entity: InventoryEntity::$entity,
            name: $name,
            column: $column,
            sql_type: PgType::$type,
            nullable: $nullable,
            default: None,
            kind: $kind,
            choice_normalized: $choice,
            absent: Absent::$absent,
        }
    };
}

#[rustfmt::skip]
pub(super) static VLAN_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Vlan),
    field!(Vlan, "vid", "vid", SmallInt, false, FieldKind::Scalar, false, Reject),
    field!(Vlan, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(Vlan, "status", "status", Text, false, FieldKind::Scalar, true, Reject),
    field!(Vlan, "description", "description", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Vlan, "comments", "comments", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Vlan, "group", "group_id", Uuid, true, FieldKind::ForeignKey { target: None }, false, Preserve),
    field!(Vlan, "role", "role_id", Uuid, true, FieldKind::ForeignKey { target: None }, false, Preserve),
    field!(Vlan, "site", "site_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Site) }, false, Preserve),
    field!(Vlan, "tenant", "tenant_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, false, Preserve),
    field!(Vlan, "qinq_role", "qinq_role", Text, true, FieldKind::Scalar, true, Preserve),
    field!(Vlan, "qinq_svlan", "qinq_svlan_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Vlan) }, false, Preserve),
    field!(Vlan, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static CONTACT_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Contact),
    field!(Contact, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(Contact, "title", "title", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Contact, "phone", "phone", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Contact, "email", "email", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Contact, "address", "address", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Contact, "comments", "comments", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Contact, "link", "link", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Contact, "description", "description", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(Contact, "groups", "tenancy_contact_groups", Uuid, true, FieldKind::Junction { table: "tenancy_contact_groups", local_column: "contact_id", remote_column: "contactgroup_id", target: Some(InventoryEntity::ContactGroup) }, false, Preserve),
    field!(Contact, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static CONTACT_GROUP_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::ContactGroup),
    field!(ContactGroup, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(ContactGroup, "slug", "slug", Text, false, FieldKind::Scalar, false, Reject),
    field!(ContactGroup, "description", "description", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(ContactGroup, "comments", "comments", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(ContactGroup, "parent", "parent_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::ContactGroup) }, false, Preserve),
    field!(ContactGroup, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static CONTACT_ROLE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::ContactRole),
    field!(ContactRole, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(ContactRole, "slug", "slug", Text, false, FieldKind::Scalar, false, Reject),
    field!(ContactRole, "description", "description", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(ContactRole, "comments", "comments", Text, false, FieldKind::Scalar, false, EmptyText),
    field!(ContactRole, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static CONTACT_ASSIGNMENT_FIELDS: &[FieldSpec] = &[
    field!(ContactAssignment, "contact", "contact_id", Uuid, false, FieldKind::ForeignKey { target: Some(InventoryEntity::Contact) }, false, Reject),
    field!(ContactAssignment, "role", "role_id", Uuid, false, FieldKind::ForeignKey { target: Some(InventoryEntity::ContactRole) }, false, Reject),
    field!(ContactAssignment, "priority", "priority", Text, true, FieldKind::Scalar, true, Preserve),
    field!(ContactAssignment, "object", "object_id", Uuid, false, FieldKind::GenericReference { type_column: "object_type_id", source_type_field: "object_type", allowed: &[InventoryEntity::Site, InventoryEntity::Region, InventoryEntity::Device, InventoryEntity::Tenant, InventoryEntity::Cluster, InventoryEntity::VirtualMachine] }, false, Reject),
    field!(ContactAssignment, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static OWNER_GROUP_FIELDS: &[FieldSpec] = &[
    FieldSpec { entity: InventoryEntity::OwnerGroup, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::OwnerGroup, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: Some("''::character varying"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::EmptyText },
];

#[rustfmt::skip]
pub(super) static OWNER_FIELDS: &[FieldSpec] = &[
    FieldSpec { entity: InventoryEntity::Owner, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Owner, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: Some("''::character varying"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::EmptyText },
    FieldSpec { entity: InventoryEntity::Owner, name: "group", column: "group_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::OwnerGroup) }, choice_normalized: false, absent: Absent::Preserve },
];

// Keep this catalogue's existing row-per-field layout for reviewing field contracts.
#[rustfmt::skip]
pub(super) static VLAN_TRANSLATION_POLICY_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::VlanTranslationPolicy),
    FieldSpec { entity: InventoryEntity::VlanTranslationPolicy, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VlanTranslationPolicy, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::EmptyText },
    FieldSpec { entity: InventoryEntity::VlanTranslationPolicy, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::EmptyText },
    FieldSpec { entity: InventoryEntity::VlanTranslationPolicy, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static DEVICE_ROLE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::DeviceRole),
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "color", column: "color", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "config_template", column: "config_template_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::DeviceRole) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceRole, name: "vm_role", column: "vm_role", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
];

#[rustfmt::skip]
pub(super) static DEVICE_TYPE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::DeviceType),
    FieldSpec { entity: InventoryEntity::DeviceType, name: "cooling_method", column: "cooling_method", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "end_of_life", column: "end_of_life", sql_type: PgType::Date, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "airflow", column: "airflow", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "default_platform", column: "default_platform_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Platform) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "exclude_from_utilization", column: "exclude_from_utilization", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "front_image", column: "front_image", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::EmptyText },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "is_full_depth", column: "is_full_depth", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "manufacturer", column: "manufacturer_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Manufacturer) }, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "model", column: "model", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "part_number", column: "part_number", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "rear_image", column: "rear_image", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::EmptyText },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "subdevice_role", column: "subdevice_role", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "u_height", column: "u_height", sql_type: PgType::Numeric, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "weight", column: "weight", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::DeviceType, name: "weight_unit", column: "weight_unit", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static DEVICE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Device),
    FieldSpec { entity: InventoryEntity::Device, name: "cooling_method", column: "cooling_method", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "airflow", column: "airflow", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "asset_tag", column: "asset_tag", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "cluster", column: "cluster_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Cluster) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Device, name: "config_template", column: "config_template_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Device, name: "device_type", column: "device_type_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::DeviceType) }, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Device, name: "face", column: "face", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "latitude", column: "latitude", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "local_context_data", column: "local_context_data", sql_type: PgType::Jsonb, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "location", column: "location_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Location) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "longitude", column: "longitude", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "name", column: "name", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "oob_ip", column: "oob_ip_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::IpAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "platform", column: "platform_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Platform) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "position", column: "position", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "primary_ip4", column: "primary_ip4_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::IpAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "primary_ip6", column: "primary_ip6_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::IpAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "rack", column: "rack_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Rack) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "role", column: "role_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::DeviceRole) }, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Device, name: "serial", column: "serial", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Device, name: "site", column: "site_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Site) }, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Device, name: "status", column: "status", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Device, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "tenant", column: "tenant_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "vc_position", column: "vc_position", sql_type: PgType::Integer, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "vc_priority", column: "vc_priority", sql_type: PgType::SmallInt, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Device, name: "virtual_chassis", column: "virtual_chassis_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static INTERFACE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Interface),
    FieldSpec { entity: InventoryEntity::Interface, name: "mode", column: "mode", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "duplex", column: "duplex", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "poe_mode", column: "poe_mode", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "poe_type", column: "poe_type", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "rf_role", column: "rf_role", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "rf_channel", column: "rf_channel", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "rf_channel_frequency", column: "rf_channel_frequency", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "rf_channel_width", column: "rf_channel_width", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "tx_power", column: "tx_power", sql_type: PgType::SmallInt, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "wwn", column: "wwn", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "channel_id", column: "channel_id", sql_type: PgType::SmallInt, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "channels", column: "channels", sql_type: PgType::SmallInt, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "bridge", column: "bridge_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Interface) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "cable", column: "cable_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Cable) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "cable_end", column: "cable_end", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "cable_connector", column: "cable_connector", sql_type: PgType::SmallInt, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "cable_positions", column: "cable_positions", sql_type: PgType::SmallIntArray, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Interface, name: "device", column: "device_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Device) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "enabled", column: "enabled", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Interface, name: "label", column: "label", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Interface, name: "lag", column: "lag_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Interface) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "mark_connected", column: "mark_connected", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Interface, name: "mgmt_only", column: "mgmt_only", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Interface, name: "module", column: "module_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Module) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "mtu", column: "mtu", sql_type: PgType::Integer, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Interface, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Interface) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "primary_mac_address", column: "primary_mac_address_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::MacAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "qinq_svlan", column: "qinq_svlan_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "speed", column: "speed", sql_type: PgType::BigInt, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "tagged_vlans", column: "dcim_interface_tagged_vlans", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::Junction { table: "dcim_interface_tagged_vlans", local_column: "interface_id", remote_column: "vlan_id", target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "type", column: "type", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Interface, name: "untagged_vlan", column: "untagged_vlan_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "vlan_translation_policy", column: "vlan_translation_policy_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::VlanTranslationPolicy) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Interface, name: "vrf", column: "vrf_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Vrf) }, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static INVENTORY_ITEM_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::InventoryItem),
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "asset_tag", column: "asset_tag", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "component", column: "component_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::GenericReference { type_column: "component_type_id", source_type_field: "component_type", allowed: &[InventoryEntity::Interface] }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "device", column: "device_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Device) }, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "discovered", column: "discovered", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "label", column: "label", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "manufacturer", column: "manufacturer_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Manufacturer) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::InventoryItem) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "part_id", column: "part_id", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "role", column: "role_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "serial", column: "serial", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "status", column: "status", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::InventoryItem, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static MAC_ADDRESS_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::MacAddress),
    FieldSpec { entity: InventoryEntity::MacAddress, name: "assigned_object", column: "assigned_object_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::GenericReference { type_column: "assigned_object_type_id", source_type_field: "assigned_object_type", allowed: &[InventoryEntity::Interface, InventoryEntity::VmInterface] }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::MacAddress, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::MacAddress, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::MacAddress, name: "mac_address", column: "mac_address", sql_type: PgType::MacAddr, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::MacAddress, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static MANUFACTURER_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Manufacturer),
    FieldSpec { entity: InventoryEntity::Manufacturer, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Manufacturer, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Manufacturer, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Manufacturer, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Manufacturer, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static MODULE_TYPE_PROFILE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::ModuleTypeProfile),
    FieldSpec { entity: InventoryEntity::ModuleTypeProfile, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ModuleTypeProfile, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ModuleTypeProfile, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ModuleTypeProfile, name: "schema", column: "schema", sql_type: PgType::Jsonb, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::ModuleTypeProfile, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static PLATFORM_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Platform),
    FieldSpec { entity: InventoryEntity::Platform, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Platform, name: "config_template", column: "config_template_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Platform, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Platform, name: "manufacturer", column: "manufacturer_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Manufacturer) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Platform, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Platform, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Platform) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Platform, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Platform, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static REGION_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Region),
    FieldSpec { entity: InventoryEntity::Region, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Region, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Region, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Region, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Region) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Region, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Region, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static SITE_GROUP_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::SiteGroup),
    FieldSpec { entity: InventoryEntity::SiteGroup, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::SiteGroup, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::SiteGroup, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::SiteGroup, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::SiteGroup) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::SiteGroup, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::SiteGroup, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static TENANT_GROUP_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::TenantGroup),
    FieldSpec { entity: InventoryEntity::TenantGroup, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::TenantGroup, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::TenantGroup, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::TenantGroup, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::TenantGroup) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::TenantGroup, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::TenantGroup, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static SITE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Site),
    FieldSpec { entity: InventoryEntity::Site, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "facility", column: "facility", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "group", column: "group_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::SiteGroup) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Site, name: "latitude", column: "latitude", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Site, name: "longitude", column: "longitude", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Site, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "physical_address", column: "physical_address", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "region", column: "region_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Region) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Site, name: "shipping_address", column: "shipping_address", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "status", column: "status", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Site, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Site, name: "tenant", column: "tenant_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Site, name: "time_zone", column: "time_zone", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static TAG_FIELDS: &[FieldSpec] = &[
    FieldSpec { entity: InventoryEntity::Tag, name: "color", column: "color", sql_type: PgType::Text, nullable: false, default: Some("'9e9e9e'::character varying"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Tag, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: Some("''::character varying"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Tag, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Tag, name: "object_types", column: "inventory_tag_object_type", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ContentTypeJunction { table: "inventory_tag_object_type", local_column: "tag_id", remote_column: "content_type_id" }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Tag, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Tag, name: "weight", column: "weight", sql_type: PgType::Integer, nullable: false, default: Some("0"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static CUSTOM_FIELD_FIELDS: &[FieldSpec] = &[
    FieldSpec { entity: InventoryEntity::CustomField, name: "choice_set", column: "choice_set_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "default", column: "default_value", sql_type: PgType::Jsonb, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: Some("''::character"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "filter_logic", column: "filter_logic", sql_type: PgType::Text, nullable: false, default: Some("'loose'::character"), kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "group_name", column: "group_name", sql_type: PgType::Text, nullable: false, default: Some("''::character"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "is_cloneable", column: "is_cloneable", sql_type: PgType::Bool, nullable: false, default: Some("false"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "label", column: "label", sql_type: PgType::Text, nullable: false, default: Some("''::character"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::CustomField, name: "object_types", column: "custom_field_object_type", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ContentTypeJunction { table: "custom_field_object_type", local_column: "custom_field_id", remote_column: "content_type_id" }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "related_object_filter", column: "related_object_filter", sql_type: PgType::Jsonb, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "related_object_type", column: "related_object_type_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "required", column: "required", sql_type: PgType::Bool, nullable: false, default: Some("false"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "search_weight", column: "search_weight", sql_type: PgType::Integer, nullable: false, default: Some("1000"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "type", column: "field_type", sql_type: PgType::Text, nullable: false, default: Some("'text'::character"), kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "ui_editable", column: "ui_editable", sql_type: PgType::Text, nullable: false, default: Some("'yes'::character"), kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "ui_visible", column: "ui_visible", sql_type: PgType::Text, nullable: false, default: Some("'always'::character"), kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "unique", column: "enforce_unique", sql_type: PgType::Bool, nullable: false, default: Some("false"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "validation_maximum", column: "validation_maximum", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "validation_minimum", column: "validation_minimum", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "validation_regex", column: "validation_regex", sql_type: PgType::Text, nullable: false, default: Some("''::character"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "validation_schema", column: "validation_schema", sql_type: PgType::Jsonb, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::CustomField, name: "weight", column: "weight", sql_type: PgType::Integer, nullable: false, default: Some("100"), kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static IP_ADDRESS_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::IpAddress),
    FieldSpec { entity: InventoryEntity::IpAddress, name: "address", column: "address", sql_type: PgType::Inet, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "assigned_object", column: "assigned_object_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::GenericReference { type_column: "assigned_object_type_id", source_type_field: "assigned_object_type", allowed: &[InventoryEntity::Interface, InventoryEntity::VmInterface] }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "dns_name", column: "dns_name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "nat_inside", column: "nat_inside_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::IpAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "role", column: "role", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "status", column: "status", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "tenant", column: "tenant_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::IpAddress, name: "vrf", column: "vrf_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Vrf) }, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static PREFIX_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Prefix),
    FieldSpec { entity: InventoryEntity::Prefix, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Prefix, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Prefix, name: "is_pool", column: "is_pool", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Prefix, name: "mark_utilized", column: "mark_utilized", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Prefix, name: "prefix", column: "prefix", sql_type: PgType::Cidr, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Prefix, name: "role", column: "role_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Prefix, name: "scope", column: "scope_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::GenericReference { type_column: "scope_type_id", source_type_field: "scope_type", allowed: &[InventoryEntity::Region, InventoryEntity::SiteGroup, InventoryEntity::Site] }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Prefix, name: "status", column: "status", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Prefix, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Prefix, name: "tenant", column: "tenant_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Prefix, name: "vlan", column: "vlan_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Vlan) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Prefix, name: "vrf", column: "vrf_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Vrf) }, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static SERVICE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Service),
    FieldSpec { entity: InventoryEntity::Service, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Service, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Service, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Service, name: "parent", column: "parent_object_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::GenericReference { type_column: "parent_object_type_id", source_type_field: "parent_object_type", allowed: &[InventoryEntity::Device, InventoryEntity::VirtualMachine] }, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Service, name: "port_mappings", column: "port_mappings", sql_type: PgType::TextArray, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Service, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static VRF_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Vrf),
    FieldSpec { entity: InventoryEntity::Vrf, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Vrf, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Vrf, name: "enforce_unique", column: "enforce_unique", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Vrf, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Vrf, name: "rd", column: "rd", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Vrf, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Vrf, name: "tenant", column: "tenant_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static TENANT_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Tenant),
    FieldSpec { entity: InventoryEntity::Tenant, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Tenant, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Tenant, name: "group", column: "group_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::TenantGroup) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Tenant, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Tenant, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Tenant, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static CLUSTER_GROUP_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::ClusterGroup),
    FieldSpec { entity: InventoryEntity::ClusterGroup, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterGroup, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterGroup, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterGroup, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterGroup, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static CLUSTER_TYPE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::ClusterType),
    FieldSpec { entity: InventoryEntity::ClusterType, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterType, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterType, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterType, name: "slug", column: "slug", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::ClusterType, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static CLUSTER_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Cluster),
    FieldSpec { entity: InventoryEntity::Cluster, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Cluster, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Cluster, name: "group", column: "group_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::ClusterGroup) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Cluster, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Cluster, name: "scope", column: "scope_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::GenericReference { type_column: "scope_type_id", source_type_field: "scope_type", allowed: &[InventoryEntity::Region, InventoryEntity::SiteGroup, InventoryEntity::Site] }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Cluster, name: "status", column: "status", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::Cluster, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Cluster, name: "tenant", column: "tenant_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::Cluster, name: "type", column: "type_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::ClusterType) }, choice_normalized: false, absent: Absent::Reject },
];

#[rustfmt::skip]
pub(super) static VM_INTERFACE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::VmInterface),
    FieldSpec { entity: InventoryEntity::VmInterface, name: "bridge", column: "bridge_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::VmInterface) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "enabled", column: "enabled", sql_type: PgType::Bool, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "mode", column: "mode", sql_type: PgType::Text, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "mtu", column: "mtu", sql_type: PgType::Integer, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "parent", column: "parent_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::VmInterface) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "primary_mac_address", column: "primary_mac_address_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::MacAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "qinq_svlan", column: "qinq_svlan_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "tagged_vlans", column: "virtualization_vminterface_tagged_vlans", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::Junction { table: "virtualization_vminterface_tagged_vlans", local_column: "vminterface_id", remote_column: "vlan_id", target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "untagged_vlan", column: "untagged_vlan_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "virtual_machine", column: "virtual_machine_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::VirtualMachine) }, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "vlan_translation_policy", column: "vlan_translation_policy_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::VlanTranslationPolicy) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VmInterface, name: "vrf", column: "vrf_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Vrf) }, choice_normalized: false, absent: Absent::Preserve },
];

#[rustfmt::skip]
pub(super) static VIRTUAL_DISK_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::VirtualDisk),
    FieldSpec { entity: InventoryEntity::VirtualDisk, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualDisk, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualDisk, name: "size", column: "size", sql_type: PgType::Integer, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualDisk, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualDisk, name: "virtual_machine", column: "virtual_machine_id", sql_type: PgType::Uuid, nullable: false, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::VirtualMachine) }, choice_normalized: false, absent: Absent::Reject },
];

#[rustfmt::skip]
pub(super) static VIRTUAL_MACHINE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::VirtualMachine),
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "cluster", column: "cluster_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Cluster) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "comments", column: "comments", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "config_template", column: "config_template_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: None }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "description", column: "description", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "device", column: "device_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Device) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "disk", column: "disk", sql_type: PgType::Integer, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "local_context_data", column: "local_context_data", sql_type: PgType::Jsonb, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "memory", column: "memory", sql_type: PgType::Integer, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "name", column: "name", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "platform", column: "platform_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Platform) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "primary_ip4", column: "primary_ip4_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::IpAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "primary_ip6", column: "primary_ip6_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::IpAddress) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "role", column: "role_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::DeviceRole) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "serial", column: "serial", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "site", column: "site_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Site) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "start_on_boot", column: "start_on_boot", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "status", column: "status", sql_type: PgType::Text, nullable: false, default: None, kind: FieldKind::Scalar, choice_normalized: true, absent: Absent::Reject },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "tags", column: "tags", sql_type: PgType::TextArray, nullable: false, default: Some("ARRAY[]::text[]"), kind: FieldKind::Tags, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "tenant", column: "tenant_id", sql_type: PgType::Uuid, nullable: true, default: None, kind: FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, choice_normalized: false, absent: Absent::Preserve },
    FieldSpec { entity: InventoryEntity::VirtualMachine, name: "vcpus", column: "vcpus", sql_type: PgType::Numeric, nullable: true, default: None, kind: FieldKind::Scalar, choice_normalized: false, absent: Absent::Preserve },
];

/// Admitted endpoint shapes of a flow selector row: the projected upstream
/// endpoint is an `ObjectAlias`, which the native catalogue holds as the
/// reusable named address set.
pub(super) const FLOW_ENDPOINTS: &[(&str, &str, InventoryEntity)] = &[(
    "inventory.addressgroup",
    "addressgroup_id",
    InventoryEntity::AddressGroup,
)];

/// Admitted member shapes of an address group: its ordered set of prefixes and
/// explicit addresses.
///
/// `ipam.iprange` is a native selector form, but no inventory entity syncs IP
/// ranges, so a group that names one is rejected with its field instead of being
/// imported with a silently missing member.
pub(super) const ADDRESS_GROUP_MEMBERS: &[(&str, &str, InventoryEntity)] = &[
    ("ipam.prefix", "prefix_id", InventoryEntity::Prefix),
    ("ipam.ipaddress", "ipaddress_id", InventoryEntity::IpAddress),
];

#[rustfmt::skip]
pub(super) static APPLICATION_ROLE_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::ApplicationRole),
    field!(ApplicationRole, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(ApplicationRole, "description", "description", Text, false, FieldKind::Scalar, false, Reject),
    field!(ApplicationRole, "comments", "comments", Text, false, FieldKind::Scalar, false, Reject),
];

#[rustfmt::skip]
pub(super) static APPLICATION_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Application),
    field!(Application, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(Application, "description", "description", Text, false, FieldKind::Scalar, false, Reject),
    field!(Application, "comments", "comments", Text, false, FieldKind::Scalar, false, Reject),
    field!(Application, "role", "role_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::ApplicationRole) }, false, Preserve),
    field!(Application, "tenant", "tenant_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, false, Preserve),
    field!(Application, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static DATAFLOW_GROUP_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::DataflowGroup),
    field!(DataflowGroup, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(DataflowGroup, "description", "description", Text, false, FieldKind::Scalar, false, Reject),
    field!(DataflowGroup, "comments", "comments", Text, false, FieldKind::Scalar, false, Reject),
    field!(DataflowGroup, "status", "status", Text, false, FieldKind::Scalar, false, Reject),
    field!(DataflowGroup, "parent", "parent_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::DataflowGroup) }, false, Preserve),
    field!(DataflowGroup, "application", "application_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Application) }, false, Preserve),
    field!(DataflowGroup, "tenant", "tenant_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, false, Preserve),
    field!(DataflowGroup, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static DATAFLOW_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::Dataflow),
    field!(Dataflow, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(Dataflow, "description", "description", Text, false, FieldKind::Scalar, false, Reject),
    field!(Dataflow, "comments", "comments", Text, false, FieldKind::Scalar, false, Reject),
    field!(Dataflow, "status", "status", Text, false, FieldKind::Scalar, false, Reject),
    field!(Dataflow, "protocol", "transport", Text, false, FieldKind::Scalar, false, Reject),
    field!(Dataflow, "source_ports", "source_ports", IntArray, true, FieldKind::Scalar, false, Preserve),
    field!(Dataflow, "destination_ports", "destination_ports", IntArray, true, FieldKind::Scalar, false, Preserve),
    field!(Dataflow, "application", "application_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Application) }, false, Preserve),
    field!(Dataflow, "group", "group_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::DataflowGroup) }, false, Preserve),
    field!(Dataflow, "tenant", "tenant_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, false, Preserve),
    field!(Dataflow, "sources", "inventory_dataflow_selector", Json, false, FieldKind::SelectorRows { table: "inventory_dataflow_selector", owner_column: "dataflow_id", side: "source", allowed: FLOW_ENDPOINTS }, false, Reject),
    field!(Dataflow, "destinations", "inventory_dataflow_selector", Json, false, FieldKind::SelectorRows { table: "inventory_dataflow_selector", owner_column: "dataflow_id", side: "destination", allowed: FLOW_ENDPOINTS }, false, Reject),
    field!(Dataflow, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

#[rustfmt::skip]
pub(super) static ADDRESS_GROUP_FIELDS: &[FieldSpec] = &[
    super::owner_field(InventoryEntity::AddressGroup),
    field!(AddressGroup, "name", "name", Text, false, FieldKind::Scalar, false, Reject),
    field!(AddressGroup, "description", "description", Text, false, FieldKind::Scalar, false, Reject),
    field!(AddressGroup, "comments", "comments", Text, false, FieldKind::Scalar, false, Reject),
    field!(AddressGroup, "tenant", "tenant_id", Uuid, true, FieldKind::ForeignKey { target: Some(InventoryEntity::Tenant) }, false, Preserve),
    field!(AddressGroup, "members", "inventory_dataflow_selector", Json, false, FieldKind::SelectorRows { table: "inventory_dataflow_selector", owner_column: "group_id", side: "member", allowed: ADDRESS_GROUP_MEMBERS }, false, Reject),
    field!(AddressGroup, "tags", "tags", TextArray, false, FieldKind::Tags, false, Preserve),
];

/// Excluded source fields per entity, indexed by `InventoryEntity` discriminant.
/// Each entry is `(source_field, reason)`; the reason is mandatory evidence for
/// why a populated-or-derived source field is deliberately not persisted.
#[rustfmt::skip]
pub(super) static FIELD_DISPOSITIONS_BY_INDEX: &[&[(&str, SourceFieldDisposition)]] = &[
    &[("_depth", SourceFieldDisposition::Derived("internal serializer depth")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("device_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("url", SourceFieldDisposition::Derived("read-only derived url")), ("virtualmachine_count", SourceFieldDisposition::Derived("derived aggregate counter"))],
    &[("console_port_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("console_server_port_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("cooling_intake_template_count", SourceFieldDisposition::Derived("derived aggregate counter")),  ("cooling_outflow_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("device_bay_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("device_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")),  ("front_port_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("interface_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("inventory_item_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("module_bay_template_count", SourceFieldDisposition::Derived("derived aggregate counter")),  ("power_outlet_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("power_port_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("rear_port_template_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("config_context", SourceFieldDisposition::Derived("derived config context")), ("console_port_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("console_server_port_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("cooling_intake_count", SourceFieldDisposition::Derived("derived aggregate counter")),  ("cooling_outflow_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("device_bay_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("front_port_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("interface_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("inventory_item_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("module_bay_count", SourceFieldDisposition::Derived("derived aggregate counter")),  ("parent_device", SourceFieldDisposition::UnsupportedWhenPopulated("empty relation")), ("power_outlet_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("power_port_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("primary_ip", SourceFieldDisposition::Derived("derived primary ip4/6")), ("rear_port_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("_occupied", SourceFieldDisposition::Derived("internal serializer flag")), ("bridge_interfaces", SourceFieldDisposition::Derived("derived reverse bridge relation")),    ("connected_endpoints", SourceFieldDisposition::Derived("derived cable peers")), ("connected_endpoints_reachable", SourceFieldDisposition::Derived("derived traffic flag")), ("connected_endpoints_type", SourceFieldDisposition::Derived("derived peer type")), ("count_fhrp_groups", SourceFieldDisposition::Derived("derived counter")), ("count_ipaddresses", SourceFieldDisposition::Derived("derived counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")),  ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("l2vpn_termination", SourceFieldDisposition::UnsupportedWhenPopulated("empty relation")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("link_peers", SourceFieldDisposition::Derived("derived cable peers")), ("link_peers_type", SourceFieldDisposition::Derived("derived peer type")), ("mac_address", SourceFieldDisposition::Derived("derived from primary_mac_address_id")), ("mac_addresses", SourceFieldDisposition::HandledElsewhere("reverse MAC gfk junction")),          ("url", SourceFieldDisposition::Derived("read-only derived url")), ("vdcs", SourceFieldDisposition::UnsupportedWhenPopulated("empty membership collection")), ("wireless_lans", SourceFieldDisposition::UnsupportedWhenPopulated("empty membership collection")), ("wireless_link", SourceFieldDisposition::UnsupportedWhenPopulated("empty relation")),  ],
    &[("_depth", SourceFieldDisposition::Derived("internal serializer depth")), ("component_id", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("component_type", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("assigned_object_id", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("assigned_object_type", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("is_primary", SourceFieldDisposition::Derived("derived (referenced by primary_mac_address_id)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("devicetype_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("inventoryitem_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("moduletype_count", SourceFieldDisposition::Derived("derived aggregate counter")),  ("platform_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("_depth", SourceFieldDisposition::Derived("internal serializer depth")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("device_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("url", SourceFieldDisposition::Derived("read-only derived url")), ("virtualmachine_count", SourceFieldDisposition::Derived("derived aggregate counter"))],
    &[("_depth", SourceFieldDisposition::Derived("internal serializer depth")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("prefix_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("site_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("_depth", SourceFieldDisposition::Derived("internal serializer depth")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("prefix_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("site_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("asns", SourceFieldDisposition::UnsupportedWhenPopulated("empty membership collection")), ("circuit_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("device_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("prefix_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("rack_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url")), ("virtualmachine_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("vlan_count", SourceFieldDisposition::Derived("derived aggregate counter"))],
    &[("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("tagged_items", SourceFieldDisposition::Derived("derived counter")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("comments", SourceFieldDisposition::UnsupportedWhenPopulated("empty field")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("data_type", SourceFieldDisposition::Derived("read-only duplicate of type.value")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("nulls_first", SourceFieldDisposition::UnsupportedWhenPopulated("choice-ordering flag, no choice sets in source")),  ("status", SourceFieldDisposition::Derived("read-only API status")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("assigned_object_id", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("assigned_object_type", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("family", SourceFieldDisposition::Derived("derived from address")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("nat_outside", SourceFieldDisposition::Derived("derived reverse nat")),  ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("_depth", SourceFieldDisposition::Derived("internal serializer depth")), ("children", SourceFieldDisposition::Derived("derived pool counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("family", SourceFieldDisposition::Derived("derived from prefix")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("scope_id", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("scope_type", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("ipaddresses", SourceFieldDisposition::UnsupportedWhenPopulated("junction ipam_service_ipaddresses")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("parent_object_id", SourceFieldDisposition::HandledElsewhere("gfk raw id consumed by required parent mapping")), ("parent_object_type", SourceFieldDisposition::HandledElsewhere("gfk type discriminator consumed by required parent mapping")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("export_targets", SourceFieldDisposition::UnsupportedWhenPopulated("junction ipam_vrf_export_targets")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("import_targets", SourceFieldDisposition::UnsupportedWhenPopulated("junction ipam_vrf_import_targets")), ("ipaddress_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("prefix_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("circuit_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("cluster_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("device_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("ipaddress_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("prefix_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("rack_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("site_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("url", SourceFieldDisposition::Derived("read-only derived url")), ("virtualmachine_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("vlan_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("vrf_count", SourceFieldDisposition::Derived("derived aggregate counter"))],
    &[("cluster_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("cluster_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("allocated_disk", SourceFieldDisposition::Derived("derived aggregate counter")), ("allocated_memory", SourceFieldDisposition::Derived("derived aggregate counter")), ("allocated_vcpus", SourceFieldDisposition::Derived("derived aggregate counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("device_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("scope_id", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("scope_type", SourceFieldDisposition::HandledElsewhere("gfk companion carried by the generic reference")), ("url", SourceFieldDisposition::Derived("read-only derived url")), ("virtualmachine_count", SourceFieldDisposition::Derived("derived aggregate counter"))],
    &[("count_fhrp_groups", SourceFieldDisposition::Derived("derived counter")), ("count_ipaddresses", SourceFieldDisposition::Derived("derived counter")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("l2vpn_termination", SourceFieldDisposition::UnsupportedWhenPopulated("empty relation")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")), ("mac_address", SourceFieldDisposition::Derived("derived from primary_mac_address_id")), ("mac_addresses", SourceFieldDisposition::HandledElsewhere("reverse MAC gfk junction")),  ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("url", SourceFieldDisposition::Derived("read-only derived url"))],
    &[("config_context", SourceFieldDisposition::Derived("derived config context")), ("created", SourceFieldDisposition::Derived("read-only source timestamp")), ("custom_fields", SourceFieldDisposition::HandledElsewhere("entity custom-field values (jsonb cf writer)")), ("display", SourceFieldDisposition::Derived("read-only derived label")), ("display_url", SourceFieldDisposition::Derived("read-only derived url")), ("id", SourceFieldDisposition::HandledElsewhere("provider external id (binding)")), ("interface_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("last_updated", SourceFieldDisposition::Derived("read-only source timestamp")),  ("primary_ip", SourceFieldDisposition::Derived("derived primary ip4/6")), ("url", SourceFieldDisposition::Derived("read-only derived url")), ("virtual_disk_count", SourceFieldDisposition::Derived("derived aggregate counter")), ("virtual_machine_type", SourceFieldDisposition::UnsupportedWhenPopulated("empty relation"))],
];

/// NOT NULL columns without a database default that no mapped field writes,
/// indexed by `InventoryEntity` discriminant. Each entry is
/// `(column, kind, argument)` where `kind` is `"literal"` (bind the SQL
/// expression `argument`) or `"copy"` (bind the value of the approved field
/// `argument`).
#[rustfmt::skip]
pub(super) static INTERNAL_COLUMNS_BY_INDEX: &[&[(&str, &str, &str)]] = &[
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("level", "literal", "0"), ("lft", "literal", "0"), ("rght", "literal", "0"), ("tree_id", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("console_port_template_count", "literal", "0"), ("console_server_port_template_count", "literal", "0"), ("power_port_template_count", "literal", "0"), ("power_outlet_template_count", "literal", "0"), ("interface_template_count", "literal", "0"), ("front_port_template_count", "literal", "0"), ("rear_port_template_count", "literal", "0"), ("device_bay_template_count", "literal", "0"), ("module_bay_template_count", "literal", "0"), ("inventory_item_template_count", "literal", "0"), ("device_count", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("console_port_count", "literal", "0"), ("console_server_port_count", "literal", "0"), ("power_port_count", "literal", "0"), ("power_outlet_count", "literal", "0"), ("interface_count", "literal", "0"), ("front_port_count", "literal", "0"), ("rear_port_count", "literal", "0"), ("device_bay_count", "literal", "0"), ("module_bay_count", "literal", "0"), ("inventory_item_count", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("_name", "copy", "name")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("lft", "literal", "0"), ("rght", "literal", "0"), ("tree_id", "literal", "0"), ("level", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("level", "literal", "0"), ("lft", "literal", "0"), ("rght", "literal", "0"), ("tree_id", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("lft", "literal", "0"), ("rght", "literal", "0"), ("tree_id", "literal", "0"), ("level", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("lft", "literal", "0"), ("rght", "literal", "0"), ("tree_id", "literal", "0"), ("level", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[],
    &[],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("_children", "literal", "0"), ("_depth", "literal", "0")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("_name", "copy", "name")],
    &[("custom_field_data", "literal", "'{}'::jsonb")],
    &[("custom_field_data", "literal", "'{}'::jsonb"), ("interface_count", "literal", "0"), ("virtual_disk_count", "literal", "0")],
];
