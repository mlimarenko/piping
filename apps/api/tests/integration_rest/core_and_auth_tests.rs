use super::*;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_health_returns_ok() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("ok"));
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_api_v1_health_returns_ok() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("ok"));
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_api_v1_live_returns_ok() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/live")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("ok"));
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_api_v1_ready_returns_ok_when_dependencies_up() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state.clone());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/ready")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("ok"));
    let checks = json
        .get("checks")
        .and_then(|v| v.as_object())
        .expect("checks");
    assert_eq!(checks.get("database").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(checks.get("redis").and_then(|v| v.as_str()), Some("ok"));
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_metrics_returns_prometheus_text() {
    let mut state = build_test_app_state().await.expect("app state");
    // is_monitoring_request_authorized is fail-closed since b965f755: with no
    // metrics_token configured, /metrics denies rather than serving publicly.
    Arc::make_mut(&mut state.settings).monitoring.metrics_token =
        Some("test-metrics-token".to_string());
    let app = piping_backend::app::build_router(state);

    // piping_http_requests_total is a label vec: metrics_middleware records it
    // only after a request completes, so the vec has zero samples (and is
    // absent from the text output) until at least one prior request finishes.
    // A request can never observe its own sample, hence the warm-up call.
    let (warmup_status, _) = request(&app, "GET", "/api/v1/health", None, None)
        .await
        .expect("warmup request");
    assert_eq!(warmup_status, StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics?token=test-metrics-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(content_type.starts_with("text/plain"));

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload = String::from_utf8(body.to_vec()).expect("utf8 body");
    assert!(payload.contains("piping_http_requests_total"));
    assert!(payload.contains("piping_build_info"));
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_monitoring_sd_returns_http_sd_payload() {
    let mut state = build_test_app_state().await.expect("app state");
    Arc::make_mut(&mut state.settings).monitoring.sd_token = Some("test-sd-token".to_string());
    let app = piping_backend::app::build_router(state);

    // is_service_discovery_request_authorized only accepts the token via the
    // Authorization: Bearer header (the query token param is unused by design,
    // see monitoring_bearer_token_matches) — unlike /metrics, which accepts both.
    let (status, body) = request_with_auth(
        &app,
        "GET",
        "/api/v1/monitoring/sd?group_by=none&label=env:dev",
        None,
        None,
        Some("test-sd-token"),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(json.is_array(), "http_sd payload must be an array");
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn legacy_monitoring_sd_paths_are_not_supported() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    for path in [
        "/api/v1/monitoring/prometheus/sd?group_by=none",
        "/api/v1/monitoring/victoriametrics/sd?group_by=none",
    ] {
        let (status, _) = request(&app, "GET", path, None, None)
            .await
            .expect("request");
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_scoped_request_allows_in_scope_access() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");

    let payload = serde_json::json!({
        "name": format!("scope-ok-{}", unique_suffix()),
        "scopes": [
            {
                "action": "read",
                "resource_type": "inventory"
            }
        ]
    })
    .to_string();
    let (create_status, create_body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create token");
    assert_eq!(create_status, StatusCode::CREATED);
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let token_value = create_json
        .get("token_value")
        .and_then(|value| value.as_str())
        .expect("token value");

    let (status, _) = request_with_auth(
        &app,
        "GET",
        TOKEN_SCOPE_CHECK_PATH,
        None,
        None,
        Some(token_value),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_scoped_request_denies_out_of_scope_access() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");

    let payload = serde_json::json!({
        "name": format!("scope-deny-{}", unique_suffix()),
        "scopes": [
            {
                "action": "read",
                "resource_type": "connection_target"
            }
        ]
    })
    .to_string();
    let (create_status, create_body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create token");
    assert_eq!(create_status, StatusCode::CREATED);
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let token_value = create_json
        .get("token_value")
        .and_then(|value| value.as_str())
        .expect("token value");

    let (status, body) = request_with_auth(
        &app,
        "GET",
        TOKEN_SCOPE_CHECK_PATH,
        None,
        None,
        Some(token_value),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::FORBIDDEN);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        json.pointer("/code").and_then(|value| value.as_str()),
        Some("TOKEN_SCOPE_DENIED")
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_invalid_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, body) = request_with_auth(
        &app,
        "GET",
        TOKEN_SCOPE_CHECK_PATH,
        None,
        None,
        Some("pat_00000000-0000-7000-8000-000000000000_invalid"),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        json.pointer("/code").and_then(|value| value.as_str()),
        Some("TOKEN_INVALID")
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_revoked_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");

    let payload = serde_json::json!({
        "name": format!("scope-revoke-{}", unique_suffix()),
        "scopes": [
            {
                "action": "read",
                "resource_type": "inventory"
            }
        ]
    })
    .to_string();
    let (create_status, create_body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create token");
    assert_eq!(create_status, StatusCode::CREATED);
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let token_id = create_json
        .get("id")
        .and_then(|value| value.as_str())
        .expect("token id");
    let token_value = create_json
        .get("token_value")
        .and_then(|value| value.as_str())
        .expect("token value");

    let revoke_path = format!("/api/v1/user/settings/api-tokens/{token_id}/revoke");
    let (revoke_status, _) = request(&app, "POST", &revoke_path, None, Some(&cookie))
        .await
        .expect("revoke token");
    assert_eq!(revoke_status, StatusCode::OK);

    let (status, body) = request_with_auth(
        &app,
        "GET",
        TOKEN_SCOPE_CHECK_PATH,
        None,
        None,
        Some(token_value),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        json.pointer("/code").and_then(|value| value.as_str()),
        Some("TOKEN_REVOKED")
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_expired_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");
    let pool = build_test_db_pool().await.expect("db pool");

    let payload = serde_json::json!({
        "name": format!("scope-expired-{}", unique_suffix()),
        "scopes": [
            {
                "action": "read",
                "resource_type": "inventory"
            }
        ],
        "expires_in_seconds": 60
    })
    .to_string();
    let (create_status, create_body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create token");
    assert_eq!(create_status, StatusCode::CREATED);
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let token_id = create_json
        .get("id")
        .and_then(|value| value.as_str())
        .expect("token id");
    let token_value = create_json
        .get("token_value")
        .and_then(|value| value.as_str())
        .expect("token value");

    sqlx::query(
        r#"
        UPDATE auth_user_api_token
        SET expires_at = NOW() - INTERVAL '1 second'
        WHERE id = $1
        "#,
    )
    .bind(Uuid::parse_str(token_id).expect("uuid"))
    .execute(&pool)
    .await
    .expect("expire token");

    let (status, body) = request_with_auth(
        &app,
        "GET",
        TOKEN_SCOPE_CHECK_PATH,
        None,
        None,
        Some(token_value),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        json.pointer("/code").and_then(|value| value.as_str()),
        Some("TOKEN_EXPIRED")
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_revoke_is_idempotent() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");

    let payload = serde_json::json!({
        "name": format!("scope-idempotent-{}", unique_suffix()),
        "scopes": [
            {
                "action": "read",
                "resource_type": "inventory"
            }
        ]
    })
    .to_string();
    let (create_status, create_body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create token");
    assert_eq!(create_status, StatusCode::CREATED);
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let token_id = create_json
        .get("id")
        .and_then(|value| value.as_str())
        .expect("token id");

    let revoke_path = format!("/api/v1/user/settings/api-tokens/{token_id}/revoke");
    let (first_status, first_body) = request(&app, "POST", &revoke_path, None, Some(&cookie))
        .await
        .expect("first revoke");
    assert_eq!(first_status, StatusCode::OK);
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).expect("json");
    assert_eq!(
        first_json
            .get("status")
            .and_then(|value| value.as_str())
            .expect("status"),
        "revoked"
    );
    let first_revoked_at = first_json
        .get("revoked_at")
        .and_then(|value| value.as_str())
        .expect("revoked_at")
        .to_string();

    let (second_status, second_body) = request(&app, "POST", &revoke_path, None, Some(&cookie))
        .await
        .expect("second revoke");
    assert_eq!(second_status, StatusCode::OK);
    let second_json: serde_json::Value = serde_json::from_slice(&second_body).expect("json");
    assert_eq!(
        second_json
            .get("status")
            .and_then(|value| value.as_str())
            .expect("status"),
        "revoked"
    );
    assert_eq!(
        second_json
            .get("revoked_at")
            .and_then(|value| value.as_str())
            .expect("revoked_at"),
        first_revoked_at
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_owner_isolation_enforced_for_revoke_and_list() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");
    let pool = build_test_db_pool().await.expect("db pool");

    let suffix = unique_suffix();
    let user_body = serde_json::json!({
        "username": format!("token-owner-{suffix}"),
        "email": format!("token-owner-{suffix}@test.local"),
        "status": "active",
        "auth_source": "local"
    })
    .to_string();
    let (user_status, user_response) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&user_body),
        Some(&cookie),
    )
    .await
    .expect("create user");
    assert_eq!(user_status, StatusCode::OK);
    let user_json: serde_json::Value = serde_json::from_slice(&user_response).expect("json");
    let other_user_id = Uuid::parse_str(
        user_json
            .get("id")
            .and_then(|value| value.as_str())
            .expect("user id"),
    )
    .expect("user uuid");

    let token_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO auth_user_api_token (
            id,
            user_id,
            name,
            token_prefix,
            token_hash,
            status,
            expires_at,
            created_at,
            updated_at
        )
        VALUES ($1, $2, $3, $4, $5, 'active', NULL, NOW(), NOW())
        "#,
    )
    .bind(token_id)
    .bind(other_user_id)
    .bind(format!("foreign-token-{suffix}"))
    .bind("pat_foreign")
    .bind("deadbeef")
    .execute(&pool)
    .await
    .expect("insert foreign token");

    sqlx::query(
        r#"
        INSERT INTO auth_user_api_token_scope (id, token_id, action, resource_type)
        VALUES ($1, $2, 'READ', 'INVENTORY')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(token_id)
    .execute(&pool)
    .await
    .expect("insert foreign token scope");

    let revoke_path = format!("/api/v1/user/settings/api-tokens/{token_id}/revoke");
    let (revoke_status, _) = request(&app, "POST", &revoke_path, None, Some(&cookie))
        .await
        .expect("revoke foreign token");
    assert_eq!(revoke_status, StatusCode::NOT_FOUND);

    let (list_status, list_body) = request(
        &app,
        "GET",
        "/api/v1/user/settings/api-tokens",
        None,
        Some(&cookie),
    )
    .await
    .expect("list own tokens");
    assert_eq!(list_status, StatusCode::OK);
    let list_json: serde_json::Value = serde_json::from_slice(&list_body).expect("json");
    let contains_foreign = list_json
        .get("items")
        .and_then(|value| value.as_array())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("id")
                    .and_then(|value| value.as_str())
                    .is_some_and(|id| id == token_id.to_string())
            })
        });
    assert!(
        !contains_foreign,
        "foreign token must not appear in owner list"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn user_api_token_create_and_revoke_are_audited_without_raw_token() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");

    let token_name = format!("scope-audit-{}", unique_suffix());
    let payload = serde_json::json!({
        "name": token_name,
        "scopes": [
            {
                "action": "read",
                "resource_type": "inventory"
            }
        ]
    })
    .to_string();
    let (create_status, create_body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create token");
    assert_eq!(create_status, StatusCode::CREATED);
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).expect("json");
    let token_id = create_json
        .get("id")
        .and_then(|value| value.as_str())
        .expect("token id")
        .to_string();
    let raw_token = create_json
        .get("token_value")
        .and_then(|value| value.as_str())
        .expect("token value")
        .to_string();

    let revoke_path = format!("/api/v1/user/settings/api-tokens/{token_id}/revoke");
    let (revoke_status, _) = request(&app, "POST", &revoke_path, None, Some(&cookie))
        .await
        .expect("revoke token");
    assert_eq!(revoke_status, StatusCode::OK);

    let (audit_status, audit_body) = request(
        &app,
        "GET",
        "/api/v1/audit/events?target_type=user_api_token&limit=200",
        None,
        Some(&cookie),
    )
    .await
    .expect("list audit events");
    assert_eq!(audit_status, StatusCode::OK);
    let audit_json: serde_json::Value = serde_json::from_slice(&audit_body).expect("json");
    let items = audit_json
        .get("items")
        .and_then(|value| value.as_array())
        .expect("audit items");

    let created_event = items.iter().find(|item| {
        item.get("event_type")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "access_control.user_api_token.created")
            && item
                .get("target_id")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value == token_id)
    });
    let created_event = created_event.expect("created audit event must exist");

    let revoked_event = items.iter().find(|item| {
        item.get("event_type")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "access_control.user_api_token.revoked")
            && item
                .get("target_id")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value == token_id)
    });
    let revoked_event = revoked_event.expect("revoked audit event must exist");

    assert_eq!(
        created_event
            .get("actor")
            .and_then(|value| value.get("kind"))
            .and_then(|value| value.as_str()),
        Some("user")
    );
    assert_eq!(
        created_event
            .get("actor")
            .and_then(|value| value.get("label"))
            .and_then(|value| value.as_str()),
        Some(TEST_ADMIN_USERNAME)
    );
    assert_eq!(
        created_event
            .get("target")
            .and_then(|value| value.get("kind"))
            .and_then(|value| value.as_str()),
        Some("api_token")
    );
    assert_eq!(
        created_event
            .get("target")
            .and_then(|value| value.get("label"))
            .and_then(|value| value.as_str()),
        Some(token_name.as_str())
    );
    assert_eq!(
        created_event
            .get("target")
            .and_then(|value| value.get("id"))
            .and_then(|value| value.as_str()),
        Some(token_id.as_str())
    );
    assert!(
        created_event
            .get("actor_via")
            .is_some_and(|value| value.is_null()),
        "interactive user token creation must not be shown as a via token"
    );
    assert_eq!(
        revoked_event
            .get("target")
            .and_then(|value| value.get("label"))
            .and_then(|value| value.as_str()),
        Some(token_name.as_str())
    );

    let audit_dump = audit_json.to_string();
    assert!(
        !audit_dump.contains(raw_token.as_str()),
        "audit payload must not contain raw token"
    );
    assert!(
        !audit_dump.contains("token_value"),
        "audit payload must not contain token_value key"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn post_auth_login_success_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let body = serde_json::json!({
        "identifier": TEST_ADMIN_USERNAME,
        "password": TEST_ADMIN_PASSWORD
    })
    .to_string();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("set-cookie").is_some());
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(
        json.get("username").and_then(|v| v.as_str()),
        Some(TEST_ADMIN_USERNAME)
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn post_auth_login_invalid_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let body = serde_json::json!({
        "identifier": "nonexistent",
        "password": "wrongpassword"
    })
    .to_string();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn secrets_value_write_read_delete_persists_encrypted_payload() {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state.clone());

    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let secret_id = Uuid::parse_str(TEST_SECRET_ID).expect("secret id");
    let secret_value = format!("integration-secret-{}", unique_suffix());

    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/secrets/{}/value", secret_id),
        Some(&serde_json::json!({ "value": secret_value.clone() }).to_string()),
        Some(&cookie),
    )
    .await
    .expect("set secret");
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty());

    let stored: String = sqlx::query_scalar(
        "SELECT encrypted_value FROM secrets_store WHERE secret_id = $1 LIMIT 1",
    )
    .bind(secret_id)
    .fetch_one(&db_pool)
    .await
    .expect("stored secret");
    assert_ne!(stored, secret_value, "stored payload must be encrypted");
    assert!(
        stored.starts_with("enc:v1:"),
        "stored payload must use versioned envelope"
    );

    let resolved = state
        .services
        .secret_store
        .get_secret(secret_id)
        .await
        .expect("get secret");
    assert_eq!(resolved, secret_value);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/secrets/{}/value", secret_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete secret");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_auth_me_with_cookie_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body_bytes) = request(&app, "GET", "/api/v1/auth/me", None, Some(&cookie))
        .await
        .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(
        json.get("username").and_then(|v| v.as_str()),
        Some(TEST_ADMIN_USERNAME)
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_auth_me_without_cookie_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, _body) = request(&app, "GET", "/api/v1/auth/me", None, None)
        .await
        .expect("request");

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn post_session_refresh_with_cookie_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body_bytes) = request(
        &app,
        "POST",
        "/api/v1/auth/session-refresh",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(
        json.get("status").and_then(|v| v.as_str()),
        Some("refreshed")
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn post_session_refresh_without_cookie_returns_401() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, body_bytes) = request(&app, "POST", "/api/v1/auth/session-refresh", None, None)
        .await
        .expect("request");

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("expired"));
    assert_eq!(
        json.get("redirect").and_then(|v| v.as_str()),
        Some("/auth/login")
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_connections_sessions_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/connections/sessions",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(json.is_array() || json.get("items").is_some());
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_connections_templates_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/connections/templates",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(json.is_array() || json.get("items").is_some());
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_audit_events_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(&app, "GET", "/api/v1/audit/events", None, Some(&cookie))
        .await
        .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(json.get("items").is_some());
    assert!(json.get("total").is_some());
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_admin_users_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/admin/access-control/users",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(
        json.get("items")
            .map(|value| value.is_array())
            .unwrap_or(false)
    );
    assert!(json.get("total").is_some());
    assert!(json.get("limit").is_some());
    assert!(json.get("offset").is_some());
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_admin_groups_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/admin/access-control/groups",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(
        json.get("items")
            .map(|value| value.is_array())
            .unwrap_or(false)
    );
    assert!(json.get("total").is_some());
    assert!(json.get("limit").is_some());
    assert!(json.get("offset").is_some());
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn get_admin_roles_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/admin/access-control/roles",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(
        json.get("items")
            .map(|value| value.is_array())
            .unwrap_or(false)
    );
    assert!(json.get("total").is_some());
    assert!(json.get("limit").is_some());
    assert!(json.get("offset").is_some());
}
