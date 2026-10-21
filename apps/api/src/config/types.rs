use serde::Deserialize;

// ============================================================================
// Server Configuration
// ============================================================================

/// Server configuration for HTTP server binding.
///
/// Configured via `server.bind` in config files or `PIPING_SERVER__BIND` env var.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Server bind address in format "host:port" (e.g., "0.0.0.0:8080").
    ///
    /// Use "0.0.0.0" to listen on all interfaces, or "127.0.0.1" for localhost only.
    pub bind: String,

    /// CORS allowed origins. Empty = allow all (development). Non-empty = restrict to listed origins.
    ///
    /// Example: `["https://app.example.com", "http://localhost:5173"]`
    /// Configured via `server.cors_allowed_origins` or `PIPING_SERVER__CORS_ALLOWED_ORIGINS`.
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,

    /// Maximum HTTP request body size in bytes.
    ///
    /// Default: 100 MB
    /// Configured via `server.max_body_bytes` or `PIPING_SERVER__MAX_BODY_BYTES`.
    #[serde(default = "default_server_max_body_bytes")]
    pub max_body_bytes: usize,
}

fn default_server_max_body_bytes() -> usize {
    super::constants::DEFAULT_SERVER_MAX_BODY_BYTES
}

// ============================================================================
// Database Configuration
// ============================================================================

/// PostgreSQL database connection pool configuration.
///
/// Configured via `database.*` in config files or `PIPING_DATABASE__*` env vars.
/// The connection pool manages a set of reusable database connections.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL in format:
    /// `postgresql://user:password@host:port/database?param=value`
    ///
    /// Can also use `postgres://` protocol alias.
    /// Configured via `database.url` or `PIPING_DATABASE__URL` env var.
    pub url: String,

    /// Maximum number of concurrent connections in the pool.
    ///
    /// Higher values allow more concurrent queries but consume more resources.
    /// Default: 10 connections
    #[serde(default = "default_db_max_connections")]
    pub max_connections: u32,

    /// Minimum number of connections to maintain in the pool.
    ///
    /// These connections are created at startup and kept alive.
    /// Default: 2 connections
    #[serde(default = "default_db_min_connections")]
    pub min_connections: u32,

    /// Maximum time to wait when acquiring a connection from the pool (seconds).
    ///
    /// If all connections are busy, wait up to this time before failing.
    /// Default: 30 seconds
    #[serde(default = "default_db_acquire_timeout_seconds")]
    pub acquire_timeout_seconds: u64,

    /// Time before idle connections are closed (seconds).
    ///
    /// Idle connections are closed to free resources, but new ones are created on demand.
    /// Default: 600 seconds (10 minutes)
    #[serde(default = "default_db_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,

    /// Maximum lifetime of a connection before it's recycled (seconds).
    ///
    /// Connections are recycled to prevent stale connections and ensure freshness.
    /// Default: 1800 seconds (30 minutes)
    #[serde(default = "default_db_max_lifetime_seconds")]
    pub max_lifetime_seconds: u64,
}

fn default_db_max_connections() -> u32 {
    super::constants::DEFAULT_DB_MAX_CONNECTIONS
}

fn default_db_min_connections() -> u32 {
    super::constants::DEFAULT_DB_MIN_CONNECTIONS
}

fn default_db_acquire_timeout_seconds() -> u64 {
    super::constants::DEFAULT_DB_ACQUIRE_TIMEOUT_SECONDS
}

fn default_db_idle_timeout_seconds() -> u64 {
    super::constants::DEFAULT_DB_IDLE_TIMEOUT_SECONDS
}

fn default_db_max_lifetime_seconds() -> u64 {
    super::constants::DEFAULT_DB_MAX_LIFETIME_SECONDS
}

// ============================================================================
// Redis Configuration
// ============================================================================

/// Redis connection configuration for caching and session storage.
///
/// Configured via `redis.url` in config files or `PIPING_REDIS__URL` env var.
#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    /// Redis connection URL in format:
    /// `redis://[password@]host:port[/database]` or
    /// `rediss://[password@]host:port[/database]` (TLS)
    ///
    /// Examples:
    /// - `redis://localhost:6379`
    /// - `redis://:password@redis.example.com:6379/0`
    /// - `rediss://redis.example.com:6380` (TLS)
    pub url: String,
}

// ============================================================================
// Secrets Configuration
// ============================================================================

/// Secrets configuration for encryption-at-rest behavior.
///
/// Configured via `secrets.*` in config files or `PIPING_SECRETS__*` env vars.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecretsConfig {
    /// Secrets cipher settings.
    ///
    /// This section defines where key material is sourced from and which
    /// envelope/cipher version strategy is active.
    #[serde(default)]
    pub cipher: SecretsCipherConfig,
}

/// Cipher configuration used by secrets infrastructure.
#[derive(Debug, Clone, Deserialize)]
pub struct SecretsCipherConfig {
    /// Key material reference used by secret encryption/decryption.
    ///
    /// Default points to an environment variable reference so deployments
    /// can inject key material without changing code.
    #[serde(default = "default_secrets_cipher_key_material_ref")]
    pub key_material_ref: String,

    /// Cipher envelope version strategy.
    ///
    /// Supported values are deterministic and versioned (`v1` currently).
    #[serde(default = "default_secrets_cipher_version")]
    pub version: String,
}

fn default_secrets_cipher_key_material_ref() -> String {
    super::constants::DEFAULT_SECRETS_CIPHER_KEY_MATERIAL_REF.to_string()
}

fn default_secrets_cipher_version() -> String {
    super::constants::DEFAULT_SECRETS_CIPHER_VERSION.to_string()
}

impl Default for SecretsCipherConfig {
    fn default() -> Self {
        Self {
            key_material_ref: default_secrets_cipher_key_material_ref(),
            version: default_secrets_cipher_version(),
        }
    }
}

// ============================================================================
// Observability Configuration
// ============================================================================

/// Observability configuration for logging and tracing.
///
/// Configured via `observability.*` in config files or `PIPING_OBSERVABILITY__*` env vars.
#[derive(Debug, Clone, Deserialize)]
pub struct ObservabilityConfig {
    /// Log level for application logs.
    ///
    /// Valid values: `DEBUG`, `INFO`, `WARNING`, `ERROR`, `CRITICAL`
    /// Lower levels include higher levels (DEBUG shows all, ERROR shows only errors).
    /// Default: `INFO`
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Output format for application logs.
    ///
    /// Valid values: `json`, `text`
    /// - `json`: structured logs for machine parsing
    /// - `text`: human-readable colored logs for local debugging
    ///
    /// Default: `json`
    #[serde(default = "default_log_output_format")]
    pub log_output_format: String,

