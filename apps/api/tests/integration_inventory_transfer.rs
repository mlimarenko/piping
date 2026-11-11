//! Native read-only planner proof against an already provisioned PostgreSQL
//! fixture. No application bootstrap, migrations, Redis or HTTP are invoked.
//! Requires a fixture administrator able to create a temporary SELECT-only role.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use piping_backend::contexts::sync::domain::inventory_preflight::FindingCode;
use piping_backend::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, InventoryEntity, MappingConfigDocument,
    MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget, SourceEntity,
    SourceIdentity, SourceRecord, SourceSnapshot,
};
use piping_backend::contexts::sync::infrastructure::persistence::local_inventory_target::{
    LocalInventoryCatalog, assess_preparation,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn configuration() -> MappingConfigDocument {
    MappingConfigDocument {
        absence_policy: Default::default(),
        reference: MappingRef::new(Uuid::now_v7(), 1),
        name: "synthetic".into(),
        description: None,
        source_provider_id: Uuid::now_v7(),
        target: MappingTarget::LocalInventory,
        status: MappingConfigurationStatus::Active,
        direction: MappingDirection::Pull,
        conflict_mode: ConflictMode::NetboxWins,
        created_by: None,
        created_at: Utc::now(),
        retired_at: None,
        entity_mappings: Vec::new(),
        field_mappings: Vec::new(),
    }
}

/// Compare actual values, including graph edges, across every fixture-owned
/// public table. Never print these values, even on an assertion failure.
async fn observed_values(pool: &PgPool) -> Result<BTreeMap<String, Value>, sqlx::Error> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname='public' ORDER BY tablename",
    )
    .fetch_all(pool)
    .await?;
    let mut observed = BTreeMap::new();
    for table in tables {
        let escaped = table.replace('"', "\"\"");
        let query = format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM public.\"{escaped}\" t"
        );
        let rows: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .fetch_one(pool)
            .await?;
        observed.insert(table, rows);
    }
    Ok(observed)
}

