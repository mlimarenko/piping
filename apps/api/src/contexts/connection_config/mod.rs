//! Connection Configuration bounded context (`connconf_*`).
//!
//! A purpose-built, inheritable connection-config domain that is the sole
//! connect-time config source. It models two orthogonal inheritance
//! axes:
//!
//! 1. **Scope inheritance** — a [`domain::ConnConfRecord`] is bound to one or
//!    more of the 12 ordered scope layers via [`domain::ConnConfAssignment`];
//!    the resolver deep-merges most-specific-wins down the device's scope
//!    ancestry.
//! 2. **Template inheritance** — a record may extend a `base_record_id` chain of
//!    the same protocol; the base chain is resolved root → leaf before the
//!    record participates in scope merge.
//!
//! Deletions use the typed `{"unset": true}` JSON sentinel (not `null`), and
//! secrets reuse the existing `secret_usage_binding` layer (no new storage).
//!
//! This module provides the schema, domain, repositories, and the pure
//! effective-config resolver; the REST routes and connect-path wiring live
//! outside it.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::{
    LaunchAccount, ResolveInput, ResolverError, resolve_effective_config, resolve_launch_accounts,
};
pub use domain::{
    ConnConfAssignment, ConnConfProtocol, ConnConfRecord, ConnConfScope, ConnConfSource,
    EffectiveConnConfig, NewConnConfAssignment, NewConnConfRecord, UpdateConnConfAssignment,
    UpdateConnConfRecord,
};
pub use infrastructure::{
    CONNCONF_ASSIGNMENT_COLUMNS, CONNCONF_RECORD_COLUMNS, ConnConfAssignmentFilter,
    ConnConfAssignmentRepository, ConnConfRecordFilter, ConnConfRecordRepository, DeviceScopeIds,
    SqlxConnConfAssignmentRepository, SqlxConnConfRecordRepository,
};
