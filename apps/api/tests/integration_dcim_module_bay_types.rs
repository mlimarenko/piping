//! Native module bay compatibility and metadata behavior.
#[path = "common/child_database.rs"]
mod child_database;
mod common;

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie, unique_suffix,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const TYPES: &str = "/api/v1/inventory/module-bay-types";

async fn seed_tag(db: &sqlx::PgPool, slug: &str) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_tag (id,name,slug) VALUES ($1,$2,$2)")
        .bind(id)
        .bind(slug)
        .execute(db)
        .await?;
    Ok(id)
}

async fn seed_device(db: &sqlx::PgPool) -> TestResult<(Uuid, Uuid)> {
    let site = Uuid::now_v7();
    let device = Uuid::now_v7();
    let name = format!("pwa78-{}", unique_suffix());
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(&name).execute(db).await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(device).bind(&name).bind(site).execute(db).await?;
    Ok((site, device))
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; transactional native graph and concurrent installs"]
async fn module_bay_memberships_graph_and_installation_invariants() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let (site, device) = seed_device(&db).await?;
    let suffix = unique_suffix();
    let a = http
        .call(
            Method::POST,
            TYPES,
            Some(json!({"name":format!("cat-a-{suffix}")})),
            StatusCode::CREATED,
        )
        .await?;
    let b = http
        .call(
            Method::POST,
            TYPES,
            Some(json!({"name":format!("cat-b-{suffix}")})),
            StatusCode::CREATED,
        )
        .await?;
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .fetch_one(&db)
            .await?;
    let mt=http.call(Method::POST,"/api/v1/inventory/module-types",Some(json!({"manufacturer_id":manufacturer,"model":format!("model-{suffix}"),"module_bay_types":[b["id"]]})),StatusCode::CREATED).await?;
    let mt_path = format!(
        "/api/v1/inventory/module-types/{}",
        mt["id"].as_str().ok_or("type id")?
    );
    let bay=http.call(Method::POST,"/api/v1/inventory/module-bays",Some(json!({"device_id":device,"name":"root","label":"preserve","module_bay_types":[a["id"]]})),StatusCode::CREATED).await?;
    let bay_path = format!(
        "/api/v1/inventory/module-bays/{}",
        bay["id"].as_str().ok_or("bay id")?
    );
    assert_eq!(bay["enabled"], true);
    assert_eq!(bay["occupied"], false);
    let install = json!({"device_id":device,"module_bay_id":bay["id"],"module_type_id":mt["id"]});
    http.call(
        Method::POST,
        "/api/v1/inventory/modules",
        Some(install.clone()),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(
        Method::PUT,
        &mt_path,
        Some(json!({"module_bay_types":[a["id"]]})),
        StatusCode::OK,
    )
    .await?;
    // Force an install to begin while a category replacement owns the type lock.
    // It must re-read the committed category set after waiting, not install using
    // the old compatible set from its statement's initial snapshot.
    let mut category_change = db.begin().await?;
    let type_id = Uuid::parse_str(mt["id"].as_str().ok_or("type id")?)?;
    sqlx::query("SELECT id FROM dcim_moduletype WHERE id=$1 FOR UPDATE")
        .bind(type_id)
        .execute(&mut *category_change)
        .await?;
    let locker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *category_change)
        .await?;
    sqlx::query("DELETE FROM dcim_moduletype_module_bay_types WHERE moduletype_id=$1")
        .bind(type_id)
        .execute(&mut *category_change)
        .await?;
    sqlx::query("INSERT INTO dcim_moduletype_module_bay_types (moduletype_id,modulebaytype_id) VALUES ($1,$2)").bind(type_id).bind(Uuid::parse_str(b["id"].as_str().ok_or("category id")?)?).execute(&mut *category_change).await?;
    let pending_request = http
        .client
        .post(format!("{}/api/v1/inventory/modules", http.base))
        .header("cookie", &http.cookie)
        .json(&install);
    let pending = tokio::spawn(async move { pending_request.send().await });
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))").bind(locker).fetch_one(&db).await?;
            if waiting { break; }
            tokio::task::yield_now().await;
        }
        Ok::<(),sqlx::Error>(())
    }).await??;
    category_change.commit().await?;
    let stale_install =
        tokio::time::timeout(std::time::Duration::from_secs(10), pending).await???;
    assert_eq!(
        stale_install.status(),
        StatusCode::BAD_REQUEST,
        "an install must not use category membership read before its owner lock"
    );
    http.call(
        Method::PUT,
        &mt_path,
        Some(json!({"module_bay_types":[a["id"]]})),
        StatusCode::OK,
    )
    .await?;
    // The unique occupant constraint and device lock arbitrate concurrent installs.
    let request = || {
        http.client
            .post(format!("{}/api/v1/inventory/modules", http.base))
            .header("cookie", &http.cookie)
            .json(&install)
            .send()
    };
    let (left, right) = tokio::join!(request(), request());
    let left = left?;
    let right = right?;
    let statuses = [left.status(), right.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::CREATED)
            .count(),
        1,
        "concurrent occupant outcomes {statuses:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::CONFLICT)
            .count(),
        1,
        "concurrent occupant outcomes {statuses:?}"
    );
    let installed: Value = if left.status() == StatusCode::CREATED {
        left.json().await?
    } else {
        right.json().await?
    };
    let module_path = format!(
        "/api/v1/inventory/modules/{}",
        installed["id"].as_str().ok_or("module id")?
    );
    let occupied = http
        .call(Method::GET, &bay_path, None, StatusCode::OK)
        .await?;
    assert_eq!(
        occupied["module_id"],
        Value::Null,
        "root bay has no containing module"
    );
    assert_eq!(occupied["occupied"], true);
    assert_eq!(occupied["installed_module"]["id"], installed["id"]);
    assert_eq!(occupied["is_module_compatible"], true);
    http.call(
        Method::PUT,
        &bay_path,
        Some(json!({"enabled":false})),
        StatusCode::OK,
    )
    .await?;
    http.call(
        Method::PUT,
        &module_path,
        Some(json!({"description":"existing occupant edit"})),
        StatusCode::OK,
    )
    .await?;
    http.call(
        Method::PUT,
        &mt_path,
        Some(json!({"module_bay_types":[b["id"]]})),
        StatusCode::OK,
    )
    .await?;
    let incompatible = http
        .call(Method::GET, &module_path, None, StatusCode::OK)
        .await?;
    assert_eq!(incompatible["is_bay_compatible"], false);
    let incompatible = http
        .call(Method::GET, &bay_path, None, StatusCode::OK)
        .await?;
    assert_eq!(incompatible["is_module_compatible"], false);
    for edges in [
        json!(null),
        json!([a["id"], a["id"]]),
        json!([Uuid::now_v7()]),
    ] {
        http.call(
            Method::PUT,
            &bay_path,
            Some(json!({"label":"must roll back","module_bay_types":edges})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let unchanged = http
            .call(Method::GET, &bay_path, None, StatusCode::OK)
            .await?;
        assert_eq!(unchanged["label"], "preserve");
        assert_eq!(unchanged["module_bay_types"], json!([a["id"]]));
    }
    let child = http
        .call(
            Method::POST,
            "/api/v1/inventory/module-bays",
            Some(json!({"device_id":device,"module_id":installed["id"],"name":"child"})),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(child["parent_id"], bay["id"]);
    assert_eq!(child["module_id"], installed["id"]);
    let cycle = sqlx::query("UPDATE dcim_modulebay SET module_id=$1 WHERE id=$2")
        .bind(Uuid::parse_str(
            installed["id"].as_str().ok_or("module id")?,
        )?)
        .bind(Uuid::parse_str(bay["id"].as_str().ok_or("bay id")?)?)
        .execute(&db)
        .await;
    assert!(
        cycle
            .as_ref()
            .err()
            .and_then(sqlx::Error::as_database_error)
            .is_some_and(|error| error.code().as_deref() == Some("23514")),
        "cycles must be rejected by the native graph owner"
    );
    let foreign_device = seed_device(&db).await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/module-bays",
        Some(json!({"device_id":foreign_device.1,"module_id":installed["id"],"name":"foreign"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/module-bays",
        Some(json!({"device_id":device,"name":"root"})),
        StatusCode::CONFLICT,
    )
    .await?;
    let blocked = http
        .call(
            Method::DELETE,
            &format!("{TYPES}/{}", a["id"].as_str().ok_or("category id")?),
            None,
            StatusCode::CONFLICT,
        )
        .await?;
    assert!(
        blocked["details"]["total_count"]
            .as_i64()
            .is_some_and(|v| v > 0),
        "referenced categories require structured deletion blockers"
    );
    assert!(
        blocked["details"]["blockers"]
            .as_array()
            .ok_or("blockers")?
            .iter()
            .flat_map(|group| group["examples"].as_array().into_iter().flatten())
            .any(|example| example["label"]
                .as_str()
                .is_some_and(|label| label.contains("root"))),
        "membership blockers must name their owning bay"
    );
    assert!(
        blocked["details"]["blockers"]
            .as_array()
            .ok_or("blockers")?
            .iter()
            .any(|group| group["table"] == "dcim_modulebay"
                && group["examples"].as_array().is_some_and(|examples| examples
                    .iter()
                    .any(|example| example["id"] == bay["id"] && example["label"] == "root"))),
        "the blocker identity and label must belong to the bay"
    );

    // Retain delete permission on the category but remove READ on the bay.
    let saved: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(&db)
            .await?;
    let mut redis = common::build_test_redis().await?;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(json!([format!(
            "inventory:{}",
            a["id"].as_str().ok_or("category id")?
        )]))
        .execute(&db)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let hidden = http
        .call(
            Method::DELETE,
            &format!("{TYPES}/{}", a["id"].as_str().ok_or("category id")?),
            None,
            StatusCode::CONFLICT,
        )
        .await;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(saved)
        .execute(&db)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let hidden = hidden?;
    assert_eq!(hidden["reason_code"], "DELETE_BLOCKED");
    assert!(
        hidden.get("details").is_none_or(Value::is_null),
        "unreadable owner details must be withheld"
    );
    let bulk=http.call(Method::POST,"/api/v1/inventory/module-bays/bulk",Some(json!({"items":[{"device_id":device,"name":"rollback","module_bay_types":[Uuid::now_v7()]},{"device_id":device,"name":"survives","enabled":false}]})),StatusCode::OK).await?;
    assert_eq!(bulk["created"].as_array().ok_or("created")?.len(), 1);
    assert_eq!(bulk["errors"].as_array().ok_or("errors")?.len(), 1);
    let rolled_back: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dcim_modulebay WHERE device_id=$1 AND name='rollback'",
    )
    .bind(device)
    .fetch_one(&db)
    .await?;
    assert_eq!(
        rolled_back, 0,
        "failed edge insert must roll back its scalar bay"
    );
    http.call(
        Method::PUT,
        &mt_path,
        Some(json!({"module_bay_types":[]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        http.call(Method::GET, &module_path, None, StatusCode::OK)
            .await?["is_bay_compatible"],
        true
    );
    http.call(
        Method::DELETE,
        &format!(
            "/api/v1/inventory/module-bays/{}",
            child["id"].as_str().ok_or("child id")?
        ),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    http.call(Method::DELETE, &module_path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/modules",
        Some(install),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(Method::DELETE, &bay_path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(
        Method::DELETE,
        &format!(
            "/api/v1/inventory/module-bays/{}",
            bulk["created"][0]["id"].as_str().ok_or("bulk bay")?
        ),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    http.call(Method::DELETE, &mt_path, None, StatusCode::NO_CONTENT)
        .await?;
    for category in [a, b] {
        http.call(
            Method::DELETE,
            &format!("{TYPES}/{}", category["id"].as_str().ok_or("category id")?),
            None,
            StatusCode::NO_CONTENT,
        )
        .await?;
    }
    for (site, device) in [(site, device), foreign_device] {
        sqlx::query("DELETE FROM dcim_device WHERE id=$1")
            .bind(device)
            .execute(&db)
            .await?;
        sqlx::query("DELETE FROM dcim_site WHERE id=$1")
            .bind(site)
            .execute(&db)
            .await?;
    }
    Ok(())
}

struct HttpFixture {
    client: Client,
    base: String,
    cookie: String,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl HttpFixture {
    async fn start() -> TestResult<Self> {
        let state = build_test_app_state().await?;
        let app = piping_backend::app::build_router(state);
        let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
            .await
            .ok_or("admin login failed")?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("HTTP fixture");
        });
        Ok(Self {
            client: Client::new(),
            base,
            cookie,
            server,
        })
    }
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        expected: StatusCode,
    ) -> TestResult<Value> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("cookie", &self.cookie);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        assert_eq!(
            status,
            expected,
            "{path}: {}",
            String::from_utf8_lossy(&bytes)
        );
        Ok(if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        })
    }
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; actual HTTP partial updates"]
async fn module_type_update_preserves_omitted_hardware_and_custom_data() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let manufacturer = Uuid::now_v7();
    let profile = Uuid::now_v7();
    let id = Uuid::now_v7();
    let suffix = unique_suffix();
    let tag = seed_tag(&db, &format!("keep-{suffix}")).await?;
    sqlx::query("INSERT INTO dcim_manufacturer (id,name,slug,description,comments,custom_field_data) VALUES ($1,$2,$2,'','','{}')")
        .bind(manufacturer).bind(&suffix).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_moduletypeprofile (id,name,description,comments,schema,custom_field_data) VALUES ($1,$2,'profile metadata','keep','{}','{}')")
        .bind(profile).bind(&suffix).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_moduletype (id,manufacturer_id,model,part_number,weight,weight_unit,airflow,description,comments,custom_field_data,attribute_data,module_count,tags,profile_id) VALUES ($1,$2,$3,'part',12.34,'kg','front-to-rear','description','comments','{\"region\":\"west\"}','{\"ports\":8}',0,$5,$4)")
        .bind(id).bind(manufacturer).bind(&suffix).bind(profile).bind(vec![tag.to_string()]).execute(&db).await?;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(t)-'model' FROM dcim_moduletype t WHERE id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    http.call(
        Method::PUT,
        &format!("/api/v1/inventory/module-types/{id}"),
        Some(json!({"manufacturer_id":manufacturer,"model":"renamed"})),
        StatusCode::OK,
    )
    .await?;
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(t)-'model' FROM dcim_moduletype t WHERE id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    assert_eq!(
        before, after,
        "a scalar update must preserve all omitted hardware, profile and custom metadata"
    );
    sqlx::query("DELETE FROM dcim_moduletype WHERE id=$1")
        .bind(id)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM dcim_manufacturer WHERE id=$1")
        .bind(manufacturer)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM dcim_moduletypeprofile WHERE id=$1")
        .bind(profile)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(tag)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native CRUD and bulk HTTP"]
async fn module_bay_type_crud_pagination_nullable_and_validation() -> TestResult {
    let http = HttpFixture::start().await?;
    let name = format!("pwa78-{}", unique_suffix());
    let db = build_test_db_pool().await?;
    let tag = seed_tag(&db, &format!("native-{name}")).await?;
    let item = http
        .call(
            Method::POST,
            TYPES,
            Some(json!({"name":name,"comments":"preserve","tags":[tag.to_string()],"color":"A1b2C3"})),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(item["manufacturer_id"], Value::Null);
    assert!(item["created"].is_string());
    let path = format!("{TYPES}/{}", item["id"].as_str().ok_or("id")?);
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .fetch_one(&db)
            .await?;
    let assigned = http
        .call(
            Method::PUT,
            &path,
            Some(json!({"manufacturer_id":manufacturer})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(assigned["manufacturer_id"], manufacturer.to_string());
    let updated = http
        .call(
            Method::PUT,
            &path,
            Some(json!({"description":"changed","manufacturer_id":null,"owner_id":null})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(updated["comments"], "preserve");
    assert_eq!(
        updated["manufacturer_id"],
        Value::Null,
        "explicit null must clear an assigned manufacturer"
    );
    assert_eq!(updated["tags"], json!([tag.to_string()]));
    assert_eq!(updated["created"], item["created"]);
    let page = http
        .call(
            Method::GET,
            &format!("{TYPES}?search={name}&limit=1&offset=0"),
            None,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], item["id"]);
    let empty = http
        .call(
            Method::GET,
            &format!("{TYPES}?search={name}&limit=1&offset=9"),
            None,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(empty["total"], 1);
    assert_eq!(empty["items"], json!([]));
    for payload in [
        json!({"name":""}),
        json!({"name":"x","color":"#ffffff"}),
        json!({"name":"x","slug":"not a slug"}),
        json!({"name":"x","description":"x".repeat(201)}),
    ] {
        http.call(Method::POST, TYPES, Some(payload), StatusCode::BAD_REQUEST)
            .await?;
    }
    let bulk = http.call(Method::POST, &format!("{TYPES}/bulk"), Some(json!({"items":[{"name":name},{"name":format!("{name}-second")},{"name":"bad","manufacturer_id":Uuid::now_v7()}]})), StatusCode::OK).await?;
    assert_eq!(bulk["created"].as_array().ok_or("created")?.len(), 1);
    assert_eq!(bulk["errors"].as_array().ok_or("errors")?.len(), 2);
    http.call(
        Method::DELETE,
        &format!(
            "{TYPES}/{}",
            bulk["created"][0]["id"].as_str().ok_or("bulk id")?
        ),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    http.call(Method::DELETE, &path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(Method::GET, &path, None, StatusCode::NOT_FOUND)
        .await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(tag)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; scoped authorization and PAT narrowing"]
async fn module_bay_type_object_and_collection_authorization() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let a = http
        .call(
            Method::POST,
            TYPES,
            Some(json!({"name":format!("allowed-{}",unique_suffix())})),
            StatusCode::CREATED,
        )
        .await?;
    let b = http
        .call(
            Method::POST,
            TYPES,
            Some(json!({"name":format!("denied-{}",unique_suffix())})),
            StatusCode::CREATED,
        )
        .await?;
    let a_path = format!("{TYPES}/{}", a["id"].as_str().ok_or("id")?);
    let b_path = format!("{TYPES}/{}", b["id"].as_str().ok_or("id")?);
    // Never include token response bytes in an assertion or log.
    let token_response=http.client.post(format!("{}/api/v1/user/settings/api-tokens",http.base)).header("cookie",&http.cookie)
        .json(&json!({"name":format!("pwa78-{}",unique_suffix()),"scopes":[{"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{}",a["id"].as_str().ok_or("id")?)}]})).send().await?;
    assert_eq!(token_response.status(), StatusCode::CREATED);
    let token: Value = token_response.json().await?;
    let bearer = token["token_value"].as_str().ok_or("token value")?;
    for (path, expected) in [
        (&a_path, StatusCode::OK),
        (&b_path, StatusCode::FORBIDDEN),
        (&TYPES.to_string(), StatusCode::FORBIDDEN),
    ] {
        let response = http
            .client
            .get(format!("{}{path}", http.base))
            .bearer_auth(bearer)
            .send()
            .await?;
        assert_eq!(response.status(), expected, "PAT narrowing {path}");
    }
    let previous: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(&db)
            .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(json!([format!(
            "inventory:{}",
            a["id"].as_str().ok_or("id")?
        )]))
        .execute(&db)
        .await?;
    let mut redis = common::build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let result: TestResult = async {
        http.call(Method::GET, &a_path, None, StatusCode::OK)
            .await?;
        http.call(
            Method::PUT,
            &a_path,
            Some(json!({"comments":"allowed"})),
            StatusCode::OK,
        )
        .await?;
        for (method, path, body) in [
            (Method::GET, b_path.clone(), None),
            (
                Method::PUT,
                b_path.clone(),
                Some(json!({"comments":"denied"})),
            ),
            (Method::DELETE, b_path.clone(), None),
            (Method::POST, format!("{b_path}/custom-fields/reveal"), None),
            (Method::GET, TYPES.to_string(), None),
            (
                Method::POST,
                TYPES.to_string(),
                Some(json!({"name":"denied"})),
            ),
            (
                Method::POST,
                format!("{TYPES}/bulk"),
                Some(json!({"items":[{"name":"denied"}]})),
            ),
        ] {
            http.call(method, &path, body, StatusCode::FORBIDDEN)
                .await?;
        }
        Ok(())
    }
    .await;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(previous)
        .execute(&db)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    result?;
    http.call(Method::DELETE, &a_path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(Method::DELETE, &b_path, None, StatusCode::NO_CONTENT)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; atomic custom-field secret lifecycle"]
async fn module_bay_type_secret_mask_reveal_reject_and_delete() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
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
        .find(|v| v["token"] == "dcim.modulebaytype")
        .ok_or("module bay type capability")?;
    let field_name = format!("pwa78_secret_{}", Uuid::now_v7().simple());
    let definition=http.call(Method::POST,"/api/v1/custom-fields",Some(json!({"name":field_name,"label":"Module bay credential","field_type":"text","is_secret":true,"object_types":[content_type["id"]]})),StatusCode::CREATED).await?;
    let cleartext = format!("synthetic-{}", Uuid::now_v7());
    let name = format!("secret-{}", unique_suffix());
    let item = http
        .call(
            Method::POST,
            TYPES,
            Some(json!({"name":name,"custom_fields":{&field_name:&cleartext}})),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(item["custom_fields"][&field_name], "****");
    let id = Uuid::parse_str(item["id"].as_str().ok_or("id")?)?;
    let path = format!("{TYPES}/{id}");
    let stored: Value =
        sqlx::query_scalar("SELECT custom_field_data FROM dcim_modulebaytype WHERE id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    assert!(
        !stored.to_string().contains(&cleartext),
        "plaintext must not be stored in inventory JSON"
    );
    let response = http
        .client
        .post(format!("{}{path}/custom-fields/reveal", http.base))
        .header("cookie", &http.cookie)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let revealed: Value = response.json().await?;
    assert!(
        revealed["custom_fields"][&field_name].as_str() == Some(cleartext.as_str()),
        "authorized reveal must return the stored synthetic value"
    );
    let bindings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    assert_eq!(bindings, 1);
    // A rejected scalar mutation cannot create or rotate secret bindings.
    http.call(Method::POST,TYPES,Some(json!({"name":name,"custom_fields":{&field_name:format!("rejected-{}",Uuid::now_v7())}})),StatusCode::CONFLICT).await?;
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    assert_eq!(after, bindings);
    http.call(Method::DELETE, &path, None, StatusCode::NO_CONTENT)
        .await?;
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM secret_usage_binding WHERE consumer_id=$1")
            .bind(id)
            .fetch_one(&db)
            .await?;
    assert_eq!(
        after, 0,
        "deletion must detach the shared secret consumer atomically"
    );
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
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit owned PostgreSQL with CREATEDB; genuine SQLx fresh and staged migration ledgers"]
async fn fresh_upgrade_and_invalid_data_migration_are_atomic() -> TestResult {
    use sqlx::migrate::Migrator;
    use std::{borrow::Cow, path::PathBuf};
    let all = Migrator::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations")).await?;
    let prefix = Migrator {
        migrations: Cow::Owned(all.iter().filter(|m| m.version <= 4).cloned().collect()),
        ..Migrator::DEFAULT
    };
    std::env::var("PIPING_DATABASE__URL")?;
    let mut children = Vec::new();
    let mut pools = Vec::new();
    let proof: TestResult = async {
        for _ in 0..3 {
            let (child, url) = child_database::ChildDatabase::create("pwa78_migration_").await?;
            children.push(child);
            pools.push(sqlx::PgPool::connect(&url).await?);
        }
        let fresh = pools[0].clone();
        let staged = pools[1].clone();
        let invalid = pools[2].clone();
    // A spawned task lets the fixture owner clean up its databases even on assertion failure.
    tokio::spawn(async move {
        all.run(&fresh).await?;
        prefix.run(&staged).await?;
        prefix.run(&invalid).await?;
        let (_,device)=seed_device(&staged).await?;
        let id=Uuid::now_v7();
        let tag=seed_tag(&staged,"keep").await?;
        sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id,tags,external_id) VALUES ($1,'root','label','A','preserve',$2,'{\"native\":\"metadata\"}',0,1,2,1,ARRAY['keep'],78)")
            .bind(id).bind(device).execute(&staged).await?;
        sqlx::query("INSERT INTO dcim_modulebaytype (name,slug,color,description,comments,custom_field_data,tags,external_id) VALUES ('upgrade-metadata','upgrade-metadata','','description','comments','{\"type\":\"metadata\"}',ARRAY['keep'],78)").execute(&staged).await?;
        let before_bay: Value=sqlx::query_scalar("SELECT to_jsonb(b)-'tags' FROM dcim_modulebay b WHERE id=$1").bind(id).fetch_one(&staged).await?;
        let before_types: Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t)-'tags' ORDER BY id) FROM dcim_modulebaytype t").fetch_one(&staged).await?;
        let history_before: Vec<(i64,Vec<u8>)>=sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version").fetch_all(&staged).await?;
        assert_eq!(history_before.len(),4);
        all.run(&staged).await?;
        let after_bay: Value=sqlx::query_scalar("SELECT to_jsonb(b)-'enabled'-'tags' FROM dcim_modulebay b WHERE id=$1").bind(id).fetch_one(&staged).await?;
        let after_types: Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t)-'tags' ORDER BY id) FROM dcim_modulebaytype t").fetch_one(&staged).await?;
        let bay_tags:Vec<String>=sqlx::query_scalar("SELECT tags FROM dcim_modulebay WHERE id=$1").bind(id).fetch_one(&staged).await?;
        let type_tags:Vec<String>=sqlx::query_scalar("SELECT tags FROM dcim_modulebaytype WHERE slug='upgrade-metadata'").fetch_one(&staged).await?;
        assert_eq!(bay_tags,vec![tag.to_string()],"legacy bay membership must resolve to its catalog identity");
        assert_eq!(type_tags,vec![tag.to_string()],"legacy bay type membership must resolve to its catalog identity");
        assert_eq!(before_bay,after_bay,"upgrade must not rebuild existing valid graph metadata");
        assert_eq!(before_types,after_types,"unknown native dates and metadata must remain unchanged");
        let history_after: Vec<(i64,Vec<u8>)>=sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version").fetch_all(&staged).await?;
        assert_eq!(history_after.len(),all.iter().count());
        assert_eq!(&history_after[..4],history_before.as_slice(),"immutable prefix ledger must not be rewritten");
        all.run(&staged).await?;
        let schema_query="SELECT table_name,column_name,data_type,is_nullable,column_default FROM information_schema.columns WHERE table_schema='public' AND table_name IN ('dcim_modulebay','dcim_modulebaytype','dcim_modulebaytemplate','dcim_moduletype_module_bay_types','dcim_modulebay_module_bay_types','dcim_modulebaytemplate_module_bay_types') ORDER BY table_name,ordinal_position";
        type Column=(String,String,String,String,Option<String>);
        let fresh_schema:Vec<Column>=sqlx::query_as(schema_query).fetch_all(&fresh).await?;
        let staged_schema:Vec<Column>=sqlx::query_as(schema_query).fetch_all(&staged).await?;
        assert_eq!(fresh_schema,staged_schema,"fresh and upgrade schemas must agree");
        let (_,device)=seed_device(&invalid).await?;
        for _ in 0..2 {
            sqlx::query("INSERT INTO dcim_modulebay (name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ('duplicate-root','','','',$1,'{}',0,1,2,1)").bind(device).execute(&invalid).await?;
        }
        assert!(all.run(&invalid).await.is_err(),"nullable-root duplicates must refuse the migration");
        let enabled_columns:i64=sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='dcim_modulebay' AND column_name='enabled'").fetch_one(&invalid).await?;
        assert_eq!(enabled_columns,0,"failed migration must roll back all DDL");
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM dcim_modulebay WHERE device_id=$1").bind(device).fetch_one(&invalid).await?;
        assert_eq!(count,2,"invalid rows must not be normalized or dropped");
        let versions:i64=sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations").fetch_one(&invalid).await?;
        assert_eq!(versions,4,"failed migration must not stamp its version");
        Ok::<(),Box<dyn std::error::Error+Send+Sync>>(())
    }).await??;
        Ok(())
    }
    .await;
    for pool in pools {
        pool.close().await;
    }
    let mut cleanup_error = None;
    for child in children {
        if let Err(error) = child.cleanup().await {
            eprintln!("child database cleanup failed: {error}");
            if cleanup_error.is_none() {
                cleanup_error = Some(error);
            }
        }
    }
    proof?;
    if let Some(error) = cleanup_error {
        return Err(error);
    }
    Ok(())
}
