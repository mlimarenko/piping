//! PIP-244 proof of the `0019 -> 0020` monitoring profile upgrade.
//!
//! Runs against a disposable database created from `PIPING_DATABASE__URL` and
//! force-dropped afterwards. The fixture seeds every legacy exporter-assignment
//! shape, applies 0020 once, and asserts row/override preservation, built-in
//! profile seeding, and the documented delete behaviour.
use sqlx::{PgPool, Postgres, migrate::MigrateDatabase};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const MIGRATIONS: [&str; 19] = [
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_interface_networking.sql"),
    include_str!("../migrations/0003_service_port_mappings.sql"),
    include_str!("../migrations/0004_fhrp_auth_key_secret.sql"),
    include_str!("../migrations/0005_module_bay_types.sql"),
    include_str!("../migrations/0006_canonical_tag_ids.sql"),
    include_str!("../migrations/0007_cooling_hardware.sql"),
    include_str!("../migrations/0008_module_type_profile_scope.sql"),
    include_str!("../migrations/0009_circuit_group_assignments.sql"),
    include_str!("../migrations/0010_vlan_translation.sql"),
    include_str!("../migrations/0011_termination_graph.sql"),
    include_str!("../migrations/0012_inventory_transfer_run.sql"),
    include_str!("../migrations/0013_component_template_instantiation.sql"),
    include_str!("../migrations/0014_tag_object_type_scope.sql"),
    include_str!("../migrations/0015_device_bay_template_enabled.sql"),
    include_str!("../migrations/0016_project_workspace.sql"),
    include_str!("../migrations/0017_dataflow_catalog.sql"),
    include_str!("../migrations/0018_target_set_delete_restrict.sql"),
    include_str!("../migrations/0019_config_context_semantics.sql"),
];
const PROFILES: &str = include_str!("../migrations/0020_monitoring_profiles.sql");

const NODE_EXPORTER: &str = "019ee0e0-69f8-7d17-93d1-d5299497329a";

const ASSIGNMENT_GLOBAL: &str = "02440000-0000-7000-8000-000000000001";
const ASSIGNMENT_ROLE: &str = "02440000-0000-7000-8000-000000000002";
const ASSIGNMENT_HOST: &str = "02440000-0000-7000-8000-000000000003";
const ASSIGNMENT_DISABLED: &str = "02440000-0000-7000-8000-000000000004";
const ASSIGNMENT_TARGET: &str = "02440000-0000-7000-8000-000000000005";

const ROLE_ID: &str = "02440000-0000-7000-8000-000000001000";
const HOST_ID: &str = "02440000-0000-7000-8000-000000002000";
const SITE_ID: &str = "02440000-0000-7000-8000-000000003000";

/// Legacy fixture: global default, role with a job override, host-specific
/// override, disabled row and explicit target. `smartctl` and
/// `blackbox-interface` exporters are removed so the conditional seeding of
/// built-in profiles is observable.
const FIXTURE: &str = r#"
DELETE FROM monitoring_exporter_assignment;
DELETE FROM monitoring_exporter WHERE slug IN ('smartctl', 'blackbox-interface');

INSERT INTO monitoring_exporter_assignment
    (id, exporter_id, name, scope_type, scope_id, weight, enabled, target,
     job, scheme, port, metrics_path, labels)
VALUES
    ('02440000-0000-7000-8000-000000000001', '019ee0e0-69f8-7d17-93d1-d5299497329a',
     'Default node exporter', 'global', NULL, 0, true, NULL, NULL, NULL, NULL, NULL, '{}'),
    ('02440000-0000-7000-8000-000000000002', '019ee0e0-69f8-7e95-bdf6-1ad81f201faa',
     'Role nginx', 'role', '02440000-0000-7000-8000-000000001000', 5, true,
     'edge.example', 'nginx', 'https', 9113, '/nginx', '{"tier":"web"}'),
    ('02440000-0000-7000-8000-000000000003', '019ee0e0-69f8-7d17-93d1-d5299497329a',
     NULL, 'connection_target', '02440000-0000-7000-8000-000000002000', 2, true,
     NULL, 'node-alt', NULL, 9101, '/custom', '{"team":"ops"}'),
    ('02440000-0000-7000-8000-000000000004', '019ee0e0-69f8-7eae-9923-019965141280',
     '   ', 'global', NULL, 1, false, NULL, NULL, NULL, NULL, NULL, '{}'),
    ('02440000-0000-7000-8000-000000000005', '019ee0e0-69f8-7d17-93d1-d5299497329a',
     'Site node', 'site', '02440000-0000-7000-8000-000000003000', 0, true,
     'metrics.example', NULL, NULL, NULL, NULL, '{}');
