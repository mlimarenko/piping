use super::*;

#[tokio::test]
#[ignore]
async fn instance_branding_get_is_public() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);

    let (status, body) = request(&app, "GET", "/api/v1/instance/branding", None, None)
        .await
        .expect("get branding without auth");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("branding json");
    assert!(
        json.get("app_name")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.trim().is_empty()),
        "public branding response must include a display name"
    );
    assert!(
        json.get("theme").is_some_and(serde_json::Value::is_object),
        "public branding response must include theme tokens"
    );
}

#[tokio::test]
#[ignore]
async fn config_contexts_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let create_body = serde_json::json!({
        "name": format!("ctx-{}", suffix),
        "weight": 100,
        "is_active": true,
        "data": {
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": "127.0.0.1",
                        "port": 22,
                        "credentials": {
                            "username": "root"
                        }
                    }]
                }
            }
        }
    })
    .to_string();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/config-contexts",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("create context");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let context_id = created
        .get("id")
        .and_then(|value| value.as_str())
        .expect("context id");

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/config-contexts/{context_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get context");
    assert_eq!(status, StatusCode::OK);
    let fetched: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        fetched.get("name").and_then(|value| value.as_str()),
        created.get("name").and_then(|value| value.as_str())
    );

    let update_body = serde_json::json!({
        "name": format!("ctx-updated-{}", suffix),
        "weight": 90,
        "is_active": false,
        "data": {
            "piping": {
                "connections": {
                    "app": [{
                        "url": "https://127.0.0.1:8080",
                        "label": "ui"
                    }]
                }
            }
        }
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/config-contexts/{context_id}"),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("update context");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/config-contexts/{context_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete context");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn config_context_create_rejects_unknown_protocol_schema_with_422() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let payload = serde_json::json!({
        "name": format!("ctx-invalid-{}", unique_suffix()),
        "weight": 10,
        "is_active": true,
        "data": {
            "piping": {
                "connections": {
                    "legacy_proto": [{
                        "host": "127.0.0.1"
                    }]
                }
            }
        }
    })
    .to_string();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/config-contexts",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create context with unknown protocol");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("validation json");
    assert_eq!(
        json.get("error").and_then(|value| value.as_str()),
        Some("VALIDATION_ERROR")
    );
}

#[tokio::test]
#[ignore]
async fn config_contexts_with_non_standard_fields_remain_readable() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state.clone());
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let context_id = Uuid::now_v7();
    let context_data = serde_json::json!({
        "piping": {
            "connections": {
                "ssh": [{
                    "host": "10.60.0.10",
                    "legacy_option": "legacy-value"
                }],
                "legacy_proto": [{
                    "custom_field": "still-readable"
                }]
            }
        }
    });
    sqlx::query(
        r#"
        INSERT INTO config_context (id, name, weight, data, is_active)
        VALUES ($1, $2, $3, $4, true)
        "#,
    )
    .bind(context_id)
    .bind(format!("legacy-read-context-{}", unique_suffix()))
    .bind(100_i32)
    .bind(&context_data)
    .execute(&state.db_pool)
    .await
    .expect("insert legacy context");

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/config-contexts/{context_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get legacy context");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("context json");
    assert_eq!(
        json.pointer("/data/piping/connections/ssh/0/legacy_option")
            .and_then(|value| value.as_str()),
        Some("legacy-value")
    );
    assert_eq!(
        json.pointer("/data/piping/connections/legacy_proto/0/custom_field")
            .and_then(|value| value.as_str()),
        Some("still-readable")
    );
}

