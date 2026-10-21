//! Retry scheduling for perpetual background workers.
//!
//! A worker that polls an external dependency on a fixed-rate
//! `tokio::time::interval` keeps hammering that dependency at full rate while
//! it is down, and — if it logs per attempt — turns a dependency outage into a
//! disk-space outage. `MissedTickBehavior` does not help: it only chooses which
//! tick of an existing fixed schedule comes next, and can even yield an
//! interval shorter than one period.
//!
//! This type replaces the ticker with an adaptive delay: the normal poll
//! interval while healthy, capped exponential backoff with jitter while
//! failing, reset on the first success.
//!
//! It is the single backoff vocabulary for perpetual workers. It is
//! deliberately *not* used for playbook job dispatch, which is persisted,
//! finite-attempt scheduling with a different lifecycle
//! (`PLAYBOOK_DISPATCH_RETRY_BACKOFF_SECONDS`).

use chrono::Utc;

/// Exponent cap: `2^10` multiples of the base delay before the ceiling applies.
const MAX_BACKOFF_EXPONENT: u32 = 10;

/// Retry timing bounds for a perpetual worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkerRetryConfig {
    /// First retry delay after a single failure.
    pub base_ms: u64,
    /// Ceiling for the exponential delay.
    pub max_ms: u64,
    /// Upper bound of the random additive jitter; `0` disables jitter.
    pub jitter_ms: u64,
}

impl WorkerRetryConfig {
    /// Defaults for an infrastructure poller: 5s → 5min ceiling.
    ///
    /// A dependency outage therefore costs at most ~12 attempts per hour per
    /// worker once the ceiling is reached, instead of one per poll interval.
    pub(super) const POLLER: Self = Self {
        base_ms: 5_000,
        max_ms: 300_000,
        jitter_ms: 1_000,
    };
}

/// Tracks consecutive failures and computes the next sleep for a worker loop.
#[derive(Debug, Clone)]
pub(super) struct WorkerBackoff {
    idle_interval_ms: u64,
    retry: WorkerRetryConfig,
    consecutive_failures: u32,
    next_sleep_ms: u64,
}

impl WorkerBackoff {
    /// Build a backoff whose first poll runs immediately.
    ///
    /// This preserves the startup semantics of `tokio::time::interval`, whose
    /// first tick completes without delay.
    pub(super) fn starting_immediately(idle_interval_ms: u64, retry: WorkerRetryConfig) -> Self {
        Self {
            idle_interval_ms,
            retry,
            consecutive_failures: 0,
            next_sleep_ms: 0,
        }
    }

    /// Build a backoff that waits one full interval before the first poll.
    pub(super) fn starting_after_interval(idle_interval_ms: u64, retry: WorkerRetryConfig) -> Self {
        Self {
            idle_interval_ms,
            retry,
            consecutive_failures: 0,
            next_sleep_ms: idle_interval_ms,
        }
    }

    /// Delay to sleep before the next attempt.
    pub(super) const fn next_sleep_ms(&self) -> u64 {
        self.next_sleep_ms
    }

    /// Consecutive failures observed, reset by [`Self::on_success`].
    pub(super) const fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Whether the previous attempt failed — used to emit a single recovery
    /// event on the first success after an outage.
    pub(super) const fn was_failing(&self) -> bool {
        self.consecutive_failures > 0
    }

    /// Record a success: clear the failure streak, return to normal cadence.
    pub(super) const fn on_success(&mut self) {
        self.consecutive_failures = 0;
        self.next_sleep_ms = self.idle_interval_ms;
    }

    /// Record a failure and schedule the next attempt with capped exponential
    /// backoff plus jitter. Returns the delay just scheduled.
    pub(super) fn on_failure(&mut self) -> u64 {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let exponent = self
            .consecutive_failures
            .saturating_sub(1)
            .min(MAX_BACKOFF_EXPONENT);
        let backoff_ms = self
            .retry
            .base_ms
            .saturating_mul(2_u64.saturating_pow(exponent))
            .min(self.retry.max_ms);
        self.next_sleep_ms = backoff_ms.saturating_add(self.jitter_ms());
        self.next_sleep_ms
    }

    /// Additive jitter so restarted replicas do not retry in lockstep.
    ///
    /// Reads the clock and hands the sample to [`jitter_from_sample`]; the
    /// arithmetic itself stays a pure function so it can be tested at its
    /// boundaries instead of only within a range.
    fn jitter_ms(&self) -> u64 {
        jitter_from_sample(
            self.retry.jitter_ms,
            u64::from(Utc::now().timestamp_subsec_nanos()),
        )
    }
}

