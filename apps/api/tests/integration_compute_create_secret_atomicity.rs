//! Native compute and grouping creates roll back entity and secret writes together.
#[path = "common/child_database.rs"]
mod child_database;
mod common;

use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use sqlx::{PgPool, migrate::Migrator};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const FIELD: &str = "compute_atomic_secret";
const UPDATE_FIELD: &str = "compute_update_secret";
const MODE: &str = "PWA79_COMPUTE_ATOMIC_CHILD";

async fn request(
    app: &Router,
    cookie: Option<&str>,
    method: &str,
    path: &str,
    payload: Value,
) -> TestResult<(StatusCode, Value)> {
    let payload = payload.to_string();
    let (status, body) = common::request(app, method, path, Some(&payload), cookie).await?;
    Ok((
        status,
        if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)?
        },
    ))
}

async fn create(app: &Router, cookie: &str, path: &str, payload: Value) -> TestResult<Value> {
    let (status, body) = request(app, Some(cookie), "POST", path, payload).await?;
    // Response bodies can contain credentials. Only the status is diagnostic.
    assert!(
        status == StatusCode::CREATED,
        "fixture create failed: {path}, {status}"
    );
    Ok(body)
}

fn id(value: &Value) -> TestResult<Uuid> {
    Ok(Uuid::parse_str(
        value["id"].as_str().ok_or("missing fixture id")?,
    )?)
}

async fn snapshot(pool: &PgPool) -> TestResult<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    // Include native materialization, reference counters and every secret owner.
    // Static table names only; snapshots never appear in assertion diagnostics.
    for table in [
        "dcim_device",
        "virtualization_virtualmachine",
        "dcim_rackgroup",
        "dcim_cablebundle",
        "dcim_devicetype",
        "dcim_devicerole",
        "dcim_site",
        "dcim_platform",
        "virtualization_cluster",
        "virtualization_virtualmachinetype",
        "tenancy_tenant",
        "dcim_consoleport",
        "dcim_consoleserverport",
        "dcim_interface",
        "dcim_powerport",
        "dcim_poweroutlet",
        "dcim_frontport",
        "dcim_rearport",
        "dcim_devicebay",
        "dcim_modulebay",
        "dcim_inventoryitem",
        "dcim_coolingintake",
        "dcim_coolingoutflow",
        "secrets_variable",
        "secrets_store",
        "secrets_binding",
        "secret_usage_binding",
    ] {
        let rows: Value = sqlx::query_scalar(&format!("SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM {table} t"))
            .fetch_one(pool).await?;
        result.insert(table.into(), rows);
    }
    Ok(result)
}

fn payload(base: &Value, name: &str) -> Value {
    let mut value = base.clone();
    value["name"] = json!(name);
    if value.get("slug").is_some() {
        value["slug"] = json!(name);
    }
    // Synthetic values are compared only with boolean assertions below.
    value["custom_fields"] = json!({FIELD: format!("synthetic-{}", Uuid::now_v7())});
    value
}

async fn probe(pool: &PgPool, sequence: &str) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar(&format!("SELECT last_value FROM {sequence}"))
            .fetch_one(pool)
            .await?,
    )
}

