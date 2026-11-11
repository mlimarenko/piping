//! Mounted organizational writes commit metadata and custom secrets together.
#[path = "common/child_database.rs"]
mod child_database;
mod common;

use std::sync::Arc;

use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use sqlx::{PgPool, migrate::Migrator};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const WORKER: &str = "PWA79_ORGANIZATIONAL_ATOMIC_WORKER";
const TEST: &str = "organizational_creates_roll_back_late_secret_failures";

async fn call(
    app: &Router,
    cookie: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> TestResult<(StatusCode, Value)> {
    let body = body.map(|value| value.to_string());
    let (status, bytes) = common::request(app, method, path, body.as_deref(), Some(cookie)).await?;
    Ok((
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        },
    ))
}

async fn create(app: &Router, cookie: &str, path: &str, body: Value) -> TestResult<Value> {
    let (status, value) = call(app, cookie, "POST", path, Some(body)).await?;
    // Never include bodies, supplied secrets or database snapshots in failures.
    assert_eq!(
        status,
        StatusCode::CREATED,
        "organizational fixture: {path}"
    );
    Ok(value)
}

fn id(value: &Value) -> TestResult<&str> {
    value["id"]
        .as_str()
        .ok_or_else(|| "fixture id missing".into())
}

async fn snapshot(pool: &PgPool) -> TestResult<Value> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object(
        'sites',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM dcim_site t),
        'regions',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM dcim_region t),
        'tenants',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM tenancy_tenant t),
        'clusters',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM virtualization_cluster t),
        'variables',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secrets_variable t),
        'store',(SELECT jsonb_agg(to_jsonb(t) ORDER BY secret_id) FROM secrets_store t),
        'bindings',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secrets_binding t),
        'usage',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secret_usage_binding t),
        'outbound',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM sync_outbound_event t))",
    )
    .fetch_one(pool)
    .await?)
}

async fn probe(pool: &PgPool, sequence: &str) -> TestResult<i64> {
    // Sequence identifiers come only from this test's fixed fixture names.
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT last_value FROM ");
    query.push(sequence);
    Ok(query.build_query_scalar().fetch_one(pool).await?)
}

async fn install_fault_probes(pool: &PgPool) -> TestResult {
    for sequence in [
        "org_atomic_row_probe",
        "org_atomic_store_probe",
        "org_atomic_fault_probe",
        "org_atomic_update_probe",
        "org_atomic_entity_fault_probe",
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SEQUENCE ");
        query.push(sequence);
        query.build().execute(pool).await?;
        sqlx::query("SELECT setval($1::regclass,1,true)")
            .bind(sequence)
            .execute(pool)
            .await?;
    }
    // Sequences survive rollback, proving that validation reached each INSERT.
    sqlx::query("CREATE FUNCTION org_atomic_row_probe() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('org_atomic_row_probe'); RETURN NEW; END $$").execute(pool).await?;
    for table in [
        "dcim_site",
        "dcim_region",
        "tenancy_tenant",
        "virtualization_cluster",
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "CREATE TRIGGER org_atomic_row_probe BEFORE INSERT ON ",
        );
        query
            .push(table)
            .push(" FOR EACH ROW EXECUTE FUNCTION org_atomic_row_probe()");
        query.build().execute(pool).await?;
    }
    sqlx::query("CREATE FUNCTION org_atomic_update_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        PERFORM nextval('org_atomic_update_probe');
        IF NEW.name LIKE 'org-entity-fail-%' THEN
            PERFORM nextval('org_atomic_entity_fault_probe'); RAISE EXCEPTION 'synthetic organizational entity update failure';
        END IF; RETURN NEW; END $$").execute(pool).await?;
    for table in [
        "dcim_site",
        "dcim_region",
        "tenancy_tenant",
        "virtualization_cluster",
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "CREATE TRIGGER org_atomic_update_fault BEFORE UPDATE ON ",
        );
        query
            .push(table)
            .push(" FOR EACH ROW EXECUTE FUNCTION org_atomic_update_fault()");
        query.build().execute(pool).await?;
    }
    // Usage is attached before encrypted storage. Reject only named failing
    // items, so one bulk envelope can also prove successful siblings commit.
    sqlx::query("CREATE FUNCTION org_atomic_store_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        PERFORM nextval('org_atomic_store_probe');
        IF EXISTS (SELECT 1 FROM secret_usage_binding u JOIN (
            SELECT id,name FROM dcim_site UNION ALL SELECT id,name FROM dcim_region
            UNION ALL SELECT id,name FROM tenancy_tenant UNION ALL SELECT id,name FROM virtualization_cluster
        ) entity ON entity.id=u.consumer_id WHERE u.secret_id=NEW.secret_id AND entity.name LIKE 'org-fail-%') THEN
            PERFORM nextval('org_atomic_fault_probe'); RAISE EXCEPTION 'synthetic organizational storage failure';
        END IF; RETURN NEW; END $$").execute(pool).await?;
    sqlx::query("CREATE TRIGGER org_atomic_store_fault BEFORE INSERT OR UPDATE ON secrets_store FOR EACH ROW EXECUTE FUNCTION org_atomic_store_fault()").execute(pool).await?;
    Ok(())
}

