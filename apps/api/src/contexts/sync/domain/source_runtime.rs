use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceBootstrapStatus {
    Pending,
    Running,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceOperationalStatus {
    Ready,
    Degraded,
    BootstrapFailed,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceRuntimeErrorCode {
    BootstrapRequestFailed,
    BootstrapInvalidConfig,
    WebhookInvalidSignature,
    WebhookDedupConflict,
    WebhookApplyFailed,
    OutboundPushFailed,
    OutboundRetryExhausted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRuntime {
    pub source_id: Uuid,
    pub bootstrap_status: SourceBootstrapStatus,
    pub operational_status: SourceOperationalStatus,
    pub external_webhook_id: Option<String>,
    pub webhook_last_received_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<SourceRuntimeErrorCode>,
    pub last_error_message: Option<String>,
    pub updated_at: DateTime<Utc>,
}
