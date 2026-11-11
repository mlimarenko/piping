//! Regression: NUMERIC columns must round-trip through the repositories.
//!
//! `dcim_device.position` is `numeric(4,1)` and `virtualization_virtualmachine
//! .vcpus` is `numeric(6,2)`, but the domain models them as `f64`. sqlx does
//! not decode NUMERIC into f64, so a fixed SELECT without an explicit
//! `::float8` cast cannot read them. Under the old `.ok()` row reads that
//! failure silently decayed to `None` — a device with a real rack position
//! answered `position: null` from `get_by_id` for as long as that code
//! existed. The typed reads turned the same defect into a loud error, and
//! this test pins the actual fix: the cast, and the value coming back.

mod common;

use piping_backend::contexts::inventory::infrastructure::persistence::repositories::dcim::device_repository::SqlxDeviceRepository;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::dcim::device_type_repository::SqlxDeviceTypeRepository;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::dcim::rack_type_repository::{
    RackTypeRepository, SqlxRackTypeRepository,
};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::repository::{
    DeviceRepository, DeviceTypeRepository, VirtualMachineRepository,
};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::virtualization::virtual_machine_repository::SqlxVirtualMachineRepository;
use uuid::Uuid;

#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_numeric_columns -- --ignored
async fn device_position_round_trips_through_get_by_id()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    let (device_id, _) = common::seed_device(&pool, "numeric-cast").await?;
    sqlx::query("UPDATE dcim_device SET position = 5.0 WHERE id = $1")
        .bind(device_id)
        .execute(&pool)
        .await?;

    let repo = SqlxDeviceRepository::new(pool);
    let device = repo
        .get_by_id(device_id)
        .await?
        .ok_or("seeded device not found")?;
    assert_eq!(
        device.position,
        Some(5.0),
        "a real rack position must survive the read, not decay to None or error"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_numeric_columns -- --ignored
async fn vm_vcpus_round_trips_through_get_by_id()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    let vm_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO virtualization_virtualmachine (
            id, custom_field_data, name, status, vcpus, comments, description,
            serial, start_on_boot, interface_count, virtual_disk_count
        )
        VALUES ($1, '{}'::jsonb, $2, 'active', 2.5, '', '', '', true, 0, 0)
        "#,
    )
    .bind(vm_id)
    .bind(format!("numeric-cast-vm-{}", common::unique_suffix()))
    .execute(&pool)
    .await?;

    let repo = SqlxVirtualMachineRepository::new(pool);
    let vm = repo.get_by_id(vm_id).await?.ok_or("seeded vm not found")?;
    assert_eq!(
        vm.vcpus,
        Some(2.5),
        "vcpus must survive the read, not decay to None or error"
    );
    Ok(())
}

/// u_height is NOT NULL numeric read into a bare f64 — under the old `.ok()`
/// reads its decode failure fell back to `unwrap_or(1.0)`, so every device
/// type reported height 1.0 no matter what was stored, and weight was always
/// None. This pins the actual values coming back.
#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_numeric_columns -- --ignored
async fn device_type_u_height_and_weight_round_trip()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    let (device_id, _) = common::seed_device(&pool, "numeric-utype").await?;
    let type_id: Uuid = sqlx::query_scalar("SELECT device_type_id FROM dcim_device WHERE id = $1")
        .bind(device_id)
        .fetch_one(&pool)
        .await?;
    sqlx::query("UPDATE dcim_devicetype SET u_height = 2.5, weight = 12.75 WHERE id = $1")
        .bind(type_id)
        .execute(&pool)
        .await?;

    let repo = SqlxDeviceTypeRepository::new(pool);
    let device_type = repo
        .get_by_id(type_id)
        .await?
        .ok_or("seeded device type not found")?;
    assert_eq!(
        device_type.u_height, 2.5,
        "stored u_height must come back, not the 1.0 fallback"
    );
    assert_eq!(
        device_type.weight,
        Some(12.75),
        "stored weight must come back, not None"
    );
    Ok(())
}

