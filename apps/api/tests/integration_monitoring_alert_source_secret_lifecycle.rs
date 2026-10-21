//! Integration coverage for monitoring alert-source named-secret references.
//!
//! Requires PostgreSQL and Redis (e.g. `make -C apps/api run-up`). Run with:
//! `cargo test --test integration_monitoring_alert_source_secret_lifecycle -- --ignored --test-threads=1`
//! Every test here creates access-control state, so it additionally requires
//! `PIPING_TEST_DISPOSABLE_DB=1` and a disposable `pwa*` database name. The CI
//! coverage lane uses `pwa10_coverage` while local verification uses
//! `pwa11_coverage`, so the guard accepts any `pwa` prefix rather than pinning a
//! single task id.

mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, TEST_SECRET_ID, build_test_app_state,
    login_and_get_cookie, request, request_with_auth, unique_suffix,
};
use piping_backend::contexts::monitoring::application::source_resolution::resolve_metric_query_source;
use piping_backend::contexts::monitoring::infrastructure::persistence::{
    HostScopeIds, fetch_alert_source_assignments_for_hosts,
};
use piping_backend::contexts::secrets::infrastructure::persistence::DbSecretStore;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

const ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE: &str = "monitoring_alert_source";
const ALERT_SOURCE_CREDENTIAL_FIELD_KEY: &str = "bearer_token";
/// Endpoint-only URL the alert-source API accepts. `alertmanager` appends its
/// own provider path; the durable value never carries a credential channel.
const ALERT_SOURCE_URL: &str = "https://alerts.example.test/api/v1/alerts";

/// Alert-source regressions that create access-control state must run only
/// against a declared disposable database. The explicit flag makes that
/// ownership part of the command; the name prefix prevents accidental use of
/// the regular development database.
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
        "PWA-11 alert-source regression requires PIPING_TEST_DISPOSABLE_DB=1 and a disposable pwa* database"
            .into(),
    )
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
/// only ever produces unprotected named secrets. The credential gate must
/// refuse it, and the repository must not write a binding.
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

fn alert_source_payload(suffix: &str, credential_ref: Option<Uuid>) -> Value {
    serde_json::json!({
        "name": format!("itest monitoring alert source {suffix}"),
        "source_type": "alertmanager",
        "query_url": ALERT_SOURCE_URL,
        "credential_ref": credential_ref,
        "timeout_ms": 3000,
        "options": {},
        "enabled": true,
    })
}

async fn alert_source_binding_secret_id(
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
    .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(source_id)
    .bind(ALERT_SOURCE_CREDENTIAL_FIELD_KEY)
    .fetch_optional(db)
    .await?;

    row.map(|row| row.try_get("secret_id"))
        .transpose()
        .map_err(Into::into)
}

async fn alert_source_binding_count_for_secret(
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
    .bind(ALERT_SOURCE_CREDENTIAL_CONSUMER_TYPE)
    .bind(ALERT_SOURCE_CREDENTIAL_FIELD_KEY)
    .bind(secret_id)
    .fetch_one(db)
    .await?)
}

async fn alert_source_exists(
    db: &sqlx::PgPool,
    name: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM monitoring_alert_source WHERE name = $1)",
    )
    .bind(name)
    .fetch_one(db)
    .await?)
}

async fn alert_source_id_by_query_url(
    db: &sqlx::PgPool,
    query_url: &str,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM monitoring_alert_source WHERE query_url = $1")
            .bind(query_url)
            .fetch_optional(db)
            .await?,
    )
}

async fn alert_source_id_by_name(
    db: &sqlx::PgPool,
    name: &str,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM monitoring_alert_source WHERE name = $1")
            .bind(name)
            .fetch_optional(db)
            .await?,
    )
}

async fn alert_source_has_raw_credential_column(
    db: &sqlx::PgPool,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'monitoring_alert_source'
              AND column_name = 'credential_ref'
        )
        "#,
    )
    .fetch_one(db)
    .await?)
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

