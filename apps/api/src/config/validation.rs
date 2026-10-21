use std::env;
use std::str::FromStr;

use thiserror::Error;
use url::Url;

use sqlx::postgres::PgConnectOptions;

use crate::integrations::proxy::{PROXY_NODE_ID_ENV_KEY, PROXY_NODE_SECRET_ENV_KEY};

use super::{RunMode, Settings};

// ============================================================================
// Configuration Validation
// ============================================================================

/// Configuration validation errors.
///
/// These errors are returned during application startup if configuration
/// is invalid or missing required fields.
#[derive(Debug, Error)]
pub enum ConfigValidationError {
    #[error("Database URL is required but was empty")]
    EmptyDatabaseUrl,
    #[error("Database URL is invalid")]
    InvalidDatabaseUrl,
    #[error("Redis URL is required but was empty")]
    EmptyRedisUrl,
    #[error("Secrets cipher key material reference is required")]
    MissingSecretsCipherKeyMaterialRef,
    #[error("Secrets cipher key material value is required for env reference: {0}")]
    MissingSecretsCipherKeyMaterialValue(String),
    #[error("Secrets cipher version must be one of: v1")]
    InvalidSecretsCipherVersion,
    #[error("Server bind address is required but was empty")]
    EmptyBindAddress,
    #[error("Server max body size must be greater than zero")]
    InvalidServerMaxBodyBytes,
    #[error("Database pool min_connections cannot exceed max_connections")]
    InvalidDbPoolRange,
    #[error("Database pool timeout values must be greater than zero")]
    InvalidDbPoolTimeout,
    #[error("Observability log output format must be either 'json' or 'text'")]
    InvalidLogOutputFormat,
    #[error("Observability noise window must be greater than zero")]
    InvalidObservabilityNoiseWindow,
    #[error("Observability noise max events per window must be greater than zero")]
    InvalidObservabilityNoiseMaxEventsPerWindow,
    #[error("Observability noise overflow mode must be one of: 'off', 'suppress', 'summarize'")]
    InvalidObservabilityNoiseOverflowMode,
    #[error("Observability summarize mode requires noise max events per window to be at least 2")]
    InvalidObservabilityNoiseCombination,
    #[error("Auth session cookie name is required")]
    MissingAuthCookieName,
    #[error("Auth session TTL must be greater than zero")]
    InvalidAuthSessionTtl,
    #[error("Auth password minimum length must be at least 8 when local auth is enabled")]
    InvalidAuthPasswordMinLength,
    #[error("Auth password pepper is required when local auth is enabled")]
    MissingAuthPasswordPepper,
    #[error("Auth cookie SameSite must be one of: None, Lax, Strict")]
    InvalidAuthCookieSameSite,
    #[error("Auth API token max active per user must be greater than zero")]
    InvalidAuthApiTokenMaxActivePerUser,
    #[error("Auth API token min/max TTL bounds are invalid")]
    InvalidAuthApiTokenTtlBounds,
    #[error("Auth API token default TTL must be within min/max bounds or zero")]
    InvalidAuthApiTokenDefaultTtl,
    #[error("Auth API token secret bytes must be at least 16")]
    InvalidAuthApiTokenSecretBytes,
    #[error("OIDC issuer URL is required when OIDC is enabled")]
    MissingOidcIssuerUrl,
    #[error("OIDC client ID is required when OIDC is enabled")]
    MissingOidcClientId,
    #[error("OIDC redirect URL is required when OIDC is enabled")]
    MissingOidcRedirectUrl,
    #[error("OIDC scopes are required when OIDC is enabled")]
    MissingOidcScopes,
    #[error("OIDC group claim is required when OIDC is enabled")]
    MissingOidcGroupClaim,
    #[error("Monitoring default service discovery port must be greater than zero")]
    InvalidMonitoringDefaultPort,
    #[error("Monitoring default service discovery scheme is required")]
    MissingMonitoringDefaultScheme,
    #[error("Monitoring default service discovery metrics path is required")]
    MissingMonitoringDefaultMetricsPath,
    #[error("Monitoring service discovery max limit must be greater than zero")]
    InvalidMonitoringMaxLimit,
    #[error("Monitoring query URL must be a valid http or https URL")]
    InvalidMonitoringQueryUrl,
    #[error("Monitoring query timeout must be greater than zero")]
    InvalidMonitoringQueryTimeout,
    #[error("Monitoring query max queries must be greater than zero")]
    InvalidMonitoringQueryMaxQueries,
    #[error("Proxy node id is required in proxy mode (env: PIPING_PROXY__NODE_ID)")]
    MissingProxyNodeIdEnv,
    #[error("Proxy node secret is required in proxy mode (env: PIPING_PROXY__NODE_SECRET)")]
    MissingProxyNodeSecretEnv,
    #[error("Proxy WebSocket token TTL must be greater than zero")]
    InvalidProxyWsTokenTtl,
    #[error("Proxy SSH detach grace seconds must be greater than zero and at most 3600")]
    InvalidProxySshDetachGrace,
    #[error("Proxy validation timeout must be greater than zero")]
    InvalidProxyValidationTimeout,
    #[error("Proxy admin list max limit must be greater than zero")]
    InvalidProxyAdminListMaxLimit,
    #[error("Proxy SSH file transfer max upload bytes must be greater than zero")]
    InvalidProxySshFileTransferMaxUploadBytes,
    #[error("Proxy SSH file transfer max download bytes must be greater than zero")]
    InvalidProxySshFileTransferMaxDownloadBytes,
    #[error("Proxy SSH file transfer total sidecar limit must be greater than zero")]
    InvalidProxySshFileTransferMaxSidecarsTotal,
    #[error(
        "Proxy SSH file transfer per-session sidecar limit must be greater than zero and at most the total sidecar limit"
    )]
    InvalidProxySshFileTransferMaxSidecarsPerSession,
    #[error("Sync runtime callback base URL is required")]
    MissingSyncCallbackBaseUrl,
    #[error("Sync runtime signature window must be greater than zero")]
    InvalidSyncSignatureWindow,
    #[error("Sync runtime dedup window must be greater than zero")]
    InvalidSyncDedupWindow,
    #[error("Sync runtime outbox worker batch size must be greater than zero")]
    InvalidSyncOutboxBatchSize,
    #[error("Sync runtime outbox worker poll interval must be greater than zero")]
    InvalidSyncOutboxPollInterval,
    #[error("Sync runtime outbox retry max attempts must be greater than zero")]
    InvalidSyncOutboxRetryAttempts,
    #[error("Sync runtime outbox retry backoff must be greater than zero")]
    InvalidSyncOutboxRetryBackoff,
    #[error("Sync runtime dedup retention must be greater than zero")]
    InvalidSyncDedupRetention,
    #[error("Playbook runtime self-callback base URL is required")]
    MissingPlaybookSelfCallbackBaseUrl,
    #[error("Playbook runtime self-callback base URL must be a valid http(s) URL")]
    InvalidPlaybookSelfCallbackBaseUrl,
    #[error("Playbook runtime poll intervals must be greater than zero")]
    InvalidPlaybookPollInterval,
    #[error("Playbook runtime lease limits must be greater than zero")]
    InvalidPlaybookLeaseLimit,
    #[error("Playbook runtime lease TTLs must be greater than zero")]
    InvalidPlaybookLeaseTtl,
    #[error("Playbook runtime max concurrent runs must be greater than zero")]
    InvalidPlaybookMaxConcurrentRuns,
    #[error("Playbook runtime timeout reap limit must be greater than zero")]
    InvalidPlaybookTimeoutReapLimit,
    #[error("Playbook runtime exec max bytes per stream must be greater than zero")]
    InvalidPlaybookExecMaxBytesPerStream,
    #[error("Ansible runner base URL is required")]
    MissingAnsibleRunnerBaseUrl,
    #[error("Ansible runner base URL must be a valid http(s) URL")]
    InvalidAnsibleRunnerBaseUrl,
    #[error("Ansible runner request timeout must be greater than zero")]
    InvalidAnsibleRunnerRequestTimeout,
    #[error("Ansible runner forks must be greater than zero")]
    InvalidAnsibleRunnerForks,
    #[error("License request timeout must be greater than zero")]
    InvalidLicenseRequestTimeout,
    #[error("License refresh interval must be greater than zero")]
    InvalidLicenseRefreshInterval,
    #[error("License grace period must be greater than zero")]
    InvalidLicenseGracePeriod,
    #[error("License grace failure threshold must be greater than zero")]
    InvalidLicenseGraceFailureThreshold,
    #[error("License refresh retry base/max/jitter must be greater than zero")]
    InvalidLicenseRetrySettings,
    #[error("License refresh retry max must be >= retry base")]
    InvalidLicenseRetryRange,
    #[error("License base_url must be a valid absolute URL")]
    InvalidLicenseBaseUrl,
    #[error(
        "Production security violation: secrets cipher key must be provisioned with a strong, non-default value (env: PIPING_SECRETS__CIPHER_KEY_B64); set security.allow_weak_dev_secrets=true only for non-prod"
    )]
    InsecureSecretsCipherKey,
    #[error(
        "Production security violation: proxy node secret must be provisioned with a strong, non-default value of at least {min} characters (env: PIPING_PROXY__NODE_SECRET); set security.allow_weak_dev_secrets=true only for non-prod"
    )]
    InsecureProxySharedSecret { min: usize },
    #[error(
        "Production security violation: auth password pepper must be provisioned with a strong, non-default value (env: PIPING_AUTH__PASSWORD_PEPPER); set security.allow_weak_dev_secrets=true only for non-prod"
    )]
    InsecureAuthPasswordPepper,
    #[error(
        "Production security violation: local admin password must be provisioned with a strong, non-default value of at least {min} characters (env: PIPING_AUTH__LOCAL_ADMIN_PASSWORD); set security.allow_weak_dev_secrets=true only for non-prod"
    )]
    InsecureAuthLocalAdminPassword { min: usize },
    #[error(
        "Production security violation: database password must not be a known default (env: PIPING_DATABASE__URL); set security.allow_weak_dev_secrets=true only for non-prod"
    )]
    InsecureDatabasePassword,
    #[error(
        "Production security violation: Redis password must not be a known default (env: PIPING_REDIS__URL); set security.allow_weak_dev_secrets=true only for non-prod"
    )]
    InsecureRedisPassword,
}

