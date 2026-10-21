use std::time::Duration;

// ============================================================================
// Pagination Constants
// ============================================================================

/// Default pagination limit for REST list queries.
///
/// Used when no limit is specified by the client.
/// This provides a balance between performance and result completeness.
pub const DEFAULT_PAGINATION_LIMIT: u32 = 50;

/// Maximum allowed pagination limit to prevent excessive resource usage.
///
/// Clients requesting more than this limit will be clamped to this value.
/// Prevents memory exhaustion and database query timeouts.
pub const MAX_PAGINATION_LIMIT: u32 = 1000;

/// Maximum pagination limit for the connection-target workspace list
/// (`GET /api/v1/connections/targets`).
///
/// The connections workspace tree needs the *entire* target set to build its
/// grouping, and the underlying UNION read-model is recomputed per page. Fanning
/// out N paged round-trips therefore recomputes the whole projection N times; a
/// single request computes it once. This ceiling lets that one request succeed
/// for realistic inventories. Beyond it, the precomputed read-model
/// (materialized view) is the intended fix — not a higher ceiling.
pub const MAX_CONNECTION_TARGET_LIST_LIMIT: u32 = 50_000;

/// Default pagination limit for the sources provider list endpoint (`GET /api/v1/providers`).
pub const DEFAULT_SOURCES_LIST_LIMIT: usize = 100;

/// Minimum allowed pagination limit for the sources provider list endpoint.
pub const MIN_SOURCES_LIST_LIMIT: usize = 1;

/// Maximum allowed pagination limit for the sources provider list endpoint.
pub const MAX_SOURCES_LIST_LIMIT: usize = 500;

/// Default pagination offset for the sources provider list endpoint.
pub const DEFAULT_SOURCES_LIST_OFFSET: usize = 0;

// ============================================================================
// Cache TTL Constants
// ============================================================================

/// Default cache TTL (Time-To-Live) in seconds for general-purpose caching.
///
/// Used for Redis cache operations when no specific TTL is provided.
/// Value: 5 minutes (300 seconds)
pub const DEFAULT_CACHE_TTL_SECONDS: u64 = 300; // 5 minutes

/// Cache TTL for host configuration data.
///
/// Host configs are relatively stable but may change during deployments.
/// Value: 5 minutes (300 seconds)
pub const HOST_CONFIG_CACHE_TTL_SECONDS: u64 = DEFAULT_CACHE_TTL_SECONDS;

/// Short cache TTL for frequently changing data.
///
/// Use for data that changes often but benefits from brief caching
/// (e.g., connection status, recent activity).
/// Value: 1 minute (60 seconds)
pub const SHORT_CACHE_TTL_SECONDS: u64 = 60; // 1 minute

/// Long cache TTL for stable data that rarely changes.
///
/// Use for data that is expensive to compute but changes infrequently
/// (e.g., static configuration, reference data).
/// Value: 1 hour (3600 seconds)
pub const LONG_CACHE_TTL_SECONDS: u64 = 3600; // 1 hour

// ============================================================================
// Sync Constants
// ============================================================================

/// Redis lock TTL for sync jobs in seconds.
///
/// Prevents concurrent syncs per provider and avoids stale locks.
/// Value: 1 hour (3600 seconds)
pub const SYNC_LOCK_TTL_SECONDS: u64 = 3600; // 1 hour

// ============================================================================
// Configuration Context Constants
// ============================================================================

/// Minimum allowed weight for configuration context bindings.
///
/// Negative weights allow contexts to override defaults with lower priority.
/// Used in config context evaluation and merging.
pub const CONFIG_CONTEXT_WEIGHT_MIN: i32 = -1000;

/// Maximum allowed weight for configuration context bindings.
///
/// Higher weights give contexts higher priority in evaluation.
/// Used in config context evaluation and merging.
pub const CONFIG_CONTEXT_WEIGHT_MAX: i32 = 1000;

// ============================================================================
// Timeout Constants
// ============================================================================

/// Default timeout for database operations (queries, transactions).
///
/// Prevents queries from hanging indefinitely.
/// Value: 30 seconds
pub const DEFAULT_DB_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for HTTP client requests (external APIs, OIDC).
///
/// Used for outbound HTTP requests to prevent hanging connections.
/// Value: 10 seconds
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for Redis operations (get, set, delete).
///
/// Prevents Redis operations from blocking indefinitely.
/// Value: 5 seconds
pub const DEFAULT_REDIS_TIMEOUT: Duration = Duration::from_secs(5);

// ============================================================================
// Configuration defaults
// ============================================================================

