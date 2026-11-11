//! Browser RUM metrics: Web Vitals, JS/API failures and product funnel events.
//!
//! The browser never reaches the OTLP collector directly. It posts bounded
//! reports to the public relay in `api::rest::routes::rum`, which validates the
//! payload and records these families twice from the same call: into the
//! Prometheus registry the backend scrape publishes, and through the OTLP
//! meter provider when it is active. A deployment that scrapes only Prometheus
//! loses no series.
//!
//! Labels are closed vocabularies owned by this module. A caller cannot create
//! a series from a route, URL, error message or user-supplied string: the relay
//! normalises every value first and unknown values are dropped. No label carries
//! a query string, user or session id, identifier or form value.
//!
//! `release` is the backend build's own `PRODUCT_VERSION`, not a browser
//! string. Both images in one release share it, so a report is attributed to
//! the deployed bundle without trusting the client.

use once_cell::sync::OnceCell;
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, MeterProvider as _};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use prometheus::{
    HistogramOpts, HistogramVec, IntCounterVec, Opts, register_histogram_vec_with_registry,
    register_int_counter_vec_with_registry,
};

pub const VITAL_METRICS: [&str; 5] = ["lcp", "cls", "inp", "fcp", "ttfb"];
pub const JS_ERROR_SOURCES: [&str; 4] = ["uncaught", "unhandled_rejection", "react", "api"];
const REPORT_OUTCOMES: [&str; 3] = ["accepted", "rejected", "rate_limited"];

/// Fixed product vocabulary: event name -> funnel. The relay accepts only these
/// names, so a new product event is a code change here, not a client string.
pub const PRODUCT_EVENTS: [(&str, &str); 3] = [
    ("app_opened", "engagement"),
    ("connection_session_started", "activation"),
    ("playbook_run_started", "automation"),
];

/// First path segments that may appear in a `route` label. Anything else, and
/// every value that is not a bare segment, collapses to `other`.
const ROUTE_SEGMENTS: [&str; 30] = [
    "access-control",
    "admin",
    "api-docs",
    "assistant",
    "audit",
    "auth",
    "automation-secrets",
    "backup",
    "config-contexts",
    "connections",
    "custom-fields",
    "device-dashboard",
    "health",
    "inventory",
    "license",
    "licensing",
    "mcp",
    "metrics",
    "monitoring",
    "observability",
    "playbooks",
    "providers",
    "proxy-nodes",
    "secrets",
    "settings",
    "sources",
    "sync-jobs",
    "user-settings",
    "webhooks",
    "instance-branding",
];

const OTHER: &str = "other";

const VITALS_MS_BUCKETS: [f64; 12] = [
    50.0, 100.0, 200.0, 500.0, 800.0, 1_000.0, 1_800.0, 2_500.0, 4_000.0, 6_000.0, 10_000.0,
    30_000.0,
];
const VITALS_CLS_BUCKETS: [f64; 8] = [0.01, 0.025, 0.05, 0.1, 0.15, 0.25, 0.4, 1.0];

/// Largest Web Vital value accepted from the browser. A runaway or hostile
/// value would otherwise stretch histogram buckets and dashboards.
const MAX_VITAL_MS: f64 = 600_000.0;
const MAX_VITAL_CLS: f64 = 100.0;

struct WebMetrics {
    vitals_ms: HistogramVec,
    vitals_cls: HistogramVec,
    js_errors: IntCounterVec,
    api_failures: IntCounterVec,
    events: IntCounterVec,
    reports: IntCounterVec,
}

