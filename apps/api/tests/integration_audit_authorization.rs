//! Collection authorization across audit list, search, lookup and detail.
//! Requires isolated PostgreSQL and Redis; run with
//! `cargo test -p piping-backend --test integration_audit_authorization -- --ignored`.
mod common;

use axum::{Router, http::StatusCode};
use common::{TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, login_and_get_cookie, request_with_auth};
use piping_backend::contexts::access_control::application::CreateUserApiTokenInput;
use piping_backend::contexts::access_control::domain::{Action, ResourceType, UserApiTokenScope};
use serde_json::{Value, json};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn check_routes(
    app: &Router,
    cookie: Option<&str>,
    token: Option<&str>,
    id: Uuid,
    expected: StatusCode,
    detail_expected: StatusCode,
) -> TestResult {
    for path in [
        "/api/v1/audit/events?q=pip205synthetic&limit=1&offset=0".to_string(),
        "/api/v1/audit/targets?search=pip205synthetic&limit=1".to_string(),
        format!("/api/v1/audit/events/{id}"),
    ] {
        let (status, body) = request_with_auth(app, "GET", &path, None, cookie, token).await?;
        let expected = if path.contains("/events/") {
            detail_expected
        } else {
            expected
        };
        assert_eq!(
            status,
            expected,
            "{path}: {}",
            String::from_utf8_lossy(&body)
        );
        if expected == StatusCode::FORBIDDEN {
            let value: Value = serde_json::from_slice(&body)?;
            assert!(value.get("items").is_none() && value.get("total").is_none());
        }
    }
    Ok(())
}

