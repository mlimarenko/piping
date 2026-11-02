use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

pub const CONTENT_TYPE: &str = "ipam.vlantranslationrule";

/// A bijective mapping within one policy, not a named or owned resource.
#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct VlanTranslationRule {
    pub id: Uuid,
    pub policy_id: Uuid,
    pub policy_name: String,
    pub local_vid: i16,
    pub remote_vid: i16,
    pub description: String,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
}

impl VlanTranslationRule {
    pub fn validate(&self) -> Result<(), AppError> {
        for (field, value) in [
            ("local_vid", self.local_vid),
            ("remote_vid", self.remote_vid),
        ] {
            if !(1..=4094).contains(&value) {
                return Err(AppError::Validation(format!(
                    "{field} must be between 1 and 4094"
                )));
            }
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
