//! Shared app-layer orchestrator for the playbook launch gate: the per-secret
//! `Execute × SecretVariable` authorization plus explicit credential delegation.
//!
//! One gate authorizes the prepared final launch spec for both callers and they
//! MUST reach the same verdict:
//! - a manual REST launch, driven by the live request context (and its PAT token
//!   scope ceiling);
//! - a scheduled fire, driven by the run group's service principal (no token, so
//!   the ceiling is always satisfied).
//!
//! Both resolve policies/constraints once and delegate the per-secret decision
//! to the pure [`secret_env_authorization_snapshot`] core, so enforcement,
//! preflight and the authorization snapshot share one implementation.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::contexts::access_control::domain::{Action, PolicyDecision, Principal, ResourceType};
use crate::contexts::access_control::infrastructure::persistence::UserRepository;
use crate::contexts::connections::application::connection_service::SshRecordDelegationAuthorizer;
use crate::contexts::inventory::application::resolve_connection_target_authorization_facets;
use crate::contexts::playbooks::application::secret_gate::{
    ResolvedDeclaredSecret, secret_env_authorization_snapshot, secret_use_candidates,
    secret_use_pat_allows,
};
use crate::contexts::playbooks::application::{
    LaunchAuthorizationSnapshot, PlaybookLaunchAuthorizer, principal_subject,
};
use crate::contexts::playbooks::domain::{InventorySelection, LaunchSpec};
use crate::contexts::secrets::application::JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE;
use crate::contexts::secrets::domain::SecretUsageBinding;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use super::{
    AccessControlServiceType, ConnectionServiceType, SqlxSecretRepository, SqlxUserRepository,
};

/// Applies the existing live policy evaluator to the exact prepared SSH record route.
pub(crate) struct RecordSshDelegation<'a> {
    access_control: &'a AccessControlServiceType,
    principal: &'a Principal,
    request_context: Option<&'a RequestContext>,
    launch_contexts: Option<&'a [Uuid]>,
}

impl<'a> RecordSshDelegation<'a> {
    pub(crate) fn for_launch(
        access_control: &'a AccessControlServiceType,
        principal: &'a Principal,
        request_context: Option<&'a RequestContext>,
    ) -> Self {
        Self {
            access_control,
            principal,
            request_context,
            launch_contexts: None,
        }
    }

    pub(crate) fn for_dispatch(
        access_control: &'a AccessControlServiceType,
        principal: &'a Principal,
        launch_contexts: &'a [Uuid],
    ) -> Self {
        Self {
            access_control,
            principal,
            request_context: None,
            launch_contexts: Some(launch_contexts),
        }
    }
}

#[async_trait]
impl SshRecordDelegationAuthorizer for RecordSshDelegation<'_> {
    async fn authorize_records(&self, record_ids: &[Uuid]) -> Result<(), AppError> {
        if self
            .launch_contexts
            .is_some_and(|authorized| record_ids.iter().any(|id| !authorized.contains(id)))
        {
            return Err(AppError::Forbidden(
                "SSH record route exceeds the launch-time credential delegation".into(),
            ));
        }
        for id in record_ids {
            require_launch_action(
                self.access_control,
                self.principal,
                self.request_context,
                Action::DelegateCredentials,
                ResourceType::ConfigContext,
                &id.to_string(),
                None,
            )
            .await?;
        }
        Ok(())
    }
}

/// The dispatch-time C3 re-check for an already-resolved live principal.
///
/// The worker holds the run group's stored initiator, not a request context, so
/// it evaluates the principal's live policies directly. A grant revoked between
/// authoring and dispatch denies here. There is no PAT ceiling at dispatch: the
/// token ceiling belongs to the launch request.
pub(crate) async fn principal_secret_use_grant(
    access_control: &AccessControlServiceType,
    principal: &Principal,
    secret_id: Uuid,
    key: &str,
) -> Result<bool, AppError> {
    let candidates = secret_use_candidates(secret_id, key);
    let mut decisions = Vec::with_capacity(candidates.len());
    for candidate in &candidates {
        decisions.push(
            access_control
                .check_access_for_principal(
                    principal,
                    Action::Execute,
                    ResourceType::SecretVariable,
                    Some(candidate.as_str()),
                    None,
                )
                .await?,
        );
    }
    if decisions.iter().any(|meta| !meta.matched_deny.is_empty()) {
        return Ok(false);
    }
    Ok(decisions
        .iter()
        .any(|meta| meta.decision == PolicyDecision::Allow))
}

