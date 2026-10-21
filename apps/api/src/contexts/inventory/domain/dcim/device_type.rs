use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Device type domain model (make & model for devices) — full NetBox field surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeviceType {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub manufacturer_id: Uuid,
    pub model: String,
    pub slug: String,
    pub default_platform_id: Option<Uuid>,
    pub part_number: Option<String>,
    pub u_height: f64,
    pub is_full_depth: bool,
    pub subdevice_role: Option<String>,
    pub airflow: Option<String>,
    pub weight: Option<f64>,
    pub weight_unit: Option<String>,
    pub front_image: Option<String>,
    pub rear_image: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}

/// Hydrated list row — includes the manufacturer name via LEFT JOIN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceTypeListView {
    pub record: DeviceType,
    pub manufacturer_name: Option<String>,
}