"#;

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL 18 database with CREATEDB"]
async fn profile_upgrade_preserves_assignments_and_enforces_delete_rules() -> TestResult {
    let url = create_database("pip244mig").await?;
    let outcome = run_upgrade(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    outcome?;
    cleanup?;
    Ok(())
}

async fn create_database(prefix: &str) -> TestResult<String> {
    let mut url = url::Url::parse(&std::env::var("PIPING_DATABASE__URL")?)?;
    url.set_path(&format!("/{prefix}_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    Ok(url)
}

async fn run_upgrade(url: &str) -> TestResult {
    let pool = PgPool::connect(url).await?;
    for migration in MIGRATIONS {
        sqlx::raw_sql(migration).execute(&pool).await?;
    }
    sqlx::raw_sql(FIXTURE).execute(&pool).await?;

    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(PROFILES).execute(&mut *transaction).await?;
    transaction.commit().await?;

    assert!(
        !table_exists(&pool, "monitoring_exporter_assignment").await?,
        "0020 must drop the legacy assignment table"
    );

    assert_converted_rows(&pool).await?;
    assert_seeded_profiles(&pool).await?;
    assert_delete_rules(&pool).await?;

    pool.close().await;
    Ok(())
}

async fn assert_converted_rows(pool: &PgPool) -> TestResult {
    let rows = sqlx::query(
        r#"
        SELECT
            a.id,
            a.scope_type,
            a.scope_id,
            a.weight,
            a.enabled,
            a.target,
            p.id AS profile_id,
            p.name AS profile_name,
            p.slug AS profile_slug,
            i.exporter_id,
            i.job,
            i.scheme,
            i.port,
            i.metrics_path,
            i.labels,
            i.enabled AS item_enabled,
            i.sort_order
        FROM monitoring_profile_assignment a
        JOIN monitoring_profile p ON p.id = a.profile_id
        JOIN monitoring_profile_item i ON i.profile_id = p.id
        ORDER BY a.id
        "#,
    )
    .fetch_all(pool)
    .await?;
    assert_eq!(
        rows.len(),
        5,
        "every legacy row must become one profile item"
    );

    for row in &rows {
        use sqlx::Row;
        let slug: String = row.try_get("profile_slug")?;
        assert!(
            slug.len() <= 128,
            "profile slug must fit the column: {slug}"
        );
        let assignment_id = format!("{}", field_uuid(row, "id")?).replace('-', "");
        assert!(
            slug.ends_with(&assignment_id),
            "profile slug must end with the assignment id hex so same-named rows stay unique: {slug}"
        );
        let sort_order: i32 = row.try_get("sort_order")?;
        assert_eq!(sort_order, 0, "migrated items keep sort_order 0");
        let item_enabled: bool = row.try_get("item_enabled")?;
        assert!(item_enabled, "migrated items are enabled");
    }

    let global = row_for(&rows, ASSIGNMENT_GLOBAL)?;
    assert_eq!(field_str(global, "profile_name")?, "Default node exporter");
    assert_eq!(field_str(global, "scope_type")?, "global");
    assert_eq!(field_i32(global, "weight")?, 0);
    assert!(field_bool(global, "enabled")?);
    assert!(field_opt_str(global, "target")?.is_none());

    let role = row_for(&rows, ASSIGNMENT_ROLE)?;
    assert_eq!(field_str(role, "profile_name")?, "Role nginx");
    assert_eq!(field_uuid(role, "scope_id")?, Uuid::parse_str(ROLE_ID)?);
    assert_eq!(field_i32(role, "weight")?, 5);
    assert_eq!(field_str(role, "target")?, "edge.example");
    assert_eq!(field_str(role, "job")?, "nginx");
    assert_eq!(field_str(role, "scheme")?, "https");
    assert_eq!(field_i32(role, "port")?, 9113);
    assert_eq!(field_str(role, "metrics_path")?, "/nginx");
    assert_eq!(
        field_json(role, "labels")?,
        serde_json::json!({"tier": "web"})
    );

    let host = row_for(&rows, ASSIGNMENT_HOST)?;
    assert_eq!(
        field_str(host, "profile_name")?,
        "Node exporter",
        "an empty legacy name falls back to the exporter name"
    );
    assert_eq!(field_str(host, "scope_type")?, "connection_target");
    assert_eq!(field_uuid(host, "scope_id")?, Uuid::parse_str(HOST_ID)?);
    assert_eq!(field_str(host, "job")?, "node-alt");
    assert_eq!(field_i32(host, "port")?, 9101);
    assert_eq!(field_str(host, "metrics_path")?, "/custom");
    assert_eq!(
        field_json(host, "labels")?,
        serde_json::json!({"team": "ops"})
    );

    let disabled = row_for(&rows, ASSIGNMENT_DISABLED)?;
    assert_eq!(
        field_str(disabled, "profile_name")?,
        "Postgres exporter",
        "a whitespace-only legacy name falls back to the exporter name"
    );
    assert!(!field_bool(disabled, "enabled")?, "disabled stays disabled");

    let target = row_for(&rows, ASSIGNMENT_TARGET)?;
    assert_eq!(field_str(target, "profile_name")?, "Site node");
    assert_eq!(field_str(target, "target")?, "metrics.example");
    assert_eq!(field_str(target, "scope_type")?, "site");
    assert_eq!(field_uuid(target, "scope_id")?, Uuid::parse_str(SITE_ID)?);

    // Converted profiles are single-item profiles.
    let multi_item: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM (
            SELECT p.id
            FROM monitoring_profile p
            JOIN monitoring_profile_assignment a ON a.profile_id = p.id
            LEFT JOIN monitoring_profile_item i ON i.profile_id = p.id
            GROUP BY p.id
            HAVING COUNT(i.id) <> 1
        ) mismatched
        "#,
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(
        multi_item, 0,
        "each migrated assignment has exactly one item"
    );
    Ok(())
}

async fn assert_seeded_profiles(pool: &PgPool) -> TestResult {
    let seeds: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT p.slug, COUNT(i.id)
        FROM monitoring_profile p
        LEFT JOIN monitoring_profile_item i ON i.profile_id = p.id
        WHERE p.slug IN (
            'linux-baseline', 'docker-host', 'web-nginx', 'postgres',
            'network-blackbox', 'k8s-node'
        )
        GROUP BY p.slug
        ORDER BY p.slug
        "#,
    )
    .fetch_all(pool)
    .await?;
    assert_eq!(
        seeds,
        vec![
            ("docker-host".to_string(), 2),
            ("k8s-node".to_string(), 2),
            // smartctl is absent from this fixture.
            ("linux-baseline".to_string(), 2),
            ("postgres".to_string(), 1),
            ("web-nginx".to_string(), 1),
        ],
        "only profiles whose exporters exist are seeded, with existing items only"
    );
    assert!(
        !seeds.iter().any(|(slug, _)| slug == "network-blackbox"),
        "blackbox-interface is absent, so network-blackbox must not be seeded"
    );

    let nginx_items: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT e.slug
        FROM monitoring_profile p
        JOIN monitoring_profile_item i ON i.profile_id = p.id
        JOIN monitoring_exporter e ON e.id = i.exporter_id
        WHERE p.slug = 'web-nginx'
        "#,
    )
    .fetch_all(pool)
    .await?;
    assert_eq!(nginx_items, vec!["nginx".to_string()]);
    Ok(())
}

async fn assert_delete_rules(pool: &PgPool) -> TestResult {
    use sqlx::Row;

    // Profile delete is restricted while an assignment references it.
    let assigned_profile: Uuid =
        sqlx::query_scalar("SELECT profile_id FROM monitoring_profile_assignment WHERE id = $1")
            .bind(Uuid::parse_str(ASSIGNMENT_GLOBAL)?)
            .fetch_one(pool)
            .await?;
    let restricted = sqlx::query("DELETE FROM monitoring_profile WHERE id = $1")
        .bind(assigned_profile)
        .execute(pool)
        .await;
    assert!(
        restricted.is_err_and(|error| error
            .as_database_error()
            .is_some_and(|database_error| database_error.code().as_deref() == Some("23001"))),
        "profile delete must be restricted while assigned"
    );

    // Exporter delete is restricted while a profile item references it.
    let restricted = sqlx::query("DELETE FROM monitoring_exporter WHERE id = $1")
        .bind(Uuid::parse_str(NODE_EXPORTER)?)
        .execute(pool)
        .await;
    assert!(
        restricted.is_err_and(|error| error
            .as_database_error()
            .is_some_and(|database_error| database_error.code().as_deref() == Some("23001"))),
        "exporter delete must be restricted while used by a profile item"
    );

    // Metric set / query source / alert source links are cleared, not blocked.
    let metric_set: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO monitoring_metric_set (name, slug)
        VALUES ('Migration linked set', 'migration-linked-set')
        RETURNING id
        "#,
    )
    .fetch_one(pool)
    .await?;
    let query_source: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO monitoring_query_source (name, slug, query_url, source_type)
        VALUES ('Migration source', 'migration-source', 'http://metrics.example/api/v1/query', 'prometheus')
        RETURNING id
        "#,
    )
    .fetch_one(pool)
    .await?;
    let alert_source: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO monitoring_alert_source (name, source_type, query_url)
        VALUES ('Migration alerts', 'alertmanager', 'http://alerts.example/api/v1/alerts')
        RETURNING id
        "#,
    )
    .fetch_one(pool)
    .await?;
    let linked_profile: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO monitoring_profile
            (name, slug, metric_set_id, query_source_id, alert_source_id)
        VALUES ('Linked profile', 'linked-profile', $1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(metric_set)
    .bind(query_source)
    .bind(alert_source)
    .fetch_one(pool)
    .await?;

    sqlx::query("DELETE FROM monitoring_metric_set WHERE id = $1")
        .bind(metric_set)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM monitoring_query_source WHERE id = $1")
        .bind(query_source)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM monitoring_alert_source WHERE id = $1")
        .bind(alert_source)
        .execute(pool)
        .await?;

    let links = sqlx::query(
        r#"
        SELECT metric_set_id, query_source_id, alert_source_id
        FROM monitoring_profile WHERE id = $1
        "#,
    )
    .bind(linked_profile)
    .fetch_one(pool)
    .await?;
    assert!(links.try_get::<Option<Uuid>, _>("metric_set_id")?.is_none());
    assert!(
        links
            .try_get::<Option<Uuid>, _>("query_source_id")?
            .is_none()
    );
    assert!(
        links
            .try_get::<Option<Uuid>, _>("alert_source_id")?
            .is_none()
    );

    // Once unassigned, deleting the profile removes its items.
    let profile_item: Uuid =
        sqlx::query_scalar("SELECT id FROM monitoring_profile_item WHERE profile_id = $1")
            .bind(assigned_profile)
            .fetch_one(pool)
            .await?;
    sqlx::query("DELETE FROM monitoring_profile_assignment WHERE id = $1")
        .bind(Uuid::parse_str(ASSIGNMENT_GLOBAL)?)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM monitoring_profile WHERE id = $1")
        .bind(assigned_profile)
        .execute(pool)
        .await?;
    let remaining_item: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM monitoring_profile_item WHERE id = $1")
            .bind(profile_item)
            .fetch_one(pool)
            .await?;
    assert_eq!(remaining_item, 0, "profile delete cascades its items");
    Ok(())
}

