//! Synthetic API coverage. Requires isolated PostgreSQL and Redis.
mod common;

use axum::http::StatusCode;
use common::{TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, login_and_get_cookie, request};
use piping_backend::contexts::audit::{
    domain::{
        AuditAutomationState, AuditContext, AuditEvent, AuditLifecycleState, AuditOutcome,
        AuditSessionState, AuditTransportState,
    },
    infrastructure::persistence::AuditRepository,
};
use serde_json::{Value, json};
use strum::IntoEnumIterator;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn audit_repository_records_system_events_on_migrated_schema() -> TestResult {
    let state = common::build_test_app_state().await?;
    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: "pip593.schema.system".into(),
        actor_id: None,
        actor_kind: "system".into(),
        target_type: None,
        target_id: None,
        metadata: json!({}),
        created_at: chrono::Utc::now(),
    };
    // PostgreSQL resolves every relation in a CASE expression, even when the
    // selected event has no target. This exercises the complete migrated schema.
    state.repositories.audit.insert(event.clone()).await?;
    let stored = state
        .repositories
        .audit
        .get_event(event.id)
        .await?
        .ok_or("recorded system event")?;
    assert_eq!(stored.actor_id, None);
    assert_eq!(stored.target_id, None);
    assert_eq!(stored.audit_context().outcome, AuditOutcome::Unknown);
    let labels = state
        .repositories
        .audit
        .event_labels(std::slice::from_ref(&stored))
        .await?;
    assert_eq!(labels.get(&event.id), Some(&(None, None)));
    sqlx::query("DELETE FROM audit_event WHERE id=$1")
        .bind(event.id)
        .execute(&state.db_pool)
        .await?;
    Ok(())
}

