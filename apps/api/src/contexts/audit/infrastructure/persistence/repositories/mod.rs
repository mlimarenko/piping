//! SQLx-backed audit repository, row hydration types and insert helpers.
pub mod audit_repository;
pub mod sqlx_audit_repository;

pub use audit_repository::{AUDIT_EVENT_COLUMNS, AuditEventFilter, AuditRepository};
pub use sqlx_audit_repository::{
    AuditHydration, AuditObjectRef, AuditTargetOption, SessionHydration, SqlxAuditRepository,
};
pub(crate) use sqlx_audit_repository::{insert_audit_event, metadata_uuid, normalize_audit_type};