/// One access-control policy granting `subject` `actions` on `resources`.
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
    policy: PolicySpec<'_>,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let PolicySpec {
        name,
        effect,
        actions,
        resource_types,
        subject,
        resources,
    } = policy;
    let payload = serde_json::json!({
        "name": name,
        "description": null,
        "effect": effect,
        "actions": actions,
        "resource_types": resource_types,
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
async fn alert_source_credentials_use_named_secret_bindings_and_reconcile_lifecycle()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_disposable_pwa_database()?;

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
    let invalid_name = format!("itest monitoring alert source invalid-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(
            &alert_source_payload(&format!("invalid-{suffix}"), Some(nonexistent_secret_id))
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
        !alert_source_exists(&db, &invalid_name).await?,
        "an invalid credential must not leave an alert source row behind",
    );

    // The pre-seeded field secret is a real Piping secret but not an
    // admin-managed *named* secret, so UUID shape/existence alone is still
    // insufficient.
    let field_secret_id = Uuid::parse_str(TEST_SECRET_ID)?;
    let field_secret_suffix = format!("field-kind-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&alert_source_payload(&field_secret_suffix, Some(field_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a non-named secret must not be accepted as an alert-source credential",
    );
    assert!(
        !alert_source_exists(
            &db,
            &format!("itest monitoring alert source {field_secret_suffix}"),
        )
        .await?,
        "a non-named credential must not leave an alert source row behind",
    );

    // A protected named secret is refused by the credential gate (403) and
    // must not produce a binding.
    let protected_secret_id =
        seed_protected_named_secret(&db, &format!("itest-alert-protected-{suffix}")).await?;
    let protected_suffix = format!("protected-{suffix}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&alert_source_payload(&protected_suffix, Some(protected_secret_id)).to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a protected named secret must not be bindable to an alert source",
    );
    assert!(
        !alert_source_exists(
            &db,
            &format!("itest monitoring alert source {protected_suffix}"),
        )
        .await?,
        "a protected credential must not leave an alert source row behind",
    );
    assert_eq!(
        alert_source_binding_count_for_secret(&db, protected_secret_id).await?,
        0,
        "a refused protected credential must not write a usage binding",
    );

    assert!(
        !alert_source_has_raw_credential_column(&db).await?,
        "the raw alert-source credential column must be gone after migration 0003",
    );

    // Create with a real named secret: exactly one canonical binding row, the
    // public contract returns the secret UUID, and the durable URL is redacted
    // (the create validator cannot persist a query/fragment/userinfo channel).
    let first_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-alert-credential-one-{suffix}"),
    )
    .await?;
    let source_payload = alert_source_payload(&suffix, Some(first_secret_id));
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&source_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create alert source");
    let created: Value = serde_json::from_slice(&body)?;
    let source_id: Uuid = created["id"]
        .as_str()
        .ok_or("missing alert source id")?
        .parse()?;
    assert_eq!(
        created["credential_ref"].as_str(),
        Some(first_secret_id.to_string().as_str()),
        "the public alert-source contract must return the typed named-secret UUID",
    );
    let returned_url = created["query_url"]
        .as_str()
        .ok_or("missing alert source query_url")?;
    assert_eq!(
        returned_url, ALERT_SOURCE_URL,
        "the API must return the endpoint-only URL",
    );
    assert!(
        !returned_url.contains('?') && !returned_url.contains('#') && !returned_url.contains('@'),
        "the API must return a redacted endpoint URL with no query, fragment, or userinfo channel",
    );
    assert_eq!(
        alert_source_binding_secret_id(&db, source_id).await?,
        Some(first_secret_id),
        "source creation must project the named-secret reference into the canonical usage binding",
    );
    assert_eq!(
        alert_source_binding_count_for_secret(&db, first_secret_id).await?,
        1,
        "source creation must write exactly one usage binding row",
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
        "a named secret referenced by an alert source must be deletion-protected",
    );

    // Update reconciles the binding: the old row is gone and the new one is the
    // only row for this source.
    let second_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-alert-credential-two-{suffix}"),
    )
    .await?;
    let replacement_payload = alert_source_payload(&suffix, Some(second_secret_id));
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/alert-sources/{source_id}"),
        Some(&replacement_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "replace alert source credential");
    assert_eq!(
        alert_source_binding_secret_id(&db, source_id).await?,
        Some(second_secret_id),
        "source update must replace rather than accumulate credential bindings",
    );
    assert_eq!(
        alert_source_binding_count_for_secret(&db, first_secret_id).await?,
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

    // Clearing the credential removes the binding in the same update.
    let removal_payload = alert_source_payload(&suffix, None);
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/alert-sources/{source_id}"),
        Some(&removal_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clear alert source credential");
    assert_eq!(
        alert_source_binding_secret_id(&db, source_id).await?,
        None,
        "clearing credential_ref must remove the usage binding in the same update",
    );

    // Only canonical UUID text is accepted; migration-only spellings and
    // unrecognized field aliases are rejected without writing a source.
    for (alias_label, alias_ref) in [
        ("short", format!("secret:{second_secret_id}")),
        ("uri", format!("secret://{second_secret_id}")),
        ("compact", second_secret_id.simple().to_string()),
        ("braced", format!("{{{second_secret_id}}}")),
        ("urn", format!("urn:uuid:{second_secret_id}")),
    ] {
        let alias_suffix = format!("alias-{alias_label}-{suffix}");
        let mut alias_payload = alert_source_payload(&alias_suffix, None);
        alias_payload["credential_ref"] = Value::String(alias_ref);
        let (status, _) = request(
            &app,
            "POST",
            "/api/v1/monitoring/alert-sources",
            Some(&alias_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "non-canonical {alias_label} credential text must be rejected by the UUID contract",
        );
        assert!(
            !alert_source_exists(
                &db,
                &format!("itest monitoring alert source {alias_suffix}"),
            )
            .await?,
            "rejected non-canonical credential text must not leave a source row behind",
        );
    }
    let unknown_alias_suffix = format!("unknown-alias-{suffix}");
    let unknown_alias_payload = serde_json::json!({
        "name": format!("itest monitoring alert source {unknown_alias_suffix}"),
        "source_type": "alertmanager",
        "query_url": ALERT_SOURCE_URL,
        "credential_secret_id": second_secret_id,
        "timeout_ms": 3000,
        "options": {},
        "enabled": true,
    });
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
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
        !alert_source_exists(
            &db,
            &format!("itest monitoring alert source {unknown_alias_suffix}"),
        )
        .await?,
        "an unrecognized credential alias must not create an unbound source",
    );

    // Inline credential channels in the durable URL are rejected on both
    // writers before persistence.
    let baseline_payload = alert_source_payload(&format!("url-baseline-{suffix}"), None);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&baseline_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create URL-validation target");
    let url_target_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing URL-validation target id")?
        .parse()?;
    for (channel, query_url) in [
        (
            "userinfo",
            format!("https://operator:placeholder@alerts-{suffix}.example.test/api/v1/alerts"),
        ),
        (
            "query component",
            format!("https://alerts-{suffix}.example.test/api/v1/alerts?scope=platform"),
        ),
        (
            "fragment",
            format!("https://alerts-{suffix}.example.test/api/v1/alerts#diagnostic"),
        ),
    ] {
        let mut create_payload = alert_source_payload(&format!("url-{channel}-{suffix}"), None);
        create_payload["query_url"] = Value::String(query_url.clone());
        let (create_status, _) = request(
            &app,
            "POST",
            "/api/v1/monitoring/alert-sources",
            Some(&create_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        let create_source_id = alert_source_id_by_query_url(&db, &query_url).await?;
        if let Some(source_id) = create_source_id {
            // A deliberately weakened validator can persist the bad URL during
            // a RED proof; remove it before asserting so no credential-like URL
            // survives the run.
            let (cleanup_status, _) = request(
                &app,
                "DELETE",
                &format!("/api/v1/monitoring/alert-sources/{source_id}"),
                None,
                Some(&cookie),
            )
            .await?;
            assert_eq!(
                cleanup_status,
                StatusCode::NO_CONTENT,
                "clean up a RED-proof source for URL {channel}",
            );
        }

        let mut update_payload = baseline_payload.clone();
        update_payload["query_url"] = Value::String(query_url.clone());
        let (update_status, _) = request(
            &app,
            "PUT",
            &format!("/api/v1/monitoring/alert-sources/{url_target_id}"),
            Some(&update_payload.to_string()),
            Some(&cookie),
        )
        .await?;
        // A deliberately weakened validator can persist the bad URL during a
        // RED proof. Restore the known-good target before asserting so this
        // ignored integration test leaves no credential-like URL behind.
        let (restore_status, _) = request(
            &app,
            "PUT",
            &format!("/api/v1/monitoring/alert-sources/{url_target_id}"),
            Some(&baseline_payload.to_string()),
            Some(&cookie),
        )
        .await?;

        assert_eq!(
            create_status,
            StatusCode::BAD_REQUEST,
            "alert-source create must reject URL {channel} before persistence",
        );
        assert!(
            create_source_id.is_none(),
            "alert-source create must not persist URL {channel}",
        );
        assert_eq!(
            update_status,
            StatusCode::BAD_REQUEST,
            "alert-source update must reject URL {channel} before persistence",
        );
        assert_eq!(
            restore_status,
            StatusCode::OK,
            "restore URL-validation target after {channel} attempt",
        );
    }
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-sources/{url_target_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up URL-validation target"
    );

    // Deleting the alert source sweeps its binding and unblocks the secret.
    let third_secret_id = create_named_secret(
        &app,
        &cookie,
        &format!("itest-alert-credential-three-{suffix}"),
    )
    .await?;
    let delete_suffix = format!("delete-{suffix}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&alert_source_payload(&delete_suffix, Some(third_secret_id)).to_string()),
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
        &format!("/api/v1/monitoring/alert-sources/{source_to_delete}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete alert source");
    assert_eq!(
        alert_source_binding_secret_id(&db, source_to_delete).await?,
        None,
        "deleting a source must sweep its credential binding transactionally",
    );
    assert_eq!(
        alert_source_binding_count_for_secret(&db, third_secret_id).await?,
        0,
        "a deleted source must own no usage binding rows",
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
        "deleted source must unblock its credential",
    );

    // Tear down the cleared source and its unbound secret.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-sources/{source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "clean up cleared source");
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
        "clean up the cleared credential"
    );

    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn alert_source_credential_binding_requires_secret_disclosure_authority()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_disposable_pwa_database()?;

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
        &format!("itest-alert-authority-{suffix}"),
    )
    .await?;
    let restricted_username = format!("itest-alert-authority-{suffix}");
    let restricted_user_id = create_local_user(&app, &admin_cookie, &restricted_username).await?;
    let restricted_subject = format!("user:{restricted_user_id}");
    let all_resources = vec!["*".to_string()];
    let secret_resources = vec![format!("secret_variable:{secret_id}")];
    let create_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicySpec {
            name: format!("itest-alert-authority-create-{suffix}"),
            effect: "allow",
            actions: &["CREATE"],
            resource_types: &["MONITORING"],
            subject: &restricted_subject,
            resources: &all_resources,
        },
    )
    .await?;
    // Execute is purpose-bound to playbook environment export (C3). A subject
    // that can create alert sources must not reuse it to bind a bearer
    // credential to an arbitrary alert URL.
    let execute_policy_id = create_policy_for_subject(
        &app,
        &admin_cookie,
        PolicySpec {
            name: format!("itest-alert-authority-execute-{suffix}"),
            effect: "allow",
            actions: &["EXECUTE"],
            resource_types: &["SECRET_VARIABLE"],
            subject: &restricted_subject,
            resources: &secret_resources,
        },
    )
    .await?;
    let restricted_cookie = login_and_get_cookie(&app, &restricted_username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("restricted user login failed")?;

    let denied_suffix = format!("execute-only-{suffix}");
    let (denied_status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&alert_source_payload(&denied_suffix, Some(secret_id)).to_string()),
        Some(&restricted_cookie),
    )
    .await?;
    let denied_source_exists = alert_source_exists(
        &db,
        &format!("itest monitoring alert source {denied_suffix}"),
    )
    .await?;
    let denied_binding_count = alert_source_binding_count_for_secret(&db, secret_id).await?;
    if denied_source_exists {
        // A pre-fix route that let C3 bind the credential would leave a source
        // and binding; remove them before asserting so the shared ignored-test
        // database stays clean after a deliberate RED execution.
        let denied_name = format!("itest monitoring alert source {denied_suffix}");
        if let Some(source_id) = alert_source_id_by_name(&db, &denied_name).await? {
            let (cleanup_status, _) = request(
                &app,
                "DELETE",
                &format!("/api/v1/monitoring/alert-sources/{source_id}"),
                None,
                Some(&admin_cookie),
            )
            .await?;
            assert_eq!(
                cleanup_status,
                StatusCode::NO_CONTENT,
                "clean up Execute-only alert source after deliberate RED",
            );
        }
    }

    for policy_id in [create_policy_id, execute_policy_id] {
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
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/admin/access-control/users/{restricted_user_id}"),
        None,
        Some(&admin_cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "clean up restricted local user");
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
        "clean up the unbound named secret",
    );

    // Assert after cleanup: a RED execution against the pre-fix route must not
    // leave bindings or access-control rows in the shared database.
    assert_eq!(
        denied_status,
        StatusCode::FORBIDDEN,
        "Execute×SecretVariable must not authorize direct alert-source credential binding",
    );
    assert!(
        !denied_source_exists,
        "an Execute-only caller must not create an alert source",
    );
    assert_eq!(
        denied_binding_count, 0,
        "an Execute-only caller must not write a secret usage binding",
    );

    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with the command in this file's header.
async fn zabbix_auth_param_alert_source_resolves_its_bound_secret()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    require_disposable_pwa_database()?;

    let state = build_test_app_state().await?;
    let settings = state.settings.clone();
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("test admin login failed")?;
    let suffix = unique_suffix();

    let secret_id =
        create_named_secret(&app, &cookie, &format!("itest-zabbix-auth-param-{suffix}")).await?;
    // Operators load the real Zabbix API token from a token file; a test
    // sentinel makes an accidental disclosure detectable without embedding a
    // live credential.
    let sentinel = format!("zabbix-auth-param-token-{suffix}");
    let (status, _) = request_with_auth(
        &app,
        "PUT",
        &format!("/api/v1/secrets/{secret_id}/value"),
        Some(&serde_json::json!({ "value": sentinel }).to_string()),
        Some(&cookie),
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "set named secret value");

    let zabbix_url = format!("https://zabbix-alert-{suffix}.example.test/api_jsonrpc.php");
    let source_payload = serde_json::json!({
        "name": format!("itest zabbix alert source {suffix}"),
        "source_type": "zabbix",
        "query_url": zabbix_url,
        "credential_ref": secret_id,
        "timeout_ms": 3000,
        "options": { "auth_mode": "auth_param" },
        "enabled": true,
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-sources",
        Some(&source_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "create zabbix alert source");
    let source_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing zabbix alert source id")?
        .parse()?;

    let assignment_payload = serde_json::json!({
        "alert_source_id": source_id,
        "scope_type": "global",
    });
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/alert-source-assignments",
        Some(&assignment_payload.to_string()),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "assign zabbix alert source");
    let assignment_id: Uuid = serde_json::from_slice::<Value>(&body)?["id"]
        .as_str()
        .ok_or("missing zabbix alert source assignment id")?
        .parse()?;

    // The persistence layer must read the credential identity from the
    // canonical binding, not from a source-owned column. This is the assertion
    // that fails RED on the pre-migration code, where the alert-source join
    // still yields `credential_secret_id = None`.
    let empty_scope: Vec<Uuid> = Vec::new();
    let scopes = HostScopeIds {
        host_ids: &empty_scope,
        site_ids: &empty_scope,
        tenant_ids: &empty_scope,
        role_ids: &empty_scope,
    };
    let assignments = fetch_alert_source_assignments_for_hosts(&db, scopes).await?;
    let assignment = assignments
        .into_iter()
        .find(|assignment| assignment.id == assignment_id)
        .ok_or("the zabbix alert source assignment was not resolved")?;
    assert_eq!(
        assignment.source.credential_secret_id,
        Some(secret_id),
        "the alert-source read model must resolve the bound named-secret identity",
    );
    assert_eq!(
        assignment.source.options["auth_mode"], "auth_param",
        "the zabbix source must retain its auth_param transport option",
    );

    // Prove the bound secret resolves to its stored value through the same
    // resolver the alert poller uses. The query URL is swapped to a loopback
    // endpoint only to avoid external DNS during the SSRF pin; the credential
    // identity itself comes from the persisted binding.
    let mut resolvable_source = assignment.source.clone();
    resolvable_source.query_url = Some("http://127.0.0.1:1/api_jsonrpc.php".to_string());
    let secret_store = DbSecretStore::new(db.clone());
    let resolved = resolve_metric_query_source(
        &settings.monitoring,
        &secret_store,
        std::slice::from_ref(&resolvable_source),
        "zabbix",
    )
    .await?
    .ok_or("the zabbix alert source must resolve")?;
    assert_eq!(
        resolved.token.as_deref(),
        Some(sentinel.as_str()),
        "the resolver must load the bound secret value for the zabbix auth_param source",
    );
    assert_eq!(
        resolved.options["auth_mode"], "auth_param",
        "the resolved zabbix source must keep its auth_param transport option",
    );
    // Coverage gap, stated rather than faked: the provider's JSON-RPC `auth`
    // field placement is not reachable from this HTTP/DB layer. It is asserted
    // in the providers unit test
    // `zabbix_auth_param_puts_trimmed_token_only_in_json_rpc_envelope`
    // (src/contexts/monitoring/infrastructure/providers.rs), which is the
    // narrowest surface where the transport can be observed directly.

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-source-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "clean up zabbix assignment");
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/alert-sources/{source_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up zabbix alert source"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/automation-secrets/{secret_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "clean up zabbix named secret"
    );

    Ok(())
}
