//! Wireless entity and custom-field secret failures share one commit boundary.
mod common;

use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use sqlx::{
    PgPool, Postgres,
    migrate::{MigrateDatabase, Migrator},
};
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn request(
    app: &Router,
    cookie: &str,
    method: &str,
    path: &str,
    payload: Value,
) -> TestResult<(StatusCode, Value)> {
    let payload = payload.to_string();
    let (status, body) = common::request(app, method, path, Some(&payload), Some(cookie)).await?;
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
    let (status, value) = request(app, cookie, "POST", path, payload).await?;
    // Never print response bodies or secret snapshots, including on fixture failure.
    if status != StatusCode::CREATED {
        return Err(format!("Atomicity fixture creation failed: {path}, {status}").into());
    }
    Ok(value)
}

fn id(value: &Value) -> TestResult<Uuid> {
    Ok(Uuid::parse_str(
        value["id"].as_str().ok_or("Missing fixture UUID")?,
    )?)
}

async fn payload(
    app: &Router,
    cookie: &str,
    kind: &str,
    name: &str,
    device: Uuid,
) -> TestResult<Value> {
    Ok(match kind {
        "group" => json!({"name":name, "slug":name}),
        "lan" => json!({"ssid":name}),
        "link" => {
            let a = create(
                app,
                cookie,
                "/api/v1/inventory/interfaces",
                json!({"device_id":device, "name":format!("{name}-a"), "type":"ieee802.11ax"}),
            )
            .await?;
            let b = create(
                app,
                cookie,
                "/api/v1/inventory/interfaces",
                json!({"device_id":device, "name":format!("{name}-b"), "type":"ieee802.11ax"}),
            )
            .await?;
            json!({"ssid":name, "interface_a_id":id(&a)?, "interface_b_id":id(&b)?})
        }
        _ => return Err("Unknown Wireless fixture kind".into()),
    })
}

async fn entity(pool: &PgPool, table: &str, id: Uuid) -> TestResult<Value> {
    // The table is a static test-owned Wireless table, never request data.
    Ok(
        sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"))
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

async fn count(pool: &PgPool, table: &str) -> TestResult<i64> {
    Ok(sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await?)
}

async fn secrets(pool: &PgPool) -> TestResult<Value> {
    // Compare ciphertext and binding snapshots without exposing any field values.
    Ok(sqlx::query_scalar("SELECT jsonb_build_object(
        'variables', (SELECT COALESCE(jsonb_agg(to_jsonb(v) ORDER BY id), '[]'::jsonb) FROM secrets_variable v),
        'store', (SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY secret_id), '[]'::jsonb) FROM secrets_store s),
        'bindings', (SELECT COALESCE(jsonb_agg(to_jsonb(b) ORDER BY id), '[]'::jsonb) FROM secrets_binding b),
        'usage', (SELECT COALESCE(jsonb_agg(to_jsonb(u) ORDER BY id), '[]'::jsonb) FROM secret_usage_binding u))")
        .fetch_one(pool).await?)
}

fn check(failures: &mut Vec<String>, condition: bool, kind: &str, claim: &str) {
    if !condition {
        failures.push(format!("{kind}: {claim}"));
    }
}

async fn store_failure_probe(pool: &PgPool) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT last_value FROM wireless_atomic_store_probe")
            .fetch_one(pool)
            .await?,
    )
}

