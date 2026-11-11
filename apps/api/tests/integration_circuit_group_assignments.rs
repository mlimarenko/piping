//! Selected native proof for Main's disposable PostgreSQL 18/Redis fixture.
//! This source-preparation pass does not execute native tests.
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
const COLLECTION: &str = "/api/v1/inventory/circuit-group-assignments";

#[test]
fn published_membership_input_contract_validates_boundary_cases() -> TestResult {
    use utoipa::OpenApi;
    let doc = serde_json::to_value(piping_backend::api::openapi::ApiDoc::openapi())?;
    let group = Uuid::from_u128(0x80);
    let member = Uuid::from_u128(0x81);
    let tag = "00000000-0000-4000-8000-000000000082";
    for (schema_name, create) in [
        ("CircuitGroupAssignmentCreatePayload", true),
        ("CircuitGroupAssignmentPatchPayload", false),
    ] {
        let schema = json!({"$ref":format!("#/components/schemas/{schema_name}"),"components":doc["components"]});
        let validator = jsonschema::validator_for(&schema)?;
        let base = if create {
            payload(group, member, "circuits.circuit")
        } else {
            json!({})
        };
        assert!(validator.is_valid(&base));
        for change in [
            json!({"tags":[]}),
            json!({"tags":[tag]}),
            json!({"priority":null}),
            json!({"priority":""}),
            json!({"priority":"tertiary"}),
        ] {
            let mut valid = base.clone();
            valid
                .as_object_mut()
                .ok_or("body")?
                .extend(change.as_object().ok_or("change")?.clone());
            assert!(
                validator.is_valid(&valid),
                "valid {schema_name} boundary input rejected"
            );
        }
        for change in [
            json!({"group_id":null}),
            json!({"member":null}),
            json!({"member":{"object_type":"dcim.device","object_id":member}}),
            json!({"member":{"object_type":"circuits.circuit","object_id":member,"label":"do not accept"}}),
            json!({"tags":null}),
            json!({"tags":[tag,tag]}),
            json!({"tags":["00000000-0000-4000-8000-00000000008A"]}),
            json!({"priority":"high"}),
            json!({"custom_fields":null}),
            json!({"name":"phantom"}),
            json!({"created":"2000-01-01T00:00:00Z"}),
        ] {
            let mut invalid = base.clone();
            invalid
                .as_object_mut()
                .ok_or("body")?
                .extend(change.as_object().ok_or("change")?.clone());
            assert!(
                !validator.is_valid(&invalid),
                "invalid {schema_name} boundary input accepted"
            );
        }
    }
    assert!(doc["paths"][COLLECTION]["post"].is_object());
    assert!(doc["paths"][format!("{COLLECTION}/{{id}}")]["patch"].is_object());
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
    let payload = payload.map(|v| v.to_string());
    let (status, body) = common::request(app, method, path, payload.as_deref(), cookie).await?;
    assert_eq!(status, expected, "{method} {path}");
    Ok(if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    })
}
#[derive(Clone, Copy)]
struct Graph {
    group: Uuid,
    other_group: Uuid,
    member: Uuid,
}
async fn graph(pool: &PgPool) -> TestResult<Graph> {
    let provider = Uuid::now_v7();
    let circuit_type = Uuid::now_v7();
    let virtual_type = Uuid::now_v7();
    let network = Uuid::now_v7();
    let graph = Graph {
        group: Uuid::now_v7(),
        other_group: Uuid::now_v7(),
        member: Uuid::now_v7(),
    };
    sqlx::query("INSERT INTO circuits_provider(id,name,slug,description,comments,custom_field_data) VALUES($1,'PWA80 Provider','pwa80-provider','','','{}')").bind(provider).execute(pool).await?;
    sqlx::query("INSERT INTO circuits_circuittype(id,name,slug,color,description,comments,custom_field_data) VALUES($1,'Physical','physical','155dfc','','','{}')").bind(circuit_type).execute(pool).await?;
    sqlx::query("INSERT INTO circuits_virtualcircuittype(id,name,slug,color,description,comments,custom_field_data) VALUES($1,'Virtual','virtual','155dfc','','','{}')").bind(virtual_type).execute(pool).await?;
    sqlx::query("INSERT INTO circuits_providernetwork(id,provider_id,name,service_id,description,comments,custom_field_data) VALUES($1,$2,'PWA80 Network','','','','{}')").bind(network).bind(provider).execute(pool).await?;
    sqlx::query("INSERT INTO circuits_circuit(id,provider_id,type_id,cid,status,description,comments,custom_field_data) VALUES($1,$2,$3,'Physical member','active','','','{}')").bind(graph.member).bind(provider).bind(circuit_type).execute(pool).await?;
    // A typed GFK must distinguish equal UUIDs in different target tables.
    sqlx::query("INSERT INTO circuits_virtualcircuit(id,provider_network_id,type_id,cid,status,description,comments,custom_field_data) VALUES($1,$2,$3,'Virtual member','active','','','{}')").bind(graph.member).bind(network).bind(virtual_type).execute(pool).await?;
    for (id, name) in [(graph.group, "WAN"), (graph.other_group, "Other WAN")] {
        sqlx::query("INSERT INTO circuits_circuitgroup(id,name,slug,description,comments,custom_field_data) VALUES($1,$2,$3,'','','{}')").bind(id).bind(name).bind(id.to_string()).execute(pool).await?;
    }
    Ok(graph)
}
fn payload(group: Uuid, member: Uuid, kind: &str) -> Value {
    json!({"group_id":group,"member":{"object_type":kind,"object_id":member},"priority":"primary"})
}

