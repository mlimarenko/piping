//! Piping API server binary.

use std::error::Error;
use std::sync::Arc;

use piping_backend::{app, config, contexts, observability, proxy, storage};
use redis::aio::ConnectionManager;
use sqlx::PgPool;

type MainResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Main entry point for the Piping backend application.
///
/// Initializes the application in the following order:
/// 1. Loads and validates configuration from files and environment variables
/// 2. Initializes observability (logging, tracing)
/// 3. Connects to `PostgreSQL` database and runs migrations
/// 4. Connects to Redis cache
/// 5. Starts the HTTP server with configured routes and middleware
#[tokio::main]
async fn main() -> MainResult {
    install_rustls_crypto_provider();

    // Load configuration from config files and environment variables
    // See config::loader::load_settings() for loading order
    let mut settings = config::load_settings()?;
    let run_mode = settings.run_mode;
    apply_run_mode_defaults(&mut settings, run_mode);
    let settings = Arc::new(settings);
    settings.validate_for_mode(run_mode)?;

    // Initialize logging and tracing based on configuration
    observability::init_observability(&settings)?;

    // Apply the SSRF escape-hatch from config (default off / fail-closed).
    // Done after observability init so the disabled-guard warning is captured.
    contexts::shared::ssrf::set_ssrf_allow_private(settings.security.ssrf_allow_private);

    log_bootstrap_started();

    if matches!(run_mode, config::RunMode::Proxy) {
        return run_proxy_mode(&settings).await;
    }

    run_backend_mode(settings).await
}

fn install_rustls_crypto_provider() {
    // rustls 0.23 requires a process-level crypto provider when TLS is used via
    // tokio-tungstenite (wss proxy routing). Ignore "already installed" races.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn apply_run_mode_defaults(settings: &mut config::Settings, run_mode: config::RunMode) {
    if matches!(run_mode, config::RunMode::Proxy)
        && std::env::var_os("PIPING_OBSERVABILITY__OTEL_ENABLED").is_none()
    {
        settings.observability.otel_enabled = false;
    }
}

fn log_bootstrap_started() {
    observability::log_event(
        observability::LogLevel::Info,
        "system.bootstrap.started",
        observability::EventFields::default()
            .with_operation("bootstrap")
            .with_status("starting"),
    );
}

async fn run_proxy_mode(settings: &Arc<config::Settings>) -> MainResult {
    let proxy_state = Arc::new(proxy::ProxyState::new(&settings.proxy)?);
    let bind = settings.server.bind.clone();
    observability::log_event(
        observability::LogLevel::Info,
        "system.proxy_mode.started",
        observability::EventFields::default()
            .with_operation("proxy_mode_start")
            .with_resource("listener", bind.clone()),
    );
    proxy::spawn_ssh_detach_reaper(proxy_state.clone());
    proxy::serve_proxy(proxy_state, &bind).await?;
    Ok(())
}

async fn run_backend_mode(settings: Arc<config::Settings>) -> MainResult {
    let db_pool = init_database(&settings).await?;
    apply_migrations(&db_pool).await?;

    // Allow migration-only runs to exit before starting the server.
    if !should_start_backend_server(settings.start_mode) {
        observability::log_event(
            observability::LogLevel::Info,
            "system.bootstrap.completed",
            observability::EventFields::default()
                .with_operation("bootstrap")
                .with_status("migrations_only"),
        );
        return Ok(());
    }

    let redis_manager = init_redis(&settings).await?;
    serve_backend(settings, db_pool, redis_manager).await
}

fn should_start_backend_server(start_mode: config::StartMode) -> bool {
    matches!(start_mode, config::StartMode::Start)
}

