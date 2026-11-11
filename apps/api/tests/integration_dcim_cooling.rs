//! Native cooling graph, template factory and authorization behavior.
mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct HttpFixture {
    app: Router,
    cookie: String,
}
impl HttpFixture {
    async fn start() -> TestResult<Self> {
        let app = piping_backend::app::build_router(build_test_app_state().await?);
        let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
            .await
            .ok_or("admin login")?;
        Ok(Self { app, cookie })
    }
    async fn call(
        &self,
        method: Method,
        path: &str,
        payload: Option<Value>,
        expected: StatusCode,
    ) -> TestResult<Value> {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("cookie", &self.cookie)
            .header("content-type", "application/json")
            .body(Body::from(
                payload.map(|value| value.to_string()).unwrap_or_default(),
            ))?;
        let response = self.app.clone().oneshot(request).await?;
        assert_eq!(response.status(), expected, "{path}");
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok(if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        })
    }
}

async fn site(db: &sqlx::PgPool) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(id).bind(format!("cooling-{id}")).execute(db).await?;
    Ok(id)
}

async fn device(db: &sqlx::PgPool, site: Uuid, device_type: Uuid) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),$4,$3,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(id).bind(format!("cooling-{id}")).bind(site).bind(device_type).execute(db).await?;
    Ok(id)
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native REST cooling graph"]
async fn cooling_crud_graph_sites_nullable_patch_and_delete_blockers() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let a = site(&db).await?;
    let b = site(&db).await?;
    let tag = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_tag (id,name,slug) VALUES ($1,$2,$2)")
        .bind(tag)
        .bind(format!("cooling-tag-{tag}"))
        .execute(&db)
        .await?;
    let rack = http
        .call(
            Method::POST,
            "/api/v1/inventory/racks",
            Some(json!({"name":format!("cooling-rack-{a}"),"site_id":a,"width":19,"u_height":42})),
            StatusCode::CREATED,
        )
        .await?;
    let source=http.call(Method::POST,"/api/v1/inventory/cooling-sources",Some(json!({"name":"chiller","site_id":a,"type":"chiller","cooling_capacity":100.25,"comments":"retain","tags":[tag.to_string()]})),StatusCode::CREATED).await?;
    let source_path = format!(
        "/api/v1/inventory/cooling-sources/{}",
        source["id"].as_str().ok_or("source id")?
    );
    let feed=http.call(Method::POST,"/api/v1/inventory/cooling-feeds",Some(json!({"name":"loop","cooling_source_id":source["id"],"rack_id":rack["id"],"max_flow":10.25,"max_flow_unit":"lpm"})),StatusCode::CREATED).await?;
    let feed_path = format!(
        "/api/v1/inventory/cooling-feeds/{}",
        feed["id"].as_str().ok_or("feed id")?
    );
    http.call(
        Method::PUT,
        &source_path,
        Some(json!({"site_id":b})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let blocked = http
        .call(Method::DELETE, &source_path, None, StatusCode::CONFLICT)
        .await?;
    assert!(
        !blocked.is_null(),
        "referenced cooling source must return a structured blocker"
    );
    let changed = http
        .call(
            Method::PUT,
            &source_path,
            Some(json!({"description":"changed","fluid_type":null})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(changed["comments"], "retain");
    assert_eq!(changed["tags"], json!([tag.to_string()]));
    assert_eq!(changed["created"], source["created"]);
    let page = http
        .call(
            Method::GET,
            &format!("/api/v1/inventory/cooling-sources?site_id={a}&limit=1&offset=20"),
            None,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"], json!([]));
    for payload in [
        json!({"type":"unknown"}),
        json!({"cooling_capacity":-1}),
        json!({"status":null}),
        json!({"tags":null}),
        json!({"cooling_capacity":1.001}),
    ] {
        http.call(
            Method::PUT,
            &source_path,
            Some(payload),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .fetch_one(&db)
            .await?;
    let kind = http.call(Method::POST, "/api/v1/inventory/device_types", Some(json!({"manufacturer_id":manufacturer,"model":format!("cooling-fresh-{a}"),"slug":format!("cooling-fresh-{a}")})), StatusCode::CREATED).await?;
    let kind_id = Uuid::parse_str(kind["id"].as_str().ok_or("device type id")?)?;
    let first = device(&db, a, kind_id).await?;
    let second = device(&db, a, kind_id).await?;
    let intake = http
        .call(
            Method::POST,
            "/api/v1/inventory/cooling-intakes",
            Some(json!({"name":"in","device_id":first,"diameter":10,"diameter_unit":"mm"})),
            StatusCode::CREATED,
        )
        .await?;
    let intake_path = format!(
        "/api/v1/inventory/cooling-intakes/{}",
        intake["id"].as_str().ok_or("intake id")?
    );
    let outflow = http
        .call(
            Method::POST,
            "/api/v1/inventory/cooling-outflows",
            Some(json!({"name":"out","device_id":first,"cooling_intake_id":intake["id"]})),
            StatusCode::CREATED,
        )
        .await?;
    let outflow_path = format!(
        "/api/v1/inventory/cooling-outflows/{}",
        outflow["id"].as_str().ok_or("outflow id")?
    );
    http.call(
        Method::PUT,
        &intake_path,
        Some(json!({"cooling_outflow_id":outflow["id"]})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(
        Method::PUT,
        &intake_path,
        Some(json!({"device_id":second})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/cooling-outflows",
        Some(json!({"name":"cross","device_id":second,"cooling_intake_id":intake["id"]})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let counts = http
        .call(
            Method::GET,
            &format!("/api/v1/inventory/devices/{first}"),
            None,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(counts["cooling_intake_count"], 1);
    assert_eq!(counts["cooling_outflow_count"], 1);
    http.call(Method::DELETE, &intake_path, None, StatusCode::NO_CONTENT)
        .await?;
    let detached = http
        .call(Method::GET, &outflow_path, None, StatusCode::OK)
        .await?;
    assert_eq!(detached["cooling_intake_id"], Value::Null);
    http.call(Method::DELETE, &outflow_path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(Method::DELETE, &feed_path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(Method::DELETE, &source_path, None, StatusCode::NO_CONTENT)
        .await?;
    sqlx::query("DELETE FROM dcim_device WHERE id=ANY($1)")
        .bind(vec![first, second])
        .execute(&db)
        .await?;
    http.call(
        Method::DELETE,
        &format!(
            "/api/v1/inventory/racks/{}",
            rack["id"].as_str().ok_or("rack id")?
        ),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=ANY($1)")
        .bind(vec![a, b])
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(tag)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; atomic hardware component instantiation"]
async fn cooling_templates_instantiate_device_ports_with_parent_mapping_and_counts() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .fetch_one(&db)
            .await?;
    let kind=http.call(Method::POST,"/api/v1/inventory/device_types",Some(json!({"manufacturer_id":manufacturer,"model":format!("cooling-{}",Uuid::now_v7()),"slug":format!("cooling-{}",Uuid::now_v7()),"cooling_method":"liquid","end_of_life":"2035-12-31"})),StatusCode::CREATED).await?;
    let kind_id = Uuid::parse_str(kind["id"].as_str().ok_or("device type id")?)?;
    let intake=http.call(Method::POST,"/api/v1/inventory/cooling-intake-templates",Some(json!({"name":"in","device_type_id":kind_id,"type":"uqd","max_flow":12.5,"max_flow_unit":"lpm"})),StatusCode::CREATED).await?;
    let outflow = http
        .call(
            Method::POST,
            "/api/v1/inventory/cooling-outflow-templates",
            Some(json!({"name":"out","device_type_id":kind_id,"cooling_intake_id":intake["id"]})),
            StatusCode::CREATED,
        )
        .await?;
    let category = http
        .call(
            Method::POST,
            "/api/v1/inventory/module-bay-types",
            Some(json!({"name":format!("cooling-category-{kind_id}")})),
            StatusCode::CREATED,
        )
        .await?;
    let bay_template=http.call(Method::POST,"/api/v1/inventory/module-bay-templates",Some(json!({"name":"slot","label":"Module slot","position":"A","device_type_id":kind_id,"module_bay_types":[category["id"]]})),StatusCode::CREATED).await?;
    let module_type=http.call(Method::POST,"/api/v1/inventory/module-types",Some(json!({"manufacturer_id":manufacturer,"model":format!("cooling-module-{kind_id}"),"module_bay_types":[category["id"]],"cooling_method":"hybrid","end_of_life":"2036-01-01"})),StatusCode::CREATED).await?;
    let module_intake = http
        .call(
            Method::POST,
            "/api/v1/inventory/cooling-intake-templates",
            Some(json!({"name":"in-{module}","module_type_id":module_type["id"]})),
            StatusCode::CREATED,
        )
        .await?;
    let module_outflow=http.call(Method::POST,"/api/v1/inventory/cooling-outflow-templates",Some(json!({"name":"out-{module}","module_type_id":module_type["id"],"cooling_intake_id":module_intake["id"]})),StatusCode::CREATED).await?;
    let child_template=http.call(Method::POST,"/api/v1/inventory/module-bay-templates",Some(json!({"name":"child-{module}","position":"{module}-1","module_type_id":module_type["id"],"module_bay_types":[category["id"]]})),StatusCode::CREATED).await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/cooling-intake-templates",
        Some(json!({"name":"invalid"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let site_id = site(&db).await?;
    let device_id = device(&db, site_id, kind_id).await?;
    let root: Uuid = sqlx::query_scalar(
        "SELECT id FROM dcim_modulebay WHERE device_id=$1 AND module_id IS NULL",
    )
    .bind(device_id)
    .fetch_one(&db)
    .await?;
    let cloned_categories: Vec<Uuid> = sqlx::query_scalar(
        "SELECT modulebaytype_id FROM dcim_modulebay_module_bay_types WHERE modulebay_id=$1",
    )
    .bind(root)
    .fetch_all(&db)
    .await?;
    assert_eq!(
        cloned_categories,
        vec![Uuid::parse_str(
            category["id"].as_str().ok_or("category id")?
        )?]
    );
    let port_parent: Option<Uuid> =
        sqlx::query_scalar("SELECT cooling_intake_id FROM dcim_coolingoutflow WHERE device_id=$1")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    let port_id: Uuid = sqlx::query_scalar("SELECT id FROM dcim_coolingintake WHERE device_id=$1")
        .bind(device_id)
        .fetch_one(&db)
        .await?;
    assert_eq!(port_parent, Some(port_id));
    let installed=http.call(Method::POST,"/api/v1/inventory/modules",Some(json!({"device_id":device_id,"module_bay_id":root,"module_type_id":module_type["id"]})),StatusCode::CREATED).await?;
    let module_id = Uuid::parse_str(installed["id"].as_str().ok_or("module id")?)?;
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM dcim_coolingintake WHERE module_id=$1")
            .bind(module_id)
            .fetch_all(&db)
            .await?;
    assert_eq!(names, vec!["in-A".to_string()]);
    let nested: (Uuid, Option<Uuid>, String) =
        sqlx::query_as("SELECT id,parent_id,name FROM dcim_modulebay WHERE module_id=$1")
            .bind(module_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(nested.1, Some(root));
    assert_eq!(nested.2, "child-A");
    let kind_path = format!("/api/v1/inventory/device_types/{kind_id}");
    let projected = http
        .call(Method::GET, &kind_path, None, StatusCode::OK)
        .await?;
    assert_eq!(projected["cooling_intake_template_count"], 1);
    assert_eq!(projected["cooling_outflow_template_count"], 1);
    assert_eq!(projected["end_of_life"], "2035-12-31");
    let updated=http.call(Method::PUT,&kind_path,Some(json!({"manufacturer_id":manufacturer,"model":kind["model"],"slug":kind["slug"],"description":"changed"})),StatusCode::OK).await?;
    assert_eq!(updated["cooling_method"], "liquid");
    assert_eq!(updated["end_of_life"], "2035-12-31");
    http.call(
        Method::DELETE,
        &format!("/api/v1/inventory/module-bays/{}", nested.0),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    http.call(
        Method::DELETE,
        &format!("/api/v1/inventory/modules/{module_id}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    http.call(
        Method::DELETE,
        &format!("/api/v1/inventory/module-bays/{root}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    sqlx::query("DELETE FROM dcim_device WHERE id=$1")
        .bind(device_id)
        .execute(&db)
        .await?;
    for (path, item) in [
        ("cooling-outflow-templates", outflow),
        ("cooling-intake-templates", intake),
    ] {
        http.call(
            Method::DELETE,
            &format!(
                "/api/v1/inventory/{path}/{}",
                item["id"].as_str().ok_or("template id")?
            ),
            None,
            StatusCode::NO_CONTENT,
        )
        .await?;
    }
    for (path, item) in [
        ("cooling-outflow-templates", module_outflow),
        ("cooling-intake-templates", module_intake),
        ("module-bay-templates", child_template),
        ("module-bay-templates", bay_template),
        ("module-types", module_type),
        ("module-bay-types", category),
    ] {
        http.call(
            Method::DELETE,
            &format!(
                "/api/v1/inventory/{path}/{}",
                item["id"].as_str().ok_or("template id")?
            ),
            None,
            StatusCode::NO_CONTENT,
        )
        .await?;
    }
    http.call(Method::DELETE, &kind_path, None, StatusCode::NO_CONTENT)
        .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; cooling PAT scope and atomic secret lifecycle"]
async fn cooling_permissions_and_atomic_custom_field_secret_intents() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let site_id = site(&db).await?;
    let types = http
        .call(
            Method::GET,
            "/api/v1/inventory/content-types",
            None,
            StatusCode::OK,
        )
        .await?;
    let content_type = types
        .as_array()
        .ok_or("content types")?
        .iter()
        .find(|item| item["token"] == "dcim.coolingsource")
        .ok_or("cooling source content type")?;
    let field_name = format!("cooling_secret_{}", Uuid::now_v7().simple());
    let definition=http.call(Method::POST,"/api/v1/custom-fields",Some(json!({"name":field_name,"label":"Cooling credential","field_type":"text","is_secret":true,"object_types":[content_type["id"]]})),StatusCode::CREATED).await?;
    let synthetic = format!("synthetic-{}", Uuid::now_v7());
    let source=http.call(Method::POST,"/api/v1/inventory/cooling-sources",Some(json!({"name":"secret-source","type":"chiller","site_id":site_id,"custom_fields":{&field_name:&synthetic}})),StatusCode::CREATED).await?;
    assert_eq!(source["custom_fields"][&field_name], "****");
    let id = Uuid::parse_str(source["id"].as_str().ok_or("source id")?)?;
    let path = format!("/api/v1/inventory/cooling-sources/{id}");
    let stored: Value =
        sqlx::query_scalar("SELECT custom_field_data FROM dcim_coolingsource WHERE id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    assert!(
        !stored.to_string().contains(&synthetic),
        "inventory JSON must not contain secret plaintext"
    );
    let before: Vec<Uuid> = sqlx::query_scalar(
        "SELECT secret_id FROM secret_usage_binding WHERE consumer_id=$1 ORDER BY secret_id",
    )
    .bind(id)
    .fetch_all(&db)
    .await?;
    assert_eq!(before.len(), 1);
    http.call(Method::PUT,&path,Some(json!({"status":"invalid","custom_fields":{&field_name:format!("rejected-{}",Uuid::now_v7())}})),StatusCode::BAD_REQUEST).await?;
    let after: Vec<Uuid> = sqlx::query_scalar(
        "SELECT secret_id FROM secret_usage_binding WHERE consumer_id=$1 ORDER BY secret_id",
    )
    .bind(id)
    .fetch_all(&db)
    .await?;
    assert_eq!(
        after, before,
        "failed metadata change must not alter secret bindings"
    );
    let revealed = http
        .call(
            Method::POST,
            &format!("{path}/custom-fields/reveal"),
            None,
            StatusCode::OK,
        )
        .await?;
    assert!(
        revealed["custom_fields"][&field_name].as_str() == Some(synthetic.as_str()),
        "authorized reveal must preserve the synthetic secret after rollback"
    );
    let token=http.call(Method::POST,"/api/v1/user/settings/api-tokens",Some(json!({"name":format!("cooling-{id}"),"scopes":[{"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{id}")}]})),StatusCode::CREATED).await?;
    let bearer = token["token_value"].as_str().ok_or("token value")?;
    for (target, status) in [
        (path.clone(), StatusCode::OK),
        (
            "/api/v1/inventory/cooling-sources".into(),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/api/v1/inventory/cooling-sources/{}", Uuid::now_v7()),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let request = Request::builder()
            .uri(&target)
            .header("authorization", format!("Bearer {bearer}"))
            .body(Body::empty())?;
        let response = http.app.clone().oneshot(request).await?;
        assert_eq!(response.status(), status, "cooling PAT scope");
    }
    http.call(Method::DELETE, &path, None, StatusCode::NO_CONTENT)
        .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    assert_eq!(remaining, 0);
    http.call(
        Method::DELETE,
        &format!(
            "/api/v1/custom-fields/{}",
            definition["id"].as_str().ok_or("definition id")?
        ),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; nullable cooling hardware fields"]
async fn rack_and_module_type_cooling_fields_preserve_omission_and_clear_explicit_null()
-> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let site_id = site(&db).await?;
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .fetch_one(&db)
            .await?;
    let rack=http.call(Method::POST,"/api/v1/inventory/racks",Some(json!({"name":format!("hardware-{site_id}"),"site_id":site_id,"width":19,"u_height":42,"cooling_capability":"hybrid","cooling_capacity":123.45})),StatusCode::CREATED).await?;
    let rack_type=http.call(Method::POST,"/api/v1/inventory/rack_types",Some(json!({"manufacturer_id":manufacturer,"model":format!("hardware-{site_id}"),"width":19,"u_height":42,"cooling_capability":"liquid-only","cooling_capacity":200.25})),StatusCode::CREATED).await?;
    for (collection, item) in [("racks", rack), ("rack_types", rack_type)] {
        let path = format!(
            "/api/v1/inventory/{collection}/{}",
            item["id"].as_str().ok_or("rack id")?
        );
        let preserved = http
            .call(Method::PUT, &path, Some(json!({})), StatusCode::OK)
            .await?;
        assert_eq!(preserved["cooling_capability"], item["cooling_capability"]);
        assert_eq!(preserved["cooling_capacity"], item["cooling_capacity"]);
        http.call(
            Method::PUT,
            &path,
            Some(json!({"cooling_capacity":-1})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let cleared = http
            .call(
                Method::PUT,
                &path,
                Some(json!({"cooling_capability":null,"cooling_capacity":null})),
                StatusCode::OK,
            )
            .await?;
        assert!(cleared["cooling_capability"].is_null());
        assert!(cleared["cooling_capacity"].is_null());
        http.call(Method::DELETE, &path, None, StatusCode::NO_CONTENT)
            .await?;
    }
    let module_type=http.call(Method::POST,"/api/v1/inventory/module-types",Some(json!({"manufacturer_id":manufacturer,"model":format!("hardware-module-{site_id}"),"cooling_method":"hybrid","end_of_life":"2036-01-01"})),StatusCode::CREATED).await?;
    let path = format!(
        "/api/v1/inventory/module-types/{}",
        module_type["id"].as_str().ok_or("type id")?
    );
    let preserved = http
        .call(
            Method::PUT,
            &path,
            Some(json!({"description":"updated"})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(preserved["cooling_method"], "hybrid");
    assert_eq!(preserved["end_of_life"], "2036-01-01");
    let cleared = http
        .call(
            Method::PUT,
            &path,
            Some(json!({"cooling_method":null,"end_of_life":null})),
            StatusCode::OK,
        )
        .await?;
    assert!(cleared["cooling_method"].is_null());
    assert!(cleared["end_of_life"].is_null());
    http.call(Method::DELETE, &path, None, StatusCode::NO_CONTENT)
        .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native Rack create defaults and partial-success bulk"]
async fn rack_create_and_bulk_defaults_match_storage_and_get() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let site_id = site(&db).await?;
    let collection = "/api/v1/inventory/racks";
    let minimal = http
        .call(
            Method::POST,
            collection,
            Some(json!({"name":"minimal-single","site_id":site_id})),
            StatusCode::CREATED,
        )
        .await?;
    let explicit = http
        .call(
            Method::POST,
            collection,
            Some(json!({"name":"explicit-single","site_id":site_id,"serial":"single-serial","width":23,"u_height":48,"cooling_capacity":12.34})),
            StatusCode::CREATED,
        )
        .await?;
    // A malformed cooling number is rejected while decoding the whole request,
    // before the repository's per-item savepoints can run.
    http.call(
        Method::POST,
        &format!("{collection}/bulk"),
        Some(json!({"items":[
            {"name":"rejected-envelope-valid","site_id":site_id},
            {"name":"rejected-envelope-cooling","site_id":site_id,"cooling_capacity":-1}
        ]})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let bulk = http
        .call(
            Method::POST,
            &format!("{collection}/bulk"),
            Some(json!({"items":[
                {"name":"minimal-before","site_id":site_id},
                {"name":"invalid-cooling","site_id":site_id,"cooling_capability":"invalid"},
                {"name":"invalid-width","site_id":site_id,"width":-1},
                {"name":"invalid-site","site_id":Uuid::now_v7()},
                {"name":"explicit-bulk","site_id":site_id,"serial":"bulk-serial","width":23,"u_height":48,"cooling_capacity":23.45},
                {"name":"minimal-after","site_id":site_id}
            ]})),
            StatusCode::OK,
        )
        .await?;
    let created = bulk["created"].as_array().ok_or("created racks")?;
    assert_eq!(created.len(), 3, "valid rows around failures must commit");
    assert_eq!(bulk["errors"].as_array().ok_or("bulk errors")?.len(), 3);
    assert_eq!(created[0]["name"], "minimal-before");
    assert_eq!(created[1]["name"], "explicit-bulk");
    assert_eq!(created[2]["name"], "minimal-after");
    for (item, serial, width, u_height) in [
        (&minimal, "", 19, 42),
        (&explicit, "single-serial", 23, 48),
        (&created[0], "", 19, 42),
        (&created[1], "bulk-serial", 23, 48),
        (&created[2], "", 19, 42),
    ] {
        assert_eq!(item["serial"], serial);
        assert_eq!(item["width"], width);
        assert_eq!(item["u_height"], u_height);
        let id = Uuid::parse_str(item["id"].as_str().ok_or("rack id")?)?;
        let stored: (String, i32, i32) = sqlx::query_as(
            "SELECT serial,width::integer,u_height::integer FROM dcim_rack WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&db)
        .await?;
        assert_eq!(stored, (serial.to_string(), width, u_height));
        let path = format!("{collection}/{id}");
        let fetched = http.call(Method::GET, &path, None, StatusCode::OK).await?;
        assert_eq!(
            &fetched, item,
            "create response must reflect the stored row"
        );
        http.call(Method::DELETE, &path, None, StatusCode::NO_CONTENT)
            .await?;
    }
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dcim_rack WHERE site_id=$1")
        .bind(site_id)
        .fetch_one(&db)
        .await?;
    assert_eq!(remaining, 0, "rejected rows must not persist");
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .execute(&db)
        .await?;
    Ok(())
}
