//! PostgreSQL fidelity for generation-pinned session reconciliation and paging.
mod common;

use chrono::{Duration, Utc};
use piping_backend::contexts::connections::infrastructure::persistence::repositories::{
    connection_repository::{ConnectionRepository, ConnectionSessionFilter, ConnectionSessionView},
    sqlx_connection_repository::SqlxConnectionRepository,
};
use uuid::Uuid;

fn test_reconciliation_event(session: Uuid) -> piping_backend::contexts::audit::domain::AuditEvent {
    piping_backend::contexts::audit::domain::AuditEvent {
        id: Uuid::now_v7(),
        event_type: "connection.session.disconnected".into(),
        actor_id: None,
        actor_kind: "system".into(),
        target_type: Some("connection_session".into()),
        target_id: Some(session),
        metadata: serde_json::json!({"reason":"runtime_ended"}),
        created_at: Utc::now(),
    }
    .with_context(piping_backend::contexts::audit::domain::AuditContext::default())
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the explicitly isolated PostgreSQL/Redis test stack"]
async fn runtime_end_audit_insert_failure_rolls_back_and_retry_records_once()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use piping_backend::contexts::audit::domain::{AuditEmitterKind, AuditEvent};
    use piping_backend::integrations::proxy::ProxyRuntimePresence;

    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let user = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_user (id,username,email,auth_source) VALUES ($1,$2,$3,'local')",
    )
    .bind(user)
    .bind(format!("atomic-{user}"))
    .bind(format!("{user}@example.invalid"))
    .execute(pool)
    .await?;
    let session = Uuid::now_v7();
    let proxy = Uuid::now_v7();
    let generation = Uuid::now_v7();
    let started = Utc::now() - Duration::minutes(5);
    let ended = started + Duration::minutes(1);
    sqlx::query("INSERT INTO connections_session (id,target_id,user_id,protocol,status,proxy_id,session_metadata,started_at,last_activity) VALUES ($1,$2,$3,'ssh','active',$4,$5,$6,$6)")
        .bind(session).bind(Uuid::now_v7()).bind(user).bind(proxy)
        .bind(serde_json::json!({"_runtime_generation":generation.to_string()})).bind(started)
        .execute(pool).await?;
    let repo = SqlxConnectionRepository::new(pool.clone());
    let event = test_reconciliation_event(session);
    // A duplicate event ID makes the real INSERT fail after the guarded UPDATE.
    sqlx::query("INSERT INTO audit_event (id,event_type,actor_kind,metadata,created_at) VALUES ($1,'fixture','system','{}'::jsonb,$2)")
        .bind(event.id).bind(Utc::now()).execute(pool).await?;
    assert!(
        repo.reconcile_runtime_end(session, proxy, generation, ended, &event)
            .await
            .is_err()
    );
    let unchanged = repo.get_session(session).await?.expect("session");
    assert!(unchanged.ended_at.is_none());
    assert_eq!(unchanged.status.as_str(), "active");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE target_id=$1 AND event_type='connection.session.disconnected'")
        .bind(session).fetch_one(pool).await?;
    assert_eq!(count, 0);

    let runtime = ProxyRuntimePresence {
        state: "ended".into(),
        generation: Some(generation.to_string()),
        ended_at: Some(ended.to_rfc3339()),
        ..Default::default()
    };
    assert!(
        state
            .services
            .connection
            .reconcile_runtime_end(
                &unchanged,
                &runtime,
                AuditEmitterKind::Api,
                Some("atomic-test")
            )
            .await?
    );
    assert!(
        !state
            .services
            .connection
            .reconcile_runtime_end(
                &unchanged,
                &runtime,
                AuditEmitterKind::Api,
                Some("atomic-test")
            )
            .await?
    );
    let saved = repo.get_session(session).await?.expect("ended session");
    assert_eq!(saved.status.as_str(), "disconnected");
    assert!(saved.ended_at.is_some());
    let rows: Vec<(Option<Uuid>, String, serde_json::Value)> = sqlx::query_as("SELECT actor_id,actor_kind,metadata FROM audit_event WHERE target_id=$1 AND event_type='connection.session.disconnected'")
        .bind(session).fetch_all(pool).await?;
    assert_eq!(rows.len(), 1);
    let (actor_id, actor_kind, metadata) = rows.into_iter().next().unwrap();
    assert_eq!(actor_id, None);
    assert_eq!(actor_kind, "system");
    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: "connection.session.disconnected".into(),
        actor_id,
        actor_kind,
        target_type: Some("connection_session".into()),
        target_id: Some(session),
        metadata,
        created_at: Utc::now(),
    };
    assert_eq!(
        event.audit_context().emitter.unwrap().kind,
        AuditEmitterKind::Api
    );
    Ok(())
}

