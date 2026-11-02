//! Service-layer unit tests with in-memory fake repositories.
//!
//! Demonstrates the pattern of testing services (e.g. PolicyEvaluationService)
//! using fake/in-memory implementations of repository traits, without PostgreSQL or Redis.
//!
//! Run with: `cargo test --test service_layer`

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures_util::future::BoxFuture;
use uuid::Uuid;

use piping_backend::config::ApiTokenConfig;
use piping_backend::contexts::access_control::application::{
    AccessControlService, PolicyEvaluationService, UserApiTokenService,
};
use piping_backend::contexts::access_control::domain::{
    Action, Group, Policy, PolicyDecision, PolicyEffect, ResourceType, Role, User, UserApiToken,
    UserApiTokenScope, UserApiTokenStatus,
};
use piping_backend::contexts::access_control::infrastructure::persistence::{
    NamedListFilter, PolicyFilter, PolicyRepository, RoleRepository, UserApiTokenListFilter,
    UserApiTokenRepository, UserListFilter, UserRepository,
};
use piping_backend::contexts::audit::domain::AuditEvent;
use piping_backend::contexts::shared::pagination::PageResult;
use piping_backend::contexts::shared::request_context::{
    ApiTokenScopeContext, RequestAuth, RequestContext,
};
use piping_backend::contexts::sync::application::{
    PolicyRequestCommitAuthority, RequestCommitAuthorisation,
};
use piping_backend::contexts::sync::domain::inventory_sync::InventoryCommitAuthorisation;
use piping_backend::errors::AppError;

// --- In-memory fake repositories ---

/// In-memory UserRepository for service-layer tests.
struct InMemoryUserRepository {
    /// Groups keyed by their `external_id`, returned when that external id is in
    /// the requested set — mirrors the production `external_id = ANY($1)` match.
    groups_by_external_id: HashMap<String, Vec<Group>>,
    persisted_groups: Vec<Group>,
    /// Rows returned by `get_by_id`, keyed by user id.
    users: HashMap<Uuid, User>,
}

impl InMemoryUserRepository {
    fn new() -> Self {
        Self {
            groups_by_external_id: HashMap::new(),
            persisted_groups: Vec::new(),
            users: HashMap::new(),
        }
    }

    fn with_user(mut self, user: User) -> Self {
        self.users.insert(user.id, user);
        self
    }

    #[allow(dead_code)]
    fn with_group_for_external_id(mut self, external_id: &str, groups: Vec<Group>) -> Self {
        self.groups_by_external_id
            .insert(external_id.to_string(), groups);
        self
    }

    #[allow(dead_code)]
    fn with_persisted_groups(mut self, groups: Vec<Group>) -> Self {
        self.persisted_groups = groups;
        self
    }
}

#[async_trait]
impl UserRepository for InMemoryUserRepository {
    async fn get_by_id(&self, user_id: Uuid) -> Result<Option<User>, AppError> {
        Ok(self.users.get(&user_id).cloned())
    }

    async fn get_by_username_or_email(&self, _identifier: &str) -> Result<Option<User>, AppError> {
        Ok(None)
    }

    async fn get_by_external_id(&self, _external_id: &str) -> Result<Option<User>, AppError> {
        Ok(None)
    }

    async fn list_users(&self) -> Result<Vec<User>, AppError> {
        Ok(Vec::new())
    }