/// App-layer adapter for the single launch authorization gate.
///
/// One gate authorizes the final immutable launch spec before storage for both
/// manual (request context, PAT ceiling preserved) and scheduled
/// (schedule owner) fires. The same function is re-run at dispatch against the
/// stored spec and the live principal.
pub(crate) struct PlaybookLaunchGate {
    access_control: Arc<AccessControlServiceType>,
    secrets: Arc<SqlxSecretRepository>,
    users: Arc<SqlxUserRepository>,
    connection: Arc<ConnectionServiceType>,
    pool: PgPool,
}

impl PlaybookLaunchGate {
    pub(crate) fn new(
        access_control: Arc<AccessControlServiceType>,
        secrets: Arc<SqlxSecretRepository>,
        users: Arc<SqlxUserRepository>,
        connection: Arc<ConnectionServiceType>,
        pool: PgPool,
    ) -> Self {
        Self {
            access_control,
            secrets,
            users,
            connection,
            pool,
        }
    }
}

#[async_trait]
impl PlaybookLaunchAuthorizer for PlaybookLaunchGate {
    async fn authorize_launch(
        &self,
        principal: &Principal,
        request_context: Option<&RequestContext>,
        spec: &LaunchSpec,
    ) -> Result<LaunchAuthorizationSnapshot, AppError> {
        let secret_env = authorize_prepared_launch(
            &self.pool,
            &self.access_control,
            &self.secrets,
            principal,
            request_context,
            spec,
        )
        .await?;
        let ssh_record_contexts = match &spec.inventory {
            InventorySelection::Native {
                ssh_record_id: Some(record_id),
                ..
            } => {
                let credential_user_id = match principal {
                    Principal::User { id } => Some(*id),
                    Principal::ServicePrincipal(_) => None,
                };
                self.connection
                    .authorize_ssh_record_delegation(
                        *record_id,
                        credential_user_id,
                        &RecordSshDelegation::for_launch(
                            &self.access_control,
                            principal,
                            request_context,
                        ),
                    )
                    .await?
            }
            _ => Vec::new(),
        };
        Ok(LaunchAuthorizationSnapshot {
            secret_env,
            ssh_record_contexts,
        })
    }

    async fn validate_schedule_owner(&self, user_id: Uuid) -> Result<(), AppError> {
        let user = self.users.get_by_id(user_id).await?;
        if !user.is_some_and(|user| user.is_active()) {
            return Err(AppError::Forbidden(
                "Schedule owner no longer exists or is not active".to_string(),
            ));
        }
        Ok(())
    }
}

