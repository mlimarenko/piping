use crate::errors::AppError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Compile the supplied JSON Schema without HTTP/file resolution. Schemas and
/// module attributes are untrusted API inputs, not permission to fetch a URL.
pub fn profile_validator(schema: &serde_json::Value) -> Result<jsonschema::Validator, AppError> {
    if !schema.is_object() {
        return Err(AppError::Validation(
            "schema must be a JSON object or null".into(),
        ));
    }
    jsonschema::validator_for(schema)
        .map_err(|_| AppError::Validation("Invalid module type profile JSON schema".into()))
}

pub fn validate_profile_attributes(
    schema: Option<&serde_json::Value>,
    attributes: Option<&serde_json::Value>,
) -> Result<(), AppError> {
    if let Some(schema) = schema {
        let validator = profile_validator(schema)?;
        let empty = serde_json::Value::Null;
        if !validator.is_valid(attributes.unwrap_or(&empty)) {
            return Err(AppError::Validation(
                "attribute_data does not match the module type profile schema".into(),
            ));
        }
    }
    Ok(())
}

/// Schema describing the attributes of a module type.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ModuleTypeProfile {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub comments: String,
    pub schema: Option<serde_json::Value>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: serde_json::Value,
    pub tags: Vec<String>,
    pub created: Option<chrono::DateTime<chrono::Utc>>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
}
