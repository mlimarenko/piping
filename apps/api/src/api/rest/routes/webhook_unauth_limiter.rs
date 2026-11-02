//! Per-provider rate limiter for invalid-signature webhook attempts.
//!
//! # Purpose
//! This module exists to prevent a flood of unauthenticated webhook requests
//! from flipping a provider's operational status to `Degraded` faster than an
//! operator can react.  It does NOT replace reverse-proxy request-rate limiting
//! (operators should configure that separately).
//!
//! # Windows
//! * **Short window** — 10 invalid-signature attempts in 60 s: metric is
//!   recorded and a warning is logged, but the provider status is NOT flipped.
//!   The caller should skip the `mark_signature_failure` status-flip when the
//!   limiter says the short window is exceeded.
//! * **Long window** — 100 invalid-signature attempts in 300 s (5 min): the
//!   limiter allows the status flip to occur (returns `ShouldFlipStatus`).
//!
//! # Clock
//! The internal clock is injected via the `Clock` trait so unit tests can use
//! a mock without sleeping.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use once_cell::sync::Lazy;
use tokio::sync::Mutex;
use uuid::Uuid;

// std::collections::HashMap is fine here; the map is shared behind a single
// Mutex so we don't need dashmap's fine-grained locking.
use std::collections::HashMap;

/// Process-global rate limiter instance shared by all webhook handlers.
pub static WEBHOOK_UNAUTH_LIMITER: Lazy<WebhookUnauthLimiter> =
    Lazy::new(WebhookUnauthLimiter::new);

/// Short-window threshold: suppress status flip for up to this many attempts.
const SHORT_WINDOW_MAX: usize = 10;
/// Short-window duration in seconds.
const SHORT_WINDOW_SECS: u64 = 60;

/// Long-window threshold: allow status flip once this many attempts accumulate.
const LONG_WINDOW_MAX: usize = 100;
/// Long-window duration in seconds (5 minutes).
const LONG_WINDOW_SECS: u64 = 300;

/// Result returned by [`WebhookUnauthLimiter::record_attempt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnauthLimitDecision {
    /// Below both thresholds — normal path, allow status flip.
    Allow,
    /// Short window exceeded; suppress status flip to avoid DoS-driven flaps.
    SuppressStatusFlip,
    /// Long window exceeded; the pattern is sustained — allow status flip.
    ShouldFlipStatus,
}

/// Abstraction over wall-clock time so tests can inject a mock.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Instant;
}

/// Production clock backed by `Instant::now()`.
#[derive(Clone, Default)]
pub struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

struct ProviderState {
    /// Ring-buffer of attempt timestamps within the short window.
    short: VecDeque<Instant>,
    /// Ring-buffer of attempt timestamps within the long window.
    long: VecDeque<Instant>,
}

impl ProviderState {
    fn new() -> Self {
        Self {
            short: VecDeque::new(),
            long: VecDeque::new(),
        }
    }

    /// Evict entries older than `window_secs` from `buf`.
    fn evict(buf: &mut VecDeque<Instant>, now: Instant, window_secs: u64) {
        let cutoff = now
            .checked_sub(std::time::Duration::from_secs(window_secs))
            .unwrap_or(now);
        while buf.front().is_some_and(|t| *t < cutoff) {
            buf.pop_front();
        }
    }
}

/// Shared rate-limiter state, cheaply cloneable via `Arc`.
#[derive(Clone)]
pub struct WebhookUnauthLimiter<C: Clock = WallClock> {
    state: Arc<Mutex<HashMap<Uuid, ProviderState>>>,
    clock: Arc<C>,
}

impl WebhookUnauthLimiter<WallClock> {
    /// Create a production limiter using the wall clock.
    pub fn new() -> Self {
        Self::with_clock(WallClock)
    }
}

