//! Integration coverage for playbook-project SCM named-secret references
//! (PWA-53).
//!
//! A `playbook_project` SCM credential is a named-secret reference; it must be
//! stored only as a canonical `secret_usage_binding` row so the named-secret
//! delete guard can see it. This suite proves the create/update/delete
//! lifecycle against PostgreSQL, including the `409` that blocks deleting a
//! secret a project still binds.
//!
//! Requires PostgreSQL and Redis (e.g. `make -C apps/api run-up`). Run with:
//! `cargo test --test integration_playbook_project_scm_credential_binding -- --ignored --test-threads=1`
//! Every test here creates access-control state, so it additionally requires
//! `PIPING_TEST_DISPOSABLE_DB=1` and a disposable `pwa*` database name (the CI
//! coverage lane uses `pwa10_coverage`, local verification uses
//! `pwa11_coverage`; the guard accepts any `pwa` prefix).

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, TEST_SECRET_ID, build_test_app_state,
    build_test_db_pool, build_test_redis, login_and_get_cookie, request, unique_suffix,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::repositories::playbook_repository::{
    PlaybookRepository, SqlxPlaybookRepository,
};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

const PROJECT_CREDENTIAL_CONSUMER_TYPE: &str = "playbook_project";
const PROJECT_CREDENTIAL_FIELD_KEY: &str = "scm_credential";

/// Project regressions that create access-control state must run only against a
/// declared disposable database. Mirrors the alert-source lifecycle suite.
fn require_disposable_pwa_database() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url = std::env::var("PIPING_DATABASE__URL")?;
    let database_name = database_url
        .rsplit('/')
        .next()
        .and_then(|segment| segment.split('?').next())
        .unwrap_or_default();
    if std::env::var("PIPING_TEST_DISPOSABLE_DB").as_deref() == Ok("1")
        && database_name.starts_with("pwa")
    {
        return Ok(());
    }

    Err(
        "PWA-53 project-credential regression requires PIPING_TEST_DISPOSABLE_DB=1 and a disposable pwa* database"
            .into(),
    )
}

/// The shared `test-admin-policy` does not grant playbook-resource authority;
/// grant the test-admin role playbook project CRUD for this suite (idempotent).
async fn grant_playbook_project_admin(
    db: &sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'test-admin-role'")
            .fetch_one(db)
            .await
            .map_err(|e| format!("test-admin role: {e}"))?;
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, effect, actions, resource_types, constraints, subjects, resources, enabled
        )
        VALUES ($1, 'test-admin-playbook-projects-policy', 'allow', $2, $3, NULL, $4, '[]'::jsonb, true)
        ON CONFLICT (name)
        DO UPDATE SET actions = EXCLUDED.actions,
                      resource_types = EXCLUDED.resource_types,
                      subjects = EXCLUDED.subjects,
                      enabled = true
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(serde_json::json!(["CREATE", "READ", "UPDATE", "DELETE"]))
    .bind(serde_json::json!(["PLAYBOOK_PROJECT"]))
    .bind(serde_json::json!([format!("role:{role_id}")]))
    .execute(db)
    .await
    .map_err(|e| format!("seed playbook project admin policy: {e}"))?;

    // The policy set is cached in Redis behind a generation key; a direct SQL
    // seed bypasses the admin API's invalidation, so bump the generation here.
    let mut redis = build_test_redis()
        .await
        .map_err(|e| format!("redis: {e}"))?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await
        .map_err(|e| format!("bump policy cache generation: {e}"))?;
    Ok(())
}

async fn create_named_secret(
    app: &axum::Router,
    cookie: &str,
    key: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/automation-secrets",
        Some(&serde_json::json!({ "key": key }).to_string()),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create named secret");
    serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing named secret id")?
        .parse()
        .map_err(Into::into)
}

/// Seed a protected named secret directly, because the product REST create path
/// only produces unprotected named secrets. The credential gate must refuse it
/// with 403 and no binding may be written.
async fn seed_protected_named_secret(
    db: &sqlx::PgPool,
    key: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let secret_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO secrets_variable (id, key, kind, is_masked, is_protected)
        VALUES ($1, $2, 'named', true, true)
        "#,
    )
    .bind(secret_id)
    .bind(key)
    .execute(db)
    .await?;
    Ok(secret_id)
}

