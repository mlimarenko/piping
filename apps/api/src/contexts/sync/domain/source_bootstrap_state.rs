// Per-provider bootstrap state for the lifecycle
// endpoints (`POST /api/v1/providers/{id}/bootstrap`,
// `GET .../bootstrap/status`, `POST .../bootstrap/replay`,
// `GET .../bootstrap/last-error`).
//
// Distinct from `SourceRuntime` (which carries operational/runtime status of
// an already-bootstrapped source). `SourceBootstrapState` describes the
// dedicated bootstrap workflow with explicit attempted/completed timestamps
// and a stable error trail across replays.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceBootstrapStateStatus {
    NotStarted,
    InProgress,
    Complete,
    Failed,
}

impl SourceBootstrapStateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceBootstrapStateStatus::NotStarted => "not_started",
            SourceBootstrapStateStatus::InProgress => "in_progress",
            SourceBootstrapStateStatus::Complete => "complete",
            SourceBootstrapStateStatus::Failed => "failed",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "not_started" => Some(SourceBootstrapStateStatus::NotStarted),
            "in_progress" => Some(SourceBootstrapStateStatus::InProgress),
            "complete" => Some(SourceBootstrapStateStatus::Complete),
            "failed" => Some(SourceBootstrapStateStatus::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceBootstrapState {
    pub provider_id: Uuid,
    pub status: SourceBootstrapStateStatus,
    pub attempts: i32,
    pub attempted_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}
