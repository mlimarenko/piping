//! Integration coverage for monitoring query-source named-secret references.
//!
//! Requires PostgreSQL and Redis (e.g. `make -C apps/api run-up`). Run with:
//! `cargo test --test integration_monitoring_query_source_secret_lifecycle -- --ignored`
//! The quick-start preset RED regression additionally requires
//! `PIPING_TEST_DISPOSABLE_DB=1` and a disposable `pwa10_` database name.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, TEST_SECRET_ID, build_test_app_state,
    login_and_get_cookie, request, request_with_auth, unique_suffix,
};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

const QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE: &str = "monitoring_query_source";
const QUERY_SOURCE_CREDENTIAL_FIELD_KEY: &str = "bearer_token";
/// Query endpoint the PWA-10 runtime migration repoints a source to. The
/// deployment URL lives in `apps/api/migrations/README.md`; the test uses a
/// loopback name from `/etc/hosts` so it never depends on the runner's DNS.
const VMAUTH_QUERY_URL: &str = "http://ip6-localhost:49153/api/v1/query";

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

fn query_source_payload(suffix: &str, credential_ref: Option<Uuid>) -> Value {
    serde_json::json!({
        "name": format!("itest monitoring source {suffix}"),
        "slug": format!("itest-monitoring-source-{suffix}"),
        "source_type": "victoriametrics",
        "query_url": "https://monitoring.example.test/api/v1/query",
        "credential_ref": credential_ref,
        "timeout_ms": 3000,
        "options": {},
        "enabled": true,
    })
}

/// PWA-10 regressions that create access-control state must run only against a
/// declared disposable database. The explicit flag makes that ownership part
/// of the command; the name prefix prevents accidental use of the regular
/// development database.
fn require_pwa10_disposable_database() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url = std::env::var("PIPING_DATABASE__URL")?;
    let database_name = database_url
        .rsplit('/')
        .next()
        .and_then(|segment| segment.split('?').next())
        .unwrap_or_default();
    if std::env::var("PIPING_TEST_DISPOSABLE_DB").as_deref() == Ok("1")
        && database_name.starts_with("pwa10_")
    {
        return Ok(());
    }

    Err(
        "PWA-10 regression requires PIPING_TEST_DISPOSABLE_DB=1 and a disposable pwa10_ database"
            .into(),
    )
}

async fn query_source_binding_secret_id(
    db: &sqlx::PgPool,
    source_id: Uuid,
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
    .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(source_id)
    .bind(QUERY_SOURCE_CREDENTIAL_FIELD_KEY)
    .fetch_optional(db)
    .await?;

    row.map(|row| row.try_get("secret_id"))
        .transpose()
        .map_err(Into::into)
}

async fn query_source_binding_count_for_secret(
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
    .bind(QUERY_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(QUERY_SOURCE_CREDENTIAL_FIELD_KEY)
    .bind(secret_id)
    .fetch_one(db)
    .await?)
}

async fn query_source_name(
    db: &sqlx::PgPool,
    source_id: Uuid,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT name FROM monitoring_query_source WHERE id = $1")
            .bind(source_id)
            .fetch_optional(db)
            .await?,
    )
}

async fn query_source_id_by_slug(
    db: &sqlx::PgPool,
    slug: &str,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM monitoring_query_source WHERE slug = $1")
            .bind(slug)
            .fetch_optional(db)
            .await?,
    )
}

async fn query_source_id_by_query_url(
    db: &sqlx::PgPool,
    query_url: &str,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM monitoring_query_source WHERE query_url = $1")
            .bind(query_url)
            .fetch_optional(db)
            .await?,
    )
}

async fn query_source_assignment_ids_for_scope(
    db: &sqlx::PgPool,
    source_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT id
        FROM monitoring_query_source_assignment
        WHERE query_source_id = $1
          AND scope_type = $2
          AND scope_id = $3
        ORDER BY id
        "#,
    )
    .bind(source_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_all(db)
    .await?)
}

async fn metric_set_assignment_ids_for_scope(
    db: &sqlx::PgPool,
    scope_type: &str,
    scope_id: Uuid,
) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT id
        FROM monitoring_metric_set_assignment
        WHERE scope_type = $1
          AND scope_id = $2
        ORDER BY id
        "#,
    )
    .bind(scope_type)
    .bind(scope_id)
    .fetch_all(db)
    .await?)
}

async fn query_source_exists(
    db: &sqlx::PgPool,
    slug: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM monitoring_query_source WHERE slug = $1)",
    )
    .bind(slug)
    .fetch_one(db)
    .await?)
}

async fn query_source_exists_for_query_url(
    db: &sqlx::PgPool,
    query_url: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM monitoring_query_source WHERE query_url = $1)",
    )
    .bind(query_url)
    .fetch_one(db)
    .await?)
}

async fn query_source_has_legacy_credential_column(
    db: &sqlx::PgPool,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'monitoring_query_source'
              AND column_name = 'credential_ref'
        )
        "#,
    )
    .fetch_one(db)
    .await?)
}

async fn test_admin_policy_subjects(
    app: &axum::Router,
    cookie: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = request(
        app,
        "GET",
        "/api/v1/admin/access-control/policies/00000000-0000-7000-8000-000000000003",
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "load test-admin policy");
    let policy: Value = serde_json::from_slice(&body)?;
    let subjects = policy["subjects"]
        .as_array()
        .ok_or("test-admin policy has no subjects")?;
    let mut values = Vec::with_capacity(subjects.len());
    for subject in subjects {
        values.push(
            subject
                .as_str()
                .ok_or("test-admin policy subject must be a string")?
                .to_string(),
        );
    }
    Ok(values)
}

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

struct PolicyRule<'a> {
    name: String,
    effect: &'a str,
    actions: &'a [&'a str],
    resource_types: &'a [&'a str],
}

