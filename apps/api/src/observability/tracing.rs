use crate::config::Settings;
use axum::http::HeaderMap;
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::propagation::TextMapCompositePropagator;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::future::Future;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{Instrument, Span, field};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use url::Url;

static TRACING_INITIALIZED: OnceLock<bool> = OnceLock::new();
static TRACER_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();
static METER_PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();
static LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();
static FALLBACK_INSTANCE_ID: OnceLock<String> = OnceLock::new();
const OTEL_EXPORTER_OTLP_PROTOCOL_GRPC: &str = "grpc";
const OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF: &str = "http/protobuf";
const OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";
const OTEL_EXPORTER_OTLP_METRICS_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT";
const OTEL_EXPORTER_OTLP_LOGS_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT";
const OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE: &str =
    "OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE";
const OTEL_TRACES_EXPORTER: &str = "OTEL_TRACES_EXPORTER";
const OTEL_METRICS_EXPORTER: &str = "OTEL_METRICS_EXPORTER";
const OTEL_LOGS_EXPORTER: &str = "OTEL_LOGS_EXPORTER";
const DEFAULT_OTEL_METRIC_INTERVAL_SECONDS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OtlpProtocol {
    Grpc,
    HttpProtobuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OtlpSignal {
    Traces,
    Metrics,
    Logs,
}

impl OtlpSignal {
    const fn endpoint_env(self) -> &'static str {
        match self {
            Self::Traces => OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
            Self::Metrics => OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
            Self::Logs => OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
        }
    }

    const fn exporter_env(self) -> &'static str {
        match self {
            Self::Traces => OTEL_TRACES_EXPORTER,
            Self::Metrics => OTEL_METRICS_EXPORTER,
            Self::Logs => OTEL_LOGS_EXPORTER,
        }
    }

    const fn http_path(self) -> &'static str {
        match self {
            Self::Traces => "/v1/traces",
            Self::Metrics => "/v1/metrics",
            Self::Logs => "/v1/logs",
        }
    }
}

#[derive(Debug, Default)]
pub struct OtelProviders {
    pub(crate) tracer_provider: Option<SdkTracerProvider>,
    pub(crate) meter_provider: Option<SdkMeterProvider>,
    pub(crate) logger_provider: Option<SdkLoggerProvider>,
    pub(crate) traces_enabled: bool,
    pub(crate) metrics_enabled: bool,
    pub(crate) logs_enabled: bool,
}

impl OtelProviders {
    pub(crate) fn is_enabled(&self) -> bool {
        self.tracer_provider.is_some()
            || self.meter_provider.is_some()
            || self.logger_provider.is_some()
    }
}

/// Initialize OpenTelemetry tracing with OTLP exporter
pub fn init_tracing(
    settings: &Settings,
) -> Result<OtelProviders, Box<dyn std::error::Error + Send + Sync>> {
    // Prevent double initialization
    if TRACING_INITIALIZED.get().is_some() {
        return Ok(OtelProviders::default());
    }

    let propagator = TextMapCompositePropagator::new(vec![
        Box::new(BaggagePropagator::new()),
        Box::new(TraceContextPropagator::new()),
    ]);
    global::set_text_map_propagator(propagator);

    if is_otel_disabled(settings) {
        if TRACING_INITIALIZED.set(true).is_err() {
            return Ok(OtelProviders::default());
        }
        return Ok(OtelProviders::default());
    }

    let endpoint = resolve_otel_endpoint(settings);
    if endpoint.is_empty() {
        if TRACING_INITIALIZED.set(true).is_err() {
            return Ok(OtelProviders::default());
        }
        return Ok(OtelProviders::default());
    }

    let protocol = resolve_otel_protocol();
    let traces_enabled = signal_enabled(OtlpSignal::Traces);
    let metrics_enabled = signal_enabled(OtlpSignal::Metrics);
    let logs_enabled = signal_enabled(OtlpSignal::Logs);

    let tracer_provider = traces_enabled
        .then(|| build_tracer_provider(endpoint.as_str(), protocol))
        .transpose()?;
    let meter_provider = metrics_enabled
        .then(|| build_meter_provider(endpoint.as_str(), protocol))
        .transpose()?;
    let logger_provider = logs_enabled
        .then(|| build_logger_provider(endpoint.as_str(), protocol))
        .transpose()?;

    if let Some(provider) = tracer_provider.as_ref() {
        global::set_tracer_provider(provider.clone());
        let _ = TRACER_PROVIDER.set(provider.clone());
    }
    if let Some(provider) = meter_provider.as_ref() {
        global::set_meter_provider(provider.clone());
        let _ = METER_PROVIDER.set(provider.clone());
    }
    if let Some(provider) = logger_provider.as_ref() {
        let _ = LOGGER_PROVIDER.set(provider.clone());
    }

    if TRACING_INITIALIZED.set(true).is_err() {
        return Ok(OtelProviders::default());
    }

    Ok(OtelProviders {
        tracer_provider,
        meter_provider,
        logger_provider,
        traces_enabled,
        metrics_enabled,
        logs_enabled,
    })
}