    async fn list_users_page(
        &self,
        _filter: UserListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<User>, AppError> {
        Ok(PageResult {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
        })
    }

    async fn create_user(&self, user: User) -> Result<User, AppError> {
        Ok(user)
    }

    async fn update_user(&self, user: User) -> Result<User, AppError> {
        Ok(user)
    }

    async fn delete_user(&self, _user_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn list_groups_by_external_ids(
        &self,
        external_ids: &[String],
    ) -> Result<Vec<Group>, AppError> {
        Ok(external_ids
            .iter()
            .filter_map(|external_id| self.groups_by_external_id.get(external_id))
            .flat_map(|groups| groups.iter().cloned())
            .collect())
    }

    async fn list_groups_for_user(&self, _user_id: Uuid) -> Result<Vec<Group>, AppError> {
        Ok(self.persisted_groups.clone())
    }

    async fn sync_oidc_group_memberships(
        &self,
        _user_id: Uuid,
        _oidc_group_ids: &[String],
    ) -> Result<(), AppError> {
        Ok(())
    }
}

/// In-memory RoleRepository for service-layer tests.
struct InMemoryRoleRepository {
    roles_by_user: HashMap<Uuid, Vec<Role>>,
}

impl InMemoryRoleRepository {
    fn new() -> Self {
        Self {
            roles_by_user: HashMap::new(),
        }
    }

    fn with_roles(mut self, user_id: Uuid, roles: Vec<Role>) -> Self {
        self.roles_by_user.insert(user_id, roles);
        self
    }
}

#[async_trait]
impl RoleRepository for InMemoryRoleRepository {
    async fn list_roles_for_user(&self, user_id: Uuid) -> Result<Vec<Role>, AppError> {
        let mut roles = self
            .roles_by_user
            .get(&user_id)
            .cloned()
            .unwrap_or_default();
        roles.sort_by_key(|role| role.id);
        roles.dedup_by_key(|role| role.id);
        Ok(roles)
    }

    async fn get_by_id(&self, _role_id: Uuid) -> Result<Option<Role>, AppError> {
        Ok(None)
    }

    async fn list_roles(&self) -> Result<Vec<Role>, AppError> {
        Ok(Vec::new())
    }

    async fn list_roles_page(
        &self,
        _filter: NamedListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Role>, AppError> {
        Ok(PageResult {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
        })
    }

    async fn create_role(&self, role: Role) -> Result<Role, AppError> {
        Ok(role)
    }

    async fn update_role(&self, role: Role) -> Result<Role, AppError> {
        Ok(role)
    }

    async fn delete_role(&self, _role_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }
}

/// In-memory PolicyRepository for service-layer tests.
struct InMemoryPolicyRepository {
    policies_by_subjects: Vec<Policy>,
}

impl InMemoryPolicyRepository {
    fn new() -> Self {
        Self {
            policies_by_subjects: Vec::new(),
        }
    }

    fn with_policies_for_subjects(mut self, policies: Vec<Policy>) -> Self {
        self.policies_by_subjects = policies;
        self
    }
}

#[async_trait]
impl PolicyRepository for InMemoryPolicyRepository {
    async fn list_policies(&self, _filter: PolicyFilter) -> Result<Vec<Policy>, AppError> {
        Ok(Vec::new())
    }

    async fn list_policies_page(
        &self,
        _filter: PolicyFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Policy>, AppError> {
        Ok(PageResult {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
        })
    }

    async fn get_policy(&self, _policy_id: Uuid) -> Result<Option<Policy>, AppError> {
        Ok(None)
    }

    async fn create_policy(&self, policy: Policy) -> Result<Policy, AppError> {
        Ok(policy)
    }

    async fn update_policy(&self, policy: Policy) -> Result<Policy, AppError> {
        Ok(policy)
    }

    async fn delete_policy(&self, _policy_id: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    async fn set_policy_enabled(
        &self,
        _policy_id: Uuid,
        _enabled: bool,
    ) -> Result<Policy, AppError> {
        Err(AppError::Internal(
            "InMemoryPolicyRepository::set_policy_enabled not used in tests".into(),
        ))
    }

    async fn list_for_subjects(&self, subjects: &[String]) -> Result<Vec<Policy>, AppError> {
        Ok(self
            .policies_by_subjects
            .iter()
            .filter(|policy| {
                policy
                    .subjects
                    .iter()
                    .any(|subject| subjects.iter().any(|requested| requested == subject))
            })
            .cloned()
            .collect())
    }
}

// --- Tests ---

/// In-memory UserApiTokenRepository holding at most one stored token row.
///
/// Deliberately narrow: only `find_token_with_scopes` is meaningful, because the
/// deferred-authorization boundary re-reads an existing token by id and never
/// mints, lists or revokes one.
struct InMemoryUserApiTokenRepository {
    token: Option<UserApiToken>,
}

impl InMemoryUserApiTokenRepository {
    fn new(token: Option<UserApiToken>) -> Self {
        Self { token }
    }

    fn not_used(operation: &str) -> AppError {
        AppError::Internal(format!(
            "InMemoryUserApiTokenRepository::{operation} is not used in tests"
        ))
    }
}

#[async_trait]
impl UserApiTokenRepository for InMemoryUserApiTokenRepository {
    async fn create_token(
        &self,
        _token: UserApiToken,
        _audit_event: AuditEvent,
    ) -> Result<UserApiToken, AppError> {
        Err(Self::not_used("create_token"))
    }

    async fn list_user_tokens_page(
        &self,
        _user_id: Uuid,
        _limit: u32,
        _offset: u32,
        _filter: UserApiTokenListFilter,
    ) -> Result<Vec<UserApiToken>, AppError> {
        Err(Self::not_used("list_user_tokens_page"))
    }

    async fn count_user_tokens(
        &self,
        _user_id: Uuid,
        _filter: UserApiTokenListFilter,
    ) -> Result<u64, AppError> {
        Err(Self::not_used("count_user_tokens"))
    }

    async fn find_token_with_scopes(
        &self,
        token_id: Uuid,
    ) -> Result<Option<UserApiToken>, AppError> {
        Ok(self.token.clone().filter(|token| token.id == token_id))
    }

    async fn revoke_user_token(
        &self,
        _user_id: Uuid,
        _token_id: Uuid,
        _now: DateTime<Utc>,
        _audit_event: AuditEvent,
    ) -> Result<Option<UserApiToken>, AppError> {
        Err(Self::not_used("revoke_user_token"))
    }

    async fn touch_last_used(&self, _token_id: Uuid, _now: DateTime<Utc>) -> Result<(), AppError> {
        Ok(())
    }

    async fn count_active_tokens_for_user(
        &self,
        _user_id: Uuid,
        _now: DateTime<Utc>,
    ) -> Result<u32, AppError> {
        Err(Self::not_used("count_active_tokens_for_user"))
    }
}

fn allow_read_host_policy(role_id: Uuid) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "allow-read-connection_target".to_string(),
        description: None,
        effect: PolicyEffect::Allow,
        actions: vec![Action::Read],
        resource_types: vec![ResourceType::ConnectionTarget],
        constraints: None,
        subjects: vec![format!("role:{role_id}")],
        resources: Vec::new(),
        enabled: true,
    }
}

fn allow_update_host_policy(role_id: Uuid) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "allow-update-connection_target".to_string(),
        description: None,
        effect: PolicyEffect::Allow,
        actions: vec![Action::Update],
        resource_types: vec![ResourceType::ConnectionTarget],
        constraints: None,
        subjects: vec![format!("role:{role_id}")],
        resources: Vec::new(),
        enabled: true,
    }
}

fn request_context(user_id: Uuid) -> RequestContext {
    request_context_with_oidc_groups(user_id, Vec::new())
}

fn request_context_with_oidc_groups(user_id: Uuid, oidc_groups: Vec<String>) -> RequestContext {
    RequestContext::new(
        Some(user_id),
        "session".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth {
            oidc_groups,
            ..RequestAuth::default()
        },
    )
}

#[tokio::test]
async fn persisted_group_viewer_role_does_not_drop_manual_user_elevation() {
    let user_id = Uuid::now_v7();
    let viewer_role = Role {
        id: Uuid::now_v7(),
        name: "viewer".to_string(),
        description: None,
    };
    let editor_role = Role {
        id: Uuid::now_v7(),
        name: "editor".to_string(),
        description: None,
    };

    let user_repo = InMemoryUserRepository::new();
    let role_repo = InMemoryRoleRepository::new()
        .with_roles(user_id, vec![editor_role.clone(), viewer_role.clone()]);
    let policy_repo = InMemoryPolicyRepository::new().with_policies_for_subjects(vec![
        allow_read_host_policy(viewer_role.id),
        allow_update_host_policy(editor_role.id),
    ]);

    let access_control = AccessControlService::new(user_repo, role_repo, policy_repo);
    let policy_service = PolicyEvaluationService::new(access_control);

    let ctx = request_context(user_id);
    let viewer_decision = policy_service
        .evaluate(&ctx, Action::Read, ResourceType::ConnectionTarget, None)
        .await
        .expect("evaluate viewer role");
    let manual_elevation_decision = policy_service
        .evaluate(&ctx, Action::Update, ResourceType::ConnectionTarget, None)
        .await
        .expect("evaluate manual role");
    let denied_decision = policy_service
        .evaluate(&ctx, Action::Delete, ResourceType::ConnectionTarget, None)
        .await
        .expect("evaluate missing role");

    assert_eq!(viewer_decision, PolicyDecision::Allow);
    assert_eq!(manual_elevation_decision, PolicyDecision::Allow);
    assert_eq!(denied_decision, PolicyDecision::Deny);
}

#[tokio::test]
async fn policy_evaluation_service_denies_when_no_policies() {
    let user_id = Uuid::now_v7();
    let role_id = Uuid::now_v7();

    let user_repo = InMemoryUserRepository::new();
    let role_repo = InMemoryRoleRepository::new().with_roles(
        user_id,
        vec![Role {
            id: role_id,
            name: "test-role".to_string(),
            description: None,
        }],
    );
    let policy_repo = InMemoryPolicyRepository::new(); // no policies

    let access_control = AccessControlService::new(user_repo, role_repo, policy_repo);
    let policy_service = PolicyEvaluationService::new(access_control);

    let ctx = request_context(user_id);
    let decision = policy_service
        .evaluate(&ctx, Action::Read, ResourceType::ConnectionTarget, None)
        .await
        .expect("evaluate");

    assert_eq!(decision, PolicyDecision::Deny);
}

#[tokio::test]
async fn policy_evaluation_service_allows_when_policy_matches() {
    let user_id = Uuid::now_v7();
    let role_id = Uuid::now_v7();
    let policy = allow_read_host_policy(role_id);

    let user_repo = InMemoryUserRepository::new();
    let role_repo = InMemoryRoleRepository::new().with_roles(
        user_id,
        vec![Role {
            id: role_id,
            name: "test-role".to_string(),
            description: None,
        }],
    );
    let policy_repo = InMemoryPolicyRepository::new().with_policies_for_subjects(vec![policy]);

    let access_control = AccessControlService::new(user_repo, role_repo, policy_repo);
    let policy_service = PolicyEvaluationService::new(access_control);

    let ctx = request_context(user_id);
    let decision = policy_service
        .evaluate(&ctx, Action::Read, ResourceType::ConnectionTarget, None)
        .await
        .expect("evaluate");

    assert_eq!(decision, PolicyDecision::Allow);
}

#[tokio::test]
async fn policy_evaluation_service_denies_unauthenticated() {
    let user_repo = InMemoryUserRepository::new();
    let role_repo = InMemoryRoleRepository::new();
    let policy_repo = InMemoryPolicyRepository::new()
        .with_policies_for_subjects(vec![allow_read_host_policy(Uuid::now_v7())]);

    let access_control = AccessControlService::new(user_repo, role_repo, policy_repo);
    let policy_service = PolicyEvaluationService::new(access_control);

    let ctx = RequestContext::new(
        None, // no user_id
        "none".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth::default(),
    );

    let decision = policy_service
        .evaluate(&ctx, Action::Read, ResourceType::ConnectionTarget, None)
        .await
        .expect("evaluate");

    assert_eq!(decision, PolicyDecision::Deny);
}

// ── Attribute-aware connect authorization ────────────────────────────────────
//
// End-to-end at the authorization-decision level (mocked repos, no DB): a role
// whose ONLY connect grant is constrained to device-role RU3 AND protocol VNC.
// Exercises the real subject→role→policy resolution, build_policy_context
// attribute merge, and and/or/equality constraint evaluation that
// `create_connection_session` now drives via `ensure_access_with_attributes`.

fn connect_only_ru3_vnc_policy(role_id: Uuid) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "connect-ru3-vnc-only".to_string(),
        description: None,
        effect: PolicyEffect::Allow,
        actions: vec![Action::Connect],
        resource_types: vec![ResourceType::ConnectionTarget],
        constraints: Some(json!({
            "and": [
                {"attributes": {"role_id": "role-ru3"}},
                {"attributes": {"protocol": "vnc"}}
            ]
        })),
        subjects: vec![format!("role:{role_id}")],
        resources: Vec::new(),
        enabled: true,
    }
}

fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
        .collect()
}

#[tokio::test]
async fn connect_is_gated_by_device_role_and_protocol_attributes() {
    let user_id = Uuid::now_v7();
    let role = Role {
        id: Uuid::now_v7(),
        name: "kiosk-operator".to_string(),
        description: None,
    };

    let user_repo = InMemoryUserRepository::new();
    let role_repo = InMemoryRoleRepository::new().with_roles(user_id, vec![role.clone()]);
    let policy_repo = InMemoryPolicyRepository::new()
        .with_policies_for_subjects(vec![connect_only_ru3_vnc_policy(role.id)]);
    let policy_service =
        PolicyEvaluationService::new(AccessControlService::new(user_repo, role_repo, policy_repo));

    let ctx = request_context(user_id);
    let target = Some("connection_target:kiosk-1");
    let connect = |attributes: HashMap<String, Value>| {
        policy_service.ensure_access_with_attributes(
            &ctx,
            Action::Connect,
            ResourceType::ConnectionTarget,
            target,
            Some(attributes),
        )
    };

    // VNC to an RU3 device — both constraint legs satisfied → allowed.
    connect(attrs(&[("role_id", "role-ru3"), ("protocol", "vnc")]))
        .await
        .expect("VNC connect to an RU3 device must be allowed");

    // SSH to the same RU3 device — protocol leg fails → denied.
    connect(attrs(&[("role_id", "role-ru3"), ("protocol", "ssh")]))
        .await
        .expect_err("SSH connect must be denied (protocol not VNC)");

    // VNC to a non-RU3 device — role leg fails → denied.
    connect(attrs(&[("role_id", "role-ru1"), ("protocol", "vnc")]))
        .await
        .expect_err("VNC connect to a non-RU3 device must be denied");

    // No attributes at all (an ensure_access context without attributes) — the
    // constraint can't be satisfied → denied. This is the fail-closed boundary.
    policy_service
        .ensure_access(
            &ctx,
            Action::Connect,
            ResourceType::ConnectionTarget,
            target,
        )
        .await
        .expect_err("without object attributes the constrained grant yields nothing");
}

