use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboundWebhookEventStatus {
    Accepted,
    Deduplicated,
    Applied,
    Ignored,
    Failed,
}

/// An inbound NetBox webhook event.
///
/// `payload` is an **in-memory-only** field populated during the current
/// request so that `apply_event` can act on the data. It is NEVER persisted
/// to the database; only `payload_hash` (SHA-256 hex of the raw HTTP body) is
/// stored. When a record is loaded from the database `payload` is `Value::Null`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundWebhookEvent {
    pub id: Uuid,
    pub source_id: Uuid,
    pub dedup_key: String,
    pub event_type: String,
    pub model: String,
    pub request_id: Option<String>,
    pub status: InboundWebhookEventStatus,
    /// SHA-256 hex digest of the raw HTTP request body. Persisted. Never empty.
    pub payload_hash: String,
    /// Parsed payload — populated in-memory only; never written to the DB.
    /// Callers that load events from the DB will find `Value::Null` here.
    #[serde(skip_serializing_if = "Value::is_null", default)]
    pub payload: Value,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}
