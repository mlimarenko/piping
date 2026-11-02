use super::tests::{assessment_config, provider_record};
use super::*;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

use super::interface_graph::assess_observed;
use crate::contexts::inventory::domain::dcim::interface::InterfaceCableEnd;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::cabling_repository::{CableAssociation, CableEndpointKind};

fn endpoint_plan(id: Uuid, cable: Option<Uuid>, end: Option<InterfaceCableEnd>) -> PlannedRow {
    PlannedRow {
        entity: InventoryEntity::Interface,
        source_entity: InventoryEntity::Interface.source_key().into(),
        external_id: "test-planned-endpoint".into(),
        local_id: id,
        source_timestamps: SourceTimestamps::default(),
        columns: vec![
            PlannedColumn {
                column: "cable_id",
                value: ColumnValue::Uuid(cable),
                updatable: true,
            },
            PlannedColumn {
                column: "cable_end",
                value: ColumnValue::Scalar(
                    PgType::Text,
                    end.map_or(InventoryValue::Null, |end| InventoryValue::Text {
                        value: serde_json::to_value(end)
                            .expect("native side")
                            .as_str()
                            .expect("side token")
                            .into(),
                    }),
                ),
                updatable: true,
            },
        ],
        relations: Vec::new(),
        junctions: Vec::new(),
        reference_targets: HashMap::new(),
    }
}

#[test]
fn selected_empty_side_survives_source_preparation_and_blocks_unseen_native_interface() {
    let mut source = snapshot(Uuid::from_u128(78));
    let original = batch(&source);
    let cable_id = row(&original, InventoryEntity::Cable, "1").local_id;
    let unseen_id = row(&original, InventoryEntity::Interface, "2").local_id;
    record_mut(&mut source, InventoryEntity::Cable, "1")
        .fields
        .insert("a_terminations".into(), json!([]));
    record_mut(&mut source, InventoryEntity::Cable, "1")
        .fields
        .remove("b_terminations");
    source.records.retain(|record| {
        !(record.identity.entity.as_str() == "dcim.interfaces"
            && record.identity.external_id == "2")
    });
    let prepared = batch(&source);
    assert_eq!(prepared.cable_sides.len(), 1);
    let ids = merge_local_ids(&prepared, HashMap::new());
    let observed = [CableAssociation {
        kind: CableEndpointKind::Interface,
        endpoint_id: unseen_id,
        cable_id,
        end: Some(InterfaceCableEnd::A),
    }];
    let (findings, complete) = assess_observed(&prepared, &[], &ids, &observed);
    assert!(!complete);
    assert!(findings.iter().any(|finding| finding.code
        == FindingCode::AnalysisDependencyUnavailable
        && finding.field_path.as_deref() == Some("a_terminations")));
    let mut original_source = snapshot(Uuid::from_u128(78));
    let mut cleared = record_mut(&mut original_source, InventoryEntity::Interface, "2").clone();
    cleared.fields.insert("cable".into(), Value::Null);
    cleared.fields.insert("cable_end".into(), Value::Null);
    source.records.push(cleared);
    let cleared_batch = batch(&source);
    let clear = endpoint_plan(unseen_id, None, None);
    let (findings, complete) = assess_observed(
        &cleared_batch,
        &[clear],
        &merge_local_ids(&cleared_batch, HashMap::new()),
        &observed,
    );
    assert!(complete && findings.is_empty());
    record_mut(&mut source, InventoryEntity::Cable, "1")
        .fields
        .remove("a_terminations");
    let omitted = batch(&source);
    assert!(omitted.cable_sides.is_empty());
    let (findings, complete) = assess_observed(&omitted, &[], &ids, &observed);
    assert!(complete && findings.is_empty());
}

