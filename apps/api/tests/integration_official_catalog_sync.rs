//! DB-backed idempotency check for the official playbook catalog sync.
//!
//! Guards the upgrade path: on a database that already carries the system
//! inventory playbook (seeded historically or by a previous boot), re-running
//! `sync_official_catalog` must dedup the version by content sha and mint no
//! spurious `playbook_version` rows, while keeping the fixed identity wiring
//! (`…0247` playbook, satellites, current_version_id) intact.
//!
//! Requires PostgreSQL (e.g. `make run-up`). Run with:
//! `cargo test --test integration_official_catalog_sync -- --ignored`

mod common;

use std::path::PathBuf;

use common::build_test_db_pool;
use piping_backend::contexts::playbooks::infrastructure::official_catalog::{
    OfficialCatalog, SYSTEM_INVENTORY_PLAYBOOK_ID, load_official_catalog, sync_official_catalog,
};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

const SYSTEM_CONTENT_SHA256: &str =
    "b849fbaa475ab5166c7f2765fa07d2afc03e9cef06865de41ca24347e8ce28dd";

fn repo_catalog_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks")
}

async fn version_count(
    db: &sqlx::PgPool,
    playbook_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query(
            "SELECT COUNT(*)::BIGINT AS count FROM playbook_version WHERE playbook_id = $1",
        )
        .bind(playbook_id)
        .fetch_one(db)
        .await
        .map_err(|e| format!("count playbook versions: {e}"))?
        .try_get("count")
        .map_err(|e| format!("count column: {e}"))?,
    )
}

