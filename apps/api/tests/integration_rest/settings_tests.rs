use super::*;

#[tokio::test]
#[ignore]
async fn settings_routes_require_authentication() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/settings/effective?keys=app.theme",
        None,
        None,
    )
    .await
    .expect("effective settings without auth");
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let body = serde_json::json!({ "value": "dark" }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/settings/user/app.theme",
        Some(&body),
        None,
    )
    .await
    .expect("user setting without auth");
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = request(&app, "GET", "/api/v1/admin/settings/defaults", None, None)
        .await
        .expect("admin defaults without auth");
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/admin/settings/defaults/app.theme",
        Some(&body),
        None,
    )
    .await
    .expect("admin default without auth");
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore]
async fn admin_settings_routes_forbid_authenticated_non_admin() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");

    let suffix = unique_suffix();
    let username = format!("settings-regular-{suffix}");
    let create_body = serde_json::json!({
        "username": username,
        "email": format!("{username}@test.local"),
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
    .expect("create regular user");
    assert_eq!(status, StatusCode::OK);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("created user json");
    let user_id = created
        .get("id")
        .and_then(|value| value.as_str())
        .expect("created user id");
    let user_cookie = login_and_get_cookie(&app, &username, REST_TEST_LOCAL_PASSWORD)
        .await
        .expect("regular user login");

    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/admin/settings/defaults",
        None,
        Some(&user_cookie),
    )
    .await
    .expect("regular user get admin defaults");
    assert_eq!(status, StatusCode::FORBIDDEN);

    let key = format!("test.settings.{suffix}");
    let body = serde_json::json!({ "value": "forbidden" }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/settings/defaults/{key}"),
        Some(&body),
        Some(&user_cookie),
    )
    .await
    .expect("regular user put admin default");
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/settings/defaults/{key}"),
        None,
        Some(&user_cookie),
    )
    .await
    .expect("regular user delete admin default");
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await
    .expect("delete regular user");
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn settings_effective_values_prefer_user_over_instance_default() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let key = format!("test.settings.{}", unique_suffix());

    let default_body = serde_json::json!({ "value": "instance-value" }).to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/settings/defaults/{key}"),
        Some(&default_body),
        Some(&cookie),
    )
    .await
    .expect("upsert instance default");
    assert_eq!(status, StatusCode::OK);
    let default_json: serde_json::Value = serde_json::from_slice(&body).expect("default json");
    assert_eq!(
        default_json.get("key").and_then(|value| value.as_str()),
        Some(key.as_str())
    );
    assert_eq!(
        default_json.get("value").and_then(|value| value.as_str()),
        Some("instance-value")
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/settings/effective/{key}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get effective instance default");
    assert_eq!(status, StatusCode::OK);
    let effective: serde_json::Value = serde_json::from_slice(&body).expect("effective json");
    assert_eq!(
        effective.get("source").and_then(|value| value.as_str()),
        Some("instance")
    );
    assert_eq!(
        effective.get("value").and_then(|value| value.as_str()),
        Some("instance-value")
    );

    let user_body = serde_json::json!({ "value": { "mode": "user-value" } }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/settings/user/{key}"),
        Some(&user_body),
        Some(&cookie),
    )
    .await
    .expect("upsert user override");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/settings/effective?keys={key}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("list effective filtered");
    assert_eq!(status, StatusCode::OK);
    let list: serde_json::Value = serde_json::from_slice(&body).expect("list json");
    let setting = list
        .get("settings")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
        .expect("one setting");
    assert_eq!(
        setting.get("source").and_then(|value| value.as_str()),
        Some("user")
    );
    assert_eq!(
        setting
            .pointer("/value/mode")
            .and_then(|value| value.as_str()),
        Some("user-value")
    );
    assert_eq!(
        setting
            .get("instance_value")
            .and_then(|value| value.as_str()),
        Some("instance-value")
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/settings/user/{key}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete user override");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/settings/effective/{key}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get effective after reset");
    assert_eq!(status, StatusCode::OK);
    let effective: serde_json::Value = serde_json::from_slice(&body).expect("effective json");
    assert_eq!(
        effective.get("source").and_then(|value| value.as_str()),
        Some("instance")
    );
    assert_eq!(
        effective.get("value").and_then(|value| value.as_str()),
        Some("instance-value")
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/settings/defaults/{key}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete instance default");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn settings_routes_validate_keys_and_value_size() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let body = serde_json::json!({ "value": true }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/settings/user/App.theme",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("invalid key");
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let oversized = "x".repeat(
        piping_backend::contexts::access_control::application::settings_service::SETTING_VALUE_MAX_BYTES
            + 1,
    );
    let body = serde_json::json!({ "value": oversized }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        "/api/v1/settings/user/test.settings.oversized",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("oversized value");
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
