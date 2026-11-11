# Piping.App metrics and SLO contract

Owner: `piping-space`, Piping.App.

Scope: the `backend` service (API, background workers, queues, database pool,
outbound dependencies) and the business signals that backend already owns.
Browser RUM and product analytics are relayed by the backend and recorded here;
their collection is consent-gated in the web frontend and their privacy
boundary is the relay endpoint.

This document is the contract between the code that emits metrics and the
dashboards, alerts and runbooks that consume them. A metric name, label
vocabulary or target changes here first; changing a threshold to make a
dashboard green is not a valid change.

## Metric families

All families are registered once at startup from
`apps/api/src/observability/metrics.rs` and
`apps/api/src/observability/metrics/runtime.rs`; external calls additionally
register from `apps/api/src/observability/metrics/external.rs`. The scrape
endpoint is `GET /metrics` (also `GET /api/v1/metrics`), gated by the
monitoring token from `PIPING_MONITORING__METRICS_TOKEN`.

Label vocabularies are closed. A value outside the vocabulary is ignored by the
recorder instead of creating a new series. No label carries a URL, host, user
id, provider id, session id, query text or request path.

### API

| Family | Type | Labels | Emitted by |
| --- | --- | --- | --- |
| `piping_http_requests_total` | counter | `method`, `route`, `status` | request metrics middleware |
| `piping_http_request_duration_seconds` | histogram | `method`, `route` | request metrics middleware |
| `piping_http_requests_in_flight` | gauge | `method`, `route` | request metrics middleware |
| `piping_api_token_auth_total` | counter | `result`, `reason` | API token authentication |
| `piping_build_info` | gauge | `version` | startup |

### Workers

| Family | Type | Labels | Emitted by |
| --- | --- | --- | --- |
| `piping_worker_heartbeat_timestamp_seconds` | gauge | `worker` | every worker loop start and tick |
| `piping_worker_ticks_total` | counter | `worker`, `outcome` | worker tick guard |
| `piping_worker_tick_duration_seconds` | histogram | `worker` | worker tick guard |

Workers: `connection_reconciliation`, `outbox`, `webhook_event_pruner`,
`journal_pruner`, `license_refresh`, `playbook_run`, `playbook_cancel`,
`observability_poller`, `playbook_schedule`. The list is the `Worker` enum, so
a worker cannot be added without a heartbeat label.

### Queues

| Family | Type | Labels | Emitted by |
| --- | --- | --- | --- |
| `piping_outbox_events` | gauge | `state` | outbox worker sample, every 15 s |
| `piping_outbox_oldest_pending_age_seconds` | gauge | — | outbox worker sample, every 15 s |
| `piping_outbox_batches_total` | counter | `outcome` | outbox worker tick |
| `piping_playbook_runs` | gauge | `status` | playbook run worker tick |
| `piping_playbook_oldest_queued_age_seconds` | gauge | — | playbook run worker tick |
| `piping_playbook_run_outcomes_total` | counter | `outcome` | playbook completion |
| `piping_playbook_run_duration_seconds` | histogram | `outcome` | playbook completion |
| `piping_playbook_dispatch_retries_total` | counter | — | playbook dispatch |
| `piping_playbook_timeouts_total` | counter | — | playbook reaper |
| `piping_sync_jobs_running` | gauge | — | sync job execution |
| `piping_sync_job_outcomes_total` | counter | `outcome` | sync job execution |
| `piping_sync_job_duration_seconds` | histogram | `outcome` | sync job execution |
| `piping_sync_last_success_timestamp_seconds` | gauge | — | successful sync job |

### Database

| Family | Type | Labels | Emitted by |
| --- | --- | --- | --- |
| `piping_db_pool_connections` | gauge | `state` | readiness probe and outbox sample |
| `piping_db_pool_acquire_failures_total` | counter | — | readiness probe |
| `piping_metrics_sample_errors_total` | counter | `source` | sample failure |

Database query duration has no family: the pool does not expose per-query
timing, and the request histogram already covers the end-to-end latency that
users observe. Pool saturation and acquire failures are the database USE
signals.