    /// Window size for repetitive log event budgeting in milliseconds.
    ///
    /// Within each window, the backend can suppress or summarize repeated events
    /// according to `noise_max_events_per_window` and `noise_overflow_mode`.
    ///
    /// Default: `60000` (60 seconds)
    #[serde(default = "default_noise_window_ms")]
    pub noise_window_ms: u64,

    /// Maximum number of repeated events emitted per `noise_window_ms`.
    ///
    /// Events over this limit are handled according to `noise_overflow_mode`.
    ///
    /// Default: `100`
    #[serde(default = "default_noise_max_events_per_window")]
    pub noise_max_events_per_window: u32,

    /// Behavior when repeated events exceed `noise_max_events_per_window`.
    ///
    /// Valid values: `off`, `suppress`, `summarize`
    /// - `off`: disable repetitive-event budgeting
    /// - `suppress`: drop overflow events
    /// - `summarize`: emit periodic summary events with suppression counters
    ///
    /// Default: `summarize`
    #[serde(default = "default_noise_overflow_mode")]
    pub noise_overflow_mode: String,

    /// Whether OpenTelemetry export is enabled.
    ///
    /// Default: `true`
    #[serde(default = "default_otel_enabled")]
    pub otel_enabled: bool,

    /// OpenTelemetry OTLP exporter endpoint for distributed tracing.
    ///
    /// Format: `http://host:port` or `https://host:port`
    /// Example: `http://jaeger:4317` or `https://otel-collector:4318`
    #[serde(default = "default_otel_exporter_otlp_endpoint")]
    pub otel_exporter_otlp_endpoint: String,
}

fn default_log_level() -> String {
    super::constants::DEFAULT_OBSERVABILITY_LOG_LEVEL.to_string()
}

fn default_log_output_format() -> String {
    super::constants::DEFAULT_OBSERVABILITY_LOG_OUTPUT_FORMAT.to_string()
}

fn default_noise_window_ms() -> u64 {
    super::constants::DEFAULT_NOISE_WINDOW_MS
}

fn default_noise_max_events_per_window() -> u32 {
    super::constants::DEFAULT_NOISE_MAX_EVENTS_PER_WINDOW
}

fn default_noise_overflow_mode() -> String {
    super::constants::DEFAULT_NOISE_OVERFLOW_MODE.to_string()
}

fn default_otel_enabled() -> bool {
    super::constants::DEFAULT_OBSERVABILITY_OTEL_ENABLED
}

fn default_otel_exporter_otlp_endpoint() -> String {
    super::constants::DEFAULT_OBSERVABILITY_OTEL_EXPORTER_OTLP_ENDPOINT.to_string()
}

// ============================================================================
// Monitoring Configuration
// ============================================================================

/// Monitoring configuration for Prometheus metrics and HTTP service discovery.
///
/// Configured via `monitoring.*` in config files or `PIPING_MONITORING__*` env vars.
#[derive(Debug, Clone, Deserialize)]
pub struct MonitoringConfig {
    /// Bearer token for `/metrics`. Set it via `PIPING_MONITORING__METRICS_TOKEN`.
    ///
    /// Leaving it unset does NOT make the endpoint public — the handler fails
    /// closed and rejects every scrape with 401, including the collector's.
    /// The comment here used to claim the opposite, which is how a hardening
    /// change silently turned into "no application metrics are collected":
    /// nothing alerted, because the thing that stopped working was the
    /// monitoring. [`metrics_scrape_disabled_warning`] says so at startup.
    #[serde(default)]
    pub metrics_token: Option<String>,

    /// Optional bearer token for service discovery endpoint.
    ///
    /// If empty and `sd_public_enabled` is false, service discovery rejects requests.
    #[serde(default)]
    pub sd_token: Option<String>,

    /// Explicitly allow unauthenticated service discovery requests.
    #[serde(default)]
    pub sd_public_enabled: bool,

    /// Default target port used by service discovery when `with_port=true`.
    #[serde(default = "default_sd_default_port")]
    pub sd_default_port: u16,

    /// Default HTTP scheme for discovery targets (`__scheme__`).
    #[serde(default = "default_sd_default_scheme")]
    pub sd_default_scheme: String,

    /// Default metrics path for discovery targets (`__metrics_path__`).
    #[serde(default = "default_sd_default_metrics_path")]
    pub sd_default_metrics_path: String,

    /// Default job label for discovery targets (`job` and `piping_job`).
    #[serde(default = "default_sd_default_job")]
    pub sd_default_job: String,

    /// Maximum allowed `limit` for discovery endpoint.
    #[serde(default = "default_sd_max_limit")]
    pub sd_max_limit: u32,

    /// Optional full Prometheus-compatible `/api/v1/query` URL used by the UI.
    ///
    /// If empty, latest-metrics UI endpoints report that querying is disabled.
    #[serde(default)]
    pub query_url: Option<String>,

    /// Optional bearer token sent to `query_url`.
    #[serde(default)]
    pub query_token: Option<String>,

    /// Timeout for Prometheus-compatible instant query calls.
    #[serde(default = "default_monitoring_query_timeout_ms")]
    pub query_timeout_ms: u64,

    /// Maximum instant queries accepted per latest-metrics request.
    #[serde(default = "default_monitoring_query_max_queries")]
    pub query_max_queries: u32,

    /// Poll interval (milliseconds) for the observability list-labels poller.
    ///
    /// The background poller batch-queries fleet (non-`$host`-templated)
    /// list metrics on this cadence and caches per-host results in Redis.
    #[serde(default = "default_monitoring_list_poll_interval_ms")]
    pub list_poll_interval_ms: u64,

    /// When `true` (default), hosts with zero explicitly assigned
    /// `monitoring_metric_set` rows still surface every enabled metric query
    /// whose `default_show_in_list` flag is set. Set to `false` to make
    /// list-view metrics opt-in via explicit set assignments only.
    #[serde(default = "default_monitoring_default_set_fallback_enabled")]
    pub default_set_fallback_enabled: bool,
}

fn default_sd_default_port() -> u16 {
    super::constants::DEFAULT_SD_DEFAULT_PORT
}

fn default_sd_default_scheme() -> String {
    super::constants::DEFAULT_SD_DEFAULT_SCHEME.to_string()
}

fn default_sd_default_metrics_path() -> String {
    super::constants::DEFAULT_SD_DEFAULT_METRICS_PATH.to_string()
}

fn default_sd_default_job() -> String {
    super::constants::DEFAULT_SD_DEFAULT_JOB.to_string()
}

fn default_sd_max_limit() -> u32 {
    super::constants::DEFAULT_SD_MAX_LIMIT
}

fn default_monitoring_query_timeout_ms() -> u64 {
    super::constants::DEFAULT_MONITORING_QUERY_TIMEOUT_MS
}

fn default_monitoring_query_max_queries() -> u32 {
    super::constants::DEFAULT_MONITORING_QUERY_MAX_QUERIES
}