// ── Deferred pull commit authorisation (PWA-81) ──────────────────────────────
//
// A pull started by a REST request commits after that request has returned. The
// provider lease is liveness only; the writer must re-ask the canonical policy
// evaluator and token owner for the caller's *current* ceilings, including READ
// of one persisted inventory object with the attributes stored on that row.

/// A lease that always holds. Reference READ and object writes are still
/// refused, because a lease never speaks for a request principal.
struct HeldLease;

impl InventoryCommitAuthorisation for HeldLease {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn ensure_reference_access(
        &self,
        _id: Uuid,
        _attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async {
            Err(AppError::Forbidden(
                "Provider lease is not a request authorisation".into(),
            ))
        })
    }

    fn ensure_mutation_access(
        &self,
        _action: Action,
        _id: Uuid,
        _attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async {
            Err(AppError::Forbidden(
                "Provider lease is not a request authorisation".into(),
            ))
        })
    }
}

fn active_user(user_id: Uuid) -> User {
    User {
        id: user_id,
        username: "pull-operator".to_string(),
        email: "pull-operator@example.test".to_string(),
        status: "active".to_string(),
        auth_source: "local".to_string(),
        external_id: None,
    }
}

fn operator_role() -> Role {
    Role {
        id: Uuid::now_v7(),
        name: "inventory-operator".to_string(),
        description: None,
    }
}