impl Default for WebhookUnauthLimiter<WallClock> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Clock> WebhookUnauthLimiter<C> {
    /// Create a limiter with a custom clock (useful for testing).
    pub fn with_clock(clock: C) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(clock),
        }
    }

    /// Record one invalid-signature attempt for `provider_id` and return the
    /// appropriate rate-limit decision.
    ///
    /// This must be called BEFORE `mark_signature_failure` so the caller can
    /// decide whether to suppress the operational-status flip.
    pub async fn record_attempt(&self, provider_id: Uuid) -> UnauthLimitDecision {
        let now = self.clock.now();
        let mut map = self.state.lock().await;
        let entry = map.entry(provider_id).or_insert_with(ProviderState::new);

        // Evict stale entries before counting.
        ProviderState::evict(&mut entry.short, now, SHORT_WINDOW_SECS);
        ProviderState::evict(&mut entry.long, now, LONG_WINDOW_SECS);

        entry.short.push_back(now);
        entry.long.push_back(now);

        let short_count = entry.short.len();
        let long_count = entry.long.len();

        if long_count >= LONG_WINDOW_MAX {
            UnauthLimitDecision::ShouldFlipStatus
        } else if short_count > SHORT_WINDOW_MAX {
            UnauthLimitDecision::SuppressStatusFlip
        } else {
            UnauthLimitDecision::Allow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Shared offset counter so the test can advance time for the same clock
    /// instance held by the limiter.
    #[derive(Clone)]
    struct MockClock {
        offset_secs: StdArc<AtomicU64>,
    }

    impl MockClock {
        fn new() -> Self {
            Self {
                offset_secs: StdArc::new(AtomicU64::new(0)),
            }
        }

        fn advance_secs(&self, secs: u64) {
            self.offset_secs.fetch_add(secs, Ordering::SeqCst);
        }
    }

    impl Clock for MockClock {
        fn now(&self) -> Instant {
            // Use a fixed epoch (process start) + offset so `Instant` math works.
            static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
            let epoch = *EPOCH.get_or_init(Instant::now);
            let secs = self.offset_secs.load(Ordering::SeqCst);
            epoch + std::time::Duration::from_secs(secs)
        }
    }

    #[tokio::test]
    async fn first_ten_attempts_allow_status_flip() {
        let limiter = WebhookUnauthLimiter::with_clock(MockClock::new());
        let id = Uuid::now_v7();
        for _ in 0..10 {
            assert_eq!(limiter.record_attempt(id).await, UnauthLimitDecision::Allow);
        }
    }

    #[tokio::test]
    async fn eleventh_attempt_in_short_window_suppresses_flip() {
        let clock = MockClock::new();
        // Clock is frozen — all attempts land at the same Instant.
        let limiter = WebhookUnauthLimiter::with_clock(clock);
        let id = Uuid::now_v7();
        for _ in 0..10 {
            limiter.record_attempt(id).await;
        }
        // 11th attempt exceeds SHORT_WINDOW_MAX (10).
        assert_eq!(
            limiter.record_attempt(id).await,
            UnauthLimitDecision::SuppressStatusFlip
        );
    }

    #[tokio::test]
    async fn hundred_attempts_in_long_window_flips_status() {
        // All 100 attempts at t=0 → inside the 5-min long window.
        let limiter = WebhookUnauthLimiter::with_clock(MockClock::new());
        let id = Uuid::now_v7();
        let mut last = UnauthLimitDecision::Allow;
        for _ in 0..100 {
            last = limiter.record_attempt(id).await;
        }
        assert_eq!(last, UnauthLimitDecision::ShouldFlipStatus);
    }

    #[tokio::test]
    async fn attempts_outside_long_window_do_not_accumulate() {
        // 50 attempts at t=0, advance 400 s (past 300 s long window), 50 more.
        // The first batch has expired so the total stays below LONG_WINDOW_MAX.
        let clock = MockClock::new();
        let limiter = WebhookUnauthLimiter::with_clock(clock.clone());
        let id = Uuid::now_v7();

        for _ in 0..50 {
            limiter.record_attempt(id).await;
        }

        // Advance past the long window so the first 50 entries age out.
        clock.advance_secs(400);

        let mut last = UnauthLimitDecision::Allow;
        for _ in 0..50 {
            last = limiter.record_attempt(id).await;
        }
        // Only 50 live entries remain — below LONG_WINDOW_MAX (100).
        assert_ne!(last, UnauthLimitDecision::ShouldFlipStatus);
    }
}