fn build_tracer_provider(
    endpoint: &str,
    protocol: OtlpProtocol,
) -> Result<SdkTracerProvider, Box<dyn std::error::Error + Send + Sync>> {
    let signal_endpoint = resolved_signal_endpoint(endpoint, protocol, OtlpSignal::Traces);
    let exporter = match protocol {
        OtlpProtocol::Grpc => SpanExporter::builder()
            .with_tonic()
            .with_endpoint(signal_endpoint)
            .with_protocol(Protocol::Grpc)
            .with_timeout(Duration::from_secs(5))
            .build()?,
        OtlpProtocol::HttpProtobuf => SpanExporter::builder()
            .with_http()
            .with_endpoint(signal_endpoint)
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(Duration::from_secs(5))
            .build()?,
    };

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(build_otel_resource())
        .build())
}

fn build_meter_provider(
    endpoint: &str,
    protocol: OtlpProtocol,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error + Send + Sync>> {
    let signal_endpoint = resolved_signal_endpoint(endpoint, protocol, OtlpSignal::Metrics);
    let exporter = match protocol {
        OtlpProtocol::Grpc => opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(signal_endpoint)
            .with_protocol(Protocol::Grpc)
            .with_temporality(resolve_metrics_temporality())
            .with_timeout(Duration::from_secs(5))
            .build()?,
        OtlpProtocol::HttpProtobuf => opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_endpoint(signal_endpoint)
            .with_protocol(Protocol::HttpBinary)
            .with_temporality(resolve_metrics_temporality())
            .with_timeout(Duration::from_secs(5))
            .build()?,
    };
    let reader = PeriodicReader::builder(exporter)
        .with_interval(Duration::from_secs(DEFAULT_OTEL_METRIC_INTERVAL_SECONDS))
        .build();
    Ok(SdkMeterProvider::builder()
        .with_resource(build_otel_resource())
        .with_reader(reader)
        .build())
}

fn build_logger_provider(
    endpoint: &str,
    protocol: OtlpProtocol,
) -> Result<SdkLoggerProvider, Box<dyn std::error::Error + Send + Sync>> {
    let signal_endpoint = resolved_signal_endpoint(endpoint, protocol, OtlpSignal::Logs);
    let exporter = match protocol {
        OtlpProtocol::Grpc => opentelemetry_otlp::LogExporter::builder()
            .with_tonic()
            .with_endpoint(signal_endpoint)
            .with_protocol(Protocol::Grpc)
            .with_timeout(Duration::from_secs(5))
            .build()?,
        OtlpProtocol::HttpProtobuf => opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_endpoint(signal_endpoint)
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(Duration::from_secs(5))
            .build()?,
    };
    Ok(SdkLoggerProvider::builder()
        .with_resource(build_otel_resource())
        .with_batch_exporter(exporter)
        .build())
}

/// Returns true when OpenTelemetry tracing layer is configured and active.
pub fn is_otel_enabled() -> bool {
    TRACER_PROVIDER.get().is_some()
        || METER_PROVIDER.get().is_some()
        || LOGGER_PROVIDER.get().is_some()
}

pub fn http_request_span(
    request_id: &str,
    method: &str,
    path: &str,
    client_ip: Option<&str>,
) -> Span {
    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        correlation_id = %request_id,
        method = %method,
        path = %path,
        client_ip = %client_ip.unwrap_or("unknown"),
        trace_id = field::Empty,
        span_id = field::Empty
    );
    record_trace_and_span_fields(&span);
    span
}

pub fn attach_parent_context_from_headers(span: &Span, _headers: &HeaderMap) {
    if is_otel_enabled() && span.context().span().span_context().is_valid() {
        tracing::debug!("OpenTelemetry span already started before parent context extraction");
    }

    record_trace_and_span_fields(span);
}

fn record_trace_and_span_fields(span: &Span) {
    let otel_context = span.context();
    let span_context = otel_context.span().span_context().clone();
    if span_context.is_valid() {
        span.record("trace_id", field::display(span_context.trace_id()));
        span.record("span_id", field::display(span_context.span_id()));
        return;
    }

    if let Some(local_span_id) = local_span_id(span) {
        span.record("span_id", field::display(local_span_id));
    }
}