async fn get(
    app: &axum::Router,
    cookie: &str,
    path: String,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(app, "GET", &path, None, Some(cookie)).await?;
    Ok((status, serde_json::from_slice(&body)?))
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn every_typed_lifecycle_state_is_projected_and_filterable() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let action = format!("pip205.states.{}", Uuid::now_v7());
    let states = AuditSessionState::iter()
        .map(AuditLifecycleState::Session)
        .chain(AuditTransportState::iter().map(AuditLifecycleState::Transport))
        .chain(AuditAutomationState::iter().map(AuditLifecycleState::Automation));
    for lifecycle in states {
        let context = AuditContext {
            outcome: AuditOutcome::Succeeded,
            state: Some(lifecycle),
            ..Default::default()
        };
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO audit_event (id,event_type,actor_id,actor_kind,target_type,target_id,metadata,created_at) VALUES ($1,$2,NULL,'system',NULL,NULL,$3,now())")
            .bind(id).bind(&action).bind(json!({"audit_context":context})).execute(&pool).await?;
        let state = serde_json::to_value(lifecycle)?;
        let domain = state["domain"].as_str().ok_or("state domain")?;
        let value = state["value"].as_str().ok_or("state value")?;
        let (status, page) = get(
            &app,
            &cookie,
            format!(
                "/api/v1/audit/events?event_type={action}&state_domain={domain}&state_value={value}"
            ),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], 1, "{domain}/{value}");
        assert_eq!(page["items"][0]["context"]["state"], state);
    }
    sqlx::query("DELETE FROM audit_event WHERE event_type=$1")
        .bind(action)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn audit_projection_filter_count_detail_and_recorded_targets() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let invalid_new = AuditEvent {
        id: Uuid::now_v7(),
        event_type: "pip205.invalid".into(),
        actor_id: None,
        actor_kind: "system".into(),
        target_type: None,
        target_id: None,
        metadata: json!({"audit_context":{"outcome":"invalid"}}),
        created_at: chrono::Utc::now(),
    };
    assert!(
        state
            .repositories
            .audit
            .insert(invalid_new.clone())
            .await
            .is_err()
    );
    let persisted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE id=$1")
        .bind(invalid_new.id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(persisted, 0);
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let action = format!("pip205.{}", Uuid::now_v7());
    let target = Uuid::now_v7();
    let username = format!("pip205-target-{target}");
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')")
        .bind(target).bind(&username).bind(format!("{username}@test.local")).execute(&pool).await?;
    let request_id = Uuid::now_v7();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let malformed = Uuid::now_v7();
    let historic = Uuid::now_v7();
    let partial = Uuid::now_v7();
    let context = json!({"outcome":"succeeded", "decision":null,
        "state":{"domain":"session", "value":"failed"}, "delegated_principal":null, "emitter":null,
        "correlation":{"request_id":request_id, "session_id":null, "run_id":null, "run_group_id":null, "causation_id":null},
        "display":{"actor":"recorded-actor-snapshot", "target":"event-time snapshot"}});
    let mut invalid = context.clone();
    invalid["state"]["credential"] =
        json!({"screen":"nested-context-canary", "keystrokes":"nested-key-canary"});
    invalid["display"]["actor"] = json!("malformed-actor-only");
    invalid["display"]["target"] = json!("malformed-target-only");
    for (id, metadata) in [
        (
            first,
            json!({"audit_context":context, "protocol":"ssh", "nested":{"credential":"credential-canary", "screen":"screen-canary", "keystroke":"key-canary"}, "summary":"hidden-canary"}),
        ),
        (second, json!({"audit_context":context})),
        (malformed, json!({"audit_context":invalid})),
        (historic, json!({"status":"success"})),
        (
            partial,
            json!({"audit_context":{"outcome":"succeeded", "display":{"actor":"partial-actor-only", "target":"partial-target-only"}}}),
        ),
    ] {
        sqlx::query("INSERT INTO audit_event (id,event_type,actor_id,actor_kind,target_type,target_id,metadata,created_at) VALUES ($1,$2,NULL,'system','user',$3,$4,'2026-01-01T00:00:00Z')")
            .bind(id).bind(&action).bind(target).bind(metadata).execute(&pool).await?;
    }
    sqlx::query("UPDATE identity_user SET username=$2 WHERE id=$1")
        .bind(target)
        .bind(format!("renamed-{target}"))
        .execute(&pool)
        .await?;
    let (status, page) = get(&app, &cookie, format!("/api/v1/audit/events?event_type={action}&outcome=succeeded&state_domain=session&state_value=failed&request_id={request_id}&limit=1&offset=0")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 2);
    for (query, field, expected) in [
        (
            "recorded-actor-snapshot",
            "actor",
            "recorded-actor-snapshot",
        ),
        ("event-time", "target", "event-time snapshot"),
    ] {
        let (status, matches) = get(
            &app,
            &cookie,
            format!("/api/v1/audit/events?event_type={action}&q={query}"),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(matches["total"], 2);
        let items = matches["items"].as_array().ok_or("items")?;
        assert_eq!(items.len(), 2);
        for item in items {
            assert_eq!(item["context"]["display"][field], expected);
            let id = item["id"].as_str().ok_or("event id")?;
            let (detail_status, detail) =
                get(&app, &cookie, format!("/api/v1/audit/events/{id}")).await?;
            assert_eq!(detail_status, StatusCode::OK);
            assert_eq!(detail["context"]["display"][field], expected);
        }
    }
    for query in [
        "malformed-actor-only",
        "malformed-target-only",
        "partial-actor-only",
        "partial-target-only",
        "nested-context-canary",
        "nested-key-canary",
        "credential-canary",
        "screen-canary",
        "key-canary",
    ] {
        let (status, matches) = get(
            &app,
            &cookie,
            format!("/api/v1/audit/events?event_type={action}&q={query}"),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(matches["total"], 0, "{query}");
        assert_eq!(matches["items"].as_array().ok_or("items")?.len(), 0);
    }
    let (_, historical) = get(
        &app,
        &cookie,
        format!(
            "/api/v1/audit/events?event_type={action}&outcome=unknown&q=recorded-actor-snapshot"
        ),
    )
    .await?;
    assert_eq!(historical["total"], 0);
    assert!(historical["items"].as_array().ok_or("items")?.is_empty());
    let (_, search_page) = get(
        &app,
        &cookie,
        format!(
            "/api/v1/audit/events?event_type={action}&q=recorded-actor-snapshot&limit=1&offset=1"
        ),
    )
    .await?;
    assert_eq!(search_page["total"], 2);
    assert_eq!(search_page["items"].as_array().ok_or("items")?.len(), 1);
    let (_, current_name) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/events?event_type={action}&q=renamed-{target}"),
    )
    .await?;
    assert_eq!(current_name["total"], 3);
    let current_items = current_name["items"].as_array().ok_or("items")?;
    assert_eq!(current_items.len(), 3);
    for id in [malformed, historic, partial] {
        assert!(
            current_items
                .iter()
                .any(|item| item["id"] == id.to_string())
        );
    }
    for item in current_items {
        assert_eq!(item["target"]["label"], format!("renamed-{target}"));
        assert!(item["context"]["display"]["target"].is_null());
    }
    let (_, next) = get(&app, &cookie, format!("/api/v1/audit/events?event_type={action}&outcome=succeeded&state_domain=session&state_value=failed&request_id={request_id}&limit=1&offset=1")).await?;
    assert_eq!(next["total"], 2);
    assert_ne!(page["items"][0]["id"], next["items"][0]["id"]);
    let (_, unknown) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/events?event_type={action}&outcome=unknown"),
    )
    .await?;
    assert_eq!(unknown["total"], 3);
    let (_, hidden) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/events?event_type={action}&q=hidden-canary"),
    )
    .await?;
    assert_eq!(hidden["total"], 0);
    let (_, nested_hidden) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/events?event_type={action}&q=nested-context-canary"),
    )
    .await?;
    assert_eq!(nested_hidden["total"], 0);
    let (_, by_event_id) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/events?event_type={action}&q={first}"),
    )
    .await?;
    assert_eq!(by_event_id["total"], 1);
    for obsolete in ["result=allowed", "mcp_tool=anything", "mcp_protocol=ssh"] {
        let (status, _) = get(&app, &cookie, format!("/api/v1/audit/events?{obsolete}")).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (_, sorted) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/events?event_type={action}&sort=outcome&order=asc"),
    )
    .await?;
    let outcomes: Vec<_> = sorted["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["context"]["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(
        outcomes,
        ["succeeded", "succeeded", "unknown", "unknown", "unknown"]
    );
    let (_, exact_date) = get(&app, &cookie, format!("/api/v1/audit/events?event_type={action}&created_from=2026-01-01T00:00:00Z&created_to=2026-01-01T00:00:00Z")).await?;
    assert_eq!(exact_date["total"], 5);
    let (_, outside_date) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/events?event_type={action}&created_from=2026-01-01T00:00:01Z"),
    )
    .await?;
    assert_eq!(outside_date["total"], 0);
    let (_, by_identity) = get(&app, &cookie, format!("/api/v1/audit/events?q={action}")).await?;
    assert_eq!(by_identity["total"], 5);
    for canary in [
        "credential-canary",
        "screen-canary",
        "key-canary",
        "hidden-canary",
        "nested-context-canary",
        "nested-key-canary",
    ] {
        assert!(!by_identity.to_string().contains(canary));
    }
    let (status, detail) = get(&app, &cookie, format!("/api/v1/audit/events/{first}")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["context"]["outcome"], "succeeded");
    assert_eq!(detail["context"]["state"]["value"], "failed");
    assert_eq!(detail["target"]["label"], "event-time snapshot");
    assert_eq!(detail["metadata"]["protocol"], "ssh");
    assert_eq!(detail["metadata"]["_omitted"], true);
    sqlx::query("DELETE FROM identity_user WHERE id=$1")
        .bind(target)
        .execute(&pool)
        .await?;
    let (_, deleted) = get(&app, &cookie, format!("/api/v1/audit/events/{first}")).await?;
    assert_eq!(deleted["target"]["label"], "event-time snapshot");
    let dump = detail.to_string();
    for canary in [
        "credential-canary",
        "screen-canary",
        "key-canary",
        "hidden-canary",
    ] {
        assert!(!dump.contains(canary));
    }
    let (_, old) = get(&app, &cookie, format!("/api/v1/audit/events/{historic}")).await?;
    assert_eq!(old["context"]["outcome"], "unknown");
    assert_eq!(old["target"]["label"], target.to_string());
    let (_, invalid_detail) =
        get(&app, &cookie, format!("/api/v1/audit/events/{malformed}")).await?;
    assert_eq!(invalid_detail["context"]["outcome"], "unknown");
    assert!(!invalid_detail.to_string().contains("nested-context-canary"));
    let (_, partial_detail) = get(&app, &cookie, format!("/api/v1/audit/events/{partial}")).await?;
    assert_eq!(partial_detail["context"]["outcome"], "unknown");
    let (_, targets) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/targets?target_type=user&search={target}"),
    )
    .await?;
    assert_eq!(targets["items"][0]["id"], target.to_string());
    assert_eq!(targets["items"][0]["label"], "event-time snapshot");
    sqlx::query("DELETE FROM audit_event WHERE event_type=$1")
        .bind(action)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn audit_names_survive_rename_and_deletion_and_historic_search_uses_current_names()
-> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let actor = Uuid::now_v7();
    let action = format!("pip593.{}", Uuid::now_v7());
    let original = format!("audit-original-{actor}");
    let renamed = format!("audit-renamed-{actor}");
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')")
        .bind(actor).bind(&original).bind(format!("{actor}@test.local")).execute(&pool).await?;
    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: action.clone(),
        actor_id: Some(actor),
        actor_kind: "user".into(),
        target_type: Some("user".into()),
        target_id: Some(actor),
        metadata: json!({"credential":"secret-canary"}),
        created_at: chrono::Utc::now(),
    }
    .with_context(AuditContext::api(AuditOutcome::Succeeded, None))?;
    state.repositories.audit.insert(event.clone()).await?;
    let historic = Uuid::now_v7();
    sqlx::query("INSERT INTO audit_event (id,event_type,actor_id,actor_kind,target_type,target_id,metadata) VALUES ($1,$2,$3,'user','user',$3,'{}')")
        .bind(historic).bind(&action).bind(actor).execute(&pool).await?;
    sqlx::query("UPDATE identity_user SET username=$2 WHERE id=$1")
        .bind(actor)
        .bind(&renamed)
        .execute(&pool)
        .await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    for (name, id) in [(&original, event.id), (&renamed, historic)] {
        let (status, list) = get(
            &app,
            &cookie,
            format!("/api/v1/audit/events?event_type={action}&q={name}"),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["total"], 1);
        assert_eq!(list["items"][0]["id"], id.to_string());
        assert_eq!(list["items"][0]["actor"]["label"], *name);
        assert_eq!(list["items"][0]["target"]["label"], *name);
        assert!(!list.to_string().contains("secret-canary"));
    }
    let (_, old) = get(&app, &cookie, format!("/api/v1/audit/events/{historic}")).await?;
    assert_eq!(old["context"]["outcome"], "unknown");
    assert!(old["context"]["display"]["actor"].is_null());
    for (order, expected) in [("asc", event.id), ("desc", historic)] {
        let (status, sorted) = get(
            &app,
            &cookie,
            format!("/api/v1/audit/events?event_type={action}&sort=actor&order={order}"),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sorted["total"], 2);
        assert_eq!(sorted["items"][0]["id"], expected.to_string());
    }
    let (_, targets) = get(
        &app,
        &cookie,
        format!("/api/v1/audit/targets?target_type=user&search={renamed}"),
    )
    .await?;
    assert_eq!(targets["items"][0]["id"], actor.to_string());
    assert_eq!(targets["items"][0]["label"], renamed);
    sqlx::query("DELETE FROM identity_user WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await?;
    let (_, deleted) = get(&app, &cookie, format!("/api/v1/audit/events/{}", event.id)).await?;
    assert_eq!(deleted["context"]["display"]["actor"], original);
    assert_eq!(deleted["target"]["label"], original);
    let (_, old_deleted) = get(&app, &cookie, format!("/api/v1/audit/events/{historic}")).await?;
    assert_eq!(old_deleted["target"]["label"], actor.to_string());
    sqlx::query("DELETE FROM audit_event WHERE event_type=$1")
        .bind(action)
        .execute(&pool)
        .await?;
    Ok(())
}

