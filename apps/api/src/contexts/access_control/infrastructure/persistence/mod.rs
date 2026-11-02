//! Access-control persistence adapters re-exported from the repositories module.
pub mod repositories;

pub use repositories::{
    AuthSessionRepository, GROUP_COLUMNS, GroupRepository, NamedListFilter, OidcSettingsRepository,
    POLICY_COLUMNS, PolicyFilter, PolicyRepository, ROLE_ASSIGNMENT_COLUMNS, ROLE_COLUMNS,
    RoleAssignmentFilter, RoleAssignmentRepository, RoleRepository, SqlxAuthSessionRepository,
    SqlxGroupRepository, SqlxOidcSettingsRepository, SqlxPolicyRepository,
    SqlxRoleAssignmentRepository, SqlxRoleRepository, SqlxSettingsRepository,
    SqlxUserApiTokenRepository, SqlxUserCredentialRepository, SqlxUserRepository, USER_COLUMNS,
    UserApiTokenListFilter, UserApiTokenRepository, UserCredentialRepository, UserListFilter,
    UserRepository,
};
