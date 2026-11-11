//! Remembered-device session contract (`POST /api/v1/auth/login`,
//! OIDC login state, `POST /api/v1/auth/session-refresh`).
//!
//! These tests drive the real `AuthService` against an in-process session
//! repository so the asserted behaviour is the product's own deadline
//! arithmetic and its refusals: an ordinary session keeps the inactivity TTL, a
//! remembered one carries an absolute deadline that refresh never passes, and a
//! guarded update that matched no row is never reported as a successful refresh.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use piping_backend::api::rest::routes::auth::{LoginRequest, OidcLoginQuery};
use piping_backend::config::AuthConfig;
use piping_backend::contexts::access_control::application::AuthService;
use piping_backend::contexts::access_control::domain::{AuthSession, Group, User, UserCredential};
use piping_backend::contexts::access_control::infrastructure::persistence::{
    AuthSessionRepository, UserCredentialRepository, UserListFilter, UserRepository,
};
use piping_backend::contexts::audit::domain::AuditEvent;
use piping_backend::contexts::shared::pagination::PageResult;
use piping_backend::errors::AppError;

const SESSION_TTL_SECONDS: u64 = 86_400;
const REMEMBER_TTL_SECONDS: u64 = 2_592_000;

fn auth_config() -> AuthConfig {
    AuthConfig {
        session_ttl_seconds: SESSION_TTL_SECONDS,
        remember_device_ttl_seconds: REMEMBER_TTL_SECONDS,
        ..AuthConfig::default()
    }
}

fn audit_event() -> AuditEvent {
    AuditEvent {
        id: Uuid::now_v7(),
        event_type: "auth.signin".to_string(),
        actor_id: None,
        actor_kind: "user".to_string(),
        target_type: None,
        target_id: None,
        metadata: serde_json::json!({}),
        created_at: Utc::now(),
    }
}

fn active_user(id: Uuid) -> User {
    User {
        id,
        username: "remembered-user".to_string(),
        email: "remembered-user@example.test".to_string(),
        status: "active".to_string(),
        auth_source: "local".to_string(),
        external_id: None,
    }
}

// ---------------------------------------------------------------------------
// Fakes: a session store that mirrors the SQL guards, and a user lookup.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RecordedExtend {
    session_id: Uuid,
    requested_expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct FakeSessionStore {
    session: Mutex<Option<AuthSession>>,
    created: Mutex<Vec<AuthSession>>,
    extends: Mutex<Vec<RecordedExtend>>,
    revocations: Mutex<Vec<Uuid>>,
    /// Simulates a revoke that lands between the service's read and the guarded
    /// update: the update then matches no row.
    revoke_before_extend: Mutex<bool>,
}

impl FakeSessionStore {
    fn stored(&self) -> AuthSession {
        self.session
            .lock()
            .expect("session lock")
            .clone()
            .expect("session row")
    }

    fn revoke_before_extend(&self) {
        *self.revoke_before_extend.lock().expect("flag lock") = true;
    }

    fn extends(&self) -> Vec<RecordedExtend> {
        self.extends.lock().expect("extends lock").clone()
    }
}

#[async_trait]
impl AuthSessionRepository for FakeSessionStore {
    async fn create_session(
        &self,
        session: AuthSession,
        _audit_event: AuditEvent,
    ) -> Result<AuthSession, AppError> {
        self.created
            .lock()
            .expect("created lock")
            .push(session.clone());
        *self.session.lock().expect("session lock") = Some(session.clone());
        Ok(session)
    }

    async fn get_session(&self, session_id: Uuid) -> Result<Option<AuthSession>, AppError> {
        Ok(self
            .session
            .lock()
            .expect("session lock")
            .clone()
            .filter(|session| session.id == session_id))
    }

    async fn extend_session(
        &self,
        session_id: Uuid,
        new_expires_at: DateTime<Utc>,
    ) -> Result<Option<AuthSession>, AppError> {
        self.extends
            .lock()
            .expect("extends lock")
            .push(RecordedExtend {
                session_id,
                requested_expires_at: new_expires_at,
            });

        if *self.revoke_before_extend.lock().expect("flag lock") {
            *self.revoke_before_extend.lock().expect("flag lock") = false;
            let mut slot = self.session.lock().expect("session lock");
            if let Some(session) = slot.as_mut() {
                session.revoked_at = Some(Utc::now());
            }
            return Ok(None);
        }

        let mut slot = self.session.lock().expect("session lock");
        let Some(session) = slot.as_mut() else {
            return Ok(None);
        };
        // Same guard and clamp as the SQL statement.
        if session.revoked_at.is_some() || session.expires_at <= Utc::now() {
            return Ok(None);
        }
        session.expires_at = match session.remember_until {
            Some(absolute_deadline) => new_expires_at
                .min(absolute_deadline)
                .max(session.expires_at),
            None => new_expires_at,
        };
        Ok(Some(session.clone()))
    }

    async fn revoke_session(
        &self,
        session_id: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        self.revocations
            .lock()
            .expect("revocations lock")
            .push(session_id);
        let mut slot = self.session.lock().expect("session lock");
        if let Some(session) = slot.as_mut()
            && session.id == session_id
        {
            session.revoked_at = Some(revoked_at);
        }
        Ok(())
    }
}

