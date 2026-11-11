use super::tests::{assessment_config, provider_record};
use super::*;
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn hardware_snapshot(provider: Uuid, suffix: &str) -> SourceSnapshot {
    use InventoryEntity as E;
    let rows = [
        (E::Manufacturer, json!({"name": suffix, "slug": suffix})),
        (E::Site, json!({"name": suffix, "slug": suffix})),
        (
            E::DeviceRole,
            json!({"name": suffix, "slug": suffix, "color": "123456", "vm_role": false}),
        ),
        (
            E::DeviceType,
            json!({"model": suffix, "slug": suffix, "manufacturer": 1, "u_height": 1, "is_full_depth": true, "front_image": null, "rear_image": null, "exclude_from_utilization": false, "cooling_method": {"value": "liquid"}, "end_of_life": "2030-02-28"}),
        ),
        (
            E::Device,
            json!({"name": suffix, "device_type": 1, "role": 1, "site": 1, "cooling_method": {"value": "liquid"}}),
        ),
        // Deliberately put immediate FK consumers ahead of their owners.
        (
            E::CoolingFeed,
            json!({"name": suffix, "cooling_source": 1, "rack": 1, "cooling_capacity": 12.5, "max_flow": 2.5, "max_flow_unit": {"value": "lpm"}}),
        ),
        (
            E::CoolingOutflow,
            json!({"name": "out", "device": 1, "module": 1, "cooling_intake": 1, "diameter": 12.5, "diameter_unit": "mm"}),
        ),
        (
            E::CoolingSource,
            json!({"name": suffix, "type": {"value": "chiller"}, "site": 1, "location": 1, "cooling_capacity": 20.5}),
        ),
        (
            E::CoolingIntake,
            json!({"name": "in", "device": 1, "module": 1, "max_flow": 2.5, "max_flow_unit": "lpm"}),
        ),
        (
            E::CoolingOutflowTemplate,
            json!({"name": "out", "module_type": 1, "cooling_intake": 1, "diameter": 12.5, "diameter_unit": "mm"}),
        ),
        (
            E::CoolingIntakeTemplate,
            json!({"name": "in", "module_type": 1, "max_flow": 2.5, "max_flow_unit": "lpm"}),
        ),
        (
            E::ModuleBayTemplate,
            json!({"name": "child-slot", "module_type": 1, "enabled": true, "module_bay_types": [1]}),
        ),
        (
            E::Module,
            json!({"device": 1, "module_bay": 1, "module_type": 1}),
        ),
        (
            E::ModuleBay,
            json!({"name": "slot", "device": 1, "enabled": true, "module_bay_types": [1]}),
        ),
        (
            E::ModuleType,
            json!({"model": suffix, "manufacturer": 1, "module_bay_types": [1], "cooling_method": {"value": "liquid"}, "end_of_life": "2030-02-28"}),
        ),
        (
            E::ModuleBayType,
            json!({"name": suffix, "slug": suffix, "manufacturer": 1, "color": "123456"}),
        ),
        (
            E::Cable,
            json!({"label": suffix, "profile": "", "color": "123456", "bundle": 1}),
        ),
        (
            E::Rack,
            json!({"name": suffix, "site": 1, "location": 1, "group": 1, "width": {"value": 19}, "u_height": 42, "desc_units": false, "starting_unit": 1, "cooling_capability": {"value": "liquid-only"}, "cooling_capacity": 20.5}),
        ),
        (
            E::Location,
            json!({"name": suffix, "slug": suffix, "site": 1}),
        ),
        (E::CableBundle, json!({"name": suffix})),
        (E::RackGroup, json!({"name": suffix, "slug": suffix})),
    ];
    let mut snapshot = SourceSnapshot {
        coverage: None,
        records: rows
            .into_iter()
            .map(|(entity, fields)| provider_record(provider, entity, "1", fields))
            .collect(),
    };
    for (id, name, end) in [("1", "eth0", "A"), ("2", "eth1", "B")] {
        snapshot.records.push(provider_record(provider, E::Interface, id,
            json!({"name": name, "device": 1, "cable": {"id": 1}, "cable_end": end, "type": {"value": "1000base-t"}, "enabled": true, "mark_connected": false, "mgmt_only": false})));
    }
    snapshot
}

fn mapping_for(snapshot: &SourceSnapshot) -> MappingConfigDocument {
    let mut mapped = HashSet::new();
    let mut configs = snapshot
        .records
        .iter()
        .filter(|record| mapped.insert(record.identity.entity.clone()))
        .map(|record| {
            let entity = InventoryEntity::from_source_key(record.identity.entity.as_str())
                .expect("catalogue entity");
            let names: Vec<_> = if entity.is_membership() {
                Vec::new()
            } else {
                record
                    .fields
                    .keys()
                    .map(String::as_str)
                    .filter(|name| {
                        // The component discriminator is source metadata, not a
                        // separately mapped InventoryItem target field.
                        entity != InventoryEntity::InventoryItem || *name != "component_type"
                    })
                    .collect()
            };
            assessment_config(entity, record.identity.source_provider_id, &names)
        });
    let mut config = configs.next().expect("non-empty fixture");
    for additional in configs {
        config.entity_mappings.extend(additional.entity_mappings);
        config.field_mappings.extend(additional.field_mappings);
    }
    config
}

#[test]
fn full_component_template_snapshot_prepares_rows_and_typed_edges() {
    use InventoryEntity as E;
    let provider = Uuid::from_u128(1);
    let rows = [
        (
            E::ConsolePortTemplate,
            json!({"name":"console", "label":"", "description":"", "type":null, "device_type":1, "module_type":null}),
        ),
        (
            E::ConsoleServerPortTemplate,
            json!({"name":"server", "label":"", "description":"", "type":null, "device_type":1, "module_type":null}),
        ),
        (
            E::DeviceBayTemplate,
            json!({"name":"bay", "label":"", "description":"", "device_type":1}),
        ),
        (
            E::FrontPortTemplate,
            json!({"name":"front", "label":"", "description":"", "type":"8p8c", "color":"", "positions":2, "device_type":1, "module_type":null}),
        ),
        (
            E::RearPortTemplate,
            json!({"name":"rear", "label":"", "description":"", "type":"8p8c", "color":"", "positions":2, "device_type":1, "module_type":null}),
        ),
        (
            E::InterfaceTemplate,
            json!({"name":"interface", "label":"", "description":"", "type":"1000base-t", "enabled":true, "mgmt_only":false, "bridge":null, "poe_mode":null, "poe_type":null, "rf_role":null, "device_type":1, "module_type":null}),
        ),
        (
            E::InventoryItemTemplate,
            json!({"name":"item", "label":"", "description":"", "part_id":"", "device_type":1, "parent":null, "manufacturer":null, "role":null, "component":null}),
        ),
        (
            E::PowerPortTemplate,
            json!({"name":"power", "label":"", "description":"", "type":null, "maximum_draw":null, "allocated_draw":null, "device_type":1, "module_type":null}),
        ),
        (
            E::PowerOutletTemplate,
            json!({"name":"outlet", "label":"", "description":"", "type":null, "color":"", "feed_leg":null, "power_port":1, "device_type":1, "module_type":null}),
        ),
        (
            E::PortTemplateMapping,
            json!({"front_port":1, "rear_port":1, "front_port_position":1, "rear_port_position":2, "device_type":1, "module_type":null}),
        ),
    ];
    let snapshot = SourceSnapshot {
        coverage: None,
        records: rows
            .into_iter()
            .map(|(entity, fields)| provider_record(provider, entity, "1", fields))
            .collect(),
    };
    let config = mapping_for(&snapshot);
    let result = assess_preparation(&config, &snapshot);
    let batch = result
        .batch
        .unwrap_or_else(|| panic!("{:?}", result.findings));
    assert_eq!(batch.rows.len(), 10);
    let mapping = batch
        .rows
        .iter()
        .find(|row| row.entity == E::PortTemplateMapping)
        .expect("mapping");
    for (field, target) in [
        ("front_port", E::FrontPortTemplate),
        ("rear_port", E::RearPortTemplate),
        ("device_type", E::DeviceType),
    ] {
        assert!(
            mapping
                .references
                .iter()
                .any(|(name, binding)| name.name() == field && binding.target == Some(target))
        );
    }
    assert!(
        mapping
            .cleared
            .iter()
            .any(|field| field.name() == "module_type")
    );
    let mut invalid = snapshot.clone();
    invalid
        .records
        .iter_mut()
        .find(|row| row.identity.entity.as_str() == E::InterfaceTemplate.source_key())
        .unwrap()
        .fields
        .insert("channels".into(), json!([1]));
    assert!(assess_preparation(&config, &invalid).batch.is_none());
}

#[test]
fn hardware_snapshot_prepares_all_nine_models_and_their_relation_consumers() {
    let snapshot = hardware_snapshot(Uuid::now_v7(), "hardware-prepare");
    let assessment = assess_preparation(&mapping_for(&snapshot), &snapshot);
    assert!(assessment.analysis_complete);
    let batch = assessment
        .batch
        .unwrap_or_else(|| panic!("{:?}", assessment.findings));
    assert_eq!(batch.rows.len(), 23);
    for entity in [
        InventoryEntity::ModuleType,
        InventoryEntity::ModuleBayTemplate,
        InventoryEntity::ModuleBay,
    ] {
        let row = batch
            .rows
            .iter()
            .find(|row| row.entity == entity)
            .expect("junction owner");
        assert_eq!(row.junctions.len(), 1);
        assert_eq!(
            row.junctions[0].1[0].target,
            Some(InventoryEntity::ModuleBayType)
        );
    }
    let outflow = batch
        .rows
        .iter()
        .find(|row| row.entity == InventoryEntity::CoolingOutflow)
        .expect("outflow");
    assert!(
        outflow
            .references
            .iter()
            .any(|(field, reference)| field.name() == "cooling_intake"
                && reference.target == Some(InventoryEntity::CoolingIntake))
    );
}

