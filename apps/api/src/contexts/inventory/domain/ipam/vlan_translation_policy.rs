use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

pub const CONTENT_TYPE: &str = "ipam.vlantranslationpolicy";

/// A named VLAN translation policy; interface references protect its deletion.
#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct VlanTranslationPolicy {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub comments: String,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
}

impl VlanTranslationPolicy {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.name.trim().is_empty() || self.name.chars().count() > 100 {
            return Err(AppError::Validation(
                "name must contain 1 to 100 characters".into(),
            ));
        }
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