fn git_project_payload(name: &str, credential_id: Option<Uuid>) -> Value {
    serde_json::json!({
        "name": name,
        "scm_type": "git",
        "scm_url": format!("https://git.example.test/{name}.git"),
        "credential_id": credential_id,
    })
}

async fn project_id_by_name(
    db: &sqlx::PgPool,
    name: &str,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM playbook_project WHERE name = $1")
            .bind(name)
            .fetch_optional(db)
            .await?,
    )
}

async fn project_binding_secret_id(
    db: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query(
        r#"
        SELECT secret_id
        FROM secret_usage_binding
        WHERE consumer_type = $1
          AND consumer_id = $2
          AND field_key = $3
        "#,
    )
    .bind(PROJECT_CREDENTIAL_CONSUMER_TYPE)
    .bind(project_id)
    .bind(PROJECT_CREDENTIAL_FIELD_KEY)
    .fetch_optional(db)
    .await?;
    row.map(|row| row.try_get("secret_id"))
        .transpose()
        .map_err(Into::into)
}

async fn project_binding_count_for_secret(
    db: &sqlx::PgPool,
    secret_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM secret_usage_binding
        WHERE consumer_type = $1
          AND field_key = $2
          AND secret_id = $3
        "#,
    )
    .bind(PROJECT_CREDENTIAL_CONSUMER_TYPE)
    .bind(PROJECT_CREDENTIAL_FIELD_KEY)
    .bind(secret_id)
    .fetch_one(db)
    .await?)
}