/// Authorize one prepared launch spec and return its secret snapshot.
///
/// The snapshot records `ENV -> {secret_id, key}` from the spec's UUID references,
/// never a mutable named-key alias. Delegation is explicit: every project launch
/// requires `ExecuteNative` on the job template (controller-side execution is
/// possible even with a managed inventory), managed SSH targets additionally
/// require `DelegateCredentials` on each connection target, and record-backed
/// native/SCM transport requires `DelegateCredentials` on its config context.
pub(crate) async fn authorize_prepared_launch(
    pool: &PgPool,
    access_control: &AccessControlServiceType,
    secrets: &SqlxSecretRepository,
    principal: &Principal,
    request_context: Option<&RequestContext>,
    spec: &LaunchSpec,
) -> Result<Option<Value>, AppError> {
    authorize_delegation(pool, access_control, principal, request_context, spec).await?;

    let mut resolved = Vec::with_capacity(spec.options.secret_refs.len());
    for (env_name, secret_id) in &spec.options.secret_refs {
        // Explicit delegation is separate from the C3 Execute grant: exporting a
        // raw secret value is a credential-delegation decision.
        require_launch_action(
            access_control,
            principal,
            request_context,
            Action::DelegateCredentials,
            ResourceType::SecretVariable,
            &secret_id.to_string(),
            None,
        )
        .await?;
        let binding = secrets
            .get_usage_binding(
                JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE,
                spec.template_id,
                env_name,
            )
            .await?;
        let Some(binding) = binding else {
            return Err(AppError::Forbidden(format!(
                "runtime secret binding for env `{env_name}` was removed from the job template"
            )));
        };
        if !runtime_env_binding_matches(&binding, spec.template_id, env_name, *secret_id) {
            return Err(AppError::Forbidden(format!(
                "runtime secret binding for env `{env_name}` was revoked or retargeted"
            )));
        }
        let entry = match secrets.get_variable_by_id(*secret_id).await {
            Ok(Some(secret)) => Ok(ResolvedDeclaredSecret {
                env_name: env_name.clone(),
                key: secret.key,
                secret_id: secret.id,
                is_protected: secret.is_protected,
            }),
            Ok(None) => Err(AppError::Validation(format!(
                "Playbook declares runtime secret (env {env_name}) that does not exist"
            ))),
            Err(error) => Err(error),
        };
        resolved.push(entry);
    }
    if resolved.is_empty() {
        return Ok(None);
    }

    let subject = principal_subject(principal);
    match request_context {
        Some(request_context) => {
            let policies = access_control
                .resolve_policies_for_request(request_context)
                .await?;
            let context = access_control.constraint_context_for_request(
                request_context,
                ResourceType::SecretVariable,
                None,
            )?;
            secret_env_authorization_snapshot(
                &policies,
                &context,
                &subject,
                resolved,
                |candidate| secret_use_pat_allows(request_context, candidate),
            )
        }
        None => {
            let policies = access_control
                .resolve_policies_for_principal(principal)
                .await?;
            let context = access_control.constraint_context_for_principal(
                principal,
                ResourceType::SecretVariable,
                None,
            )?;
            secret_env_authorization_snapshot(&policies, &context, &subject, resolved, |_| true)
        }
    }
}