fn default_monitoring_list_poll_interval_ms() -> u64 {
    super::constants::DEFAULT_MONITORING_LIST_POLL_INTERVAL_MS
}

fn default_monitoring_default_set_fallback_enabled() -> bool {
    super::constants::DEFAULT_MONITORING_DEFAULT_SET_FALLBACK_ENABLED
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            metrics_token: None,
            sd_token: None,
            sd_public_enabled: false,
            sd_default_port: default_sd_default_port(),
            sd_default_scheme: default_sd_default_scheme(),
            sd_default_metrics_path: default_sd_default_metrics_path(),
            sd_default_job: default_sd_default_job(),
            sd_max_limit: default_sd_max_limit(),
            query_url: None,
            query_token: None,
            query_timeout_ms: default_monitoring_query_timeout_ms(),
            query_max_queries: default_monitoring_query_max_queries(),
            list_poll_interval_ms: default_monitoring_list_poll_interval_ms(),
            default_set_fallback_enabled: default_monitoring_default_set_fallback_enabled(),
        }
    }
}

// ============================================================================
// Authentication Configuration
// ============================================================================

/// Authentication configuration supporting both local and OIDC authentication.
///
/// Configured via `auth.*` in config files or `PIPING_AUTH__*` env vars.
/// Both authentication methods can be enabled simultaneously.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    // ------------------------------------------------------------------------
    // Local Authentication
    // ------------------------------------------------------------------------
    /// Whether local username/password authentication is enabled.
    ///
    /// When enabled, users can log in with username/email and password.
    /// Requires `password_pepper` to be configured.
    /// Default: `false`
    #[serde(default)]
    pub local_enabled: bool,

    /// Minimum password length for local authentication.
    ///
    /// Passwords shorter than this are rejected during registration/login.
    /// Internal baseline is 8 characters when local auth is enabled.
    /// Default: 8 characters
    #[serde(default = "default_auth_password_min_length")]
    pub password_min_length: usize,

    /// Password pepper (secret salt) for local authentication.
    ///
    /// This is combined with passwords before hashing with Argon2.
    /// Provides additional security even if the database is compromised.
    /// **Required** when `local_enabled` is `true`.
    /// Should be a long, random string (e.g., 32+ characters).
    #[serde(default)]
    pub password_pepper: String,

    /// Emergency local administrator password.
    ///
    /// Optional. When set, startup rotates the canonical local `admin`
    /// credential to this exact value (idempotent, this account only); when
    /// absent, the development fallback applies only while
    /// `security.allow_weak_dev_secrets` is on. Outside that explicit dev mode
    /// it must be a strong, non-default value and startup fails closed without
    /// it while local auth is enabled.
    /// Provisioned through Vault -> ESO; never commit or log the value.
    /// Env override: `PIPING_AUTH__LOCAL_ADMIN_PASSWORD`.
    #[serde(default)]
    pub local_admin_password: Option<String>,

    // ------------------------------------------------------------------------
    // Session Configuration
    // ------------------------------------------------------------------------
    /// Session time-to-live (TTL) in seconds.
    ///
    /// Sessions expire after this duration of inactivity.
    /// Default: 86400 seconds (24 hours)
    #[serde(default = "default_auth_session_ttl_seconds")]
    pub session_ttl_seconds: u64,

    /// API token policy configuration for non-interactive API access.
    #[serde(default)]
    pub api_tokens: ApiTokenConfig,

    /// Name of the session cookie sent to clients.
    ///
    /// This cookie contains the session ID (UUID).
    /// Default: `"piping_session"`
    #[serde(default = "default_auth_cookie_name")]
    pub session_cookie_name: String,

    /// Cookie domain (optional).
    ///
    /// If set, cookie is sent to this domain and subdomains.
    /// If `None`, cookie is sent only to the exact host.
    /// Example: `".example.com"` for all subdomains
    #[serde(default)]
    pub cookie_domain: Option<String>,

    /// Whether the session cookie should only be sent over HTTPS.
    ///
    /// Set to `true` in production, `false` for local development.
    /// Default: `true`
    #[serde(default = "default_auth_cookie_secure")]
    pub cookie_secure: bool,

    /// SameSite policy for the session cookie.
    ///
    /// Valid values: `"None"`, `"Lax"`, `"Strict"`
    /// - `"Strict"`: Cookie never sent in cross-site requests (most secure)
    /// - `"Lax"`: Cookie sent in top-level navigation (default, balanced)
    /// - `"None"`: Cookie sent in all requests (requires `cookie_secure: true`)
    ///   Default: `"Lax"`
    #[serde(default = "default_auth_cookie_same_site")]
    pub cookie_same_site: String,

    // ------------------------------------------------------------------------
    // OIDC Authentication
    // ------------------------------------------------------------------------
    /// Whether OpenID Connect (OIDC) authentication is enabled.
    ///
    /// When enabled, users can authenticate via an external OIDC provider
    /// (e.g., Keycloak, Okta, Google, Azure AD).
    /// Requires OIDC configuration fields below.
    /// Default: `false`
    #[serde(default)]
    pub oidc_enabled: bool,

    /// OIDC issuer URL (provider discovery endpoint).
    ///
    /// The application will fetch `.well-known/openid-configuration` from this URL.
    /// Example: `https://keycloak.example.com/realms/myrealm`
    /// **Required** when `oidc_enabled` is `true`.
    #[serde(default)]
    pub oidc_issuer_url: String,

    /// OIDC client ID (registered with the OIDC provider).
    ///
    /// This identifies the application to the OIDC provider.
    /// **Required** when `oidc_enabled` is `true`.
    #[serde(default)]
    pub oidc_client_id: String,

    /// OIDC redirect URL (callback URL after authentication).
    ///
    /// Must match the redirect URI registered with the OIDC provider.
    /// Example: `https://app.example.com/api/v1/auth/oidc/callback`
    /// **Required** when `oidc_enabled` is `true`.
    #[serde(default)]
    pub oidc_redirect_url: String,

    /// OIDC scopes to request from the provider.
    ///
    /// Common scopes: `openid`, `profile`, `email`, `groups`
    /// Default: `["openid", "profile", "email"]`
    #[serde(default = "default_oidc_scopes")]
    pub oidc_scopes: Vec<String>,

    /// OIDC claim key for extracting user groups.
    ///
    /// Groups are extracted from the userinfo endpoint's additional claims.
    /// Common values: `"groups"`, `"roles"`, `"groupMembership"`
    /// Default: `"groups"`
    #[serde(default = "default_oidc_group_claim")]
    pub oidc_group_claim: String,

    /// Clock skew tolerance for OIDC token validation (seconds).
    ///
    /// Allows for small time differences between server and OIDC provider clocks.
    /// Tokens are considered valid if they expire within this window.
    /// Default: 30 seconds
    #[serde(default = "default_oidc_clock_skew_seconds")]
    pub oidc_clock_skew_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiTokenConfig {
    /// Whether API token authentication is enabled.
    #[serde(default = "default_auth_api_tokens_enabled")]
    pub enabled: bool,
    /// Maximum number of active tokens per user.
    #[serde(default = "default_auth_api_tokens_max_active_per_user")]
    pub max_active_per_user: u32,
    /// Minimum allowed token TTL in seconds when expiration is provided.
    #[serde(default = "default_auth_api_tokens_min_ttl_seconds")]
    pub min_ttl_seconds: u64,
    /// Maximum allowed token TTL in seconds when expiration is provided.
    #[serde(default = "default_auth_api_tokens_max_ttl_seconds")]
    pub max_ttl_seconds: u64,
    /// Default token TTL in seconds. `0` means no expiration by default.
    #[serde(default = "default_auth_api_tokens_default_ttl_seconds")]
    pub default_ttl_seconds: u64,
    /// Number of random bytes used to generate token secret.
    #[serde(default = "default_auth_api_tokens_secret_bytes")]
    pub secret_bytes: u32,
    /// Per-token MCP endpoint request limit per 60-second window.
    ///
    /// `0` disables the in-process MCP limiter for deployments that enforce a
    /// stricter edge limit.
    #[serde(default = "default_auth_api_tokens_mcp_rate_limit_per_minute")]
    pub mcp_rate_limit_per_minute: u32,
}

