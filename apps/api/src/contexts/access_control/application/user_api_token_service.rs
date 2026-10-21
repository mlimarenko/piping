use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use openssl::{rand::rand_bytes, sha::sha256};
use serde_json::json;
use uuid::Uuid;

use std::sync::LazyLock;

use crate::config::ApiTokenConfig;
use crate::contexts::access_control::domain::vocabulary::pat_grantable_pairs;
use crate::contexts::access_control::domain::{
    Action, ResourceType, UserApiToken, UserApiTokenScope, UserApiTokenStatus,
};
use crate::contexts::access_control::infrastructure::persistence::{
    UserApiTokenListFilter, UserApiTokenRepository, UserRepository,
};
use crate::contexts::audit::application::AuditPostCommit;
use crate::contexts::audit::domain::AuditEvent;
use crate::errors::AppError;

const USER_API_TOKEN_PREFIX: &str = "pat";

#[derive(Debug, Clone)]
pub struct CreateUserApiTokenInput {
    pub name: String,
    pub scopes: Vec<UserApiTokenScope>,
    pub expires_in_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct CreatedUserApiToken {
    pub token: UserApiToken,
    pub raw_token: String,
}

#[derive(Debug, Clone)]
pub struct ValidatedUserApiToken {
    pub token_id: Uuid,
    pub user_id: Uuid,
    pub scopes: Vec<UserApiTokenScope>,
}

/// The human-PAT grantable scope catalog. Derived from the access-control
/// vocabulary registry (`domain::vocabulary`) — the former hand-maintained
/// 107-pair literal is gone; a resource's grantability now lives in exactly one
/// place. Computed once and cached: token creation validates every requested
/// scope against this list, so it must stay a stable `&'static` slice.
pub fn user_api_token_scope_catalog_pairs() -> &'static [(Action, ResourceType)] {
    static CATALOG: LazyLock<Vec<(Action, ResourceType)>> = LazyLock::new(pat_grantable_pairs);
    CATALOG.as_slice()
}

#[derive(Clone)]
pub struct UserApiTokenService {
    token_repository: Arc<dyn UserApiTokenRepository>,
    user_repository: Arc<dyn UserRepository>,
    config: ApiTokenConfig,
    audit_post_commit: Option<Arc<dyn AuditPostCommit>>,
}

