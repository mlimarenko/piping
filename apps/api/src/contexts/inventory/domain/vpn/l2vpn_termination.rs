use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::contexts::inventory::domain::{ObjectRef, object_ref::ObjectRefInput};
use crate::errors::AppError;

pub const CONTENT_TYPE: &str = "vpn.l2vpntermination";
pub const ENDPOINT_TYPES: &[&str] = &["dcim.interface", "virtualization.vminterface", "ipam.vlan"];

pub fn validate_endpoint(input: &ObjectRefInput) -> Result<(), AppError> {
    if !ENDPOINT_TYPES.contains(&input.object_type.as_str()) {
        return Err(AppError::Validation("assigned_object must reference dcim.interface, virtualization.vminterface or ipam.vlan".into()));
    }
    Ok(())
}

pub fn endpoint_schema() -> utoipa::openapi::schema::Object {
    use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .property(
            "object_type",
            ObjectBuilder::new()
                .schema_type(Type::String)
                .enum_values(Some(ENDPOINT_TYPES.iter().copied())),
        )
        .property(
            "object_id",
            ObjectBuilder::new()
                .schema_type(Type::String)
                .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid))),
        )
        .required("object_type")
        .required("object_id")
        .additional_properties(Some(
            utoipa::openapi::schema::AdditionalProperties::FreeForm(false),
        ))
        .build()
}

/// A termination is a NetBoxModel, not a named or owned PrimaryModel.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct L2VpnTermination {
    pub id: Uuid,
    pub l2vpn_id: Uuid,
    pub l2vpn_name: String,
    pub assigned_object: ObjectRef,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
}