fn build(registry: &prometheus::Registry) -> Result<WebMetrics, prometheus::Error> {
    Ok(WebMetrics {
        vitals_ms: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_web_vitals_ms",
                "Browser Web Vitals timing metrics by release, metric and rating",
            )
            .buckets(VITALS_MS_BUCKETS.to_vec()),
            &["release", "metric", "rating"],
            registry
        )?,
        vitals_cls: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_web_vitals_cls",
                "Browser cumulative layout shift by release and rating",
            )
            .buckets(VITALS_CLS_BUCKETS.to_vec()),
            &["release", "rating"],
            registry
        )?,
        js_errors: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_web_js_errors_total",
                "Browser JS errors by release, source and bounded route",
            ),
            &["release", "source", "route"],
            registry
        )?,
        api_failures: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_web_api_failures_total",
                "API responses the browser observed as failures by release, route and status class",
            ),
            &["release", "route", "status_class"],
            registry
        )?,
        events: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_web_events_total",
                "Consent-gated product funnel events by release, event and funnel",
            ),
            &["release", "event", "funnel"],
            registry
        )?,
        reports: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_web_reports_total",
                "Browser RUM reports by relay outcome",
            ),
            &["outcome"],
            registry
        )?,
    })
}

static WEB_METRICS: OnceCell<WebMetrics> = OnceCell::new();

/// Fallible startup registration. A duplicate family fails startup like every
/// other metric family instead of being created lazily at first report.
pub(super) fn init_metrics() -> Result<(), prometheus::Error> {
    WEB_METRICS.get_or_try_init(|| build(prometheus::default_registry()))?;
    Ok(())
}

#[expect(
    clippy::expect_used,
    reason = "duplicate metric name is a programmer error already surfaced by init_metrics() at startup"
)]
fn web_metrics() -> &'static WebMetrics {
    WEB_METRICS.get_or_init(|| build(prometheus::default_registry()).expect("register web metrics"))
}

/// Instrumentation scope of the exported browser telemetry families.
const OTEL_SCOPE: &str = "piping-web-rum";

/// OTLP mirror of [`WebMetrics`], built from the startup meter provider.
///
/// Families, units and label keys match the Prometheus ones so a dashboard can
/// read either signal, and only already-normalised values reach these
/// instruments: every recorder validates its vocabulary before it writes.
struct WebOtel {
    vitals_ms: Histogram<f64>,
    vitals_cls: Histogram<f64>,
    js_errors: Counter<u64>,
    api_failures: Counter<u64>,
    events: Counter<u64>,
    reports: Counter<u64>,
}

impl WebOtel {
    fn from_provider(provider: &SdkMeterProvider) -> Self {
        let meter = provider.meter(OTEL_SCOPE);
        Self {
            vitals_ms: meter
                .f64_histogram("piping_web_vitals_ms")
                .with_unit("ms")
                .with_boundaries(VITALS_MS_BUCKETS.to_vec())
                .with_description(
                    "Browser Web Vitals timing metrics by release, metric and rating",
                )
                .build(),
            vitals_cls: meter
                .f64_histogram("piping_web_vitals_cls")
                .with_unit("1")
                .with_boundaries(VITALS_CLS_BUCKETS.to_vec())
                .with_description("Browser cumulative layout shift by release and rating")
                .build(),
            js_errors: meter
                .u64_counter("piping_web_js_errors_total")
                .with_description("Browser JS errors by release, source and bounded route")
                .build(),
            api_failures: meter
                .u64_counter("piping_web_api_failures_total")
                .with_description(
                    "API responses the browser observed as failures by release, route and status class",
                )
                .build(),
            events: meter
                .u64_counter("piping_web_events_total")
                .with_description("Consent-gated product funnel events by release, event and funnel")
                .build(),
            reports: meter
                .u64_counter("piping_web_reports_total")
                .with_description("Browser RUM reports by relay outcome")
                .build(),
        }
    }
}

static OTEL_WEB: OnceCell<WebOtel> = OnceCell::new();