async fn authorize_delegation(
    pool: &PgPool,
    access_control: &AccessControlServiceType,
    principal: &Principal,
    request_context: Option<&RequestContext>,
    spec: &LaunchSpec,
) -> Result<(), AppError> {
    let template_id = spec.template_id.to_string();
    // Every launch must be authorized to use the template itself.
    require_launch_action(
        access_control,
        principal,
        request_context,
        Action::Execute,
        ResourceType::PlaybookJobTemplate,
        &template_id,
        None,
    )
    .await?;
    // A project launch can execute controller-side code (local/delegate_to), so
    // controller execution is required for every project launch, not only for a
    // native inventory. Managed inventory does not remove that risk.
    require_launch_action(
        access_control,
        principal,
        request_context,
        Action::ExecuteNative,
        ResourceType::PlaybookJobTemplate,
        &template_id,
        None,
    )
    .await?;

    match &spec.inventory {
        InventorySelection::Native { ssh_record_id, .. } => {
            if let Some(record_id) = ssh_record_id {
                // Record-backed native/SCM transport is a config-context
                // delegation. The shared record resolver owns inherited-chain
                // expansion; this gate authorizes the explicit record UUID.
                require_launch_action(
                    access_control,
                    principal,
                    request_context,
                    Action::DelegateCredentials,
                    ResourceType::ConfigContext,
                    &record_id.to_string(),
                    None,
                )
                .await?;
            }
        }
        InventorySelection::Managed { target_set_id } => {
            require_launch_action(
                access_control,
                principal,
                request_context,
                Action::Read,
                ResourceType::PlaybookTargetSet,
                &target_set_id.to_string(),
                None,
            )
            .await?;
            if spec.targets.is_empty() {
                return Err(AppError::Validation(
                    "Managed launch has no fixed targets".to_string(),
                ));
            }
        }
    }
    let target_ids = spec
        .targets
        .iter()
        .map(|target| {
            target.target_id.ok_or_else(|| {
                AppError::Validation("Launch target has no connection target".into())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let facets = resolve_connection_target_authorization_facets(pool, &target_ids).await?;
    for target_id in target_ids {
        let target_facets = facets.get(&target_id).ok_or_else(|| {
            AppError::NotFound("Launch connection target no longer exists".into())
        })?;
        let resource_id = format!("connection_target:{target_id}");
        let attributes =
            crate::api::connection_authz_attributes::connection_target_authz_attributes(
                target_facets,
                Some("ssh"),
            );
        for action in [Action::Connect, Action::DelegateCredentials] {
            require_launch_action(
                access_control,
                principal,
                request_context,
                action,
                ResourceType::ConnectionTarget,
                &resource_id,
                Some(attributes.clone()),
            )
            .await?;
        }
    }
    Ok(())
}

/// Whether a live usage binding still authorizes the stored runtime env.
///
/// The launch spec is immutable, but the template's env binding is not: a
/// removed or retargeted binding must revoke the stored reference even while the
/// old secret and the principal's grant still exist.
fn runtime_env_binding_matches(
    binding: &SecretUsageBinding,
    template_id: Uuid,
    env_name: &str,
    secret_id: Uuid,
) -> bool {
    binding.consumer_type == JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE
        && binding.consumer_id == template_id
        && binding.field_key == env_name
        && binding.secret_id == secret_id
        && binding.owner_scope_type.as_str() == ResourceType::PlaybookJobTemplate.as_str()
        && binding.owner_scope_id == template_id
}

/// Whether the request's personal API token carries the required capability.
///
/// This is the canonical PAT ceiling, not a second permission model: it only
/// narrows what the authenticated user's live policies already allow.
fn request_context_action_allowed(
    request_context: &RequestContext,
    action: &Action,
    resource_type: &ResourceType,
    resource_id: &str,
) -> bool {
    AccessControlServiceType::api_token_scope_allows(
        request_context,
        action,
        resource_type,
        Some(resource_id),
    )
}

async fn require_launch_action(
    access_control: &AccessControlServiceType,
    principal: &Principal,
    request_context: Option<&RequestContext>,
    action: Action,
    resource_type: ResourceType,
    resource_id: &str,
    attributes: Option<HashMap<String, Value>>,
) -> Result<(), AppError> {
    match request_context {
        Some(request_context) => {
            let Principal::User { id } = principal else {
                return Err(AppError::Forbidden(
                    "request-scoped playbook launch requires the authenticated user principal"
                        .to_string(),
                ));
            };
            if request_context.user_id != Some(*id) {
                return Err(AppError::Forbidden(
                    "playbook launch principal does not match the authenticated request user"
                        .to_string(),
                ));
            }
            // `check_access_with_attributes` evaluates the user's live policies
            // but does not impose the PAT ceiling itself; both are required.
            if !request_context_action_allowed(
                request_context,
                &action,
                &resource_type,
                resource_id,
            ) {
                return Err(AppError::Forbidden(format!(
                    "playbook API token lacks {} on {} {resource_id}",
                    action.as_str(),
                    resource_type.as_str()
                )));
            }
            let decision = access_control
                .check_access_with_attributes(
                    request_context,
                    action.clone(),
                    resource_type.clone(),
                    Some(resource_id),
                    attributes,
                )
                .await?;
            if decision != PolicyDecision::Allow {
                return Err(AppError::Forbidden(format!(
                    "playbook launch initiator lacks {} on {} {resource_id}",
                    action.as_str(),
                    resource_type.as_str()
                )));
            }
        }
        None => {
            let metadata = access_control
                .check_access_for_principal(
                    principal,
                    action.clone(),
                    resource_type.clone(),
                    Some(resource_id),
                    attributes,
                )
                .await?;
            if metadata.decision != PolicyDecision::Allow {
                return Err(AppError::Forbidden(format!(
                    "playbook launch initiator lacks {} on {} {resource_id}",
                    action.as_str(),
                    resource_type.as_str()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::shared::request_context::{ApiTokenScopeContext, RequestAuth};

    fn pat_context(
        user_id: Uuid,
        action: &str,
        resource_type: &str,
        resource_pattern: &str,
    ) -> RequestContext {
        RequestContext::new(
            Some(user_id),
            "api_token".to_string(),
            None,
            Uuid::nil().to_string(),
            RequestAuth {
                api_token_scopes: vec![ApiTokenScopeContext {
                    action: action.to_string(),
                    resource_type: resource_type.to_string(),
                    resource_pattern: resource_pattern.to_string(),
                }],
                ..RequestAuth::default()
            },
        )
    }

    /// A PAT scoped only to template Execute must not inherit the owner's
    /// ExecuteNative/DelegateCredentials/Connect grants. The gate calls the
    /// canonical PAT ceiling for every action, not only for C3 secrets.
    #[test]
    fn pat_ceiling_denies_actions_outside_its_scopes() {
        let user_id = Uuid::from_u128(1);
        let template_id = Uuid::from_u128(2).to_string();
        let target_id = Uuid::from_u128(3).to_string();
        let context = pat_context(user_id, "execute", "playbook_job_template", &template_id);

        assert!(request_context_action_allowed(
            &context,
            &Action::Execute,
            &ResourceType::PlaybookJobTemplate,
            &template_id
        ));
        for (action, resource_type, resource_id) in [
            (
                Action::ExecuteNative,
                ResourceType::PlaybookJobTemplate,
                template_id.as_str(),
            ),
            (
                Action::DelegateCredentials,
                ResourceType::ConnectionTarget,
                target_id.as_str(),
            ),
            (
                Action::Connect,
                ResourceType::ConnectionTarget,
                target_id.as_str(),
            ),
            (
                Action::Read,
                ResourceType::PlaybookTargetSet,
                template_id.as_str(),
            ),
        ] {
            assert!(
                !request_context_action_allowed(&context, &action, &resource_type, resource_id),
                "PAT must not gain {action:?} on {resource_type:?} outside its scopes"
            );
        }
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    fn binding(template_id: Uuid, env_name: &str, secret_id: Uuid) -> SecretUsageBinding {
        SecretUsageBinding {
            id: Uuid::from_u128(9),
            consumer_type: JOB_TEMPLATE_RUNTIME_ENV_CONSUMER_TYPE.to_string(),
            consumer_id: template_id,
            field_key: env_name.to_string(),
            owner_scope_type: ResourceType::PlaybookJobTemplate.as_str().to_string(),
            owner_scope_id: template_id,
            secret_id,
        }
    }

    #[test]
    fn retargeted_or_revoked_runtime_env_binding_does_not_match() {
        let template_id = Uuid::from_u128(1);
        let env_name = "TOKEN";
        let secret_id = Uuid::from_u128(2);
        let good = binding(template_id, env_name, secret_id);
        assert!(runtime_env_binding_matches(
            &good,
            template_id,
            env_name,
            secret_id
        ));

        let mut retargeted = good.clone();
        retargeted.secret_id = Uuid::from_u128(3);
        assert!(
            !runtime_env_binding_matches(&retargeted, template_id, env_name, secret_id),
            "a binding retargeted to another secret must revoke the stored env"
        );

        let mut wrong_owner = good.clone();
        wrong_owner.owner_scope_id = Uuid::from_u128(4);
        assert!(
            !runtime_env_binding_matches(&wrong_owner, template_id, env_name, secret_id),
            "a binding owned by another template must not authorize this launch"
        );

        let mut wrong_scope = good;
        wrong_scope.owner_scope_type = "playbook_project".to_string();
        assert!(
            !runtime_env_binding_matches(&wrong_scope, template_id, env_name, secret_id),
            "a non-template owner scope must not authorize runtime env export"
        );
    }
}
