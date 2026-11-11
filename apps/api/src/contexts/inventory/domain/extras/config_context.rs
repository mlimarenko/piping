use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::{
    CONFIG_CONTEXT_WEIGHT_DEFAULT, CONFIG_CONTEXT_WEIGHT_MAX, CONFIG_CONTEXT_WEIGHT_MIN,
};
use crate::errors::AppError;

/// ConfigContext domain model
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigContext {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    #[serde(default = "default_config_context_weight")]
    #[schema(required = true)]
    pub weight: i32,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>, additional_properties = true)]
    pub data: Value,
    pub is_active: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub fn default_config_context_weight() -> i32 {
    CONFIG_CONTEXT_WEIGHT_DEFAULT
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
        if !self.data.is_object() {
            return Err(AppError::Validation(
                "ConfigContext data must be a JSON object".to_string(),
            ));
        }
        if self.weight < CONFIG_CONTEXT_WEIGHT_MIN || self.weight > CONFIG_CONTEXT_WEIGHT_MAX {
            return Err(AppError::Validation(format!(
                "ConfigContext weight {} outside range {}..{}",
                self.weight, CONFIG_CONTEXT_WEIGHT_MIN, CONFIG_CONTEXT_WEIGHT_MAX
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn context_data_must_be_an_object() {
        let mut context = ConfigContext {
            id: Uuid::nil(),
            name: "test".into(),
            description: None,
            weight: 100,
            data: json!({}),
            is_active: true,
            created_at: None,
            updated_at: None,
        };
        assert!(context.validate().is_ok());
        for value in [json!(null), json!([]), json!(1), json!("text")] {
            context.data = value;
            assert!(matches!(context.validate(), Err(AppError::Validation(_))));
        }
    }

    #[test]
    fn weights_follow_netbox_range() {
        let mut context: ConfigContext = serde_json::from_value(json!({
            "id": Uuid::nil(), "name": "test", "description": null,
            "data": {}, "is_active": true, "created_at": null, "updated_at": null
        }))
        .expect("deserialize default weight");
        assert_eq!(context.weight, 1000);
        for weight in [0, 2000, 32767] {
            context.weight = weight;
            assert!(context.validate().is_ok());
        }
        for weight in [-1, 32768] {
            context.weight = weight;
            assert!(context.validate().is_err());
        }
    }
}
