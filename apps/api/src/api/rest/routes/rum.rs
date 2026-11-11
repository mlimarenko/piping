//! Public browser RUM relay.
//!
//! The SPA asks `GET /api/v1/rum/config` what this deployment allows and posts
//! Web Vitals, JS errors and product events to `POST /api/v1/rum/report`. These
//! handlers are the only browser-facing telemetry boundary: the browser never
//! receives collector credentials or an OTLP endpoint. Export is owned by the
//! backend's configured OTLP pipeline.
//!
//! Safeguards live here, not in the client:
//! - an operator switch (`observability.browser_telemetry_enabled`): a disabled
//!   relay records nothing at all — no metric series, no OTLP point, no log;
//! - a route-local body limit and a fixed maximum event count;
//! - a process-wide token bucket so one client cannot amplify into the scrape
//!   registry;
//! - closed vocabularies for metrics, error sources, routes and product events;
//! - stack frames reduced to `file:line:column`, with origins, queries,
//!   fragments, function names and messages discarded.
//!
//! Nothing from the request body is logged verbatim. Only normalised labels and
//! sanitised frames leave the process, so a token, form value or query string
//! cannot reach the collector or the log store through this path.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use axum::{
    Json,
    extract::{DefaultBodyLimit, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::config;
use crate::observability::metrics;
use crate::observability::{EventFields, LogLevel, log_event};

/// Version of the browser payload contract. A different version is rejected
/// instead of guessed: the client and relay deploy together.
const PAYLOAD_VERSION: u8 = 1;
/// One report is a small batch; more than this is not a real browser.
const MAX_EVENTS: usize = 20;
/// Route-local body cap. A normal report is well under 2 KiB.
pub const MAX_REPORT_BYTES: usize = 8 * 1024;
const MAX_STACK_FRAMES: usize = 40;
const MAX_STACK_CHARS: usize = 4 * 1024;

const BUCKET_CAPACITY: f64 = 120.0;
const BUCKET_REFILL_PER_SECOND: f64 = 30.0;

/// The route is mounted with its own body limit so the global API limit stays
/// generous for inventory uploads.
pub fn route() -> axum::routing::MethodRouter<crate::app::AppState> {
    post(report_handler).layer(DefaultBodyLimit::max(MAX_REPORT_BYTES))
}

/// What the browser may do in this deployment.
///
/// `enabled` is the operator switch for the whole relay. `default_enabled` is
/// the default when the browser has no saved choice; a saved refusal always
/// wins in the browser, which is why the backend never sends a decision.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RumConfig {
    pub enabled: bool,
    pub default_enabled: bool,
}

/// Public bootstrap for the browser telemetry client. No session and no
/// credentials: the answer is a fixed pair of booleans.
pub async fn config_handler(State(state): State<AppState>) -> Json<RumConfig> {
    Json(rum_config(&state.settings.observability))
}

