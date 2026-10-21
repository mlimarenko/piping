use super::*;

#[path = "../support/license_http_contract.rs"]
mod license_http_contract;

struct LicenseAdminHarness {
    app: Router,
    db_pool: sqlx::PgPool,
    admin_cookie: String,
}

async fn build_default_license_admin_harness()
-> Result<LicenseAdminHarness, Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state()
        .await
        .map_err(|e| format!("app state: {e}"))?;
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "admin login".to_string())?;
    Ok(LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    })
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_default_inactive_limit_blocks_eleventh_login()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    } = build_default_license_admin_harness().await?;

    reset_license_tables(&db_pool).await?;

    // Admin consumes seat #1 here.
    let admin_relogin = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD).await;
    assert!(admin_relogin.is_some(), "admin relogin must succeed");

    let mut created_user_ids = Vec::new();
    for idx in 0..LICENSE_LOGIN_ATTEMPTS_AT_BOUNDARY {
        let suffix = unique_suffix();
        let username = format!("license-seat-user-{}-{}", idx, suffix);
        let user_id =
            create_local_user(&app, &admin_cookie, &username, REST_TEST_LOCAL_PASSWORD).await?;
        created_user_ids.push(user_id.clone());

        let (status, body) = login_local(&app, &username, REST_TEST_LOCAL_PASSWORD).await?;
        if idx < LICENSE_LOGIN_ATTEMPTS_AT_BOUNDARY - 1 {
            assert_eq!(status, StatusCode::OK, "user {} must login", idx + 1);
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "11th login must be blocked");
            let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(
                payload.get("code").and_then(|value| value.as_str()),
                Some("LICENSE_SEAT_LIMIT_EXCEEDED")
            );
            let active_sessions: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM auth_session WHERE user_id = $1::uuid AND revoked_at IS NULL",
            )
            .bind(&user_id)
            .fetch_one(&db_pool)
            .await
            .expect("count active sessions for denied login");
            assert_eq!(
                active_sessions, 0,
                "license-denied local login must not leave an active session"
            );
        }
    }

    for user_id in created_user_ids {
        let _ = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/users/{user_id}"),
            None,
            Some(&admin_cookie),
        )
        .await
        .expect("delete user");
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_state_without_activation_reports_default_free_tier_limit()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);

    reset_license_tables(&db_pool).await?;

    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login before free tier status");

    let (status, body) = request(&app, "GET", LICENSE_STATUS_PATH, None, Some(&cookie))
        .await
        .expect("free tier status");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("status").and_then(|value| value.as_str()),
        Some("inactive")
    );
    assert_eq!(
        payload.get("seatLimit").and_then(|value| value.as_i64()),
        Some(LICENSE_MAX_SEATS_COMMUNITY as i64)
    );
    assert_eq!(
        payload.get("version").and_then(|value| value.as_i64()),
        Some(0)
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_default_upgrade_preserves_five_assignments_without_activation()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let db_pool = state.db_pool.clone();
    reset_license_tables(&db_pool).await?;
    let app = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let mut user_ids = Vec::new();
    for idx in 0..4 {
        let username = format!("community-existing-{idx}-{}", unique_suffix());
        user_ids.push(create_local_user(&app, &cookie, &username, REST_TEST_LOCAL_PASSWORD).await?);
        let (status, _) = login_local(&app, &username, REST_TEST_LOCAL_PASSWORD).await?;
        assert_eq!(status, StatusCode::OK);
    }
    let existing: Vec<(Uuid, Uuid, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, user_id, assigned_at FROM license_seat_assignment WHERE revoked_at IS NULL ORDER BY id",
    )
    .fetch_all(&db_pool)
    .await?;
    assert_eq!(existing.len(), 5);

    // Rebuild against the persisted pre-upgrade ledger, without resetting its data.
    let settings = state.settings.clone();
    let app =
        piping_backend::app::build_router(rebuild_state_with_settings(state, settings).await?);
    let (status, body) = request(&app, "GET", LICENSE_STATUS_PATH, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(payload["seatLimit"], LICENSE_MAX_SEATS_COMMUNITY);
    assert_eq!(payload["seatsUsed"], 5);
    let preserved: Vec<(Uuid, Uuid, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, user_id, assigned_at FROM license_seat_assignment WHERE revoked_at IS NULL ORDER BY id",
    )
    .fetch_all(&db_pool)
    .await?;
    assert_eq!(
        preserved, existing,
        "upgrade must preserve existing assignments"
    );

    for idx in 0..5 {
        let username = format!("community-added-{idx}-{}", unique_suffix());
        user_ids.push(create_local_user(&app, &cookie, &username, REST_TEST_LOCAL_PASSWORD).await?);
        let (status, _) = login_local(&app, &username, REST_TEST_LOCAL_PASSWORD).await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "new community capacity must be usable"
        );
    }
    let state_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM license_state")
        .fetch_one(&db_pool)
        .await?;
    assert_eq!(
        state_count, 0,
        "community capacity must not fabricate activation state"
    );
    for user_id in user_ids {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/users/{user_id}"),
            None,
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_expired_entitlement_blocks_new_login()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    } = build_default_license_admin_harness().await?;

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "expired",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() - chrono::Duration::minutes(LICENSE_EXPIRED_MINUTES),
    )
    .await?;

    let username = format!("expired-seat-user-{}", unique_suffix());
    let user_id =
        create_local_user(&app, &admin_cookie, &username, REST_TEST_LOCAL_PASSWORD).await?;

    let (status, body) = login_local(&app, &username, REST_TEST_LOCAL_PASSWORD).await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("code").and_then(|value| value.as_str()),
        Some("LICENSE_REFRESH_REQUIRED")
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("delete user");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_degraded_state_allows_login_until_expiry()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    } = build_default_license_admin_harness().await?;

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "degraded",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_GRACE),
    )
    .await?;

    let username = format!("degraded-seat-user-{}", unique_suffix());
    let user_id =
        create_local_user(&app, &admin_cookie, &username, REST_TEST_LOCAL_PASSWORD).await?;
    let (status, _body) = login_local(&app, &username, REST_TEST_LOCAL_PASSWORD).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "degraded non-expired license should allow login"
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("delete user");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_grace_state_blocks_new_login_and_writes_ledger_event()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    } = build_default_license_admin_harness().await?;

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "grace",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_GRACE),
    )
    .await?;

    let username = format!("grace-seat-user-{}", unique_suffix());
    let user_id =
        create_local_user(&app, &admin_cookie, &username, REST_TEST_LOCAL_PASSWORD).await?;
    let (status, body) = login_local(&app, &username, REST_TEST_LOCAL_PASSWORD).await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("code").and_then(|value| value.as_str()),
        Some("LICENSE_GRACE_ENDED")
    );

    let ledger_event_type: Option<String> = sqlx::query_scalar(
        r#"
        SELECT event_type
        FROM license_seat_ledger
        WHERE instance_id = $1
          AND reason_code = 'login_gate'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(LICENSE_INSTANCE_ID)
    .fetch_optional(&db_pool)
    .await
    .expect("query seat ledger");
    assert_eq!(ledger_event_type.as_deref(), Some("seat_denied"));

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("delete user");
    Ok(())
}

