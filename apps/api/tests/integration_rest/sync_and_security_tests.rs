use super::*;

const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";

#[tokio::test]
#[ignore]
async fn put_secrets_value_returns_204() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let body = serde_json::json!({ "value": "secret-value-123" }).to_string();
    let (status, body_bytes) = request(
        &app,
        "PUT",
        &format!("/api/v1/secrets/{}/value", TEST_SECRET_ID),
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body_bytes.is_empty());
}

#[tokio::test]
#[ignore]
async fn delete_secrets_value_returns_204() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    // Ensure value exists first
    let put_body = serde_json::json!({ "value": "to-delete" }).to_string();
    let (_, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/secrets/{}/value", TEST_SECRET_ID),
        Some(&put_body),
        Some(&cookie),
    )
    .await
    .expect("request");

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/secrets/{}/value", TEST_SECRET_ID),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::NO_CONTENT);
}

// --- Auth: logout ---

#[tokio::test]
#[ignore]
async fn post_auth_logout_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, _) = request(&app, "POST", "/api/v1/auth/logout", None, Some(&cookie))
        .await
        .expect("request");

    assert!(
        status == StatusCode::OK || status == StatusCode::NO_CONTENT,
        "expected 200 or 204, got {}",
        status
    );
}

// --- Audit: filters ---

#[tokio::test]
#[ignore]
async fn get_audit_events_with_filters_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/audit/events?limit=5&offset=0",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(json.get("items").is_some());
    assert!(json.get("total").is_some());

    let event_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let marker = format!("audit-filter-{}", unique_suffix());
    sqlx::query(
        r#"
        INSERT INTO audit_event (
            id,
            event_type,
            actor_id,
            target_type,
            target_id,
            metadata,
            created_at
        )
        VALUES ($1, 'mcp.tool.invoked', NULL, 'organization', $2, $3, NOW())
        "#,
    )
    .bind(event_id)
    .bind(target_id)
    .bind(serde_json::json!({
        "decision": "allowed",
        "status": "success",
        "summary": marker,
        "tool": "piping.connection_target.inspect",
        "tool_group": "target_diagnostics",
        "protocol": "ssh",
        "risk": "interactive_session",
    }))
    .execute(&pool)
    .await
    .expect("insert filter audit event");

    let filter_path = format!(
        "/api/v1/audit/events?q={marker}&result=allowed&target_type=organization&target_id={target_id}&mcp_tool=piping.connection_target.inspect&mcp_tool_group=target_diagnostics&mcp_protocol=ssh&mcp_risk=interactive_session&limit=10"
    );
    let (filter_status, filter_body) = request(&app, "GET", &filter_path, None, Some(&cookie))
        .await
        .expect("filtered audit request");

    assert_eq!(filter_status, StatusCode::OK);
    let filter_json: serde_json::Value = serde_json::from_slice(&filter_body).expect("json");
    let filtered_items = filter_json
        .get("items")
        .and_then(|value| value.as_array())
        .expect("filtered audit items");
    assert!(
        filtered_items.iter().any(|item| {
            item.get("id")
                .and_then(|value| value.as_str())
                .is_some_and(|id| id == event_id.to_string())
        }),
        "q/result/target filters must find the inserted audit event"
    );

    let (lookup_status, lookup_body) = request(
        &app,
        "GET",
        "/api/v1/audit/targets?target_type=user&search=admin&limit=10",
        None,
        Some(&cookie),
    )
    .await
    .expect("audit target lookup request");

    assert_eq!(lookup_status, StatusCode::OK);
    let lookup_json: serde_json::Value = serde_json::from_slice(&lookup_body).expect("json");
    let lookup_items = lookup_json
        .get("items")
        .and_then(|value| value.as_array())
        .expect("audit lookup items");
    assert!(
        lookup_items.iter().any(|item| {
            item.get("kind").and_then(|value| value.as_str()) == Some("user")
                && item
                    .get("label")
                    .and_then(|value| value.as_str())
                    .is_some_and(|label| label.contains(TEST_ADMIN_USERNAME))
        }),
        "audit target lookup must search hydrated users"
    );
}

#[tokio::test]
#[ignore]
async fn get_sync_job_report_returns_200_or_404() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/sync/jobs/{NIL_UUID}/report"),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert!(status == StatusCode::OK || status == StatusCode::NOT_FOUND);
}

// --- Connections: create session (400 without valid host) ---

#[tokio::test]
#[ignore]
async fn post_connections_sessions_missing_host_returns_400() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let body = serde_json::json!({
        "target_id": NIL_UUID,
        "protocol": "ssh"
    })
    .to_string();

    let (status, body_bytes) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND);
    let _json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
}

// --- Admin: full CRUD users, groups, roles ---

#[tokio::test]
#[ignore]
async fn admin_users_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let username = format!("test-user-{}", suffix);
    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{}@test.local", username),
        "status": "active",
        "auth_source": "local"
    })
    .to_string();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("user id");

    let update_body = serde_json::json!({
        "username": format!("{}-updated", username),
        "email": format!("{}-updated@test.local", username),
        "status": "inactive",
        "auth_source": "local"
    })
    .to_string();

    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/access-control/users/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn admin_groups_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let name = format!("test-group-{}", suffix);
    let create_body = serde_json::json!({ "name": name }).to_string();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/groups",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created
        .get("id")
        .and_then(|v| v.as_str())
        .expect("group id");

    let update_body = serde_json::json!({ "name": format!("{}-updated", name) }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/access-control/groups/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/groups/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn admin_roles_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let name = format!("test-role-{}", suffix);
    let create_body = serde_json::json!({ "name": name, "description": "test" }).to_string();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/roles",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("role id");

    let update_body = serde_json::json!({
        "name": format!("{}-updated", name),
        "description": "updated"
    })
    .to_string();

    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/access-control/roles/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/roles/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
}

// --- Error cases: 401, 404 ---

#[tokio::test]
#[ignore]
async fn get_admin_users_unauthorized_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/admin/access-control/users",
        None,
        None,
    )
    .await
    .expect("request");

    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "expected 401 or 403, got {}",
        status
    );
}

#[tokio::test]
#[ignore]
async fn get_inventory_tenant_groups_unauthorized_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, _) = request(&app, "GET", "/api/v1/inventory/tenant_groups", None, None)
        .await
        .expect("request");

    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "expected 401 or 403, got {}",
        status
    );
}

#[tokio::test]
#[ignore]
async fn get_inventory_nonexistent_returns_404() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body_bytes) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tenant_groups/{NIL_UUID}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::NOT_FOUND);
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(
        json.get("code").and_then(|value| value.as_str()),
        Some("NOT_FOUND")
    );
}