#[test]
fn selected_nonempty_side_blocks_stale_extras_and_unsupported_native_endpoint_kinds() {
    let mut source = snapshot(Uuid::from_u128(78));
    record_mut(&mut source, InventoryEntity::Cable, "1")
        .fields
        .remove("b_terminations");
    let prepared = batch(&source);
    let ids = merge_local_ids(&prepared, HashMap::new());
    let cable_id = row(&prepared, InventoryEntity::Cable, "1").local_id;
    let declared_id = row(&prepared, InventoryEntity::Interface, "2").local_id;
    let extra_id = row(&prepared, InventoryEntity::Interface, "1").local_id;
    let mut plans = vec![endpoint_plan(
        declared_id,
        Some(cable_id),
        Some(InterfaceCableEnd::A),
    )];
    let observed = [CableAssociation {
        kind: CableEndpointKind::Interface,
        endpoint_id: extra_id,
        cable_id,
        end: Some(InterfaceCableEnd::A),
    }];
    let (findings, complete) = assess_observed(&prepared, &plans, &ids, &observed);
    assert!(!complete && !findings.is_empty());
    plans.push(endpoint_plan(extra_id, None, None));
    let (findings, complete) = assess_observed(&prepared, &plans, &ids, &observed);
    assert!(complete && findings.is_empty());
    plans[1] = endpoint_plan(extra_id, Some(cable_id), Some(InterfaceCableEnd::B));
    let (findings, complete) = assess_observed(&prepared, &plans, &ids, &observed);
    assert!(complete && findings.is_empty());
    for kind in [
        CableEndpointKind::FrontPort,
        CableEndpointKind::RearPort,
        CableEndpointKind::ConsolePort,
        CableEndpointKind::ConsoleServerPort,
        CableEndpointKind::PowerPort,
        CableEndpointKind::PowerOutlet,
        CableEndpointKind::PowerFeed,
        CableEndpointKind::CircuitTermination,
    ] {
        let unsupported = [CableAssociation {
            kind,
            endpoint_id: extra_id,
            cable_id,
            end: Some(InterfaceCableEnd::A),
        }];
        let (findings, complete) = assess_observed(&prepared, &plans, &ids, &unsupported);
        assert!(!complete && !findings.is_empty(), "{kind:?}");
    }
    let unknown_side = [CableAssociation {
        kind: CableEndpointKind::Interface,
        endpoint_id: extra_id,
        cable_id,
        end: None,
    }];
    assert!(!assess_observed(&prepared, &plans, &ids, &unknown_side).1);
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL with integrated schema; seeds rollback-only native associations, plans and preflights but never applies imported rows"]
async fn selected_reverse_sides_observe_actual_native_associations_before_any_import_write()
-> TestResult {
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let mut tx = pool.begin().await?;
    lock_tag_catalog_for_usage(&mut tx).await?;
    lock_hardware_graph(&mut tx).await?;
    let cable: Uuid = sqlx::query_scalar("INSERT INTO dcim_cable (custom_field_data,status,label,color,comments,description,profile) VALUES ('{}','connected','captured-side-preflight','123456','','','') RETURNING id")
        .fetch_one(&mut *tx).await?;
    let existing: Uuid = sqlx::query_scalar("INSERT INTO dcim_interface (custom_field_data,name,_name,label,description,mark_connected,enabled,type,mgmt_only,cable_id,cable_end) VALUES ('{}','captured-side-I','captured-side-I','','',false,true,'1000base-t',false,$1,'A') RETURNING id")
        .bind(cable).fetch_one(&mut *tx).await?;
    let provider = Uuid::now_v7();
    for scenario in ["unseen-empty", "explicit-clear", "stale-extra", "matching"] {
        let mut source = snapshot(provider);
        record_mut(&mut source, InventoryEntity::Cable, "1")
            .fields
            .remove("b_terminations");
        if scenario == "unseen-empty" || scenario == "explicit-clear" {
            record_mut(&mut source, InventoryEntity::Cable, "1")
                .fields
                .insert("a_terminations".into(), json!([]));
            if scenario == "unseen-empty" {
                source.records.retain(|record| {
                    !(record.identity.entity.as_str() == "dcim.interfaces"
                        && record.identity.external_id == "2")
                });
            } else {
                record_mut(&mut source, InventoryEntity::Interface, "2")
                    .fields
                    .insert("cable".into(), Value::Null);
                record_mut(&mut source, InventoryEntity::Interface, "2")
                    .fields
                    .insert("cable_end".into(), Value::Null);
            }
        }
        let prepared = batch(&source);
        let mut bindings = HashMap::from([(
            (InventoryEntity::Cable.source_key().into(), "1".into()),
            cable,
        )]);
        if scenario != "stale-extra" {
            bindings.insert(
                (InventoryEntity::Interface.source_key().into(), "2".into()),
                existing,
            );
        }
        let mut ids = merge_local_ids(&prepared, bindings);
        for row in &prepared.rows {
            for (_, reference) in &row.references {
                let entity = reference.target.expect("captured supported relation");
                ids.entry((entity.source_key().into(), reference.external_id.clone()))
                    .or_insert_with(|| {
                        derive_local_id(
                            provider,
                            entity.source_key(),
                            &reference.external_id,
                            &entity.target_key(),
                        )
                    });
            }
        }
        let mut plans = Vec::new();
        let mut content_types = HashMap::new();
        for row in &prepared.rows {
            plans.push(plan_row(row, &ids, None, &mut content_types, &mut tx).await?);
        }
        hardware_write_order(&plans)?;
        let (findings, complete) =
            interface_graph::assess_target(&prepared, &plans, &ids, &mut tx).await?;
        if scenario == "unseen-empty" || scenario == "stale-extra" {
            assert!(!complete, "{scenario}");
            assert!(
                findings
                    .iter()
                    .any(|finding| finding.code == FindingCode::AnalysisDependencyUnavailable),
                "{scenario}"
            );
        } else {
            assert!(complete && findings.is_empty(), "{scenario}: {findings:?}");
        }
        let unchanged: (Option<Uuid>, Option<String>) =
            sqlx::query_as("SELECT cable_id,cable_end::text FROM dcim_interface WHERE id=$1")
                .bind(existing)
                .fetch_one(&mut *tx)
                .await?;
        assert_eq!(
            unchanged,
            (Some(cable), Some("A".into())),
            "planning must not write endpoints"
        );
    }
    tx.rollback().await?;
    Ok(())
}

macro_rules! capture {
    ($resource:literal) => {
        (
            $resource,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/netbox/captured-hardware-20261027/authentic/full-",
                $resource,
                "-page-0.json"
            )),
        )
    };
}