/// OTLP instruments, created on first use from the installed meter provider.
///
/// `init_observability` registers the Prometheus families before it installs
/// the meter provider, so the instruments cannot be built at metric
/// registration time: a handle created against the pre-provider global meter
/// would stay a no-op. The first recorded report is always after startup, and
/// a failed lookup is retried rather than cached, so the instruments bind to
/// the real provider whenever it appears.
fn otel_web() -> Option<&'static WebOtel> {
    if let Some(instruments) = OTEL_WEB.get() {
        return Some(instruments);
    }
    let provider = crate::observability::meter_provider()?;
    Some(OTEL_WEB.get_or_init(|| WebOtel::from_provider(provider)))
}

/// Bind the OTLP instruments to a provider supplied by a test.
#[cfg(test)]
fn install_otel_for_test(provider: &SdkMeterProvider) -> bool {
    OTEL_WEB.set(WebOtel::from_provider(provider)).is_ok()
}

/// Current value of one relay-outcome counter, for the route contract tests.
///
/// Initialises a zero-valued series when it has not been recorded yet.
#[cfg(test)]
pub(crate) fn recorded_reports(outcome: &str) -> u64 {
    web_metrics()
        .reports
        .get_metric_with_label_values(&[outcome])
        .map(|counter| counter.get())
        .unwrap_or(0)
}

/// Collapse a browser path to one closed route label.
///
/// Accepts `/connections/abc?x=1` and `/api/v1/connections/sessions` alike; the
/// first segment decides. Query, fragment, identifiers and unknown segments
/// never reach a label.
pub fn bounded_route(raw: &str) -> &'static str {
    let path = raw.split(['?', '#']).next().unwrap_or("").trim();
    let path = path
        .strip_prefix("/api/v1/")
        .or_else(|| path.strip_prefix("/api/"))
        .unwrap_or(path);
    let segment = path.trim_start_matches('/').split('/').next().unwrap_or("");
    ROUTE_SEGMENTS
        .iter()
        .copied()
        .find(|known| *known == segment)
        .unwrap_or(OTHER)
}

/// Classify a Web Vital against its Core Web Vitals thresholds.
pub fn vital_rating(metric: &str, value: f64) -> &'static str {
    let (good, poor) = match metric {
        "lcp" => (2_500.0, 4_000.0),
        "inp" => (200.0, 500.0),
        "fcp" => (1_800.0, 3_000.0),
        "ttfb" => (800.0, 1_800.0),
        "cls" => (0.1, 0.25),
        _ => return "poor",
    };
    if value <= good {
        "good"
    } else if value <= poor {
        "needs-improvement"
    } else {
        "poor"
    }
}

fn valid_vital(metric: &str, value: f64) -> bool {
    if !value.is_finite() || value < 0.0 {
        return false;
    }
    match metric {
        "cls" => value <= MAX_VITAL_CLS,
        _ => value <= MAX_VITAL_MS,
    }
}

pub fn record_vital(metric: &str, value: f64) {
    if !VITAL_METRICS.contains(&metric) || !valid_vital(metric, value) {
        return;
    }
    let rating = vital_rating(metric, value);
    let metrics = web_metrics();
    if metric == "cls" {
        metrics
            .vitals_cls
            .with_label_values(&[crate::PRODUCT_VERSION, rating])
            .observe(value);
    } else {
        metrics
            .vitals_ms
            .with_label_values(&[crate::PRODUCT_VERSION, metric, rating])
            .observe(value);
    }
    let Some(otel) = otel_web() else {
        return;
    };
    if metric == "cls" {
        otel.vitals_cls.record(
            value,
            &[
                KeyValue::new("release", crate::PRODUCT_VERSION.to_string()),
                KeyValue::new("rating", rating.to_string()),
            ],
        );
    } else {
        otel.vitals_ms.record(
            value,
            &[
                KeyValue::new("release", crate::PRODUCT_VERSION.to_string()),
                KeyValue::new("metric", metric.to_string()),
                KeyValue::new("rating", rating.to_string()),
            ],
        );
    }
}

