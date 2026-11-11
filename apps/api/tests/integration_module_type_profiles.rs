//! Native profile CRUD. Main runs this on disposable PostgreSQL 18 and Redis.
mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{
    PgPool, Postgres,
    migrate::{MigrateDatabase, Migrator},
};
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PROFILES: &str = "/api/v1/inventory/module-type-profiles";

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn module_profiles_preserve_schema_and_page_native_rows() -> TestResult {
    let state = common::build_test_app_state().await?;
    let prefix = format!("profile-read-{}", common::unique_suffix());
    let mut ids = Vec::new();
    let schema = json!({
        "type": "object", "properties": {"ports": {"type": "integer", "minimum": 1}},
        "required": ["ports"],
    });
    for index in 0..3 {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO dcim_moduletypeprofile (id, name, description, comments, schema, custom_field_data) VALUES ($1, $2, $3, $4, $5, '{}'::jsonb)",
        )
        .bind(id)
        .bind(format!("{prefix}-{index}"))
        .bind("Module attribute schema")
        .bind("Imported catalog metadata")
        .bind(&schema)
        .execute(&state.db_pool)
        .await?;
        ids.push(id);
    }
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("test login failed")?;
    let (status, body) = common::request(
        &app, "GET",
        &format!("/api/v1/inventory/module-type-profiles?search={prefix}&limit=1&offset=1&sort=name&order=asc"),
        None, Some(&cookie),
    ).await?;
    assert_eq!(status, StatusCode::OK);
    let page: Value = serde_json::from_slice(&body)?;
    assert_eq!(page["total"], 3);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 1);
    assert_eq!(
        page["items"].as_array().ok_or("items array missing")?.len(),
        1
    );
    assert_eq!(page["items"][0]["id"], ids[1].to_string());
    assert_eq!(page["items"][0]["schema"], schema);
    let (status, body) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/module-type-profiles/{}", ids[0]),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let profile: Value = serde_json::from_slice(&body)?;
    assert_eq!(profile["schema"], schema);
    assert_eq!(profile["description"], "Module attribute schema");
    assert_eq!(profile["comments"], "Imported catalog metadata");
    assert_eq!(profile["owner_id"], Value::Null);
    let (status, _) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/module-type-profiles/{}", Uuid::now_v7()),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = common::request(
        &app,
        "GET",
        "/api/v1/inventory/module-type-profiles?sort=unapproved_column",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn module_profiles_require_inventory_read_access() -> TestResult {
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    for uri in [
        PROFILES.to_string(),
        format!("{PROFILES}/{}", Uuid::now_v7()),
    ] {
        let (status, _) = common::request(&app, "GET", &uri, None, None).await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    Ok(())
}

async fn call(
    app: &axum::Router,
    cookie: Option<&str>,
    method: &str,
    path: &str,
    payload: Option<Value>,
    expected: StatusCode,
) -> TestResult<Value> {
    let payload = payload.map(|value| value.to_string());
    let (status, body) = common::request(app, method, path, payload.as_deref(), cookie).await?;
    assert_eq!(status, expected, "{method} {path}");
    Ok(if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    })
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn native_profile_crud_denials_concurrency_and_secret_rollback() -> TestResult {
    let mut url = Url::parse(&common::database_url())?;
    url.set_path(&format!("/pwa80_profiles_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let result = exercise(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    result?;
    cleanup?;
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
    settings.redis.url = common::redis_url(Some(14))?;
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    common::configure_test_cipher(&mut settings);
    common::ensure_test_admin(&pool, &settings.auth.password_pepper).await?;
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let state =
        piping_backend::app::AppState::build(Arc::new(settings), pool.clone(), redis.clone())
            .await?;
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login")?;
    call(
        &app,
        None,
        "POST",
        PROFILES,
        Some(json!({"name":"anonymous"})),
        StatusCode::UNAUTHORIZED,
    )
    .await?;
    for invalid in [
        json!({"name":""}),
        json!({"name":"x".repeat(101)}),
        json!({"name":"invalid","schema":[]}),
        json!({"name":"invalid","owner_id":Uuid::now_v7()}),
        json!({"name":"invalid","tags":null}),
        json!({"name":"invalid","unknown":true}),
    ] {
        call(
            &app,
            Some(&cookie),
            "POST",
            PROFILES,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let owner = call(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/owners",
        Some(json!({"name":"Profile owner"})),
        StatusCode::CREATED,
    )
    .await?;
    let item = call(&app, Some(&cookie), "POST", PROFILES, Some(json!({"name":"Fiber","schema":{"properties":{"wavelength":{"type":"integer"}}},"owner_id":owner["id"]})), StatusCode::CREATED).await?;
    let id = item["id"].as_str().ok_or("profile id")?;
    let path = format!("{PROFILES}/{id}");
    call(
        &app,
        Some(&cookie),
        "PATCH",
        &path,
        Some(json!({"description":"kept"})),
        StatusCode::OK,
    )
    .await?;
    let preserved = call(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await?;
    assert_eq!(preserved["schema"], item["schema"]);
    assert_eq!(preserved["owner_id"], owner["id"]);
    for invalid in [
        json!({"name":null}),
        json!({"comments":null}),
        json!({"custom_fields":null}),
        json!({"schema":true}),
        json!({"description":"x".repeat(201)}),
    ] {
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &path,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let filtered = call(
        &app,
        Some(&cookie),
        "GET",
        &format!(
            "{PROFILES}?owner_id={}&offset=100",
            owner["id"].as_str().ok_or("owner id")?
        ),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(filtered["total"], 1);
    assert_eq!(filtered["items"], json!([]));
    let (first, second) = tokio::join!(
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &path,
            Some(json!({"name":"Renamed"})),
            StatusCode::OK
        ),
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &path,
            Some(json!({"comments":"Concurrent note"})),
            StatusCode::OK
        )
    );
    first?;
    second?;
    let concurrent = call(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await?;
    assert_eq!(concurrent["name"], "Renamed");
    assert_eq!(concurrent["comments"], "Concurrent note");
    let (first, second) = tokio::join!(
        common::request(
            &app,
            "POST",
            PROFILES,
            Some(r#"{"name":"Duplicate"}"#),
            Some(&cookie)
        ),
        common::request(
            &app,
            "POST",
            PROFILES,
            Some(r#"{"name":"Duplicate"}"#),
            Some(&cookie)
        )
    );
    let statuses = [first?.0, second?.0];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::CONFLICT));
    let blocked = call(
        &app,
        Some(&cookie),
        "DELETE",
        &format!(
            "/api/v1/inventory/owners/{}",
            owner["id"].as_str().ok_or("owner id")?
        ),
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    assert!(blocked.is_object());
    secret_rollback(&app, &cookie, &pool, &path).await?;
    profile_tag_dates(&app, &cookie, &path).await?;
    token_ceilings(&app, &cookie, &path).await?;
    let manufacturer = call(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/manufacturers",
        Some(json!({"name":"Profile manufacturer","slug":"profile-manufacturer"})),
        StatusCode::CREATED,
    )
    .await?;
    let module_id = Uuid::now_v7();
    profile_delete_consumer_lock_order(&pool, url, &app, &cookie, &manufacturer).await?;
    module_type_profile_consumer(&app, &cookie, &manufacturer).await?;
    sqlx::query("INSERT INTO dcim_moduletype (id,custom_field_data,model,part_number,comments,manufacturer_id,description,profile_id,module_count) VALUES ($1,'{}','Synthetic module','','',$2,'',$3,0)")
        .bind(module_id).bind(Uuid::parse_str(manufacturer["id"].as_str().ok_or("manufacturer id")?)?).bind(Uuid::parse_str(id)?).execute(&pool).await?;
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &path,
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    sqlx::query("DELETE FROM dcim_moduletype WHERE id=$1")
        .bind(module_id)
        .execute(&pool)
        .await?;
    scoped_denials(&app, &cookie, &pool, &mut redis, &path, id, &owner["id"]).await?;
    let cleared = call(
        &app,
        Some(&cookie),
        "PATCH",
        &path,
        Some(json!({"schema":null,"owner_id":null})),
        StatusCode::OK,
    )
    .await?;
    assert!(cleared["schema"].is_null());
    assert!(cleared["owner_id"].is_null());
    assert_eq!(cleared["description"], "kept");
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    call(
        &app,
        Some(&cookie),
        "GET",
        &path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    pool.close().await;
    Ok(())
}

async fn scoped_denials(
    app: &axum::Router,
    cookie: &str,
    pool: &PgPool,
    redis: &mut redis::aio::ConnectionManager,
    path: &str,
    id: &str,
    owner: &Value,
) -> TestResult {
    let saved: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(pool)
            .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(json!([format!("inventory:{id}")]))
        .execute(pool)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut *redis)
        .await?;
    let result: TestResult = async {
        call(app, Some(cookie), "GET", path, None, StatusCode::OK).await?;
        call(
            app,
            Some(cookie),
            "GET",
            PROFILES,
            None,
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "POST",
            PROFILES,
            Some(json!({"name":"Forbidden"})),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(json!({"owner_id":owner})),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(json!({"description":"kept"})),
            StatusCode::OK,
        )
        .await?;
        Ok(())
    }
    .await;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(saved)
        .execute(pool)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut *redis)
        .await?;
    result
}

async fn secret_rollback(
    app: &axum::Router,
    cookie: &str,
    pool: &PgPool,
    path: &str,
) -> TestResult {
    let scope: Uuid = sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='moduletypeprofile'").fetch_one(pool).await?;
    call(app, Some(cookie), "POST", "/api/v1/custom-fields", Some(json!({"name":"profile_secret","label":"Profile secret","field_type":"text","is_secret":true,"object_types":[scope]})), StatusCode::CREATED).await?;
    let value = Uuid::now_v7().to_string();
    let masked = call(
        app,
        Some(cookie),
        "PATCH",
        path,
        Some(json!({"custom_fields":{"profile_secret":value}})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(masked["custom_fields"]["profile_secret"], "****");
    let revealed = call(
        app,
        Some(cookie),
        "POST",
        &format!("{path}/custom-fields/reveal"),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(revealed["custom_fields"]["profile_secret"], value);
    sqlx::query("CREATE FUNCTION pwa80_profile_secret_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic secret storage failure'; END $$").execute(pool).await?;
    sqlx::query("CREATE TRIGGER pwa80_profile_secret_fault BEFORE INSERT OR UPDATE ON secrets_store FOR EACH ROW EXECUTE FUNCTION pwa80_profile_secret_fault()").execute(pool).await?;
    let snapshot = sqlx::query_scalar::<_, Value>("SELECT jsonb_build_object('profiles',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM dcim_moduletypeprofile t),'store',(SELECT jsonb_agg(to_jsonb(t) ORDER BY secret_id) FROM secrets_store t),'variables',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secrets_variable t),'usage',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secret_usage_binding t))").fetch_one(pool).await?;
    for (method, target, payload) in [
        (
            "POST",
            PROFILES,
            json!({"name":"Atomic profile","custom_fields":{"profile_secret":Uuid::now_v7().to_string()}}),
        ),
        (
            "PATCH",
            path,
            json!({"description":"must roll back","custom_fields":{"profile_secret":Uuid::now_v7().to_string()}}),
        ),
    ] {
        call(
            app,
            Some(cookie),
            method,
            target,
            Some(payload),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await?;
        let after = sqlx::query_scalar::<_, Value>("SELECT jsonb_build_object('profiles',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM dcim_moduletypeprofile t),'store',(SELECT jsonb_agg(to_jsonb(t) ORDER BY secret_id) FROM secrets_store t),'variables',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secrets_variable t),'usage',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secret_usage_binding t))").fetch_one(pool).await?;
        assert_eq!(after, snapshot);
    }
    sqlx::query("DROP TRIGGER pwa80_profile_secret_fault ON secrets_store")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION pwa80_profile_secret_fault()")
        .execute(pool)
        .await?;
    Ok(())
}

async fn token_ceilings(app: &axum::Router, cookie: &str, path: &str) -> TestResult {
    let id = path.rsplit('/').next().ok_or("profile id")?;
    for (actions, can_read, can_reveal) in [
        (vec!["read"], true, false),
        (vec!["secret"], false, false),
        (vec!["read", "secret"], true, true),
    ] {
        let scopes: Vec<_> = actions.iter().map(|action| json!({"action":action,"resource_type":"inventory","resource_pattern":format!("inventory:{id}")})).collect();
        let token = call(
            app,
            Some(cookie),
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(json!({"name":format!("Profile ceiling {}",Uuid::now_v7()),"scopes":scopes})),
            StatusCode::CREATED,
        )
        .await?;
        let bearer = token["token_value"].as_str().ok_or("profile token")?;
        for (method, uri, payload, expected) in [
            (
                "GET",
                path.to_string(),
                None,
                if can_read {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
            ),
            (
                "POST",
                format!("{path}/custom-fields/reveal"),
                None,
                if can_reveal {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
            ),
            ("GET", PROFILES.to_string(), None, StatusCode::FORBIDDEN),
            (
                "POST",
                PROFILES.to_string(),
                Some(json!({"name":"Forbidden"})),
                StatusCode::FORBIDDEN,
            ),
            (
                "PATCH",
                path.to_string(),
                Some(json!({"description":"Forbidden"})),
                StatusCode::FORBIDDEN,
            ),
            ("DELETE", path.to_string(), None, StatusCode::FORBIDDEN),
        ] {
            let payload = payload.map(|value| value.to_string());
            let (status, _) = common::request_with_auth(
                app,
                method,
                &uri,
                payload.as_deref(),
                None,
                Some(bearer),
            )
            .await?;
            assert_eq!(status, expected, "Profile PAT {actions:?}: {method} {uri}");
        }
    }
    Ok(())
}

async fn module_type_profile_consumer(
    app: &axum::Router,
    cookie: &str,
    manufacturer: &Value,
) -> TestResult {
    let profile = call(app, Some(cookie), "POST", PROFILES, Some(json!({"name":"Consumer profile","schema":{"type":"object","required":["ports"],"properties":{"ports":{"type":"integer","minimum":1}},"additionalProperties":false}})), StatusCode::CREATED).await?;
    let payload = json!({"manufacturer_id":manufacturer["id"],"model":"Consumer module","profile_id":profile["id"],"attribute_data":{"ports":4}});
    for attributes in [
        json!({}),
        json!({"ports":0}),
        json!({"ports":"four"}),
        json!({"ports":4,"unknown":1}),
        Value::Null,
    ] {
        let mut invalid = payload.clone();
        invalid["attribute_data"] = attributes;
        call(
            app,
            Some(cookie),
            "POST",
            "/api/v1/inventory/module-types",
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let module = call(
        app,
        Some(cookie),
        "POST",
        "/api/v1/inventory/module-types",
        Some(payload),
        StatusCode::CREATED,
    )
    .await?;
    assert_eq!(module["profile_id"], profile["id"]);
    assert_eq!(module["attribute_data"], json!({"ports":4}));
    let path = format!(
        "/api/v1/inventory/module-types/{}",
        module["id"].as_str().ok_or("module type id")?
    );
    let updated = call(
        app,
        Some(cookie),
        "PUT",
        &path,
        Some(json!({"description":"metadata"})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(updated["profile_id"], profile["id"]);
    assert_eq!(updated["attribute_data"], json!({"ports":4}));
    for invalid in [
        json!({"attribute_data":null}),
        json!({"attribute_data":{"ports":-1}}),
        json!({"profile_id":Uuid::now_v7()}),
    ] {
        call(
            app,
            Some(cookie),
            "PUT",
            &path,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let unchanged = call(app, Some(cookie), "GET", &path, None, StatusCode::OK).await?;
    assert_eq!(unchanged["attribute_data"], json!({"ports":4}));
    let profile_path = format!("{PROFILES}/{}", profile["id"].as_str().ok_or("profile id")?);
    call(
        app,
        Some(cookie),
        "DELETE",
        &profile_path,
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    profile_delete_blocker_read_ceilings(app, cookie, &profile_path, &path, &module).await?;
    let cleared = call(
        app,
        Some(cookie),
        "PUT",
        &path,
        Some(json!({"profile_id":null})),
        StatusCode::OK,
    )
    .await?;
    assert!(cleared["profile_id"].is_null());
    assert!(cleared["attribute_data"].is_null());
    let invalid = json!({"schema":{"type":"not-a-json-schema-type"}});
    call(
        app,
        Some(cookie),
        "PATCH",
        &profile_path,
        Some(invalid),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    call(
        app,
        Some(cookie),
        "PATCH",
        &profile_path,
        Some(json!({"schema":{"$ref":"http://127.0.0.1:1/not-authorized"}})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    call(
        app,
        Some(cookie),
        "DELETE",
        &profile_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    Ok(())
}

async fn profile_delete_blocker_read_ceilings(
    app: &axum::Router,
    cookie: &str,
    profile_path: &str,
    module_path: &str,
    module: &Value,
) -> TestResult {
    let profile_id = profile_path.rsplit('/').next().ok_or("profile id")?;
    let module_id = module["id"].as_str().ok_or("module id")?;
    for can_read in [false, true] {
        let mut scopes = vec![
            json!({"action":"delete","resource_type":"inventory","resource_pattern":format!("inventory:{profile_id}")}),
        ];
        if can_read {
            scopes.push(json!({"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{module_id}")}));
        }
        let credential = call(
            app,
            Some(cookie),
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(json!({"name":format!("profile-blocker-{}", Uuid::now_v7()),"scopes":scopes})),
            StatusCode::CREATED,
        )
        .await?;
        // Synthetic credentials remain in memory and are never included in assertions.
        let bearer = credential["token_value"]
            .as_str()
            .ok_or("profile blocker token")?;
        let (status, _) =
            common::request_with_auth(app, "GET", module_path, None, None, Some(bearer)).await?;
        assert_eq!(
            status,
            if can_read {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        let (status, bytes) =
            common::request_with_auth(app, "DELETE", profile_path, None, None, Some(bearer))
                .await?;
        assert_eq!(status, StatusCode::CONFLICT);
        let body: Value = serde_json::from_slice(&bytes)?;
        if can_read {
            assert_eq!(body["code"], "DELETE_BLOCKED");
            let blockers = body["details"]["blockers"]
                .as_array()
                .ok_or("structured profile blockers")?;
            assert!(
                blockers
                    .iter()
                    .any(|blocker| blocker["table"] == "dcim_moduletype"
                        && blocker["examples"]
                            .as_array()
                            .is_some_and(|examples| examples
                                .iter()
                                .any(|example| example["id"] == module["id"])))
            );
        } else {
            assert_eq!(body["reason_code"], "DELETE_BLOCKED");
            assert!(body.get("details").is_none_or(Value::is_null));
            let serialized = serde_json::to_string(&body)?;
            for forbidden in [
                module_id,
                module["model"].as_str().ok_or("module model")?,
                "dcim_moduletype",
                "blockers",
                "total_count",
            ] {
                assert!(
                    !serialized.contains(forbidden),
                    "unreadable profile blocker must remain non-identifying"
                );
            }
        }
        call(app, Some(cookie), "GET", profile_path, None, StatusCode::OK).await?;
        call(app, Some(cookie), "GET", module_path, None, StatusCode::OK).await?;
    }
    Ok(())
}

/// Observe an actual PostgreSQL lock wait, not elapsed time or task scheduling.
async fn wait_for_profile_graph_queue(pool: &PgPool, worker: i32, gate: i32) -> TestResult {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks w JOIN pg_locks g USING (locktype,database,classid,objid,objsubid) WHERE w.pid=$1 AND NOT w.granted AND g.pid=$2 AND g.granted AND g.locktype='advisory' AND g.mode='ExclusiveLock')",
            ).bind(worker).bind(gate).fetch_one(pool).await?;
            if waiting {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    }).await??;
    Ok(())
}

async fn profile_lock_worker_pool(url: &str) -> TestResult<PgPool> {
    Ok(sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET lock_timeout='20s'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET statement_timeout='30s'")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await?)
}

async fn profile_delete_consumer_lock_order(
    pool: &PgPool,
    url: &str,
    app: &axum::Router,
    cookie: &str,
    manufacturer: &Value,
) -> TestResult {
    use piping_backend::contexts::inventory::{
        domain::{ModuleType, dcim::module_type::UpdateModuleType},
        infrastructure::persistence::repositories::dcim::{
            module_type_profile_repository::SqlxModuleTypeProfileRepository,
            module_type_repository::{ModuleTypeRepository, SqlxModuleTypeRepository},
        },
    };
    let profile = call(app, Some(cookie), "POST", PROFILES,
        Some(json!({"name":"Lock order profile","schema":{"type":"object","required":["ports"],"properties":{"ports":{"type":"integer","minimum":1}}}})),
        StatusCode::CREATED).await?;
    let profile_id = Uuid::parse_str(profile["id"].as_str().ok_or("profile id")?)?;
    let module = call(app, Some(cookie), "POST", "/api/v1/inventory/module-types",
        Some(json!({"manufacturer_id":manufacturer["id"],"model":"Lock order consumer","profile_id":profile_id,"attribute_data":{"ports":4}})),
        StatusCode::CREATED).await?;
    let module_id = Uuid::parse_str(module["id"].as_str().ok_or("module id")?)?;
    let manufacturer_id = Uuid::parse_str(manufacturer["id"].as_str().ok_or("manufacturer id")?)?;
    for operation in ["update", "create"] {
        let consumer_pool = profile_lock_worker_pool(url).await?;
        let delete_pool = profile_lock_worker_pool(url).await?;
        let consumer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&consumer_pool)
            .await?;
        let delete_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&delete_pool)
            .await?;
        let mut gate = pool.begin().await?;
        sqlx::raw_sql("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='5s'")
            .execute(&mut *gate)
            .await?;
        // Use the same native Tag -> graph owner as production, not a test lock key.
        sqlx::query("SELECT dcim_acquire_hardware_graph()")
            .execute(&mut *gate)
            .await?;
        let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *gate)
            .await?;
        let mut jobs = tokio::task::JoinSet::new();
        let repository = SqlxModuleTypeRepository::new(consumer_pool.clone());
        let created_id = Uuid::now_v7();
        jobs.spawn(async move {
            if operation == "update" {
                // Omitted profile_id must resolve from the stored, locked type.
                let changes = UpdateModuleType {
                    attribute_data: Some(Some(json!({"ports":8}))),
                    ..Default::default()
                };
                let updated = repository
                    .update(module_id, &changes, vec![])
                    .await?
                    .ok_or("missing consumer")?;
                assert_eq!(updated.profile_id, Some(profile_id));
                assert_eq!(updated.attribute_data, Some(json!({"ports":8})));
            } else {
                repository
                    .create(&ModuleType {
                        id: created_id,
                        manufacturer_id,
                        model: "Queued profile consumer".into(),
                        profile_id: Some(profile_id),
                        attribute_data: Some(json!({"ports":2})),
                        ..Default::default()
                    })
                    .await?;
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>("consumer")
        });
        wait_for_profile_graph_queue(pool, consumer_pid, gate_pid).await?;
        let repository = SqlxModuleTypeProfileRepository::new(delete_pool.clone());
        jobs.spawn(async move {
            let error = repository
                .delete(profile_id)
                .await
                .expect_err("referenced profile must refuse deletion");
            assert!(
                matches!(&error, piping_backend::errors::AppError::DeleteBlocked(_)),
                "{error}"
            );
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>("delete")
        });
        wait_for_profile_graph_queue(pool, delete_pid, gate_pid).await?;
        // Neither worker may retain a field catalog, consumer or profile lock
        // while waiting for graph ownership. The old order fails these probes.
        let field_free: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended('inventory-secret-field-catalog',0))")
            .fetch_one(&mut *gate).await?;
        assert!(
            field_free,
            "{operation} took the field catalog before the graph"
        );
        sqlx::query("SELECT id FROM dcim_moduletype WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(module_id)
            .fetch_one(&mut *gate)
            .await?;
        sqlx::query("SELECT id FROM dcim_moduletypeprofile WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(profile_id)
            .fetch_one(&mut *gate)
            .await?;
        gate.rollback().await?;
        let completed = tokio::time::timeout(std::time::Duration::from_secs(35), async {
            let mut completed = Vec::new();
            while let Some(result) = jobs.join_next().await {
                completed.push(result??);
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(completed)
        })
        .await??;
        assert_eq!(completed.len(), 2);
        let retained: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dcim_moduletypeprofile WHERE id=$1)")
                .bind(profile_id)
                .fetch_one(pool)
                .await?;
        assert!(retained);
        if operation == "create" {
            sqlx::query("DELETE FROM dcim_moduletype WHERE id=$1")
                .bind(created_id)
                .execute(pool)
                .await?;
        }
        consumer_pool.close().await;
        delete_pool.close().await;
    }
    sqlx::query("DELETE FROM dcim_moduletype WHERE id=$1")
        .bind(module_id)
        .execute(pool)
        .await?;
    call(
        app,
        Some(cookie),
        "DELETE",
        &format!("{PROFILES}/{profile_id}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18"]
async fn profile_scope_seed_preserves_upgrade_identity_and_native_data() -> TestResult {
    let mut url = Url::parse(&common::database_url())?;
    url.set_path(&format!(
        "/pwa80_profile_upgrade_{}",
        Uuid::now_v7().simple()
    ));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let result = profile_scope_upgrade(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    result?;
    cleanup?;
    Ok(())
}

async fn profile_scope_upgrade(url: &str) -> TestResult {
    let pool = PgPool::connect(url).await?;
    let migrations = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let prefix = tempfile::tempdir()?;
    for filename in [
        "0001_init.sql",
        "0002_interface_networking.sql",
        "0003_service_port_mappings.sql",
        "0004_fhrp_auth_key_secret.sql",
        "0005_module_bay_types.sql",
        "0006_canonical_tag_ids.sql",
        "0007_cooling_hardware.sql",
    ] {
        std::fs::copy(migrations.join(filename), prefix.path().join(filename))?;
    }
    Migrator::new(prefix.path()).await?.run(&pool).await?;
    let scope = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_content_type(id,app_label,model) VALUES ($1,'dcim','moduletypeprofile')").bind(scope).execute(&pool).await?;
    sqlx::query("INSERT INTO dcim_moduletypeprofile (id,name,description,comments,schema,custom_field_data) VALUES ($1,'Upgrade profile','preserve description','preserve comments','{\"type\":\"object\"}','{\"unregistered_imported_value\":42}')").bind(Uuid::now_v7()).execute(&pool).await?;
    let before: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM dcim_moduletypeprofile t",
    )
    .fetch_one(&pool)
    .await?;
    let history: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(m)::text FROM _sqlx_migrations m ORDER BY version")
            .fetch_all(&pool)
            .await?;
    Migrator::new(migrations.as_path())
        .await?
        .run(&pool)
        .await?;
    let after: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM dcim_moduletypeprofile t",
    )
    .fetch_one(&pool)
    .await?;
    let after_without_metadata: Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t)-'tags'-'created'-'last_updated' ORDER BY id) FROM dcim_moduletypeprofile t").fetch_one(&pool).await?;
    assert_eq!(before, after_without_metadata);
    let rows = after.as_array().ok_or("upgraded profiles")?;
    assert!(rows.iter().all(|row| row["tags"] == json!([])
        && row["created"].is_null()
        && row["last_updated"].is_null()));
    let after_history: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(m)::text FROM _sqlx_migrations m WHERE version<=7 ORDER BY version",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(history, after_history);
    let scopes: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='moduletypeprofile'").fetch_all(&pool).await?;
    assert_eq!(scopes, vec![scope]);
    pool.close().await;
    Ok(())
}

async fn profile_tag_dates(app: &axum::Router, cookie: &str, path: &str) -> TestResult {
    let tag = call(app, Some(cookie), "POST", "/api/v1/inventory/tags", Some(json!({"name":"Profile metadata tag","slug":"profile-metadata-tag","color":"155dfc","weight":0})), StatusCode::CREATED).await?;
    let tag_id = tag["id"].as_str().ok_or("tag id")?;
    let original = call(app, Some(cookie), "GET", path, None, StatusCode::OK).await?;
    assert!(original["created"].is_string());
    assert!(original["last_updated"].is_string());
    let tagged = call(
        app,
        Some(cookie),
        "PATCH",
        path,
        Some(json!({"tags":[tag_id]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(tagged["tags"], json!([tag_id]));
    assert_eq!(tagged["created"], original["created"]);
    let kept = call(
        app,
        Some(cookie),
        "PATCH",
        path,
        Some(json!({"comments":"metadata omission"})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(kept["tags"], json!([tag_id]));
    for invalid in [
        json!({"tags":null}),
        json!({"tags":[tag_id,tag_id]}),
        json!({"tags":["00000000-0000-4000-8000-00000000000A"]}),
        json!({"tags":[Uuid::now_v7()]}),
        json!({"created":"2001-01-01T00:00:00Z"}),
        json!({"last_updated":null}),
    ] {
        call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    call(
        app,
        Some(cookie),
        "DELETE",
        &format!("/api/v1/inventory/tags/{tag_id}"),
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    let cleared = call(
        app,
        Some(cookie),
        "PATCH",
        path,
        Some(json!({"tags":[]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(cleared["tags"], json!([]));
    assert_eq!(cleared["created"], original["created"]);
    call(
        app,
        Some(cookie),
        "DELETE",
        &format!("/api/v1/inventory/tags/{tag_id}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    Ok(())
}