async fn verify_reveal_access(
    app: &Router,
    cookie: &str,
    kind: &str,
    collection: &str,
    entity_id: Uuid,
    expected_secret: &Value,
    failures: &mut Vec<String>,
) -> TestResult {
    let path = format!("{collection}/{entity_id}");
    let reveal = format!("{path}/custom-fields/reveal");
    let (status, value) = request(app, cookie, "POST", &reveal, json!({})).await?;
    check(
        failures,
        status == StatusCode::OK,
        kind,
        "authorized reveal succeeds",
    );
    check(
        failures,
        value["custom_fields"]["wireless_atomic_secret"] == *expected_secret,
        kind,
        "authorized reveal resolves this object's managed secret",
    );
    let (status, _) = common::request(app, "POST", &reveal, None, None).await?;
    check(
        failures,
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        kind,
        "anonymous reveal denied",
    );
    let read = json!({"action":"read", "resource_type":"inventory", "resource_pattern":format!("inventory:{entity_id}")});
    let secret = json!({"action":"secret", "resource_type":"inventory", "resource_pattern":format!("inventory:{entity_id}")});
    let other = json!({"action":"secret", "resource_type":"inventory", "resource_pattern":format!("inventory:{}", Uuid::now_v7())});
    for (case, scopes, expected) in [
        ("read-only", json!([read.clone()]), StatusCode::FORBIDDEN),
        (
            "secret-only",
            json!([secret.clone()]),
            StatusCode::FORBIDDEN,
        ),
        (
            "wrong-object",
            json!([read.clone(), other]),
            StatusCode::FORBIDDEN,
        ),
        ("read-secret", json!([read, secret]), StatusCode::OK),
    ] {
        let token = create(
            app,
            cookie,
            "/api/v1/user/settings/api-tokens",
            json!({"name":format!("wireless-{kind}-{case}-{}", Uuid::now_v7().simple()), "scopes":scopes}),
        ).await?;
        let bearer = token["token_value"]
            .as_str()
            .ok_or("Missing synthetic token")?;
        let (status, body) =
            common::request_with_auth(app, "POST", &reveal, None, None, Some(bearer)).await?;
        check(
            failures,
            status == expected,
            kind,
            &format!("{case} reveal scope enforcement"),
        );
        if expected == StatusCode::OK {
            let value: Value = serde_json::from_slice(&body)?;
            check(
                failures,
                value["custom_fields"]["wireless_atomic_secret"] == *expected_secret,
                kind,
                "scoped reveal resolves the correct value",
            );
        } else if let Some(secret) = expected_secret.as_str() {
            check(
                failures,
                !String::from_utf8_lossy(&body).contains(secret),
                kind,
                "denied reveal does not disclose plaintext",
            );
        }
        if case != "secret-only" {
            let (status, body) =
                common::request_with_auth(app, "GET", &path, None, None, Some(bearer)).await?;
            check(
                failures,
                status == StatusCode::OK,
                kind,
                "read-scoped ordinary GET succeeds",
            );
            let value: Value = serde_json::from_slice(&body)?;
            check(
                failures,
                value["custom_fields"]["wireless_atomic_secret"] == "****",
                kind,
                "ordinary scoped GET stays masked",
            );
        }
        let token_id = id(&token)?;
        let (status, _) = request(
            app,
            cookie,
            "POST",
            &format!("/api/v1/user/settings/api-tokens/{token_id}/revoke"),
            json!({}),
        )
        .await?;
        check(
            failures,
            status == StatusCode::OK,
            kind,
            "synthetic token revoked",
        );
    }
    Ok(())
}

