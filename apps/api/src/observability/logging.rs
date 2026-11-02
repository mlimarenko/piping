//! Tracing-subscriber setup and structured event logging: environment filter, JSON and text rendering, scrubbing and noise budget.
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use serde_json::{Map, Value};
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, layer::Layer, registry::LookupSpan};

use super::log_context::LogContext;
use super::noise_policy::{
    EffectiveBudget, NoiseBudgetProfile, NoiseOverflowMode, NoisePolicy, NoiseSummaryPayload,
    normalize_event_key,
};
use super::scrubbing::{scrub_context, scrub_text};
use crate::config::Settings;

mod text_renderer;

const THIRD_PARTY_LOG_FILTERS: &[&str] = &[
    "hyper=warn",
    "h2=warn",
    "tower_http=warn",
    "sqlx=warn",
    "tungstenite=warn",
    "tokio_tungstenite=warn",
    "opentelemetry_sdk=warn",
    "reqwest=warn",
    "rustls=warn",
];

const UNKNOWN: &str = "unknown";
const MILLIS_PER_SECOND: f64 = 1_000.0;
const MIN_NOISE_WINDOW_MS: u64 = 1;
const MIN_NOISE_SUMMARY_THRESHOLD: u32 = 1;
const UNKNOWN_ERROR_TYPE: &str = "unknown_error";
const UNKNOWN_ERROR_MESSAGE: &str = "unknown error";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogOutputFormat {
    Json,
    Text,
}

