use std::sync::Arc;
use std::time::Instant;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
use chrono::Utc;
use redis::aio::ConnectionManager;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::password_generator::generate_local_password;
use super::password_policy::validate_local_password;
use super::policy_cache::{bump_policy_cache_generation, bump_policy_cache_user_generation};
use crate::config::AuthConfig;
use crate::contexts::access_control::domain::{
    CredentialProvisioningCommand, CredentialProvisioningMode, Group, Policy, PolicyEffect, Role,
    RoleAssignment, User, UserCredential,
};
use crate::contexts::access_control::infrastructure::persistence::{
    GroupRepository, NamedListFilter, PolicyFilter, PolicyRepository, RoleAssignmentFilter,
    RoleAssignmentRepository, RoleRepository, UserCredentialRepository, UserListFilter,
    UserRepository,
};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentApplier,
};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_event, metrics};

pub struct AccessControlAdminService<URepo, CRepo, GRepo, RRepo, RARepo, PRepo, SFI>
where
    URepo: UserRepository,
    CRepo: UserCredentialRepository,
    GRepo: GroupRepository,
    RRepo: RoleRepository,
    RARepo: RoleAssignmentRepository,
    PRepo: PolicyRepository,
    SFI: SecretFieldIntentApplier,
{
    users: Arc<URepo>,
    credentials: Arc<CRepo>,
    groups: Arc<GRepo>,
    roles: Arc<RRepo>,
    role_assignments: Arc<RARepo>,
    policies: Arc<PRepo>,
    auth_config: AuthConfig,
    secret_field_intents: Arc<SFI>,
    policy_cache: Option<Arc<Mutex<ConnectionManager>>>,
}

pub struct AccessControlAdminDependencies<URepo, CRepo, GRepo, RRepo, RARepo, PRepo, SFI>
where
    URepo: UserRepository,
    CRepo: UserCredentialRepository,
    GRepo: GroupRepository,
    RRepo: RoleRepository,
    RARepo: RoleAssignmentRepository,
    PRepo: PolicyRepository,
    SFI: SecretFieldIntentApplier,
{
    pub users: Arc<URepo>,
    pub credentials: Arc<CRepo>,
    pub groups: Arc<GRepo>,
    pub roles: Arc<RRepo>,
    pub role_assignments: Arc<RARepo>,
    pub policies: Arc<PRepo>,
    pub auth_config: AuthConfig,
    pub secret_field_intents: Arc<SFI>,
}

#[derive(Debug, Clone)]
pub struct RoleGrants {
    pub role_id: Uuid,
    pub assignments: Vec<RoleAssignment>,
    /// Policies granted to this role: policies whose `subjects` contains
    /// `role:{role_id}` (the role→policy linkage after `rbac_policy_binding` was
    /// folded into policy subjects).
    pub policies: Vec<Policy>,
}

#[derive(Debug, Clone)]
pub struct PasswordProvisioningResult {
    pub credential: UserCredential,
    pub generated_password: Option<String>,
}

/// List parameters for users: free-text search, multi-value `status` /
/// `auth_source` IN filters, and an allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct UserListParams {
    pub search: Option<String>,
    pub status: Vec<String>,
    pub auth_source: Vec<String>,
    pub sort: Option<SortSpec>,
}

