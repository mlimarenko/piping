//! DB-backed integration tests for the materialized generic-FK `ObjectRef`
//! projection: the batch resolver labels (qualified vs bare vs dangling) and the
//! end-to-end REST round-trips for the entities that own a polymorphic target.
//!
//! `#[ignore]`d like the other DB integration tests; run with the dev Postgres
//! (`make test`) or:
//!
//! ```text
//! cargo test --test integration_object_ref -- --ignored --test-threads=1
//! ```

mod common;

use axum::http::StatusCode;
use piping_backend::contexts::inventory::infrastructure::persistence::SqlxObjectRefResolver;
use sqlx::Row;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

async fn migrated_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    Ok(pool)
}

/// Look up a seeded `inventory_content_type` id by token.
async fn content_type_id(
    pool: &sqlx::PgPool,
    app_label: &str,
    model: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query("SELECT id FROM inventory_content_type WHERE app_label = $1 AND model = $2")
            .bind(app_label)
            .bind(model)
            .fetch_one(pool)
            .await
            .map_err(|e| format!("content type seeded: {e}"))?
            .try_get("id")
            .map_err(|e| format!("content type id: {e}"))?,
    )
}

/// Seed a `dcim_interface` on `device_id`; return its id and name.
async fn seed_interface(
    pool: &sqlx::PgPool,
    device_id: Uuid,
) -> Result<(Uuid, String), Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::now_v7();
    let name = format!("eth-{}", common::unique_suffix());
    sqlx::query(
        r#"
        INSERT INTO dcim_interface (
            id, custom_field_data, name, label, description, mark_connected,
            enabled, _name, type, mgmt_only, device_id
        )
        VALUES ($1, '{}'::jsonb, $2, $2, '', false, true, $2, 'virtual', false, $3)
        "#,
    )
    .bind(id)
    .bind(&name)
    .bind(device_id)
    .execute(pool)
    .await
    .map_err(|e| format!("seed interface: {e}"))?;
    Ok((id, name))
}

/// Seed a `tenancy_tenant`; return its id and name.
async fn seed_tenant(
    pool: &sqlx::PgPool,
) -> Result<(Uuid, String), Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::now_v7();
    let name = format!("objref-tenant-{}", common::unique_suffix());
    sqlx::query(
        r#"
        INSERT INTO tenancy_tenant (id, custom_field_data, name, slug, description, comments)
        VALUES ($1, '{}'::jsonb, $2, $2, '', '')
        "#,
    )
    .bind(id)
    .bind(&name)
    .execute(pool)
    .await
    .map_err(|e| format!("seed tenant: {e}"))?;
    Ok((id, name))
}

// ---------------------------------------------------------------------------
// Resolver label behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires database (run via make test)"]
async fn resolver_builds_qualified_bare_and_dangling_labels()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let resolver = SqlxObjectRefResolver::new(pool.clone());

    let (device_id, device_name) = common::seed_device(&pool, "objref").await?;
    let (iface_id, iface_name) = seed_interface(&pool, device_id).await?;
    let (tenant_id, tenant_name) = seed_tenant(&pool).await?;

    let iface_ct = content_type_id(&pool, "dcim", "interface").await?;
    let device_ct = content_type_id(&pool, "dcim", "device").await?;
    let tenant_ct = content_type_id(&pool, "tenancy", "tenant").await?;

    // Interface has a parent device → qualified "name @ device".
    let dangling_object_id = Uuid::now_v7();
    let map = resolver
        .resolve(&[
            (iface_ct, iface_id),
            (tenant_ct, tenant_id),
            (device_ct, device_id),
            (iface_ct, dangling_object_id),
        ])
        .await
        .expect("resolve");

    let iface_ref = map.get(&(iface_ct, iface_id)).expect("iface ref");
    assert_eq!(iface_ref.object_type, "dcim.interface");
    assert_eq!(
        iface_ref.label.as_deref(),
        Some(format!("{iface_name} @ {device_name}").as_str()),
        "interface label should be qualified by its device"
    );

    // Tenant has no parent → bare name.
    let tenant_ref = map.get(&(tenant_ct, tenant_id)).expect("tenant ref");
    assert_eq!(tenant_ref.object_type, "tenancy.tenant");
    assert_eq!(tenant_ref.label.as_deref(), Some(tenant_name.as_str()));

    // Device has no parent → bare name.
    let device_ref = map.get(&(device_ct, device_id)).expect("device ref");
    assert_eq!(device_ref.label.as_deref(), Some(device_name.as_str()));

    // Dangling: known type, missing row → label None, never a 500.
    let dangling = map
        .get(&(iface_ct, dangling_object_id))
        .expect("dangling ref present");
    assert_eq!(dangling.object_type, "dcim.interface");
    assert_eq!(dangling.label, None);
    Ok(())
}