// Default value functions for AuthConfig

/// Default session TTL: 24 hours (86400 seconds)
fn default_auth_session_ttl_seconds() -> u64 {
    super::constants::DEFAULT_AUTH_SESSION_TTL_SECONDS
}

fn default_auth_api_tokens_enabled() -> bool {
    super::constants::DEFAULT_AUTH_API_TOKENS_ENABLED
}

fn default_auth_api_tokens_max_active_per_user() -> u32 {
    super::constants::DEFAULT_AUTH_API_TOKENS_MAX_ACTIVE_PER_USER
}

fn default_auth_api_tokens_min_ttl_seconds() -> u64 {
    super::constants::DEFAULT_AUTH_API_TOKENS_MIN_TTL_SECONDS
}

fn default_auth_api_tokens_max_ttl_seconds() -> u64 {
    super::constants::DEFAULT_AUTH_API_TOKENS_MAX_TTL_SECONDS
}

fn default_auth_api_tokens_default_ttl_seconds() -> u64 {
    super::constants::DEFAULT_AUTH_API_TOKENS_DEFAULT_TTL_SECONDS
}

fn default_auth_api_tokens_secret_bytes() -> u32 {
    super::constants::DEFAULT_AUTH_API_TOKENS_SECRET_BYTES
}

fn default_auth_api_tokens_mcp_rate_limit_per_minute() -> u32 {
    super::constants::DEFAULT_AUTH_API_TOKENS_MCP_RATE_LIMIT_PER_MINUTE
}

/// Default cookie name: "piping_session"
fn default_auth_cookie_name() -> String {
    super::constants::DEFAULT_AUTH_COOKIE_NAME.to_string()
}

/// Default cookie secure flag: true (HTTPS only)
fn default_auth_cookie_secure() -> bool {
    super::constants::DEFAULT_AUTH_COOKIE_SECURE
}

/// Default SameSite policy: "Lax" (balanced security and usability)
fn default_auth_cookie_same_site() -> String {
    super::constants::DEFAULT_AUTH_COOKIE_SAME_SITE.to_string()
}

/// Default minimum password length: 8 characters
fn default_auth_password_min_length() -> usize {
    super::constants::DEFAULT_AUTH_PASSWORD_MIN_LENGTH
}

/// Default OIDC scopes: openid, profile, email
fn default_oidc_scopes() -> Vec<String> {
    super::constants::DEFAULT_AUTH_OIDC_SCOPES
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

/// Default OIDC group claim key: "groups"
fn default_oidc_group_claim() -> String {
    super::constants::DEFAULT_AUTH_OIDC_GROUP_CLAIM.to_string()
}

/// Default OIDC clock skew tolerance: 30 seconds
fn default_oidc_clock_skew_seconds() -> u64 {
    super::constants::DEFAULT_AUTH_OIDC_CLOCK_SKEW_SECONDS
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            local_enabled: false,
            session_ttl_seconds: default_auth_session_ttl_seconds(),
            api_tokens: ApiTokenConfig::default(),
            session_cookie_name: default_auth_cookie_name(),
            cookie_domain: None,
            cookie_secure: default_auth_cookie_secure(),
            cookie_same_site: default_auth_cookie_same_site(),
            password_min_length: default_auth_password_min_length(),
            password_pepper: String::new(),
            local_admin_password: None,
            oidc_enabled: false,
            oidc_issuer_url: String::new(),
            oidc_client_id: String::new(),
            oidc_redirect_url: String::new(),
            oidc_scopes: default_oidc_scopes(),
            oidc_group_claim: default_oidc_group_claim(),
            oidc_clock_skew_seconds: default_oidc_clock_skew_seconds(),
        }
    }
}

impl Default for ApiTokenConfig {
    fn default() -> Self {
        Self {
            enabled: default_auth_api_tokens_enabled(),
            max_active_per_user: default_auth_api_tokens_max_active_per_user(),
            min_ttl_seconds: default_auth_api_tokens_min_ttl_seconds(),
            max_ttl_seconds: default_auth_api_tokens_max_ttl_seconds(),
            default_ttl_seconds: default_auth_api_tokens_default_ttl_seconds(),
            secret_bytes: default_auth_api_tokens_secret_bytes(),
            mcp_rate_limit_per_minute: default_auth_api_tokens_mcp_rate_limit_per_minute(),
        }
    }
}

// ============================================================================
// Proxy Configuration
// ============================================================================

/// Proxy infrastructure configuration (token signing, TTL).
///
/// Proxy servers and their URLs are business logic: stored in DB, selected in UI
/// (primary or per-connection). Configured via `proxy.*` in config files or `PIPING_PROXY__*` env vars.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    /// TTL in seconds for proxy WebSocket tokens.
    ///
    /// Matches the proxy's token validation window.
    #[serde(default = "default_proxy_ws_token_ttl_seconds")]
    pub ws_token_ttl_seconds: u64,
    /// Grace window (seconds) for keeping a detached-but-alive SSH session
    /// re-attachable before the proxy reaper shuts it down.
    #[serde(default = "default_proxy_ssh_detach_grace_seconds")]
    pub ssh_detach_grace_seconds: u64,
    /// Timeout in milliseconds for proxy node URL validation.
    #[serde(default = "default_proxy_validation_timeout_ms")]
    pub validation_timeout_ms: u64,
    /// Maximum page size allowed for proxy node admin listing.
    #[serde(default = "default_proxy_admin_list_max_limit")]
    pub admin_list_max_limit: u32,
    /// guacd host for RDP/Guacamole bridging.
    #[serde(default = "default_proxy_guacd_host")]
    pub guacd_host: String,
    /// guacd port for RDP/Guacamole bridging.
    #[serde(default = "default_proxy_guacd_port")]
    pub guacd_port: u16,
    /// SSH file browser and transfer resource policy.
    #[serde(default)]
    pub ssh_file_transfer: ProxySshFileTransferConfig,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            ws_token_ttl_seconds: default_proxy_ws_token_ttl_seconds(),
            ssh_detach_grace_seconds: default_proxy_ssh_detach_grace_seconds(),
            validation_timeout_ms: default_proxy_validation_timeout_ms(),
            admin_list_max_limit: default_proxy_admin_list_max_limit(),
            guacd_host: default_proxy_guacd_host(),
            guacd_port: default_proxy_guacd_port(),
            ssh_file_transfer: ProxySshFileTransferConfig::default(),
        }
    }
}

