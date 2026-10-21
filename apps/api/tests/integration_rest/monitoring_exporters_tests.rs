//! Monitoring exporter / metric-set REST CRUD behaviour tests.
//!
//! These sub-resources were split out of `monitoring.rs` (PWA-33) and are
//! measured by Sonar as new code; no other suite drives their handlers, which
//! is why they carried 0 % new coverage. The tests exercise real request
//! handling against the disposable database, not helper internals: each
//! endpoint is driven through the in-process router and its response body is
//! asserted, so the handler, validation and repository lines all execute.

use super::*;

/// Build the router and an admin session cookie. The seeded test admin owns a
/// full-access policy, so every monitoring write below is authorized.
#[allow(
    clippy::expect_used,
    reason = "clippy.toml allow-expect-in-tests covers #[test] fns, not these shared helpers; a fixture failure must abort the calling test"
)]
async fn admin_router_and_cookie() -> (Router, String) {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    (app, cookie)
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn create_exporter(app: &Router, cookie: &str, slug: &str, name: &str) -> serde_json::Value {
    let payload = serde_json::json!({ "name": name, "slug": slug }).to_string();
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/monitoring/exporters",
        Some(&payload),
        Some(cookie),
    )
    .await
    .expect("create exporter request");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create exporter body={}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("created exporter json")
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn create_metric_query(app: &Router, cookie: &str, suffix: &str) -> serde_json::Value {
    let payload = serde_json::json!({
        "name": format!("Metric Query {suffix}"),
        "slug": format!("metric-query-{suffix}"),
        "query": "up",
    })
    .to_string();
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/monitoring/metric-queries",
        Some(&payload),
        Some(cookie),
    )
    .await
    .expect("create metric query request");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create metric query body={}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("created metric query json")
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn create_metric_set(app: &Router, cookie: &str, suffix: &str) -> serde_json::Value {
    let payload = serde_json::json!({
        "name": format!("Metric Set {suffix}"),
        "slug": format!("metric-set-{suffix}"),
        "description": "integration metric set",
        "enabled": true,
    })
    .to_string();
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/monitoring/metric-sets",
        Some(&payload),
        Some(cookie),
    )
    .await
    .expect("create metric set request");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create metric set body={}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("created metric set json")
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn list_monitoring(app: &Router, cookie: &str, uri: &str) -> serde_json::Value {
    let (status, body) = request(app, "GET", uri, None, Some(cookie))
        .await
        .expect("list request");
    assert_eq!(
        status,
        StatusCode::OK,
        "GET {uri} body={}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("list json")
}

/// POST a monitoring payload and assert the exact status, printing both the
/// request payload and the error envelope when the assertion fails.
#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn post_expect(
    app: &Router,
    cookie: &str,
    uri: &str,
    payload: serde_json::Value,
    expected: StatusCode,
) {
    let body = payload.to_string();
    let (status, response) = request(app, "POST", uri, Some(&body), Some(cookie))
        .await
        .expect("monitoring POST request");
    assert_eq!(
        status,
        expected,
        "POST {uri} payload={body} body={}",
        String::from_utf8_lossy(&response)
    );
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
fn items_contain_id(response: &serde_json::Value, id: &str) -> bool {
    response["items"]
        .as_array()
        .expect("items array")
        .iter()
        .any(|item| item["id"].as_str() == Some(id))
}

/// Smoke proof that the module is wired into the suite and that the exporters
/// list denies an anonymous caller. Monitoring list handlers gate through the
/// policy layer rather than the `RequireAuthenticated` extractor, so the
/// denial is a 403 envelope, not a 401.
#[tokio::test]
#[ignore]
async fn monitoring_exporters_list_requires_authentication() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, _) = request(&app, "GET", "/api/v1/monitoring/exporters", None, None)
        .await
        .expect("exporters without auth");
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore]
async fn monitoring_exporters_crud_round_trip() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let raw_slug = format!("Exporters Round Trip {suffix}");
    let expected_slug = format!("exporters-round-trip-{suffix}");

    let create_body = serde_json::json!({
        "name": "Round Trip Exporter",
        "slug": raw_slug,
        "exporter_type": "Node Exporter",
        "description": "   ",
        "default_job": "node",
        "default_scheme": "HTTPS",
        "default_port": 9100,
        "default_metrics_path": "custom/metrics",
        "default_target_source": "host_name",
        "labels": { "env": "test" },
        "enabled": true,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/exporters",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("create exporter");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create exporter body={}",
        String::from_utf8_lossy(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body).expect("created exporter json");
    let id = created["id"].as_str().expect("created id").to_string();
    assert_eq!(created["name"].as_str(), Some("round trip exporter"));
    assert_eq!(created["slug"].as_str(), Some(expected_slug.as_str()));
    assert_eq!(created["exporter_type"].as_str(), Some("node-exporter"));
    assert_eq!(created["default_job"].as_str(), Some("node"));
    assert_eq!(created["default_scheme"].as_str(), Some("https"));
    assert_eq!(created["default_port"].as_i64(), Some(9100));
    assert_eq!(
        created["default_metrics_path"].as_str(),
        Some("/custom/metrics")
    );
    assert_eq!(created["default_target_source"].as_str(), Some("host_name"));
    assert_eq!(created["enabled"].as_bool(), Some(true));
    assert_eq!(created["labels"]["env"].as_str(), Some("test"));
    assert!(
        created["description"].is_null(),
        "whitespace-only description must normalize to null"
    );

    let listed = list_monitoring(&app, &cookie, "/api/v1/monitoring/exporters").await;
    assert!(items_contain_id(&listed, &id));
    assert_eq!(
        listed["total"].as_u64(),
        Some(listed["items"].as_array().expect("items").len() as u64)
    );

    let update_body = serde_json::json!({
        "name": "Round Trip Exporter Updated",
        "slug": raw_slug,
        "default_scheme": "http",
        "default_port": 9200,
        "default_target_source": "host_address",
        "labels": {},
        "enabled": false,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/exporters/{id}"),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("update exporter");
    assert_eq!(
        status,
        StatusCode::OK,
        "update exporter body={}",
        String::from_utf8_lossy(&body)
    );
    let updated: serde_json::Value = serde_json::from_slice(&body).expect("updated exporter json");
    assert_eq!(
        updated["name"].as_str(),
        Some("round trip exporter updated")
    );
    assert_eq!(updated["default_port"].as_i64(), Some(9200));
    assert_eq!(updated["default_scheme"].as_str(), Some("http"));
    assert_eq!(
        updated["default_target_source"].as_str(),
        Some("host_address")
    );
    assert_eq!(
        updated["default_job"].as_str(),
        Some(expected_slug.as_str())
    );
    assert_eq!(updated["default_metrics_path"].as_str(), Some("/metrics"));
    assert_eq!(updated["enabled"].as_bool(), Some(false));
    assert!(updated["description"].is_null());

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporters/{id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete exporter");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let listed = list_monitoring(&app, &cookie, "/api/v1/monitoring/exporters").await;
    assert!(!items_contain_id(&listed, &id));
}

#[tokio::test]
#[ignore]
async fn monitoring_exporter_validation_and_conflict_paths() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let uri = "/api/v1/monitoring/exporters";

    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({ "name": "Bad Slug", "slug": "---" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({ "name": "   ", "slug": format!("valid-{suffix}") }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "name": "Bad Port",
            "slug": format!("bad-port-{suffix}"),
            "default_port": 0,
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "name": "Bad Scheme",
            "slug": format!("bad-scheme-{suffix}"),
            "default_scheme": "ftp",
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "name": "Bad Target",
            "slug": format!("bad-target-{suffix}"),
            "default_target_source": "nowhere",
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "name": "Bad Labels",
            "slug": format!("bad-labels-{suffix}"),
            "labels": ["not", "an", "object"],
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;

    let duplicate_slug = format!("duplicate-{suffix}");
    let duplicate_body = serde_json::json!({
        "name": "Duplicate Exporter",
        "slug": duplicate_slug,
    })
    .to_string();
    let (status, body) = request(&app, "POST", uri, Some(&duplicate_body), Some(&cookie))
        .await
        .expect("create duplicate exporter");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "body={}",
        String::from_utf8_lossy(&body)
    );
    let created: serde_json::Value =
        serde_json::from_slice(&body).expect("duplicate exporter json");
    let id = created["id"].as_str().expect("id").to_string();

    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({ "name": "Duplicate Exporter", "slug": duplicate_slug }),
        StatusCode::CONFLICT,
    )
    .await;

    let unknown = Uuid::now_v7();
    let valid_body = serde_json::json!({
        "name": "Ghost Exporter",
        "slug": format!("ghost-{suffix}"),
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/exporters/{unknown}"),
        Some(&valid_body),
        Some(&cookie),
    )
    .await
    .expect("update unknown exporter");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporters/{unknown}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete unknown exporter");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporters/{id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup exporter");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn monitoring_exporter_assignments_crud() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let exporter = create_exporter(
        &app,
        &cookie,
        &format!("Assignment Exporter {suffix}"),
        "Assignment Exporter",
    )
    .await;
    let exporter_id = exporter["id"].as_str().expect("exporter id").to_string();

    let create_body = serde_json::json!({
        "exporter_id": exporter_id,
        "name": "Primary",
        "scope_type": "global",
        "weight": 5,
        "enabled": true,
        "target": " Host ",
        "job": "node",
        "scheme": "HTTPS",
        "port": 9100,
        "metrics_path": "custom",
        "labels": { "team": "ops" },
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/exporter-assignments",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("create assignment");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create assignment body={}",
        String::from_utf8_lossy(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body).expect("assignment json");
    let assignment_id = created["id"].as_str().expect("assignment id").to_string();
    assert_eq!(created["exporter_id"].as_str(), Some(exporter_id.as_str()));
    assert_eq!(created["scope_type"].as_str(), Some("global"));
    assert_eq!(created["name"].as_str(), Some("Primary"));
    assert_eq!(created["weight"].as_i64(), Some(5));
    assert_eq!(created["enabled"].as_bool(), Some(true));
    assert_eq!(created["target"].as_str(), Some("host"));
    assert_eq!(created["scheme"].as_str(), Some("https"));
    assert_eq!(created["port"].as_i64(), Some(9100));
    assert_eq!(created["metrics_path"].as_str(), Some("custom"));
    assert_eq!(created["labels"]["team"].as_str(), Some("ops"));

    let listed = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/exporter-assignments?exporter_id={exporter_id}"),
    )
    .await;
    assert!(items_contain_id(&listed, &assignment_id));

    let update_body = serde_json::json!({
        "exporter_id": exporter_id,
        "name": "Secondary",
        "scope_type": "global",
        "weight": 9,
        "enabled": false,
        "labels": {},
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/exporter-assignments/{assignment_id}"),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("update assignment");
    assert_eq!(
        status,
        StatusCode::OK,
        "update assignment body={}",
        String::from_utf8_lossy(&body)
    );
    let updated: serde_json::Value =
        serde_json::from_slice(&body).expect("updated assignment json");
    assert_eq!(updated["name"].as_str(), Some("Secondary"));
    assert_eq!(updated["weight"].as_i64(), Some(9));
    assert_eq!(updated["enabled"].as_bool(), Some(false));
    assert!(updated["target"].is_null());
    assert!(updated["scheme"].is_null());

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporter-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete assignment");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let listed = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/exporter-assignments?exporter_id={exporter_id}"),
    )
    .await;
    assert!(!items_contain_id(&listed, &assignment_id));

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporters/{exporter_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup exporter");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn monitoring_exporter_assignment_custom_catalog_and_validation() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let uri = "/api/v1/monitoring/exporter-assignments";

    let custom_body = serde_json::json!({
        "exporter_slug": format!("Custom Exporter {suffix}"),
        "exporter_name": "Custom Exporter",
        "scope_type": "global",
        "weight": 1,
    })
    .to_string();
    let (status, body) = request(&app, "POST", uri, Some(&custom_body), Some(&cookie))
        .await
        .expect("create custom assignment");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create custom assignment body={}",
        String::from_utf8_lossy(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body).expect("custom assignment json");
    let assignment_id = created["id"].as_str().expect("assignment id").to_string();
    let catalog_id = created["exporter_id"]
        .as_str()
        .expect("catalog id")
        .to_string();
    assert_eq!(
        created["exporter_slug"].as_str(),
        Some(format!("custom-exporter-{suffix}").as_str())
    );
    assert_eq!(created["exporter_name"].as_str(), Some("Custom Exporter"));

    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({ "exporter_id": catalog_id, "scope_type": "cluster" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({ "exporter_id": catalog_id, "scope_type": "site" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "exporter_id": catalog_id,
            "scope_type": "global",
            "scope_id": Uuid::now_v7(),
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "exporter_id": catalog_id,
            "scope_type": "global",
            "port": 70000,
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "exporter_id": catalog_id,
            "scope_type": "global",
            "scheme": "ftp",
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({
            "exporter_id": catalog_id,
            "scope_type": "global",
            "labels": ["not", "an", "object"],
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({ "scope_type": "global" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        uri,
        serde_json::json!({ "exporter_id": Uuid::now_v7(), "scope_type": "global" }),
        StatusCode::BAD_REQUEST,
    )
    .await;

    let unknown = Uuid::now_v7();
    let valid_body = serde_json::json!({
        "exporter_id": catalog_id,
        "scope_type": "global",
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/exporter-assignments/{unknown}"),
        Some(&valid_body),
        Some(&cookie),
    )
    .await
    .expect("update unknown assignment");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporter-assignments/{unknown}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete unknown assignment");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporter-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup custom assignment");
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/exporters/{catalog_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup custom exporter");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn monitoring_metric_sets_crud_with_items() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let query = create_metric_query(&app, &cookie, &suffix).await;
    let query_id = query["id"].as_str().expect("query id").to_string();
    let query_slug = query["slug"].as_str().expect("query slug").to_string();

    let set = create_metric_set(&app, &cookie, &suffix).await;
    let set_id = set["id"].as_str().expect("set id").to_string();
    assert_eq!(
        set["items"].as_array().expect("items").len(),
        0,
        "new metric set has no items"
    );

    let item_body = serde_json::json!({
        "metric_query_id": query_id,
        "show_in_inspector": true,
        "show_in_list": true,
        "show_in_hover": true,
        "list_render": "gauge",
        "sort_order": 3,
        "label_override": "CPU",
        "unit_override": "%",
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/monitoring/metric-sets/{set_id}/items"),
        Some(&item_body),
        Some(&cookie),
    )
    .await
    .expect("create metric set item");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create metric set item body={}",
        String::from_utf8_lossy(&body)
    );
    let item: serde_json::Value = serde_json::from_slice(&body).expect("created item json");
    let item_id = item["id"].as_str().expect("item id").to_string();
    assert_eq!(item["metric_set_id"].as_str(), Some(set_id.as_str()));
    assert_eq!(item["metric_query_id"].as_str(), Some(query_id.as_str()));
    assert_eq!(
        item["metric_query_slug"].as_str(),
        Some(query_slug.as_str())
    );
    assert_eq!(item["list_render"].as_str(), Some("gauge"));
    assert_eq!(item["sort_order"].as_i64(), Some(3));
    assert_eq!(item["show_in_list"].as_bool(), Some(true));

    let listed = list_monitoring(&app, &cookie, "/api/v1/monitoring/metric-sets").await;
    let found = listed["items"]
        .as_array()
        .expect("sets")
        .iter()
        .find(|entry| entry["id"].as_str() == Some(set_id.as_str()))
        .expect("metric set in list");
    let nested = found["items"].as_array().expect("nested items");
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0]["id"].as_str(), Some(item_id.as_str()));

    let item_update = serde_json::json!({
        "metric_query_id": query_id,
        "show_in_list": false,
        "show_in_hover": false,
        "list_render": "value",
        "sort_order": 7,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/metric-sets/{set_id}/items/{item_id}"),
        Some(&item_update),
        Some(&cookie),
    )
    .await
    .expect("update metric set item");
    assert_eq!(
        status,
        StatusCode::OK,
        "update metric set item body={}",
        String::from_utf8_lossy(&body)
    );
    let updated_item: serde_json::Value = serde_json::from_slice(&body).expect("updated item json");
    assert_eq!(updated_item["list_render"].as_str(), Some("value"));
    assert_eq!(updated_item["sort_order"].as_i64(), Some(7));
    assert_eq!(updated_item["show_in_list"].as_bool(), Some(false));
    assert_eq!(updated_item["show_in_inspector"].as_bool(), Some(true));
    assert!(updated_item["label_override"].is_null());

    let set_update = serde_json::json!({
        "name": "Metric Set Updated",
        "slug": format!("metric-set-{suffix}"),
        "enabled": false,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/metric-sets/{set_id}"),
        Some(&set_update),
        Some(&cookie),
    )
    .await
    .expect("update metric set");
    assert_eq!(
        status,
        StatusCode::OK,
        "update metric set body={}",
        String::from_utf8_lossy(&body)
    );
    let updated_set: serde_json::Value = serde_json::from_slice(&body).expect("updated set json");
    assert_eq!(updated_set["name"].as_str(), Some("metric set updated"));
    assert_eq!(updated_set["enabled"].as_bool(), Some(false));
    assert_eq!(
        updated_set["items"].as_array().expect("items").len(),
        1,
        "nested items survive the set update"
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/metric-sets/{set_id}/items/{item_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete metric set item");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let listed = list_monitoring(&app, &cookie, "/api/v1/monitoring/metric-sets").await;
    let found = listed["items"]
        .as_array()
        .expect("sets")
        .iter()
        .find(|entry| entry["id"].as_str() == Some(set_id.as_str()))
        .expect("metric set still in list");
    assert_eq!(found["items"].as_array().expect("items").len(), 0);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/metric-sets/{set_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete metric set");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let listed = list_monitoring(&app, &cookie, "/api/v1/monitoring/metric-sets").await;
    assert!(!items_contain_id(&listed, &set_id));
}

#[tokio::test]
#[ignore]
async fn monitoring_metric_set_validation_error_paths() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let sets_uri = "/api/v1/monitoring/metric-sets";

    post_expect(
        &app,
        &cookie,
        sets_uri,
        serde_json::json!({ "name": "   ", "slug": format!("set-{suffix}") }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        sets_uri,
        serde_json::json!({ "name": "Empty Slug", "slug": "  " }),
        StatusCode::BAD_REQUEST,
    )
    .await;

    let set = create_metric_set(&app, &cookie, &suffix).await;
    let set_id = set["id"].as_str().expect("set id").to_string();
    let query = create_metric_query(&app, &cookie, &suffix).await;
    let query_id = query["id"].as_str().expect("query id").to_string();

    let bad_render = serde_json::json!({
        "metric_query_id": query_id,
        "list_render": "ring",
    })
    .to_string();
    let (status, _) = request(
        &app,
        "POST",
        &format!("{sets_uri}/{set_id}/items"),
        Some(&bad_render),
        Some(&cookie),
    )
    .await
    .expect("item with invalid list_render");
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let unknown_set = Uuid::now_v7();
    let valid_item = serde_json::json!({ "metric_query_id": query_id }).to_string();
    let (status, _) = request(
        &app,
        "POST",
        &format!("{sets_uri}/{unknown_set}/items"),
        Some(&valid_item),
        Some(&cookie),
    )
    .await
    .expect("item on unknown set");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let unknown_item = Uuid::now_v7();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("{sets_uri}/{set_id}/items/{unknown_item}"),
        Some(&valid_item),
        Some(&cookie),
    )
    .await
    .expect("update unknown item");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("{sets_uri}/{set_id}/items/{unknown_item}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete unknown item");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let set_update = serde_json::json!({
        "name": "Ghost Set",
        "slug": format!("ghost-set-{suffix}"),
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("{sets_uri}/{unknown_set}"),
        Some(&set_update),
        Some(&cookie),
    )
    .await
    .expect("update unknown set");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("{sets_uri}/{unknown_set}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete unknown set");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("{sets_uri}/{set_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup metric set");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn monitoring_metric_set_assignments_crud() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    create_metric_query(&app, &cookie, &suffix).await;
    let set = create_metric_set(&app, &cookie, &suffix).await;
    let set_id = set["id"].as_str().expect("set id").to_string();
    let set_slug = set["slug"].as_str().expect("set slug").to_string();

    let create_body = serde_json::json!({
        "metric_set_id": set_id,
        "scope_type": "global",
        "weight": 2,
        "enabled": true,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/metric-set-assignments",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("create metric set assignment");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create metric set assignment body={}",
        String::from_utf8_lossy(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body).expect("assignment json");
    let assignment_id = created["id"].as_str().expect("assignment id").to_string();
    assert_eq!(created["metric_set_id"].as_str(), Some(set_id.as_str()));
    assert_eq!(created["metric_set_slug"].as_str(), Some(set_slug.as_str()));
    assert_eq!(created["scope_type"].as_str(), Some("global"));
    assert_eq!(created["weight"].as_i64(), Some(2));
    assert_eq!(created["enabled"].as_bool(), Some(true));

    let listed = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/metric-set-assignments?metric_set_id={set_id}"),
    )
    .await;
    assert!(items_contain_id(&listed, &assignment_id));

    let update_body = serde_json::json!({
        "metric_set_id": set_id,
        "scope_type": "global",
        "weight": 4,
        "enabled": false,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/metric-set-assignments/{assignment_id}"),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("update metric set assignment");
    assert_eq!(
        status,
        StatusCode::OK,
        "update metric set assignment body={}",
        String::from_utf8_lossy(&body)
    );
    let updated: serde_json::Value =
        serde_json::from_slice(&body).expect("updated assignment json");
    assert_eq!(updated["weight"].as_i64(), Some(4));
    assert_eq!(updated["enabled"].as_bool(), Some(false));

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/metric-set-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete metric set assignment");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let listed = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/metric-set-assignments?metric_set_id={set_id}"),
    )
    .await;
    assert!(!items_contain_id(&listed, &assignment_id));

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/metric-sets/{set_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup metric set");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn monitoring_metric_set_assignment_validation_paths() {
    let (app, cookie) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let assignments_uri = "/api/v1/monitoring/metric-set-assignments";
    let set = create_metric_set(&app, &cookie, &suffix).await;
    let set_id = set["id"].as_str().expect("set id").to_string();

    post_expect(
        &app,
        &cookie,
        assignments_uri,
        serde_json::json!({ "metric_set_id": set_id, "scope_type": "cluster" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        assignments_uri,
        serde_json::json!({ "metric_set_id": set_id, "scope_type": "site" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        assignments_uri,
        serde_json::json!({
            "metric_set_id": set_id,
            "scope_type": "global",
            "scope_id": Uuid::now_v7(),
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    post_expect(
        &app,
        &cookie,
        assignments_uri,
        serde_json::json!({ "metric_set_id": Uuid::now_v7(), "scope_type": "global" }),
        StatusCode::BAD_REQUEST,
    )
    .await;

    let unknown = Uuid::now_v7();
    let valid_body = serde_json::json!({
        "metric_set_id": set_id,
        "scope_type": "global",
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("{assignments_uri}/{unknown}"),
        Some(&valid_body),
        Some(&cookie),
    )
    .await
    .expect("update unknown metric set assignment");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("{assignments_uri}/{unknown}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete unknown metric set assignment");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/metric-sets/{set_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup metric set");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn monitoring_sd_label_settings_get_then_update() {
    let (app, cookie) = admin_router_and_cookie().await;

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/sd/label-settings",
        None,
        Some(&cookie),
    )
    .await
    .expect("get sd label settings");
    assert_eq!(
        status,
        StatusCode::OK,
        "get sd label settings body={}",
        String::from_utf8_lossy(&body)
    );
    let initial: serde_json::Value = serde_json::from_slice(&body).expect("initial settings json");
    let groups = initial["groups"].as_object().expect("groups object");
    for key in [
        "location",
        "tenant",
        "role",
        "status",
        "hardware",
        "network",
        "contacts",
        "description",
        "identifiers",
    ] {
        assert!(
            groups[key].is_boolean(),
            "group toggle {key} must be a boolean"
        );
    }
    assert!(groups["device_statuses"].is_array());
    assert!(groups["site_statuses"].is_array());

    let update_body = serde_json::json!({
        "groups": {
            "location": true,
            "tenant": true,
            "role": false,
            "status": false,
            "hardware": false,
            "network": false,
            "contacts": false,
            "description": false,
            "identifiers": false,
            "device_statuses": [" Planned ", "ACTIVE", "active"],
            "site_statuses": ["Active"],
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/monitoring/sd/label-settings",
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("update sd label settings");
    assert_eq!(
        status,
        StatusCode::OK,
        "update sd label settings body={}",
        String::from_utf8_lossy(&body)
    );
    let updated: serde_json::Value = serde_json::from_slice(&body).expect("updated settings json");
    assert_eq!(updated["groups"]["location"].as_bool(), Some(true));
    assert_eq!(updated["groups"]["tenant"].as_bool(), Some(true));
    assert_eq!(updated["groups"]["role"].as_bool(), Some(false));
    assert_eq!(
        updated["groups"]["device_statuses"],
        serde_json::json!(["active", "planned"]),
        "statuses are trimmed, lower-cased, sorted and de-duplicated"
    );
    assert_eq!(
        updated["groups"]["site_statuses"],
        serde_json::json!(["active"])
    );

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/sd/label-settings",
        None,
        Some(&cookie),
    )
    .await
    .expect("re-get sd label settings");
    assert_eq!(status, StatusCode::OK);
    let reread: serde_json::Value = serde_json::from_slice(&body).expect("reread settings json");
    assert_eq!(reread, updated, "GET reflects the persisted update");
}
