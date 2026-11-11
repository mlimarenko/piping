use redis::aio::ConnectionManager;
use uuid::Uuid;

use crate::config::constants::DEFAULT_CACHE_TTL_SECONDS;
use crate::errors::AppError;
use crate::storage::cache::redis::cache;

// `v2` namespace: the subject grammar moved from mutable bare names to canonical
// `kind:uuid` strings (RBAC subject unification). Bumping the static namespace
// guarantees no stale `v1` bare-subject cache entry is ever served post-cutover.
pub(super) const POLICY_CACHE_PREFIX: &str = "access_control:policies:v2";
pub(super) const POLICY_CACHE_VERSION_KEY: &str = "access_control:policies:version";
pub(super) const POLICY_CACHE_USER_VERSION_PREFIX: &str = "access_control:policies:user_version";

// A script keeps the first increment and its expiry indivisible. Separate INCR
// and EXPIRE commands can strand a persistent key if the client is cancelled.
const BUMP_USER_GENERATION_WITH_TTL: &str = r"
    local generation = redis.call('incr', KEYS[1])
    if generation == 1 then
        redis.call('expire', KEYS[1], ARGV[1])
    end
    return generation
";

pub(super) fn build_policy_cache_key(
    user_id: Uuid,
    subjects: &[String],
    generation: u64,
    user_generation: u64,
) -> String {
    let mut sorted_subjects = subjects.to_vec();
    sorted_subjects.sort();
    format!(
        "{}:v{}:u{}:{}:{}",
        POLICY_CACHE_PREFIX,
        generation,
        user_generation,
        user_id,
        sorted_subjects.join("|")
    )
}

pub(super) fn policy_cache_user_version_key(user_id: Uuid) -> String {
    format!("{}:{}", POLICY_CACHE_USER_VERSION_PREFIX, user_id)
}

pub(super) async fn bump_policy_cache_generation(
    conn: &mut ConnectionManager,
) -> Result<u64, AppError> {
    cache::increment(conn, POLICY_CACHE_VERSION_KEY, 1).await
}

async fn parse_generation(
    conn: &mut ConnectionManager,
    key: &str,
    raw: Option<&str>,
) -> Result<u64, AppError> {
    let generation = decode_generation(key, raw);
    if generation.is_err() {
        cache::delete(conn, key).await?;
    }
    generation
}

fn decode_generation(key: &str, raw: Option<&str>) -> Result<u64, AppError> {
    match raw {
        Some(raw) => raw.parse::<u64>().map_err(|error| {
            AppError::Internal(format!(
                "Invalid policy cache generation in `{key}`: {error}"
            ))
        }),
        None => Ok(0),
    }
}

pub(super) async fn load_policy_cache_generations(
    conn: &mut ConnectionManager,
    user_id: Uuid,
) -> Result<(u64, u64), AppError> {
    let user_key = policy_cache_user_version_key(user_id);
    let keys = [POLICY_CACHE_VERSION_KEY.to_string(), user_key.clone()];
    let values = cache::mget(conn, &keys).await?;
    let [generation, user_generation] = values.as_slice() else {
        return Err(AppError::Internal(
            "Redis returned the wrong number of policy cache generations".to_string(),
        ));
    };
    let generation =
        parse_generation(conn, POLICY_CACHE_VERSION_KEY, generation.as_deref()).await?;
    let user_generation = parse_generation(conn, &user_key, user_generation.as_deref()).await?;
    Ok((generation, user_generation))
}

pub(super) async fn bump_policy_cache_user_generation(
    conn: &mut ConnectionManager,
    user_id: Uuid,
) -> Result<u64, AppError> {
    let key = policy_cache_user_version_key(user_id);
    redis::Script::new(BUMP_USER_GENERATION_WITH_TTL)
        .key(key)
        .arg(DEFAULT_CACHE_TTL_SECONDS)
        .invoke_async(conn)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{
        build_policy_cache_key, bump_policy_cache_user_generation, decode_generation,
        load_policy_cache_generations, policy_cache_user_version_key,
    };
    use redis::aio::ConnectionManager;
    use uuid::Uuid;

    /// The Redis-backed cases below live here rather than in `tests/` on
    /// purpose. The functions under test are `pub(super)`, and the alternative
    /// — a `#[path]` include of this file from an integration target —
    /// compiles a SECOND copy of the module. That copy cannot see
    /// `admin_service`, so every function only the sibling calls is reported
    /// dead, and the two copies are then free to drift apart.
    async fn redis() -> Result<ConnectionManager, Box<dyn std::error::Error + Send + Sync>> {
        let url = std::env::var("PIPING_REDIS__URL")
            .unwrap_or_else(|_| "redis://:dev-redis-password@localhost:56380/0".into());
        Ok(ConnectionManager::new(redis::Client::open(url)?).await?)
    }

    #[tokio::test]
    #[ignore = "requires Redis (run with the local dev stack)"]
    async fn a_newly_created_user_generation_expires()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = redis().await?;
        let user_id = Uuid::now_v7();
        let key = policy_cache_user_version_key(user_id);

        let generation = bump_policy_cache_user_generation(&mut conn, user_id).await?;
        let ttl: i64 = redis::cmd("TTL").arg(&key).query_async(&mut conn).await?;
        let _: i64 = redis::cmd("DEL").arg(&key).query_async(&mut conn).await?;

        assert_eq!(generation, 1);
        assert!(
            ttl > 0,
            "a per-user generation key with no expiry accumulates forever; got TTL {ttl}"
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires Redis (run with the local dev stack)"]
    async fn a_missing_generation_reads_as_zero_without_allocating_a_key()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = redis().await?;
        let user_id = Uuid::now_v7();
        let user_key = policy_cache_user_version_key(user_id);
        let _: i64 = redis::cmd("DEL")
            .arg(&user_key)
            .query_async(&mut conn)
            .await?;

        let (_, user_generation) = load_policy_cache_generations(&mut conn, user_id).await?;
        let allocated: bool = redis::cmd("EXISTS")
            .arg(&user_key)
            .query_async(&mut conn)
            .await?;
        let _: i64 = redis::cmd("DEL")
            .arg(&user_key)
            .query_async(&mut conn)
            .await?;

        assert_eq!(user_generation, 0);
        assert!(
            !allocated,
            "reading a generation must not materialise the key it read"
        );
        Ok(())
    }

    #[test]
    fn build_policy_cache_key_sorts_subjects() {
        let user_id = Uuid::nil();
        let key = build_policy_cache_key(user_id, &["ops".to_string(), "admins".to_string()], 7, 3);
        assert_eq!(
            key,
            "access_control:policies:v2:v7:u3:00000000-0000-0000-0000-000000000000:admins|ops"
        );
    }

    #[test]
    fn missing_generation_is_zero_but_corrupt_generation_is_an_error() {
        assert_eq!(
            decode_generation("policy-version", None).expect("missing key starts at zero"),
            0
        );
        assert!(
            decode_generation("policy-version", Some("not-a-generation")).is_err(),
            "a corrupt generation must not select the authoritative v0 policy cache"
        );
    }
}
