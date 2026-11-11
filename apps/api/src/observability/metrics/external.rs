//! RED metrics for outbound calls to external systems.
//!
//! Every outbound provider call passes through an [`ExternalCallGuard`]: the
//! guard starts a timer and records exactly one counter sample plus one
//! duration observation when it finishes. A call that returns without an
//! explicit outcome records `error`, so a forgotten success path can only
//! over-report failures, never hide them.
//!
//! Labels are bounded: the target is a closed enum and the operation is a
//! static literal supplied by the call site. Provider URLs, hosts, query text
//! and credentials never reach a label.

use std::time::Instant;

use once_cell::sync::OnceCell;
use prometheus::{
    HistogramOpts, HistogramVec, IntCounterVec, Opts, register_histogram_vec_with_registry,
    register_int_counter_vec_with_registry,
};

/// External system a call is made to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalTarget {
    Netbox,
    AnsibleRunner,
    ProxyNode,
    Webhook,
    Monitoring,
    Licensing,
    Oidc,
}

impl ExternalTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            ExternalTarget::Netbox => "netbox",
            ExternalTarget::AnsibleRunner => "ansible_runner",
            ExternalTarget::ProxyNode => "proxy_node",
            ExternalTarget::Webhook => "webhook",
            ExternalTarget::Monitoring => "monitoring",
            ExternalTarget::Licensing => "licensing",
            ExternalTarget::Oidc => "oidc",
        }
    }
}

const SUCCESS: &str = "success";
const ERROR: &str = "error";

struct ExternalMetrics {
    calls_total: IntCounterVec,
    duration_seconds: HistogramVec,
}

fn build(registry: &prometheus::Registry) -> Result<ExternalMetrics, prometheus::Error> {
    Ok(ExternalMetrics {
        calls_total: register_int_counter_vec_with_registry!(
            Opts::new(
                "piping_external_calls_total",
                "Total outbound external calls by target, operation and outcome",
            ),
            &["target", "operation", "outcome"],
            registry
        )?,
        duration_seconds: register_histogram_vec_with_registry!(
            HistogramOpts::new(
                "piping_external_calls_duration_seconds",
                "Outbound external call duration by target and operation",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0
            ]),
            &["target", "operation"],
            registry
        )?,
    })
}

static EXTERNAL_METRICS: OnceCell<ExternalMetrics> = OnceCell::new();

/// Fallible startup registration. A duplicate family name is returned as
/// `Err(AlreadyReg)` so the process can fail fast at boot; callers that touch a
/// metric before startup go through the private `external_metrics` accessor.
pub(super) fn init_metrics() -> Result<(), prometheus::Error> {
    EXTERNAL_METRICS.get_or_try_init(|| build(prometheus::default_registry()))?;
    Ok(())
}

#[expect(
    clippy::expect_used,
    reason = "duplicate metric name is a programmer error already surfaced by init_metrics() at startup"
)]
fn external_metrics() -> &'static ExternalMetrics {
    EXTERNAL_METRICS.get_or_init(|| {
        build(prometheus::default_registry()).expect("register external call metrics")
    })
}

/// One in-flight outbound call. Records exactly one outcome and one duration.
pub struct ExternalCallGuard {
    target: ExternalTarget,
    operation: &'static str,
    started_at: Instant,
    recorded: bool,
}

impl ExternalCallGuard {
    /// The call finished with the outcome the caller asked for.
    pub fn succeeded(&mut self) {
        self.finish(SUCCESS);
    }

    /// The call failed: transport error, non-success response or unusable body.
    pub fn failed(&mut self) {
        self.finish(ERROR);
    }

    fn finish(&mut self, outcome: &'static str) {
        if self.recorded {
            return;
        }
        self.recorded = true;
        let metrics = external_metrics();
        metrics
            .calls_total
            .with_label_values(&[self.target.as_str(), self.operation, outcome])
            .inc();
        metrics
            .duration_seconds
            .with_label_values(&[self.target.as_str(), self.operation])
            .observe(self.started_at.elapsed().as_secs_f64());
    }
}

impl Drop for ExternalCallGuard {
    fn drop(&mut self) {
        self.finish(ERROR);
    }
}

/// Begin timing an outbound call. `operation` is a static literal owned by the
/// call site, never a request path or provider-supplied value.
pub fn start_external_call(target: ExternalTarget, operation: &'static str) -> ExternalCallGuard {
    ExternalCallGuard {
        target,
        operation,
        started_at: Instant::now(),
        recorded: false,
    }
}
