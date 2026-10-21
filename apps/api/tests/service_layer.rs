//! Service-layer unit tests with in-memory fake repositories.
//!
//! Demonstrates the pattern of testing services (e.g. PolicyEvaluationService)
//! using fake/in-memory implementations of repository traits, without PostgreSQL or Redis.
//!
//! Run with: `cargo test --test service_layer`

use std::collections::HashMap;

use serde_json::{Value, json};

use async_trait::async_trait;
use uuid::Uuid;

use piping_backend::contexts::access_control::application::{
    AccessControlService, PolicyEvaluationService,
};
use piping_backend::contexts::access_control::domain::{
    Action, Group, Policy, PolicyDecision, PolicyEffect, ResourceType, Role,
};
use piping_backend::contexts::access_control::infrastructure::persistence::{
    NamedListFilter, PolicyFilter, PolicyRepository, RoleRepository, UserListFilter, UserRepository,
};
use piping_backend::contexts::shared::pagination::PageResult;
use piping_backend::contexts::shared::request_context::{RequestAuth, RequestContext};
use piping_backend::errors::AppError;

// --- In-memory fake repositories ---

/// In-memory UserRepository for service-layer tests.
struct InMemoryUserRepository {
    /// Groups keyed by their `external_id`, returned when that external id is in
    /// the requested set — mirrors the production `external_id = ANY($1)` match.
    groups_by_external_id: HashMap<String, Vec<Group>>,
    persisted_groups: Vec<Group>,
}

impl InMemoryUserRepository {
    fn new() -> Self {
        Self {
            groups_by_external_id: HashMap::new(),
            persisted_groups: Vec::new(),
        }
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
    async fn get_by_id(
        &self,
        _user_id: Uuid,
    ) -> Result<Option<piping_backend::contexts::access_control::domain::User>, AppError> {
        Ok(None)
    }

    async fn get_by_username_or_email(
        &self,
        _identifier: &str,
    ) -> Result<Option<piping_backend::contexts::access_control::domain::User>, AppError> {
        Ok(None)
    }

    async fn get_by_external_id(
        &self,
        _external_id: &str,
    ) -> Result<Option<piping_backend::contexts::access_control::domain::User>, AppError> {
        Ok(None)
    }

    async fn list_users(
        &self,
    ) -> Result<Vec<piping_backend::contexts::access_control::domain::User>, AppError> {
        Ok(Vec::new())
    }

    async fn list_users_page(
        &self,
        _filter: UserListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<piping_backend::contexts::access_control::domain::User>, AppError> {
        Ok(PageResult {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
        })
    }

    async fn create_user(
        &self,
        user: piping_backend::contexts::access_control::domain::User,
    ) -> Result<piping_backend::contexts::access_control::domain::User, AppError> {
        Ok(user)
    }

    async fn update_user(
        &self,
        user: piping_backend::contexts::access_control::domain::User,
    ) -> Result<piping_backend::contexts::access_control::domain::User, AppError> {
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