async fn create_policy_for_subject(
    app: &axum::Router,
    admin_cookie: &str,
    rule: PolicyRule<'_>,
    subject: &str,
    resources: &[String],
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let payload = serde_json::json!({
        "name": rule.name,
        "description": null,
        "effect": rule.effect,
        "actions": rule.actions,
        "resource_types": rule.resource_types,
        "constraints": null,
        "subjects": [subject],
        "resources": resources,
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

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn query_source_credentials_use_named_secret_bindings_and_reconcile_lifecycle()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    // A UUID-shaped reference alone is not sufficient: it must name an existing
    // named automation secret. This is deliberately the first assertion so the
    // pre-fix free-text persistence path fails RED instead of merely compiling.
    let nonexistent_secret_id = Uuid::now_v7();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(
            &query_source_payload(&format!("invalid-{suffix}"), Some(nonexistent_secret_id))
                .to_string(),
        ),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unknown credential_ref must be rejected before a source is written",
    );
    assert!(
        !query_source_exists(&db, &format!("itest-monitoring-source-invalid-{suffix}")).await?,
        "an invalid credential must not leave a source row behind",
    );

    // The pre-seeded field secret is a real Piping secret but not an
    // admin-managed *named* secret, so UUID shape/existence alone is still
    // insufficient. This uses the ordinary test fixture; the test itself
    // mutates product state only through REST.
    let field_secret_id = Uuid::parse_str(TEST_SECRET_ID)?;
    let field_secret_suffix = format!("field-kind-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&field_secret_suffix, Some(field_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a non-named secret must not be accepted as a query-source credential",
    );
    assert!(
        !query_source_exists(
            &db,
            &format!("itest-monitoring-source-{field_secret_suffix}"),
        )
        .await?,
        "a non-named credential must not leave a source row behind",
    );
    assert!(
        !query_source_has_legacy_credential_column(&db).await?,
        "the legacy free-text query-source credential column must be removed after backfill",
    );

    let first_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-monitoring-credential-one-{suffix}"),
    )
    .await?;
    for (alias_label, legacy_ref) in [
        ("legacy-short", format!("secret:{first_secret_id}")),
        ("legacy-uri", format!("secret://{first_secret_id}")),
        ("compact", first_secret_id.simple().to_string()),
        ("braced", format!("{{{first_secret_id}}}")),
        ("urn", format!("urn:uuid:{first_secret_id}")),
    ] {
        let legacy_slug = format!("itest-monitoring-source-{alias_label}-{suffix}");
        let legacy_alias_payload = serde_json::json!({
            "name": format!("itest legacy alias {alias_label} {suffix}"),
            "slug": legacy_slug.clone(),
            "source_type": "victoriametrics",
            "query_url": "https://monitoring.example.test/api/v1/query",
            "credential_ref": legacy_ref,
            "timeout_ms": 3000,
            "options": {},
            "enabled": true,
        });
        let (status, _) = request(
            &app,
            "POST",
            "/api/v1/monitoring/query-sources",
            Some(&legacy_alias_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "non-canonical {alias_label} credential text must be rejected by the UUID contract",
        );
        assert!(
            !query_source_exists(&db, &legacy_slug).await?,
            "rejected non-canonical credential text must not leave a source row behind",
        );
    }
    let unknown_alias_slug = format!("itest-monitoring-source-unknown-alias-{suffix}");
    let unknown_alias_payload = serde_json::json!({
        "name": format!("itest unknown alias {suffix}"),
        "slug": unknown_alias_slug.clone(),
        "source_type": "victoriametrics",
        "query_url": "https://monitoring.example.test/api/v1/query",
        "credential_secret_id": first_secret_id,
        "timeout_ms": 3000,
        "options": {},
        "enabled": true,
    });
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&unknown_alias_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unrecognized credential field aliases must be rejected rather than ignored",
    );
    assert!(
        !query_source_exists(&db, &unknown_alias_slug).await?,
        "an unrecognized credential alias must not create an unbound source",
    );
    let source_payload = query_source_payload(&suffix, Some(first_secret_id));
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&source_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create query source");
    let created: Value = serde_json::from_slice(&body)?;
    let source_id: Uuid = created["id"]
        .as_str()
        .ok_or("missing query source id")?
        .parse()?;
    let first_secret_ref = first_secret_id.to_string();
    assert_eq!(
        created["credential_ref"].as_str(),
        Some(first_secret_ref.as_str()),
        "the public query-source contract must return the typed named-secret UUID",
    );
    assert_eq!(
        query_source_binding_secret_id(&db, source_id).await?,
        Some(first_secret_id),
        "source creation must project the named-secret reference into the canonical usage binding",
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
        StatusCode::CONFLICT,
        "a named secret referenced by a query source must be deletion-protected",
    );

    let second_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-monitoring-credential-two-{suffix}"),
    )
    .await?;
    let replacement_payload = query_source_payload(&suffix, Some(second_secret_id));
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        Some(&replacement_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "replace source credential");
    assert_eq!(
        query_source_binding_secret_id(&db, source_id).await?,
        Some(second_secret_id),
        "source update must replace rather than accumulate credential bindings",
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
        "old credential must be unblocked"
    );

    let removal_payload = query_source_payload(&suffix, None);
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        Some(&removal_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clear source credential");
    assert_eq!(
        query_source_binding_secret_id(&db, source_id).await?,
        None,
        "clearing credential_ref must remove the usage binding in the same update",
    );

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
        "cleared credential must be deletable"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up the ordinary query source through its REST lifecycle"
    );

    let invalid_payload = serde_json::json!({
        "name": format!("itest malformed monitoring source {suffix}"),
        "slug": format!("itest-malformed-monitoring-source-{suffix}"),
        "source_type": "victoriametrics",
        "query_url": "https://monitoring.example.test/api/v1/query",
        "credential_ref": "secret://not-a-uuid",
    });
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&invalid_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert!(
        status.is_client_error(),
        "malformed credential_ref must be rejected, got {status}",
    );

    let third_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-monitoring-credential-three-{suffix}"),
    )
    .await?;
    let delete_source_payload =
        query_source_payload(&format!("delete-{suffix}"), Some(third_secret_id));
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&delete_source_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create source to delete");
    let source_to_delete: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing delete-source id")?
        .parse()?;

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{source_to_delete}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete query source");
    assert_eq!(
        query_source_binding_secret_id(&db, source_to_delete).await?,
        None,
        "deleting a source must sweep its credential binding transactionally",
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
        "deleted source must unblock its credential"
    );

    // Quick-start is a second query-source writer. Re-running it with `null`
    // must reconcile the same binding rather than leave the prior secret
    // protected forever.
    let quick_start_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-monitoring-credential-quick-start-{suffix}"),
    )
    .await?;
    // Quick-start has a separate request shape, so prove it cannot accidentally
    // re-open the migration-only compact, braced, or URN UUID spellings.
    for (alias_label, credential_ref) in [
        ("compact", quick_start_secret_id.simple().to_string()),
        ("braced", format!("{{{quick_start_secret_id}}}")),
        ("urn", format!("urn:uuid:{quick_start_secret_id}")),
    ] {
        let rejected_url =
            format!("https://quick-start-{alias_label}-{suffix}.example.test/api/v1/query");
        let rejected_payload = serde_json::json!({
            "name": format!("itest quick start {alias_label} {suffix}"),
            "query_url": rejected_url,
            "source_type": "victoriametrics",
            "credential_ref": credential_ref,
            "scope_type": "global",
        });
        let (status, _) = request(
            &app,
            "POST",
            "/api/v1/monitoring/quick-start",
            Some(&rejected_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "quick-start must reject non-canonical {alias_label} UUID text",
        );
        assert!(
            !query_source_exists_for_query_url(&db, &rejected_url).await?,
            "rejected quick-start {alias_label} UUID text must not leave a query source",
        );
    }
    let unknown_quick_start_alias_url =
        format!("https://quick-start-unknown-alias-{suffix}.example.test/api/v1/query");
    let unknown_quick_start_alias_payload = serde_json::json!({
        "name": format!("itest quick start unknown alias {suffix}"),
        "query_url": unknown_quick_start_alias_url,
        "source_type": "victoriametrics",
        "credential_secret_id": quick_start_secret_id,
        "scope_type": "global",
    });
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&unknown_quick_start_alias_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "quick-start must reject an unrecognized credential field alias",
    );
    assert!(
        !query_source_exists_for_query_url(&db, &unknown_quick_start_alias_url).await?,
        "an unrecognized quick-start credential alias must not write a query source",
    );
    let quick_start_url = format!("https://quick-start-{suffix}.example.test/api/v1/query");
    let quick_start_payload = serde_json::json!({
        "name": format!("itest quick start {suffix}"),
        "query_url": quick_start_url,
        "source_type": "victoriametrics",
        "credential_ref": quick_start_secret_id,
        "scope_type": "global",
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&quick_start_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "quick-start source with named credential"
    );
    let quick_start_source_id: Uuid = serde_json::from_slice::<Value>(&body)?["query_source_id"]
        .as_str()
        .ok_or("missing quick-start query source id")?
        .parse()?;
    assert_eq!(
        query_source_binding_secret_id(&db, quick_start_source_id).await?,
        Some(quick_start_secret_id),
        "quick-start creation must project its named credential binding",
    );

    let quick_start_clear_payload = serde_json::json!({
        "name": format!("itest quick start {suffix}"),
        "query_url": quick_start_url,
        "source_type": "victoriametrics",
        "credential_ref": null,
        "scope_type": "global",
    });
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&quick_start_clear_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "quick-start clears source credential"
    );
    assert_eq!(
        query_source_binding_secret_id(&db, quick_start_source_id).await?,
        None,
        "quick-start removal must delete its usage binding",
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{quick_start_secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "quick-start removal unblocks secret deletion"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{quick_start_source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up the quick-start query source through its REST lifecycle"
    );

    // Credential binding is disclosure-equivalent: a Secret denial on the
    // canonical secret resource must veto every writer. Configure it through
    // the public access-control API, not direct SQL, then prove the rejected
    // binding cannot persist.
    let denied_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-monitoring-credential-secret-deny-{suffix}"),
    )
    .await?;
    let deny_policy = serde_json::json!({
        "name": format!("itest-monitoring-query-source-deny-{suffix}"),
        "description": null,
        "effect": "deny",
        "actions": ["SECRET"],
        "resource_types": ["SECRET_VARIABLE"],
        "constraints": null,
        "subjects": test_admin_policy_subjects(&app, &cookie).await?,
        "resources": [format!("secret_variable:{denied_secret_id}")],
        "enabled": true,
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/admin/access-control/policies",
        Some(&deny_policy.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "create explicit Secret deny policy");
    let deny_policy_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing Secret deny policy id")?
        .parse()?;

    // The create, update, and quick-start writers are independently wired.
    // Start with an unbound source, then make the denied update prove the
    // existing binding stays untouched.
    let denied_update_suffix = format!("secret-denied-update-{suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&denied_update_suffix, None).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create Secret-denied update target"
    );
    let denied_update_source_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing Secret-denied update target id")?
        .parse()?;
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-sources/{denied_update_source_id}"),
        Some(&query_source_payload(&denied_update_suffix, Some(denied_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Secret denial must veto query-source credential replacement",
    );
    assert_eq!(
        query_source_binding_secret_id(&db, denied_update_source_id).await?,
        None,
        "a Secret-denied update must not create a credential binding",
    );

    let denied_slug_suffix = format!("secret-denied-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&denied_slug_suffix, Some(denied_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Secret denial must veto credential binding"
    );
    assert!(
        !query_source_exists(
            &db,
            &format!("itest-monitoring-source-{denied_slug_suffix}"),
        )
        .await?,
        "a Secret-denied credential must not write a query source",
    );

    let denied_quick_start_url =
        format!("https://quick-start-secret-denied-{suffix}.example.test/api/v1/query");
    let denied_quick_start_payload = serde_json::json!({
        "name": format!("itest quick start Secret denied {suffix}"),
        "query_url": denied_quick_start_url,
        "source_type": "victoriametrics",
        "credential_ref": denied_secret_id,
        "scope_type": "global",
    });
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&denied_quick_start_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Secret denial must veto quick-start credential binding",
    );
    assert!(
        !query_source_exists_for_query_url(&db, &denied_quick_start_url).await?,
        "a Secret-denied quick-start credential must not write a query source",
    );
    assert_eq!(
        query_source_binding_count_for_secret(&db, denied_secret_id).await?,
        0,
        "Secret-denied query-source writers must not leave secret bindings",
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/policies/{deny_policy_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "remove temporary Secret deny policy"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{denied_update_source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up Secret-denied update target"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{denied_secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "Secret-rejected credential remains unbound"
    );

    // Named-secret deletion and credential binding use incompatible locks on
    // the same secret row (`FOR UPDATE` vs `FOR KEY SHARE`). Each concurrent
    // pair must serialize into one of the two valid terminal states; neither
    // a dangling binding nor a source bound to a deleted secret is allowed.
    for attempt in 0..6 {
        let race_secret_id = create_named_secret(
            &app,
            &cookie,
            &format!("itest-monitoring-credential-race-{attempt}-{suffix}"),
        )
        .await?;
        let race_slug_suffix = format!("race-{attempt}-{suffix}");
        let race_slug = format!("itest-monitoring-source-{race_slug_suffix}");
        let create_payload =
            query_source_payload(&race_slug_suffix, Some(race_secret_id)).to_string();
        let delete_path = format!("/api/v1/automation-secrets/{race_secret_id}");

        let race = tokio::time::timeout(Duration::from_secs(10), async {
            let (created, deleted) = tokio::join!(
                request(
                    &app,
                    "POST",
                    "/api/v1/monitoring/query-sources",
                    Some(&create_payload),
                    Some(&cookie),
                ),
                request(&app, "DELETE", &delete_path, None, Some(&cookie)),
            );
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((created?, deleted?))
        })
        .await
        .map_err(|_| "timed out waiting for concurrent source-bind/delete serialization")??;
        let ((create_status, create_body), (delete_status, _)) = race;

        match (create_status, delete_status) {
            (StatusCode::CREATED, StatusCode::CONFLICT) => {
                let race_source_id: Uuid = serde_json::from_slice::<Value>(&create_body)?["id"]
                    .as_str()
                    .ok_or("missing raced query source id")?
                    .parse()?;
                assert_eq!(
                    query_source_binding_secret_id(&db, race_source_id).await?,
                    Some(race_secret_id),
                    "successful source creation must retain the deletion guard",
                );
                let (status, _) = request(
                    &app,
                    "DELETE",
                    &format!("/api/v1/monitoring/query-sources/{race_source_id}"),
                    None,
                    Some(&cookie),
                )
                .await?;
                assert_eq!(status, StatusCode::NO_CONTENT, "clean up raced source");
                let (status, _) = request(
                    &app,
                    "DELETE",
                    &format!("/api/v1/automation-secrets/{race_secret_id}"),
                    None,
                    Some(&cookie),
                )
                .await?;
                assert_eq!(
                    status,
                    StatusCode::NO_CONTENT,
                    "source cleanup unblocks secret"
                );
            }
            (StatusCode::BAD_REQUEST, StatusCode::NO_CONTENT) => {
                assert!(
                    !query_source_exists(&db, &race_slug).await?,
                    "a deleted credential must roll back the concurrent source write",
                );
                assert_eq!(
                    query_source_binding_count_for_secret(&db, race_secret_id).await?,
                    0,
                    "a deleted credential must not retain a binding",
                );
            }
            unexpected => {
                return Err(format!(
                    "unexpected source-bind/delete race outcome: create={}, delete={}",
                    unexpected.0, unexpected.1
                )
                .into());
            }
        }
    }

    Ok(())
}

/// PWA-10 runtime migration. A legacy source is repointed at the canonical
/// vmauth endpoint and bound to a named secret through the supported REST
/// write path; the test proves the persisted reference and that the credential
/// value never appears in any response body.
#[tokio::test]
#[ignore] // Requires PostgreSQL, Redis, and an explicitly disposable PWA-10 database.
async fn runtime_migration_points_legacy_query_source_at_vmauth_without_exposing_token()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_pwa10_disposable_database()?;

    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    // Every response body this test receives is retained so the final assertion
    // can prove the credential value never leaves the secret store.
    let mut observed_response_bodies: Vec<Vec<u8>> = Vec::new();

    let secret_id =
        create_named_secret(&app, &cookie, &format!("itest-vmauth-query-token-{suffix}")).await?;

    // Operators load the real vmauth `query-bearer-token` from a token file; a
    // test sentinel makes an accidental disclosure detectable without embedding
    // a live credential.
    let sentinel = format!("vmauth-query-bearer-{suffix}");
    let value_payload = serde_json::json!({ "value": sentinel }).to_string();
    let (status, body) = request_with_auth(
        &app,
        "PUT",
        &format!("/api/v1/secrets/{secret_id}/value"),
        Some(&value_payload),
        Some(&cookie),
        None,
    )
    .await?;
    observed_response_bodies.push(body);
    assert_eq!(status, StatusCode::NO_CONTENT, "set named secret value");

    // The supported migration write path: point the source at the canonical
    // vmauth endpoint and bind the named secret as its bearer token.
    let migration_payload = serde_json::json!({
        "name": format!("itest vmauth migration {suffix}"),
        "slug": format!("itest-vmauth-migration-{suffix}"),
        "source_type": "victoriametrics",
        "query_url": VMAUTH_QUERY_URL,
        "credential_ref": secret_id,
        "timeout_ms": 3000,
        "options": {},
        "enabled": true,
    });
    let (status, body) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&migration_payload.to_string()),
        Some(&cookie),
        None,
    )
    .await?;
    observed_response_bodies.push(body.clone());
    assert_eq!(
        status,
        StatusCode::CREATED,
        "migrate query source to the vmauth endpoint",
    );
    let source_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing query source id")?
        .parse()?;
    let source_id_text = source_id.to_string();
    let secret_id_text = secret_id.to_string();

    // The query-source API has no single-item GET; select the migrated source
    // from the list route.
    let (status, body) = request_with_auth(
        &app,
        "GET",
        "/api/v1/monitoring/query-sources",
        None,
        Some(&cookie),
        None,
    )
    .await?;
    observed_response_bodies.push(body.clone());
    assert_eq!(status, StatusCode::OK, "read migrated query source");
    let listed: Value = serde_json::from_slice(&body)?;
    let item = listed["items"]
        .as_array()
        .ok_or("missing query source items")?
        .iter()
        .find(|item| item["id"].as_str() == Some(source_id_text.as_str()))
        .ok_or("migrated query source missing from the list")?;
    assert_eq!(
        item["query_url"].as_str(),
        Some(VMAUTH_QUERY_URL),
        "the migrated source must persist the canonical vmauth endpoint",
    );
    assert_eq!(
        item["credential_ref"].as_str(),
        Some(secret_id_text.as_str()),
        "the migrated source must retain a non-null named-secret credential",
    );

    assert_eq!(
        query_source_binding_secret_id(&db, source_id).await?,
        Some(secret_id),
        "the migration must record the canonical query-source usage binding",
    );

    // The live test route matches against effective (assigned) sources, so bind
    // the migrated source globally. Without this the route would only take the
    // ad-hoc one-shot branch, which does not reuse the persisted credential.
    let (status, body) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/query-source-assignments",
        Some(
            &serde_json::json!({
                "query_source_id": source_id,
                "scope_type": "global",
                "enabled": true,
            })
            .to_string(),
        ),
        Some(&cookie),
        None,
    )
    .await?;
    observed_response_bodies.push(body.clone());
    assert_eq!(
        status,
        StatusCode::CREATED,
        "assign the migrated source globally so the live test route can resolve it: {}",
        String::from_utf8_lossy(&body),
    );

    // Resolve the persisted source through the live test route so the
    // no-exposure scan covers a body built from the bound credential, not only
    // responses from routes that cannot carry it. The route answers upstream
    // failures inside a 200 body. A matched *persisted* source is the one branch
    // whose admin provenance lets a privately-resolving host through the SSRF
    // pin, so this 200 is what proves the stored URL matched an effective
    // source and its binding was read — `source_resolved` alone is true for the
    // unauthenticated one-shot branch too.
    let (status, body) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries/test",
        Some(
            &serde_json::json!({
                "query": "up",
                "source_type": "victoriametrics",
                "query_url": VMAUTH_QUERY_URL,
            })
            .to_string(),
        ),
        Some(&cookie),
        None,
    )
    .await?;
    observed_response_bodies.push(body.clone());
    assert_eq!(
        status,
        StatusCode::OK,
        "live metric test over the migrated source must answer inside a 200 body: {}",
        String::from_utf8_lossy(&body),
    );
    let test_response: Value = serde_json::from_slice(&body)?;
    assert_eq!(
        test_response["source_resolved"],
        Value::Bool(true),
        "the live metric test must resolve the persisted source and its bound credential: {}",
        String::from_utf8_lossy(&body),
    );

    let (status, body) = request_with_auth(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&cookie),
        None,
    )
    .await?;
    observed_response_bodies.push(body);
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the bound credential must be deletion-protected",
    );

    for (index, response_body) in observed_response_bodies.iter().enumerate() {
        let text = String::from_utf8_lossy(response_body);
        assert!(
            !text.contains(&sentinel),
            "response body {index} exposed the vmauth bearer token",
        );
    }

    Ok(())
}

