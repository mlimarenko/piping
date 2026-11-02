//! SQLx-backed access-control repository implementations and their column lists.
pub mod credential_repository;
pub mod group_repository;
pub mod oidc_settings_repository;
pub mod policy_repository;
pub mod repository;
pub mod role_assignment_repository;
pub mod role_repository;
pub mod session_repository;
pub mod settings_repository;
pub mod user_api_token_repository;
pub mod user_repository;

pub use credential_repository::SqlxUserCredentialRepository;
pub use group_repository::SqlxGroupRepository;
pub use oidc_settings_repository::SqlxOidcSettingsRepository;
pub use policy_repository::SqlxPolicyRepository;
pub use repository::{
    AuthSessionRepository, GROUP_COLUMNS, GroupRepository, NamedListFilter, OidcSettingsRepository,
    POLICY_COLUMNS, PolicyFilter, PolicyRepository, ROLE_ASSIGNMENT_COLUMNS, ROLE_COLUMNS,
    RoleAssignmentFilter, RoleAssignmentRepository, RoleRepository, USER_COLUMNS,
    UserApiTokenListFilter, UserApiTokenRepository, UserCredentialRepository, UserListFilter,
    UserRepository,
};
pub use role_assignment_repository::SqlxRoleAssignmentRepository;
pub use role_repository::SqlxRoleRepository;
pub use session_repository::SqlxAuthSessionRepository;
pub use settings_repository::{SettingValueRecord, SettingsRepository, SqlxSettingsRepository};
pub use user_api_token_repository::SqlxUserApiTokenRepository;
pub use user_repository::SqlxUserRepository;