/// Real database, auth and policy evaluation; only proxy metrics are a protocol fixture.
#[tokio::test]
#[ignore = "requires the explicitly isolated PostgreSQL/Redis test stack"]
async fn observation_capability_requires_both_concrete_observe_grants()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::{
        Json,
        extract::{
            FromRequestParts,
            ws::{Message, WebSocketUpgrade},
        },
        http::StatusCode,
        response::IntoResponse,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let state = common::build_test_app_state_with(|settings| {
        settings.server.bind = address.to_string();
        settings.auth.local_enabled = true;
    })
    .await?;
    let (target, _) = common::seed_device(&state.db_pool, "observe-policy").await?;
    let app = piping_backend::app::build_router(state.clone());
    let admin = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login")?;
    let name = format!("observer-{}", Uuid::now_v7().simple());
    let password = "Observation-test-password-24";
    let payload = json!({"username":name, "email":format!("{name}@example.invalid"), "status":"active", "auth_source":"local", "credential":{"mode":"manual", "password":password}}).to_string();
    let (status, body) = common::request(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&payload),
        Some(&admin),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let user: Value = serde_json::from_slice(&body)?;
    let user_id = Uuid::parse_str(user["id"].as_str().ok_or("user id")?)?;
    let cookie = common::login_and_get_cookie(&app, &name, password)
        .await
        .ok_or("observer login")?;
    let session = Uuid::now_v7();
    let proxy = Uuid::now_v7();
    let generation = Uuid::now_v7();
    let owner = Uuid::parse_str("00000000-0000-7000-8000-000000000001")?;
    sqlx::query("INSERT INTO connections_session (id, target_id, user_id, protocol, status, proxy_id, session_metadata) VALUES ($1,$2,$3,'ssh','active',$4,$5)")
        .bind(session).bind(target).bind(owner).bind(proxy).bind(json!({"_runtime_generation":generation.to_string()})).execute(&state.db_pool).await?;
    let metrics = json!({"connection_id":proxy.to_string(),"updated_at":Utc::now().to_rfc3339(),"overall_status":"connected","links":[],"runtime":{"state":"live","generation":generation.to_string(),"operator_attached":true,"observer_count":0,"observed_at":Utc::now().to_rfc3339(),"last_operator_activity_at":null,"reconnect_deadline":null,"ended_at":null}});
    let upstream_opens = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fixture_opens = upstream_opens.clone();
    let upstream_inputs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fixture_inputs = upstream_inputs.clone();
    let served = app.clone().layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let metrics = metrics.clone();
            let fixture_opens = fixture_opens.clone();
            let fixture_inputs = fixture_inputs.clone();
            async move {
                if request.method() == axum::http::Method::GET
                    && request.uri().path().starts_with("/proxy/v1/connections/")
                    && request.uri().path().ends_with("/metrics")
                {
                    return Json(metrics).into_response();
                }
                if request.uri().path().starts_with("/proxy/v1/ws/observe/") {
                    fixture_opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    // Protocol/control fixture only: real API authentication, policy,
                    // audit and relay run; this is not native proxy proof.
                    let (mut parts, _) = request.into_parts();
                    return match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
                        Ok(upgrade) => upgrade.protocols(["guacamole"])
                            .on_upgrade(move |mut socket| async move {
                                let mut tick =
                                    tokio::time::interval(std::time::Duration::from_millis(100));
                                loop {
                                    tokio::select! {
                                        _ = tick.tick() => {
                                            if socket.send(Message::Text("fixture-output".into())).await.is_err() { break; }
                                        }
                                        input = socket.recv() => match input {
                                            Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => { fixture_inputs.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
                                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                            _ => {}
                                        }
                                    }
                                }
                            })
                            .into_response(),
                        Err(error) => error.into_response(),
                    };
                }
                next.run(request).await
            }
        },
    ));
    let server = tokio::spawn(async move { axum::serve(listener, served).await });
    let subject = json!([format!("user:{user_id}")]);
    let read = Uuid::now_v7();
    let session_policy = Uuid::now_v7();
    let target_policy = Uuid::now_v7();
    for (id, kind, actions, resources) in [
        (
            read,
            "CONNECTION_SESSION",
            json!(["READ"]),
            json!(["connection_session:*"]),
        ),
        (
            session_policy,
            "CONNECTION_SESSION",
            json!(["READ", "CONNECT", "EXECUTE"]),
            json!([format!("connection_session:{session}")]),
        ),
        (
            target_policy,
            "CONNECTION_TARGET",
            json!(["READ", "CONNECT", "EXECUTE"]),
            json!([format!("connection_target:{target}")]),
        ),
    ] {
        sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,subjects,resources,enabled) VALUES ($1,$2,'allow',$3,$4,$5,$6,true)")
            .bind(id).bind(format!("observe-{id}")).bind(actions).bind(json!([kind])).bind(subject.clone()).bind(resources).execute(&state.db_pool).await?;
    }
    async fn allowed(
        app: &axum::Router,
        cookie: &str,
        session: Uuid,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let (status, body) = common::request(
            app,
            "GET",
            &format!("/api/v1/connections/sessions?search={session}"),
            None,
            Some(cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let response: Value = serde_json::from_slice(&body)?;
        let row = response["items"]
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|row| {
                    row["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()) == Some(session)
                })
            })
            .ok_or("session not returned")?;
        Ok(row["observe"]["allowed"].as_bool().ok_or("capability")?)
    }
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    assert!(
        !allowed(&app, &cookie, session).await?,
        "READ/CONNECT/EXECUTE are not OBSERVE"
    );
    sqlx::query("UPDATE rbac_policy SET actions = '[\"OBSERVE\"]'::jsonb WHERE id = $1")
        .bind(target_policy)
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    assert!(!allowed(&app, &cookie, session).await?, "target alone");
    sqlx::query("UPDATE rbac_policy SET actions = '[\"OBSERVE\"]'::jsonb WHERE id = $1")
        .bind(session_policy)
        .execute(&state.db_pool)
        .await?;
    sqlx::query("UPDATE rbac_policy SET enabled = false WHERE id = $1")
        .bind(target_policy)
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    assert!(!allowed(&app, &cookie, session).await?, "session alone");
    sqlx::query("UPDATE rbac_policy SET enabled = true WHERE id = $1")
        .bind(target_policy)
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    assert!(
        allowed(&app, &cookie, session).await?,
        "both concrete grants"
    );
    sqlx::query("UPDATE rbac_policy SET resources = $2 WHERE id = $1")
        .bind(session_policy)
        .bind(json!([format!("connection_session:{}", Uuid::now_v7())]))
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    assert!(!allowed(&app, &cookie, session).await?, "wrong session");
    sqlx::query("UPDATE rbac_policy SET resources = $2 WHERE id = $1")
        .bind(session_policy)
        .bind(json!([format!("connection_session:{session}")]))
        .execute(&state.db_pool)
        .await?;
    sqlx::query("UPDATE rbac_policy SET resources = $2 WHERE id = $1")
        .bind(target_policy)
        .bind(json!([format!("connection_target:{}", Uuid::now_v7())]))
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    assert!(!allowed(&app, &cookie, session).await?, "wrong target");
    sqlx::query("UPDATE rbac_policy SET resources = $2 WHERE id = $1")
        .bind(target_policy)
        .bind(json!([format!("connection_target:{target}")]))
        .execute(&state.db_pool)
        .await?;
    sqlx::query("UPDATE connections_session SET session_metadata = $2 WHERE id = $1")
        .bind(session)
        .bind(json!({"_runtime_generation":Uuid::now_v7().to_string()}))
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    assert!(
        !allowed(&app, &cookie, session).await?,
        "mismatched generation"
    );
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};
    let mut direct = format!("ws://{address}/api/v1/ws/observe/{session}")
        .as_str()
        .into_client_request()?;
    direct.headers_mut().insert("cookie", cookie.parse()?);
    direct
        .headers_mut()
        .insert("origin", format!("http://{address}").parse()?);
    assert!(
        matches!(tokio_tungstenite::connect_async(direct).await, Err(Error::Http(response)) if response.status().as_u16() == 403),
        "direct route must reject mismatched generation"
    );
    sqlx::query("UPDATE connections_session SET session_metadata = $2 WHERE id = $1")
        .bind(session)
        .bind(json!({"_runtime_generation":generation.to_string()}))
        .execute(&state.db_pool)
        .await?;
    let connect = |cookie: &str| -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
        let mut request = format!("ws://{address}/api/v1/ws/observe/{session}")
            .as_str()
            .into_client_request()?;
        request.headers_mut().insert("cookie", cookie.parse()?);
        request
            .headers_mut()
            .insert("origin", format!("http://{address}").parse()?);
        Ok(request)
    };
    let (mut policy_stream, _) = tokio_tungstenite::connect_async(connect(&cookie)?).await?;
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), policy_stream.next()).await?,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
    ));
    sqlx::query("UPDATE rbac_policy SET enabled = false WHERE id = $1")
        .bind(target_policy)
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        while let Some(Ok(frame)) = policy_stream.next().await {
            if frame.is_close() {
                break;
            }
        }
    })
    .await
    .expect("policy revocation must close the observer");
    sqlx::query("UPDATE rbac_policy SET enabled = true WHERE id = $1")
        .bind(target_policy)
        .execute(&state.db_pool)
        .await?;
    state
        .services
        .access_control_admin
        .invalidate_policy_cache()
        .await?;
    // Isolated DB only: the trigger fails exactly this session's start event.
    let suffix = Uuid::now_v7().simple().to_string();
    let function = format!("observe_audit_fail_{suffix}");
    let trigger = format!("observe_audit_fail_{suffix}");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_type = 'connection.session.observe.started' AND NEW.target_id = '{session}'::uuid THEN RAISE EXCEPTION 'synthetic audit failure'; END IF; RETURN NEW; END $$")))
        .execute(&state.db_pool).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER {trigger} BEFORE INSERT ON audit_event FOR EACH ROW EXECUTE FUNCTION {function}()")))
        .execute(&state.db_pool).await?;
    let before = upstream_opens.load(std::sync::atomic::Ordering::SeqCst);
    let denied = tokio_tungstenite::connect_async(connect(&cookie)?).await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER {trigger} ON audit_event"
    )))
    .execute(&state.db_pool)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP FUNCTION {function}()")))
        .execute(&state.db_pool)
        .await?;
    assert!(matches!(denied, Err(Error::Http(response)) if response.status().as_u16() == 403));
    assert_eq!(
        upstream_opens.load(std::sync::atomic::Ordering::SeqCst),
        before,
        "audit failure must prevent any proxy connection and output"
    );
    sqlx::query("UPDATE connections_session SET protocol = 'rdp' WHERE id = $1")
        .bind(session)
        .execute(&state.db_pool)
        .await?;
    let mut desktop_request = connect(&cookie)?;
    desktop_request
        .headers_mut()
        .insert("sec-websocket-protocol", "guacamole".parse()?);
    let (mut desktop, _) = tokio_tungstenite::connect_async(desktop_request).await?;
    desktop
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "0.,4.ping,3.123;".into(),
        ))
        .await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(Ok(message)) = desktop.next().await {
            if matches!(message, tokio_tungstenite::tungstenite::Message::Text(ref text) if text.to_string() == "0.,4.ping,3.123;") { return; }
        }
        panic!("desktop closed before local ping echo");
    }).await.expect("desktop ping echo");
    assert_eq!(
        upstream_inputs.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "ping must never reach the proxy"
    );
    desktop.close(None).await?;
    sqlx::query("UPDATE connections_session SET protocol = 'ssh' WHERE id = $1")
        .bind(session)
        .execute(&state.db_pool)
        .await?;
    let (mut credential_stream, _) = tokio_tungstenite::connect_async(connect(&cookie)?).await?;
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), credential_stream.next()).await?,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
    ));
    let auth_id = Uuid::parse_str(cookie.split_once('=').ok_or("cookie value")?.1)?;
    state.services.auth.logout(auth_id, Utc::now()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        while let Some(Ok(frame)) = credential_stream.next().await {
            if frame.is_close() {
                break;
            }
        }
    })
    .await
    .expect("credential revocation must close the observer");
    server.abort();
    Ok(())
}

