//! Audit infrastructure adapters and their persistence re-exports.
pub mod persistence;

pub use persistence::{AuditEventFilter, AuditRepository, SqlxAuditRepository};
