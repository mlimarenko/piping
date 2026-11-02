use piping_backend::contexts::inventory::application::{BackupService, FullBackupImportRequest};
use serde_json::json;
use uuid::Uuid;

use crate::support::{TestError, canonical_database_bytes, migrated_pool, suite_guard};

async fn seed_selective_installation(db: &sqlx::PgPool) -> Result<(Uuid, Uuid, Uuid), TestError> {
    let site = Uuid::now_v7();
    let device = Uuid::now_v7();
    let module_type = Uuid::now_v7();
    let bay = Uuid::now_v7();
    let module = Uuid::now_v7();
    let name = format!("selective-install-{device}");
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(&name).execute(db).await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(device).bind(&name).bind(site).execute(db).await?;
    sqlx::query("INSERT INTO dcim_moduletype (id,manufacturer_id,model,part_number,description,comments,custom_field_data,module_count) SELECT $1,id,$2,'','','','{}',0 FROM dcim_manufacturer ORDER BY id LIMIT 1")
        .bind(module_type).bind(&name).execute(db).await?;
    sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'slot','','','',$2,'{}',0,1,2,1)")
        .bind(bay).bind(device).execute(db).await?;
    sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'','active','','','{}')")
        .bind(module).bind(device).bind(bay).bind(module_type).execute(db).await?;
    Ok((device, bay, module))
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; full device bay graph restore"]
async fn full_restore_retains_disabled_occupied_device_bay_atomically() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let db = migrated_pool().await?;
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let (parent, _, _) = seed_selective_installation(&db).await?;
    let (child, _, _) = seed_selective_installation(&db).await?;
    let bay = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_devicebay (id,name,label,description,device_id,installed_device_id,custom_field_data) VALUES ($1,'occupied','','',$2,$3,'{}')")
        .bind(bay).bind(parent).bind(child).execute(&db).await?;
    sqlx::query("UPDATE dcim_devicebay SET enabled=false WHERE id=$1")
        .bind(bay)
        .execute(&db)
        .await?;
    let archive = service.export_full_backup(true, None).await?;
    let before = canonical_database_bytes(&archive)?;
    let request = |payload| FullBackupImportRequest {
        payload,
        confirm_full_restore: true,
        dry_run: false,
        selection: None,
        rules: None,
    };
    service.import_full_backup(request(archive.clone())).await?;
    let restored: (bool, Option<Uuid>) =
        sqlx::query_as("SELECT enabled,installed_device_id FROM dcim_devicebay WHERE id=$1")
            .bind(bay)
            .fetch_one(&db)
            .await?;
    assert_eq!(restored, (false, Some(child)));
    assert_eq!(
        before,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?
    );
    let error = sqlx::query("UPDATE dcim_devicebay SET installed_device_id=$2 WHERE id=$1")
        .bind(bay)
        .bind(Uuid::now_v7())
        .execute(&db)
        .await
        .expect_err("native guard must reject replacement in a disabled bay");
    assert!(
        error
            .to_string()
            .contains("Cannot install a device in a disabled device bay")
    );

    let mut invalid = archive;
    let row = invalid
        .tables
        .iter_mut()
        .find(|table| table.name == "dcim_devicebay")
        .ok_or("missing device bays")?
        .rows
        .iter_mut()
        .find(|row| row["id"] == json!(bay))
        .ok_or("missing occupied bay")?;
    row["installed_device_id"] = json!(parent);
    service
        .import_full_backup(request(invalid))
        .await
        .expect_err("self-installation must fail during staged replay");
    assert_eq!(
        before,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?,
        "failed staged restore must roll back all tables"
    );
    service.import_full_backup(request(original)).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; selective device bay graph restore"]