fn row_for<'a>(
    rows: &'a [sqlx::postgres::PgRow],
    id: &str,
) -> TestResult<&'a sqlx::postgres::PgRow> {
    use sqlx::Row;
    rows.iter()
        .find(|row| {
            row.try_get::<Uuid, _>("id")
                .is_ok_and(|value| value == Uuid::parse_str(id).unwrap_or(Uuid::nil()))
        })
        .ok_or_else(|| format!("missing converted assignment {id}").into())
}

fn field_str(row: &sqlx::postgres::PgRow, column: &str) -> TestResult<String> {
    use sqlx::Row;
    Ok(row.try_get::<String, _>(column)?)
}

fn field_i32(row: &sqlx::postgres::PgRow, column: &str) -> TestResult<i32> {
    use sqlx::Row;
    Ok(row.try_get::<i32, _>(column)?)
}

fn field_bool(row: &sqlx::postgres::PgRow, column: &str) -> TestResult<bool> {
    use sqlx::Row;
    Ok(row.try_get::<bool, _>(column)?)
}

fn field_opt_str(row: &sqlx::postgres::PgRow, column: &str) -> TestResult<Option<String>> {
    use sqlx::Row;
    Ok(row.try_get::<Option<String>, _>(column)?)
}

fn field_json(row: &sqlx::postgres::PgRow, column: &str) -> TestResult<serde_json::Value> {
    use sqlx::Row;
    Ok(row.try_get::<serde_json::Value, _>(column)?)
}

fn field_uuid(row: &sqlx::postgres::PgRow, column: &str) -> TestResult<Uuid> {
    use sqlx::Row;
    Ok(row.try_get::<Uuid, _>(column)?)
}

async fn table_exists(pool: &PgPool, table: &str) -> TestResult<bool> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await?;
    Ok(exists)
}