async fn reset_license_tables(
    pool: &sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("DELETE FROM license_seat_ledger")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_seat_ledger: {e}"))?;
    sqlx::query("DELETE FROM license_seat_assignment")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_seat_assignment: {e}"))?;
    sqlx::query("DELETE FROM license_state")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_state: {e}"))?;
    sqlx::query("DELETE FROM license_instance_identity")
        .execute(pool)
        .await
        .map_err(|e| format!("delete license_instance_identity: {e}"))?;
    Ok(())
}

async fn latest_license_instance_id(
    pool: &sqlx::PgPool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT instance_id
        FROM license_state
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("latest license instance id: {e}"))?)
}

async fn upsert_license_state(
    pool: &sqlx::PgPool,
    status: &str,
    max_seats: i32,
    entitlement_expires_at: chrono::DateTime<Utc>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let entitlement_version = format!("ver-{}", unique_suffix());
    let entitlement_jwt = build_test_entitlement_jwt(
        LICENSE_INSTANCE_ID,
        max_seats,
        entitlement_expires_at,
        &entitlement_version,
    )?;
    sqlx::query(
        r#"
        INSERT INTO license_state (
            instance_id,
            status,
            max_seats,
            entitlement_jwt,
            entitlement_version,
            entitlement_expires_at,
            activation_token_encrypted,
            last_sync_at,
            last_sync_error,
            created_at,
            updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), NULL, NOW(), NOW())
        ON CONFLICT (instance_id) DO UPDATE
        SET
            status = EXCLUDED.status,
            max_seats = EXCLUDED.max_seats,
            entitlement_jwt = EXCLUDED.entitlement_jwt,
            entitlement_version = EXCLUDED.entitlement_version,
            entitlement_expires_at = EXCLUDED.entitlement_expires_at,
            activation_token_encrypted = EXCLUDED.activation_token_encrypted,
            last_sync_at = EXCLUDED.last_sync_at,
            last_sync_error = EXCLUDED.last_sync_error,
            updated_at = NOW()
        "#,
    )
    .bind(LICENSE_INSTANCE_ID)
    .bind(status)
    .bind(max_seats)
    .bind(entitlement_jwt)
    .bind(entitlement_version)
    .bind(entitlement_expires_at)
    .bind("legacy-activation-token")
    .execute(pool)
    .await
    .map_err(|e| format!("upsert license_state: {e}"))?;
    Ok(())
}

fn build_test_entitlement_jwt(
    instance_fingerprint: &str,
    max_seats: i32,
    entitlement_expires_at: chrono::DateTime<Utc>,
    entitlement_version: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    ensure_jsonwebtoken_provider_installed();
    let now = Utc::now();
    let issued_at = if entitlement_expires_at <= now {
        entitlement_expires_at - chrono::Duration::seconds(LICENSE_CLOCK_SKEW_SECONDS)
    } else {
        now - chrono::Duration::seconds(LICENSE_CLOCK_SKEW_SECONDS)
    };
    let claims = MockClaims {
        iss: "piping-com-backend".to_string(),
        aud: "piping-instance".to_string(),
        sub: "org:test".to_string(),
        iat: issued_at.timestamp(),
        nbf: issued_at.timestamp(),
        exp: entitlement_expires_at.timestamp(),
        organization_id: LICENSE_MOCK_ORGANIZATION_ID.to_string(),
        instance_fingerprint: instance_fingerprint.to_string(),
        max_seats,
        entitlement_version: entitlement_version.to_string(),
    };
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some("k1".to_string());
    let encoding_key = EncodingKey::from_ed_pem(TEST_PRIVATE_KEY_1.as_bytes())
        .map_err(|e| format!("private key: {e}"))?;
    Ok(encode(&header, &claims, &encoding_key)
        .map_err(|e| format!("encode test entitlement: {e}"))?)
}

async fn create_local_user(
    app: &Router,
    admin_cookie: &str,
    username: &str,
    password: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{username}@test.local"),
        "status": "active",
        "auth_source": "local",
        "credential": {
            "mode": "manual",
            "password": password
        }
    })
    .to_string();
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&create_body),
        Some(admin_cookie),
    )
    .await
    .map_err(|e| format!("create user: {e}"))?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("json: {e}"))?;
    Ok(payload
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "user id".to_string())?
        .to_string())
}

