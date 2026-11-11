//! Rack groups and cable bundles: native HTTP, authorization and persisted relations.
mod common;

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    build_test_redis, login_and_get_cookie, unique_suffix,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const GROUPS: &str = "/api/v1/inventory/rack-groups";
const BUNDLES: &str = "/api/v1/inventory/cable-bundles";

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis; native HTTP scoped RBAC"]
async fn http_object_grant_does_not_authorize_other_objects_or_collections() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let mut objects = Vec::new();
    for collection in [GROUPS, BUNDLES] {
        for side in ["allowed", "denied"] {
            let name = format!("pwa84-{side}-{}", unique_suffix());
            let created = http
                .call(
                    Method::POST,
                    collection,
                    Some(json!({"name":name})),
                    StatusCode::CREATED,
                )
                .await?;
            objects.push((
                collection,
                created["id"].as_str().ok_or("object id")?.to_string(),
            ));
        }
    }
    let resources = json!([
        format!("inventory:{}", objects[0].1),
        format!("inventory:{}", objects[2].1)
    ]);
    let previous: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(&db)
            .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(resources)
        .execute(&db)
        .await?;
    let mut redis = build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let result: TestResult = async {
        for pair in objects.chunks_exact(2) {
            let collection = pair[0].0;
            let allowed = format!("{collection}/{}", pair[0].1);
            let denied = format!("{collection}/{}", pair[1].1);
            http.call(Method::GET, &allowed, None, StatusCode::OK)
                .await?;
            http.call(
                Method::PUT,
                &allowed,
                Some(json!({"comments":"authorized"})),
                StatusCode::OK,
            )
            .await?;
            for (method, path, body) in [
                (Method::GET, denied.clone(), None),
                (
                    Method::PUT,
                    denied.clone(),
                    Some(json!({"comments":"forbidden"})),
                ),
                (Method::DELETE, denied.clone(), None),
                (Method::POST, format!("{denied}/custom-fields/reveal"), None),
                (Method::GET, collection.to_string(), None),
                (
                    Method::POST,
                    collection.to_string(),
                    Some(json!({"name":"forbidden"})),
                ),
                (
                    Method::POST,
                    format!("{collection}/bulk"),
                    Some(json!({"items":[{"name":"forbidden"}]})),
                ),
            ] {
                http.call(method, &path, body, StatusCode::FORBIDDEN)
                    .await?;
            }
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
    for (collection, id) in objects {
        http.call(
            Method::DELETE,
            &format!("{collection}/{id}"),
            None,
            StatusCode::NO_CONTENT,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; native repository JSON patch contract"]
async fn repository_custom_field_deltas_preserve_other_keys() -> TestResult {
    use piping_backend::contexts::inventory::infrastructure::persistence::{
        CableBundleRepository, NewCableBundle, NewRackGroup, RackGroupRepository,
        SqlxCableBundleRepository, SqlxRackGroupRepository, UpdateCableBundle, UpdateRackGroup,
    };
    let db = build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&db).await?;
    let groups = SqlxRackGroupRepository::new(db.clone());
    let bundles = SqlxCableBundleRepository::new(db);
    let name = format!("pwa84-patch-{}", unique_suffix());
    let group = groups
        .create(&NewRackGroup {
            id: Uuid::now_v7(),
            name: name.clone(),
            slug: name.clone(),
            description: None,
            comments: Some("preserve".into()),
            owner_id: None,
            tags: vec![],
            custom_fields: Some(json!({"a":"old","b":"old"})),
        })
        .await?;
    let bundle = bundles
        .create(&NewCableBundle {
            id: Uuid::now_v7(),
            name,
            description: None,
            comments: Some("preserve".into()),
            owner_id: None,
            tags: vec![],
            custom_fields: Some(json!({"a":"old","b":"old"})),
        })
        .await?;
    for (key, value) in [("a", "new-a"), ("b", "new-b")] {
        groups
            .update(
                group.id,
                &UpdateRackGroup {
                    custom_fields: Some(json!({key:value})),
                    ..Default::default()
                },
            )
            .await?;
        bundles
            .update(
                bundle.id,
                &UpdateCableBundle {
                    custom_fields: Some(json!({key:value})),
                    ..Default::default()
                },
            )
            .await?;
    }
    for fields in [
        groups
            .get_by_id(group.id)
            .await?
            .ok_or("group")?
            .custom_fields,
        bundles
            .get_by_id(bundle.id)
            .await?
            .ok_or("bundle")?
            .custom_fields,
    ] {
        assert_eq!(fields, json!({"a":"new-a","b":"new-b"}));
    }
    groups.delete(group.id).await?;
    bundles.delete(bundle.id).await?;
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
        // Restore only this suite's authorization fixtures after an interrupted test.
        let db = build_test_db_pool().await?;
        sqlx::query("DELETE FROM rbac_policy WHERE name LIKE 'pwa84-deny-%'")
            .execute(&db)
            .await?;
        sqlx::query("UPDATE rbac_policy SET resources='[]'::jsonb WHERE name='test-admin-policy'")
            .execute(&db)
            .await?;
        let mut redis = build_test_redis().await?;
        let _: i64 = redis::cmd("INCR")
            .arg("access_control:policies:version")
            .query_async(&mut redis)
            .await?;
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
#[ignore = "requires isolated PostgreSQL and Redis; native HTTP fixture"]
async fn http_crud_comments_uniqueness_bulk_and_validation() -> TestResult {
    let http = HttpFixture::start().await?;
    let suffix = unique_suffix();
    let content_types = http
        .call(
            Method::GET,
            "/api/v1/inventory/content-types",
            None,
            StatusCode::OK,
        )
        .await?;
    for collection in [GROUPS, BUNDLES] {
        http.call(
            Method::POST,
            collection,
            Some(json!({"name": "  "})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        http.call(
            Method::POST,
            collection,
            Some(json!({"name": "x".repeat(101)})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        if collection == GROUPS {
            http.call(
                Method::POST,
                collection,
                Some(json!({"name": "!!!"})),
                StatusCode::BAD_REQUEST,
            )
            .await?;
            http.call(
                Method::POST,
                collection,
                Some(json!({"name": "invalid slug", "slug": "bad slug"})),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
        let model = if collection == GROUPS {
            "rackgroup"
        } else {
            "cablebundle"
        };
        let token = format!("dcim.{model}");
        let advertised = content_types
            .as_array()
            .ok_or("content types array")?
            .iter()
            .find(|item| item["token"] == token)
            .ok_or("advertised custom-field scope")?;
        assert_eq!(advertised["supported_for_custom_fields"], true);
        let scope = Uuid::parse_str(advertised["id"].as_str().ok_or("scope id")?)?;
        let field_name = format!("pwa84_{model}_{suffix}");
        let field = http.call(Method::POST, "/api/v1/custom-fields", Some(json!({"name":field_name,"label":"Rack and cable notes","field_type":"text","object_types":[scope]})), StatusCode::CREATED).await?;
        let name = format!(
            "pwa84-{suffix}-{}",
            if collection == GROUPS {
                "group"
            } else {
                "bundle"
            }
        );
        let mut payload = json!({"name": name, "description": "initial", "comments": "first\n**markdown**", "custom_fields": {field_name: "keep"}});
        if collection == GROUPS {
            payload["slug"] = json!(name);
        }
        let created = http
            .call(
                Method::POST,
                collection,
                Some(payload.clone()),
                StatusCode::CREATED,
            )
            .await?;
        assert_eq!(created["comments"], payload["comments"]);
        assert_eq!(created["custom_fields"], payload["custom_fields"]);
        let path = format!(
            "{collection}/{}",
            created["id"].as_str().ok_or("missing id")?
        );
        let fetched = http.call(Method::GET, &path, None, StatusCode::OK).await?;
        assert_eq!(fetched["comments"], payload["comments"]);
        let revealed = http
            .call(
                Method::POST,
                &format!("{path}/custom-fields/reveal"),
                None,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(revealed["custom_fields"], created["custom_fields"]);
        http.call(
            Method::POST,
            collection,
            Some(payload.clone()),
            StatusCode::CONFLICT,
        )
        .await?;
        if collection == GROUPS {
            let mut duplicate_slug = payload.clone();
            duplicate_slug["name"] = json!(format!("{name}-other"));
            http.call(
                Method::POST,
                collection,
                Some(duplicate_slug),
                StatusCode::CONFLICT,
            )
            .await?;
        }
        let updated = http
            .call(
                Method::PUT,
                &path,
                Some(json!({"comments": "changed\nкириллица", "description": "updated"})),
                StatusCode::OK,
            )
            .await?;
        assert_eq!(updated["comments"], "changed\nкириллица");
        http.call(
            Method::PUT,
            &path,
            Some(json!({"name": "  "})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        http.call(
            Method::PUT,
            &path,
            Some(json!({"custom_fields": ["invalid"]})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let after_rejected = http.call(Method::GET, &path, None, StatusCode::OK).await?;
        assert_eq!(after_rejected["name"], name);
        assert_eq!(after_rejected["custom_fields"], created["custom_fields"]);
        let list = http
            .call(
                Method::GET,
                &format!("{collection}?search={name}&sort=name&order=desc"),
                None,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(list["total"], 1);
        assert_eq!(list["items"][0]["comments"], updated["comments"]);
        http.call(
            Method::GET,
            &format!("{collection}?search={name}&sort=comments"),
            None,
            StatusCode::OK,
        )
        .await?;
        let empty_page = http
            .call(
                Method::GET,
                &format!("{collection}?search={name}&offset=100"),
                None,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(empty_page["total"], 1);
        assert_eq!(empty_page["items"], json!([]));
        http.call(
            Method::GET,
            &format!("{collection}?sort=not-a-column"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let mut bulk_item = payload.clone();
        bulk_item["name"] = json!(format!("{name}-bulk"));
        if collection == GROUPS {
            bulk_item["slug"] = json!(format!("{name}-bulk"));
        }
        let bulk = http
            .call(
                Method::POST,
                &format!("{collection}/bulk"),
                Some(json!({"items": [payload, bulk_item]})),
                StatusCode::OK,
            )
            .await?;
        assert_eq!(bulk["created"].as_array().ok_or("bulk created")?.len(), 1);
        assert_eq!(bulk["errors"].as_array().ok_or("bulk errors")?.len(), 1);
        assert_eq!(bulk["created"][0]["comments"], "first\n**markdown**");
        http.call(
            Method::DELETE,
            &format!(
                "{collection}/{}",
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
        http.call(Method::DELETE, &path, None, StatusCode::NOT_FOUND)
            .await?;
        http.call(
            Method::DELETE,
            &format!(
                "/api/v1/custom-fields/{}",
                field["id"].as_str().ok_or("custom field id")?
            ),
            None,
            StatusCode::NO_CONTENT,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis; native HTTP fixture"]
async fn http_relations_protect_set_null_and_existing_baseline_preservation() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let suffix = unique_suffix();
    let name = format!("pwa84-relations-{suffix}");
    let group = http
        .call(
            Method::POST,
            GROUPS,
            Some(json!({"name": name, "slug": name, "comments": "preserve group"})),
            StatusCode::CREATED,
        )
        .await?;
    let bundle = http
        .call(
            Method::POST,
            BUNDLES,
            Some(json!({"name": name, "comments": "preserve bundle"})),
            StatusCode::CREATED,
        )
        .await?;
    let site = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_site (id, name, slug, status, facility, description, physical_address, shipping_address, comments, custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')").bind(site).bind(&name).execute(&db).await?;
    let rack_payload = json!({"name": name, "site_id": site, "group_id": group["id"], "serial":"", "width":19, "u_height":42});
    let rack = http
        .call(
            Method::POST,
            "/api/v1/inventory/racks",
            Some(rack_payload.clone()),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(rack["group_id"], group["id"]);
    let cable = http
        .call(
            Method::POST,
            "/api/v1/inventory/cables",
            Some(json!({"label": name, "bundle_id": bundle["id"]})),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(cable["bundle_id"], bundle["id"]);
    assert_eq!(cable["bundle_name"], name);
    let racks = http
        .call(
            Method::GET,
            &format!(
                "/api/v1/inventory/racks?group_id={}&sort=group",
                group["id"].as_str().ok_or("group id")?
            ),
            None,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(racks["total"], 1);
    assert_eq!(racks["items"][0]["group_name"], name);
    let cables = http
        .call(
            Method::GET,
            &format!(
                "/api/v1/inventory/cables?bundle_id={}&sort=bundle",
                bundle["id"].as_str().ok_or("bundle id")?
            ),
            None,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(cables["total"], 1);
    assert_eq!(cables["items"][0]["bundle_name"], name);
    http.call(
        Method::POST,
        "/api/v1/inventory/racks",
        Some(json!({"name":"bad UUID", "site_id":site, "group_id":"not-uuid"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/cables",
        Some(json!({"bundle_id":"not-uuid"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let group_path = format!("{GROUPS}/{}", group["id"].as_str().ok_or("group id")?);
    let bundle_path = format!("{BUNDLES}/{}", bundle["id"].as_str().ok_or("bundle id")?);
    // Reopening an existing 0001 installation must not re-seed or lose user data.
    piping_backend::storage::database::migrations::run_migrations(&db).await?;
    for (path, comments) in [
        (&group_path, "preserve group"),
        (&bundle_path, "preserve bundle"),
    ] {
        let existing = http.call(Method::GET, path, None, StatusCode::OK).await?;
        assert_eq!(existing["comments"], comments);
    }
    let rack_path = format!(
        "/api/v1/inventory/racks/{}",
        rack["id"].as_str().ok_or("rack id")?
    );
    let cable_path = format!(
        "/api/v1/inventory/cables/{}",
        cable["id"].as_str().ok_or("cable id")?
    );
    assert_eq!(
        http.call(Method::GET, &rack_path, None, StatusCode::OK)
            .await?["group_id"],
        group["id"]
    );
    assert_eq!(
        http.call(Method::GET, &cable_path, None, StatusCode::OK)
            .await?["bundle_id"],
        bundle["id"]
    );
    http.call(Method::DELETE, &group_path, None, StatusCode::CONFLICT)
        .await?;
    let mut invalid_rack = rack_payload;
    invalid_rack["name"] = json!(format!("{name}-missing-group"));
    invalid_rack["group_id"] = json!(Uuid::now_v7());
    http.call(
        Method::POST,
        "/api/v1/inventory/racks",
        Some(invalid_rack),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/cables",
        Some(json!({"bundle_id": Uuid::now_v7()})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    http.call(
        Method::PUT,
        &cable_path,
        Some(json!({"label": "must-not-persist", "bundle_id": Uuid::now_v7()})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    assert_eq!(
        http.call(Method::GET, &cable_path, None, StatusCode::OK)
            .await?["label"],
        name
    );
    http.call(
        Method::PUT,
        &rack_path,
        Some(json!({"name": "must-not-persist", "group_id": Uuid::now_v7()})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    assert_eq!(
        http.call(Method::GET, &rack_path, None, StatusCode::OK)
            .await?["name"],
        name
    );
    http.call(Method::DELETE, &bundle_path, None, StatusCode::NO_CONTENT)
        .await?;
    assert!(
        http.call(Method::GET, &cable_path, None, StatusCode::OK)
            .await?["bundle_id"]
            .is_null()
    );
    http.call(
        Method::PUT,
        &rack_path,
        Some(json!({"group_id": null})),
        StatusCode::OK,
    )
    .await?;
    assert!(
        http.call(Method::GET, &rack_path, None, StatusCode::OK)
            .await?["group_id"]
            .is_null()
    );
    http.call(Method::DELETE, &group_path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(Method::DELETE, &rack_path, None, StatusCode::NO_CONTENT)
        .await?;
    http.call(Method::DELETE, &cable_path, None, StatusCode::NO_CONTENT)
        .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis; native HTTP RBAC-negative"]
async fn http_authenticated_inventory_deny_blocks_every_operation() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let id = Uuid::now_v7();
    let role: Uuid = sqlx::query_scalar("SELECT id FROM rbac_role WHERE name='test-admin-role'")
        .fetch_one(&db)
        .await?;
    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,subjects,resources,enabled) VALUES ($1,$2,'deny',$3,$4,$5,'[]',true)")
        .bind(id).bind(format!("pwa84-deny-{id}"))
        .bind(json!(["READ","CREATE","UPDATE","DELETE"]))
        .bind(json!(["INVENTORY"]))
        .bind(json!([format!("role:{role}")]))
        .execute(&db).await?;
    let mut redis = build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let result: TestResult = async {
        for collection in [GROUPS, BUNDLES] {
            let path = format!("{collection}/{}", Uuid::now_v7());
            for (method, uri, body) in [
                (Method::GET, collection.to_string(), None),
                (Method::GET, path.clone(), None),
                (
                    Method::POST,
                    collection.to_string(),
                    Some(json!({"name":"denied"})),
                ),
                (Method::PUT, path.clone(), Some(json!({"name":"denied"}))),
                (Method::DELETE, path, None),
                (
                    Method::POST,
                    format!("{collection}/{}/custom-fields/reveal", Uuid::now_v7()),
                    None,
                ),
                (
                    Method::POST,
                    format!("{collection}/bulk"),
                    Some(json!({"items":[{"name":"denied"}]})),
                ),
            ] {
                http.call(method, &uri, body, StatusCode::FORBIDDEN).await?;
            }
        }
        Ok(())
    }
    .await;
    sqlx::query("DELETE FROM rbac_policy WHERE id=$1")
        .bind(id)
        .execute(&db)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    result
}