async fn exercise(url: &str, encryption_fault: bool) -> TestResult {
    let pool = PgPool::connect(url).await?;
    Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await?
    .run(&pool)
    .await?;
    let mut settings = piping_backend::config::load_settings()?;
    settings.database.url = url.into();
    settings.redis.url = common::redis_url(Some(14))?;
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    if encryption_fault {
        // A separate process owns the immutable cipher singleton. A one-byte
        // synthetic key fails at encrypt_secret, not at request validation.
        settings.secrets.cipher.key_material_ref = "inline:AA==".into();
    } else {
        common::configure_test_cipher(&mut settings);
    }
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    common::ensure_test_admin(&pool, &settings.auth.password_pepper).await?;
    let state =
        piping_backend::app::AppState::build(Arc::new(settings), pool.clone(), redis).await?;
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("fixture login failed")?;

    let (seed, _) = common::seed_device(&pool, "compute-atomic").await?;
    let refs: (Uuid, Uuid, Uuid) =
        sqlx::query_as("SELECT site_id, role_id, device_type_id FROM dcim_device WHERE id=$1")
            .bind(seed)
            .fetch_one(&pool)
            .await?;
    sqlx::query("INSERT INTO dcim_consoleporttemplate (id, name, label, description, device_type_id) VALUES ($1, 'console-atomic', '', '', $2)")
        .bind(Uuid::now_v7()).bind(refs.2).execute(&pool).await?;
    // Module bays have a native graph-maintained device counter; console-port
    // counters are currently rebuilt by the sync path rather than native writes.
    sqlx::query("INSERT INTO dcim_modulebaytemplate (name, label, description, device_type_id, position, enabled) VALUES ('bay-atomic', '', '', $1, 'A', true)")
        .bind(refs.2).execute(&pool).await?;
    let vm_type = create(&app, &cookie, "/api/v1/inventory/virtual-machine-types", json!({"name":"atomic-type", "slug":"atomic-type", "default_vcpus":2, "default_memory":4096})).await?;
    let cases = [
        (
            "device",
            "dcim.device",
            "/api/v1/inventory/devices",
            "devices",
            "dcim_device",
            json!({"site_id":refs.0,"role_id":refs.1,"device_type_id":refs.2}),
        ),
        (
            "vm",
            "virtualization.virtualmachine",
            "/api/v1/inventory/virtual_machines",
            "virtual-machines",
            "virtualization_virtualmachine",
            json!({"virtual_machine_type_id":id(&vm_type)?}),
        ),
        (
            "rack",
            "dcim.rackgroup",
            "/api/v1/inventory/rack-groups",
            "rack-groups",
            "dcim_rackgroup",
            json!({"slug":"placeholder"}),
        ),
        (
            "bundle",
            "dcim.cablebundle",
            "/api/v1/inventory/cable-bundles",
            "cable-bundles",
            "dcim_cablebundle",
            json!({}),
        ),
    ];
    let (status, metadata) = request(
        &app,
        Some(&cookie),
        "GET",
        "/api/v1/inventory/content-types",
        Value::Null,
    )
    .await?;
    assert!(status == StatusCode::OK, "content types unavailable");
    let metadata = metadata
        .as_array()
        .ok_or("content types are not an array")?;
    let types: Vec<_> = cases
        .iter()
        .map(|(_, token, _, _, _, _)| {
            metadata
                .iter()
                .find(|v| v["token"] == *token)
                .ok_or("content type missing")
                .and_then(|v| v["id"].as_str().ok_or("content type id missing"))
        })
        .collect::<Result<_, _>>()?;
    // Seed update targets before the required create-only field is defined.
    let mut update_targets = BTreeMap::new();
    for (kind, _, collection, _, _, base) in &cases[2..] {
        let mut fixture = base.clone();
        fixture["name"] = json!(format!("{kind}-update-original"));
        if fixture.get("slug").is_some() {
            fixture["slug"] = json!(format!("{kind}-update-original"));
        }
        fixture["comments"] = json!("original comments");
        let row = create(&app, &cookie, collection, fixture).await?;
        update_targets.insert(*kind, id(&row)?);
    }
    let update_types = vec![types[2], types[3]];
    create(&app, &cookie, "/api/v1/custom-fields", json!({"name":UPDATE_FIELD,"label":UPDATE_FIELD,"field_type":"text","is_secret":true,"object_types":update_types})).await?;
    create(&app, &cookie, "/api/v1/custom-fields", json!({"name":"compute_update_note","label":"Update note","field_type":"text","object_types":update_types})).await?;
    create(&app, &cookie, "/api/v1/custom-fields", json!({"name":FIELD,"label":FIELD,"field_type":"text","is_secret":true,"required":true,"object_types":types})).await?;
    sqlx::query("CREATE SEQUENCE compute_atomic_entity_probe START 1")
        .execute(&pool)
        .await?;
    sqlx::query("SELECT setval('compute_atomic_entity_probe',1,true)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE FUNCTION compute_atomic_entity_probe() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('compute_atomic_entity_probe'); RETURN NEW; END $$").execute(&pool).await?;
    sqlx::query("CREATE SEQUENCE compute_atomic_store_probe START 1")
        .execute(&pool)
        .await?;
    sqlx::query("SELECT setval('compute_atomic_store_probe',1,true)")
        .execute(&pool)
        .await?;

    for (kind, _, collection, reveal_entity, table, base) in cases {
        sqlx::query(&format!("CREATE TRIGGER compute_atomic_entity_probe AFTER INSERT ON {table} FOR EACH ROW EXECUTE FUNCTION compute_atomic_entity_probe()" )).execute(&pool).await?;
        if !encryption_fault {
            // Find the resource through its managed variable key before usage
            // attachment. Only '-bad' rows fail, so bulk can recover afterwards.
            sqlx::query(&format!("CREATE FUNCTION compute_atomic_reject_store() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('compute_atomic_store_probe'); IF EXISTS (SELECT 1 FROM secrets_variable v JOIN {table} r ON position(r.id::text in v.key)>0 WHERE v.id=NEW.secret_id AND r.name LIKE '%-bad') THEN RAISE EXCEPTION 'synthetic compute storage failure'; END IF; RETURN NEW; END $$")).execute(&pool).await?;
            sqlx::query("CREATE TRIGGER compute_atomic_reject_store BEFORE INSERT OR UPDATE ON secrets_store FOR EACH ROW EXECUTE FUNCTION compute_atomic_reject_store()").execute(&pool).await?;
        }
        if let Some(target) = update_targets.get(kind) {
            exercise_group_update(
                &app,
                &cookie,
                &pool,
                (kind, collection, table),
                *target,
                encryption_fault,
            )
            .await?;
        }
        let bad = payload(&base, &format!("{kind}-single-bad"));
        let before = snapshot(&pool).await?;
        let entity_probe = probe(&pool, "compute_atomic_entity_probe").await?;
        let store_probe = probe(&pool, "compute_atomic_store_probe").await?;
        let (status, failure) =
            request(&app, Some(&cookie), "POST", collection, bad.clone()).await?;
        assert!(
            status.is_server_error(),
            "{kind}: late create did not report failure"
        );
        if encryption_fault {
            assert!(
                status == StatusCode::SERVICE_UNAVAILABLE && failure["code"] == "INVALID_CONFIG",
                "{kind}: expected late cipher configuration failure"
            );
            assert!(
                probe(&pool, "compute_atomic_store_probe").await? == store_probe,
                "{kind}: encryption failure unexpectedly reached ciphertext storage"
            );
        }
        assert!(
            probe(&pool, "compute_atomic_entity_probe").await? > entity_probe,
            "{kind}: fault was not after native INSERT"
        );
        assert!(
            snapshot(&pool).await? == before,
            "{kind}: late create left entity, component, counter or secret state"
        );
        if !encryption_fault {
            assert!(
                probe(&pool, "compute_atomic_store_probe").await? > store_probe,
                "{kind}: storage fault not reached"
            );
        }

        let bulk_items = if encryption_fault {
            vec![bad]
        } else {
            vec![
                payload(&base, &format!("{kind}-bulk-first")),
                payload(&base, &format!("{kind}-bulk-bad")),
                payload(&base, &format!("{kind}-bulk-last")),
            ]
        };
        let expected_values: Vec<_> = bulk_items
            .iter()
            .map(|v| v["custom_fields"][FIELD].clone())
            .collect();
        let entity_probe = probe(&pool, "compute_atomic_entity_probe").await?;
        let (status, bulk) = request(
            &app,
            Some(&cookie),
            "POST",
            &format!("{collection}/bulk"),
            json!({"items":bulk_items}),
        )
        .await?;
        assert!(status == StatusCode::OK, "{kind}: bulk envelope failed");
        assert!(
            probe(&pool, "compute_atomic_entity_probe").await? > entity_probe,
            "{kind}: bulk fault did not reach native INSERT"
        );
        assert!(
            bulk["errors"].as_array().is_some_and(|v| v.len() == 1),
            "{kind}: bulk must report one failed item"
        );
        let created = bulk["created"].as_array().ok_or("bulk created missing")?;
        assert!(
            created.len() == if encryption_fault { 0 } else { 2 },
            "{kind}: bulk partial success lost"
        );
        if encryption_fault {
            assert!(
                snapshot(&pool).await? == before,
                "{kind}: encryption failure left bulk state"
            );
        } else {
            let missing: i64 = sqlx::query_scalar(&format!(
                "SELECT count(*) FROM {table} WHERE name LIKE '%-bad'"
            ))
            .fetch_one(&pool)
            .await?;
            assert!(missing == 0, "{kind}: failed bulk row persisted");
            for (position, row) in created.iter().enumerate() {
                verify_mask_and_reveal(
                    &app,
                    &cookie,
                    collection,
                    reveal_entity,
                    row,
                    &expected_values[position * 2],
                )
                .await?;
            }
            let single_payload = payload(&base, &format!("{kind}-single-ok"));
            let expected = single_payload["custom_fields"][FIELD].clone();
            let row = create(&app, &cookie, collection, single_payload).await?;
            verify_mask_and_reveal(&app, &cookie, collection, reveal_entity, &row, &expected)
                .await?;
            verify_scoped_access(
                &app,
                &cookie,
                collection,
                reveal_entity,
                id(&row)?,
                &expected,
            )
            .await?;
            let stored: Value = sqlx::query_scalar(&format!(
                "SELECT custom_field_data FROM {table} WHERE id=$1"
            ))
            .bind(id(&row)?)
            .fetch_one(&pool)
            .await?;
            assert!(
                stored.get(FIELD).is_none(),
                "{kind}: secret entered entity JSON"
            );
            if kind == "device" {
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_consoleport WHERE device_id=$1 AND name='console-atomic'").bind(id(&row)?).fetch_one(&pool).await?;
                assert!(count == 1, "device native template materialization lost");
                let count: i64 =
                    sqlx::query_scalar("SELECT console_port_count FROM dcim_device WHERE id=$1")
                        .bind(id(&row)?)
                        .fetch_one(&pool)
                        .await?;
                assert!(count == 1, "device native component counter lost");
                let count: i64 =
                    sqlx::query_scalar("SELECT module_bay_count FROM dcim_device WHERE id=$1")
                        .bind(id(&row)?)
                        .fetch_one(&pool)
                        .await?;
                assert!(count == 1, "device native bay counter lost");
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM dcim_modulebay WHERE device_id=$1 AND name='bay-atomic'",
                )
                .bind(id(&row)?)
                .fetch_one(&pool)
                .await?;
                assert!(count == 1, "device native bay materialization lost");
            }
            if kind == "vm" {
                assert!(
                    row["vcpus"] == json!(2.0) && row["memory"] == json!(4096),
                    "VM type defaults lost"
                );
            }
            sqlx::query("DROP TRIGGER compute_atomic_reject_store ON secrets_store")
                .execute(&pool)
                .await?;
            sqlx::query("DROP FUNCTION compute_atomic_reject_store()")
                .execute(&pool)
                .await?;
        }
        sqlx::query(&format!(
            "DROP TRIGGER compute_atomic_entity_probe ON {table}"
        ))
        .execute(&pool)
        .await?;
    }
    drop(app);
    pool.close().await;
    Ok(())
}

