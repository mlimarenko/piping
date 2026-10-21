use super::*;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored setup_wizard_
async fn setup_wizard_readiness_returns_expected_shape() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/setup/wizard/readiness",
        None,
        Some(&cookie),
    )
    .await
    .expect("get setup wizard readiness");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("readiness json");
    assert!(
        json.get("has_hosts")
            .and_then(|value| value.as_bool())
            .is_some()
    );
    assert!(
        json.get("has_connection_configs")
            .and_then(|value| value.as_bool())
            .is_some()
    );
    assert!(
        json.get("user_has_credential_sets")
            .and_then(|value| value.as_bool())
            .is_some()
    );
    assert!(
        json.get("user_has_connected")
            .and_then(|value| value.as_bool())
            .is_some()
    );
    assert_eq!(
        json.get("user_is_admin").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(
        json.get("wizard_status").is_some(),
        "wizard_status field must be present"
    );
    assert!(
        json.get("wizard_current_step").is_some(),
        "wizard_current_step field must be present"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored setup_wizard_
async fn setup_wizard_readiness_admin_flag_honors_token_scope() {
    // The `user_is_admin` readiness flag must be reported through the SAME
    // guarded authorization an admin action uses, so it honors the API-token
    // scope ceiling. A full-admin user minting a PAT scoped to `read:inventory`
    // only (no `manage_policies`) must read as NOT admin through that token —
    // otherwise the flag would over-report admin for a deliberately narrowed
    // credential (the null-context `evaluate` path this replaced skipped the
    // scope gate entirely).
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("session cookie");

    // Baseline: the admin session itself still reports admin.
    let (session_status, session_body) = request(
        &app,
        "GET",
        "/api/v1/setup/wizard/readiness",
        None,
        Some(&cookie),
    )
    .await
    .expect("readiness via session");
    assert_eq!(session_status, StatusCode::OK);
    let session_json: serde_json::Value =
        serde_json::from_slice(&session_body).expect("readiness json");
    assert_eq!(
        session_json.get("user_is_admin").and_then(|v| v.as_bool()),
        Some(true),
        "admin session must report user_is_admin = true"
    );

    // Mint a PAT scoped away from `manage_policies`.
    let payload = serde_json::json!({
        "name": format!("wizard-scope-{}", unique_suffix()),
        "scopes": [{ "action": "read", "resource_type": "inventory" }]
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
    .expect("create scoped token");
    assert_eq!(create_status, StatusCode::CREATED);
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).expect("token json");
    let token_value = create_json
        .get("token_value")
        .and_then(|v| v.as_str())
        .expect("token value");

    // Through the scoped token, the same user must NOT read as admin.
    let (token_status, token_body) = request_with_auth(
        &app,
        "GET",
        "/api/v1/setup/wizard/readiness",
        None,
        None,
        Some(token_value),
    )
    .await
    .expect("readiness via scoped token");
    assert_eq!(token_status, StatusCode::OK);
    let token_json: serde_json::Value =
        serde_json::from_slice(&token_body).expect("readiness json");
    assert_eq!(
        token_json.get("user_is_admin").and_then(|v| v.as_bool()),
        Some(false),
        "a token scoped away from manage_policies must report user_is_admin = false"
    );
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored setup_wizard_
async fn setup_wizard_progress_lifecycle_respects_merge_complete_and_reset_semantics() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/setup/wizard/progress",
        None,
        Some(&cookie),
    )
    .await
    .expect("get missing progress");
    assert_eq!(status, StatusCode::NOT_FOUND);

    let first_progress_payload = serde_json::json!({
        "current_step": "inventory",
        "step_data": {
            "inventory_checked": true
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/setup/wizard/progress",
        Some(&first_progress_payload),
        Some(&cookie),
    )
    .await
    .expect("create progress");
    assert_eq!(status, StatusCode::OK);
    let first_progress: serde_json::Value = serde_json::from_slice(&body).expect("progress json");
    assert_eq!(
        first_progress
            .get("current_step")
            .and_then(|value| value.as_str()),
        Some("inventory")
    );
    assert_eq!(
        first_progress
            .get("status")
            .and_then(|value| value.as_str()),
        Some("active")
    );

    let second_progress_payload = serde_json::json!({
        "current_step": "org_credentials",
        "step_data": {
            "protocol_configured": "ssh"
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/setup/wizard/progress",
        Some(&second_progress_payload),
        Some(&cookie),
    )
    .await
    .expect("update progress");
    assert_eq!(status, StatusCode::OK);
    let second_progress: serde_json::Value =
        serde_json::from_slice(&body).expect("progress update json");
    let step_data = second_progress
        .get("step_data")
        .and_then(|value| value.as_object())
        .expect("step_data object");
    assert_eq!(
        step_data
            .get("inventory_checked")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        step_data
            .get("protocol_configured")
            .and_then(|value| value.as_str()),
        Some("ssh")
    );

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/setup/wizard/complete",
        None,
        Some(&cookie),
    )
    .await
    .expect("complete wizard");
    assert_eq!(status, StatusCode::OK);
    let completed: serde_json::Value = serde_json::from_slice(&body).expect("complete json");
    assert_eq!(
        completed.get("status").and_then(|value| value.as_str()),
        Some("completed")
    );
    assert!(
        completed
            .get("completed_at")
            .and_then(|value| value.as_str())
            .is_some(),
        "completed response must include completed_at"
    );

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/setup/wizard/reset",
        None,
        Some(&cookie),
    )
    .await
    .expect("reset after complete");
    assert_eq!(status, StatusCode::OK);
    let reset_after_complete: serde_json::Value =
        serde_json::from_slice(&body).expect("reset json");
    assert_eq!(
        reset_after_complete
            .get("status")
            .and_then(|value| value.as_str()),
        Some("active")
    );
    assert_eq!(
        reset_after_complete
            .get("current_step")
            .and_then(|value| value.as_str()),
        None
    );

    let third_progress_payload = serde_json::json!({
        "current_step": "test_connection",
        "step_data": {
            "test_host_id": Uuid::now_v7().to_string()
        }
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/setup/wizard/progress",
        Some(&third_progress_payload),
        Some(&cookie),
    )
    .await
    .expect("set post-reset progress");
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/setup/wizard/dismiss",
        None,
        Some(&cookie),
    )
    .await
    .expect("dismiss wizard");
    assert_eq!(status, StatusCode::OK);
    let dismissed: serde_json::Value = serde_json::from_slice(&body).expect("dismiss json");
    assert_eq!(
        dismissed.get("status").and_then(|value| value.as_str()),
        Some("dismissed")
    );

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/setup/wizard/reset",
        None,
        Some(&cookie),
    )
    .await
    .expect("reset after dismiss");
    assert_eq!(status, StatusCode::OK);
    let reset_after_dismiss: serde_json::Value = serde_json::from_slice(&body).expect("reset json");
    assert_eq!(
        reset_after_dismiss
            .get("status")
            .and_then(|value| value.as_str()),
        Some("active")
    );
    assert_eq!(
        reset_after_dismiss
            .get("current_step")
            .and_then(|value| value.as_str()),
        Some("test_connection"),
        "dismiss reset must preserve current_step"
    );
}
