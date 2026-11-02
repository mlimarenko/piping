use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub use crate::contexts::access_control::application::ports::{
    UserApiTokenListFilter, UserApiTokenRepository,
};
use crate::contexts::access_control::domain::{
    AuthSession, Group, OidcSettings, Policy, PolicyEffect, Role, RoleAssignment, User,
    UserCredential,
};
use crate::contexts::shared::list_query::{ListColumn, SortSpec};
use crate::errors::AppError;

pub const USER_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("username", "username", "access.users.username"),
    ListColumn::sortable("email", "email", "access.users.email"),
    ListColumn::enum_filter("status", "status", "status", "access.users.status"),
    ListColumn::sortable(
        "roles",
        "(SELECT COALESCE(string_agg(r.name, ',' ORDER BY r.name), '') FROM rbac_role_assignment ra JOIN rbac_role r ON r.id = ra.role_id WHERE ra.user_id = identity_user.id)",
        "access.users.roles",
    ),
    ListColumn::enum_filter(
        "auth_source",
        "auth_source",
        "auth_source",
        "access.users.auth_source",
    ),
];

pub const ROLE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "access.roles.name"),
    ListColumn::sortable("description", "description", "access.roles.description"),
    ListColumn::sortable(
        "policyCount",
        "(SELECT COUNT(*) FROM rbac_policy p WHERE p.subjects ? ('role:' || rbac_role.id::text))",
        "access.roles.policy_count",
    ),
];

pub const GROUP_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "access.groups.name"),
    ListColumn::sortable("external_id", "external_id", "access.groups.external_id"),
    ListColumn::sortable(
        "assignments",
        "(SELECT COUNT(*) FROM rbac_role_assignment ra WHERE ra.group_id = identity_group.id)",
        "access.groups.assignments",
    ),
];

pub const ROLE_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("created", "ra.id", "access.assignments.created"),
    ListColumn::fk_filter(
        "role",
        "r.name",
        "role_id",
        "ra.role_id",
        "access.roles",
        "access.assignments.role",
    ),
    ListColumn::sortable(
        "subject",
        "COALESCE(u.username, g.name)",
        "access.assignments.subject",
    ),
    ListColumn::fk_filter(
        "role_id",
        "ra.role_id",
        "role_id",
        "ra.role_id",
        "access.roles",
        "access.assignments.role_id",
    ),
    ListColumn::fk_filter(
        "user_id",
        "ra.user_id",
        "user_id",
        "ra.user_id",
        "access.users",
        "access.assignments.user_id",
    ),
    ListColumn::fk_filter(
        "group_id",
        "ra.group_id",
        "group_id",
        "ra.group_id",
        "access.groups",
        "access.assignments.group_id",
    ),
    ListColumn::enum_filter_with_param(
        "type",
        "CASE WHEN ra.user_id IS NULL THEN 'group' ELSE 'user' END",
        "subject_type",
        "CASE WHEN ra.user_id IS NULL THEN 'group' ELSE 'user' END",
        "access.assignments.type",
    ),
];

pub const POLICY_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "access.policies.name"),
    ListColumn::enum_filter("effect", "effect", "effect", "access.policies.effect"),
    ListColumn::enum_filter("enabled", "enabled", "enabled", "access.policies.enabled"),
    ListColumn::sortable("subjects", "subjects::text", "access.policies.subjects"),
    ListColumn::sortable("policy_actions", "actions::text", "access.policies.actions"),
    ListColumn::sortable("scopes", "resources::text", "access.policies.scopes"),
];

/// List parameters for users: free-text search, multi-value `status` /
/// `auth_source` IN filters, and an allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct UserListFilter {
    /// Pre-trimmed ILIKE pattern over username/email; empty means "no filter".
    pub search: Option<String>,
    /// Multi-value `status` IN filter (matches any).
    pub status: Vec<String>,
    /// Multi-value `auth_source` IN filter (matches any).
    pub auth_source: Vec<String>,
    pub sort: Option<SortSpec>,
}

