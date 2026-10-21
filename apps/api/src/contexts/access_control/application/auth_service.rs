use std::sync::Arc;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordVerifier},
};
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::password_policy::validate_local_password;
use crate::config::AuthConfig;
use crate::contexts::access_control::domain::{AuthSession, OidcProvisioningMode, User};
use crate::contexts::access_control::infrastructure::persistence::{
    AuthSessionRepository, UserCredentialRepository, UserRepository,
};
use crate::contexts::audit::domain::AuditEvent;
use crate::errors::AppError;

/// Authentication service handling user login, session management, and authentication flows.
///
/// Supports two authentication methods:
/// - Local authentication: username/email + password (Argon2 hashing)
/// - OIDC authentication: external identity provider via OpenID Connect
///
/// Sessions are stored in the database and validated on each request.
#[derive(Clone)]
pub struct AuthService {
    /// Repository for user data operations
    user_repository: Arc<dyn UserRepository>,
    /// Repository for user credentials (password hashes)
    credential_repository: Arc<dyn UserCredentialRepository>,
    /// Repository for authentication sessions
    session_repository: Arc<dyn AuthSessionRepository>,
    /// Authentication configuration (TTL, password requirements, etc.)
    config: AuthConfig,
}

impl AuthService {
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        credential_repository: Arc<dyn UserCredentialRepository>,
        session_repository: Arc<dyn AuthSessionRepository>,
        config: AuthConfig,
    ) -> Self {
        Self {
            user_repository,
            credential_repository,
            session_repository,
            config,
        }
    }

    /// Authenticates a user with local credentials (username/email + password).
    ///
    /// Process:
    /// 1. Validates that local auth is enabled
    /// 2. Checks password minimum length
    /// 3. Looks up user by username or email
    /// 4. Verifies user is active and has local auth source
    /// 5. Retrieves stored password hash
    /// 6. Verifies password using Argon2 (with pepper for additional security)
    /// 7. Returns the authenticated user; the caller opens the audited session
    ///
    /// Returns a generic "Invalid credentials" error to prevent user enumeration.
    pub async fn authenticate_local(
        &self,
        identifier: &str,
        password: &str,
    ) -> Result<User, AppError> {
        let identifier = identifier.trim();
        let password = password.trim();
        if !self.config.local_enabled {
            return Err(AppError::Validation(
                "Local authentication is disabled".to_string(),
            ));
        }
        if validate_local_password(password, self.config.password_min_length).is_err() {
            return Err(AppError::Auth("Invalid credentials".to_string()));
        }

        // Look up user by username or email (case-insensitive)
        let user = self
            .user_repository
            .get_by_username_or_email(identifier)
            .await?
            .ok_or_else(|| AppError::Auth("Invalid credentials".to_string()))?;

        // Verify user account is active
        if user.status != "active" {
            return Err(AppError::Auth("User is inactive".to_string()));
        }
        // Ensure user is configured for local authentication
        if user.auth_source != "local" {
            return Err(AppError::Auth("User is not a local account".to_string()));
        }

        self.verify_local_user_password(&user, password).await?;

        Ok(user)
    }

    pub async fn verify_local_password_for_user(
        &self,
        user_id: Uuid,
        password: &str,
    ) -> Result<User, AppError> {
        let user = self
            .user_repository
            .get_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
        if !self.config.local_enabled {
            return Err(AppError::Validation(
                "Local authentication is disabled".to_string(),
            ));
        }
        if user.status != "active" {
            return Err(AppError::Auth("User is inactive".to_string()));
        }
        if user.auth_source != "local" {
            return Err(AppError::Validation(
                "Password can only be changed for local users".to_string(),
            ));
        }
        self.verify_local_user_password(&user, password).await?;
        Ok(user)
    }

    /// Authenticates a user via OIDC (OpenID Connect) external identity provider.
    ///
    /// This method is called after OIDC token exchange and verification.
    /// The external_id is the subject (sub) claim from the OIDC ID token.
    ///
    /// Process:
    /// 1. Validates that OIDC auth is enabled
    /// 2. Looks up user by external ID (OIDC subject)
    /// 3. Verifies user is active and has OIDC auth source
    /// 4. Returns the authenticated user; the caller opens the audited session
    ///
    /// Note: OIDC token verification happens before this method is called.
    pub async fn authenticate_oidc(
        &self,
        external_id: &str,
        preferred_username: Option<&str>,
        email: Option<&str>,
        oidc_group_ids: &[String],
        provisioning_mode: OidcProvisioningMode,
    ) -> Result<User, AppError> {
        // Look up user by OIDC subject (external_id), optionally provisioning on first login.
        let user = self
            .resolve_or_provision_oidc_user(
                external_id,
                preferred_username,
                email,
                provisioning_mode,
            )
            .await?;

        // Verify user account is active
        if user.status != "active" {
            return Err(AppError::Auth("User is inactive".to_string()));
        }
        // Ensure user is configured for OIDC authentication
        if user.auth_source != "oidc" {
            return Err(AppError::Auth("User is not an OIDC account".to_string()));
        }

        // Materialize external group claims before issuing a session. Persisted
        // membership is the RBAC source of truth for both browser sessions and
        // personal API tokens; a sync failure must not create a logged-in user
        // with stale permissions.
        self.user_repository
            .sync_oidc_group_memberships(user.id, oidc_group_ids)
            .await?;

        Ok(user)
    }

    async fn resolve_or_provision_oidc_user(
        &self,
        external_id: &str,
        preferred_username: Option<&str>,
        email: Option<&str>,
        provisioning_mode: OidcProvisioningMode,
    ) -> Result<User, AppError> {
        if let Some(user) = self.user_repository.get_by_external_id(external_id).await? {
            return Ok(user);
        }

        if provisioning_mode == OidcProvisioningMode::Manual {
            return Err(AppError::Auth(
                "User is not registered for OIDC".to_string(),
            ));
        }

        let username_seed = preferred_username
            .and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .or_else(|| {
                email.and_then(|value| {
                    value
                        .split_once('@')
                        .map(|(local, _)| local)
                        .filter(|local| !local.trim().is_empty())
                })
            })
            .unwrap_or("oidc-user");
        let username_base = sanitize_username_seed(username_seed);
        let username = self.unique_username(&username_base).await?;

        let email_base = sanitize_email_seed(email, external_id);
        let user_email = self.unique_email(&email_base).await?;

        self.user_repository
            .create_user(User {
                id: Uuid::now_v7(),
                username,
                email: user_email,
                status: "active".to_string(),
                auth_source: "oidc".to_string(),
                external_id: Some(external_id.to_string()),
            })
            .await
    }

    async fn unique_username(&self, base: &str) -> Result<String, AppError> {
        let base = if base.is_empty() { "oidc-user" } else { base };
        for index in 0..100 {
            let candidate = if index == 0 {
                base.to_string()
            } else {
                format!("{}-{}", base, index + 1)
            };
            if self
                .user_repository
                .get_by_username_or_email(&candidate)
                .await?
                .is_none()
            {
                return Ok(candidate);
            }
        }
        Err(AppError::Conflict(
            "Failed to allocate unique OIDC username".to_string(),
        ))
    }

    async fn unique_email(&self, base: &str) -> Result<String, AppError> {
        let (local_raw, domain_raw) = base.split_once('@').unwrap_or((base, "oidc.local"));
        let local_base = if local_raw.trim().is_empty() {
            "oidc-user"
        } else {
            local_raw.trim()
        };
        let domain_base = if domain_raw.trim().is_empty() {
            "oidc.local"
        } else {
            domain_raw.trim()
        };

        for index in 0..100 {
            let candidate = if index == 0 {
                format!("{}@{}", local_base, domain_base)
            } else {
                format!("{}+{}@{}", local_base, index + 1, domain_base)
            };
            if self
                .user_repository
                .get_by_username_or_email(&candidate)
                .await?
                .is_none()
            {
                return Ok(candidate);
            }
        }

        Err(AppError::Conflict(
            "Failed to allocate unique OIDC email".to_string(),
        ))
    }

    /// Resolves a session ID to a user and session tuple.
    ///
    /// Used by middleware to validate session cookies on each request.
    ///
    /// Returns None if:
    /// - Session doesn't exist
    /// - Session is expired or revoked
    /// - User doesn't exist
    /// - User account is inactive
    ///
    /// Returns Some((user, session)) if all validations pass.
    pub async fn resolve_session(
        &self,
        session_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<(User, AuthSession)>, AppError> {
        // Retrieve session from database
        let session = match self.session_repository.get_session(session_id).await? {
            Some(session) => session,
            None => return Ok(None),
        };

        // Check if session is still active (not expired, not revoked)
        if !session.is_active(now) {
            return Ok(None);
        }

        // Retrieve user associated with the session
        let user = match self.user_repository.get_by_id(session.user_id).await? {
            Some(user) => user,
            None => return Ok(None),
        };

        // Verify user account is still active
        if user.status != "active" {
            return Ok(None);
        }

        Ok(Some((user, session)))
    }

    /// Refreshes a session: validates it and extends TTL if still active.
    ///
    /// Used by session-refresh endpoint for frontend 401 handling.
    /// For both local and OIDC auth: validates session, extends expires_at, returns user+session.
    pub async fn refresh_session(
        &self,
        session_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<(User, AuthSession)>, AppError> {
        let Some((user, session)) = self.resolve_session(session_id, now).await? else {
            return Ok(None);
        };

        let new_expires_at = now
            .checked_add_signed(Duration::seconds(self.config.session_ttl_seconds as i64))
            .ok_or_else(|| AppError::Validation("Invalid session TTL".to_string()))?;

        self.session_repository
            .extend_session(session_id, new_expires_at)
            .await?;

        let refreshed_session = AuthSession {
            id: session.id,
            user_id: session.user_id,
            expires_at: new_expires_at,
            revoked_at: session.revoked_at,
        };

        Ok(Some((user, refreshed_session)))
    }

    /// Logs out a user by revoking their session.
    ///
    /// The session is marked as revoked in the database, preventing further use.
    pub async fn logout(&self, session_id: Uuid, now: DateTime<Utc>) -> Result<(), AppError> {
        self.session_repository
            .revoke_session(session_id, now)
            .await
    }

    /// Creates a new authentication session and its mandatory audit event.
    ///
    /// The session ID is generated using UUID v7 (time-based UUID).
    /// Session expiration is calculated based on configured TTL. The repository
    /// commits the session and audit rows in one transaction.
    pub async fn open_session(
        &self,
        user_id: Uuid,
        audit_event: AuditEvent,
    ) -> Result<AuthSession, AppError> {
        let now = Utc::now();
        // Calculate expiration time: now + session_ttl_seconds
        let expires_at = now
            .checked_add_signed(Duration::seconds(self.config.session_ttl_seconds as i64))
            .ok_or_else(|| AppError::Validation("Invalid session TTL".to_string()))?;

        // Create session with UUID v7 (time-ordered UUID for better database indexing)
        let session = AuthSession {
            id: Uuid::now_v7(),
            user_id,
            expires_at,
            revoked_at: None,
        };

        // Store session in database
        self.session_repository
            .create_session(session, audit_event)
            .await
    }

    async fn verify_local_user_password(
        &self,
        user: &User,
        password: &str,
    ) -> Result<(), AppError> {
        let password = password.trim();
        if validate_local_password(password, self.config.password_min_length).is_err() {
            return Err(AppError::Auth("Invalid credentials".to_string()));
        }

        let credential = self
            .credential_repository
            .get_by_user_id(user.id)
            .await?
            .ok_or_else(|| AppError::Auth("Invalid credentials".to_string()))?;

        let password_input = format!("{}{}", password, self.config.password_pepper);
        let parsed_hash = PasswordHash::new(&credential.password_hash)
            .map_err(|_| AppError::Auth("Invalid credentials".to_string()))?;
        Argon2::default()
            .verify_password(password_input.as_bytes(), &parsed_hash)
            .map_err(|_| AppError::Auth("Invalid credentials".to_string()))
    }
}

fn sanitize_username_seed(value: &str) -> String {
    let mut normalized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    normalized = normalized.trim_matches('-').to_string();
    if normalized.is_empty() {
        "oidc-user".to_string()
    } else {
        normalized.chars().take(48).collect()
    }
}

fn sanitize_email_seed(email: Option<&str>, subject: &str) -> String {
    let trimmed = email.map(str::trim).unwrap_or_default();
    if !trimmed.is_empty() && trimmed.contains('@') {
        return trimmed.to_string();
    }
    let mut local = subject
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    local = local.trim_matches('-').to_string();
    if local.is_empty() {
        local = "oidc-user".to_string();
    }
    format!("{}@oidc.local", local)
}