async fn selective_device_bay_inserts_stage_without_overriding_existing_policy()
-> Result<(), TestError> {
    use piping_backend::contexts::inventory::application::FullBackupSelection;
    let _guard = suite_guard().await;
    let db = migrated_pool().await?;
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let (parent, _, _) = seed_selective_installation(&db).await?;
    let (child, _, _) = seed_selective_installation(&db).await?;
    let (other_child, _, _) = seed_selective_installation(&db).await?;
    let (destination_child, _, _) = seed_selective_installation(&db).await?;
    let missing = Uuid::now_v7();
    let existing = Uuid::now_v7();
    for (id, name, occupant) in [
        (missing, "missing", child),
        (existing, "existing", other_child),
    ] {
        sqlx::query("INSERT INTO dcim_devicebay (id,name,label,description,device_id,installed_device_id,custom_field_data) VALUES ($1,$2,'','archive',$3,$4,'{}')")
            .bind(id).bind(name).bind(parent).bind(occupant).execute(&db).await?;
        sqlx::query("UPDATE dcim_devicebay SET enabled=false WHERE id=$1")
            .bind(id)
            .execute(&db)
            .await?;
    }
    let archive = service.export_full_backup(true, None).await?;
    sqlx::query("DELETE FROM dcim_devicebay WHERE id=$1")
        .bind(missing)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_devicebay SET enabled=true,installed_device_id=NULL,description='destination' WHERE id=$1").bind(existing).execute(&db).await?;
    let selection = FullBackupSelection {
        sections: vec![],
        tables: vec!["dcim_devicebay".into()],
    };
    let request = |payload,
                   rules: serde_json::Value,
                   selection: FullBackupSelection|
     -> Result<_, serde_json::Error> {
        Ok(FullBackupImportRequest {
            payload,
            confirm_full_restore: true,
            dry_run: false,
            selection: Some(selection),
            rules: Some(serde_json::from_value(rules)?),
        })
    };
    let policy = json!({"tablePolicies":{"dcim_devicebay":{"preserveColumns":["enabled","installed_device_id"]}}});
    service
        .import_full_backup(request(archive.clone(), policy.clone(), selection.clone())?)
        .await?;
    let inserted: (bool, Option<Uuid>) =
        sqlx::query_as("SELECT enabled,installed_device_id FROM dcim_devicebay WHERE id=$1")
            .bind(missing)
            .fetch_one(&db)
            .await?;
    assert_eq!(inserted, (false, Some(child)));
    let kept: (bool, Option<Uuid>, String) = sqlx::query_as(
        "SELECT enabled,installed_device_id,description FROM dcim_devicebay WHERE id=$1",
    )
    .bind(existing)
    .fetch_one(&db)
    .await?;
    assert_eq!(kept, (true, None, "archive".into()));
    sqlx::query("UPDATE dcim_devicebay SET description='destination' WHERE id=$1")
        .bind(existing)
        .execute(&db)
        .await?;
    service
        .import_full_backup(request(
            archive.clone(),
            json!({"tablePolicies":{"dcim_devicebay":{"updateColumns":["description"]}}}),
            selection.clone(),
        )?)
        .await?;
    let kept: (bool, Option<Uuid>, String) = sqlx::query_as(
        "SELECT enabled,installed_device_id,description FROM dcim_devicebay WHERE id=$1",
    )
    .bind(existing)
    .fetch_one(&db)
    .await?;
    assert_eq!(kept, (true, None, "archive".into()));

    sqlx::query("UPDATE dcim_devicebay SET installed_device_id=$2 WHERE id=$1")
        .bind(existing)
        .bind(destination_child)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_devicebay SET enabled=false WHERE id=$1")
        .bind(existing)
        .execute(&db)
        .await?;
    let before_replacement =
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    service
        .import_full_backup(request(
            archive.clone(),
            json!({"tablePolicies":{"dcim_devicebay":{"updateColumns":["installed_device_id"]}}}),
            selection.clone(),
        )?)
        .await
        .expect_err("preserving disabled state must not bypass the native installation guard");
    assert_eq!(
        before_replacement,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?
    );
    service.import_full_backup(request(archive.clone(), json!({"tablePolicies":{"dcim_devicebay":{"updateColumns":["enabled","installed_device_id"]}}}), selection.clone())?).await?;
    let replaced: (bool, Option<Uuid>) =
        sqlx::query_as("SELECT enabled,installed_device_id FROM dcim_devicebay WHERE id=$1")
            .bind(existing)
            .fetch_one(&db)
            .await?;
    assert_eq!(replaced, (false, Some(other_child)));

    sqlx::query("DELETE FROM dcim_devicebay WHERE id=$1")
        .bind(missing)
        .execute(&db)
        .await?;
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    let mut invalid = archive.clone();
    let row = invalid
        .tables
        .iter_mut()
        .find(|table| table.name == "dcim_devicebay")
        .ok_or("missing device bays")?
        .rows
        .iter_mut()
        .find(|row| row["id"] == json!(missing))
        .ok_or("missing bay")?;
    row["installed_device_id"] = json!(parent);
    service
        .import_full_backup(request(invalid, policy, selection.clone())?)
        .await
        .expect_err("invalid staged installation must fail");
    assert_eq!(
        before,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?
    );

    let skip_selection = FullBackupSelection {
        sections: vec![],
        tables: vec!["dcim_devicebay".into(), "dcim_site".into()],
    };
    service
        .import_full_backup(request(
            archive,
            json!({"tables":{"dcim_devicebay":"skip"}}),
            skip_selection,
        )?)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_devicebay WHERE id=$1")
        .bind(missing)
        .fetch_one(&db)
        .await?;
    assert_eq!(count, 0);
    service
        .import_full_backup(FullBackupImportRequest {
            payload: original,
            confirm_full_restore: true,
            dry_run: false,
            selection: None,
            rules: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; INSERT references versus update-only selective policies"]
async fn selective_insert_deferred_references_ignore_update_only_policies() -> Result<(), TestError>
{
    use piping_backend::contexts::inventory::application::FullBackupSelection;
    let _guard = suite_guard().await;
    let db = migrated_pool().await?;
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let (device, _, module) = seed_selective_installation(&db).await?;
    let inserted = Uuid::now_v7();
    let existing = Uuid::now_v7();
    let outflow = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_coolingoutflow (id,name,label,device_id,module_id,description,custom_field_data) VALUES ($1,'out','',$2,$3,'','{}')")
        .bind(outflow).bind(device).bind(module).execute(&db).await?;
    for (id, name) in [(inserted, "inserted"), (existing, "existing")] {
        sqlx::query("INSERT INTO dcim_coolingintake (id,name,label,device_id,module_id,cooling_outflow_id,description,custom_field_data) VALUES ($1,$2,'',$3,$4,$5,'archive','{}')")
            .bind(id).bind(name).bind(device).bind(module).bind(outflow).execute(&db).await?;
    }
    let selection = FullBackupSelection {
        sections: vec![],
        tables: vec!["dcim_coolingintake".into(), "dcim_coolingoutflow".into()],
    };
    let archive = service
        .export_full_backup(true, Some(selection.clone()))
        .await?;
    let request = |payload, policy: serde_json::Value| -> Result<_, serde_json::Error> {
        Ok(FullBackupImportRequest {
            payload,
            confirm_full_restore: true,
            dry_run: false,
            selection: Some(selection.clone()),
            rules: Some(serde_json::from_value(json!({
                "tablePolicies": {"dcim_coolingintake": policy}
            }))?),
        })
    };
    for (policy, only_intake) in [
        (
            json!({"preserveColumns": ["module_id", "cooling_outflow_id"]}),
            true,
        ),
        (
            json!({"preserveColumns": ["module_id", "cooling_outflow_id"]}),
            false,
        ),
        (json!({"updateColumns": ["description"]}), false),
    ] {
        sqlx::query("DELETE FROM dcim_coolingintake WHERE id=$1")
            .bind(inserted)
            .execute(&db)
            .await?;
        sqlx::query("UPDATE dcim_coolingintake SET module_id=NULL,cooling_outflow_id=NULL,description='destination' WHERE id=$1")
            .bind(existing).execute(&db).await?;
        let mut import = request(archive.clone(), policy)?;
        if only_intake {
            import.selection = Some(FullBackupSelection {
                sections: vec![],
                tables: vec!["dcim_coolingintake".into()],
            });
        }
        service.import_full_backup(import).await?;
        let new_edges: (Option<Uuid>, Option<Uuid>, String) = sqlx::query_as(
            "SELECT module_id,cooling_outflow_id,description FROM dcim_coolingintake WHERE id=$1",
        )
        .bind(inserted)
        .fetch_one(&db)
        .await?;
        assert_eq!(new_edges, (Some(module), Some(outflow), "archive".into()));
        let old_edges: (Option<Uuid>, Option<Uuid>, String) = sqlx::query_as(
            "SELECT module_id,cooling_outflow_id,description FROM dcim_coolingintake WHERE id=$1",
        )
        .bind(existing)
        .fetch_one(&db)
        .await?;
        assert_eq!(old_edges, (None, None, "archive".into()));
    }
    // Without update restrictions, existing deferred references still update.
    service
        .import_full_backup(request(archive.clone(), json!({}))?)
        .await?;
    let old_edges: (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT module_id,cooling_outflow_id FROM dcim_coolingintake WHERE id=$1")
            .bind(existing)
            .fetch_one(&db)
            .await?;
    assert_eq!(old_edges, (Some(module), Some(outflow)));
    sqlx::query("DELETE FROM dcim_coolingintake WHERE id=$1")
        .bind(inserted)
        .execute(&db)
        .await?;
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    let mut malformed = archive.clone();
    let row = malformed
        .tables
        .iter_mut()
        .find(|table| table.name == "dcim_coolingintake")
        .ok_or("missing intake table")?
        .rows
        .iter_mut()
        .find(|row| row["id"] == json!(inserted))
        .ok_or("missing inserted intake")?;
    row["module_id"] = json!(Uuid::now_v7());
    let error = service
        .import_full_backup(request(
            malformed,
            json!({"preserveColumns": ["module_id"]}),
        )?)
        .await
        .expect_err("a missing inserted module reference must fail native FK validation");
    assert!(
        error
            .to_string()
            .contains("dcim_coolingintake_module_id_fkey"),
        "{error}"
    );
    assert_eq!(
        before,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?,
        "a missing deferred INSERT owner must fail and roll back, not be silently preserved as NULL"
    );
    // Ownership is operational: native uninstall must cascade the restored port.
    service
        .import_full_backup(request(archive, json!({}))?)
        .await?;
    sqlx::query("DELETE FROM dcim_module WHERE id=$1")
        .bind(module)
        .execute(&db)
        .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dcim_coolingintake WHERE id=ANY($1)")
            .bind(vec![existing, inserted])
            .fetch_one(&db)
            .await?;
    assert_eq!(remaining, 0);
    service
        .import_full_backup(FullBackupImportRequest {
            payload: original,
            confirm_full_restore: true,
            dry_run: false,
            selection: None,
            rules: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; existing installation dependencies and atomic policy-aware replay"]
async fn selective_existing_bay_dependencies_precede_missing_module_installation()
-> Result<(), TestError> {
    use piping_backend::contexts::inventory::application::FullBackupSelection;
    let _guard = suite_guard().await;
    let db = migrated_pool().await?;
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let (_, bay, module) = seed_selective_installation(&db).await?;
    let selection = FullBackupSelection {
        sections: vec![],
        tables: vec![
            "dcim_modulebay".into(),
            "dcim_module".into(),
            "dcim_modulebay_module_bay_types".into(),
        ],
    };
    let unrestricted = service
        .export_full_backup(true, Some(selection.clone()))
        .await?;
    let request = |payload, policies: serde_json::Value| -> Result<_, serde_json::Error> {
        Ok(FullBackupImportRequest {
            payload,
            confirm_full_restore: true,
            dry_run: false,
            selection: Some(selection.clone()),
            rules: Some(serde_json::from_value(json!({"tablePolicies": policies}))?),
        })
    };
    sqlx::query("DELETE FROM dcim_module WHERE id=$1")
        .bind(module)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_modulebay SET enabled=false WHERE id=$1")
        .bind(bay)
        .execute(&db)
        .await?;
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    for policy in [
        json!({"preserveColumns": ["enabled"]}),
        json!({"updateColumns": ["description"]}),
    ] {
        let error = service
            .import_full_backup(request(
                unrestricted.clone(),
                json!({"dcim_modulebay": policy}),
            )?)
            .await
            .expect_err("preserved disabled destination bay must reject a new installation");
        assert!(
            error
                .to_string()
                .contains("Install requires an enabled bay"),
            "{error}"
        );
        assert_eq!(
            before,
            canonical_database_bytes(&service.export_full_backup(true, None).await?)?
        );
    }
    // Default replay must enable this existing bay before INSERT's live guard.
    service
        .import_full_backup(request(unrestricted.clone(), json!({}))?)
        .await?;
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM dcim_modulebay WHERE id=$1")
        .bind(bay)
        .fetch_one(&db)
        .await?;
    assert!(enabled);
    let categories = [Uuid::now_v7(), Uuid::now_v7()];
    for category in categories {
        sqlx::query("INSERT INTO dcim_modulebaytype (id,name,slug,description,comments,custom_field_data) VALUES ($1,$2,$2,'','','{}')")
            .bind(category).bind(category.to_string()).execute(&db).await?;
    }
    let membership = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_modulebay_module_bay_types (id,modulebay_id,modulebaytype_id) VALUES ($1,$2,$3)")
        .bind(membership).bind(bay).bind(categories[0]).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_moduletype_module_bay_types (moduletype_id,modulebaytype_id) SELECT module_type_id,$2 FROM dcim_module WHERE id=$1")
        .bind(module).bind(categories[0]).execute(&db).await?;
    let compatible = service
        .export_full_backup(true, Some(selection.clone()))
        .await?;
    sqlx::query("DELETE FROM dcim_module WHERE id=$1")
        .bind(module)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_modulebay SET enabled=false WHERE id=$1")
        .bind(bay)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_modulebay_module_bay_types SET modulebaytype_id=$2 WHERE id=$1")
        .bind(membership)
        .bind(categories[1])
        .execute(&db)
        .await?;
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    let error = service
        .import_full_backup(request(
            compatible.clone(),
            json!({
                "dcim_modulebay_module_bay_types": {"preserveColumns": ["modulebaytype_id"]}
            }),
        )?)
        .await
        .expect_err("preserved incompatible category must still reject installation");
    assert!(
        error
            .to_string()
            .contains("compatibility categories do not intersect"),
        "{error}"
    );
    assert_eq!(
        before,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?
    );
    let mut cyclic = compatible.clone();
    let row = cyclic
        .tables
        .iter_mut()
        .find(|table| table.name == "dcim_modulebay")
        .ok_or("missing bay table")?
        .rows
        .iter_mut()
        .find(|row| row["id"] == json!(bay))
        .ok_or("missing bay")?;
    row["module_id"] = json!(module);
    row["parent_id"] = json!(bay);
    let error = service
        .import_full_backup(request(cyclic, json!({}))?)
        .await
        .expect_err("completed module containment cycle must be rejected");
    assert!(
        error
            .to_string()
            .contains("Module bay containment cannot contain cycles"),
        "{error}"
    );
    assert_eq!(
        before,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?,
        "completed-graph failure must roll back dependency preparation and module installation"
    );
    service
        .import_full_backup(request(compatible.clone(), json!({}))?)
        .await?;
    let installed: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dcim_module WHERE id=$1)")
            .bind(module)
            .fetch_one(&db)
            .await?;
    assert!(
        installed,
        "selected category replacement must precede native installation validation"
    );
    let category: Uuid = sqlx::query_scalar(
        "SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE id=$1",
    )
    .bind(membership)
    .fetch_one(&db)
    .await?;
    assert_eq!(category, categories[0]);
    // A stale compatible destination must not hide an archived restriction that
    // would reject this new occupant under the native installation guard.
    sqlx::query("DELETE FROM dcim_module WHERE id=$1")
        .bind(module)
        .execute(&db)
        .await?;
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    let mut incompatible = compatible;
    let row = incompatible
        .tables
        .iter_mut()
        .find(|table| table.name == "dcim_modulebay_module_bay_types")
        .ok_or("missing bay membership table")?
        .rows
        .iter_mut()
        .find(|row| row["id"] == json!(membership))
        .ok_or("missing bay membership")?;
    row["modulebaytype_id"] = json!(categories[1]);
    let error = service
        .import_full_backup(request(incompatible, json!({}))?)
        .await
        .expect_err("selected incompatible category must be checked before insertion");
    assert!(
        error
            .to_string()
            .contains("compatibility categories do not intersect"),
        "{error}"
    );
    assert_eq!(
        before,
        canonical_database_bytes(&service.export_full_backup(true, None).await?)?
    );
    service
        .import_full_backup(FullBackupImportRequest {
            payload: original,
            confirm_full_restore: true,
            dry_run: false,
            selection: None,
            rules: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; full archive conservation of valid non-UUID sibling bookkeeping"]
async fn full_archive_preserves_valid_archived_bay_bookkeeping_order() -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let db = migrated_pool().await?;
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let site = Uuid::now_v7();
    let device = Uuid::now_v7();
    let module_type = Uuid::now_v7();
    let module = Uuid::now_v7();
    let root = Uuid::now_v7();
    let mut children = [Uuid::now_v7(), Uuid::now_v7()];
    children.sort();
    let name = format!("archive-bay-order-{device}");
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(&name).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(device).bind(&name).bind(site).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_moduletype (id,manufacturer_id,model,part_number,description,comments,custom_field_data,module_count) SELECT $1,id,$2,'','','','{}',0 FROM dcim_manufacturer ORDER BY id LIMIT 1")
        .bind(module_type).bind(&name).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'root','','','',$2,'{}',0,1,2,1)")
        .bind(root).bind(device).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'','active','','','{}')")
        .bind(module).bind(device).bind(root).bind(module_type).execute(&db).await?;
    for (id, label) in [(children[0], "low-uuid"), (children[1], "high-uuid")] {
        sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,module_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,$2,'','','',$3,$4,'{}',0,1,2,1)")
            .bind(id).bind(label).bind(device).bind(module).execute(&db).await?;
    }
    // Change only stored bookkeeping, not containment or trigger configuration.
    // This valid tree visits the higher UUID child before the lower UUID child.
    sqlx::query("UPDATE dcim_modulebay SET level=0,lft=1,rght=6 WHERE id=$1")
        .bind(root)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_modulebay SET level=1,lft=4,rght=5 WHERE id=$1")
        .bind(children[0])
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_modulebay SET level=1,lft=2,rght=3 WHERE id=$1")
        .bind(children[1])
        .execute(&db)
        .await?;
    let archive = service.export_full_backup(true, None).await?;
    let before = canonical_database_bytes(&archive)?;
    service
        .import_full_backup(FullBackupImportRequest {
            payload: archive,
            confirm_full_restore: true,
            dry_run: false,
            selection: None,
            rules: None,
        })
        .await?;
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert!(
        before == after,
        "full restore must conserve valid archived bookkeeping instead of imposing UUID sibling order"
    );
    let projected: Vec<(Uuid, i32, i32, i32)> =
        sqlx::query_as("SELECT id,level,lft,rght FROM dcim_modulebay WHERE id=ANY($1) ORDER BY id")
            .bind(children.to_vec())
            .fetch_all(&db)
            .await?;
    assert_eq!(
        projected,
        vec![(children[0], 1, 4, 5), (children[1], 1, 2, 3)]
    );
    service
        .import_full_backup(FullBackupImportRequest {
            payload: original,
            confirm_full_restore: true,
            dry_run: false,
            selection: None,
            rules: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; real selective replay of existing cooling edges with a transient cycle"]
async fn selective_existing_cooling_edges_validate_the_final_graph_not_intermediate_upserts()
-> Result<(), TestError> {
    use piping_backend::contexts::inventory::application::FullBackupSelection;
    let _guard = suite_guard().await;
    let db = migrated_pool().await?;
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let site = Uuid::now_v7();
    let device = Uuid::now_v7();
    let i1 = Uuid::now_v7();
    let i2 = Uuid::now_v7();
    let o1 = Uuid::now_v7();
    let o2 = Uuid::now_v7();
    let name = format!("cooling-edge-replay-{device}");
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(&name).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(device).bind(&name).bind(site).execute(&db).await?;
    for (id, name) in [(i1, "i1"), (i2, "i2")] {
        sqlx::query("INSERT INTO dcim_coolingintake (id,name,label,device_id,description,custom_field_data) VALUES ($1,$2,'',$3,'','{}')")
            .bind(id).bind(name).bind(device).execute(&db).await?;
    }
    for (id, name) in [(o1, "o1"), (o2, "o2")] {
        sqlx::query("INSERT INTO dcim_coolingoutflow (id,name,label,device_id,description,custom_field_data) VALUES ($1,$2,'',$3,'','{}')")
            .bind(id).bind(name).bind(device).execute(&db).await?;
    }
    // Export the valid final graph: O1 -> I2 -> O2 -> I1 -> NULL.
    sqlx::query("UPDATE dcim_coolingoutflow SET cooling_intake_id=$2 WHERE id=$1")
        .bind(o1)
        .bind(i2)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_coolingintake SET cooling_outflow_id=$2 WHERE id=$1")
        .bind(i2)
        .bind(o2)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_coolingoutflow SET cooling_intake_id=$2 WHERE id=$1")
        .bind(o2)
        .bind(i1)
        .execute(&db)
        .await?;
    let selection = FullBackupSelection {
        sections: vec![],
        tables: vec!["dcim_coolingintake".into(), "dcim_coolingoutflow".into()],
    };
    let mut archive = service
        .export_full_backup(true, Some(selection.clone()))
        .await?;
    for table in &mut archive.tables {
        table.rows.retain(|row| row["device_id"] == json!(device));
    }
    archive.row_count = archive.tables.iter().map(|table| table.rows.len()).sum();
    // Restore the different valid current graph: I1 -> O1 -> I2 -> O2 -> NULL.
    sqlx::query("UPDATE dcim_coolingoutflow SET cooling_intake_id=NULL WHERE id=$1")
        .bind(o2)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE dcim_coolingintake SET cooling_outflow_id=$2 WHERE id=$1")
        .bind(i1)
        .bind(o1)
        .execute(&db)
        .await?;
    let request = |payload| FullBackupImportRequest {
        payload,
        confirm_full_restore: true,
        dry_run: false,
        selection: Some(selection.clone()),
        rules: None,
    };
    service.import_full_backup(request(archive.clone())).await?;
    let intake_edge: Option<Uuid> =
        sqlx::query_scalar("SELECT cooling_outflow_id FROM dcim_coolingintake WHERE id=$1")
            .bind(i1)
            .fetch_one(&db)
            .await?;
    let outflow_edge: Option<Uuid> =
        sqlx::query_scalar("SELECT cooling_intake_id FROM dcim_coolingoutflow WHERE id=$1")
            .bind(o2)
            .fetch_one(&db)
            .await?;
    assert_eq!(
        intake_edge, None,
        "explicit archived null must clear the OLD deferred edge"
    );
    assert_eq!(outflow_edge, Some(i1));
    let other_edges:(Option<Uuid>,Option<Uuid>)=sqlx::query_as("SELECT i.cooling_outflow_id,o.cooling_intake_id FROM dcim_coolingintake i CROSS JOIN dcim_coolingoutflow o WHERE i.id=$1 AND o.id=$2").bind(i2).bind(o1).fetch_one(&db).await?;
    assert_eq!(other_edges, (Some(o2), Some(i2)));
    // The same native edge mutation remains immediately invalid outside replay.
    assert!(
        sqlx::query("UPDATE dcim_coolingintake SET cooling_outflow_id=$2 WHERE id=$1")
            .bind(i1)
            .bind(o1)
            .execute(&db)
            .await
            .is_err()
    );
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    let mut invalid = archive;
    let row = invalid
        .tables
        .iter_mut()
        .find(|table| table.name == "dcim_coolingintake")
        .ok_or("missing intakes")?
        .rows
        .iter_mut()
        .find(|row| row["id"] == json!(i1))
        .ok_or("missing i1")?;
    row["cooling_outflow_id"] = json!(o1);
    let error = service
        .import_full_backup(request(invalid))
        .await
        .expect_err("a cyclic final archive must still be rejected");
    assert!(
        error
            .to_string()
            .contains("Cooling connections cannot contain cycles"),
        "{error}"
    );
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert!(
        before == after,
        "failed final cooling validation must roll back selective replay"
    );
    service
        .import_full_backup(FullBackupImportRequest {
            payload: original,
            confirm_full_restore: true,
            dry_run: false,
            selection: None,
            rules: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; canonical selective replay with explicit template-derived identities"]
async fn selective_explicit_hardware_graph_replay_preserves_ids_and_does_not_run_factory()
-> Result<(), TestError> {
    use piping_backend::contexts::inventory::application::FullBackupSelection;
    let _guard = suite_guard().await;
    let db = migrated_pool().await?;
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let site = Uuid::now_v7();
    let device_type = Uuid::now_v7();
    let module_type = Uuid::now_v7();
    let device = Uuid::now_v7();
    let module = Uuid::now_v7();
    let name = format!("selective-graph-{device}");
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(&name).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_devicetype SELECT (jsonb_populate_record(NULL::dcim_devicetype,to_jsonb(t)||jsonb_build_object('id',$1::uuid,'model',$2::text,'slug',$2::text,'external_id',NULL))).* FROM dcim_devicetype t ORDER BY t.id LIMIT 1")
        .bind(device_type).bind(&name).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_moduletype (id,manufacturer_id,model,part_number,description,comments,custom_field_data,module_count) SELECT $1,id,$2,'','','','{}',0 FROM dcim_manufacturer ORDER BY id LIMIT 1")
        .bind(module_type).bind(&name).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_modulebaytemplate (name,label,position,description,device_type_id) VALUES ('slot','','A','',$1)").bind(device_type).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_modulebaytemplate (name,label,position,description,module_type_id,enabled) VALUES ('child-{module}','','B','',$1,false)").bind(module_type).execute(&db).await?;
    for (column, id, port) in [
        ("device_type_id", device_type, "in"),
        ("module_type_id", module_type, "module-in"),
    ] {
        sqlx::query(&format!("INSERT INTO dcim_coolingintaketemplate (name,label,description,{column}) VALUES ($1,'','',$2)")).bind(port).bind(id).execute(&db).await?;
    }
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),$3,$4,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(device).bind(&name).bind(device_type).bind(site).execute(&db).await?;
    let root: Uuid = sqlx::query_scalar(
        "SELECT id FROM dcim_modulebay WHERE device_id=$1 AND module_id IS NULL",
    )
    .bind(device)
    .fetch_one(&db)
    .await?;
    sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'','active','','','{}')")
        .bind(module).bind(device).bind(root).bind(module_type).execute(&db).await?;
    let child: Uuid = sqlx::query_scalar("SELECT id FROM dcim_modulebay WHERE module_id=$1")
        .bind(module)
        .fetch_one(&db)
        .await?;
    sqlx::query("UPDATE dcim_modulebay SET enabled=false WHERE id=$1")
        .bind(root)
        .execute(&db)
        .await?;
    let selection = FullBackupSelection {
        sections: vec![],
        tables: vec![
            "dcim_device".into(),
            "dcim_module".into(),
            "dcim_modulebay".into(),
            "dcim_coolingintake".into(),
            "dcim_coolingoutflow".into(),
            "dcim_modulebay_module_bay_types".into(),
        ],
    };
    let mut archive = service
        .export_full_backup(true, Some(selection.clone()))
        .await?;
    for table in &mut archive.tables {
        table.rows.retain(|row| {
            row["id"] == json!(device)
                || row["device_id"] == json!(device)
                || row["modulebay_id"] == json!(root)
                || row["modulebay_id"] == json!(child)
        });
    }
    archive.row_count = archive.tables.iter().map(|table| table.rows.len()).sum();
    let expected_ports: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM dcim_coolingintake WHERE device_id=$1 ORDER BY id")
            .bind(device)
            .fetch_all(&db)
            .await?;
    sqlx::query("DELETE FROM dcim_modulebay WHERE id=$1")
        .bind(child)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM dcim_module WHERE id=$1")
        .bind(module)
        .execute(&db)
        .await?;
    // Raw SQL retains the native restrictive bay -> device FK. Remove the now
    // empty root explicitly after its child/occupant, before deleting Device.
    sqlx::query("DELETE FROM dcim_modulebay WHERE id=$1")
        .bind(root)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM dcim_device WHERE id=$1")
        .bind(device)
        .execute(&db)
        .await?;
    let request = |payload| FullBackupImportRequest {
        payload,
        confirm_full_restore: true,
        dry_run: false,
        selection: Some(selection.clone()),
        rules: None,
    };
    // First replay inserts explicit archived IDs. A second replay updates the
    // existing disabled occupant, not an INSERT guard disguised as an UPSERT.
    for _ in 0..2 {
        service.import_full_backup(request(archive.clone())).await?;
        let bays: Vec<(Uuid, bool)> =
            sqlx::query_as("SELECT id,enabled FROM dcim_modulebay WHERE device_id=$1 ORDER BY id")
                .bind(device)
                .fetch_all(&db)
                .await?;
        let mut expected = vec![(root, false), (child, false)];
        expected.sort_by_key(|row| row.0);
        assert_eq!(
            bays, expected,
            "no generated competing bay or temporary enabled state may commit"
        );
        let ports: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM dcim_coolingintake WHERE device_id=$1 ORDER BY id")
                .bind(device)
                .fetch_all(&db)
                .await?;
        assert_eq!(
            ports, expected_ports,
            "explicit cooling identities must be preserved without factory duplicates"
        );
    }
    let before = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    let mut malformed = archive.clone();
    let bay = malformed
        .tables
        .iter_mut()
        .find(|table| table.name == "dcim_modulebay")
        .ok_or("missing bay table")?
        .rows
        .iter_mut()
        .find(|row| row["id"] == json!(child))
        .ok_or("missing child")?;
    bay["parent_id"] = json!(Uuid::now_v7());
    let error = service
        .import_full_backup(request(malformed))
        .await
        .expect_err("invalid archived bay containment must be rejected");
    assert!(
        error
            .to_string()
            .contains("Archived module graph has invalid containment"),
        "{error}"
    );
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert!(
        before == after,
        "invalid selective graph replay must roll back all prior table changes"
    );
    service
        .import_full_backup(FullBackupImportRequest {
            payload: original,
            confirm_full_restore: true,
            dry_run: false,
            selection: None,
            rules: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL fixture; full restore truncates all application tables"]
async fn full_archive_preserves_nested_disabled_incompatible_occupants_and_rejects_invalid_graph()
-> Result<(), TestError> {
    module_graph_roundtrip(false).await
}

#[tokio::test]
#[ignore = "requires Main's integrated immutable 0005 + canonical Tag 0006 + cooling 0007 source cohort and isolated PostgreSQL"]
async fn full_archive_restores_canonical_tag_catalog_before_nested_module_memberships()
-> Result<(), TestError> {
    module_graph_roundtrip(true).await
}

async fn module_graph_roundtrip(require_canonical_tags: bool) -> Result<(), TestError> {
    let _guard = suite_guard().await;
    let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let source = sqlx::migrate::Migrator::new(source_path.as_path()).await?;
    if require_canonical_tags {
        let migration=source.iter().find(|migration|migration.version==6).ok_or("Integrated Tag acceptance requires genuine source migration 0006; no fallback is permitted")?;
        let checksum = migration
            .checksum
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            checksum,
            "433906473689b082c2ea627cd5dd0b0a66b90d9c8c25378b2b89f5af4a20522d9506bc60e1e8edb5227faf7ed37c5859",
            "Tag migration must match Main's immutable canonical source"
        );
    }
    let db = migrated_pool().await?;
    if require_canonical_tags {
        for version in [5, 6, 7] {
            let migration = source
                .iter()
                .find(|migration| migration.version == version)
                .ok_or("Integrated restore source cohort is incomplete")?;
            let (success, checksum): (bool, Vec<u8>) =
                sqlx::query_as("SELECT success,checksum FROM _sqlx_migrations WHERE version=$1")
                    .bind(version)
                    .fetch_one(&db)
                    .await?;
            assert!(
                success && checksum.as_slice() == migration.checksum.as_ref(),
                "Applied migration must match the integrated source cohort"
            );
        }
        let guards:bool=sqlx::query_scalar("SELECT to_regprocedure('inventory_validate_tag_ids(text[])') IS NOT NULL AND EXISTS(SELECT 1 FROM pg_trigger t JOIN pg_proc p ON p.oid=t.tgfoid WHERE t.tgrelid='inventory_tag'::regclass AND NOT t.tgisinternal AND p.proname='inventory_tag_catalog_lock')").fetch_one(&db).await?;
        assert!(
            guards,
            "Tag restore acceptance requires the genuine catalog and membership guards"
        );
    }
    let service = BackupService::new(db.clone());
    let original = service.export_full_backup(true, None).await?;
    let site = Uuid::now_v7();
    let device = Uuid::now_v7();
    let module_type = Uuid::now_v7();
    let root = Uuid::now_v7();
    let child = Uuid::now_v7();
    let module = Uuid::now_v7();
    let a = Uuid::now_v7();
    let b = Uuid::now_v7();
    let tag = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_tag (id,name,slug) VALUES ($1,$2,$2)")
        .bind(tag)
        .bind(format!("restore-tag-{tag}"))
        .execute(&db)
        .await?;
    let name = format!("restore-graph-{device}");
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(&name).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(device).bind(&name).bind(site).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_moduletype (id,manufacturer_id,model,part_number,description,comments,custom_field_data,module_count) SELECT $1,id,$2,'','','','{}',0 FROM dcim_manufacturer ORDER BY id LIMIT 1")
        .bind(module_type).bind(&name).execute(&db).await?;
    for category in [a, b] {
        sqlx::query("INSERT INTO dcim_modulebaytype (id,name,slug,description,comments,custom_field_data) VALUES ($1,$2,$2,'','','{}')")
            .bind(category).bind(category.to_string()).execute(&db).await?;
    }
    sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'same','root label','A','root metadata',$2,'{}',0,1,2,1)")
        .bind(root).bind(device).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_modulebay_module_bay_types (modulebay_id,modulebaytype_id) VALUES ($1,$2)")
        .bind(root).bind(a).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_moduletype_module_bay_types (moduletype_id,modulebaytype_id) VALUES ($1,$2)")
        .bind(module_type).bind(a).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'archived','active','','keep','{}')")
        .bind(module).bind(device).bind(root).bind(module_type).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,module_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'same','child label','B','child metadata',$2,$3,'{}',0,1,2,1)")
        .bind(child).bind(device).bind(module).execute(&db).await?;
    sqlx::query("UPDATE dcim_modulebay SET enabled=false WHERE id=ANY($1)")
        .bind(vec![root, child])
        .execute(&db)
        .await?;
    sqlx::query(
        "UPDATE dcim_moduletype_module_bay_types SET modulebaytype_id=$2 WHERE moduletype_id=$1",
    )
    .bind(module_type)
    .bind(b)
    .execute(&db)
    .await?;

    for (table, id) in [
        ("dcim_device", device),
        ("dcim_module", module),
        ("dcim_modulebay", root),
        ("dcim_modulebay", child),
        ("dcim_moduletype", module_type),
    ] {
        sqlx::query(&format!("UPDATE {table} SET tags=$2 WHERE id=$1"))
            .bind(id)
            .bind(vec![tag.to_string()])
            .execute(&db)
            .await?;
    }
    sqlx::query("INSERT INTO playbook_target_set (name,mode,selector) VALUES ($1,'dynamic',$2)")
        .bind(format!("restore-targets-{tag}"))
        .bind(json!({"tag_ids":[tag.to_string()]}))
        .execute(&db)
        .await?;
    let archive = service.export_full_backup(true, None).await?;
    let before = canonical_database_bytes(&archive)?;
    let request = |payload| FullBackupImportRequest {
        payload,
        confirm_full_restore: true,
        dry_run: false,
        selection: None,
        rules: None,
    };
    service.import_full_backup(request(archive.clone())).await?;
    let after = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
    assert!(
        before == after,
        "full restore must retain archived graph metadata and memberships"
    );
    let restored_parent: Option<Uuid> =
        sqlx::query_scalar("SELECT parent_id FROM dcim_modulebay WHERE id=$1")
            .bind(child)
            .fetch_one(&db)
            .await?;
    assert_eq!(restored_parent, Some(root));
    let restored_enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM dcim_modulebay WHERE id=$1")
            .bind(root)
            .fetch_one(&db)
            .await?;
    assert!(!restored_enabled);

    // A malformed parent projection must be rejected before staging overwrites
    // it. A cycle must likewise roll back the full transaction after TRUNCATE.
    for cycle in [false, true] {
        let mut invalid = archive.clone();
        let table = invalid
            .tables
            .iter_mut()
            .find(|table| table.name == "dcim_modulebay")
            .ok_or("missing bays")?;
        let row = table
            .rows
            .iter_mut()
            .find(|row| row["id"] == json!(root))
            .ok_or("missing root")?;
        row["parent_id"] = json!(child);
        if cycle {
            row["module_id"] = json!(module);
        }
        let result = service.import_full_backup(request(invalid)).await;
        assert!(result.is_err(), "invalid archived graph must not commit");
        let state = canonical_database_bytes(&service.export_full_backup(true, None).await?)?;
        assert!(
            state == after,
            "failed graph restore must leave the pre-restore database intact"
        );
    }
    // Restore staging did not turn off the live disabled-bay install guard.
    let live=sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'','active','','','{}')")
        .bind(Uuid::now_v7()).bind(device).bind(child).bind(module_type).execute(&db).await;
    assert!(live.is_err());
    service.import_full_backup(request(original)).await?;
    Ok(())
}