const LOG_OUTPUT_JSON: &str = "json";
const LOG_OUTPUT_TEXT: &str = "text";
const NOISE_OVERFLOW_OFF: &str = "off";
const NOISE_OVERFLOW_SUPPRESS: &str = "suppress";
const NOISE_OVERFLOW_SUMMARIZE: &str = "summarize";
const COOKIE_SAME_SITE_NONE: &str = "none";
const COOKIE_SAME_SITE_LAX: &str = "lax";
const COOKIE_SAME_SITE_STRICT: &str = "strict";
const SECRETS_CIPHER_KEY_ENV_KEY: &str = "PIPING_SECRETS__CIPHER_KEY_B64";

/// Minimum acceptable length for a proxy node secret in production.
const MIN_PROXY_NODE_SECRET_LEN: usize = 16;

/// Minimum acceptable length for the production local-admin emergency password.
/// Deliberately above the interactive-user baseline: this is a break-glass
/// credential and must be a generated secret, not a memorable human password.
const MIN_LOCAL_ADMIN_PASSWORD_LEN: usize = 16;

/// Known-default cipher key material that must never be used in production.
/// Includes the base64 form shipped in compose files and its decoded ASCII value.
const DEFAULT_CIPHER_KEY_VALUES: &[&str] = &[
    "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
    "0123456789abcdef0123456789abcdef",
];

/// Known-default proxy node secret values rejected in production.
const DEFAULT_PROXY_NODE_SECRET_VALUES: &[&str] = &["change-this-proxy-secret"];

/// Known-default auth password pepper values rejected in production.
const DEFAULT_AUTH_PASSWORD_PEPPER_VALUES: &[&str] = &["dev-pepper"];

/// Known-default local-admin password values rejected in production.
const DEFAULT_LOCAL_ADMIN_PASSWORD_VALUES: &[&str] = &["piping123"];

/// Known-default database password values rejected in production.
const DEFAULT_DATABASE_PASSWORD_VALUES: &[&str] = &["piping-change-me"];

/// Known-default Redis password values rejected in production.
const DEFAULT_REDIS_PASSWORD_VALUES: &[&str] = &["redis-change-me"];

/// Validates application configuration and returns errors for invalid settings.
///
/// Performs the following validations:
/// 1. Database URL is present and valid PostgreSQL connection string
/// 2. Redis URL is present
/// 3. Server bind address is present
/// 4. Database pool settings are valid (min <= max, timeouts > 0)
/// 5. Auth session cookie name is present
/// 6. Auth session TTL > 0
/// 7. If local auth enabled: password pepper is present
/// 8. If OIDC enabled: all required OIDC fields are present
/// 9. Proxy runtime settings are structurally valid (proxy URLs are business logic, from DB/UI)
///
/// This is called during application startup to fail-fast on configuration errors.
pub(super) fn validate_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    validate_database_settings(settings)?;
    validate_redis_settings(settings)?;
    validate_secrets_settings(settings)?;
    validate_server_settings(settings)?;
    validate_observability_settings(settings)?;
    validate_auth_settings(settings)?;
    validate_monitoring_settings(settings)?;
    validate_proxy_settings(settings)?;
    validate_sync_runtime_settings(settings)?;
    validate_playbook_runtime_settings(settings)?;
    validate_ansible_runner_settings(settings)?;
    validate_license_settings(settings)?;
    validate_production_secrets(settings)?;
    Ok(())
}

