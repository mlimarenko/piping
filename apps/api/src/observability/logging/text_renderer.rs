use std::collections::BTreeMap;
use std::io::Write;

use super::{EventFields, LogContext, LogLevel, UNKNOWN, scrub_context, scrub_text};

// Calm palette tuned for readability in `docker logs` (dark backgrounds).
const UUID7_COLOR_PREFIX: &str = "\u{001b}[38;5;179m"; // soft beige
const DEBUG_COLOR_PREFIX: &str = "\u{001b}[38;5;244m";
const INFO_COLOR_PREFIX: &str = "\u{001b}[38;5;114m"; // muted green
const WARNING_COLOR_PREFIX: &str = "\u{001b}[38;5;180m"; // muted yellow
const ERROR_COLOR_PREFIX: &str = "\u{001b}[38;5;203m"; // muted red
const CRITICAL_COLOR_PREFIX: &str = "\u{001b}[1;38;5;203m";
const TIMESTAMP_COLOR_PREFIX: &str = "\u{001b}[2;38;5;246m"; // dim grey
const EVENT_COLOR_PREFIX: &str = "\u{001b}[38;5;110m"; // soft blue
const OPERATION_COLOR_PREFIX: &str = "\u{001b}[38;5;109m"; // soft teal
const RESOURCE_TYPE_COLOR_PREFIX: &str = "\u{001b}[2;38;5;246m"; // dim grey
const RESOURCE_ID_COLOR_PREFIX: &str = "\u{001b}[38;5;111m"; // soft sky
const METRIC_COLOR_PREFIX: &str = "\u{001b}[2;38;5;247m"; // dim grey
const DIM_COLOR_PREFIX: &str = "\u{001b}[2m";
const COLOR_SUFFIX: &str = "\u{001b}[0m";

const HTTP_STATUS_1XX_MIN: u16 = 100;
const HTTP_STATUS_1XX_MAX: u16 = 199;
const HTTP_STATUS_2XX_MIN: u16 = 200;
const HTTP_STATUS_2XX_MAX: u16 = 299;
const HTTP_STATUS_3XX_MIN: u16 = 300;
const HTTP_STATUS_3XX_MAX: u16 = 399;
const HTTP_STATUS_4XX_MIN: u16 = 400;
const HTTP_STATUS_4XX_MAX: u16 = 499;

const ERROR_STATUS_KEYWORDS: &[&str] = &["fail", "error", "denied", "timeout"];
const WARNING_STATUS_KEYWORDS: &[&str] = &["warn", "suppressed", "degraded"];
const HEALTHY_STATUS_VALUES: &[&str] = &[
    "ok",
    "ready",
    "connected",
    "completed",
    "created",
    "running",
    "starting",
];

const UUID7_EXPECTED_LENGTH: usize = 36;
const UUID7_VERSION_INDEX: usize = 14;
const UUID7_EXPECTED_VERSION: u8 = b'7';
const UUID7_DASH_POSITIONS: [usize; 4] = [8, 13, 18, 23];

const HEX_IDENTIFIER_SHORT_LEN: usize = 16;
const HEX_IDENTIFIER_LONG_LEN: usize = 32;

const TIMESTAMP_PRECISION: chrono::SecondsFormat = chrono::SecondsFormat::Millis;
const TIMESTAMP_USE_ZULU_SUFFIX: bool = true;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextOutputStream {
    Stdout,
    Stderr,
}

/// Everything needed to render and route one text-format log line.
pub(super) struct TextEvent<'a> {
    pub(super) level: LogLevel,
    pub(super) event: &'a str,
    pub(super) fields: &'a EventFields,
    pub(super) context: &'a LogContext,
    pub(super) target: &'a str,
    pub(super) min_log_level: LogLevel,
    pub(super) color_enabled: bool,
}

pub(super) fn emit_text_event(event: TextEvent<'_>) {
    if !should_emit_text_level(event.level, event.min_log_level) {
        return;
    }

    let line = format_text_line(
        event.event,
        event.fields,
        event.context,
        event.color_enabled,
    );
    let timestamp = format_text_timestamp(event.color_enabled);
    let level_label = format_text_level(event.level, event.color_enabled);
    let target = format_text_target(event.target, event.color_enabled);
    let rendered = format!("{timestamp} {level_label} {target}: {line}");
    emit_text_output(text_output_stream_for_level(event.level), &rendered);
}