### External calls

| Family | Type | Labels | Emitted by |
| --- | --- | --- | --- |
| `piping_external_calls_total` | counter | `target`, `operation`, `outcome` | external call guard |
| `piping_external_calls_duration_seconds` | histogram | `target`, `operation` | external call guard |

Targets: `netbox`, `ansible_runner`, `proxy_node`, `webhook`, `monitoring`,
`licensing`, `oidc`. Outcome: `success`, `error`. A call that returns without
an explicit outcome records `error`, so a forgotten success path over-reports
failures instead of hiding them.

### Business and licensing

| Family | Type | Labels | Emitted by |
| --- | --- | --- | --- |
| `piping_connection_sessions` | gauge | `protocol`, `state` | connection reconciliation tick |
| `piping_connection_session_events_total` | counter | `protocol`, `event` | session created and ended paths |
| `piping_auth_logins_total` | counter | `method`, `outcome` | local and OIDC sign-in paths |
| `piping_license_seats` | gauge | `state` | license refresh |
| `piping_license_expiry_timestamp_seconds` | gauge | — | license refresh |
| `piping_license_activation_total` | counter | `result`, `reason` | activation and refresh |
| `piping_license_anomalies_total` | counter | `kind`, `source` | license verification |
| `piping_service_discovery_requests_total` | counter | `result` | service discovery |
| `piping_service_discovery_targets` | gauge | `kind` | service discovery |
| `piping_service_discovery_last_success_timestamp_seconds` | gauge | `kind` | service discovery |
| `piping_cache_operations_total` | counter | `cache`, `operation`, `result` | Redis cache |
| `piping_cache_operation_duration_seconds` | histogram | `cache`, `operation` | Redis cache |

Protocol vocabulary: `app`, `rdp`, `ssh`, `vnc`, `cctv`. Session state:
`pending`, `active`. Session event: `started`, `ended`. Auth method: `local`,
`oidc`; outcome: `success`, `failure`.

### Browser RUM (web)

The SPA reads `GET /api/v1/rum/config` for `{enabled, default_enabled}` and
posts bounded reports to `POST /api/v1/rum/report`. Both endpoints are public
(no session), the report route has its own 8 KiB body limit, rejects reports
over 20 events, and is guarded by a process-wide token bucket. The relay never
forwards the browser to the collector: the collector stays cluster-internal and
the backend records these families itself.

`observability.browser_telemetry_enabled` (default `true`) is the operator switch
for the whole relay. When it is `false` the config endpoint reports
`enabled: false` and a report is answered `404` without touching a metric, an
OTLP point or a log record. `observability.browser_telemetry_default_enabled`
(default `false`) is only the default for a browser with no saved choice. The
Compose preset sets both from the single `PIPING_TELEMETRY_ENABLED` variable
together with `otel_enabled`, so a standalone deployment keeps the previous
behaviour: the relay is available and telemetry is opt-in.

The families below are recorded twice from the same bounded call: into the
Prometheus scrape registry, and through OTLP metrics with the same names and
label keys once `otel_enabled` has installed the meter provider. A deployment
that scrapes only Prometheus loses no series; one that exports only OTLP gets
the browser vitals without a scrape.

| Family | Type | Labels | Emitted by |
| --- | --- | --- | --- |
| `piping_web_vitals_ms` | histogram | `release`, `metric`, `rating` | relay, Web Vital timings |
| `piping_web_vitals_cls` | histogram | `release`, `rating` | relay, cumulative layout shift |
| `piping_web_js_errors_total` | counter | `release`, `source`, `route` | relay, browser JS errors |
| `piping_web_api_failures_total` | counter | `release`, `route`, `status_class` | relay, browser-observed API failures |
| `piping_web_events_total` | counter | `release`, `event`, `funnel` | relay, consent-gated product events |
| `piping_web_reports_total` | counter | `outcome` | relay request handling |