pub fn record_js_error(source: &str, route: &str) {
    if !JS_ERROR_SOURCES.contains(&source) {
        return;
    }
    web_metrics()
        .js_errors
        .with_label_values(&[crate::PRODUCT_VERSION, source, route])
        .inc();
    if let Some(otel) = otel_web() {
        otel.js_errors.add(
            1,
            &[
                KeyValue::new("release", crate::PRODUCT_VERSION.to_string()),
                KeyValue::new("source", source.to_string()),
                KeyValue::new("route", route.to_string()),
            ],
        );
    }
}

/// Status class is one of `4xx`, `5xx`; anything else is not a browser-observed
/// server failure and is ignored by the caller.
pub fn record_api_failure(route: &str, status_class: &str) {
    if !matches!(status_class, "4xx" | "5xx") {
        return;
    }
    web_metrics()
        .api_failures
        .with_label_values(&[crate::PRODUCT_VERSION, route, status_class])
        .inc();
    if let Some(otel) = otel_web() {
        otel.api_failures.add(
            1,
            &[
                KeyValue::new("release", crate::PRODUCT_VERSION.to_string()),
                KeyValue::new("route", route.to_string()),
                KeyValue::new("status_class", status_class.to_string()),
            ],
        );
    }
}

pub fn record_product_event(event: &str) {
    let Some((_, funnel)) = PRODUCT_EVENTS.iter().find(|(name, _)| *name == event) else {
        return;
    };
    web_metrics()
        .events
        .with_label_values(&[crate::PRODUCT_VERSION, event, funnel])
        .inc();
    if let Some(otel) = otel_web() {
        otel.events.add(
            1,
            &[
                KeyValue::new("release", crate::PRODUCT_VERSION.to_string()),
                KeyValue::new("event", event.to_string()),
                KeyValue::new("funnel", funnel.to_string()),
            ],
        );
    }
}