const PAGES: &[(&str, &str)] = &[
    capture!("rack-groups"),
    capture!("cable-bundles"),
    capture!("module-bay-types"),
    capture!("cooling-sources"),
    capture!("cooling-feeds"),
    capture!("cooling-intakes"),
    capture!("cooling-outflows"),
    capture!("cooling-intake-templates"),
    capture!("cooling-outflow-templates"),
    capture!("locations"),
    capture!("racks"),
    capture!("cables"),
    capture!("module-types"),
    capture!("module-bay-templates"),
    capture!("module-bays"),
    capture!("modules"),
    capture!("interfaces"),
];

fn snapshot(provider: Uuid) -> SourceSnapshot {
    let mut records = Vec::new();
    for (resource, raw) in PAGES {
        let page: Value = serde_json::from_str(raw).expect("captured JSON page");
        assert!(page["next"].is_null());
        let rows = page["results"].as_array().expect("complete result list");
        assert_eq!(page["count"].as_u64(), Some(rows.len() as u64));
        let entity =
            InventoryEntity::from_source_key(&format!("dcim.{resource}")).expect("source model");
        for row in rows {
            records.push(provider_record(
                provider,
                entity,
                &row["id"].to_string(),
                row.clone(),
            ));
        }
    }
    SourceSnapshot { records }
}

fn config(snapshot: &SourceSnapshot) -> MappingConfigDocument {
    let mut seen = HashSet::new();
    let mut mappings = snapshot.records.iter().filter_map(|record| {
        let entity = InventoryEntity::from_source_key(record.identity.entity.as_str())?;
        if !seen.insert(entity) {
            return None;
        }
        // Mapping every approved field also maps omitted optionals without
        // supplying literals. Raw maps remain unfiltered for the boundary scan.
        let names: Vec<_> = entity.field_iter().map(|field| field.name()).collect();
        Some(assessment_config(
            entity,
            record.identity.source_provider_id,
            &names,
        ))
    });
    let mut config = mappings.next().expect("captured models");
    for mapping in mappings {
        config.entity_mappings.extend(mapping.entity_mappings);
        config.field_mappings.extend(mapping.field_mappings);
    }
    config
}