#[test]
fn hardware_mapping_rejects_malformed_junction_entries_and_calendar_dates() {
    let mut snapshot = hardware_snapshot(Uuid::now_v7(), "hardware-invalid");
    snapshot
        .records
        .iter_mut()
        .find(|record| record.identity.entity.as_str() == "dcim.module-bays")
        .expect("bay")
        .fields
        .insert(
            "module_bay_types".into(),
            json!([{"label": "not an identity"}]),
        );
    let assessment = assess_preparation(&mapping_for(&snapshot), &snapshot);
    assert!(assessment.batch.is_none());
    assert!(
        assessment
            .findings
            .iter()
            .any(|finding| finding.field_path.as_deref() == Some("module_bay_types"))
    );
    let field = InventoryEntity::ModuleType
        .field("end_of_life")
        .expect("date");
    for invalid in ["2030-02-29", "2030-2-01", "2030-02-28T00:00:00Z"] {
        assert!(convert_scalar(field, &json!(invalid)).is_err(), "{invalid}");
    }
    assert!(convert_scalar(field, &json!("2032-02-29")).is_ok());
    let unit = InventoryEntity::CoolingIntake
        .field("max_flow_unit")
        .expect("flow unit");
    assert!(convert_scalar(unit, &json!({"value": "lpm"})).is_ok());
    assert!(convert_scalar(unit, &json!({"value": "litres"})).is_err());
    let capacity = InventoryEntity::CoolingSource
        .field("cooling_capacity")
        .expect("capacity");
    assert!(
        convert_scalar(capacity, &json!(12.345)).is_err(),
        "a source capacity must not be rounded silently"
    );
}

#[test]
fn hardware_measurements_reject_lossy_source_values_and_accept_canonical_units() {
    let provider = Uuid::now_v7();
    let valid = SourceSnapshot {
        coverage: None,
        records: vec![provider_record(
            provider,
            InventoryEntity::CoolingIntake,
            "1",
            json!({"name": "in", "device": 1, "diameter": 12.35, "diameter_unit": {"value": "cm"}, "max_flow": 20.25, "max_flow_unit": {"value": "m3ph"}}),
        )],
    };
    let assessment = assess_preparation(&mapping_for(&valid), &valid);
    assert!(
        assessment.batch.is_some(),
        "cm and m3ph are native units: {:?}",
        assessment.findings
    );
    for field in ["diameter", "max_flow"] {
        for invalid in [12.345, 0.0, -0.01, 1_000_000.0] {
            let mut snapshot = valid.clone();
            snapshot.records[0]
                .fields
                .insert(field.into(), json!(invalid));
            let assessment = assess_preparation(&mapping_for(&snapshot), &snapshot);
            assert!(assessment.analysis_complete);
            assert!(
                assessment.batch.is_none(),
                "{field}={invalid} must fail before SQL can round or overflow it"
            );
            assert!(
                assessment
                    .findings
                    .iter()
                    .any(|finding| finding.field_path.as_deref() == Some(field))
            );
        }
    }
}

#[test]
fn hardware_measurements_validate_raw_numeric_and_netbox_string_precision() -> TestResult {
    for (entity, name, zero_allowed, maximum) in [
        (
            InventoryEntity::CoolingSource,
            "cooling_capacity",
            true,
            "99999999.99",
        ),
        (
            InventoryEntity::CoolingFeed,
            "cooling_capacity",
            true,
            "99999999.99",
        ),
        (InventoryEntity::CoolingFeed, "max_flow", false, "999999.99"),
        (
            InventoryEntity::CoolingIntake,
            "diameter",
            false,
            "999999.99",
        ),
        (
            InventoryEntity::CoolingIntake,
            "max_flow",
            false,
            "999999.99",
        ),
        (
            InventoryEntity::CoolingOutflow,
            "diameter",
            false,
            "999999.99",
        ),
        (
            InventoryEntity::CoolingIntakeTemplate,
            "diameter",
            false,
            "999999.99",
        ),
        (
            InventoryEntity::CoolingIntakeTemplate,
            "max_flow",
            false,
            "999999.99",
        ),
        (
            InventoryEntity::CoolingOutflowTemplate,
            "diameter",
            false,
            "999999.99",
        ),
        (
            InventoryEntity::Rack,
            "cooling_capacity",
            true,
            "99999999.99",
        ),
    ] {
        let field = entity.field(name).expect("measurement field");
        for raw in [
            "0.01",
            "0.29",
            "1.14",
            "0.290000",
            "29e-2",
            "11400e-4",
            "1.1400E+0",
            maximum,
        ] {
            for value in [serde_json::from_str::<Value>(raw)?, json!(raw)] {
                let converted = convert_scalar(field, &value)?;
                let InventoryValue::Number { value } = converted else {
                    panic!("measurement must remain a numeric inventory value");
                };
                assert_eq!(value, raw.parse::<f64>()?, "{entity:?} {name} {raw}");
                let stored = InventoryValue::Number { value };
                let mut query = QueryBuilder::<Postgres>::new("SELECT ");
                push_scalar(&mut query, PgType::Numeric, &stored)?;
                assert_eq!(query.sql(), "SELECT $1::numeric");
            }
        }
        for raw in [
            "0.00000005",
            "5e-8",
            "0.2900000000000000000000000000000000000001",
            "1.1400000000000000000000000000000000000001",
            "99999999.99000000000000000000000000000001",
            "0.001",
            "-0.01",
            "100000000",
            "1e1000",
            "1e-1000",
        ] {
            for value in [serde_json::from_str::<Value>(raw)?, json!(raw)] {
                assert!(
                    convert_scalar(field, &value).is_err(),
                    "{entity:?} {name} {value}"
                );
            }
        }
        for raw in ["0", "0.0000", "0e-1000"] {
            for value in [serde_json::from_str::<Value>(raw)?, json!(raw)] {
                assert_eq!(
                    convert_scalar(field, &value).is_ok(),
                    zero_allowed,
                    "{name} {raw}"
                );
            }
        }
        for value in [
            json!("NaN"),
            json!("Infinity"),
            json!(true),
            json!({"value": "0.29"}),
        ] {
            assert!(convert_scalar(field, &value).is_err());
        }
    }
    Ok(())
}

#[test]
fn hardware_measurement_raw_tail_blocks_preparation_at_its_field() -> TestResult {
    for raw in ["0.00000005", "0.2900000000000000000000000000000000000001"] {
        for value in [serde_json::from_str::<Value>(raw)?, json!(raw)] {
            let snapshot = SourceSnapshot {
                coverage: None,
                records: vec![provider_record(
                    Uuid::now_v7(),
                    InventoryEntity::CoolingSource,
                    "1",
                    json!({"name": "precision", "type": "chiller", "site": 1, "cooling_capacity": value}),
                )],
            };
            let assessment = assess_preparation(&mapping_for(&snapshot), &snapshot);
            assert!(assessment.analysis_complete);
            assert!(assessment.batch.is_none());
            assert!(
                assessment
                    .findings
                    .iter()
                    .any(|finding| finding.field_path.as_deref() == Some("cooling_capacity"))
            );
        }
    }
    Ok(())
}

#[test]
fn interface_cable_end_accepts_only_canonical_sides() {
    let field = InventoryEntity::Interface
        .field("cable_end")
        .expect("native cable side");
    for side in ["A", "B"] {
        assert!(convert_scalar(field, &json!(side)).is_ok());
    }
    for side in ["a", "b", "AB", "C", " A"] {
        assert!(convert_scalar(field, &json!(side)).is_err(), "{side}");
    }
}

async fn apply_snapshot(
    target: &LocalInventoryCatalog,
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &SourceSnapshot,
    existing: &HashMap<(String, String), Uuid>,
) -> TestResult<HashMap<(String, String), Uuid>> {
    let batch = target.prepare(&mapping_for(snapshot), snapshot)?;
    let ids = merge_local_ids(&batch, existing.clone());
    let mut content_types = HashMap::new();
    let mut planned = Vec::new();
    for row in &batch.rows {
        planned.push(plan_row(row, &ids, None, &mut content_types, tx).await?);
    }
    apply_planned_rows(tx, &planned).await?;
    rebuild_touched_trees(tx, &planned).await?;
    recompute_counts(tx, &planned, &[]).await?;
    Ok(ids)
}

fn local_id(ids: &HashMap<(String, String), Uuid>, entity: InventoryEntity) -> Uuid {
    ids[&(entity.source_key().to_owned(), "1".to_owned())]
}

struct TemplateReferenceAccess {
    denied: Option<Uuid>,
}

struct DeviceBayTemplateAccess {
    template: Uuid,
    fail_after_write: bool,
    ownership_calls: std::sync::atomic::AtomicUsize,
    seen: std::sync::Mutex<Vec<(bool, String)>>,
}