/// Write paths persist the projection, so the read path projects it again. A
/// stored visible projection must stay visible and a stored hidden one must
/// stay hidden; anything else is a false hidden-fields warning.
#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn stored_audit_projection_reprojects_without_false_hidden_fields() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let action = format!("pip330.{}", Uuid::now_v7());
    let context = json!({"outcome":"succeeded", "decision":null, "state":null,
        "delegated_principal":null, "emitter":null,
        "correlation":{"request_id":null, "session_id":null, "run_id":null, "run_group_id":null, "causation_id":null},
        "display":{"actor":null, "target":null}});
    let visible = Uuid::now_v7();
    let hidden = Uuid::now_v7();
    let tampered = Uuid::now_v7();
    for (id, metadata) in [
        (
            visible,
            json!({"audit_context":context, "protocol":"ssh", "_omitted":false}),
        ),
        (hidden, json!({"audit_context":context, "_omitted":true})),
        (
            tampered,
            json!({"audit_context":context, "protocol":"ssh", "_omitted":false, "credential":"reprojection-canary"}),
        ),
    ] {
        sqlx::query("INSERT INTO audit_event (id,event_type,actor_id,actor_kind,target_type,target_id,metadata,created_at) VALUES ($1,$2,NULL,'system',NULL,NULL,$3,'2026-01-01T00:00:00Z')")
            .bind(id).bind(&action).bind(metadata).execute(&pool).await?;
    }
    for (id, omitted) in [(visible, false), (hidden, true), (tampered, true)] {
        let (status, detail) = get(&app, &cookie, format!("/api/v1/audit/events/{id}")).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["metadata"]["_omitted"], omitted, "{id}");
    }
    let (_, visible_detail) = get(&app, &cookie, format!("/api/v1/audit/events/{visible}")).await?;
    assert_eq!(visible_detail["metadata"]["protocol"], "ssh");
    let (_, tampered_detail) =
        get(&app, &cookie, format!("/api/v1/audit/events/{tampered}")).await?;
    assert_eq!(tampered_detail["metadata"]["protocol"], "ssh");
    assert!(!tampered_detail.to_string().contains("reprojection-canary"));
    sqlx::query("DELETE FROM audit_event WHERE event_type=$1")
        .bind(action)
        .execute(&pool)
        .await?;
    Ok(())
}