fn inventory_execute_policy(role_id: Uuid) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "execute-inventory".to_string(),
        description: None,
        effect: PolicyEffect::Allow,
        actions: vec![Action::Execute],
        resource_types: vec![ResourceType::Inventory],
        constraints: None,
        subjects: vec![format!("role:{role_id}")],
        resources: Vec::new(),
        enabled: true,
    }
}

fn inventory_read_by_site_policy(role_id: Uuid, target: Uuid) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "read-inventory-object-by-site".to_string(),
        description: None,
        effect: PolicyEffect::Allow,
        actions: vec![Action::Read],
        resource_types: vec![ResourceType::Inventory],
        constraints: Some(json!({"and": [{"attributes": {"site_id": "site-a"}}]})),
        subjects: vec![format!("role:{role_id}")],
        resources: vec![format!("inventory:{target}")],
        enabled: true,
    }
}

fn api_token_config() -> ApiTokenConfig {
    serde_json::from_value(json!({})).expect("api token config defaults")
}

fn personal_token(
    user_id: Uuid,
    scopes: Vec<UserApiTokenScope>,
    status: UserApiTokenStatus,
    expires_at: Option<DateTime<Utc>>,
) -> UserApiToken {
    let now = Utc::now();
    UserApiToken {
        id: Uuid::now_v7(),
        user_id,
        name: "deferred-pull".to_string(),
        token_prefix: "pat_prefix".to_string(),
        token_hash: "not-compared-by-an-id-only-boundary".to_string(),
        status,
        expires_at,
        last_used_at: None,
        created_at: now,
        updated_at: now,
        revoked_at: None,
        revoked_by_user_id: None,
        scopes,
    }
}

fn session_request(user_id: Uuid) -> RequestContext {
    RequestContext::new(
        Some(user_id),
        "session".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth::default(),
    )
}

/// The request-time PAT snapshot is deliberately broad; the stored token state is
/// what the deferred boundary re-reads.
fn api_token_request(user_id: Uuid, token_id: Uuid) -> RequestContext {
    let mut request = RequestContext::new(
        Some(user_id),
        "api_token".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth {
            api_token_scopes: vec![
                ApiTokenScopeContext {
                    action: "execute".to_string(),
                    resource_type: "inventory".to_string(),
                    resource_pattern: "*".to_string(),
                },
                ApiTokenScopeContext {
                    action: "read".to_string(),
                    resource_type: "inventory".to_string(),
                    resource_pattern: "*".to_string(),
                },
            ],
            ..RequestAuth::default()
        },
    );
    request.api_token_id = Some(token_id.to_string());
    request
}

