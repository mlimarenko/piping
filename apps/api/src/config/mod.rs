//! Configuration module for application settings.
//!
//! This module handles loading, parsing, and validating configuration from:
//! - Config files (`config/default.toml`, `config/local.toml`)
//! - Environment variables (`PIPING_*` prefix with `__` separators)
//!
//! Configuration is loaded in order (later sources override earlier):
//! 1. `config/default.toml` (base configuration)
//! 2. `config/local.toml` (optional local overrides)
//! 3. Environment variables (highest priority)
//!
//! Environment variables use double underscores between nesting levels,
//! e.g. `PIPING_DATABASE__URL` maps to `database.url`.
//!
//! # Example
//! ```rust,ignore
//! use piping_backend::config;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let settings = config::load_settings()?;
//!     settings.validate()?;
//!     Ok(())
//! }
//! ```

pub mod constants;
mod helpers;
mod loader;
mod types;
mod validation;

pub use constants::*;
pub use helpers::{effective_proxy_base_url, public_base_url_from_headers};
pub use loader::load_settings;
pub use types::*;
pub use validation::ConfigValidationError;

use serde::Deserialize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    #[default]
    Backend,
    Proxy,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartMode {
    #[default]
    Start,
    Migrate,
}

/// Application settings aggregating all configuration sections.
///
/// This is the root configuration structure that contains all subsystem configs.
/// It's loaded from config files and environment variables via `load_settings()`.
///
/// # Configuration Sources
///
/// Settings are loaded from:
/// 1. `config/default.toml` (required base config)
/// 2. `config/local.toml` (optional local overrides)
/// 3. Environment variables with `PIPING_` prefix
///
/// # Validation
///
/// Call `validate()` after loading to ensure all required fields are present
/// and values are within valid ranges.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    /// HTTP server configuration (bind address, etc.)
    pub server: ServerConfig,
    /// Runtime mode (backend or proxy).
    #[serde(default)]
    pub run_mode: RunMode,
    /// Startup mode (start or migrate).
    #[serde(default)]
    pub start_mode: StartMode,

    /// PostgreSQL database configuration (connection pool, URLs, etc.)
    pub database: DatabaseConfig,

    /// Redis cache configuration (connection URL, etc.)
    pub redis: RedisConfig,

    /// Secrets cipher configuration (key material ref + version strategy).
    /// Defaults to deterministic values if not configured.
    #[serde(default)]
    pub secrets: SecretsConfig,

    /// Observability configuration (logging, tracing, etc.)
    pub observability: ObservabilityConfig,

    /// Monitoring configuration for Prometheus metrics and service discovery.
    /// Defaults to sensible values if not configured.
    #[serde(default)]
    pub monitoring: MonitoringConfig,

    /// Authentication configuration (local auth, OIDC, sessions, etc.)
    /// Defaults to all disabled if not configured
    #[serde(default)]
    pub auth: AuthConfig,

    /// Proxy service configuration (connection routing)
    /// Defaults to empty if not configured
    #[serde(default)]
    pub proxy: ProxyConfig,

    /// Sync runtime configuration for source bootstrap/webhook/outbox behavior.
    #[serde(default)]
    pub sync_runtime: SyncRuntimeConfig,

    /// Playbook executor data-plane runtime configuration (run/schedule workers).
    #[serde(default)]
    pub playbook_runtime: PlaybookRuntimeConfig,

    /// `ansible-runner` sidecar connection settings (job dispatch transport).
    #[serde(default)]
    pub ansible_runner: AnsibleRunnerConfig,

    /// Official playbook catalog source configuration (baked snapshot +
    /// optional git refresh overlay).
    #[serde(default)]
    pub official_catalog: OfficialCatalogConfig,

    /// Commercial license integration configuration.
    #[serde(default)]
    pub license: LicenseConfig,

    /// Security posture configuration (weak-dev-secret tolerance, etc.).
    #[serde(default)]
    pub security: SecurityConfig,
}

impl Settings {
    /// Validates all required configuration fields and constraints.
    ///
    /// Performs validation checks:
    /// - Required URLs are present and valid
    /// - Database pool settings are valid (min <= max, timeouts > 0)
    /// - Auth configuration is valid (pepper if local enabled, OIDC fields if OIDC enabled)
    ///
    /// Returns `ConfigValidationError` if validation fails.
    /// Should be called immediately after `load_settings()`.
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        validation::validate_settings(self)
    }

    pub fn validate_for_mode(&self, run_mode: RunMode) -> Result<(), ConfigValidationError> {
        validation::validate_settings_for_mode(self, run_mode)
    }
}