impl LogOutputFormat {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "text" => Some(Self::Text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

impl LogLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
            Self::Critical => "CRITICAL",
        }
    }

    const fn priority(self) -> u8 {
        match self {
            Self::Debug => 10,
            Self::Info => 20,
            Self::Warning => 30,
            Self::Error => 40,
            Self::Critical => 50,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventFields {
    pub message: Option<String>,
    pub request_id: Option<String>,
    pub correlation_id: Option<String>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub client_ip: Option<String>,
    pub operation: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub status: Option<String>,
    pub duration_ms: Option<f64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub noise_key: Option<String>,
    pub noise_budget: Option<NoiseBudgetProfile>,
    pub context: BTreeMap<String, String>,
}

impl EventFields {
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        let correlation_id = correlation_id.into();
        self.request_id = Some(correlation_id.clone());
        self.correlation_id = Some(correlation_id);
        self
    }

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_client_ip(mut self, client_ip: impl Into<String>) -> Self {
        self.client_ip = Some(client_ip.into());
        self
    }

    pub fn with_operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }

    pub fn with_resource(
        mut self,
        resource_type: impl Into<String>,
        resource_id: impl Into<String>,
    ) -> Self {
        self.resource_type = Some(resource_type.into());
        self.resource_id = Some(resource_id.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_duration_ms(mut self, duration_ms: f64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_error(
        mut self,
        error_type: impl Into<String>,
        error_message: impl Into<String>,
    ) -> Self {
        self.error_type = Some(error_type.into());
        self.error_message = Some(error_message.into());
        self
    }

    pub fn with_noise_key(mut self, noise_key: impl Into<String>) -> Self {
        self.noise_key = Some(noise_key.into());
        self
    }

    /// Override the global repetitive-event allowance and summary cadence.
    ///
    /// Pair this with [`Self::with_noise_key`] when one event name covers
    /// several independent failure sources, so unrelated failures do not share
    /// a counter. The key must be bounded — never derive it from an error
    /// message, host id, or request id.
    pub fn with_noise_budget(mut self, profile: NoiseBudgetProfile) -> Self {
        self.noise_budget = Some(profile);
        self
    }

    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    /// Context values must remain primitive for OTEL/log compatibility.
    pub fn with_json_context(mut self, key: impl Into<String>, value: &Value) -> Self {
        let normalized = match value {
            Value::Null => "null".to_string(),
            Value::Bool(v) => v.to_string(),
            Value::Number(v) => v.to_string(),
            Value::String(v) => v.clone(),
            _ => "[non-primitive]".to_string(),
        };
        self.context.insert(key.into(), normalized);
        self
    }
}

#[derive(Debug, Clone)]
struct NoiseBudgetState {
    window_started_at: Instant,
    emitted: u32,
    suppressed: u32,
}

impl NoiseBudgetState {
    fn new(now: Instant) -> Self {
        Self {
            window_started_at: now,
            emitted: 0,
            suppressed: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoiseDecision {
    Emit,
    Suppress,
    EmitSummaryOnly { suppressed_count: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoiseBudgetMetrics {
    pub emitted: u32,
    pub suppressed: u32,
    pub summary_events: u32,
}

static NOISE_POLICY: OnceLock<NoisePolicy> = OnceLock::new();
static NOISE_BUDGETS: Lazy<Mutex<HashMap<String, NoiseBudgetState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static ACTIVE_LOG_OUTPUT_FORMAT: OnceLock<LogOutputFormat> = OnceLock::new();
static TEXT_COLOR_ENABLED: OnceLock<bool> = OnceLock::new();
static ACTIVE_TEXT_MIN_LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

/// Build EnvFilter from config or environment.
pub fn build_env_filter(settings: &Settings) -> EnvFilter {
    if let Some(level) = read_env("PIPING_LOG_LEVEL") {
        let directives = with_third_party_filters(&level);
        return EnvFilter::try_new(directives).unwrap_or_else(|_| EnvFilter::new("info"));
    }

    let directives = with_third_party_filters(&settings.observability.log_level);
    EnvFilter::try_new(directives).unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Filter used by the OTLP log bridge to avoid exporting exporter internals.
pub(super) fn otel_log_export_filter() -> EnvFilter {
    EnvFilter::new(format!(
        "{},opentelemetry=off,opentelemetry_otlp=off,opentelemetry_sdk=off,tonic=off,h2=off,hyper=off,hyper_util=off,reqwest=off",
        with_third_party_filters("info")
    ))
}

/// Build logging layer with span context included.
pub fn build_log_layer<S>(settings: &Settings) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    set_noise_policy(settings);
    let output_format = resolve_log_output_format(settings);
    let use_ansi = should_use_ansi();
    let min_text_log_level = resolve_text_min_log_level(settings);
    let _ = ACTIVE_LOG_OUTPUT_FORMAT.set(output_format);
    let _ = TEXT_COLOR_ENABLED.set(matches!(output_format, LogOutputFormat::Text) && use_ansi);
    let _ = ACTIVE_TEXT_MIN_LOG_LEVEL.set(min_text_log_level);

    match output_format {
        LogOutputFormat::Json => Box::new(
            fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(true)
                .with_target(true)
                .with_level(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .with_ansi(false),
        ),
        LogOutputFormat::Text => Box::new(
            fmt::layer()
                .compact()
                .with_target(true)
                .with_level(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .with_ansi(use_ansi),
        ),
    }
}

/// Return the effective repetitive-event noise policy from typed settings.
pub fn current_noise_policy(settings: &Settings) -> NoisePolicy {
    NoisePolicy::from_observability(&settings.observability)
}

/// Normalize a canonical event key for repetitive-event budgeting.
pub fn canonical_event_key(event: &str) -> String {
    normalize_event_key(event)
}

fn event_noise_key(event: &str, fields: &EventFields) -> String {
    let base = canonical_event_key(event);
    if let Some(noise_key) = fields
        .noise_key
        .as_deref()
        .map(normalize_event_key)
        .filter(|value| !value.is_empty())
    {
        return format!("{base}:{noise_key}");
    }
    base
}

/// Log a canonical structured event.
pub fn log_event(level: LogLevel, event: &str, fields: EventFields) {
    let event_key = event_noise_key(event, &fields);
    let budget = effective_budget_for(level, &fields, effective_noise_policy());

    match evaluate_noise_budget(&event_key, budget) {
        NoiseDecision::Suppress => return,
        NoiseDecision::EmitSummaryOnly { suppressed_count } => {
            emit_noise_summary(level, &event_key, suppressed_count, budget);
            return;
        }
        NoiseDecision::Emit => {}
    }

    let merged_context = merge_context(&fields);
    let record = build_event_record(level, event, &fields, &merged_context);
    let payload = Value::Object(record);

    emit_payload(level, event, &fields, &merged_context, payload);
}

/// Log an error event with mandatory error metadata.
pub fn log_error_event(
    event: &str,
    error_type: impl Into<String>,
    error_message: impl Into<String>,
    fields: EventFields,
) {
    let fields = fields.with_error(error_type, error_message);
    log_event(LogLevel::Error, event, fields);
}

/// Log operation completion with computed duration.
pub fn log_operation_completion(
    level: LogLevel,
    event: &str,
    status: impl Into<String>,
    started_at: Instant,
    fields: EventFields,
) {
    let duration_ms = started_at.elapsed().as_secs_f64() * MILLIS_PER_SECOND;
    let fields = fields.with_status(status).with_duration_ms(duration_ms);
    log_event(level, event, fields);
}

/// Exposed for integration contract checks.
pub fn build_event_record_for_contract(
    level: LogLevel,
    event: &str,
    fields: EventFields,
) -> Map<String, Value> {
    let merged_context = merge_context(&fields);
    build_event_record(level, event, &fields, &merged_context)
}

/// Simulate repetitive-event budget behavior for diagnostics and tests.
pub fn simulate_noise_budget(
    level: LogLevel,
    event: &str,
    fields: EventFields,
    policy: NoisePolicy,
    attempts: u32,
) -> NoiseBudgetMetrics {
    let mut state = NoiseBudgetState::new(Instant::now());
    let mut metrics = NoiseBudgetMetrics {
        emitted: 0,
        suppressed: 0,
        summary_events: 0,
    };
    let _event_key = event_noise_key(event, &fields);
    let budget = effective_budget_for(level, &fields, policy);

    for _ in 0..attempts {
        match evaluate_noise_budget_for_state(budget, &mut state, Instant::now()) {
            NoiseDecision::Emit => metrics.emitted += 1,
            NoiseDecision::Suppress => metrics.suppressed += 1,
            NoiseDecision::EmitSummaryOnly { .. } => {
                metrics.suppressed += 1;
                metrics.summary_events += 1;
            }
        }
    }

    metrics
}

fn with_third_party_filters(base_level: &str) -> String {
    let trimmed = base_level.trim();
    let normalized_level = if trimmed.is_empty() {
        "info".to_string()
    } else if trimmed.contains('=') || trimmed.contains(',') {
        // Caller supplied full directive(s); keep as-is.
        trimmed.to_string()
    } else {
        // Normal level strings must be normalized for EnvFilter parsing.
        trimmed.to_ascii_lowercase()
    };
    let base_directive = if normalized_level.contains('=') || normalized_level.contains(',') {
        normalized_level
    } else {
        // Keep a global base level for robust parsing and also pin application target explicitly.
        // This avoids silent fallback to INFO when target directives are misparsed.
        format!("{normalized_level},piping_backend={normalized_level}")
    };
    format!("{base_directive},{}", THIRD_PARTY_LOG_FILTERS.join(","))
}

fn read_env(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn resolve_log_output_format(settings: &Settings) -> LogOutputFormat {
    if let Some(raw) = read_env("PIPING_LOG_OUTPUT_FORMAT") {
        return parse_log_output_format(&raw);
    }

    parse_log_output_format(&settings.observability.log_output_format)
}

fn parse_log_output_format(raw: &str) -> LogOutputFormat {
    if let Some(output_format) = LogOutputFormat::parse(raw) {
        return output_format;
    }

    log_event(
        LogLevel::Warning,
        "observability.logging.output_format.invalid",
        EventFields::default()
            .with_operation("logging_init")
            .with_context("provided_output_format", raw.to_string())
            .with_context("fallback_output_format", "json".to_string()),
    );
    LogOutputFormat::Json
}

fn resolve_text_min_log_level(settings: &Settings) -> LogLevel {
    if let Some(raw) = read_env("PIPING_LOG_LEVEL")
        && let Some(level) = parse_text_log_level(&raw)
    {
        return level;
    }

    parse_text_log_level(&settings.observability.log_level).unwrap_or(LogLevel::Info)
}

fn parse_text_log_level(raw: &str) -> Option<LogLevel> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(level) = LogLevel::parse(trimmed) {
        return Some(level);
    }

    let mut first_target_level = None;
    for token in trimmed
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(level) = LogLevel::parse(token) {
            return Some(level);
        }

        if let Some((target, level_raw)) = token.split_once('=') {
            let target = target.trim();
            if first_target_level.is_none() && target == "piping_backend" {
                first_target_level = LogLevel::parse(level_raw);
            }
        }
    }

    first_target_level
}

fn should_use_ansi() -> bool {
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }

    match read_env("PIPING_LOG_COLOR")
        .as_deref()
        .map(|value| value.to_ascii_lowercase())
    {
        Some(value) if value == "0" || value == "false" || value == "off" => false,
        Some(value) if value == "1" || value == "true" || value == "on" => true,
        Some(_) => true,
        None => true,
    }
}

fn set_noise_policy(settings: &Settings) {
    let _ = NOISE_POLICY.set(current_noise_policy(settings));
}

fn effective_noise_policy() -> NoisePolicy {
    NOISE_POLICY.get().copied().unwrap_or(NoisePolicy {
        window_ms: crate::config::DEFAULT_NOISE_FALLBACK_WINDOW_MS,
        max_events_per_window: crate::config::DEFAULT_NOISE_FALLBACK_MAX_EVENTS_PER_WINDOW,
        overflow_mode: NoiseOverflowMode::Summarize,
    })
}

/// The overflow mode that actually applies to `level`.
///
/// The repetition budget covers every level, errors included. Exempting them
/// reads like the careful choice — never drop a failure — and it is how a
/// database outage turns into a disk outage: the failures that repeat forever
/// are precisely the ones that flood. On 2026-08-06 the GRAD backend filled a
/// 59 GB disk with 7.8 GB of identical ERROR lines from workers retrying
/// against a database that was down, which left Postgres unable to write a
/// checkpoint, which kept the workers failing.
///
/// What must not happen is a failure vanishing silently, so at warning level
/// and above a configured `suppress` is raised to `summarize`: the operator
/// keeps getting a periodic line carrying the repeat count. `off` stays off —
/// that is an explicit choice to log everything.
fn effective_overflow_mode(level: LogLevel, configured: NoiseOverflowMode) -> NoiseOverflowMode {
    match level {
        LogLevel::Debug | LogLevel::Info => configured,
        LogLevel::Warning | LogLevel::Error | LogLevel::Critical => match configured {
            NoiseOverflowMode::Suppress => NoiseOverflowMode::Summarize,
            other => other,
        },
    }
}

/// Resolve the event-specific override or the global policy into one evaluator input.
fn effective_budget_for(
    level: LogLevel,
    fields: &EventFields,
    policy: NoisePolicy,
) -> EffectiveBudget {
    // An explicit global `off` asks for every original event. It bypasses an
    // event profile too: nothing is collapsed, so no summary can go missing.
    if matches!(
        effective_overflow_mode(level, policy.overflow_mode),
        NoiseOverflowMode::Off
    ) {
        return EffectiveBudget::from_policy(policy);
    }

    let mut budget = fields
        .noise_budget
        .map(EffectiveBudget::from_profile)
        .unwrap_or_else(|| EffectiveBudget::from_policy(policy));
    budget.overflow_mode = effective_overflow_mode(level, budget.overflow_mode);
    budget
}

fn evaluate_noise_budget(event_key: &str, budget: EffectiveBudget) -> NoiseDecision {
    if matches!(budget.overflow_mode, NoiseOverflowMode::Off) {
        return NoiseDecision::Emit;
    }

    let mut budgets = match NOISE_BUDGETS.lock() {
        Ok(guard) => guard,
        Err(_) => return NoiseDecision::Emit,
    };

    let now = Instant::now();
    let state = budgets
        .entry(event_key.to_string())
        .or_insert_with(|| NoiseBudgetState::new(now));

    evaluate_noise_budget_for_state(budget, state, now)
}

fn evaluate_noise_budget_for_state(
    budget: EffectiveBudget,
    state: &mut NoiseBudgetState,
    now: Instant,
) -> NoiseDecision {
    if matches!(budget.overflow_mode, NoiseOverflowMode::Off) {
        return NoiseDecision::Emit;
    }
    let window = Duration::from_millis(budget.window_ms.max(MIN_NOISE_WINDOW_MS));

    if now.duration_since(state.window_started_at) >= window {
        state.window_started_at = now;
        state.emitted = 0;
        state.suppressed = 0;
    }

    if state.emitted < budget.burst {
        state.emitted += 1;
        return NoiseDecision::Emit;
    }

    state.suppressed += 1;

    match budget.overflow_mode {
        NoiseOverflowMode::Off => NoiseDecision::Emit,
        NoiseOverflowMode::Suppress => NoiseDecision::Suppress,
        NoiseOverflowMode::Summarize => {
            let threshold = budget.summary_every.max(MIN_NOISE_SUMMARY_THRESHOLD);
            if state.suppressed.is_multiple_of(threshold) {
                NoiseDecision::EmitSummaryOnly {
                    suppressed_count: state.suppressed,
                }
            } else {
                NoiseDecision::Suppress
            }
        }
    }
}

/// Emit the "this keeps happening" line for a collapsed event.
///
/// The summary carries the level of the events it stands for. Emitting it at
/// INFO regardless would undo the point of budgeting errors at all: an operator
/// watching for ERROR would see the first hundred failures and then silence,
/// with the one line saying the failure is still repeating filed under a level
/// they are not reading.
fn noise_summary_fields(
    level: LogLevel,
    event_key: &str,
    suppressed_count: u32,
    budget: EffectiveBudget,
) -> EventFields {
    let summary = NoiseSummaryPayload::new(event_key.to_string(), suppressed_count, budget);
    let mut fields = EventFields::default()
        .with_message("observability noise summary")
        .with_operation("observability.noise_policy")
        .with_resource("noise_budget", summary.event_key.clone())
        .with_status("suppressed");
    if matches!(
        level,
        LogLevel::Warning | LogLevel::Error | LogLevel::Critical
    ) {
        // Error levels require error metadata; without this the record is
        // stamped `unknown_error`, which reads like a second, mysterious fault.
        fields = fields.with_error(
            "repeated_failure_suppressed",
            format!("{suppressed_count} further occurrences of `{event_key}` were collapsed"),
        );
    }
    for (key, value) in summary.to_context_fields() {
        fields = fields.with_context(key, value);
    }
    fields
}

fn emit_noise_summary(
    level: LogLevel,
    event_key: &str,
    suppressed_count: u32,
    budget: EffectiveBudget,
) {
    let fields = noise_summary_fields(level, event_key, suppressed_count, budget);
    let merged_context = merge_context(&fields);
    let record = build_event_record(
        level,
        super::events::CanonicalEvent::NoiseSummary.as_str(),
        &fields,
        &merged_context,
    );
    emit_payload(
        level,
        super::events::CanonicalEvent::NoiseSummary.as_str(),
        &fields,
        &merged_context,
        Value::Object(record),
    );
}

fn merge_context(fields: &EventFields) -> LogContext {
    let span_context = LogContext::from_current_span();
    let correlation_id = fields
        .correlation_id
        .clone()
        .or_else(|| span_context.correlation_id.clone())
        .or_else(|| fields.request_id.clone());
    let request_id = fields
        .request_id
        .clone()
        .or_else(|| span_context.request_id.clone())
        .or_else(|| correlation_id.clone());

    LogContext {
        request_id,
        correlation_id,
        trace_id: span_context.trace_id,
        span_id: span_context.span_id,
        user_id: fields
            .user_id
            .clone()
            .or_else(|| span_context.user_id.clone()),
        session_id: fields
            .session_id
            .clone()
            .or_else(|| span_context.session_id.clone()),
        client_ip: fields
            .client_ip
            .clone()
            .or_else(|| span_context.client_ip.clone()),
    }
}

fn build_event_record(
    level: LogLevel,
    event: &str,
    fields: &EventFields,
    context: &LogContext,
) -> Map<String, Value> {
    let mut record = base_event_record(level, event);
    insert_message_field(&mut record, fields);
    insert_context_identity_fields(&mut record, context);
    insert_event_detail_fields(&mut record, level, fields);
    insert_context_map_field(&mut record, &fields.context);
    record
}

fn base_event_record(level: LogLevel, event: &str) -> Map<String, Value> {
    let mut record = Map::new();
    record.insert(
        "timestamp".to_string(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );
    record.insert(
        "level".to_string(),
        Value::String(level.as_str().to_string()),
    );
    record.insert("event".to_string(), Value::String(event.to_string()));
    record.insert("logger".to_string(), Value::String(current_logger_target()));
    record
}

fn insert_message_field(record: &mut Map<String, Value>, fields: &EventFields) {
    if let Some(message) = fields.message.as_ref() {
        record.insert("message".to_string(), Value::String(scrub_text(message)));
    }
}

fn insert_context_identity_fields(record: &mut Map<String, Value>, context: &LogContext) {
    if let Some(request_id) = context.request_id.as_ref() {
        record.insert("request_id".to_string(), Value::String(request_id.clone()));
    }
    if let Some(correlation_id) = context.correlation_id.as_ref() {
        record.insert(
            "correlation_id".to_string(),
            Value::String(correlation_id.clone()),
        );
    }
    if let Some(trace_id) = context.trace_id.as_ref() {
        record.insert("trace_id".to_string(), Value::String(trace_id.clone()));
    }
    if let Some(span_id) = context.span_id.as_ref() {
        record.insert("span_id".to_string(), Value::String(span_id.clone()));
    }
    if let Some(user_id) = context.user_id.as_ref() {
        record.insert("user_id".to_string(), Value::String(user_id.clone()));
    }
    if let Some(session_id) = context.session_id.as_ref() {
        record.insert("session_id".to_string(), Value::String(session_id.clone()));
    }
    if let Some(client_ip) = context.client_ip.as_ref() {
        record.insert("client_ip".to_string(), Value::String(client_ip.clone()));
    }
}

fn insert_event_detail_fields(
    record: &mut Map<String, Value>,
    level: LogLevel,
    fields: &EventFields,
) {
    if let Some(operation) = fields.operation.as_ref() {
        record.insert("operation".to_string(), Value::String(operation.clone()));
    }
    if let Some(resource_type) = fields.resource_type.as_ref() {
        record.insert(
            "resource_type".to_string(),
            Value::String(resource_type.clone()),
        );
    }
    if let Some(resource_id) = fields.resource_id.as_ref() {
        record.insert(
            "resource_id".to_string(),
            Value::String(resource_id.clone()),
        );
    }
    if let Some(status) = fields.status.as_ref() {
        record.insert("status".to_string(), Value::String(status.clone()));
    }
    if let Some(duration_ms) = fields.duration_ms {
        let value = serde_json::Number::from_f64(duration_ms)
            .map(Value::Number)
            .unwrap_or(Value::Null);
        record.insert("duration_ms".to_string(), value);
    }

    let (error_type, error_message) = normalize_error_fields(level, fields);
    if let Some(error_type) = error_type {
        record.insert("error_type".to_string(), Value::String(error_type));
    }
    if let Some(error_message) = error_message {
        record.insert("error_message".to_string(), Value::String(error_message));
    }
}

fn insert_context_map_field(record: &mut Map<String, Value>, context: &BTreeMap<String, String>) {
    if context.is_empty() {
        return;
    }
    let scrubbed_context = scrub_context(context)
        .into_iter()
        .map(|(key, value)| (key, Value::String(value)))
        .collect::<Map<String, Value>>();
    record.insert("context".to_string(), Value::Object(scrubbed_context));
}

fn normalize_error_fields(
    level: LogLevel,
    fields: &EventFields,
) -> (Option<String>, Option<String>) {
    if !matches!(level, LogLevel::Error | LogLevel::Critical) {
        return (fields.error_type.clone(), fields.error_message.clone());
    }

    let error_type = fields
        .error_type
        .as_ref()
        .map(|value| scrub_text(value))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| UNKNOWN_ERROR_TYPE.to_string());
    let error_message = fields
        .error_message
        .as_ref()
        .map(|value| scrub_text(value))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| UNKNOWN_ERROR_MESSAGE.to_string());

    (Some(error_type), Some(error_message))
}

fn current_logger_target() -> String {
    tracing::Span::current()
        .metadata()
        .map(|metadata| metadata.target().to_string())
        .unwrap_or_else(|| module_path!().to_string())
}

/// Precomputed string/JSON field values shared by every JSON log level.
struct EmitPayloadValues<'a> {
    event: &'a str,
    message: String,
    request_id: String,
    correlation_id: String,
    trace_id: String,
    span_id: String,
    operation: String,
    resource_type: String,
    resource_id: String,
    status: String,
    duration_ms: f64,
    error_type: String,
    error_message: String,
}

fn emit_payload(
    level: LogLevel,
    event: &str,
    fields: &EventFields,
    context: &LogContext,
    payload: Value,
) {
    if matches!(ACTIVE_LOG_OUTPUT_FORMAT.get(), Some(LogOutputFormat::Text)) {
        emit_text_payload(level, event, fields, context);
        return;
    }

    let values = emit_payload_values(event, fields, context);
    match level {
        LogLevel::Debug => emit_debug_payload(&values, &payload),
        LogLevel::Info => emit_info_payload(&values, &payload),
        LogLevel::Warning => emit_warning_payload(&values, &payload),
        LogLevel::Error | LogLevel::Critical => emit_error_payload(&values, &payload),
    }
}

fn emit_payload_values<'a>(
    event: &'a str,
    fields: &EventFields,
    context: &LogContext,
) -> EmitPayloadValues<'a> {
    EmitPayloadValues {
        event,
        message: fields
            .message
            .as_ref()
            .map(|value| scrub_text(value))
            .unwrap_or_else(|| event.to_string()),
        request_id: context.request_id_or_unknown().to_string(),
        correlation_id: context.correlation_id_or_unknown().to_string(),
        trace_id: context.trace_id.clone().unwrap_or_default(),
        span_id: context.span_id.clone().unwrap_or_default(),
        operation: fields.operation.as_deref().unwrap_or(UNKNOWN).to_string(),
        resource_type: fields
            .resource_type
            .as_deref()
            .unwrap_or(UNKNOWN)
            .to_string(),
        resource_id: fields.resource_id.as_deref().unwrap_or(UNKNOWN).to_string(),
        status: fields.status.as_deref().unwrap_or(UNKNOWN).to_string(),
        duration_ms: fields.duration_ms.unwrap_or_default(),
        error_type: fields.error_type.as_deref().unwrap_or_default().to_string(),
        error_message: fields
            .error_message
            .as_deref()
            .map(scrub_text)
            .unwrap_or_default(),
    }
}

fn emit_text_payload(level: LogLevel, event: &str, fields: &EventFields, context: &LogContext) {
    let min_level = active_text_min_log_level();
    let color_enabled = should_color_text_rendering();
    let target = current_logger_target();
    text_renderer::emit_text_event(text_renderer::TextEvent {
        level,
        event,
        fields,
        context,
        target: &target,
        min_log_level: min_level,
        color_enabled,
    });
}

fn emit_debug_payload(values: &EmitPayloadValues<'_>, payload: &Value) {
    let message = values.message.as_str();
    debug!(
        event = %values.event,
        request_id = %values.request_id,
        correlation_id = %values.correlation_id,
        trace_id = %values.trace_id,
        span_id = %values.span_id,
        operation = %values.operation,
        resource_type = %values.resource_type,
        resource_id = %values.resource_id,
        status = %values.status,
        duration_ms = values.duration_ms,
        error_type = %values.error_type,
        error_message = %values.error_message,
        payload = %payload,
        "{message}"
    );
}

fn emit_info_payload(values: &EmitPayloadValues<'_>, payload: &Value) {
    let message = values.message.as_str();
    info!(
        event = %values.event,
        request_id = %values.request_id,
        correlation_id = %values.correlation_id,
        trace_id = %values.trace_id,
        span_id = %values.span_id,
        operation = %values.operation,
        resource_type = %values.resource_type,
        resource_id = %values.resource_id,
        status = %values.status,
        duration_ms = values.duration_ms,
        error_type = %values.error_type,
        error_message = %values.error_message,
        payload = %payload,
        "{message}"
    );
}

fn emit_warning_payload(values: &EmitPayloadValues<'_>, payload: &Value) {
    let message = values.message.as_str();
    warn!(
        event = %values.event,
        request_id = %values.request_id,
        correlation_id = %values.correlation_id,
        trace_id = %values.trace_id,
        span_id = %values.span_id,
        operation = %values.operation,
        resource_type = %values.resource_type,
        resource_id = %values.resource_id,
        status = %values.status,
        duration_ms = values.duration_ms,
        error_type = %values.error_type,
        error_message = %values.error_message,
        payload = %payload,
        "{message}"
    );
}

fn emit_error_payload(values: &EmitPayloadValues<'_>, payload: &Value) {
    let message = values.message.as_str();
    error!(
        event = %values.event,
        request_id = %values.request_id,
        correlation_id = %values.correlation_id,
        trace_id = %values.trace_id,
        span_id = %values.span_id,
        operation = %values.operation,
        resource_type = %values.resource_type,
        resource_id = %values.resource_id,
        status = %values.status,
        duration_ms = values.duration_ms,
        error_type = %values.error_type,
        error_message = %values.error_message,
        payload = %payload,
        "{message}"
    );
}

fn should_color_text_rendering() -> bool {
    TEXT_COLOR_ENABLED.get().copied().unwrap_or(false)
}

#[cfg(test)]
fn looks_like_uuid7(value: &str) -> bool {
    text_renderer::looks_like_uuid7(value)
}

fn active_text_min_log_level() -> LogLevel {
    ACTIVE_TEXT_MIN_LOG_LEVEL
        .get()
        .copied()
        .unwrap_or(LogLevel::Info)
}

pub mod log_connection_event {
    pub struct Input<'a> {
        pub event: &'a str,
        pub session_id: Option<String>,
        pub host_id: Option<String>,
        pub user_id: Option<String>,
        pub protocol: Option<String>,
        pub status: Option<String>,
        pub correlation_id: Option<String>,
    }
}

pub fn log_connection_event(input: log_connection_event::Input<'_>) {
    let log_connection_event::Input {
        event,
        session_id,
        host_id,
        user_id,
        protocol,
        status,
        correlation_id,
    } = input;
    let session_id = session_id.unwrap_or_else(|| UNKNOWN.to_string());
    let host_id = host_id.unwrap_or_else(|| UNKNOWN.to_string());
    let user_id = user_id.unwrap_or_else(|| "anonymous".to_string());
    let protocol = protocol.unwrap_or_else(|| UNKNOWN.to_string());
    let status = status.unwrap_or_else(|| UNKNOWN.to_string());
    let correlation_id = correlation_id.unwrap_or_else(|| UNKNOWN.to_string());

    let fields = EventFields::default()
        .with_message("connection lifecycle")
        .with_correlation_id(correlation_id)
        .with_user_id(user_id)
        .with_session_id(session_id.clone())
        .with_resource("connection", session_id)
        .with_status(status)
        .with_context("host_id", host_id)
        .with_context("protocol", protocol);

    log_event(LogLevel::Info, event, fields);
}

#[cfg(test)]
mod tests {
    use super::{
        EffectiveBudget, EventFields, LogLevel, NoiseDecision, build_event_record_for_contract,
        canonical_event_key, effective_overflow_mode, looks_like_uuid7, noise_summary_fields,
        normalize_error_fields, parse_text_log_level, simulate_noise_budget,
    };
    use crate::observability::{NoiseBudgetProfile, NoiseOverflowMode, NoisePolicy};

    #[test]
    fn failures_are_summarized_never_silently_suppressed() {
        // Warning and above keep a periodic line with the repeat count even
        // when the operator configured plain suppression: losing a flood of
        // errors entirely is worse than the flood.
        for level in [LogLevel::Warning, LogLevel::Error, LogLevel::Critical] {
            assert_eq!(
                effective_overflow_mode(level, NoiseOverflowMode::Suppress),
                NoiseOverflowMode::Summarize,
                "{level:?} must be summarized rather than dropped"
            );
        }
        for level in [LogLevel::Debug, LogLevel::Info] {
            assert_eq!(
                effective_overflow_mode(level, NoiseOverflowMode::Suppress),
                NoiseOverflowMode::Suppress,
                "{level:?} is chatter; plain suppression is the point"
            );
        }
    }

    #[test]
    fn a_collapsed_failure_summary_is_itself_a_failure_record() {
        // The summary must be readable by whoever is watching for errors. If it
        // came out at INFO, an operator filtering on ERROR would see the first
        // hundred failures and then nothing, with the line saying "still
        // happening" filed where they are not looking.
        let policy = NoisePolicy {
            window_ms: 60_000,
            max_events_per_window: 100,
            overflow_mode: NoiseOverflowMode::Summarize,
        };
        let fields = noise_summary_fields(
            LogLevel::Error,
            "worker.tick.failed",
            900,
            EffectiveBudget::from_policy(policy),
        );
        let record =
            build_event_record_for_contract(LogLevel::Error, "observability.noise.summary", fields);

        assert_eq!(
            record.get("level").and_then(|value| value.as_str()),
            Some(LogLevel::Error.as_str()),
            "the summary must carry the level of the events it stands for"
        );
        let error_type = record
            .get("error_type")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        assert_eq!(
            error_type, "repeated_failure_suppressed",
            "an error-level record without error metadata is stamped \
             `unknown_error`, which reads like a second, unrelated fault"
        );
    }

    #[test]
    fn off_is_honoured_at_every_level() {
        // An operator who asks for every line gets every line, errors included.
        for level in [
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
            LogLevel::Critical,
        ] {
            assert_eq!(
                effective_overflow_mode(level, NoiseOverflowMode::Off),
                NoiseOverflowMode::Off
            );
        }
    }

    #[test]
    fn a_repeating_error_stops_flooding_the_log() {
        // The shape of the 2026-08-06 outage: one worker failing on every tick
        // against a dead database. 10_000 attempts must not become 10_000
        // lines, and must not become zero either.
        let policy = NoisePolicy {
            window_ms: 60_000,
            max_events_per_window: 100,
            overflow_mode: NoiseOverflowMode::Summarize,
        };
        let metrics = simulate_noise_budget(
            LogLevel::Error,
            "monitoring.observability.list_poll.failed",
            EventFields::default(),
            policy,
            10_000,
        );

        assert_eq!(metrics.emitted, 100, "the budget must cap the flood");
        assert!(
            metrics.summary_events > 0,
            "the operator must still learn the error is repeating"
        );
        assert!(
            metrics.emitted + metrics.summary_events < 1_000,
            "10_000 identical failures produced {} log lines",
            metrics.emitted + metrics.summary_events
        );
    }

    #[test]
    fn retrying_worker_noise_profile_overrides_global_allowance_and_cadence() {
        let policy = NoisePolicy {
            window_ms: 60_000,
            max_events_per_window: 100,
            overflow_mode: NoiseOverflowMode::Summarize,
        };
        let metrics = simulate_noise_budget(
            LogLevel::Error,
            "monitoring.observability.list_poll.failed",
            EventFields::default().with_noise_budget(NoiseBudgetProfile::RETRYING_WORKER_FAILURE),
            policy,
            100,
        );

        assert_eq!(metrics.emitted, 1);
        assert_eq!(metrics.suppressed, 99);
        assert_eq!(metrics.summary_events, 9);
    }

    #[test]
    fn error_fields_are_enforced_for_error_levels() {
        let fields = EventFields::default();
        let (error_type, error_message) = normalize_error_fields(LogLevel::Error, &fields);
        assert_eq!(error_type.as_deref(), Some("unknown_error"));
        assert_eq!(error_message.as_deref(), Some("unknown error"));
    }

    #[test]
    fn canonical_event_key_normalizes_whitespace() {
        assert_eq!(canonical_event_key(" REST Route "), "rest_route");
    }

    #[test]
    fn non_primitive_context_is_sanitized() {
        let fields = EventFields::default()
            .with_json_context("payload", &serde_json::json!({"nested": {"value": 1}}));

        assert_eq!(
            fields.context.get("payload").expect("payload"),
            "[non-primitive]"
        );
    }

    #[test]
    fn noise_decision_type_is_comparable() {
        assert_ne!(NoiseDecision::Emit, NoiseDecision::Suppress);
    }

    #[test]
    fn uuid7_detection_validates_version_and_shape() {
        assert!(looks_like_uuid7("019c4277-6bf3-7843-9f3f-c88e31d25e58"));
        assert!(!looks_like_uuid7("019c4277-6bf3-6843-9f3f-c88e31d25e58"));
        assert!(!looks_like_uuid7("not-a-uuid"));
    }

    #[test]
    fn simulate_noise_budget_off_mode_does_not_suppress_events() {
        let metrics = simulate_noise_budget(
            LogLevel::Info,
            "rest.route.received",
            EventFields::default().with_operation("rest.route.inventory_hosts"),
            NoisePolicy {
                window_ms: crate::config::DEFAULT_NOISE_FALLBACK_WINDOW_MS,
                max_events_per_window: 5,
                overflow_mode: NoiseOverflowMode::Off,
            },
            20,
        );
        assert_eq!(metrics.emitted, 20);
        assert_eq!(metrics.suppressed, 0);
        assert_eq!(metrics.summary_events, 0);
    }

    #[test]
    fn parse_text_log_level_supports_target_directives() {
        assert_eq!(parse_text_log_level("DEBUG"), Some(LogLevel::Debug));
        assert_eq!(
            parse_text_log_level("piping_backend=warning,hyper=error"),
            Some(LogLevel::Warning)
        );
    }
}
