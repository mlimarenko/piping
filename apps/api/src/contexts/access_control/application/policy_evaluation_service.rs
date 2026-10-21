use crate::contexts::access_control::application::AccessControlService;
use crate::contexts::access_control::domain::{Action, PolicyDecision, Principal, ResourceType};
use crate::contexts::access_control::infrastructure::persistence::{
    PolicyRepository, RoleRepository, UserRepository,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

pub struct PolicyEvaluationService<U, R, P> {
    access_control: AccessControlService<U, R, P>,
}

impl<U, R, P> PolicyEvaluationService<U, R, P>
where
    U: UserRepository,
    R: RoleRepository,
    P: PolicyRepository,
{
    pub fn new(access_control: AccessControlService<U, R, P>) -> Self {
        Self { access_control }
    }

    pub async fn evaluate(
        &self,
        request_context: &RequestContext,
        action: Action,
        resource_type: ResourceType,
        resource_id: Option<&str>,
    ) -> Result<PolicyDecision, AppError> {
        self.access_control
            .check_access(request_context, action, resource_type, resource_id)
            .await
    }

    pub async fn ensure_access(
        &self,
        request_context: &RequestContext,
        action: Action,
        resource_type: ResourceType,
        resource_id: Option<&str>,
    ) -> Result<(), AppError> {
        self.ensure_access_with_attributes(
            request_context,
            action,
            resource_type,
            resource_id,
            None,
        )
        .await
    }

    /// Inventory collections require a genuinely global user grant; object
    /// grants must not expose collection rows or counts. Inventory job tokens
    /// use the canonical scope decision instead of a user-policy identity.
    pub async fn ensure_inventory_collection_access(
        &self,
        request_context: &RequestContext,
        action: Action,
    ) -> Result<(), AppError> {
        self.ensure_access(
            request_context,
            action.clone(),
            ResourceType::Inventory,
            Some("inventory:*"),
        )
        .await?;
        // ensure_access has already checked the token's action/resource scope.
        // A job token has no user; resolving user policies would reject a valid
        // inventory principal after its canonical decision has allowed access.
        if request_context.auth_method == "playbook_job_token" {
            return Ok(());
        }
        let trace = self
            .access_control
            .check_access_with_trace(request_context, action, ResourceType::Inventory, None, None)
            .await?;
        let policies = self
            .access_control
            .resolve_policies_for_principal(&Principal::User {
                id: request_context.require_authenticated_user_id()?,
            })
            .await?;
        if trace.decision == PolicyDecision::Allow
            && policies.iter().any(|policy| {
                trace.matched_allow.contains(&policy.id)
                    && (policy.resources.is_empty()
                        || policy
                            .resources
                            .iter()
                            .any(|resource| resource == "*" || resource == "inventory:*"))
            })
        {
            Ok(())
        } else {
            Err(AppError::Forbidden(
                "Global inventory permission required".into(),
            ))
        }
    }

    /// Like [`ensure_access`], but carries the RESOLVED TARGET OBJECT's
    /// attributes (e.g. a device's `role_id`/`site_id`/`protocol`) into the
    /// policy-evaluation context so attribute-scoped constraints actually apply.
    ///
    /// `ensure_access` passes `None` — the behavior-preserving default for the
    /// ~600 enforcement sites that do not (yet) resolve object attributes. A site
    /// that CAN resolve the target uses this variant to enable ABAC scoping.
    /// The attributes MUST come from the resolved target object, not
    /// the caller — a caller-controlled attribute would be an authorization
    /// bypass.
    ///
    /// [`ensure_access`]: Self::ensure_access
    pub async fn ensure_access_with_attributes(
        &self,
        request_context: &RequestContext,
        action: Action,
        resource_type: ResourceType,
        resource_id: Option<&str>,
        attributes: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<(), AppError> {
        if !AccessControlService::<U, R, P>::api_token_scope_allows(
            request_context,
            &action,
            &resource_type,
            resource_id,
        ) {
            return Err(AppError::Forbidden(
                "TOKEN_SCOPE_DENIED:token scope does not permit this action".to_string(),
            ));
        }
        if request_context.auth_method == "playbook_job_token" {
            if resource_type == ResourceType::Inventory {
                return Ok(());
            }
            return Err(AppError::Forbidden(
                "TOKEN_SCOPE_DENIED:playbook job token is limited to inventory".to_string(),
            ));
        }
        // Evaluate with the request-derived context (ip/timestamp/subject/
        // resource) plus any resolved object attributes so policy constraints
        // actually apply. The bare `evaluate` path passes a null context: every
        // constraint then reads a missing value and evaluates false, so a
        // constrained policy is DROPPED from the decision (a constrained allow
        // silently never grants; a constrained deny silently never fires). This
        // guarded path restores that evaluation and enforces the token scope.
        let decision = self
            .access_control
            .check_access_with_attributes(
                request_context,
                action,
                resource_type,
                resource_id,
                attributes,
            )
            .await?;
        if decision == PolicyDecision::Deny {
            return Err(AppError::Forbidden("Access denied".to_string()));
        }
        Ok(())
    }
}