// Server
/// Default maximum request body size for HTTP server: 100 MB.
pub const DEFAULT_SERVER_MAX_BODY_BYTES: usize = 100 * 1024 * 1024;

/// Default server port used as a fallback when parsing bind addresses fails.
pub const DEFAULT_SERVER_PORT: u16 = 8080;

// Database
/// Default maximum size of PostgreSQL connection pool.
pub const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;

/// Default minimum size of PostgreSQL connection pool.
pub const DEFAULT_DB_MIN_CONNECTIONS: u32 = 2;

/// Default wait timeout (seconds) for acquiring a PostgreSQL connection from pool.
pub const DEFAULT_DB_ACQUIRE_TIMEOUT_SECONDS: u64 = 30;

/// Default idle timeout (seconds) before PostgreSQL connection is closed.
pub const DEFAULT_DB_IDLE_TIMEOUT_SECONDS: u64 = 600;

/// Default maximum lifetime (seconds) of PostgreSQL connection before recycle.
pub const DEFAULT_DB_MAX_LIFETIME_SECONDS: u64 = 1800;

// Secrets
/// Default environment variable reference for secret material.
pub const DEFAULT_SECRETS_CIPHER_KEY_MATERIAL_REF: &str = "env:PIPING_SECRETS__CIPHER_KEY_B64";

/// Default cipher version for encrypted secrets.
pub const DEFAULT_SECRETS_CIPHER_VERSION: &str = "v1";

// Observability
/// Default log level for application logs.
pub const DEFAULT_OBSERVABILITY_LOG_LEVEL: &str = "INFO";

/// Default output format for application logs.
pub const DEFAULT_OBSERVABILITY_LOG_OUTPUT_FORMAT: &str = "json";

/// Default window size for noisy event suppression (milliseconds).
pub const DEFAULT_NOISE_WINDOW_MS: u64 = 60_000;

/// Default maximum number of repeated events per noise window.
pub const DEFAULT_NOISE_MAX_EVENTS_PER_WINDOW: u32 = 100;

/// Minimum number of events per noise window required when overflow mode is `summarize`.
pub const MIN_OBSERVABILITY_NOISE_MAX_EVENTS_PER_WINDOW_FOR_SUMMARIZE: u32 = 2;

/// Default overflow behavior for noise budgeting.
pub const DEFAULT_NOISE_OVERFLOW_MODE: &str = "summarize";

/// OTLP export is opt-in: the shipped default targets no collector, because the
/// estate has no external OTLP entry and a product default must not name
/// cluster-internal DNS. Operators enable telemetry per deployment with
/// `PIPING_OBSERVABILITY__OTEL_ENABLED=true` plus an explicit endpoint.
pub const DEFAULT_OBSERVABILITY_OTEL_ENABLED: bool = false;

/// Default OTLP endpoint is empty: the endpoint is set per deployment. The
/// estate's k8s contour sets `OTEL_EXPORTER_OTLP_ENDPOINT` to the in-cluster
/// collector (`http://otel-collector.monitoring.svc.piping-eu:4318`).
pub const DEFAULT_OBSERVABILITY_OTEL_EXPORTER_OTLP_ENDPOINT: &str = "";

/// Fallback noise policy window (milliseconds) used when logging is initialized
/// before typed settings are available.
pub const DEFAULT_NOISE_FALLBACK_WINDOW_MS: u64 = 30_000;

/// Fallback max events per noise window used when logging is initialized
/// before typed settings are available.
pub const DEFAULT_NOISE_FALLBACK_MAX_EVENTS_PER_WINDOW: u32 = 40;

// Monitoring
/// Default service-discovery port.
pub const DEFAULT_SD_DEFAULT_PORT: u16 = 9100;

/// Default service-discovery scheme.
pub const DEFAULT_SD_DEFAULT_SCHEME: &str = "http";

/// Default path for service-discovery metrics.
pub const DEFAULT_SD_DEFAULT_METRICS_PATH: &str = "/metrics";

/// Default job label in service discovery.
pub const DEFAULT_SD_DEFAULT_JOB: &str = "node";

/// Default maximum discovery query limit.
pub const DEFAULT_SD_MAX_LIMIT: u32 = 50000;

/// Default timeout for Prometheus-compatible instant query calls.
pub const DEFAULT_MONITORING_QUERY_TIMEOUT_MS: u64 = 3000;

/// Default maximum number of instant queries accepted by the UI metrics endpoint.
pub const DEFAULT_MONITORING_QUERY_MAX_QUERIES: u32 = 100;