struct FakeUsers {
    user: Mutex<User>,
}

#[async_trait]
impl UserRepository for FakeUsers {
    async fn get_by_id(&self, user_id: Uuid) -> Result<Option<User>, AppError> {
        let user = self.user.lock().expect("user lock").clone();
        Ok((user.id == user_id).then_some(user))
    }

    async fn get_by_username_or_email(&self, _identifier: &str) -> Result<Option<User>, AppError> {
        Ok(None)
    }

    async fn get_by_external_id(&self, _external_id: &str) -> Result<Option<User>, AppError> {
        Ok(None)
    }

    async fn list_users(&self) -> Result<Vec<User>, AppError> {
        Ok(Vec::new())
    }

    async fn list_users_page(
        &self,
        _filter: UserListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<User>, AppError> {
        Ok(PageResult {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
        })
    }

    async fn create_user(&self, user: User) -> Result<User, AppError> {
        Ok(user)
    }

    async fn update_user(&self, user: User) -> Result<User, AppError> {
        Ok(user)
    }

    async fn delete_user(&self, _user_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn list_groups_by_external_ids(
        &self,
        _external_ids: &[String],
    ) -> Result<Vec<Group>, AppError> {
        Ok(Vec::new())
    }

    async fn list_groups_for_user(&self, _user_id: Uuid) -> Result<Vec<Group>, AppError> {
        Ok(Vec::new())
    }

    async fn sync_oidc_group_memberships(
        &self,
        _user_id: Uuid,
        _oidc_group_ids: &[String],
    ) -> Result<(), AppError> {
        Ok(())
    }
}

struct FakeCredentials;

#[async_trait]
impl UserCredentialRepository for FakeCredentials {
    async fn get_by_user_id(&self, _user_id: Uuid) -> Result<Option<UserCredential>, AppError> {
        Ok(None)
    }

    async fn upsert_credential_hash(
        &self,
        _user_id: Uuid,
        _password_hash: &str,
        _password_changed_at: DateTime<Utc>,
    ) -> Result<UserCredential, AppError> {
        Err(AppError::Database("unused in this test".to_string()))
    }
}

struct Fixture {
    auth: AuthService,
    sessions: Arc<FakeSessionStore>,
    users: Arc<FakeUsers>,
    user_id: Uuid,
}

impl Fixture {
    fn new() -> Self {
        let user_id = Uuid::now_v7();
        let sessions = Arc::new(FakeSessionStore::default());
        let users = Arc::new(FakeUsers {
            user: Mutex::new(active_user(user_id)),
        });
        let auth = AuthService::new(
            users.clone(),
            Arc::new(FakeCredentials),
            sessions.clone(),
            auth_config(),
        );
        Self {
            auth,
            sessions,
            users,
            user_id,
        }
    }

    fn set_user_status(&self, status: &str) {
        self.users.lock_user().status = status.to_string();
    }
}

impl FakeUsers {
    fn lock_user(&self) -> std::sync::MutexGuard<'_, User> {
        self.user.lock().expect("user lock")
    }
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ordinary_login_keeps_the_inactivity_ttl_and_no_absolute_deadline() {
    let fixture = Fixture::new();
    let before = Utc::now();

    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), false)
        .await
        .expect("open session");

    let after = Utc::now();
    assert!(session.remember_until.is_none());
    assert!(session.expires_at >= before + Duration::seconds(SESSION_TTL_SECONDS as i64));
    assert!(session.expires_at <= after + Duration::seconds(SESSION_TTL_SECONDS as i64));
    assert_eq!(fixture.sessions.stored().expires_at, session.expires_at);
}

#[tokio::test]
async fn remembered_login_starts_at_and_records_the_absolute_deadline() {
    let fixture = Fixture::new();
    let before = Utc::now();

    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), true)
        .await
        .expect("open remembered session");

    let after = Utc::now();
    assert_eq!(session.remember_until, Some(session.expires_at));
    assert!(
        session.expires_at >= before + Duration::seconds(REMEMBER_TTL_SECONDS as i64),
        "remembered session must start at the remember-device deadline"
    );
    assert!(
        session.expires_at <= after + Duration::seconds(REMEMBER_TTL_SECONDS as i64),
        "remembered session must not outlive the remember-device TTL"
    );
    assert_eq!(
        fixture.sessions.stored().remember_until,
        session.remember_until
    );
}

