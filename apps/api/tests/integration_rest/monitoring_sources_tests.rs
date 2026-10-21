//! Monitoring source and metric-query REST CRUD behaviour tests.
//!
//! These sub-resources were split out of `monitoring.rs` (PWA-33) and are
//! measured by Sonar as new code; no other suite in `integration_rest` drives
//! their handlers, which is why they carried 0 % new coverage. The tests
//! exercise real request handling against the disposable database, not helper
//! internals.
//!
//! Routes covered here:
//!   * `query-sources` GET/POST + `{id}` PUT/DELETE and their assignments
//!   * `alert-sources` GET/POST + `{id}` PUT/DELETE and their assignments
//!   * `metric-queries` GET/POST + `{id}` PUT/DELETE, `/test`, `host-overrides`
//!   * read-only composites (`presets`, `sd`, `sd/preview`, `latest`,
//!     `host-labels`, `metrics`, `history`, `targets/{id}`) and `quick-start`

use super::common::seed_device;
use super::*;

/// A syntactically valid, non-private monitoring endpoint. The test database
/// and DNS are offline, so handlers that actually call this URL must answer a
/// handled failure rather than live data.
fn monitoring_test_url(suffix: &str) -> String {
    format!("https://monitoring-{suffix}.example.test/api/v1/query")
}

/// Valid `options.display` subtree used to exercise the typed display-config
/// validator on metric queries and host overrides.
fn display_options() -> serde_json::Value {
    serde_json::json!({
        "display": {
            "status": {
                "mode": "numeric",
                "warning": 1.0,
                "critical": 2.0,
                "direction": "higher_is_worse"
            }
        }
    })
}

fn body_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).to_string()
}