#[tokio::test]
#[ignore = "requires the explicitly isolated PostgreSQL/Redis test stack"]
async fn websocket_rejects_hostile_cookie_origin_and_revoked_cookie()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("admin session");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let url = format!("ws://{address}/api/v1/ws/observe/{}", Uuid::now_v7());
    let mut request = url.as_str().into_client_request()?;
    request.headers_mut().insert("cookie", cookie.parse()?);
    request
        .headers_mut()
        .insert("origin", "https://attacker.example".parse()?);
    assert!(
        matches!(tokio_tungstenite::connect_async(request).await, Err(Error::Http(response)) if response.status().as_u16() == 403)
    );
    let session_id = Uuid::parse_str(cookie.split_once('=').expect("cookie value").1)?;
    state.services.auth.logout(session_id, Utc::now()).await?;
    let mut request = url.as_str().into_client_request()?;
    request.headers_mut().insert("cookie", cookie.parse()?);
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse()?);
    assert!(
        matches!(tokio_tungstenite::connect_async(request).await, Err(Error::Http(response)) if response.status().as_u16() == 403)
    );
    let login_app = piping_backend::app::build_router(state.clone());
    let expired = common::login_and_get_cookie(
        &login_app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("second admin session");
    let expired_id = Uuid::parse_str(expired.split_once('=').expect("cookie value").1)?;
    sqlx::query("UPDATE auth_session SET expires_at = $2 WHERE id = $1")
        .bind(expired_id)
        .bind(Utc::now() - Duration::seconds(1))
        .execute(&state.db_pool)
        .await?;
    let mut request = url.as_str().into_client_request()?;
    request.headers_mut().insert("cookie", expired.parse()?);
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse()?);
    assert!(
        matches!(tokio_tungstenite::connect_async(request).await, Err(Error::Http(response)) if response.status().as_u16() == 403)
    );
    server.abort();
    Ok(())
}

