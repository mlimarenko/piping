use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::access_control::application::AccessControlService;
use crate::contexts::access_control::domain::PolicyDecision;
use crate::contexts::access_control::infrastructure::persistence::{
    PolicyRepository, RoleRepository, UserRepository,
};
use crate::contexts::audit::application::AuditRecorder;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::secrets::application::SecretStore;
use crate::contexts::secrets::domain::{SecretBinding, SecretVariable};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

const MASKED_SECRET_VALUE: &str = "********";

pub trait SecretVisibilityChecker: Send + Sync {
    fn check_secret_visibility<'a>(
        &'a self,
        request_context: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<PolicyDecision, AppError>> + Send + 'a>>;
}

impl<U, R, P> SecretVisibilityChecker for AccessControlService<U, R, P>
where
    U: UserRepository + Send + Sync,
    R: RoleRepository + Send + Sync,
    P: PolicyRepository + Send + Sync,
{
    fn check_secret_visibility<'a>(
        &'a self,
        request_context: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<PolicyDecision, AppError>> + Send + 'a>> {
        Box::pin(async move { self.check_secret_visibility(request_context).await })
    }
}

/// Resolve secret variables with inheritance and overrides
pub async fn resolve_secret_visibility(
    access_control: Option<&dyn SecretVisibilityChecker>,
    request_context: Option<&RequestContext>,
) -> Result<bool, AppError> {
    let can_view_secrets = match (access_control, request_context) {
        (Some(access_control), Some(request_context)) => {
            matches!(
                access_control
                    .check_secret_visibility(request_context)
                    .await?,
                PolicyDecision::Allow
            )
        }
        _ => false,
    };
    Ok(can_view_secrets)
}

/// Request-scoped inputs for [`resolve_secrets`]: how secret visibility is
/// checked, which audit recorder sees the access, and the acting identity.
pub struct SecretResolutionContext<'a> {
    pub access_control: Option<&'a dyn SecretVisibilityChecker>,
    pub request_context: Option<&'a RequestContext>,
    pub audit: Option<&'a dyn AuditRecorder>,
    pub actor_id: Option<Uuid>,
    pub can_view_override: Option<bool>,
}

pub async fn resolve_secrets(
    variables: &[SecretVariable],
    bindings: &[SecretBinding],
    secret_store: Option<&dyn SecretStore>,
    context: SecretResolutionContext<'_>,
) -> Result<Value, AppError> {
    let SecretResolutionContext {
        access_control,
        request_context,
        audit,
        actor_id,
        can_view_override,
    } = context;
    let mut binding_map: HashMap<Uuid, Vec<&SecretBinding>> = HashMap::new();
    for binding in bindings {
        binding_map
            .entry(binding.secret_id)
            .or_default()
            .push(binding);
    }

    let mut candidates = Vec::new();
    for (index, variable) in variables.iter().enumerate() {
        match binding_map.get(&variable.id) {
            Some(bindings) => {
                for binding in bindings {
                    candidates.push(SecretCandidate {
                        variable,
                        scope_type: &binding.scope_type,
                        weight: binding.weight,
                        binding_id: Some(binding.id),
                        index,
                    });
                }
            }
            None => {
                candidates.push(SecretCandidate {
                    variable,
                    scope_type: "global",
                    weight: 0,
                    binding_id: None,
                    index,
                });
            }
        }
    }

    candidates.sort_by(|a, b| {
        a.weight
            .cmp(&b.weight)
            .then(scope_precedence(a.scope_type).cmp(&scope_precedence(b.scope_type)))
            .then(a.binding_id.cmp(&b.binding_id))
            .then(a.index.cmp(&b.index))
            .then(a.variable.id.cmp(&b.variable.id))
    });

    let can_view_secrets = match can_view_override {
        Some(value) => value,
        None => resolve_secret_visibility(access_control, request_context).await?,
    };

    if !can_view_secrets {
        return Ok(Value::Object(serde_json::Map::new()));
    }

    let mut map = serde_json::Map::new();
    let mut keys = Vec::new();
    let should_audit = secret_store.is_some();
    for candidate in candidates {
        let key = candidate.variable.key.clone();
        if !map.contains_key(&key) {
            keys.push(key.clone());
        }

        let value = resolve_secret_value(candidate.variable, secret_store).await?;
        map.insert(key, value);
    }

    if should_audit && let Some(audit_recorder) = audit {
        let event = AuditEvent {
            id: Uuid::now_v7(),
            event_type: "secrets.resolve".to_string(),
            actor_id,
            actor_kind: request_context
                .map_or("user", RequestContext::audit_actor_kind)
                .to_string(),
            target_type: Some("secret_variable".to_string()),
            target_id: None,
            metadata: serde_json::json!({ "keys": keys }),
            created_at: Utc::now(),
        };
        audit_recorder.record_event(event).await?;
    }

    Ok(Value::Object(map))
}

