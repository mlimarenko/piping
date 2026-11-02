//! SQLx-backed connection-configuration repositories for records and assignments.
pub mod assignment_repository;
pub mod record_repository;

pub use assignment_repository::{
    CONNCONF_ASSIGNMENT_COLUMNS, ConnConfAssignmentFilter, ConnConfAssignmentRepository,
    DeviceScopeIds, SqlxConnConfAssignmentRepository,
};
pub use record_repository::{
    CONNCONF_RECORD_COLUMNS, ConnConfRecordFilter, ConnConfRecordRepository,
    SqlxConnConfRecordRepository,
};