async fn assert_assignment_metadata_survives_resync(
    db: &sqlx::PgPool,
    catalog: &OfficialCatalog,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixed_id = Uuid::parse_str("00000000-0000-7000-8000-000000000547")?;
    let custom_id = Uuid::now_v7();
    let original_metadata: Value =
        sqlx::query_scalar("SELECT default_metadata FROM playbook_assignment WHERE id = $1")
            .bind(fixed_id)
            .fetch_one(db)
            .await?;
    let customer_metadata = json!({
        "customer_root": {"nested": ["keep", {"enabled": false}]},
        "launch_options": {
            "pip117_customer_option": {"nested": {"values": [1, 2, 3]}},
            "forks": -1
        }
    });
    sqlx::query("UPDATE playbook_assignment SET default_metadata = $2 WHERE id = $1")
        .bind(fixed_id)
        .bind(&customer_metadata)
        .execute(db)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_assignment (
            id, playbook_id, target_set_id, default_version_id, name,
            scope_kind, priority, enabled, visible, executable,
            quick_action_enabled, default_input, default_metadata, metadata
        )
        SELECT $2, playbook_id, target_set_id, default_version_id,
               'Customer inventory assignment', 'global', 17, false, false,
               false, true, '{"customer_input": "keep"}'::jsonb, $3,
               '{"customer_assignment": {"nested": true}}'::jsonb
        FROM playbook_assignment WHERE id = $1
        "#,
    )
    .bind(fixed_id)
    .bind(custom_id)
    .bind(&customer_metadata)
    .execute(db)
    .await?;

    let manifest_options = &catalog
        .entries
        .iter()
        .find(|entry| entry.manifest.system)
        .ok_or("system catalog entry missing")?
        .manifest
        .launch_options;
    let mut expected_metadata = customer_metadata;
    expected_metadata["launch_options"]
        .as_object_mut()
        .ok_or("customer launch options must be an object")?
        .extend(
            manifest_options
                .as_object()
                .ok_or("manifest launch options must be an object")?
                .clone(),
        );
    let mut original_rows = Vec::new();
    for id in [fixed_id, custom_id] {
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(a) - 'updated_at' - 'default_metadata' FROM playbook_assignment a WHERE id = $1",
        )
        .bind(id)
        .fetch_one(db)
        .await?;
        original_rows.push((id, row));
    }
    for _ in 0..2 {
        sync_official_catalog(db, catalog).await?;
        for (id, original) in &original_rows {
            let row = sqlx::query(
                "SELECT default_metadata, to_jsonb(a) - 'updated_at' - 'default_metadata' AS attributes FROM playbook_assignment a WHERE id = $1",
            )
            .bind(id)
            .fetch_one(db)
            .await?;
            assert_eq!(
                row.try_get::<Value, _>("default_metadata")?,
                expected_metadata,
                "assignment {id}: retain all customer keys and refresh manifest options"
            );
            assert_eq!(
                &row.try_get::<Value, _>("attributes")?,
                original,
                "assignment {id}: unrelated attributes must remain unchanged"
            );
        }
    }
    sqlx::query("DELETE FROM playbook_assignment WHERE id = $1")
        .bind(custom_id)
        .execute(db)
        .await?;
    sqlx::query("UPDATE playbook_assignment SET default_metadata = $2 WHERE id = $1")
        .bind(fixed_id)
        .bind(original_metadata)
        .execute(db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_official_catalog_sync -- --ignored
async fn resyncing_catalog_is_idempotent_and_keeps_fixed_identity()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let db = build_test_db_pool().await.expect("database pool");
    // Self-sufficient on a fresh database: apply the baseline first (no-op on
    // an already-migrated stack), so this test also proves the fresh-install
    // path — migrate, then catalog-sync from the repository files.
    piping_backend::storage::database::migrations::run_migrations(&db)
        .await
        .expect("run migrations");
    let catalog = load_official_catalog(&repo_catalog_dir()).expect("load repo catalog");
    let system_id = Uuid::parse_str(SYSTEM_INVENTORY_PLAYBOOK_ID).expect("system uuid");

    let first = sync_official_catalog(&db, &catalog)
        .await
        .expect("first sync");
    let after_first = version_count(&db, system_id).await?;

    let second = sync_official_catalog(&db, &catalog)
        .await
        .expect("second sync");
    let after_second = version_count(&db, system_id).await?;

    // The dedup predicate must match the existing row: no version growth
    // between two identical syncs, and both report the same version id.
    assert_eq!(
        after_first, after_second,
        "re-running the catalog sync must not mint new playbook versions"
    );
    let first_system = first
        .iter()
        .find(|synced| synced.playbook_id == system_id)
        .expect("first sync returns the system playbook");
    let second_system = second
        .iter()
        .find(|synced| synced.playbook_id == system_id)
        .expect("second sync returns the system playbook");
    assert_eq!(first_system.version_id, second_system.version_id);
    assert!(first_system.system);

    // Fixed identity wiring survives the sync.
    let playbook =
        sqlx::query("SELECT source_kind, inventory_rest_enabled, current_version_id, default_target_set_id FROM playbook WHERE id = $1")
            .bind(system_id)
            .fetch_one(&db)
            .await
            .expect("system playbook row");
    assert_eq!(
        playbook
            .try_get::<String, _>("source_kind")
            .expect("source_kind"),
        "system"
    );
    assert!(
        playbook
            .try_get::<bool, _>("inventory_rest_enabled")
            .expect("inventory_rest_enabled")
    );
    assert_eq!(
        playbook
            .try_get::<Option<Uuid>, _>("current_version_id")
            .expect("current_version_id"),
        Some(first_system.version_id)
    );
    assert_eq!(
        playbook
            .try_get::<Option<Uuid>, _>("default_target_set_id")
            .expect("default_target_set_id")
            .map(|id| id.to_string()),
        Some("00000000-0000-7000-8000-000000000147".to_string())
    );

    // The current version carries the byte-locked catalog content sha.
    let sha: String = sqlx::query("SELECT content_sha256 FROM playbook_version WHERE id = $1")
        .bind(first_system.version_id)
        .fetch_one(&db)
        .await
        .expect("current version row")
        .try_get("content_sha256")
        .expect("content_sha256");
    assert_eq!(sha, SYSTEM_CONTENT_SHA256);

    // Satellites keep their fixed ids.
    for (table, id) in [
        (
            "playbook_service_principal",
            "00000000-0000-7000-8000-000000000047",
        ),
        (
            "playbook_target_set",
            "00000000-0000-7000-8000-000000000147",
        ),
        (
            "playbook_assignment",
            "00000000-0000-7000-8000-000000000547",
        ),
        ("playbook_schedule", "00000000-0000-7000-8000-000000000447"),
    ] {
        let exists: bool = sqlx::query(&format!(
            "SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1) AS found"
        ))
        .bind(Uuid::parse_str(id).expect("fixed uuid"))
        .fetch_one(&db)
        .await
        .expect("satellite query")
        .try_get("found")
        .expect("found column");
        assert!(exists, "{table} must keep its fixed row {id}");
    }
    assert_assignment_metadata_survives_resync(&db, &catalog).await?;
    Ok(())
}