/// Returns `true` when strong cryptographic material must be enforced.
///
/// Fail-closed by default: weak/default secrets are rejected unless an operator
/// explicitly opts into dev tolerance via `PIPING_SECURITY__ALLOW_WEAK_DEV_SECRETS=true`
/// (local/dev compose sets this), which keeps the weak compose defaults permissive.
fn requires_strong_secrets(settings: &Settings) -> bool {
    !settings.security.allow_weak_dev_secrets
}

/// Fail-closed validation of cryptographic material in production.
///
/// Rejects empty, known-default, or too-short values for the secrets cipher key,
/// auth password pepper, and the database/Redis passwords.
/// Skipped entirely outside production so local/dev keeps working with the weak
/// defaults shipped in `docker-compose.yml`.
fn validate_production_secrets(settings: &Settings) -> Result<(), ConfigValidationError> {
    if !requires_strong_secrets(settings) {
        return Ok(());
    }

    validate_production_cipher_key(settings)?;
    validate_production_password_pepper(settings)?;
    validate_production_local_admin_password(settings)?;
    validate_production_database_password(settings)?;
    validate_production_redis_password(settings)?;
    Ok(())
}

fn validate_production_cipher_key(settings: &Settings) -> Result<(), ConfigValidationError> {
    let reference = settings.secrets.cipher.key_material_ref.trim();
    // Resolve the literal key material whether sourced from env or inline.
    let key_material = if let Some(env_key) = reference.strip_prefix("env:") {
        env::var(env_key.trim()).unwrap_or_default()
    } else if let Some(inline) = reference.strip_prefix("inline:") {
        inline.to_string()
    } else {
        // Fall back to the canonical env var when the ref shape is unexpected.
        env::var(SECRETS_CIPHER_KEY_ENV_KEY).unwrap_or_default()
    };

    check_cipher_key_value(&key_material)
}

/// Pure cipher-key check: rejects empty or known-default key material.
fn check_cipher_key_value(key_material: &str) -> Result<(), ConfigValidationError> {
    let key_material = key_material.trim();
    if key_material.is_empty() || is_known_default(key_material, DEFAULT_CIPHER_KEY_VALUES) {
        return Err(ConfigValidationError::InsecureSecretsCipherKey);
    }
    Ok(())
}

/// Pure proxy-secret check: rejects empty, too-short, or known-default values.
fn check_proxy_secret_value(secret: &str) -> Result<(), ConfigValidationError> {
    let secret = secret.trim();
    if secret.is_empty()
        || secret.len() < MIN_PROXY_NODE_SECRET_LEN
        || is_known_default(secret, DEFAULT_PROXY_NODE_SECRET_VALUES)
    {
        return Err(ConfigValidationError::InsecureProxySharedSecret {
            min: MIN_PROXY_NODE_SECRET_LEN,
        });
    }
    Ok(())
}

fn validate_production_password_pepper(settings: &Settings) -> Result<(), ConfigValidationError> {
    let pepper = settings.auth.password_pepper.trim();
    if pepper.is_empty() || is_known_default(pepper, DEFAULT_AUTH_PASSWORD_PEPPER_VALUES) {
        return Err(ConfigValidationError::InsecureAuthPasswordPepper);
    }
    Ok(())
}

/// Fail-closed check of the local emergency administrator password.
///
/// `auth.local_admin_password` is optional only in explicitly allowed dev mode.
/// Whenever strong secrets are enforced and local auth is enabled, a missing,
/// too-short or known-default value aborts startup: a production instance must
/// never come up with a predictable break-glass password. When local auth is
/// disabled no local administrator is created, so no password is required.
fn validate_production_local_admin_password(
    settings: &Settings,
) -> Result<(), ConfigValidationError> {
    let auth = &settings.auth;
    if !auth.local_enabled {
        return Ok(());
    }
    let password = auth
        .local_admin_password
        .as_deref()
        .unwrap_or_default()
        .trim();
    if password.is_empty()
        || password.len() < MIN_LOCAL_ADMIN_PASSWORD_LEN
        || is_known_default(password, DEFAULT_LOCAL_ADMIN_PASSWORD_VALUES)
    {
        return Err(ConfigValidationError::InsecureAuthLocalAdminPassword {
            min: MIN_LOCAL_ADMIN_PASSWORD_LEN,
        });
    }
    Ok(())
}

fn validate_production_database_password(settings: &Settings) -> Result<(), ConfigValidationError> {
    if let Some(password) = url_password(&settings.database.url)
        && is_known_default(&password, DEFAULT_DATABASE_PASSWORD_VALUES)
    {
        return Err(ConfigValidationError::InsecureDatabasePassword);
    }
    Ok(())
}

fn validate_production_redis_password(settings: &Settings) -> Result<(), ConfigValidationError> {
    if let Some(password) = url_password(&settings.redis.url)
        && is_known_default(&password, DEFAULT_REDIS_PASSWORD_VALUES)
    {
        return Err(ConfigValidationError::InsecureRedisPassword);
    }
    Ok(())
}

/// Extracts the password component from a connection URL, if present.
///
/// The `url` crate returns the password in its percent-encoded form; the known
/// default passwords contain no reserved characters, so a direct comparison is
/// sufficient to catch them.
fn url_password(raw_url: &str) -> Option<String> {
    let parsed = Url::parse(raw_url.trim()).ok()?;
    parsed.password().map(ToString::to_string)
}

/// Case-sensitive comparison of a trimmed value against a set of known defaults.
fn is_known_default(value: &str, defaults: &[&str]) -> bool {
    defaults.contains(&value)
}

fn validate_database_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let database = &settings.database;
    if database.url.is_empty() {
        return Err(ConfigValidationError::EmptyDatabaseUrl);
    }
    if PgConnectOptions::from_str(&database.url).is_err() {
        return Err(ConfigValidationError::InvalidDatabaseUrl);
    }
    if database.min_connections > database.max_connections {
        return Err(ConfigValidationError::InvalidDbPoolRange);
    }
    if database.acquire_timeout_seconds == 0
        || database.idle_timeout_seconds == 0
        || database.max_lifetime_seconds == 0
    {
        return Err(ConfigValidationError::InvalidDbPoolTimeout);
    }
    Ok(())
}

