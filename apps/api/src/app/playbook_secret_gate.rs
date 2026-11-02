//! Shared app-layer orchestrator for gate **C3** — the per-secret
//! `Execute × SecretVariable` authorization a playbook launch requires for every
//! secret the effective version exports into the runner env via
//! `token_policy.runtime.secrets.env`.
//!
//! There are exactly two callers, and they MUST reach the same verdict:
//! - a manual REST launch, driven by the live request context (and its PAT token
//!   scope ceiling);
//! - a scheduled fire, driven by the run group's service principal (no token, so
//!   the ceiling is always satisfied).
//!
//! Both resolve policies/constraints once and delegate the per-secret decision
//! to the pure [`secret_env_authorization_snapshot`] core, so enforcement,
//! preflight and the authorization snapshot share one implementation.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::contexts::access_control::domain::{Principal, ResourceType};
use crate::contexts::playbooks::application::secret_gate::{
    ResolvedDeclaredSecret, secret_env_authorization_snapshot, secret_use_pat_allows,
};
use crate::contexts::playbooks::application::{
    PlaybookScheduledRunSecretAuthorizer, principal_subject,
};
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use super::{AccessControlServiceType, SqlxSecretRepository};

/// Who is exercising C3. The request variant carries the live context (for the
/// PAT ceiling and request constraints); the service-principal variant carries
/// the already-validated [`Principal::ServicePrincipal`], never a bare UUID.
pub(crate) enum SecretGateCaller<'a> {
    Request {
        request_context: &'a RequestContext,
        subject: &'a str,
    },
    ServicePrincipal {
        principal: &'a Principal,
    },
}

/// Resolve every declared runtime secret and clear the per-secret C3 grant,
/// returning the server-authored authorization snapshot (or `None` when nothing
/// is declared).
///
/// Fail-closed: a declared key that no longer resolves is a validation error; a
/// protected secret is refused; a missing grant is forbidden.
pub(crate) async fn ensure_secret_use_grants(
    access_control: &AccessControlServiceType,
    secrets: &SqlxSecretRepository,
    caller: SecretGateCaller<'_>,
    declared: &BTreeMap<String, String>,
) -> Result<Option<Value>, AppError> {
    if declared.is_empty() {
        return Ok(None);
    }

    match caller {
        SecretGateCaller::Request {
            request_context,
            subject,
        } => {
            let policies = access_control
                .resolve_policies_for_request(request_context)
                .await?;
            let context = access_control.constraint_context_for_request(
                request_context,
                ResourceType::SecretVariable,
                None,
            )?;
            let resolved = resolve_declared_secrets(secrets, declared).await?;
            secret_env_authorization_snapshot(&policies, &context, subject, resolved, |candidate| {
                secret_use_pat_allows(request_context, candidate)
            })
        }
        SecretGateCaller::ServicePrincipal { principal } => {
            let policies = access_control
                .resolve_policies_for_principal(principal)
                .await?;
            let context = access_control.constraint_context_for_principal(
                principal,
                ResourceType::SecretVariable,
                None,
            )?;
            let resolved = resolve_declared_secrets(secrets, declared).await?;
            let subject = principal_subject(principal);
            secret_env_authorization_snapshot(
                &policies,
                &context,
                &subject,
                resolved,
                |_candidate| true,
            )
        }
    }
}

/// Load every declared secret into its pure gate input. Fail-closed: a key that
/// no longer resolves is a validation error.
async fn resolve_declared_secrets(
    secrets: &SqlxSecretRepository,
    declared: &BTreeMap<String, String>,
) -> Result<Vec<Result<ResolvedDeclaredSecret, AppError>>, AppError> {
    let mut resolved = Vec::with_capacity(declared.len());
    for (env_name, key) in declared {
        let entry = match secrets.get_variable_by_key(key).await {
            Ok(Some(secret)) => Ok(ResolvedDeclaredSecret {
                env_name: env_name.clone(),
                key: key.clone(),
                secret_id: secret.id,
                is_protected: secret.is_protected,
            }),
            Ok(None) => Err(AppError::Validation(format!(
                "Playbook declares runtime secret `{key}` (env {env_name}) that does not exist"
            ))),
            Err(error) => Err(error),
        };
        resolved.push(entry);
    }
    Ok(resolved)
}

/// The scheduled-run authoring seam: clears C3 for the run group's service
/// principal and authors the snapshot the run worker later re-checks.
pub(crate) struct PlaybookScheduledRunSecretGate {
    access_control: Arc<AccessControlServiceType>,
    secrets: Arc<SqlxSecretRepository>,
}

impl PlaybookScheduledRunSecretGate {
    pub(crate) fn new(
        access_control: Arc<AccessControlServiceType>,
        secrets: Arc<SqlxSecretRepository>,
    ) -> Self {
        Self {
            access_control,
            secrets,
        }
    }
}

#[async_trait]
impl PlaybookScheduledRunSecretAuthorizer for PlaybookScheduledRunSecretGate {
    async fn authorize_scheduled_run_secret_env(
        &self,
        principal: &Principal,
        declared: &BTreeMap<String, String>,
    ) -> Result<Option<Value>, AppError> {
        ensure_secret_use_grants(
            &self.access_control,
            &self.secrets,
            SecretGateCaller::ServicePrincipal { principal },
            declared,
        )
        .await
    }
}