/// List parameters shared by groups and roles: free-text search plus an
/// allowlisted sort directive.
#[derive(Debug, Clone, Default)]
pub struct NamedListParams {
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct RoleAssignmentListParams {
    /// Multi-value `role_id` filter (matches any).
    pub role_id: Vec<Uuid>,
    /// Multi-value `user_id` filter (matches any).
    pub user_id: Vec<Uuid>,
    /// Multi-value `group_id` filter (matches any).
    pub group_id: Vec<Uuid>,
    /// Multi-value subject type filter: user, group.
    pub subject_type: Vec<String>,
    /// Free-text filter over the assignment's role name and subject
    /// (user username/email, group name). Empty/whitespace means "no filter".
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct PolicyListParams {
    pub name: Option<String>,
    /// Multi-value `effect` filter (matches any).
    pub effect: Vec<PolicyEffect>,
    /// Multi-value `enabled` filter (matches any).
    pub enabled: Vec<bool>,
    pub sort: Option<SortSpec>,
}

fn normalize_search(search: Option<String>) -> Option<String> {
    search
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

impl From<UserListParams> for UserListFilter {
    fn from(value: UserListParams) -> Self {
        Self {
            search: normalize_search(value.search).map(|term| format!("%{term}%")),
            status: value.status,
            auth_source: value.auth_source,
            sort: value.sort,
        }
    }
}

impl From<NamedListParams> for NamedListFilter {
    fn from(value: NamedListParams) -> Self {
        Self {
            search: normalize_search(value.search).map(|term| format!("%{term}%")),
            sort: value.sort,
        }
    }
}

impl From<RoleAssignmentListParams> for RoleAssignmentFilter {
    fn from(value: RoleAssignmentListParams) -> Self {
        Self {
            role_id: value.role_id,
            user_id: value.user_id,
            group_id: value.group_id,
            subject_type: value.subject_type,
            search: normalize_search(value.search),
            sort: value.sort,
        }
    }
}

impl From<PolicyListParams> for PolicyFilter {
    fn from(value: PolicyListParams) -> Self {
        Self {
            name: value.name,
            effect: value.effect,
            enabled: value.enabled,
            sort: value.sort,
        }
    }
}

const POLICY_CACHE_METRIC_NAME: &str = "access_control_policies";
const USER_PASSWORD_SECRET_CONSUMER_TYPE: &str = "user_auth_password";
const USER_PASSWORD_SECRET_FIELD_KEY: &str = "local_password";
const USER_PASSWORD_SECRET_OWNER_SCOPE_TYPE: &str = "user";

impl<URepo, CRepo, GRepo, RRepo, RARepo, PRepo, SFI>
    AccessControlAdminService<URepo, CRepo, GRepo, RRepo, RARepo, PRepo, SFI>
where
    URepo: UserRepository,
    CRepo: UserCredentialRepository,
    GRepo: GroupRepository,
    RRepo: RoleRepository,
    RARepo: RoleAssignmentRepository,
    PRepo: PolicyRepository,
    SFI: SecretFieldIntentApplier,
{
    pub fn new(
        dependencies: AccessControlAdminDependencies<
            URepo,
            CRepo,
            GRepo,
            RRepo,
            RARepo,
            PRepo,
            SFI,
        >,
    ) -> Self {
        Self::from_dependencies(dependencies, None)
    }

    pub fn new_with_cache(
        dependencies: AccessControlAdminDependencies<
            URepo,
            CRepo,
            GRepo,
            RRepo,
            RARepo,
            PRepo,
            SFI,
        >,
        policy_cache: Arc<Mutex<ConnectionManager>>,
    ) -> Self {
        Self::from_dependencies(dependencies, Some(policy_cache))
    }

    fn from_dependencies(
        dependencies: AccessControlAdminDependencies<
            URepo,
            CRepo,
            GRepo,
            RRepo,
            RARepo,
            PRepo,
            SFI,
        >,
        policy_cache: Option<Arc<Mutex<ConnectionManager>>>,
    ) -> Self {
        let AccessControlAdminDependencies {
            users,
            credentials,
            groups,
            roles,
            role_assignments,
            policies,
            auth_config,
            secret_field_intents,
        } = dependencies;
        Self {
            users,
            credentials,
            groups,
            roles,
            role_assignments,
            policies,
            auth_config,
            secret_field_intents,
            policy_cache,
        }
    }

    pub async fn list_users(&self) -> Result<Vec<User>, AppError> {
        self.users.list_users().await
    }

    pub async fn list_users_page(
        &self,
        params: UserListParams,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<User>, AppError> {
        self.users
            .list_users_page(params.into(), limit, offset)
            .await
    }

    pub async fn get_user(&self, user_id: Uuid) -> Result<Option<User>, AppError> {
        self.users.get_by_id(user_id).await
    }

    pub async fn create_user(&self, user: User) -> Result<User, AppError> {
        let created = self.users.create_user(user).await?;
        self.invalidate_policy_cache_for_user_best_effort(created.id)
            .await;
        Ok(created)
    }

    pub async fn update_user(&self, user: User) -> Result<User, AppError> {
        let updated = self.users.update_user(user).await?;
        self.invalidate_policy_cache_for_user_best_effort(updated.id)
            .await;
        Ok(updated)
    }

    pub async fn delete_user(&self, user_id: Uuid) -> Result<bool, AppError> {
        let deleted = self.users.delete_user(user_id).await?;
        if deleted {
            self.invalidate_policy_cache_for_user_best_effort(user_id)
                .await;
        }
        Ok(deleted)
    }

    pub async fn set_local_password_manual(
        &self,
        user_id: Uuid,
        password: &str,
    ) -> Result<UserCredential, AppError> {
        let user = self.ensure_local_user_for_credentials(user_id).await?;
        let password = password.trim();
        validate_local_password(password, self.auth_config.password_min_length)?;
        let _ = self
            .secret_field_intents
            .apply_intent(ApplySecretFieldIntentInput {
                consumer_type: USER_PASSWORD_SECRET_CONSUMER_TYPE.to_string(),
                consumer_id: user.id,
                owner_scope_type: USER_PASSWORD_SECRET_OWNER_SCOPE_TYPE.to_string(),
                owner_scope_id: user.id,
                field_key: USER_PASSWORD_SECRET_FIELD_KEY.to_string(),
                intent: SecretFieldIntent::Rotate,
                value: Some(password.to_string()),
                secret_id: None,
            })
            .await?;
        let hash = self.hash_password(password)?;
        let credential = self
            .credentials
            .upsert_credential_hash(user.id, &hash, Utc::now())
            .await?;
        self.invalidate_policy_cache_for_user_best_effort(user.id)
            .await;
        Ok(credential)
    }

    pub async fn set_local_password_generated(
        &self,
        user_id: Uuid,
    ) -> Result<(UserCredential, String), AppError> {
        let user = self.ensure_local_user_for_credentials(user_id).await?;
        let generated_password = generate_local_password(self.auth_config.password_min_length)?;
        let _ = self
            .secret_field_intents
            .apply_intent(ApplySecretFieldIntentInput {
                consumer_type: USER_PASSWORD_SECRET_CONSUMER_TYPE.to_string(),
                consumer_id: user.id,
                owner_scope_type: USER_PASSWORD_SECRET_OWNER_SCOPE_TYPE.to_string(),
                owner_scope_id: user.id,
                field_key: USER_PASSWORD_SECRET_FIELD_KEY.to_string(),
                intent: SecretFieldIntent::Rotate,
                value: Some(generated_password.clone()),
                secret_id: None,
            })
            .await?;
        let hash = self.hash_password(&generated_password)?;
        let credential = self
            .credentials
            .upsert_credential_hash(user.id, &hash, Utc::now())
            .await?;
        self.invalidate_policy_cache_for_user_best_effort(user.id)
            .await;
        Ok((credential, generated_password))
    }

    pub async fn set_local_password_with_command(
        &self,
        user_id: Uuid,
        command: CredentialProvisioningCommand,
    ) -> Result<PasswordProvisioningResult, AppError> {
        match command.mode {
            CredentialProvisioningMode::Manual => {
                let password = command.password.ok_or_else(|| {
                    AppError::Validation(
                        "Password is required when credential mode is manual".to_string(),
                    )
                })?;
                let credential = self.set_local_password_manual(user_id, &password).await?;
                Ok(PasswordProvisioningResult {
                    credential,
                    generated_password: None,
                })
            }
            CredentialProvisioningMode::Generate => {
                let (credential, generated_password) =
                    self.set_local_password_generated(user_id).await?;
                Ok(PasswordProvisioningResult {
                    credential,
                    generated_password: Some(generated_password),
                })
            }
        }
    }

    pub async fn list_groups(&self) -> Result<Vec<Group>, AppError> {
        self.groups.list_groups().await
    }

    pub async fn list_groups_page(
        &self,
        params: NamedListParams,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Group>, AppError> {
        self.groups
            .list_groups_page(params.into(), limit, offset)
            .await
    }

    pub async fn get_group(&self, group_id: Uuid) -> Result<Option<Group>, AppError> {
        self.groups.get_by_id(group_id).await
    }

    pub async fn create_group(&self, group: Group) -> Result<Group, AppError> {
        let created = self.groups.create_group(group).await?;
        self.invalidate_policy_cache_best_effort().await;
        Ok(created)
    }

    pub async fn update_group(&self, group: Group) -> Result<Group, AppError> {
        let updated = self.groups.update_group(group).await?;
        self.invalidate_policy_cache_best_effort().await;
        Ok(updated)
    }

    pub async fn delete_group(&self, group_id: Uuid) -> Result<bool, AppError> {
        let deleted = self.groups.delete_group(group_id).await?;
        if deleted {
            self.invalidate_policy_cache_best_effort().await;
        }
        Ok(deleted)
    }

    pub async fn list_roles(&self) -> Result<Vec<Role>, AppError> {
        self.roles.list_roles().await
    }

    pub async fn list_roles_page(
        &self,
        params: NamedListParams,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Role>, AppError> {
        self.roles
            .list_roles_page(params.into(), limit, offset)
            .await
    }

    pub async fn get_role(&self, role_id: Uuid) -> Result<Option<Role>, AppError> {
        self.roles.get_by_id(role_id).await
    }

    pub async fn create_role(&self, role: Role) -> Result<Role, AppError> {
        let created = self.roles.create_role(role).await?;
        self.invalidate_policy_cache_best_effort().await;
        Ok(created)
    }

    pub async fn update_role(&self, role: Role) -> Result<Role, AppError> {
        let updated = self.roles.update_role(role).await?;
        self.invalidate_policy_cache_best_effort().await;
        Ok(updated)
    }

    pub async fn delete_role(&self, role_id: Uuid) -> Result<bool, AppError> {
        let deleted = self.roles.delete_role(role_id).await?;
        if deleted {
            self.invalidate_policy_cache_enforced().await?;
        }
        Ok(deleted)
    }

    pub async fn list_role_assignments(&self) -> Result<Vec<RoleAssignment>, AppError> {
        self.role_assignments.list_assignments().await
    }

    pub async fn list_role_assignments_filtered(
        &self,
        params: RoleAssignmentListParams,
    ) -> Result<Vec<RoleAssignment>, AppError> {
        self.role_assignments
            .list_assignments_filtered(params.into())
            .await
    }

    pub async fn list_role_assignments_page(
        &self,
        params: RoleAssignmentListParams,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<RoleAssignment>, AppError> {
        self.role_assignments
            .list_assignments_page(params.into(), limit, offset)
            .await
    }

    pub async fn create_role_assignment(
        &self,
        assignment: RoleAssignment,
    ) -> Result<RoleAssignment, AppError> {
        let created = self.role_assignments.create_assignment(assignment).await?;
        if let Some(user_id) = created.user_id {
            self.invalidate_policy_cache_for_user_best_effort(user_id)
                .await;
        } else {
            self.invalidate_policy_cache_best_effort().await;
        }
        Ok(created)
    }

    pub async fn delete_role_assignment(&self, assignment_id: Uuid) -> Result<bool, AppError> {
        let deleted = self
            .role_assignments
            .delete_assignment(assignment_id)
            .await?;
        if deleted {
            self.invalidate_policy_cache_enforced().await?;
        }
        Ok(deleted)
    }

    pub async fn get_role_grants(&self, role_id: Uuid) -> Result<RoleGrants, AppError> {
        if self.roles.get_by_id(role_id).await?.is_none() {
            return Err(AppError::NotFound("Role not found".to_string()));
        }
        let assignments = self
            .role_assignments
            .list_assignments_filtered(RoleAssignmentFilter {
                role_id: vec![role_id],
                ..RoleAssignmentFilter::default()
            })
            .await?;
        // Role→policy linkage is now a `role:{uuid}` subject match (the
        // `rbac_policy_binding` table is gone).
        let policies = self
            .policies
            .list_for_subjects(&[format!("role:{role_id}")])
            .await?;
        Ok(RoleGrants {
            role_id,
            assignments,
            policies,
        })
    }

    pub async fn list_policies(&self, params: PolicyListParams) -> Result<Vec<Policy>, AppError> {
        self.policies.list_policies(params.into()).await
    }

    pub async fn list_policies_page(
        &self,
        params: PolicyListParams,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Policy>, AppError> {
        self.policies
            .list_policies_page(params.into(), limit, offset)
            .await
    }

    pub async fn get_policy(&self, policy_id: Uuid) -> Result<Option<Policy>, AppError> {
        self.policies.get_policy(policy_id).await
    }

    pub async fn create_policy(&self, policy: Policy) -> Result<Policy, AppError> {
        let created = self.policies.create_policy(policy).await?;
        self.invalidate_policy_cache_best_effort().await;
        Ok(created)
    }

    pub async fn update_policy(&self, policy: Policy) -> Result<Policy, AppError> {
        let updated = self.policies.update_policy(policy).await?;
        self.invalidate_policy_cache_best_effort().await;
        Ok(updated)
    }

    pub async fn delete_policy(&self, policy_id: Uuid) -> Result<(), AppError> {
        self.policies.delete_policy(policy_id).await?;
        self.invalidate_policy_cache_enforced().await?;
        Ok(())
    }

    pub async fn set_policy_enabled(
        &self,
        policy_id: Uuid,
        enabled: bool,
    ) -> Result<Policy, AppError> {
        let policy = self.policies.set_policy_enabled(policy_id, enabled).await?;
        if enabled {
            // Enabling a policy is access-increasing for an allow effect; a failed
            // invalidation only leaves a fail-safe stale-DENY, so stay best-effort.
            self.invalidate_policy_cache_best_effort().await;
        } else {
            // Disabling a policy can revoke grants (allow effect); enforce.
            self.invalidate_policy_cache_enforced().await?;
        }
        Ok(policy)
    }

    pub async fn invalidate_policy_cache(&self) -> Result<u64, AppError> {
        let Some(policy_cache) = &self.policy_cache else {
            return Ok(0);
        };
        let started_at = Instant::now();
        let mut conn = policy_cache.lock().await;
        match bump_policy_cache_generation(&mut conn).await {
            Ok(generation) => {
                metrics::record_cache_operation(
                    POLICY_CACHE_METRIC_NAME,
                    "invalidate",
                    "ok",
                    started_at.elapsed().as_secs_f64(),
                );
                Ok(generation)
            }
            Err(error) => {
                metrics::record_cache_operation(
                    POLICY_CACHE_METRIC_NAME,
                    "invalidate",
                    "error",
                    started_at.elapsed().as_secs_f64(),
                );
                Err(error)
            }
        }
    }

    pub async fn invalidate_policy_cache_for_user(&self, user_id: Uuid) -> Result<u64, AppError> {
        let Some(policy_cache) = &self.policy_cache else {
            return Ok(0);
        };
        let started_at = Instant::now();
        let mut conn = policy_cache.lock().await;
        match bump_policy_cache_user_generation(&mut conn, user_id).await {
            Ok(generation) => {
                metrics::record_cache_operation(
                    POLICY_CACHE_METRIC_NAME,
                    "invalidate_user",
                    "ok",
                    started_at.elapsed().as_secs_f64(),
                );
                Ok(generation)
            }
            Err(error) => {
                metrics::record_cache_operation(
                    POLICY_CACHE_METRIC_NAME,
                    "invalidate_user",
                    "error",
                    started_at.elapsed().as_secs_f64(),
                );
                Err(error)
            }
        }
    }

    fn hash_password(&self, password: &str) -> Result<String, AppError> {
        let password_input = format!("{}{}", password, self.auth_config.password_pepper);
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password_input.as_bytes(), &salt)
            .map_err(|error| AppError::Validation(format!("Failed to hash password: {}", error)))
            .map(|hash| hash.to_string())
    }

    async fn ensure_local_user_for_credentials(&self, user_id: Uuid) -> Result<User, AppError> {
        let user = self
            .users
            .get_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
        if !self.auth_config.local_enabled {
            return Err(AppError::Validation(
                "Local authentication is disabled".to_string(),
            ));
        }
        if !user.auth_source.eq_ignore_ascii_case("local") {
            return Err(AppError::Validation(
                "Credentials can only be managed for local users".to_string(),
            ));
        }
        Ok(user)
    }

    async fn invalidate_policy_cache_best_effort(&self) {
        if let Err(error) = self.invalidate_policy_cache().await {
            log_event(
                LogLevel::Warning,
                "access_control.policy_cache.invalidate.failed",
                EventFields::default()
                    .with_operation("access_control.policy_cache.invalidate")
                    .with_resource("policy_cache", "global_generation")
                    .with_error("policy_cache_invalidation_error", error.to_string())
                    .with_message("Access control policy cache generation bump failed"),
            );
        }
    }

    /// Strict invalidation for access-reducing mutations (revoking a grant:
    /// role/policy/assignment delete, policy disable). A failed cache-generation
    /// bump here is a security risk — a revoked ALLOW could keep being served
    /// from a stale cache entry until its TTL — so unlike the best-effort path it
    /// is escalated to an error log and propagated to the caller. Tradeoff: the
    /// DB mutation has already committed, so a propagated failure is a partial
    /// success (the change is durable but cache invalidation lagged); surfacing
    /// it lets the caller retry, which is preferred to silently serving a revoked
    /// grant. Access-increasing mutations must NOT use this path: there, a failed
    /// invalidation only leaves a stale-DENY, which is fail-safe.
    async fn invalidate_policy_cache_enforced(&self) -> Result<(), AppError> {
        if let Err(error) = self.invalidate_policy_cache().await {
            log_event(
                LogLevel::Error,
                "access_control.policy_cache.invalidate.failed",
                EventFields::default()
                    .with_operation("access_control.policy_cache.invalidate")
                    .with_resource("policy_cache", "global_generation")
                    .with_error("policy_cache_invalidation_error", error.to_string())
                    .with_message(
                        "Access control policy cache generation bump failed for an \
                         access-reducing mutation; a revoked grant may be served from \
                         cache until TTL",
                    ),
            );
            return Err(error);
        }
        Ok(())
    }

    async fn invalidate_policy_cache_for_user_best_effort(&self, user_id: Uuid) {
        if let Err(error) = self.invalidate_policy_cache_for_user(user_id).await {
            log_event(
                LogLevel::Warning,
                "access_control.policy_cache.invalidate.failed",
                EventFields::default()
                    .with_operation("access_control.policy_cache.invalidate_user")
                    .with_resource("user_policy_cache", user_id.to_string())
                    .with_error("policy_cache_invalidation_error", error.to_string())
                    .with_message("Access control policy cache user generation bump failed"),
            );
        }
    }
}
