//! PostgreSQL proof of the current 0001 -> 0002 upgrade, not a historical chain.
use sqlx::{Connection, PgConnection, PgPool, postgres::PgConnectOptions};
use uuid::Uuid;

const BASELINE: &str = include_str!("../migrations/0001_init.sql");
const FORWARD: &str = include_str!("../migrations/0002_interface_networking.sql");

async fn database() -> Result<PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let options: PgConnectOptions = std::env::var("PIPING_DATABASE__URL")?.parse()?;
    let mut admin = PgConnection::connect_with(&options).await?;
    let name = format!("pwa90_{}", Uuid::now_v7().simple());
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE DATABASE ");
    query.push(&name);
    query.build().execute(&mut admin).await?;
    Ok(PgPool::connect_with(options.database(&name)).await?)
}

#[tokio::test]
#[ignore = "requires an exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn interface_networking_preserves_current_baseline_rows_and_refuses_invalid_upgrade()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = database().await?;
    sqlx::raw_sql(BASELINE).execute(&pool).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dcim_interface (id, custom_field_data, name, label, description,
         mark_connected, enabled, _name, type, mgmt_only, mtu, mode, speed, duplex,
         poe_mode, poe_type, rf_role, rf_channel, rf_channel_frequency, rf_channel_width,
         tx_power, wwn, channel_id, channels, cable_connector, cable_end, cable_positions)
         VALUES ($1, '{\"sentinel\":true}', 'preserve', 'label', 'existing row', false,
         true, 'preserve', 'virtual', false, 1500, 'tagged', 1000000, 'full', 'pse',
         'type1-ieee802.3af', 'ap', '5g-36-5180-20', 5180.125, 20.125, -40,
         '00:11:22:33:44:55:66:77', 1, 1024, 256, 'A', ARRAY[1,1024]::smallint[])",
    )
    .bind(id)
    .execute(&pool)
    .await?;
    let snapshot: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM dcim_interface i WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;

    // Baseline accepts MTU 0; the stricter upstream model must refuse it without
    // changing either stored values or the installed constraint on failure.
    sqlx::query("UPDATE dcim_interface SET mtu = 0 WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    assert!(sqlx::raw_sql(FORWARD).execute(&mut *tx).await.is_err());
    tx.rollback().await?;
    let mtu: i32 = sqlx::query_scalar("SELECT mtu FROM dcim_interface WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(mtu, 0, "failed upgrade must not rewrite data");
    let new_constraint: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM pg_constraint WHERE conname = 'dcim_interface_rf_channel_check')")
        .fetch_one(&pool).await?;
    assert!(
        !new_constraint,
        "failed upgrade must not leave partial constraints"
    );
    sqlx::query("UPDATE dcim_interface SET mtu = 1500 WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(FORWARD).execute(&mut *tx).await?;
    tx.commit().await?;
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM dcim_interface i WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        after, snapshot,
        "all pre-existing columns and values must survive"
    );
    for assignment in [
        "mtu = 65537",
        "mode = 'trunk'",
        "speed = -1",
        "duplex = 'bogus'",
        "poe_mode = 'bogus'",
        "poe_type = 'bogus'",
        "rf_role = 'bogus'",
        "rf_channel = '5g-999-1-20'",
        "rf_channel_frequency = 'NaN'",
        "rf_channel_width = 'NaN'",
        "tx_power = 128",
        "wwn = 'bad'",
        "channel_id = 0",
        "channels = 1025",
        "cable_connector = 257",
        "cable_end = 'C'",
        "cable_positions = ARRAY[0]::smallint[]",
        "cable_positions = ARRAY[NULL]::smallint[]",
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE dcim_interface SET ");
        query.push(assignment).push(" WHERE id = $1");
        let error = query
            .build()
            .bind(id)
            .execute(&pool)
            .await
            .expect_err(assignment);
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{assignment}: {error}"
        );
    }
    let fresh = database().await?;
    sqlx::raw_sql(BASELINE).execute(&fresh).await?;
    sqlx::raw_sql(FORWARD).execute(&fresh).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_interface")
        .fetch_one(&fresh)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}