fn record_mut<'a>(
    snapshot: &'a mut SourceSnapshot,
    entity: InventoryEntity,
    id: &str,
) -> &'a mut SourceRecord {
    snapshot
        .records
        .iter_mut()
        .find(|record| {
            record.identity.entity.as_str() == entity.source_key()
                && record.identity.external_id == id
        })
        .expect("captured identity")
}

fn batch(snapshot: &SourceSnapshot) -> LocalInventoryBatch {
    let assessment = assess_preparation(&config(snapshot), snapshot);
    assert!(assessment.analysis_complete, "{:?}", assessment.findings);
    assessment
        .batch
        .unwrap_or_else(|| panic!("{:?}", assessment.findings))
}

fn row<'a>(batch: &'a LocalInventoryBatch, entity: InventoryEntity, id: &str) -> &'a PreparedRow {
    batch
        .rows
        .iter()
        .find(|row| row.entity == entity && row.external_id == id)
        .expect("prepared identity")
}

fn scalar<'a>(row: &'a PreparedRow, name: &str) -> Option<&'a InventoryValue> {
    row.entity
        .field(name)
        .and_then(|field| row.scalars.get(&field))
}

#[test]
fn captured_interfaces_validate_raw_rf_precision_before_scalar_and_graph_conversion() {
    for field in ["rf_channel_frequency", "rf_channel_width"] {
        for token in [
            "0.000000005",
            "0.0290000000000000000000000000000000000001",
            "-1.114000000000000000000000000000000000001",
        ] {
            for value in [
                serde_json::from_str::<Value>(token).expect("raw numeric lexeme"),
                Value::String(token.into()),
            ] {
                let mut source = snapshot(Uuid::from_u128(78));
                record_mut(&mut source, InventoryEntity::Interface, "1")
                    .fields
                    .insert(field.into(), value);
                let assessment = assess_preparation(&config(&source), &source);
                assert!(assessment.batch.is_none(), "{field}: {token}");
            }
        }
        for token in ["0.029", "1.114000", "1114e-3", "-0.001", "0e10"] {
            for value in [
                serde_json::from_str::<Value>(token).expect("raw numeric lexeme"),
                Value::String(token.into()),
            ] {
                let mut source = snapshot(Uuid::from_u128(78));
                record_mut(&mut source, InventoryEntity::Interface, "1")
                    .fields
                    .insert(field.into(), value);
                let prepared = batch(&source);
                assert!(matches!(
                    scalar(row(&prepared, InventoryEntity::Interface, "1"), field),
                    Some(InventoryValue::Number { value })
                        if *value == token.parse::<f64>().expect("valid decimal")
                ));
            }
        }
    }
}

#[test]
fn captured_full_maps_prepare_sixteen_hardware_models_and_four_interfaces() {
    let snapshot = snapshot(Uuid::from_u128(78));
    assert_eq!(snapshot.records.len(), 25);
    let batch = batch(&snapshot);
    assert_eq!(batch.rows.len(), 25);
    assert_eq!(
        batch
            .rows
            .iter()
            .map(|row| row.entity)
            .collect::<HashSet<_>>()
            .len(),
        17
    );
    for source in &snapshot.records {
        let entity =
            InventoryEntity::from_source_key(source.identity.entity.as_str()).expect("model");
        assert_eq!(
            row(&batch, entity, &source.identity.external_id).source_timestamps,
            SourceTimestamps::from_record(source).expect("source observations")
        );
    }
    assert!(
        matches!(scalar(row(&batch, InventoryEntity::Cable, "1"), "profile"),
        Some(InventoryValue::Text { value }) if value.is_empty())
    );
    assert!(matches!(
        scalar(row(&batch, InventoryEntity::ModuleType, "1"), "attributes"),
        Some(InventoryValue::Null)
    ));
    assert!(matches!(
        scalar(row(&batch, InventoryEntity::Interface, "2"), "mgmt_only"),
        Some(InventoryValue::Boolean { value: true })
    ));
    assert!(matches!(
        scalar(row(&batch, InventoryEntity::Interface, "1"), "mtu"),
        Some(InventoryValue::Integer { value: 1500 })
    ));
    assert!(
        matches!(scalar(row(&batch, InventoryEntity::ModuleType, "1"), "end_of_life"),
        Some(InventoryValue::Text { value }) if value == "2030-02-28")
    );
    assert!(
        InventoryEntity::ModuleType
            .field("attribute_data")
            .is_none()
    );
    assert_eq!(
        InventoryEntity::ModuleType
            .field("attributes")
            .expect("writable attributes")
            .column(),
        "attribute_data"
    );
    assert!(InventoryEntity::from_source_key("dcim.cable-terminations").is_none());
}

