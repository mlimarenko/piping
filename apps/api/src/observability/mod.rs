//! Observability primitives: tracing, structured logging, metrics, log scrubbing and noise policy.
mod attributes;
mod events;
mod log_context;
mod logging;
pub mod metrics;
mod noise_policy;
mod scrubbing;
mod tracing;

pub use attributes::{
    set_outbound_event_id, set_request_id, set_session_id, set_source_id, set_sync_outcome,
    set_user_id, set_webhook_event_id,
};
pub use events::{CanonicalEvent, compose_event_name};
pub use log_context::LogContext;
pub use logging::{
    EventFields, LogLevel, NoiseBudgetMetrics, build_env_filter, build_event_record_for_contract,
    build_log_layer, canonical_event_key, current_noise_policy, log_connection_event,
    log_error_event, log_event, log_operation_completion, simulate_noise_budget,
};
pub use noise_policy::{NoiseBudgetProfile, NoiseOverflowMode, NoisePolicy, normalize_event_key};
pub use scrubbing::{scrub_context, scrub_text};
pub use tracing::{
    attach_parent_context_from_headers, current_trace_and_span_ids, http_request_span,
    init_tracing, instrument_span, is_otel_enabled, operation_span, proxy_call_span,
};

use crate::config;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use tracing_opentelemetry::MetricsLayer;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize observability (tracing, logging, metrics)
pub fn init_observability(
    settings: &config::Settings,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    metrics::init_metrics()?;
    let filter = build_env_filter(settings);
    let otel_providers = init_tracing(settings)?;
    let tracer_layer = otel_providers.tracer_provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer().with_tracer(provider.tracer("piping-backend"))
    });
    let metrics_layer = otel_providers
        .meter_provider
        .as_ref()
        .map(|provider| MetricsLayer::new(provider.clone()));
    let logs_layer = otel_providers.logger_provider.as_ref().map(|provider| {
        OpenTelemetryTracingBridge::new(provider).with_filter(logging::otel_log_export_filter())
    });

    let fmt_layer = build_log_layer(settings).with_filter(filter);
    tracing_subscriber::registry()
        .with(tracer_layer)
        .with(metrics_layer)
        .with(logs_layer)
        .with(fmt_layer)
        .init();

    if otel_providers.is_enabled() {
        ::tracing::info!(
            monotonic_counter.piping.telemetry.events = 1_u64,
            traces = otel_providers.traces_enabled,
            metrics = otel_providers.metrics_enabled,
            logs = otel_providers.logs_enabled,
            "observability: enabled"
        );
    }
    Ok(())
}

/// Get the current tracing span
pub fn current_span() -> ::tracing::Span {
    ::tracing::Span::current()
}

/// Set an attribute on the current span
pub fn set_attribute(key: &str, value: &str) {
    ::tracing::Span::current().record(key, value);
}
