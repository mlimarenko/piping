//! Prepared native contracts. Run only against disposable PostgreSQL and Redis.
#[path = "common/child_database.rs"]
mod child_database;
mod common;

use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, migrate::Migrator};
use std::sync::Arc;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const POLICIES: &str = "/api/v1/inventory/vlan-translation-policies";
const RULES: &str = "/api/v1/inventory/vlan-translation-rules";

#[test]
fn published_vlan_translation_payloads_match_boundary_contracts() -> TestResult {
    use utoipa::OpenApi;
    let document = serde_json::to_value(piping_backend::api::openapi::ApiDoc::openapi())?;
    let policy_id = Uuid::from_u128(0x80);
    let tag = "00000000-0000-4000-8000-000000000081";
    for (schema_name, base, invalid) in [
        (
            "VlanTranslationPolicyCreatePayload",
            json!({"name":"Campus edge"}),
            vec![
                json!({"name":""}),
                json!({"name":"a".repeat(101)}),
                json!({"description":"a".repeat(201)}),
                json!({"status":"active"}),
            ],
        ),
        (
            "VlanTranslationPolicyPatchPayload",
            json!({}),
            vec![
                json!({"name":null}),
                json!({"comments":null}),
                json!({"description":null}),
            ],
        ),
        (
            "VlanTranslationRuleCreatePayload",
            json!({"policy_id":policy_id,"local_vid":1,"remote_vid":4094}),
            vec![
                json!({"policy_id":null}),
                json!({"local_vid":0}),
                json!({"remote_vid":4095}),
                json!({"local_vid":1.5}),
                json!({"name":"phantom"}),
                json!({"owner_id":policy_id}),
                json!({"comments":"phantom"}),
            ],
        ),
        (
            "VlanTranslationRulePatchPayload",
            json!({}),
            vec![
                json!({"policy_id":null}),
                json!({"local_vid":null}),
                json!({"remote_vid":0}),
                json!({"description":null}),
                json!({"name":"phantom"}),
                json!({"status":"active"}),
            ],
        ),
    ] {
        let schema = json!({"$ref":format!("#/components/schemas/{schema_name}"),"components":document["components"]});
        let validator = jsonschema::validator_for(&schema)?;
        assert!(
            validator.is_valid(&base),
            "valid {schema_name} base rejected"
        );
        for change in [
            json!({"tags":[]}),
            json!({"tags":[tag]}),
            json!({"custom_fields":{}}),
        ] {
            let mut valid = base.clone();
            valid
                .as_object_mut()
                .ok_or("body")?
                .extend(change.as_object().ok_or("change")?.clone());
            assert!(
                validator.is_valid(&valid),
                "valid {schema_name} edit rejected"
            );
        }
        for change in invalid.into_iter().chain([
            json!({"tags":null}),
            json!({"tags":[tag,tag]}),
            json!({"tags":["00000000-0000-4000-8000-00000000008A"]}),
            json!({"custom_fields":null}),
            json!({"created":"2000-01-01T00:00:00Z"}),
        ]) {
            let mut bad = base.clone();
            bad.as_object_mut()
                .ok_or("body")?
                .extend(change.as_object().ok_or("change")?.clone());
            assert!(
                !validator.is_valid(&bad),
                "invalid {schema_name} edit accepted"
            );
        }
    }
    assert!(document["paths"][POLICIES]["post"].is_object());
    assert!(document["paths"][RULES]["post"].is_object());
    Ok(())
}

