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

/// Seed an account the way its owning authentication boundary would: the
/// persisted `auth_source` is the discriminator, `external_id` is the provider
/// subject. Direct insert keeps the fixture independent of the admin create
/// endpoint under test.
async fn seed_directory_user(
    pool: &sqlx::PgPool,
    username: &str,
    auth_source: &str,
    external_id: Option<&str>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, status, auth_source, external_id) \
         VALUES ($1, $2, $3, 'active', $4, $5)",
    )
    .bind(id)
    .bind(username)
    .bind(format!("{username}@test.local"))
    .bind(auth_source)
    .bind(external_id)
    .execute(pool)
    .await
    .expect("seed directory user");
    id
}

async fn count_user_credentials(pool: &sqlx::PgPool, user_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM auth_user_credential WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("count credentials")
}

async fn count_user_update_audits(pool: &sqlx::PgPool, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_event \
         WHERE event_type = 'access_control.user.updated' AND target_id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("count update audits")
}

#[tokio::test]
#[ignore]
async fn admin_update_refuses_accounts_owned_by_an_external_identity_provider() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let pool = build_test_db_pool().await.expect("db pool");
    let suffix = unique_suffix();

    let mut seeded = Vec::new();
    // `oidc`/`ldap` are the providers the product knows; `saml` stands for any
    // source it does not. `LOCAL` is the noncanonical spelling of the local
    // source: `identity_user.auth_source` is free text without a CHECK
    // constraint, so such a row is representable, and it must not acquire local
    // login or edit authority from a case-insensitive comparison.
    for auth_source in ["oidc", "ldap", "saml", "LOCAL"] {
        let username = format!("external-{auth_source}-{suffix}");
        let external_id = format!("ext-{auth_source}-{suffix}");
        let user_id = seed_directory_user(&pool, &username, auth_source, Some(&external_id)).await;
        seeded.push(user_id);

        // The forged `local` source is the attack worth naming: it would have
        // unlocked the route's credential payload and the old unconditional
        // UPDATE. The honest source must be refused just the same.
        let forged_body = serde_json::json!({
            "username": format!("renamed-{auth_source}-{suffix}"),
            "email": format!("renamed-{auth_source}-{suffix}@test.local"),
            "status": "suspended",
            "auth_source": "local",
            "external_id": "spoofed",
            "credential": { "mode": "manual", "password": REST_TEST_LOCAL_PASSWORD }
        })
        .to_string();
        let honest_body = serde_json::json!({
            "username": format!("renamed-{auth_source}-{suffix}"),
            "email": format!("renamed-{auth_source}-{suffix}@test.local"),
            "status": "suspended",
            "auth_source": auth_source,
            "external_id": "spoofed"
        })
        .to_string();
        for (claimed_source, body) in [("local", forged_body), (auth_source, honest_body)] {
            let (status, _) = request(
                &app,
                "PUT",
                &format!("/api/v1/admin/access-control/users/{}", user_id),
                Some(&body),
                Some(&admin_cookie),
            )
            .await
            .expect("request");
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{auth_source} account update claiming {claimed_source} must be refused"
            );
        }

        // The refusal must leave every field, provenance and credential alone and
        // must not record a success audit.
        let row: (String, String, String, String, Option<String>) = sqlx::query_as(
            "SELECT username, email, status, auth_source, external_id \
             FROM identity_user WHERE id = $1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("reload user");
        assert_eq!(row.0, username, "{auth_source} username must be unchanged");
        assert_eq!(
            row.1,
            format!("{username}@test.local"),
            "{auth_source} email must be unchanged"
        );
        assert_eq!(row.2, "active", "{auth_source} status must be unchanged");
        assert_eq!(
            row.3, auth_source,
            "{auth_source} provenance must be unchanged"
        );
        assert_eq!(
            row.4,
            Some(external_id),
            "{auth_source} external identity must be unchanged"
        );
        assert_eq!(
            count_user_credentials(&pool, user_id).await,
            0,
            "{auth_source} account must not gain a local password"
        );
        assert_eq!(
            count_user_update_audits(&pool, user_id).await,
            0,
            "{auth_source} refusal must not record a success audit"
        );
    }

    sqlx::query("DELETE FROM audit_event WHERE target_id = ANY($1)")
        .bind(&seeded)
        .execute(&pool)
        .await
        .expect("cleanup audits");
    sqlx::query("DELETE FROM identity_user WHERE id = ANY($1)")
        .bind(&seeded)
        .execute(&pool)
        .await
        .expect("cleanup users");
}

#[tokio::test]
#[ignore]
async fn admin_update_edits_a_local_user_and_keeps_its_auth_source_fixed() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let pool = build_test_db_pool().await.expect("db pool");
    let suffix = unique_suffix();
    let username = format!("local-edited-{suffix}");
    let user_id = seed_directory_user(&pool, &username, "local", None).await;

    let rename = format!("local-renamed-{suffix}");
    let body = serde_json::json!({
        "username": rename,
        "email": format!("{rename}@test.local"),
        "status": "inactive",
        "auth_source": "local",
        "external_id": ""
    })
    .to_string();
    let (status, response) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/access-control/users/{}", user_id),
        Some(&body),
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(
        json.get("username").and_then(|v| v.as_str()),
        Some(rename.as_str())
    );
    assert_eq!(
        json.get("auth_source").and_then(|v| v.as_str()),
        Some("local")
    );

    let row: (String, String, String) =
        sqlx::query_as("SELECT username, status, auth_source FROM identity_user WHERE id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("reload user");
    assert_eq!(row.0, rename);
    assert_eq!(row.1, "inactive");
    assert_eq!(row.2, "local");
    assert_eq!(count_user_update_audits(&pool, user_id).await, 1);

    // Provenance is not part of the editable surface: a payload moving a local
    // account to an external source is refused and writes nothing.
    let body = serde_json::json!({
        "username": format!("local-moved-{suffix}"),
        "email": format!("local-moved-{suffix}@test.local"),
        "status": "active",
        "auth_source": "oidc",
        "external_id": format!("ext-{suffix}")
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/access-control/users/{}", user_id),
        Some(&body),
        Some(&admin_cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let row: (String, String, String, Option<String>) = sqlx::query_as(
        "SELECT username, status, auth_source, external_id FROM identity_user WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("reload user");
    assert_eq!(row.0, rename, "rejected source change must not rename");
    assert_eq!(row.1, "inactive");
    assert_eq!(row.2, "local");
    assert_eq!(
        row.3.as_deref(),
        Some(""),
        "rejected source change must not write the external subject"
    );
    assert_eq!(count_user_update_audits(&pool, user_id).await, 1);

    sqlx::query("DELETE FROM audit_event WHERE target_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("cleanup audits");
    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("cleanup user");
}

// --- Secrets ---
