use std::collections::HashMap;

use piping_backend::contexts::inventory::application::backup_service::FullBackupArchive;
use piping_backend::contexts::inventory::application::{
    BackupService, FullBackupImportAction, FullBackupImportRequest, FullBackupImportRules,
    FullBackupSelection,
};
use uuid::Uuid;

use crate::support::{
    TestError, archive_table_counts, archive_table_names, canonical_database_bytes,
    cleanup_fixture, cleanup_tenants, expected_backup_table_names, migrated_pool,
    seed_standalone_tenant, seed_tenant_fixture, suite_guard,
};

fn full_import_request(
    payload: FullBackupArchive,
    confirm_full_restore: bool,
    dry_run: bool,
) -> FullBackupImportRequest {
    FullBackupImportRequest {
        payload,
        confirm_full_restore,
        dry_run,
        selection: None,
        rules: None,
    }
}

fn tenant_selection() -> FullBackupSelection {
    FullBackupSelection {
        sections: Vec::new(),
        tables: vec!["tenancy_tenant".to_string()],
    }
}

/// Mutation caught: omitting any base table from export, skipping post-truncate
/// inserts, or failing to restore a nullable foreign key after its parent row.
#[tokio::test]
#[ignore = "requires PostgreSQL; full restore is destructive and must run serially"]
async fn export_mutate_full_restore_recovers_all_counts_and_foreign_keys() -> Result<(), TestError>
{
    let _guard = suite_guard().await;
    let pool = migrated_pool().await?;
    let service = BackupService::new(pool.clone());
    let fixture = seed_tenant_fixture(&pool, "round-trip").await?;

    let archive = service.export_full_backup(true, None).await?;
    assert_eq!(
        archive_table_names(&archive),
        expected_backup_table_names(&pool).await?,
        "a full export must contain every non-internal base table"
    );
    let original_counts = archive_table_counts(&archive);

    sqlx::query("DELETE FROM tenancy_tenant WHERE id = $1")
        .bind(fixture.tenant_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM tenancy_tenantgroup WHERE id = $1")
        .bind(fixture.group_id)
        .execute(&pool)
        .await?;
    let mutation_id = seed_standalone_tenant(&pool, "round-trip-mutation").await?;

    let summary = service
        .import_full_backup(full_import_request(archive, true, false))
        .await?;
    assert!(summary.applied);

    let restored = service.export_full_backup(true, None).await?;
    assert_eq!(archive_table_counts(&restored), original_counts);
    let restored_group: Option<Uuid> =
        sqlx::query_scalar("SELECT group_id FROM tenancy_tenant WHERE id = $1")
            .bind(fixture.tenant_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(restored_group, Some(fixture.group_id));
    let mutation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tenancy_tenant WHERE id = $1")
            .bind(mutation_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        mutation_count, 0,
        "post-export rows must not survive full restore"
    );

    cleanup_fixture(&pool, fixture).await?;
    Ok(())
}

/// Mutation caught: allowing a dry run to fall through into TRUNCATE, DELETE,
/// upsert, deferred-update, or commit logic.
#[tokio::test]
#[ignore = "requires PostgreSQL; backup tests must run serially"]
async fn full_restore_dry_run_leaves_canonical_database_bytes_unchanged() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let pool = migrated_pool().await?;
    let service = BackupService::new(pool.clone());
    let fixture = seed_tenant_fixture(&pool, "dry-run").await?;
    let restore_point = service.export_full_backup(true, None).await?;

    sqlx::query("UPDATE tenancy_tenant SET description = 'mutated' WHERE id = $1")
        .bind(fixture.tenant_id)
        .execute(&pool)
        .await?;
    let extra_id = seed_standalone_tenant(&pool, "dry-run-extra").await?;
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;

    let summary = service
        .import_full_backup(full_import_request(restore_point, false, true))
        .await?;
    assert!(!summary.applied);
    assert!(summary.plan.is_some());
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert_eq!(after, before, "dry run changed persisted database bytes");

    cleanup_tenants(&pool, &[extra_id]).await?;
    cleanup_fixture(&pool, fixture).await?;
    Ok(())
}

/// Mutation caught: moving or removing the confirmation guard so destructive
/// restore work starts before an unconfirmed request is rejected.
#[tokio::test]
#[ignore = "requires PostgreSQL; backup tests must run serially"]
async fn missing_confirmation_fails_before_any_database_mutation() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let pool = migrated_pool().await?;
    let service = BackupService::new(pool.clone());
    let fixture = seed_tenant_fixture(&pool, "confirmation").await?;
    let archive = service.export_full_backup(true, None).await?;
    let before = canonical_database_bytes(&archive)?;

    let error = service
        .import_full_backup(full_import_request(archive, false, false))
        .await
        .expect_err("restore without explicit confirmation must fail");
    assert!(error.to_string().contains("explicitly confirmed"));
    let present: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenancy_tenant WHERE id = $1")
        .bind(fixture.tenant_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(present, 1, "confirmation rejection must precede mutation");
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert_eq!(after, before);

    cleanup_fixture(&pool, fixture).await?;
    Ok(())
}

/// Mutation caught: inverting the NOT EXISTS membership predicate in
/// `delete_missing_rows`, which would delete archived rows and retain absentees.
#[tokio::test]
#[ignore = "requires PostgreSQL; backup tests must run serially"]
async fn selective_delete_removes_only_rows_absent_from_the_archive() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let pool = migrated_pool().await?;
    let service = BackupService::new(pool.clone());
    let retained_a = seed_standalone_tenant(&pool, "selective-retained-a").await?;
    let retained_b = seed_standalone_tenant(&pool, "selective-retained-b").await?;
    let selection = tenant_selection();
    let archive = service
        .export_full_backup(false, Some(selection.clone()))
        .await?;
    let absent = seed_standalone_tenant(&pool, "selective-absent").await?;

    let rules = FullBackupImportRules {
        default_action: FullBackupImportAction::Replace,
        sections: HashMap::new(),
        tables: HashMap::from([("tenancy_tenant".to_string(), FullBackupImportAction::Delete)]),
        table_policies: HashMap::new(),
    };
    service
        .import_full_backup(FullBackupImportRequest {
            payload: archive,
            confirm_full_restore: true,
            dry_run: false,
            selection: Some(selection),
            rules: Some(rules),
        })
        .await?;

    let rows = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tenancy_tenant WHERE id = ANY($1) ORDER BY id",
    )
    .bind(vec![retained_a, retained_b, absent])
    .fetch_all(&pool)
    .await?;
    assert!(rows.contains(&retained_a));
    assert!(rows.contains(&retained_b));
    assert!(!rows.contains(&absent));

    cleanup_tenants(&pool, &[retained_a, retained_b]).await?;
    Ok(())
}

