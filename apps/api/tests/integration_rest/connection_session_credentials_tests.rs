use super::common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state_with, login_and_get_cookie,
    request, seed_device, unique_suffix,
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

async fn assert_owner_credential_status(
    app: &axum::Router,
    id: Uuid,
    cookie: &str,
    password: &str,
    expected: StatusCode,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "GET",
        &format!("/api/v1/connections/sessions/{id}/client-credentials"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(
        status, expected,
        "credential availability must follow persisted session state"
    );
    if status == StatusCode::OK {
        let response: Value = serde_json::from_slice(&body)?;
        assert!(
            response["credentials"]["password"].as_str() == Some(password),
            "failed terminal dependencies must preserve the unsaved password"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn connection_session_credentials_failed_terminal_dependencies_preserve_replay()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::response::IntoResponse;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let state = build_test_app_state_with(|settings| {
        settings.server.bind = address.to_string();
        settings.auth.local_enabled = true;
    })
    .await
    .map_err(|_| "fixture initialization failed")?;
    let pool = state.db_pool.clone();
    let service = state.services.connection.clone();
    let target_id = seed_local_connection_target(&pool).await?;
    let actor = Uuid::parse_str("00000000-0000-7000-8000-000000000001")?;
    let app = piping_backend::app::build_router(state);
    let fail_delete = Arc::new(AtomicBool::new(false));
    let delete_attempts = Arc::new(AtomicUsize::new(0));
    let fail = fail_delete.clone();
    let attempts = delete_attempts.clone();
    // Exercise the real HttpProxyClient against the local router. Only the
    // dependency's DELETE response is injected; credential GET remains real.
    let served = app.clone().layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let fail = fail.clone();
            let attempts = attempts.clone();
            async move {
                if request.method() == axum::http::Method::DELETE
                    && request.uri().path().starts_with("/proxy/v1/connections/")
                {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    if fail.load(Ordering::SeqCst) {
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                }
                next.run(request).await
            }
        },
    ));
    let server = tokio::spawn(async move { axum::serve(listener, served).await });
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("login")?;
    let record = json!({"name": format!("terminal-failure-{}", unique_suffix()), "protocol": "vnc", "params": {"host": "127.0.0.1", "port": 5900}}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connection-config/records",
        Some(&record),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let record: Value = serde_json::from_slice(&body)?;
    let assignment =
        json!({"record_id": record["id"], "scope_type": "device", "scope_id": target_id})
            .to_string();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/connection-config/assignments",
        Some(&assignment),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let password = Uuid::new_v4().simple().to_string()[..8].to_string();
    let payload = json!({"target_id": target_id, "protocol": "vnc", "params": {"credentials": {"password": password}}}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let response: Value = serde_json::from_slice(&body)?;
    let id = Uuid::parse_str(response["session"]["id"].as_str().ok_or("session id")?)?;
    assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::OK).await?;
    service.mark_session_active(id, actor).await?;
    let disabled_node = service.create_proxy_node(piping_backend::contexts::connections::application::connection_service::CreateProxyNodeInput {
        name: format!("disabled-route-{}", unique_suffix()), slug: format!("disabled-route-{}", unique_suffix()),
        base_url: format!("http://{address}"), enabled: false,
    }).await.map_err(|_| "disabled-route fixture initialization failed")?;
    sqlx::query("UPDATE connections_session SET proxy_node_id = $2 WHERE id = $1")
        .bind(id)
        .bind(disabled_node.id)
        .execute(&pool)
        .await?;
    assert!(
        matches!(
            service.disconnect_session(id, actor).await,
            Err(piping_backend::errors::AppError::Validation(_))
        ),
        "disabled route failure must propagate"
    );
    assert_eq!(service.require_session(id).await?.status.as_str(), "active");
    assert_eq!(delete_attempts.load(Ordering::SeqCst), 0);
    assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::OK).await?;
    sqlx::query("UPDATE connections_session SET proxy_node_id = NULL WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    fail_delete.store(true, Ordering::SeqCst);
    assert!(
        service.disconnect_session(id, actor).await.is_err(),
        "proxy failure must propagate"
    );
    assert_eq!(service.require_session(id).await?.status.as_str(), "active");
    assert_eq!(
        delete_attempts.load(Ordering::SeqCst),
        1,
        "proxy failure must actually be injected"
    );
    assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::OK).await?;
    assert!(
        service.terminate_session(id, actor).await.is_err(),
        "proxy failure must propagate"
    );
    assert_eq!(delete_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(service.require_session(id).await?.status.as_str(), "active");
    assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::OK).await?;
    fail_delete.store(false, Ordering::SeqCst);

    // Native persistence failure, scoped to this isolated test database. Every
    // live terminal operation must leave its replay intact if status is rejected.
    sqlx::query("ALTER TABLE connections_session ADD CONSTRAINT pwa97_reject_terminal_status CHECK (status IN ('pending', 'active'))").execute(&pool).await?;
    for operation in ["fail", "mark-disconnected", "disconnect", "terminate"] {
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/connections/sessions",
            Some(&payload),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let response: Value = serde_json::from_slice(&body)?;
        let id = Uuid::parse_str(response["session"]["id"].as_str().ok_or("session id")?)?;
        let result = match operation {
            "fail" => service.mark_session_failed(id, actor).await,
            "mark-disconnected" => service.mark_session_disconnected(id, actor).await,
            "disconnect" => service.disconnect_session(id, actor).await,
            _ => service.terminate_session(id, actor).await,
        };
        assert!(
            matches!(result, Err(piping_backend::errors::AppError::Database(_))),
            "terminal status failure must propagate as a database error"
        );
        assert_eq!(
            service.require_session(id).await?.status.as_str(),
            "pending"
        );
        assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::OK).await?;
    }
    sqlx::query("ALTER TABLE connections_session DROP CONSTRAINT pwa97_reject_terminal_status")
        .execute(&pool)
        .await?;
    // The original proxy was never terminated by the injected failures.
    service.disconnect_session(id, actor).await?;
    assert_eq!(
        service.require_session(id).await?.status.as_str(),
        "disconnected"
    );
    assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::NOT_FOUND).await?;
    assert!(
        service.disconnect_session(id, actor).await.is_ok(),
        "already-terminal disconnect is idempotent"
    );
    assert!(
        service.terminate_session(id, actor).await.is_ok(),
        "already-terminal terminate is idempotent"
    );
    assert!(
        service.mark_session_failed(id, actor).await.is_ok(),
        "already-terminal failure is idempotent"
    );
    assert!(
        service.mark_session_disconnected(id, actor).await.is_ok(),
        "already-terminal mark-disconnected is idempotent"
    );
    assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::NOT_FOUND).await?;
    for (operation, expected) in [
        ("fail", "failed"),
        ("mark-disconnected", "disconnected"),
        ("disconnect", "disconnected"),
        ("terminate", "terminated"),
    ] {
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/connections/sessions",
            Some(&payload),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let response: Value = serde_json::from_slice(&body)?;
        let id = Uuid::parse_str(response["session"]["id"].as_str().ok_or("session id")?)?;
        let result = match operation {
            "fail" => service.mark_session_failed(id, actor).await,
            "mark-disconnected" => service.mark_session_disconnected(id, actor).await,
            "disconnect" => service.disconnect_session(id, actor).await,
            _ => service.terminate_session(id, actor).await,
        };
        assert!(
            result.is_ok(),
            "successful terminal dependencies must permit the transition"
        );
        assert_eq!(service.require_session(id).await?.status.as_str(), expected);
        assert_owner_credential_status(&app, id, &cookie, &password, StatusCode::NOT_FOUND).await?;
    }
    server.abort();
    let _ = server.await;
    Ok(())
}

