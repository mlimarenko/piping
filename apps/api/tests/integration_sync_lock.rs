//! Regression tests for the sync lock, against a live Redis.
//!
//! The previous implementation had two defects that only a real server can
//! demonstrate, because both are about what the key looks like between two
//! round trips:
//!
//! * acquisition was `SET NX` plus a separate `EXPIRE`, so a failure between
//!   them left a key with NO TTL and locked the provider out permanently;
//! * release was an unconditional `DEL`, so a run that overran its TTL deleted
//!   the lock a *different* run had since taken.

mod common;

use piping_backend::contexts::sync::application::SyncLock;
use uuid::Uuid;

fn unique_key(kind: &str) -> String {
    format!("test:sync-lock:{kind}:{}", Uuid::now_v7())
}

#[tokio::test]
#[ignore = "requires Redis (run with local dev Redis)"]
async fn acquire_sets_the_ttl_atomically() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = common::build_test_redis().await?;
    let key = unique_key("ttl");
    let lock = SyncLock::new(key.clone(), 30);

    let guard = lock
        .acquire(&mut conn)
        .await?
        .ok_or("lock must be free at the start")?;

    // The whole point: the key can never exist without an expiry, because the
    // SET that creates it carries EX. `-1` means "no TTL" — the state the old
    // two-command form could leave behind and never recover from.
    let ttl: i64 = redis::cmd("TTL").arg(&key).query_async(&mut conn).await?;
    assert!(
        ttl > 0 && ttl <= 30,
        "a held lock must carry its TTL; got {ttl} (-1 means no expiry at all)"
    );

    assert!(lock.release(&mut conn, &guard).await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires Redis (run with local dev Redis)"]
async fn second_acquire_is_refused_while_held()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = common::build_test_redis().await?;
    let key = unique_key("mutex");
    let lock = SyncLock::new(key, 30);

    let first = lock
        .acquire(&mut conn)
        .await?
        .ok_or("first acquire must win")?;
    assert!(
        lock.acquire(&mut conn).await?.is_none(),
        "a held lock must refuse a second holder"
    );

    assert!(lock.release(&mut conn, &first).await?);
    assert!(
        lock.acquire(&mut conn).await?.is_some(),
        "the lock must be takeable again after release"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Redis (run with local dev Redis)"]
async fn release_does_not_steal_a_lock_owned_by_someone_else()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = common::build_test_redis().await?;
    let key = unique_key("owner");
    let lock = SyncLock::new(key.clone(), 30);

    // A run takes the lock, then overruns its TTL — simulated by deleting the
    // key, which is what expiry amounts to from the next run's point of view.
    let stale = lock
        .acquire(&mut conn)
        .await?
        .ok_or("first acquire must win")?;
    let _: i64 = redis::cmd("DEL").arg(&key).query_async(&mut conn).await?;

    // A different run legitimately takes the now-free lock.
    let current = lock
        .acquire(&mut conn)
        .await?
        .ok_or("the expired lock must be retakeable")?;

    // The first run finally finishes and releases. Under the old unconditional
    // DEL this deleted the SECOND run's lock and let a third sync start
    // concurrently. It must be a no-op instead.
    assert!(
        !lock.release(&mut conn, &stale).await?,
        "a release from an expired holder must not remove someone else's lock"
    );
    assert!(
        lock.acquire(&mut conn).await?.is_none(),
        "the current holder must still hold the lock after the stale release"
    );

    assert!(lock.release(&mut conn, &current).await?);
    Ok(())
}