Vital metrics: `lcp`, `cls`, `inp`, `fcp`, `ttfb`. Rating: `good`,
`needs-improvement`, `poor`, computed by the relay from Core Web Vitals
thresholds, never by the browser. Error source: `uncaught`,
`unhandled_rejection`, `react`, `api`. Status class: `4xx`, `5xx`. Product
events and their funnel are a three-entry code contract in
`apps/api/src/observability/metrics/web.rs`; unknown names are ignored.

`route` is one segment from a closed list, for example `connections` or
`playbooks`; anything else is `other`. The relay also accepts a full path but
discards everything outside that segment. `release` is the backend build's own
`PRODUCT_VERSION`, not a browser string, so a report is attributed to the
deployed release without trusting the client.

A JS error is counted and, when its stack survives sanitisation, logged once as
`web_rum.js_error` with frames reduced to `file:line:column`. Error messages,
function names, origins, query strings, fragments, user/session ids and form
values never enter a metric label or a log record. Consent is enforced in the
browser before anything is queued; the backend cannot see a browser-local
choice, so it supplies only the default and the operator switch.

Source maps are a private release artifact: the `frontend:sourcemaps` CI job
builds with hidden maps, moves every map out of `dist` into
`sourcemaps/<release>/`, and attaches that directory to the job. The manifest
records each bundle-to-map binding and lists the chunks Rolldown emitted without
a map (pure re-export facades, the runtime and the public worker). The image and
the public site contain no maps.

## Reading rules

- Absence of a series is not zero. A worker that never started has a primed
  zero heartbeat, but a family that failed to register has nothing at all;
  judge both with `absent()` and `last_over_time`, never with `or 0`.
- Gauges sampled by workers are as fresh as their sample interval. The outbox
  sample runs at most every 15 s while the outbox loop polls normally, and less
  often while its retry backoff is engaged; the connection session sample runs
  with the reconciliation loop every 10 s. Use `last_over_time(...[5m])` where a
  single scrape could miss the sample.
- Histogram quantiles aggregate over `le` across routes or workers. Do not take
  a maximum of per-route quantiles and call it a service percentile.
- A failed sample increments `piping_metrics_sample_errors_total` and leaves
  the previous gauge in place. A stale value plus a growing sample-error counter
  means the signal is unavailable, not healthy.
- `piping_http_requests_total`, `piping_http_request_duration_seconds`,
  `piping_db_pool_connections` and `piping_build_info` are also emitted by the
  Piping.Cloud backend under its own job. Every dashboard and alert query selects
  `job="piping-app-backend"`; without the selector two products share one series
  name and the panels silently mix them.

## SLIs, targets and windows

Targets are the initial contract for this story. Changing a target or deleting
an SLO needs its own issue with the product owner.

### API availability

SLI: share of successful (non-5xx) API responses.

```promql
sum(rate(piping_http_requests_total{status!~"5.."}[5m]))
/
sum(rate(piping_http_requests_total[5m]))
```

Target: 99.5% or better over 24h and over 7d. Error budget: 0.5% of requests.

### API latency

SLI: p95 and p99 of `piping_http_request_duration_seconds`.

```promql
histogram_quantile(0.95, sum by (le) (rate(piping_http_request_duration_seconds_bucket[5m])))
histogram_quantile(0.99, sum by (le) (rate(piping_http_request_duration_seconds_bucket[5m])))
```

Target: p95 at most 500 ms and p99 at most 2 s over 24h. Interactive inventory
and connection routes aim at p95 at most 300 ms; that aim is shown on the
dashboard but pages only on the 500 ms threshold.

### Worker liveness

SLI: seconds since the newest heartbeat for each worker.

```promql
time() - piping_worker_heartbeat_timestamp_seconds
```

Heartbeat thresholds depend on the worker's polling interval:

| Workers | Poll interval | Stale threshold |
| --- | --- | --- |
| All except the workers below | Configured short polling interval (`connection_reconciliation`: 10 s) | 300 s |
| `license_refresh` | 6 h by default | 64,800 s (18 h) |
| `journal_pruner`, `webhook_event_pruner` | 24 h | 259,200 s (72 h) |

