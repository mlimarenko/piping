use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::config::constants::DEFAULT_CACHE_TTL_SECONDS;
use crate::errors::AppError;

/// Gets a value from Redis cache by key.
///
/// Returns `Ok(None)` if the key doesn't exist.
/// Returns `Ok(Some(value))` if the key exists.
///
/// Used for retrieving cached data (e.g., OIDC session metadata, host configs).
pub async fn get(conn: &mut ConnectionManager, key: &str) -> Result<Option<String>, AppError> {
    conn.get(key).await.map_err(Into::into)
}

/// Sets a value in Redis cache with optional TTL (time-to-live).
///
/// If `ttl_seconds` is `None`, uses the default TTL from configuration.
/// The value will automatically expire after the TTL, freeing memory.
///
/// Used for caching data that should expire (e.g., OIDC state, session metadata).
pub async fn set(
    conn: &mut ConnectionManager,
    key: &str,
    value: &str,
    ttl_seconds: Option<u64>,
) -> Result<(), AppError> {
    let ttl = ttl_seconds.unwrap_or(DEFAULT_CACHE_TTL_SECONDS);
    // SETEX: Set key with expiration time in seconds
    conn.set_ex(key, value, ttl).await.map_err(Into::into)
}

/// Gets several values from Redis cache in one `MGET` round-trip.
///
/// Returns one element per requested key in the same order; a missing key maps
/// to `None` positionally, so the result zips back onto the input keys without
/// misalignment. An empty `keys` slice returns an empty vector without a call.
pub async fn mget(
    conn: &mut ConnectionManager,
    keys: &[String],
) -> Result<Vec<Option<String>>, AppError> {
    match keys {
        [] => Ok(Vec::new()),
        // redis-rs returns a single bulk value (not a one-element array) for a
        // single-key MGET, which would not parse into `Vec<_>`; route it through
        // GET so the positional contract holds for any key count.
        [key] => Ok(vec![conn.get(key).await.map_err(AppError::from)?]),
        _ => conn.mget(keys).await.map_err(Into::into),
    }
}

/// Deletes a value from Redis cache by key.
///
/// Used for explicit cache invalidation (e.g., when data is updated).
/// Returns success even if the key didn't exist.
pub async fn delete(conn: &mut ConnectionManager, key: &str) -> Result<(), AppError> {
    conn.del(key).await.map_err(Into::into)
}

/// Increments a numeric Redis key by the given delta.
///
/// If the key does not exist, Redis initializes it to zero before incrementing.
/// Used for generation-based cache invalidation.
pub async fn increment(
    conn: &mut ConnectionManager,
    key: &str,
    delta: u64,
) -> Result<u64, AppError> {
    conn.incr(key, delta).await.map_err(Into::into)
}

/// Commands per pipelined round trip in [`set_many`]. Bounds the per-trip
/// payload while still collapsing thousands of writes into a handful of
/// round trips (e.g. 4,200 keys -> 9 trips instead of 4,200).
const SET_MANY_CHUNK: usize = 500;