#[tokio::test]
#[ignore = "requires provisioned PostgreSQL fixture and role-administration authority"]
async fn select_only_role_plans_success_and_conflict_without_changing_values_or_graphs()
-> TestResult {
    let url = std::env::var("PIPING_DATABASE__URL")?;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let suffix = Uuid::now_v7().simple().to_string();
    let role = format!("transfer_reader_{suffix}");
    let tag_id = Uuid::now_v7();
    let field_id = Uuid::now_v7();
    let provider_id = Uuid::now_v7();
    let field_name = format!("transfer_{suffix}");
    let slug = format!("transfer-{suffix}");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE ROLE {role} NOLOGIN")))
        .execute(&admin)
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "GRANT USAGE ON SCHEMA public TO {role}"
    )))
    .execute(&admin)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "GRANT SELECT ON ALL TABLES IN SCHEMA public TO {role}"
    )))
    .execute(&admin)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("GRANT {role} TO CURRENT_USER")))
        .execute(&admin)
        .await?;
    sqlx::query("INSERT INTO inventory_tag(id, name, slug) VALUES ($1,$2,$2)")
        .bind(tag_id)
        .bind(&slug)
        .execute(&admin)
        .await?;
    sqlx::query("INSERT INTO sync_provider(id, name, provider_type, config, is_enabled) VALUES ($1,$2,'netbox','{}'::jsonb,false)")
        .bind(provider_id).bind(&slug).execute(&admin).await?;
    sqlx::query("INSERT INTO custom_field(id,name) VALUES ($1,$2)")
        .bind(field_id)
        .bind(&field_name)
        .execute(&admin)
        .await?;
    for (source, target, id) in [
        ("extras.tags", InventoryEntity::Tag.target_key(), tag_id),
        (
            "extras.custom-fields",
            InventoryEntity::CustomField.target_key(),
            field_id,
        ),
    ] {
        sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id) VALUES ($1,$2,'1',$3,$4)")
            .bind(provider_id).bind(source).bind(target).bind(id).execute(&admin).await?;
    }

    let set_role = format!("SET ROLE {role}");
    let reader = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |connection, _| {
            let command = set_role.clone();
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(command))
                    .execute(connection)
                    .await
                    .map(|_| ())
            })
        })
        .connect(&url)
        .await?;
    let target = LocalInventoryCatalog::new(reader.clone());
    let result: TestResult = async {
        let before = observed_values(&admin).await?;
        let write = sqlx::query("UPDATE inventory_tag SET name=name WHERE false")
            .execute(&reader)
            .await;
        let forbidden = write
            .err()
            .ok_or_else(|| std::io::Error::other("SELECT-only role unexpectedly allowed UPDATE"))?;
        assert!(
            forbidden
                .as_database_error()
                .and_then(|e| e.code())
                .is_some_and(|code| code == "42501")
        );
        let mut config = configuration();
        config.source_provider_id = provider_id;
        let batch = assess_preparation(&config, &SourceSnapshot::default())
            .batch
            .ok_or_else(|| std::io::Error::other("empty valid batch rejected"))?;
        let success = target.plan_read_only(&batch).await?;
        assert!(success.analysis_complete && success.findings.is_empty());
        assert!(!success.snapshot_fingerprint.is_empty() && !success.snapshot_identity.is_empty());
        let mapping_id = Uuid::now_v7();
        config.entity_mappings.push(EntityMapping {
            id: mapping_id,
            mapping_config_logical_id: config.reference.logical_id,
            mapping_config_version_id: config.reference.version_id,
            source_entity: "extras.tags".into(),
            target_entity: InventoryEntity::Tag.target_key(),
            enabled: true,
            created_at: Utc::now(),
        });
        for (field, value) in [("name", json!("synthetic")), ("slug", json!(slug.clone()))] {
            config.field_mappings.push(FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: mapping_id,
                source_field: field.into(),
                target_field: field.into(),
                forward_transform_json: json!({"op": "literal", "args": [value]}),
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: Utc::now(),
            });
        }
        let snapshot = SourceSnapshot { coverage: None, records: vec![SourceRecord::new(
                SourceIdentity::new(
                    config.source_provider_id,
                    SourceEntity::new("extras.tags")?,
                    "1",
                )?,
                serde_json::Map::new(),
            )],
        };
        let bound_batch = assess_preparation(&config, &snapshot)
            .batch
            .ok_or_else(|| std::io::Error::other("synthetic mapped tag rejected"))?;
        let bound = target.plan_read_only(&bound_batch).await?;
        assert!(bound.analysis_complete);
        assert!(bound.identities.iter().any(|identity| identity.local_id == tag_id));
        let mut unbound_snapshot = snapshot.clone();
        unbound_snapshot.records[0].identity.external_id = "2".into();
        let blocked_batch = assess_preparation(&config, &unbound_snapshot).batch
            .ok_or_else(|| std::io::Error::other("synthetic conflict batch rejected"))?;
        let blocked = target.plan_read_only(&blocked_batch).await?;
        assert!(blocked.analysis_complete);
        assert!(
            blocked
                .findings
                .iter()
                .any(|f| f.code == FindingCode::TargetDefinitionConflict)
        );
        let mut cf_config = config.clone();
        cf_config.entity_mappings.clear();
        cf_config.field_mappings.clear();
        let cf_mapping_id = Uuid::now_v7();
        cf_config.entity_mappings.push(EntityMapping {
            id: cf_mapping_id, mapping_config_logical_id: cf_config.reference.logical_id,
            mapping_config_version_id: cf_config.reference.version_id, source_entity: "extras.custom-fields".into(),
            target_entity: InventoryEntity::CustomField.target_key(), enabled: true, created_at: Utc::now(),
        });
        for field in ["name", "type"] {
            cf_config.field_mappings.push(FieldMapping {
                id: Uuid::now_v7(), entity_mapping_id: cf_mapping_id, source_field: field.into(), target_field: field.into(),
                forward_transform_json: json!({"op": "identity", "args": [{"field": field}]}),
                reverse_transform_json: None, round_trip_critical: false, created_at: Utc::now(),
            });
        }
        let cf_snapshot = SourceSnapshot { coverage: None, records: vec![SourceRecord::new(
            SourceIdentity::new(provider_id, SourceEntity::new("extras.custom-fields")?, "1")?,
            serde_json::from_value(json!({"name": field_name.clone(), "type": "text"}))?,
        )] };
        let cf_batch = assess_preparation(&cf_config, &cf_snapshot).batch
            .ok_or_else(|| std::io::Error::other("synthetic custom-field batch rejected"))?;
        let cf_bound = target.plan_read_only(&cf_batch).await?;
        assert!(cf_bound.analysis_complete);
        assert!(cf_bound.identities.iter().any(|identity| identity.local_id == field_id));
        let mut conflict_cf = cf_snapshot.clone();
        conflict_cf.records[0].identity.external_id = "2".into();
        let conflict_batch = assess_preparation(&cf_config, &conflict_cf).batch
            .ok_or_else(|| std::io::Error::other("synthetic definition conflict rejected before graph"))?;
        let conflict = target.plan_read_only(&conflict_batch).await?;
        assert!(conflict.analysis_complete);
        assert!(conflict.findings.iter().any(|f| f.code == FindingCode::TargetDefinitionConflict));
        let mut region_config = config.clone();
        region_config.entity_mappings.clear();
        region_config.field_mappings.clear();
        let region_mapping_id = Uuid::now_v7();
        region_config.entity_mappings.push(EntityMapping {
            id: region_mapping_id, mapping_config_logical_id: region_config.reference.logical_id,
            mapping_config_version_id: region_config.reference.version_id, source_entity: "dcim.regions".into(),
            target_entity: InventoryEntity::Region.target_key(), enabled: true, created_at: Utc::now(),
        });
        for field in ["name", "slug", "comments", "description", "parent"] {
            region_config.field_mappings.push(FieldMapping {
                id: Uuid::now_v7(), entity_mapping_id: region_mapping_id,
                source_field: field.into(), target_field: field.into(),
                forward_transform_json: json!({"op": "identity", "args": [{"field": field}]}),
                reverse_transform_json: None, round_trip_critical: false, created_at: Utc::now(),
            });
        }
        let region = |id: &str, parent: u64| -> Result<SourceRecord, Box<dyn std::error::Error + Send + Sync>> {
            Ok(SourceRecord::new(
                SourceIdentity::new(provider_id, SourceEntity::new("dcim.regions")?, id)?,
                serde_json::from_value(json!({"name": format!("transfer-{suffix}-{id}"), "slug": format!("transfer-{suffix}-{id}"), "comments": "", "description": "", "parent": {"id": parent}}))?,
            ))
        };
        let missing_batch = assess_preparation(&region_config, &SourceSnapshot { coverage: None, records: vec![region("1", 2)?] }).batch
            .ok_or_else(|| std::io::Error::other("synthetic region rejected before graph"))?;
        let missing = target.plan_read_only(&missing_batch).await?;
        assert!(!missing.analysis_complete);
        assert!(missing.findings.iter().any(|f| f.code == FindingCode::TargetReferenceMissing));
        let cyclic_batch = assess_preparation(&region_config, &SourceSnapshot { coverage: None, records: vec![region("1", 2)?, region("2", 1)?] }).batch
            .ok_or_else(|| std::io::Error::other("synthetic cyclic regions rejected before graph"))?;
        let cyclic = target.plan_read_only(&cyclic_batch).await?;
        assert!(cyclic.analysis_complete);
        assert!(cyclic.findings.iter().any(|f| f.code == FindingCode::HierarchyCycle));
        let mut composite_config = config.clone();
        composite_config.entity_mappings.extend(cf_config.entity_mappings.clone());
        composite_config.entity_mappings.extend(region_config.entity_mappings.clone());
        composite_config.field_mappings.extend(cf_config.field_mappings.clone());
        composite_config.field_mappings.extend(region_config.field_mappings.clone());
        composite_config.field_mappings.push(FieldMapping {
            id: Uuid::now_v7(), entity_mapping_id: cf_mapping_id,
            source_field: "object_types".into(), target_field: "object_types".into(),
            forward_transform_json: json!({"op": "identity", "args": [{"field": "object_types"}]}),
            reverse_transform_json: None, round_trip_critical: false, created_at: Utc::now(),
        });
        let mut scoped_conflict = conflict_cf.records[0].clone();
        scoped_conflict.fields.insert("object_types".into(), json!([format!("transfer_{suffix}.first"), format!("transfer_{suffix}.second")]));
        let mut dependent_region = region("1", 2)?;
        dependent_region.fields.insert("custom_fields".into(), Value::Object(serde_json::Map::from_iter([
            (field_name.clone(), json!({"private": "CANARY-private-value"})),
        ])));
        let composite_snapshot = SourceSnapshot { coverage: None, records: vec![
            unbound_snapshot.records[0].clone(), scoped_conflict,
            dependent_region, region("2", 1)?, region("3", 999)?, region("4", 998)?,
        ] };
        let composite_batch = assess_preparation(&composite_config, &composite_snapshot).batch
            .ok_or_else(|| std::io::Error::other("independent graph fixture rejected before planning"))?;
        let composite = target.plan_read_only(&composite_batch).await?;
        assert!(!composite.analysis_complete);
        for code in [FindingCode::TargetDefinitionConflict, FindingCode::CustomFieldScopeInvalid,
            FindingCode::TargetReferenceMissing, FindingCode::HierarchyCycle] {
            let independent: Vec<_> = composite.findings.iter().filter(|finding| finding.code == code).collect();
            assert!(independent.len() >= 2);
            assert!(independent.iter().all(|finding| finding.record_identity_sha256.is_some() && finding.field_path.is_some() && finding.edge.is_some()));
        }
        assert_eq!(composite.identities.len(), 6);
        assert!(composite.findings.iter().any(|finding| finding.code == FindingCode::CustomFieldValueInvalid));
        assert!(!composite.cyclic_components.is_empty());
        let encoded = serde_json::to_string(&composite)?;
        assert!(!encoded.contains(&slug) && !encoded.contains(&field_name) && !encoded.contains("CANARY"));
        let repeated = target.plan_read_only(&composite_batch).await?;
        assert_eq!(composite.findings, repeated.findings);
        assert_eq!(composite.snapshot_fingerprint, repeated.snapshot_fingerprint);
        let after = observed_values(&admin).await?;
        assert!(before == after, "analysis changed fixture values or graphs");
        Ok(())
    }
    .await;
    reader.close().await;
    sqlx::query("DELETE FROM sync_provider WHERE id=$1")
        .bind(provider_id)
        .execute(&admin)
        .await?;
    sqlx::query("DELETE FROM custom_field WHERE id=$1")
        .bind(field_id)
        .execute(&admin)
        .await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(tag_id)
        .execute(&admin)
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP OWNED BY {role}")))
        .execute(&admin)
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE {role}")))
        .execute(&admin)
        .await?;
    result
}
