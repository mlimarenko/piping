//! PWA-95 route behaviour on an isolated child database. The caller supplies
//! disposable PostgreSQL/Redis services; the parent database is never modified.
mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::{PgPool, Postgres};
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const TYPES: &str = "/api/v1/inventory/virtual-machine-types";
const VMS: &str = "/api/v1/inventory/virtual_machines";

#[test]
fn virtual_machine_type_openapi_is_generated_and_registers_native_operations() -> TestResult {
    use utoipa::OpenApi;
    let doc = piping_backend::api::openapi::ApiDoc::openapi();
    assert_eq!(
        serde_yml::to_string(&doc)?,
        include_str!("../contracts/piping.openapi.yaml")
    );
    let value = serde_json::to_value(doc)?;
    for method in ["get", "post"] {
        assert!(value["paths"][TYPES][method].is_object());
    }
    for method in ["get", "put", "patch", "delete"] {
        assert!(value["paths"][format!("{TYPES}/{{id}}")][method].is_object());
    }
    assert!(
        value["components"]["schemas"]["VirtualMachineTypePatchPayload"]["additionalProperties"]
            == false
    );
    assert_eq!(
        value["components"]["schemas"]["VirtualMachineTypePayload"]["properties"]["default_vcpus"]
            ["multipleOf"],
        json!(0.01)
    );
    Ok(())
}

async fn api(
    app: &axum::Router,
    method: &str,
    path: &str,
    payload: Option<Value>,
    cookie: Option<&str>,
    expected: StatusCode,
) -> TestResult<Value> {
    let payload = payload.map(|value| value.to_string());
    let (status, body) = common::request(app, method, path, payload.as_deref(), cookie).await?;
    assert_eq!(status, expected, "{method} {path}: unexpected status");
    if body.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_slice(&body)?)
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
    // Reserve a logical DB for this binary; each child PostgreSQL DB also needs
    // a fresh policy epoch so a previous child's deny grant cannot leak in.
    let mut redis_url = Url::parse(&common::redis_url())?;
    redis_url.set_path("/15");
    settings.redis.url = redis_url.to_string();
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
        piping_backend::app::AppState::build(Arc::new(settings), pool.clone(), redis.clone())
            .await?;
    let app = piping_backend::app::build_router(state.clone());
    job_token_contract(&app, &state, &pool).await?;
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("test admin login failed")?;
    legacy_compute_metadata_contract(&app, &cookie, &pool).await?;
    run_scenarios(app, cookie, pool, redis).await
}