async fn seed_local_connection_target(
    pool: &sqlx::PgPool,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let (target_id, _) = seed_device(pool, "session-credentials").await?;
    let ip_id = Uuid::now_v7();
    let interface_id = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments) VALUES ($1, '{}', '127.0.0.1/32', 'active', '', '', '')")
        .bind(ip_id).execute(pool).await?;
    sqlx::query("INSERT INTO dcim_interface (id, custom_field_data, device_id, name, _name, label, description, type, enabled, mgmt_only, mark_connected) VALUES ($1, '{}', $2, 'vnc-proof', 'vnc-proof', '', '', 'virtual', true, false, false)")
        .bind(interface_id).bind(target_id).execute(pool).await?;
    sqlx::query("UPDATE ipam_ipaddress SET assigned_object_id = $2, assigned_object_type_id = (SELECT id FROM inventory_content_type WHERE app_label = 'dcim' AND model = 'interface') WHERE id = $1")
        .bind(ip_id).bind(interface_id).execute(pool).await?;
    sqlx::query("UPDATE dcim_device SET primary_ip4_id = $2 WHERE id = $1")
        .bind(target_id)
        .bind(ip_id)
        .execute(pool)
        .await?;
    Ok(target_id)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn connection_session_credentials_pinned_account_survives_reorder()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let state = build_test_app_state_with(|settings| {
        settings.server.bind = address.to_string();
        settings.auth.local_enabled = true;
    })
    .await
    .map_err(|_| "fixture initialization failed")?;
    let target_id = seed_local_connection_target(&state.db_pool).await?;
    let app = piping_backend::app::build_router(state);
    let served_app = app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, served_app).await });
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("login")?;
    let password_a = Uuid::new_v4().simple().to_string()[..8].to_string();
    let password_b = Uuid::new_v4().simple().to_string()[..8].to_string();
    let mut records = Vec::new();
    let mut assignments = Vec::new();
    for (name, password, weight) in [
        ("account-a", &password_a, 20),
        ("account-b", &password_b, 0),
    ] {
        let payload = json!({"name": format!("{name}-{}", unique_suffix()), "protocol": "rdp", "params": {"host": "127.0.0.1", "port": 3389, "credentials": {"username": name, "password": password}}}).to_string();
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/connection-config/records",
            Some(&payload),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED);
        let record: Value = serde_json::from_slice(&body)?;
        let payload = json!({"record_id": record["id"], "scope_type": "device", "scope_id": target_id, "weight": weight}).to_string();
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/connection-config/assignments",
            Some(&payload),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED);
        records.push(record);
        assignments.push(serde_json::from_slice::<Value>(&body)?);
    }
    let launch_uri = format!("/api/v1/connections/targets/{target_id}/launch-options");
    let (status, body) = request(&app, "GET", &launch_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let options: Value = serde_json::from_slice(&body)?;
    let accounts = options["protocols"]
        .as_array()
        .ok_or("protocols")?
        .iter()
        .find(|item| item["protocol"] == "rdp")
        .and_then(|item| item["accounts"].as_array())
        .ok_or("accounts")?;
    assert_eq!(accounts.len(), 2);
    assert!(accounts[0]["source"]["record_name"] == records[0]["name"]);
    let payload = json!({"target_id": target_id, "protocol": "rdp", "metadata": {"entryId": accounts[0]["entry_id"]}}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let session: Value = serde_json::from_slice(&body)?;
    let id = session["session"]["id"].as_str().ok_or("session id")?;
    let credential_uri = format!("/api/v1/connections/sessions/{id}/client-credentials");
    let assignment_id = assignments[0]["id"].as_str().ok_or("assignment id")?;
    let promoted_assignment_id = assignments[1]["id"]
        .as_str()
        .ok_or("promoted assignment id")?;
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/connection-config/assignments/{promoted_assignment_id}"),
        Some(&json!({"weight": 40}).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = request(&app, "GET", &launch_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let reordered: Value = serde_json::from_slice(&body)?;
    let accounts = reordered["protocols"]
        .as_array()
        .ok_or("protocols")?
        .iter()
        .find(|item| item["protocol"] == "rdp")
        .and_then(|item| item["accounts"].as_array())
        .ok_or("accounts")?;
    assert!(
        accounts[0]["source"]["record_name"] == records[1]["name"],
        "fixture must really change the current launch order"
    );
    let (status, body) = request(&app, "GET", &credential_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let credentials: Value = serde_json::from_slice(&body)?;
    assert!(
        credentials["credentials"]["password"].as_str() == Some(password_a.as_str()),
        "GET must use pinned record ID rather than the old index"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/connection-config/assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let record_id = records[0]["id"].as_str().ok_or("record id")?;
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/connection-config/records/{record_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(&app, "GET", &credential_uri, None, Some(&cookie)).await?;
    server.abort();
    let _ = server.await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "deleted pinned account must not switch to the remaining account"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn connection_session_credentials_saved_literal_clear_revokes_snapshot()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let state = build_test_app_state_with(|settings| {
        settings.server.bind = address.to_string();
        settings.auth.local_enabled = true;
    })
    .await
    .map_err(|_| "fixture initialization failed")?;
    let target_id = seed_local_connection_target(&state.db_pool).await?;
    let app = piping_backend::app::build_router(state);
    let served_app = app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, served_app).await });
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("login")?;
    let password = Uuid::new_v4().simple().to_string()[..8].to_string();
    let params = json!({"host": "127.0.0.1", "credentials": {"password": password}});
    let record_payload = json!({"name": format!("literal-clear-{}", unique_suffix()), "protocol": "vnc", "params": params}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connection-config/records",
        Some(&record_payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create saved literal fixture");
    let record: Value = serde_json::from_slice(&body)?;
    let record_id = record["id"].as_str().ok_or("record id")?;
    let assignment =
        json!({"record_id": record_id, "scope_type": "device", "scope_id": target_id}).to_string();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/connection-config/assignments",
        Some(&assignment),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets/{target_id}/launch-options"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let options: Value = serde_json::from_slice(&body)?;
    let account = options["protocols"]
        .as_array()
        .ok_or("protocols")?
        .iter()
        .find(|item| item["protocol"] == "vnc")
        .and_then(|item| item["accounts"].as_array())
        .and_then(|items| items.first())
        .ok_or("launch account")?;
    let payload = json!({"target_id": target_id, "protocol": "vnc", "metadata": {"entryId": account["entry_id"]}}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let created: Value = serde_json::from_slice(&body)?;
    let session_id = created["session"]["id"].as_str().ok_or("session id")?;
    let uri = format!("/api/v1/connections/sessions/{session_id}/client-credentials");
    let (status, body) = request(&app, "GET", &uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "saved literal retrieval before Clear"
    );
    let before: Value = serde_json::from_slice(&body)?;
    assert!(
        before["credentials"]["password"].as_str() == Some(password.as_str()),
        "saved literal fixture must supply the launched password"
    );
    let clear = json!({"params": {"host": "127.0.0.1", "port": 5900}}).to_string();
    let (clear_status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/connection-config/records/{record_id}"),
        Some(&clear),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        clear_status,
        StatusCode::OK,
        "Clear original defining field through the record API"
    );
    let (status, _) = request(&app, "GET", &uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "clearing the original saved literal must revoke snapshot disclosure"
    );
    let replacement = Uuid::new_v4().simple().to_string()[..8].to_string();
    let mut changed = json!({"params": {"host": "127.0.0.1", "port": 5900, "credentials": {"password": replacement}}});
    let record_uri = format!("/api/v1/connection-config/records/{record_id}");
    let (status, _) = request(
        &app,
        "PUT",
        &record_uri,
        Some(&changed.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = request(&app, "GET", &uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "current saved value may evolve within the pinned account"
    );
    let current: Value = serde_json::from_slice(&body)?;
    assert!(current["credentials"]["password"].as_str() == Some(replacement.as_str()));
    changed["params"]["port"] = json!(5901);
    let (status, _) = request(
        &app,
        "PUT",
        &record_uri,
        Some(&changed.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&app, "GET", &uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "changed effective endpoint must not expose credentials to the original live socket"
    );
    changed["params"]["port"] = json!(5900);
    let (status, _) = request(
        &app,
        "PUT",
        &record_uri,
        Some(&changed.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&app, "GET", &uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    server.abort();
    let _ = server.await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn connection_session_credentials_preserve_ad_hoc_vnc_password()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Bind before building state: the real HttpProxyClient derives its loopback
    // destination from server.bind and must reach this router, not a mock.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let state = build_test_app_state_with(|settings| {
        settings.server.bind = address.to_string();
        settings.auth.local_enabled = true;
    })
    .await
    .map_err(|_| "fixture: application state initialization failed")?;
    let pool = state.db_pool.clone();
    let secret_store = state.services.secret_store.clone();
    let field_intents = state.services.secret_field_intents.clone();
    let connection_service = state.services.connection.clone();
    let settings = state.settings.clone();
    let redis = state.redis.clone();
    let target_id = seed_local_connection_target(&pool).await?;

    let app = piping_backend::app::build_router(state);
    let served_app = app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, served_app).await });
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("fixture: login failed")?;
    let record_payload = json!({
        "name": format!("vnc-ad-hoc-{}", unique_suffix()),
        "protocol": "vnc",
        "params": {"host": "127.0.0.1", "port": 5900}
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connection-config/records",
        Some(&record_payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "fixture: create credentialless VNC record"
    );
    let record: Value = serde_json::from_slice(&body)?;
    let assignment =
        json!({"record_id": record["id"], "scope_type": "device", "scope_id": target_id})
            .to_string();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/connection-config/assignments",
        Some(&assignment),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "fixture: assign VNC record");
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets/{target_id}/launch-options"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "fixture: resolve launch account");
    let options: Value = serde_json::from_slice(&body)?;
    let account = options["protocols"]
        .as_array()
        .ok_or("fixture: protocols missing")?
        .iter()
        .find(|item| item["protocol"] == "vnc")
        .and_then(|item| item["accounts"].as_array())
        .and_then(|items| items.first())
        .ok_or("fixture: VNC account missing")?;
    assert_eq!(account["requires_input"].as_bool(), Some(true));
    // Eight ASCII bytes avoid the native VNC password truncation boundary.
    let password = Uuid::new_v4().simple().to_string()[..8].to_string();
    let payload = json!({
        "target_id": target_id, "protocol": "vnc",
        "metadata": {"entryId": account["entry_id"]},
        "params": {"credentials": {"password": password}}
    })
    .to_string();
    let serialized: Value = serde_json::from_str(&payload)?;
    assert!(
        serialized["params"]["credentials"]["password"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "fixture: serialized request must carry a nonempty password"
    );
    let secret_counts_before: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM secrets_variable), (SELECT COUNT(*) FROM secrets_store), (SELECT COUNT(*) FROM secrets_binding), (SELECT COUNT(*) FROM secret_usage_binding)").fetch_one(&pool).await?;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "fixture: create actual embedded-proxy VNC session"
    );
    let created: Value = serde_json::from_slice(&body)?;
    let session_id = created["session"]["id"]
        .as_str()
        .ok_or("fixture: session id missing")?;
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/sessions/{session_id}/client-credentials"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "fixture: authorized owner credential retrieval"
    );
    let response: Value = serde_json::from_slice(&body)?;
    let credentials = response["credentials"]
        .as_object()
        .ok_or("fixture: credentials object missing")?;
    assert!(
        credentials.get("password").and_then(Value::as_str) == Some(password.as_str()),
        "ad-hoc password must remain available to authorized session owner"
    );
    let secret_counts_after: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM secrets_variable), (SELECT COUNT(*) FROM secrets_store), (SELECT COUNT(*) FROM secrets_binding), (SELECT COUNT(*) FROM secret_usage_binding)").fetch_one(&pool).await?;
    assert_eq!(
        secret_counts_before, secret_counts_after,
        "unsaved credentials must not create persistent secret rows"
    );
    // The snapshot never enters persistent session metadata, audit, or the
    // managed secret store. Compare in memory, without printing the value.
    let persisted: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM connections_session WHERE session_metadata::text LIKE '%' || $1 || '%')")
        .bind(&password).fetch_one(&pool).await?;
    assert!(
        !persisted,
        "ad-hoc credentials must not enter session metadata"
    );
    let audit_contains_password: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM audit_event WHERE metadata::text LIKE '%' || $1 || '%')",
    )
    .bind(&password)
    .fetch_one(&pool)
    .await?;
    assert!(
        !audit_contains_password,
        "audit must not persist ad-hoc credentials"
    );
    {
        let (status, body) = request(
            &app,
            "GET",
            "/api/v1/connections/sessions",
            None,
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !String::from_utf8_lossy(&body).contains(&password),
            "session metadata endpoints must not disclose ad-hoc credentials"
        );
    }
    assert!(
        !created.to_string().contains(&password),
        "create session details must not disclose credentials"
    );
    let credential_uri = format!("/api/v1/connections/sessions/{session_id}/client-credentials");
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(&credential_uri)
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("cache-control")
            .is_some_and(|value| value == "no-store")
    );
    let policy_uri = "/api/v1/admin/access-control/policies/00000000-0000-7000-8000-000000000003";
    let (status, body) = request(&app, "GET", policy_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let original_policy: Value = serde_json::from_slice(&body)?;
    for action in [
        piping_backend::contexts::access_control::domain::Action::Connect,
        piping_backend::contexts::access_control::domain::Action::Secret,
    ] {
        let action = serde_json::to_value(action)?;
        let mut policy = original_policy.clone();
        let actions = policy["actions"].as_array_mut().ok_or("policy actions")?;
        let before = actions.len();
        actions.retain(|value| value != &action);
        assert_eq!(
            before - actions.len(),
            1,
            "fixture must actually remove the serialized action"
        );
        let (status, _) = request(
            &app,
            "PUT",
            policy_uri,
            Some(&policy.to_string()),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "canonical permission mutation");
        let (status, _) = request(&app, "GET", &credential_uri, None, Some(&cookie)).await?;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "current access revocation must prevent disclosure"
        );
        let (status, _) = request(
            &app,
            "PUT",
            policy_uri,
            Some(&original_policy.to_string()),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
    }

    let mut no_global_secret = original_policy.clone();
    let secret_variable = serde_json::to_value(
        piping_backend::contexts::access_control::domain::ResourceType::SecretVariable,
    )?;
    no_global_secret["resource_types"]
        .as_array_mut()
        .ok_or("policy resources")?
        .retain(|value| value != &secret_variable);
    let (status, _) = request(
        &app,
        "PUT",
        policy_uri,
        Some(&no_global_secret.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&app, "GET", &credential_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "global secret visibility must still be required when target grants remain"
    );
    let (status, _) = request(
        &app,
        "PUT",
        policy_uri,
        Some(&original_policy.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let foreign_id = Uuid::now_v7();
    let foreign_name = format!("vnc-foreign-{}", unique_suffix());
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')")
        .bind(foreign_id).bind(&foreign_name).bind(format!("{foreign_name}@test.local")).execute(&pool).await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at) SELECT $1, password_hash, NOW() FROM auth_user_credential WHERE user_id = '00000000-0000-7000-8000-000000000001'")
        .bind(foreign_id).execute(&pool).await?;
    sqlx::query("INSERT INTO rbac_role_assignment (user_id, role_id) SELECT $1, role_id FROM rbac_role_assignment WHERE user_id = '00000000-0000-7000-8000-000000000001'")
        .bind(foreign_id).execute(&pool).await?;
    let foreign_cookie = login_and_get_cookie(&app, &foreign_name, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("foreign login")?;
    let (status, _) = request(&app, "GET", &credential_uri, None, Some(&foreign_cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another fully privileged user must not read the snapshot"
    );

    let mut second_payload = serialized.clone();
    let second_password = Uuid::new_v4().simple().to_string()[..8].to_string();
    second_payload["params"]["credentials"]["password"] = json!(second_password);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&second_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let second: Value = serde_json::from_slice(&body)?;
    let second_id = second["session"]["id"].as_str().ok_or("second id")?;
    let second_uri = format!("/api/v1/connections/sessions/{second_id}/client-credentials");
    let (status, body) = request(&app, "GET", &second_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let returned: Value = serde_json::from_slice(&body)?;
    assert!(
        returned["credentials"]["password"].as_str() == Some(second_password.as_str()),
        "session snapshots must be isolated"
    );

    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/connections/sessions/{session_id}/disconnect"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&app, "GET", &credential_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "disconnected sessions must not expose credentials"
    );
    let (status, _) = request(&app, "GET", &second_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "disconnect must not evict another session"
    );
    use piping_backend::contexts::secrets::application::{
        ApplySecretFieldIntentInput, SecretFieldIntent, SecretStore,
    };
    let borrowed_id = Uuid::parse_str(super::common::TEST_SECRET_ID)?;
    let record_id = Uuid::parse_str(record["id"].as_str().ok_or("record id")?)?;
    let saved_password = Uuid::new_v4().simple().to_string()[..8].to_string();
    secret_store
        .set_secret(borrowed_id, &saved_password)
        .await?;
    sqlx::query("INSERT INTO secrets_binding (id, secret_id, scope_type, scope_id, weight, managed_by_field_usage) VALUES ($1, $2, 'connection_config', $3, 0, false)")
        .bind(Uuid::now_v7()).bind(borrowed_id).bind(record_id).execute(&pool).await?;
    let input = ApplySecretFieldIntentInput {
        consumer_type: "connection_config:record".to_string(),
        consumer_id: record_id,
        owner_scope_type: "connection_config".to_string(),
        owner_scope_id: record_id,
        field_key: "credentials.password".to_string(),
        intent: SecretFieldIntent::Set,
        value: None,
        secret_id: Some(borrowed_id),
    };
    field_intents.apply_intent(input.clone()).await?;
    sqlx::query("UPDATE connconf_record SET params = jsonb_set(params, '{credentials}', jsonb_build_object('password', $2::text)) WHERE id = $1")
        .bind(record_id).bind(borrowed_id.to_string()).execute(&pool).await?;
    let mut saved_payload = serialized.clone();
    saved_payload
        .as_object_mut()
        .ok_or("payload")?
        .remove("params");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&saved_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "saved credential launch");
    let saved_session: Value = serde_json::from_slice(&body)?;
    let saved_id = saved_session["session"]["id"]
        .as_str()
        .ok_or("saved session id")?;
    let saved_uri = format!("/api/v1/connections/sessions/{saved_id}/client-credentials");
    let rotated = Uuid::new_v4().simple().to_string()[..8].to_string();
    secret_store.set_secret(borrowed_id, &rotated).await?;
    let (status, body) = request(&app, "GET", &saved_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let frozen: Value = serde_json::from_slice(&body)?;
    assert!(
        frozen["credentials"]["password"].as_str() == Some(rotated.as_str()),
        "shared resolution must reflect the current saved value"
    );
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&saved_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let next_session: Value = serde_json::from_slice(&body)?;
    let next_id = next_session["session"]["id"]
        .as_str()
        .ok_or("rotated session id")?;
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/sessions/{next_id}/client-credentials"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let next: Value = serde_json::from_slice(&body)?;
    assert!(
        next["credentials"]["password"].as_str() == Some(rotated.as_str()),
        "new launch must use rotated source"
    );
    field_intents
        .apply_intent(ApplySecretFieldIntentInput {
            intent: SecretFieldIntent::Clear,
            secret_id: None,
            ..input
        })
        .await?;
    let retained: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM secrets_binding WHERE secret_id = $1)")
            .bind(borrowed_id)
            .fetch_one(&pool)
            .await?;
    assert!(retained, "borrowed historical binding must survive Clear");
    let (status, _) = request(&app, "GET", &saved_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "retained historical binding must not authorize a detached field"
    );
    let (status, _) = request(&app, "GET", &second_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "ad-hoc snapshot must not depend on the unused saved password"
    );
    // Remove the now-stale record UUID only after proving that detaching its
    // usage alone denied GET despite the retained historical scope binding.
    sqlx::query("UPDATE connconf_record SET params = params - 'credentials' WHERE id = $1")
        .bind(record_id)
        .execute(&pool)
        .await?;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&saved_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "passwordless VNC launch");
    let empty_session: Value = serde_json::from_slice(&body)?;
    let empty_id = empty_session["session"]["id"]
        .as_str()
        .ok_or("empty session id")?;
    let empty_uri = format!("/api/v1/connections/sessions/{empty_id}/client-credentials");
    let (status, body) = request(&app, "GET", &empty_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "explicit empty VNC snapshot is not a cache miss"
    );
    let empty: Value = serde_json::from_slice(&body)?;
    assert!(
        empty["credentials"]
            .as_object()
            .is_some_and(|credentials| credentials.is_empty())
    );
    connection_service
        .mark_session_failed(
            Uuid::parse_str(empty_id)?,
            Uuid::parse_str("00000000-0000-7000-8000-000000000001")?,
        )
        .await?;
    let (status, _) = request(&app, "GET", &empty_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "failed session must not expose snapshot"
    );
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/connections/sessions/{second_id}/terminate"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&app, "GET", &second_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "terminated session must not expose snapshot"
    );
    let rdp_record = json!({"name": format!("rdp-ad-hoc-{}", unique_suffix()), "protocol": "rdp", "params": {"host": "127.0.0.1", "port": 3389}}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connection-config/records",
        Some(&rdp_record),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let record: Value = serde_json::from_slice(&body)?;
    let assignment =
        json!({"record_id": record["id"], "scope_type": "device", "scope_id": target_id})
            .to_string();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/connection-config/assignments",
        Some(&assignment),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets/{target_id}/launch-options"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let options: Value = serde_json::from_slice(&body)?;
    let account = options["protocols"]
        .as_array()
        .ok_or("RDP protocols")?
        .iter()
        .find(|item| item["protocol"] == "rdp")
        .and_then(|item| item["accounts"].as_array())
        .and_then(|items| items.first())
        .ok_or("RDP launch account")?;
    let rdp_payload = json!({"target_id": target_id, "protocol": "rdp", "metadata": {"entryId": account["entry_id"]}, "params": {"credentials": {"username": "operator", "domain": "test.local", "password": password}}}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&rdp_payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let rdp: Value = serde_json::from_slice(&body)?;
    let rdp_id = rdp["session"]["id"].as_str().ok_or("RDP session id")?;
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/sessions/{rdp_id}/client-credentials"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let rdp_credentials: Value = serde_json::from_slice(&body)?;
    assert!(
        rdp_credentials["credentials"]["password"].as_str() == Some(password.as_str()),
        "RDP must preserve the launch password"
    );
    assert!(
        rdp_credentials["credentials"]["username"] == "operator"
            && rdp_credentials["credentials"]["domain"] == "test.local"
    );
    assert!(
        rdp_credentials["credentials"]
            .as_object()
            .is_some_and(|map| map.len() == 3)
    );
    let template_id = Uuid::now_v7();
    let template_defaults =
        json!({"host": "127.0.0.1", "port": 5900, "credentials": {"password": password}});
    sqlx::query("INSERT INTO connections_template (id, name, protocol, defaults) VALUES ($1, $2, 'vnc', $3)").bind(template_id).bind(format!("replay-template-{}", unique_suffix())).bind(template_defaults).execute(&pool).await?;
    let template_payload =
        json!({"target_id": target_id, "protocol": "vnc", "template_id": template_id}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&template_payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "verified template launch");
    let template_session: Value = serde_json::from_slice(&body)?;
    let template_session_id = template_session["session"]["id"]
        .as_str()
        .ok_or("template session")?;
    let template_uri =
        format!("/api/v1/connections/sessions/{template_session_id}/client-credentials");
    let (status, _) = request(&app, "GET", &template_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("DELETE FROM connections_template WHERE id = $1")
        .bind(template_id)
        .execute(&pool)
        .await?;
    let (status, _) = request(&app, "GET", &template_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "deleted pinned template must not fall back to target defaults"
    );

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&saved_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let before_default: Value = serde_json::from_slice(&body)?;
    let before_default_id = before_default["session"]["id"]
        .as_str()
        .ok_or("pre-default session")?;
    let before_default_uri =
        format!("/api/v1/connections/sessions/{before_default_id}/client-credentials");
    let personal_payload = json!({"protocol": "vnc", "name": format!("replay-personal-{}", unique_suffix()), "params": {}, "is_default": true}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&personal_payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let personal: Value = serde_json::from_slice(&body)?;
    let personal_id = Uuid::parse_str(personal["id"].as_str().ok_or("personal id")?)?;
    field_intents
        .apply_intent(ApplySecretFieldIntentInput {
            consumer_type: "user_connection_credential_set".to_string(),
            consumer_id: personal_id,
            owner_scope_type: "user".to_string(),
            owner_scope_id: Uuid::parse_str("00000000-0000-7000-8000-000000000001")?,
            field_key: "vnc_password".to_string(),
            intent: SecretFieldIntent::Set,
            value: None,
            secret_id: Some(borrowed_id),
        })
        .await?;
    let (status, body) = request(&app, "GET", &before_default_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let unchanged: Value = serde_json::from_slice(&body)?;
    assert!(
        unchanged["credentials"]
            .as_object()
            .is_some_and(|credentials| credentials.is_empty()),
        "explicit absent personal selection must not adopt a newly configured default"
    );
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/connections/sessions",
        Some(&saved_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let personal_session: Value = serde_json::from_slice(&body)?;
    let personal_session_id = personal_session["session"]["id"]
        .as_str()
        .ok_or("personal session")?;
    let personal_uri =
        format!("/api/v1/connections/sessions/{personal_session_id}/client-credentials");
    let (status, body) = request(&app, "GET", &personal_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let personal_credentials: Value = serde_json::from_slice(&body)?;
    assert!(personal_credentials["credentials"]["password"].as_str() == Some(rotated.as_str()));
    // The canonical lifecycle forbids deleting the active default. Promote a
    // different set first, and prove the old session does not switch to it.
    let replacement_default = json!({"protocol": "vnc", "name": format!("replay-other-default-{}", unique_suffix()), "params": {}, "is_default": true}).to_string();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/user/settings/credentials",
        Some(&replacement_default),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = request(&app, "GET", &personal_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let still_pinned: Value = serde_json::from_slice(&body)?;
    assert!(
        still_pinned["credentials"]["password"].as_str() == Some(rotated.as_str()),
        "changing personal default must not change the pinned identity"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/user/settings/credentials/{personal_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(&app, "GET", &personal_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "deleted pinned personal set must not adopt another default"
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connection-config/assignments?record_id={record_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let assignments: Value = serde_json::from_slice(&body)?;
    for assignment in assignments["items"].as_array().ok_or("assignment list")? {
        let id = assignment["id"].as_str().ok_or("assignment id")?;
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/connection-config/assignments/{id}"),
            None,
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/connection-config/records/{record_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(&app, "GET", &before_default_uri, None, Some(&cookie)).await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "deleted pinned account must not switch to a different selection"
    );

    let restarted = piping_backend::app::AppState::build(settings, pool.clone(), redis)
        .await
        .map_err(|_| "restart fixture failed")?;
    let restarted_app = piping_backend::app::build_router(restarted);
    let (status, _) = request(
        &restarted_app,
        "GET",
        &format!("/api/v1/connections/sessions/{rdp_id}/client-credentials"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "missing process snapshot must not fall back to saved credentials"
    );
    server.abort();
    let _ = server.await;
    Ok(())
}