/// rack type weight is numeric(8,2) read as Option<f64> — same class again:
/// it has always come back None. Pins the stored value coming back.
#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_numeric_columns -- --ignored
async fn rack_type_weight_round_trips() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    let type_id = Uuid::now_v7();
    let mfr_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dcim_manufacturer (id, custom_field_data, name, slug, description, comments) \
         VALUES (uuidv7(), '{}'::jsonb, $1, $1, '', '') RETURNING id",
    )
    .bind(format!("numeric-rt-mfr-{}", common::unique_suffix()))
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO dcim_racktype (id, custom_field_data, manufacturer_id, model, slug, \
         form_factor, width, u_height, starting_unit, desc_units, weight, description, comments, rack_count) \
         VALUES ($1, '{}'::jsonb, $2, $3, $3, '4-post-frame', 19, 42, 1, false, 21.5, '', '', 0)",
    )
    .bind(type_id)
    .bind(mfr_id)
    .bind(format!("numeric-rt-{}", common::unique_suffix()))
    .execute(&pool)
    .await?;

    let repo = SqlxRackTypeRepository::new(pool);
    let rack_type = repo
        .get_by_id(type_id)
        .await?
        .ok_or("seeded rack type not found")?;
    assert_eq!(
        rack_type.weight,
        Some(21.5),
        "stored weight must come back, not None"
    );
    Ok(())
}