/// Default poll interval (milliseconds) for the observability list-labels poller.
pub const DEFAULT_MONITORING_LIST_POLL_INTERVAL_MS: u64 = 60000;

/// Default for `monitoring.default_set_fallback_enabled`: when no
/// explicit `monitoring_metric_set` is assigned to a host, surface every
/// enabled metric query that opts in via `default_show_in_list`.
pub const DEFAULT_MONITORING_DEFAULT_SET_FALLBACK_ENABLED: bool = true;

// Authentication
/// Default local auth session TTL (seconds): 24 hours.
pub const DEFAULT_AUTH_SESSION_TTL_SECONDS: u64 = 86_400;

/// Default local auth session cookie name.
pub const DEFAULT_AUTH_COOKIE_NAME: &str = "piping_session";

/// Default session cookie secure flag in production-like setups.
pub const DEFAULT_AUTH_COOKIE_SECURE: bool = true;

/// Default session cookie SameSite policy.
pub const DEFAULT_AUTH_COOKIE_SAME_SITE: &str = "Lax";

/// Default minimum password length for local auth.
pub const DEFAULT_AUTH_PASSWORD_MIN_LENGTH: usize = 8;

/// Default OIDC scopes.
pub const DEFAULT_AUTH_OIDC_SCOPES: [&str; 3] = ["openid", "profile", "email"];

/// Default OIDC user group claim key.
pub const DEFAULT_AUTH_OIDC_GROUP_CLAIM: &str = "groups";

/// Default OIDC clock skew tolerance in seconds.
pub const DEFAULT_AUTH_OIDC_CLOCK_SKEW_SECONDS: u64 = 30;

/// Default API token auth enabled flag.
pub const DEFAULT_AUTH_API_TOKENS_ENABLED: bool = true;

/// Default maximum number of active API tokens per user.
pub const DEFAULT_AUTH_API_TOKENS_MAX_ACTIVE_PER_USER: u32 = 20;

/// Default minimum allowed API token TTL in seconds.
pub const DEFAULT_AUTH_API_TOKENS_MIN_TTL_SECONDS: u64 = 60;

/// Default maximum allowed API token TTL in seconds (365 days).
pub const DEFAULT_AUTH_API_TOKENS_MAX_TTL_SECONDS: u64 = 31_536_000;

/// Default API token TTL in seconds (0 means no expiration by default).
pub const DEFAULT_AUTH_API_TOKENS_DEFAULT_TTL_SECONDS: u64 = 0;

/// Default number of random bytes used for API token secret generation.
pub const DEFAULT_AUTH_API_TOKENS_SECRET_BYTES: u32 = 32;

/// Default per-token MCP endpoint request limit per 60-second window.
///
/// This is higher than an interactive UI rate but caps runaway agent loops using
/// broad API tokens.
pub const DEFAULT_AUTH_API_TOKENS_MCP_RATE_LIMIT_PER_MINUTE: u32 = 300;

/// Default proxy token TTL (seconds).
pub const DEFAULT_PROXY_WS_TOKEN_TTL_SECONDS: u64 = 120;

/// Default grace window (seconds) for a detached-but-alive SSH session.
///
/// After the client WebSocket leg drops, the server-side SSH PTY + child
/// process are kept alive for this long so a re-attach to the same session
/// can resume the running shell. Once the window elapses without a re-attach,
/// the proxy reaper shuts the session down and frees the registry slot.
pub const DEFAULT_PROXY_SSH_DETACH_GRACE_SECONDS: u64 = 60;

/// Default proxy URL validation timeout (milliseconds).
pub const DEFAULT_PROXY_VALIDATION_TIMEOUT_MS: u64 = 3000;

/// Default proxy connect timeout in seconds.
///
/// This is used as the default when no proxy connect timeout is configured.
pub const DEFAULT_PROXY_CONNECT_TIMEOUT_SECS: f64 = 15.0;

/// Minimum allowed proxy connect timeout in seconds.
pub const MIN_PROXY_CONNECT_TIMEOUT_SECS: f64 = 1.0;

/// Maximum allowed proxy connect timeout in seconds.
pub const MAX_PROXY_CONNECT_TIMEOUT_SECS: f64 = 300.0;

/// Default maximum limit for proxy admin list pagination.
pub const DEFAULT_PROXY_ADMIN_LIST_MAX_LIMIT: u32 = 500;

/// Default maximum SSH file upload size: 512 MiB.
pub const DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// Default maximum SSH file download size: 512 MiB.
pub const DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// Default proxy-wide concurrent SSH file sidecar limit.
pub const DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_SIDECARS_TOTAL: usize = 8;