fn validate_redis_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    if settings.redis.url.is_empty() {
        return Err(ConfigValidationError::EmptyRedisUrl);
    }
    Ok(())
}

fn validate_secrets_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let cipher = &settings.secrets.cipher;
    require_non_blank(
        &cipher.key_material_ref,
        ConfigValidationError::MissingSecretsCipherKeyMaterialRef,
    )?;
    if cipher.version.trim() != super::constants::DEFAULT_SECRETS_CIPHER_VERSION {
        return Err(ConfigValidationError::InvalidSecretsCipherVersion);
    }
    if let Some(env_key) = cipher.key_material_ref.trim().strip_prefix("env:") {
        let env_key = env_key.trim();
        require_non_blank(
            env_key,
            ConfigValidationError::MissingSecretsCipherKeyMaterialRef,
        )?;
        if env::var(env_key).is_err() {
            return Err(ConfigValidationError::MissingSecretsCipherKeyMaterialValue(
                env_key.to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_server_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let server = &settings.server;
    if server.bind.is_empty() {
        return Err(ConfigValidationError::EmptyBindAddress);
    }
    if server.max_body_bytes == 0 {
        return Err(ConfigValidationError::InvalidServerMaxBodyBytes);
    }
    Ok(())
}

fn validate_observability_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let observability = &settings.observability;
    let log_output_format = observability.log_output_format.to_ascii_lowercase();
    if !is_one_of(&log_output_format, &[LOG_OUTPUT_JSON, LOG_OUTPUT_TEXT]) {
        return Err(ConfigValidationError::InvalidLogOutputFormat);
    }

    if observability.noise_window_ms == 0 {
        return Err(ConfigValidationError::InvalidObservabilityNoiseWindow);
    }
    if observability.noise_max_events_per_window == 0 {
        return Err(ConfigValidationError::InvalidObservabilityNoiseMaxEventsPerWindow);
    }

    let noise_overflow_mode = observability.noise_overflow_mode.to_ascii_lowercase();
    if !is_one_of(
        &noise_overflow_mode,
        &[
            NOISE_OVERFLOW_OFF,
            NOISE_OVERFLOW_SUPPRESS,
            NOISE_OVERFLOW_SUMMARIZE,
        ],
    ) {
        return Err(ConfigValidationError::InvalidObservabilityNoiseOverflowMode);
    }
    if noise_overflow_mode == NOISE_OVERFLOW_SUMMARIZE
        && observability.noise_max_events_per_window
            < super::constants::MIN_OBSERVABILITY_NOISE_MAX_EVENTS_PER_WINDOW_FOR_SUMMARIZE
    {
        return Err(ConfigValidationError::InvalidObservabilityNoiseCombination);
    }
    Ok(())
}

fn validate_auth_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let auth = &settings.auth;
    require_non_blank(
        &auth.session_cookie_name,
        ConfigValidationError::MissingAuthCookieName,
    )?;
    if auth.session_ttl_seconds == 0 {
        return Err(ConfigValidationError::InvalidAuthSessionTtl);
    }

    let cookie_same_site = auth.cookie_same_site.to_ascii_lowercase();
    if !is_one_of(
        &cookie_same_site,
        &[
            COOKIE_SAME_SITE_NONE,
            COOKIE_SAME_SITE_LAX,
            COOKIE_SAME_SITE_STRICT,
        ],
    ) {
        return Err(ConfigValidationError::InvalidAuthCookieSameSite);
    }

    if auth.local_enabled
        && auth.password_min_length < super::constants::DEFAULT_AUTH_PASSWORD_MIN_LENGTH
    {
        return Err(ConfigValidationError::InvalidAuthPasswordMinLength);
    }
    if auth.local_enabled && auth.password_pepper.trim().is_empty() {
        return Err(ConfigValidationError::MissingAuthPasswordPepper);
    }

    validate_auth_api_token_settings(settings)?;
    if auth.oidc_enabled {
        validate_auth_oidc_settings(settings)?;
    }
    Ok(())
}

fn validate_auth_api_token_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let api_tokens = &settings.auth.api_tokens;
    if api_tokens.max_active_per_user == 0 {
        return Err(ConfigValidationError::InvalidAuthApiTokenMaxActivePerUser);
    }
    if api_tokens.min_ttl_seconds == 0
        || api_tokens.max_ttl_seconds == 0
        || api_tokens.min_ttl_seconds > api_tokens.max_ttl_seconds
    {
        return Err(ConfigValidationError::InvalidAuthApiTokenTtlBounds);
    }
    if api_tokens.default_ttl_seconds != 0
        && (api_tokens.default_ttl_seconds < api_tokens.min_ttl_seconds
            || api_tokens.default_ttl_seconds > api_tokens.max_ttl_seconds)
    {
        return Err(ConfigValidationError::InvalidAuthApiTokenDefaultTtl);
    }
    if api_tokens.secret_bytes < 16 {
        return Err(ConfigValidationError::InvalidAuthApiTokenSecretBytes);
    }
    Ok(())
}

fn validate_auth_oidc_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let auth = &settings.auth;
    require_non_blank(
        &auth.oidc_issuer_url,
        ConfigValidationError::MissingOidcIssuerUrl,
    )?;
    require_non_blank(
        &auth.oidc_client_id,
        ConfigValidationError::MissingOidcClientId,
    )?;
    require_non_blank(
        &auth.oidc_redirect_url,
        ConfigValidationError::MissingOidcRedirectUrl,
    )?;
    if auth.oidc_scopes.iter().all(|scope| scope.trim().is_empty()) {
        return Err(ConfigValidationError::MissingOidcScopes);
    }
    require_non_blank(
        &auth.oidc_group_claim,
        ConfigValidationError::MissingOidcGroupClaim,
    )?;
    Ok(())
}

fn validate_monitoring_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let monitoring = &settings.monitoring;
    if monitoring.sd_default_port == 0 {
        return Err(ConfigValidationError::InvalidMonitoringDefaultPort);
    }
    require_non_blank(
        &monitoring.sd_default_scheme,
        ConfigValidationError::MissingMonitoringDefaultScheme,
    )?;
    require_non_blank(
        &monitoring.sd_default_metrics_path,
        ConfigValidationError::MissingMonitoringDefaultMetricsPath,
    )?;
    if monitoring.sd_max_limit == 0 {
        return Err(ConfigValidationError::InvalidMonitoringMaxLimit);
    }
    if let Some(query_url) = monitoring
        .query_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let parsed =
            Url::parse(query_url).map_err(|_| ConfigValidationError::InvalidMonitoringQueryUrl)?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ConfigValidationError::InvalidMonitoringQueryUrl);
        }
    }
    if monitoring.query_timeout_ms == 0 {
        return Err(ConfigValidationError::InvalidMonitoringQueryTimeout);
    }
    if monitoring.query_max_queries == 0 {
        return Err(ConfigValidationError::InvalidMonitoringQueryMaxQueries);
    }
    Ok(())
}