async fn project_has_raw_credential_column(
    db: &sqlx::PgPool,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'playbook_project'
              AND column_name = 'credential_id'
        )
        "#,
    )
    .fetch_one(db)
    .await?)
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn project_scm_credential_uses_named_secret_binding_and_reconciles_lifecycle()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_disposable_pwa_database()?;

    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    grant_playbook_project_admin(&db).await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    // Create with a real named secret: exactly one canonical binding row and
    // the public contract returns the secret UUID. This is deliberately the
    // first lifecycle assertion so the pre-fix path fails RED on the missing
    // binding rather than only on input validation.
    let first_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-pwa53-credential-one-{suffix}"),
    )
    .await?;
    let project_name = format!("itest pwa53 project {suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&project_name, Some(first_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create project: {}",
        String::from_utf8_lossy(&body)
    );
    let created: Value = serde_json::from_slice(&body)?;
    let project_id: Uuid = created["id"]
        .as_str()
        .ok_or("missing project id")?
        .parse()?;
    assert_eq!(
        created["credential_id"].as_str(),
        Some(first_secret_id.to_string().as_str()),
        "the project contract must return the typed named-secret UUID",
    );
    assert_eq!(
        project_binding_secret_id(&db, project_id).await?,
        Some(first_secret_id),
        "project creation must project the named-secret reference into the canonical usage binding",
    );
    assert_eq!(
        project_binding_count_for_secret(&db, first_secret_id).await?,
        1,
        "project creation must write exactly one usage binding row",
    );

    // The bound secret is deletion-protected.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{first_secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a named secret referenced by a playbook project must be deletion-protected",
    );

    // Unknown credential: a UUID alone is insufficient; it must name an
    // existing non-protected named secret. Rejected before any project row.
    let nonexistent_secret_id = Uuid::now_v7();
    let invalid_name = format!("itest pwa53 project invalid-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&invalid_name, Some(nonexistent_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unknown credential_id must be rejected before a project is written",
    );
    assert!(
        project_id_by_name(&db, &invalid_name).await?.is_none(),
        "an invalid credential must not leave a project row behind",
    );

    // A field-kind secret is a real Piping secret but not an admin-managed
    // *named* secret.
    let field_secret_id = Uuid::parse_str(TEST_SECRET_ID)?;
    let field_name = format!("itest pwa53 project field-kind-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&field_name, Some(field_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a non-named secret must not be accepted as a project credential",
    );
    assert!(
        project_id_by_name(&db, &field_name).await?.is_none(),
        "a non-named credential must not leave a project row behind",
    );

    // A protected named secret is refused by the gate (403) with no binding.
    let protected_secret_id =
        seed_protected_named_secret(&db, &format!("itest-pwa53-protected-{suffix}")).await?;
    let protected_name = format!("itest pwa53 project protected-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&protected_name, Some(protected_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a protected named secret must not be bindable to a project",
    );
    assert!(
        project_id_by_name(&db, &protected_name).await?.is_none(),
        "a protected credential must not leave a project row behind",
    );
    assert_eq!(
        project_binding_count_for_secret(&db, protected_secret_id).await?,
        0,
        "a refused protected credential must not write a usage binding",
    );

    // Updating the credential reconciles the binding: the old row is gone and
    // the new one is the only row for this project.
    let second_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-pwa53-credential-two-{suffix}"),
    )
    .await?;
    let (status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        Some(&serde_json::json!({ "credential_id": second_secret_id }).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "replace project credential");
    assert_eq!(
        project_binding_secret_id(&db, project_id).await?,
        Some(second_secret_id),
        "project update must replace rather than accumulate credential bindings",
    );
    assert_eq!(
        project_binding_count_for_secret(&db, first_secret_id).await?,
        0,
        "the replaced credential binding must be gone",
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{first_secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the replaced credential must be unblocked",
    );

    // An update that does not carry the field preserves the existing binding.
    let (status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        Some(&serde_json::json!({ "description": "unchanged credential" }).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "update project without credential");
    assert_eq!(
        project_binding_secret_id(&db, project_id).await?,
        Some(second_secret_id),
        "an update that omits credential_id must not drop the binding",
    );

    // Deleting the project sweeps its binding and unblocks the secret.
    let third_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-pwa53-credential-three-{suffix}"),
    )
    .await?;
    let delete_name = format!("itest pwa53 project delete-{suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&delete_name, Some(third_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create project to delete");
    let delete_project_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing delete project id")?
        .parse()?;
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/projects/{delete_project_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete project");
    assert_eq!(
        project_binding_secret_id(&db, delete_project_id).await?,
        None,
        "deleting a project must sweep its credential binding transactionally",
    );
    assert_eq!(
        project_binding_count_for_secret(&db, third_secret_id).await?,
        0,
        "a deleted project must own no usage binding rows",
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{third_secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "deleted project must unblock its credential",
    );

    // The raw column is gone after migration 0005: the binding is the single
    // source of truth. Asserted after the lifecycle so a RED run exercises the
    // behavioral assertions, not only the column shape.
    assert!(
        !project_has_raw_credential_column(&db).await?,
        "the raw playbook_project credential column must be gone after migration 0005",
    );

    // Tear down the remaining project and its unbound secret.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "clean up project");
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{second_secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up the unbound credential",
    );

    Ok(())
}

async fn project_scm_url(
    db: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT scm_url FROM playbook_project WHERE id = $1")
            .bind(project_id)
            .fetch_optional(db)
            .await?,
    )
}

/// Restricted-principal helper: create a local user through the admin API so it
/// can authenticate. Mirrors the alert-source authority suite.
async fn create_local_user(
    app: &axum::Router,
    admin_cookie: &str,
    username: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let payload = serde_json::json!({
        "username": username,
        "email": format!("{username}@test.local"),
        "status": "active",
        "auth_source": "local",
        "credential": {
            "mode": "manual",
            "password": TEST_ADMIN_PASSWORD,
        },
    });
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(&payload.to_string()),
        Some(admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "create restricted local user");
    serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing restricted local user id")?
        .parse()
        .map_err(Into::into)
}

struct PolicySpec<'a> {
    name: String,
    effect: &'a str,
    actions: &'a [&'a str],
    resource_types: &'a [&'a str],
    subject: &'a str,
    resources: &'a [String],
}

async fn create_policy_for_subject(
    app: &axum::Router,
    admin_cookie: &str,
    spec: PolicySpec<'_>,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let payload = serde_json::json!({
        "name": spec.name,
        "description": null,
        "effect": spec.effect,
        "actions": spec.actions,
        "resource_types": spec.resource_types,
        "constraints": null,
        "subjects": [spec.subject],
        "resources": spec.resources,
        "enabled": true,
    });
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/admin/access-control/policies",
        Some(&payload.to_string()),
        Some(admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "create restricted-user policy");
    serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing restricted-user policy id")?
        .parse()
        .map_err(Into::into)
}