/// Default per-session concurrent SSH file sidecar limit.
pub const DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_SIDECARS_PER_SESSION: usize = 2;

/// Minimum interval between proxy metrics samples (milliseconds).
pub const PROXY_METRICS_RATE_MIN_INTERVAL_MS: i64 = 200;

/// Latency threshold (milliseconds) considered \"good\" for proxy quality summaries.
pub const PROXY_QUALITY_GOOD_LATENCY_MS: i64 = 50;

/// Latency threshold (milliseconds) considered \"warning\" for proxy quality summaries.
pub const PROXY_QUALITY_WARNING_LATENCY_MS: i64 = 150;

/// Default guacd host for RDP/Guacamole integrations.
pub const DEFAULT_GUACD_HOST: &str = "127.0.0.1";

/// Default guacd port for RDP/Guacamole integrations.
pub const DEFAULT_GUACD_PORT: u16 = 4822;

/// Maximum bytes allowed per WebSocket control message element.
pub const PROXY_WS_CONTROL_MAX_ELEMENT_BYTES: usize = 8 * 1024 * 1024;

/// Maximum number of elements allowed in a single WebSocket control message.
pub const PROXY_WS_CONTROL_MAX_ELEMENTS: usize = 128;

// Sync runtime
/// Default callback base URL for sync runtime.
pub const DEFAULT_SYNC_CALLBACK_BASE_URL: &str = "http://localhost:8080";

/// Default webhook signature age acceptance window (seconds).
pub const DEFAULT_SYNC_WEBHOOK_SIGNATURE_WINDOW_SECONDS: u64 = 300;

/// Default webhook dedup window (seconds).
pub const DEFAULT_SYNC_WEBHOOK_DEDUP_WINDOW_SECONDS: u64 = 3600;

/// Default outbox batch size.
pub const DEFAULT_SYNC_OUTBOX_WORKER_BATCH_SIZE: u32 = 100;

/// Default outbox poll interval (milliseconds).
pub const DEFAULT_SYNC_OUTBOX_WORKER_POLL_INTERVAL_MS: u64 = 1000;

/// Default maximum number of outbox retries.
pub const DEFAULT_SYNC_OUTBOX_RETRY_MAX_ATTEMPTS: u32 = 8;

/// Default backoff for outbox retries (milliseconds).
pub const DEFAULT_SYNC_OUTBOX_RETRY_BACKOFF_MS: u64 = 2000;

/// Default outbox dedup retention (hours).
pub const DEFAULT_SYNC_DEDUP_RETENTION_HOURS: u64 = 24;

// Playbook runtime (executor data-plane)
/// Default self-callback base URL the remote wrapper curls back to
/// (`PIPING_API_URL`). Must be reachable from target hosts in real deployments.
pub const DEFAULT_PLAYBOOK_RUNTIME_SELF_CALLBACK_BASE_URL: &str = "http://localhost:7000";

/// Default poll interval for the playbook run worker (milliseconds).
pub const DEFAULT_PLAYBOOK_RUNTIME_RUN_POLL_INTERVAL_MS: u64 = 2000;

/// Default poll interval for the playbook schedule worker (milliseconds).
pub const DEFAULT_PLAYBOOK_RUNTIME_SCHEDULE_POLL_INTERVAL_MS: u64 = 5000;

/// Default maximum queued run rows leased per worker tick.
///
/// The worker dispatch unit is a run group, so this limit must cover the whole
/// target-set ceiling. Runtime parallelism is capped separately by
/// `ansible_runner.forks`; lowering this value would split one mass launch into
/// multiple `ansible-playbook` processes.
pub const DEFAULT_PLAYBOOK_RUNTIME_RUN_LEASE_LIMIT: i64 = 50_000;

/// Default run lease TTL (seconds).
pub const DEFAULT_PLAYBOOK_RUNTIME_RUN_LEASE_TTL_SECONDS: i64 = 300;

/// Default maximum due schedules leased per schedule-worker tick.
pub const DEFAULT_PLAYBOOK_RUNTIME_SCHEDULE_LEASE_LIMIT: i64 = 25;

/// Default schedule lease TTL (seconds).
pub const DEFAULT_PLAYBOOK_RUNTIME_SCHEDULE_LEASE_TTL_SECONDS: i64 = 120;

/// Default maximum concurrently active playbook run groups.
pub const DEFAULT_PLAYBOOK_RUNTIME_MAX_CONCURRENT_RUNS: u32 = 16;