async fn call(
    app: &Router,
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
#[ignore = "requires disposable PostgreSQL 18 and Redis; main owns native execution"]
async fn native_vlan_translation_contract() -> TestResult {
    let (child, url) = child_database::ChildDatabase::create("pwa80_vlan_").await?;
    let result = async {
        let pool = PgPool::connect(&url).await?;
        let result = exercise_native(&url, pool.clone()).await;
        pool.close().await;
        result
    }
    .await;
    // Pool close sends termination without a server-side acknowledgement. Wait
    // for its owned sessions to disappear before the unchanged DROP guard.
    let name = url::Url::parse(&url)?
        .path()
        .trim_start_matches('/')
        .to_owned();
    let mut observer = PgConnection::connect(&common::database_url()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let connected: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = $1)",
            )
            .bind(&name)
            .fetch_one(&mut observer)
            .await?;
            if !connected {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    observer.close().await?;
    let cleanup = child.cleanup().await;
    if cleanup.is_err() {
        // Do not print connection URLs or SQL text from the disposable fixture.
        let diagnostic = async {
            let name = url::Url::parse(&url)?.path().trim_start_matches('/').to_owned();
            let admin = PgPool::connect(&common::database_url()).await?;
            let counts: (i64, i64, i64) = sqlx::query_as(
                "SELECT count(*), count(*) FILTER (WHERE state = 'active'), count(*) FILTER (WHERE state LIKE 'idle%') FROM pg_stat_activity WHERE datname = $1",
            )
            .bind(name)
            .fetch_one(&admin)
            .await?;
            admin.close().await;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(counts)
        }
        .await;
        match diagnostic {
            Ok((total, active, idle)) => eprintln!(
                "VLAN child cleanup activity: total={total}, active={active}, idle={idle}"
            ),
            Err(_) => eprintln!("VLAN child cleanup activity diagnostic unavailable"),
        }
    }
    result?;
    cleanup?;
    Ok(())
}

async fn exercise_native(url: &str, pool: PgPool) -> TestResult {
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

    for collection in [POLICIES, RULES] {
        call(&app, None, "GET", collection, None, StatusCode::FORBIDDEN).await?;
        call(
            &app,
            None,
            "POST",
            collection,
            Some(json!({})),
            StatusCode::UNAUTHORIZED,
        )
        .await?;
        call(
            &app,
            Some(&cookie),
            "GET",
            &format!("{collection}?sort=unsupported"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        call(
            &app,
            Some(&cookie),
            "GET",
            &format!("{collection}/{}", Uuid::now_v7()),
            None,
            StatusCode::NOT_FOUND,
        )
        .await?;
    }
    for invalid in [
        json!({"name":""}),
        json!({"name":"x".repeat(101)}),
        json!({"name":"bad","description":"x".repeat(201)}),
        json!({"name":"bad","tags":null}),
        json!({"name":"bad","custom_fields":null}),
        json!({"name":"bad","owner_id":Uuid::now_v7()}),
        json!({"name":"bad","status":"active"}),
    ] {
        call(
            &app,
            Some(&cookie),
            "POST",
            POLICIES,
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
        Some(json!({"name":"Translation owner"})),
        StatusCode::CREATED,
    )
    .await?;
    let tag = call(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/tags",
        Some(json!({"name":"Translation tag","slug":"translation-tag","color":"123abc"})),
        StatusCode::CREATED,
    )
    .await?;
    let policy = call(&app,Some(&cookie),"POST",POLICIES,Some(json!({"name":"Translate","owner_id":owner["id"],"description":"kept","comments":"note","tags":[tag["id"]]})),StatusCode::CREATED).await?;
    let policy_id = policy["id"].as_str().ok_or("policy id")?;
    let policy_path = format!("{POLICIES}/{policy_id}");
    assert_eq!(policy["owner_name"], "Translation owner");
    assert_eq!(policy["tags"], json!([tag["id"]]));
    assert!(policy["created"].is_string());
    assert_eq!(policy["created"], policy["last_updated"]);
    call(
        &app,
        Some(&cookie),
        "POST",
        POLICIES,
        Some(json!({"name":"Translate"})),
        StatusCode::CONFLICT,
    )
    .await?;
    let rule = call(&app,Some(&cookie),"POST",RULES,Some(json!({"policy_id":policy_id,"local_vid":1,"remote_vid":4094,"description":"edge","tags":[tag["id"]]})),StatusCode::CREATED).await?;
    let rule_id = rule["id"].as_str().ok_or("rule id")?;
    let rule_path = format!("{RULES}/{rule_id}");
    assert_eq!(rule["policy_name"], "Translate");
    assert_eq!(rule["tags"], json!([tag["id"]]));
    for absent in [
        "name",
        "owner_id",
        "owner_name",
        "comments",
        "status",
        "piping_synced_at",
    ] {
        assert!(rule.get(absent).is_none());
    }
    for invalid in [
        json!({"local_vid":0}),
        json!({"local_vid":4095}),
        json!({"remote_vid":-1}),
        json!({"remote_vid":4095}),
        json!({"policy_id":Uuid::now_v7()}),
        json!({"description":"x".repeat(201)}),
        json!({"name":"not-a-primary-model"}),
        json!({"owner_id":owner["id"]}),
        json!({"comments":"invalid"}),
        json!({"tags":[Uuid::now_v7()]}),
    ] {
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &rule_path,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    for (path, fields) in [
        (
            &policy_path,
            vec!["name", "description", "comments", "tags", "custom_fields"],
        ),
        (
            &rule_path,
            vec![
                "policy_id",
                "local_vid",
                "remote_vid",
                "description",
                "tags",
                "custom_fields",
            ],
        ),
    ] {
        for field in fields {
            let payload = Value::Object([(field.to_string(), Value::Null)].into_iter().collect());
            call(
                &app,
                Some(&cookie),
                "PATCH",
                path,
                Some(payload),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
    }
    let changed = call(
        &app,
        Some(&cookie),
        "PATCH",
        &policy_path,
        Some(json!({"owner_id":null,"tags":[]})),
        StatusCode::OK,
    )
    .await?;
    assert!(changed["owner_id"].is_null());
    assert!(changed["owner_name"].is_null());
    assert_eq!(changed["tags"], json!([]));
    assert_eq!(changed["description"], "kept");
    assert_eq!(changed["comments"], "note");
    assert_eq!(changed["created"], policy["created"]);
    let cleared = call(
        &app,
        Some(&cookie),
        "PATCH",
        &rule_path,
        Some(json!({"tags":[]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(cleared["tags"], json!([]));
    assert_eq!(cleared["local_vid"], 1);
    assert_eq!(cleared["remote_vid"], 4094);
    assert_eq!(cleared["created"], rule["created"]);
    let page = call(
        &app,
        Some(&cookie),
        "GET",
        &format!("{POLICIES}?name=Translate&search=Translate&offset=100&sort=name"),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"], json!([]));
    let page = call(
        &app,
        Some(&cookie),
        "GET",
        &format!(
            "{RULES}?policy_id={policy_id}&local_vid=1&remote_vid=4094&search=Trans&sort=local_vid"
        ),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], rule["id"]);
    let unfiltered = call(
        &app,
        Some(&cookie),
        "GET",
        &format!("{RULES}?policy_id={policy_id}&search=%20%20"),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(unfiltered["total"], 1);
    for query in ["local_vid=0", "remote_vid=4095", "name=unsupported"] {
        call(
            &app,
            Some(&cookie),
            "GET",
            &format!("{RULES}?{query}"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let (a, b) = tokio::join!(
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &policy_path,
            Some(json!({"name":"Renamed"})),
            StatusCode::OK
        ),
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &policy_path,
            Some(json!({"comments":"Concurrent"})),
            StatusCode::OK
        )
    );
    a?;
    b?;
    let final_policy = call(
        &app,
        Some(&cookie),
        "GET",
        &policy_path,
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(final_policy["name"], "Renamed");
    assert_eq!(final_policy["comments"], "Concurrent");
    for (a, b) in [
        (
            json!({"policy_id":policy_id,"local_vid":20,"remote_vid":21}),
            json!({"policy_id":policy_id,"local_vid":20,"remote_vid":22}),
        ),
        (
            json!({"policy_id":policy_id,"local_vid":30,"remote_vid":31}),
            json!({"policy_id":policy_id,"local_vid":32,"remote_vid":31}),
        ),
    ] {
        let a = a.to_string();
        let b = b.to_string();
        let (first, second) = tokio::join!(
            common::request(&app, "POST", RULES, Some(&a), Some(&cookie)),
            common::request(&app, "POST", RULES, Some(&b), Some(&cookie))
        );
        let statuses = [first?.0, second?.0];
        assert!(statuses.contains(&StatusCode::CREATED));
        assert!(statuses.contains(&StatusCode::CONFLICT));
    }
    direct_writer_graph_locks(&pool).await?;
    permissions(
        &app,
        &cookie,
        &pool,
        &mut redis,
        &policy_path,
        &rule_path,
        &owner["id"],
    )
    .await?;
    token_ceilings(&app, &cookie, &policy_path, POLICIES).await?;
    token_ceilings(&app, &cookie, &rule_path, RULES).await?;
    secret_atomicity(&app, &cookie, &pool, &policy_path, &rule_path, policy_id).await?;

    // Both interface classes protect policies and produce structured blockers.
    let vm = Uuid::now_v7();
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot) VALUES ($1,'{}','VLAN translation VM','active','','',0,0,'','enabled')").bind(vm).execute(&pool).await?;
    for table in ["dcim_interface", "virtualization_vminterface"] {
        let id = Uuid::now_v7();
        let sql = if table == "dcim_interface" {
            "INSERT INTO dcim_interface (id,custom_field_data,name,label,description,mark_connected,enabled,_name,type,mgmt_only,vlan_translation_policy_id) VALUES ($1,'{}','eth0','','',false,true,'eth0','virtual',false,$2)"
        } else {
            "INSERT INTO virtualization_vminterface (id,custom_field_data,name,_name,enabled,description,mtu,vlan_translation_policy_id,virtual_machine_id) VALUES ($1,'{}','eth0','eth0',true,'',1500,$2,$3)"
        };
        let mut insert = sqlx::query(sql).bind(id).bind(Uuid::parse_str(policy_id)?);
        if table == "virtualization_vminterface" {
            insert = insert.bind(vm);
        }
        insert.execute(&pool).await?;
        let blocked = call(
            &app,
            Some(&cookie),
            "DELETE",
            &policy_path,
            None,
            StatusCode::CONFLICT,
        )
        .await?;
        assert_eq!(blocked["code"], "DELETE_BLOCKED");
        assert_eq!(blocked["details"]["total_count"], 1);
        assert!(
            blocked["details"]["blockers"]
                .as_array()
                .ok_or("blocker groups")?
                .iter()
                .any(|group| group["table"] == table && group["count"] == 1)
        );
        assert_eq!(
            call(&app, Some(&cookie), "GET", &rule_path, None, StatusCode::OK).await?["id"],
            rule["id"]
        );
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("DELETE FROM ");
        query.push(table).push(" WHERE id=$1");
        query.build().bind(id).execute(&pool).await?;
    }
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &policy_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    let rules_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ipam_vlantranslationrule WHERE policy_id=$1")
            .bind(Uuid::parse_str(policy_id)?)
            .fetch_one(&pool)
            .await?;
    assert_eq!(rules_left, 0);
    let usages_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM secret_usage_binding WHERE consumer_id=ANY($1)")
            .bind(vec![Uuid::parse_str(policy_id)?, Uuid::parse_str(rule_id)?])
            .fetch_one(&pool)
            .await?;
    assert_eq!(usages_left, 0);
    call(
        &app,
        Some(&cookie),
        "GET",
        &rule_path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &rule_path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    Ok(())
}

async fn direct_writer_graph_locks(pool: &PgPool) -> TestResult {
    let mut owner = pool.begin().await?;
    sqlx::query("SELECT dcim_acquire_hardware_graph()")
        .execute(&mut *owner)
        .await?;
    for table in [
        "ipam_vlantranslationpolicy",
        "ipam_vlantranslationrule",
        "dcim_interface",
        "virtualization_vminterface",
        "wireless_wirelesslink",
    ] {
        let mut writer = pool.begin().await?;
        sqlx::query("SET LOCAL lock_timeout='100ms'")
            .execute(&mut *writer)
            .await?;
        // Statement triggers must lock even with no matching row; no row lock
        // can explain this timeout. The transaction owner is the shared graph.
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE ");
        query
            .push(table)
            .push(" SET description=description WHERE false");
        let error = query
            .build()
            .execute(&mut *writer)
            .await
            .expect_err("direct writer bypassed graph owner");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        );
        writer.rollback().await?;
    }
    owner.rollback().await?;
    Ok(())
}

async fn permissions(
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    redis: &mut redis::aio::ConnectionManager,
    policy: &str,
    rule: &str,
    owner: &Value,
) -> TestResult {
    let inaccessible_policy = call(
        app,
        Some(cookie),
        "POST",
        POLICIES,
        Some(json!({"name":format!("Permission target {}", Uuid::now_v7())})),
        StatusCode::CREATED,
    )
    .await?;
    let saved: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(pool)
            .await?;
    let policy_id = policy.rsplit('/').next().ok_or("policy id")?;
    let rule_id = rule.rsplit('/').next().ok_or("rule id")?;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(json!([
            format!("inventory:{policy_id}"),
            format!("inventory:{rule_id}")
        ]))
        .execute(pool)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut *redis)
        .await?;
    let result: TestResult = async {
        for path in [policy, rule] {
            call(app, Some(cookie), "GET", path, None, StatusCode::OK).await?;
            call(
                app,
                Some(cookie),
                "PATCH",
                path,
                Some(json!({"description":"scoped"})),
                StatusCode::OK,
            )
            .await?;
        }
        for collection in [POLICIES, RULES] {
            call(
                app,
                Some(cookie),
                "GET",
                collection,
                None,
                StatusCode::FORBIDDEN,
            )
            .await?;
        }
        call(
            app,
            Some(cookie),
            "POST",
            POLICIES,
            Some(json!({"name":"Denied"})),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "POST",
            RULES,
            Some(json!({"policy_id":policy_id,"local_vid":99,"remote_vid":99})),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            policy,
            Some(json!({"owner_id":owner})),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            rule,
            Some(json!({"policy_id":inaccessible_policy["id"]})),
            StatusCode::FORBIDDEN,
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

async fn token_ceilings(app: &Router, cookie: &str, path: &str, collection: &str) -> TestResult {
    let id = path.rsplit('/').next().ok_or("resource id")?;
    for actions in [vec!["read"], vec!["update"], vec!["read", "update"]] {
        let scopes: Vec<_> = actions.iter().map(|action| json!({"action":action,"resource_type":"inventory","resource_pattern":format!("inventory:{id}")})).collect();
        let token = call(
            app,
            Some(cookie),
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(json!({"name":format!("Translation {}",Uuid::now_v7()),"scopes":scopes})),
            StatusCode::CREATED,
        )
        .await?;
        let bearer = token["token_value"].as_str().ok_or("token value")?;
        for (method, uri, payload, expected) in [
            (
                "GET",
                path,
                None,
                if actions.contains(&"read") {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
            ),
            (
                "PATCH",
                path,
                Some(json!({"description":"PAT update"})),
                if actions.contains(&"update") {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
            ),
            ("GET", collection, None, StatusCode::FORBIDDEN),
            ("DELETE", path, None, StatusCode::FORBIDDEN),
        ] {
            let payload = payload.map(|v| v.to_string());
            let (status, _) =
                common::request_with_auth(app, method, uri, payload.as_deref(), None, Some(bearer))
                    .await?;
            assert_eq!(status, expected, "PAT {actions:?}: {method} {uri}");
        }
    }
    Ok(())
}

async fn snapshot(pool: &PgPool) -> TestResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('policies',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM ipam_vlantranslationpolicy t),'rules',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM ipam_vlantranslationrule t),'store',(SELECT jsonb_agg(to_jsonb(t) ORDER BY secret_id) FROM secrets_store t),'variables',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secrets_variable t),'usage',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secret_usage_binding t))").fetch_one(pool).await?)
}

async fn secret_atomicity(
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    policy: &str,
    rule: &str,
    policy_id: &str,
) -> TestResult {
    for (model, path) in [
        ("vlantranslationpolicy", policy),
        ("vlantranslationrule", rule),
    ] {
        let scope: Uuid = sqlx::query_scalar(
            "SELECT id FROM inventory_content_type WHERE app_label='ipam' AND model=$1",
        )
        .bind(model)
        .fetch_one(pool)
        .await?;
        let field = format!("{model}_credential");
        call(app,Some(cookie),"POST","/api/v1/custom-fields",Some(json!({"name":field,"label":"Translation credential","field_type":"text","is_secret":true,"object_types":[scope]})),StatusCode::CREATED).await?;
        let value = Uuid::now_v7().to_string();
        let fields = Value::Object([(field.clone(), json!(value))].into_iter().collect());
        let masked = call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(json!({"custom_fields":fields})),
            StatusCode::OK,
        )
        .await?;
        assert_eq!(masked["custom_fields"][&field], "****");
        let id = Uuid::parse_str(path.rsplit('/').next().ok_or("resource id")?)?;
        let bindings: i64 =
            sqlx::query_scalar("SELECT count(*) FROM secret_usage_binding WHERE consumer_id=$1")
                .bind(id)
                .fetch_one(pool)
                .await?;
        assert_eq!(bindings, 1);
    }
    sqlx::query("CREATE FUNCTION pwa80_vlan_secret_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic secret storage failure'; END $$").execute(pool).await?;
    sqlx::query("CREATE TRIGGER pwa80_vlan_secret_fault BEFORE INSERT OR UPDATE ON secrets_store FOR EACH ROW EXECUTE FUNCTION pwa80_vlan_secret_fault()").execute(pool).await?;
    let before = snapshot(pool).await?;
    for (method, path, payload) in [
        (
            "PATCH",
            policy,
            json!({"description":"must roll back","custom_fields":{"vlantranslationpolicy_credential":Uuid::now_v7().to_string()}}),
        ),
        (
            "PATCH",
            rule,
            json!({"description":"must roll back","custom_fields":{"vlantranslationrule_credential":Uuid::now_v7().to_string()}}),
        ),
        (
            "POST",
            POLICIES,
            json!({"name":"Must roll back","custom_fields":{"vlantranslationpolicy_credential":Uuid::now_v7().to_string()}}),
        ),
        (
            "POST",
            RULES,
            json!({"policy_id":policy_id,"local_vid":4000,"remote_vid":4001,"custom_fields":{"vlantranslationrule_credential":Uuid::now_v7().to_string()}}),
        ),
    ] {
        call(
            app,
            Some(cookie),
            method,
            path,
            Some(payload),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await?;
        assert_eq!(snapshot(pool).await?, before);
    }
    sqlx::query("DROP TRIGGER pwa80_vlan_secret_fault ON secrets_store")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION pwa80_vlan_secret_fault()")
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18; exercises raw pre-0010 upgrade and rollback"]
async fn vlan_translation_upgrade_preserves_rows_and_refuses_bad_vids() -> TestResult {
    let (child, url) = child_database::ChildDatabase::create("pwa80_vlan_").await?;
    let result = async {
        let pool = PgPool::connect(&url).await?;
        let result = exercise_upgrade(pool.clone()).await;
        pool.close().await;
        result
    }
    .await;
    let cleanup = child.cleanup().await;
    result?;
    cleanup?;
    Ok(())
}

async fn exercise_upgrade(pool: PgPool) -> TestResult {
    for sql in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_interface_networking.sql"),
        include_str!("../migrations/0003_service_port_mappings.sql"),
        include_str!("../migrations/0004_fhrp_auth_key_secret.sql"),
        include_str!("../migrations/0005_module_bay_types.sql"),
        include_str!("../migrations/0006_canonical_tag_ids.sql"),
        include_str!("../migrations/0007_cooling_hardware.sql"),
        include_str!("../migrations/0008_module_type_profile_scope.sql"),
        include_str!("../migrations/0009_circuit_group_assignments.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await?;
    }
    let policy = Uuid::now_v7();
    let rule = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_vlantranslationpolicy (id,name,description,comments,custom_field_data) VALUES ($1,'Historical policy','historical','preserved','{\"text\":\"retained\"}')").bind(policy).execute(&pool).await?;
    sqlx::query("INSERT INTO ipam_vlantranslationrule (id,policy_id,local_vid,remote_vid,description,custom_field_data) VALUES ($1,$2,0,4095,'historical','{\"number\":7}')").bind(rule).bind(policy).execute(&pool).await?;
    sqlx::query("INSERT INTO inventory_content_type (app_label,model) VALUES ('ipam','vlantranslationpolicy'),('ipam','vlantranslationrule') ON CONFLICT (app_label,model) DO NOTHING").execute(&pool).await?;
    let ct: Vec<(Uuid,String)> = sqlx::query_as("SELECT id,model FROM inventory_content_type WHERE app_label='ipam' AND model IN ('vlantranslationpolicy','vlantranslationrule') ORDER BY model").fetch_all(&pool).await?;
    let forward = include_str!("../migrations/0010_vlan_translation.sql");
    let mut tx = pool.begin().await?;
    let failure = sqlx::raw_sql(forward)
        .execute(&mut *tx)
        .await
        .expect_err("bad existing VIDs accepted");
    assert_eq!(
        failure
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );
    tx.rollback().await?;
    let values: (i16, i16) =
        sqlx::query_as("SELECT local_vid,remote_vid FROM ipam_vlantranslationrule WHERE id=$1")
            .bind(rule)
            .fetch_one(&pool)
            .await?;
    assert_eq!(values, (0, 4095));
    let tags_exist: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='ipam_vlantranslationrule' AND column_name='tags')").fetch_one(&pool).await?;
    assert!(!tags_exist);
    // A separate operator correction is explicit test setup, not migration repair.
    sqlx::query("UPDATE ipam_vlantranslationrule SET local_vid=1,remote_vid=4094 WHERE id=$1")
        .bind(rule)
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(forward).execute(&mut *tx).await?;
    tx.commit().await?;
    let after_ct: Vec<(Uuid,String)> = sqlx::query_as("SELECT id,model FROM inventory_content_type WHERE app_label='ipam' AND model IN ('vlantranslationpolicy','vlantranslationrule') ORDER BY model").fetch_all(&pool).await?;
    assert_eq!(ct, after_ct);
    for (table, id, expected) in [
        (
            "ipam_vlantranslationpolicy",
            policy,
            json!({"text":"retained"}),
        ),
        ("ipam_vlantranslationrule", rule, json!({"number":7})),
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT custom_field_data,tags,created,last_updated FROM ",
        );
        query.push(table).push(" WHERE id=$1");
        let row: (
            Value,
            Vec<String>,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) = query.build_query_as().bind(id).fetch_one(&pool).await?;
        assert_eq!(row.0, expected);
        assert!(row.1.is_empty());
        assert!(row.2.is_none());
        assert!(row.3.is_none());
    }
    for (local, remote) in [(0, 2), (4095, 2), (2, 0), (2, 4095)] {
        assert!(sqlx::query("INSERT INTO ipam_vlantranslationrule (id,policy_id,local_vid,remote_vid,description,custom_field_data) VALUES ($1,$2,$3,$4,'','{}')").bind(Uuid::now_v7()).bind(policy).bind(local as i16).bind(remote as i16).execute(&pool).await.is_err());
    }
    sqlx::query("DELETE FROM ipam_vlantranslationpolicy WHERE id=$1")
        .bind(policy)
        .execute(&pool)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ipam_vlantranslationrule WHERE id=$1")
            .bind(rule)
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}