fn rum_config(observability: &config::ObservabilityConfig) -> RumConfig {
    RumConfig {
        enabled: observability.browser_telemetry_enabled,
        default_enabled: observability.browser_telemetry_default_enabled,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RumReport {
    pub version: u8,
    pub events: Vec<RumEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RumEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    metric: Option<String>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    stack: Option<String>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    event: Option<String>,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(now: Instant) -> Self {
        Self {
            tokens: BUCKET_CAPACITY,
            last_refill: now,
        }
    }

    fn try_acquire(&mut self, now: Instant) -> bool {
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        self.tokens = (self.tokens + elapsed * BUCKET_REFILL_PER_SECOND).min(BUCKET_CAPACITY);
        self.last_refill = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

static REPORT_LIMITER: OnceLock<Mutex<TokenBucket>> = OnceLock::new();

fn limiter() -> &'static Mutex<TokenBucket> {
    REPORT_LIMITER.get_or_init(|| Mutex::new(TokenBucket::new(Instant::now())))
}

fn within_rate_limit() -> bool {
    #[expect(
        clippy::expect_used,
        reason = "the relay mutex has no panicking critical section; poisoning means a previous panic already aborted the request"
    )]
    let mut bucket = limiter().lock().expect("rum relay limiter lock");
    bucket.try_acquire(Instant::now())
}

async fn report_handler(State(state): State<AppState>, Json(report): Json<RumReport>) -> Response {
    handle_report(
        report,
        state.settings.observability.browser_telemetry_enabled,
    )
}

/// Report intake behind the operator switch.
///
/// A disabled deployment answers `404` and records nothing: the early return is
/// before the rate limiter, the recorders and the error log, so a report that
/// arrives anyway cannot create a metric series, an OTLP point or a log record.
/// Splitting this from the extractors keeps the contract testable without a
/// database-backed `AppState`.
fn handle_report(report: RumReport, enabled: bool) -> Response {
    if !enabled {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    if !within_rate_limit() {
        metrics::record_report("rate_limited");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "1")],
            "telemetry rate limit exceeded",
        )
            .into_response();
    }

    if report.version != PAYLOAD_VERSION
        || report.events.is_empty()
        || report.events.len() > MAX_EVENTS
    {
        metrics::record_report("rejected");
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported telemetry report",
        )
            .into_response();
    }

    let mut accepted = 0_usize;
    for event in report.events {
        match event.kind.as_str() {
            "vital" => {
                let (Some(metric), Some(value)) = (event.metric, event.value) else {
                    continue;
                };
                if metrics::VITAL_METRICS.contains(&metric.as_str())
                    && value.is_finite()
                    && value >= 0.0
                {
                    metrics::record_vital(&metric, value);
                    accepted += 1;
                }
            }
            "js_error" => {
                let Some(source) = event.source else {
                    continue;
                };
                if !metrics::JS_ERROR_SOURCES.contains(&source.as_str()) {
                    continue;
                }
                let route = metrics::bounded_route(event.route.as_deref().unwrap_or(""));
                metrics::record_js_error(&source, route);
                if let Some(frames) = event.stack.as_deref().and_then(sanitise_stack) {
                    log_event(
                        LogLevel::Warning,
                        "web_rum.js_error",
                        EventFields::default()
                            .with_status("js_error")
                            .with_error_stack(frames)
                            .with_context("source", source)
                            .with_context("route", route)
                            .with_context("release", crate::PRODUCT_VERSION),
                    );
                }
                accepted += 1;
            }
            "api_failure" => {
                let (Some(route), Some(status)) = (event.route, event.status) else {
                    continue;
                };
                let route = metrics::bounded_route(&route);
                match status {
                    400..=499 => {
                        metrics::record_api_failure(route, "4xx");
                        accepted += 1;
                    }
                    500..=599 => {
                        metrics::record_api_failure(route, "5xx");
                        accepted += 1;
                    }
                    _ => {}
                }
            }
            "product_event" => {
                let Some(name) = event.event else {
                    continue;
                };
                if metrics::PRODUCT_EVENTS
                    .iter()
                    .any(|(known, _)| *known == name.as_str())
                {
                    metrics::record_product_event(&name);
                    accepted += 1;
                }
            }
            _ => {}
        }
    }

    metrics::record_report(if accepted == 0 {
        "rejected"
    } else {
        "accepted"
    });
    (StatusCode::ACCEPTED, "").into_response()
}

/// Reduce a V8 stack to `file:line:column` frames.
///
/// Everything that can carry user data is dropped: the exception message and
/// error name (not part of the input), function names, origins, query strings
/// and fragments. A resource path such as `/assets/index-abc.js` plus a
/// position is all the private source-map artifact needs.
fn sanitise_stack(raw: &str) -> Option<String> {
    let mut frames = Vec::new();
    let mut chars = 0_usize;
    for line in raw.lines() {
        if frames.len() >= MAX_STACK_FRAMES {
            break;
        }
        let Some(frame) = frame_location(line) else {
            continue;
        };
        if chars + frame.len() > MAX_STACK_CHARS {
            break;
        }
        chars += frame.len() + 1;
        frames.push(frame);
    }
    if frames.is_empty() {
        None
    } else {
        Some(frames.join("\n"))
    }
}