#[test]
fn captured_readonly_projection_acceptance_does_not_permit_unknown_populated_fields() {
    let mut snapshot = snapshot(Uuid::from_u128(78));
    batch(&snapshot);
    for entity in [
        InventoryEntity::CoolingSource,
        InventoryEntity::Location,
        InventoryEntity::Rack,
        InventoryEntity::ModuleType,
        InventoryEntity::ModuleBay,
        InventoryEntity::Module,
    ] {
        let source = snapshot
            .records
            .iter_mut()
            .find(|record| record.identity.entity.as_str() == entity.source_key())
            .expect("model");
        source
            .fields
            .insert("unrecognised_populated_projection".into(), json!(1));
    }
    let assessment = assess_preparation(&config(&snapshot), &snapshot);
    assert!(assessment.batch.is_none());
    assert_eq!(
        assessment
            .findings
            .iter()
            .filter(|finding| finding.code == FindingCode::FieldUnsupported)
            .count(),
        6
    );
}

#[test]
fn captured_writable_attributes_and_relation_omission_clear_remain_distinct() {
    let mut snapshot = snapshot(Uuid::from_u128(78));
    let attributes = json!({"vendor": {"rating": 3}, "nullable": null, "empty": []});
    record_mut(&mut snapshot, InventoryEntity::ModuleType, "1")
        .fields
        .insert("attributes".into(), attributes.clone());
    let prepared = batch(&snapshot);
    assert!(
        matches!(scalar(row(&prepared, InventoryEntity::ModuleType, "1"), "attributes"),
        Some(InventoryValue::Json { value }) if value == &attributes)
    );
    record_mut(&mut snapshot, InventoryEntity::ModuleType, "1")
        .fields
        .remove("attributes");
    record_mut(&mut snapshot, InventoryEntity::Cable, "1")
        .fields
        .remove("bundle");
    let prepared = batch(&snapshot);
    assert!(
        scalar(
            row(&prepared, InventoryEntity::ModuleType, "1"),
            "attributes"
        )
        .is_none()
    );
    let cable = row(&prepared, InventoryEntity::Cable, "1");
    assert!(
        !cable
            .references
            .iter()
            .any(|(field, _)| field.name() == "bundle")
    );
    assert!(!cable.cleared.iter().any(|field| field.name() == "bundle"));
    record_mut(&mut snapshot, InventoryEntity::Cable, "1")
        .fields
        .insert("bundle".into(), Value::Null);
    let prepared = batch(&snapshot);
    assert!(
        row(&prepared, InventoryEntity::Cable, "1")
            .cleared
            .iter()
            .any(|field| field.name() == "bundle")
    );
}

#[test]
fn captured_provider_qualification_and_nullable_networking_survive_preparation() {
    let mut first = snapshot(Uuid::from_u128(78));
    let prepared = batch(&first);
    let other = batch(&snapshot(Uuid::from_u128(79)));
    for source in &prepared.rows {
        assert_ne!(
            source.local_id,
            row(&other, source.entity, &source.external_id).local_id
        );
    }
    assert!(matches!(
        scalar(row(&prepared, InventoryEntity::Interface, "1"), "duplex"),
        Some(InventoryValue::Null)
    ));
    record_mut(&mut first, InventoryEntity::Interface, "1")
        .fields
        .remove("duplex");
    assert!(
        scalar(
            row(&batch(&first), InventoryEntity::Interface, "1"),
            "duplex"
        )
        .is_none()
    );
    record_mut(&mut first, InventoryEntity::ModuleType, "1")
        .fields
        .insert("attribute_data".into(), json!({"wrong_source_name": true}));
    let assessment = assess_preparation(&config(&first), &first);
    assert!(assessment.batch.is_none());
    assert!(
        assessment
            .findings
            .iter()
            .any(|finding| finding.code == FindingCode::FieldUnsupported)
    );
}

