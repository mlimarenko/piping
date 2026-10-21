use std::collections::HashMap;
use std::time::Instant;

use chrono::Utc;
use redis::aio::ConnectionManager;
use serde_json::{Map, Value};
use uuid::Uuid;

use super::policy_cache::{build_policy_cache_key, load_policy_cache_generations};
use crate::config::constants::DEFAULT_CACHE_TTL_SECONDS;
use crate::contexts::access_control::domain::{
    Action, Policy, PolicyDecision, PolicyDecisionMetadata, Principal, ResourceType,
    evaluate_policies, evaluate_policies_with_metadata,
};
use crate::contexts::access_control::infrastructure::persistence::{
    PolicyRepository, RoleRepository, UserRepository,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::observability::metrics;
use crate::storage::cache::redis::cache;

pub struct AccessControlService<U, R, P> {
    user_repository: U,
    role_repository: R,
    policy_repository: P,
    policy_cache: Option<ConnectionManager>,
    policy_cache_ttl_seconds: u64,
}

const POLICY_CACHE_METRIC_NAME: &str = "access_control_policies";

/// A single allowed `(action, resource_type)` pair in a principal's effective
/// authorization. Type-level (resource-id-agnostic): a resource-scoped allow
/// policy still surfaces here, because UI gating keys off "can do X to some
/// resource of this type", while per-object authority stays an API-side check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub action: Action,
    pub resource_type: ResourceType,
}

/// A principal's effective authorization snapshot: the role names it resolves to
/// and the type-level capability grid the UI uses to gate navigation and controls.
#[derive(Debug, Clone, Default)]
pub struct EffectiveAuthorization {
    pub roles: Vec<String>,
    pub capabilities: Vec<Capability>,
}

