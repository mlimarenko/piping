use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ModuleBayTemplate {
    pub id: Uuid,
    pub name: String,
    pub label: String,
    pub position: String,
    pub enabled: bool,
    pub description: String,
    pub device_type_id: Option<Uuid>,
    pub module_type_id: Option<Uuid>,
    pub module_bay_types: Vec<Uuid>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleBayTemplateChanges {
    pub name: Option<String>,
    pub label: Option<String>,
    pub position: Option<String>,
    pub enabled: Option<bool>,
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub device_type_id: Option<Option<Uuid>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub module_type_id: Option<Option<Uuid>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub module_bay_types: Option<Option<Vec<Uuid>>>,
}
