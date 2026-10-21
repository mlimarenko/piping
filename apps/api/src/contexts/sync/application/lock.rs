use redis::aio::ConnectionManager;
use uuid::Uuid;

use crate::errors::AppError;

/// Redis-based lock for sync jobs.
///
/// Two properties this deliberately gets right, because the previous version
/// got both wrong:
///
/// * **Acquisition is one command.** It used to be `SET NX` followed by a
///   separate `EXPIRE`. Anything between them — a dropped connection, a
///   cancelled future, an error on the second call — left the key with NO
///   expiry, and the provider was then locked out permanently with no path
///   back except deleting the key by hand. `SET key token NX EX ttl` cannot
///   land half-applied.
/// * **Release is owner-checked.** It used to be an unconditional `DEL`, so
///   whoever called it deleted whatever lock was there — including one a
///   different run had legitimately taken after this one's TTL expired. The
///   release is now a compare-and-delete against the token minted at
///   acquisition, so a late releaser is a no-op instead of a lock thief.
#[derive(Clone)]
pub struct SyncLock {
    pub key: String,
    pub ttl_seconds: usize,
}

/// Proof that this caller holds the lock, carrying the token release must
/// match. Deliberately not `Clone`: two holders would defeat the owner check.
#[derive(Debug)]
pub struct SyncLockGuard {
    token: String,
}

/// Delete the key only when it still holds our token.
///
/// `GET`-then-`DEL` from the client would re-open the same race the owner
/// check exists to close: the TTL can expire and another run can take the lock
/// between the two round trips. A script is one atomic step on the server.
const RELEASE_IF_OWNER: &str = r"
    if redis.call('get', KEYS[1]) == ARGV[1] then
        return redis.call('del', KEYS[1])
    else
        return 0
    end
";

const RENEW_IF_OWNER: &str = r"
    if redis.call('get', KEYS[1]) == ARGV[1] then
        return redis.call('expire', KEYS[1], ARGV[2])
    else
        return 0
    end
";

impl SyncLock {
    pub fn new(key: String, ttl_seconds: usize) -> Self {
        Self { key, ttl_seconds }
    }

    /// Take the lock, or `None` when someone else holds it.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] when Redis is unreachable.
    pub async fn acquire(
        &self,
        conn: &mut ConnectionManager,
    ) -> Result<Option<SyncLockGuard>, AppError> {
        let token = Uuid::now_v7().to_string();
        // `SET key value NX EX ttl` — atomic; returns nil when the key exists.
        let outcome: Option<String> = redis::cmd("SET")
            .arg(&self.key)
            .arg(&token)
            .arg("NX")
            .arg("EX")
            .arg(self.ttl_seconds)
            .query_async(conn)
            .await?;
        Ok(outcome.map(|_| SyncLockGuard { token }))
    }

    /// Extend the lease atomically, without renewing a successor's lock.
    pub async fn renew(
        &self,
        conn: &mut ConnectionManager,
        guard: &SyncLockGuard,
    ) -> Result<bool, AppError> {
        let renewed: i64 = redis::Script::new(RENEW_IF_OWNER)
            .key(&self.key)
            .arg(&guard.token)
            .arg(self.ttl_seconds)
            .invoke_async(conn)
            .await?;
        Ok(renewed == 1)
    }

    /// Release the lock if we still own it.
    ///
    /// Returns `true` when this call actually removed the lock, and `false`
    /// when it had already expired and possibly been retaken — the caller
    /// overran its TTL, which is worth logging but is not an error.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] when Redis is unreachable.
    pub async fn release(
        &self,
        conn: &mut ConnectionManager,
        guard: &SyncLockGuard,
    ) -> Result<bool, AppError> {
        let removed: i64 = redis::Script::new(RELEASE_IF_OWNER)
            .key(&self.key)
            .arg(&guard.token)
            .invoke_async(conn)
            .await?;
        Ok(removed == 1)
    }
}