async fn login_local(
    app: &Router,
    username: &str,
    password: &str,
) -> Result<(StatusCode, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({
        "identifier": username,
        "password": password
    })
    .to_string();
    Ok(
        request(app, "POST", "/api/v1/auth/login", Some(&body), None)
            .await
            .map_err(|e| format!("login request: {e}"))?,
    )
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_refresh_marks_degraded_on_stale_entitlement()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ensure_jsonwebtoken_provider_installed();
    let mock = start_mock_license_server().await?;
    mock.mode.store(1, Ordering::SeqCst); // key1 valid

    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let state = with_license_settings(state, &mock.base_url).await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    reset_license_tables(&db_pool).await?;

    let activate_body = serde_json::json!({
        "activationToken": "integration-token",
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        LICENSE_ACTIVATE_PATH,
        Some(&activate_body),
        Some(&cookie),
    )
    .await
    .expect("activate");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    mock.mode.store(3, Ordering::SeqCst); // expired token
    let (status, body) = request(
        &app,
        "POST",
        LICENSE_REFRESH_PATH,
        Some("{}"),
        Some(&cookie),
    )
    .await
    .expect("refresh");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let error_code = payload.get("code").and_then(|value| value.as_str());
    assert!(
        matches!(error_code, Some("VALIDATION_ERROR") | Some("400")),
        "unexpected stale-entitlement error code: {error_code:?}"
    );

    let instance_id = latest_license_instance_id(&db_pool).await?;
    let row =
        sqlx::query("SELECT status, last_sync_error FROM license_state WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_one(&db_pool)
            .await
            .expect("license row");
    let status: String = sqlx::Row::try_get(&row, "status").expect("status");
    let last_sync_error: Option<String> =
        sqlx::Row::try_get(&row, "last_sync_error").expect("last_sync_error");
    assert_eq!(status, "degraded");
    assert!(
        last_sync_error
            .unwrap_or_default()
            .contains("Entitlement JWT verification failed"),
        "stale entitlement must persist sync error"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_activate_returns_canonical_entitlement_state_and_backfills_active_session_seat()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ensure_jsonwebtoken_provider_installed();
    let mock = start_mock_license_server().await?;
    mock.mode.store(1, Ordering::SeqCst);

    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let state = with_license_settings(state, &mock.base_url).await?;
    let app = piping_backend::app::build_router(state);

    reset_license_tables(&db_pool).await?;
    sqlx::query("DELETE FROM auth_session")
        .execute(&db_pool)
        .await
        .expect("delete auth_session");

    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login before activation");

    let activate_body = serde_json::json!({
        "activationToken": "integration-token",
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        LICENSE_ACTIVATE_PATH,
        Some(&activate_body),
        Some(&cookie),
    )
    .await
    .expect("activate");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("status").and_then(|value| value.as_str()),
        Some("active")
    );
    assert_eq!(
        payload.get("seatLimit").and_then(|value| value.as_i64()),
        Some(LICENSE_MAX_SEATS_SIGNED as i64),
    );
    assert!(
        payload
            .get("validFrom")
            .and_then(|value| value.as_str())
            .is_some(),
        "activation response must include validFrom",
    );
    assert!(
        payload
            .get("validUntil")
            .and_then(|value| value.as_str())
            .is_some(),
        "activation response must include validUntil",
    );
    assert!(
        payload
            .get("version")
            .and_then(|value| value.as_i64())
            .is_some(),
        "activation response must include version",
    );

    let assignments_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM license_seat_assignment
        WHERE revoked_at IS NULL
        "#,
    )
    .fetch_one(&db_pool)
    .await
    .expect("assignments count");
    assert_eq!(
        assignments_count, 1,
        "existing authenticated user must consume one seat after activation",
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_activate_ignores_client_supplied_instance_fingerprint()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ensure_jsonwebtoken_provider_installed();
    let mock = start_mock_license_server().await?;
    mock.mode.store(1, Ordering::SeqCst);

    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let state = with_license_settings(state, &mock.base_url).await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    reset_license_tables(&db_pool).await?;

    let activate_body = serde_json::json!({
        "activationToken": "integration-token",
        "instanceFingerprint": LICENSE_INSTANCE_A
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        LICENSE_ACTIVATE_PATH,
        Some(&activate_body),
        Some(&cookie),
    )
    .await
    .expect("activate");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let instance_id = latest_license_instance_id(&db_pool).await?;
    assert_ne!(instance_id, LICENSE_INSTANCE_A);
    assert!(instance_id.starts_with("piping-"));
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_state_returns_canonical_projection_for_existing_license()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);

    reset_license_tables(&db_pool).await?;
    sqlx::query("DELETE FROM auth_session")
        .execute(&db_pool)
        .await
        .expect("delete auth_session");

    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login before license state");

    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    let (status, body) = request(&app, "GET", LICENSE_STATUS_PATH, None, Some(&cookie))
        .await
        .expect("state");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("status").and_then(|value| value.as_str()),
        Some("active"),
        "state endpoint must return canonical active lifecycle",
    );
    assert_eq!(
        payload.get("seatLimit").and_then(|value| value.as_i64()),
        Some(LICENSE_MAX_SEATS_SIGNED as i64),
    );
    assert!(
        payload
            .get("validFrom")
            .and_then(|value| value.as_str())
            .is_some(),
        "state endpoint must expose validFrom",
    );
    assert!(
        payload
            .get("validUntil")
            .and_then(|value| value.as_str())
            .is_some(),
        "state endpoint must expose validUntil",
    );
    assert!(
        payload
            .get("version")
            .and_then(|value| value.as_i64())
            .is_some(),
        "state endpoint must expose version",
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_login_and_status_ignore_tampered_entitlement_columns()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    } = build_default_license_admin_harness().await?;

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_CONCURRENT_BOUNDARY,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    sqlx::query(
        r#"
        UPDATE license_state
        SET max_seats = 999,
            entitlement_expires_at = NOW() + INTERVAL '100 years',
            updated_at = NOW()
        WHERE instance_id = $1
        "#,
    )
    .bind(LICENSE_INSTANCE_ID)
    .execute(&db_pool)
    .await
    .expect("tamper cached entitlement columns");

    let (status, body) = request(&app, "GET", LICENSE_STATUS_PATH, None, Some(&admin_cookie))
        .await
        .expect("state");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("seatLimit").and_then(|value| value.as_i64()),
        Some(LICENSE_MAX_SEATS_CONCURRENT_BOUNDARY as i64),
        "status must project the signed JWT seat limit, not DB cache columns",
    );

    let username1 = format!("tamper-seat-user-a-{}", unique_suffix());
    let username2 = format!("tamper-seat-user-b-{}", unique_suffix());
    let user_id1 =
        create_local_user(&app, &admin_cookie, &username1, REST_TEST_LOCAL_PASSWORD).await?;
    let user_id2 =
        create_local_user(&app, &admin_cookie, &username2, REST_TEST_LOCAL_PASSWORD).await?;

    let (first_status, _first_body) =
        login_local(&app, &username1, REST_TEST_LOCAL_PASSWORD).await?;
    assert_eq!(first_status, StatusCode::OK);

    let (second_status, second_body) =
        login_local(&app, &username2, REST_TEST_LOCAL_PASSWORD).await?;
    assert_eq!(second_status, StatusCode::FORBIDDEN);
    let payload: serde_json::Value = serde_json::from_slice(&second_body).expect("json");
    assert_eq!(
        payload.get("code").and_then(|value| value.as_str()),
        Some("LICENSE_SEAT_LIMIT_EXCEEDED")
    );

    for user_id in [user_id1, user_id2] {
        let _ = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/users/{user_id}"),
            None,
            Some(&admin_cookie),
        )
        .await
        .expect("delete user");
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_state_rejects_garbage_stored_entitlement_jwt()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);

    reset_license_tables(&db_pool).await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login before license state");
    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE license_state
        SET entitlement_jwt = 'garbage',
            max_seats = 999,
            entitlement_expires_at = NOW() + INTERVAL '100 years',
            updated_at = NOW()
        WHERE instance_id = $1
        "#,
    )
    .bind(LICENSE_INSTANCE_ID)
    .execute(&db_pool)
    .await
    .expect("tamper stored JWT");

    let (status, body) = request(&app, "GET", LICENSE_STATUS_PATH, None, Some(&cookie))
        .await
        .expect("state");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("reason_code").and_then(|value| value.as_str()),
        Some(piping_backend::contexts::shared::domain::reason_codes::ENTITLEMENT_BINDING_INVALID)
    );
    Ok(())
}

async fn with_license_settings(
    state: piping_backend::app::AppState,
    base_url: &str,
) -> Result<piping_backend::app::AppState, Box<dyn std::error::Error + Send + Sync>> {
    let mut updated = (*state.settings).clone();
    updated.license.base_url = base_url.to_string();
    rebuild_state_with_settings(state, Arc::new(updated)).await
}

