use std::collections::BTreeMap;

use crate::config::ObservabilityConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseOverflowMode {
    Off,
    Suppress,
    Summarize,
}

impl NoiseOverflowMode {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "suppress" => Self::Suppress,
            _ => Self::Summarize,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Suppress => "suppress",
            Self::Summarize => "summarize",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoisePolicy {
    pub window_ms: u64,
    pub max_events_per_window: u32,
    pub overflow_mode: NoiseOverflowMode,
}

impl NoisePolicy {
    pub fn from_observability(config: &ObservabilityConfig) -> Self {
        Self {
            window_ms: config.noise_window_ms,
            max_events_per_window: config.noise_max_events_per_window,
            overflow_mode: NoiseOverflowMode::parse(&config.noise_overflow_mode),
        }
    }
}

/// Event-specific repetitive-event budget overriding the global policy.
///
/// Unlike [`NoisePolicy`], the initial allowance and the aggregate cadence are
/// separate knobs. A retrying worker wants "emit the first, then one aggregate
/// per N suppressed", which a single `max_events_per_window` cannot express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoiseBudgetProfile {
    /// Length of the counting window.
    pub window_ms: u64,
    /// Events emitted at full severity before aggregation begins.
    pub burst: u32,
    /// Emit one aggregate per this many suppressed events.
    pub summary_every: u32,
}

impl NoiseBudgetProfile {
    /// Profile for a perpetual worker retrying an unavailable dependency.
    ///
    /// Emits the first failure at full severity, then one aggregate per ten
    /// suppressed failures. Paired with worker retry backoff this bounds an
    /// outage to a handful of lines per hour instead of one line per attempt.
    pub const RETRYING_WORKER_FAILURE: Self = Self {
        window_ms: 300_000,
        burst: 1,
        summary_every: 10,
    };
}

/// Resolved budget parameters shared by the global and event-specific paths.
///
/// Both [`NoisePolicy`] and [`NoiseBudgetProfile`] collapse into this so there
/// is exactly one budget evaluator rather than two parallel limiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EffectiveBudget {
    pub window_ms: u64,
    pub burst: u32,
    pub summary_every: u32,
    pub overflow_mode: NoiseOverflowMode,
}

impl EffectiveBudget {
    /// Global budget: one knob drives both allowance and cadence.
    pub(super) const fn from_policy(policy: NoisePolicy) -> Self {
        Self {
            window_ms: policy.window_ms,
            burst: policy.max_events_per_window,
            summary_every: policy.max_events_per_window,
            overflow_mode: policy.overflow_mode,
        }
    }

