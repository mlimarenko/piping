// ProviderRequestJournal: 5-state machine for outbound
// pushes to providers. Used as the PRIMARY echo-dedup mechanism by inbound
// webhook handlers (see `netbox_webhooks.rs`).
//
// Storage contract: INSERT-only for new attempts; UPDATE only allowed to
// transition `state` FORWARD (e.g. prepared -> sent_unknown -> succeeded).
// State backward transitions are rejected at the repository layer.
//
// `payload_hash` = SHA-256 hex of canonicalized JSON payload — never raw
// payload. `stamped_external_request_id` is fallback evidence
// only; primary dedup is `(provider_id, model, entity_identity, payload_hash)`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 5-state machine for journal rows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    /// Row inserted; HTTP not yet issued. May be a ghost on crash before send.
    Prepared,
    /// HTTP request issued; response not yet received (timeout, connection reset).
    SentUnknown,
    /// HTTP 2xx received; provider revision recorded.
    Succeeded,
    /// HTTP never issued (network unreachable, circuit breaker, validation).
    FailedNoSend,
    /// Confirmed server error after send (5xx, NetBox 412 conflict).
    FailedAfterSend,
}

impl JournalState {
    pub fn as_str(&self) -> &'static str {
        match self {
            JournalState::Prepared => "prepared",
            JournalState::SentUnknown => "sent_unknown",
            JournalState::Succeeded => "succeeded",
            JournalState::FailedNoSend => "failed_no_send",
            JournalState::FailedAfterSend => "failed_after_send",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "prepared" => Some(JournalState::Prepared),
            "sent_unknown" => Some(JournalState::SentUnknown),
            "succeeded" => Some(JournalState::Succeeded),
            "failed_no_send" => Some(JournalState::FailedNoSend),
            "failed_after_send" => Some(JournalState::FailedAfterSend),
            _ => None,
        }
    }

    /// Return `true` if `next` is an allowed forward transition from `self`.
    /// Rules:
    ///   prepared -> sent_unknown | failed_no_send
    ///   sent_unknown -> succeeded | failed_after_send
    ///   succeeded / failed_no_send / failed_after_send are terminal.
    pub fn can_transition_to(&self, next: JournalState) -> bool {
        matches!(
            (*self, next),
            (JournalState::Prepared, JournalState::SentUnknown)
                | (JournalState::Prepared, JournalState::FailedNoSend)
                | (JournalState::SentUnknown, JournalState::Succeeded)
                | (JournalState::SentUnknown, JournalState::FailedAfterSend)
        )
    }

    /// States included in the inbound webhook echo-dedup probe.
    /// Dedup eligible IF state IN
    /// ('prepared', 'sent_unknown', 'succeeded').
    pub fn is_dedup_eligible(&self) -> bool {
        matches!(
            self,
            JournalState::Prepared | JournalState::SentUnknown | JournalState::Succeeded
        )
    }
}

/// Single row in `provider_request_journal`. One per attempt; retries get a
/// new row with `attempt_no = N+1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequestJournalEntry {
    pub id: Uuid,
    pub provider_id: Uuid,
    /// Stable identifier for the outbound event across retries; one
    /// outbound_change_event_uuid groups all attempts together.
    pub outbound_event_uuid: Uuid,
    pub attempt_no: i32,
    pub state: JournalState,
    /// NetBox model name (e.g. `dcim.device`).
    pub model: String,
    /// External entity identity as string (NetBox int id stringified, or
    /// composite key for entities without int id).
    pub entity_identity: String,
    /// SHA-256 hex digest of canonicalized payload.
    pub payload_hash: String,
    /// NetBox revision/ETag observed on 2xx response.
    pub netbox_revision_observed: Option<i32>,
    /// Optional `external_request_id` header echoed back by NetBox.
    pub stamped_external_request_id: Option<String>,
    pub http_status: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_transitions_allowed() {
        assert!(JournalState::Prepared.can_transition_to(JournalState::SentUnknown));
        assert!(JournalState::Prepared.can_transition_to(JournalState::FailedNoSend));
        assert!(JournalState::SentUnknown.can_transition_to(JournalState::Succeeded));
        assert!(JournalState::SentUnknown.can_transition_to(JournalState::FailedAfterSend));
    }

    #[test]
    fn backward_transitions_rejected() {
        assert!(!JournalState::Succeeded.can_transition_to(JournalState::Prepared));
        assert!(!JournalState::FailedAfterSend.can_transition_to(JournalState::SentUnknown));
        assert!(!JournalState::FailedNoSend.can_transition_to(JournalState::Prepared));
        // Terminal states cannot move at all
        assert!(!JournalState::Succeeded.can_transition_to(JournalState::Succeeded));
    }

    #[test]
    fn dedup_eligibility() {
        assert!(JournalState::Prepared.is_dedup_eligible());
        assert!(JournalState::SentUnknown.is_dedup_eligible());
        assert!(JournalState::Succeeded.is_dedup_eligible());
        assert!(!JournalState::FailedNoSend.is_dedup_eligible());
        assert!(!JournalState::FailedAfterSend.is_dedup_eligible());
    }

    #[test]
    fn round_trip_str() {
        for state in [
            JournalState::Prepared,
            JournalState::SentUnknown,
            JournalState::Succeeded,
            JournalState::FailedNoSend,
            JournalState::FailedAfterSend,
        ] {
            assert_eq!(JournalState::from_wire(state.as_str()), Some(state));
        }
        assert_eq!(JournalState::from_wire("bogus"), None);
    }
}
