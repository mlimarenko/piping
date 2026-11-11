use piping_backend::contexts::inventory::application::{
    BackupService, FullBackupImportRequest, FullBackupSection, FullBackupSelection,
};
use serde_json::json;
use uuid::Uuid;

use crate::support::{
    TestError, canonical_database_bytes, migrated_pool, seed_standalone_tenant, suite_guard,
};

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; full restore is destructive and must run serially"]
async fn full_backup_preserves_scope_and_rejects_incompatible_archive_atomically()
-> Result<(), TestError> {
    let _guard = suite_guard().await;
    let pool = migrated_pool().await?;
    let service = BackupService::new(pool.clone());
    let original = service.export_full_backup(true, None).await?;
    let tag = Uuid::now_v7();
    let unused_tag = Uuid::now_v7();
    let tenant = seed_standalone_tenant(&pool, "tag-scope").await?;
    let slug = format!("backup-scope-{tag}");
    sqlx::query("INSERT INTO inventory_tag(id,name,slug,color,description,weight) VALUES($1,$2,$2,'abcdef','scope archive',0)")
        .bind(tag).bind(slug).execute(&pool).await?;
    sqlx::query("INSERT INTO inventory_tag_object_type(tag_id,content_type_id) SELECT $1,id FROM inventory_content_type WHERE (app_label,model) IN (('dcim','device'),('tenancy','tenant'))")
        .bind(tag).execute(&pool).await?;
    sqlx::query("UPDATE tenancy_tenant SET tags=$2 WHERE id=$1")
        .bind(tenant)
        .bind(vec![tag.to_string()])
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO inventory_tag(id,name,slug,color,description,weight) VALUES($1,$2,$2,'abcdef','unused scope archive',0)")
        .bind(unused_tag).bind(format!("backup-unused-scope-{unused_tag}")).execute(&pool).await?;
    sqlx::query("INSERT INTO inventory_tag_object_type(tag_id,content_type_id) SELECT $1,id FROM inventory_content_type WHERE app_label='dcim' AND model='site'")
        .bind(unused_tag).execute(&pool).await?;

    let selected = service
        .export_full_backup(
            false,
            Some(FullBackupSelection {
                sections: vec![FullBackupSection::Inventory],
                tables: vec![],
            }),
        )
        .await?;
    for name in ["inventory_tag", "inventory_tag_object_type"] {
        assert!(
            selected.tables.iter().any(|table| table.name == name),
            "Inventory selection omitted {name}"
        );
    }
    let archive = service.export_full_backup(true, None).await?;
    let scope_table = archive
        .tables
        .iter()
        .find(|table| table.name == "inventory_tag_object_type")
        .ok_or("scope table missing")?;
    assert_eq!(scope_table.section, FullBackupSection::Inventory);
    assert_eq!(
        scope_table
            .rows
            .iter()
            .filter(|row| row["tag_id"] == json!(tag))
            .count(),
        2
    );
    sqlx::query("UPDATE tenancy_tenant SET tags='{}' WHERE id=$1")
        .bind(tenant)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM inventory_tag_object_type WHERE tag_id=$1")
        .bind(tag)
        .execute(&pool)
        .await?;
    let request = |payload| FullBackupImportRequest {
        payload,
        confirm_full_restore: true,
        dry_run: false,
        selection: None,
        rules: None,
    };
    service.import_full_backup(request(archive.clone())).await?;
    let restored: Vec<String> = sqlx::query_scalar("SELECT c.app_label || '.' || c.model FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id WHERE s.tag_id=$1 ORDER BY 1")
        .bind(tag).fetch_all(&pool).await?;
    assert_eq!(restored, vec!["dcim.device", "tenancy.tenant"]);
    let memberships: Vec<String> =
        sqlx::query_scalar("SELECT tags FROM tenancy_tenant WHERE id=$1")
            .bind(tenant)
            .fetch_one(&pool)
            .await?;
    assert_eq!(memberships, vec![tag.to_string()]);

    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    let site_type: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='site'",
    )
    .fetch_one(&pool)
    .await?;
    let tenant_type: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='tenancy' AND model='tenant'",
    )
    .fetch_one(&pool)
    .await?;
    let mut incompatible = archive.clone();
    let scopes = incompatible
        .tables
        .iter_mut()
        .find(|table| table.name == "inventory_tag_object_type")
        .ok_or("scope table missing")?;
    let row = scopes
        .rows
        .iter_mut()
        .find(|row| row["tag_id"] == json!(tag) && row["content_type_id"] == json!(tenant_type))
        .ok_or("tenant scope missing")?;
    row["content_type_id"] = json!(site_type);
    assert!(
        service
            .import_full_backup(request(incompatible))
            .await
            .is_err(),
        "a scoped archive cannot attach its Tag to an incompatible restored owner"
    );
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert!(
        before == after,
        "invalid scope replay must roll back the entire restore"
    );

    let mut unknown_type = archive;
    let content_types = unknown_type
        .tables
        .iter_mut()
        .find(|table| table.name == "inventory_content_type")
        .ok_or("content-type catalogue missing")?;
    let unknown_id = if let Some(existing) = content_types
        .rows
        .iter()
        .find(|row| row["app_label"] == "dcim" && row["model"] == "manufacturer")
    {
        existing["id"].clone()
    } else {
        let mut row = content_types
            .rows
            .iter()
            .find(|row| row["id"] == json!(site_type))
            .ok_or("site content type missing")?
            .clone();
        let id = json!(Uuid::now_v7());
        row["id"] = id.clone();
        row["model"] = json!("manufacturer");
        content_types.rows.push(row);
        unknown_type.row_count += 1;
        id
    };
    let scopes = unknown_type
        .tables
        .iter_mut()
        .find(|table| table.name == "inventory_tag_object_type")
        .ok_or("scope table missing")?;
    let row = scopes
        .rows
        .iter_mut()
        .find(|row| row["tag_id"] == json!(unused_tag))
        .ok_or("unused Tag scope missing")?;
    row["content_type_id"] = unknown_id;
    assert!(
        service
            .import_full_backup(request(unknown_type))
            .await
            .is_err(),
        "archive-supplied catalogue rows cannot mint an unregistered Tag scope, even for an unused Tag and a real taggable table"
    );
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert!(
        before == after,
        "invalid scope registry replay must roll back the entire restore"
    );
    service.import_full_backup(request(original)).await?;
    Ok(())
}