async fn with_license_policy_settings(
    state: piping_backend::app::AppState,
    grace_failure_threshold: u32,
    grace_period_seconds: u64,
) -> Result<piping_backend::app::AppState, Box<dyn std::error::Error + Send + Sync>> {
    let mut updated = (*state.settings).clone();
    updated.license.grace_failure_threshold = grace_failure_threshold;
    updated.license.grace_period_seconds = grace_period_seconds;
    rebuild_state_with_settings(state, Arc::new(updated)).await
}

async fn rebuild_state_with_settings(
    state: piping_backend::app::AppState,
    settings: Arc<piping_backend::config::Settings>,
) -> Result<piping_backend::app::AppState, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        piping_backend::app::AppState::build(settings, state.db_pool.clone(), state.redis.clone())
            .await
            .map_err(|e| format!("rebuild app state with updated settings: {e}"))?,
    )
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_concurrent_logins_enforce_seat_boundary()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    } = build_default_license_admin_harness().await?;

    reset_license_tables(&db_pool).await?;
    // Give the admin ample room for the setup actions below.
    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_CONCURRENT_BOUNDARY + 100,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    let _admin_relogin = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin relogin");

    let username1 = format!("race-seat-user-a-{}", unique_suffix());
    let username2 = format!("race-seat-user-b-{}", unique_suffix());
    let user_id1 =
        create_local_user(&app, &admin_cookie, &username1, REST_TEST_LOCAL_PASSWORD).await?;
    let user_id2 =
        create_local_user(&app, &admin_cookie, &username2, REST_TEST_LOCAL_PASSWORD).await?;

    // A COUNT-based boundary ("held seats + 1") proved racy twice: seats held
    // by sessions that earlier suites left behind get revoked lazily between
    // the measurement and the race, so both logins found capacity and nothing
    // was rejected (0/2 in CI, while a fast local run passed). Make the
    // boundary self-contained instead: with all setup done, revoke every held
    // seat directly and set the limit to exactly one. A background sweeper
    // revoking an already-revoked seat is a no-op, so the race is now over
    // ONE free seat by construction — exactly one login must win it.
    sqlx::query("UPDATE license_seat_assignment SET revoked_at = NOW() WHERE revoked_at IS NULL")
        .execute(&db_pool)
        .await
        .expect("revoke all held seats");
    upsert_license_state(
        &db_pool,
        "active",
        1,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    let (login1, login2) = tokio::join!(
        login_local(&app, &username1, REST_TEST_LOCAL_PASSWORD),
        login_local(&app, &username2, REST_TEST_LOCAL_PASSWORD)
    );
    let login1 = login1?;
    let login2 = login2?;
    let statuses = [login1.0, login2.0];
    let ok_count = statuses
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    let forbidden_count = statuses
        .iter()
        .filter(|status| **status == StatusCode::FORBIDDEN)
        .count();
    #[allow(clippy::print_stderr)] // diag must reach the CI trace of the flake it hunts
    if ok_count != 1 || forbidden_count != 1 {
        // Diagnostic dump for the intermittent 0-rejections failure: what did
        // the server answer, and what do the license tables actually hold.
        eprintln!(
            "SEAT-RACE DIAG statuses={statuses:?}\nbody1={}\nbody2={}",
            String::from_utf8_lossy(&login1.1),
            String::from_utf8_lossy(&login2.1)
        );
        for (name, sql) in [
            (
                "seats",
                "SELECT user_id::text || ' revoked=' || COALESCE(revoked_at::text,'-') \
                 FROM license_seat_assignment ORDER BY assigned_at DESC LIMIT 10",
            ),
            (
                "ledger",
                "SELECT event_type || '/' || reason_code || '/' || COALESCE(user_id::text,'-') \
                 FROM license_seat_ledger ORDER BY created_at DESC LIMIT 10",
            ),
            (
                "state",
                "SELECT 'status=' || status || ' max_seats=' || max_seats::text \
                 FROM license_state ORDER BY updated_at DESC LIMIT 3",
            ),
        ] {
            let rows: Vec<String> = sqlx::query_scalar(sql)
                .fetch_all(&db_pool)
                .await
                .unwrap_or_else(|e| vec![format!("query failed: {e}")]);
            eprintln!("SEAT-RACE DIAG {name}: {rows:?}");
        }
    }
    assert_eq!(
        ok_count, 1,
        "exactly one concurrent login should acquire last seat"
    );
    assert_eq!(
        forbidden_count, 1,
        "exactly one concurrent login should be rejected by seat limit"
    );

    let active_assignments: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM license_seat_assignment
        WHERE revoked_at IS NULL
        "#,
    )
    .fetch_one(&db_pool)
    .await
    .expect("active assignments");
    assert_eq!(
        active_assignments, 1,
        "must preserve the one-seat boundary under concurrent logins"
    );

    for user_id in [user_id1, user_id2] {
        let _ = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/users/{user_id}"),
            None,
            Some(&admin_cookie),
        )
        .await
        .expect("delete user");
    }
    Ok(())
}

