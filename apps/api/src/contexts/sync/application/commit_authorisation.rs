//! Deferred-request commit authorisation (PWA-81).
//!
//! A pull started by a REST request finishes after that request has returned.
//! The provider lease keeps two runs from overlapping, but it proves nothing
//! about who asked: it is liveness only. [`RequestCommitAuthorisation`] keeps the
//! lease and re-asks the canonical request owner for the caller's *current*
//! ceilings at every commit boundary, at every reference READ and at every
//! object-level CREATE/UPDATE.
//!
//! What is re-checked is exactly: the account is still active, the current
//! policy decision still allows the action, and — for a personal API token — the
//! token is still active, unexpired, owned by the same user and still carries
//! the scope the action needs. The request context carries no session identity,
//! so session revocation itself is not re-checked here; only the durable state
//! this owner can actually read is.
//!
//! [`PolicyRequestCommitAuthority`] is the production implementation of that
//! fresh check. It holds no interpretation of its own: user activity comes from
//! the canonical account rule, personal-token state and scopes come from the
//! canonical token service, and every decision comes from the one policy
//! evaluator. A principal or token state this owner cannot represent fails
//! closed instead of inheriting the decision the request made earlier.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use futures_util::future::BoxFuture;
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::access_control::application::{PolicyEvaluationService, UserApiTokenService};
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::access_control::infrastructure::persistence::{
    PolicyRepository, RoleRepository, UserRepository,
};
use crate::contexts::shared::request_context::{ApiTokenScopeContext, RequestContext};
use crate::contexts::sync::domain::InventoryCommitAuthorisation;
use crate::errors::AppError;

/// The request principal's authority, re-resolved from its owning stores.
///
/// Implemented at the API boundary so the sync application never interprets
/// tokens or policies itself, and so a test can drive the composition with the
/// real policy evaluator and in-memory repositories.
pub trait RequestCommitAuthority: Send + Sync {
    /// Re-check that the request principal may still execute inventory work.
    fn ensure_commit_allowed<'a>(
        &'a self,
        request: &'a RequestContext,
    ) -> BoxFuture<'a, Result<(), AppError>>;

    /// Check READ on the persisted inventory object `target`, evaluated with the
    /// attributes read from that row.
    fn ensure_reference_read<'a>(
        &'a self,
        request: &'a RequestContext,
        target: Uuid,
        attributes: &'a Value,
    ) -> BoxFuture<'a, Result<(), AppError>>;

    /// Check CREATE or UPDATE on the inventory object `target`, evaluated with the
    /// attributes the writer owns for that decision.
    fn ensure_mutation_allowed<'a>(
        &'a self,
        request: &'a RequestContext,
        action: Action,
        target: Uuid,
        attributes: &'a Value,
    ) -> BoxFuture<'a, Result<(), AppError>>;
}

/// The provider lease plus the request that started the work.
///
/// Both the lease and the request authority must hold, so this is what a job
/// driven by a request hands to the writer instead of the bare lease.
pub struct RequestCommitAuthorisation {
    lease: Arc<dyn InventoryCommitAuthorisation>,
    authority: Arc<dyn RequestCommitAuthority>,
    request: RequestContext,
}

impl RequestCommitAuthorisation {
    pub fn new(
        lease: Arc<dyn InventoryCommitAuthorisation>,
        authority: Arc<dyn RequestCommitAuthority>,
        request: RequestContext,
    ) -> Self {
        Self {
            lease,
            authority,
            request,
        }
    }
}

impl InventoryCommitAuthorisation for RequestCommitAuthorisation {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            self.lease.ensure_owned().await?;
            self.authority.ensure_commit_allowed(&self.request).await
        })
    }

    fn ensure_reference_access(
        &self,
        id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            self.authority
                .ensure_reference_read(&self.request, id, &attributes)
                .await
        })
    }

    fn ensure_mutation_access(
        &self,
        action: Action,
        id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            self.authority
                .ensure_mutation_allowed(&self.request, action, id, &attributes)
                .await
        })
    }
}

/// Fresh request authority backed by the canonical access-control owners.
pub struct PolicyRequestCommitAuthority<U, R, P> {
    policy: Arc<PolicyEvaluationService<U, R, P>>,
    tokens: Arc<UserApiTokenService>,
    users: Arc<dyn UserRepository>,
}