/// `circuits_circuit.distance` is `numeric`, modelled as `f64`. The fixed
/// SELECT read it without a cast through `.ok().flatten()`, so it ALWAYS
/// decayed to `None` — and `update()` merges unspecified fields from the row
/// it just read, so editing any other field wrote that `None` back. A length
/// the operator typed in was destroyed by the product on the next edit. This
/// pins both halves: the read survives, and an unrelated edit preserves it.
#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_numeric_columns -- --ignored
async fn circuit_distance_survives_read_and_unrelated_update()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use piping_backend::contexts::inventory::infrastructure::persistence::repositories::circuits::circuit_repository::{
        CircuitFilter, CircuitRepository, SqlxCircuitRepository, UpdateCircuit,
    };
    let pool = common::build_test_db_pool().await?;
    let suffix = common::unique_suffix();
    let provider_id = Uuid::now_v7();
    let type_id = Uuid::now_v7();
    let circuit_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO circuits_provider (id, name, slug, description, comments, custom_field_data) \
         VALUES ($1, $2, $2, '', '', '{}'::jsonb)",
    )
    .bind(provider_id)
    .bind(format!("num-prov-{suffix}"))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO circuits_circuittype (id, name, slug, color, description, comments, \
         custom_field_data) VALUES ($1, $2, $2, '', '', '', '{}'::jsonb)",
    )
    .bind(type_id)
    .bind(format!("num-ctype-{suffix}"))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO circuits_circuit (id, cid, status, provider_id, type_id, description, \
         comments, custom_field_data, distance, distance_unit) \
         VALUES ($1, $2, 'active', $3, $4, '', '', '{}'::jsonb, 42.5, 'km')",
    )
    .bind(circuit_id)
    .bind(format!("num-cid-{suffix}"))
    .bind(provider_id)
    .bind(type_id)
    .execute(&pool)
    .await?;

    let repo = SqlxCircuitRepository::new(pool.clone());
    let read = repo
        .get_by_id(circuit_id)
        .await?
        .ok_or("seeded circuit not found")?;
    assert_eq!(
        read.distance,
        Some(42.5),
        "a stored circuit length must survive the read, not decay to None"
    );

    // The hydrated list builds its own projection — the same cast must be there.
    let (rows, _total) = repo
        .list_hydrated(
            CircuitFilter {
                search: Some(format!("num-cid-{suffix}")),
                ..CircuitFilter::default()
            },
            Some(10),
            Some(0),
        )
        .await?;
    let listed = rows
        .iter()
        .find(|r| r.record.id == circuit_id)
        .ok_or("seeded circuit missing from list")?;
    assert_eq!(
        listed.record.distance,
        Some(42.5),
        "the list projection must carry the same cast as the by-id read"
    );

    // Edit an unrelated field. `distance` is absent from the change set, so the
    // merge must keep the stored value rather than write back a decayed None.
    let changes = UpdateCircuit {
        cid: None,
        status: None,
        provider_id: None,
        type_id: None,
        tenant_id: None,
        provider_account_id: None,
        termination_a_id: None,
        termination_z_id: None,
        install_date: None,
        termination_date: None,
        commit_rate: None,
        description: Some("edited elsewhere".to_string()),
        comments: None,
        distance: None,
        distance_unit: None,
    };
    repo.update(circuit_id, &changes).await?;

    let stored: Option<f64> =
        sqlx::query_scalar("SELECT distance::float8 FROM circuits_circuit WHERE id = $1")
            .bind(circuit_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        stored,
        Some(42.5),
        "editing an unrelated field must not erase the operator's circuit length"
    );

    sqlx::query("DELETE FROM circuits_circuit WHERE id = $1")
        .bind(circuit_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM circuits_circuittype WHERE id = $1")
        .bind(type_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM circuits_provider WHERE id = $1")
        .bind(provider_id)
        .execute(&pool)
        .await?;
    Ok(())
}

/// `custom_field.validation_minimum/maximum` are NUMERIC read as `f64` through
/// the LOUD `try_column`. Because the same column list backs `RETURNING`, the
/// failure aborted create/update BEFORE commit — the "Minimum"/"Maximum"
/// validation feature could never be saved at all, and the operator got an
/// opaque 500. This pins create, read-back and list.
#[tokio::test]
#[ignore] // Requires PostgreSQL; run with: cargo test --test integration_numeric_columns -- --ignored
async fn custom_field_validation_bounds_round_trip()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use piping_backend::contexts::custom_fields::domain::{
        CustomFieldType, FilterLogic, NewCustomFieldDefinition, UiEditable, UiVisible,
    };
    use piping_backend::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::{
        CustomFieldDefinitionListFilter, CustomFieldDefinitionRepository,
        SqlxCustomFieldDefinitionRepository,
    };
    let pool = common::build_test_db_pool().await?;
    let repo = SqlxCustomFieldDefinitionRepository::new(pool.clone());
    let name = format!("numbounds_{}", common::unique_suffix());

    let input = NewCustomFieldDefinition {
        name: name.clone(),
        label: name.clone(),
        field_type: CustomFieldType::Integer,
        description: String::new(),
        group_name: String::new(),
        required: false,
        enforce_unique: false,
        is_secret: false,
        default_value: None,
        weight: 100,
        validation_minimum: Some(3.0),
        validation_maximum: Some(9.0),
        validation_regex: String::new(),
        validation_schema: None,
        choice_set_id: None,
        related_object_type_id: None,
        related_object_filter: None,
        ui_visible: UiVisible::Always,
        ui_editable: UiEditable::Yes,
        is_cloneable: true,
        search_weight: 1000,
        filter_logic: FilterLogic::Loose,
        object_types: Vec::new(),
    };

    // Before the cast this call itself failed: RETURNING could not be parsed,
    // so the transaction never reached commit.
    let created = repo.create(&input).await?;
    assert_eq!(created.validation_minimum, Some(3.0));
    assert_eq!(created.validation_maximum, Some(9.0));

    let fetched = repo.get(created.id).await?.ok_or("definition not found")?;
    assert_eq!(
        (fetched.validation_minimum, fetched.validation_maximum),
        (Some(3.0), Some(9.0)),
        "bounds must survive the by-id read"
    );

    // The list projection is a separate column list — it needs the same cast.
    let filter = CustomFieldDefinitionListFilter {
        search: Some(name.clone()),
        ..Default::default()
    };
    let page = repo.list_page(&filter, 10, 0).await?;
    let listed = page
        .iter()
        .find(|d| d.id == created.id)
        .ok_or("definition missing from list")?;
    assert_eq!(
        (listed.validation_minimum, listed.validation_maximum),
        (Some(3.0), Some(9.0)),
        "the list projection must carry the same cast"
    );

    repo.delete(created.id).await?;
    Ok(())
}