/// A deployed finite compute value can predate REST range validation. Omitting
/// it in an update must preserve it, while explicitly supplying it is validated.
async fn legacy_compute_metadata_contract(
    app: &axum::Router,
    cookie: &str,
    pool: &PgPool,
) -> TestResult {
    let created = api(
        app,
        "POST",
        VMS,
        Some(json!({"name":"Legacy VM"})),
        Some(cookie),
        StatusCode::CREATED,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing legacy id")?;
    sqlx::query("UPDATE virtualization_virtualmachine SET vcpus=0 WHERE id=$1")
        .bind(Uuid::parse_str(id)?)
        .execute(pool)
        .await?;
    let object = format!("{VMS}/{id}");
    let metadata = json!({"name":"Renamed legacy VM","description":"metadata only"});
    let updated = api(
        app,
        "PUT",
        &object,
        Some(metadata.clone()),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(updated["vcpus"], json!(0.0), "omitted legacy CPU preserved");
    assert!(updated["memory"].is_null(), "omitted memory preserved");
    for supplied in [json!({"vcpus":0}), json!({"memory":-1})] {
        let mut rejected = metadata.clone();
        rejected
            .as_object_mut()
            .ok_or("metadata object")?
            .extend(supplied.as_object().ok_or("compute object")?.clone());
        api(
            app,
            "PUT",
            &object,
            Some(rejected),
            Some(cookie),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let cleared = api(
        app,
        "PUT",
        &object,
        Some(json!({"name":"Cleared legacy VM","vcpus":null,"memory":null})),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(cleared["vcpus"].is_null() && cleared["memory"].is_null());
    Ok(())
}

async fn job_token_contract(
    app: &axum::Router,
    state: &piping_backend::app::AppState,
    pool: &PgPool,
) -> TestResult {
    use piping_backend::contexts::playbooks::application::CreateJobTokenInput;

    let playbook = Uuid::now_v7();
    let version = Uuid::now_v7();
    let group = Uuid::now_v7();
    let run = Uuid::now_v7();
    sqlx::query("INSERT INTO playbook(id,slug,name,status,runtime,risk,category) VALUES ($1,$2,'VM type job scope','active','ansible','low','general')")
        .bind(playbook).bind(format!("vm-type-job-{playbook}")).execute(pool).await?;
    sqlx::query("INSERT INTO playbook_version(id,playbook_id,version,language,runtime,content,content_sha256) VALUES ($1,$2,1,'yaml','ansible','---', $3)")
        .bind(version).bind(playbook).bind("0".repeat(64)).execute(pool).await?;
    sqlx::query("INSERT INTO playbook_run_group(id,playbook_id,playbook_version_id,trigger_kind,status,total_runs) VALUES ($1,$2,$3,'manual','running',1)")
        .bind(group).bind(playbook).bind(version).execute(pool).await?;
    sqlx::query("INSERT INTO playbook_run(id,run_group_id,playbook_id,playbook_version_id,target_kind,target_id,source_kind,source_id,status,started_at) VALUES ($1,$2,$3,$4,'connection_target',$5,'device',$6,'running',now())")
        .bind(run).bind(group).bind(playbook).bind(version).bind(Uuid::now_v7()).bind(Uuid::now_v7()).execute(pool).await?;
    for scope in ["inventory:read", "inventory:create", "inventory:update"] {
        let created = state
            .services
            .playbooks
            .create_job_token(
                CreateJobTokenInput {
                    run_id: run,
                    expires_in_seconds: Some(60),
                    scopes: Some(vec![scope.into()]),
                    metadata: None,
                },
                chrono::Utc::now(),
            )
            .await?;
        let token = created.raw_token;
        assert_job_token_scope(app, pool, scope, &token, created.token.id).await?;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM virtualization_virtualmachinetype WHERE slug IN ('job-inventory-create','bulk-inventory-create')").fetch_one(pool).await?;
    assert_eq!(count, 2, "job-token creates persisted");
    sqlx::query("DELETE FROM playbook_run_group WHERE id=$1")
        .bind(group)
        .execute(pool)
        .await?;
    Ok(())
}

/// Drive one collection scope through a real production job token: assert the
/// read/create split, then revoke the token and assert it is rejected.
async fn assert_job_token_scope(
    app: &axum::Router,
    pool: &PgPool,
    scope: &str,
    token: &str,
    job_token_id: Uuid,
) -> TestResult {
    let (status, _) = common::request_with_auth(app, "GET", TYPES, None, None, Some(token)).await?;
    assert_eq!(
        status,
        if scope == "inventory:read" {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        },
        "job-token {scope} collection read"
    );
    for (path, body, allowed_status) in [
        (
            TYPES,
            json!({"name":format!("Job {scope}"),"slug":format!("job-{}",scope.replace(':',"-"))}),
            StatusCode::CREATED,
        ),
        (
            "/api/v1/inventory/virtual-machine-types/bulk",
            json!({"items":[{"name":format!("Bulk {scope}"),"slug":format!("bulk-{}",scope.replace(':',"-"))}]}),
            StatusCode::OK,
        ),
    ] {
        let (status, response) = common::request_with_auth(
            app,
            "POST",
            path,
            Some(&body.to_string()),
            None,
            Some(token),
        )
        .await?;
        assert_eq!(
            status,
            if scope == "inventory:create" {
                allowed_status
            } else {
                StatusCode::FORBIDDEN
            },
            "job-token {scope} collection create"
        );
        if scope == "inventory:create" {
            assert_job_token_created_payload(path, &response)?;
        }
    }
    sqlx::query("UPDATE playbook_job_token SET status='revoked' WHERE id=$1")
        .bind(job_token_id)
        .execute(pool)
        .await?;
    let (status, _) = common::request_with_auth(app, "GET", TYPES, None, None, Some(token)).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked job token");
    Ok(())
}

fn assert_job_token_created_payload(path: &str, response: &[u8]) -> TestResult {
    let result: Value = serde_json::from_slice(response)?;
    if path == TYPES {
        assert!(result["id"].is_string());
    } else {
        assert_eq!(result["created"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["errors"], json!([]));
    }
    Ok(())
}

async fn run_scenarios(
    app: axum::Router,
    cookie: String,
    pool: PgPool,
    redis: redis::aio::ConnectionManager,
) -> TestResult {
    let type_id = create_type(app.clone(), cookie.clone()).await?;
    let type_path = format!("{TYPES}/{type_id}");
    patch_contract(app.clone(), cookie.clone(), type_path.clone()).await?;
    let secret_value =
        configure_custom_fields(app.clone(), cookie.clone(), type_path.clone()).await?;
    let vm_path = create_vm_defaults(app.clone(), cookie.clone(), type_id.clone()).await?;
    vm_empty_page_totals(&app, &cookie, &pool, &type_id, &vm_path).await?;
    vm_overrides(app.clone(), cookie.clone(), type_id.clone()).await?;
    hydration_and_referenced_delete(app.clone(), cookie.clone(), type_path.clone()).await?;
    preserve_fields(
        app.clone(),
        cookie.clone(),
        type_id,
        type_path.clone(),
        vm_path,
        secret_value,
    )
    .await?;
    vm_compute_readback(&app, &cookie).await?;
    scoped_permissions(&app, &cookie, &pool, &type_path, redis.clone()).await?;
    deny_mutations(app, cookie, pool, type_path, redis).await
}

async fn vm_empty_page_totals(
    app: &axum::Router,
    cookie: &str,
    pool: &PgPool,
    type_id: &str,
    vm_path: &str,
) -> TestResult {
    let vm_id = vm_path.rsplit('/').next().ok_or("missing VM id")?;
    let missing = Uuid::now_v7();
    let typed = format!("virtual_machine_type_id={type_id}");
    let combined = format!(
        "virtual_machine_type_id={type_id},{missing}&ids={vm_id},{missing}&search=VM%20defaults"
    );
    let mut cases = vec![(typed.clone(), 1), (combined.clone(), 1)];
    cases.push((format!("{combined}&status=not-a-status"), 0));
    cases.push((format!("{typed}&ids="), 0));
    cases.push((format!("virtual_machine_type_id={missing}"), 0));
    cases.push((format!("{typed}&search=NoSuchVirtualMachine"), 0));
    for field in [
        "cluster_id",
        "site_id",
        "tenant_id",
        "role_id",
        "platform_id",
    ] {
        cases.push((format!("{combined}&{field}={missing}"), 0));
    }
    for (query, expected) in cases {
        for offset in [0, 100] {
            let page = api(
                app,
                "GET",
                &format!("{VMS}?{query}&offset={offset}"),
                None,
                Some(cookie),
                StatusCode::OK,
            )
            .await?;
            assert_eq!(page["total"], expected, "{query}, offset={offset}");
            let items = page["items"].as_array().ok_or("VM items not an array")?;
            assert_eq!(items.len(), if offset == 0 { expected as usize } else { 0 });
            if expected == 1 && offset == 0 {
                assert_eq!(items[0]["id"], vm_id);
            }
        }
    }

    vm_empty_page_visibility(pool, type_id, vm_id).await
}

async fn vm_empty_page_visibility(pool: &PgPool, type_id: &str, vm_id: &str) -> TestResult {
    use piping_backend::contexts::access_control::domain::VisibilityPredicate;
    use piping_backend::contexts::inventory::infrastructure::persistence::{
        SqlxVirtualMachineRepository, VirtualMachineFilter, VirtualMachineRepository,
    };

    // Exercise the same repository boundary with restrictive visibility, rather
    // than relying on an administrator's unrestricted HTTP visibility.
    let repository = SqlxVirtualMachineRepository::new(pool.clone());
    for (visibility, expected) in [
        (
            VisibilityPredicate::IdMatch {
                pattern: vm_id.into(),
            },
            1,
        ),
        (
            VisibilityPredicate::IdMatch {
                pattern: Uuid::now_v7().to_string(),
            },
            0,
        ),
        (VisibilityPredicate::Never, 0),
    ] {
        for offset in [0, 100] {
            let (items, total) = repository
                .list_hydrated(
                    VirtualMachineFilter {
                        virtual_machine_type_id: vec![Uuid::parse_str(type_id)?],
                        visibility: Some(visibility.clone()),
                        ..Default::default()
                    },
                    Some(50),
                    Some(offset),
                    Some("%VM defaults%"),
                )
                .await?;
            assert_eq!(total, expected, "{visibility:?}, offset={offset}");
            assert_eq!(items.len(), if offset == 0 { expected as usize } else { 0 });
        }
    }
    Ok(())
}

async fn vm_compute_readback(app: &axum::Router, cookie: &str) -> TestResult {
    let platform = api(
        app,
        "POST",
        "/api/v1/inventory/platforms",
        Some(json!({"name":"Compute readback","slug":"compute-readback"})),
        Some(cookie),
        StatusCode::CREATED,
    )
    .await?;
    let platform_id = platform["id"].as_str().ok_or("missing platform id")?;
    // Both single edit and client fan-out bulk use the identical native PUT.
    for name in ["Single compute readback", "Bulk compute readback"] {
        vm_compute_update_readback(app, cookie, name, platform_id).await?;
    }
    Ok(())
}

async fn vm_compute_update_readback(
    app: &axum::Router,
    cookie: &str,
    name: &str,
    platform_id: &str,
) -> TestResult {
    let created = api(
        app,
        "POST",
        VMS,
        Some(json!({"name":name,"platform_id":platform_id,"vcpus":1.5,"memory":2048})),
        Some(cookie),
        StatusCode::CREATED,
    )
    .await?;
    let path = format!("{VMS}/{}", created["id"].as_str().ok_or("missing VM id")?);
    api(
        app,
        "PUT",
        &path,
        Some(json!({"name":name,"description":"omission sentinel"})),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    let preserved = api(app, "GET", &path, None, Some(cookie), StatusCode::OK).await?;
    assert_eq!(preserved["platform_id"], json!(platform_id));
    assert_eq!(preserved["vcpus"], json!(1.5));
    assert_eq!(preserved["memory"], json!(2048));
    api(
        app,
        "PUT",
        &path,
        Some(json!({"name":name,"platform_id":null,"vcpus":null,"memory":null})),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    vm_compute_cleared(app, cookie, &path).await?;
    api(
        app,
        "PUT",
        &path,
        Some(json!({"name":name,"vcpus":0.25,"memory":0})),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    vm_invalid_compute_updates(app, cookie, name, &path).await?;
    let persisted = api(app, "GET", &path, None, Some(cookie), StatusCode::OK).await?;
    assert_eq!(persisted["vcpus"], json!(0.25));
    assert_eq!(persisted["memory"], json!(0));
    assert!(persisted["platform_id"].is_null());
    Ok(())
}

async fn vm_compute_cleared(app: &axum::Router, cookie: &str, path: &str) -> TestResult {
    let cleared = api(app, "GET", path, None, Some(cookie), StatusCode::OK).await?;
    for key in ["platform_id", "vcpus", "memory"] {
        assert!(cleared[key].is_null());
    }
    Ok(())
}

async fn vm_invalid_compute_updates(
    app: &axum::Router,
    cookie: &str,
    name: &str,
    path: &str,
) -> TestResult {
    for invalid in [
        json!({"name":name,"slug":"unsupported"}),
        json!({"name":name,"unknown":1}),
        json!({"name":name,"vcpus":0.015}),
        json!({"name":name,"vcpus":10000}),
        json!({"name":name,"memory":-1}),
    ] {
        api(
            app,
            "PUT",
            path,
            Some(invalid),
            Some(cookie),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct ScopedTypes<'a> {
    app: &'a axum::Router,
    cookie: &'a str,
    pool: &'a PgPool,
    allowed_path: &'a str,
    other_path: &'a str,
    allowed_id: &'a str,
    denied_type: &'a str,
    platform_id: Uuid,
    owner_id: &'a str,
}

async fn scoped_permissions(
    app: &axum::Router,
    cookie: &str,
    pool: &PgPool,
    allowed_path: &str,
    mut redis: redis::aio::ConnectionManager,
) -> TestResult {
    let secret_value = Uuid::now_v7().to_string();
    let other = api(app, "POST", TYPES, Some(json!({"name":"Scoped B","slug":"scoped-b","custom_fields":{"vm_type_secret":secret_value}})), Some(cookie), StatusCode::CREATED).await?;
    assert!(
        other["custom_fields"]["vm_type_secret"] == "****",
        "secret must be masked"
    );
    let other_path = format!("{TYPES}/{}", other["id"].as_str().ok_or("missing B id")?);
    let allowed_id = allowed_path.rsplit('/').next().ok_or("missing A id")?;
    let platform_id: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_platform WHERE slug='compute-readback'")
            .fetch_one(pool)
            .await?;
    let owner = api(
        app,
        "POST",
        "/api/v1/inventory/owners",
        Some(json!({"name":"Scoped FK owner"})),
        Some(cookie),
        StatusCode::CREATED,
    )
    .await?;
    let owner_id = owner["id"].as_str().ok_or("missing owner id")?;
    let fixture = ScopedTypes {
        app,
        cookie,
        pool,
        allowed_path,
        other_path: &other_path,
        allowed_id,
        denied_type: other["id"].as_str().ok_or("missing B id")?,
        platform_id,
        owner_id,
    };
    scoped_object_contract(fixture, &mut redis).await?;
    scoped_reference_contract(fixture).await?;
    scoped_secret_contract(fixture, &mut redis).await?;
    scoped_secret_fk_contract(fixture, &mut redis).await?;
    scoped_collection_contract(fixture, &mut redis).await
}

async fn invalidate_policies(redis: &mut redis::aio::ConnectionManager) -> TestResult {
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(redis)
        .await?;
    Ok(())
}

async fn scoped_object_contract(
    fixture: ScopedTypes<'_>,
    redis: &mut redis::aio::ConnectionManager,
) -> TestResult {
    let ScopedTypes {
        app,
        cookie,
        pool,
        allowed_path,
        other_path,
        allowed_id,
        ..
    } = fixture;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE effect='allow'")
        .bind(json!([format!("inventory:{allowed_id}")]))
        .execute(pool)
        .await?;
    invalidate_policies(redis).await?;
    api(app, "GET", allowed_path, None, Some(cookie), StatusCode::OK).await?;
    api(
        app,
        "GET",
        &format!("{TYPES}/{}", allowed_id.to_uppercase()),
        None,
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    for (method, payload) in [
        ("GET", None),
        (
            "PUT",
            Some(json!({"name":"Unauthorized B","slug":"unauthorized-b"})),
        ),
        ("PATCH", Some(json!({"default_memory":null}))),
        ("DELETE", None),
    ] {
        api(
            app,
            method,
            other_path,
            payload,
            Some(cookie),
            StatusCode::FORBIDDEN,
        )
        .await?;
    }
    api(app, "GET", TYPES, None, Some(cookie), StatusCode::FORBIDDEN).await?;
    api(
        app,
        "POST",
        TYPES,
        Some(json!({"name":"Scoped create","slug":"scoped-create"})),
        Some(cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    api(
        app,
        "POST",
        &format!("{TYPES}/bulk"),
        Some(json!({"items":[{"name":"Scoped bulk","slug":"scoped-bulk"}]})),
        Some(cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    // Bulk updates are client fan-out PATCH requests, so every B request must fail.
    api(
        app,
        "PATCH",
        allowed_path,
        Some(json!({"description":"scoped edit"})),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    api(
        app,
        "PUT",
        allowed_path,
        Some(
            json!({"name":"REST type","slug":"rest-type","default_vcpus":2,"default_memory":8192}),
        ),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    Ok(())
}

async fn scoped_reference_contract(fixture: ScopedTypes<'_>) -> TestResult {
    let ScopedTypes {
        app,
        cookie,
        allowed_path,
        allowed_id,
        denied_type,
        platform_id,
        owner_id,
        ..
    } = fixture;
    for payload in [
        json!({"default_platform_id":platform_id}),
        json!({"owner_id":owner_id}),
    ] {
        api(
            app,
            "PATCH",
            allowed_path,
            Some(payload),
            Some(cookie),
            StatusCode::FORBIDDEN,
        )
        .await?;
    }
    api(
        app,
        "POST",
        VMS,
        Some(json!({"name":"Inaccessible type","virtual_machine_type_id":denied_type})),
        Some(cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    let allowed_vm = api(
        app,
        "POST",
        VMS,
        Some(json!({"name":"Accessible type","virtual_machine_type_id":allowed_id})),
        Some(cookie),
        StatusCode::CREATED,
    )
    .await?;
    let vm_path = format!(
        "{VMS}/{}",
        allowed_vm["id"].as_str().ok_or("missing VM id")?
    );
    api(
        app,
        "PUT",
        &vm_path,
        Some(json!({"name":"Accessible type","virtual_machine_type_id":denied_type})),
        Some(cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    let bulk = api(app, "POST", &format!("{VMS}/bulk"), Some(json!({"items":[{"name":"Bulk inaccessible type","virtual_machine_type_id":denied_type},{"name":"Bulk accessible type","virtual_machine_type_id":allowed_id}]})), Some(cookie), StatusCode::OK).await?;
    assert_eq!(
        bulk["created"]
            .as_array()
            .ok_or("missing bulk created")?
            .len(),
        1
    );
    assert_eq!(
        bulk["errors"]
            .as_array()
            .ok_or("missing bulk errors")?
            .len(),
        1
    );
    assert_eq!(
        bulk["created"][0]["virtual_machine_type_id"],
        json!(allowed_id)
    );
    Ok(())
}

async fn scoped_secret_contract(
    fixture: ScopedTypes<'_>,
    redis: &mut redis::aio::ConnectionManager,
) -> TestResult {
    let ScopedTypes {
        app,
        cookie,
        pool,
        allowed_path,
        other_path,
        ..
    } = fixture;
    api(
        app,
        "POST",
        &format!("{other_path}/custom-fields/reveal"),
        None,
        Some(cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    let revealed = api(
        app,
        "POST",
        &format!("{allowed_path}/custom-fields/reveal"),
        None,
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(
        revealed["custom_fields"]["vm_type_secret"] != "****",
        "scoped A reveal must be permitted"
    );
    // READ alone never reveals a populated value.
    sqlx::query("UPDATE rbac_policy SET actions='[\"READ\"]'::jsonb WHERE effect='allow'")
        .execute(pool)
        .await?;
    invalidate_policies(redis).await?;
    let masked = api(app, "GET", allowed_path, None, Some(cookie), StatusCode::OK).await?;
    assert!(
        masked["custom_fields"]["vm_type_secret"] == "****",
        "READ must mask populated secrets"
    );
    api(
        app,
        "POST",
        &format!("{allowed_path}/custom-fields/reveal"),
        None,
        Some(cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    Ok(())
}

async fn scoped_secret_fk_contract(
    fixture: ScopedTypes<'_>,
    redis: &mut redis::aio::ConnectionManager,
) -> TestResult {
    let ScopedTypes {
        app,
        cookie,
        pool,
        allowed_path,
        allowed_id,
        platform_id,
        ..
    } = fixture;
    // The reveal's FK attribute constraint comes from the stored type, not input.
    let secret_policy_id = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) VALUES ($1,'PWA95 scoped platform secret','allow','[\"SECRET\"]'::jsonb,'[\"INVENTORY\"]'::jsonb,$2,$3,$4,true)")
        .bind(secret_policy_id).bind(json!({"attributes":{"platform_id":platform_id}}))
        .bind(json!(["role:00000000-0000-7000-8000-000000000002"]))
        .bind(json!([format!("inventory:{allowed_id}")])).execute(pool).await?;
    invalidate_policies(redis).await?;
    api(
        app,
        "POST",
        &format!("{allowed_path}/custom-fields/reveal"),
        None,
        Some(cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    sqlx::query("UPDATE virtualization_virtualmachinetype SET default_platform_id=$1 WHERE id=$2")
        .bind(platform_id)
        .bind(Uuid::parse_str(allowed_id)?)
        .execute(pool)
        .await?;
    let revealed = api(
        app,
        "POST",
        &format!("{allowed_path}/custom-fields/reveal"),
        None,
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(
        revealed["custom_fields"]["vm_type_secret"] != "****",
        "stored FK constraint permits A reveal"
    );
    sqlx::query("DELETE FROM rbac_policy WHERE id=$1")
        .bind(secret_policy_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn scoped_collection_contract(
    fixture: ScopedTypes<'_>,
    redis: &mut redis::aio::ConnectionManager,
) -> TestResult {
    let ScopedTypes {
        app,
        cookie,
        pool,
        other_path,
        denied_type,
        ..
    } = fixture;
    // A grant to the literal wildcard probe is not a genuinely global grant.
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE effect='allow'")
        .bind(json!(["re:inventory:\\*"]))
        .execute(pool)
        .await?;
    invalidate_policies(redis).await?;
    api(app, "GET", TYPES, None, Some(cookie), StatusCode::FORBIDDEN).await?;
    sqlx::query("UPDATE rbac_policy SET resources='[\"inventory:*\"]'::jsonb WHERE effect='allow'")
        .execute(pool)
        .await?;
    invalidate_policies(redis).await?;
    api(app, "GET", TYPES, None, Some(cookie), StatusCode::OK).await?;
    let deny_id = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,subjects,resources,enabled) VALUES ($1,'PWA95 scoped read deny','deny','[\"READ\"]'::jsonb,'[\"INVENTORY\"]'::jsonb,$2,$3,true)")
        .bind(deny_id).bind(json!(["role:00000000-0000-7000-8000-000000000002"]))
        .bind(json!([format!("inventory:{denied_type}")])).execute(pool).await?;
    invalidate_policies(redis).await?;
    api(app, "GET", TYPES, None, Some(cookie), StatusCode::FORBIDDEN).await?;
    sqlx::query("DELETE FROM rbac_policy WHERE id=$1")
        .bind(deny_id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE rbac_policy SET resources='[]'::jsonb, actions='[\"CREATE\",\"READ\",\"UPDATE\",\"DELETE\",\"SECRET\"]'::jsonb WHERE effect='allow'").execute(pool).await?;
    invalidate_policies(redis).await?;
    let persisted = api(app, "GET", other_path, None, Some(cookie), StatusCode::OK).await?;
    assert_eq!(persisted["name"], "Scoped B");
    Ok(())
}

async fn create_type(app: axum::Router, cookie: String) -> TestResult<String> {
    api(
        &app,
        "POST",
        TYPES,
        Some(json!({"name":"unauthenticated", "slug":"unauthenticated"})),
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await?;
    api(
        &app,
        "POST",
        TYPES,
        Some(json!({"name":"invalid", "slug":"invalid", "default_vcpus":0.015})),
        Some(&cookie),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    api(
        &app,
        "POST",
        TYPES,
        Some(json!({"name":"invalid", "slug":"invalid", "default_platform_id":Uuid::now_v7()})),
        Some(&cookie),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let created = api(&app, "POST", TYPES, Some(json!({"name":"REST type", "slug":"rest-type", "default_vcpus":0.25, "default_memory":512})), Some(&cookie), StatusCode::CREATED).await?;
    assert_eq!(created["default_vcpus"], json!(0.25));
    let type_id = created["id"].as_str().ok_or("type id missing")?;
    Ok(type_id.to_owned())
}

async fn patch_contract(app: axum::Router, cookie: String, type_path: String) -> TestResult {
    for invalid in [
        json!({"name":"invalid","slug":"invalid","unknown":1}),
        json!({"name":"invalid","slug":"bad slug"}),
        json!({"name":"invalid","slug":"invalid","default_memory":-1}),
        json!({"name":"invalid","slug":"invalid","default_vcpus":10000}),
        json!({"name":"invalid","slug":"invalid","owner_id":Uuid::now_v7()}),
    ] {
        api(
            &app,
            "POST",
            TYPES,
            Some(invalid),
            Some(&cookie),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let patched = api(
        &app,
        "PATCH",
        &type_path,
        Some(json!({"description":"patch sentinel"})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(patched["default_vcpus"], json!(0.25));
    assert_eq!(patched["default_memory"], json!(512));
    let cleared_default = api(
        &app,
        "PATCH",
        &type_path,
        Some(json!({"default_vcpus":null})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(cleared_default["default_vcpus"].is_null());
    assert_eq!(cleared_default["default_memory"], json!(512));
    assert_eq!(cleared_default["description"], "patch sentinel");
    api(
        &app,
        "PATCH",
        &type_path,
        Some(json!({"default_vcpus":0.25})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    api(
        &app,
        "PATCH",
        &type_path,
        Some(json!({"name":null})),
        Some(&cookie),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let disposable = api(
        &app,
        "POST",
        TYPES,
        Some(json!({"name":"Disposable type","slug":"disposable-type"})),
        Some(&cookie),
        StatusCode::CREATED,
    )
    .await?;
    let disposable_path = format!(
        "{TYPES}/{}",
        disposable["id"].as_str().ok_or("disposable id missing")?
    );
    api(
        &app,
        "DELETE",
        &disposable_path,
        None,
        Some(&cookie),
        StatusCode::NO_CONTENT,
    )
    .await?;
    api(
        &app,
        "GET",
        &disposable_path,
        None,
        Some(&cookie),
        StatusCode::NOT_FOUND,
    )
    .await?;
    Ok(())
}

async fn configure_custom_fields(
    app: axum::Router,
    cookie: String,
    type_path: String,
) -> TestResult<String> {
    let content_types = api(
        &app,
        "GET",
        "/api/v1/inventory/content-types",
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    let type_scope = content_types
        .as_array()
        .ok_or("content types is not an array")?
        .iter()
        .find(|item| item["token"] == "virtualization.virtualmachinetype")
        .ok_or("VM type custom-field scope missing")?;
    assert_eq!(type_scope["supported_for_custom_fields"], true);
    let reveal_path = format!("{type_path}/custom-fields/reveal");
    let revealed = api(
        &app,
        "POST",
        &reveal_path,
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(revealed["custom_fields"], json!({}));
    for (name, is_secret) in [("vm_type_plain", false), ("vm_type_secret", true)] {
        api(
            &app,
            "POST",
            "/api/v1/custom-fields",
            Some(json!({
                "name": name, "label": name, "field_type": "text", "is_secret": is_secret,
                "object_types": [type_scope["id"]]
            })),
            Some(&cookie),
            StatusCode::CREATED,
        )
        .await?;
    }
    let secret_value = Uuid::now_v7().to_string();
    let masked = api(
        &app,
        "PUT",
        &type_path,
        Some(json!({
            "name":"REST type", "slug":"rest-type", "default_vcpus":0.25, "default_memory":512,
            "custom_fields":{"vm_type_plain":"persisted value", "vm_type_secret":secret_value}
        })),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(
        masked["custom_fields"]["vm_type_secret"] == "****",
        "write response must mask the secret"
    );
    let revealed = api(
        &app,
        "POST",
        &reveal_path,
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(
        revealed["custom_fields"]["vm_type_secret"] == secret_value,
        "authorized reveal must read the saved value"
    );
    api(
        &app,
        "POST",
        TYPES,
        Some(json!({"name":"rest TYPE", "slug":"another-slug"})),
        Some(&cookie),
        StatusCode::CONFLICT,
    )
    .await?;
    Ok(secret_value)
}

async fn create_vm_defaults(
    app: axum::Router,
    cookie: String,
    type_id: String,
) -> TestResult<String> {
    for invalid in [
        json!({"name":"Invalid VM", "virtual_machine_type_id":Uuid::now_v7()}),
        json!({"name":"Invalid VM", "unknown":true}),
        json!({"name":"Invalid VM", "slug":"invalid-vm"}),
        json!({"name":"Invalid VM", "vcpus":0.015}),
        json!({"name":"Invalid VM", "memory":-1}),
    ] {
        api(
            &app,
            "POST",
            VMS,
            Some(invalid),
            Some(&cookie),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let vm = api(
        &app,
        "POST",
        VMS,
        Some(json!({"name":"VM defaults", "virtual_machine_type_id":type_id})),
        Some(&cookie),
        StatusCode::CREATED,
    )
    .await?;
    assert_eq!(vm["vcpus"], json!(0.25));
    assert_eq!(vm["memory"], json!(512));
    assert_eq!(vm["virtual_machine_type_id"], type_id);
    let vm_id = vm["id"].as_str().ok_or("vm id missing")?;
    let vm_path = format!("{VMS}/{vm_id}");
    Ok(vm_path)
}

async fn vm_overrides(app: axum::Router, cookie: String, type_id: String) -> TestResult {
    let explicit = api(&app, "POST", VMS, Some(json!({"name":"VM explicit", "virtual_machine_type_id":type_id, "vcpus":1.5, "memory":0})), Some(&cookie), StatusCode::CREATED).await?;
    assert_eq!(explicit["vcpus"], json!(1.5));
    assert_eq!(explicit["memory"], json!(0));
    let explicit_id = explicit["id"].as_str().ok_or("explicit VM id missing")?;
    let preserved_overrides = api(
        &app,
        "PUT",
        &format!("{VMS}/{explicit_id}"),
        Some(json!({"name":"VM explicit","virtual_machine_type_id":null})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(preserved_overrides["vcpus"], json!(1.5));
    assert_eq!(preserved_overrides["memory"], json!(0));
    let untyped = api(
        &app,
        "POST",
        VMS,
        Some(json!({"name":"VM created without type"})),
        Some(&cookie),
        StatusCode::CREATED,
    )
    .await?;
    let untyped_id = untyped["id"].as_str().ok_or("untyped VM id missing")?;
    let assigned_later = api(
        &app,
        "PUT",
        &format!("{VMS}/{untyped_id}"),
        Some(json!({"name":"VM created without type","virtual_machine_type_id":type_id})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(assigned_later["vcpus"].is_null());
    assert!(assigned_later["memory"].is_null());
    let null_defaults = api(&app, "POST", VMS, Some(json!({"name":"VM explicit null defaults","virtual_machine_type_id":type_id,"vcpus":null,"memory":null})), Some(&cookie), StatusCode::CREATED).await?;
    assert_eq!(null_defaults["vcpus"], json!(0.25));
    assert_eq!(null_defaults["memory"], json!(512));
    Ok(())
}

async fn hydration_and_referenced_delete(
    app: axum::Router,
    cookie: String,
    type_path: String,
) -> TestResult {
    let hydrated = api(
        &app,
        "GET",
        &format!("{VMS}?search=VM%20defaults"),
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        hydrated["items"][0]["virtual_machine_type_name"],
        "REST type"
    );
    api(
        &app,
        "DELETE",
        &type_path,
        None,
        Some(&cookie),
        StatusCode::CONFLICT,
    )
    .await?;
    Ok(())
}

async fn preserve_fields(
    app: axum::Router,
    cookie: String,
    type_id: String,
    type_path: String,
    vm_path: String,
    secret_value: String,
) -> TestResult {
    let reveal_path = format!("{type_path}/custom-fields/reveal");
    api(&app, "PUT", &type_path, Some(json!({"name":"REST type", "slug":"rest-type", "default_vcpus":2.0, "default_memory":8192, "custom_fields":null})), Some(&cookie), StatusCode::OK).await?;
    let preserved = api(&app, "GET", &type_path, None, Some(&cookie), StatusCode::OK).await?;
    assert_eq!(
        preserved["custom_fields"]["vm_type_plain"],
        "persisted value"
    );
    assert!(
        preserved["custom_fields"]["vm_type_secret"] == "****",
        "read response must mask the preserved secret"
    );
    let revealed = api(
        &app,
        "POST",
        &reveal_path,
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert!(
        revealed["custom_fields"]["vm_type_secret"] == secret_value,
        "null custom fields must preserve secret storage"
    );
    vm_update_association(app, cookie, type_id, vm_path).await
}

async fn vm_update_association(
    app: axum::Router,
    cookie: String,
    type_id: String,
    vm_path: String,
) -> TestResult {
    let existing = api(&app, "GET", &vm_path, None, Some(&cookie), StatusCode::OK).await?;
    assert_eq!(existing["vcpus"], json!(0.25));
    assert_eq!(existing["memory"], json!(512));
    let updated = api(
        &app,
        "PUT",
        &vm_path,
        Some(json!({"name":"VM defaults", "vcpus":0.75, "memory":1024})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(updated["virtual_machine_type_id"], type_id);
    let cleared = api(&app, "PUT", &vm_path, Some(json!({"name":"VM defaults", "virtual_machine_type_id":null, "vcpus":0.75, "memory":1024})), Some(&cookie), StatusCode::OK).await?;
    assert!(cleared["virtual_machine_type_id"].is_null());
    assert_eq!(cleared["vcpus"], json!(0.75));
    let persisted = api(&app, "GET", &vm_path, None, Some(&cookie), StatusCode::OK).await?;
    assert!(persisted["virtual_machine_type_id"].is_null());

    Ok(())
}

async fn deny_mutations(
    app: axum::Router,
    cookie: String,
    pool: PgPool,
    type_path: String,
    mut redis: redis::aio::ConnectionManager,
) -> TestResult {
    let reveal_path = format!("{type_path}/custom-fields/reveal");
    // A deny grant must reject every mutation before persistence. This policy
    // lives only in this test's child database, and is invalidated in Redis.
    let policy_id = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) VALUES ($1,'PWA-95 deny mutations','deny',$2,$3,NULL,$4,'[]'::jsonb,true)")
        .bind(policy_id).bind(json!(["CREATE","UPDATE","DELETE","SECRET"])).bind(json!(["INVENTORY"]))
        .bind(json!(["role:00000000-0000-7000-8000-000000000002"])).execute(&pool).await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    api(
        &app,
        "POST",
        TYPES,
        Some(json!({"name":"denied","slug":"denied"})),
        Some(&cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    api(
        &app,
        "POST",
        &format!("{TYPES}/bulk"),
        Some(json!({"items":[{"name":"denied","slug":"denied"}]})),
        Some(&cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    api(
        &app,
        "PUT",
        &type_path,
        Some(json!({"name":"denied","slug":"denied"})),
        Some(&cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    api(
        &app,
        "DELETE",
        &type_path,
        None,
        Some(&cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    api(
        &app,
        "PATCH",
        &type_path,
        Some(json!({"default_memory":null})),
        Some(&cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM virtualization_virtualmachinetype WHERE name='denied'"
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        api(&app, "GET", &type_path, None, Some(&cookie), StatusCode::OK).await?["name"],
        "REST type"
    );
    api(
        &app,
        "POST",
        &reveal_path,
        None,
        Some(&cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    drop(app);
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis via PIPING_DATABASE__URL/PIPING_REDIS__URL"]
async fn vm_type_rest_defaults_nullable_updates_and_denied_mutations() -> TestResult {
    let mut url = Url::parse(&common::database_url())?;
    url.set_path(&format!("/pwa95_http_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let result = exercise(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    result?;
    cleanup?;
    Ok(())
}