/// Writes many key/value pairs, each with the same TTL, through pipelined
/// `SETEX` commands — `SET_MANY_CHUNK` commands per Redis round trip instead
/// of one round trip per key.
///
/// Returns one result per entry, in input order. Every response in every
/// chunk is checked individually (`ignore_errors()` keeps per-command server
/// errors in the response array instead of failing the whole chunk); a
/// transport failure marks every entry of its chunk as failed. An empty
/// `entries` slice issues no call.
///
/// Generic over [`redis::aio::ConnectionLike`] so tests can count round
/// trips against a recording connection; production callers pass the shared
/// [`ConnectionManager`].
pub async fn set_many<C>(
    conn: &mut C,
    entries: &[(String, String)],
    ttl_seconds: u64,
) -> Vec<Result<(), String>>
where
    C: redis::aio::ConnectionLike + Send,
{
    let mut results = Vec::with_capacity(entries.len());
    for chunk in entries.chunks(SET_MANY_CHUNK) {
        let mut pipe = redis::pipe();
        pipe.ignore_errors();
        for (key, value) in chunk {
            pipe.cmd("SETEX").arg(key).arg(ttl_seconds).arg(value);
        }
        match pipe.query_async::<Vec<redis::Value>>(conn).await {
            Ok(responses) => {
                for index in 0..chunk.len() {
                    results.push(match responses.get(index) {
                        Some(redis::Value::Okay) => Ok(()),
                        Some(redis::Value::SimpleString(status)) if status == "OK" => Ok(()),
                        Some(redis::Value::ServerError(error)) => Err(error.to_string()),
                        Some(other) => Err(format!("unexpected SETEX response: {other:?}")),
                        None => Err("pipeline response missing for entry".to_string()),
                    });
                }
            }
            Err(error) => {
                let message = error.to_string();
                results.extend(chunk.iter().map(|_| Err(message.clone())));
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use redis::aio::ConnectionLike;
    use redis::{Cmd, Pipeline, RedisFuture, Value};

    use super::set_many;

    /// Records every pipeline round trip and answers with a scripted response
    /// per trip, so tests can assert both the round-trip count and the
    /// per-command result handling.
    struct RecordingConnection {
        commands_per_trip: Vec<usize>,
        /// One scripted outcome per expected round trip, consumed in order.
        script: Vec<Result<Vec<Value>, ()>>,
    }

    impl RecordingConnection {
        fn all_ok() -> Self {
            Self {
                commands_per_trip: Vec::new(),
                script: Vec::new(),
            }
        }

        fn scripted(script: Vec<Result<Vec<Value>, ()>>) -> Self {
            Self {
                commands_per_trip: Vec::new(),
                script,
            }
        }
    }

    impl ConnectionLike for RecordingConnection {
        fn req_packed_command<'a>(&'a mut self, _cmd: &'a Cmd) -> RedisFuture<'a, Value> {
            Box::pin(async { Ok(Value::Okay) })
        }

        fn req_packed_commands<'a>(
            &'a mut self,
            _cmd: &'a Pipeline,
            _offset: usize,
            count: usize,
        ) -> RedisFuture<'a, Vec<Value>> {
            self.commands_per_trip.push(count);
            let trip = self.commands_per_trip.len() - 1;
            let outcome = match self.script.get(trip) {
                Some(Ok(values)) => Ok(values.clone()),
                Some(Err(())) => Err(redis::RedisError::from((
                    redis::ErrorKind::Io,
                    "scripted transport failure",
                ))),
                None => Ok(vec![Value::Okay; count]),
            };
            Box::pin(async move { outcome })
        }

        fn get_db(&self) -> i64 {
            0
        }
    }

    fn entries(count: usize) -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("obs:test:{index}"), format!("payload-{index}")))
            .collect()
    }

    #[tokio::test]
    async fn set_many_batches_writes_into_pipelined_chunks() {
        let mut conn = RecordingConnection::all_ok();

        let results = set_many(&mut conn, &entries(1_100), 60).await;

        assert_eq!(results.len(), 1_100);
        assert!(results.iter().all(Result::is_ok));
        // ceil(1100 / 500) = 3 round trips instead of 1,100 sequential SETEXes.
        assert_eq!(conn.commands_per_trip, vec![500, 500, 100]);
    }

    #[tokio::test]
    async fn set_many_marks_only_the_failed_chunk_on_transport_error() {
        // Three chunks; the middle round trip dies on the wire.
        let mut conn = RecordingConnection::scripted(vec![
            Ok(vec![Value::Okay; 500]),
            Err(()),
            Ok(vec![Value::Okay; 100]),
        ]);

        let results = set_many(&mut conn, &entries(1_100), 60).await;

        assert_eq!(results.len(), 1_100);
        assert!(results[..500].iter().all(Result::is_ok));
        assert!(results[500..1_000].iter().all(Result::is_err));
        assert!(results[1_000..].iter().all(Result::is_ok));
    }

    #[tokio::test]
    async fn set_many_checks_every_response_not_just_the_last() {
        // One chunk of three commands where the middle response is anomalous:
        // only that entry fails, and the later Okay does not paper over it.
        let mut conn =
            RecordingConnection::scripted(vec![Ok(vec![Value::Okay, Value::Nil, Value::Okay])]);

        let results = set_many(&mut conn, &entries(3), 60).await;

        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }
}