async fn exercise(url: &str) -> TestResult {
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
    settings.redis.url = common::redis_url(Some(13))?;
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    common::configure_test_cipher(&mut settings);
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
    .ok_or("Atomicity fixture login failed")?;
    let (device, _) = common::seed_device(&pool, "wireless-atomic").await?;
    let (status, body) = common::request(
        &app,
        "GET",
        "/api/v1/inventory/content-types",
        None,
        Some(&cookie),
    )
    .await?;
    if status != StatusCode::OK {
        return Err("Content-type metadata unavailable".into());
    }
    let metadata: Vec<Value> = serde_json::from_slice(&body)?;
    let mut types = Vec::new();
    for token in [
        "wireless.wirelesslangroup",
        "wireless.wirelesslan",
        "wireless.wirelesslink",
    ] {
        let scope = metadata
            .iter()
            .find(|scope| scope["token"] == token)
            .ok_or("Wireless content type missing")?;
        if scope["supported_for_custom_fields"] != true {
            return Err("Wireless custom-field capability missing".into());
        }
        types.push(id(scope)?);
    }
    for (name, secret) in [
        ("wireless_atomic_secret", true),
        ("wireless_atomic_note", false),
    ] {
        create(&app, &cookie, "/api/v1/custom-fields", json!({"name":name, "label":name, "field_type":"text", "is_secret":secret, "object_types":types})).await?;
    }
    let mut failures = Vec::new();
    for (kind, collection, table) in [
        (
            "group",
            "/api/v1/inventory/wireless_lan_groups",
            "wireless_wirelesslangroup",
        ),
        (
            "lan",
            "/api/v1/inventory/wireless_lans",
            "wireless_wirelesslan",
        ),
        (
            "link",
            "/api/v1/inventory/wireless_links",
            "wireless_wirelesslink",
        ),
    ] {
        // Whitespace reaches canonical Set validation only after CF validation.
        let mut invalid = payload(&app, &cookie, kind, &format!("{kind}-invalid"), device).await?;
        invalid["custom_fields"] =
            json!({"wireless_atomic_secret":" \t ", "wireless_atomic_note":"must-not-persist"});
        let before = count(&pool, table).await?;
        let secret_before = secrets(&pool).await?;
        let (status, _) = request(&app, &cookie, "POST", collection, invalid.clone()).await?;
        check(
            &mut failures,
            status == StatusCode::BAD_REQUEST,
            kind,
            "invalid create rejected",
        );
        check(
            &mut failures,
            count(&pool, table).await? == before,
            kind,
            "invalid create rolled back",
        );
        check(
            &mut failures,
            secrets(&pool).await? == secret_before,
            kind,
            "invalid create secret state unchanged",
        );

        let mut original =
            payload(&app, &cookie, kind, &format!("{kind}-original"), device).await?;
        original["description"] = json!("original metadata");
        original["custom_fields"] = json!({"wireless_atomic_secret":"synthetic-original", "wireless_atomic_note":"original note"});
        let expected_secret = original["custom_fields"]["wireless_atomic_secret"].clone();
        let original = create(&app, &cookie, collection, original).await?;
        let original_id = id(&original)?;
        let path = format!("{collection}/{original_id}");
        verify_reveal_access(
            &app,
            &cookie,
            kind,
            collection,
            original_id,
            &expected_secret,
            &mut failures,
        )
        .await?;
        let entity_before = entity(&pool, table, original_id).await?;
        let secret_before = secrets(&pool).await?;
        for method in ["PUT", "PATCH"] {
            let (status, _) = request(&app, &cookie, method, &path, json!({"description":"must-not-persist", "custom_fields":{"wireless_atomic_secret":" \t ", "wireless_atomic_note":"changed note"}})).await?;
            check(
                &mut failures,
                status == StatusCode::BAD_REQUEST,
                kind,
                "invalid metadata update rejected",
            );
            check(
                &mut failures,
                entity(&pool, table, original_id).await? == entity_before,
                kind,
                "invalid metadata update rolled back",
            );
            check(
                &mut failures,
                secrets(&pool).await? == secret_before,
                kind,
                "invalid update secret state unchanged",
            );
        }

        let mut valid_bulk =
            payload(&app, &cookie, kind, &format!("{kind}-bulk-ok"), device).await?;
        valid_bulk["custom_fields"] = json!({"wireless_atomic_secret":"synthetic-bulk"});
        let mut invalid_bulk =
            payload(&app, &cookie, kind, &format!("{kind}-bulk-bad"), device).await?;
        invalid_bulk["custom_fields"] = invalid["custom_fields"].clone();
        let before = count(&pool, table).await?;
        let (status, bulk) = request(
            &app,
            &cookie,
            "POST",
            &format!("{collection}/bulk"),
            json!({"items":[valid_bulk, invalid_bulk]}),
        )
        .await?;
        check(
            &mut failures,
            status == StatusCode::OK,
            kind,
            "bulk envelope succeeds",
        );
        check(
            &mut failures,
            bulk["created"].as_array().is_some_and(|v| v.len() == 1)
                && bulk["errors"].as_array().is_some_and(|v| v.len() == 1)
                && bulk["errors"][0]["index"] == 2,
            kind,
            "bulk reports exactly its successful item",
        );
        check(
            &mut failures,
            count(&pool, table).await? == before + 1,
            kind,
            "bulk failure does not commit a hidden row",
        );

        // A real database storage failure must roll back, not just fail validation.
        // This isolated child database owns the trigger; it never reaches another test.
        let late_payload = payload(&app, &cookie, kind, &format!("{kind}-late"), device).await?;
        sqlx::query("CREATE SEQUENCE wireless_atomic_store_probe")
            .execute(&pool)
            .await?;
        sqlx::query("SELECT setval('wireless_atomic_store_probe', 1, true)")
            .execute(&pool)
            .await?;
        // Sequence advancement survives rollback and proves each request reached storage.
        sqlx::query("CREATE FUNCTION wireless_atomic_reject_store() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('wireless_atomic_store_probe'); RAISE EXCEPTION 'injected wireless storage failure'; END; $$").execute(&pool).await?;
        sqlx::query("CREATE TRIGGER wireless_atomic_reject_store BEFORE INSERT OR UPDATE ON secrets_store FOR EACH ROW EXECUTE FUNCTION wireless_atomic_reject_store()").execute(&pool).await?;
        let before = count(&pool, table).await?;
        let entity_before = entity(&pool, table, original_id).await?;
        let secret_before = secrets(&pool).await?;
        let mut late_payload = late_payload;
        late_payload["custom_fields"] = json!({"wireless_atomic_secret":"synthetic-storage-failure", "wireless_atomic_note":"late note"});
        let probe_before = store_failure_probe(&pool).await?;
        let (create_status, _) =
            request(&app, &cookie, "POST", collection, late_payload.clone()).await?;
        let probe_after_create = store_failure_probe(&pool).await?;
        check(
            &mut failures,
            probe_after_create > probe_before,
            kind,
            "late create reached injected storage failure",
        );
        check(
            &mut failures,
            count(&pool, table).await? == before
                && entity(&pool, table, original_id).await? == entity_before,
            kind,
            "late create preserves entity rows",
        );
        check(
            &mut failures,
            secrets(&pool).await? == secret_before,
            kind,
            "late create preserves secret state",
        );
        let (update_status, _) = request(
            &app,
            &cookie,
            "PATCH",
            &path,
            json!({"description":"late metadata", "custom_fields":late_payload["custom_fields"]}),
        )
        .await?;
        let probe_after_update = store_failure_probe(&pool).await?;
        check(
            &mut failures,
            probe_after_update > probe_after_create,
            kind,
            "late update reached injected storage failure",
        );
        check(
            &mut failures,
            count(&pool, table).await? == before
                && entity(&pool, table, original_id).await? == entity_before,
            kind,
            "late update preserves entity rows",
        );
        check(
            &mut failures,
            secrets(&pool).await? == secret_before,
            kind,
            "late update preserves secret state",
        );
        let (bulk_status, late_bulk) = request(
            &app,
            &cookie,
            "POST",
            &format!("{collection}/bulk"),
            json!({"items":[late_payload]}),
        )
        .await?;
        let probe_after_bulk = store_failure_probe(&pool).await?;
        check(
            &mut failures,
            probe_after_bulk > probe_after_update,
            kind,
            "late bulk item reached injected storage failure",
        );
        check(
            &mut failures,
            count(&pool, table).await? == before
                && entity(&pool, table, original_id).await? == entity_before,
            kind,
            "late bulk preserves entity rows",
        );
        check(
            &mut failures,
            secrets(&pool).await? == secret_before,
            kind,
            "late bulk preserves secret state",
        );
        sqlx::query("DROP TRIGGER wireless_atomic_reject_store ON secrets_store")
            .execute(&pool)
            .await?;
        sqlx::query("DROP FUNCTION wireless_atomic_reject_store()")
            .execute(&pool)
            .await?;
        sqlx::query("DROP SEQUENCE wireless_atomic_store_probe")
            .execute(&pool)
            .await?;
        check(
            &mut failures,
            create_status.is_server_error() && update_status.is_server_error(),
            kind,
            "late storage errors reported",
        );
        check(
            &mut failures,
            bulk_status == StatusCode::OK
                && late_bulk["created"]
                    .as_array()
                    .is_some_and(|v| v.is_empty())
                && late_bulk["errors"].as_array().is_some_and(|v| v.len() == 1),
            kind,
            "late bulk item reports failure",
        );
        check(
            &mut failures,
            count(&pool, table).await? == before,
            kind,
            "late create and bulk rows rolled back",
        );
        check(
            &mut failures,
            entity(&pool, table, original_id).await? == entity_before,
            kind,
            "late metadata update rolled back",
        );
        check(
            &mut failures,
            secrets(&pool).await? == secret_before,
            kind,
            "late failure secret state unchanged",
        );
    }
    drop(app);
    pool.close().await;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("Wireless atomicity violations: {}", failures.join("; ")).into())
    }
}

#[tokio::test]
#[ignore = "Requires isolated PostgreSQL and Redis; creates and drops a disposable child database"]
async fn wireless_custom_field_secret_failures_roll_back_entities_and_bulk_items() -> TestResult {
    let mut url = Url::parse(&common::database_url())?;
    url.set_path(&format!("/wireless_atomic_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let result = exercise(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    result?;
    cleanup?;
    Ok(())
}