/// SSH file browser and transfer resource policy.
///
/// Configured via `proxy.ssh_file_transfer.*` or
/// `PIPING_PROXY__SSH_FILE_TRANSFER__*`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxySshFileTransferConfig {
    /// Maximum accepted file upload payload size in bytes.
    #[serde(default = "default_proxy_ssh_file_transfer_max_upload_bytes")]
    pub max_upload_bytes: u64,
    /// Maximum remote file download size in bytes.
    #[serde(default = "default_proxy_ssh_file_transfer_max_download_bytes")]
    pub max_download_bytes: u64,
    /// Maximum concurrent SSH file sidecars across the proxy process.
    #[serde(default = "default_proxy_ssh_file_transfer_max_sidecars_total")]
    pub max_sidecars_total: usize,
    /// Maximum concurrent SSH file sidecars per proxy connection/session.
    #[serde(default = "default_proxy_ssh_file_transfer_max_sidecars_per_session")]
    pub max_sidecars_per_session: usize,
}

impl Default for ProxySshFileTransferConfig {
    fn default() -> Self {
        Self {
            max_upload_bytes: default_proxy_ssh_file_transfer_max_upload_bytes(),
            max_download_bytes: default_proxy_ssh_file_transfer_max_download_bytes(),
            max_sidecars_total: default_proxy_ssh_file_transfer_max_sidecars_total(),
            max_sidecars_per_session: default_proxy_ssh_file_transfer_max_sidecars_per_session(),
        }
    }
}

fn default_proxy_ws_token_ttl_seconds() -> u64 {
    super::constants::DEFAULT_PROXY_WS_TOKEN_TTL_SECONDS
}

fn default_proxy_ssh_detach_grace_seconds() -> u64 {
    super::constants::DEFAULT_PROXY_SSH_DETACH_GRACE_SECONDS
}

fn default_proxy_validation_timeout_ms() -> u64 {
    super::constants::DEFAULT_PROXY_VALIDATION_TIMEOUT_MS
}

fn default_proxy_admin_list_max_limit() -> u32 {
    super::constants::DEFAULT_PROXY_ADMIN_LIST_MAX_LIMIT
}

fn default_proxy_guacd_host() -> String {
    super::constants::DEFAULT_GUACD_HOST.to_string()
}

fn default_proxy_guacd_port() -> u16 {
    super::constants::DEFAULT_GUACD_PORT
}

fn default_proxy_ssh_file_transfer_max_upload_bytes() -> u64 {
    super::constants::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_UPLOAD_BYTES
}

fn default_proxy_ssh_file_transfer_max_download_bytes() -> u64 {
    super::constants::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_DOWNLOAD_BYTES
}

fn default_proxy_ssh_file_transfer_max_sidecars_total() -> usize {
    super::constants::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_SIDECARS_TOTAL
}

fn default_proxy_ssh_file_transfer_max_sidecars_per_session() -> usize {
    super::constants::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_SIDECARS_PER_SESSION
}

// ============================================================================
// Sync Runtime Configuration
// ============================================================================

/// Runtime configuration for source bootstrap/webhook/outbox flows.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncRuntimeConfig {
    /// Public base URL used to build callback endpoints for inbound webhooks.
    #[serde(default = "default_sync_callback_base_url")]
    pub callback_base_url: String,
    /// Allowed age for signature timestamp verification.
    #[serde(default = "default_sync_webhook_signature_window_seconds")]
    pub webhook_signature_window_seconds: u64,
    /// Deduplication window for inbound webhook events.
    #[serde(default = "default_sync_webhook_dedup_window_seconds")]
    pub webhook_dedup_window_seconds: u64,
    /// Maximum events processed in one outbox worker batch.
    #[serde(default = "default_sync_outbox_worker_batch_size")]
    pub outbox_worker_batch_size: u32,
    /// Polling interval for outbox worker.
    #[serde(default = "default_sync_outbox_worker_poll_interval_ms")]
    pub outbox_worker_poll_interval_ms: u64,
    /// Maximum retry attempts for outbound events.
    #[serde(default = "default_sync_outbox_retry_max_attempts")]
    pub outbox_retry_max_attempts: u32,
    /// Base backoff delay for outbound retry scheduling.
    #[serde(default = "default_sync_outbox_retry_backoff_ms")]
    pub outbox_retry_backoff_ms: u64,
    /// Retention window for dedup journal records.
    #[serde(default = "default_sync_dedup_retention_hours")]
    pub dedup_retention_hours: u64,
}

fn default_sync_callback_base_url() -> String {
    super::constants::DEFAULT_SYNC_CALLBACK_BASE_URL.to_string()
}

fn default_sync_webhook_signature_window_seconds() -> u64 {
    super::constants::DEFAULT_SYNC_WEBHOOK_SIGNATURE_WINDOW_SECONDS
}

fn default_sync_webhook_dedup_window_seconds() -> u64 {
    super::constants::DEFAULT_SYNC_WEBHOOK_DEDUP_WINDOW_SECONDS
}

fn default_sync_outbox_worker_batch_size() -> u32 {
    super::constants::DEFAULT_SYNC_OUTBOX_WORKER_BATCH_SIZE
}

fn default_sync_outbox_worker_poll_interval_ms() -> u64 {
    super::constants::DEFAULT_SYNC_OUTBOX_WORKER_POLL_INTERVAL_MS
}

fn default_sync_outbox_retry_max_attempts() -> u32 {
    super::constants::DEFAULT_SYNC_OUTBOX_RETRY_MAX_ATTEMPTS
}

fn default_sync_outbox_retry_backoff_ms() -> u64 {
    super::constants::DEFAULT_SYNC_OUTBOX_RETRY_BACKOFF_MS
}

fn default_sync_dedup_retention_hours() -> u64 {
    super::constants::DEFAULT_SYNC_DEDUP_RETENTION_HOURS
}

