use crate::contexts::access_control::domain::Action;
use crate::contexts::sync::domain::InventoryCommitAuthorisation;
use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use std::{future::Future, sync::Arc, time::Duration};
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

/// Renewable provider lease shared by pull, inbound and outbound work.
pub struct ProviderLease {
    lock: SyncLock,
    guard: Arc<SyncLockGuard>,
    redis: ConnectionManager,
}

/// Admission failures are distinct from errors reported by a running lease.
#[derive(Debug, thiserror::Error)]
pub enum ProviderLeaseError {
    #[error("Sync already running for provider {0}")]
    Busy(Uuid),
    #[error(transparent)]
    Unavailable(#[from] AppError),
}

impl From<ProviderLeaseError> for AppError {
    fn from(error: ProviderLeaseError) -> Self {
        match error {
            ProviderLeaseError::Busy(provider) => {
                Self::Conflict(format!("Sync already running for provider {provider}"))
            }
            ProviderLeaseError::Unavailable(error) => error,
        }
    }
}

impl ProviderLease {
    pub async fn acquire(
        redis: ConnectionManager,
        provider: Uuid,
        ttl: usize,
    ) -> Result<Self, ProviderLeaseError> {
        let lock = SyncLock::new(format!("sync:provider:{provider}"), ttl.max(1));
        let guard = lock
            .acquire(&mut redis.clone())
            .await?
            .ok_or(ProviderLeaseError::Busy(provider))?;
        Ok(Self {
            lock,
            guard: Arc::new(guard),
            redis,
        })
    }

    pub async fn run<T, F, Work>(self, work: F) -> Result<T, AppError>
    where
        F: FnOnce(Arc<dyn InventoryCommitAuthorisation>) -> Work,
        Work: Future<Output = Result<T, AppError>>,
    {
        let authorisation = Arc::new(self);
        let result = {
            let work = work(authorisation.clone());
            tokio::pin!(work);
            let period = Duration::from_millis(
                ((authorisation.lock.ttl_seconds as u64).saturating_mul(1000) / 3).max(1),
            );
            let mut renewals = tokio::time::interval(period);
            renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    result = &mut work => break result,
                    _ = renewals.tick() => {
                        if let Err(error) = authorisation.ensure_owned().await { break Err(error); }
                    }
                }
            }
        };
        // A release failure cannot undo committed work; the owner-checked TTL bounds it.
        if let Err(error) = authorisation
            .lock
            .release(&mut authorisation.redis.clone(), &authorisation.guard)
            .await
        {
            tracing::warn!(%error, "Failed to release provider lease");
        }
        result
    }
}

impl InventoryCommitAuthorisation for ProviderLease {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            match self.lock.renew(&mut self.redis.clone(), &self.guard).await {
                Ok(true) => Ok(()),
                Ok(false) => Err(AppError::Conflict("Sync lease ownership was lost".into())),
                Err(_) => Err(AppError::Redis(
                    "Sync lease ownership cannot be verified".into(),
                )),
            }
        })
    }

    /// A lease proves that no other run owns this provider; it says nothing about
    /// who is asking, so it can never authorise reading a persisted object.
    fn ensure_reference_access(
        &self,
        _id: Uuid,
        _attributes: serde_json::Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async {
            Err(AppError::Forbidden(
                "Provider lease is not a request authorisation".into(),
            ))
        })
    }

    /// The same rule for writes: lease ownership is liveness, not a per-object
    /// CREATE/UPDATE grant.
    fn ensure_mutation_access(
        &self,
        _action: Action,
        _id: Uuid,
        _attributes: serde_json::Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async {
            Err(AppError::Forbidden(
                "Provider lease is not a request authorisation".into(),
            ))
        })
    }
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