impl<U, R, P> AccessControlService<U, R, P>
where
    U: UserRepository,
    R: RoleRepository,
    P: PolicyRepository,
{
    pub fn new(user_repository: U, role_repository: R, policy_repository: P) -> Self {
        Self {
            user_repository,
            role_repository,
            policy_repository,
            policy_cache: None,
            policy_cache_ttl_seconds: DEFAULT_CACHE_TTL_SECONDS,
        }
    }

    pub fn new_with_cache(
        user_repository: U,
        role_repository: R,
        policy_repository: P,
        // A plain `ConnectionManager`, not `Arc<Mutex<_>>`. The manager is
        // already cheaply cloneable and internally multiplexed, so a mutex only
        // serializes concurrent authorization checks into a queue. Taking it
        // unwrapped here also removes the need to unlock it during construction,
        // so the constructor never has to panic on a lock.
        policy_cache: ConnectionManager,
    ) -> Self {
        Self {
            user_repository,
            role_repository,
            policy_repository,
            policy_cache: Some(policy_cache),
            policy_cache_ttl_seconds: DEFAULT_CACHE_TTL_SECONDS,
        }
    }

    pub async fn check_access(
        &self,
        request_context: &RequestContext,
        action: Action,
        resource_type: ResourceType,
        resource_id: Option<&str>,
    ) -> Result<PolicyDecision, AppError> {
        let principal = match Self::user_principal(request_context)? {
            Some(principal) => principal,
            None => return Ok(PolicyDecision::Deny),
        };
        let policies = self.resolve_policies_for_principal(&principal).await?;
        Ok(evaluate_policies(
            &policies,
            action,
            resource_type,
            resource_id,
        ))
    }

    pub fn api_token_scope_allows(
        request_context: &RequestContext,
        action: &Action,
        resource_type: &ResourceType,
        resource_id: Option<&str>,
    ) -> bool {
        if !matches!(
            request_context.auth_method.as_str(),
            "api_token" | "playbook_job_token"
        ) {
            return true;
        }
        request_context.allows_api_token_scope_for_resource(
            action_key(action),
            resource_type.as_str(),
            resource_id,
        )
    }

    pub async fn check_access_with_attributes(
        &self,
        request_context: &RequestContext,
        action: Action,
        resource_type: ResourceType,
        resource_id: Option<&str>,
        attributes: Option<HashMap<String, Value>>,
    ) -> Result<PolicyDecision, AppError> {
        let principal = match Self::user_principal(request_context)? {
            Some(principal) => principal,
            None => return Ok(PolicyDecision::Deny),
        };
        let policies = self.resolve_policies_for_principal(&principal).await?;
        let context = Self::build_policy_context(
            request_context,
            &resource_type,
            resource_id,
            attributes.as_ref(),
        )?;
        let decision = evaluate_policies_with_metadata(
            &policies,
            action,
            resource_type,
            resource_id,
            &context,
        )
        .decision;
        Ok(decision)
    }

    pub async fn check_access_with_trace(
        &self,
        request_context: &RequestContext,
        action: Action,
        resource_type: ResourceType,
        resource_id: Option<&str>,
        attributes: Option<HashMap<String, Value>>,
    ) -> Result<PolicyDecisionMetadata, AppError> {
        let principal = match Self::user_principal(request_context)? {
            Some(principal) => principal,
            None => {
                return Ok(PolicyDecisionMetadata {
                    decision: PolicyDecision::Deny,
                    matched_allow: Vec::new(),
                    matched_deny: Vec::new(),
                    reasons: vec!["missing user context; default deny".to_string()],
                });
            }
        };
        let policies = self.resolve_policies_for_principal(&principal).await?;
        let context = Self::build_policy_context(
            request_context,
            &resource_type,
            resource_id,
            attributes.as_ref(),
        )?;
        let mut metadata = evaluate_policies_with_metadata(
            &policies,
            action,
            resource_type,
            resource_id,
            &context,
        );
        if metadata.reasons.is_empty() && metadata.decision == PolicyDecision::Deny {
            metadata
                .reasons
                .push("no matching policy; default deny".to_string());
        }
        Ok(metadata)
    }

    /// Principal-aware evaluation core.
    ///
    /// This is the single entry both the user-facing `check_access*` methods and
    /// the playbook service-principal authorizer delegate to. It resolves the
    /// principal's canonical subjects, lists the matching policies, builds the
    /// constraint context, and evaluates with trace metadata.
    ///
    /// Caching is intentionally principal-aware (codex HIGH #6): only
    /// `Principal::User` policy resolution is cached; `Principal::ServicePrincipal`
    /// resolution is always uncached because the SP revoke path lives in the
    /// playbooks context with no access-control cache-invalidation hook.
    pub async fn check_access_for_principal(
        &self,
        principal: &Principal,
        action: Action,
        resource_type: ResourceType,
        resource_id: Option<&str>,
        attributes: Option<HashMap<String, Value>>,
    ) -> Result<PolicyDecisionMetadata, AppError> {
        let policies = self.resolve_policies_for_principal(principal).await?;
        let context =
            Self::build_principal_context(principal, &resource_type, resource_id, attributes)?;
        let mut metadata = evaluate_policies_with_metadata(
            &policies,
            action,
            resource_type,
            resource_id,
            &context,
        );
        if metadata.reasons.is_empty() && metadata.decision == PolicyDecision::Deny {
            metadata
                .reasons
                .push("no matching policy; default deny".to_string());
        }
        Ok(metadata)
    }

    pub async fn check_secret_visibility(
        &self,
        request_context: &RequestContext,
    ) -> Result<PolicyDecision, AppError> {
        self.check_access(
            request_context,
            Action::Secret,
            ResourceType::SecretVariable,
            None,
        )
        .await
    }

    /// Resolve a principal's effective authorization: the role names it holds
    /// and the type-level capability grid (every `(action, resource_type)` that
    /// evaluates to Allow with no specific resource id).
    ///
    /// This is the single source the `/auth/me` contract uses to let the SPA gate
    /// navigation and controls by capability instead of rendering everything and
    /// relying on a 403 on fetch. Policies are resolved once (cached for users),
    /// then the `Action::ALL` x `ResourceType::ALL` grid is evaluated in-memory.
    pub async fn effective_authorization(
        &self,
        principal: &Principal,
    ) -> Result<EffectiveAuthorization, AppError> {
        let policies = self.resolve_policies_for_principal(principal).await?;

        let roles = match principal {
            Principal::User { id } => self
                .role_repository
                .list_roles_for_user(*id)
                .await?
                .into_iter()
                .map(|role| role.name)
                .collect(),
            Principal::ServicePrincipal(_) => Vec::new(),
        };

        let mut capabilities = Vec::new();
        for action in Action::ALL {
            for resource_type in ResourceType::ALL {
                if !crate::contexts::access_control::domain::is_valid_permission_pair(
                    &action,
                    &resource_type,
                ) {
                    continue;
                }
                if evaluate_policies(&policies, action.clone(), resource_type.clone(), None)
                    == PolicyDecision::Allow
                {
                    capabilities.push(Capability {
                        action: action.clone(),
                        resource_type,
                    });
                }
            }
        }

        Ok(EffectiveAuthorization {
            roles,
            capabilities,
        })
    }

    /// Emit the canonical `kind:uuid` subject grammar for a principal.
    ///
    /// User closure:
    /// 1. `user:{uuid}`.
    /// 2. Direct + persisted group role assignments → `role:{uuid}`. Personal
    ///    API tokens and browser sessions therefore resolve the same user RBAC
    ///    envelope; token scopes can only narrow it.
    /// 3. Persisted group memberships → `group:{group_uuid}`.
    ///
    /// Service-principal closure: exactly `service_principal:{uuid}` — no bare
    /// `"service_principal"` catch-all and no slug form (codex CRIT #1).
    pub async fn resolve_subjects(&self, principal: &Principal) -> Result<Vec<String>, AppError> {
        let mut subjects = Vec::new();

        match principal {
            Principal::User { id } => {
                subjects.push(format!("user:{id}"));

                // role:{uuid} — direct user assignments plus persisted group
                // memberships.
                let roles = self.role_repository.list_roles_for_user(*id).await?;
                for role in roles {
                    subjects.push(format!("role:{}", role.id));
                }

                // group:{uuid} — persisted memberships, independent of auth method.
                let persisted_groups = self.user_repository.list_groups_for_user(*id).await?;
                for group in &persisted_groups {
                    subjects.push(format!("group:{}", group.id));
                }
            }
            Principal::ServicePrincipal(sp_id) => {
                subjects.push(format!("service_principal:{}", sp_id.get()));
            }
        }

        subjects.sort();
        subjects.dedup();
        Ok(subjects)
    }

    /// Resolve the policies that apply to a principal.
    ///
    /// `Principal::User` is cached (namespace `v2`); `Principal::ServicePrincipal`
    /// is always uncached (codex HIGH #6).
    pub async fn resolve_policies_for_principal(
        &self,
        principal: &Principal,
    ) -> Result<Vec<Policy>, AppError> {
        let subjects = self.resolve_subjects(principal).await?;

        let Principal::User { id, .. } = principal else {
            // Service principals: never cached.
            return self.policy_repository.list_for_subjects(&subjects).await;
        };
        let user_id = *id;

        let (generation, user_generation) = self.load_cache_generations(user_id).await?;
        let cache_key = build_policy_cache_key(user_id, &subjects, generation, user_generation);

        if let Some(cache_policies) = self.get_cached_policies(&cache_key).await? {
            return Ok(cache_policies);
        }

        let policies = self.policy_repository.list_for_subjects(&subjects).await?;
        let _ = self.set_cached_policies(&cache_key, &policies).await;
        Ok(policies)
    }

    /// Resolve the effective policy set for the request's user principal, or an
    /// empty set when there is no user id (callers MUST treat empty as default
    /// deny). Exposed so composite gates — e.g. the playbook C3 secret-use gate —
    /// can resolve policies ONCE and drive a pure per-resource decision core off
    /// them, instead of issuing one `check_access*` round-trip per candidate.
    pub async fn resolve_policies_for_request(
        &self,
        request_context: &RequestContext,
    ) -> Result<Vec<Policy>, AppError> {
        match Self::user_principal(request_context)? {
            Some(principal) => self.resolve_policies_for_principal(&principal).await,
            None => Ok(Vec::new()),
        }
    }

    /// Build the constraint-evaluation context for a request, so pure decision
    /// cores evaluate policy constraints with the SAME `ip`/`timestamp`/subject
    /// context the internal `check_access*` methods use.
    pub fn constraint_context_for_request(
        &self,
        request_context: &RequestContext,
        resource_type: ResourceType,
        resource_id: Option<&str>,
    ) -> Result<Value, AppError> {
        Self::build_policy_context(request_context, &resource_type, resource_id, None)
    }

    /// Build the constraint-evaluation context for a principal, so pure decision
    /// cores evaluate policy constraints with the SAME context the internal
    /// `check_access_for_principal` uses.
    ///
    /// Mirrors `check_access_for_principal`: the service-principal path has no
    /// `ip`/correlation/OIDC context, so an IP- or request-scoped constraint
    /// never matches for an SP — fail-closed for an Allow.
    pub fn constraint_context_for_principal(
        &self,
        principal: &Principal,
        resource_type: ResourceType,
        resource_id: Option<&str>,
    ) -> Result<Value, AppError> {
        Self::build_principal_context(principal, &resource_type, resource_id, None)
    }

    fn parse_user_id(request_context: &RequestContext) -> Result<Option<Uuid>, AppError> {
        Ok(request_context.user_id)
    }

    /// Build a `Principal::User` from the request context, or `None` when there
    /// is no user id (which the callers map to a default deny).
    fn user_principal(request_context: &RequestContext) -> Result<Option<Principal>, AppError> {
        Ok(Self::parse_user_id(request_context)?.map(Principal::user))
    }

    /// Build the constraint-evaluation context for a principal.
    ///
    /// Constraints only read `ip`, `timestamp`, and `attributes`
    /// (see `domain/conditions.rs`); `subject`/`auth_method`/`resource` blocks are
    /// trace metadata. The SP path has no IP/correlation/OIDC context, so only the
    /// keys that change a decision are populated identically for both principals.
    fn build_principal_context(
        principal: &Principal,
        resource_type: &ResourceType,
        resource_id: Option<&str>,
        attributes: Option<HashMap<String, Value>>,
    ) -> Result<Value, AppError> {
        let mut context = Map::new();
        let mut subject = Map::new();
        let mut resource = Map::new();
        let mut merged_attributes = Map::new();

        let auth_method = match principal {
            Principal::User { id } => {
                let user_id = id.to_string();
                subject.insert("kind".to_string(), Value::String("user".to_string()));
                subject.insert("id".to_string(), Value::String(user_id.clone()));
                merged_attributes
                    .entry("subject_id".to_string())
                    .or_insert(Value::String(user_id));
                "user"
            }
            Principal::ServicePrincipal(sp_id) => {
                subject.insert(
                    "kind".to_string(),
                    Value::String("service_principal".to_string()),
                );
                subject.insert("id".to_string(), Value::String(sp_id.get().to_string()));
                merged_attributes
                    .entry("subject_id".to_string())
                    .or_insert(Value::String(sp_id.get().to_string()));
                "service_principal"
            }
        };

        context.insert(
            "auth_method".to_string(),
            Value::String(auth_method.to_string()),
        );
        context.insert(
            "timestamp".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        );

        resource.insert(
            "type".to_string(),
            Value::String(resource_type.as_str().to_string()),
        );
        merged_attributes
            .entry("resource_type".to_string())
            .or_insert(Value::String(resource_type.as_str().to_string()));

        if let Some(resource_id) = resource_id {
            resource.insert("id".to_string(), Value::String(resource_id.to_string()));
            merged_attributes
                .entry("resource_id".to_string())
                .or_insert(Value::String(resource_id.to_string()));
        }

        if let Some(attributes) = attributes {
            let mut resource_attributes = Map::new();
            for (key, value) in attributes {
                resource_attributes.insert(key.clone(), value.clone());
                merged_attributes.entry(key).or_insert(value);
            }
            resource.insert("attributes".to_string(), Value::Object(resource_attributes));
        }

        context.insert("subject".to_string(), Value::Object(subject));
        context.insert("resource".to_string(), Value::Object(resource));
        if !merged_attributes.is_empty() {
            context.insert("attributes".to_string(), Value::Object(merged_attributes));
        }

        Ok(Value::Object(context))
    }

    fn build_policy_context(
        request_context: &RequestContext,
        resource_type: &ResourceType,
        resource_id: Option<&str>,
        attributes: Option<&HashMap<String, Value>>,
    ) -> Result<Value, AppError> {
        let mut context = Map::new();
        let mut subject = Map::new();
        let mut resource = Map::new();
        let mut merged_attributes = Map::new();

        if let Some(ip) = &request_context.request_ip {
            context.insert("ip".to_string(), Value::String(ip.clone()));
        }

        if let Some(user_id) = &request_context.user_id {
            let user_id = user_id.to_string();
            context.insert("user_id".to_string(), Value::String(user_id.clone()));
            subject.insert("id".to_string(), Value::String(user_id.clone()));
            merged_attributes
                .entry("subject_id".to_string())
                .or_insert(Value::String(user_id));
        }

        context.insert(
            "auth_method".to_string(),
            Value::String(request_context.auth_method.clone()),
        );
        context.insert(
            "correlation_id".to_string(),
            Value::String(request_context.correlation_id.clone()),
        );

        context.insert(
            "timestamp".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        );

        resource.insert(
            "type".to_string(),
            Value::String(resource_type.as_str().to_string()),
        );
        merged_attributes
            .entry("resource_type".to_string())
            .or_insert(Value::String(resource_type.as_str().to_string()));

        if let Some(resource_id) = resource_id {
            resource.insert("id".to_string(), Value::String(resource_id.to_string()));
            merged_attributes
                .entry("resource_id".to_string())
                .or_insert(Value::String(resource_id.to_string()));
        }

        if let Some(attributes) = attributes {
            let mut resource_attributes = Map::new();
            for (key, value) in attributes {
                resource_attributes.insert(key.clone(), value.clone());
                merged_attributes
                    .entry(key.clone())
                    .or_insert(value.clone());
            }
            resource.insert("attributes".to_string(), Value::Object(resource_attributes));
        }

        if !subject.is_empty() {
            context.insert("subject".to_string(), Value::Object(subject));
        }

        if !resource.is_empty() {
            context.insert("resource".to_string(), Value::Object(resource));
        }

        if !merged_attributes.is_empty() {
            context.insert("attributes".to_string(), Value::Object(merged_attributes));
        }

        Ok(Value::Object(context))
    }

    async fn get_cached_policies(&self, cache_key: &str) -> Result<Option<Vec<Policy>>, AppError> {
        let Some(cache) = &self.policy_cache else {
            return Ok(None);
        };
        let started_at = Instant::now();
        let cached = {
            let mut conn = cache.clone();
            cache::get(&mut conn, cache_key).await?
        };
        match cached {
            Some(raw) => match serde_json::from_str(&raw) {
                Ok(policies) => {
                    metrics::record_cache_operation(
                        POLICY_CACHE_METRIC_NAME,
                        "get",
                        "hit",
                        started_at.elapsed().as_secs_f64(),
                    );
                    Ok(Some(policies))
                }
                Err(_) => {
                    let mut conn = cache.clone();
                    let _ = cache::delete(&mut conn, cache_key).await;
                    metrics::record_cache_operation(
                        POLICY_CACHE_METRIC_NAME,
                        "get",
                        "error",
                        started_at.elapsed().as_secs_f64(),
                    );
                    Ok(None)
                }
            },
            None => {
                metrics::record_cache_operation(
                    POLICY_CACHE_METRIC_NAME,
                    "get",
                    "miss",
                    started_at.elapsed().as_secs_f64(),
                );
                Ok(None)
            }
        }
    }

    async fn set_cached_policies(
        &self,
        cache_key: &str,
        policies: &[Policy],
    ) -> Result<(), AppError> {
        let Some(cache) = &self.policy_cache else {
            return Ok(());
        };
        let started_at = Instant::now();
        let raw = serde_json::to_string(policies)
            .map_err(|e| AppError::Validation(format!("Invalid cache payload: {}", e)))?;
        let result = {
            let mut conn = cache.clone();
            cache::set(
                &mut conn,
                cache_key,
                &raw,
                Some(self.policy_cache_ttl_seconds),
            )
            .await
        };
        metrics::record_cache_operation(
            POLICY_CACHE_METRIC_NAME,
            "set",
            if result.is_ok() { "ok" } else { "error" },
            started_at.elapsed().as_secs_f64(),
        );
        result
    }

    async fn load_cache_generations(&self, user_id: Uuid) -> Result<(u64, u64), AppError> {
        let Some(cache) = &self.policy_cache else {
            return Ok((0, 0));
        };
        let mut conn = cache.clone();
        load_policy_cache_generations(&mut conn, user_id).await
    }
}