// ---------------------------------------------------------------------------
// REST round-trips
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires database (run via make test)"]
async fn mac_assigned_object_round_trips_and_omits_raw_fields()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state()
        .await
        .expect("test app state must build; this suite is #[ignore]d and only runs in the DB lane");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("login");

    let (device_id, device_name) = common::seed_device(&pool, "objref").await?;
    let (iface_id, iface_name) = seed_interface(&pool, device_id).await?;

    // Create a MAC assigned to the interface via the canonical payload.
    let create_body = serde_json::json!({
        "mac_address": "00:11:22:33:44:55",
        "assigned_object": { "object_type": "dcim.interface", "object_id": iface_id },
    })
    .to_string();
    let (status, body) = common::request(
        &app,
        "POST",
        "/api/v1/inventory/mac-addresses",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("create mac");
    assert_eq!(status, StatusCode::CREATED, "create body: {body:?}");
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let mac_id = created.get("id").and_then(|v| v.as_str()).expect("mac id");

    // Raw generic-FK fields must be ABSENT from the response.
    assert!(created.get("assigned_object_type_id").is_none());
    assert!(created.get("assigned_object_id").is_none());

    // GET round-trip: assigned_object.label is the qualified interface label.
    let (status, body) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/mac-addresses/{mac_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get mac");
    assert_eq!(status, StatusCode::OK);
    let fetched: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let assigned = fetched.get("assigned_object").expect("assigned_object");
    assert_eq!(
        assigned.get("object_type").and_then(|v| v.as_str()),
        Some("dcim.interface")
    );
    assert_eq!(
        assigned.get("label").and_then(|v| v.as_str()),
        Some(format!("{iface_name} @ {device_name}").as_str())
    );
    assert!(fetched.get("assigned_object_type_id").is_none());
    assert!(fetched.get("assigned_object_id").is_none());
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (run via make test)"]
async fn mac_rejects_invalid_object_type() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state()
        .await
        .expect("test app state must build; this suite is #[ignore]d and only runs in the DB lane");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("login");

    let (device_id, _) = common::seed_device(&pool, "objref").await?;
    let (iface_id, _) = seed_interface(&pool, device_id).await?;

    // `dcim.device` is NOT in the MAC allowed subset → 400.
    let body = serde_json::json!({
        "mac_address": "66:77:88:99:aa:bb",
        "assigned_object": { "object_type": "dcim.device", "object_id": iface_id },
    })
    .to_string();
    let (status, _) = common::request(
        &app,
        "POST",
        "/api/v1/inventory/mac-addresses",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create mac");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (run via make test)"]
async fn service_parent_object_round_trips_to_device_label()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state()
        .await
        .expect("test app state must build; this suite is #[ignore]d and only runs in the DB lane");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("login");

    let (device_id, device_name) = common::seed_device(&pool, "objref").await?;

    let create_body = serde_json::json!({
        "name": format!("svc-{}", common::unique_suffix()),
        "protocol": "tcp",
        "ports": [22],
        "parent_object": { "object_type": "dcim.device", "object_id": device_id },
    })
    .to_string();
    let (status, body) = common::request(
        &app,
        "POST",
        "/api/v1/inventory/services",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("create service");
    assert_eq!(status, StatusCode::CREATED, "create body: {body:?}");
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");

    let parent = created.get("parent_object").expect("parent_object");
    assert_eq!(
        parent.get("object_type").and_then(|v| v.as_str()),
        Some("dcim.device")
    );
    assert_eq!(
        parent.get("label").and_then(|v| v.as_str()),
        Some(device_name.as_str())
    );
    // Raw pair must be absent.
    assert!(created.get("parent_object_type_id").is_none());
    assert!(created.get("parent_object_id").is_none());
    Ok(())
}
