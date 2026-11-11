//! Connection-configuration infrastructure adapters and their persistence re-exports.
pub mod persistence;

pub use persistence::{
    CONNCONF_ASSIGNMENT_COLUMNS, CONNCONF_RECORD_COLUMNS, ConnConfAssignmentFilter,
    ConnConfAssignmentRepository, ConnConfRecordFilter, ConnConfRecordRepository, DeviceScopeIds,
    SqlxConnConfAssignmentRepository, SqlxConnConfRecordRepository,
};