/// Percent-encode a query-string value, used to pass a PromQL descriptor back
/// to the history endpoint without pulling in a URL crate.
fn encode_query_param(raw: &str) -> String {
    let mut encoded = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Smoke proof that the module is wired into the suite and that the admin
/// session reaches the query-sources list.
#[tokio::test]
#[ignore]
async fn monitoring_query_sources_list_requires_authentication() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, _) = request(&app, "GET", "/api/v1/monitoring/query-sources", None, None)
        .await
        .expect("query sources without auth");
    // The auth extractor rejects the anonymous principal before the handler
    // runs; RBAC denial is the documented 403 envelope.
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Create → list → update → delete a query source, plus its scope assignment,
/// duplicate-slug and validation/error branches.
#[tokio::test]
#[ignore]
async fn monitoring_query_source_and_assignment_crud()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login failed")?;
    let suffix = unique_suffix();
    let slug = format!("itest-qs-{suffix}");

    let create = serde_json::json!({
        "name": format!("itest query source {suffix}"),
        "slug": slug,
        "source_type": "prometheus",
        "query_url": monitoring_test_url(&suffix),
        "timeout_ms": 3000,
        "options": {},
        "enabled": true,
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&create.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create query source: {}",
        body_text(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body)?;
    let source_id = created["id"].as_str().ok_or("missing query source id")?;
    assert_eq!(created["slug"].as_str(), Some(slug.as_str()));
    assert_eq!(created["source_type"].as_str(), Some("prometheus"));
    assert_eq!(created["enabled"].as_bool(), Some(true));
    assert_eq!(created["timeout_ms"].as_i64(), Some(3000));
    assert!(
        created["options"].is_object(),
        "options must echo an object"
    );

    // Duplicate slug is a database unique violation → 409 Conflict.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&create.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "duplicate slug must conflict: {}",
        body_text(&body)
    );

    // List filtered by source_type and find the row we just created.
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/query-sources?source_type=prometheus&sort=slug&order=asc",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let list: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        list["items"]
            .as_array()
            .ok_or("items array")?
            .iter()
            .any(|item| item["id"].as_str() == Some(source_id)),
        "created source must appear in the filtered list: {}",
        body_text(&body)
    );

    // Update: full replacement, including source_type, timeout and enabled.
    let update = serde_json::json!({
        "name": format!("itest query source updated {suffix}"),
        "slug": slug,
        "source_type": "victoriametrics",
        "query_url": monitoring_test_url(&suffix),
        "timeout_ms": 5000,
        "options": {},
        "enabled": false,
    });
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        Some(&update.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "update: {}", body_text(&body));
    let updated: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(updated["source_type"].as_str(), Some("victoriametrics"));
    assert_eq!(updated["enabled"].as_bool(), Some(false));
    assert_eq!(updated["timeout_ms"].as_i64(), Some(5000));
    let expected_name = format!("itest query source updated {suffix}");
    assert_eq!(updated["name"].as_str(), Some(expected_name.as_str()));

    // Validation branches: too-small timeout, unknown enum, malformed body.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(
            &serde_json::json!({
                "name": "bad timeout", "slug": format!("itest-qs-bad-{suffix}"),
                "query_url": monitoring_test_url(&suffix), "timeout_ms": 50
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(
            &serde_json::json!({
                "name": "bad type", "slug": format!("itest-qs-type-{suffix}"),
                "source_type": "not-a-source", "query_url": monitoring_test_url(&suffix)
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&serde_json::json!({ "name": "missing required fields" }).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    // Unknown ids behave as documented.
    let missing = Uuid::now_v7().to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-sources/{missing}"),
        Some(&update.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{missing}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // --- assignment lifecycle against the created source -------------------
    let assignment = serde_json::json!({
        "query_source_id": source_id,
        "scope_type": "global",
        "weight": 5,
        "enabled": true,
        "options": {},
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-source-assignments",
        Some(&assignment.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create assignment: {}",
        body_text(&body)
    );
    let created_assignment: serde_json::Value = serde_json::from_slice(&body)?;
    let assignment_id = created_assignment["id"]
        .as_str()
        .ok_or("missing assignment id")?;
    assert_eq!(created_assignment["scope_type"].as_str(), Some("global"));
    assert_eq!(created_assignment["weight"].as_i64(), Some(5));
    assert_eq!(created_assignment["enabled"].as_bool(), Some(true));

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/query-source-assignments?query_source_id={source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let assignments: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        assignments["items"]
            .as_array()
            .ok_or("assignment items")?
            .iter()
            .any(|item| item["id"].as_str() == Some(assignment_id)),
        "assignment must be listed by source id: {}",
        body_text(&body)
    );

    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-source-assignments/{assignment_id}"),
        Some(
            &serde_json::json!({
                "query_source_id": source_id, "scope_type": "global",
                "weight": 9, "enabled": false, "options": {}
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "update assignment: {}",
        body_text(&body)
    );
    let updated_assignment: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(updated_assignment["weight"].as_i64(), Some(9));
    assert_eq!(updated_assignment["enabled"].as_bool(), Some(false));

    // `site` scope without a scope_id is rejected.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-source-assignments",
        Some(
            &serde_json::json!({
                "query_source_id": source_id, "scope_type": "site"
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-source-assignments/{missing}"),
        Some(&assignment.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-source-assignments/{missing}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-source-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    Ok(())
}

/// Create → list → update → delete an alert source and its assignment, with
/// validation and unknown-id branches.
#[tokio::test]
#[ignore]
async fn monitoring_alert_source_and_assignment_crud()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login failed")?;
    let suffix = unique_suffix();
    let name = format!("itest alert source {suffix}");

    let create = serde_json::json!({
        "name": name,
        "source_type": "alertmanager",
        "query_url": format!("https://alerts-{suffix}.example.test/api/v2/alerts"),
        "timeout_ms": 3000,
        "options": {},
        "enabled": true,
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&create.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create alert source: {}",
        body_text(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body)?;
    let source_id = created["id"].as_str().ok_or("missing alert source id")?;
    assert_eq!(created["name"].as_str(), Some(name.as_str()));
    assert_eq!(created["source_type"].as_str(), Some("alertmanager"));
    assert_eq!(created["enabled"].as_bool(), Some(true));

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/alert-sources?source_type=alertmanager",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let list: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        list["items"]
            .as_array()
            .ok_or("items array")?
            .iter()
            .any(|item| item["id"].as_str() == Some(source_id)),
        "created alert source must appear in the list: {}",
        body_text(&body)
    );

    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/alert-sources/{source_id}"),
        Some(
            &serde_json::json!({
                "name": format!("itest alert source updated {suffix}"),
                "source_type": "vmalert",
                "query_url": format!("https://alerts-{suffix}.example.test/api/v2/alerts"),
                "timeout_ms": 7000,
                "options": {},
                "enabled": false,
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "update alert source: {}",
        body_text(&body)
    );
    let updated: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(updated["source_type"].as_str(), Some("vmalert"));
    assert_eq!(updated["enabled"].as_bool(), Some(false));
    assert_eq!(updated["timeout_ms"].as_i64(), Some(7000));

    // Validation branches.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(
            &serde_json::json!({
                "name": "bad alert type", "source_type": "pagerduty",
                "query_url": "https://alerts.example.test/api"
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(
            &serde_json::json!({
                "name": "bad alert timeout", "source_type": "alertmanager",
                "query_url": "https://alerts.example.test/api", "timeout_ms": 10
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    let missing = Uuid::now_v7().to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/alert-sources/{missing}"),
        Some(&create.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-sources/{missing}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // --- assignment lifecycle ---------------------------------------------
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-source-assignments",
        Some(
            &serde_json::json!({
                "alert_source_id": source_id, "scope_type": "global", "weight": 3, "enabled": true
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create alert assignment: {}",
        body_text(&body)
    );
    let created_assignment: serde_json::Value = serde_json::from_slice(&body)?;
    let assignment_id = created_assignment["id"]
        .as_str()
        .ok_or("missing alert assignment id")?;
    assert_eq!(created_assignment["scope_type"].as_str(), Some("global"));
    assert_eq!(created_assignment["weight"].as_i64(), Some(3));

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/alert-source-assignments?alert_source_id={source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let assignments: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        assignments["items"]
            .as_array()
            .ok_or("assignment items")?
            .iter()
            .any(|item| item["id"].as_str() == Some(assignment_id)),
        "alert assignment must be listed by source id: {}",
        body_text(&body)
    );

    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/alert-source-assignments/{assignment_id}"),
        Some(
            &serde_json::json!({
                "alert_source_id": source_id, "scope_type": "global",
                "weight": 8, "enabled": false
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "update alert assignment: {}",
        body_text(&body)
    );
    let updated_assignment: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(updated_assignment["weight"].as_i64(), Some(8));
    assert_eq!(updated_assignment["enabled"].as_bool(), Some(false));

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-source-assignments",
        Some(
            &serde_json::json!({
                "alert_source_id": source_id, "scope_type": "role"
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-source-assignments/{missing}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-source-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-sources/{source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    Ok(())
}

/// Metric query CRUD, its `/test` endpoint, and per-host overrides (which need
/// a real inventory host to satisfy the existence gate).
#[tokio::test]
#[ignore]
async fn monitoring_metric_query_host_override_and_live_test()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login failed")?;
    let suffix = unique_suffix();
    let slug = format!("itest-mq-{suffix}");

    let pool = build_test_db_pool().await?;
    let (host_id, _) = seed_device(&pool, &format!("itest-mq-{suffix}")).await?;

    let create = serde_json::json!({
        "name": format!("itest metric query {suffix}"),
        "slug": slug,
        "query": "up",
        "source_type": "prometheus",
        "query_type": "promql",
        "value_type": "number",
        "widget_type": "stat",
        "options": display_options(),
        "enabled": true,
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries",
        Some(&create.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create metric query: {}",
        body_text(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body)?;
    let query_id = created["id"].as_str().ok_or("missing metric query id")?;
    assert_eq!(created["slug"].as_str(), Some(slug.as_str()));
    assert_eq!(created["source_type"].as_str(), Some("prometheus"));
    assert_eq!(created["query_type"].as_str(), Some("promql"));
    assert_eq!(created["enabled"].as_bool(), Some(true));

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/metric-queries?source_type=prometheus",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let list: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        list["items"]
            .as_array()
            .ok_or("items array")?
            .iter()
            .any(|item| item["id"].as_str() == Some(query_id)),
        "created metric query must appear in the list: {}",
        body_text(&body)
    );

    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/metric-queries/{query_id}"),
        Some(
            &serde_json::json!({
                "name": format!("itest metric query updated {suffix}"),
                "slug": slug,
                "query": "up == 0",
                "source_type": "prometheus",
                "query_type": "promql",
                "value_type": "state",
                "widget_type": "badge",
                "options": {},
                "enabled": false,
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "update metric query: {}",
        body_text(&body)
    );
    let updated: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(updated["query"].as_str(), Some("up == 0"));
    assert_eq!(updated["value_type"].as_str(), Some("state"));
    assert_eq!(updated["enabled"].as_bool(), Some(false));

    // Incompatible query_type for the source family is rejected.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries",
        Some(
            &serde_json::json!({
                "name": "incompatible", "slug": format!("itest-mq-bad-{suffix}"),
                "query": "up", "source_type": "prometheus", "query_type": "zabbix_item_key"
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    let missing = Uuid::now_v7().to_string();
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/metric-queries/{missing}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // --- /metric-queries/test validation and handled upstream failure ------
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries/test",
        Some(
            &serde_json::json!({
                "query": "", "source_type": "prometheus",
                "query_url": monitoring_test_url(&suffix)
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    // A public URL passes shape validation; the offline test runner cannot
    // resolve or reach it, so the handler must answer a handled failure (the
    // documented 4xx validation path) instead of live data.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries/test",
        Some(
            &serde_json::json!({
                "query": "up", "source_type": "prometheus",
                "query_url": monitoring_test_url(&suffix)
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert!(
        status.is_client_error() || status == StatusCode::OK,
        "live test must answer a handled result, got {status}: {}",
        body_text(&body)
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&body).is_ok(),
        "live test body must be JSON: {}",
        body_text(&body)
    );

    // --- host overrides -----------------------------------------------------
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/host-overrides",
        Some(
            &serde_json::json!({
                "metric_query_id": query_id,
                "host_id": host_id.to_string(),
                "label_override": "Availability",
                "unit_override": "bool",
                "options_override": {
                    "status": {
                        "mode": "state",
                        "normal": ["up", "1"],
                        "critical": ["down", "0"]
                    }
                }
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create host override: {}",
        body_text(&body)
    );
    let created_override: serde_json::Value = serde_json::from_slice(&body)?;
    let override_id = created_override["id"]
        .as_str()
        .ok_or("missing host override id")?;
    assert_eq!(
        created_override["host_id"].as_str(),
        Some(host_id.to_string().as_str())
    );
    // Free-text label/unit overrides are normalized to lower case by the
    // handler's `normalize_string`.
    assert_eq!(
        created_override["label_override"].as_str(),
        Some("availability")
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/host-overrides?host_id={host_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let overrides: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        overrides["items"]
            .as_array()
            .ok_or("override items")?
            .iter()
            .any(|item| item["id"].as_str() == Some(override_id)),
        "host override must be listed for its host: {}",
        body_text(&body)
    );

    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/host-overrides/{override_id}"),
        Some(
            &serde_json::json!({
                "metric_query_id": query_id, "host_id": host_id.to_string(),
                "label_override": "Renamed", "unit_override": "percent"
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "update host override: {}",
        body_text(&body)
    );
    let updated_override: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(updated_override["label_override"].as_str(), Some("renamed"));
    assert_eq!(updated_override["unit_override"].as_str(), Some("percent"));

    // Unknown host (device not in inventory) is refused by the existence gate.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/host-overrides",
        Some(
            &serde_json::json!({
                "metric_query_id": query_id, "host_id": Uuid::now_v7().to_string()
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/host-overrides/{missing}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/host-overrides/{override_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/metric-queries/{query_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    Ok(())
}

/// Read-only monitoring endpoints: static preset catalog, service discovery,
/// latest metrics, host labels, host metrics, history and target observability.
#[tokio::test]
#[ignore]
async fn monitoring_read_only_endpoints_and_presets()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut state = build_test_app_state().await?;
    // The HTTP-SD feed is machine-authenticated by its configured bearer token.
    Arc::make_mut(&mut state.settings).monitoring.sd_token = Some("itest-sd-token".to_string());
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login failed")?;
    let suffix = unique_suffix();

    let pool = build_test_db_pool().await?;
    let (host_id, _) = seed_device(&pool, &format!("itest-readonly-{suffix}")).await?;

    // Static preset catalog.
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/presets",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "presets: {}", body_text(&body));
    let presets: serde_json::Value = serde_json::from_slice(&body)?;
    let preset_items = presets["items"].as_array().ok_or("preset items")?;
    assert!(!preset_items.is_empty(), "preset catalog must not be empty");
    assert!(
        preset_items
            .iter()
            .all(|item| item["exporter_slug"].is_string() && item["set_slug"].is_string()),
        "every preset must name an exporter and a metric set slug"
    );

    // Service discovery: token-gated, and an array payload when admitted.
    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/monitoring/sd?group_by=none",
        None,
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = request_with_auth(
        &app,
        "GET",
        "/api/v1/monitoring/sd?group_by=none",
        None,
        None,
        Some("wrong-token"),
    )
    .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{}", body_text(&body));

    let (status, body) = request_with_auth(
        &app,
        "GET",
        "/api/v1/monitoring/sd?group_by=none",
        None,
        None,
        Some("itest-sd-token"),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "sd: {}", body_text(&body));
    assert!(
        serde_json::from_slice::<serde_json::Value>(&body)?.is_array(),
        "HTTP-SD payload must be a JSON array"
    );

    // SD preview: human-facing, requires Read × Monitoring.
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/sd/preview?group_by=none",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "sd preview: {}", body_text(&body));
    let preview: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(preview["groups"].is_array());
    assert!(preview["targets_count"].is_number());

    // Latest metrics with no configured source is a valid empty response.
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/latest",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "latest: {}", body_text(&body));
    let latest: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(latest["configured"].is_boolean());
    assert!(latest["items"].is_array());

    // Host labels are cache-only; a cold cache yields a stale placeholder.
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/host-labels?host_id={host_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "host labels: {}", body_text(&body));
    let labels: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(labels["hosts"].as_array().map(Vec::len), Some(1));

    // Host metrics reflect the global preset metric sets attached to the
    // discovered host; a host with no monitoring config is still a valid
    // `configured` response, not an error.
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/metrics?target_id={host_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "host metrics: {}", body_text(&body));
    let metrics: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(metrics["configured"].is_boolean());
    let first_query = metrics["metrics"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["query"].as_str())
        .ok_or("global preset metric descriptors must be attached to a discovered host")?
        .to_string();

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/metrics",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "target_id is required: {}",
        body_text(&body)
    );

    // History with a descriptor's own query reaches the service; with no
    // configured query source it answers a configured body, not an error.
    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/monitoring/history?target_id={host_id}&query={}&start=0&end=3600",
            encode_query_param(&first_query)
        ),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "history: {}", body_text(&body));
    let history: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(history["configured"].is_boolean());

    // Missing required history parameters is a validation error.
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/history?target_id={host_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    // Target observability resolves the discovered host.
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/targets/{host_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "targets: {}", body_text(&body));
    let target: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        target["target"]["host_id"].as_str(),
        Some(host_id.to_string().as_str())
    );

    // An unknown target is a handled 404.
    let missing = Uuid::now_v7();
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/monitoring/targets/{missing}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));

    Ok(())
}

/// Quick-start composite: create + assign a source in one call, reject an
/// unknown preset slug and a malformed URL, and clean up.
#[tokio::test]
#[ignore]
async fn monitoring_quick_start_composite() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login failed")?;
    let suffix = unique_suffix();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(
            &serde_json::json!({
                "query_url": monitoring_test_url(&suffix),
                "source_type": "prometheus",
                "name": format!("itest quick start {suffix}"),
                "scope_type": "global",
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "quick-start: {}", body_text(&body));
    let created: serde_json::Value = serde_json::from_slice(&body)?;
    let source_id = created["query_source_id"]
        .as_str()
        .ok_or("missing quick-start source id")?;
    let assignment_id = created["query_source_assignment_id"]
        .as_str()
        .ok_or("missing quick-start assignment id")?;
    assert!(
        created["applied_preset"].is_null(),
        "no preset was requested, so none may be applied"
    );

    // The source and its global assignment are now listable.
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/monitoring/query-sources?source_type=prometheus",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let sources: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(
        sources["items"]
            .as_array()
            .ok_or("items array")?
            .iter()
            .any(|item| item["id"].as_str() == Some(source_id)),
        "quick-start source must be listable: {}",
        body_text(&body)
    );

    // Unknown preset slug → 404.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(
            &serde_json::json!({
                "query_url": monitoring_test_url(&suffix),
                "source_type": "prometheus",
                "preset_slug": "definitely-not-a-real-preset",
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body_text(&body));

    // URL userinfo is rejected at the boundary.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(
            &serde_json::json!({
                "query_url": "https://user:secret@monitoring.example.test/api/v1/query",
                "source_type": "prometheus",
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    // Unknown source_type is rejected.
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(
            &serde_json::json!({
                "query_url": monitoring_test_url(&suffix),
                "source_type": "nope",
            })
            .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));

    // Clean up the composite rows.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-source-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    Ok(())
}