fn validate_proxy_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let proxy = &settings.proxy;
    if proxy.ws_token_ttl_seconds == 0 {
        return Err(ConfigValidationError::InvalidProxyWsTokenTtl);
    }
    if proxy.ssh_detach_grace_seconds == 0 || proxy.ssh_detach_grace_seconds > 3600 {
        return Err(ConfigValidationError::InvalidProxySshDetachGrace);
    }
    if proxy.validation_timeout_ms == 0 {
        return Err(ConfigValidationError::InvalidProxyValidationTimeout);
    }
    if proxy.admin_list_max_limit == 0 {
        return Err(ConfigValidationError::InvalidProxyAdminListMaxLimit);
    }
    validate_proxy_ssh_file_transfer_policy(settings)?;
    Ok(())
}

fn validate_proxy_ssh_file_transfer_policy(
    settings: &Settings,
) -> Result<(), ConfigValidationError> {
    let transfer = &settings.proxy.ssh_file_transfer;
    if transfer.max_upload_bytes == 0 {
        return Err(ConfigValidationError::InvalidProxySshFileTransferMaxUploadBytes);
    }
    if transfer.max_download_bytes == 0 {
        return Err(ConfigValidationError::InvalidProxySshFileTransferMaxDownloadBytes);
    }
    if transfer.max_sidecars_total == 0 {
        return Err(ConfigValidationError::InvalidProxySshFileTransferMaxSidecarsTotal);
    }
    if transfer.max_sidecars_per_session == 0
        || transfer.max_sidecars_per_session > transfer.max_sidecars_total
    {
        return Err(ConfigValidationError::InvalidProxySshFileTransferMaxSidecarsPerSession);
    }
    Ok(())
}

fn validate_sync_runtime_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let sync_runtime = &settings.sync_runtime;
    require_non_blank(
        &sync_runtime.callback_base_url,
        ConfigValidationError::MissingSyncCallbackBaseUrl,
    )?;
    if sync_runtime.webhook_signature_window_seconds == 0 {
        return Err(ConfigValidationError::InvalidSyncSignatureWindow);
    }
    if sync_runtime.webhook_dedup_window_seconds == 0 {
        return Err(ConfigValidationError::InvalidSyncDedupWindow);
    }
    if sync_runtime.outbox_worker_batch_size == 0 {
        return Err(ConfigValidationError::InvalidSyncOutboxBatchSize);
    }
    if sync_runtime.outbox_worker_poll_interval_ms == 0 {
        return Err(ConfigValidationError::InvalidSyncOutboxPollInterval);
    }
    if sync_runtime.outbox_retry_max_attempts == 0 {
        return Err(ConfigValidationError::InvalidSyncOutboxRetryAttempts);
    }
    if sync_runtime.outbox_retry_backoff_ms == 0 {
        return Err(ConfigValidationError::InvalidSyncOutboxRetryBackoff);
    }
    if sync_runtime.dedup_retention_hours == 0 {
        return Err(ConfigValidationError::InvalidSyncDedupRetention);
    }
    Ok(())
}

fn validate_playbook_runtime_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let runtime = &settings.playbook_runtime;
    require_non_blank(
        &runtime.self_callback_base_url,
        ConfigValidationError::MissingPlaybookSelfCallbackBaseUrl,
    )?;
    // The ansible-runner sidecar POSTs run events/completion back to this URL
    // with the run-scoped Bearer job token. That token only ever transits the
    // internal backend<->sidecar hop: it is never written to the playbook
    // private_data_dir nor passed to ansible, so it never reaches a target host.
    // This is the SAME internal hop as the dispatch (ansible_runner.base_url),
    // reversed — so it follows the SAME rule as validate_ansible_runner_settings:
    // require a structurally valid http(s) URL; plaintext http to an internal
    // service host is acceptable. (If remote sidecars are ever supported, BOTH
    // hops must move to https/mTLS together — never make the callback stricter
    // than the dispatch.)
    let callback_url = Url::parse(runtime.self_callback_base_url.trim())
        .map_err(|_| ConfigValidationError::InvalidPlaybookSelfCallbackBaseUrl)?;
    if !matches!(callback_url.scheme(), "http" | "https") {
        return Err(ConfigValidationError::InvalidPlaybookSelfCallbackBaseUrl);
    }
    if runtime.run_poll_interval_ms == 0 || runtime.schedule_poll_interval_ms == 0 {
        return Err(ConfigValidationError::InvalidPlaybookPollInterval);
    }
    if runtime.run_lease_limit <= 0 || runtime.schedule_lease_limit <= 0 {
        return Err(ConfigValidationError::InvalidPlaybookLeaseLimit);
    }
    if runtime.run_lease_ttl_seconds <= 0 || runtime.schedule_lease_ttl_seconds <= 0 {
        return Err(ConfigValidationError::InvalidPlaybookLeaseTtl);
    }
    if runtime.max_concurrent_runs == 0 || runtime.max_concurrent_runs_per_group == 0 {
        return Err(ConfigValidationError::InvalidPlaybookMaxConcurrentRuns);
    }
    if runtime.timeout_reap_limit <= 0 {
        return Err(ConfigValidationError::InvalidPlaybookTimeoutReapLimit);
    }
    if runtime.exec_max_bytes_per_stream == 0 {
        return Err(ConfigValidationError::InvalidPlaybookExecMaxBytesPerStream);
    }
    Ok(())
}

fn validate_ansible_runner_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let runner = &settings.ansible_runner;
    require_non_blank(
        &runner.base_url,
        ConfigValidationError::MissingAnsibleRunnerBaseUrl,
    )?;
    // The sidecar is an internal-network service, so plaintext http to any host
    // is acceptable; only require a structurally valid http(s) URL.
    let base_url = Url::parse(runner.base_url.trim())
        .map_err(|_| ConfigValidationError::InvalidAnsibleRunnerBaseUrl)?;
    if !matches!(base_url.scheme(), "http" | "https") {
        return Err(ConfigValidationError::InvalidAnsibleRunnerBaseUrl);
    }
    if runner.request_timeout_ms == 0 {
        return Err(ConfigValidationError::InvalidAnsibleRunnerRequestTimeout);
    }
    if runner.forks == 0 {
        return Err(ConfigValidationError::InvalidAnsibleRunnerForks);
    }
    Ok(())
}

