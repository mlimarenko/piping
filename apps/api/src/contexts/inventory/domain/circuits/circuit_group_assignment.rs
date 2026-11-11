use crate::contexts::inventory::domain::object_ref::ObjectRef;
use crate::errors::AppError;
use serde::Serialize;
use uuid::Uuid;

pub const CONTENT_TYPE: &str = "circuits.circuitgroupassignment";
pub const MEMBER_TYPES: &[&str] = &["circuits.circuit", "circuits.virtualcircuit"];
pub const PRIORITIES: &[&str] = &["primary", "secondary", "tertiary", "inactive"];

/// Constrain the shared ObjectRefInput shape to this relation's allowed targets.
pub fn member_input_schema() -> utoipa::openapi::schema::Object {
    use utoipa::openapi::schema::{
        AdditionalProperties, KnownFormat, ObjectBuilder, SchemaFormat, Type,
    };
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .property(
            "object_type",
            ObjectBuilder::new()
                .schema_type(Type::String)
                .enum_values(Some(MEMBER_TYPES.iter().copied())),
        )
        .property(
            "object_id",
            ObjectBuilder::new()
                .schema_type(Type::String)
                .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid))),
        )
        .required("object_type")
        .required("object_id")
        .additional_properties(Some(AdditionalProperties::FreeForm(false)))
        .build()
}

pub fn priority_input_schema() -> utoipa::openapi::schema::Object {
    use utoipa::openapi::schema::{ObjectBuilder, SchemaType, Type};
    let values = PRIORITIES
        .iter()
        .map(|value| serde_json::Value::String((*value).into()))
        .chain([
            serde_json::Value::String(String::new()),
            serde_json::Value::Null,
        ]);
    ObjectBuilder::new()
        .schema_type(SchemaType::Array(vec![Type::String, Type::Null]))
        .enum_values(Some(values))
        .build()
}

/// A physical or virtual circuit's membership in a circuit group. This ancillary
/// model has no name, owner, description or comments of its own.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CircuitGroupAssignment {
    pub id: Uuid,
    pub group_id: Uuid,
    pub group_name: String,
    pub member: ObjectRef,
    pub priority: Option<String>,
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<chrono::DateTime<chrono::Utc>>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub external_id: Option<i64>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub fn validate_member_type(token: &str) -> Result<(), AppError> {
    if !MEMBER_TYPES.contains(&token) {
        return Err(AppError::Validation(
            "member must be a physical or virtual circuit".into(),
        ));
    }
    Ok(())
}

pub fn normalize_priority(value: Option<String>) -> Result<Option<String>, AppError> {
    if value
        .as_deref()
        .is_none_or(|priority| priority.is_empty() || PRIORITIES.contains(&priority))
    {
        Ok(value.filter(|v| !v.is_empty()))
    } else {
        Err(AppError::Validation(
            "Invalid circuit assignment priority".into(),
        ))
    }
}