fn should_emit_text_level(level: LogLevel, min_log_level: LogLevel) -> bool {
    level.priority() >= min_log_level.priority()
}

fn format_text_line(
    event: &str,
    fields: &EventFields,
    context: &LogContext,
    color_enabled: bool,
) -> String {
    let mut tokens = Vec::new();
    tokens.push(format_text_token("event", event, color_enabled));
    push_text_field_tokens(&mut tokens, fields, color_enabled);
    push_text_context_tokens(&mut tokens, context, color_enabled);
    if !fields.context.is_empty() {
        tokens.push(format_text_context_field(&fields.context, color_enabled));
    }
    if let Some(message) = fields.message.as_ref() {
        let scrubbed = scrub_text(message);
        tokens.push(format_text_token("message", &scrubbed, color_enabled));
    }

    tokens.join(" ")
}

/// Event-supplied fields, in canonical render order.
fn push_text_field_tokens(tokens: &mut Vec<String>, fields: &EventFields, color_enabled: bool) {
    if let Some(operation) = fields.operation.as_ref() {
        tokens.push(format_text_token("operation", operation, color_enabled));
    }
    if let Some(resource_type) = fields.resource_type.as_ref() {
        tokens.push(format_text_token(
            "resource_type",
            resource_type,
            color_enabled,
        ));
    }
    if let Some(resource_id) = fields.resource_id.as_ref() {
        tokens.push(format_text_token("resource_id", resource_id, color_enabled));
    }
    if let Some(status) = fields.status.as_ref() {
        tokens.push(format_text_token("status", status, color_enabled));
    }
    if let Some(duration_ms) = fields.duration_ms {
        let duration = format!("{duration_ms:.2}");
        tokens.push(format_text_token("duration_ms", &duration, color_enabled));
    }
    if let Some(error_type) = fields.error_type.as_ref() {
        tokens.push(format_text_token("error_type", error_type, color_enabled));
    }
    if let Some(error_message) = fields.error_message.as_ref() {
        let scrubbed = scrub_text(error_message);
        tokens.push(format_text_token("error_message", &scrubbed, color_enabled));
    }
}

/// Request/span context fields, in canonical render order.
fn push_text_context_tokens(tokens: &mut Vec<String>, context: &LogContext, color_enabled: bool) {
    if let Some(request_id) = context.request_id.as_ref() {
        tokens.push(format_text_token("request_id", request_id, color_enabled));
    }
    if let Some(correlation_id) = context.correlation_id.as_ref() {
        tokens.push(format_text_token(
            "correlation_id",
            correlation_id,
            color_enabled,
        ));
    }
    if let Some(trace_id) = context.trace_id.as_ref() {
        tokens.push(format_text_token("trace_id", trace_id, color_enabled));
    }
    if let Some(span_id) = context.span_id.as_ref() {
        tokens.push(format_text_token("span_id", span_id, color_enabled));
    }
    if let Some(user_id) = context.user_id.as_ref() {
        tokens.push(format_text_token("user_id", user_id, color_enabled));
    }
    if let Some(session_id) = context.session_id.as_ref() {
        tokens.push(format_text_token("session_id", session_id, color_enabled));
    }
    if let Some(client_ip) = context.client_ip.as_ref() {
        tokens.push(format_text_token("client_ip", client_ip, color_enabled));
    }
}

fn format_text_token(key: &str, value: &str, color_enabled: bool) -> String {
    let value = format_text_value(key, value, color_enabled);
    format!("{key}={value}")
}