    /// Event-specific profile. Always summarize: an aggregate must remain
    /// observable, so a caller cannot silently drop the failures it profiles.
    pub(super) const fn from_profile(profile: NoiseBudgetProfile) -> Self {
        Self {
            window_ms: profile.window_ms,
            burst: profile.burst,
            summary_every: profile.summary_every,
            overflow_mode: NoiseOverflowMode::Summarize,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NoiseSummaryPayload {
    pub event_key: String,
    pub suppressed_count: u32,
    pub window_ms: u64,
    pub overflow_mode: NoiseOverflowMode,
}

impl NoiseSummaryPayload {
    pub(super) fn new(
        event_key: impl Into<String>,
        suppressed_count: u32,
        budget: EffectiveBudget,
    ) -> Self {
        Self {
            event_key: event_key.into(),
            suppressed_count,
            window_ms: budget.window_ms,
            overflow_mode: budget.overflow_mode,
        }
    }

    pub(super) fn to_context_fields(&self) -> BTreeMap<String, String> {
        let mut context = BTreeMap::new();
        context.insert("event_key".to_string(), self.event_key.clone());
        context.insert(
            "suppressed_count".to_string(),
            self.suppressed_count.to_string(),
        );
        context.insert("window_ms".to_string(), self.window_ms.to_string());
        context.insert(
            "overflow_mode".to_string(),
            self.overflow_mode.as_str().to_string(),
        );
        context
    }
}

/// Build a stable event key used for repetitive-event budgeting.
pub fn normalize_event_key(event: &str) -> String {
    event.trim().to_ascii_lowercase().replace([' ', '/'], "_")
}

#[cfg(test)]
mod tests {
    use super::{
        EffectiveBudget, NoiseBudgetProfile, NoiseOverflowMode, NoisePolicy, NoiseSummaryPayload,
        normalize_event_key,
    };
    use crate::config::ObservabilityConfig;

    #[test]
    fn policy_budget_uses_one_knob_for_allowance_and_cadence() {
        let budget = EffectiveBudget::from_policy(NoisePolicy {
            window_ms: 60_000,
            max_events_per_window: 100,
            overflow_mode: NoiseOverflowMode::Summarize,
        });

        assert_eq!(budget.burst, 100);
        assert_eq!(budget.summary_every, 100);
        assert_eq!(budget.window_ms, 60_000);
    }

    #[test]
    fn profile_budget_separates_allowance_from_cadence() {
        let budget = EffectiveBudget::from_profile(NoiseBudgetProfile::RETRYING_WORKER_FAILURE);

        // The whole point of the profile: emit one, then aggregate — which a
        // single `max_events_per_window` cannot express.
        assert_eq!(budget.burst, 1);
        assert_eq!(budget.summary_every, 10);
        assert_ne!(budget.burst, budget.summary_every);
    }

    #[test]
    fn profile_budget_always_summarizes_never_silently_drops() {
        let budget = EffectiveBudget::from_profile(NoiseBudgetProfile::RETRYING_WORKER_FAILURE);
        assert_eq!(budget.overflow_mode, NoiseOverflowMode::Summarize);
    }

    #[test]
    fn parse_overflow_mode_accepts_known_values() {
        assert_eq!(NoiseOverflowMode::parse("off"), NoiseOverflowMode::Off);
        assert_eq!(
            NoiseOverflowMode::parse("suppress"),
            NoiseOverflowMode::Suppress
        );
        assert_eq!(
            NoiseOverflowMode::parse("summarize"),
            NoiseOverflowMode::Summarize
        );
    }

    #[test]
    fn parse_overflow_mode_defaults_to_summarize() {
        assert_eq!(
            NoiseOverflowMode::parse("unexpected"),
            NoiseOverflowMode::Summarize
        );
    }

    #[test]
    fn normalize_event_key_trims_and_lowercases() {
        assert_eq!(normalize_event_key(" REST/Route "), "rest_route");
    }

    #[test]
    fn from_observability_preserves_window_and_limit_values() {
        let config = ObservabilityConfig {
            log_level: "INFO".to_string(),
            log_output_format: "json".to_string(),
            noise_window_ms: 15_000,
            noise_max_events_per_window: 42,
            noise_overflow_mode: "suppress".to_string(),
            otel_enabled: true,
            otel_exporter_otlp_endpoint: String::new(),
        };

        let policy = NoisePolicy::from_observability(&config);
        assert_eq!(policy.window_ms, 15_000);
        assert_eq!(policy.max_events_per_window, 42);
        assert_eq!(policy.overflow_mode, NoiseOverflowMode::Suppress);
    }

    #[test]
    fn summary_payload_builds_context_fields() {
        let payload = NoiseSummaryPayload::new(
            "rest_route_received",
            15,
            EffectiveBudget::from_policy(NoisePolicy {
                window_ms: 60_000,
                max_events_per_window: 100,
                overflow_mode: NoiseOverflowMode::Summarize,
            }),
        );
        let context = payload.to_context_fields();
        assert_eq!(
            context.get("event_key").map(String::as_str),
            Some("rest_route_received")
        );
        assert_eq!(
            context.get("suppressed_count").map(String::as_str),
            Some("15")
        );
        assert_eq!(context.get("window_ms").map(String::as_str), Some("60000"));
        assert_eq!(
            context.get("overflow_mode").map(String::as_str),
            Some("summarize")
        );
    }
}