pub fn operation_span(name: &str, fields: &[(&str, String)]) -> Span {
    let span = tracing::info_span!("operation", name = %name);
    for (key, value) in fields {
        span.record(*key, field::display(value));
    }
    span
}

pub fn proxy_call_span(operation: &str, fields: &[(&str, String)]) -> Span {
    let span = tracing::info_span!("proxy_call", operation = %operation);
    for (key, value) in fields {
        span.record(*key, field::display(value));
    }
    span
}

pub async fn instrument_span<T, F>(span: Span, future: F) -> T
where
    F: Future<Output = T>,
{
    future.instrument(span).await
}

pub fn current_trace_and_span_ids() -> (Option<String>, Option<String>) {
    let current_span = Span::current();
    let otel_context = current_span.context();
    let span_context = otel_context.span().span_context().clone();
    if !span_context.is_valid() {
        return (None, local_span_id(&current_span));
    }
    (
        Some(span_context.trace_id().to_string()),
        Some(span_context.span_id().to_string()),
    )
}

fn local_span_id(span: &Span) -> Option<String> {
    span.id().map(|id| format!("{:016x}", id.into_u64()))
}

fn resolve_otel_endpoint(settings: &Settings) -> String {
    std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .unwrap_or_else(|| settings.observability.otel_exporter_otlp_endpoint.clone())
        .trim()
        .to_string()
}

fn resolve_otel_protocol() -> OtlpProtocol {
    let raw = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
        .ok()
        .unwrap_or_default();
    parse_otel_protocol(Some(raw.trim()))
}

fn parse_otel_protocol(raw_protocol: Option<&str>) -> OtlpProtocol {
    let normalized = raw_protocol
        .unwrap_or(OTEL_EXPORTER_OTLP_PROTOCOL_GRPC)
        .trim()
        .to_ascii_lowercase();
    match normalized.as_str() {
        "" | OTEL_EXPORTER_OTLP_PROTOCOL_GRPC => OtlpProtocol::Grpc,
        OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF | "http/proto" | "protobuf" => {
            OtlpProtocol::HttpProtobuf
        }
        unknown => {
            tracing::warn!(
                protocol = %unknown,
                "unsupported OTEL_EXPORTER_OTLP_PROTOCOL, falling back to grpc"
            );
            OtlpProtocol::Grpc
        }
    }
}

fn resolve_metrics_temporality() -> Temporality {
    let Some(raw_value) = read_env_trimmed(OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE)
    else {
        return Temporality::Cumulative;
    };
    match raw_value.to_ascii_lowercase().as_str() {
        "cumulative" => Temporality::Cumulative,
        "delta" => Temporality::Delta,
        "lowmemory" | "low_memory" => Temporality::LowMemory,
        _ => {
            tracing::warn!(
                temporality = %raw_value,
                "unsupported OTEL metric temporality, falling back to cumulative"
            );
            Temporality::Cumulative
        }
    }
}

fn signal_enabled(signal: OtlpSignal) -> bool {
    let Some(raw_value) = read_env_trimmed(signal.exporter_env()) else {
        return true;
    };
    match raw_value.to_ascii_lowercase().as_str() {
        "otlp" => true,
        "none" => false,
        _ => {
            tracing::warn!(
                exporter = %raw_value,
                signal = ?signal,
                "unsupported OTEL exporter, using otlp"
            );
            true
        }
    }
}

fn resolved_signal_endpoint(endpoint: &str, protocol: OtlpProtocol, signal: OtlpSignal) -> String {
    if let Some(signal_endpoint) = read_env_trimmed(signal.endpoint_env()) {
        return signal_endpoint;
    }
    match protocol {
        OtlpProtocol::Grpc => endpoint.to_string(),
        OtlpProtocol::HttpProtobuf => append_http_signal_path(endpoint, signal),
    }
}

fn append_http_signal_path(endpoint: &str, signal: OtlpSignal) -> String {
    let Ok(mut url) = Url::parse(endpoint) else {
        return endpoint.to_string();
    };
    if matches!(url.path(), "" | "/") {
        url.set_path(signal.http_path());
    }
    url.to_string()
}

fn build_otel_resource() -> Resource {
    let mut attributes = read_env_trimmed("OTEL_RESOURCE_ATTRIBUTES")
        .map(|raw_attributes| parse_resource_attributes(&raw_attributes))
        .unwrap_or_default();
    let service_name = resolve_service_name();
    let instance_id = resolve_instance_id(&service_name);

    if !has_resource_key(&attributes, "service.name") {
        attributes.push(KeyValue::new("service.name", service_name));
    }
    if !has_resource_key(&attributes, "service.version") {
        attributes.push(KeyValue::new("service.version", crate::PRODUCT_VERSION));
    }
    if !has_resource_key(&attributes, "service.instance.id") {
        attributes.push(KeyValue::new("service.instance.id", instance_id.clone()));
    }
    if !has_resource_key(&attributes, "piping.instance.id") {
        attributes.push(KeyValue::new("piping.instance.id", instance_id));
    }

    Resource::builder().with_attributes(attributes).build()
}