#[test]
fn captured_cable_sides_reject_bad_types_id_mismatches_duplicates_conflicts_and_partial_snapshots()
{
    let original = snapshot(Uuid::from_u128(78));
    let original_side =
        record_mut(&mut original.clone(), InventoryEntity::Cable, "1").fields["a_terminations"]
            .clone();
    let mut variants = Vec::new();
    for (pointer, value) in [
        ("/0/object_type", json!("dcim.frontport")),
        ("/0/object_id", json!(true)),
        ("/0/object/id", json!(3)),
        ("/0/object/cable/id", json!(99)),
    ] {
        let mut value_side = original_side.clone();
        *value_side.pointer_mut(pointer).expect("captured path") = value;
        variants.push(value_side);
    }
    let mut nested_type = original_side.clone();
    nested_type[0]["object"]["object_type"] = json!("dcim.consoleport");
    variants.push(nested_type);
    let mut crossing = original_side.clone();
    crossing[0]["source_provider_id"] = json!(Uuid::from_u128(79));
    variants.push(crossing);
    variants.push(json!([original_side[0].clone(), original_side[0].clone()]));
    variants.extend([json!([]), Value::Null, json!({})]);
    for invalid in variants {
        let mut snapshot = original.clone();
        record_mut(&mut snapshot, InventoryEntity::Cable, "1")
            .fields
            .insert("a_terminations".into(), invalid);
        let assessment = assess_preparation(&config(&snapshot), &snapshot);
        assert!(
            assessment.batch.is_none(),
            "invalid reverse side was accepted"
        );
        assert!(
            assessment
                .findings
                .iter()
                .any(|finding| finding.field_path.as_deref() == Some("a_terminations"))
        );
    }
    for missing in ["record", "cable", "cable_end", "provider"] {
        let mut snapshot = original.clone();
        match missing {
            "record" => snapshot.records.retain(|record| {
                !(record.identity.entity.as_str() == "dcim.interfaces"
                    && record.identity.external_id == "2")
            }),
            "provider" => {
                record_mut(&mut snapshot, InventoryEntity::Interface, "2")
                    .identity
                    .source_provider_id = Uuid::from_u128(79)
            }
            field => {
                record_mut(&mut snapshot, InventoryEntity::Interface, "2")
                    .fields
                    .remove(field);
            }
        }
        let assessment = assess_preparation(&config(&snapshot), &snapshot);
        assert!(assessment.batch.is_none());
        assert!(!assessment.analysis_complete);
    }
    let mut snapshot = original.clone();
    record_mut(&mut snapshot, InventoryEntity::Cable, "1")
        .fields
        .insert("b_terminations".into(), original_side);
    assert!(
        assess_preparation(&config(&snapshot), &snapshot)
            .batch
            .is_none()
    );
    record_mut(&mut snapshot, InventoryEntity::Interface, "2")
        .fields
        .insert("cable_end".into(), json!("B"));
    assert!(
        assess_preparation(&config(&snapshot), &snapshot)
            .batch
            .is_none()
    );
}

#[test]
fn captured_sides_omitted_preserve_and_explicit_empty_requires_forward_clear() {
    let mut snapshot = snapshot(Uuid::from_u128(78));
    record_mut(&mut snapshot, InventoryEntity::Cable, "1")
        .fields
        .remove("a_terminations");
    batch(&snapshot);
    record_mut(&mut snapshot, InventoryEntity::Cable, "1")
        .fields
        .insert("a_terminations".into(), json!([]));
    record_mut(&mut snapshot, InventoryEntity::Interface, "2")
        .fields
        .insert("cable".into(), Value::Null);
    record_mut(&mut snapshot, InventoryEntity::Interface, "2")
        .fields
        .insert("cable_end".into(), Value::Null);
    let prepared = batch(&snapshot);
    let interface = row(&prepared, InventoryEntity::Interface, "2");
    assert!(
        interface
            .cleared
            .iter()
            .any(|field| field.name() == "cable")
    );
    assert!(matches!(
        scalar(interface, "cable_end"),
        Some(InventoryValue::Null)
    ));
}

