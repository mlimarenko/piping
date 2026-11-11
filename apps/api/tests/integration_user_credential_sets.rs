//! Integration tests for `/api/v1/user/settings/credentials`.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_user_credential_sets -- --ignored`

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie, request,
};
use serde_json::Value;
use uuid::Uuid;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_user_credential_sets -- --ignored
async fn user_credential_sets_crud_and_default_switch_flow() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let payload_a = serde_json::json!({
        "protocol": "ssh",
        "name": "Admin set",
        "params": {
            "port": 2222,
            "credentials": {
                "username": "admin"
            }
        },
        "is_default": true
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&payload_a),
        Some(&cookie),
    )
    .await
    .expect("create set a");
    assert_eq!(status, StatusCode::CREATED);
    let set_a: Value = serde_json::from_slice(&body).expect("set a json");
    let set_a_id = set_a
        .get("id")
        .and_then(Value::as_str)
        .expect("set a id")
        .to_string();
    assert_eq!(
        set_a
            .get("params")
            .and_then(|params| params.get("credentials"))
            .and_then(|credentials| credentials.get("username"))
            .and_then(Value::as_str),
        Some("admin")
    );

    let payload_b = serde_json::json!({
        "protocol": "ssh",
        "name": "Readonly set",
        "params": {
            "credentials": {
                "username": "readonly"
            }
        },
        "is_default": false
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&payload_b),
        Some(&cookie),
    )
    .await
    .expect("create set b");
    assert_eq!(status, StatusCode::CREATED);
    let set_b: Value = serde_json::from_slice(&body).expect("set b json");
    let set_b_id = set_b
        .get("id")
        .and_then(Value::as_str)
        .expect("set b id")
        .to_string();

    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/user/settings/credentials/{}/default", set_b_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("set default");
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty());

    let update_payload = serde_json::json!({
        "name": "Readonly set (updated)",
        "params": {
            "credentials": {
                "username": "readonly-updated"
            },
            "port": 2022
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/user/settings/credentials/{}", set_b_id),
        Some(&update_payload),
        Some(&cookie),
    )
    .await
    .expect("update set");
    assert_eq!(status, StatusCode::OK);
    let updated: Value = serde_json::from_slice(&body).expect("updated json");
    assert_eq!(
        updated.get("name").and_then(Value::as_str),
        Some("Readonly set (updated)")
    );
    assert_eq!(
        updated.get("is_default").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        updated
            .get("params")
            .and_then(|params| params.get("credentials"))
            .and_then(|credentials| credentials.get("username"))
            .and_then(Value::as_str),
        Some("readonly-updated")
    );

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/user/settings/credentials?protocol=ssh",
        None,
        Some(&cookie),
    )
    .await
    .expect("list sets");
    assert_eq!(status, StatusCode::OK);
    let list_json: Value = serde_json::from_slice(&body).expect("list json");
    let items = list_json
        .get("items")
        .and_then(Value::as_array)
        .expect("items array");
    assert!(
        items.len() >= 2,
        "expected at least 2 sets, got {}",
        items.len()
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/user/settings/credentials/{}", set_b_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete default set");
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "default set must not be deletable"
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/user/settings/credentials/{}", set_a_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete non-default set");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_user_credential_sets -- --ignored
async fn user_credential_sets_enforce_single_default_and_foreign_set_restrictions() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let payload_a = serde_json::json!({
        "protocol": "ssh",
        "name": "Default A",
        "params": {},
        "is_default": true
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&payload_a),
        Some(&cookie),
    )
    .await
    .expect("create set a");
    assert_eq!(status, StatusCode::CREATED);
    let set_a: Value = serde_json::from_slice(&body).expect("set a json");
    let set_a_id = set_a
        .get("id")
        .and_then(Value::as_str)
        .expect("set a id")
        .to_string();

    let payload_b = serde_json::json!({
        "protocol": "ssh",
        "name": "Default B",
        "params": {},
        "is_default": true
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&payload_b),
        Some(&cookie),
    )
    .await
    .expect("create set b");
    assert_eq!(status, StatusCode::CREATED);
    let set_b: Value = serde_json::from_slice(&body).expect("set b json");
    let set_b_id = set_b
        .get("id")
        .and_then(Value::as_str)
        .expect("set b id")
        .to_string();

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/user/settings/credentials?protocol=ssh",
        None,
        Some(&cookie),
    )
    .await
    .expect("list sets");
    assert_eq!(status, StatusCode::OK);
    let list_json: Value = serde_json::from_slice(&body).expect("list json");
    let items = list_json
        .get("items")
        .and_then(Value::as_array)
        .expect("items array");
    let default_items: Vec<&Value> = items
        .iter()
        .filter(|item| item.get("is_default").and_then(Value::as_bool) == Some(true))
        .collect();
    assert_eq!(default_items.len(), 1, "exactly one default set expected");

    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/user/settings/credentials/{}/default", set_b_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("set explicit default b");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/user/settings/credentials?protocol=ssh",
        None,
        Some(&cookie),
    )
    .await
    .expect("list sets after explicit default");
    assert_eq!(status, StatusCode::OK);
    let list_json: Value = serde_json::from_slice(&body).expect("list json after explicit default");
    let items = list_json
        .get("items")
        .and_then(Value::as_array)
        .expect("items array after explicit default");
    let default_items: Vec<&Value> = items
        .iter()
        .filter(|item| item.get("is_default").and_then(Value::as_bool) == Some(true))
        .collect();
    assert_eq!(
        default_items.len(),
        1,
        "exactly one default set expected after explicit switch"
    );
    assert_eq!(
        default_items[0].get("id").and_then(Value::as_str),
        Some(set_b_id.as_str()),
        "explicit default endpoint should switch default set deterministically"
    );

    let pool = build_test_db_pool().await.expect("db pool");
    let foreign_user_id = Uuid::now_v7();
    let foreign_set_id = Uuid::now_v7();
    let foreign_username = format!("foreign-user-{}", Uuid::now_v7());
    let foreign_email = format!("{foreign_username}@test.local");
    sqlx::query(
        r#"
        INSERT INTO identity_user (id, username, email, status, auth_source)
        VALUES ($1, $2, $3, 'active', 'local')
        "#,
    )
    .bind(foreign_user_id)
    .bind(&foreign_username)
    .bind(&foreign_email)
    .execute(&pool)
    .await
    .expect("insert foreign user");
    sqlx::query(
        r#"
        INSERT INTO connections_user_credential_set (id, user_id, protocol, name, is_default)
        VALUES ($1, $2, 'ssh', 'Foreign Set', false)
        "#,
    )
    .bind(foreign_set_id)
    .bind(foreign_user_id)
    .execute(&pool)
    .await
    .expect("insert foreign set");

    let update_payload = serde_json::json!({
        "name": "foreign-updated",
        "params": {}
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/user/settings/credentials/{}", foreign_set_id),
        Some(&update_payload),
        Some(&cookie),
    )
    .await
    .expect("update foreign set");
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request(
        &app,
        "POST",
        &format!(
            "/api/v1/user/settings/credentials/{}/default",
            foreign_set_id
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("set default for foreign set");
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/user/settings/credentials/{}", foreign_set_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete foreign set");
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/user/settings/credentials/{}", set_a_id),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup set a");
    assert_eq!(status, StatusCode::NO_CONTENT);
}
