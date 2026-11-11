use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Module type domain model (make & model for modules), like `DeviceType` without
/// `u_height`/`is_full_depth`/`slug`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ModuleType {
    pub id: Uuid,
    pub profile_id: Option<Uuid>,
    pub attribute_data: Option<serde_json::Value>,
    pub manufacturer_id: Uuid,
    pub model: String,
    pub part_number: Option<String>,
    pub weight: Option<f64>,
    pub weight_unit: Option<String>,
    pub airflow: Option<String>,
    pub cooling_method: Option<String>,
    pub end_of_life: Option<chrono::NaiveDate>,
    #[serde(default)]
    pub cooling_intake_template_count: i64,
    #[serde(default)]
    pub cooling_outflow_template_count: i64,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
    pub module_bay_types: Vec<Uuid>,
}

/// A partial update; omitted native hardware and profile columns are untouched.
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateModuleType {
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub profile_id: Option<Option<Uuid>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub attribute_data: Option<Option<serde_json::Value>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub cooling_method: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub end_of_life: Option<Option<chrono::NaiveDate>>,
    pub manufacturer_id: Option<Uuid>,
    pub model: Option<String>,
    pub part_number: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub weight: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub weight_unit: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub airflow: Option<Option<String>>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub module_bay_types: Option<Option<Vec<Uuid>>>,
}