async fn init_database(
    settings: &Arc<config::Settings>,
) -> Result<PgPool, Box<dyn Error + Send + Sync>> {
    // Create PostgreSQL connection pool with configured limits
    // The pool manages multiple database connections for concurrent requests
    observability::log_event(
        observability::LogLevel::Info,
        "database.connection.requested",
        observability::EventFields::default().with_operation("database.connect"),
    );
    let db_pool = match storage::database::pool::build_pool(&settings.database).await {
        Ok(pool) => {
            observability::log_event(
                observability::LogLevel::Info,
                "database.connection.ready",
                observability::EventFields::default()
                    .with_operation("database.connect")
                    .with_status("ok"),
            );
            pool
        }
        Err(e) => {
            observability::log_error_event(
                "database.connection.failed",
                "database_error",
                e.to_string(),
                observability::EventFields::default()
                    .with_operation("database.connect")
                    .with_resource("system_component", "database"),
            );
            observability::log_error_event(
                "database.connection.hint",
                "database_error",
                "ensure PostgreSQL is running and PIPING_DATABASE__URL is correct (e.g. postgres://user:pass@localhost:5432/dbname)",
                observability::EventFields::default().with_operation("database.connect"),
            );
            return Err(e.into());
        }
    };

    Ok(db_pool)
}

async fn apply_migrations(db_pool: &PgPool) -> MainResult {
    // Run database migrations to ensure schema is up to date
    // Migrations are applied automatically on startup
    observability::log_event(
        observability::LogLevel::Info,
        "database.migration.started",
        observability::EventFields::default().with_operation("database.migrate"),
    );
    if let Err(e) = storage::database::migrations::run_migrations(db_pool).await {
        observability::log_error_event(
            "database.migration.failed",
            "migration_error",
            e.to_string(),
            observability::EventFields::default()
                .with_operation("database.migrate")
                .with_resource("system_component", "database_migration"),
        );
        return Err(e.into());
    }
    observability::log_event(
        observability::LogLevel::Info,
        "database.migration.completed",
        observability::EventFields::default()
            .with_operation("database.migrate")
            .with_status("ok"),
    );

    Ok(())
}

async fn init_redis(
    settings: &Arc<config::Settings>,
) -> Result<ConnectionManager, Box<dyn Error + Send + Sync>> {
    // Connect to Redis for caching and session storage
    // Redis is used for OIDC session metadata and other cached data
    observability::log_event(
        observability::LogLevel::Info,
        "cache.redis.connection.requested",
        observability::EventFields::default().with_operation("redis.connect"),
    );
    let redis_manager = match storage::cache::redis::client::build_connection_manager(
        &settings.redis.url,
    )
    .await
    {
        Ok(conn) => {
            observability::log_event(
                observability::LogLevel::Info,
                "cache.redis.connection.ready",
                observability::EventFields::default()
                    .with_operation("redis.connect")
                    .with_status("ok"),
            );
            conn
        }
        Err(e) => {
            observability::log_error_event(
                "cache.redis.connection.failed",
                "redis_error",
                e.to_string(),
                observability::EventFields::default()
                    .with_operation("redis.connect")
                    .with_resource("system_component", "redis"),
            );
            observability::log_error_event(
                "cache.redis.connection.hint",
                "redis_error",
                "ensure Redis is running and PIPING_REDIS__URL is correct (e.g. redis://localhost:6379)",
                observability::EventFields::default().with_operation("redis.connect"),
            );
            return Err(e.into());
        }
    };

    Ok(redis_manager)
}

async fn serve_backend(
    settings: Arc<config::Settings>,
    db_pool: PgPool,
    redis_manager: ConnectionManager,
) -> MainResult {
    // Create application state with all dependencies
    // AppState is shared across all request handlers via axum's State extractor
    let bind = settings.server.bind.clone();
    let state = app::AppState::build(settings, db_pool, redis_manager).await?;

    // Start the HTTP server and begin accepting connections
    // The server handles REST API and WebSocket connections
    observability::log_event(
        observability::LogLevel::Info,
        "system.bootstrap.completed",
        observability::EventFields::default()
            .with_operation("bootstrap")
            .with_status("ready")
            .with_resource("listener", bind),
    );
    app::serve(state).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::should_start_backend_server;
    use piping_backend::config::StartMode;

    #[test]
    fn start_mode_start_runs_server() {
        assert!(should_start_backend_server(StartMode::Start));
    }

    #[test]
    fn start_mode_migrate_skips_server() {
        assert!(!should_start_backend_server(StartMode::Migrate));
    }
}
