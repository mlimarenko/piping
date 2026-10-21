use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Module type domain model (make & model for modules), like `DeviceType` without
/// `u_height`/`is_full_depth`/`slug`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ModuleType {
    pub id: Uuid,
    pub manufacturer_id: Uuid,
    pub model: String,
    pub part_number: Option<String>,
    pub weight: Option<f64>,
    pub weight_unit: Option<String>,
    pub airflow: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}