/// The seat decision runs inside a transaction that holds the seat advisory
/// lock. If any read in that scope goes through the pool instead of the
/// transaction, it checks out a SECOND connection while the first is still
/// held — and with more concurrent logins than pool connections that deadlocks
/// until the acquire timeout. The login then answers with a database error
/// instead of a seat verdict, and the rollback hands the contested seat to
/// whoever was waiting on the lock.
///
/// The test harness pool is deliberately small (`max_connections(2)`), so
/// firing four concurrent logins reproduces that. Every response must be a seat
/// verdict — 200 or 403 — and never a 5xx.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_concurrent_logins_never_answer_with_a_database_error()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let LicenseAdminHarness {
        app,
        db_pool,
        admin_cookie,
    } = build_default_license_admin_harness().await?;

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_CONCURRENT_BOUNDARY,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    let _admin_relogin = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin relogin");

    let mut usernames = Vec::new();
    let mut user_ids = Vec::new();
    for index in 0..4 {
        let username = format!("pool-race-user-{index}-{}", unique_suffix());
        user_ids.push(
            create_local_user(&app, &admin_cookie, &username, REST_TEST_LOCAL_PASSWORD).await?,
        );
        usernames.push(username);
    }

    let logins = futures_util::future::join_all(
        usernames
            .iter()
            .map(|username| login_local(&app, username, REST_TEST_LOCAL_PASSWORD)),
    )
    .await;

    let mut statuses = Vec::new();
    for login in logins {
        statuses.push(login?.0);
    }

    let verdicts = statuses
        .iter()
        .filter(|status| **status == StatusCode::OK || **status == StatusCode::FORBIDDEN)
        .count();
    assert_eq!(
        verdicts,
        statuses.len(),
        "every concurrent login must answer with a seat verdict, got {statuses:?}"
    );

    for user_id in user_ids {
        let _ = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/users/{user_id}"),
            None,
            Some(&admin_cookie),
        )
        .await
        .expect("delete user");
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_refresh_with_control_plane_outage_marks_degraded()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let state = with_license_settings(state, REST_TEST_LICENSE_OUTAGE_URL).await?;
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    let (status, _body) = request(
        &app,
        "POST",
        LICENSE_REFRESH_PATH,
        Some("{}"),
        Some(&cookie),
    )
    .await
    .expect("refresh");
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let row =
        sqlx::query("SELECT status, last_failure_kind FROM license_state WHERE instance_id = $1")
            .bind(LICENSE_INSTANCE_ID)
            .fetch_one(&db_pool)
            .await
            .expect("license row");
    let status_value: String = sqlx::Row::try_get(&row, "status").expect("status");
    let failure_kind: Option<String> = sqlx::Row::try_get(&row, "last_failure_kind").ok();
    assert_eq!(status_value, "degraded");
    assert_eq!(
        failure_kind.as_deref(),
        Some("CONTROL_PLANE_UNAVAILABLE"),
        "degraded state should persist failure taxonomy"
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_refresh_transitions_to_grace_on_threshold()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let state = with_license_settings(state, REST_TEST_LICENSE_OUTAGE_URL).await?;
    let state = with_license_policy_settings(
        state,
        LICENSE_GRACE_FAILURE_THRESHOLD,
        LICENSE_GRACE_PERIOD_SECONDS,
    )
    .await?;
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    let (status, _body) = request(
        &app,
        "POST",
        LICENSE_REFRESH_PATH,
        Some("{}"),
        Some(&cookie),
    )
    .await
    .expect("refresh");
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let row = sqlx::query(
        "SELECT status, grace_started_at, grace_ends_at FROM license_state WHERE instance_id = $1",
    )
    .bind(LICENSE_INSTANCE_ID)
    .fetch_one(&db_pool)
    .await
    .expect("license row");
    let status_value: String = sqlx::Row::try_get(&row, "status").expect("status");
    let grace_started_at: Option<chrono::DateTime<Utc>> =
        sqlx::Row::try_get(&row, "grace_started_at").ok();
    let grace_ends_at: Option<chrono::DateTime<Utc>> =
        sqlx::Row::try_get(&row, "grace_ends_at").ok();
    assert_eq!(status_value, "grace");
    assert!(grace_started_at.is_some(), "grace_started_at should be set");
    assert!(grace_ends_at.is_some(), "grace_ends_at should be set");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn license_grace_end_blocks_new_login() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let state = build_test_app_state().await.expect("app state");
    let state = with_license_settings(state, REST_TEST_LICENSE_OUTAGE_URL).await?;
    let state = with_license_policy_settings(
        state,
        LICENSE_GRACE_FAILURE_THRESHOLD,
        LICENSE_GRACE_PERIOD_IMMEDIATE_SECONDS,
    )
    .await?;
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");

    reset_license_tables(&db_pool).await?;
    upsert_license_state(
        &db_pool,
        "active",
        LICENSE_MAX_SEATS_SIGNED,
        Utc::now() + chrono::Duration::days(LICENSE_EXPIRY_DAYS_DEFAULT),
    )
    .await?;

    let _ = request(
        &app,
        "POST",
        LICENSE_REFRESH_PATH,
        Some("{}"),
        Some(&admin_cookie),
    )
    .await
    .expect("first refresh to grace");
    tokio::time::sleep(std::time::Duration::from_secs(
        LICENSE_GRACE_END_WAIT_SECONDS,
    ))
    .await;
    let _ = request(
        &app,
        "POST",
        LICENSE_REFRESH_PATH,
        Some("{}"),
        Some(&admin_cookie),
    )
    .await
    .expect("second refresh to expired");

    let username = format!("grace-end-user-{}", unique_suffix());
    let user_id =
        create_local_user(&app, &admin_cookie, &username, REST_TEST_LOCAL_PASSWORD).await?;
    let (status, body) = login_local(&app, &username, REST_TEST_LOCAL_PASSWORD).await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload.get("code").and_then(|value| value.as_str()),
        Some("LICENSE_GRACE_ENDED")
    );

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("delete user");
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn license_issuer_refusal_blocks_only_new_seats_and_survives_outage_restart()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mock = start_mock_license_server().await?;
    let state = with_license_settings(build_test_app_state().await?, &mock.base_url).await?;
    let pool = state.db_pool.clone();
    reset_license_tables(&pool).await?;
    let app = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("login")?;
    let (status, _) = request(
        &app,
        "POST",
        LICENSE_ACTIVATE_PATH,
        Some(r#"{"activationToken":"integration-token"}"#),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let username = format!("refusal-new-{}", unique_suffix());
    let user_id = create_local_user(&app, &cookie, &username, REST_TEST_LOCAL_PASSWORD).await?;
    for (mode, kind) in [
        (8, "CONTROL_PLANE_RESPONSE_INVALID"),
        (9, "CONTROL_PLANE_RESPONSE_INVALID"),
        (10, "ENTITLEMENT_INVALID"),
    ] {
        mock.mode.store(mode, Ordering::SeqCst);
        let result = request(
            &app,
            "POST",
            LICENSE_REFRESH_PATH,
            Some("{}"),
            Some(&cookie),
        )
        .await?;
        assert!(!result.0.is_success());
        let diagnostic: (String, Option<String>) =
            sqlx::query_as("SELECT status, last_failure_kind FROM license_state")
                .fetch_one(&pool)
                .await?;
        assert_ne!(diagnostic.0, "suspended");
        assert_eq!(diagnostic.1.as_deref(), Some(kind));
        mock.mode.store(1, Ordering::SeqCst);
        assert_eq!(
            request(
                &app,
                "POST",
                LICENSE_REFRESH_PATH,
                Some("{}"),
                Some(&cookie)
            )
            .await?
            .0,
            StatusCode::OK
        );
    }
    for mode in [5, 11] {
        // Forged denial on 5xx and unresolved peer lookup.
        mock.mode.store(mode, Ordering::SeqCst);
        assert_eq!(
            request(
                &app,
                "POST",
                LICENSE_REFRESH_PATH,
                Some("{}"),
                Some(&cookie)
            )
            .await?
            .0,
            StatusCode::BAD_GATEWAY
        );
        let diagnostic: (String, Option<String>) =
            sqlx::query_as("SELECT status, last_failure_kind FROM license_state")
                .fetch_one(&pool)
                .await?;
        assert_ne!(
            diagnostic.0, "suspended",
            "a 503 body is not an authoritative refusal"
        );
        assert_eq!(diagnostic.1.as_deref(), Some("CONTROL_PLANE_UNAVAILABLE"));
        mock.mode.store(1, Ordering::SeqCst);
        assert_eq!(
            request(
                &app,
                "POST",
                LICENSE_REFRESH_PATH,
                Some("{}"),
                Some(&cookie)
            )
            .await?
            .0,
            StatusCode::OK
        );
    }
    mock.mode.store(4, Ordering::SeqCst);
    for _ in 0..4 {
        let (status, body) = request(
            &app,
            "POST",
            LICENSE_REFRESH_PATH,
            Some("{}"),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let refusal: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(refusal["code"], "TOKEN_REVOKED");
        assert_eq!(refusal["reason_code"], "TOKEN_REVOKED");
        let persisted: (
            String,
            Option<String>,
            Option<chrono::DateTime<Utc>>,
            Option<chrono::DateTime<Utc>>,
        ) = sqlx::query_as(
            "SELECT status, last_failure_kind, grace_started_at, grace_ends_at FROM license_state",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            persisted,
            ("suspended".into(), Some("TOKEN_REVOKED".into()), None, None)
        );
        assert_eq!(
            login_local(&app, &username, REST_TEST_LOCAL_PASSWORD)
                .await?
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for mode in [5, 11] {
        mock.mode.store(mode, Ordering::SeqCst);
        let _ = request(
            &app,
            "POST",
            LICENSE_REFRESH_PATH,
            Some("{}"),
            Some(&cookie),
        )
        .await?;
    }
    let settings = state.settings.clone();
    let app =
        piping_backend::app::build_router(rebuild_state_with_settings(state, settings).await?);
    let (status, body) = request(&app, "GET", LICENSE_STATUS_PATH, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body)?["status"],
        "suspended"
    );
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/license/health/details",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let health: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(health["status"], "suspended");
    assert_eq!(health["reasonCode"], "TOKEN_REVOKED");
    let persisted: (String, Option<String>, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("SELECT status, last_failure_kind, grace_started_at FROM license_state")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        persisted,
        ("suspended".into(), Some("TOKEN_REVOKED".into()), None)
    );
    assert!(
        login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
            .await
            .is_some()
    );
    assert_eq!(
        login_local(&app, &username, REST_TEST_LOCAL_PASSWORD)
            .await?
            .0,
        StatusCode::FORBIDDEN
    );
    mock.mode.store(1, Ordering::SeqCst);
    assert_eq!(
        request(
            &app,
            "POST",
            LICENSE_REFRESH_PATH,
            Some("{}"),
            Some(&cookie)
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        login_local(&app, &username, REST_TEST_LOCAL_PASSWORD)
            .await?
            .0,
        StatusCode::OK
    );
    mock.mode.store(4, Ordering::SeqCst);
    request(
        &app,
        "POST",
        LICENSE_REFRESH_PATH,
        Some("{}"),
        Some(&cookie),
    )
    .await?;
    // A surviving session without an assignment must not let a status read
    // allocate a seat while suspended.
    sqlx::query("DELETE FROM license_seat_assignment WHERE user_id = $1::uuid")
        .bind(&user_id)
        .execute(&pool)
        .await?;
    request(&app, "GET", LICENSE_STATUS_PATH, None, Some(&cookie)).await?;
    let assigned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM license_seat_assignment WHERE user_id = $1::uuid AND revoked_at IS NULL)")
        .bind(&user_id).fetch_one(&pool).await?;
    assert!(!assigned, "status backfill must not bypass issuer refusal");
    mock.mode.store(2, Ordering::SeqCst);
    let before_failed_replacement: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(s) FROM license_state s")
            .fetch_one(&pool)
            .await?;
    // Native PostgreSQL failure after the replacement upsert, during backfill.
    // Existing rows are preserved; only attempted inserts fail this test check.
    sqlx::query("ALTER TABLE license_seat_assignment ADD CONSTRAINT pwa162_test_backfill_rejected CHECK (false) NOT VALID").execute(&pool).await?;
    let failed_replacement = request(
        &app,
        "POST",
        LICENSE_ACTIVATE_PATH,
        Some(r#"{"activationToken":"replacement-token"}"#),
        Some(&cookie),
    )
    .await;
    sqlx::query(
        "ALTER TABLE license_seat_assignment DROP CONSTRAINT pwa162_test_backfill_rejected",
    )
    .execute(&pool)
    .await?;
    // The canonical SQLx mapper reports CHECK violations as validation errors.
    assert_eq!(failed_replacement?.0, StatusCode::BAD_REQUEST);
    let after_failed_replacement: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(s) FROM license_state s")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        after_failed_replacement, before_failed_replacement,
        "backfill failure must roll back replacement state, token and revision"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            LICENSE_ACTIVATE_PATH,
            Some(r#"{"activationToken":"replacement-token"}"#),
            Some(&cookie)
        )
        .await?
        .0,
        StatusCode::OK
    );
    let restored: (String, i32, Option<String>, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT status, max_seats, last_failure_kind, grace_started_at FROM license_state",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(restored, ("active".into(), 3, None, None));
    request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn license_refresh_results_are_fenced_by_replacement_and_newer_refresh()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for replacement in [true, false] {
        for old_mode in [6, 7] {
            // Delayed valid success and confirmed refusal.
            let mock = start_mock_license_server().await?;
            let state =
                with_license_settings(build_test_app_state().await?, &mock.base_url).await?;
            let pool = state.db_pool.clone();
            reset_license_tables(&pool).await?;
            let app = piping_backend::app::build_router(state);
            let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
                .await
                .ok_or("login")?;
            assert_eq!(
                request(
                    &app,
                    "POST",
                    LICENSE_ACTIVATE_PATH,
                    Some(r#"{"activationToken":"first-token"}"#),
                    Some(&cookie)
                )
                .await?
                .0,
                StatusCode::OK
            );
            let instance: String = sqlx::query_scalar("SELECT instance_id FROM license_state")
                .fetch_one(&pool)
                .await?;
            let seats: serde_json::Value = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY id), '[]') FROM license_seat_assignment s").fetch_one(&pool).await?;
            mock.mode.store(old_mode, Ordering::SeqCst);
            let old_app = app.clone();
            let old_cookie = cookie.clone();
            let old = tokio::spawn(async move {
                request(
                    &old_app,
                    "POST",
                    LICENSE_REFRESH_PATH,
                    Some("{}"),
                    Some(&old_cookie),
                )
                .await
            });
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                mock.refresh_entered.acquire(),
            )
            .await??
            .forget();
            mock.mode.store(2, Ordering::SeqCst);
            let (path, body) = if replacement {
                (
                    LICENSE_ACTIVATE_PATH,
                    r#"{"activationToken":"replacement-token"}"#,
                )
            } else {
                (LICENSE_REFRESH_PATH, "{}")
            };
            assert_eq!(
                request(&app, "POST", path, Some(body), Some(&cookie))
                    .await?
                    .0,
                StatusCode::OK
            );
            let before: serde_json::Value =
                sqlx::query_scalar("SELECT to_jsonb(s) FROM license_state s")
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(before["instance_id"], instance);
            assert_eq!(before["entitlement_version"], "replacement-v2");
            assert_eq!(before["max_seats"], 3);
            mock.refresh_release.add_permits(1);
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(10), old)
                    .await???
                    .0,
                StatusCode::CONFLICT
            );
            let after: serde_json::Value =
                sqlx::query_scalar("SELECT to_jsonb(s) FROM license_state s")
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(
                after, before,
                "superseded result must not mutate the replacement"
            );
            let after_seats: serde_json::Value = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY id), '[]') FROM license_seat_assignment s").fetch_one(&pool).await?;
            assert_eq!(after_seats, seats);
            // A refused replacement cannot erase or downgrade the working license.
            mock.mode.store(4, Ordering::SeqCst);
            assert_eq!(
                request(
                    &app,
                    "POST",
                    LICENSE_ACTIVATE_PATH,
                    Some(r#"{"activationToken":"refused-replacement"}"#),
                    Some(&cookie)
                )
                .await?
                .0,
                StatusCode::FORBIDDEN
            );
            let unchanged: serde_json::Value =
                sqlx::query_scalar("SELECT to_jsonb(s) FROM license_state s")
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(unchanged, before);
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Requires isolated PostgreSQL and Redis.
async fn license_cloud_envelope_refusals_suspend_new_seats_and_preserve_failed_replacement()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for (mode, reason, expected) in [
        (12, "TOKEN_NOT_FOUND", StatusCode::NOT_FOUND),
        (13, "ENTITLEMENT_BINDING_INVALID", StatusCode::CONFLICT),
        (14, "LICENSE_INSTANCE_CONFLICT", StatusCode::CONFLICT),
        (16, "ORGANIZATION_ARCHIVED", StatusCode::FORBIDDEN),
    ] {
        let mock = start_mock_license_server().await?;
        let state = with_license_settings(build_test_app_state().await?, &mock.base_url).await?;
        let pool = state.db_pool.clone();
        reset_license_tables(&pool).await?;
        let app = piping_backend::app::build_router(state);
        let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
            .await
            .ok_or("admin login")?;
        assert_eq!(
            request(
                &app,
                "POST",
                LICENSE_ACTIVATE_PATH,
                Some(r#"{"activationToken":"initial-token"}"#),
                Some(&cookie)
            )
            .await?
            .0,
            StatusCode::OK
        );
        let username = format!("cloud-refusal-{mode}-{}", unique_suffix());
        let user_id = create_local_user(&app, &cookie, &username, REST_TEST_LOCAL_PASSWORD).await?;
        let before: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(s) FROM license_state s")
                .fetch_one(&pool)
                .await?;
        mock.mode.store(15, Ordering::SeqCst);
        let (status, body) = request(
            &app,
            "POST",
            LICENSE_ACTIVATE_PATH,
            Some(r#"{"activationToken":"consumed-replacement"}"#),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)?["reason_code"],
            "TOKEN_ALREADY_CONSUMED"
        );
        let after: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(s) FROM license_state s")
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            after, before,
            "failed replacement must preserve valid entitlement"
        );
        mock.mode.store(mode, Ordering::SeqCst);
        let (status, body) = request(
            &app,
            "POST",
            LICENSE_REFRESH_PATH,
            Some("{}"),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, expected, "producer envelope for {reason}");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)?["reason_code"],
            reason
        );
        let persisted: (
            String,
            Option<String>,
            Option<chrono::DateTime<Utc>>,
            Option<chrono::DateTime<Utc>>,
        ) = sqlx::query_as(
            "SELECT status,last_failure_kind,grace_started_at,grace_ends_at FROM license_state",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            persisted,
            ("suspended".into(), Some(reason.into()), None, None)
        );
        assert_eq!(
            login_local(&app, &username, REST_TEST_LOCAL_PASSWORD)
                .await?
                .0,
            StatusCode::FORBIDDEN
        );
        assert!(
            login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
                .await
                .is_some()
        );
        let (_, body) = request(
            &app,
            "GET",
            "/api/v1/license/health/details",
            None,
            Some(&cookie),
        )
        .await?;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)?["reasonCode"],
            reason
        );
        request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/users/{user_id}"),
            None,
            Some(&cookie),
        )
        .await?;
    }
    Ok(())
}

