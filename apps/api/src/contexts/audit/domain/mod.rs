//! Audit-event domain model.
pub mod audit_event;
pub mod event_context;
pub mod safe_projection;

pub use audit_event::{AUDIT_ACTOR_KIND_PLAYBOOK_JOB_TOKEN, AuditEvent};
pub use event_context::{
    AuditAutomationState, AuditContext, AuditCorrelation, AuditDecision, AuditDelegatedPrincipal,
    AuditDelegatedPrincipalKind, AuditDisplaySnapshot, AuditEmitter, AuditEmitterKind,
    AuditLifecycleState, AuditOutcome, AuditSessionState, AuditTransportState,
};
