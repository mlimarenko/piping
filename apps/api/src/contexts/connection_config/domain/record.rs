use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::scope::ConnConfProtocol;

/// A named, protocol-typed bundle of connection parameters.
///
/// Maps to the `connconf_record` table. The `params` JSONB carries the typed
/// parameter tree; `base_record_id` is the optional template-inheritance parent
/// (same protocol, cycle-checked in the service). `owner_user_id = None` means
/// org-shared, `Some` means a personal/private record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnConfRecord {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub protocol: ConnConfProtocol,
    pub base_record_id: Option<Uuid>,
    pub enabled: bool,
    pub owner_user_id: Option<Uuid>,
    pub params: Value,
    pub custom_field_data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewConnConfRecord {
    pub name: String,
    pub description: Option<String>,
    pub protocol: ConnConfProtocol,
    pub base_record_id: Option<Uuid>,
    pub enabled: Option<bool>,
    pub owner_user_id: Option<Uuid>,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpdateConnConfRecord {
    pub name: Option<String>,
    pub description: Option<String>,
    pub protocol: Option<ConnConfProtocol>,
    /// Outer `Some` = caller set the field; inner `Option` carries null-able value.
    pub base_record_id: Option<Option<Uuid>>,
    pub enabled: Option<bool>,
    pub params: Option<Value>,
}
