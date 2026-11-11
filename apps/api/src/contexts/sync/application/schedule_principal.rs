use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contexts::shared::request_context::{RequestAuth, RequestContext};
use crate::errors::AppError;

/// Durable identity for a deferred inventory pull. No credential or scope snapshot is stored.
/// The reconstructed request must still be checked by `PolicyRequestCommitAuthority`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchedulePrincipal {
    Session { user_id: Uuid },
    Oidc { user_id: Uuid },
    ApiToken { user_id: Uuid, token_id: Uuid },
}

impl SchedulePrincipal {
    /// Capture only identities which the deferred commit authority can revalidate.
    ///
    /// # Errors
    /// Rejects requests without a supported user or personal-token identity.
    pub fn from_request(request: &RequestContext) -> Result<Self, AppError> {
        if request.playbook_job_binding.is_some() {
            return Err(unrepresentable());
        }
        let user_id = request.user_id.ok_or_else(unrepresentable)?;
        match request.auth_method.as_str() {
            "session" if request.api_token_id.is_none() => Ok(Self::Session { user_id }),
            "oidc" if request.api_token_id.is_none() => Ok(Self::Oidc { user_id }),
            "api_token" => {
                let token_id = request
                    .api_token_id
                    .as_deref()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .ok_or_else(unrepresentable)?;
                Ok(Self::ApiToken { user_id, token_id })
            }
            _ => Err(unrepresentable()),
        }
    }

    /// Reconstitute identity only; scopes and one-request overrides are never carried forward.
    pub fn request_context(&self, correlation_id: String) -> RequestContext {
        let (user_id, method, token_id) = match self {
            Self::Session { user_id } => (*user_id, "session", None),
            Self::Oidc { user_id } => (*user_id, "oidc", None),
            Self::ApiToken { user_id, token_id } => (*user_id, "api_token", Some(*token_id)),
        };
        let mut request = RequestContext::new(
            Some(user_id),
            method.to_string(),
            None,
            correlation_id,
            RequestAuth::default(),
        );
        request.api_token_id = token_id.map(|id| id.to_string());
        request
    }
}

fn unrepresentable() -> AppError {
    AppError::Forbidden("Schedule requires a representable user principal".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principal_round_trip_never_carries_request_authority_snapshot() {
        let user_id = Uuid::now_v7();
        let token_id = Uuid::now_v7();
        let principal = SchedulePrincipal::ApiToken { user_id, token_id };
        let encoded = serde_json::to_string(&principal).unwrap();
        let decoded: SchedulePrincipal = serde_json::from_str(&encoded).unwrap();
        let request = decoded.request_context("scheduled".into());
        assert_eq!(request.user_id, Some(user_id));
        assert_eq!(request.auth_method, "api_token");
        assert_eq!(
            request.api_token_id.as_deref(),
            Some(token_id.to_string().as_str())
        );
        assert!(request.api_token_scopes.is_empty());
        assert!(request.api_token_scope_resource_override.is_none());
        assert_eq!(
            SchedulePrincipal::from_request(&request).unwrap(),
            principal
        );
    }

    #[test]
    fn principal_rejects_unrepresentable_and_inconsistent_authentication() {
        let user_id = Uuid::now_v7();
        let mut request = RequestContext::new(
            Some(user_id),
            "playbook_job_token".into(),
            None,
            "request".into(),
            RequestAuth::default(),
        );
        assert!(SchedulePrincipal::from_request(&request).is_err());
        request.auth_method = "api_token".into();
        assert!(SchedulePrincipal::from_request(&request).is_err());
        request.api_token_id = Some("not-a-uuid".into());
        assert!(SchedulePrincipal::from_request(&request).is_err());
        request.auth_method = "session".into();
        assert!(SchedulePrincipal::from_request(&request).is_err());
        request.api_token_id = None;
        assert_eq!(
            SchedulePrincipal::from_request(&request).unwrap(),
            SchedulePrincipal::Session { user_id }
        );
        request.user_id = None;
        assert!(SchedulePrincipal::from_request(&request).is_err());
        assert!(serde_json::from_str::<SchedulePrincipal>(r#"{"method":"session","user_id":"00000000-0000-0000-0000-000000000000","token":"secret"}"#).is_err());
    }
}