pub fn record_report(outcome: &str) {
    if !REPORT_OUTCOMES.contains(&outcome) {
        return;
    }
    web_metrics().reports.with_label_values(&[outcome]).inc();
    if let Some(otel) = otel_web() {
        otel.reports
            .add(1, &[KeyValue::new("outcome", outcome.to_string())]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
    use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
    use opentelemetry_sdk::metrics::{PeriodicReader, Temporality};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// One exported data point, flattened out of the SDK tree.
    #[derive(Debug, Clone, PartialEq)]
    struct CapturedPoint {
        name: String,
        attributes: Vec<(String, String)>,
        value: f64,
        count: u64,
    }

    impl CapturedPoint {
        fn labels(&self) -> BTreeMap<&str, &str> {
            self.attributes
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        }
    }

    #[derive(Clone, Default)]
    struct CapturingExporter {
        points: Arc<Mutex<Vec<CapturedPoint>>>,
    }

    impl CapturingExporter {
        fn points(&self) -> Vec<CapturedPoint> {
            self.points.lock().expect("captured metrics lock").clone()
        }
    }

    impl PushMetricExporter for CapturingExporter {
        async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
            let mut captured = self.points.lock().expect("captured metrics lock");
            for scope in metrics.scope_metrics() {
                for metric in scope.metrics() {
                    match metric.data() {
                        AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                            for point in sum.data_points() {
                                captured.push(CapturedPoint {
                                    name: metric.name().to_string(),
                                    attributes: point
                                        .attributes()
                                        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                                        .collect(),
                                    value: point.value() as f64,
                                    count: 0,
                                });
                            }
                        }
                        AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                            for point in histogram.data_points() {
                                captured.push(CapturedPoint {
                                    name: metric.name().to_string(),
                                    attributes: point
                                        .attributes()
                                        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                                        .collect(),
                                    value: point.sum(),
                                    count: point.count(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(())
        }

        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }

        fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
            Ok(())
        }

        fn temporality(&self) -> Temporality {
            Temporality::Cumulative
        }
    }

    fn capture_provider() -> (CapturingExporter, SdkMeterProvider) {
        let exporter = CapturingExporter::default();
        // Only the explicit flush below exports: the periodic interval is out
        // of the test's lifetime, so the captured batch is the one the
        // recorders produced.
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_secs(3_600))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        (exporter, provider)
    }

    /// The relay's own recorders must push the same bounded labels through the
    /// OTLP pipeline; an instrument created against a real provider is what
    /// makes `enabled` telemetry visible to the collector at all.
    #[test]
    fn recorders_export_bounded_labels_through_meter_provider() {
        let (exporter, provider) = capture_provider();
        assert!(
            install_otel_for_test(&provider),
            "only this test installs the capture provider"
        );

        record_vital("inp", 250.0);
        record_js_error("uncaught", "connections");
        record_product_event("app_opened");
        record_vital("secret_metric", 1.0);
        provider.force_flush().expect("flush metrics");

        let points = exporter.points();
        assert!(
            points
                .iter()
                .filter(|point| point.name == "piping_web_vitals_ms")
                .all(|point| point.labels().get("metric") != Some(&"secret_metric")),
            "out-of-vocabulary input must not create a series: {points:?}"
        );

        let vital = points
            .iter()
            .filter(|point| point.name == "piping_web_vitals_ms")
            .find(|point| point.labels().get("metric") == Some(&"inp"))
            .expect("vital histogram exported");
        assert_eq!(vital.value, 250.0);
        assert_eq!(vital.count, 1);
        assert_eq!(
            vital.labels(),
            BTreeMap::from([
                ("metric", "inp"),
                ("rating", "needs-improvement"),
                ("release", crate::PRODUCT_VERSION),
            ])
        );

        let js_error = points
            .iter()
            .find(|point| point.name == "piping_web_js_errors_total")
            .expect("js error counter exported");
        assert_eq!(js_error.value, 1.0);
        assert_eq!(
            js_error.labels(),
            BTreeMap::from([
                ("release", crate::PRODUCT_VERSION),
                ("route", "connections"),
                ("source", "uncaught"),
            ])
        );

        let event = points
            .iter()
            .find(|point| point.name == "piping_web_events_total")
            .expect("product event counter exported");
        assert_eq!(
            event.labels(),
            BTreeMap::from([
                ("event", "app_opened"),
                ("funnel", "engagement"),
                ("release", crate::PRODUCT_VERSION),
            ])
        );
    }

    #[test]
    fn route_label_is_a_closed_segment() {
        assert_eq!(
            bounded_route("/connections/host-1?token=secret"),
            "connections"
        );
        assert_eq!(bounded_route("/api/v1/playbooks/runs/123"), "playbooks");
        assert_eq!(bounded_route("/unknown/deep/path#frag"), OTHER);
        assert_eq!(bounded_route("https://evil.example/connections"), OTHER);
        assert_eq!(bounded_route(""), OTHER);
        assert_eq!(bounded_route("/"), OTHER);
    }

    #[test]
    fn vital_rating_matches_thresholds() {
        assert_eq!(vital_rating("lcp", 2_500.0), "good");
        assert_eq!(vital_rating("lcp", 2_500.1), "needs-improvement");
        assert_eq!(vital_rating("lcp", 4_000.1), "poor");
        assert_eq!(vital_rating("cls", 0.05), "good");
        assert_eq!(vital_rating("cls", 0.2), "needs-improvement");
        assert_eq!(vital_rating("unknown", 1.0), "poor");
    }

    #[test]
    fn invalid_vitals_are_out_of_vocabulary() {
        assert!(valid_vital("lcp", 1_000.0));
        assert!(!valid_vital("lcp", -1.0));
        assert!(!valid_vital("lcp", MAX_VITAL_MS + 1.0));
        assert!(!valid_vital("cls", MAX_VITAL_CLS + 1.0));
        assert!(!valid_vital("lcp", f64::NAN));
    }
}