#[derive(Debug, Clone)]
struct SecretCandidate<'a> {
    variable: &'a SecretVariable,
    scope_type: &'a str,
    weight: i32,
    binding_id: Option<Uuid>,
    index: usize,
}

fn scope_precedence(scope_type: &str) -> u8 {
    match scope_type {
        "global" => 0,
        "tenant_group" => 1,
        "tenant" => 2,
        "site" => 3,
        "connection_target" => 4,
        _ => 0,
    }
}

async fn resolve_secret_value(
    variable: &SecretVariable,
    secret_store: Option<&dyn SecretStore>,
) -> Result<Value, AppError> {
    if variable.is_protected {
        return Ok(Value::Null);
    }
    if variable.is_masked {
        return Ok(Value::String(MASKED_SECRET_VALUE.to_string()));
    }
    if let Some(secret_store) = secret_store {
        match secret_store.get_secret(variable.id).await {
            Ok(value) => Ok(Value::String(value)),
            Err(AppError::MissingSecret(_)) => Ok(Value::Null),
            Err(error) => Err(error),
        }
    } else {
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::shared::request_context::{RequestAuth, RequestContext};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct StubVisibility {
        decision: PolicyDecision,
    }

    impl SecretVisibilityChecker for StubVisibility {
        fn check_secret_visibility<'a>(
            &'a self,
            _request_context: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<PolicyDecision, AppError>> + Send + 'a>> {
            Box::pin(async move { Ok(self.decision) })
        }
    }

    fn test_request_context() -> RequestContext {
        RequestContext::new(
            Some(Uuid::nil()),
            "session".to_string(),
            None,
            "corr".to_string(),
            RequestAuth {
                oidc_groups: Vec::new(),
                api_token_scopes: Vec::new(),
                api_token_scope_resource_override: None,
            },
        )
    }

    struct StubSecretStore {
        values_by_id: Mutex<HashMap<Uuid, String>>,
    }

    impl StubSecretStore {
        fn new(values_by_id: HashMap<Uuid, String>) -> Self {
            Self {
                values_by_id: Mutex::new(values_by_id),
            }
        }
    }

    #[async_trait]
    impl SecretStore for StubSecretStore {
        async fn get_secret(&self, secret_id: Uuid) -> Result<String, AppError> {
            let values = self
                .values_by_id
                .lock()
                .map_err(|_| AppError::Internal("Failed to lock secret store".to_string()))?;
            values
                .get(&secret_id)
                .cloned()
                .ok_or_else(|| AppError::MissingSecret(format!("Secret not found: {}", secret_id)))
        }

        async fn set_secret(&self, _secret_id: Uuid, _value: &str) -> Result<(), AppError> {
            Ok(())
        }

        async fn delete_secret(&self, _secret_id: Uuid) -> Result<(), AppError> {
            Ok(())
        }

        async fn ensure_unique_key(
            &self,
            _scope_type: &str,
            _scope_id: Uuid,
            _key: &str,
        ) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn resolve_secrets_prefers_more_specific_scope() {
        let tenant_var = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: false,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let host_var = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: false,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let store = StubSecretStore::new(HashMap::from([
            (tenant_var.id, "tenant-secret".to_string()),
            (host_var.id, "host-secret".to_string()),
        ]));
        let bindings = vec![
            SecretBinding {
                id: Uuid::now_v7(),
                secret_id: tenant_var.id,
                scope_type: "tenant".to_string(),
                scope_id: Uuid::now_v7(),
                weight: 0,
                managed_by_field_usage: false,
            },
            SecretBinding {
                id: Uuid::now_v7(),
                secret_id: host_var.id,
                scope_type: "connection_target".to_string(),
                scope_id: Uuid::now_v7(),
                weight: 0,
                managed_by_field_usage: false,
            },
        ];
        let visibility = StubVisibility {
            decision: PolicyDecision::Allow,
        };
        let resolved = resolve_secrets(
            &[tenant_var, host_var],
            &bindings,
            Some(&store),
            SecretResolutionContext {
                access_control: Some(&visibility),
                request_context: Some(&test_request_context()),
                audit: None,
                actor_id: None,
                can_view_override: None,
            },
        )
        .await
        .expect("resolve secrets");
        let value = resolved
            .get("TOKEN")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        assert_eq!(value, "host-secret");
    }

    #[tokio::test]
    async fn resolve_secrets_prefers_higher_weight() {
        let low_var = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: false,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let high_var = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: false,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let store = StubSecretStore::new(HashMap::from([
            (low_var.id, "low-secret".to_string()),
            (high_var.id, "high-secret".to_string()),
        ]));
        let scope_id = Uuid::now_v7();
        let bindings = vec![
            SecretBinding {
                id: Uuid::now_v7(),
                secret_id: low_var.id,
                scope_type: "connection_target".to_string(),
                scope_id,
                weight: 0,
                managed_by_field_usage: false,
            },
            SecretBinding {
                id: Uuid::now_v7(),
                secret_id: high_var.id,
                scope_type: "connection_target".to_string(),
                scope_id,
                weight: 10,
                managed_by_field_usage: false,
            },
        ];
        let visibility = StubVisibility {
            decision: PolicyDecision::Allow,
        };
        let resolved = resolve_secrets(
            &[low_var, high_var],
            &bindings,
            Some(&store),
            SecretResolutionContext {
                access_control: Some(&visibility),
                request_context: Some(&test_request_context()),
                audit: None,
                actor_id: None,
                can_view_override: None,
            },
        )
        .await
        .expect("resolve secrets");
        let value = resolved
            .get("TOKEN")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        assert_eq!(value, "high-secret");
    }

    #[tokio::test]
    async fn resolve_secrets_denied_returns_empty() {
        let secret = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: true,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let visibility = StubVisibility {
            decision: PolicyDecision::Deny,
        };
        let resolved = resolve_secrets(
            &[secret],
            &[],
            None,
            SecretResolutionContext {
                access_control: Some(&visibility),
                request_context: Some(&test_request_context()),
                audit: None,
                actor_id: None,
                can_view_override: None,
            },
        )
        .await
        .expect("resolve secrets");
        assert!(
            resolved
                .as_object()
                .map(|map| map.is_empty())
                .unwrap_or(false),
            "denied should return empty object"
        );
    }

    #[tokio::test]
    async fn resolve_secrets_without_visibility_returns_empty() {
        let secret = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: false,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let store = StubSecretStore::new(HashMap::from([(secret.id, "real-secret".to_string())]));
        let resolved = resolve_secrets(
            &[secret],
            &[],
            Some(&store),
            SecretResolutionContext {
                access_control: None,
                request_context: None,
                audit: None,
                actor_id: None,
                can_view_override: None,
            },
        )
        .await
        .expect("resolve secrets");
        assert!(
            resolved
                .as_object()
                .map(|map| map.is_empty())
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn resolve_secrets_returns_masked_value() {
        let secret = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: true,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let store = StubSecretStore::new(HashMap::from([(secret.id, "real-secret".to_string())]));
        let visibility = StubVisibility {
            decision: PolicyDecision::Allow,
        };
        let resolved = resolve_secrets(
            &[secret],
            &[],
            Some(&store),
            SecretResolutionContext {
                access_control: Some(&visibility),
                request_context: Some(&test_request_context()),
                audit: None,
                actor_id: None,
                can_view_override: None,
            },
        )
        .await
        .expect("resolve secrets");
        let value = resolved
            .get("TOKEN")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        assert_eq!(value, MASKED_SECRET_VALUE);
    }

    #[tokio::test]
    async fn resolve_secrets_returns_null_for_protected() {
        let secret = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: false,
            is_protected: true,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let store = StubSecretStore::new(HashMap::from([(secret.id, "real-secret".to_string())]));
        let visibility = StubVisibility {
            decision: PolicyDecision::Allow,
        };
        let resolved = resolve_secrets(
            &[secret],
            &[],
            Some(&store),
            SecretResolutionContext {
                access_control: Some(&visibility),
                request_context: Some(&test_request_context()),
                audit: None,
                actor_id: None,
                can_view_override: None,
            },
        )
        .await
        .expect("resolve secrets");
        assert!(
            resolved
                .get("TOKEN")
                .map(|value| value.is_null())
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn resolve_secrets_returns_secret_value() {
        let secret = SecretVariable {
            id: Uuid::now_v7(),
            key: "TOKEN".to_string(),
            is_masked: false,
            is_protected: false,
            kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
            description: None,
            retire_with_field_usage: false,
        };
        let store = StubSecretStore::new(HashMap::from([(secret.id, "real-secret".to_string())]));
        let visibility = StubVisibility {
            decision: PolicyDecision::Allow,
        };
        let resolved = resolve_secrets(
            &[secret],
            &[],
            Some(&store),
            SecretResolutionContext {
                access_control: Some(&visibility),
                request_context: Some(&test_request_context()),
                audit: None,
                actor_id: None,
                can_view_override: None,
            },
        )
        .await
        .expect("resolve secrets");
        let value = resolved
            .get("TOKEN")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        assert_eq!(value, "real-secret");
    }
}