async fn invalidate() -> TestResult {
    let mut redis = common::build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn audit_collection_requires_global_authority_and_obeys_denies_and_token_ceiling()
-> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state.clone());
    let admin = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let user = Uuid::now_v7();
    let username = format!("pip205-audit-{user}");
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')")
        .bind(user).bind(&username).bind(format!("{username}@test.local")).execute(&pool).await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id,password_hash,password_changed_at) SELECT $1,c.password_hash,now() FROM auth_user_credential c JOIN identity_user u ON u.id=c.user_id WHERE u.username=$2")
        .bind(user).bind(TEST_ADMIN_USERNAME).execute(&pool).await?;
    let cookie = login_and_get_cookie(&app, &username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("restricted user login")?;
    let event = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    let target = Uuid::now_v7();
    sqlx::query("INSERT INTO audit_event (id,event_type,actor_id,target_type,target_id,metadata,created_at) VALUES ($1,'pip205.synthetic',NULL,'organization',$2,$3,now())")
        .bind(event).bind(target).bind(json!({"summary":"pip205synthetic"})).execute(&pool).await?;
    sqlx::query("INSERT INTO audit_event (id,event_type,actor_id,target_type,target_id,metadata,created_at) VALUES ($1,'pip205.synthetic',NULL,'organization',$2,$3,now())")
        .bind(foreign).bind(target).bind(json!({"summary":"pip205synthetic"})).execute(&pool).await?;
    let policy = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) VALUES ($1,$2,'allow',$3,$4,NULL,$5,$6,true)")
        .bind(policy).bind(format!("pip205-audit-{policy}"))
        .bind(json!(["VIEW_AUDIT"])).bind(json!(["AUDIT_EVENT"]))
        .bind(json!([format!("user:{user}")])).bind(json!([format!("audit_event:{event}")]))
        .execute(&pool).await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::FORBIDDEN,
        StatusCode::OK,
    )
    .await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        foreign,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;
    check_routes(
        &app,
        Some(&admin),
        None,
        event,
        StatusCode::OK,
        StatusCode::OK,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$2 WHERE id=$1")
        .bind(policy)
        .bind(json!([event.to_string()]))
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::FORBIDDEN,
        StatusCode::OK,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$2 WHERE id=$1")
        .bind(policy)
        .bind(json!([format!("audit_event:{target}")]))
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;

    sqlx::query("UPDATE rbac_policy SET resources='[\"audit_event:*\"]'::jsonb WHERE id=$1")
        .bind(policy)
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::OK,
        StatusCode::OK,
    )
    .await?;
    let now = chrono::Utc::now();
    let matching = json!({"time_window": {
        "start": (now - chrono::Duration::days(1)).to_rfc3339(),
        "end": (now + chrono::Duration::days(1)).to_rfc3339()
    }});
    let nonmatching = json!({"time_window": {
        "start": (now - chrono::Duration::days(3)).to_rfc3339(),
        "end": (now - chrono::Duration::days(2)).to_rfc3339()
    }});
    sqlx::query("UPDATE rbac_policy SET constraints=$2 WHERE id=$1")
        .bind(policy)
        .bind(&matching)
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::OK,
        StatusCode::OK,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET constraints=$2 WHERE id=$1")
        .bind(policy)
        .bind(&nonmatching)
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET constraints=NULL WHERE id=$1")
        .bind(policy)
        .execute(&pool)
        .await?;
    invalidate().await?;

    let issued = state
        .services
        .user_api_tokens
        .create_for_user(
            user,
            CreateUserApiTokenInput {
                name: "pip205 restricted audit token".into(),
                scopes: vec![UserApiTokenScope {
                    action: Action::Read,
                    resource_type: ResourceType::Inventory,
                    resource_pattern: "*".into(),
                }],
                expires_in_seconds: Some(3600),
            },
            chrono::Utc::now(),
        )
        .await?;
    check_routes(
        &app,
        None,
        Some(&issued.raw_token),
        event,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;

    sqlx::query("UPDATE rbac_policy SET resources=$2 WHERE id=$1")
        .bind(policy)
        .bind(json!([format!("audit_event:{event}")]))
        .execute(&pool)
        .await?;
    invalidate().await?;
    let scoped = state
        .services
        .user_api_tokens
        .create_for_user(
            user,
            CreateUserApiTokenInput {
                name: "pip205 scoped audit token".into(),
                scopes: vec![UserApiTokenScope {
                    action: Action::ViewAudit,
                    resource_type: ResourceType::AuditEvent,
                    resource_pattern: format!("audit_event:{event}"),
                }],
                expires_in_seconds: Some(3600),
            },
            chrono::Utc::now(),
        )
        .await?;
    check_routes(
        &app,
        None,
        Some(&scoped.raw_token),
        event,
        StatusCode::FORBIDDEN,
        StatusCode::OK,
    )
    .await?;
    check_routes(
        &app,
        None,
        Some(&scoped.raw_token),
        foreign,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET resources='[\"audit_event:*\"]'::jsonb WHERE id=$1")
        .bind(policy)
        .execute(&pool)
        .await?;
    invalidate().await?;
    let global = state
        .services
        .user_api_tokens
        .create_for_user(
            user,
            CreateUserApiTokenInput {
                name: "pip205 global audit token".into(),
                scopes: vec![UserApiTokenScope {
                    action: Action::ViewAudit,
                    resource_type: ResourceType::AuditEvent,
                    resource_pattern: "audit_event:*".into(),
                }],
                expires_in_seconds: Some(3600),
            },
            now,
        )
        .await?;
    check_routes(
        &app,
        None,
        Some(&global.raw_token),
        event,
        StatusCode::OK,
        StatusCode::OK,
    )
    .await?;

    for (name, action, resource_type) in [
        (
            "insufficient action",
            Action::Read,
            ResourceType::AuditEvent,
        ),
        ("wrong resource", Action::ViewAudit, ResourceType::Assistant),
    ] {
        let restricted = state
            .services
            .user_api_tokens
            .create_for_user(
                user,
                CreateUserApiTokenInput {
                    name: format!("pip205 {name}"),
                    scopes: vec![UserApiTokenScope {
                        action,
                        resource_type,
                        resource_pattern: "*".into(),
                    }],
                    expires_in_seconds: Some(3600),
                },
                chrono::Utc::now(),
            )
            .await?;
        check_routes(
            &app,
            None,
            Some(&restricted.raw_token),
            event,
            StatusCode::FORBIDDEN,
            StatusCode::FORBIDDEN,
        )
        .await?;
    }

    let deny = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) VALUES ($1,$2,'deny',$3,$4,NULL,$5,'[]'::jsonb,true)")
        .bind(deny).bind(format!("pip205-audit-deny-{deny}"))
        .bind(json!(["VIEW_AUDIT"])).bind(json!(["AUDIT_EVENT"]))
        .bind(json!([format!("user:{user}")])).execute(&pool).await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$2 WHERE id=$1")
        .bind(deny)
        .bind(json!([format!("audit_event:{event}")]))
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET constraints=$2 WHERE id=$1")
        .bind(deny)
        .bind(&nonmatching)
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::OK,
        StatusCode::OK,
    )
    .await?;
    sqlx::query("UPDATE rbac_policy SET constraints=$2 WHERE id=$1")
        .bind(deny)
        .bind(&matching)
        .execute(&pool)
        .await?;
    invalidate().await?;
    check_routes(
        &app,
        Some(&cookie),
        None,
        event,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
    )
    .await?;
    sqlx::query("DELETE FROM rbac_policy WHERE id IN ($1,$2)")
        .bind(policy)
        .bind(deny)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM audit_event WHERE id IN ($1,$2)")
        .bind(event)
        .bind(foreign)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM identity_user WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await?;
    invalidate().await?;
    Ok(())
}
