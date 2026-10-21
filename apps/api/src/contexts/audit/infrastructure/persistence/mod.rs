//! Audit persistence adapters and shared row-hydration helpers.
pub mod repositories;

pub use repositories::{
    AUDIT_EVENT_COLUMNS, AuditEventFilter, AuditHydration, AuditObjectRef, AuditRepository,
    AuditTargetOption, SessionHydration, SqlxAuditRepository,
};
pub(crate) use repositories::{insert_audit_event, metadata_uuid, normalize_audit_type};