async fn exercise_group_update(
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    resource: (&str, &str, &str),
    entity_id: Uuid,
    encryption_fault: bool,
) -> TestResult {
    let (kind, collection, table) = resource;
    let path = format!("{collection}/{entity_id}");
    // This probe survives rollback and proves the fault followed native UPDATE.
    sqlx::query(&format!("CREATE TRIGGER compute_atomic_update_probe AFTER UPDATE ON {table} FOR EACH ROW EXECUTE FUNCTION compute_atomic_entity_probe()")).execute(pool).await?;
    let original_secret = json!(format!("synthetic-update-{}", Uuid::now_v7()));
    if !encryption_fault {
        let (status, row) = request(app, Some(cookie), "PUT", &path, json!({"custom_fields":{UPDATE_FIELD:original_secret,"compute_update_note":"original note"}})).await?;
        assert!(
            status == StatusCode::OK && row["custom_fields"][UPDATE_FIELD] == "****",
            "{kind}: update setup failed or secret not masked"
        );
    }
    let before = snapshot(pool).await?;
    let probe_before = probe(pool, "compute_atomic_entity_probe").await?;
    let store_before = probe(pool, "compute_atomic_store_probe").await?;
    let (status, _) = request(
        app,
        Some(cookie),
        "PUT",
        &format!("{collection}/{}", Uuid::now_v7()),
        json!({"custom_fields":{UPDATE_FIELD:format!("synthetic-missing-{}", Uuid::now_v7())}}),
    )
    .await?;
    assert!(
        status == StatusCode::NOT_FOUND && snapshot(pool).await? == before,
        "{kind}: missing update target did not return 404 without secret writes"
    );
    let attempted = json!({"name":format!("{kind}-update-bad"),"comments":"must roll back","owner_id":null,"tags":[],"custom_fields":{UPDATE_FIELD:format!("synthetic-replacement-{}", Uuid::now_v7()),"compute_update_note":"must roll back"}});
    let (status, _) = request(app, None, "PUT", &path, attempted.clone()).await?;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{kind}: anonymous update not denied"
    );
    assert!(
        snapshot(pool).await? == before
            && probe(pool, "compute_atomic_entity_probe").await? == probe_before,
        "{kind}: denied update performed DML"
    );
    let (status, failure) = request(app, Some(cookie), "PUT", &path, attempted).await?;
    assert!(
        status.is_server_error(),
        "{kind}: late update fault not reported"
    );
    assert!(
        probe(pool, "compute_atomic_entity_probe").await? > probe_before,
        "{kind}: late fault did not follow UPDATE"
    );
    assert!(
        snapshot(pool).await? == before,
        "{kind}: late update left metadata, references, JSON or secret changes"
    );
    if encryption_fault {
        assert!(
            status == StatusCode::SERVICE_UNAVAILABLE && failure["code"] == "INVALID_CONFIG",
            "{kind}: expected encryption failure on update"
        );
        assert!(
            probe(pool, "compute_atomic_store_probe").await? == store_before,
            "{kind}: cipher fault reached storage"
        );
    } else {
        assert!(
            probe(pool, "compute_atomic_store_probe").await? > store_before,
            "{kind}: update storage fault not reached"
        );
        let (status, revealed) = request(
            app,
            Some(cookie),
            "POST",
            &format!("{path}/custom-fields/reveal"),
            json!({}),
        )
        .await?;
        assert!(
            status == StatusCode::OK && revealed["custom_fields"][UPDATE_FIELD] == original_secret,
            "{kind}: failed update changed existing secret"
        );

        // Omitted fields preserve owner/tags and secret bindings; a masked value
        // also means keep. Neither request may rotate ciphertext or bindings.
        let (status, original) = request(app, Some(cookie), "GET", &path, Value::Null).await?;
        assert!(
            status == StatusCode::OK,
            "{kind}: update target unavailable"
        );
        for update in [
            json!({"comments":"metadata only"}),
            json!({"custom_fields":{UPDATE_FIELD:"****","compute_update_note":"changed note"}}),
        ] {
            let secret_before = snapshot(pool).await?;
            let (status, row) = request(app, Some(cookie), "PUT", &path, update).await?;
            assert!(
                status == StatusCode::OK && row["custom_fields"][UPDATE_FIELD] == "****",
                "{kind}: omission or mask update failed"
            );
            assert!(
                row["owner_id"] == original["owner_id"] && row["tags"] == original["tags"],
                "{kind}: omitted references or tags changed"
            );
            let after = snapshot(pool).await?;
            for key in [
                "secrets_variable",
                "secrets_store",
                "secrets_binding",
                "secret_usage_binding",
            ] {
                assert!(
                    after[key] == secret_before[key],
                    "{kind}: omission or mask changed secret state"
                );
            }
        }
        let replacement = json!(format!("synthetic-success-{}", Uuid::now_v7()));
        let (status, row) = request(
            app,
            Some(cookie),
            "PUT",
            &path,
            json!({"comments":"committed metadata","custom_fields":{UPDATE_FIELD:replacement}}),
        )
        .await?;
        assert!(
            status == StatusCode::OK
                && row["comments"] == "committed metadata"
                && row["custom_fields"][UPDATE_FIELD] == "****",
            "{kind}: successful update lost metadata or masking"
        );
        let (status, revealed) = request(
            app,
            Some(cookie),
            "POST",
            &format!("{path}/custom-fields/reveal"),
            json!({}),
        )
        .await?;
        assert!(
            status == StatusCode::OK
                && revealed["custom_fields"][UPDATE_FIELD] == replacement
                && revealed["custom_fields"]["compute_update_note"] == "changed note",
            "{kind}: successful update lost secret or omitted JSON keys"
        );
        let (status, _) = request(
            app,
            Some(cookie),
            "PUT",
            &path,
            json!({"owner_id":null,"tags":[],"custom_fields":{UPDATE_FIELD:null}}),
        )
        .await?;
        assert!(status == StatusCode::OK, "{kind}: explicit clear failed");
        let usages: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM secret_usage_binding WHERE consumer_id=$1 AND field_key=$2",
        )
        .bind(entity_id)
        .bind(UPDATE_FIELD)
        .fetch_one(pool)
        .await?;
        assert!(usages == 0, "{kind}: clear retained secret usage");
        let (status, row) = request(app, Some(cookie), "GET", &path, Value::Null).await?;
        assert!(
            status == StatusCode::OK
                && row["owner_id"].is_null()
                && row["tags"] == json!([])
                && row["custom_fields"][UPDATE_FIELD] != "****",
            "{kind}: clear did not persist"
        );
    }
    sqlx::query(&format!(
        "DROP TRIGGER compute_atomic_update_probe ON {table}"
    ))
    .execute(pool)
    .await?;
    Ok(())
}