impl Default for SyncRuntimeConfig {
    fn default() -> Self {
        Self {
            callback_base_url: default_sync_callback_base_url(),
            webhook_signature_window_seconds: default_sync_webhook_signature_window_seconds(),
            webhook_dedup_window_seconds: default_sync_webhook_dedup_window_seconds(),
            outbox_worker_batch_size: default_sync_outbox_worker_batch_size(),
            outbox_worker_poll_interval_ms: default_sync_outbox_worker_poll_interval_ms(),
            outbox_retry_max_attempts: default_sync_outbox_retry_max_attempts(),
            outbox_retry_backoff_ms: default_sync_outbox_retry_backoff_ms(),
            dedup_retention_hours: default_sync_dedup_retention_hours(),
        }
    }
}

// ============================================================================
// Playbook Runtime Configuration
// ============================================================================

/// Background executor data-plane configuration: poll cadence, lease sizing, and
/// the self-callback base URL the ansible-runner sidecar posts run lifecycle
/// events back to.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaybookRuntimeConfig {
    /// Backend base URL the ansible-runner SIDECAR posts run
    /// events/artifacts/completion back to (with the run-scoped job token). Must
    /// be reachable from the sidecar — e.g. the compose service
    /// `http://backend:7000` — NOT from target hosts; the token never reaches a
    /// target. Env override: `PIPING_PLAYBOOK_RUNTIME__SELF_CALLBACK_BASE_URL`.
    #[serde(default = "default_playbook_runtime_self_callback_base_url")]
    pub self_callback_base_url: String,
    /// Run worker poll interval (milliseconds).
    #[serde(default = "default_playbook_runtime_run_poll_interval_ms")]
    pub run_poll_interval_ms: u64,
    /// Schedule worker poll interval (milliseconds).
    #[serde(default = "default_playbook_runtime_schedule_poll_interval_ms")]
    pub schedule_poll_interval_ms: u64,
    /// Maximum queued run rows leased per worker tick. Keep this at or above
    /// the target-set ceiling so one mass launch becomes one grouped
    /// Ansible runner job; `ansible_runner.forks` controls host concurrency.
    #[serde(default = "default_playbook_runtime_run_lease_limit")]
    pub run_lease_limit: i64,
    /// Run lease TTL (seconds).
    #[serde(default = "default_playbook_runtime_run_lease_ttl_seconds")]
    pub run_lease_ttl_seconds: i64,
    /// Maximum due schedules leased per schedule-worker tick.
    #[serde(default = "default_playbook_runtime_schedule_lease_limit")]
    pub schedule_lease_limit: i64,
    /// Schedule lease TTL (seconds).
    #[serde(default = "default_playbook_runtime_schedule_lease_ttl_seconds")]
    pub schedule_lease_ttl_seconds: i64,
    /// Maximum concurrently active playbook run groups.
    #[serde(default = "default_playbook_runtime_max_concurrent_runs")]
    pub max_concurrent_runs: u32,
    /// Maximum concurrently active target rows per playbook run group. This is
    /// a run-group admission limit, not an Ansible fork count.
    #[serde(default = "default_playbook_runtime_max_concurrent_runs_per_group")]
    pub max_concurrent_runs_per_group: u32,
    /// Maximum stuck runs reaped per run-worker tick.
    #[serde(default = "default_playbook_runtime_timeout_reap_limit")]
    pub timeout_reap_limit: i64,
    /// Per-stream capture ceiling for the SSH exit-code backstop (bytes).
    #[serde(default = "default_playbook_runtime_exec_max_bytes_per_stream")]
    pub exec_max_bytes_per_stream: usize,
}

fn default_playbook_runtime_self_callback_base_url() -> String {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_SELF_CALLBACK_BASE_URL.to_string()
}

fn default_playbook_runtime_run_poll_interval_ms() -> u64 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_RUN_POLL_INTERVAL_MS
}

fn default_playbook_runtime_schedule_poll_interval_ms() -> u64 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_SCHEDULE_POLL_INTERVAL_MS
}

fn default_playbook_runtime_run_lease_limit() -> i64 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_RUN_LEASE_LIMIT
}

fn default_playbook_runtime_run_lease_ttl_seconds() -> i64 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_RUN_LEASE_TTL_SECONDS
}

fn default_playbook_runtime_schedule_lease_limit() -> i64 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_SCHEDULE_LEASE_LIMIT
}

fn default_playbook_runtime_schedule_lease_ttl_seconds() -> i64 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_SCHEDULE_LEASE_TTL_SECONDS
}

fn default_playbook_runtime_max_concurrent_runs() -> u32 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_MAX_CONCURRENT_RUNS
}

fn default_playbook_runtime_max_concurrent_runs_per_group() -> u32 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_MAX_CONCURRENT_RUNS_PER_GROUP
}

fn default_playbook_runtime_timeout_reap_limit() -> i64 {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_TIMEOUT_REAP_LIMIT
}

fn default_playbook_runtime_exec_max_bytes_per_stream() -> usize {
    super::constants::DEFAULT_PLAYBOOK_RUNTIME_EXEC_MAX_BYTES_PER_STREAM
}

impl Default for PlaybookRuntimeConfig {
    fn default() -> Self {
        Self {
            self_callback_base_url: default_playbook_runtime_self_callback_base_url(),
            run_poll_interval_ms: default_playbook_runtime_run_poll_interval_ms(),
            schedule_poll_interval_ms: default_playbook_runtime_schedule_poll_interval_ms(),
            run_lease_limit: default_playbook_runtime_run_lease_limit(),
            run_lease_ttl_seconds: default_playbook_runtime_run_lease_ttl_seconds(),
            schedule_lease_limit: default_playbook_runtime_schedule_lease_limit(),
            schedule_lease_ttl_seconds: default_playbook_runtime_schedule_lease_ttl_seconds(),
            max_concurrent_runs: default_playbook_runtime_max_concurrent_runs(),
            max_concurrent_runs_per_group: default_playbook_runtime_max_concurrent_runs_per_group(),
            timeout_reap_limit: default_playbook_runtime_timeout_reap_limit(),
            exec_max_bytes_per_stream: default_playbook_runtime_exec_max_bytes_per_stream(),
        }
    }
}

// ============================================================================
// Ansible Runner Sidecar Configuration
// ============================================================================

/// Connection settings for the `ansible-runner` sidecar that executes playbook
/// jobs. The backend resolves each leased run into a fully-specified Ansible job
/// and POSTs it to `base_url`, authenticated by `shared_secret`.
#[derive(Debug, Clone, Deserialize)]
pub struct AnsibleRunnerConfig {
    /// Base URL of the sidecar (`POST {base_url}/jobs`). Env override:
    /// `PIPING_ANSIBLE_RUNNER__BASE_URL`.
    #[serde(default = "default_ansible_runner_base_url")]
    pub base_url: String,
    /// Shared secret sent on every request as a bearer token. Env override:
    /// `PIPING_ANSIBLE_RUNNER__SHARED_SECRET`. Empty in local dev; required in
    /// production deployments that enable the sidecar.
    #[serde(default)]
    pub shared_secret: String,
    /// HTTP request timeout for dispatch/cancel calls (milliseconds). Env
    /// override: `PIPING_ANSIBLE_RUNNER__REQUEST_TIMEOUT_MS`.
    #[serde(default = "default_ansible_runner_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// Default Ansible forks per sidecar job. Env override:
    /// `PIPING_ANSIBLE_RUNNER__FORKS`.
    #[serde(default = "default_ansible_runner_forks")]
    pub forks: u32,
}

