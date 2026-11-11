use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

/// Auth session stored for cookie-based authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Absolute deadline of a "remember this device" session. `None` is an
    /// ordinary session whose lifetime is reset by every refresh.
    pub remember_until: Option<DateTime<Utc>>,
}

impl AuthSession {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }

    /// Opens a session at `now`.
    ///
    /// An ordinary session expires after `session_ttl_seconds` of inactivity.
    /// A remembered session starts at its absolute deadline `remembered_ttl_seconds`
    /// and stores it, so refresh can never move it further out.
    pub fn open(
        now: DateTime<Utc>,
        user_id: Uuid,
        session_ttl_seconds: u64,
        remembered_ttl_seconds: Option<u64>,
    ) -> Result<Self, AppError> {
        let remember_until = remembered_ttl_seconds
            .map(|ttl| deadline(now, ttl))
            .transpose()?;
        let expires_at = match remember_until {
            Some(absolute_deadline) => absolute_deadline,
            None => deadline(now, session_ttl_seconds)?,
        };
        Ok(Self {
            id: Uuid::now_v7(),
            user_id,
            expires_at,
            revoked_at: None,
            remember_until,
        })
    }

    /// Deadline a refresh may write for this session.
    ///
    /// Both session kinds extend by the ordinary inactivity TTL; a remembered
    /// session is capped at its absolute deadline and never cut back below the
    /// expiry it already has. Refresh therefore cannot shorten the lifetime an
    /// explicit opt-in promised, and it never passes the absolute deadline.
    pub fn refresh_deadline(
        &self,
        now: DateTime<Utc>,
        session_ttl_seconds: u64,
    ) -> Result<DateTime<Utc>, AppError> {
        let extended = deadline(now, session_ttl_seconds)?;
        Ok(match self.remember_until {
            Some(absolute_deadline) => extended
                .min(absolute_deadline)
                .max(self.expires_at),
            None => extended,
        })
    }

    /// Seconds this session stays usable from `now`; non-positive once it is
    /// past its effective deadline.
    pub fn remaining_seconds(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_seconds()
    }
}