fn payload(base: &Value, name: &str, secret: &str) -> Value {
    let mut value = base.clone();
    value["name"] = json!(name);
    value["slug"] = json!(name);
    // ClusterPayload has no slug; the caller supplies the appropriate base.
    if base.get("cluster_type_id").is_some() {
        value
            .as_object_mut()
            .expect("static object payload")
            .remove("slug");
    }
    value["custom_fields"] =
        json!({"org_atomic_secret":secret,"org_atomic_note":"kept plain metadata"});
    value
}

async fn assert_success(
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    path: &str,
    phase: &str,
    value: &Value,
    expected: &Value,
    secret: &str,
) -> TestResult {
    assert!(
        value["custom_fields"]["org_atomic_secret"] == "****",
        "{path}: {phase} response masks secret"
    );
    assert!(
        value["custom_fields"]["org_atomic_note"] == "kept plain metadata",
        "{path}: create retains plain field"
    );
    for field in [
        "parent_id",
        "tenant_id",
        "region_id",
        "group_id",
        "site_id",
        "cluster_type_id",
    ] {
        if let Some(reference) = expected.get(field) {
            assert!(
                value.get(field) == Some(reference),
                "{path}: reference {field} preserved"
            );
        }
    }
    let object = format!("{path}/{}", id(value)?);
    let (status, fetched) = call(app, cookie, "GET", &object, None).await?;
    assert_eq!(status, StatusCode::OK, "{path}: committed read");
    assert!(
        fetched["custom_fields"]["org_atomic_secret"] == "****",
        "{path}: GET masks secret"
    );
    let (status, revealed) = call(
        app,
        cookie,
        "POST",
        &format!("{object}/custom-fields/reveal"),
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{path}: authorized reveal");
    assert!(
        revealed["custom_fields"]["org_atomic_secret"] == secret,
        "{path}: reveal resolves committed secret"
    );
    let uuid = Uuid::parse_str(id(value)?)?;
    let bound: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1 AND owner_scope_id=$1 AND field_key='org_atomic_secret' AND consumer_type='inventory_custom_field'").bind(uuid).fetch_one(pool).await?;
    assert_eq!(bound, 1, "{path}: one canonical secret usage");
    assert!(
        !snapshot(pool).await?.to_string().contains(secret),
        "{path}: raw secret must not persist in metadata or outbound events"
    );
    Ok(())
}

async fn assert_update_atomicity(
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    path: &str,
    base: &Value,
    existing: &Value,
    encryption_fault: bool,
) -> TestResult {
    let kind = path.rsplit('/').next().ok_or("resource name")?;
    let object = format!("{path}/{}", id(existing)?);
    let original_secret = Uuid::now_v7().to_string();
    let optional_secret = Uuid::now_v7().to_string();
    if !encryption_fault {
        let mut good = payload(base, &format!("org-update-{kind}"), &original_secret);
        good["custom_fields"]["org_atomic_optional_secret"] = json!(optional_secret);
        let (status, updated) = call(app, cookie, "PUT", &object, Some(good.clone())).await?;
        assert_eq!(status, StatusCode::OK, "{kind}: successful atomic update");
        assert_success(
            app,
            cookie,
            pool,
            path,
            "update",
            &updated,
            &good,
            &original_secret,
        )
        .await?;
        assert!(
            updated["custom_fields"]["org_atomic_optional_secret"] == "****",
            "{kind}: optional secret masks after update"
        );
        // Omission must neither clear secrets nor replace stored plain fields.
        good.as_object_mut()
            .ok_or("static update object")?
            .remove("custom_fields");
        let (status, unchanged) = call(app, cookie, "PUT", &object, Some(good.clone())).await?;
        assert_eq!(status, StatusCode::OK, "{kind}: omitted fields update");
        assert_success(
            app,
            cookie,
            pool,
            path,
            "omitted-fields update",
            &unchanged,
            &good,
            &original_secret,
        )
        .await?;
        let (status, revealed) = call(
            app,
            cookie,
            "POST",
            &format!("{object}/custom-fields/reveal"),
            None,
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert!(
            revealed["custom_fields"]["org_atomic_optional_secret"] == optional_secret,
            "{kind}: omission preserves optional secret"
        );
    }
    for entity_fault in [false, true] {
        if encryption_fault && entity_fault {
            continue;
        }
        let prefix = if entity_fault {
            "org-entity-fail"
        } else {
            "org-fail"
        };
        let mut failed = payload(
            base,
            &format!("{prefix}-{kind}-update"),
            &Uuid::now_v7().to_string(),
        );
        failed["description"] = json!("must roll back with secrets");
        failed["custom_fields"]["org_atomic_optional_secret"] = Value::Null;
        let before = snapshot(pool).await?;
        let update_before = probe(pool, "org_atomic_update_probe").await?;
        let store_before = probe(pool, "org_atomic_store_probe").await?;
        let secret_fault_before = probe(pool, "org_atomic_fault_probe").await?;
        let entity_fault_before = probe(pool, "org_atomic_entity_fault_probe").await?;
        let (status, _) = call(app, cookie, "PUT", &object, Some(failed)).await?;
        assert!(
            status.is_server_error(),
            "{kind}: failed update must report error"
        );
        assert_eq!(
            probe(pool, "org_atomic_update_probe").await?,
            update_before + 1,
            "{kind}: update statement reached before fault"
        );
        assert!(
            snapshot(pool).await? == before,
            "{kind}: failed update preserves metadata, hierarchy, references, secret rotations/clears and events"
        );
        if entity_fault {
            assert_eq!(
                probe(pool, "org_atomic_entity_fault_probe").await?,
                entity_fault_before + 1,
                "{kind}: injected entity failure reached"
            );
            assert_eq!(
                probe(pool, "org_atomic_store_probe").await?,
                store_before,
                "{kind}: entity failure precedes secret storage"
            );
        } else if encryption_fault {
            assert_eq!(
                probe(pool, "org_atomic_store_probe").await?,
                store_before,
                "{kind}: update encryption fails before storage"
            );
        } else {
            assert_eq!(
                probe(pool, "org_atomic_store_probe").await?,
                store_before + 1,
                "{kind}: update reached secret storage"
            );
            assert_eq!(
                probe(pool, "org_atomic_fault_probe").await?,
                secret_fault_before + 1,
                "{kind}: injected update storage failure reached"
            );
        }
    }
    if !encryption_fault {
        let mut clear = payload(base, &format!("org-cleared-{kind}"), &original_secret);
        clear["custom_fields"]["org_atomic_optional_secret"] = Value::Null;
        let (status, cleared) = call(app, cookie, "PUT", &object, Some(clear.clone())).await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "{kind}: explicit optional secret clear"
        );
        assert_success(
            app,
            cookie,
            pool,
            path,
            "explicit-clear update",
            &cleared,
            &clear,
            &original_secret,
        )
        .await?;
        assert!(
            cleared["custom_fields"]["org_atomic_optional_secret"].is_null(),
            "{kind}: explicit clear is not omission"
        );
        let optional_usage: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1 AND field_key='org_atomic_optional_secret'").bind(Uuid::parse_str(id(existing)?)?).fetch_one(pool).await?;
        assert_eq!(optional_usage, 0, "{kind}: cleared binding is removed");
        let credential = create(app,cookie,"/api/v1/user/settings/api-tokens",json!({"name":format!("org-update-readonly-{}",Uuid::now_v7()),"scopes":[{"action":"read","resource_type":"inventory"}]})).await?;
        let bearer = credential["token_value"]
            .as_str()
            .ok_or("read-only token")?;
        let before = snapshot(pool).await?;
        let clear = clear.to_string();
        let (status, _) =
            common::request_with_auth(app, "PUT", &object, Some(&clear), None, Some(bearer))
                .await?;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{kind}: read-only PAT cannot update"
        );
        assert!(
            snapshot(pool).await? == before,
            "{kind}: permission denial has no writes"
        );
    }
    Ok(())
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
    settings.redis.url = common::redis_url(Some(10))?;
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
    if encryption_fault {
        // The cipher key is resolved lazily on the first actual encryption.
        // A separate process prevents the successful-key OnceCell masking this fault.
        settings.secrets.cipher.key_material_ref = "inline:invalid-base64".into();
    }
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
    .ok_or("organizational login")?;
    // Build real hierarchy/reference fixtures before required fields are added.
    let parent = create(
        &app,
        &cookie,
        "/api/v1/inventory/regions",
        json!({"name":"org-parent","slug":"org-parent"}),
    )
    .await?;
    let group = create(
        &app,
        &cookie,
        "/api/v1/inventory/tenant_groups",
        json!({"name":"org-group","slug":"org-group"}),
    )
    .await?;
    let tenant = create(
        &app,
        &cookie,
        "/api/v1/inventory/tenants",
        json!({"name":"org-tenant","slug":"org-tenant","group_id":id(&group)?}),
    )
    .await?;
    let site = create(&app,&cookie,"/api/v1/inventory/sites",json!({"name":"org-site","slug":"org-site","region_id":id(&parent)?,"tenant_id":id(&tenant)?})).await?;
    let cluster_type = create(
        &app,
        &cookie,
        "/api/v1/inventory/cluster_types",
        json!({"name":"org-cluster-type","slug":"org-cluster-type"}),
    )
    .await?;
    let region_to_update = create(
        &app,
        &cookie,
        "/api/v1/inventory/regions",
        json!({"name":"org-existing-region","slug":"org-existing-region","parent_id":id(&parent)?}),
    )
    .await?;
    let cluster_to_update = create(&app,&cookie,"/api/v1/inventory/clusters",json!({"name":"org-existing-cluster","site_id":id(&site)?,"tenant_id":id(&tenant)?,"cluster_type_id":id(&cluster_type)?})).await?;
    let (status, metadata) = call(
        &app,
        &cookie,
        "GET",
        "/api/v1/inventory/content-types",
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let metadata = metadata.as_array().ok_or("content-type metadata")?;
    let mut types = Vec::new();
    for token in [
        "dcim.site",
        "dcim.region",
        "tenancy.tenant",
        "virtualization.cluster",
    ] {
        types.push(
            id(metadata
                .iter()
                .find(|value| value["token"] == token)
                .ok_or("organizational content type")?)?
            .to_string(),
        );
    }
    for (name, secret, required) in [
        ("org_atomic_secret", true, true),
        ("org_atomic_optional_secret", true, false),
        ("org_atomic_note", false, false),
    ] {
        create(&app,&cookie,"/api/v1/custom-fields",json!({"name":name,"label":name,"field_type":"text","is_secret":secret,"required":required,"object_types":types})).await?;
    }
    install_fault_probes(&pool).await?;
    for (kind, path, table, base, existing) in [
        (
            "site",
            "/api/v1/inventory/sites",
            "dcim_site",
            json!({"tenant_id":id(&tenant)?,"region_id":id(&parent)?}),
            &site,
        ),
        (
            "region",
            "/api/v1/inventory/regions",
            "dcim_region",
            json!({"parent_id":id(&parent)?}),
            &region_to_update,
        ),
        (
            "tenant",
            "/api/v1/inventory/tenants",
            "tenancy_tenant",
            json!({"group_id":id(&group)?}),
            &tenant,
        ),
        (
            "cluster",
            "/api/v1/inventory/clusters",
            "virtualization_cluster",
            json!({"site_id":id(&site)?,"tenant_id":id(&tenant)?,"cluster_type_id":id(&cluster_type)?}),
            &cluster_to_update,
        ),
    ] {
        assert_update_atomicity(
            &app,
            &cookie,
            &pool,
            path,
            &base,
            existing,
            encryption_fault,
        )
        .await?;
        let supplied_secret = Uuid::now_v7().to_string();
        let failed_name = format!("org-fail-{kind}-single");
        let failed = payload(&base, &failed_name, &supplied_secret);
        let before = snapshot(&pool).await?;
        let row_before = probe(&pool, "org_atomic_row_probe").await?;
        let store_before = probe(&pool, "org_atomic_store_probe").await?;
        let fault_before = probe(&pool, "org_atomic_fault_probe").await?;
        let (status, _) = call(&app, &cookie, "POST", path, Some(failed.clone())).await?;
        assert!(
            status.is_server_error(),
            "{kind}: late failure must be reported"
        );
        assert_eq!(
            probe(&pool, "org_atomic_row_probe").await?,
            row_before + 1,
            "{kind}: row INSERT reached before failure"
        );
        assert!(
            snapshot(&pool).await? == before,
            "{kind}: entity, references, secrets and events must roll back"
        );
        if encryption_fault {
            assert_eq!(
                probe(&pool, "org_atomic_store_probe").await?,
                store_before,
                "{kind}: encryption fails before storage"
            );
            let (status, bulk) = call(
                &app,
                &cookie,
                "POST",
                &format!("{path}/bulk"),
                Some(json!({"items":[failed]})),
            )
            .await?;
            assert_eq!(status, StatusCode::OK);
            assert!(
                bulk["created"].as_array().is_some_and(Vec::is_empty),
                "{kind}: encryption-failed bulk has no successes"
            );
            assert!(
                bulk["errors"]
                    .as_array()
                    .is_some_and(|errors| errors.len() == 1),
                "{kind}: failed item reported"
            );
            assert_eq!(
                probe(&pool, "org_atomic_row_probe").await?,
                row_before + 2,
                "{kind}: bulk INSERT reached"
            );
            assert!(
                snapshot(&pool).await? == before,
                "{kind}: encryption-failed bulk must roll back"
            );
            assert_eq!(
                probe(&pool, "org_atomic_store_probe").await?,
                store_before,
                "{kind}: bulk encryption fails before storage"
            );
        } else {
            assert_eq!(
                probe(&pool, "org_atomic_store_probe").await?,
                store_before + 1,
                "{kind}: storage reached"
            );
            assert_eq!(
                probe(&pool, "org_atomic_fault_probe").await?,
                fault_before + 1,
                "{kind}: injected storage rejection reached"
            );
            let good = payload(&base, &format!("org-good-{kind}-single"), &supplied_secret);
            let created = create(&app, &cookie, path, good.clone()).await?;
            assert_success(
                &app,
                &cookie,
                &pool,
                path,
                "single",
                &created,
                &good,
                &supplied_secret,
            )
            .await?;
            let counts_before: (i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM secrets_variable),(SELECT COUNT(*) FROM secrets_store),(SELECT COUNT(*) FROM secrets_binding),(SELECT COUNT(*) FROM secret_usage_binding)").fetch_one(&pool).await?;
            let first = payload(&base, &format!("org-good-{kind}-bulk-a"), &supplied_secret);
            let last = payload(&base, &format!("org-good-{kind}-bulk-b"), &supplied_secret);
            let (status, bulk) = call(
                &app,
                &cookie,
                "POST",
                &format!("{path}/bulk"),
                Some(json!({"items":[first.clone(),failed,last.clone()]})),
            )
            .await?;
            assert_eq!(status, StatusCode::OK);
            let created = bulk["created"].as_array().ok_or("bulk created")?;
            assert_eq!(created.len(), 2, "{kind}: good bulk siblings commit");
            assert!(
                bulk["errors"]
                    .as_array()
                    .is_some_and(|errors| errors.len() == 1 && errors[0]["index"] == 2),
                "{kind}: only middle item fails"
            );
            for (value, expected) in created.iter().zip([&first, &last]) {
                assert_success(
                    &app,
                    &cookie,
                    &pool,
                    path,
                    "bulk",
                    value,
                    expected,
                    &supplied_secret,
                )
                .await?;
            }
            let counts_after: (i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM secrets_variable),(SELECT COUNT(*) FROM secrets_store),(SELECT COUNT(*) FROM secrets_binding),(SELECT COUNT(*) FROM secret_usage_binding)").fetch_one(&pool).await?;
            assert!(
                counts_after
                    == (
                        counts_before.0 + 2,
                        counts_before.1 + 2,
                        counts_before.2 + 2,
                        counts_before.3 + 2
                    ),
                "{kind}: no orphan secret state from failed bulk item"
            );
            assert_eq!(
                probe(&pool, "org_atomic_row_probe").await?,
                row_before + 5,
                "{kind}: single and all three bulk INSERTs reached"
            );
            assert_eq!(
                probe(&pool, "org_atomic_store_probe").await?,
                store_before + 5,
                "{kind}: all writes reached encrypted storage"
            );
            assert_eq!(
                probe(&pool, "org_atomic_fault_probe").await?,
                fault_before + 2,
                "{kind}: only failing single and middle bulk item hit the injected fault"
            );
        }
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT COUNT(*) FROM ");
        query.push(table).push(" WHERE name=$1");
        let failed_rows: i64 = query
            .build_query_scalar()
            .bind(&failed_name)
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            failed_rows, 0,
            "{kind}: failed create cannot leave a hidden row"
        );
    }
    drop(app);
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "Requires disposable PostgreSQL and Redis; child processes isolate the cipher OnceCell"]
async fn organizational_creates_roll_back_late_secret_failures() -> TestResult {
    match std::env::var(WORKER) {
        Ok(mode) if mode == "encryption" || mode == "storage" => {
            let url = std::env::var("PWA79_ATOMIC_CHILD_URL")?;
            child_database::ChildDatabase::validate_url("org_atomic_", &url)?;
            exercise(&url, mode == "encryption").await?;
        }
        Ok(_) => return Err("unknown organizational atomicity worker mode".into()),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("invalid organizational atomicity worker mode".into());
        }
        Err(std::env::VarError::NotPresent) => {
            // Independent processes are required: configuring a different key
            // after the first successful encryption is intentionally rejected.
            for mode in ["encryption", "storage"] {
                let (child, url) = child_database::ChildDatabase::create("org_atomic_").await?;
                let result = std::process::Command::new(std::env::current_exe()?)
                    .args(["--exact", TEST, "--ignored", "--test-threads=1"])
                    .env(WORKER, mode)
                    .env("PWA79_ATOMIC_CHILD_URL", url)
                    .output();
                let cleanup = child.cleanup().await;
                let result = result?;
                // Child stdout/stderr may contain driver details; never echo it.
                assert!(
                    result.status.success(),
                    "organizational atomicity {mode} worker failed; run that worker mode for its native assertion"
                );
                cleanup?;
            }
        }
    }
    Ok(())
}