impl InventoryCommitAuthorisation for DeviceBayTemplateAccess {
    fn ensure_owned(&self) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            let call = self
                .ownership_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail_after_write && call == 1 {
                Err(AppError::Forbidden(
                    "synthetic post-write ownership loss".into(),
                ))
            } else {
                Ok(())
            }
        })
    }

    fn ensure_mutation_access(
        &self,
        _action: crate::contexts::access_control::domain::Action,
        id: Uuid,
        attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        assert_eq!(id, self.template);
        assert_eq!(attributes["object_type"], "dcim.devicebaytemplate");
        self.seen.lock().expect("mutation observations").push((
            attributes["enabled"]
                .as_bool()
                .expect("materialized enabled"),
            attributes["label"]
                .as_str()
                .expect("materialized label")
                .to_owned(),
        ));
        Box::pin(async { Ok(()) })
    }

    fn ensure_reference_access(
        &self,
        _id: Uuid,
        _attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
#[ignore = "requires an explicitly owned PostgreSQL fixture with integrated migrations and inventory owners"]
async fn device_bay_template_enabled_commit_pull_preserves_and_rolls_back() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let provider = Uuid::now_v7();
    let suffix = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    let seed = hardware_snapshot(provider, &format!("bay-template-{suffix}"));
    let ids = apply_snapshot(&target, &mut tx, &seed, &HashMap::new()).await?;
    let device_type = local_id(&ids, InventoryEntity::DeviceType);
    sqlx::query("UPDATE dcim_devicetype SET subdevice_role='parent' WHERE id=$1")
        .bind(device_type)
        .execute(&mut *tx)
        .await?;
    let template_seed = SourceSnapshot {
        coverage: None,
        records: vec![provider_record(
            provider,
            InventoryEntity::DeviceBayTemplate,
            "1",
            json!({"name":"bay", "label":"seed", "description":"", "device_type":1, "enabled":true}),
        )],
    };
    let template_ids = apply_snapshot(&target, &mut tx, &template_seed, &ids).await?;
    let template = local_id(&template_ids, InventoryEntity::DeviceBayTemplate);
    sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES ($1,$2,'netbox','{}',false)")
        .bind(provider).bind(format!("bay-template-{suffix}")).execute(&mut *tx).await?;
    for (entity, id) in [
        (InventoryEntity::DeviceType, device_type),
        (InventoryEntity::DeviceBayTemplate, template),
    ] {
        sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id) VALUES ($1,$2,'1',$3,$4)")
            .bind(provider).bind(entity.source_key()).bind(entity.target_key()).bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;

    let mut invalid = template_seed.clone();
    invalid.records[0]
        .fields
        .insert("enabled".into(), json!(null));
    assert!(
        matches!(
            target.prepare(&mapping_for(&invalid), &invalid),
            Err(AppError::Validation(_))
        ),
        "explicit null must not become an omitted enabled field"
    );

    for (enabled, label, fail, expected, previous) in [
        (Some(false), "disabled", false, false, (true, "seed")),
        (None, "omitted-disabled", false, false, (false, "disabled")),
        (
            Some(true),
            "enabled",
            false,
            true,
            (false, "omitted-disabled"),
        ),
        (None, "omitted", false, true, (true, "enabled")),
        (Some(false), "rolled-back", true, true, (true, "omitted")),
    ] {
        let mut fields = json!({"name":"bay", "label":label, "description":"", "device_type":1});
        if let Some(value) = enabled {
            fields["enabled"] = json!(value);
        }
        let snapshot = SourceSnapshot {
            coverage: None,
            records: vec![provider_record(
                provider,
                InventoryEntity::DeviceBayTemplate,
                "1",
                fields,
            )],
        };
        let config = mapping_for(&snapshot);
        let batch = target.prepare(&config, &snapshot)?;
        let job_id = Uuid::now_v7();
        sqlx::query("INSERT INTO sync_job(id,provider_id,mapping_version,status,started_at,stats) VALUES ($1,$2,$3,'running',now(),'{}')")
            .bind(job_id).bind(provider).bind(config.reference.to_string()).execute(&pool).await?;
        let policy = std::sync::Arc::new(DeviceBayTemplateAccess {
            template,
            fail_after_write: fail,
            ownership_calls: std::sync::atomic::AtomicUsize::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let result = target
            .commit_pull(CommitPull {
                job: SyncJob {
                    id: job_id,
                    provider_id: provider,
                    mapping_version: config.reference.to_string(),
                    status: SyncStatus::Running,
                    started_at: Some(chrono::Utc::now()),
                    finished_at: None,
                    stats: json!({}),
                },
                batch,
                authorisation: policy.clone(),
            })
            .await;
        if fail {
            assert!(matches!(result, Err(AppError::Forbidden(_))));
        } else {
            result?;
        }
        let seen = policy.seen.lock().expect("mutation observations");
        assert_eq!(seen.first(), Some(&(previous.0, previous.1.to_owned())));
        assert_eq!(
            seen.last(),
            Some(&(enabled.unwrap_or(previous.0), label.to_owned()))
        );
        drop(seen);
        let stored: (bool, String) =
            sqlx::query_as("SELECT enabled,label FROM dcim_devicebaytemplate WHERE id=$1")
                .bind(template)
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            stored,
            (expected, if fail { previous.1 } else { label }.to_owned())
        );
        let status: String = sqlx::query_scalar("SELECT status FROM sync_job WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(status, if fail { "running" } else { "completed" });
    }
    Ok(())
}

struct InterfaceReferenceAccess {
    interface: Uuid,
    device: Uuid,
    deny: bool,
}

struct InterfaceNameAccess {
    interface: Uuid,
    denied_name: &'static str,
    seen: std::sync::Mutex<Vec<String>>,
}

impl InventoryCommitAuthorisation for InterfaceNameAccess {
    fn ensure_owned(&self) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_mutation_access(
        &self,
        _action: crate::contexts::access_control::domain::Action,
        _id: Uuid,
        _attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_reference_access(
        &self,
        id: Uuid,
        attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        assert_eq!(id, self.interface);
        assert_eq!(attributes["object_type"], "dcim.interface");
        let name = attributes["name"]
            .as_str()
            .expect("stored interface name")
            .to_owned();
        self.seen
            .lock()
            .expect("reference observations")
            .push(name.clone());
        Box::pin(async move {
            if name == self.denied_name {
                Err(AppError::Forbidden("synthetic reference denial".into()))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
#[ignore = "requires an explicitly owned PostgreSQL fixture with integrated inventory owners"]
async fn runtime_component_writer_denial_rolls_back_original_and_proposed() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let provider = Uuid::now_v7();
    let suffix = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    let seed = hardware_snapshot(Uuid::now_v7(), &format!("component-writer-{suffix}"));
    let ids = apply_snapshot(&target, &mut tx, &seed, &HashMap::new()).await?;
    let device = local_id(&ids, InventoryEntity::Device);
    let other_seed = hardware_snapshot(Uuid::now_v7(), &format!("component-writer-other-{suffix}"));
    let other_ids = apply_snapshot(&target, &mut tx, &other_seed, &HashMap::new()).await?;
    let other = local_id(&other_ids, InventoryEntity::Device);
    let interface = local_id(&ids, InventoryEntity::Interface);
    let item = Uuid::now_v7();
    let ct: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='interface'",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO dcim_inventoryitem(id,custom_field_data,device_id,name,label,part_id,serial,discovered,description,status,component_type_id,component_id,lft,rght,tree_id,level) VALUES ($1,'{}',$2,'item','','','',false,'','active',$3,$4,0,0,0,0)")
        .bind(item).bind(device).bind(ct).bind(interface).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES ($1,$2,'netbox','{}',false)")
        .bind(provider).bind(format!("component-writer-{suffix}")).execute(&mut *tx).await?;
    for (entity, id) in [
        (InventoryEntity::Device, device),
        (InventoryEntity::Interface, interface),
        (InventoryEntity::InventoryItem, item),
    ] {
        sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id) VALUES ($1,$2,'1',$3,$4)")
            .bind(provider).bind(entity.source_key()).bind(entity.target_key()).bind(id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO sync_inventory_binding(source_provider_id,source_entity,external_id,target_entity,local_id) VALUES ($1,$2,'2',$3,$4)")
        .bind(provider).bind(InventoryEntity::Device.source_key()).bind(InventoryEntity::Device.target_key()).bind(other).execute(&mut *tx).await?;
    let snapshot = SourceSnapshot {
        coverage: None,
        records: vec![
            provider_record(
                provider,
                InventoryEntity::Interface,
                "1",
                json!({"name":"changed", "device":1, "type":"1000base-t", "enabled":true, "mark_connected":false, "mgmt_only":false}),
            ),
            provider_record(
                provider,
                InventoryEntity::InventoryItem,
                "1",
                json!({"name":"item", "device":1, "label":"", "part_id":"", "serial":"", "discovered":false, "description":"", "status":"active", "component":{"id":1}, "component_type":"dcim.interface"}),
            ),
        ],
    };
    let config = mapping_for(&snapshot);
    let batch = target.prepare(&config, &snapshot)?;
    let job_id = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_job(id,provider_id,mapping_version,status,started_at,stats) VALUES ($1,$2,$3,'running',now(),'{}')")
        .bind(job_id).bind(provider).bind(config.reference.to_string()).execute(&mut *tx).await?;
    tx.commit().await?;
    {
        use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::inventory_item_repository::{InventoryItemRepository, NewInventoryItem, SqlxInventoryItemRepository, UpdateInventoryItem};
        let repository = SqlxInventoryItemRepository::new(pool.clone());
        let native = |device_id, name: &str, component_id: Option<Uuid>| NewInventoryItem {
            id: Uuid::now_v7(),
            device_id,
            parent_id: None,
            role_id: None,
            manufacturer_id: None,
            name: name.into(),
            label: None,
            part_id: None,
            serial: None,
            asset_tag: None,
            discovered: None,
            description: None,
            status: None,
            component_type_id: component_id.map(|_| ct),
            component_id,
        };
        repository
            .create(&native(device, "native-positive", Some(interface)))
            .await?;
        let wrong = native(other, "native-negative", Some(interface));
        assert!(matches!(
            repository.create(&wrong).await,
            Err(AppError::Validation(_))
        ));
        let unbound = native(other, "native-update", None);
        repository.create(&unbound).await?;
        let changes = UpdateInventoryItem {
            parent_id: None,
            role_id: None,
            manufacturer_id: None,
            name: None,
            label: None,
            part_id: None,
            serial: None,
            asset_tag: None,
            discovered: None,
            description: None,
            status: None,
            component_type_id: Some(ct),
            component_id: Some(interface),
        };
        assert!(matches!(
            repository.update(unbound.id, &changes).await,
            Err(AppError::Validation(_))
        ));
        let persisted = repository
            .get_by_id(unbound.id)
            .await?
            .ok_or("native update item missing")?;
        assert!(persisted.component_id.is_none());
        let bulk = repository
            .bulk_create(&[
                native(device, "native-bulk-positive", Some(interface)),
                native(other, "native-bulk-negative", Some(interface)),
            ])
            .await?;
        assert_eq!(bulk.created.len(), 1);
        assert_eq!(bulk.errors.len(), 1);
    }
    for denied_name in ["eth0", "changed"] {
        let policy = std::sync::Arc::new(InterfaceNameAccess {
            interface,
            denied_name,
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let result = target
            .commit_pull(CommitPull {
                job: SyncJob {
                    id: job_id,
                    provider_id: provider,
                    mapping_version: config.reference.to_string(),
                    status: SyncStatus::Running,
                    started_at: Some(chrono::Utc::now()),
                    finished_at: None,
                    stats: json!({}),
                },
                batch: batch.clone(),
                authorisation: policy.clone(),
            })
            .await;
        assert!(matches!(result, Err(AppError::Forbidden(_))));
        let seen = policy.seen.lock().expect("reference observations");
        assert_eq!(seen.first().map(String::as_str), Some("eth0"));
        if denied_name == "changed" {
            assert_eq!(seen.last().map(String::as_str), Some("changed"));
        }
        drop(seen);
        let stored: (String, Uuid) =
            sqlx::query_as("SELECT name,device_id FROM dcim_interface WHERE id=$1")
                .bind(interface)
                .fetch_one(&pool)
                .await?;
        assert_eq!(stored, ("eth0".into(), device));
        let binding: (Uuid, Uuid) =
            sqlx::query_as("SELECT device_id,component_id FROM dcim_inventoryitem WHERE id=$1")
                .bind(item)
                .fetch_one(&pool)
                .await?;
        assert_eq!(binding, (device, interface));
        let status: String = sqlx::query_scalar("SELECT status FROM sync_job WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(status, "running");
    }
    let clear_snapshot = SourceSnapshot {
        coverage: None,
        records: vec![provider_record(
            provider,
            InventoryEntity::InventoryItem,
            "1",
            json!({"name":"item", "device":2, "label":"", "part_id":"", "serial":"", "discovered":false, "description":"", "status":"active", "component":null}),
        )],
    };
    let clear_config = mapping_for(&clear_snapshot);
    let clear_batch = target.prepare(&clear_config, &clear_snapshot)?;
    let policy = std::sync::Arc::new(InterfaceNameAccess {
        interface,
        denied_name: "eth0",
        seen: std::sync::Mutex::new(Vec::new()),
    });
    target
        .commit_pull(CommitPull {
            job: SyncJob {
                id: job_id,
                provider_id: provider,
                mapping_version: clear_config.reference.to_string(),
                status: SyncStatus::Running,
                started_at: Some(chrono::Utc::now()),
                finished_at: None,
                stats: json!({}),
            },
            batch: clear_batch,
            authorisation: policy.clone(),
        })
        .await?;
    assert!(
        policy
            .seen
            .lock()
            .expect("reference observations")
            .is_empty()
    );
    let cleared: (Uuid, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT device_id,component_id,component_type_id FROM dcim_inventoryitem WHERE id=$1",
    )
    .bind(item)
    .fetch_one(&pool)
    .await?;
    assert_eq!(cleared, (other, None, None));
    Ok(())
}

impl InventoryCommitAuthorisation for InterfaceReferenceAccess {
    fn ensure_mutation_access(
        &self,
        _action: crate::contexts::access_control::domain::Action,
        _id: Uuid,
        _attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_owned(&self) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn ensure_reference_access(
        &self,
        id: Uuid,
        attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        assert_eq!(id, self.interface);
        assert_eq!(attributes["object_type"], "dcim.interface");
        assert_eq!(attributes["device_id"], json!(self.device));
        assert_eq!(attributes["name"], "eth0");
        Box::pin(async move {
            if self.deny {
                Err(AppError::Forbidden("synthetic reference denial".into()))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
#[ignore = "requires an explicitly owned PostgreSQL fixture with integrated inventory owners"]
async fn runtime_inventory_component_authorization_and_device_graph() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let suffix = Uuid::now_v7();
    let first = apply_snapshot(
        &target,
        &mut tx,
        &hardware_snapshot(Uuid::now_v7(), &format!("component-a-{suffix}")),
        &HashMap::new(),
    )
    .await?;
    let second = apply_snapshot(
        &target,
        &mut tx,
        &hardware_snapshot(Uuid::now_v7(), &format!("component-b-{suffix}")),
        &HashMap::new(),
    )
    .await?;
    let device = local_id(&first, InventoryEntity::Device);
    let other = local_id(&second, InventoryEntity::Device);
    let interface = local_id(&first, InventoryEntity::Interface);
    let other_interface = local_id(&second, InventoryEntity::Interface);
    sqlx::query("UPDATE dcim_interface SET name='destination-eth0' WHERE id=$1")
        .bind(other_interface)
        .execute(&mut *tx)
        .await?;
    let item = Uuid::now_v7();
    let ct: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='interface'",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO dcim_inventoryitem(id,custom_field_data,device_id,name,label,part_id,serial,discovered,description,status,component_type_id,component_id,lft,rght,tree_id,level) VALUES ($1,'{}',$2,'item','','','',false,'','active',$3,$4,0,0,0,0)")
        .bind(item).bind(device).bind(ct).bind(interface).execute(&mut *tx).await?;
    let plan = [template_dependency_plan(
        InventoryEntity::InventoryItem,
        item,
    )];
    let allowed = InterfaceReferenceAccess {
        interface,
        device,
        deny: false,
    };
    let denied = InterfaceReferenceAccess {
        interface,
        device,
        deny: true,
    };
    let together = [
        template_dependency_plan(InventoryEntity::InventoryItem, item),
        template_dependency_plan(InventoryEntity::Interface, interface),
    ];
    template_validation::authorize_mutations(&mut tx, &together, &HashSet::new(), &allowed).await?;
    sqlx::query("SAVEPOINT component_move_together")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE dcim_inventoryitem SET device_id=$2 WHERE id=$1")
        .bind(item)
        .bind(other)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE dcim_interface SET device_id=$2 WHERE id=$1")
        .bind(interface)
        .bind(other)
        .execute(&mut *tx)
        .await?;
    template_validation::validate(
        &mut tx,
        &together,
        &InterfaceReferenceAccess {
            interface,
            device: other,
            deny: false,
        },
    )
    .await?;
    sqlx::query("ROLLBACK TO SAVEPOINT component_move_together")
        .execute(&mut *tx)
        .await?;
    let changed = HashSet::from([(InventoryEntity::InventoryItem, item)]);
    assert!(matches!(
        template_validation::authorize_mutations(&mut tx, &plan, &changed, &denied).await,
        Err(AppError::Forbidden(_))
    ));
    let mut clear = template_dependency_plan(InventoryEntity::InventoryItem, item);
    for (column, value) in [
        ("device_id", Some(other)),
        ("component_id", None),
        ("component_type_id", None),
    ] {
        clear.columns.push(PlannedColumn {
            column,
            value: ColumnValue::Uuid(value),
            updatable: true,
        });
    }
    template_validation::authorize_mutations(&mut tx, &[clear.clone()], &changed, &denied).await?;
    sqlx::query("UPDATE dcim_inventoryitem SET device_id=$2,component_id=NULL,component_type_id=NULL WHERE id=$1")
        .bind(item).bind(other).execute(&mut *tx).await?;
    template_validation::validate(&mut tx, &[clear], &denied).await?;
    let cleared: (Uuid, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT device_id,component_id,component_type_id FROM dcim_inventoryitem WHERE id=$1",
    )
    .bind(item)
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(cleared, (other, None, None));
    sqlx::query("UPDATE dcim_inventoryitem SET device_id=$2,component_type_id=$3,component_id=$4 WHERE id=$1")
        .bind(item).bind(device).bind(ct).bind(interface).execute(&mut *tx).await?;
    template_validation::validate(&mut tx, &plan, &allowed).await?;
    assert!(matches!(
        template_validation::validate(&mut tx, &plan, &denied).await,
        Err(AppError::Forbidden(_))
    ));
    let wrong_ct: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='device'",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE dcim_inventoryitem SET component_type_id=$2 WHERE id=$1")
        .bind(item)
        .bind(wrong_ct)
        .execute(&mut *tx)
        .await?;
    assert!(matches!(
        template_validation::validate(&mut tx, &plan, &allowed).await,
        Err(AppError::Validation(_))
    ));
    sqlx::query("UPDATE dcim_inventoryitem SET component_type_id=$2 WHERE id=$1")
        .bind(item)
        .bind(ct)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE dcim_inventoryitem SET device_id=$2 WHERE id=$1")
        .bind(item)
        .bind(other)
        .execute(&mut *tx)
        .await?;
    assert!(matches!(
        template_validation::validate(&mut tx, &plan, &allowed).await,
        Err(AppError::Validation(_))
    ));
    sqlx::query("UPDATE dcim_inventoryitem SET device_id=$2 WHERE id=$1")
        .bind(item)
        .bind(device)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE dcim_interface SET device_id=$2 WHERE id=$1")
        .bind(interface)
        .bind(other)
        .execute(&mut *tx)
        .await?;
    let reverse = [template_dependency_plan(
        InventoryEntity::Interface,
        interface,
    )];
    let moved = InterfaceReferenceAccess {
        interface,
        device: other,
        deny: false,
    };
    let moved_denied = InterfaceReferenceAccess {
        interface,
        device: other,
        deny: true,
    };
    assert!(matches!(
        template_validation::validate(&mut tx, &reverse, &moved_denied).await,
        Err(AppError::Forbidden(_))
    ));
    assert!(matches!(
        template_validation::validate(&mut tx, &reverse, &moved).await,
        Err(AppError::Validation(_))
    ));
    tx.rollback().await?;
    Ok(())
}

impl InventoryCommitAuthorisation for TemplateReferenceAccess {
    fn ensure_mutation_access(
        &self,
        _action: crate::contexts::access_control::domain::Action,
        _id: Uuid,
        _attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_owned(&self) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn ensure_reference_access(
        &self,
        id: Uuid,
        _attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            if self.denied == Some(id) {
                Err(AppError::Forbidden("synthetic reference denial".into()))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture; verifies component-only previous/current owner counts"]
async fn component_only_replay_recounts_previous_and_current_device_owners() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let suffix = Uuid::now_v7();
    let first = apply_snapshot(
        &target,
        &mut tx,
        &hardware_snapshot(Uuid::now_v7(), &format!("counter-first-{suffix}")),
        &HashMap::new(),
    )
    .await?;
    let second = apply_snapshot(
        &target,
        &mut tx,
        &hardware_snapshot(Uuid::now_v7(), &format!("counter-second-{suffix}")),
        &HashMap::new(),
    )
    .await?;
    let source = local_id(&first, InventoryEntity::Device);
    let destination = local_id(&second, InventoryEntity::Device);
    let interface = local_id(&first, InventoryEntity::Interface);
    let other_interface = local_id(&second, InventoryEntity::Interface);
    sqlx::query("UPDATE dcim_interface SET name='destination-eth0' WHERE id=$1")
        .bind(other_interface)
        .execute(&mut *tx)
        .await?;
    let planned = vec![template_dependency_plan(
        InventoryEntity::Interface,
        interface,
    )];
    let previous = counters::device_owners(&mut tx, &planned).await?;
    assert_eq!(previous, vec![source]);
    sqlx::query("UPDATE dcim_interface SET device_id=$2 WHERE id=$1")
        .bind(interface)
        .bind(destination)
        .execute(&mut *tx)
        .await?;
    recompute_counts(&mut tx, &planned, &previous).await?;
    for (device, expected) in [(source, 1_i64), (destination, 3_i64)] {
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT interface_count, (SELECT count(*) FROM dcim_interface WHERE device_id=$1) FROM dcim_device WHERE id=$1",
        )
        .bind(device)
        .fetch_one(&mut *tx)
        .await?;
        assert_eq!(counts, (expected, expected));
    }
    tx.rollback().await?;
    Ok(())
}

fn template_dependency_plan(entity: InventoryEntity, id: Uuid) -> PlannedRow {
    PlannedRow {
        entity,
        source_entity: entity.source_key().into(),
        external_id: "1".into(),
        local_id: id,
        source_timestamps: SourceTimestamps::default(),
        columns: vec![],
        reference_targets: HashMap::new(),
        relations: vec![],
        junctions: vec![],
        selectors: Vec::new(),
    }
}

struct FrontMutationAccess(Option<Uuid>);
impl InventoryCommitAuthorisation for FrontMutationAccess {
    fn ensure_owned(&self) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_reference_access(
        &self,
        _id: Uuid,
        _attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn ensure_mutation_access(
        &self,
        action: crate::contexts::access_control::domain::Action,
        id: Uuid,
        attributes: Value,
    ) -> futures_util::future::BoxFuture<'_, Result<(), AppError>> {
        let denied = self.0 == Some(id)
            && action == crate::contexts::access_control::domain::Action::Update
            && attributes["object_type"] == "dcim.frontporttemplate";
        Box::pin(async move {
            if denied {
                Err(AppError::Forbidden("synthetic front update denial".into()))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
#[ignore = "requires an explicitly owned PostgreSQL fixture with integrated template and authorization owners"]
async fn positioned_mapping_mutation_requires_old_and_new_front_update() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let snapshot = hardware_snapshot(Uuid::now_v7(), &format!("mapping-owner-{}", Uuid::now_v7()));
    let ids = apply_snapshot(&target, &mut tx, &snapshot, &HashMap::new()).await?;
    let device = local_id(&ids, InventoryEntity::DeviceType);
    let front_one = Uuid::now_v7();
    let front_two = Uuid::now_v7();
    let rear = Uuid::now_v7();
    let mapping = Uuid::now_v7();
    for (id, name) in [(front_one, "front-one"), (front_two, "front-two")] {
        sqlx::query("INSERT INTO dcim_frontporttemplate(id,name,label,description,type,color,positions,device_type_id) VALUES ($1,$2,'','','8p8c','',2,$3)").bind(id).bind(name).bind(device).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO dcim_rearporttemplate(id,name,label,description,type,color,positions,device_type_id) VALUES ($1,'rear','','','8p8c','',2,$2)").bind(rear).bind(device).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO dcim_porttemplatemapping(id,front_port_id,rear_port_id,front_port_position,rear_port_position,device_type_id) VALUES ($1,$2,$3,1,1,$4)").bind(mapping).bind(front_one).bind(rear).bind(device).execute(&mut *tx).await?;
    let mut plan = template_dependency_plan(InventoryEntity::PortTemplateMapping, mapping);
    plan.columns = vec![
        PlannedColumn {
            column: "front_port_id",
            value: ColumnValue::Uuid(Some(front_one)),
            updatable: true,
        },
        PlannedColumn {
            column: "rear_port_id",
            value: ColumnValue::Uuid(Some(rear)),
            updatable: true,
        },
        PlannedColumn {
            column: "device_type_id",
            value: ColumnValue::Uuid(Some(device)),
            updatable: true,
        },
        PlannedColumn {
            column: "module_type_id",
            value: ColumnValue::Uuid(None),
            updatable: true,
        },
        PlannedColumn {
            column: "front_port_position",
            value: ColumnValue::Scalar(PgType::SmallInt, InventoryValue::Integer { value: 2 }),
            updatable: true,
        },
        PlannedColumn {
            column: "rear_port_position",
            value: ColumnValue::Scalar(PgType::SmallInt, InventoryValue::Integer { value: 1 }),
            updatable: true,
        },
    ];
    let changed = HashSet::from([(InventoryEntity::PortTemplateMapping, mapping)]);
    let bindings_before: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_inventory_binding")
        .fetch_one(&mut *tx)
        .await?;
    assert!(matches!(
        template_validation::authorize_mutations(
            &mut tx,
            &[plan.clone()],
            &changed,
            &FrontMutationAccess(Some(front_one))
        )
        .await,
        Err(AppError::Forbidden(_))
    ));
    let state: (Uuid, i16) = sqlx::query_as(
        "SELECT front_port_id,front_port_position FROM dcim_porttemplatemapping WHERE id=$1",
    )
    .bind(mapping)
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(state, (front_one, 1));
    let bindings_after: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_inventory_binding")
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(bindings_before, bindings_after);
    template_validation::authorize_mutations(
        &mut tx,
        &[plan.clone()],
        &changed,
        &FrontMutationAccess(None),
    )
    .await?;
    apply_planned_rows(&mut tx, &[plan.clone()]).await?;
    let state: (Uuid, i16) = sqlx::query_as(
        "SELECT front_port_id,front_port_position FROM dcim_porttemplatemapping WHERE id=$1",
    )
    .bind(mapping)
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(state, (front_one, 2));
    plan.columns[0].value = ColumnValue::Uuid(Some(front_two));
    for denied in [front_one, front_two] {
        assert!(matches!(
            template_validation::authorize_mutations(
                &mut tx,
                &[plan.clone()],
                &changed,
                &FrontMutationAccess(Some(denied))
            )
            .await,
            Err(AppError::Forbidden(_))
        ));
    }
    template_validation::authorize_mutations(
        &mut tx,
        &[plan.clone()],
        &changed,
        &FrontMutationAccess(None),
    )
    .await?;
    apply_planned_rows(&mut tx, &[plan]).await?;
    let state: (Uuid, i16) = sqlx::query_as(
        "SELECT front_port_id,front_port_position FROM dcim_porttemplatemapping WHERE id=$1",
    )
    .bind(mapping)
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(state, (front_two, 2));
    tx.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an explicitly owned PostgreSQL fixture with integrated template and authorization owners"]
async fn retained_template_dependencies_are_authorized_and_revalidated() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let suffix = Uuid::now_v7();
    let one = hardware_snapshot(Uuid::now_v7(), &format!("dependency-a-{suffix}"));
    let two = hardware_snapshot(Uuid::now_v7(), &format!("dependency-b-{suffix}"));
    let first = apply_snapshot(&target, &mut tx, &one, &HashMap::new()).await?;
    let second = apply_snapshot(&target, &mut tx, &two, &HashMap::new()).await?;
    let device_a = local_id(&first, InventoryEntity::DeviceType);
    let device_b = local_id(&second, InventoryEntity::DeviceType);
    let cooling = local_id(&first, InventoryEntity::CoolingIntakeTemplate);
    let outflow = local_id(&first, InventoryEntity::CoolingOutflowTemplate);
    sqlx::query("UPDATE dcim_coolingoutflowtemplate SET cooling_intake_id=NULL WHERE id=$1")
        .bind(outflow)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE dcim_coolingintaketemplate SET device_type_id=$2,module_type_id=NULL WHERE id=$1",
    )
    .bind(cooling)
    .bind(device_a)
    .execute(&mut *tx)
    .await?;
    let ct: Uuid = sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='coolingintaketemplate'")
        .fetch_one(&mut *tx).await?;
    let item = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_inventoryitemtemplate(id,name,label,description,part_id,lft,rght,tree_id,level,device_type_id,component_id,component_type_id) VALUES ($1,'retained-item','','','',0,0,0,0,$2,$3,$4)")
        .bind(item).bind(device_a).bind(cooling).bind(ct).execute(&mut *tx).await?;
    sqlx::query("SAVEPOINT cooling_change")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE dcim_coolingintaketemplate SET device_type_id=$2 WHERE id=$1")
        .bind(cooling)
        .bind(device_b)
        .execute(&mut *tx)
        .await?;
    let wrong_type = [template_dependency_plan(
        InventoryEntity::CoolingOutflowTemplate,
        cooling,
    )];
    template_validation::validate(
        &mut tx,
        &wrong_type,
        &TemplateReferenceAccess { denied: Some(item) },
    )
    .await?;
    let changed = [template_dependency_plan(
        InventoryEntity::CoolingIntakeTemplate,
        cooling,
    )];
    let denied = template_validation::validate(
        &mut tx,
        &changed,
        &TemplateReferenceAccess { denied: Some(item) },
    )
    .await;
    assert!(matches!(denied, Err(AppError::Forbidden(_))));
    let invalid =
        template_validation::validate(&mut tx, &changed, &TemplateReferenceAccess { denied: None })
            .await;
    assert!(matches!(invalid, Err(AppError::Validation(_))));
    sqlx::query("ROLLBACK TO SAVEPOINT cooling_change")
        .execute(&mut *tx)
        .await?;
    let retained: Uuid =
        sqlx::query_scalar("SELECT device_type_id FROM dcim_coolingintaketemplate WHERE id=$1")
            .bind(cooling)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(retained, device_a);

    sqlx::query("UPDATE dcim_devicetype SET subdevice_role='parent' WHERE id=$1")
        .bind(device_a)
        .execute(&mut *tx)
        .await?;
    let bay = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_devicebaytemplate(id,name,label,description,device_type_id) VALUES ($1,'retained-bay','','',$2)")
        .bind(bay).bind(device_a).execute(&mut *tx).await?;
    sqlx::query("SAVEPOINT role_change")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE dcim_devicetype SET subdevice_role='child' WHERE id=$1")
        .bind(device_a)
        .execute(&mut *tx)
        .await?;
    let changed = [template_dependency_plan(
        InventoryEntity::DeviceType,
        device_a,
    )];
    let denied = template_validation::validate(
        &mut tx,
        &changed,
        &TemplateReferenceAccess { denied: Some(bay) },
    )
    .await;
    assert!(matches!(denied, Err(AppError::Forbidden(_))));
    let invalid =
        template_validation::validate(&mut tx, &changed, &TemplateReferenceAccess { denied: None })
            .await;
    assert!(matches!(invalid, Err(AppError::Validation(_))));
    sqlx::query("ROLLBACK TO SAVEPOINT role_change")
        .execute(&mut *tx)
        .await?;

    sqlx::query("SAVEPOINT duplicate_root")
        .execute(&mut *tx)
        .await?;
    let duplicate = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_inventoryitemtemplate(id,name,label,description,part_id,lft,rght,tree_id,level,device_type_id) VALUES ($1,'retained-item','','','',0,0,0,0,$2)")
        .bind(duplicate).bind(device_a).execute(&mut *tx).await?;
    let changed = [template_dependency_plan(
        InventoryEntity::InventoryItemTemplate,
        duplicate,
    )];
    let denied = template_validation::validate(
        &mut tx,
        &changed,
        &TemplateReferenceAccess { denied: Some(item) },
    )
    .await;
    assert!(matches!(denied, Err(AppError::Forbidden(_))));
    let conflict =
        template_validation::validate(&mut tx, &changed, &TemplateReferenceAccess { denied: None })
            .await;
    assert!(matches!(conflict, Err(AppError::Conflict(_))));
    sqlx::query("ROLLBACK TO SAVEPOINT duplicate_root")
        .execute(&mut *tx)
        .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dcim_inventoryitemtemplate WHERE id=$1)")
            .bind(duplicate)
            .fetch_one(&mut *tx)
            .await?;
    assert!(!exists);
    tx.rollback().await?;
    Ok(())
}

#[test]
fn existing_module_update_query_preserves_unmapped_and_insert_only_columns() {
    use sqlx::Execute;

    let mut plan = PlannedRow {
        entity: InventoryEntity::Module,
        source_entity: InventoryEntity::Module.source_key().into(),
        external_id: "module-update-query".into(),
        local_id: Uuid::now_v7(),
        source_timestamps: SourceTimestamps::default(),
        columns: vec![
            PlannedColumn {
                column: "serial",
                value: ColumnValue::Scalar(
                    PgType::Text,
                    InventoryValue::Text {
                        value: "updated serial".into(),
                    },
                ),
                updatable: true,
            },
            PlannedColumn {
                column: "comments",
                value: ColumnValue::Literal("''"),
                updatable: false,
            },
        ],
        relations: Vec::new(),
        junctions: Vec::new(),
        selectors: Vec::new(),
        reference_targets: HashMap::new(),
    };
    let mut query = module_update_query(&plan)
        .expect("supported planned values")
        .expect("serial update");
    assert_eq!(
        query.build().sql(),
        "UPDATE dcim_module SET serial = $1 WHERE id = $2"
    );
    drop(query);
    plan.columns[0].updatable = false;
    assert!(
        module_update_query(&plan)
            .expect("insert-only defaults remain valid")
            .is_none()
    );
}

#[test]
fn hardware_write_order_resolves_forward_dependencies_and_refuses_cycles() {
    fn row(entity: InventoryEntity, id: Uuid, column: &'static str, target: Uuid) -> PlannedRow {
        PlannedRow {
            entity,
            source_entity: entity.source_key().into(),
            external_id: "1".into(),
            local_id: id,
            source_timestamps: SourceTimestamps::default(),
            columns: vec![PlannedColumn {
                column,
                value: ColumnValue::Uuid(Some(target)),
                updatable: true,
            }],
            relations: Vec::new(),
            junctions: Vec::new(),
            selectors: Vec::new(),
            reference_targets: HashMap::new(),
        }
    }
    let source = Uuid::now_v7();
    let feed = Uuid::now_v7();
    let plans = [
        row(
            InventoryEntity::CoolingFeed,
            feed,
            "cooling_source_id",
            source,
        ),
        row(
            InventoryEntity::CoolingSource,
            source,
            "site_id",
            Uuid::now_v7(),
        ),
    ];
    let order = hardware_write_order(&plans).expect("resolvable dependency");
    assert_eq!(
        order.iter().map(|row| row.local_id).collect::<Vec<_>>(),
        vec![source, feed]
    );
    let cyclic = [
        row(
            InventoryEntity::CoolingIntake,
            source,
            "cooling_outflow_id",
            feed,
        ),
        row(
            InventoryEntity::CoolingOutflow,
            feed,
            "cooling_intake_id",
            source,
        ),
    ];
    assert!(hardware_write_order(&cyclic).is_err());
}

fn termination_snapshot() -> SourceSnapshot {
    let provider = Uuid::from_u128(78);
    let mut snapshot = hardware_snapshot(provider, "internal-cable-relation");
    for (id, end, connector, positions) in [
        ("1", "A", json!(2), json!([1024, 1, 1])),
        ("2", "B", Value::Null, Value::Null),
    ] {
        snapshot.records.push(provider_record(
            provider,
            InventoryEntity::CableTermination,
            id,
            json!({"cable": 1, "cable_end": end, "connector": connector, "positions": positions,
                "termination_type": "dcim.interface", "termination": id,
                "created": "2026-10-28T10:00:00Z", "last_updated": "2026-10-28T11:00:00Z"}),
        ));
    }
    snapshot
}

fn termination_mut<'a>(snapshot: &'a mut SourceSnapshot, id: &str) -> &'a mut SourceRecord {
    snapshot
        .records
        .iter_mut()
        .find(|record| {
            record.identity.entity.as_str() == InventoryEntity::CableTermination.source_key()
                && record.identity.external_id == id
        })
        .expect("source termination")
}

#[test]
fn internal_cable_rows_project_only_to_canonical_interface_columns() {
    let source = termination_snapshot();
    let assessment = assess_preparation(&mapping_for(&source), &source);
    assert!(assessment.analysis_complete, "{:?}", assessment.findings);
    let batch = assessment
        .batch
        .expect("source-only internal relation prepares");
    assert_eq!(batch.rows.len() + 2, source.records.len());
    assert_eq!(batch.cable_sides.len(), 2);
    assert!(
        !batch
            .rows
            .iter()
            .any(|row| row.entity == InventoryEntity::CableTermination)
    );
    let a = batch
        .rows
        .iter()
        .find(|row| row.entity == InventoryEntity::Interface && row.external_id == "1")
        .unwrap();
    let b = batch
        .rows
        .iter()
        .find(|row| row.entity == InventoryEntity::Interface && row.external_id == "2")
        .unwrap();
    assert_eq!(
        a.scalars[&InventoryEntity::Interface.field("cable_connector").unwrap()],
        InventoryValue::Integer { value: 2 }
    );
    assert_eq!(
        a.scalars[&InventoryEntity::Interface.field("cable_positions").unwrap()],
        InventoryValue::Json {
            value: json!([1024, 1, 1])
        }
    );
    assert_eq!(
        b.scalars[&InventoryEntity::Interface.field("cable_connector").unwrap()],
        InventoryValue::Null
    );
    assert_eq!(
        b.scalars[&InventoryEntity::Interface.field("cable_positions").unwrap()],
        InventoryValue::Null
    );
}

#[test]
fn internal_cable_projection_preserves_empty_null_and_omission() {
    for (positions, expected) in [
        (Value::Null, InventoryValue::Null),
        (json!([]), InventoryValue::Json { value: json!([]) }),
    ] {
        let mut source = termination_snapshot();
        termination_mut(&mut source, "1")
            .fields
            .insert("positions".into(), positions);
        let assessment = assess_preparation(&mapping_for(&source), &source);
        let batch = assessment
            .batch
            .unwrap_or_else(|| panic!("{:?}", assessment.findings));
        let row = batch
            .rows
            .iter()
            .find(|row| row.entity == InventoryEntity::Interface && row.external_id == "1")
            .unwrap();
        assert_eq!(
            row.scalars[&InventoryEntity::Interface.field("cable_positions").unwrap()],
            expected
        );
    }
    let mut source = termination_snapshot();
    source.records.retain(|record| {
        record.identity.entity.as_str() != InventoryEntity::CableTermination.source_key()
    });
    let batch = assess_preparation(&mapping_for(&source), &source)
        .batch
        .expect("omitted relation");
    for row in batch
        .rows
        .iter()
        .filter(|row| row.entity == InventoryEntity::Interface)
    {
        assert!(
            !row.scalars
                .contains_key(&InventoryEntity::Interface.field("cable_positions").unwrap())
        );
        assert!(
            !row.scalars
                .contains_key(&InventoryEntity::Interface.field("cable_connector").unwrap())
        );
    }
    assert!(batch.cable_sides.is_empty());
}

#[test]
fn internal_cable_projection_refuses_missing_ambiguous_or_unrepresentable_values() {
    for (field, value) in [
        ("termination_type", json!("dcim.frontport")),
        ("termination", json!(99)),
        ("termination", json!({"id": 1, "object_type": "dcim.cable"})),
        (
            "termination",
            json!({"id": 1, "source_provider_id": Uuid::from_u128(99)}),
        ),
        ("cable", json!(99)),
        ("cable_end", json!("B")),
        ("connector", json!(0)),
        ("connector", json!(257)),
        ("positions", json!([0])),
        ("positions", json!([1025])),
        ("positions", json!([true])),
        ("positions", json!([null])),
        ("positions", json!("1")),
        ("unknown", json!([1])),
    ] {
        let mut source = termination_snapshot();
        termination_mut(&mut source, "1")
            .fields
            .insert(field.into(), value);
        let assessment = assess_preparation(&mapping_for(&source), &source);
        assert!(
            assessment.batch.is_none(),
            "{field}: {:?}",
            assessment.findings
        );
    }
    for field in [
        "cable",
        "cable_end",
        "termination",
        "termination_type",
        "connector",
        "positions",
    ] {
        let mut source = termination_snapshot();
        termination_mut(&mut source, "1").fields.remove(field);
        assert!(
            assess_preparation(&mapping_for(&source), &source)
                .batch
                .is_none(),
            "missing {field}"
        );
    }
    let mut source = termination_snapshot();
    let mut duplicate = termination_mut(&mut source, "1").clone();
    duplicate.identity.external_id = "3".into();
    source.records.push(duplicate);
    assert!(
        assess_preparation(&mapping_for(&source), &source)
            .batch
            .is_none()
    );
}

#[test]
fn internal_cable_projection_retains_endpoint_multiplicity_but_not_duplicate_connectors() {
    let mut source = termination_snapshot();
    let mut endpoint = source
        .records
        .iter()
        .find(|record| {
            record.identity.entity.as_str() == "dcim.interfaces"
                && record.identity.external_id == "1"
        })
        .unwrap()
        .clone();
    endpoint.identity.external_id = "3".into();
    endpoint.fields.insert("name".into(), json!("eth2"));
    source.records.push(endpoint);
    let mut relation = termination_mut(&mut source, "1").clone();
    relation.identity.external_id = "3".into();
    relation.fields.insert("termination".into(), json!(3));
    relation.fields.insert("connector".into(), json!(3));
    source.records.push(relation);
    let assessment = assess_preparation(&mapping_for(&source), &source);
    assert!(assessment.batch.is_some(), "{:?}", assessment.findings);
    termination_mut(&mut source, "3")
        .fields
        .insert("connector".into(), json!(2));
    assert!(
        assess_preparation(&mapping_for(&source), &source)
            .batch
            .is_none()
    );
}

#[test]
fn internal_cable_relation_cannot_be_hidden_by_an_interface_mapping() {
    let source = termination_snapshot();
    let mut config = mapping_for(&source);
    let owner = config
        .entity_mappings
        .iter()
        .find(|mapping| mapping.source_entity == "dcim.interfaces")
        .unwrap()
        .id;
    config
        .field_mappings
        .iter_mut()
        .find(|mapping| mapping.entity_mapping_id == owner && mapping.target_field == "cable_end")
        .unwrap()
        .forward_transform_json = json!({"op": "literal", "args": ["B"]});
    assert!(assess_preparation(&config, &source).batch.is_none());

    let mut source = termination_snapshot();
    source
        .records
        .iter_mut()
        .find(|record| {
            record.identity.entity.as_str() == "dcim.interfaces"
                && record.identity.external_id == "1"
        })
        .unwrap()
        .fields
        .insert("cable_positions".into(), json!([1, 1024, 1]));
    assert!(
        assess_preparation(&mapping_for(&source), &source)
            .batch
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture; exercises internal source relation projection through the native Interface writer"]
async fn internal_cable_relation_persists_ordered_positions_and_explicit_clears() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    lock_hardware_graph(&mut tx).await?;
    let mut source = termination_snapshot();
    for record in &mut source.records {
        record.identity.source_provider_id = Uuid::from_u128(79);
    }
    let ids = apply_snapshot(&target, &mut tx, &source, &HashMap::new()).await?;
    let endpoint = ids[&("dcim.interfaces".into(), "1".into())];
    async fn read(
        tx: &mut Transaction<'_, Postgres>,
        endpoint: Uuid,
    ) -> Result<(Option<i16>, Option<Vec<i16>>), sqlx::Error> {
        sqlx::query_as::<_, (Option<i16>, Option<Vec<i16>>)>(
            "SELECT cable_connector,cable_positions FROM dcim_interface WHERE id=$1",
        )
        .bind(endpoint)
        .fetch_one(&mut **tx)
        .await
    }
    assert_eq!(
        read(&mut tx, endpoint).await?,
        (Some(2), Some(vec![1024, 1, 1]))
    );
    termination_mut(&mut source, "1")
        .fields
        .insert("positions".into(), json!([]));
    apply_snapshot(&target, &mut tx, &source, &ids).await?;
    assert_eq!(read(&mut tx, endpoint).await?, (Some(2), Some(vec![])));
    termination_mut(&mut source, "1")
        .fields
        .insert("positions".into(), Value::Null);
    termination_mut(&mut source, "1")
        .fields
        .insert("connector".into(), Value::Null);
    apply_snapshot(&target, &mut tx, &source, &ids).await?;
    assert_eq!(read(&mut tx, endpoint).await?, (None, None));
    assert!(
        !ids.keys()
            .any(|(entity, _)| entity == InventoryEntity::CableTermination.source_key())
    );
    tx.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture with integrated migrations; exercises snapshot planning and native SQL writers"]
async fn hardware_snapshot_persists_foreign_keys_and_all_three_bay_type_junctions() -> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    crate::storage::database::migrations::run_migrations(&pool).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let snapshot = hardware_snapshot(Uuid::now_v7(), &format!("hardware-{}", Uuid::now_v7()));
    // The same catalogue exposed to source discovery must advertise every row.
    for record in &snapshot.records {
        assert!(target.resource(&record.identity.entity).is_some());
    }
    let ids = apply_snapshot(&target, &mut tx, &snapshot, &HashMap::new()).await?;
    let rack: (Uuid, Uuid) =
        sqlx::query_as("SELECT group_id,location_id FROM dcim_rack WHERE id=$1")
            .bind(local_id(&ids, InventoryEntity::Rack))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        rack,
        (
            local_id(&ids, InventoryEntity::RackGroup),
            local_id(&ids, InventoryEntity::Location)
        )
    );
    let bundle: Uuid = sqlx::query_scalar("SELECT bundle_id FROM dcim_cable WHERE id=$1")
        .bind(local_id(&ids, InventoryEntity::Cable))
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(bundle, local_id(&ids, InventoryEntity::CableBundle));
    for (external_id, side) in [("1", "A"), ("2", "B")] {
        let endpoint = ids[&("dcim.interfaces".to_owned(), external_id.to_owned())];
        let persisted: (Uuid, String) =
            sqlx::query_as("SELECT cable_id,cable_end::text FROM dcim_interface WHERE id=$1")
                .bind(endpoint)
                .fetch_one(&mut *tx)
                .await?;
        assert_eq!(
            persisted,
            (local_id(&ids, InventoryEntity::Cable), side.to_owned())
        );
    }
    let cooling: (Uuid, Uuid) =
        sqlx::query_as("SELECT cooling_source_id,rack_id FROM dcim_coolingfeed WHERE id=$1")
            .bind(local_id(&ids, InventoryEntity::CoolingFeed))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        cooling,
        (
            local_id(&ids, InventoryEntity::CoolingSource),
            local_id(&ids, InventoryEntity::Rack)
        )
    );
    let parent: (Uuid, Uuid) =
        sqlx::query_as("SELECT cooling_intake_id,module_id FROM dcim_coolingoutflow WHERE id=$1")
            .bind(local_id(&ids, InventoryEntity::CoolingOutflow))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        parent,
        (
            local_id(&ids, InventoryEntity::CoolingIntake),
            local_id(&ids, InventoryEntity::Module)
        )
    );
    let template: Uuid =
        sqlx::query_scalar("SELECT cooling_intake_id FROM dcim_coolingoutflowtemplate WHERE id=$1")
            .bind(local_id(&ids, InventoryEntity::CoolingOutflowTemplate))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        template,
        local_id(&ids, InventoryEntity::CoolingIntakeTemplate)
    );
    let bay_type = local_id(&ids, InventoryEntity::ModuleBayType);
    for (query, owner) in [
        (
            "SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE modulebay_id=$1",
            InventoryEntity::ModuleBay,
        ),
        (
            "SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=$1",
            InventoryEntity::ModuleType,
        ),
        (
            "SELECT modulebaytype_id FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=$1",
            InventoryEntity::ModuleBayTemplate,
        ),
    ] {
        let persisted: Vec<Uuid> = sqlx::query_scalar(query)
            .bind(local_id(&ids, owner))
            .fetch_all(&mut *tx)
            .await?;
        assert_eq!(persisted, vec![bay_type]);
    }
    let date: chrono::NaiveDate =
        sqlx::query_scalar("SELECT end_of_life FROM dcim_moduletype WHERE id=$1")
            .bind(local_id(&ids, InventoryEntity::ModuleType))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(date.to_string(), "2030-02-28");
    // Omission preserves a relationship; an explicit empty list clears it.
    for (query, entity) in [
        (
            "SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE modulebay_id=$1",
            InventoryEntity::ModuleBay,
        ),
        (
            "SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=$1",
            InventoryEntity::ModuleType,
        ),
        (
            "SELECT modulebaytype_id FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=$1",
            InventoryEntity::ModuleBayTemplate,
        ),
    ] {
        let mut record = snapshot
            .records
            .iter()
            .find(|record| record.identity.entity.as_str() == entity.source_key())
            .expect("junction source")
            .clone();
        record.fields.remove("module_bay_types");
        apply_snapshot(
            &target,
            &mut tx,
            &SourceSnapshot {
                coverage: None,
                records: vec![record.clone()],
            },
            &ids,
        )
        .await?;
        let preserved: Vec<Uuid> = sqlx::query_scalar(query)
            .bind(local_id(&ids, entity))
            .fetch_all(&mut *tx)
            .await?;
        assert_eq!(preserved, vec![bay_type]);
        record.fields.insert("module_bay_types".into(), json!([]));
        apply_snapshot(
            &target,
            &mut tx,
            &SourceSnapshot {
                coverage: None,
                records: vec![record],
            },
            &ids,
        )
        .await?;
        let cleared: Vec<Uuid> = sqlx::query_scalar(query)
            .bind(local_id(&ids, entity))
            .fetch_all(&mut *tx)
            .await?;
        assert!(cleared.is_empty());
    }
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture with integrated migrations; verifies module-only snapshot replay and native template factory"]
async fn module_only_snapshot_keeps_explicit_component_ids_without_disabling_native_factory()
-> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    crate::storage::database::migrations::run_migrations(&pool).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let mut snapshot =
        hardware_snapshot(Uuid::now_v7(), &format!("module-replay-{}", Uuid::now_v7()));
    let mut replay = SourceSnapshot {
        coverage: None,
        records: Vec::new(),
    };
    snapshot.records.retain(|record| {
        if matches!(
            record.identity.entity.as_str(),
            "dcim.modules" | "dcim.cooling-intakes" | "dcim.cooling-outflows"
        ) {
            replay.records.push(record.clone());
            false
        } else {
            true
        }
    });
    let ids = apply_snapshot(&target, &mut tx, &snapshot, &HashMap::new()).await?;
    let native_bay = Uuid::now_v7();
    let native_module = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'native-slot','','','',$2,'{}',0,0,0,0)").bind(native_bay).bind(local_id(&ids, InventoryEntity::Device)).execute(&mut *tx).await?;
    sqlx::query("SELECT set_config('piping.hardware_materialization','native',true)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,comments,description,status,custom_field_data) VALUES ($1,$2,$3,$4,'','','','active','{}')").bind(native_module).bind(local_id(&ids, InventoryEntity::Device)).bind(native_bay).bind(local_id(&ids, InventoryEntity::ModuleType)).execute(&mut *tx).await?;
    let native_count: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM dcim_coolingintake WHERE module_id=$1)+(SELECT count(*) FROM dcim_coolingoutflow WHERE module_id=$1)").bind(native_module).fetch_one(&mut *tx).await?;
    assert_eq!(
        native_count, 2,
        "ordinary native creation must still materialize templates"
    );
    let ids = apply_snapshot(&target, &mut tx, &replay, &ids).await?;
    let actual: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM dcim_coolingintake WHERE module_id=$1 UNION ALL SELECT id FROM dcim_coolingoutflow WHERE module_id=$1").bind(local_id(&ids, InventoryEntity::Module)).fetch_all(&mut *tx).await?;
    assert_eq!(actual.len(), 2);
    assert!(actual.contains(&local_id(&ids, InventoryEntity::CoolingIntake)));
    assert!(actual.contains(&local_id(&ids, InventoryEntity::CoolingOutflow)));
    let extra_bays: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dcim_modulebay WHERE module_id=$1")
            .bind(local_id(&ids, InventoryEntity::Module))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        extra_bays, 0,
        "explicit module replay must not synthesize a child bay"
    );
    let root = local_id(&ids, InventoryEntity::ModuleBay);
    let module = local_id(&ids, InventoryEntity::Module);
    sqlx::query("UPDATE dcim_modulebay SET enabled=false WHERE id=$1")
        .bind(root)
        .execute(&mut *tx)
        .await?;
    // Native metadata UPDATE remains legal after the occupied bay is disabled.
    sqlx::query("UPDATE dcim_module SET serial='native-update',local_context_data='{\"preserved\":true}' WHERE id=$1")
        .bind(module).execute(&mut *tx).await?;
    let mut metadata = replay
        .records
        .iter()
        .find(|record| record.identity.entity.as_str() == "dcim.modules")
        .expect("module source")
        .clone();
    metadata
        .fields
        .insert("serial".into(), json!("snapshot-update"));
    apply_snapshot(
        &target,
        &mut tx,
        &SourceSnapshot {
            coverage: None,
            records: vec![metadata],
        },
        &ids,
    )
    .await?;
    let persisted: (String, Value) =
        sqlx::query_as("SELECT serial,local_context_data FROM dcim_module WHERE id=$1")
            .bind(module)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        persisted,
        ("snapshot-update".into(), json!({"preserved": true})),
        "metadata replay must use UPDATE and preserve omitted columns"
    );

    let tree_id: i32 = sqlx::query_scalar("SELECT tree_id FROM dcim_modulebay WHERE id=$1")
        .bind(root)
        .fetch_one(&mut *tx)
        .await?;
    let child = provider_record(
        replay.records[0].identity.source_provider_id,
        InventoryEntity::ModuleBay,
        "2",
        json!({"name": "explicit-child", "device": 1, "module": 1, "enabled": true, "module_bay_types": [1]}),
    );
    let ids = apply_snapshot(
        &target,
        &mut tx,
        &SourceSnapshot {
            coverage: None,
            records: vec![child],
        },
        &ids,
    )
    .await?;
    let child_id = ids[&("dcim.module-bays".into(), "2".into())];
    let root_tree: (i32, i32, i32, i32) =
        sqlx::query_as("SELECT tree_id,lft,rght,level FROM dcim_modulebay WHERE id=$1")
            .bind(root)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        root_tree,
        (tree_id, 1, 4, 0),
        "the native root tree identity and nested-set convention must survive replay"
    );
    let child_tree: (Uuid, i32, i32, i32, i32) =
        sqlx::query_as("SELECT parent_id,tree_id,lft,rght,level FROM dcim_modulebay WHERE id=$1")
            .bind(child_id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(child_tree, (root, tree_id, 2, 3, 1));
    let bays: (i64, i64) = sqlx::query_as("SELECT module_bay_count,(SELECT count(*) FROM dcim_modulebay WHERE device_id=$1) FROM dcim_device WHERE id=$1")
        .bind(local_id(&ids, InventoryEntity::Device)).fetch_one(&mut *tx).await?;
    assert_eq!(bays.0, bays.1);
    let modules: (i64, i64) = sqlx::query_as("SELECT module_count,(SELECT count(*) FROM dcim_module WHERE module_type_id=$1) FROM dcim_moduletype WHERE id=$1")
        .bind(local_id(&ids, InventoryEntity::ModuleType)).fetch_one(&mut *tx).await?;
    assert_eq!(modules.0, modules.1);
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture with integrated migrations; selected category updates must precede a missing Module installation"]
async fn hardware_snapshot_finalizes_selected_categories_before_installing_missing_module()
-> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    crate::storage::database::migrations::run_migrations(&pool).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let provider = Uuid::now_v7();
    let suffix = format!("categories-{}", Uuid::now_v7());
    let mut snapshot = hardware_snapshot(provider, &suffix);
    let module = snapshot
        .records
        .iter()
        .find(|record| record.identity.entity.as_str() == "dcim.modules")
        .expect("module source")
        .clone();
    snapshot.records.retain(|record| {
        !matches!(
            record.identity.entity.as_str(),
            "dcim.modules" | "dcim.cooling-intakes" | "dcim.cooling-outflows"
        )
    });
    for record in &mut snapshot.records {
        if matches!(
            record.identity.entity.as_str(),
            "dcim.module-types" | "dcim.module-bay-templates"
        ) {
            record.fields.insert("module_bay_types".into(), json!([2]));
        }
    }
    snapshot.records.push(provider_record(provider, InventoryEntity::ModuleBayType, "2",
        json!({"name": format!("{suffix}-b"), "slug": format!("{suffix}-b"), "manufacturer": 1, "color": "123456"})));
    let ids = apply_snapshot(&target, &mut tx, &snapshot, &HashMap::new()).await?;
    let compatible: bool = sqlx::query_scalar("SELECT dcim_module_bay_compatible($1,$2)")
        .bind(local_id(&ids, InventoryEntity::ModuleBay))
        .bind(local_id(&ids, InventoryEntity::ModuleType))
        .fetch_one(&mut *tx)
        .await?;
    assert!(
        !compatible,
        "the empty destination starts with disjoint A/B memberships"
    );
    let mut incoming = SourceSnapshot {
        coverage: None,
        records: vec![module],
    };
    for entity in [
        InventoryEntity::ModuleBay,
        InventoryEntity::ModuleType,
        InventoryEntity::ModuleBayTemplate,
    ] {
        let mut selected = snapshot
            .records
            .iter()
            .find(|record| record.identity.entity.as_str() == entity.source_key())
            .expect("selected category owner")
            .clone();
        selected
            .fields
            .insert("module_bay_types".into(), json!([3]));
        incoming.records.push(selected);
    }
    // The new shared category deliberately occurs after its selected consumers.
    incoming.records.push(provider_record(provider, InventoryEntity::ModuleBayType, "3",
        json!({"name": format!("{suffix}-c"), "slug": format!("{suffix}-c"), "manufacturer": 1, "color": "123456"})));
    let ids = apply_snapshot(&target, &mut tx, &incoming, &ids).await?;
    let installed: (Uuid, Uuid) =
        sqlx::query_as("SELECT module_bay_id,module_type_id FROM dcim_module WHERE id=$1")
            .bind(local_id(&ids, InventoryEntity::Module))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        installed,
        (
            local_id(&ids, InventoryEntity::ModuleBay),
            local_id(&ids, InventoryEntity::ModuleType)
        )
    );
    let category = ids[&("dcim.module-bay-types".into(), "3".into())];
    for (query, owner) in [
        (
            "SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE modulebay_id=$1",
            InventoryEntity::ModuleBay,
        ),
        (
            "SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=$1",
            InventoryEntity::ModuleType,
        ),
        (
            "SELECT modulebaytype_id FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=$1",
            InventoryEntity::ModuleBayTemplate,
        ),
    ] {
        let final_categories: Vec<Uuid> = sqlx::query_scalar(query)
            .bind(local_id(&ids, owner))
            .fetch_all(&mut *tx)
            .await?;
        assert_eq!(final_categories, vec![category]);
    }
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture with integrated migrations; occupied graph category changes require global clear and atomic final sets"]
async fn hardware_snapshot_replaces_occupied_categories_without_mixed_or_partial_sets() -> TestResult
{
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    crate::storage::database::migrations::run_migrations(&pool).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let mut tx = pool.begin().await?;
    let provider = Uuid::now_v7();
    let suffix = format!("occupied-{}", Uuid::now_v7());
    let snapshot = hardware_snapshot(provider, &suffix);
    let ids = apply_snapshot(&target, &mut tx, &snapshot, &HashMap::new()).await?;
    let module = local_id(&ids, InventoryEntity::Module);
    let before: (Uuid, Uuid, Uuid, String) = sqlx::query_as(
        "SELECT id,module_bay_id,module_type_id,serial FROM dcim_module WHERE id=$1",
    )
    .bind(module)
    .fetch_one(&mut *tx)
    .await?;
    for (query, owner) in [
        (
            "SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=$1",
            InventoryEntity::ModuleType,
        ),
        (
            "SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE modulebay_id=$1",
            InventoryEntity::ModuleBay,
        ),
    ] {
        let starting: Vec<Uuid> = sqlx::query_scalar(query)
            .bind(local_id(&ids, owner))
            .fetch_all(&mut *tx)
            .await?;
        assert_eq!(
            starting,
            vec![local_id(&ids, InventoryEntity::ModuleBayType)]
        );
    }
    let mut incoming = SourceSnapshot {
        coverage: None,
        records: Vec::new(),
    };
    // Both selected owners initially share A. Final Type={C,D}, Bay={E,D}.
    // Module is deliberately omitted and must stay installed with its identity.
    for (entity, categories) in [
        (InventoryEntity::ModuleType, json!([3, 4])),
        (InventoryEntity::ModuleBay, json!([5, 4])),
    ] {
        let mut selected = snapshot
            .records
            .iter()
            .find(|record| record.identity.entity.as_str() == entity.source_key())
            .expect("selected occupied owner")
            .clone();
        selected
            .fields
            .insert("module_bay_types".into(), categories);
        incoming.records.push(selected);
    }
    for (id, category) in [("3", "c"), ("4", "d"), ("5", "e")] {
        incoming.records.push(provider_record(provider, InventoryEntity::ModuleBayType, id,
            json!({"name": format!("{suffix}-{category}"), "slug": format!("{suffix}-{category}"), "manufacturer": 1, "color": "123456"})));
    }
    let ids = apply_snapshot(&target, &mut tx, &incoming, &ids).await?;
    let category = |id: &str| ids[&("dcim.module-bay-types".to_owned(), id.to_owned())];
    for (query, entity, expected) in [
        (
            "SELECT modulebaytype_id FROM dcim_moduletype_module_bay_types WHERE moduletype_id=$1",
            InventoryEntity::ModuleType,
            HashSet::from([category("3"), category("4")]),
        ),
        (
            "SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE modulebay_id=$1",
            InventoryEntity::ModuleBay,
            HashSet::from([category("5"), category("4")]),
        ),
    ] {
        let persisted: Vec<Uuid> = sqlx::query_scalar(query)
            .bind(local_id(&ids, entity))
            .fetch_all(&mut *tx)
            .await?;
        assert_eq!(persisted.into_iter().collect::<HashSet<_>>(), expected);
    }
    let after: (Uuid, Uuid, Uuid, String) = sqlx::query_as(
        "SELECT id,module_bay_id,module_type_id,serial FROM dcim_module WHERE id=$1",
    )
    .bind(module)
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(
        after, before,
        "category replay must retain the omitted existing occupant"
    );
    let occupants: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM dcim_module WHERE module_bay_id=$1")
            .bind(local_id(&ids, InventoryEntity::ModuleBay))
            .fetch_all(&mut *tx)
            .await?;
    assert_eq!(occupants, vec![module]);
    let omitted_template: Vec<Uuid> = sqlx::query_scalar("SELECT modulebaytype_id FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=$1")
        .bind(local_id(&ids, InventoryEntity::ModuleBayTemplate)).fetch_all(&mut *tx).await?;
    assert_eq!(
        omitted_template,
        vec![local_id(&ids, InventoryEntity::ModuleBayType)],
        "an omitted owner must retain its A membership"
    );
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(())
}