async fn verify_mask_and_reveal(
    app: &Router,
    cookie: &str,
    collection: &str,
    reveal_entity: &str,
    row: &Value,
    expected: &Value,
) -> TestResult {
    assert!(
        row["custom_fields"][FIELD] == "****",
        "create response did not mask secret"
    );
    let path = format!("{collection}/{}", id(row)?);
    let (status, read) = request(app, Some(cookie), "GET", &path, Value::Null).await?;
    assert!(
        status == StatusCode::OK && read["custom_fields"][FIELD] == "****",
        "GET did not mask secret"
    );
    let reveal = format!(
        "/api/v1/inventory/{reveal_entity}/{}/custom-fields/reveal",
        id(row)?
    );
    let (status, read) = request(app, Some(cookie), "POST", &reveal, json!({})).await?;
    assert!(
        status == StatusCode::OK && read["custom_fields"][FIELD] == *expected,
        "authorized reveal lost managed value"
    );
    let (status, _) = request(app, None, "POST", &reveal, json!({})).await?;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "anonymous reveal was not denied"
    );
    Ok(())
}

async fn verify_scoped_access(
    app: &Router,
    cookie: &str,
    collection: &str,
    reveal_entity: &str,
    entity_id: Uuid,
    expected: &Value,
) -> TestResult {
    let path = format!("{collection}/{entity_id}");
    let read = json!({"action":"read", "resource_type":"inventory", "resource_pattern":format!("inventory:{entity_id}")});
    let secret = json!({"action":"secret", "resource_type":"inventory", "resource_pattern":format!("inventory:{entity_id}")});
    let other = json!({"action":"secret", "resource_type":"inventory", "resource_pattern":format!("inventory:{}", Uuid::now_v7())});
    for (case, scopes, allowed) in [
        ("read-only", json!([read.clone()]), false),
        ("secret-only", json!([secret.clone()]), false),
        ("wrong-object", json!([read.clone(), other]), false),
        ("read-secret", json!([read, secret]), true),
    ] {
        let token = create(
            app,
            cookie,
            "/api/v1/user/settings/api-tokens",
            json!({"name":format!("compute-{case}-{}", Uuid::now_v7().simple()), "scopes":scopes}),
        )
        .await?;
        let bearer = token["token_value"]
            .as_str()
            .ok_or("missing synthetic token")?;
        let (status, body) = common::request_with_auth(
            app,
            "POST",
            &format!("/api/v1/inventory/{reveal_entity}/{entity_id}/custom-fields/reveal"),
            None,
            None,
            Some(bearer),
        )
        .await?;
        assert!(
            status
                == if allowed {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
            "{collection}: {case} scoped reveal permission changed with {status}"
        );
        if allowed {
            let value: Value = serde_json::from_slice(&body)?;
            assert!(
                value["custom_fields"][FIELD] == *expected,
                "scoped reveal lost managed value"
            );
        }
        if case != "secret-only" {
            let (status, body) =
                common::request_with_auth(app, "GET", &path, None, None, Some(bearer)).await?;
            let value: Value = serde_json::from_slice(&body)?;
            assert!(
                status == StatusCode::OK,
                "{collection}: {case} scoped ordinary GET failed with {status}"
            );
            assert!(
                value["custom_fields"][FIELD] == "****",
                "{collection}: {case} scoped ordinary GET did not remain masked"
            );
        } else {
            let (status, _) =
                common::request_with_auth(app, "GET", &path, None, None, Some(bearer)).await?;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{collection}: secret-only scope must not grant ordinary GET"
            );
        }
        let other_id = Uuid::now_v7();
        let other_path = format!("{collection}/{other_id}");
        let (status, _) =
            common::request_with_auth(app, "GET", &other_path, None, None, Some(bearer)).await?;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{collection}: {case} scope must not grant GET for another object"
        );
        let (status, _) = common::request_with_auth(
            app,
            "POST",
            &format!("/api/v1/inventory/{reveal_entity}/{other_id}/custom-fields/reveal"),
            None,
            None,
            Some(bearer),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{collection}: {case} scope must not reveal another object"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; each cipher mode uses a fresh process and disposable child database"]
async fn native_compute_creates_are_atomic_with_secret_fields() -> TestResult {
    let mode = std::env::var(MODE).ok();
    if let Some(mode) = mode {
        if mode != "encryption" && mode != "storage" {
            return Err("invalid compute atomicity worker mode".into());
        }
        let url = std::env::var("PWA79_ATOMIC_CHILD_URL")?;
        child_database::ChildDatabase::validate_url("compute_atomic_", &url)?;
        exercise(&url, mode == "encryption").await?;
        return Ok(());
    }
    // Cipher configuration is process-global and immutable. Do not mutate the
    // environment of a running async runtime or add production fault hooks.
    for mode in ["encryption", "storage"] {
        let (child, url) = child_database::ChildDatabase::create("compute_atomic_").await?;
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .args([
                "--ignored",
                "--exact",
                "native_compute_creates_are_atomic_with_secret_fields",
                "--test-threads=1",
            ])
            .env(MODE, mode)
            .env("PWA79_ATOMIC_CHILD_URL", url)
            .kill_on_drop(true);
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(180), command.output()).await;
        let cleanup = child.cleanup().await;
        let output = output??;
        assert!(
            output.status.success(),
            "{mode} child failed; output suppressed to protect secret-bearing diagnostics"
        );
        cleanup?;
    }
    Ok(())
}