fn validate_license_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    let license = &settings.license;
    if license.request_timeout_ms == 0 {
        return Err(ConfigValidationError::InvalidLicenseRequestTimeout);
    }
    if license.refresh_interval_seconds == 0 {
        return Err(ConfigValidationError::InvalidLicenseRefreshInterval);
    }
    if license.grace_period_seconds == 0 {
        return Err(ConfigValidationError::InvalidLicenseGracePeriod);
    }
    if license.grace_failure_threshold == 0 {
        return Err(ConfigValidationError::InvalidLicenseGraceFailureThreshold);
    }
    if license.refresh_retry_base_ms == 0
        || license.refresh_retry_max_ms == 0
        || license.refresh_retry_jitter_ms == 0
    {
        return Err(ConfigValidationError::InvalidLicenseRetrySettings);
    }
    if license.refresh_retry_max_ms < license.refresh_retry_base_ms {
        return Err(ConfigValidationError::InvalidLicenseRetryRange);
    }

    validate_license_base_url_policy(settings)?;
    Ok(())
}

/// Validates that a configured license control-plane `base_url` is a parseable URL.
///
/// The verification key is baked at compile time and is the sole trust root, so a
/// repointed control-plane host is not a bypass; only basic URL well-formedness is
/// enforced here. An empty `base_url` disables outbound calls and is accepted.
fn validate_license_base_url_policy(settings: &Settings) -> Result<(), ConfigValidationError> {
    let base_url = settings.license.base_url.trim();
    if base_url.is_empty() {
        return Ok(());
    }

    Url::parse(base_url).map_err(|_| ConfigValidationError::InvalidLicenseBaseUrl)?;
    Ok(())
}

fn require_non_blank(
    value: &str,
    error: ConfigValidationError,
) -> Result<(), ConfigValidationError> {
    if value.trim().is_empty() {
        return Err(error);
    }
    Ok(())
}

fn is_one_of(value: &str, options: &[&str]) -> bool {
    options.contains(&value)
}

fn env_var_missing_or_blank(key: &str) -> bool {
    match env::var(key) {
        Ok(value) => value.trim().is_empty(),
        Err(_) => true,
    }
}

pub(super) fn validate_settings_for_mode(
    settings: &Settings,
    run_mode: RunMode,
) -> Result<(), ConfigValidationError> {
    match run_mode {
        RunMode::Backend => validate_settings(settings),
        RunMode::Proxy => validate_proxy_mode_settings(settings),
    }
}