impl<U, R, P> PolicyRequestCommitAuthority<U, R, P>
where
    U: UserRepository,
    R: RoleRepository,
    P: PolicyRepository,
{
    pub fn new(
        policy: Arc<PolicyEvaluationService<U, R, P>>,
        tokens: Arc<UserApiTokenService>,
        users: Arc<dyn UserRepository>,
    ) -> Self {
        Self {
            policy,
            tokens,
            users,
        }
    }

    /// Rebuild the request context from current durable state.
    ///
    /// The request's own snapshot is kept for subject and correlation metadata
    /// only; the ceilings that decide the boundary are re-read.
    async fn current_request(&self, request: &RequestContext) -> Result<RequestContext, AppError> {
        let user_id = request
            .user_id
            .ok_or_else(|| unrepresentable("no_principal"))?;
        let mut current = request.clone();
        match request.auth_method.as_str() {
            "api_token" => {
                let token_id = request
                    .api_token_id
                    .as_deref()
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| unrepresentable("token_unrepresentable"))?;
                // The token service re-checks status, expiry, the owning user
                // and the current scopes. Anything it refuses stays refused.
                let token = self
                    .tokens
                    .ensure_current_by_id(token_id, user_id, Utc::now())
                    .await?;
                current.api_token_scopes = token
                    .scopes
                    .iter()
                    .map(ApiTokenScopeContext::from)
                    .collect();
                // A one-request resource override belongs to the handler that
                // produced it. A deferred boundary derives its ceiling from the
                // token's current scopes alone.
                current.api_token_scope_resource_override = None;
            }
            "session" | "oidc" => {
                let user = self
                    .users
                    .get_by_id(user_id)
                    .await?
                    .ok_or_else(|| unrepresentable("principal_inactive"))?;
                if !user.is_active() {
                    return Err(unrepresentable("principal_inactive"));
                }
            }
            // Job tokens, anonymous requests and any future method have no
            // representable request principal for a deferred inventory write.
            _ => return Err(unrepresentable("principal_unrepresentable")),
        }
        Ok(current)
    }
}

impl<U, R, P> RequestCommitAuthority for PolicyRequestCommitAuthority<U, R, P>
where
    U: UserRepository,
    R: RoleRepository,
    P: PolicyRepository,
{
    fn ensure_commit_allowed<'a>(
        &'a self,
        request: &'a RequestContext,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            let current = self.current_request(request).await?;
            // The same decision the route made before dispatching the job, now
            // against the principal's current grants.
            self.policy
                .ensure_access_with_attributes(
                    &current,
                    Action::Execute,
                    ResourceType::Inventory,
                    None,
                    None,
                )
                .await
        })
    }

    fn ensure_reference_read<'a>(
        &'a self,
        request: &'a RequestContext,
        target: Uuid,
        attributes: &'a Value,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            self.ensure_object_access(request, Action::Read, target, attributes)
                .await
        })
    }

    fn ensure_mutation_allowed<'a>(
        &'a self,
        request: &'a RequestContext,
        action: Action,
        target: Uuid,
        attributes: &'a Value,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            // The port represents object-level inventory writes only. Anything
            // else is a refusal, never a downgrade to a READ or to the
            // operation-level grant that dispatched the job.
            if !matches!(action, Action::Create | Action::Update) {
                return Err(unrepresentable("unsupported_mutation_action"));
            }
            self.ensure_object_access(request, action, target, attributes)
                .await
        })
    }
}

impl<U, R, P> PolicyRequestCommitAuthority<U, R, P>
where
    U: UserRepository,
    R: RoleRepository,
    P: PolicyRepository,
{
    /// The one object-level inventory decision.
    ///
    /// READ and CREATE/UPDATE share this path so they use the same
    /// `inventory:<id>` resource id, the same resolved-attribute merge and the
    /// same policy/token interpreter as the native inventory routes. Only the
    /// action and the caller's choice of attributes differ.
    async fn ensure_object_access(
        &self,
        request: &RequestContext,
        action: Action,
        target: Uuid,
        attributes: &Value,
    ) -> Result<(), AppError> {
        let current = self.current_request(request).await?;
        let attributes: HashMap<String, Value> = serde_json::from_value(attributes.clone())?;
        let resource_id = format!("inventory:{target}");
        self.policy
            .ensure_access_with_attributes(
                &current,
                action,
                ResourceType::Inventory,
                Some(&resource_id),
                Some(attributes),
            )
            .await
    }
}

