use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboundEventStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
    DeadLetter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundEvent {
    pub id: Uuid,
    pub source_id: Uuid,
    pub resource_type: String,
    pub operation: String,
    pub payload: Value,
    pub status: OutboundEventStatus,
    pub attempts: i32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub processed_at: Option<DateTime<Utc>>,
    pub correlation_id: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutboundEventCounters {
    pub pending_count: i64,
    pub failed_count: i64,
    pub last_successful_outbound_at: Option<DateTime<Utc>>,
}
