use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

pub const CONTENT_TYPE: &str = "circuits.virtualcircuittermination";
pub const ROLES: &[&str] = &["peer", "hub", "spoke"];

pub fn role_schema() -> utoipa::openapi::schema::Object {
    use utoipa::openapi::schema::{ObjectBuilder, Type};
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(ROLES.iter().copied()))
        .build()
}

/// One virtual circuit endpoint on a DCIM virtual, LAG or bridge interface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct VirtualCircuitTermination {
    pub id: Uuid,
    pub virtual_circuit_id: Uuid,
    pub virtual_circuit_name: String,
    pub interface_id: Uuid,
    pub interface_name: String,
    #[schema(schema_with=role_schema)]
    pub role: String,
    pub description: String,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
}

pub fn validate_role(role: &str) -> Result<(), AppError> {
    if !ROLES.contains(&role) {
        return Err(AppError::Validation(
            "role must be peer, hub or spoke".into(),
        ));
    }
    Ok(())
}

impl VirtualCircuitTermination {
    pub fn validate(&self) -> Result<(), AppError> {
        validate_role(&self.role)?;
        if self.description.chars().count() > 200 {
            return Err(AppError::Validation(
                "description must be at most 200 characters".into(),
            ));
        }
        if !self.custom_fields.is_object() {
            return Err(AppError::Validation(
                "custom_fields must be an object".into(),
            ));
        }
        Ok(())
    }
}