#[tokio::test]
#[ignore]
async fn config_context_bindings_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let tenant_name = format!("ctx-binding-tenant-{suffix}");
    let tenant_body = serde_json::json!({
        "name": tenant_name,
        "slug": format!("ctx-binding-tenant-{suffix}")
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/tenants",
        Some(&tenant_body),
        Some(&cookie),
    )
    .await
    .expect("create tenant");
    assert_eq!(status, StatusCode::CREATED);
    let tenant_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("tenant json")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("tenant id")
        .to_string();

    let context_body = serde_json::json!({
        "name": format!("ctx-binding-{suffix}"),
        "weight": 100,
        "is_active": true,
        "data": { "piping": { "connections": { "ssh": [] } } }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/config-contexts",
        Some(&context_body),
        Some(&cookie),
    )
    .await
    .expect("create context");
    assert_eq!(status, StatusCode::CREATED);
    let context_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("context json")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("context id")
        .to_string();

    let create_binding_body = serde_json::json!({
        "context_id": context_id,
        "scope_type": "tenant",
        "scope_id": tenant_id
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/config-context-bindings",
        Some(&create_binding_body),
        Some(&cookie),
    )
    .await
    .expect("create binding");
    assert_eq!(status, StatusCode::CREATED);
    let binding_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("binding json")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("binding id")
        .to_string();

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/config-context-bindings?context_id={context_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("list bindings");
    assert_eq!(status, StatusCode::OK);
    let bindings: serde_json::Value = serde_json::from_slice(&body).expect("bindings json");
    let binding_items = bindings
        .get("items")
        .and_then(|value| value.as_array())
        .expect("bindings items array");
    assert!(
        binding_items.iter().any(
            |item| item.get("id").and_then(|value| value.as_str()) == Some(binding_id.as_str())
        )
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/config-contexts?scope_type=tenant&scope_id={tenant_id}&include_hierarchy=false"
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("list config contexts for scope");
    assert_eq!(status, StatusCode::OK);
    let contexts: serde_json::Value = serde_json::from_slice(&body).expect("contexts json");
    let context_items = contexts
        .get("items")
        .and_then(|value| value.as_array())
        .expect("contexts items array");
    assert!(
        context_items.iter().any(
            |item| item.get("id").and_then(|value| value.as_str()) == Some(context_id.as_str())
        )
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/config-context-bindings/{binding_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete binding");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn secret_bindings_crud_returns_metadata_only() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let tenant_body = serde_json::json!({
        "name": format!("secret-binding-tenant-{suffix}"),
        "slug": format!("secret-binding-tenant-{suffix}")
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/tenants",
        Some(&tenant_body),
        Some(&cookie),
    )
    .await
    .expect("create tenant");
    assert_eq!(status, StatusCode::CREATED);
    let tenant_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("tenant json")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("tenant id")
        .to_string();

    let consumer_id = Uuid::now_v7().to_string();
    let field_key = format!("rest.secret.field.{}", unique_suffix());
    let set_payload = serde_json::json!({
        "consumer_type": "inventory_custom_field",
        "consumer_id": consumer_id,
        "owner_scope_type": "tenant",
        "owner_scope_id": tenant_id,
        "field_key": field_key,
        "intent": "set",
        "value": format!("secret-value-{}", unique_suffix())
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&set_payload),
        Some(&cookie),
    )
    .await
    .expect("set secret field");
    assert_eq!(status, StatusCode::OK);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("secret field json");
    assert_eq!(
        created.get("field_key").and_then(|value| value.as_str()),
        Some(field_key.as_str())
    );
    assert_eq!(
        created.get("has_value").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(
        created
            .get("secret_id")
            .and_then(|value| value.as_str())
            .is_some()
    );
    assert!(
        created.get("value").is_none(),
        "secret field response must not expose secret value"
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/secret-fields?consumer_type=inventory_custom_field&consumer_id={consumer_id}"
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("list secret fields");
    assert_eq!(status, StatusCode::OK);
    let items: serde_json::Value = serde_json::from_slice(&body).expect("secret field list json");
    let item = items
        .as_array()
        .expect("secret fields array")
        .iter()
        .find(|entry| {
            entry.get("field_key").and_then(|value| value.as_str()) == Some(field_key.as_str())
        })
        .expect("created secret field state");
    assert_eq!(
        item.get("has_value").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(item.get("value").is_none());

    let clear_payload = serde_json::json!({
        "consumer_type": "inventory_custom_field",
        "consumer_id": consumer_id,
        "owner_scope_type": "tenant",
        "owner_scope_id": tenant_id,
        "field_key": field_key,
        "intent": "clear"
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&clear_payload),
        Some(&cookie),
    )
    .await
    .expect("clear secret field");
    assert_eq!(status, StatusCode::OK);
    let cleared: serde_json::Value =
        serde_json::from_slice(&body).expect("secret field clear json");
    assert_eq!(
        cleared.get("enabled").and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        cleared.get("has_value").and_then(|value| value.as_bool()),
        Some(false)
    );
}

#[tokio::test]
#[ignore]
async fn inventory_bulk_create_reports_partial_errors() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let body = serde_json::json!({
        "items": [
            {
                "name": format!("bulk-tenant-group-{suffix}"),
                "slug": format!("bulk-tenant-group-{suffix}")
            },
            {
                "name": "",
                "slug": format!("bulk-tenant-group-invalid-{suffix}")
            }
        ]
    })
    .to_string();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/tenant_groups/bulk",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("bulk create tenant groups");
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let created = json
        .get("created")
        .and_then(|value| value.as_array())
        .expect("created array");
    let errors = json
        .get("errors")
        .and_then(|value| value.as_array())
        .expect("errors array");
    assert_eq!(created.len(), 1);
    assert_eq!(errors.len(), 1);
    // push_bulk_error reports a 1-based index on the wire (row numbers that
    // match human-friendly UIs); the invalid item is at array position 1.
    assert_eq!(
        errors
            .first()
            .and_then(|item| item.get("index"))
            .and_then(|value| value.as_u64()),
        Some(2)
    );
}

#[tokio::test]
#[ignore]
async fn access_control_get_by_id_routes_return_entities() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();

    let user_body = serde_json::json!({
        "username": format!("rest-get-user-{suffix}"),
        "email": format!("rest-get-user-{suffix}@test.local"),
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
    .expect("create user");
    assert_eq!(status, StatusCode::OK);
    let user_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("user json")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("user id")
        .to_string();

    let group_body = serde_json::json!({
        "name": format!("rest-get-group-{suffix}")
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/groups",
        Some(&group_body),
        Some(&cookie),
    )
    .await
    .expect("create group");
    assert_eq!(status, StatusCode::OK);
    let group_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("group json")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("group id")
        .to_string();

    let role_body = serde_json::json!({
        "name": format!("rest-get-role-{suffix}"),
        "description": "role for get route test"
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
    .expect("create role");
    assert_eq!(status, StatusCode::OK);
    let role_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("role json")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("role id")
        .to_string();

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/admin/access-control/users/{user_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get user by id");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/admin/access-control/groups/{group_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get group by id");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/admin/access-control/roles/{role_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get role by id");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "GET",
        "/api/v1/admin/access-control/policies/00000000-0000-7000-8000-000000000003",
        None,
        Some(&cookie),
    )
    .await
    .expect("get policy by id");
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn oidc_settings_routes_return_metadata_only_secret_state() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let update_payload = serde_json::json!({
        "enabled": false,
        "issuer_url": "https://issuer.example.test/realms/main",
        "client_id": "piping-client-rest",
        "client_secret_enabled": true,
        "client_secret_value": format!("oidc-rest-secret-{}", unique_suffix()),
        "redirect_url": "https://piping.example.test/api/v1/auth/sso/callback",
        "scopes": ["openid", "profile"],
        "group_claim": "groups",
        "provisioning_mode": "manual",
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        "/api/v1/admin/auth/oidc/settings",
        Some(&update_payload),
        Some(&cookie),
    )
    .await
    .expect("update oidc settings");
    assert_eq!(status, StatusCode::OK);
    let updated: serde_json::Value = serde_json::from_slice(&body).expect("oidc update json");
    assert!(updated.get("client_secret").is_none());
    assert!(updated.get("client_secret_value").is_none());
    assert_eq!(
        updated
            .get("client_secret_state")
            .and_then(|value| value.get("enabled"))
            .and_then(|value| value.as_bool()),
        Some(true),
    );
    assert_eq!(
        updated
            .get("client_secret_state")
            .and_then(|value| value.get("has_value"))
            .and_then(|value| value.as_bool()),
        Some(true),
    );

    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/admin/auth/oidc/settings",
        None,
        Some(&cookie),
    )
    .await
    .expect("get oidc settings");
    assert_eq!(status, StatusCode::OK);
    let fetched: serde_json::Value = serde_json::from_slice(&body).expect("oidc get json");
    assert!(fetched.get("client_secret").is_none());
    assert!(fetched.get("client_secret_value").is_none());
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn providers_route_payload_hides_sensitive_config_fields() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let create_payload = serde_json::json!({
        "name": format!("rest-provider-{}", unique_suffix()),
        "provider_type": "netbox",
        "is_enabled": true,
        "bootstrap": false,
        "config": {
            "base_url": REST_TEST_NETBOX_BASE_URL,
            "sync_mode": "pull_only",
            "page_size": REST_TEST_NETBOX_PAGE_SIZE,
            "webhook_events": ["created"],
            "webhook_object_types": [1],
            "api_token": format!("rest-api-token-{}", unique_suffix()),
            "webhook_secret": format!("rest-webhook-secret-{}", unique_suffix()),
        }
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/providers",
        Some(&create_payload),
        Some(&cookie),
    )
    .await
    .expect("create provider");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("provider json");
    let provider_id = created
        .get("id")
        .and_then(|value| value.as_str())
        .expect("provider id");

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/providers/{provider_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get provider");
    assert_eq!(status, StatusCode::OK);
    let provider: serde_json::Value = serde_json::from_slice(&body).expect("provider json");
    let config = provider.get("config").and_then(|value| value.as_object());
    let api_token = config.and_then(|value| value.get("api_token"));
    let webhook_secret = config.and_then(|value| value.get("webhook_secret"));
    assert!(
        api_token.is_none() || api_token == Some(&serde_json::Value::Null),
        "provider response must not expose plaintext api_token",
    );
    assert!(
        webhook_secret.is_none() || webhook_secret == Some(&serde_json::Value::Null),
        "provider response must not expose plaintext webhook_secret",
    );
}