/// The live metric-test SSRF guard must distinguish an admin-created effective
/// source from an ad-hoc URL, on a proof that does not depend on the runner's
/// external DNS. `ip6-localhost` resolves to loopback from `/etc/hosts` — the
/// same name the resolver unit tests use. The assigned source's URL is admitted
/// by its provenance; the identical loopback resolution for an unassigned URL
/// is rejected as SSRF.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn metric_query_test_trusts_assigned_source_but_not_ad_hoc_url_on_loopback()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let assigned_url = "http://ip6-localhost:49151/api/v1/query";
    let source_payload = serde_json::json!({
        "name": format!("itest loopback source {suffix}"),
        "slug": format!("itest-loopback-source-{suffix}"),
        "source_type": "victoriametrics",
        "query_url": assigned_url,
        "timeout_ms": 1000,
        "options": {},
        "enabled": true,
    });
    let (status, body) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&source_payload.to_string()),
        Some(&cookie),
        None,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create loopback query source: {}",
        String::from_utf8_lossy(&body),
    );
    let source_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing loopback source id")?
        .parse()?;

    let (status, body) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/query-source-assignments",
        Some(
            &serde_json::json!({
                "query_source_id": source_id,
                "scope_type": "global",
                "enabled": true,
            })
            .to_string(),
        ),
        Some(&cookie),
        None,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "assign the loopback source globally: {}",
        String::from_utf8_lossy(&body),
    );

    // Ad-hoc control: the same loopback resolution without a matching source
    // stays blocked by the strict SSRF guard.
    let (status, body) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries/test",
        Some(
            &serde_json::json!({
                "query": "up",
                "source_type": "victoriametrics",
                "query_url": "http://ip6-localhost:49152/api/v1/query",
            })
            .to_string(),
        ),
        Some(&cookie),
        None,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unassigned loopback URL must stay SSRF-blocked: {}",
        String::from_utf8_lossy(&body),
    );
    assert!(
        String::from_utf8_lossy(&body).contains("private/internal address"),
        "the ad-hoc control must fail at the SSRF guard: {}",
        String::from_utf8_lossy(&body),
    );

    // The assigned source's identical loopback host is trusted by provenance,
    // and the route answers upstream reachability inside a 200 body.
    let (status, body) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries/test",
        Some(
            &serde_json::json!({
                "query": "up",
                "source_type": "victoriametrics",
                "query_url": assigned_url,
            })
            .to_string(),
        ),
        Some(&cookie),
        None,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "a matched persisted source must resolve through the SSRF pin: {}",
        String::from_utf8_lossy(&body),
    );

    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn query_source_urls_reject_inline_credential_channels_before_persistence()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let baseline_suffix = format!("endpoint-url-baseline-{suffix}");
    let baseline_payload = query_source_payload(&baseline_suffix, None);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&baseline_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create URL-validation update target"
    );
    let baseline_source_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing URL-validation update target id")?
        .parse()?;

    for (channel, query_url) in [
        (
            "userinfo",
            format!("https://operator@monitoring-{suffix}.example.test/api/v1/query"),
        ),
        (
            "userinfo after extra slash",
            format!("https:///operator:placeholder@monitoring-{suffix}.example.test/api/v1/query"),
        ),
        (
            "userinfo without slash",
            format!("https:operator:placeholder@monitoring-{suffix}.example.test/api/v1/query"),
        ),
        (
            "userinfo after backslash",
            format!(
                r"https:\\\\operator:placeholder@monitoring-{suffix}.example.test/api/v1/query"
            ),
        ),
        (
            "query component",
            format!("https://monitoring-{suffix}.example.test/api/v1/query?scope=platform"),
        ),
        (
            "fragment",
            format!("https://monitoring-{suffix}.example.test/api/v1/query#diagnostic"),
        ),
    ] {
        let create_suffix = format!("endpoint-url-create-{channel}-{suffix}");
        let create_slug = format!("itest-monitoring-source-{create_suffix}");
        let mut create_payload = query_source_payload(&create_suffix, None);
        create_payload["query_url"] = Value::String(query_url.clone());
        let (create_status, _) = request(
            &app,
            "POST",
            "/api/v1/monitoring/query-sources",
            Some(&create_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        let create_source_id = query_source_id_by_slug(&db, &create_slug).await?;
        let create_cleanup_status = if let Some(source_id) = create_source_id {
            let (status, _) = request(
                &app,
                "DELETE",
                &format!("/api/v1/monitoring/query-sources/{source_id}"),
                None,
                Some(&cookie),
            )
            .await?;
            Some(status)
        } else {
            None
        };

        let mut update_payload = baseline_payload.clone();
        update_payload["query_url"] = Value::String(query_url.clone());
        let (update_status, _) = request(
            &app,
            "PUT",
            &format!("/api/v1/monitoring/query-sources/{baseline_source_id}"),
            Some(&update_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        // A deliberately weakened validator can persist the bad URL during a
        // RED proof. Restore the known-good source before asserting so this
        // ignored integration test leaves no credential-like URL behind.
        let (update_restore_status, _) = request(
            &app,
            "PUT",
            &format!("/api/v1/monitoring/query-sources/{baseline_source_id}"),
            Some(&baseline_payload.to_string()),
            Some(&cookie),
        )
        .await?;

        let quick_start_payload = serde_json::json!({
            "query_url": query_url.clone(),
            "source_type": "victoriametrics",
            "name": format!("itest endpoint-url quick-start {channel} {suffix}"),
        });
        let (quick_start_status, _) = request(
            &app,
            "POST",
            "/api/v1/monitoring/quick-start",
            Some(&quick_start_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        let quick_start_source_id = query_source_id_by_query_url(&db, &query_url).await?;
        let quick_start_cleanup_status = if let Some(source_id) = quick_start_source_id {
            let (status, _) = request(
                &app,
                "DELETE",
                &format!("/api/v1/monitoring/query-sources/{source_id}"),
                None,
                Some(&cookie),
            )
            .await?;
            Some(status)
        } else {
            None
        };

        assert_eq!(
            create_status,
            StatusCode::BAD_REQUEST,
            "query-source create must reject URL {channel} before persistence",
        );
        assert!(
            create_source_id.is_none(),
            "query-source create must not persist URL {channel}",
        );
        assert_eq!(
            update_status,
            StatusCode::BAD_REQUEST,
            "query-source update must reject URL {channel} before persistence",
        );
        assert_eq!(
            update_restore_status,
            StatusCode::OK,
            "restore URL-validation update target after {channel} attempt",
        );
        assert_eq!(
            quick_start_status,
            StatusCode::BAD_REQUEST,
            "quick-start must reject URL {channel} before persistence",
        );
        assert!(
            quick_start_source_id.is_none(),
            "quick-start must not persist URL {channel}",
        );
        assert!(
            query_source_id_by_query_url(&db, &query_url)
                .await?
                .is_none(),
            "no source with URL {channel} may remain after a rejected request",
        );
        for (operation, cleanup_status) in [
            ("create", create_cleanup_status),
            ("quick-start", quick_start_cleanup_status),
        ] {
            if let Some(cleanup_status) = cleanup_status {
                assert_eq!(
                    cleanup_status,
                    StatusCode::NO_CONTENT,
                    "a RED-proof {operation} source for URL {channel} must be removable before assertion",
                );
            }
        }
    }

    let (baseline_cleanup_status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{baseline_source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        baseline_cleanup_status,
        StatusCode::NO_CONTENT,
        "clean up URL-validation update target",
    );

    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn quick_start_cannot_mutate_existing_source_when_update_is_explicitly_denied()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let original_secret_id = create_named_secret(
        &app,
        &admin_cookie,
        &format!("itest-monitoring-quick-start-original-{suffix}"),
    )
    .await?;
    let replacement_secret_id = create_named_secret(
        &app,
        &admin_cookie,
        &format!("itest-monitoring-quick-start-replacement-{suffix}"),
    )
    .await?;
    let query_url = format!("https://quick-start-update-deny-{suffix}.example.test/api/v1/query");
    let original_name = format!("itest quick start update deny original {suffix}");
    let create_payload = serde_json::json!({
        "name": original_name,
        "query_url": query_url,
        "source_type": "victoriametrics",
        "credential_ref": original_secret_id,
        "scope_type": "global",
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&create_payload.to_string()),
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "admin creates quick-start source");
    let source_id: Uuid = serde_json::from_slice::<Value>(&body)?["query_source_id"]
        .as_str()
        .ok_or("missing quick-start source id")?
        .parse()?;
    assert_eq!(
        query_source_binding_secret_id(&db, source_id).await?,
        Some(original_secret_id),
        "admin-created quick-start source must begin bound to its original named secret",
    );

    let restricted_username = format!("itest-monitoring-create-only-{suffix}");
    let restricted_user_id = create_local_user(&app, &admin_cookie, &restricted_username).await?;
    let restricted_subject = format!("user:{restricted_user_id}");
    let all_resources = vec!["*".to_string()];
    let secret_resources = vec![
        format!("secret_variable:{original_secret_id}"),
        format!("secret_variable:{replacement_secret_id}"),
    ];
    let create_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-quick-start-create-{suffix}"),
            effect: "allow",
            actions: &["CREATE"],
            resource_types: &["MONITORING"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    let update_deny_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-quick-start-update-deny-{suffix}"),
            effect: "deny",
            actions: &["UPDATE"],
            resource_types: &["MONITORING"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    // Grant Secret disclosure authority for both candidate named secrets so a
    // 403 can only be the
    // explicit Monitoring×Update deny, rather than credential-use gating.
    let secret_use_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-quick-start-secret-use-{suffix}"),
            effect: "allow",
            actions: &["SECRET"],
            resource_types: &["SECRET_VARIABLE"],
        },
        &restricted_subject,
        &secret_resources,
    )
    .await?;
    let restricted_cookie = login_and_get_cookie(&app, &restricted_username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("restricted user login failed")?;

    // Establish the positive half of the policy: this is not a user with no
    // monitoring permission. It can use quick-start to create a genuinely new
    // unbound source, but must not use that Create grant as an Update grant on
    // a source that already exists.
    let create_only_url =
        format!("https://quick-start-create-only-{suffix}.example.test/api/v1/query");
    let create_only_payload = serde_json::json!({
        "name": format!("itest quick start create only {suffix}"),
        "query_url": create_only_url,
        "source_type": "victoriametrics",
        "credential_ref": null,
        "scope_type": "global",
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&create_only_payload.to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "the restricted subject must retain its Monitoring×Create quick-start capability",
    );
    let create_only_source_id: Uuid = serde_json::from_slice::<Value>(&body)?["query_source_id"]
        .as_str()
        .ok_or("missing create-only quick-start source id")?
        .parse()?;

    let rebind_name = format!("itest quick start update deny rebind {suffix}");
    let rebind_payload = serde_json::json!({
        "name": rebind_name,
        "query_url": query_url,
        "source_type": "victoriametrics",
        "credential_ref": replacement_secret_id,
        "scope_type": "global",
    });
    let (rebind_status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&rebind_payload.to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    let name_after_rebind = query_source_name(&db, source_id).await?;
    let binding_after_rebind = query_source_binding_secret_id(&db, source_id).await?;

    let clear_payload = serde_json::json!({
        "name": format!("itest quick start update deny clear {suffix}"),
        "query_url": query_url,
        "source_type": "victoriametrics",
        "credential_ref": null,
        "scope_type": "global",
    });
    let (clear_status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&clear_payload.to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    let name_after_clear = query_source_name(&db, source_id).await?;
    let binding_after_clear = query_source_binding_secret_id(&db, source_id).await?;

    for policy_id in [
        create_policy_id,
        update_deny_policy_id,
        secret_use_policy_id,
    ] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/policies/{policy_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::OK,
            "remove temporary restricted-user policy"
        );
    }
    for cleanup_source_id in [source_id, create_only_source_id] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/monitoring/query-sources/{cleanup_source_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "clean up quick-start source"
        );
    }
    for secret_id in [original_secret_id, replacement_secret_id] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/automation-secrets/{secret_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::NO_CONTENT, "clean up named credential");
    }
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{restricted_user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clean up restricted local user");

    // Deliberately assert after REST cleanup: the expected RED execution
    // against the pre-fix route must not leave policies, users, sources, or
    // named-secret bindings in the shared integration database.
    assert_eq!(
        rebind_status,
        StatusCode::FORBIDDEN,
        "Monitoring×Create must not authorize a quick-start rebind of an existing source when Monitoring×Update is explicitly denied",
    );
    assert_eq!(
        name_after_rebind,
        Some(original_name.clone()),
        "a denied quick-start rebind must not change the existing source name",
    );
    assert_eq!(
        binding_after_rebind,
        Some(original_secret_id),
        "a denied quick-start rebind must retain the original named-secret binding",
    );
    assert_eq!(
        clear_status,
        StatusCode::FORBIDDEN,
        "Monitoring×Create must not authorize clearing an existing source when Monitoring×Update is explicitly denied",
    );
    assert_eq!(
        name_after_clear,
        Some(original_name),
        "a denied quick-start clear must not change the existing source name",
    );
    assert_eq!(
        binding_after_clear,
        Some(original_secret_id),
        "a denied quick-start clear must retain the original named-secret binding",
    );

    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; CI supplies an isolated pwa10_ database.
async fn metric_query_test_requires_secret_authority_for_a_bound_source()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_pwa10_disposable_database()?;
    let state = build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();
    let secret_id = create_named_secret(
        &app,
        &admin_cookie,
        &format!("itest-monitoring-live-test-credential-{suffix}"),
    )
    .await?;
    let query_url = format!("https://metric-query-authz-{suffix}.example.test/api/v1/query");
    // Global source selection is assignment-based. Use the public quick-start
    // composite rather than a bare source row so the live-test resolver finds
    // the bound credential on its normal global path.
    let source_payload = serde_json::json!({
        "name": format!("itest metric-query authz {suffix}"),
        "query_url": query_url.clone(),
        "source_type": "victoriametrics",
        "credential_ref": secret_id,
        "scope_type": "global",
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&source_payload.to_string()),
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "create global source with named credential"
    );
    let source_id: Uuid = serde_json::from_slice::<Value>(&body)?["query_source_id"]
        .as_str()
        .ok_or("missing global source id")?
        .parse()?;

    let restricted_username = format!("itest-monitoring-live-test-denied-{suffix}");
    let restricted_user_id = create_local_user(&app, &admin_cookie, &restricted_username).await?;
    let restricted_subject = format!("user:{restricted_user_id}");
    let all_resources = vec!["*".to_string()];
    let secret_resource = vec![format!("secret_variable:{secret_id}")];
    let monitoring_update_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-live-test-update-{suffix}"),
            effect: "allow",
            actions: &["UPDATE"],
            resource_types: &["MONITORING"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    // Grant broad secret-use authority first. The exact canonical resource
    // denial below must still veto live credential disclosure, proving both
    // deny precedence and the `secret_variable:{id}` resource shape.
    let secret_use_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-live-test-secret-use-{suffix}"),
            effect: "allow",
            actions: &["SECRET"],
            resource_types: &["SECRET_VARIABLE"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    let secret_deny_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-live-test-secret-deny-{suffix}"),
            effect: "deny",
            actions: &["SECRET"],
            resource_types: &["SECRET_VARIABLE"],
        },
        &restricted_subject,
        &secret_resource,
    )
    .await?;
    let restricted_cookie = login_and_get_cookie(&app, &restricted_username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("restricted test user login failed")?;

    // The hostname is intentionally unresolvable. A correct route returns 403
    // before DNS or secret lookup; a pre-fix route reaches source resolution
    // despite an explicit Secret denial and therefore returns a different
    // failure instead of the authorization verdict.
    let (test_status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/metric-queries/test",
        Some(
            &serde_json::json!({
                "query_url": query_url,
                "source_type": "victoriametrics",
                "query": "up",
            })
            .to_string(),
        ),
        Some(&restricted_cookie),
    )
    .await?;

    for policy_id in [
        secret_deny_policy_id,
        secret_use_policy_id,
        monitoring_update_policy_id,
    ] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/policies/{policy_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "clean up temporary policy");
    }
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up bound query source"
    );
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
        "clean up bound named secret"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{restricted_user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clean up restricted local user");

    assert_eq!(
        test_status,
        StatusCode::FORBIDDEN,
        "Monitoring×Update must not authorize a live query that would use a Secret-denied bound credential",
    );

    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL, Redis, and an explicitly disposable PWA-10 database.
async fn quick_start_existing_source_still_requires_create_when_only_update_is_allowed()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_pwa10_disposable_database()?;
    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let original_secret_id = create_named_secret(
        &app,
        &admin_cookie,
        &format!("itest-monitoring-create-baseline-original-{suffix}"),
    )
    .await?;
    let replacement_secret_id = create_named_secret(
        &app,
        &admin_cookie,
        &format!("itest-monitoring-create-baseline-replacement-{suffix}"),
    )
    .await?;
    let query_url =
        format!("https://quick-start-create-baseline-{suffix}.example.test/api/v1/query");
    let original_name = format!("itest quick start create baseline original {suffix}");
    let create_payload = serde_json::json!({
        "name": original_name,
        "query_url": query_url,
        "source_type": "victoriametrics",
        "credential_ref": original_secret_id,
        "scope_type": "global",
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&create_payload.to_string()),
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "admin creates deterministic source");
    let source_id: Uuid = serde_json::from_slice::<Value>(&body)?["query_source_id"]
        .as_str()
        .ok_or("missing deterministic quick-start source id")?
        .parse()?;

    let restricted_username = format!("itest-monitoring-update-only-{suffix}");
    let restricted_user_id = create_local_user(&app, &admin_cookie, &restricted_username).await?;
    let restricted_subject = format!("user:{restricted_user_id}");
    let all_resources = vec!["*".to_string()];
    let replacement_secret_resources = vec![format!("secret_variable:{replacement_secret_id}")];
    let update_allow_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-quick-start-update-allow-{suffix}"),
            effect: "allow",
            actions: &["UPDATE"],
            resource_types: &["MONITORING"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    let create_deny_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-quick-start-create-deny-{suffix}"),
            effect: "deny",
            actions: &["CREATE"],
            resource_types: &["MONITORING"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    // Secret disclosure authority is deliberately allowed for the proposed
    // replacement. This keeps the
    // expected 403 anchored to the endpoint's original Monitoring×Create
    // baseline, not to named-secret authorization.
    let secret_use_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-quick-start-create-baseline-secret-use-{suffix}"),
            effect: "allow",
            actions: &["SECRET"],
            resource_types: &["SECRET_VARIABLE"],
        },
        &restricted_subject,
        &replacement_secret_resources,
    )
    .await?;
    let restricted_cookie = login_and_get_cookie(&app, &restricted_username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("update-only user login failed")?;

    // The fresh scope isolates assignment IDs. Preset application can still
    // touch canonical preset rows under a broken authorization path, so this
    // regression is guarded above to run only against a disposable database.
    // The source itself already exists because its slug is deterministically
    // derived from the same source_type + query_url pair as the administrator's
    // request.
    let scope_type = "connection_target";
    let scope_id = Uuid::now_v7();
    assert!(
        query_source_assignment_ids_for_scope(&db, source_id, scope_type, scope_id)
            .await?
            .is_empty(),
        "the unique scope must have no source assignment before the denied request",
    );
    assert!(
        metric_set_assignment_ids_for_scope(&db, scope_type, scope_id)
            .await?
            .is_empty(),
        "the unique scope must have no preset assignment before the denied request",
    );

    let attempted_name = format!("itest quick start create baseline attempted {suffix}");
    let attempted_payload = serde_json::json!({
        "name": attempted_name,
        "query_url": query_url,
        "source_type": "victoriametrics",
        "credential_ref": replacement_secret_id,
        "preset_slug": "node",
        "scope_type": scope_type,
        "scope_id": scope_id,
    });
    let (quick_start_status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/quick-start",
        Some(&attempted_payload.to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    let name_after_attempt = query_source_name(&db, source_id).await?;
    let binding_after_attempt = query_source_binding_secret_id(&db, source_id).await?;
    let source_assignment_ids_after_attempt =
        query_source_assignment_ids_for_scope(&db, source_id, scope_type, scope_id).await?;
    let preset_assignment_ids_after_attempt =
        metric_set_assignment_ids_for_scope(&db, scope_type, scope_id).await?;

    // Clean attempted per-scope assignments through public routes before the
    // assertion. The disposable-database guard covers canonical preset rows
    // that a deliberately broken implementation could also touch.
    for assignment_id in &preset_assignment_ids_after_attempt {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/monitoring/metric-set-assignments/{assignment_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "clean up attempted preset assignment"
        );
    }
    for assignment_id in &source_assignment_ids_after_attempt {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/monitoring/query-source-assignments/{assignment_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "clean up attempted query-source assignment"
        );
    }
    for policy_id in [
        update_allow_policy_id,
        create_deny_policy_id,
        secret_use_policy_id,
    ] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/policies/{policy_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "clean up temporary policy");
    }
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{source_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up deterministic source"
    );
    for secret_id in [original_secret_id, replacement_secret_id] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/automation-secrets/{secret_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::NO_CONTENT, "clean up named secret");
    }
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{restricted_user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clean up update-only user");

    assert_eq!(
        quick_start_status,
        StatusCode::FORBIDDEN,
        "Monitoring×Update on an existing source must not bypass quick-start's Monitoring×Create baseline",
    );
    assert_eq!(
        name_after_attempt,
        Some(original_name),
        "a Create-denied quick-start request must not mutate the existing source",
    );
    assert_eq!(
        binding_after_attempt,
        Some(original_secret_id),
        "a Create-denied quick-start request must not rebind the named credential",
    );
    assert!(
        source_assignment_ids_after_attempt.is_empty(),
        "a Create-denied quick-start request must not create a scoped query-source assignment",
    );
    assert!(
        preset_assignment_ids_after_attempt.is_empty(),
        "a Create-denied quick-start request must not apply a preset at the requested scope",
    );

    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn query_source_credential_binding_requires_secret_disclosure_authority()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let admin_cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let secret_id = create_named_secret(
        &app,
        &admin_cookie,
        &format!("itest-monitoring-secret-authority-{suffix}"),
    )
    .await?;
    let restricted_username = format!("itest-monitoring-secret-authority-{suffix}");
    let restricted_user_id = create_local_user(&app, &admin_cookie, &restricted_username).await?;
    let restricted_subject = format!("user:{restricted_user_id}");
    let all_resources = vec!["*".to_string()];
    let secret_resources = vec![format!("secret_variable:{secret_id}")];
    let create_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-secret-authority-create-{suffix}"),
            effect: "allow",
            actions: &["CREATE"],
            resource_types: &["MONITORING"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    let execute_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-secret-authority-execute-{suffix}"),
            effect: "allow",
            actions: &["EXECUTE"],
            resource_types: &["SECRET_VARIABLE"],
        },
        &restricted_subject,
        &secret_resources,
    )
    .await?;
    let restricted_cookie = login_and_get_cookie(&app, &restricted_username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("restricted user login failed")?;

    // Execute is purpose-bound to playbook environment export (C3), so even a
    // subject that can create query sources must not use it to bind a bearer
    // credential to an arbitrary query URL.
    let execute_only_session_suffix = format!("execute-only-session-{suffix}");
    let execute_only_session_slug =
        format!("itest-monitoring-source-{execute_only_session_suffix}");
    let (execute_only_session_status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&execute_only_session_suffix, Some(secret_id)).to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    let execute_only_session_source_id =
        query_source_id_by_slug(&db, &execute_only_session_slug).await?;
    let execute_only_session_binding_count =
        query_source_binding_count_for_secret(&db, secret_id).await?;
    if let Some(source_id) = execute_only_session_source_id {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/monitoring/query-sources/{source_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "clean up Execute-only session source after deliberate RED",
        );
    }
    // Mint the exact C3-shaped PAT that the old route accepted: both its
    // Monitoring create grant and its Execute×SecretVariable grant are in
    // scope. The token must still be rejected because it lacks Secret.
    let execute_only_token_payload = serde_json::json!({
        "name": format!("itest-monitoring-execute-only-pat-{suffix}"),
        "scopes": [
            {
                "action": "create",
                "resource_type": "monitoring",
            },
            {
                "action": "execute",
                "resource_type": "secret_variable",
            },
        ],
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&execute_only_token_payload.to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "mint Execute-only PAT");
    let execute_only_token: Value = serde_json::from_slice(&body)?;
    let execute_only_token_id: Uuid = execute_only_token["id"]
        .as_str()
        .ok_or("missing Execute-only PAT id")?
        .parse()?;
    let execute_only_token_value = execute_only_token["token_value"]
        .as_str()
        .ok_or("missing Execute-only PAT value")?
        .to_string();

    let execute_only_pat_suffix = format!("execute-only-pat-{suffix}");
    let execute_only_pat_slug = format!("itest-monitoring-source-{execute_only_pat_suffix}");
    let (execute_only_pat_status, _) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&execute_only_pat_suffix, Some(secret_id)).to_string()),
        None,
        Some(&execute_only_token_value),
    )
    .await?;
    let execute_only_pat_source_id = query_source_id_by_slug(&db, &execute_only_pat_slug).await?;
    let execute_only_pat_binding_count =
        query_source_binding_count_for_secret(&db, secret_id).await?;

    // If a pre-fix route leaks past the C3 boundary, remove its source and
    // usage binding before asserting so the shared ignored-test database stays
    // clean after a deliberate RED execution.
    if let Some(source_id) = execute_only_pat_source_id {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/monitoring/query-sources/{source_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "clean up Execute-only PAT source after deliberate RED",
        );
    }
    // A deliberately restored C3 gate must fail only after every temporary
    // REST resource is removed. This keeps the behavioral RED proof safe even
    // when this ignored test is pointed at a shared development database.
    let execute_only_behavior_regressed = execute_only_session_status != StatusCode::FORBIDDEN
        || execute_only_session_source_id.is_some()
        || execute_only_session_binding_count != 0
        || execute_only_pat_status != StatusCode::FORBIDDEN
        || execute_only_pat_source_id.is_some()
        || execute_only_pat_binding_count != 0;
    if execute_only_behavior_regressed {
        let (revoke_status, _) = request(
            &app,
            "POST",
            &format!("/api/v1/user/settings/api-tokens/{execute_only_token_id}/revoke"),
            None,
            Some(&restricted_cookie),
        )
        .await?;
        let mut policy_cleanup_statuses = Vec::new();
        for policy_id in [create_policy_id, execute_policy_id] {
            let (status, _) = request(
                &app,
                "DELETE",
                &format!("/api/v1/admin/access-control/policies/{policy_id}"),
                None,
                Some(&admin_cookie),
            )
            .await?;
            policy_cleanup_statuses.push(status);
        }
        let (secret_cleanup_status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/automation-secrets/{secret_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        let (user_cleanup_status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/users/{restricted_user_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;

        assert_eq!(revoke_status, StatusCode::OK, "revoke Execute-only PAT");
        assert!(
            policy_cleanup_statuses
                .iter()
                .all(|status| *status == StatusCode::OK),
            "clean up Execute-only temporary policies",
        );
        assert_eq!(
            secret_cleanup_status,
            StatusCode::NO_CONTENT,
            "clean up Execute-only named secret",
        );
        assert_eq!(
            user_cleanup_status,
            StatusCode::OK,
            "clean up Execute-only restricted user",
        );
    }
    assert_eq!(
        execute_only_session_status,
        StatusCode::FORBIDDEN,
        "Execute×SecretVariable must not authorize direct query-source credential binding",
    );
    assert!(
        execute_only_session_source_id.is_none(),
        "an Execute-only session must not create a query source",
    );
    assert_eq!(
        execute_only_session_binding_count, 0,
        "an Execute-only session must not write a secret usage binding",
    );
    assert_eq!(
        execute_only_pat_status,
        StatusCode::FORBIDDEN,
        "an Execute×SecretVariable PAT must not bind a named credential",
    );
    assert!(
        execute_only_pat_source_id.is_none(),
        "an Execute-only PAT must not create a query source",
    );
    assert_eq!(
        execute_only_pat_binding_count, 0,
        "an Execute-only PAT must not write a secret usage binding",
    );

    let secret_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-secret-authority-secret-{suffix}"),
            effect: "allow",
            actions: &["SECRET"],
            resource_types: &["SECRET_VARIABLE"],
        },
        &restricted_subject,
        &secret_resources,
    )
    .await?;

    let secret_session_suffix = format!("secret-session-{suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&secret_session_suffix, Some(secret_id)).to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "Secret×SecretVariable authorizes direct credential binding",
    );
    let secret_session_source_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing Secret-authorized session source id")?
        .parse()?;
    assert_eq!(
        query_source_binding_secret_id(&db, secret_session_source_id).await?,
        Some(secret_id),
        "Secret authority must persist the requested credential binding",
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{secret_session_source_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up Secret-authorized session source",
    );

    let secret_token_payload = serde_json::json!({
        "name": format!("itest-monitoring-secret-authority-pat-{suffix}"),
        "scopes": [
            {
                "action": "create",
                "resource_type": "monitoring",
            },
            {
                "action": "secret",
                "resource_type": "secret_variable",
            },
        ],
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&secret_token_payload.to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "mint Secret-authorized PAT");
    let secret_token: Value = serde_json::from_slice(&body)?;
    let secret_token_id: Uuid = secret_token["id"]
        .as_str()
        .ok_or("missing Secret-authorized PAT id")?
        .parse()?;
    let secret_token_value = secret_token["token_value"]
        .as_str()
        .ok_or("missing Secret-authorized PAT value")?
        .to_string();

    let secret_pat_suffix = format!("secret-pat-{suffix}");
    let secret_pat_slug = format!("itest-monitoring-source-{secret_pat_suffix}");
    let (secret_pat_status, _) = request_with_auth(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&secret_pat_suffix, Some(secret_id)).to_string()),
        None,
        Some(&secret_token_value),
    )
    .await?;
    let secret_pat_source_id = query_source_id_by_slug(&db, &secret_pat_slug)
        .await?
        .ok_or("Secret-authorized PAT did not create a query source")?;
    assert_eq!(
        secret_pat_status,
        StatusCode::CREATED,
        "a PAT needs Secret×SecretVariable to bind a named credential",
    );
    assert_eq!(
        query_source_binding_secret_id(&db, secret_pat_source_id).await?,
        Some(secret_id),
        "Secret-authorized PAT must persist the requested credential binding",
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{secret_pat_source_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up Secret-authorized PAT source",
    );

    // Direct updates are Update-only: the restricted user has Secret for the
    // credential and Monitoring×Update, but deliberately no Monitoring×Create.
    // This pins the action on the PUT writer independently of quick-start.
    let direct_update_source_suffix = format!("direct-update-source-{suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&direct_update_source_suffix, Some(secret_id)).to_string()),
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "admin creates direct-update source"
    );
    let direct_update_source_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing direct-update source id")?
        .parse()?;
    let update_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicyRule {
            name: format!("itest-monitoring-secret-authority-update-{suffix}"),
            effect: "allow",
            actions: &["UPDATE"],
            resource_types: &["MONITORING"],
        },
        &restricted_subject,
        &all_resources,
    )
    .await?;
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/policies/{create_policy_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "remove Monitoring×Create policy");

    let direct_update_request_suffix = format!("direct-update-request-{suffix}");
    let direct_update_name = format!("itest direct update {suffix}");
    let mut direct_update_payload =
        query_source_payload(&direct_update_request_suffix, Some(secret_id));
    direct_update_payload["name"] = Value::String(direct_update_name.clone());
    let (direct_update_status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/query-sources/{direct_update_source_id}"),
        Some(&direct_update_payload.to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    let direct_update_name_after = query_source_name(&db, direct_update_source_id).await?;
    let direct_update_binding_after =
        query_source_binding_secret_id(&db, direct_update_source_id).await?;
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/query-sources/{direct_update_source_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    let direct_update_source_cleanup_status = status;

    let (execute_only_token_revoke_status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/user/settings/api-tokens/{execute_only_token_id}/revoke"),
        None,
        Some(&restricted_cookie),
    )
    .await?;
    let (secret_token_revoke_status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/user/settings/api-tokens/{secret_token_id}/revoke"),
        None,
        Some(&restricted_cookie),
    )
    .await?;

    // Secret disclosure authority alone is still insufficient: query-source
    // creation remains governed by Monitoring×Create, which was removed above.
    let secret_without_monitoring_suffix = format!("secret-without-monitoring-{suffix}");
    let secret_without_monitoring_slug =
        format!("itest-monitoring-source-{secret_without_monitoring_suffix}");
    let (secret_without_monitoring_status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/query-sources",
        Some(&query_source_payload(&secret_without_monitoring_suffix, Some(secret_id)).to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    let secret_without_monitoring_source_id =
        query_source_id_by_slug(&db, &secret_without_monitoring_slug).await?;
    let secret_without_monitoring_binding_count =
        query_source_binding_count_for_secret(&db, secret_id).await?;
    let secret_without_monitoring_source_cleanup_status =
        if let Some(source_id) = secret_without_monitoring_source_id {
            let (status, _) = request(
                &app,
                "DELETE",
                &format!("/api/v1/monitoring/query-sources/{source_id}"),
                None,
                Some(&admin_cookie),
            )
            .await?;
            Some(status)
        } else {
            None
        };

    let mut policy_cleanup_statuses = Vec::new();
    for policy_id in [execute_policy_id, secret_policy_id, update_policy_id] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/admin/access-control/policies/{policy_id}"),
            None,
            Some(&admin_cookie),
        )
        .await?;
        policy_cleanup_statuses.push(status);
    }
    let (secret_cleanup_status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    let (user_cleanup_status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{restricted_user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;

    assert_eq!(
        direct_update_source_cleanup_status,
        StatusCode::NO_CONTENT,
        "clean up direct-update source",
    );
    assert_eq!(
        execute_only_token_revoke_status,
        StatusCode::OK,
        "revoke Execute-only PAT",
    );
    assert_eq!(
        secret_token_revoke_status,
        StatusCode::OK,
        "revoke Secret-authorized PAT",
    );
    assert!(
        policy_cleanup_statuses
            .iter()
            .all(|status| *status == StatusCode::OK),
        "clean up temporary policy",
    );
    assert_eq!(
        secret_cleanup_status,
        StatusCode::NO_CONTENT,
        "clean up named secret",
    );
    assert_eq!(
        user_cleanup_status,
        StatusCode::OK,
        "clean up restricted local user"
    );
    assert!(
        secret_without_monitoring_source_cleanup_status.is_none()
            || secret_without_monitoring_source_cleanup_status == Some(StatusCode::NO_CONTENT),
        "clean up source written without Monitoring authority",
    );
    assert_eq!(
        direct_update_status,
        StatusCode::OK,
        "Monitoring×Update without Create must authorize direct credential update",
    );
    assert_eq!(
        direct_update_name_after,
        Some(direct_update_name),
        "the Update-authorized direct request must change the source",
    );
    assert_eq!(
        direct_update_binding_after,
        Some(secret_id),
        "the Update-authorized direct request must preserve its requested credential binding",
    );
    assert_eq!(
        secret_without_monitoring_status,
        StatusCode::FORBIDDEN,
        "Secret×SecretVariable alone must not authorize Monitoring query-source creation",
    );
    assert!(
        secret_without_monitoring_source_id.is_none(),
        "Secret without Monitoring authority must not create a query source",
    );
    assert_eq!(
        secret_without_monitoring_binding_count, 0,
        "Secret without Monitoring authority must not write a secret usage binding",
    );

    Ok(())
}