/// List parameters shared by groups and roles: free-text search plus an
/// allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct NamedListFilter {
    /// Pre-trimmed ILIKE pattern over the primary name column; empty means "no filter".
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct PolicyFilter {
    pub name: Option<String>,
    /// Multi-value `effect` IN filter (matches any).
    pub effect: Vec<PolicyEffect>,
    /// Multi-value `enabled` IN filter (matches any).
    pub enabled: Vec<bool>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct RoleAssignmentFilter {
    /// Multi-value `role_id` IN filter (matches any).
    pub role_id: Vec<Uuid>,
    /// Multi-value `user_id` IN filter (matches any).
    pub user_id: Vec<Uuid>,
    /// Multi-value `group_id` IN filter (matches any).
    pub group_id: Vec<Uuid>,
    /// Multi-value subject type filter: user, group.
    pub subject_type: Vec<String>,
    /// Free-text ILIKE filter over role name + subject (user username/email,
    /// group name). Pre-trimmed; empty means "no filter".
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::shared::pagination::PageResult;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn get_by_id(&self, user_id: Uuid) -> Result<Option<User>, AppError>;
    async fn get_by_username_or_email(&self, identifier: &str) -> Result<Option<User>, AppError>;
    async fn get_by_external_id(&self, external_id: &str) -> Result<Option<User>, AppError>;
    async fn list_users(&self) -> Result<Vec<User>, AppError>;
    async fn list_users_page(
        &self,
        filter: UserListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<User>, AppError>;
    async fn create_user(&self, user: User) -> Result<User, AppError>;
    async fn update_user(&self, user: User) -> Result<User, AppError>;
    async fn delete_user(&self, user_id: Uuid) -> Result<bool, AppError>;
    /// Resolve the `identity_group` rows whose `external_id` matches any of the
    /// supplied (already-normalized) OIDC claim values. Used by the canonical
    /// subject resolver to classify current-session OIDC claims as mapped or
    /// unmapped. Authorization grants do not come directly from claims; durable
    /// group membership is the RBAC source of truth.
    async fn list_groups_by_external_ids(
        &self,
        external_ids: &[String],
    ) -> Result<Vec<Group>, AppError>;
    /// List groups the user is a persisted member of.
    ///
    /// This is the authorization source of truth for group-derived RBAC grants.
    /// OIDC claims update these rows at login time; request-time authorization
    /// reads the durable membership so browser sessions and personal API tokens
    /// resolve the same user permissions.
    async fn list_groups_for_user(&self, user_id: Uuid) -> Result<Vec<Group>, AppError>;
    /// Replace the OIDC-managed persisted memberships for this user with the
    /// groups mapped from the current OIDC claims. Manual memberships are left
    /// untouched.
    async fn sync_oidc_group_memberships(
        &self,
        user_id: Uuid,
        oidc_group_ids: &[String],
    ) -> Result<(), AppError>;
}

#[async_trait]
pub trait GroupRepository: Send + Sync {
    async fn get_by_id(&self, group_id: Uuid) -> Result<Option<Group>, AppError>;
    async fn list_groups(&self) -> Result<Vec<Group>, AppError>;
    async fn list_groups_page(
        &self,
        filter: NamedListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Group>, AppError>;
    async fn create_group(&self, group: Group) -> Result<Group, AppError>;
    async fn update_group(&self, group: Group) -> Result<Group, AppError>;
    async fn delete_group(&self, group_id: Uuid) -> Result<bool, AppError>;
}

#[async_trait]
pub trait UserCredentialRepository: Send + Sync {
    async fn get_by_user_id(&self, user_id: Uuid) -> Result<Option<UserCredential>, AppError>;
    async fn upsert_credential_hash(
        &self,
        user_id: Uuid,
        password_hash: &str,
        password_changed_at: DateTime<Utc>,
    ) -> Result<UserCredential, AppError>;
}

#[async_trait]
pub trait AuthSessionRepository: Send + Sync {
    async fn create_session(
        &self,
        session: AuthSession,
        audit_event: AuditEvent,
    ) -> Result<AuthSession, AppError>;
    async fn get_session(&self, session_id: Uuid) -> Result<Option<AuthSession>, AppError>;
    async fn extend_session(
        &self,
        session_id: Uuid,
        new_expires_at: DateTime<Utc>,
    ) -> Result<(), AppError>;
    async fn revoke_session(
        &self,
        session_id: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<(), AppError>;
}

#[async_trait]
pub trait RoleRepository: Send + Sync {
    async fn list_roles_for_user(&self, user_id: Uuid) -> Result<Vec<Role>, AppError>;
    async fn get_by_id(&self, role_id: Uuid) -> Result<Option<Role>, AppError>;
    async fn list_roles(&self) -> Result<Vec<Role>, AppError>;
    async fn list_roles_page(
        &self,
        filter: NamedListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Role>, AppError>;
    async fn create_role(&self, role: Role) -> Result<Role, AppError>;
    async fn update_role(&self, role: Role) -> Result<Role, AppError>;
    async fn delete_role(&self, role_id: Uuid) -> Result<bool, AppError>;
}

#[async_trait]
pub trait RoleAssignmentRepository: Send + Sync {
    async fn list_assignments(&self) -> Result<Vec<RoleAssignment>, AppError>;
    async fn list_assignments_filtered(
        &self,
        filter: RoleAssignmentFilter,
    ) -> Result<Vec<RoleAssignment>, AppError>;
    async fn list_assignments_page(
        &self,
        filter: RoleAssignmentFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<RoleAssignment>, AppError>;
    async fn create_assignment(
        &self,
        assignment: RoleAssignment,
    ) -> Result<RoleAssignment, AppError>;
    async fn delete_assignment(&self, assignment_id: Uuid) -> Result<bool, AppError>;
}

#[async_trait]
pub trait PolicyRepository: Send + Sync {
    async fn list_policies(&self, filter: PolicyFilter) -> Result<Vec<Policy>, AppError>;
    async fn list_policies_page(
        &self,
        filter: PolicyFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Policy>, AppError>;
    async fn get_policy(&self, policy_id: Uuid) -> Result<Option<Policy>, AppError>;
    async fn create_policy(&self, policy: Policy) -> Result<Policy, AppError>;
    async fn update_policy(&self, policy: Policy) -> Result<Policy, AppError>;
    async fn delete_policy(&self, policy_id: Uuid) -> Result<(), AppError>;
    async fn set_policy_enabled(&self, policy_id: Uuid, enabled: bool) -> Result<Policy, AppError>;
    async fn list_for_subjects(&self, subjects: &[String]) -> Result<Vec<Policy>, AppError>;
}

#[async_trait]
pub trait OidcSettingsRepository: Send + Sync {
    async fn get_settings(&self) -> Result<Option<OidcSettings>, AppError>;
    async fn upsert_settings(&self, settings: OidcSettings) -> Result<OidcSettings, AppError>;
}
