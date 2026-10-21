// Two-slot field mapping.
//
// `forward_transform_json` is NOT NULL — every FieldMapping must declare a
// forward DSL expression. `reverse_transform_json` is NULLABLE — when NULL,
// the field is read-only in target→source direction (UI surfaces read-only
// badge).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMapping {
    pub id: Uuid,
    pub entity_mapping_id: Uuid,
    pub source_field: String,
    pub target_field: String,
    /// Typed-DSL expression. Always required.
    pub forward_transform_json: Value,
    /// Typed-DSL expression. NULL → read-only in target→source direction.
    pub reverse_transform_json: Option<Value>,
    pub round_trip_critical: bool,
    pub created_at: DateTime<Utc>,
}
