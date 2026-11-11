//! Runtime-secret references are owned by job templates. Deleting one template
//! must retire only its binding and preserve a secret shared by another template.
//!
//! Requires isolated PostgreSQL and Redis. Run with:
//! `cargo test --test integration_playbook_secret_usage_sweep -- --ignored --test-threads=1`

mod common;
#[path = "common/project_workspace.rs"]
mod project_workspace;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie, request, unique_suffix,
};
use piping_backend::contexts::playbooks::domain::InventorySelection;
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateJobTemplateRecord, PlaybookRepository, SqlxPlaybookRepository,
};
use serde_json::Value;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn binding_count(
    db: &sqlx::PgPool,
    template_id: Uuid,
    secret_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*)::bigint FROM secret_usage_binding WHERE consumer_type = 'job_template:runtime_env' AND consumer_id = $1 AND owner_scope_type = 'playbook_job_template' AND owner_scope_id = $1 AND secret_id = $2")
        .bind(template_id).bind(secret_id).fetch_one(db).await
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis fixtures"]
async fn deleting_templates_retires_only_their_bindings_and_unblocks_shared_secret() -> TestResult {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("login failed")?;
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/automation-secrets",
        Some(
            &serde_json::json!({"key": format!("itest-usage-sweep-{}", unique_suffix())})
                .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create secret: {}",
        String::from_utf8_lossy(&body)
    );
    let secret_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("secret id")?
        .parse()?;

    let fixture = project_workspace::seed_workspace(
        &db,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let repo = SqlxPlaybookRepository::new(db.clone());
    let mut first = fixture.template.clone();
    first
        .options
        .secret_refs
        .insert("SHARED_TOKEN".into(), secret_id);
    repo.update_job_template(
        first.id,
        CreateJobTemplateRecord {
            template: first.clone(),
            user_id: None,
        },
    )
    .await?
    .ok_or("template missing")?;
    let mut second = first.clone();
    second.id = Uuid::now_v7();
    second.name = format!("shared-secret-{}", unique_suffix());
    let second = repo
        .create_job_template(CreateJobTemplateRecord {
            template: second,
            user_id: None,
        })
        .await?;
    assert_eq!(binding_count(&db, first.id, secret_id).await?, 1);
    assert_eq!(binding_count(&db, second.id, secret_id).await?, 1);
    let secret_path = format!("/api/v1/automation-secrets/{secret_id}");
    assert_eq!(
        request(&app, "DELETE", &secret_path, None, Some(&cookie))
            .await?
            .0,
        StatusCode::CONFLICT
    );

    let mut cleared = first.clone();
    cleared.options.secret_refs.clear();
    repo.update_job_template(
        first.id,
        CreateJobTemplateRecord {
            template: cleared,
            user_id: None,
        },
    )
    .await?
    .ok_or("template missing")?;
    assert_eq!(binding_count(&db, first.id, secret_id).await?, 0);
    assert_eq!(binding_count(&db, second.id, secret_id).await?, 1);
    assert_eq!(
        request(&app, "DELETE", &secret_path, None, Some(&cookie))
            .await?
            .0,
        StatusCode::CONFLICT,
        "updating one template must preserve the other template's guard"
    );
    repo.update_job_template(
        first.id,
        CreateJobTemplateRecord {
            template: first.clone(),
            user_id: None,
        },
    )
    .await?
    .ok_or("template missing")?;
    assert_eq!(binding_count(&db, first.id, secret_id).await?, 1);

    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/playbooks/job-templates/{}", first.id),
            None,
            Some(&cookie)
        )
        .await?
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(binding_count(&db, first.id, secret_id).await?, 0);
    assert_eq!(binding_count(&db, second.id, secret_id).await?, 1);
    assert_eq!(
        request(&app, "DELETE", &secret_path, None, Some(&cookie))
            .await?
            .0,
        StatusCode::CONFLICT,
        "another template still references the shared secret"
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/playbooks/job-templates/{}", second.id),
            None,
            Some(&cookie)
        )
        .await?
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(binding_count(&db, second.id, secret_id).await?, 0);
    assert_eq!(
        request(&app, "DELETE", &secret_path, None, Some(&cookie))
            .await?
            .0,
        StatusCode::NO_CONTENT
    );
    fixture.cleanup(&db).await?;
    Ok(())
}
