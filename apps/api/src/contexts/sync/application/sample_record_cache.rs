//! Sample-record per-session cache.
//!
//! `GET /api/v1/providers/{id}/sample-record?model={entity}` is hit
//! repeatedly while a user iterates on a mapping configuration in the FE
//! editor. Without caching, every keystroke that changes preview output
//! issues a fresh round-trip to NetBox. This cache stores the first
//! sample record per `(provider_id, model)` pair so subsequent reads in
//! the same session return instantly.
//!
//! Configuration:
//!   * `PIPING_SYNC__SAMPLE_CACHE_TTL_SECONDS` — entry TTL (default 600).
//!   * `PIPING_SYNC__SAMPLE_CACHE_MAX_ENTRIES` — eviction threshold
//!     (default 100).
//!
//! Eviction is approximate-LRU: when the map exceeds `max_entries`, the
//! oldest entry by `inserted_at` is dropped. This is good enough for a
//! per-session preview cache; under heavy contention the FE simply pays
//! one extra round-trip and re-warms the cache.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

const ENV_TTL_SECONDS: &str = "PIPING_SYNC__SAMPLE_CACHE_TTL_SECONDS";
const ENV_MAX_ENTRIES: &str = "PIPING_SYNC__SAMPLE_CACHE_MAX_ENTRIES";
const DEFAULT_TTL_SECONDS: u64 = 600;
const DEFAULT_MAX_ENTRIES: usize = 100;

/// One cache entry: the JSON record plus the wall-clock instant it was
/// inserted, used for both TTL expiry and LRU eviction.
#[derive(Debug, Clone)]
struct CachedSample {
    record: Value,
    inserted_at: Instant,
}

/// Per-session cache keyed by `(provider_id, model)`.
type SampleCacheMap = HashMap<(Uuid, String), CachedSample>;

#[derive(Clone)]
pub struct SampleRecordCache {
    inner: Arc<RwLock<SampleCacheMap>>,
    ttl: Duration,
    max_entries: usize,
}

impl Default for SampleRecordCache {
    fn default() -> Self {
        Self::from_env()
    }
}

impl SampleRecordCache {
    /// Construct a cache pulling TTL/size limits from `PIPING_SYNC__*`
    /// env vars, falling back to defaults if unset or unparsable.
    pub fn from_env() -> Self {
        let ttl_seconds = std::env::var(ENV_TTL_SECONDS)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TTL_SECONDS);
        let max_entries = std::env::var(ENV_MAX_ENTRIES)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_ENTRIES);
        Self::with_settings(Duration::from_secs(ttl_seconds), max_entries)
    }

    pub fn with_settings(ttl: Duration, max_entries: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
            max_entries: max_entries.max(1),
        }
    }

    pub async fn get(&self, provider_id: Uuid, model: &str) -> Option<Value> {
        let map = self.inner.read().await;
        let key = (provider_id, model.to_string());
        let entry = map.get(&key)?;
        if entry.inserted_at.elapsed() > self.ttl {
            return None;
        }
        Some(entry.record.clone())
    }

    pub async fn insert(&self, provider_id: Uuid, model: String, record: Value) {
        let mut map = self.inner.write().await;
        let now = Instant::now();
        map.insert(
            (provider_id, model),
            CachedSample {
                record,
                inserted_at: now,
            },
        );

        // Approximate-LRU eviction: drop the single oldest entry when the
        // map outgrows the configured ceiling. Cheaper than maintaining a
        // separate access list and good enough for preview-grade traffic.
        if map.len() > self.max_entries
            && let Some(oldest_key) = map
                .iter()
                .min_by_key(|(_, v)| v.inserted_at)
                .map(|(k, _)| k.clone())
        {
            map.remove(&oldest_key);
        }
    }

    /// Drop every entry tied to `provider_id`. Called from the
    /// `provider_registry/refresh` POST endpoint so the FE preview tab
    /// re-fetches against the freshly-loaded NetBox schema.
    pub async fn invalidate_provider(&self, provider_id: Uuid) {
        let mut map = self.inner.write().await;
        map.retain(|(pid, _), _| *pid != provider_id);
    }

    /// Drop the entire cache. Used by tests and by global flush operations.
    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }

    /// Visible-entry count (entries past TTL are still counted; TTL is
    /// enforced on `get`). Public for diagnostic / test use.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Whether the cache currently holds no entries (TTL not enforced here;
    /// mirrors [`len`](Self::len)). Public for diagnostic / test use.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider() -> Uuid {
        Uuid::now_v7()
    }

    #[tokio::test]
    async fn cache_miss_returns_none() {
        let cache = SampleRecordCache::with_settings(Duration::from_secs(60), 10);
        assert!(cache.get(provider(), "dcim.devices").await.is_none());
    }

    #[tokio::test]
    async fn cache_hit_returns_inserted_record() {
        let cache = SampleRecordCache::with_settings(Duration::from_secs(60), 10);
        let pid = provider();
        let record = json!({"id": 1, "name": "device-a"});
        cache
            .insert(pid, "dcim.devices".to_string(), record.clone())
            .await;
        assert_eq!(cache.get(pid, "dcim.devices").await.expect("hit"), record);
    }

    #[tokio::test]
    async fn cache_returns_none_after_ttl_expiry() {
        let cache = SampleRecordCache::with_settings(Duration::from_millis(10), 10);
        let pid = provider();
        cache
            .insert(pid, "dcim.devices".to_string(), json!({"id": 1}))
            .await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(cache.get(pid, "dcim.devices").await.is_none());
    }

    #[tokio::test]
    async fn lru_evicts_oldest_when_capacity_exceeded() {
        let cache = SampleRecordCache::with_settings(Duration::from_secs(60), 2);
        let pid = provider();
        cache.insert(pid, "a".to_string(), json!({"k": 1})).await;
        // Force a measurable instant gap between inserts so the oldest is
        // unambiguously the first entry. Instant resolution on Linux is
        // sub-microsecond, but the assertion stays robust under jitter.
        tokio::time::sleep(Duration::from_millis(2)).await;
        cache.insert(pid, "b".to_string(), json!({"k": 2})).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        cache.insert(pid, "c".to_string(), json!({"k": 3})).await;

        // Capacity is 2: "a" must have been evicted.
        assert!(cache.get(pid, "a").await.is_none(), "a should be evicted");
        assert!(cache.get(pid, "b").await.is_some(), "b should remain");
        assert!(cache.get(pid, "c").await.is_some(), "c should remain");
        assert_eq!(cache.len().await, 2);
    }

    #[tokio::test]
    async fn invalidate_provider_drops_only_target_provider() {
        let cache = SampleRecordCache::with_settings(Duration::from_secs(60), 10);
        let p1 = provider();
        let p2 = provider();
        cache.insert(p1, "a".to_string(), json!({"x": 1})).await;
        cache.insert(p2, "a".to_string(), json!({"x": 2})).await;

        cache.invalidate_provider(p1).await;
        assert!(cache.get(p1, "a").await.is_none());
        assert!(cache.get(p2, "a").await.is_some());
    }

    #[tokio::test]
    async fn invalidate_provider_is_noop_for_unknown_provider() {
        let cache = SampleRecordCache::with_settings(Duration::from_secs(60), 10);
        let p1 = provider();
        cache.insert(p1, "a".to_string(), json!({"x": 1})).await;
        cache.invalidate_provider(provider()).await;
        assert!(cache.get(p1, "a").await.is_some());
    }
}