fn validate_proxy_mode_settings(settings: &Settings) -> Result<(), ConfigValidationError> {
    if settings.server.bind.is_empty() {
        return Err(ConfigValidationError::EmptyBindAddress);
    }
    if env_var_missing_or_blank(PROXY_NODE_ID_ENV_KEY) {
        return Err(ConfigValidationError::MissingProxyNodeIdEnv);
    }
    if env_var_missing_or_blank(PROXY_NODE_SECRET_ENV_KEY) {
        return Err(ConfigValidationError::MissingProxyNodeSecretEnv);
    }
    if requires_strong_secrets(settings) {
        let secret = env::var(PROXY_NODE_SECRET_ENV_KEY).unwrap_or_default();
        check_proxy_secret_value(&secret)?;
    }
    if settings.proxy.ws_token_ttl_seconds == 0 {
        return Err(ConfigValidationError::InvalidProxyWsTokenTtl);
    }
    if settings.proxy.ssh_detach_grace_seconds == 0
        || settings.proxy.ssh_detach_grace_seconds > 3600
    {
        return Err(ConfigValidationError::InvalidProxySshDetachGrace);
    }
    if settings.proxy.validation_timeout_ms == 0 {
        return Err(ConfigValidationError::InvalidProxyValidationTimeout);
    }
    if settings.proxy.admin_list_max_limit == 0 {
        return Err(ConfigValidationError::InvalidProxyAdminListMaxLimit);
    }
    validate_proxy_ssh_file_transfer_policy(settings)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigValidationError, MIN_LOCAL_ADMIN_PASSWORD_LEN, MIN_PROXY_NODE_SECRET_LEN,
        check_cipher_key_value, check_proxy_secret_value, validate_production_database_password,
        validate_production_local_admin_password, validate_production_password_pepper,
        validate_production_redis_password, validate_production_secrets, validate_settings,
    };
    use crate::config::load_settings;

    const STRONG_CIPHER_KEY_B64: &str = "c3Ryb25nLTMyLWJ5dGUta2V5LWZvci10ZXN0aW5nISE=";
    const STRONG_PROXY_SECRET: &str = "a-strong-proxy-secret-value-32chars";
    const DEFAULT_CIPHER_KEY_B64: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
    const DEFAULT_CIPHER_KEY_ASCII: &str = "0123456789abcdef0123456789abcdef";

    fn valid_settings_fixture() -> crate::config::Settings {
        let mut settings = load_settings().expect("load test settings from config files");
        settings.database.url = "postgresql://piping:piping@localhost:5432/piping".to_string();
        settings.redis.url = "redis://localhost:6379/0".to_string();
        settings.secrets.cipher.key_material_ref = "inline:test-key-material".to_string();
        settings.sync_runtime.callback_base_url = "http://localhost:7000".to_string();
        // Dev tolerance on: weak/default secrets are permitted (mirrors local/dev).
        settings.security.allow_weak_dev_secrets = true;
        settings
    }

    #[test]
    fn accepts_cookie_same_site_known_values_case_insensitive() {
        let mut settings = valid_settings_fixture();
        settings.auth.cookie_same_site = "STRICT".to_string();
        let strict_result = validate_settings(&settings);
        assert!(strict_result.is_ok(), "{strict_result:?}");

        settings.auth.cookie_same_site = "lax".to_string();
        let lax_result = validate_settings(&settings);
        assert!(lax_result.is_ok(), "{lax_result:?}");

        settings.auth.cookie_same_site = "None".to_string();
        let none_result = validate_settings(&settings);
        assert!(none_result.is_ok(), "{none_result:?}");
    }

    #[test]
    fn rejects_cookie_same_site_unknown_value() {
        let mut settings = valid_settings_fixture();
        settings.auth.cookie_same_site = "invalid".to_string();

        assert!(matches!(
            validate_settings(&settings),
            Err(ConfigValidationError::InvalidAuthCookieSameSite)
        ));
    }

    #[test]
    fn accepts_http_monitoring_query_url() {
        let mut settings = valid_settings_fixture();
        settings.monitoring.query_url = Some("https://victoriametrics.example/api/v1/query".into());

        let result = validate_settings(&settings);

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn rejects_invalid_monitoring_query_url() {
        let mut settings = valid_settings_fixture();
        settings.monitoring.query_url = Some("file:///tmp/vm-query.sock".into());

        assert!(matches!(
            validate_settings(&settings),
            Err(ConfigValidationError::InvalidMonitoringQueryUrl)
        ));
    }

    #[test]
    fn accepts_https_playbook_self_callback_base_url() {
        let mut settings = valid_settings_fixture();
        settings.playbook_runtime.self_callback_base_url =
            "https://piping.example:7000".to_string();

        let result = validate_settings(&settings);

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn accepts_http_loopback_playbook_self_callback_base_url() {
        let mut settings = valid_settings_fixture();
        settings.playbook_runtime.self_callback_base_url = "http://127.0.0.1:7000".to_string();

        let result = validate_settings(&settings);

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn rejects_non_url_playbook_self_callback_base_url() {
        let mut settings = valid_settings_fixture();
        settings.playbook_runtime.self_callback_base_url = "not a url".to_string();

        assert!(matches!(
            validate_settings(&settings),
            Err(ConfigValidationError::InvalidPlaybookSelfCallbackBaseUrl)
        ));
    }

    #[test]
    fn rejects_non_http_scheme_playbook_self_callback_base_url() {
        let mut settings = valid_settings_fixture();
        settings.playbook_runtime.self_callback_base_url = "ftp://example.com".to_string();

        assert!(matches!(
            validate_settings(&settings),
            Err(ConfigValidationError::InvalidPlaybookSelfCallbackBaseUrl)
        ));
    }

    #[test]
    fn accepts_plaintext_http_internal_playbook_self_callback_base_url() {
        // The sidecar->backend callback is an internal-network hop carrying a
        // run-scoped token that never leaves it — same rule as the dispatch URL.
        let mut settings = valid_settings_fixture();
        settings.playbook_runtime.self_callback_base_url = "http://backend:7000".to_string();

        assert!(validate_settings(&settings).is_ok());
    }

    #[test]
    fn rejects_zero_monitoring_query_limits() {
        let mut timeout_settings = valid_settings_fixture();
        timeout_settings.monitoring.query_timeout_ms = 0;
        assert!(matches!(
            validate_settings(&timeout_settings),
            Err(ConfigValidationError::InvalidMonitoringQueryTimeout)
        ));

        let mut max_query_settings = valid_settings_fixture();
        max_query_settings.monitoring.query_max_queries = 0;
        assert!(matches!(
            validate_settings(&max_query_settings),
            Err(ConfigValidationError::InvalidMonitoringQueryMaxQueries)
        ));
    }

    #[test]
    fn proxy_ssh_file_transfer_defaults_are_validated() {
        let settings = valid_settings_fixture();

        assert_eq!(
            settings.proxy.ssh_file_transfer.max_upload_bytes,
            crate::config::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_UPLOAD_BYTES
        );
        assert_eq!(
            settings.proxy.ssh_file_transfer.max_download_bytes,
            crate::config::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_DOWNLOAD_BYTES
        );
        assert_eq!(
            settings.proxy.ssh_file_transfer.max_sidecars_total,
            crate::config::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_SIDECARS_TOTAL
        );
        assert_eq!(
            settings.proxy.ssh_file_transfer.max_sidecars_per_session,
            crate::config::DEFAULT_PROXY_SSH_FILE_TRANSFER_MAX_SIDECARS_PER_SESSION
        );
        assert!(validate_settings(&settings).is_ok());
    }

    #[test]
    fn rejects_invalid_proxy_ssh_file_transfer_policy() {
        let mut upload_settings = valid_settings_fixture();
        upload_settings.proxy.ssh_file_transfer.max_upload_bytes = 0;
        assert!(matches!(
            validate_settings(&upload_settings),
            Err(ConfigValidationError::InvalidProxySshFileTransferMaxUploadBytes)
        ));

        let mut download_settings = valid_settings_fixture();
        download_settings.proxy.ssh_file_transfer.max_download_bytes = 0;
        assert!(matches!(
            validate_settings(&download_settings),
            Err(ConfigValidationError::InvalidProxySshFileTransferMaxDownloadBytes)
        ));

        let mut total_settings = valid_settings_fixture();
        total_settings.proxy.ssh_file_transfer.max_sidecars_total = 0;
        assert!(matches!(
            validate_settings(&total_settings),
            Err(ConfigValidationError::InvalidProxySshFileTransferMaxSidecarsTotal)
        ));

        let mut per_session_settings = valid_settings_fixture();
        per_session_settings
            .proxy
            .ssh_file_transfer
            .max_sidecars_total = 2;
        per_session_settings
            .proxy
            .ssh_file_transfer
            .max_sidecars_per_session = 3;
        assert!(matches!(
            validate_settings(&per_session_settings),
            Err(ConfigValidationError::InvalidProxySshFileTransferMaxSidecarsPerSession)
        ));
    }

    /// Builds a production-mode settings fixture (enforcement on, no override)
    /// with strong, non-default config-sourced secrets (pepper, DB/Redis).
    ///
    /// The cipher key and proxy secret are env-sourced at runtime; tests exercise
    /// those via the pure `check_cipher_key_value` / `check_proxy_secret_value`
    /// helpers, since the crate forbids `unsafe` (so process env cannot be mutated).
    fn production_settings_fixture() -> crate::config::Settings {
        let mut settings = valid_settings_fixture();
        // Production posture: strong-secret enforcement on (no dev tolerance).
        settings.security.allow_weak_dev_secrets = false;
        // Strong, non-default config-sourced secrets.
        settings.auth.password_pepper = "a-strong-production-pepper-value".to_string();
        settings.database.url =
            "postgresql://piping:strong-db-password@localhost:5432/piping".to_string();
        settings.redis.url = "redis://:strong-redis-password@localhost:6379/0".to_string();
        settings.secrets.cipher.key_material_ref = format!("inline:{STRONG_CIPHER_KEY_B64}");
        settings
    }

    #[test]
    fn production_rejects_default_cipher_key() {
        // Both the shipped base64 default and its decoded ASCII form must fail.
        assert!(
            matches!(
                check_cipher_key_value(DEFAULT_CIPHER_KEY_B64),
                Err(ConfigValidationError::InsecureSecretsCipherKey)
            ),
            "expected default base64 cipher key to be rejected"
        );
        assert!(
            matches!(
                check_cipher_key_value(DEFAULT_CIPHER_KEY_ASCII),
                Err(ConfigValidationError::InsecureSecretsCipherKey)
            ),
            "expected default ascii cipher key to be rejected"
        );
        assert!(
            matches!(
                check_cipher_key_value("   "),
                Err(ConfigValidationError::InsecureSecretsCipherKey)
            ),
            "expected empty cipher key to be rejected"
        );
    }

    #[test]
    fn production_accepts_strong_cipher_key() {
        assert!(
            check_cipher_key_value(STRONG_CIPHER_KEY_B64).is_ok(),
            "expected strong cipher key to pass"
        );
    }

    #[test]
    fn production_rejects_default_or_weak_proxy_secret() {
        assert!(
            matches!(
                check_proxy_secret_value("change-this-proxy-secret"),
                Err(ConfigValidationError::InsecureProxySharedSecret {
                    min: MIN_PROXY_NODE_SECRET_LEN
                })
            ),
            "expected default proxy secret to be rejected"
        );
        assert!(
            matches!(
                check_proxy_secret_value("short"),
                Err(ConfigValidationError::InsecureProxySharedSecret { .. })
            ),
            "expected too-short proxy secret to be rejected"
        );
        assert!(
            matches!(
                check_proxy_secret_value(""),
                Err(ConfigValidationError::InsecureProxySharedSecret { .. })
            ),
            "expected empty proxy secret to be rejected"
        );
    }

    #[test]
    fn production_accepts_strong_proxy_secret() {
        assert!(
            check_proxy_secret_value(STRONG_PROXY_SECRET).is_ok(),
            "expected strong proxy secret to pass"
        );
    }

    #[test]
    fn non_production_allows_default_secrets() {
        // Non-prod override on with all-default/weak secrets must still pass:
        // validate_production_secrets short-circuits before any secret checks.
        let mut settings = production_settings_fixture();
        settings.security.allow_weak_dev_secrets = true;
        settings.auth.password_pepper = "dev-pepper".to_string();
        settings.database.url =
            "postgresql://piping:piping-change-me@localhost:5432/piping".to_string();
        settings.redis.url = "redis://:redis-change-me@localhost:6379/0".to_string();
        let result = validate_production_secrets(&settings);
        assert!(
            result.is_ok(),
            "expected non-production mode to permit default secrets, got {result:?}"
        );
    }

    #[test]
    fn production_rejects_default_password_pepper() {
        let mut settings = production_settings_fixture();
        settings.auth.password_pepper = "dev-pepper".to_string();
        assert!(
            matches!(
                validate_production_password_pepper(&settings),
                Err(ConfigValidationError::InsecureAuthPasswordPepper)
            ),
            "expected default pepper to be rejected in production"
        );
    }

    #[test]
    fn production_accepts_strong_password_pepper() {
        let settings = production_settings_fixture();
        assert!(
            validate_production_password_pepper(&settings).is_ok(),
            "expected strong pepper to pass"
        );
    }

    #[test]
    fn production_settings_fail_closed_without_local_admin_password() {
        let mut settings = production_settings_fixture();
        settings.auth.local_enabled = true;
        settings.auth.local_admin_password = None;

        assert!(
            matches!(
                validate_settings(&settings),
                Err(ConfigValidationError::InsecureAuthLocalAdminPassword {
                    min: MIN_LOCAL_ADMIN_PASSWORD_LEN
                })
            ),
            "production posture with local auth enabled must refuse to start without \
             a local-admin password"
        );
    }

    #[test]
    fn production_rejects_missing_or_weak_local_admin_password() {
        let mut settings = production_settings_fixture();
        settings.auth.local_enabled = true;

        settings.auth.local_admin_password = None;
        assert!(
            matches!(
                validate_production_local_admin_password(&settings),
                Err(ConfigValidationError::InsecureAuthLocalAdminPassword { .. })
            ),
            "expected a missing local-admin password to be rejected"
        );

        settings.auth.local_admin_password = Some("piping123".to_string());
        assert!(
            matches!(
                validate_production_local_admin_password(&settings),
                Err(ConfigValidationError::InsecureAuthLocalAdminPassword { .. })
            ),
            "expected the known dev default local-admin password to be rejected"
        );

        settings.auth.local_admin_password = Some("short1".to_string());
        assert!(
            matches!(
                validate_production_local_admin_password(&settings),
                Err(ConfigValidationError::InsecureAuthLocalAdminPassword { .. })
            ),
            "expected a too-short local-admin password to be rejected"
        );

        settings.auth.local_admin_password = Some("   ".to_string());
        assert!(
            matches!(
                validate_production_local_admin_password(&settings),
                Err(ConfigValidationError::InsecureAuthLocalAdminPassword { .. })
            ),
            "expected a blank local-admin password to be rejected"
        );
    }

    #[test]
    fn production_accepts_strong_local_admin_password() {
        let mut settings = production_settings_fixture();
        settings.auth.local_enabled = true;
        settings.auth.local_admin_password = Some("a-strong-local-admin-pass-1".to_string());

        assert!(
            validate_production_local_admin_password(&settings).is_ok(),
            "expected a strong non-default local-admin password to pass"
        );
    }

    #[test]
    fn production_does_not_require_local_admin_password_without_local_auth() {
        let mut settings = production_settings_fixture();
        settings.auth.local_enabled = false;
        settings.auth.local_admin_password = None;

        assert!(
            validate_production_local_admin_password(&settings).is_ok(),
            "no local administrator is created when local auth is disabled, so no \
             password can be required"
        );
    }

    #[test]
    fn non_production_allows_missing_local_admin_password() {
        let mut settings = production_settings_fixture();
        settings.security.allow_weak_dev_secrets = true;
        settings.auth.local_enabled = true;
        settings.auth.local_admin_password = None;

        assert!(
            validate_production_secrets(&settings).is_ok(),
            "explicit dev tolerance must keep the local fallback usable without an override"
        );
    }

    #[test]
    fn production_rejects_default_database_password() {
        let mut settings = production_settings_fixture();
        settings.database.url =
            "postgresql://piping:piping-change-me@localhost:5432/piping".to_string();
        assert!(
            matches!(
                validate_production_database_password(&settings),
                Err(ConfigValidationError::InsecureDatabasePassword)
            ),
            "expected default DB password to be rejected in production"
        );
    }

    #[test]
    fn production_rejects_default_redis_password() {
        let mut settings = production_settings_fixture();
        settings.redis.url = "redis://:redis-change-me@localhost:6379/0".to_string();
        assert!(
            matches!(
                validate_production_redis_password(&settings),
                Err(ConfigValidationError::InsecureRedisPassword)
            ),
            "expected default Redis password to be rejected in production"
        );
    }

    #[test]
    fn production_accepts_strong_db_and_redis_passwords() {
        let settings = production_settings_fixture();
        assert!(
            validate_production_database_password(&settings).is_ok(),
            "expected strong DB password to pass"
        );
        assert!(
            validate_production_redis_password(&settings).is_ok(),
            "expected strong Redis password to pass"
        );
    }
}
