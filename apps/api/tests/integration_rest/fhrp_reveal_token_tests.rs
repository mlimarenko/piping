use super::*;
use serde_json::{Value, json};

async fn session_json(
    app: &Router,
    cookie: &str,
    method: &str,
    path: &str,
    payload: Option<Value>,
    expected: StatusCode,
) -> Value {
    let payload = payload.map(|value| value.to_string());
    let (status, body) = request(app, method, path, payload.as_deref(), Some(cookie))
        .await
        .expect("session request");
    // Auth and reveal responses may contain credentials; never print the body.
    assert_eq!(status, expected, "{method} {path}");
    if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON response")
    }
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_reveal_token_tests filter"]
async fn fhrp_reveal_requires_read_and_object_scoped_secret_token_permissions() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("privileged user login");
    let types = session_json(
        &app,
        &cookie,
        "GET",
        "/api/v1/inventory/content-types",
        None,
        StatusCode::OK,
    )
    .await;
    let content_type = types
        .as_array()
        .expect("content types")
        .iter()
        .find(|item| item["token"] == "ipam.fhrpgroup")
        .expect("FHRP content type");
    let field_name = format!("fhrp_token_secret_{}", Uuid::now_v7().simple());
    let definition = session_json(
        &app,
        &cookie,
        "POST",
        "/api/v1/custom-fields",
        Some(json!({
            "name": field_name, "label": field_name, "field_type": "text",
            "is_secret": true, "object_types": [content_type["id"]]
        })),
        StatusCode::CREATED,
    )
    .await;
    let cleartext = format!("fixture-{}", Uuid::now_v7());
    let group = session_json(
        &app,
        &cookie,
        "POST",
        "/api/v1/inventory/fhrp-groups",
        Some(json!({
            "group_id": 42, "name": format!("fhrp-token-{}", unique_suffix()),
            "protocol": "vrrp3", "custom_fields": {(field_name.clone()): cleartext}
        })),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(group["custom_fields"][&field_name], "****");
    let id = group["id"].as_str().expect("group UUID");
    let group_path = format!("/api/v1/inventory/fhrp-groups/{id}");
    let reveal_path = format!("{group_path}/custom-fields/reveal");

    // The token owner can reveal the value; token scopes must still narrow it.
    let revealed = session_json(&app, &cookie, "POST", &reveal_path, None, StatusCode::OK).await;
    assert!(revealed["custom_fields"][&field_name].as_str() == Some(cleartext.as_str()));

    let read_scope = json!({
        "action": "read", "resource_type": "inventory",
        "resource_pattern": format!("inventory:{id}")
    });
    let secret_scope = json!({
        "action": "secret", "resource_type": "inventory",
        "resource_pattern": format!("inventory:{id}")
    });
    let other_secret_scope = json!({
        "action": "secret", "resource_type": "inventory",
        "resource_pattern": format!("inventory:{}", Uuid::now_v7())
    });
    for (case, scopes, expected) in [
        (
            "read-only",
            json!([read_scope.clone()]),
            StatusCode::FORBIDDEN,
        ),
        (
            "read-secret",
            json!([read_scope.clone(), secret_scope.clone()]),
            StatusCode::OK,
        ),
        (
            "secret-other-object",
            json!([read_scope, other_secret_scope]),
            StatusCode::FORBIDDEN,
        ),
        ("secret-only", json!([secret_scope]), StatusCode::FORBIDDEN),
    ] {
        let token = session_json(
            &app,
            &cookie,
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(json!({
                "name": format!("fhrp-{case}-{}", unique_suffix()), "scopes": scopes
            })),
            StatusCode::CREATED,
        )
        .await;
        let bearer = token["token_value"].as_str().expect("token value");
        if case != "secret-only" {
            let (status, body) =
                request_with_auth(&app, "GET", &group_path, None, None, Some(bearer))
                    .await
                    .expect("token read");
            assert_eq!(status, StatusCode::OK, "{case}");
            let masked: Value = serde_json::from_slice(&body).expect("masked group");
            assert_eq!(masked["custom_fields"][&field_name], "****");
        }
        let (status, body) =
            request_with_auth(&app, "POST", &reveal_path, None, None, Some(bearer))
                .await
                .expect("token reveal");
        assert_eq!(status, expected, "{case}");
        let response: Value = serde_json::from_slice(&body).expect("reveal response");
        if expected == StatusCode::OK {
            assert!(response["custom_fields"][&field_name].as_str() == Some(cleartext.as_str()));
        } else {
            assert_eq!(response["code"], "TOKEN_SCOPE_DENIED", "{case}");
            assert!(!String::from_utf8_lossy(&body).contains(&cleartext));
        }
        let token_id = token["id"].as_str().expect("token UUID");
        session_json(
            &app,
            &cookie,
            "POST",
            &format!("/api/v1/user/settings/api-tokens/{token_id}/revoke"),
            None,
            StatusCode::OK,
        )
        .await;
    }
    session_json(
        &app,
        &cookie,
        "DELETE",
        &group_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    let definition_id = definition["id"].as_str().expect("definition UUID");
    session_json(
        &app,
        &cookie,
        "DELETE",
        &format!("/api/v1/custom-fields/{definition_id}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
}
