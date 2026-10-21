use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Rack type domain model — NetBox `DCIM > rack types` 1:1 mirror.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RackType {
    pub id: Uuid,
    /// Numeric NetBox id, set when the record is pulled from NetBox.
    pub external_id: Option<i64>,
    pub manufacturer_id: Uuid,
    pub model: String,
    pub slug: String,
    /// NetBox `RackFormFactorChoices` (e.g. `2-post-frame`, `4-post-cabinet`).
    pub form_factor: String,
    /// Rail-to-rail width in millimetres (NetBox `RackWidthChoices`).
    pub width: Option<i16>,
    pub u_height: Option<i16>,
    pub starting_unit: Option<i16>,
    pub desc_units: bool,
    pub outer_width: Option<i16>,
    pub outer_depth: Option<i16>,
    pub outer_height: Option<i16>,
    pub outer_unit: Option<String>,
    pub mounting_depth: Option<i16>,
    pub weight: Option<f64>,
    pub max_weight: Option<i32>,
    pub weight_unit: Option<String>,
    pub description: Option<String>,
    /// piping ownership axis (soft-audit; created/owned-by user).
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Last successful sync watermark from the piping sync engine.
    pub piping_synced_at: Option<DateTime<Utc>>,
}