/// A boundary state this owner cannot represent is a refusal, never a fallback
/// to the decision the earlier request made.
fn unrepresentable(reason: &str) -> AppError {
    AppError::Forbidden(format!("deferred_authorisation:{reason}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::contexts::shared::request_context::RequestAuth;

    /// A lease that either holds or reports the canonical lost-ownership error.
    struct FakeLease {
        owned: bool,
        renewals: AtomicUsize,
    }

    impl InventoryCommitAuthorisation for FakeLease {
        fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
            self.renewals.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if self.owned {
                    Ok(())
                } else {
                    Err(AppError::Conflict("Sync lease ownership was lost".into()))
                }
            })
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

    /// A request authority whose ceilings are switched independently.
    struct FakeAuthority {
        commit_allowed: bool,
        reference_denied_id: Option<Uuid>,
        mutation_denied_id: Option<Uuid>,
        reference_calls: AtomicUsize,
        mutation_calls: AtomicUsize,
    }

    impl RequestCommitAuthority for FakeAuthority {
        fn ensure_commit_allowed<'a>(
            &'a self,
            _request: &'a RequestContext,
        ) -> BoxFuture<'a, Result<(), AppError>> {
            Box::pin(async move {
                if self.commit_allowed {
                    Ok(())
                } else {
                    Err(AppError::Forbidden("Access denied".into()))
                }
            })
        }

        fn ensure_reference_read<'a>(
            &'a self,
            _request: &'a RequestContext,
            target: Uuid,
            _attributes: &'a Value,
        ) -> BoxFuture<'a, Result<(), AppError>> {
            self.reference_calls.fetch_add(1, Ordering::SeqCst);
            let denied = self.reference_denied_id == Some(target);
            Box::pin(async move {
                if denied {
                    Err(AppError::Forbidden("Access denied".into()))
                } else {
                    Ok(())
                }
            })
        }

        fn ensure_mutation_allowed<'a>(
            &'a self,
            _request: &'a RequestContext,
            _action: Action,
            target: Uuid,
            _attributes: &'a Value,
        ) -> BoxFuture<'a, Result<(), AppError>> {
            self.mutation_calls.fetch_add(1, Ordering::SeqCst);
            let denied = self.mutation_denied_id == Some(target);
            Box::pin(async move {
                if denied {
                    Err(AppError::Forbidden("Access denied".into()))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn request() -> RequestContext {
        RequestContext::new(
            Some(Uuid::now_v7()),
            "session".to_string(),
            None,
            Uuid::now_v7().to_string(),
            RequestAuth::default(),
        )
    }

    fn authorisation(
        lease_owned: bool,
        commit_allowed: bool,
        reference_denied_id: Option<Uuid>,
        mutation_denied_id: Option<Uuid>,
    ) -> (
        Arc<FakeLease>,
        Arc<FakeAuthority>,
        RequestCommitAuthorisation,
    ) {
        let lease = Arc::new(FakeLease {
            owned: lease_owned,
            renewals: AtomicUsize::new(0),
        });
        let authority = Arc::new(FakeAuthority {
            commit_allowed,
            reference_denied_id,
            mutation_denied_id,
            reference_calls: AtomicUsize::new(0),
            mutation_calls: AtomicUsize::new(0),
        });
        let composed = RequestCommitAuthorisation::new(lease.clone(), authority.clone(), request());
        (lease, authority, composed)
    }

    #[tokio::test]
    async fn commit_boundary_requires_both_the_lease_and_the_request_authority() {
        let (_, _, held) = authorisation(true, true, None, None);
        held.ensure_owned().await.expect("both ceilings hold");

        let (_, _, lost_grant) = authorisation(true, false, None, None);
        lost_grant
            .ensure_owned()
            .await
            .expect_err("a request whose execute grant was lost must not commit");

        let (lease, authority, lost_lease) = authorisation(false, true, None, None);
        lost_lease
            .ensure_owned()
            .await
            .expect_err("a lost provider lease must not commit");
        assert_eq!(lease.renewals.load(Ordering::SeqCst), 1);
        assert_eq!(
            authority.reference_calls.load(Ordering::SeqCst),
            0,
            "a lost lease must not reach the request authority"
        );
    }

    #[tokio::test]
    async fn reference_access_is_a_request_read_not_a_lease_grant() {
        let target = Uuid::now_v7();
        let (lease, authority, allowed) = authorisation(true, true, None, None);
        allowed
            .ensure_reference_access(target, json!({"site_id": "site-a"}))
            .await
            .expect("a request with inventory read may reference the object");
        assert_eq!(authority.reference_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            lease.renewals.load(Ordering::SeqCst),
            0,
            "a reference check does not renew the lease"
        );

        let (_, _, denied) = authorisation(true, true, Some(target), None);
        denied
            .ensure_reference_access(target, json!({"site_id": "site-a"}))
            .await
            .expect_err("the object's own READ denial must refuse the reference");
    }

    #[tokio::test]
    async fn mutation_access_is_a_request_ceiling_not_a_lease_grant() {
        let target = Uuid::now_v7();
        let attributes = json!({
            "object_type": "dcim.consoleporttemplate",
            "device_type_id": "device-type-d",
        });

        let (_, authority, allowed) = authorisation(true, true, None, None);
        allowed
            .ensure_mutation_access(Action::Update, target, attributes.clone())
            .await
            .expect("a request with the object-level grant may write");
        assert_eq!(authority.mutation_calls.load(Ordering::SeqCst), 1);

        let (lease, _, denied) = authorisation(true, true, None, Some(target));
        denied
            .ensure_mutation_access(Action::Create, target, attributes.clone())
            .await
            .expect_err("a conditional object deny must refuse the write");
        assert_eq!(
            lease.renewals.load(Ordering::SeqCst),
            0,
            "a mutation check does not renew the lease"
        );

        let lease_only = FakeLease {
            owned: true,
            renewals: AtomicUsize::new(0),
        };
        lease_only
            .ensure_mutation_access(Action::Update, target, attributes)
            .await
            .expect_err("lease ownership alone is not a mutation grant");
    }
}
