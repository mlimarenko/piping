use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::{CONFIG_CONTEXT_WEIGHT_MAX, CONFIG_CONTEXT_WEIGHT_MIN};
use crate::errors::AppError;

/// ConfigContext domain model
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigContext {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub weight: i32,
    pub data: Value,
    pub is_active: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAttribution {
    pub context_id: Uuid,
    pub context_name: String,
    pub scope_type: String,
    pub scope_id: Uuid,
    pub scope_name: String,
    pub weight: i32,
    pub scope_precedence: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedContextEntry {
    pub context_id: Uuid,
    pub context_name: String,
    pub scope_type: String,
    pub scope_id: Uuid,
    pub scope_name: String,
    pub weight: i32,
    pub scope_precedence: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretPresence {
    pub has_value: bool,
    pub source_context_name: Option<String>,
}

impl ConfigContext {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.weight < CONFIG_CONTEXT_WEIGHT_MIN || self.weight > CONFIG_CONTEXT_WEIGHT_MAX {
            return Err(AppError::Validation(format!(
                "ConfigContext weight {} outside range {}..{}",
                self.weight, CONFIG_CONTEXT_WEIGHT_MIN, CONFIG_CONTEXT_WEIGHT_MAX
            )));
        }
        Ok(())
    }
}