fn mock_cloud_envelope_refusal(mode: u8) -> Option<axum::response::Response> {
    use axum::response::IntoResponse;
    let (status, code, reason) = match mode {
        12 => (StatusCode::NOT_FOUND, "NOT_FOUND", "TOKEN_NOT_FOUND"),
        13 => (
            StatusCode::CONFLICT,
            "CONFLICT",
            "ENTITLEMENT_BINDING_INVALID",
        ),
        14 => (
            StatusCode::CONFLICT,
            "CONFLICT",
            "LICENSE_INSTANCE_CONFLICT",
        ),
        15 => (StatusCode::CONFLICT, "CONFLICT", "TOKEN_ALREADY_CONSUMED"),
        16 => (
            StatusCode::FORBIDDEN,
            "ORGANIZATION_ARCHIVED",
            "ORGANIZATION_ARCHIVED",
        ),
        _ => return None,
    };
    Some(
        (
            status,
            Json(serde_json::json!({"code":code,"reasonCode":reason,"message":"Issuer refusal"})),
        )
            .into_response(),
    )
}

#[tokio::test]
async fn license_cloud_fixture_requires_csrf_on_both_operations()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mock = start_mock_license_server().await?;
    let client = reqwest::Client::new();
    for path in ["activate", "refresh"] {
        for header in [None, Some("0")] {
            let mut request = client.post(format!("{}/api/v1/instances/{path}", mock.base_url));
            if let Some(value) = header {
                request = request.header("X-Piping-CSRF", value);
            }
            let response = request.json(&serde_json::json!({})).send().await?;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body: serde_json::Value = response.json().await?;
            assert_eq!(body["code"], "CSRF_HEADER_MISSING");
            assert_eq!(body["reasonCode"], "CSRF_HEADER_MISSING");
        }
    }
    Ok(())
}

