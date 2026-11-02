use super::*;

#[tokio::test]
#[ignore]
async fn get_admin_role_assignments_returns_200() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/admin/access-control/role-assignments",
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
#[ignore]
async fn admin_role_assignments_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    // Create a role for assignment
    let role_body =
        serde_json::json!({ "name": "test-role-for-assignment", "description": "test" })
            .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/roles",
        Some(&role_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let role: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let role_id = role.get("id").and_then(|v| v.as_str()).expect("role id");

    // Create user for assignment
    let user_body = serde_json::json!({
        "username": "test-user-ra",
        "email": "ra@test.local",
        "status": "active",
        "auth_source": "local"
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&user_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let user: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let user_id = user.get("id").and_then(|v| v.as_str()).expect("user id");

    let assign_body = serde_json::json!({
        "user_id": user_id,
        "role_id": role_id
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/role-assignments",
        Some(&assign_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let assignment: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let assignment_id = assignment
        .get("id")
        .and_then(|v| v.as_str())
        .expect("assignment id");

    let (status, _) = request(
        &app,
        "DELETE",
        &format!(
            "/api/v1/admin/access-control/role-assignments/{}",
            assignment_id
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    // Clean up
    let (_, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", user_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    let (_, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/roles/{}", role_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
}

#[tokio::test]
#[ignore]
async fn admin_create_local_user_manual_password_and_login() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let username = format!("manual-pass-user-{}", suffix);
    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{}@test.local", username),
        "status": "active",
        "auth_source": "local",
        "credential": {
            "mode": "manual",
            "password": REST_TEST_LOCAL_PASSWORD
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let user_id = json.get("id").and_then(|v| v.as_str()).expect("user id");

    let user_cookie = login_and_get_cookie(&app, &username, REST_TEST_LOCAL_PASSWORD).await;
    assert!(user_cookie.is_some(), "manual password login must succeed");

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", user_id),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn admin_create_local_user_generated_password_and_login() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let username = format!("generated-pass-user-{}", suffix);
    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{}@test.local", username),
        "status": "active",
        "auth_source": "local",
        "credential": {
            "mode": "generate"
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let user_id = json.get("id").and_then(|v| v.as_str()).expect("user id");
    let generated_password = json
        .get("generated_password")
        .and_then(|value| value.as_str())
        .expect("generated_password");

    let user_cookie = login_and_get_cookie(&app, &username, generated_password).await;
    assert!(
        user_cookie.is_some(),
        "generated password login must succeed"
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", user_id),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn admin_rotate_user_password_response_is_metadata_only() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let username = format!("rotate-pass-user-{}", suffix);
    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{}@test.local", username),
        "status": "active",
        "auth_source": "local",
        "credential": {
            "mode": "manual",
            "password": REST_TEST_LOCAL_PASSWORD
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let user_id = json.get("id").and_then(|v| v.as_str()).expect("user id");

    let rotate_body = serde_json::json!({
        "mode": "manual",
        "password": "Zz123456"
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/access-control/users/{}/password", user_id),
        Some(&rotate_body),
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        json.get("user_id").and_then(|value| value.as_str()),
        Some(user_id)
    );
    assert!(
        json.get("password_changed_at").is_some(),
        "must include password_changed_at"
    );
    assert!(
        json.get("generated_password").is_none(),
        "manual rotation must not return generated_password"
    );
    assert!(
        json.get("password_hash").is_none(),
        "must not return password_hash"
    );
    assert!(json.get("secret_id").is_none(), "must not return secret_id");

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", user_id),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn admin_create_local_user_rejects_invalid_password() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let username = format!("bad-pass-user-{}", suffix);
    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{}@test.local", username),
        "status": "active",
        "auth_source": "local",
        "credential": {
            "mode": "manual",
            "password": "short1"
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        json.get("code").and_then(|value| value.as_str()),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
#[ignore]
async fn admin_role_grants_and_filtered_lists() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let role_body = serde_json::json!({
        "name": format!("role-grants-{}", suffix),
        "description": "role grants test"
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/roles",
        Some(&role_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let role_json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let role_id = role_json
        .get("id")
        .and_then(|v| v.as_str())
        .expect("role id");

    let user1_name = format!("grants-user1-{}", suffix);
    let user2_name = format!("grants-user2-{}", suffix);
    let user1_body = serde_json::json!({
        "username": user1_name,
        "email": format!("{}@test.local", user1_name),
        "status": "active",
        "auth_source": "local"
    })
    .to_string();
    let user2_body = serde_json::json!({
        "username": user2_name,
        "email": format!("{}@test.local", user2_name),
        "status": "active",
        "auth_source": "local"
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&user1_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let user1_json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let user1_id = user1_json
        .get("id")
        .and_then(|v| v.as_str())
        .expect("user1 id");

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&user2_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let user2_json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let user2_id = user2_json
        .get("id")
        .and_then(|v| v.as_str())
        .expect("user2 id");

    let assign1_body = serde_json::json!({ "user_id": user1_id, "role_id": role_id }).to_string();
    let assign2_body = serde_json::json!({ "user_id": user2_id, "role_id": role_id }).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/role-assignments",
        Some(&assign1_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let assign1_json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let assign1_id = assign1_json
        .get("id")
        .and_then(|v| v.as_str())
        .expect("assign1 id");

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/role-assignments",
        Some(&assign2_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let assign2_json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let assign2_id = assign2_json
        .get("id")
        .and_then(|v| v.as_str())
        .expect("assign2 id");

    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/admin/access-control/role-assignments?role_id={}&user_id={}",
            role_id, user1_id
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let filtered_assignments: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        filtered_assignments
            .get("items")
            .and_then(|v| v.as_array())
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        filtered_assignments.get("total").and_then(|v| v.as_u64()),
        Some(1)
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/admin/access-control/roles/{}/grants", role_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let grants_json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        grants_json
            .get("assignments")
            .and_then(|v| v.as_array())
            .map(Vec::len),
        Some(2)
    );

    let _ = request(
        &app,
        "DELETE",
        &format!(
            "/api/v1/admin/access-control/role-assignments/{}",
            assign1_id
        ),
        None,
        Some(&cookie),
    )
    .await;
    let _ = request(
        &app,
        "DELETE",
        &format!(
            "/api/v1/admin/access-control/role-assignments/{}",
            assign2_id
        ),
        None,
        Some(&cookie),
    )
    .await;
    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", user1_id),
        None,
        Some(&cookie),
    )
    .await;
    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{}", user2_id),
        None,
        Some(&cookie),
    )
    .await;
    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/roles/{}", role_id),
        None,
        Some(&cookie),
    )
    .await;
}

// --- Secrets ---