fn default_ansible_runner_base_url() -> String {
    super::constants::DEFAULT_ANSIBLE_RUNNER_BASE_URL.to_string()
}

fn default_ansible_runner_request_timeout_ms() -> u64 {
    super::constants::DEFAULT_ANSIBLE_RUNNER_REQUEST_TIMEOUT_MS
}

fn default_ansible_runner_forks() -> u32 {
    super::constants::DEFAULT_ANSIBLE_RUNNER_FORKS
}

impl Default for AnsibleRunnerConfig {
    fn default() -> Self {
        Self {
            base_url: default_ansible_runner_base_url(),
            shared_secret: String::new(),
            request_timeout_ms: default_ansible_runner_request_timeout_ms(),
            forks: default_ansible_runner_forks(),
        }
    }
}

// ============================================================================
// License Configuration
// ============================================================================

/// Commercial licensing integration configuration.
///
/// The entitlement-JWT verification key, issuer, audience and clock skew are no
/// longer runtime-configurable: the trust root is the compile-time baked key in
/// [`crate::contexts::licensing::license_verifier`]. `deny_unknown_fields` makes
/// any stale `PIPING_LICENSE__JWT_*` / `__ALLOW_NON_PROD_OVERRIDE` /
/// `__PRODUCTION_ENFORCE_PIPING_SPACE` override fail boot loudly.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseConfig {
    /// Base URL of commercial license server.
    ///
    /// Empty value disables outbound activate/refresh calls and keeps local-only behavior.
    #[serde(default = "default_license_base_url")]
    pub base_url: String,

    /// Timeout for license server HTTP requests in milliseconds.
    #[serde(default = "default_license_request_timeout_ms")]
    pub request_timeout_ms: u64,

    /// Interval between automatic refresh attempts in seconds.
    #[serde(default = "default_license_refresh_interval_seconds")]
    pub refresh_interval_seconds: u64,

    /// Grace window in seconds used when control-plane refresh remains unavailable.
    #[serde(default = "default_license_grace_period_seconds")]
    pub grace_period_seconds: u64,

    /// Number of consecutive refresh failures before entering grace mode.
    #[serde(default = "default_license_grace_failure_threshold")]
    pub grace_failure_threshold: u32,

    /// Base delay for refresh retry backoff in milliseconds.
    #[serde(default = "default_license_refresh_retry_base_ms")]
    pub refresh_retry_base_ms: u64,

    /// Maximum retry delay for refresh retry backoff in milliseconds.
    #[serde(default = "default_license_refresh_retry_max_ms")]
    pub refresh_retry_max_ms: u64,

    /// Random jitter added to retry delay in milliseconds.
    #[serde(default = "default_license_refresh_retry_jitter_ms")]
    pub refresh_retry_jitter_ms: u64,
}

fn default_license_base_url() -> String {
    super::constants::DEFAULT_LICENSE_BASE_URL.to_string()
}

fn default_license_request_timeout_ms() -> u64 {
    super::constants::DEFAULT_LICENSE_REQUEST_TIMEOUT_MS
}

fn default_license_refresh_interval_seconds() -> u64 {
    super::constants::DEFAULT_LICENSE_REFRESH_INTERVAL_SECONDS
}

fn default_license_grace_period_seconds() -> u64 {
    super::constants::DEFAULT_LICENSE_GRACE_PERIOD_SECONDS
}

fn default_license_grace_failure_threshold() -> u32 {
    super::constants::DEFAULT_LICENSE_GRACE_FAILURE_THRESHOLD
}

fn default_license_refresh_retry_base_ms() -> u64 {
    super::constants::DEFAULT_LICENSE_REFRESH_RETRY_BASE_MS
}

fn default_license_refresh_retry_max_ms() -> u64 {
    super::constants::DEFAULT_LICENSE_REFRESH_RETRY_MAX_MS
}

fn default_license_refresh_retry_jitter_ms() -> u64 {
    super::constants::DEFAULT_LICENSE_REFRESH_RETRY_JITTER_MS
}

impl Default for LicenseConfig {
    fn default() -> Self {
        Self {
            base_url: default_license_base_url(),
            request_timeout_ms: default_license_request_timeout_ms(),
            refresh_interval_seconds: default_license_refresh_interval_seconds(),
            grace_period_seconds: default_license_grace_period_seconds(),
            grace_failure_threshold: default_license_grace_failure_threshold(),
            refresh_retry_base_ms: default_license_refresh_retry_base_ms(),
            refresh_retry_max_ms: default_license_refresh_retry_max_ms(),
            refresh_retry_jitter_ms: default_license_refresh_retry_jitter_ms(),
        }
    }
}

// ============================================================================
// Security Configuration
// ============================================================================

/// Security posture configuration.
///
/// Decouples dev-secret tolerance from licensing. When
/// `allow_weak_dev_secrets` is `true`, the production-secrets validation
/// short-circuits so local/dev compose can keep its weak default secrets. Env
/// override: `PIPING_SECURITY__ALLOW_WEAK_DEV_SECRETS`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    /// Permit weak/default cryptographic material (dev/local only).
    #[serde(default = "default_security_allow_weak_dev_secrets")]
    pub allow_weak_dev_secrets: bool,
    /// Permit outbound requests to resolve to private/internal/loopback
    /// addresses, disabling the SSRF guard. Escape hatch for hermetic
    /// deployments where a co-located collector lives on a private network;
    /// never enable in a user-facing deployment. Env override:
    /// `PIPING_SECURITY__SSRF_ALLOW_PRIVATE`.
    #[serde(default = "default_security_ssrf_allow_private")]
    pub ssrf_allow_private: bool,
}

fn default_security_allow_weak_dev_secrets() -> bool {
    super::constants::DEFAULT_SECURITY_ALLOW_WEAK_DEV_SECRETS
}

fn default_security_ssrf_allow_private() -> bool {
    super::constants::DEFAULT_SECURITY_SSRF_ALLOW_PRIVATE
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allow_weak_dev_secrets: default_security_allow_weak_dev_secrets(),
            ssrf_allow_private: default_security_ssrf_allow_private(),
        }
    }
}

// ============================================================================
// NetBox Provider Configuration
// ============================================================================