fn deadline(now: DateTime<Utc>, ttl_seconds: u64) -> Result<DateTime<Utc>, AppError> {
    let ttl = i64::try_from(ttl_seconds)
        .map_err(|_| AppError::Validation("Invalid session TTL".to_string()))?;
    now.checked_add_signed(Duration::seconds(ttl))
        .ok_or_else(|| AppError::Validation("Invalid session TTL".to_string()))
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};
    use uuid::Uuid;

    use super::AuthSession;

    const SESSION_TTL: u64 = 86_400;
    const REMEMBER_TTL: u64 = 2_592_000;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_770_000_000, 0).expect("fixed instant")
    }

    fn open_ordinary() -> AuthSession {
        AuthSession::open(now(), Uuid::nil(), SESSION_TTL, None).expect("open ordinary session")
    }

    fn open_remembered() -> AuthSession {
        AuthSession::open(now(), Uuid::nil(), SESSION_TTL, Some(REMEMBER_TTL))
            .expect("open remembered session")
    }

    #[test]
    fn ordinary_session_expires_after_inactivity_ttl_and_has_no_absolute_deadline() {
        let session = open_ordinary();

        assert_eq!(
            session.expires_at,
            now() + Duration::seconds(SESSION_TTL as i64)
        );
        assert!(session.remember_until.is_none());
    }

    #[test]
    fn remembered_session_starts_at_and_records_its_absolute_deadline() {
        let session = open_remembered();

        assert_eq!(
            session.expires_at,
            now() + Duration::seconds(REMEMBER_TTL as i64)
        );
        assert_eq!(session.remember_until, Some(session.expires_at));
    }

    #[test]
    fn ordinary_refresh_extends_by_inactivity_ttl() {
        let session = open_ordinary();
        let later = now() + Duration::hours(1);

        let refreshed = session
            .refresh_deadline(later, SESSION_TTL)
            .expect("refresh deadline");

        assert_eq!(refreshed, later + Duration::seconds(SESSION_TTL as i64));
    }

    #[test]
    fn remembered_refresh_never_shrinks_to_the_inactivity_ttl() {
        let session = open_remembered();
        let later = now() + Duration::minutes(5);

        let refreshed = session
            .refresh_deadline(later, SESSION_TTL)
            .expect("refresh deadline");

        assert_eq!(
            refreshed,
            session.remember_until.expect("absolute deadline"),
            "an early refresh must keep the whole remembered lifetime"
        );
        assert_eq!(
            session
                .refresh_deadline(later, REMEMBER_TTL)
                .expect("refresh deadline"),
            session.remember_until.expect("absolute deadline")
        );
    }

    #[test]
    fn remembered_refresh_never_passes_the_absolute_deadline() {
        let session = open_remembered();
        let late = now() + Duration::seconds(REMEMBER_TTL as i64) - Duration::minutes(1);

        let refreshed = session
            .refresh_deadline(late, SESSION_TTL)
            .expect("refresh deadline");

        assert_eq!(
            refreshed,
            session.remember_until.expect("absolute deadline")
        );
    }

    #[test]
    fn session_past_its_deadline_is_inactive_and_not_revived_by_refresh() {
        let session = open_remembered();
        let after_deadline =
            session.remember_until.expect("absolute deadline") + Duration::seconds(1);

        assert!(!session.is_active(after_deadline));
        assert_eq!(
            session
                .refresh_deadline(after_deadline, SESSION_TTL)
                .expect("refresh deadline"),
            session.remember_until.expect("absolute deadline"),
            "refresh must clamp to the elapsed deadline instead of reviving the session"
        );
        assert!(
            session
                .refresh_deadline(after_deadline, SESSION_TTL)
                .expect("refresh deadline")
                <= after_deadline
        );
    }

    #[test]
    fn revoked_session_is_inactive_and_keeps_its_deadline() {
        let mut session = open_remembered();
        session.revoked_at = Some(now() + Duration::minutes(1));

        assert!(!session.is_active(now() + Duration::minutes(2)));
        assert_eq!(session.remember_until, Some(session.expires_at));
    }

    #[test]
    fn ttl_beyond_the_representable_range_is_rejected() {
        assert!(AuthSession::open(now(), Uuid::nil(), u64::MAX, None).is_err());
        assert!(AuthSession::open(now(), Uuid::nil(), SESSION_TTL, Some(u64::MAX)).is_err());
    }

    #[test]
    fn remaining_seconds_counts_down_from_the_effective_deadline() {
        let session = open_remembered();

        assert_eq!(session.remaining_seconds(now()), REMEMBER_TTL as i64);
        assert_eq!(
            session.remaining_seconds(now() + Duration::seconds(60)),
            REMEMBER_TTL as i64 - 60
        );
        assert!(session.remaining_seconds(now() + Duration::seconds(REMEMBER_TTL as i64)) <= 0);
    }

    #[test]
    fn ordinary_and_remembered_sessions_share_the_absolute_deadline_discipline() {
        let ordinary = open_ordinary();
        let remembered = open_remembered();
        let late = now() + Duration::seconds(SESSION_TTL as i64) - Duration::minutes(1);

        assert_eq!(
            ordinary
                .refresh_deadline(late, SESSION_TTL)
                .expect("ordinary"),
            late + Duration::seconds(SESSION_TTL as i64),
            "an ordinary session keeps sliding past its original expiry"
        );
        assert_eq!(
            remembered
                .refresh_deadline(late, SESSION_TTL)
                .expect("remembered"),
            remembered.remember_until.expect("absolute deadline"),
            "a remembered session stays at its capped, non-shrinking deadline"
        );
        assert_eq!(ordinary.expires_at, now() + Duration::seconds(SESSION_TTL as i64));
    }
}