fn deferred_authorisation(
    user: User,
    role: Role,
    policies: Vec<Policy>,
    token: Option<UserApiToken>,
    request: RequestContext,
) -> RequestCommitAuthorisation {
    let policy = Arc::new(PolicyEvaluationService::new(AccessControlService::new(
        InMemoryUserRepository::new().with_user(user.clone()),
        InMemoryRoleRepository::new().with_roles(user.id, vec![role]),
        InMemoryPolicyRepository::new().with_policies_for_subjects(policies),
    )));
    let users: Arc<dyn UserRepository> = Arc::new(InMemoryUserRepository::new().with_user(user));
    let tokens = Arc::new(UserApiTokenService::new(
        Arc::new(InMemoryUserApiTokenRepository::new(token)),
        users.clone(),
        api_token_config(),
        None,
    ));
    let authority = Arc::new(PolicyRequestCommitAuthority::new(policy, tokens, users));
    RequestCommitAuthorisation::new(Arc::new(HeldLease), authority, request)
}

fn inventory_scope(action: Action, pattern: String) -> UserApiTokenScope {
    UserApiTokenScope {
        action,
        resource_type: ResourceType::Inventory,
        resource_pattern: pattern,
    }
}

#[tokio::test]
async fn pull_writer_reads_one_object_only_with_its_persisted_attributes() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let target = Uuid::now_v7();
    let other = Uuid::now_v7();
    let composed = deferred_authorisation(
        user.clone(),
        role.clone(),
        vec![
            inventory_execute_policy(role.id),
            inventory_read_by_site_policy(role.id, target),
        ],
        None,
        session_request(user_id),
    );

    composed
        .ensure_reference_access(target, json!({"site_id": "site-a"}))
        .await
        .expect("READ of the persisted object whose attributes satisfy the grant");

    composed
        .ensure_reference_access(target, json!({"site_id": "site-b"}))
        .await
        .expect_err("a persisted attribute that fails the constraint must deny");

    composed
        .ensure_reference_access(other, json!({"site_id": "site-a"}))
        .await
        .expect_err("the grant names one object id and must not widen");

    composed
        .ensure_owned()
        .await
        .expect("the same request may still execute inventory writes");

    HeldLease
        .ensure_reference_access(target, json!({"site_id": "site-a"}))
        .await
        .expect_err("a provider lease alone is never a reference authorization");
}

#[tokio::test]
async fn pull_writer_rechecks_the_current_inventory_execute_grant() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let target = Uuid::now_v7();

    let granted = deferred_authorisation(
        user.clone(),
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        None,
        session_request(user_id),
    );
    granted
        .ensure_owned()
        .await
        .expect("a current inventory EXECUTE grant may commit");

    let lost = deferred_authorisation(
        user,
        role.clone(),
        vec![inventory_read_by_site_policy(role.id, target)],
        None,
        session_request(user_id),
    );
    lost.ensure_owned()
        .await
        .expect_err("a request whose only remaining grant is READ must not commit");
}

#[tokio::test]
async fn revoked_or_expired_personal_token_cannot_commit() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let scopes = vec![
        inventory_scope(Action::Execute, "*".to_string()),
        inventory_scope(Action::Read, "*".to_string()),
    ];

    let active = personal_token(user_id, scopes.clone(), UserApiTokenStatus::Active, None);
    let active_id = active.id;
    let composed = deferred_authorisation(
        user.clone(),
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        Some(active),
        api_token_request(user_id, active_id),
    );
    composed
        .ensure_owned()
        .await
        .expect("an active token that still carries inventory EXECUTE may commit");

    let revoked = personal_token(user_id, scopes.clone(), UserApiTokenStatus::Revoked, None);
    let revoked_id = revoked.id;
    let composed = deferred_authorisation(
        user.clone(),
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        Some(revoked),
        api_token_request(user_id, revoked_id),
    );
    composed
        .ensure_owned()
        .await
        .expect_err("a token revoked after the request must not commit");

    let expired = personal_token(
        user_id,
        scopes,
        UserApiTokenStatus::Active,
        Some(Utc::now() - Duration::seconds(1)),
    );
    let expired_id = expired.id;
    let composed = deferred_authorisation(
        user,
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        Some(expired),
        api_token_request(user_id, expired_id),
    );
    composed
        .ensure_owned()
        .await
        .expect_err("a token that expired after the request must not commit");
}