impl UserApiTokenService {
    pub fn new(
        token_repository: Arc<dyn UserApiTokenRepository>,
        user_repository: Arc<dyn UserRepository>,
        config: ApiTokenConfig,
        audit_post_commit: Option<Arc<dyn AuditPostCommit>>,
    ) -> Self {
        Self {
            token_repository,
            user_repository,
            config,
            audit_post_commit,
        }
    }

    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        limit: u32,
        offset: u32,
        filter: UserApiTokenListFilter,
    ) -> Result<(Vec<UserApiToken>, u64), AppError> {
        let items = self
            .token_repository
            .list_user_tokens_page(user_id, limit, offset, filter.clone())
            .await?;
        let total = self
            .token_repository
            .count_user_tokens(user_id, filter)
            .await?;
        Ok((items, total))
    }

    pub fn scope_catalog(&self) -> Vec<UserApiTokenScope> {
        user_api_token_scope_catalog_pairs()
            .iter()
            .map(|(action, resource_type)| UserApiTokenScope {
                action: action.clone(),
                resource_type: resource_type.clone(),
                resource_pattern: "*".to_string(),
            })
            .collect()
    }

    pub async fn create_for_user(
        &self,
        user_id: Uuid,
        input: CreateUserApiTokenInput,
        now: DateTime<Utc>,
    ) -> Result<CreatedUserApiToken, AppError> {
        let name = input.name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::Validation("Token name is required".to_string()));
        }
        if input.scopes.is_empty() {
            return Err(AppError::Validation(
                "At least one token scope is required".to_string(),
            ));
        }
        validate_user_api_token_scope_catalog(input.scopes.as_slice())?;

        let active_count = self
            .token_repository
            .count_active_tokens_for_user(user_id, now)
            .await?;
        if active_count >= self.config.max_active_per_user {
            return Err(AppError::Validation("TOKEN_LIMIT_EXCEEDED".to_string()));
        }

        let expires_at = self.resolve_expires_at(input.expires_in_seconds, now)?;

        let token_id = Uuid::now_v7();
        let secret = generate_token_secret(self.config.secret_bytes as usize)?;
        let raw_token = format!("{USER_API_TOKEN_PREFIX}_{token_id}_{secret}");
        let token_hash = hash_token(&raw_token);
        let token_prefix = format!(
            "{USER_API_TOKEN_PREFIX}_{}",
            secret.chars().take(8).collect::<String>()
        );

        let scopes = input
            .scopes
            .into_iter()
            .map(|scope| UserApiTokenScope {
                action: scope.action,
                resource_type: scope.resource_type,
                resource_pattern: normalize_resource_pattern(scope.resource_pattern.as_str()),
            })
            .collect();

        let token = UserApiToken {
            id: token_id,
            user_id,
            name,
            token_prefix,
            token_hash,
            status: UserApiTokenStatus::Active,
            expires_at,
            last_used_at: None,
            created_at: now,
            updated_at: now,
            revoked_at: None,
            revoked_by_user_id: None,
            scopes,
        };

        let audit_event = self.audit_event(
            "access_control.user_api_token.created",
            Some(user_id),
            Some(token.id),
            json!({
                "token_name": token.name.clone(),
                "token_prefix": token.token_prefix.clone(),
                "scope_count": token.scopes.len(),
                "expires_at": token.expires_at.map(|value| value.to_rfc3339()),
            }),
            now,
        );
        let token = self
            .token_repository
            .create_token(token, audit_event.clone())
            .await?;
        self.fanout_recorded_event(audit_event);
        Ok(CreatedUserApiToken { token, raw_token })
    }

    pub async fn revoke_for_user(
        &self,
        user_id: Uuid,
        token_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<UserApiToken, AppError> {
        let existing = self
            .token_repository
            .find_token_with_scopes(token_id)
            .await?
            .ok_or_else(|| AppError::NotFound("API token not found".to_string()))?;

        if existing.user_id != user_id {
            return Err(AppError::NotFound("API token not found".to_string()));
        }
        if existing.status == UserApiTokenStatus::Revoked {
            return Ok(existing);
        }

        let audit_event = self.audit_event(
            "access_control.user_api_token.revoked",
            Some(user_id),
            Some(existing.id),
            json!({
                "token_name": existing.name,
                "token_prefix": existing.token_prefix,
                "scope_count": existing.scopes.len(),
                "revoked_at": now.to_rfc3339(),
            }),
            now,
        );
        let revoked = self
            .token_repository
            .revoke_user_token(user_id, token_id, now, audit_event.clone())
            .await?
            .ok_or_else(|| AppError::NotFound("API token not found".to_string()))?;
        self.fanout_recorded_event(audit_event);
        Ok(revoked)
    }

    pub async fn validate_bearer_token(
        &self,
        raw_token: &str,
        now: DateTime<Utc>,
    ) -> Result<ValidatedUserApiToken, AppError> {
        let (token_id, _) = parse_raw_token(raw_token)
            .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;

        let token = self
            .token_repository
            .find_token_with_scopes(token_id)
            .await?
            .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;

        if token.status != UserApiTokenStatus::Active {
            return Err(AppError::Auth("TOKEN_REVOKED".to_string()));
        }
        if token.is_expired(now) {
            return Err(AppError::Auth("TOKEN_EXPIRED".to_string()));
        }

        if hash_token(raw_token) != token.token_hash {
            return Err(AppError::Auth("TOKEN_INVALID".to_string()));
        }

        let user = self
            .user_repository
            .get_by_id(token.user_id)
            .await?
            .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
        if user.status != "active" {
            return Err(AppError::Auth("TOKEN_INVALID".to_string()));
        }

        self.token_repository.touch_last_used(token.id, now).await?;

        Ok(ValidatedUserApiToken {
            token_id: token.id,
            user_id: token.user_id,
            scopes: token.scopes,
        })
    }

    fn resolve_expires_at(
        &self,
        requested_expires_in: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, AppError> {
        let requested_expires_in = requested_expires_in.unwrap_or(self.config.default_ttl_seconds);
        if requested_expires_in == 0 {
            return Ok(None);
        }
        if requested_expires_in < self.config.min_ttl_seconds
            || requested_expires_in > self.config.max_ttl_seconds
        {
            return Err(AppError::Validation(
                "Token expiration is outside allowed policy bounds".to_string(),
            ));
        }
        now.checked_add_signed(Duration::seconds(requested_expires_in as i64))
            .map(Some)
            .ok_or_else(|| AppError::Validation("Invalid token expiration".to_string()))
    }
}

impl UserApiTokenService {
    fn audit_event(
        &self,
        event_type: &str,
        actor_id: Option<Uuid>,
        token_id: Option<Uuid>,
        metadata: serde_json::Value,
        now: DateTime<Utc>,
    ) -> AuditEvent {
        AuditEvent {
            id: Uuid::now_v7(),
            event_type: event_type.to_string(),
            actor_id,
            actor_kind: "user".to_string(),
            target_type: Some("user_api_token".to_string()),
            target_id: token_id,
            metadata,
            created_at: now,
        }
    }

    fn fanout_recorded_event(&self, event: AuditEvent) {
        if let Some(audit_post_commit) = self.audit_post_commit.as_ref() {
            audit_post_commit.fanout_recorded_event(event);
        }
    }
}

fn generate_token_secret(size: usize) -> Result<String, AppError> {
    let mut bytes = vec![0_u8; size];
    rand_bytes(&mut bytes)
        .map_err(|error| AppError::Internal(format!("Failed to generate token bytes: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn hash_token(value: &str) -> String {
    hex_encode(&sha256(value.as_bytes()))
}

fn parse_raw_token(value: &str) -> Option<(Uuid, &str)> {
    if value.trim() != value {
        return None;
    }
    let mut parts = value.splitn(3, '_');
    if parts.next()? != USER_API_TOKEN_PREFIX {
        return None;
    }
    let token_id = Uuid::parse_str(parts.next()?).ok()?;
    let secret = parts.next()?;
    if !is_url_safe_secret(secret) {
        return None;
    }
    Some((token_id, secret))
}

fn validate_user_api_token_scope_catalog(scopes: &[UserApiTokenScope]) -> Result<(), AppError> {
    for scope in scopes {
        if !is_user_api_token_scope_catalog_pair(&scope.action, &scope.resource_type) {
            return Err(AppError::Validation(format!(
                "Unsupported token scope: {}",
                scope.wire_value()
            )));
        }
    }
    Ok(())
}

fn is_user_api_token_scope_catalog_pair(action: &Action, resource_type: &ResourceType) -> bool {
    user_api_token_scope_catalog_pairs()
        .iter()
        .any(|(catalog_action, catalog_resource_type)| {
            catalog_action == action && catalog_resource_type == resource_type
        })
}

fn is_url_safe_secret(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

fn normalize_resource_pattern(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "*".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::contexts::access_control::domain::{Action, ResourceType, UserApiTokenScope};

    use super::{
        USER_API_TOKEN_PREFIX, is_url_safe_secret, parse_raw_token,
        validate_user_api_token_scope_catalog,
    };

    #[test]
    fn raw_token_parser_accepts_only_pat_namespace_with_url_safe_secret() {
        let id = Uuid::from_u128(7);
        let raw = format!("{USER_API_TOKEN_PREFIX}_{id}_AbCdEf123_-");
        assert_eq!(parse_raw_token(&raw), Some((id, "AbCdEf123_-")));

        assert!(parse_raw_token(&format!("pjt_{id}_AbCdEf123_-")).is_none());
        assert!(parse_raw_token(&format!("{USER_API_TOKEN_PREFIX}_{id}_")).is_none());
        assert!(parse_raw_token(&format!("{USER_API_TOKEN_PREFIX}_{id}_has.dot")).is_none());
        assert!(parse_raw_token(&format!(" {USER_API_TOKEN_PREFIX}_{id}_AbCdEf123")).is_none());
    }

    #[test]
    fn token_secret_candidate_is_urlsafe_base64url_shape() {
        assert!(is_url_safe_secret("AbCdEf123_-"));
        assert!(!is_url_safe_secret(""));
        assert!(!is_url_safe_secret("contains/slash"));
        assert!(!is_url_safe_secret("contains.dot"));
        assert!(!is_url_safe_secret("contains space"));
    }

    #[test]
    fn pat_scope_creation_accepts_catalog_pairs_only() {
        let supported = UserApiTokenScope {
            action: Action::Read,
            resource_type: ResourceType::Inventory,
            resource_pattern: "*".to_string(),
        };
        assert!(validate_user_api_token_scope_catalog(&[supported]).is_ok());

        // Service principals are enforced via session RBAC but deliberately not
        // PAT-grantable (a token must not mint/reach machine identities), so a
        // requested scope over them is rejected.
        let session_only_resource = UserApiTokenScope {
            action: Action::Read,
            resource_type: ResourceType::PlaybookServicePrincipal,
            resource_pattern: "*".to_string(),
        };
        let error = validate_user_api_token_scope_catalog(&[session_only_resource])
            .expect_err("service principal resource must not be grantable by human PAT");
        assert!(
            error
                .to_string()
                .contains("Unsupported token scope: read:playbook_service_principal")
        );
    }
}