The slow-worker thresholds allow three poll intervals. A zero heartbeat means
the task never ran after startup; a missing series is not a healthy worker.

### Queue freshness

SLI: age of the oldest deliverable outbox event.

```promql
piping_outbox_oldest_pending_age_seconds
```

Target: at most 60 s for 95% of the 24h window, at most 300 s instant.
Terminal failures:

```promql
sum(piping_outbox_events{state="dead_letter"})
sum(rate(piping_outbox_batches_total{outcome="error"}[5m]))
/
clamp_min(sum(rate(piping_outbox_batches_total[5m])), 0.001)
```

Target: zero dead-letter events and a failed-batch share under 1% over 24h.
Both batch outcomes count one sample per batch, so the share is a ratio of
batches, not a mix of batches and events. A dead-letter event pages after 15
minutes.

Playbook queue uses `piping_playbook_oldest_queued_age_seconds` with a 900 s
page threshold because a playbook run is expected to start slowly.

### Database saturation

SLI: acquired connections over configured maximum.

```promql
sum by (instance) (piping_db_pool_connections{state="acquired"})
/
clamp_min(sum by (instance) (piping_db_pool_connections{state="max"}), 1)
```

Both sides aggregate by `instance` before dividing. The selectors carry
different `state` values, so a raw division matches no pair and returns an empty
result; the aggregate keeps one ratio per scraped instance.

Target: below 0.85 over 24h. Page at 0.9 for 10 minutes with at least one
acquire failure in the same window.

### External dependency health

SLI: error share and p95 duration per target.

```promql
sum by (target) (rate(piping_external_calls_total{outcome="error"}[5m]))
/
clamp_min(sum by (target) (rate(piping_external_calls_total[5m])), 0.001)

histogram_quantile(0.95, sum by (le, target) (rate(piping_external_calls_duration_seconds_bucket[5m])))
```

Target: under 5% errors per target over 24h, with a 10% page threshold over
10 minutes and a floor of 0.05 requests per second so a rarely used target
cannot page on a single failure.

### Business signals

- `sum by (protocol) (piping_connection_sessions{state="active"})` is the live
  session population.
- `sum by (protocol) (rate(piping_connection_session_events_total{event="started"}[5m]))`
  and the `ended` counterpart are the session churn.
- `sum by (method, outcome) (rate(piping_auth_logins_total[5m]))` is sign-in
  volume; a rising `failure` share is a sign-in degradation signal, not an
  alert on its own. A failure sample covers a rejected credential or provider
  exchange and a license-gate rejection; malformed requests, expired callback
  state and a disabled local provider are not counted as login failures.

These are informational for capacity and product judgement. Only the API,
worker, queue, database and external panels page.

## Windows

- 5m: instant alert condition and dashboard "now" line.
- 24h: short SLO window and error-budget burn for the daily review.
- 7d: the SLO window used for the error budget and the weekly review.

A condition that is only true on 5m is a spike; a condition true on 24h and 7d
is an SLO breach. Multi-window burn alerts use the 1h long window and 5m short
window; both must exceed the burn threshold.

## Runbook

First checks per page:

1. API availability or latency: read the 5xx share by `route` and compare with
   `piping_db_pool_connections`, `piping_external_calls_total` and
   `piping_worker_heartbeat_timestamp_seconds` in the same window. A single
   route with 5xx is a code or dependency defect; all routes together are
   infrastructure.
2. Worker heartbeat stale: check the pod is running and the worker loop log
   events (`*.worker_tick.failed`, `*.recovered`). Do not restart before the
   heartbeat age and the tick error share are both recorded.
3. Outbox freshness or dead letters: read `piping_outbox_events` by state and
   the latest `sync.outbound.event.process.failed` records. Replay uses the
   existing dead-letter replay path, not a manual SQL update.
4. Database saturation: read acquired, idle and max together with
   `piping_db_pool_acquire_failures_total`; sustained 0.9 saturation is a pool
   or query-cost problem, not a reason to raise `max_connections` first.
