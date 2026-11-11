//! Access-control infrastructure adapters and their persistence re-exports.
pub mod persistence;

pub use persistence::{
    AuthSessionRepository, GroupRepository, PolicyFilter, PolicyRepository,
    RoleAssignmentRepository, RoleRepository, SqlxAuthSessionRepository, SqlxGroupRepository,
    SqlxPolicyRepository, SqlxRoleAssignmentRepository, SqlxRoleRepository,
    SqlxUserCredentialRepository, SqlxUserRepository, UserCredentialRepository, UserRepository,
};