#[tokio::test]
async fn narrowed_personal_token_loses_its_inventory_ceilings() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let target = Uuid::now_v7();
    let other = Uuid::now_v7();
    let policies = vec![
        inventory_execute_policy(role.id),
        inventory_read_by_site_policy(role.id, target),
    ];

    // Current token state: READ of a different object only, no EXECUTE at all.
    let narrowed = personal_token(
        user_id,
        vec![inventory_scope(Action::Read, format!("inventory:{other}"))],
        UserApiTokenStatus::Active,
        None,
    );
    let narrowed_id = narrowed.id;
    let composed = deferred_authorisation(
        user.clone(),
        role.clone(),
        policies.clone(),
        Some(narrowed),
        api_token_request(user_id, narrowed_id),
    );
    composed
        .ensure_reference_access(target, json!({"site_id": "site-a"}))
        .await
        .expect_err("a token scoped to another object must not read this one");
    composed
        .ensure_owned()
        .await
        .expect_err("a token without the EXECUTE scope must not commit");

    // The same request once the token still names this object. The stored scope,
    // not the broad request-time snapshot, decides.
    let scoped = personal_token(
        user_id,
        vec![
            inventory_scope(Action::Read, format!("inventory:{target}")),
            inventory_scope(Action::Execute, "*".to_string()),
        ],
        UserApiTokenStatus::Active,
        None,
    );
    let scoped_id = scoped.id;
    let composed = deferred_authorisation(
        user,
        role,
        policies,
        Some(scoped),
        api_token_request(user_id, scoped_id),
    );
    composed
        .ensure_reference_access(target, json!({"site_id": "site-a"}))
        .await
        .expect("a token currently scoped to this object may read it");
    composed
        .ensure_owned()
        .await
        .expect("a token currently carrying EXECUTE may commit");
}

#[tokio::test]
async fn unrepresentable_principal_or_token_state_fails_closed() {
    let user_id = Uuid::now_v7();
    let role = operator_role();

    let mut job_token = session_request(user_id);
    job_token.auth_method = "playbook_job_token".to_string();
    job_token.api_token_id = Some(Uuid::now_v7().to_string());
    deferred_authorisation(
        active_user(user_id),
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        None,
        job_token,
    )
    .ensure_owned()
    .await
    .expect_err("a job token is not a representable deferred pull principal");

    let tokenless_pat = RequestContext::new(
        Some(user_id),
        "api_token".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth::default(),
    );
    deferred_authorisation(
        active_user(user_id),
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        None,
        tokenless_pat,
    )
    .ensure_owned()
    .await
    .expect_err("an api_token request without an opaque id must not inherit authority");

    let anonymous = RequestContext::new(
        None,
        "session".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth::default(),
    );
    deferred_authorisation(
        active_user(user_id),
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        None,
        anonymous,
    )
    .ensure_owned()
    .await
    .expect_err("a request without a principal must not commit");

    let mut disabled = active_user(user_id);
    disabled.status = "disabled".to_string();
    deferred_authorisation(
        disabled,
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        None,
        session_request(user_id),
    )
    .ensure_owned()
    .await
    .expect_err("a disabled account must not keep the authority of an earlier request");
}

// ── Attribute-aware template writes (PWA-81) ─────────────────────────────────
//
// A component-template write now asks for the ordinary canonical per-object
// CREATE/UPDATE decision in addition to the operation-level authority that
// dispatched the job. A global Inventory grant plus a conditional object DENY has
// to be able to refuse the write, and an operation-level grant on its own must
// not be enough.

const CONSOLE_PORT_TEMPLATE: &str = "dcim.consoleporttemplate";

/// The native inventory write contract for a component template: the object's
/// persisted `object_type` names a component template.
fn template_write_policy(role_id: Uuid, target: Uuid) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "write-component-template".to_string(),
        description: None,
        effect: PolicyEffect::Allow,
        actions: vec![Action::Create, Action::Update],
        resource_types: vec![ResourceType::Inventory],
        constraints: Some(json!({
            "and": [{"attributes": {"object_type": CONSOLE_PORT_TEMPLATE}}]
        })),
        subjects: vec![format!("role:{role_id}")],
        resources: vec![format!("inventory:{target}")],
        enabled: true,
    }
}

/// The conditional DENY from the reported failure: UPDATE of console-port
/// templates belonging to one device type. It names no resource id, so it applies
/// wherever an UPDATE decision is asked for.
fn console_port_update_deny_policy(role_id: Uuid, device_type_id: Uuid) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "deny-console-port-update".to_string(),
        description: None,
        effect: PolicyEffect::Deny,
        actions: vec![Action::Update],
        resource_types: vec![ResourceType::Inventory],
        constraints: Some(json!({
            "and": [
                {"attributes": {"object_type": CONSOLE_PORT_TEMPLATE}},
                {"attributes": {"device_type_id": device_type_id.to_string()}}
            ]
        })),
        subjects: vec![format!("role:{role_id}")],
        resources: Vec::new(),
        enabled: true,
    }
}

fn template_attributes(device_type_id: Uuid) -> Value {
    json!({
        "object_type": CONSOLE_PORT_TEMPLATE,
        "device_type_id": device_type_id.to_string(),
    })
}