fn format_text_context_field(context: &BTreeMap<String, String>, color_enabled: bool) -> String {
    let context_tokens = scrub_context(context)
        .into_iter()
        .map(|(key, value)| {
            let value = format_text_context_value(&key, &value, color_enabled);
            format!("{key}={value}")
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("context=[{context_tokens}]")
}

fn format_text_context_value(key: &str, value: &str, color_enabled: bool) -> String {
    match key {
        "method" => maybe_color_text(value, OPERATION_COLOR_PREFIX, color_enabled),
        "path" => maybe_color_text(value, RESOURCE_ID_COLOR_PREFIX, color_enabled),
        "client_ip" => maybe_color_text(value, DIM_COLOR_PREFIX, color_enabled),
        "first_error" => maybe_color_text(value, ERROR_COLOR_PREFIX, color_enabled),
        "error_count" | "suppressed_count" | "window_ms" => {
            maybe_color_text(value, METRIC_COLOR_PREFIX, color_enabled)
        }
        "event_key" => maybe_color_text(value, EVENT_COLOR_PREFIX, color_enabled),
        _ => format_text_scalar_value(value, color_enabled),
    }
}

fn format_text_value(key: &str, value: &str, color_enabled: bool) -> String {
    match key {
        "event" => maybe_color_text(value, EVENT_COLOR_PREFIX, color_enabled),
        "operation" => maybe_color_text(value, OPERATION_COLOR_PREFIX, color_enabled),
        "resource_type" => maybe_color_text(value, RESOURCE_TYPE_COLOR_PREFIX, color_enabled),
        "resource_id" => format_text_resource_id(value, color_enabled),
        "status" => format_text_status(value, color_enabled),
        "duration_ms" => maybe_color_text(value, METRIC_COLOR_PREFIX, color_enabled),
        "error_type" | "error_message" => {
            maybe_color_text(value, ERROR_COLOR_PREFIX, color_enabled)
        }
        "request_id" | "correlation_id" | "user_id" | "session_id" => {
            format_text_identifier(value, color_enabled)
        }
        "trace_id" | "span_id" => maybe_color_text(value, UUID7_COLOR_PREFIX, color_enabled),
        "client_ip" => maybe_color_text(value, DIM_COLOR_PREFIX, color_enabled),
        "message" => maybe_color_text(value, DIM_COLOR_PREFIX, color_enabled),
        _ => format_text_scalar_value(value, color_enabled),
    }
}

fn format_text_identifier(value: &str, color_enabled: bool) -> String {
    if looks_like_uuid7(value) {
        return maybe_color_uuid7_for_text(value, color_enabled);
    }
    if looks_like_hex_identifier(value) {
        return maybe_color_text(value, UUID7_COLOR_PREFIX, color_enabled);
    }
    if value.eq_ignore_ascii_case(UNKNOWN) {
        return maybe_color_text(value, DIM_COLOR_PREFIX, color_enabled);
    }
    maybe_color_text(value, UUID7_COLOR_PREFIX, color_enabled)
}

fn format_text_resource_id(value: &str, color_enabled: bool) -> String {
    if looks_like_uuid7(value) {
        return maybe_color_uuid7_for_text(value, color_enabled);
    }
    if value.starts_with('/') {
        return maybe_color_text(value, RESOURCE_ID_COLOR_PREFIX, color_enabled);
    }
    maybe_color_text(value, RESOURCE_ID_COLOR_PREFIX, color_enabled)
}

fn format_text_status(value: &str, color_enabled: bool) -> String {
    if let Ok(code) = value.parse::<u16>() {
        return match code {
            HTTP_STATUS_1XX_MIN..=HTTP_STATUS_1XX_MAX => {
                maybe_color_text(value, METRIC_COLOR_PREFIX, color_enabled)
            }
            HTTP_STATUS_2XX_MIN..=HTTP_STATUS_2XX_MAX => {
                maybe_color_text(value, INFO_COLOR_PREFIX, color_enabled)
            }
            HTTP_STATUS_3XX_MIN..=HTTP_STATUS_3XX_MAX => {
                maybe_color_text(value, RESOURCE_ID_COLOR_PREFIX, color_enabled)
            }
            HTTP_STATUS_4XX_MIN..=HTTP_STATUS_4XX_MAX => {
                maybe_color_text(value, WARNING_COLOR_PREFIX, color_enabled)
            }
            _ => maybe_color_text(value, ERROR_COLOR_PREFIX, color_enabled),
        };
    }

    let normalized = value.trim().to_ascii_lowercase();
    if contains_any_keyword(&normalized, ERROR_STATUS_KEYWORDS) {
        return maybe_color_text(value, ERROR_COLOR_PREFIX, color_enabled);
    }
    if contains_any_keyword(&normalized, WARNING_STATUS_KEYWORDS) {
        return maybe_color_text(value, WARNING_COLOR_PREFIX, color_enabled);
    }
    if is_healthy_status(&normalized) {
        return maybe_color_text(value, INFO_COLOR_PREFIX, color_enabled);
    }
    maybe_color_text(value, METRIC_COLOR_PREFIX, color_enabled)
}

fn contains_any_keyword(value: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| value.contains(keyword))
}

fn is_healthy_status(value: &str) -> bool {
    HEALTHY_STATUS_VALUES
        .iter()
        .any(|candidate| candidate == &value)
}

fn format_text_scalar_value(value: &str, color_enabled: bool) -> String {
    if looks_like_uuid7(value) {
        return maybe_color_uuid7_for_text(value, color_enabled);
    }
    if looks_like_hex_identifier(value) {
        return maybe_color_text(value, UUID7_COLOR_PREFIX, color_enabled);
    }
    value.to_string()
}

fn maybe_color_uuid7_for_text(value: &str, color_enabled: bool) -> String {
    if !color_enabled || !looks_like_uuid7(value) {
        return value.to_string();
    }
    format!("{UUID7_COLOR_PREFIX}{value}{COLOR_SUFFIX}")
}

fn text_output_stream_for_level(level: LogLevel) -> TextOutputStream {
    match level {
        LogLevel::Error | LogLevel::Critical => TextOutputStream::Stderr,
        LogLevel::Debug | LogLevel::Info | LogLevel::Warning => TextOutputStream::Stdout,
    }
}

fn emit_text_output(stream: TextOutputStream, line: &str) {
    let write_result = match stream {
        TextOutputStream::Stdout => {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{line}")
        }
        TextOutputStream::Stderr => {
            let mut stderr = std::io::stderr().lock();
            writeln!(stderr, "{line}")
        }
    };
    let _ = write_result;
}

fn format_text_timestamp(color_enabled: bool) -> String {
    let timestamp =
        chrono::Utc::now().to_rfc3339_opts(TIMESTAMP_PRECISION, TIMESTAMP_USE_ZULU_SUFFIX);
    maybe_color_text(&timestamp, TIMESTAMP_COLOR_PREFIX, color_enabled)
}

fn format_text_level(level: LogLevel, color_enabled: bool) -> String {
    let color = match level {
        LogLevel::Debug => DEBUG_COLOR_PREFIX,
        LogLevel::Info => INFO_COLOR_PREFIX,
        LogLevel::Warning => WARNING_COLOR_PREFIX,
        LogLevel::Error => ERROR_COLOR_PREFIX,
        LogLevel::Critical => CRITICAL_COLOR_PREFIX,
    };
    maybe_color_text(level.as_str(), color, color_enabled)
}

fn format_text_target(target: &str, color_enabled: bool) -> String {
    maybe_color_text(target, DIM_COLOR_PREFIX, color_enabled)
}

fn maybe_color_text(value: &str, color_prefix: &'static str, color_enabled: bool) -> String {
    if !color_enabled {
        return value.to_string();
    }
    format!("{color_prefix}{value}{COLOR_SUFFIX}")
}

pub(super) fn looks_like_uuid7(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != UUID7_EXPECTED_LENGTH {
        return false;
    }
    for position in UUID7_DASH_POSITIONS {
        if bytes[position] != b'-' {
            return false;
        }
    }
    let version = bytes[UUID7_VERSION_INDEX].to_ascii_lowercase();
    if version != UUID7_EXPECTED_VERSION {
        return false;
    }

    bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| UUID7_DASH_POSITIONS.contains(&index) || byte.is_ascii_hexdigit())
}

fn looks_like_hex_identifier(value: &str) -> bool {
    matches!(
        value.len(),
        HEX_IDENTIFIER_SHORT_LEN | HEX_IDENTIFIER_LONG_LEN
    ) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