/// NetBox inventory provider configuration.
///
/// Built from provider.config (base_url, api_token, page_size) when resolving
/// a NetBox provider. Not loaded from global env/config — each provider has its own config.
#[derive(Debug, Clone, Deserialize)]
pub struct NetboxConfig {
    /// NetBox instance base URL.
    ///
    /// Example: `https://netbox.example.com`
    /// The API endpoint is automatically appended (`/api/`).
    #[serde(default)]
    pub base_url: String,

    /// NetBox API token for authentication.
    ///
    /// Create a token in NetBox: User -> API Tokens -> Add Token
    /// **Required** for NetBox integration.
    /// Should be kept secret (use environment variables, not config files).
    #[serde(default)]
    pub api_token: String,

    /// Page size for NetBox API pagination.
    ///
    /// Larger values reduce API calls but increase memory usage.
    /// NetBox default is typically 50, max is usually 1000.
    /// Default: 100
    #[serde(default = "default_netbox_page_size")]
    pub page_size: usize,
}

fn default_netbox_page_size() -> usize {
    super::constants::DEFAULT_NETBOX_PAGE_SIZE
}

impl Default for NetboxConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_token: String::new(),
            page_size: default_netbox_page_size(),
        }
    }
}

// ============================================================================
// Official Playbook Catalog Configuration
// ============================================================================

/// Official playbook catalog source configuration.
///
/// The catalog ships as a baked, read-only snapshot inside the backend image
/// (`baked_dir`, always available offline) and may additionally be refreshed
/// from the public product repository into a writable overlay (`work_dir`).
/// The startup loader prefers a valid overlay over the baked snapshot; the
/// refresh worker is best-effort and never required for startup, so
/// air-gapped installs run from the baked snapshot alone.
///
/// Env overrides follow the standard scheme, e.g.
/// `PIPING_OFFICIAL_CATALOG__BAKED_DIR` and
/// `PIPING_OFFICIAL_CATALOG__REFRESH_ENABLED`.
#[derive(Debug, Clone, Deserialize)]
pub struct OfficialCatalogConfig {
    /// Baked catalog snapshot directory (relative to the working directory).
    #[serde(default = "default_official_catalog_baked_dir")]
    pub baked_dir: String,
    /// Writable overlay directory for git-refresh checkouts. Empty disables
    /// the overlay entirely (baked snapshot only).
    #[serde(default = "default_official_catalog_work_dir")]
    pub work_dir: String,
    /// Whether the periodic git refresh is enabled. Disable on air-gapped
    /// installs; a failing refresh is logged and skipped either way.
    #[serde(default = "default_official_catalog_refresh_enabled")]
    pub refresh_enabled: bool,
    /// Git refresh cadence in seconds.
    #[serde(default = "default_official_catalog_refresh_interval_seconds")]
    pub refresh_interval_seconds: u64,
    /// Public repository the catalog is refreshed from.
    #[serde(default = "default_official_catalog_repo_url")]
    pub repo_url: String,
    /// Git ref (branch or tag) the refresh tracks.
    #[serde(default = "default_official_catalog_repo_ref")]
    pub repo_ref: String,
    /// Sparse checkout path of the catalog inside the repository.
    #[serde(default = "default_official_catalog_sparse_path")]
    pub sparse_path: String,
}

fn default_official_catalog_baked_dir() -> String {
    super::constants::DEFAULT_OFFICIAL_CATALOG_BAKED_DIR.to_string()
}

fn default_official_catalog_work_dir() -> String {
    super::constants::DEFAULT_OFFICIAL_CATALOG_WORK_DIR.to_string()
}

fn default_official_catalog_refresh_enabled() -> bool {
    super::constants::DEFAULT_OFFICIAL_CATALOG_REFRESH_ENABLED
}

fn default_official_catalog_refresh_interval_seconds() -> u64 {
    super::constants::DEFAULT_OFFICIAL_CATALOG_REFRESH_INTERVAL_SECONDS
}

fn default_official_catalog_repo_url() -> String {
    super::constants::DEFAULT_OFFICIAL_CATALOG_REPO_URL.to_string()
}

fn default_official_catalog_repo_ref() -> String {
    super::constants::DEFAULT_OFFICIAL_CATALOG_REPO_REF.to_string()
}

fn default_official_catalog_sparse_path() -> String {
    super::constants::DEFAULT_OFFICIAL_CATALOG_SPARSE_PATH.to_string()
}

impl Default for OfficialCatalogConfig {
    fn default() -> Self {
        Self {
            baked_dir: default_official_catalog_baked_dir(),
            work_dir: default_official_catalog_work_dir(),
            refresh_enabled: default_official_catalog_refresh_enabled(),
            refresh_interval_seconds: default_official_catalog_refresh_interval_seconds(),
            repo_url: default_official_catalog_repo_url(),
            repo_ref: default_official_catalog_repo_ref(),
            sparse_path: default_official_catalog_sparse_path(),
        }
    }
}

/// The startup warning to emit when `/metrics` will reject every scrape.
///
/// `is_monitoring_request_authorized` fails closed: with no token configured
/// the endpoint answers 401 to everyone, collector included. That is the right
/// default — the previous fail-open behaviour served metrics publicly — but it
/// is silent, and the thing it silences is the monitoring itself. On GRAD
/// production the service-discovery token was configured while this one never
/// was, so the collector has been answering `up=0` with
/// "Invalid monitoring token" and not one application metric family reached
/// storage. Nothing raised an alarm, because alarms are built on the metrics
/// that were not arriving.
///
/// Returns `None` when a token is set and the endpoint can actually serve.
pub fn metrics_scrape_disabled_warning(metrics_token: Option<&str>) -> Option<&'static str> {
    match metrics_token {
        Some(token) if !token.trim().is_empty() => None,
        _ => Some(
            "PIPING_MONITORING__METRICS_TOKEN is not set: /metrics fails closed and will \
             reject every scrape with 401, so no application metric reaches the collector",
        ),
    }
}

#[cfg(test)]
mod metrics_token_warning_tests {
    use super::metrics_scrape_disabled_warning;

    #[test]
    fn a_configured_token_scrapes_without_a_warning() {
        assert!(metrics_scrape_disabled_warning(Some("a-real-token")).is_none());
    }

    #[test]
    fn an_absent_or_blank_token_warns_that_every_scrape_will_be_rejected() {
        // Blank counts as absent: `expected_monitoring_token` treats a
        // whitespace-only value as no token, so the endpoint fails closed just
        // the same, and a deploy that sets the variable to an empty string
        // would otherwise look configured.
        for token in [None, Some(""), Some("   ")] {
            let warning = metrics_scrape_disabled_warning(token)
                .unwrap_or_else(|| panic!("{token:?} must warn"));
            assert!(warning.contains("METRICS_TOKEN"), "names the variable");
            assert!(warning.contains("401"), "says what the collector will see");
        }
    }
}