struct MockLicenseServer {
    base_url: String,
    mode: Arc<AtomicU8>,
    refresh_entered: Arc<tokio::sync::Semaphore>,
    refresh_release: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
struct MockLicenseState {
    mode: Arc<AtomicU8>,
    last_instance_fingerprint: Arc<RwLock<Option<String>>>,
    refresh_entered: Arc<tokio::sync::Semaphore>,
    refresh_release: Arc<tokio::sync::Semaphore>,
}

#[derive(Debug, Deserialize)]
struct MockActivateRequest {
    instance_fingerprint: String,
}

#[derive(Debug, Deserialize)]
struct MockRefreshRequest {
    activation_token: String,
}

#[derive(Debug, Serialize)]
struct MockEntitlementResponse {
    entitlement_jwt: String,
    refresh_after_seconds: u64,
    max_seats: i32,
    entitlement_version: String,
    entitlement_expires_at: String,
}

#[derive(Debug, Serialize)]
struct MockClaims {
    iss: String,
    aud: String,
    sub: String,
    iat: i64,
    nbf: i64,
    exp: i64,
    organization_id: String,
    instance_fingerprint: String,
    max_seats: i32,
    entitlement_version: String,
}

async fn start_mock_license_server()
-> Result<MockLicenseServer, Box<dyn std::error::Error + Send + Sync>> {
    let mode = Arc::new(AtomicU8::new(1));
    let refresh_entered = Arc::new(tokio::sync::Semaphore::new(0));
    let refresh_release = Arc::new(tokio::sync::Semaphore::new(0));
    let state = MockLicenseState {
        mode: mode.clone(),
        last_instance_fingerprint: Arc::new(RwLock::new(None)),
        refresh_entered: refresh_entered.clone(),
        refresh_release: refresh_release.clone(),
    };
    let app = Router::new()
        .route("/api/v1/instances/activate", post(mock_activate_handler))
        .route("/api/v1/instances/refresh", post(mock_refresh_handler))
        .route_layer(axum::middleware::from_fn(
            license_http_contract::require_cloud_csrf,
        ))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(MOCK_LICENSE_BIND_ADDR)
        .await
        .map_err(|e| format!("bind mock server: {e}"))?;
    let addr: SocketAddr = listener
        .local_addr()
        .map_err(|e| format!("local addr: {e}"))?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(MockLicenseServer {
        base_url: format!("http://{}", addr),
        mode,
        refresh_entered,
        refresh_release,
    })
}

async fn mock_activate_handler(
    State(state): State<MockLicenseState>,
    Json(payload): Json<MockActivateRequest>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;
    if let Some(response) = mock_cloud_envelope_refusal(state.mode.load(Ordering::SeqCst)) {
        return Ok(response);
    }
    if state.mode.load(Ordering::SeqCst) == 4 {
        return Ok((StatusCode::FORBIDDEN, Json(serde_json::json!({
            "code":"TOKEN_REVOKED", "reasonCode":"TOKEN_REVOKED", "message":"Token has been revoked"
        }))).into_response());
    }
    *state.last_instance_fingerprint.write().await = Some(payload.instance_fingerprint.clone());
    let response = build_mock_entitlement_response(
        state.mode.load(Ordering::SeqCst),
        &payload.instance_fingerprint,
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(response).into_response())
}

async fn mock_refresh_handler(
    State(state): State<MockLicenseState>,
    Json(payload): Json<MockRefreshRequest>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;
    let mode = state.mode.load(Ordering::SeqCst);
    if let Some(response) = mock_cloud_envelope_refusal(mode) {
        return Ok(response);
    }
    if mode == 11 {
        return Ok((StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "code":"peer_unreachable", "reasonCode":"peer_unreachable", "message":"Peer lookup unavailable: TOKEN_REVOKED"
        }))).into_response());
    }
    if mode == 8 {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"code":"UNRECOGNIZED", "reasonCode":"UNRECOGNIZED", "message":"Unknown issuer condition"})),
        )
            .into_response());
    }
    if mode == 9 {
        return Ok((StatusCode::FORBIDDEN, "malformed refusal").into_response());
    }
    if mode == 6 || mode == 7 {
        state.refresh_entered.add_permits(1);
        state
            .refresh_release
            .acquire()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .forget();
    }
    if mode == 4 || mode == 5 || mode == 7 {
        return Ok((if mode == 5 { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::FORBIDDEN },
            Json(serde_json::json!({"code":"TOKEN_REVOKED","reasonCode":"TOKEN_REVOKED","message":"Activation token has been revoked"}))).into_response());
    }
    let _ = payload.activation_token;
    let instance_fingerprint = state
        .last_instance_fingerprint
        .read()
        .await
        .clone()
        .unwrap_or_else(|| "mock-unactivated-instance".to_string());
    let mut response = build_mock_entitlement_response(mode, &instance_fingerprint)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if mode == 10 {
        let (unsigned, _) = response
            .entitlement_jwt
            .rsplit_once('.')
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        response.entitlement_jwt = format!("{unsigned}.{}", "A".repeat(86));
    }
    Ok(Json(response).into_response())
}