#[test]
fn captured_reverse_sides_cannot_be_bypassed_by_mapping_forward_end_to_another_side() {
    let snapshot = snapshot(Uuid::from_u128(78));
    let mut config = config(&snapshot);
    let mapping = config
        .entity_mappings
        .iter()
        .find(|mapping| mapping.source_entity == "dcim.interfaces")
        .expect("interface mapping")
        .id;
    let field = config
        .field_mappings
        .iter_mut()
        .find(|field| field.entity_mapping_id == mapping && field.target_field == "cable_end")
        .expect("end mapping");
    field.forward_transform_json = json!({"op": "literal", "args": ["B"]});
    let assessment = assess_preparation(&config, &snapshot);
    assert!(assessment.batch.is_none());
    assert!(
        assessment
            .findings
            .iter()
            .any(|finding| finding.field_path.as_deref() == Some("a_terminations"))
    );
}

#[test]
fn native_interface_networking_vocabulary_is_checked_after_mapping() {
    for (name, valid, invalid) in [
        ("mode", json!({"value": "tagged"}), json!("unknown")),
        ("duplex", json!({"value": "full"}), json!("FULL")),
        ("poe_mode", json!("pse"), json!("unknown")),
        ("poe_type", json!("type1-ieee802.3af"), json!("unknown")),
        ("rf_role", json!("ap"), json!("unknown")),
        ("mtu", json!(1500), json!(65537)),
        ("tx_power", json!(20), json!(128)),
        ("channel_id", json!(1), json!(0)),
        ("channels", json!(1), json!(1025)),
        ("wwn", json!("00:11:22:33:44:55:66:77"), json!("invalid")),
    ] {
        let field = InventoryEntity::Interface
            .field(name)
            .expect("native networking field");
        assert!(convert_scalar(field, &valid).is_ok(), "{name}");
        assert!(convert_scalar(field, &invalid).is_err(), "{name}");
    }
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL connection for plan_row's transaction argument; prepares and schedules only, never executes inventory writes"]
async fn captured_full_maps_use_actual_row_planner_and_scheduler_without_execution() -> TestResult {
    let snapshot = snapshot(Uuid::from_u128(78));
    let batch = batch(&snapshot);
    let mut ids = merge_local_ids(&batch, HashMap::new());
    // These represent already-established provider bindings, not invented source
    // fields. Native FK lookup remains deferred to the normal prewrite owner.
    for prepared in &batch.rows {
        for (_, reference) in &prepared.references {
            let entity = reference.target.expect("captured supported FK");
            ids.entry((entity.source_key().into(), reference.external_id.clone()))
                .or_insert_with(|| {
                    derive_local_id(
                        batch.source_provider_id,
                        entity.source_key(),
                        &reference.external_id,
                        &entity.target_key(),
                    )
                });
        }
    }
    let pool = PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let target = LocalInventoryCatalog::new(pool.clone());
    for record in &snapshot.records {
        assert!(target.resource(&record.identity.entity).is_some());
    }
    let fake = SourceEntity::new("dcim.cable-terminations")?;
    assert!(target.resource(&fake).is_none());
    let mut transaction = pool.begin().await?;
    let mut content_types = HashMap::new();
    let mut planned = Vec::new();
    for prepared in &batch.rows {
        planned.push(plan_row(prepared, &ids, None, &mut content_types, &mut transaction).await?);
    }
    let ordered = hardware_write_order(&planned)?;
    assert_eq!(ordered.len(), 25);
    let position = |entity| {
        ordered
            .iter()
            .position(|row| row.entity == entity)
            .expect("scheduled owner")
    };
    assert!(position(InventoryEntity::CableBundle) < position(InventoryEntity::Cable));
    assert!(position(InventoryEntity::ModuleBayType) < position(InventoryEntity::ModuleType));
    assert!(position(InventoryEntity::ModuleType) < position(InventoryEntity::Module));
    for end in ["2", "3"] {
        let plan = planned
            .iter()
            .find(|row| row.entity == InventoryEntity::Interface && row.external_id == end)
            .expect("native endpoint plan");
        assert_eq!(
            plan.columns
                .iter()
                .filter(|column| column.column == "cable_id")
                .count(),
            1
        );
        assert_eq!(
            plan.columns
                .iter()
                .filter(|column| column.column == "cable_end")
                .count(),
            1
        );
    }
    transaction.rollback().await?;
    Ok(())
}