#[tokio::test]
async fn template_update_honours_a_conditional_object_deny() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let target = Uuid::now_v7();
    let other = Uuid::now_v7();
    let denied_device_type = Uuid::now_v7();
    let allowed_device_type = Uuid::now_v7();
    let composed = deferred_authorisation(
        user.clone(),
        role.clone(),
        vec![
            template_write_policy(role.id, target),
            console_port_update_deny_policy(role.id, denied_device_type),
        ],
        None,
        session_request(user_id),
    );

    composed
        .ensure_mutation_access(
            Action::Update,
            target,
            template_attributes(denied_device_type),
        )
        .await
        .expect_err("the conditional UPDATE deny must refuse this template");

    composed
        .ensure_mutation_access(
            Action::Update,
            target,
            template_attributes(allowed_device_type),
        )
        .await
        .expect("an UPDATE the deny does not name is still allowed");

    composed
        .ensure_mutation_access(
            Action::Create,
            target,
            template_attributes(denied_device_type),
        )
        .await
        .expect("the deny is UPDATE-only; CREATE is its own object decision");

    composed
        .ensure_mutation_access(
            Action::Create,
            other,
            template_attributes(allowed_device_type),
        )
        .await
        .expect_err("the write grant names one object id and must not widen");
}

#[tokio::test]
async fn template_write_needs_the_object_action_not_only_execute() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let target = Uuid::now_v7();
    let composed = deferred_authorisation(
        user.clone(),
        role.clone(),
        vec![inventory_execute_policy(role.id)],
        None,
        session_request(user_id),
    );

    composed
        .ensure_owned()
        .await
        .expect("the operation-level inventory EXECUTE grant still holds");

    composed
        .ensure_mutation_access(Action::Update, target, template_attributes(Uuid::now_v7()))
        .await
        .expect_err("an operation-level grant alone must not authorise the object write");

    HeldLease
        .ensure_mutation_access(Action::Update, target, template_attributes(Uuid::now_v7()))
        .await
        .expect_err("a provider lease alone is never a mutation authorization");
}

#[tokio::test]
async fn template_write_honours_the_current_token_action_scope() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let target = Uuid::now_v7();
    let device_type = Uuid::now_v7();
    let policies = vec![template_write_policy(role.id, target)];

    let read_only = personal_token(
        user_id,
        vec![inventory_scope(Action::Read, "*".to_string())],
        UserApiTokenStatus::Active,
        None,
    );
    let read_only_id = read_only.id;
    let composed = deferred_authorisation(
        user.clone(),
        role.clone(),
        policies.clone(),
        Some(read_only),
        api_token_request(user_id, read_only_id),
    );
    composed
        .ensure_mutation_access(Action::Update, target, template_attributes(device_type))
        .await
        .expect_err("a token without the UPDATE scope must not write, whatever the policy allows");

    let writable = personal_token(
        user_id,
        vec![
            inventory_scope(Action::Read, "*".to_string()),
            inventory_scope(Action::Update, "*".to_string()),
        ],
        UserApiTokenStatus::Active,
        None,
    );
    let writable_id = writable.id;
    let composed = deferred_authorisation(
        user,
        role,
        policies,
        Some(writable),
        api_token_request(user_id, writable_id),
    );
    composed
        .ensure_mutation_access(Action::Update, target, template_attributes(device_type))
        .await
        .expect("a token that still carries the UPDATE scope may write");
}

#[tokio::test]
async fn mutation_ceiling_refuses_actions_other_than_create_and_update() {
    let user_id = Uuid::now_v7();
    let user = active_user(user_id);
    let role = operator_role();
    let target = Uuid::now_v7();
    let device_type = Uuid::now_v7();
    let unconstrained_object_policy = |action: Action| Policy {
        id: Uuid::now_v7(),
        name: format!("object-{action:?}"),
        description: None,
        effect: PolicyEffect::Allow,
        actions: vec![action],
        resource_types: vec![ResourceType::Inventory],
        constraints: None,
        subjects: vec![format!("role:{}", role.id)],
        resources: vec![format!("inventory:{target}")],
        enabled: true,
    };
    let policies = vec![
        unconstrained_object_policy(Action::Read),
        unconstrained_object_policy(Action::Delete),
    ];
    let composed = deferred_authorisation(user, role, policies, None, session_request(user_id));

    composed
        .ensure_reference_access(target, template_attributes(device_type))
        .await
        .expect("READ stays available through its own method");

    composed
        .ensure_mutation_access(Action::Read, target, template_attributes(device_type))
        .await
        .expect_err("READ must not slip through the mutation port");
    composed
        .ensure_mutation_access(Action::Delete, target, template_attributes(device_type))
        .await
        .expect_err("DELETE must not slip through the mutation port");
}