/// Mutation caught: executing TRUNCATE/inserts outside one transaction or
/// committing partial work when a later table fails to restore.
#[tokio::test]
#[ignore = "requires PostgreSQL; full restore is destructive and must run serially"]
async fn error_after_restore_has_started_rolls_the_whole_transaction_back() -> Result<(), TestError>
{
    let _guard = suite_guard().await;
    let pool = migrated_pool().await?;
    let service = BackupService::new(pool.clone());
    let fixture = seed_tenant_fixture(&pool, "rollback").await?;
    let mut archive = service.export_full_backup(true, None).await?;
    let before = canonical_database_bytes(&archive)?;

    let table_index = archive
        .tables
        .iter()
        .position(|table| table.name == "tenancy_tenant")
        .ok_or("full archive omitted tenancy_tenant")?;
    assert!(
        archive.tables[..table_index]
            .iter()
            .any(|table| !table.rows.is_empty()),
        "the injected error must occur after at least one insert"
    );
    let duplicate = archive.tables[table_index].rows[0].clone();
    archive.tables[table_index].rows.push(duplicate);
    archive.row_count += 1;

    let error = service
        .import_full_backup(full_import_request(archive, true, false))
        .await
        .expect_err("duplicate primary key must fail after restore starts");
    assert!(
        error.to_string().contains("tenancy_tenant"),
        "failure should identify the injected table"
    );
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert_eq!(
        after, before,
        "failed restore left partial transaction state"
    );

    cleanup_fixture(&pool, fixture).await?;
    Ok(())
}