fn action_key(action: &Action) -> &'static str {
    match action {
        Action::Create => "create",
        Action::Read => "read",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Connect => "connect",
        Action::Observe => "observe",
        Action::Execute => "execute",
        Action::CctvView => "view",
        Action::CctvRecordings => "recordings",
        Action::CctvExport => "export",
        Action::DeviceUsers => "device_users",
        Action::CctvSettings => "settings",
        Action::CctvPtz => "ptz",
        Action::ManageRoles => "manage_roles",
        Action::ManagePolicies => "manage_policies",
        Action::ViewAudit => "view_audit",
        Action::Secret => "secret",
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use uuid::Uuid;

    use super::*;
    use crate::contexts::access_control::domain::{Group, PolicyEffect, Role, User};
    use crate::contexts::access_control::infrastructure::persistence::{
        NamedListFilter, PolicyFilter, PolicyRepository, RoleRepository, UserListFilter,
        UserRepository,
    };
    use crate::contexts::shared::pagination::PageResult;

    /// Returns a typed error for a stub method these tests never exercise.
    ///
    /// Returning an explicit error (rather than an `unimplemented` macro call)
    /// keeps an accidental call a visible test failure without a panic, and
    /// keeps the stubs free of the matching lint suppression.
    fn stub_unimplemented<T>(stub: &'static str, method: &'static str) -> Result<T, AppError> {
        Err(AppError::Internal(format!(
            "{stub}::{method} is not implemented in this test stub; a call is a test bug"
        )))
    }

    #[test]
    fn stub_unimplemented_returns_typed_error_naming_the_stub_and_method() {
        let error = stub_unimplemented::<()>("StubX", "method_y")
            .expect_err("a stub method must return an error");
        match error {
            AppError::Internal(message) => {
                assert!(message.contains("StubX"), "stub name missing: {message}");
                assert!(
                    message.contains("method_y"),
                    "method name missing: {message}"
                );
            }
            other => panic!("expected AppError::Internal, got {other:?}"),
        }
    }

    // --- Filtering stub repositories ---
    //
    // The DB does the subject/claim filtering in production (`subjects ?|` for
    // policies, `external_id = ANY($1)` for groups, the claim self-join for
    // roles). `evaluate_policies_with_metadata` does NOT filter by subject, so
    // these stubs MUST filter on their argument or the security tests below
    // (`sp_isolation`, `oidc_normalization_parity`) become vacuous.

    struct StubUserRepo {
        /// All `identity_group` rows the repo knows about for OIDC claim mapping.
        groups: Vec<Group>,
        /// Durable user memberships returned for every test user.
        persisted_groups: Vec<Group>,
    }

    impl StubUserRepo {
        fn new() -> Self {
            Self {
                groups: Vec::new(),
                persisted_groups: Vec::new(),
            }
        }

        fn with_groups(mut self, groups: Vec<Group>) -> Self {
            self.groups = groups;
            self
        }

        fn with_persisted_groups(mut self, groups: Vec<Group>) -> Self {
            self.persisted_groups = groups;
            self
        }
    }

    #[async_trait]
    impl UserRepository for StubUserRepo {
        async fn get_by_id(&self, _user_id: Uuid) -> Result<Option<User>, AppError> {
            Ok(None)
        }

        async fn get_by_username_or_email(
            &self,
            _identifier: &str,
        ) -> Result<Option<User>, AppError> {
            stub_unimplemented("StubUserRepo", "get_by_username_or_email")
        }

        async fn get_by_external_id(&self, _external_id: &str) -> Result<Option<User>, AppError> {
            stub_unimplemented("StubUserRepo", "get_by_external_id")
        }

        async fn list_users(&self) -> Result<Vec<User>, AppError> {
            stub_unimplemented("StubUserRepo", "list_users")
        }

        async fn list_users_page(
            &self,
            _filter: UserListFilter,
            _limit: u32,
            _offset: u32,
        ) -> Result<PageResult<User>, AppError> {
            stub_unimplemented("StubUserRepo", "list_users_page")
        }

        async fn create_user(&self, _user: User) -> Result<User, AppError> {
            stub_unimplemented("StubUserRepo", "create_user")
        }

        async fn update_user(&self, _user: User) -> Result<User, AppError> {
            stub_unimplemented("StubUserRepo", "update_user")
        }

        async fn delete_user(&self, _user_id: Uuid) -> Result<bool, AppError> {
            stub_unimplemented("StubUserRepo", "delete_user")
        }

        async fn list_groups_by_external_ids(
            &self,
            external_ids: &[String],
        ) -> Result<Vec<Group>, AppError> {
            Ok(self
                .groups
                .iter()
                .filter(|group| {
                    group
                        .external_id
                        .as_deref()
                        .is_some_and(|external_id| external_ids.iter().any(|id| id == external_id))
                })
                .cloned()
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

    /// Returns the effective role set for a user: direct roles plus roles from
    /// persisted group memberships. Current OIDC claims do not grant roles.
    struct StubRoleRepo {
        effective_roles: Vec<Role>,
    }

    impl StubRoleRepo {
        fn new() -> Self {
            Self {
                effective_roles: Vec::new(),
            }
        }

        fn with_direct_roles(mut self, roles: Vec<Role>) -> Self {
            self.effective_roles = roles;
            self
        }
    }

    #[async_trait]
    impl RoleRepository for StubRoleRepo {
        async fn list_roles_for_user(&self, _user_id: Uuid) -> Result<Vec<Role>, AppError> {
            let mut roles = self.effective_roles.clone();
            roles.sort_by_key(|left| left.id);
            roles.dedup_by_key(|role| role.id);
            Ok(roles)
        }

        async fn get_by_id(&self, _role_id: Uuid) -> Result<Option<Role>, AppError> {
            stub_unimplemented("StubRoleRepo", "get_by_id")
        }

        async fn list_roles(&self) -> Result<Vec<Role>, AppError> {
            stub_unimplemented("StubRoleRepo", "list_roles")
        }

        async fn list_roles_page(
            &self,
            _filter: NamedListFilter,
            _limit: u32,
            _offset: u32,
        ) -> Result<PageResult<Role>, AppError> {
            stub_unimplemented("StubRoleRepo", "list_roles_page")
        }

        async fn create_role(&self, _role: Role) -> Result<Role, AppError> {
            stub_unimplemented("StubRoleRepo", "create_role")
        }

        async fn update_role(&self, _role: Role) -> Result<Role, AppError> {
            stub_unimplemented("StubRoleRepo", "update_role")
        }

        async fn delete_role(&self, _role_id: Uuid) -> Result<bool, AppError> {
            stub_unimplemented("StubRoleRepo", "delete_role")
        }
    }

    /// Returns the configured policies whose `subjects` intersect the resolved
    /// subject set — mirrors the `subjects ?| $1` GIN match. This is what makes
    /// `sp_isolation` real: SP B's subjects never intersect SP A's policy.
    struct StubPolicyRepo {
        policies: Vec<Policy>,
    }

    impl StubPolicyRepo {
        fn new() -> Self {
            Self {
                policies: Vec::new(),
            }
        }

        fn with_policies(policies: Vec<Policy>) -> Self {
            Self { policies }
        }
    }

    #[async_trait]
    impl PolicyRepository for StubPolicyRepo {
        async fn list_policies(&self, _filter: PolicyFilter) -> Result<Vec<Policy>, AppError> {
            stub_unimplemented("StubPolicyRepo", "list_policies")
        }

        async fn list_policies_page(
            &self,
            _filter: PolicyFilter,
            _limit: u32,
            _offset: u32,
        ) -> Result<PageResult<Policy>, AppError> {
            stub_unimplemented("StubPolicyRepo", "list_policies_page")
        }

        async fn get_policy(&self, _policy_id: Uuid) -> Result<Option<Policy>, AppError> {
            stub_unimplemented("StubPolicyRepo", "get_policy")
        }

        async fn create_policy(&self, _policy: Policy) -> Result<Policy, AppError> {
            stub_unimplemented("StubPolicyRepo", "create_policy")
        }

        async fn update_policy(&self, _policy: Policy) -> Result<Policy, AppError> {
            stub_unimplemented("StubPolicyRepo", "update_policy")
        }

        async fn delete_policy(&self, _policy_id: Uuid) -> Result<(), AppError> {
            stub_unimplemented("StubPolicyRepo", "delete_policy")
        }

        async fn set_policy_enabled(
            &self,
            _policy_id: Uuid,
            _enabled: bool,
        ) -> Result<Policy, AppError> {
            stub_unimplemented("StubPolicyRepo", "set_policy_enabled")
        }

        async fn list_for_subjects(&self, subjects: &[String]) -> Result<Vec<Policy>, AppError> {
            Ok(self
                .policies
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

    fn make_group(name: &str, external_id: Option<&str>) -> Group {
        Group {
            id: Uuid::now_v7(),
            name: name.to_string(),
            external_id: external_id.map(str::to_string),
        }
    }

    fn make_role(name: &str) -> Role {
        Role {
            id: Uuid::now_v7(),
            name: name.to_string(),
            description: None,
        }
    }

    /// A policy that matches `Read` on `ConnectionTarget` for the given subjects.
    fn policy_for(effect: PolicyEffect, subjects: Vec<String>) -> Policy {
        Policy {
            id: Uuid::now_v7(),
            name: "test-policy".to_string(),
            description: None,
            effect,
            actions: vec![Action::Read],
            resource_types: vec![ResourceType::ConnectionTarget],
            constraints: None,
            subjects,
            resources: Vec::new(),
            enabled: true,
        }
    }

    fn service(
        user_repo: StubUserRepo,
        role_repo: StubRoleRepo,
        policy_repo: StubPolicyRepo,
    ) -> AccessControlService<StubUserRepo, StubRoleRepo, StubPolicyRepo> {
        AccessControlService::new(user_repo, role_repo, policy_repo)
    }

    // --- resolve_subjects: canonical `kind:uuid` grammar ---

    #[tokio::test]
    async fn resolve_subjects_user() {
        let user_id = Uuid::now_v7();
        let role = make_role("admin");
        let role_id = role.id;

        let svc = service(
            StubUserRepo::new(),
            StubRoleRepo::new().with_direct_roles(vec![role]),
            StubPolicyRepo::new(),
        );

        let subjects = svc
            .resolve_subjects(&Principal::user(user_id))
            .await
            .expect("resolve_subjects should succeed");

        assert!(
            subjects.contains(&format!("user:{user_id}")),
            "must emit canonical user subject; got {subjects:?}"
        );
        assert!(
            subjects.contains(&format!("role:{role_id}")),
            "must emit role:{{uuid}} for a direct assignment; got {subjects:?}"
        );
        // No bare names leak into the canonical grammar.
        assert!(
            !subjects.iter().any(|subject| subject == "admin"),
            "must not emit bare role name; got {subjects:?}"
        );
    }

    #[tokio::test]
    async fn resolve_subjects_persisted_group_membership() {
        // A persisted membership resolves to group:{uuid}; role resolution is
        // likewise based on the repository's effective user roles, not on the
        // request's transient OIDC claims.
        let user_id = Uuid::now_v7();
        let group = make_group("admins", Some("/piping-admin"));
        let group_id = group.id;
        let role = make_role("admin");
        let role_id = role.id;

        let svc = service(
            StubUserRepo::new()
                .with_groups(vec![group.clone()])
                .with_persisted_groups(vec![group]),
            StubRoleRepo::new().with_direct_roles(vec![role]),
            StubPolicyRepo::new(),
        );

        let subjects = svc
            .resolve_subjects(&Principal::user(user_id))
            .await
            .expect("resolve_subjects should succeed");

        assert!(
            subjects.contains(&format!("group:{group_id}")),
            "persisted membership must emit group:{{uuid}}; got {subjects:?}"
        );
        assert!(
            subjects.contains(&format!("role:{role_id}")),
            "effective group role must emit role:{{uuid}}; got {subjects:?}"
        );
    }

    #[tokio::test]
    async fn resolve_subjects_service_principal() {
        // Exactly `service_principal:{uuid}` — no bare catch-all, no slug.
        let sp_id = Uuid::now_v7();

        let svc = service(
            StubUserRepo::new(),
            StubRoleRepo::new(),
            StubPolicyRepo::new(),
        );

        let subjects = svc
            .resolve_subjects(&Principal::service_principal(sp_id))
            .await
            .expect("resolve_subjects should succeed");

        assert_eq!(
            subjects,
            vec![format!("service_principal:{sp_id}")],
            "SP must resolve to exactly its uuid subject; got {subjects:?}"
        );
        assert!(
            !subjects
                .iter()
                .any(|subject| subject == "service_principal"),
            "the bare catch-all subject must never be emitted; got {subjects:?}"
        );
        // No slug form: the only subject is the uuid one asserted above.
        assert!(
            subjects
                .iter()
                .all(|subject| subject == &format!("service_principal:{sp_id}")),
            "no slug-form service principal subject may be emitted; got {subjects:?}"
        );
    }

    #[tokio::test]
    async fn session_and_api_token_share_persisted_group_subject_policy() {
        let user_id = Uuid::now_v7();
        let group = make_group("admins", Some("/piping-admin"));
        let group_id = group.id;
        let policy = policy_for(PolicyEffect::Allow, vec![format!("group:{group_id}")]);

        let svc = service(
            StubUserRepo::new().with_persisted_groups(vec![group]),
            StubRoleRepo::new(),
            StubPolicyRepo::with_policies(vec![policy]),
        );

        let session_ctx = RequestContext::new(
            Some(user_id),
            "oidc".to_string(),
            None,
            Uuid::now_v7().to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                oidc_groups: vec!["/claim-no-longer-authorizes".to_string()],
                ..crate::contexts::shared::request_context::RequestAuth::default()
            },
        );
        let api_token_ctx = RequestContext::new(
            Some(user_id),
            "api_token".to_string(),
            None,
            Uuid::now_v7().to_string(),
            crate::contexts::shared::request_context::RequestAuth {
                api_token_scopes: vec![
                    crate::contexts::shared::request_context::ApiTokenScopeContext {
                        action: "read".to_string(),
                        resource_type: "connection_target".to_string(),
                        resource_pattern: "*".to_string(),
                    },
                ],
                ..crate::contexts::shared::request_context::RequestAuth::default()
            },
        );

        assert_eq!(
            svc.check_access(
                &session_ctx,
                Action::Read,
                ResourceType::ConnectionTarget,
                None
            )
            .await
            .expect("session check_access should succeed"),
            PolicyDecision::Allow
        );
        assert_eq!(
            svc.check_access(
                &api_token_ctx,
                Action::Read,
                ResourceType::ConnectionTarget,
                None
            )
            .await
            .expect("api token check_access should succeed"),
            PolicyDecision::Allow
        );
        assert!(
            AccessControlService::<StubUserRepo, StubRoleRepo, StubPolicyRepo>::api_token_scope_allows(
                &api_token_ctx,
                &Action::Read,
                &ResourceType::ConnectionTarget,
                None,
            ),
            "PAT scope must permit the RBAC-allowed action"
        );
    }

    // --- check_access_for_principal: evaluation semantics ---

    #[tokio::test]
    async fn deny_precedence() {
        // A role:{uuid} allow plus a user:{uuid} deny on the same
        // action/resource resolves to Deny (deny-override preserved).
        let user_id = Uuid::now_v7();
        let role = make_role("admin");
        let role_id = role.id;

        let allow = policy_for(PolicyEffect::Allow, vec![format!("role:{role_id}")]);
        let deny = policy_for(PolicyEffect::Deny, vec![format!("user:{user_id}")]);

        let svc = service(
            StubUserRepo::new(),
            StubRoleRepo::new().with_direct_roles(vec![role]),
            StubPolicyRepo::with_policies(vec![allow, deny]),
        );

        let decision = svc
            .check_access_for_principal(
                &Principal::user(user_id),
                Action::Read,
                ResourceType::ConnectionTarget,
                None,
                None,
            )
            .await
            .expect("check_access_for_principal should succeed");

        assert_eq!(
            decision.decision,
            PolicyDecision::Deny,
            "a user:{{uuid}} deny must override a role:{{uuid}} allow"
        );
    }

    #[tokio::test]
    async fn sp_isolation() {
        // SP A's policy (`service_principal:{A}`) must NOT match SP B.
        let sp_a = Uuid::now_v7();
        let sp_b = Uuid::now_v7();

        let allow_for_a = policy_for(
            PolicyEffect::Allow,
            vec![format!("service_principal:{sp_a}")],
        );

        let svc = service(
            StubUserRepo::new(),
            StubRoleRepo::new(),
            StubPolicyRepo::with_policies(vec![allow_for_a]),
        );

        let decision_a = svc
            .check_access_for_principal(
                &Principal::service_principal(sp_a),
                Action::Read,
                ResourceType::ConnectionTarget,
                None,
                None,
            )
            .await
            .expect("check_access_for_principal should succeed");
        let decision_b = svc
            .check_access_for_principal(
                &Principal::service_principal(sp_b),
                Action::Read,
                ResourceType::ConnectionTarget,
                None,
                None,
            )
            .await
            .expect("check_access_for_principal should succeed");

        assert_eq!(
            decision_a.decision,
            PolicyDecision::Allow,
            "SP A must be allowed by its own policy"
        );
        assert_eq!(
            decision_b.decision,
            PolicyDecision::Deny,
            "SP B must not inherit SP A's policy (default deny)"
        );
    }
}