#[tokio::test]
#[ignore = "requires the explicitly isolated PostgreSQL/Redis test stack"]
async fn reconciliation_is_generation_pinned_and_paging_counts_history()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let user = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, auth_source) VALUES ($1, $2, $3, 'local')",
    )
    .bind(user)
    .bind(format!("observer-{user}"))
    .bind(format!("{user}@example.invalid"))
    .execute(pool)
    .await?;
    let repo = SqlxConnectionRepository::new(pool.clone());
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let proxy = Uuid::now_v7();
    let generation = Uuid::now_v7();
    let started = chrono::DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
        - Duration::minutes(5);
    let end = started + Duration::minutes(1);
    for (id, pin) in [(first, Some(generation)), (second, None)] {
        sqlx::query("INSERT INTO connections_session (id, target_id, user_id, protocol, status, proxy_id, session_metadata, started_at, last_activity) VALUES ($1, $2, $3, 'ssh', 'active', $4, $5, $6, $6)")
            .bind(id).bind(Uuid::now_v7()).bind(user).bind(proxy)
            .bind(match pin { Some(pin) => serde_json::json!({"_runtime_generation":pin.to_string()}), None => serde_json::json!({}) })
            .bind(started).execute(pool).await?;
    }
    let pinned = repo.get_session(first).await?.expect("pinned session");
    let mut presence = piping_backend::integrations::proxy::ProxyRuntimePresence {
        state: "live".to_string(),
        generation: Some(generation.to_string()),
        last_operator_activity_at: Some((started - Duration::hours(1)).to_rfc3339()),
        ..Default::default()
    };
    assert!(
        !state
            .services
            .connection
            .reconcile_runtime_end(
                &pinned,
                &presence,
                piping_backend::contexts::audit::domain::AuditEmitterKind::Worker,
                None
            )
            .await?,
        "idle live runtime must remain active"
    );
    presence.state = "detached".to_string();
    assert!(
        !state
            .services
            .connection
            .reconcile_runtime_end(
                &pinned,
                &presence,
                piping_backend::contexts::audit::domain::AuditEmitterKind::Worker,
                None
            )
            .await?,
        "detachment is not end evidence"
    );
    presence.state = "ended".to_string();
    presence.ended_at = Some(end.to_rfc3339());
    presence.generation = Some(Uuid::now_v7().to_string());
    assert!(
        !state
            .services
            .connection
            .reconcile_runtime_end(
                &pinned,
                &presence,
                piping_backend::contexts::audit::domain::AuditEmitterKind::Worker,
                None
            )
            .await?,
        "wrong generation"
    );
    assert!(repo.get_session(first).await?.unwrap().ended_at.is_none());
    let event = test_reconciliation_event(first);
    assert!(
        !repo
            .reconcile_runtime_end(first, proxy, Uuid::now_v7(), end, &event)
            .await?
    );
    assert!(
        !repo
            .reconcile_runtime_end(first, Uuid::now_v7(), generation, end, &event)
            .await?
    );
    assert!(
        !repo
            .reconcile_runtime_end(second, proxy, generation, end, &event)
            .await?
    );
    presence.generation = Some(generation.to_string());
    assert!(
        state
            .services
            .connection
            .reconcile_runtime_end(
                &pinned,
                &presence,
                piping_backend::contexts::audit::domain::AuditEmitterKind::Worker,
                None
            )
            .await?
    );
    assert!(
        !repo
            .reconcile_runtime_end(first, proxy, generation, end + Duration::seconds(5), &event)
            .await?
    );
    let saved = repo.get_session(first).await?.expect("reconciled session");
    assert_eq!(saved.ended_at, Some(end));
    assert_eq!(saved.last_activity, started);
    let (actor, actor_kind, metadata): (Option<Uuid>, String, serde_json::Value) = sqlx::query_as(
        "SELECT actor_id, actor_kind, metadata FROM audit_event WHERE target_id = $1 AND event_type = 'connection.session.disconnected' ORDER BY created_at DESC LIMIT 1"
    ).bind(first).fetch_one(pool).await?;
    assert_eq!(actor, None);
    assert_eq!(actor_kind, "system");
    assert_eq!(metadata["reason"], "runtime_ended");
    let event = piping_backend::contexts::audit::domain::AuditEvent {
        id: Uuid::now_v7(),
        event_type: "connection.session.disconnected".into(),
        actor_id: actor,
        actor_kind,
        target_type: Some("connection_session".into()),
        target_id: Some(first),
        metadata,
        created_at: Utc::now(),
    };
    let context = event.audit_context();
    assert_eq!(
        context.emitter.unwrap().kind,
        piping_backend::contexts::audit::domain::AuditEmitterKind::Worker
    );
    assert_eq!(
        context.outcome,
        piping_backend::contexts::audit::domain::AuditOutcome::Unknown
    );
    assert_eq!(context.correlation.session_id, Some(first));
    assert_eq!(
        context.state,
        Some(
            piping_backend::contexts::audit::domain::AuditLifecycleState::Session(
                piping_backend::contexts::audit::domain::AuditSessionState::Disconnected
            )
        )
    );
    let active = ConnectionSessionFilter {
        user_id: vec![user],
        view: Some(ConnectionSessionView::Active),
        ..Default::default()
    };
    assert_eq!(repo.count_sessions(active.clone()).await?, 1);
    assert_eq!(
        repo.list_sessions(active.clone(), Some(1), Some(0)).await?[0].id,
        second
    );
    assert!(
        repo.list_sessions(active, Some(1), Some(1))
            .await?
            .is_empty()
    );
    let history = ConnectionSessionFilter {
        user_id: vec![user],
        view: Some(ConnectionSessionView::History),
        ..Default::default()
    };
    assert_eq!(repo.count_sessions(history.clone()).await?, 1);
    assert_eq!(
        repo.list_sessions(history, Some(1), Some(0)).await?[0].id,
        first
    );
    let search = ConnectionSessionFilter {
        user_id: vec![user],
        view: Some(ConnectionSessionView::History),
        search: Some("observer-".into()),
        ..Default::default()
    };
    assert_eq!(repo.count_sessions(search.clone()).await?, 1);
    assert_eq!(
        repo.list_sessions(search, Some(1), Some(0)).await?[0].id,
        first
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires the explicitly isolated PostgreSQL/Redis test stack"]
async fn listing_reconciles_off_page_before_count_and_history()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::{Json, http::StatusCode, response::IntoResponse};
    use serde_json::{Value, json};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let state = common::build_test_app_state_with(|settings| {
        settings.server.bind = address.to_string();
        settings.auth.local_enabled = true;
    })
    .await?;
    let (target, _) = common::seed_device(&state.db_pool, "paging-runtime").await?;
    let (history_target, _) = common::seed_device(&state.db_pool, "paging-history").await?;
    let (unknown_target, _) = common::seed_device(&state.db_pool, "paging-unknown").await?;
    let owner = Uuid::parse_str("00000000-0000-7000-8000-000000000001")?;
    let generation = Uuid::now_v7();
    let live = Uuid::now_v7();
    let ended = Uuid::now_v7();
    let history_ended = Uuid::now_v7();
    let wrong = Uuid::now_v7();
    let missing = Uuid::now_v7();
    let mut sessions = Vec::new();
    for (target, proxy, seconds) in [
        (target, live, 5),
        (target, ended, 4),
        (history_target, history_ended, 3),
        (unknown_target, wrong, 2),
        (unknown_target, missing, 1),
    ] {
        let id = Uuid::now_v7();
        let started = Utc::now() - Duration::minutes(5) + Duration::seconds(seconds);
        sqlx::query("INSERT INTO connections_session (id,target_id,user_id,protocol,status,proxy_id,session_metadata,started_at,last_activity) VALUES ($1,$2,$3,'ssh','active',$4,$5,$6,$6)")
            .bind(id).bind(target).bind(owner).bind(proxy)
            .bind(json!({"_runtime_generation":generation.to_string()})).bind(started)
            .execute(&state.db_pool).await?;
        sessions.push(id);
    }
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login")?;
    let fixture = app.clone().layer(axum::middleware::from_fn(move |request: axum::extract::Request, next: axum::middleware::Next| async move {
        let path = request.uri().path();
        if request.method() == axum::http::Method::GET && path.starts_with("/proxy/v1/connections/") && path.ends_with("/metrics") {
            let id = path.trim_start_matches("/proxy/v1/connections/").trim_end_matches("/metrics");
            let proxy = Uuid::parse_str(id).ok();
            let runtime = if proxy == Some(missing) { Value::Null } else {
                let is_ended = proxy == Some(ended) || proxy == Some(history_ended) || proxy == Some(wrong);
                json!({"state": if is_ended { "ended" } else { "live" },
                    "generation": if proxy == Some(wrong) { Uuid::nil().to_string() } else { generation.to_string() },
                    "ended_at": if is_ended { Some((Utc::now() - Duration::minutes(1)).to_rfc3339()) } else { None }})
            };
            return Json(json!({"connection_id":id,"updated_at":Utc::now().to_rfc3339(),"overall_status":"connected","links":[],"runtime":runtime})).into_response();
        }
        next.run(request).await
    }));
    let server = tokio::spawn(async move { axum::serve(listener, fixture).await });
    for (target, view, expected_total, expected_id) in [
        (target, "active", 1, sessions[0]),
        (target, "history", 1, sessions[1]),
        (history_target, "history", 1, sessions[2]),
        (unknown_target, "active", 2, sessions[3]),
    ] {
        let mut path =
            format!("/api/v1/connections/sessions?target_id={target}&view={view}&limit=1&offset=0");
        if target == history_target {
            path.push_str("&status=disconnected&search=disconnected");
        }
        let (status, body) = common::request(&app, "GET", &path, None, Some(&cookie)).await?;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&body)?;
        assert_eq!(body["total"], expected_total);
        assert_eq!(body["items"][0]["id"], expected_id.to_string());
    }
    let path =
        format!("/api/v1/connections/sessions?target_id={target}&view=active&limit=1&offset=1");
    let (status, body) = common::request(&app, "GET", &path, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&body)?;
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
    for id in [sessions[3], sessions[4]] {
        let ended_at: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT ended_at FROM connections_session WHERE id=$1")
                .bind(id)
                .fetch_one(&state.db_pool)
                .await?;
        assert!(ended_at.is_none());
    }
    let (count, actor, kind, metadata): (i64, Option<Uuid>, String, Value) = sqlx::query_as(
        "SELECT count(*) OVER (), actor_id, actor_kind, metadata FROM audit_event WHERE target_id=$1 AND event_type='connection.session.disconnected'"
    ).bind(sessions[1]).fetch_one(&state.db_pool).await?;
    assert_eq!(count, 1);
    assert_eq!(actor, None);
    assert_eq!(kind, "system");
    let event = piping_backend::contexts::audit::domain::AuditEvent {
        id: Uuid::now_v7(),
        event_type: "connection.session.disconnected".into(),
        actor_id: actor,
        actor_kind: kind,
        target_type: Some("connection_session".into()),
        target_id: Some(sessions[1]),
        metadata,
        created_at: Utc::now(),
    };
    assert_eq!(
        event.audit_context().emitter.unwrap().kind,
        piping_backend::contexts::audit::domain::AuditEmitterKind::Api
    );
    server.abort();
    Ok(())
}
