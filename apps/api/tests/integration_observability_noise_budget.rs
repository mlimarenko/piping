//! Integration test target.

use piping_backend::observability::{
    EventFields, LogLevel, NoiseOverflowMode, NoisePolicy, simulate_noise_budget,
};

#[test]
fn summarize_mode_suppresses_burst_and_emits_periodic_summaries() {
    let policy = NoisePolicy {
        window_ms: 30_000,
        max_events_per_window: 10,
        overflow_mode: NoiseOverflowMode::Summarize,
    };

    let metrics = simulate_noise_budget(
        LogLevel::Info,
        "rest.route.received",
        EventFields::default()
            .with_operation("rest.route.inventory_hosts")
            .with_noise_key("rest.route.inventory_hosts.received"),
        policy,
        37,
    );

    assert_eq!(metrics.emitted, 10);
    assert_eq!(metrics.suppressed, 27);
    assert_eq!(metrics.summary_events, 2);
}

#[test]
fn suppress_mode_drops_overflow_without_summary_events() {
    let policy = NoisePolicy {
        window_ms: 30_000,
        max_events_per_window: 5,
        overflow_mode: NoiseOverflowMode::Suppress,
    };

    let metrics = simulate_noise_budget(
        LogLevel::Info,
        "rest.route.received",
        EventFields::default()
            .with_operation("rest.route.connections_entry_options")
            .with_noise_key("rest.route.connections_entry_options.received"),
        policy,
        20,
    );

    assert_eq!(metrics.emitted, 5);
    assert_eq!(metrics.suppressed, 15);
    assert_eq!(metrics.summary_events, 0);
}