fn frame_location(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let line = line.strip_prefix("at ").unwrap_or(line);
    // `at fn (url:1:2)` and `fn@url:1:2` both end in a location; keep only that.
    let location = line
        .rsplit_once('(')
        .map(|(_, rest)| rest.trim_end_matches(')'))
        .unwrap_or(line);
    let location = location
        .split_once('@')
        .filter(|(prefix, rest)| {
            !prefix.contains('/')
                && !prefix.contains(':')
                && (!prefix.is_empty() || rest.starts_with('/') || rest.contains("://"))
        })
        .map_or(location, |(_, rest)| rest);
    let location = strip_origin(location.trim());
    let location = location.split(['?', '#']).next()?.trim();
    let mut parts = location.rsplitn(3, ':');
    let column = parts.next()?;
    let line_number = parts.next()?;
    let file = parts.next()?;
    if file.is_empty()
        || column.is_empty()
        || line_number.is_empty()
        || !file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '@'))
        || !column.chars().all(|c| c.is_ascii_digit())
        || !line_number.chars().all(|c| c.is_ascii_digit())
        || file.len() > 180
        || column.len() > 8
        || line_number.len() > 8
    {
        return None;
    }
    Some(format!("{file}:{line_number}:{column}"))
}

fn strip_origin(value: &str) -> &str {
    let after_origin = value
        .find("://")
        .map(|scheme| &value[scheme + 3..])
        .or_else(|| value.strip_prefix("//"));
    if let Some(after_origin) = after_origin {
        return after_origin
            .find('/')
            .map_or("", |slash| &after_origin[slash..]);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_keeps_only_locations() {
        let raw = "Error: user typed secret in field\n    at handleClick (https://app.piping.space/assets/index-abc.js:12:34)\n    at https://app.piping.space/assets/vendor.js:7:89?token=secret#frag";
        let frames = sanitise_stack(raw).expect("frames");
        assert_eq!(frames, "/assets/index-abc.js:12:34\n/assets/vendor.js:7:89");
        assert!(!frames.contains("secret"));
        assert!(!frames.contains("handleClick"));
        assert!(!frames.contains("app.piping.space"));
    }

    #[test]
    fn stack_drops_non_frames() {
        assert!(sanitise_stack("Error: boom\n  somewhere in app").is_none());
        assert!(sanitise_stack("").is_none());
    }

    #[test]
    fn stack_removes_v8_and_firefox_callable_prefixes() {
        let raw = "at /assets/index.js:1:2\nprofileValue@/assets/vendor.js:3:4\n@https://app.example/assets/runtime.js:5:6\nat //app.example/assets/chunk.js:7:8";
        let frames = sanitise_stack(raw).expect("frames");
        assert_eq!(
            frames,
            "/assets/index.js:1:2\n/assets/vendor.js:3:4\n/assets/runtime.js:5:6\n/assets/chunk.js:7:8"
        );
        assert!(!frames.contains("profileValue"));
        assert!(!frames.contains("app.example"));
    }

    #[test]
    fn stack_preserves_scoped_asset_paths() {
        assert_eq!(
            frame_location("at /assets/@scope/package.js:7:8"),
            Some("/assets/@scope/package.js:7:8".to_string())
        );
    }

    #[test]
    fn stack_is_bounded() {
        let raw = (0..1_000)
            .map(|index| format!("    at /assets/index.js:{index}:1"))
            .collect::<Vec<_>>()
            .join("\n");
        let frames = sanitise_stack(&raw).expect("frames");
        assert_eq!(frames.lines().count(), MAX_STACK_FRAMES);
    }

    #[test]
    fn payload_rejects_unknown_fields_and_types() {
        assert!(
            serde_json::from_str::<RumReport>(
                r#"{"version":1,"events":[{"type":"vital","metric":"lcp","value":1}],"extra":true}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<RumReport>(
                r#"{"version":1,"events":[{"type":"vital","metric":"lcp","value":1,"secret":"x"}]}"#
            )
            .is_err()
        );
        // The event kind is a bounded string checked by the handler; an
        // unknown kind parses and is dropped without reaching a recorder.
        let report: RumReport =
            serde_json::from_str(r#"{"version":1,"events":[{"type":"click","route":"/x"}]}"#)
                .expect("unknown kind is dropped, not a parse failure");
        assert_eq!(report.events[0].kind, "click");
    }

    #[test]
    fn payload_accepts_the_contract() {
        let report: RumReport = serde_json::from_str(
            r#"{"version":1,"events":[{"type":"vital","metric":"lcp","value":1234.5},{"type":"js_error","source":"uncaught","route":"/connections","stack":"at https://x/assets/a.js:1:2"},{"type":"api_failure","route":"/api/v1/connections/sessions","status":503},{"type":"product_event","event":"playbook_run_started"}]}"#,
        )
        .expect("valid report");
        assert_eq!(report.events.len(), 4);
    }

    #[test]
    fn limiter_refills_and_rejects_a_burst() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(start);
        for _ in 0..BUCKET_CAPACITY as usize {
            assert!(bucket.try_acquire(start));
        }
        assert!(!bucket.try_acquire(start));
        assert!(bucket.try_acquire(start + std::time::Duration::from_millis(40)));
    }

    /// The standalone defaults keep the relay available and keep the browser's
    /// own opt-in. A Compose preset sets both fields from one variable; the
    /// payload keeps `snake_case` because the browser reads the field names.
    #[test]
    fn config_payload_reports_the_operator_switches() {
        let standalone: config::ObservabilityConfig =
            serde_json::from_str("{}").expect("observability defaults");
        assert!(standalone.browser_telemetry_enabled);
        assert!(!standalone.browser_telemetry_default_enabled);
        assert_eq!(
            serde_json::to_string(&rum_config(&standalone)).expect("serialize config"),
            r#"{"enabled":true,"default_enabled":false}"#
        );

        let preset: config::ObservabilityConfig = serde_json::from_str(
            r#"{"browser_telemetry_enabled":true,"browser_telemetry_default_enabled":true}"#,
        )
        .expect("preset observability settings");
        assert_eq!(
            serde_json::to_string(&rum_config(&preset)).expect("serialize config"),
            r#"{"enabled":true,"default_enabled":true}"#
        );

        let disabled: config::ObservabilityConfig =
            serde_json::from_str(r#"{"browser_telemetry_enabled":false}"#)
                .expect("disabled observability settings");
        assert_eq!(
            serde_json::to_string(&rum_config(&disabled)).expect("serialize config"),
            r#"{"enabled":false,"default_enabled":false}"#
        );
    }

    fn valid_report() -> RumReport {
        serde_json::from_str(r#"{"version":1,"events":[{"type":"click"}]}"#)
            .expect("report with no accepted event")
    }

    /// A disabled deployment answers, but no report reaches a metric or a log.
    #[test]
    fn disabled_relay_records_nothing_and_stays_answerable() {
        let accepted_before = metrics::recorded_reports("accepted");
        let rejected_before = metrics::recorded_reports("rejected");

        let disabled = handle_report(valid_report(), false);
        assert_eq!(disabled.status(), StatusCode::NOT_FOUND);
        assert_eq!(metrics::recorded_reports("accepted"), accepted_before);
        assert_eq!(
            metrics::recorded_reports("rejected"),
            rejected_before,
            "a disabled relay must not touch the report counter"
        );

        let enabled = handle_report(valid_report(), true);
        assert_eq!(enabled.status(), StatusCode::ACCEPTED);
        assert_eq!(
            metrics::recorded_reports("rejected"),
            rejected_before + 1,
            "an enabled relay still accounts for the report it processed"
        );
    }
}