/// Fold an arbitrary sample into `0..=jitter_ms`.
///
/// The sample is the wall-clock nanosecond fraction rather than an RNG draw —
/// the crate has no `rand` and this is not a security-relevant value. Taking it
/// as a parameter is deliberate: the licence-refresh worker's jitter used to be
/// computed inline from `Utc::now()`, which made the arithmetic untestable, and
/// that was fixed once already. Keeping the clock in the caller preserves it.
fn jitter_from_sample(jitter_ms: u64, sample: u64) -> u64 {
    if jitter_ms == 0 {
        return 0;
    }
    sample % (jitter_ms + 1)
}

#[cfg(test)]
mod tests {
    use super::{WorkerBackoff, WorkerRetryConfig, jitter_from_sample};

    const NO_JITTER: WorkerRetryConfig = WorkerRetryConfig {
        base_ms: 1_000,
        max_ms: 60_000,
        jitter_ms: 0,
    };

    #[test]
    fn starts_immediately_when_requested() {
        let backoff = WorkerBackoff::starting_immediately(30_000, NO_JITTER);
        assert_eq!(backoff.next_sleep_ms(), 0);
    }

    #[test]
    fn starts_after_one_interval_when_requested() {
        let backoff = WorkerBackoff::starting_after_interval(30_000, NO_JITTER);
        assert_eq!(backoff.next_sleep_ms(), 30_000);
    }

    #[test]
    fn failure_delays_grow_exponentially() {
        let mut backoff = WorkerBackoff::starting_immediately(30_000, NO_JITTER);
        assert_eq!(backoff.on_failure(), 1_000);
        assert_eq!(backoff.on_failure(), 2_000);
        assert_eq!(backoff.on_failure(), 4_000);
        assert_eq!(backoff.on_failure(), 8_000);
        assert_eq!(backoff.consecutive_failures(), 4);
    }

    #[test]
    fn failure_delay_is_capped_at_max() {
        let mut backoff = WorkerBackoff::starting_immediately(30_000, NO_JITTER);
        for _ in 0..20 {
            backoff.on_failure();
        }
        assert_eq!(backoff.next_sleep_ms(), 60_000);
    }

    #[test]
    fn success_resets_streak_and_restores_idle_interval() {
        let mut backoff = WorkerBackoff::starting_immediately(30_000, NO_JITTER);
        backoff.on_failure();
        backoff.on_failure();
        assert!(backoff.was_failing());

        backoff.on_success();

        assert_eq!(backoff.consecutive_failures(), 0);
        assert_eq!(backoff.next_sleep_ms(), 30_000);
        assert!(!backoff.was_failing());
    }

    #[test]
    fn jitter_stays_within_configured_bound() {
        let retry = WorkerRetryConfig {
            base_ms: 1_000,
            max_ms: 60_000,
            jitter_ms: 500,
        };
        let mut backoff = WorkerBackoff::starting_immediately(30_000, retry);
        // Jitter is additive only: each delay must sit within [exact, exact+jitter]
        // of the un-jittered exponential value for that attempt.
        for attempt in 1..=20_u32 {
            let delay = backoff.on_failure();
            let exact = 1_000_u64
                .saturating_mul(2_u64.pow(attempt.saturating_sub(1).min(10)))
                .min(60_000);
            assert!(
                (exact..=exact + 500).contains(&delay),
                "attempt {attempt}: delay {delay} outside [{exact}, {}]",
                exact + 500
            );
        }
    }

    #[test]
    fn poller_defaults_cap_at_five_minutes() {
        let mut backoff = WorkerBackoff::starting_immediately(60_000, WorkerRetryConfig::POLLER);
        for _ in 0..30 {
            backoff.on_failure();
        }
        assert!(backoff.next_sleep_ms() <= 300_000 + WorkerRetryConfig::POLLER.jitter_ms);
        assert!(backoff.next_sleep_ms() >= 300_000);
    }

    /// The boundary the range assertions above cannot reach: a sample equal to
    /// the configured jitter yields the full amount, and the next sample wraps
    /// to zero rather than overflowing the bound. Carried over from the
    /// licence-refresh schedule this backoff replaced, which tested exactly
    /// this — the property is why the sample is a parameter and not a clock
    /// read hidden inside the function.
    #[test]
    fn jitter_folds_the_sample_into_the_configured_bound() {
        assert_eq!(jitter_from_sample(250, 250), 250);
        assert_eq!(jitter_from_sample(250, 251), 0);
        assert_eq!(jitter_from_sample(250, 0), 0);
        assert_eq!(jitter_from_sample(0, 12_345), 0, "no jitter configured");
    }
}