/// Default maximum concurrently active target rows per playbook run group.
///
/// This is a queue admission limit for keeping a whole run group together, not
/// an Ansible fork count.
pub const DEFAULT_PLAYBOOK_RUNTIME_MAX_CONCURRENT_RUNS_PER_GROUP: u32 = 50_000;

/// Default maximum stuck runs reaped per run-worker tick.
pub const DEFAULT_PLAYBOOK_RUNTIME_TIMEOUT_REAP_LIMIT: i64 = 50;

/// Default per-stream capture ceiling for the SSH exit-code backstop (bytes).
pub const DEFAULT_PLAYBOOK_RUNTIME_EXEC_MAX_BYTES_PER_STREAM: usize = 65_536;

// Ansible runner sidecar (job dispatch transport)
/// Default base URL of the `ansible-runner` sidecar the backend POSTs jobs to.
/// Reachable on the internal network only (compose service name).
pub const DEFAULT_ANSIBLE_RUNNER_BASE_URL: &str = "http://ansible-runner:8080";

/// Default HTTP request timeout for sidecar dispatch/cancel calls (milliseconds).
pub const DEFAULT_ANSIBLE_RUNNER_REQUEST_TIMEOUT_MS: u64 = 30_000;

/// Default Ansible forks per sidecar job.
pub const DEFAULT_ANSIBLE_RUNNER_FORKS: u32 = 128;

// License
/// Default commercial license server base URL (empty = disabled/offline-only behavior).
pub const DEFAULT_LICENSE_BASE_URL: &str = "";

/// Default timeout for license server HTTP calls in milliseconds.
pub const DEFAULT_LICENSE_REQUEST_TIMEOUT_MS: u64 = 5000;

/// Default entitlement auto-refresh interval in seconds.
pub const DEFAULT_LICENSE_REFRESH_INTERVAL_SECONDS: u64 = 21600;
/// Default grace window in seconds before license state is considered expired.
pub const DEFAULT_LICENSE_GRACE_PERIOD_SECONDS: u64 = 259200;
/// Default number of consecutive refresh failures before entering grace mode.
pub const DEFAULT_LICENSE_GRACE_FAILURE_THRESHOLD: u32 = 3;
/// Default retry delay for failed refresh attempts in milliseconds.
pub const DEFAULT_LICENSE_REFRESH_RETRY_BASE_MS: u64 = 30000;
/// Default maximum retry delay for failed refresh attempts in milliseconds.
pub const DEFAULT_LICENSE_REFRESH_RETRY_MAX_MS: u64 = 300000;
/// Default jitter budget for refresh retry in milliseconds.
pub const DEFAULT_LICENSE_REFRESH_RETRY_JITTER_MS: u64 = 5000;

// Security
/// Reject weak/default cryptographic material by default (fail-closed).
pub const DEFAULT_SECURITY_ALLOW_WEAK_DEV_SECRETS: bool = false;

/// The SSRF guard is enabled by default: outbound requests to user-supplied
/// URLs must not resolve to private/internal addresses unless an operator
/// explicitly opts out for a hermetic environment.
pub const DEFAULT_SECURITY_SSRF_ALLOW_PRIVATE: bool = false;

// NetBox
/// Default NetBox provider page size.
pub const DEFAULT_NETBOX_PAGE_SIZE: usize = 100;

// Official playbook catalog
/// Default baked official-catalog snapshot directory (relative to the working
/// directory, like `config/` and `migrations/`).
pub const DEFAULT_OFFICIAL_CATALOG_BAKED_DIR: &str = "ansible/playbooks";
/// Default writable overlay directory the git-refresh worker checks out into.
pub const DEFAULT_OFFICIAL_CATALOG_WORK_DIR: &str = "data/official-catalog";
/// Whether the periodic git refresh of the official catalog is enabled.
pub const DEFAULT_OFFICIAL_CATALOG_REFRESH_ENABLED: bool = true;
/// Default official-catalog git refresh interval in seconds.
pub const DEFAULT_OFFICIAL_CATALOG_REFRESH_INTERVAL_SECONDS: u64 = 3600;
/// Public repository the official catalog is refreshed from.
pub const DEFAULT_OFFICIAL_CATALOG_REPO_URL: &str = "https://github.com/mlimarenko/piping";
/// Default git ref the official catalog refresh tracks.
pub const DEFAULT_OFFICIAL_CATALOG_REPO_REF: &str = "main";
/// Sparse checkout path of the catalog inside the official repository.
pub const DEFAULT_OFFICIAL_CATALOG_SPARSE_PATH: &str = "ansible/playbooks";