/// Defect 1: the update gate must authorize the EFFECTIVE credential. A caller
/// holding only `PLAYBOOK_PROJECT:Update` must not be able to change `scm_url`
/// on a project whose binding survives the request (`credential_id` absent),
/// because the next git checkout would hand the still-bound secret to the
/// caller's host.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn project_credential_update_requires_the_bound_secrets_authority()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_disposable_pwa_database()?;

    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    grant_playbook_project_admin(&db).await?;
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let secret_id = create_named_secret(
        &app,
        &admin_cookie,
        &format!("itest-pwa53-authority-{suffix}"),
    )
    .await?;
    let project_name = format!("itest pwa53 authority project {suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&project_name, Some(secret_id)).to_string()),
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create bound project: {}",
        String::from_utf8_lossy(&body)
    );
    let project_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing project id")?
        .parse()?;
    let original_url = project_scm_url(&db, project_id).await?;
    assert!(
        original_url.is_some(),
        "the bound project must store scm_url"
    );

    // Positive control: the same restricted principal can update a project with
    // no credential, proving a 403 below is the secret gate and not a missing
    // `PLAYBOOK_PROJECT:Update` grant.
    let control_name = format!("itest pwa53 authority control {suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&control_name, None).to_string()),
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create control project");
    let control_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing control project id")?
        .parse()?;

    let restricted_username = format!("itest-pwa53-authority-{suffix}");
    let restricted_user_id = create_local_user(&app, &admin_cookie, &restricted_username).await?;
    let restricted_subject = format!("user:{restricted_user_id}");
    let all_resources = vec!["*".to_string()];
    // PLAYBOOK_PROJECT READ/UPDATE only: deliberately no Secret authority.
    let update_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicySpec {
            name: format!("itest-pwa53-authority-update-{suffix}"),
            effect: "allow",
            actions: &["READ", "UPDATE"],
            resource_types: &["PLAYBOOK_PROJECT"],
            subject: &restricted_subject,
            resources: &all_resources,
        },
    )
    .await?;
    let restricted_cookie = login_and_get_cookie(&app, &restricted_username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("restricted user login failed")?;

    let (control_status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/playbooks/projects/{control_id}"),
        Some(&serde_json::json!({ "description": "restricted control update" }).to_string()),
        Some(&restricted_cookie),
    )
    .await?;

    // The attack: `credential_id` absent, only `scm_url` changes. The effective
    // credential is the existing bound secret, whose authority is required.
    let attacker_url = format!("https://attacker-{suffix}.example.test/repo.git");
    let (attack_status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        Some(&serde_json::json!({ "scm_url": attacker_url }).to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    // Mutating any field of a project that owns a disclosure-equivalent binding
    // is denied: the binding is part of the resource being changed.
    let (rename_status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        Some(&serde_json::json!({ "description": "restricted rename" }).to_string()),
        Some(&restricted_cookie),
    )
    .await?;

    let binding_after = project_binding_secret_id(&db, project_id).await?;
    let url_after = project_scm_url(&db, project_id).await?;

    // Cleanup also cleans a deliberate RED run where the attack succeeded.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "clean up bound project");
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/projects/{control_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "clean up control project");
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/policies/{update_policy_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clean up restricted policy");
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{restricted_user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clean up restricted user");
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up unbound named secret"
    );

    assert_eq!(
        control_status,
        StatusCode::OK,
        "the restricted principal may update a project with no credential",
    );
    assert_eq!(
        attack_status,
        StatusCode::FORBIDDEN,
        "changing scm_url on a bound project must require the bound secret's authority even when credential_id is absent",
    );
    assert_eq!(
        rename_status,
        StatusCode::FORBIDDEN,
        "mutating a bound project must require the bound secret's authority",
    );
    assert_eq!(
        binding_after,
        Some(secret_id),
        "a denied update must not change the binding",
    );
    assert_eq!(
        url_after, original_url,
        "a denied update must not redirect scm_url",
    );

    Ok(())
}

/// Defect 2: an explicit JSON `null` must clear the binding, matching the
/// nullable contract and the `Some(None)` semantics the service implements.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn project_credential_null_clears_binding_and_unblocks_secret()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_disposable_pwa_database()?;

    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    grant_playbook_project_admin(&db).await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let secret_id =
        create_named_secret(&app, &cookie, &format!("itest-pwa53-null-{suffix}")).await?;
    let project_name = format!("itest pwa53 null project {suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&project_name, Some(secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create bound project");
    let project_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing project id")?
        .parse()?;
    assert_eq!(
        project_binding_secret_id(&db, project_id).await?,
        Some(secret_id),
        "project must start bound",
    );

    let (status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        Some(&serde_json::json!({ "credential_id": null }).to_string()),
        Some(&cookie),
    )
    .await?;
    let binding_after = project_binding_secret_id(&db, project_id).await?;
    let (secret_status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;

    let (cleanup_status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/projects/{project_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    if secret_status != StatusCode::NO_CONTENT {
        // A RED run leaves the binding in place; deleting the project sweeps it
        // so the shared database is left clean regardless.
        let _ = request(
            &app,
            "DELETE",
            &format!("/api/v1/automation-secrets/{secret_id}"),
            None,
            Some(&cookie),
        )
        .await?;
    }

    assert_eq!(
        status,
        StatusCode::OK,
        "an explicit null credential_id must be accepted",
    );
    assert_eq!(
        binding_after, None,
        "an explicit null credential_id must clear the usage binding",
    );
    assert_eq!(
        secret_status,
        StatusCode::NO_CONTENT,
        "a cleared credential must become deletable",
    );
    assert_eq!(cleanup_status, StatusCode::NO_CONTENT, "clean up project");

    Ok(())
}

/// Defect 3: `delete_project` must lock the project row before sweeping
/// bindings, so an interleaved `update_project` cannot insert a binding the
/// sweep misses and that then outlives the project as an orphan that blocks the
/// secret forever.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn project_delete_locks_row_and_sweeps_concurrent_binding()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_disposable_pwa_database()?;

    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await?;
    grant_playbook_project_admin(&db).await?;
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let secret_id =
        create_named_secret(&app, &cookie, &format!("itest-pwa53-race-{suffix}")).await?;
    // Unbound project: the interleaved update below creates the first binding.
    let project_name = format!("itest pwa53 race project {suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/playbooks/projects",
        Some(&git_project_payload(&project_name, None).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create race project");
    let project_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing race project id")?
        .parse()?;

    // Hold the project row lock — the same lock `update_project` takes before it
    // reconciles its binding.
    let mut tx = db.begin().await?;
    sqlx::query("SELECT id FROM playbook_project WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;

    let delete_pool = db.clone();
    let delete_task = tokio::spawn(async move {
        SqlxPlaybookRepository::new(delete_pool)
            .delete_project(project_id)
            .await
    });

    // Wait until the delete is blocked on the project row. A row-lock wait is
    // reported through `pg_stat_activity` (locktype `transactionid`), not as an
    // ungranted relation lock, so observe the backend, not `pg_locks`. Under the
    // fixed code it blocks on its initial `FOR UPDATE` (before the sweep); under
    // the buggy code it blocks on the final DELETE, having already swept.
    let mut blocked = false;
    for _ in 0..400 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM pg_stat_activity \
             WHERE datname = current_database() \
               AND pid <> pg_backend_pid() \
               AND wait_event_type = 'Lock' \
               AND query ILIKE '%playbook_project%'",
        )
        .fetch_one(&mut *tx)
        .await?;
        if waiting > 0 {
            blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        blocked,
        "delete_project must take the project row lock before sweeping bindings",
    );

    // Simulate the interleaved update_project: insert its binding, then commit.
    sqlx::query(
        "INSERT INTO secret_usage_binding \
         (consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(PROJECT_CREDENTIAL_CONSUMER_TYPE)
    .bind(project_id)
    .bind(PROJECT_CREDENTIAL_FIELD_KEY)
    .bind(PROJECT_CREDENTIAL_CONSUMER_TYPE)
    .bind(project_id)
    .bind(secret_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let deleted = delete_task.await??;
    let orphan_bindings = project_binding_count_for_secret(&db, secret_id).await?;
    let project_gone = project_id_by_name(&db, &project_name).await?.is_none();
    let (secret_status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;

    assert!(deleted, "delete_project must remove the project row");
    assert!(project_gone, "the project row must be gone");
    assert_eq!(
        orphan_bindings, 0,
        "the row lock must make delete_project sweep the concurrent binding, not strand it",
    );
    assert_eq!(
        secret_status,
        StatusCode::NO_CONTENT,
        "an orphan-free delete must leave the secret deletable",
    );

    Ok(())
}
