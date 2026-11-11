//! Handler-level authorization and audit persistence for automation requests.
mod common;

use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use piping_backend::api::{
    middleware::{auth::AuthContext, request_id::RequestId},
    rest::{
        envelope_extractors::{EnvelopeJson, Path},
        routes::playbooks::{cancel_run_group, launch_job_template},
    },
};
use piping_backend::contexts::access_control::domain::{Action, ResourceType};
use piping_backend::contexts::audit::domain::{
    AuditContext, AuditDecision, AuditEmitterKind, AuditOutcome,
};
use piping_backend::contexts::shared::request_context::ApiTokenScopeContext;
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis; common app setup runs migrations and resets runtime tables"]
async fn denied_launch_is_audited_before_response_for_user_and_pat()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await?;
    let db = common::build_test_db_pool().await?;
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')")
        .bind(user_id).bind(format!("launch-audit-{user_id}")).bind(format!("{user_id}@test.local"))
        .execute(&db).await?;
    for method in ["session", "api_token"] {
        let template_id = Uuid::now_v7();
        let request_id = Uuid::now_v7();
        let mut auth = AuthContext::anonymous();
        auth.user_id = Some(user_id);
        auth.auth_method = method.into();
        if method == "api_token" {
            auth.api_token_id = Some(Uuid::now_v7().to_string());
        }
        let response = launch_job_template(
            State(state.clone()),
            Extension(RequestId(request_id.to_string())),
            Some(Extension(auth)),
            HeaderMap::new(),
            Path(template_id),
            EnvelopeJson(serde_json::from_value(json!({}))?),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let rows: Vec<(Option<Uuid>, String, Option<String>, Value)> = sqlx::query_as(
            "SELECT actor_id, actor_kind, target_type, metadata FROM audit_event WHERE event_type = 'playbook.job_template.launch_denied' AND target_id = $1",
        ).bind(template_id).fetch_all(&db).await?;
        assert_eq!(rows.len(), 1);
        let (actor, kind, target_type, metadata) = &rows[0];
        assert_eq!(*actor, Some(user_id));
        assert_eq!(
            kind,
            if method == "api_token" {
                "api_token"
            } else {
                "user"
            }
        );
        assert_eq!(target_type.as_deref(), Some("playbook_job_template"));
        let context: AuditContext = serde_json::from_value(metadata["audit_context"].clone())?;
        assert_eq!(context.outcome, AuditOutcome::Denied);
        assert_eq!(context.decision, Some(AuditDecision::Denied));
        assert_eq!(context.correlation.request_id, Some(request_id));
        assert_eq!(context.correlation.run_group_id, None);
        assert_eq!(context.emitter.unwrap().kind, AuditEmitterKind::Api);
        assert!(context.state.is_none());
        assert!(context.delegated_principal.is_none());
        assert!(metadata.get("error_message").is_none());
        sqlx::query("DELETE FROM audit_event WHERE target_id = $1")
            .bind(template_id)
            .execute(&db)
            .await?;
    }
    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(user_id)
        .execute(&db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis; common app setup runs migrations and resets runtime tables"]
async fn pat_group_cancellation_audit_keeps_user_identity_and_token_channel()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await?;
    let db = common::build_test_db_pool().await?;
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
        .bind(common::TEST_ADMIN_USERNAME)
        .fetch_one(&db)
        .await?;
    let project_id = Uuid::now_v7();
    let environment_id = Uuid::now_v7();
    let template_id = Uuid::now_v7();
    let group_id = Uuid::now_v7();
    let request_id = Uuid::now_v7();
    let api_token_id = Uuid::now_v7();
    let options = json!({"argv": [], "extra_vars": {}, "limit": null, "env": {}, "secret_refs": {}, "timeout_seconds": 60});
    let inventory = json!({"kind": "native", "sources": [], "ssh_record_id": null});
    let launch = json!({
        "template_id": template_id, "project_id": project_id,
        "revision_id": Uuid::now_v7(), "tree_sha256": "0".repeat(64),
        "execution_environment_id": environment_id, "image": "registry.example/ansible",
        "image_digest": format!("sha256:{}", "0".repeat(64)), "workdir": "/work",
        "playbook_paths": ["site.yml"], "inventory": inventory, "options": options, "targets": []
    });
    sqlx::query("INSERT INTO playbook_project (id, name, source) VALUES ($1, $2, '{\"kind\":\"editable\"}'::jsonb)")
        .bind(project_id).bind(format!("cancel-audit-{project_id}")).execute(&db).await?;
    sqlx::query("INSERT INTO playbook_execution_environment (id, name, image) VALUES ($1, $2, 'registry.example/ansible')")
        .bind(environment_id).bind(format!("cancel-audit-{environment_id}")).execute(&db).await?;
    sqlx::query("INSERT INTO playbook_job_template (id, name, project_id, workdir, playbook_paths, revision_selection, inventory, options, execution_environment_id) VALUES ($1, $2, $3, '/work', ARRAY['site.yml'], '{\"kind\":\"current\"}'::jsonb, $4, $5, $6)")
        .bind(template_id).bind(format!("cancel-audit-{template_id}")).bind(project_id)
        .bind(&inventory).bind(&options).bind(environment_id).execute(&db).await?;
    sqlx::query("INSERT INTO playbook_run_group (id, template_id, launch_spec, trigger_kind, status, total_runs) VALUES ($1, $2, $3, 'manual', 'queued', 0)")
        .bind(group_id).bind(template_id).bind(launch).execute(&db).await?;

    // This exercises the handler with an authenticated PAT context. Authentication
    // middleware is outside this test; policy evaluation and persistence are real.
    let mut auth = AuthContext::anonymous();
    auth.user_id = Some(user_id);
    auth.auth_method = "api_token".into();
    auth.api_token_id = Some(api_token_id.to_string());
    auth.api_token_scopes = vec![ApiTokenScopeContext {
        action: Action::Update.as_str().into(),
        resource_type: ResourceType::PlaybookRunGroup.as_str().into(),
        resource_pattern: "*".into(),
    }];
    let response = cancel_run_group(
        State(state),
        Extension(RequestId(request_id.to_string())),
        Some(Extension(auth)),
        HeaderMap::new(),
        Path(group_id),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let events: Vec<(Option<Uuid>, String, Value)> = sqlx::query_as(
        "SELECT actor_id, actor_kind, metadata FROM audit_event WHERE event_type = 'playbook.run_group.cancel_requested' AND target_id = $1",
    ).bind(group_id).fetch_all(&db).await?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, Some(user_id));
    assert_ne!(events[0].0, Some(api_token_id));
    assert_eq!(events[0].1, "api_token");
    let context: AuditContext = serde_json::from_value(events[0].2["audit_context"].clone())?;
    assert_eq!(context.correlation.request_id, Some(request_id));
    assert_eq!(context.correlation.run_group_id, Some(group_id));
    assert_eq!(context.outcome, AuditOutcome::Succeeded);
    assert!(context.delegated_principal.is_none());
    assert!(!events[0].2.to_string().contains(&api_token_id.to_string()));

    sqlx::query("DELETE FROM audit_event WHERE target_id = $1")
        .bind(group_id)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(group_id)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM playbook_job_template WHERE id = $1")
        .bind(template_id)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM playbook_project WHERE id = $1")
        .bind(project_id)
        .execute(&db)
        .await?;
    sqlx::query("DELETE FROM playbook_execution_environment WHERE id = $1")
        .bind(environment_id)
        .execute(&db)
        .await?;
    Ok(())
}