5. External target errors: read the target in `piping_external_calls_total` and
   the matching structured failure events. NetBox, the runner sidecar, proxy
   nodes, monitoring providers and the licensing control plane each own their
   own outage procedure; this contract only proves the call failed.

## Grafana follow-up (repository `piping-space/infrastructure/grafana`)

Dashboards and alert rules are not part of this repository or this change. The
contract above needs one dashboard and one rule group in the Grafana repository.
Until they exist, the series are available at `GET /metrics` on the backend.

Proposed dashboard file: `grafana/dashboards/20-services/piping-app-api.json`,
uid `piping-app-api`, tags `ui:services` and `piping-app`, folder `Сервисы`.

Panels, in this order:

1. `API availability (5m)` — stat, `sum(rate(piping_http_requests_total{status!~"5.."}[5m])) / sum(rate(piping_http_requests_total[5m]))`, unit `percentunit`, thresholds 0.995 and 0.99.
2. `API availability 24h / 7d` — two stats over `[24h]` and `[7d]`.
3. `API latency p95 / p99` — timeseries of both histogram quantiles, unit `s`.
4. `5xx by route` — timeseries `sum by (route) (rate(piping_http_requests_total{status=~"5.."}[5m]))`.
5. `Worker heartbeats` — timeseries of `time() - piping_worker_heartbeat_timestamp_seconds` per `worker`, unit `s`. Use the worker-specific stale thresholds above, not a shared 300 s line.
6. `Worker tick error share` — timeseries `sum by (worker) (rate(piping_worker_ticks_total{outcome="error"}[5m])) / clamp_min(sum by (worker) (rate(piping_worker_ticks_total[5m])), 0.001)`.
7. `Outbox queue` — timeseries `piping_outbox_events` by `state` plus `piping_outbox_oldest_pending_age_seconds`.
8. `Outbox results` — timeseries `sum(rate(piping_outbox_batches_total[5m])) by (outcome)`.
9. `Playbook queue` — timeseries `piping_playbook_oldest_queued_age_seconds` and `piping_playbook_runs` by `status`.
10. `Database pool` — timeseries `piping_db_pool_connections` by `state` and the acquired/max ratio.
11. `External calls` — timeseries error share per `target` and p95 per `target`.
12. `Sessions and sign-ins` — timeseries `sum by (protocol) (piping_connection_sessions{state="active"})` and `sum by (method, outcome) (rate(piping_auth_logins_total[5m]))`.

Proposed alert rules appended to
`grafana/provisioning/alerting/piping-rules.yml`, group `PipingAppAlerts`,
folder `Сервисы`, `interval: 1m`:

| uid | condition | `for` |
| --- | --- | --- |
| `piping_app_api_5xx_ratio` | availability SLI below 0.98 with at least 0.05 req/s | 10m |
| `piping_app_api_latency_p95` | p95 above 0.5 s with at least 0.05 req/s | 10m |
| `piping_app_worker_heartbeat_stale` | heartbeat age above 300 s, excluding `license_refresh`, `journal_pruner`, `webhook_event_pruner` | 5m |
| `piping_app_license_heartbeat_stale` | `license_refresh` heartbeat age above 64,800 s | 5m |
| `piping_app_pruner_heartbeat_stale` | `journal_pruner` or `webhook_event_pruner` heartbeat age above 259,200 s | 5m |
| `piping_app_outbox_stale` | `piping_outbox_oldest_pending_age_seconds > 300` | 10m |
| `piping_app_outbox_dead_letter` | `sum(piping_outbox_events{state="dead_letter"}) > 0` | 15m |
| `piping_app_db_pool_saturation` | acquired/max above 0.9 | 10m |
| `piping_app_external_error_ratio` | per-target error share above 0.1 with at least 0.05 req/s | 10m |
| `piping_app_playbook_queue_age` | `piping_playbook_oldest_queued_age_seconds > 900` | 15m |

Every rule carries `severity`, `service="piping-app"`, an impact annotation and
a runbook link to this document. Rules use `noDataState: OK` only where the
series is expected to be absent before first traffic; worker heartbeat and
queue rules use `NoData` so a vanished target pages instead of passing.