fn build_mock_entitlement_response(
    mode: u8,
    instance_fingerprint: &str,
) -> Result<MockEntitlementResponse, Box<dyn std::error::Error + Send + Sync>> {
    ensure_jsonwebtoken_provider_installed();
    let now = Utc::now();
    let (private_key, kid, version, exp, max_seats) = match mode {
        2 => (
            TEST_PRIVATE_KEY_1,
            "k1",
            "replacement-v2".to_string(),
            (now + chrono::Duration::hours(LICENSE_TOKEN_VALIDITY_HOURS)).timestamp(),
            3,
        ),
        3 => (
            TEST_PRIVATE_KEY_1,
            "k1",
            "stale-v1".to_string(),
            (now - chrono::Duration::minutes(LICENSE_STALE_TOKEN_MINUTES)).timestamp(),
            LICENSE_MAX_SEATS_SIGNED,
        ),
        _ => (
            TEST_PRIVATE_KEY_1,
            "k1",
            "v1".to_string(),
            (now + chrono::Duration::hours(LICENSE_TOKEN_VALIDITY_HOURS)).timestamp(),
            LICENSE_MAX_SEATS_SIGNED,
        ),
    };
    let claims = MockClaims {
        iss: "piping-com-backend".to_string(),
        aud: "piping-instance".to_string(),
        sub: "org:test".to_string(),
        iat: (now - chrono::Duration::seconds(LICENSE_CLOCK_SKEW_SECONDS)).timestamp(),
        nbf: (now - chrono::Duration::seconds(LICENSE_CLOCK_SKEW_SECONDS)).timestamp(),
        exp,
        organization_id: LICENSE_MOCK_ORGANIZATION_ID.to_string(),
        instance_fingerprint: instance_fingerprint.to_string(),
        max_seats,
        entitlement_version: version.clone(),
    };
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some(kid.to_string());
    let encoding_key = EncodingKey::from_ed_pem(private_key.as_bytes())
        .map_err(|e| format!("private key: {e}"))?;
    let token =
        encode(&header, &claims, &encoding_key).map_err(|e| format!("encode entitlement: {e}"))?;
    Ok(MockEntitlementResponse {
        entitlement_jwt: token,
        refresh_after_seconds: LICENSE_REFRESH_AFTER_SECONDS,
        max_seats,
        entitlement_version: version,
        entitlement_expires_at: chrono::DateTime::from_timestamp(exp, 0)
            .ok_or_else(|| format!("timestamp: invalid exp {exp}"))?
            .to_rfc3339(),
    })
}

fn ensure_jsonwebtoken_provider_installed() {
    // No-op for this crate build: jsonwebtoken provider is selected by enabled features.
}

const TEST_PRIVATE_KEY_1: &str = "-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEID7TRf9+jjgEXCFuwTjFPuEpWqPTMu/Gljl46gTm48lT
-----END PRIVATE KEY-----";