async fn member_delete_blockers_follow_read_ceiling(
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    graph: Graph,
    assignment_id: &str,
    member_path: &str,
) -> TestResult {
    let assignment_id = Uuid::parse_str(assignment_id)?;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM circuits_circuitgroupassignment a WHERE id=$1")
            .bind(assignment_id)
            .fetch_one(pool)
            .await?;
    for readable in [false, true] {
        let mut scopes = vec![json!({
            "action":"delete", "resource_type":"inventory",
            "resource_pattern":format!("inventory:{}", graph.member),
        })];
        if readable {
            scopes
                .push(json!({"action":"read","resource_type":"inventory","resource_pattern":"*"}));
        }
        let issued = call(
            app,
            Some(cookie),
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(
                json!({"name":format!("Member delete ceiling {}", Uuid::now_v7()),"scopes":scopes}),
            ),
            StatusCode::CREATED,
        )
        .await?;
        let bearer = issued["token_value"].as_str().ok_or("issued token")?;
        let (get_status, _) = common::request_with_auth(
            app,
            "GET",
            &format!("{COLLECTION}/{assignment_id}"),
            None,
            None,
            Some(bearer),
        )
        .await?;
        assert_eq!(
            get_status,
            if readable {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        let (status, body) =
            common::request_with_auth(app, "DELETE", member_path, None, None, Some(bearer)).await?;
        assert_eq!(status, StatusCode::CONFLICT);
        let response: Value = serde_json::from_slice(&body)?;
        assert_eq!(response["reason_code"], "DELETE_BLOCKED");
        if readable {
            assert_eq!(response["code"], "DELETE_BLOCKED");
            assert!(response.get("details").is_some());
            assert!(response.to_string().contains(&assignment_id.to_string()));
        } else {
            assert_eq!(response["code"], "CONFLICT");
            assert!(response.get("details").is_none());
            for hidden in [
                assignment_id.to_string(),
                graph.group.to_string(),
                "WAN".into(),
            ] {
                assert!(
                    !response.to_string().contains(&hidden),
                    "unreadable blocker escaped"
                );
            }
        }
        let after: Value = sqlx::query_scalar(
            "SELECT to_jsonb(a) FROM circuits_circuitgroupassignment a WHERE id=$1",
        )
        .bind(assignment_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(before, after);
        for table in ["circuits_circuit", "circuits_virtualcircuit"] {
            let mut query =
                sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT EXISTS(SELECT 1 FROM ");
            query.push(table).push(" WHERE id=$1)");
            let present: bool = query
                .build_query_scalar()
                .bind(graph.member)
                .fetch_one(pool)
                .await?;
            assert!(present, "blocked DELETE removed a typed member");
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn assignment_native_crud_permissions_atomicity_and_concurrency() -> TestResult {
    let mut url = Url::parse(&common::database_url())?;
    url.set_path(&format!("/pwa80_memberships_{}", Uuid::now_v7().simple()));
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
    let graph = graph(&pool).await?;
    let mut settings = piping_backend::config::load_settings()?;
    settings.database.url = url.into();
    settings.redis.url = common::redis_url(Some(13))?;
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
    let create = payload(graph.group, graph.member, "circuits.circuit");
    call(
        &app,
        None,
        "POST",
        COLLECTION,
        Some(create.clone()),
        StatusCode::UNAUTHORIZED,
    )
    .await?;
    for change in [
        json!({"name":"phantom"}),
        json!({"owner_id":Uuid::now_v7()}),
        json!({"comments":"phantom"}),
        json!({"created":"2000-01-01T00:00:00Z"}),
        json!({"tags":null}),
        json!({"custom_fields":null}),
        json!({"priority":"high"}),
        json!({"member":null}),
        json!({"member":{"object_type":"dcim.device","object_id":graph.member}}),
        json!({"group_id":Uuid::now_v7()}),
    ] {
        let mut invalid = create.clone();
        invalid
            .as_object_mut()
            .ok_or("payload")?
            .extend(change.as_object().ok_or("change")?.clone());
        call(
            &app,
            Some(&cookie),
            "POST",
            COLLECTION,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let item = call(
        &app,
        Some(&cookie),
        "POST",
        COLLECTION,
        Some(create.clone()),
        StatusCode::CREATED,
    )
    .await?;
    let id = item["id"].as_str().ok_or("assignment id")?;
    let path = format!("{COLLECTION}/{id}");
    assert_eq!(item["member"]["label"], "Physical member");
    assert_eq!(item["group_name"], "WAN");
    let tag_id = tag_contract(&app, &cookie, &path).await?;
    assert!(item["created"].is_string());
    call(
        &app,
        Some(&cookie),
        "POST",
        COLLECTION,
        Some(create),
        StatusCode::CONFLICT,
    )
    .await?;
    let page=call(&app,Some(&cookie),"GET",&format!("{COLLECTION}?group_id={}&member_type=circuits.circuit&member_id={}&priority=primary&offset=100",graph.group,graph.member),None,StatusCode::OK).await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"], json!([]));
    call(
        &app,
        Some(&cookie),
        "GET",
        &format!("{COLLECTION}?member_type=dcim.device"),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &format!("/api/v1/inventory/circuits/{}", graph.member),
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    member_delete_blockers_follow_read_ceiling(
        &app,
        &cookie,
        &pool,
        graph,
        id,
        &format!("/api/v1/inventory/circuits/{}", graph.member),
    )
    .await?;
    let changed = call(
        &app,
        Some(&cookie),
        "PATCH",
        &path,
        Some(json!({"member":{"object_type":"circuits.virtualcircuit","object_id":graph.member}})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(changed["member"]["label"], "Virtual member");
    assert_eq!(changed["created"], item["created"]);
    assert_eq!(changed["priority"], "primary");
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &format!("/api/v1/inventory/virtual-circuits/{}", graph.member),
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    member_delete_blockers_follow_read_ceiling(
        &app,
        &cookie,
        &pool,
        graph,
        id,
        &format!("/api/v1/inventory/virtual-circuits/{}", graph.member),
    )
    .await?;
    let (one, two) = tokio::join!(
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &path,
            Some(json!({"priority":null})),
            StatusCode::OK
        ),
        call(
            &app,
            Some(&cookie),
            "PATCH",
            &path,
            Some(json!({"tags":[]})),
            StatusCode::OK
        )
    );
    one?;
    two?;
    assert!(
        call(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await?["priority"].is_null()
    );
    let concurrent = payload(graph.other_group, graph.member, "circuits.circuit").to_string();
    let (one, two) = tokio::join!(
        common::request(&app, "POST", COLLECTION, Some(&concurrent), Some(&cookie)),
        common::request(&app, "POST", COLLECTION, Some(&concurrent), Some(&cookie))
    );
    let statuses = [one?.0, two?.0];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::CONFLICT));
    scoped_denials(&app, &cookie, &pool, &mut redis, &path, id, graph).await?;
    secret_rollback_and_cascade(&app, &cookie, &pool, &path, id, graph).await?;
    call(
        &app,
        Some(&cookie),
        "DELETE",
        &format!("/api/v1/inventory/tags/{tag_id}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    graph_race(&app, &cookie, &pool, graph).await?;
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
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    redis: &mut redis::aio::ConnectionManager,
    path: &str,
    id: &str,
    graph: Graph,
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
            "DELETE",
            &format!("/api/v1/inventory/circuit-groups/{}", graph.group),
            None,
            StatusCode::FORBIDDEN,
        )
        .await?;
        for member_path in [
            format!("/api/v1/inventory/circuits/{}", graph.member),
            format!("/api/v1/inventory/virtual-circuits/{}", graph.member),
        ] {
            call(
                app,
                Some(cookie),
                "DELETE",
                &member_path,
                None,
                StatusCode::FORBIDDEN,
            )
            .await?;
        }
        let tag_id: Uuid =
            sqlx::query_scalar("SELECT id FROM inventory_tag WHERE slug='membership-tag'")
                .fetch_one(pool)
                .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(json!({"tags":[tag_id]})),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(json!({"priority":"secondary"})),
            StatusCode::OK,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "GET",
            COLLECTION,
            None,
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "POST",
            COLLECTION,
            Some(payload(graph.group, graph.member, "circuits.circuit")),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(json!({"group_id":graph.other_group})),
            StatusCode::FORBIDDEN,
        )
        .await?;
        call(
            app,
            Some(cookie),
            "PATCH",
            path,
            Some(json!({"member":{"object_type":"circuits.circuit","object_id":graph.member}})),
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
    result?;
    let token=call(app,Some(cookie),"POST","/api/v1/user/settings/api-tokens",Some(json!({"name":"Membership read ceiling","scopes":[{"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{id}")}]})),StatusCode::CREATED).await?;
    let bearer = token["token_value"].as_str().ok_or("token")?;
    for (method, uri, payload, expected) in [
        ("GET", path.to_string(), None, StatusCode::OK),
        ("GET", COLLECTION.to_string(), None, StatusCode::FORBIDDEN),
        (
            "POST",
            COLLECTION.to_string(),
            Some(payload(graph.group, graph.member, "circuits.circuit")),
            StatusCode::FORBIDDEN,
        ),
        (
            "PATCH",
            path.to_string(),
            Some(json!({"priority":"inactive"})),
            StatusCode::FORBIDDEN,
        ),
        (
            "POST",
            format!("{path}/custom-fields/reveal"),
            None,
            StatusCode::FORBIDDEN,
        ),
        ("DELETE", path.to_string(), None, StatusCode::FORBIDDEN),
    ] {
        let payload = payload.map(|v| v.to_string());
        let (status, _) =
            common::request_with_auth(app, method, &uri, payload.as_deref(), None, Some(bearer))
                .await?;
        assert_eq!(status, expected, "PAT {method} {uri}");
    }
    Ok(())
}

async fn snapshot(pool: &PgPool) -> TestResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('assignments',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM circuits_circuitgroupassignment a),'variables',(SELECT jsonb_agg(to_jsonb(v) ORDER BY id) FROM secrets_variable v),'store',(SELECT jsonb_agg(to_jsonb(s) ORDER BY secret_id) FROM secrets_store s),'usage',(SELECT jsonb_agg(to_jsonb(u) ORDER BY id) FROM secret_usage_binding u))").fetch_one(pool).await?)
}

async fn tag_contract(app: &Router, cookie: &str, path: &str) -> TestResult<String> {
    let tag = call(
        app,
        Some(cookie),
        "POST",
        "/api/v1/inventory/tags",
        Some(json!({"name":"Membership tag","slug":"membership-tag","color":"155dfc","weight":0})),
        StatusCode::CREATED,
    )
    .await?;
    let id = tag["id"].as_str().ok_or("tag id")?.to_string();
    let tagged = call(
        app,
        Some(cookie),
        "PATCH",
        path,
        Some(json!({"tags":[id]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(tagged["tags"], json!([id]));
    for invalid in [
        json!({"tags":null}),
        json!({"tags":[id,id]}),
        json!({"tags":["00000000-0000-4000-8000-00000000000A"]}),
        json!({"tags":[Uuid::now_v7()]}),
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
        &format!("/api/v1/inventory/tags/{id}"),
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    Ok(id)
}
async fn secret_rollback_and_cascade(
    app: &Router,
    cookie: &str,
    pool: &PgPool,
    path: &str,
    id: &str,
    graph: Graph,
) -> TestResult {
    let scope:Uuid=sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label='circuits' AND model='circuitgroupassignment'").fetch_one(pool).await?;
    call(app,Some(cookie),"POST","/api/v1/custom-fields",Some(json!({"name":"membership_secret","label":"Membership secret","field_type":"text","is_secret":true,"object_types":[scope]})),StatusCode::CREATED).await?;
    let value = Uuid::now_v7().to_string();
    let masked = call(
        app,
        Some(cookie),
        "PATCH",
        path,
        Some(json!({"custom_fields":{"membership_secret":value}})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(masked["custom_fields"]["membership_secret"], "****");
    let revealed = call(
        app,
        Some(cookie),
        "POST",
        &format!("{path}/custom-fields/reveal"),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(revealed["custom_fields"]["membership_secret"], value);
    secret_pat_ceilings(app, cookie, path, &value).await?;
    sqlx::query("CREATE FUNCTION pwa80_membership_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic membership secret fault'; END $$").execute(pool).await?;
    sqlx::query("CREATE TRIGGER pwa80_membership_fault BEFORE INSERT OR UPDATE ON secrets_store FOR EACH ROW EXECUTE FUNCTION pwa80_membership_fault()").execute(pool).await?;
    let before = snapshot(pool).await?;
    let mut create = payload(graph.other_group, graph.member, "circuits.virtualcircuit");
    create["custom_fields"] = json!({"membership_secret":Uuid::now_v7().to_string()});
    for (method, target, body) in [
        ("POST", COLLECTION, create),
        (
            "PATCH",
            path,
            json!({"priority":"inactive","custom_fields":{"membership_secret":Uuid::now_v7().to_string()}}),
        ),
    ] {
        call(
            app,
            Some(cookie),
            method,
            target,
            Some(body),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await?;
        assert_eq!(snapshot(pool).await?, before);
    }
    sqlx::query("DROP TRIGGER pwa80_membership_fault ON secrets_store")
        .execute(pool)
        .await?;
    sqlx::query("DROP FUNCTION pwa80_membership_fault()")
        .execute(pool)
        .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1")
            .bind(Uuid::parse_str(id)?)
            .fetch_one(pool)
            .await?;
    assert!(count > 0);
    call(
        app,
        Some(cookie),
        "DELETE",
        &format!("/api/v1/inventory/circuit-groups/{}", graph.group),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1")
            .bind(Uuid::parse_str(id)?)
            .fetch_one(pool)
            .await?;
    assert_eq!(count, 0);
    Ok(())
}
async fn graph_race(app: &Router, cookie: &str, pool: &PgPool, graph: Graph) -> TestResult {
    let member = Uuid::now_v7();
    sqlx::query("INSERT INTO circuits_circuit(id,provider_id,type_id,cid,status,description,comments,custom_field_data) SELECT $1,provider_id,type_id,'Race member','active','','','{}' FROM circuits_circuit WHERE id=$2").bind(member).bind(graph.member).execute(pool).await?;
    let body = payload(graph.other_group, member, "circuits.circuit").to_string();
    let (created, deleted) = tokio::join!(
        common::request(app, "POST", COLLECTION, Some(&body), Some(cookie)),
        sqlx::query("DELETE FROM circuits_circuit WHERE id=$1")
            .bind(member)
            .execute(pool)
    );
    let status = created?.0;
    assert!(matches!(
        (status, deleted.is_ok()),
        (StatusCode::CREATED, false) | (StatusCode::BAD_REQUEST, true)
    ));
    let dangling:i64=sqlx::query_scalar("SELECT COUNT(*) FROM circuits_circuitgroupassignment a JOIN inventory_content_type ct ON ct.id=a.member_type_id LEFT JOIN circuits_circuit c ON ct.model='circuit' AND c.id=a.member_id LEFT JOIN circuits_virtualcircuit v ON ct.model='virtualcircuit' AND v.id=a.member_id WHERE c.id IS NULL AND v.id IS NULL").fetch_one(pool).await?;
    assert_eq!(dangling, 0);
    Ok(())
}

async fn secret_pat_ceilings(
    app: &Router,
    cookie: &str,
    path: &str,
    expected_value: &str,
) -> TestResult {
    let id = path.rsplit('/').next().ok_or("assignment id")?;
    for (actions, can_read, can_reveal) in [
        (vec!["read"], true, false),
        (vec!["secret"], false, false),
        (vec!["read", "secret"], true, true),
    ] {
        let scopes:Vec<_>=actions.iter().map(|action|json!({"action":action,"resource_type":"inventory","resource_pattern":format!("inventory:{id}")})).collect();
        let token=call(app,Some(cookie),"POST","/api/v1/user/settings/api-tokens",Some(json!({"name":format!("Membership secret ceiling {}",Uuid::now_v7()),"scopes":scopes})),StatusCode::CREATED).await?;
        let bearer = token["token_value"].as_str().ok_or("token")?;
        for (method, uri, allowed) in [
            ("GET", path.to_string(), can_read),
            ("POST", format!("{path}/custom-fields/reveal"), can_reveal),
        ] {
            let (status, body) =
                common::request_with_auth(app, method, &uri, None, None, Some(bearer)).await?;
            assert_eq!(
                status,
                if allowed {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
                "membership secret ceiling {actions:?}: {method}"
            );
            if allowed {
                let value: Value = serde_json::from_slice(&body)?;
                assert_eq!(
                    value["custom_fields"]["membership_secret"],
                    if method == "GET" {
                        "****"
                    } else {
                        expected_value
                    }
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18"]
async fn membership_upgrade_refuses_corruption_and_conserves_identity() -> TestResult {
    let mut url = Url::parse(&common::database_url())?;
    url.set_path(&format!(
        "/pwa80_membership_upgrade_{}",
        Uuid::now_v7().simple()
    ));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let result = upgrade(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    result?;
    cleanup?;
    Ok(())
}
async fn upgrade(url: &str) -> TestResult {
    let pool = PgPool::connect(url).await?;
    let migrations = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let prefix = tempfile::tempdir()?;
    for name in [
        "0001_init.sql",
        "0002_interface_networking.sql",
        "0003_service_port_mappings.sql",
        "0004_fhrp_auth_key_secret.sql",
        "0005_module_bay_types.sql",
        "0006_canonical_tag_ids.sql",
        "0007_cooling_hardware.sql",
        "0008_module_type_profile_scope.sql",
    ] {
        std::fs::copy(migrations.join(name), prefix.path().join(name))?;
    }
    Migrator::new(prefix.path()).await?.run(&pool).await?;
    let graph = graph(&pool).await?;
    let scope = Uuid::now_v7();
    let member_type = Uuid::now_v7();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_content_type(id,app_label,model) VALUES($1,'circuits','circuitgroupassignment'),($2,'circuits','circuit')").bind(scope).bind(member_type).execute(&pool).await?;
    sqlx::query("INSERT INTO circuits_circuitgroupassignment(id,member_type_id,member_id,group_id,priority,custom_field_data,external_id,piping_synced_at) VALUES($1,$2,$3,$4,'secondary','{\"imported\":42}',1900,'2020-02-03T04:05:06Z')").bind(id).bind(member_type).bind(Uuid::now_v7()).bind(graph.group).execute(&pool).await?;
    let history: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(m)::text FROM _sqlx_migrations m ORDER BY version")
            .fetch_all(&pool)
            .await?;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM circuits_circuitgroupassignment a WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert!(
        Migrator::new(migrations.as_path())
            .await?
            .run(&pool)
            .await
            .is_err()
    );
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM circuits_circuitgroupassignment a WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(before, after);
    let ledger: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(m)::text FROM _sqlx_migrations m ORDER BY version")
            .fetch_all(&pool)
            .await?;
    assert_eq!(history, ledger);
    sqlx::query("UPDATE circuits_circuitgroupassignment SET member_id=$2 WHERE id=$1")
        .bind(id)
        .bind(graph.member)
        .execute(&pool)
        .await?;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM circuits_circuitgroupassignment a WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    Migrator::new(migrations.as_path())
        .await?
        .run(&pool)
        .await?;
    let after:Value=sqlx::query_scalar("SELECT to_jsonb(a)-'created'-'last_updated' FROM circuits_circuitgroupassignment a WHERE id=$1").bind(id).fetch_one(&pool).await?;
    assert_eq!(before, after);
    let local_dates: (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT created,last_updated FROM circuits_circuitgroupassignment WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(local_dates, (None, None));
    let actual:Uuid=sqlx::query_scalar("SELECT id FROM inventory_content_type WHERE app_label='circuits' AND model='circuitgroupassignment'").fetch_one(&pool).await?;
    assert_eq!(actual, scope);
    pool.close().await;
    Ok(())
}