fn has_resource_key(attributes: &[KeyValue], key: &str) -> bool {
    attributes.iter().any(|entry| entry.key.as_str() == key)
}

fn resolve_service_name() -> String {
    read_env_trimmed("OTEL_SERVICE_NAME").unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string())
}

fn resolve_instance_id(service_name: &str) -> String {
    if let Some(explicit) = read_env_trimmed("OTEL_SERVICE_INSTANCE_ID")
        .or_else(|| read_env_trimmed("PIPING_OBSERVABILITY__OTEL_INSTANCE_ID"))
    {
        return explicit;
    }
    read_env_trimmed("HOSTNAME")
        .or_else(read_machine_id)
        .map(|host_id| format!("{service_name}:{host_id}"))
        .unwrap_or_else(|| {
            FALLBACK_INSTANCE_ID
                .get_or_init(|| {
                    format!(
                        "{}:runtime-{}-{}",
                        service_name,
                        std::process::id(),
                        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
                    )
                })
                .clone()
        })
}

fn read_machine_id() -> Option<String> {
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn is_otel_disabled(settings: &Settings) -> bool {
    if !settings.observability.otel_enabled {
        return true;
    }
    read_env_trimmed("OTEL_SDK_DISABLED")
        .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
        .unwrap_or(false)
}

fn read_env_trimmed(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_resource_attributes(raw_attributes: &str) -> Vec<KeyValue> {
    raw_attributes
        .split(',')
        .filter_map(|part| {
            let trimmed_part = part.trim();
            if trimmed_part.is_empty() {
                return None;
            }
            let mut kv = trimmed_part.splitn(2, '=');
            let key = kv.next().map(str::trim).filter(|value| !value.is_empty());
            let value = kv.next().map(str::trim).filter(|value| !value.is_empty());
            match (key, value) {
                (Some(key), Some(value)) => Some(parse_resource_attribute_value(key, value)),
                _ => {
                    tracing::warn!(
                        attribute = %trimmed_part,
                        "invalid OTEL_RESOURCE_ATTRIBUTES entry, expected key=value"
                    );
                    None
                }
            }
        })
        .collect()
}

fn parse_resource_attribute_value(key: &str, value: &str) -> KeyValue {
    if let Ok(parsed_bool) = bool::from_str(value) {
        return KeyValue::new(key.to_string(), parsed_bool);
    }
    if let Ok(parsed_i64) = i64::from_str(value) {
        return KeyValue::new(key.to_string(), parsed_i64);
    }
    if let Ok(parsed_f64) = f64::from_str(value) {
        return KeyValue::new(key.to_string(), parsed_f64);
    }
    KeyValue::new(key.to_string(), value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF, OtlpProtocol, local_span_id,
        parse_otel_protocol, parse_resource_attributes, resolve_instance_id,
    };
    use tracing_subscriber::registry;

    #[test]
    fn local_span_id_has_hex_format() {
        tracing::subscriber::with_default(registry(), || {
            let span = tracing::info_span!("test_local_span_id");
            let span_id = local_span_id(&span).expect("span id");
            assert_eq!(span_id.len(), 16);
            assert!(span_id.chars().all(|char| char.is_ascii_hexdigit()));
        });
    }

    #[test]
    fn parse_resource_attributes_skips_invalid_pairs() {
        let attrs = parse_resource_attributes(
            "deployment.environment.name=prod,service.namespace=piping,broken,also_broken=",
        );
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].key.as_str(), "deployment.environment.name");
        assert_eq!(attrs[1].key.as_str(), "service.namespace");
    }

    #[test]
    fn parse_otel_protocol_uses_grpc_as_default() {
        let protocol = parse_otel_protocol(None);
        assert_eq!(protocol, OtlpProtocol::Grpc);
    }

    #[test]
    fn parse_otel_protocol_supports_http_protobuf() {
        let protocol = parse_otel_protocol(Some(OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF));
        assert_eq!(protocol, OtlpProtocol::HttpProtobuf);
    }

    #[test]
    fn parse_otel_protocol_falls_back_for_unknown_value() {
        let protocol = parse_otel_protocol(Some("made-up"));
        assert_eq!(protocol, OtlpProtocol::Grpc);
    }

    #[test]
    fn resolve_instance_id_returns_non_empty_value() {
        let instance_id = resolve_instance_id("piping-test");
        assert!(!instance_id.trim().is_empty());
    }
}
