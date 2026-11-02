//! DB-backed smoke tests for inventory list search SQL.
//!
//! Most inventory list endpoints build SQL dynamically through repository-level
//! `QueryBuilder`s. Unit tests and `cargo check` catch Rust contract drift, but
//! not misspelled table aliases or column names inside search expressions. This
//! ignored test executes representative repository list methods against a
//! migrated database so search wiring fails fast before it reaches the UI.

mod common;

use piping_backend::contexts::inventory::infrastructure::persistence::{
    CircuitFilter, CircuitRepository, ConsolePortRepository, ContactAssignmentFilter,
    ContactAssignmentRepository, DeviceFilter,
    DeviceRepository, IpAddressListFilter, IpAddressRepository, LocationFilter, LocationRepository,
    ProviderAccountRepository, ProviderNetworkRepository, ProviderRepository, SqlxCircuitRepository,
    SqlxConsolePortRepository, SqlxContactAssignmentRepository, SqlxDeviceRepository,
    SqlxIpAddressRepository, SqlxLocationRepository, SqlxProviderAccountRepository,
    SqlxProviderNetworkRepository, SqlxProviderRepository, SqlxTemplateRepository,
    TemplateRepository,
};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::dcim::console_port_repository::ConsolePortFilter;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::native::template_repository::TemplateRunListFilter;

async fn migrated_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    Ok(pool)
}

#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn inventory_search_queries_execute_against_live_schema()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;

    // Repository search params normally receive the route-normalized SQL pattern.
    let pattern = Some("%a%".to_string());
    let ip_pattern = Some("%10.10.1.%".to_string());

    let ip_addresses = SqlxIpAddressRepository::new(pool.clone());
    ip_addresses
        .list(
            IpAddressListFilter {
                search: ip_pattern,
                ..Default::default()
            },
            Some(5),
            Some(0),
            None,
        )
        .await
        .expect("IP address search SQL executes");

    let devices = SqlxDeviceRepository::new(pool.clone());
    devices
        .list_hydrated(
            DeviceFilter::default(),
            Some(5),
            Some(0),
            pattern.as_deref(),
        )
        .await
        .expect("device hydrated search SQL executes");

    let console_ports = SqlxConsolePortRepository::new(pool.clone());
    console_ports
        .list(
            ConsolePortFilter {
                device_id: Vec::new(),
                module_id: Vec::new(),
                type_: Vec::new(),
                mark_connected: Vec::new(),
                cable_id: Vec::new(),
                search: pattern.clone(),
                sort: None,
            },
            Some(5),
            Some(0),
        )
        .await
        .expect("console port search SQL executes");

    let locations = SqlxLocationRepository::new(pool.clone());
    locations
        .list(
            LocationFilter {
                search: pattern.clone(),
                ..LocationFilter::default()
            },
            Some(5),
            Some(0),
        )
        .await
        .expect("location search SQL executes");

    let providers = SqlxProviderRepository::new(pool.clone());
    providers
        .list_paginated(Some(5), Some(0), pattern.clone(), None)
        .await
        .expect("provider search SQL executes");

    let provider_accounts = SqlxProviderAccountRepository::new(pool.clone());
    provider_accounts
        .list_paginated_hydrated(Vec::new(), pattern.clone(), None, Some(5), Some(0))
        .await
        .expect("provider account hydrated search SQL executes");

    let provider_networks = SqlxProviderNetworkRepository::new(pool.clone());
    provider_networks
        .list_hydrated(Vec::new(), pattern.clone(), None, Some(5), Some(0))
        .await
        .expect("provider network hydrated search SQL executes");

    let circuits = SqlxCircuitRepository::new(pool.clone());
    circuits
        .list_hydrated(
            CircuitFilter {
                search: pattern.clone(),
                ..CircuitFilter::default()
            },
            Some(5),
            Some(0),
        )
        .await
        .expect("circuit hydrated search SQL executes");

    let contact_assignments = SqlxContactAssignmentRepository::new(pool.clone());
    contact_assignments
        .list(
            ContactAssignmentFilter {
                search: pattern.clone(),
                ..ContactAssignmentFilter::default()
            },
            Some(5),
            Some(0),
        )
        .await
        .expect("contact assignment search SQL executes");

    // Template repository methods accept raw search text; their SQL wraps it in
    // `%...%` itself, unlike the generic inventory repositories above.
    let templates = SqlxTemplateRepository::new(pool);
    templates
        .list_templates_page(Vec::new(), Some("a".to_string()), None, 5, 0)
        .await
        .expect("template search SQL executes");
    templates
        .list_template_runs_page(
            TemplateRunListFilter {
                status_filter: Vec::new(),
                template_filter: Vec::new(),
                search_filter: Some("a".to_string()),
                sort: None,
            },
            5,
            0,
        )
        .await
        .expect("template run search SQL executes");
    Ok(())
}