#[tokio::test]
async fn login_request_defaults_to_not_remembering_the_device() {
    let omitted: LoginRequest =
        serde_json::from_str(r#"{"identifier":"admin","password":"secret"}"#)
            .expect("login request without remember_device");
    let explicit: LoginRequest = serde_json::from_str(
        r#"{"identifier":"admin","password":"secret","remember_device":true}"#,
    )
    .expect("login request with remember_device");

    assert!(!omitted.remember_device);
    assert!(explicit.remember_device);
}

#[tokio::test]
async fn oidc_login_state_defaults_to_not_remembering_the_device() {
    let omitted: OidcLoginQuery =
        serde_json::from_str(r#"{"redirect_to":"/workspace"}"#).expect("login query");
    let explicit: OidcLoginQuery =
        serde_json::from_str(r#"{"redirect_to":null,"remember_device":true}"#)
            .expect("login query with opt-in");

    assert_eq!(omitted.remember_device, None);
    assert_eq!(explicit.remember_device, Some(true));
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ordinary_refresh_extends_by_the_inactivity_ttl() {
    let fixture = Fixture::new();
    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), false)
        .await
        .expect("open session");
    let now = Utc::now() + Duration::hours(1);

    let (_, refreshed) = fixture
        .auth
        .refresh_session(session.id, now)
        .await
        .expect("refresh")
        .expect("active session refreshes");

    assert_eq!(
        refreshed.expires_at,
        now + Duration::seconds(SESSION_TTL_SECONDS as i64)
    );
    let extends = fixture.sessions.extends();
    assert_eq!(extends.len(), 1);
    assert_eq!(extends[0].session_id, session.id);
    assert_eq!(
        extends[0].requested_expires_at,
        now + Duration::seconds(SESSION_TTL_SECONDS as i64)
    );
}

#[tokio::test]
async fn remembered_refresh_is_capped_at_the_absolute_deadline() {
    let fixture = Fixture::new();
    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), true)
        .await
        .expect("open remembered session");
    let absolute_deadline = session.remember_until.expect("absolute deadline");
    let now = absolute_deadline - Duration::minutes(10);

    let (_, refreshed) = fixture
        .auth
        .refresh_session(session.id, now)
        .await
        .expect("refresh")
        .expect("remembered session refreshes until its deadline");

    assert_eq!(refreshed.expires_at, absolute_deadline);
    assert_eq!(refreshed.remember_until, Some(absolute_deadline));
    assert_eq!(
        fixture.sessions.stored().expires_at,
        absolute_deadline,
        "stored expiry must not pass the absolute deadline"
    );
    let extends = fixture.sessions.extends();
    assert_eq!(extends.len(), 1);
    assert_eq!(extends[0].session_id, session.id);
    assert_eq!(
        extends[0].requested_expires_at, absolute_deadline,
        "the requested extension is clamped to the absolute deadline"
    );
}

#[tokio::test]
async fn remembered_refresh_does_not_shrink_to_the_inactivity_ttl() {
    let fixture = Fixture::new();
    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), true)
        .await
        .expect("open remembered session");
    let now = Utc::now() + Duration::minutes(5);

    let (_, refreshed) = fixture
        .auth
        .refresh_session(session.id, now)
        .await
        .expect("refresh")
        .expect("remembered session refreshes");

    assert_eq!(
        refreshed.expires_at,
        session
            .remember_until
            .expect("absolute remembered deadline"),
        "an early refresh must keep the whole remembered lifetime"
    );
    assert_eq!(refreshed.expires_at, session.expires_at);
}

#[tokio::test]
async fn refresh_refuses_to_report_success_when_the_update_matched_no_row() {
    let fixture = Fixture::new();
    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), true)
        .await
        .expect("open remembered session");
    fixture.sessions.revoke_before_extend();

    let result = fixture
        .auth
        .refresh_session(session.id, Utc::now())
        .await
        .expect("refresh must not fail");

    assert!(
        result.is_none(),
        "a guarded update that matched no active row is an expired session, not a refresh"
    );
    assert!(fixture.sessions.stored().revoked_at.is_some());
}

#[tokio::test]
async fn expired_remembered_session_is_not_revived_by_refresh() {
    let fixture = Fixture::new();
    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), true)
        .await
        .expect("open remembered session");
    let absolute_deadline = session.remember_until.expect("absolute deadline");

    let result = fixture
        .auth
        .refresh_session(session.id, absolute_deadline + Duration::seconds(1))
        .await
        .expect("refresh must not fail");

    assert!(result.is_none());
    assert!(fixture.sessions.extends().is_empty());
}

#[tokio::test]
async fn inactive_user_cannot_refresh_a_remembered_session() {
    let fixture = Fixture::new();
    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), true)
        .await
        .expect("open remembered session");
    fixture.set_user_status("inactive");

    let result = fixture
        .auth
        .refresh_session(session.id, Utc::now())
        .await
        .expect("refresh must not fail");

    assert!(result.is_none());
    assert!(fixture.sessions.extends().is_empty());
}

// ---------------------------------------------------------------------------
// Logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_revokes_a_remembered_session() {
    let fixture = Fixture::new();
    let session = fixture
        .auth
        .open_session(fixture.user_id, audit_event(), true)
        .await
        .expect("open remembered session");

    fixture
        .auth
        .logout(session.id, Utc::now())
        .await
        .expect("logout");

    assert!(fixture.sessions.stored().revoked_at.is_some());
    let resolved = fixture
        .auth
        .resolve_session(session.id, Utc::now())
        .await
        .expect("resolve");
    assert!(
        resolved.is_none(),
        "a revoked remembered session stays revoked"
    );
}
