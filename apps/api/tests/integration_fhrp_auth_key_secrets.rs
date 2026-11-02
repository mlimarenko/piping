//! Native FHRP lifecycle, explicit reveal and offline conversion regressions.
//! These tests reset the explicitly isolated test database via `common`.

mod common;

use std::sync::Arc;
use axum::{body::Body, http::{Request, StatusCode}};
use tower::ServiceExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::ipam::fhrp_auth_key_conversion::convert_fhrp_auth_keys_offline;
use piping_backend::contexts::secrets::application::{ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService, SecretStore};
use piping_backend::contexts::secrets::application::consumer_type::{FHRP_AUTH_KEY_FIELD, INVENTORY_FHRP_GROUP_CONSUMER_TYPE};
use piping_backend::contexts::secrets::infrastructure::{DbSecretStore, SecretRepository, SqlxSecretRepository};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn policy_json(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    path: &str,
    payload: Value,
    expected: StatusCode,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let payload = payload.to_string();
    let (status, body) = common::request(app, method, path, Some(&payload), Some(cookie)).await?;
    // Mutation responses may contain credentials; never include bodies in failures.
    assert_eq!(status, expected, "{method} {path}");
    Ok(serde_json::from_slice(&body)?)
}

async fn group(pool: &PgPool) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_fhrpgroup (id, group_id, name, protocol, custom_field_data, description, comments) VALUES ($1, 42, $2, 'vrrp3', '{}'::jsonb, '', '')")
        .bind(id).bind(format!("fhrp-credential-{id}")).execute(pool).await?;
    Ok(id)
}

fn intent(id: Uuid, value: &str) -> ApplySecretFieldIntentInput {
    ApplySecretFieldIntentInput {
        consumer_type: INVENTORY_FHRP_GROUP_CONSUMER_TYPE.into(),
        consumer_id: id,
        owner_scope_type: INVENTORY_FHRP_GROUP_CONSUMER_TYPE.into(),
        owner_scope_id: id,
        field_key: FHRP_AUTH_KEY_FIELD.into(),
        intent: SecretFieldIntent::Set,
        value: Some(value.into()),
        secret_id: None,
    }
}

async fn plaintext(pool: &PgPool, id: Uuid) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT auth_key FROM ipam_fhrpgroup WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
}

#[tokio::test]
#[ignore = "Requires exclusively owned PostgreSQL/Redis; common resets the database"]
async fn stored_protocol_deny_blocks_group_and_generic_native_key_mutations() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let denied_id = group(&pool).await?;
    let permitted_id = group(&pool).await?;
    sqlx::query("UPDATE ipam_fhrpgroup SET protocol = 'vrrp2' WHERE id = $1")
        .bind(permitted_id)
        .execute(&pool)
        .await?;
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = SecretFieldIntentService::new(repository.clone());
    let original_key = " synthetic-stored-protocol-key ";
    let original_secret_id = service
        .apply_intent(intent(denied_id, original_key))
        .await?
        .secret_id
        .expect("native key binding");
    service
        .apply_intent(intent(permitted_id, "synthetic-permitted-initial"))
        .await?;
    let before_entity: Value =
        sqlx::query_scalar("SELECT to_jsonb(g) FROM ipam_fhrpgroup g WHERE id = $1")
            .bind(denied_id)
            .fetch_one(&pool)
            .await?;
    let before_ciphertext: String =
        sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
            .bind(original_secret_id)
            .fetch_one(&pool)
            .await?;
    let app = piping_backend::app::build_router(state);
    let admin = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("admin session");
    let username = format!("fhrp-protocol-{}", common::unique_suffix());
    let password = "synthetic-fhrp-protocol-password-42";
    let user = policy_json(
        &app,
        &admin,
        "POST",
        "/api/v1/admin/access-control/users",
        json!({
            "username": username, "email": format!("{username}@test.local"), "status": "active",
            "auth_source": "local", "credential": {"mode": "manual", "password": password}
        }),
        StatusCode::OK,
    )
    .await?;
    let subject = format!("user:{}", user["id"].as_str().expect("user id"));
    let policies = "/api/v1/admin/access-control/policies";
    policy_json(
        &app,
        &admin,
        "POST",
        policies,
        json!({
            "name": format!("{username}-allow"), "effect": "allow", "actions": ["READ", "UPDATE"],
            "resource_types": ["INVENTORY"], "subjects": [subject],
            "resources": ["inventory:*"], "enabled": true
        }),
        StatusCode::OK,
    )
    .await?;
    // This mixed-resource policy is accepted by the real policy write boundary:
    // ConnectionTarget contributes `protocol` to the union of attribute keys.
    policy_json(
        &app,
        &admin,
        "POST",
        policies,
        json!({
            "name": format!("{username}-deny-vrrp3"), "effect": "deny", "actions": ["UPDATE"],
            "resource_types": ["INVENTORY", "CONNECTION_TARGET"], "subjects": [subject],
            "resources": ["inventory:*"], "constraints": {"attributes": {"protocol": "vrrp3"}},
            "enabled": true
        }),
        StatusCode::OK,
    )
    .await?;
    let cookie = common::login_and_get_cookie(&app, &username, password)
        .await
        .expect("policy user session");
    let group_path = format!("/api/v1/inventory/fhrp-groups/{denied_id}");
    // Read permission remains independent of the constrained UPDATE deny.
    let (status, _) = common::request(&app, "GET", &group_path, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);
    for key_intent in ["clear", "rotate"] {
        let mut native_key = json!({"intent": key_intent});
        if key_intent == "rotate" {
            native_key["value"] = json!("synthetic-denied-rotation");
        }
        // Caller-supplied protocol must not replace the stored vrrp3 snapshot
        // used for the authorization decision.
        policy_json(
            &app,
            &cookie,
            "PUT",
            &group_path,
            json!({
                "group_id": 42, "name": "denied-entity-change", "description": "must not persist",
                "protocol": "vrrp2", "auth_key": native_key
            }),
            StatusCode::FORBIDDEN,
        )
        .await?;
        let mut generic_key = json!({
            "consumer_type": INVENTORY_FHRP_GROUP_CONSUMER_TYPE, "consumer_id": denied_id,
            "owner_scope_type": INVENTORY_FHRP_GROUP_CONSUMER_TYPE, "owner_scope_id": denied_id,
            "field_key": FHRP_AUTH_KEY_FIELD, "intent": key_intent
        });
        if key_intent == "rotate" {
            generic_key["value"] = json!("synthetic-denied-generic-rotation");
        }
        policy_json(
            &app,
            &cookie,
            "PUT",
            "/api/v1/secret-fields",
            generic_key,
            StatusCode::FORBIDDEN,
        )
        .await?;
        let after_entity: Value =
            sqlx::query_scalar("SELECT to_jsonb(g) FROM ipam_fhrpgroup g WHERE id = $1")
                .bind(denied_id)
                .fetch_one(&pool)
                .await?;
        assert!(after_entity == before_entity);
        let binding = repository
            .get_usage_binding(
                INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
                denied_id,
                FHRP_AUTH_KEY_FIELD,
            )
            .await?
            .expect("unchanged native binding");
        assert_eq!(binding.secret_id, original_secret_id);
        let after_ciphertext: String =
            sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
                .bind(original_secret_id)
                .fetch_one(&pool)
                .await?;
        assert!(after_ciphertext == before_ciphertext);
        assert!(
            DbSecretStore::new(pool.clone())
                .get_secret(original_secret_id)
                .await?
                == original_key
        );
    }
    // The same accepted deny must not block a stored vrrp2 group.
    policy_json(
        &app,
        &cookie,
        "PUT",
        &format!("/api/v1/inventory/fhrp-groups/{permitted_id}"),
        json!({
            "group_id": 42, "name": "permitted-entity-change", "protocol": "vrrp2",
            "auth_key": {"intent": "rotate", "value": "synthetic-permitted-group-rotation"}
        }),
        StatusCode::OK,
    )
    .await?;
    policy_json(&app, &cookie, "PUT", "/api/v1/secret-fields", json!({
        "consumer_type": INVENTORY_FHRP_GROUP_CONSUMER_TYPE, "consumer_id": permitted_id,
        "owner_scope_type": INVENTORY_FHRP_GROUP_CONSUMER_TYPE, "owner_scope_id": permitted_id,
        "field_key": FHRP_AUTH_KEY_FIELD, "intent": "rotate", "value": "synthetic-permitted-generic-rotation"
    }), StatusCode::OK).await?;
    let binding = repository
        .get_usage_binding(
            INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
            permitted_id,
            FHRP_AUTH_KEY_FIELD,
        )
        .await?
        .expect("permitted binding");
    assert!(
        DbSecretStore::new(pool.clone())
            .get_secret(binding.secret_id)
            .await?
            == "synthetic-permitted-generic-rotation"
    );
    let name: String = sqlx::query_scalar("SELECT name FROM ipam_fhrpgroup WHERE id = $1")
        .bind(permitted_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(name, "permitted-entity-change");
    Ok(())
}

#[tokio::test]
#[ignore = "Requires exclusively owned PostgreSQL/Redis; common resets the database"]
async fn native_fhrp_lifecycle_preserves_bytes_and_refuses_foreign_attachments() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = SecretFieldIntentService::new(repository.clone());
    let store = DbSecretStore::new(pool.clone());
    let id = group(pool).await?;
    let key = "  synthetic-κλειδί\t ";
    let created = service.apply_intent(intent(id, key)).await?;
    let secret_id = created.secret_id.expect("field secret");
    assert!(store.get_secret(secret_id).await? == key);
    let ciphertext: String =
        sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
            .bind(secret_id)
            .fetch_one(pool)
            .await?;
    assert!(ciphertext.starts_with("enc:v1:") && !ciphertext.contains(key));

    let mut invalid = intent(id, "synthetic");
    invalid.secret_id = Some(secret_id);
    assert!(service.apply_intent(invalid).await.is_err());
    let mut invalid = intent(id, "synthetic");
    invalid.owner_scope_id = Uuid::now_v7();
    assert!(service.apply_intent(invalid).await.is_err());
    let mut invalid = intent(id, "synthetic");
    invalid.field_key = "other".into();
    assert!(service.apply_intent(invalid).await.is_err());
    assert!(
        service
            .apply_intent(intent(Uuid::now_v7(), "synthetic"))
            .await
            .is_err()
    );
    let foreign = repository
        .create_named(&format!("fhrp-foreign-{}", Uuid::now_v7()), None, None)
        .await?;
    let mut invalid = intent(id, "synthetic");
    invalid.secret_id = Some(foreign.id);
    assert!(service.apply_intent(invalid).await.is_err());
    assert!(store.get_secret(secret_id).await? == key);
    let mut rotate = intent(id, " next-synthetic ");
    rotate.intent = SecretFieldIntent::Rotate;
    assert_eq!(
        service.apply_intent(rotate).await?.secret_id,
        Some(secret_id)
    );
    assert!(store.get_secret(secret_id).await? == " next-synthetic ");
    let mut clear = intent(id, "");
    clear.intent = SecretFieldIntent::Clear;
    clear.value = None;
    service.apply_intent(clear).await?;
    assert!(repository.get_variable_by_id(secret_id).await?.is_none());
    assert!(store.get_secret(secret_id).await.is_err());
    assert!(repository.get_variable_by_id(foreign.id).await?.is_some());
    Ok(())
}

#[tokio::test]
#[ignore = "Requires exclusively owned PostgreSQL/Redis; constructs a pre-0004 schema and common resets the database"]
async fn offline_conversion_is_atomic_fail_closed_and_exact_before_plaintext_drop() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    // Deliberately reconstruct only the legacy column for the offline test.
    sqlx::query("ALTER TABLE ipam_fhrpgroup ADD COLUMN auth_key varchar(255) NOT NULL DEFAULT ''")
        .execute(pool)
        .await?;
    let first = group(pool).await?;
    let last = group(pool).await?;
    let key = " synthetic-conversion-κλειδί\t ";
    sqlx::query("UPDATE ipam_fhrpgroup SET auth_key = $2 WHERE id = $1")
        .bind(first)
        .bind(key)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE ipam_fhrpgroup SET auth_key = '   ' WHERE id = $1")
        .bind(last)
        .execute(pool)
        .await?;
    assert!(convert_fhrp_auth_keys_offline(pool).await.is_err());
    assert!(plaintext(pool, first).await? == key);
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    assert!(
        repository
            .get_usage_binding(
                INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
                first,
                FHRP_AUTH_KEY_FIELD
            )
            .await?
            .is_none()
    );
    // Migration must refuse even whitespace-only remaining plaintext.
    let mut tx = pool.begin().await?;
    assert!(
        sqlx::raw_sql(include_str!("../migrations/0004_fhrp_auth_key_secret.sql"))
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await?;
    assert!(plaintext(pool, first).await? == key);
    sqlx::query("UPDATE ipam_fhrpgroup SET auth_key = 'second-synthetic' WHERE id = $1")
        .bind(last)
        .execute(pool)
        .await?;
    let service = SecretFieldIntentService::new(repository.clone());
    service
        .apply_intent(intent(last, "already-managed"))
        .await?;
    assert!(convert_fhrp_auth_keys_offline(pool).await.is_err());
    assert!(plaintext(pool, first).await? == key);
    let mut clear = intent(last, "");
    clear.intent = SecretFieldIntent::Clear;
    clear.value = None;
    service.apply_intent(clear).await?;

    // A late persistence error must roll back all earlier lifecycle writes.
    sqlx::raw_sql("CREATE FUNCTION test_refuse_fhrp_secret_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF (SELECT count(*) FROM secret_usage_binding WHERE consumer_type = 'inventory_fhrp_group') >= 2 THEN RAISE EXCEPTION 'synthetic persistence refusal'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_refuse_fhrp_secret_write BEFORE INSERT ON secrets_store FOR EACH ROW EXECUTE FUNCTION test_refuse_fhrp_secret_write();").execute(pool).await?;
    assert!(convert_fhrp_auth_keys_offline(pool).await.is_err());
    assert!(plaintext(pool, first).await? == key);
    assert!(
        repository
            .get_usage_binding(
                INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
                first,
                FHRP_AUTH_KEY_FIELD
            )
            .await?
            .is_none()
    );
    sqlx::raw_sql("DROP TRIGGER test_refuse_fhrp_secret_write ON secrets_store; DROP FUNCTION test_refuse_fhrp_secret_write();").execute(pool).await?;
    assert_eq!(convert_fhrp_auth_keys_offline(pool).await?, 2);
    assert!(plaintext(pool, first).await?.is_empty());
    let binding = repository
        .get_usage_binding(
            INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
            first,
            FHRP_AUTH_KEY_FIELD,
        )
        .await?
        .expect("converted binding");
    assert_eq!(binding.owner_scope_type, INVENTORY_FHRP_GROUP_CONSUMER_TYPE);
    assert_eq!(binding.owner_scope_id, first);
    assert!(
        DbSecretStore::new(pool.clone())
            .get_secret(binding.secret_id)
            .await?
            == key
    );
    assert_eq!(convert_fhrp_auth_keys_offline(pool).await?, 0);
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(include_str!("../migrations/0004_fhrp_auth_key_secret.sql"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'ipam_fhrpgroup' AND column_name = 'auth_key')").fetch_one(pool).await?;
    assert!(!exists);
    assert!(
        DbSecretStore::new(pool.clone())
            .get_secret(binding.secret_id)
            .await?
            == key
    );
    Ok(())
}

#[tokio::test]
#[ignore = "Requires exclusively owned PostgreSQL/Redis; common resets the database"]
async fn explicit_native_reveal_requires_both_object_scopes_and_never_leaks_on_get() -> TestResult {
    let state = common::build_test_app_state().await?;
    let id = group(&state.db_pool).await?;
    let key = " synthetic-native-reveal ";
    let repository = Arc::new(SqlxSecretRepository::new(state.db_pool.clone()));
    SecretFieldIntentService::new(repository)
        .apply_intent(intent(id, key))
        .await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("admin session");
    let path = format!("/api/v1/inventory/fhrp-groups/{id}");
    let reveal = format!("{path}/auth-key/reveal");
    let read = json!({"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{id}")});
    let secret = json!({"action":"secret","resource_type":"inventory","resource_pattern":format!("inventory:{id}")});
    for (scopes, expected) in [
        (json!([read.clone()]), StatusCode::FORBIDDEN),
        (json!([secret.clone()]), StatusCode::FORBIDDEN),
        (
            json!([read.clone(), {"action":"secret","resource_type":"inventory","resource_pattern":format!("inventory:{}", Uuid::now_v7())}]),
            StatusCode::FORBIDDEN,
        ),
        (json!([read, secret]), StatusCode::OK),
    ] {
        let can_read = scopes
            .as_array()
            .expect("scope array")
            .iter()
            .any(|scope| scope["action"] == "read");
        let payload =
            json!({"name":format!("fhrp-native-{}", Uuid::now_v7()), "scopes":scopes}).to_string();
        let (status, body) = common::request(
            &app,
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(&payload),
            Some(&cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED);
        let token: Value = serde_json::from_slice(&body)?;
        let bearer = token["token_value"].as_str().expect("token value");
        let (status, body) = common::request_with_auth(&app, "GET", &format!("/api/v1/secret-fields?consumer_type={INVENTORY_FHRP_GROUP_CONSUMER_TYPE}&consumer_id={id}"), None, None, Some(bearer)).await?;
        assert_eq!(
            status,
            if can_read {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert!(!String::from_utf8_lossy(&body).contains(key));
        let (status, body) =
            common::request_with_auth(&app, "POST", &reveal, None, None, Some(bearer)).await?;
        assert_eq!(status, expected);
        if expected == StatusCode::OK {
            let response: Value = serde_json::from_slice(&body)?;
            assert!(response["value"].as_str() == Some(key));
        } else {
            assert!(!String::from_utf8_lossy(&body).contains(key));
        }
    }
    for path in [path.as_str(), "/api/v1/inventory/fhrp-groups"] {
        let (status, body) = common::request(&app, "GET", path, None, Some(&cookie)).await?;
        assert_eq!(status, StatusCode::OK);
        assert!(!String::from_utf8_lossy(&body).contains(key));
        assert!(!String::from_utf8_lossy(&body).contains("auth_key"));
    }
    let payload = json!({"name":format!("fhrp-update-only-{}", Uuid::now_v7()), "scopes":[{"action":"update","resource_type":"inventory","resource_pattern":format!("inventory:{id}")}]}).to_string();
    let (status, body) = common::request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let token: Value = serde_json::from_slice(&body)?;
    let bearer = token["token_value"].as_str().expect("token value");
    let payload = json!({"consumer_type":INVENTORY_FHRP_GROUP_CONSUMER_TYPE,"consumer_id":id,"owner_scope_type":INVENTORY_FHRP_GROUP_CONSUMER_TYPE,"owner_scope_id":id,"field_key":FHRP_AUTH_KEY_FIELD,"intent":"rotate","value":key}).to_string();
    let (status, body) = common::request_with_auth(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        None,
        Some(bearer),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&body).contains(key));
    // Generic Secrets rights alone must not confer access to a native group.
    let payload = json!({"name":format!("fhrp-secrets-only-{}", Uuid::now_v7()), "scopes":[{"action":"update","resource_type":"secret_variable","resource_pattern":"*"}]}).to_string();
    let (status, body) = common::request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&payload),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    let token: Value = serde_json::from_slice(&body)?;
    let bearer = token["token_value"].as_str().expect("token value");
    let payload = json!({"consumer_type":INVENTORY_FHRP_GROUP_CONSUMER_TYPE,"consumer_id":id,"owner_scope_type":INVENTORY_FHRP_GROUP_CONSUMER_TYPE,"owner_scope_id":id,"field_key":FHRP_AUTH_KEY_FIELD,"intent":"set","value":"synthetic-denied"}).to_string();
    let (status, _) = common::request_with_auth(
        &app,
        "PUT",
        "/api/v1/secret-fields",
        Some(&payload),
        None,
        Some(bearer),
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = common::request_with_auth(&app, "GET", &format!("/api/v1/secret-fields?consumer_type={INVENTORY_FHRP_GROUP_CONSUMER_TYPE}&consumer_id={id}"), None, None, Some(bearer)).await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&reveal)
                .header("cookie", cookie.as_str())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .expect("no-store header"),
        "no-store"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let response: Value = serde_json::from_slice(&body)?;
    assert!(response["value"].as_str() == Some(key));
    let metadata: Vec<Value> = sqlx::query_scalar("SELECT metadata FROM audit_event WHERE event_type = 'inventory.fhrp_group.auth_key.reveal' AND target_id = $1").bind(id).fetch_all(&pool).await?;
    assert!(!metadata.is_empty());
    for metadata in metadata {
        assert!(!metadata.to_string().contains(key));
        assert_eq!(metadata["field_key"], FHRP_AUTH_KEY_FIELD);
    }

    // A grantable PAT action is still bounded by the issuer's effective policy.
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
        .bind(common::TEST_ADMIN_USERNAME)
        .fetch_one(&pool)
        .await?;
    let policies = "/api/v1/admin/access-control/policies";
    let deny = policy_json(
        &app,
        &cookie,
        "POST",
        policies,
        json!({
            "name": format!("fhrp-deny-secret-issuer-{}", Uuid::now_v7()),
            "effect": "deny", "actions": ["SECRET"], "resource_types": ["INVENTORY"],
            "subjects": [format!("user:{user_id}")],
            "resources": [format!("inventory:{id}")], "enabled": true
        }),
        StatusCode::OK,
    )
    .await?;
    let requested = json!({
        "name": format!("fhrp-forbidden-issuer-{}", Uuid::now_v7()),
        "scopes": [{"action":"secret", "resource_type":"inventory", "resource_pattern":format!("inventory:{id}")}]
    })
    .to_string();
    let (secret_status, _) = common::request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&requested),
        Some(&cookie),
    )
    .await?;
    let requested = json!({
        "name": format!("fhrp-read-issuer-{}", Uuid::now_v7()),
        "scopes": [{"action":"read", "resource_type":"inventory", "resource_pattern":format!("inventory:{id}")}]
    })
    .to_string();
    let (read_status, _) = common::request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(&requested),
        Some(&cookie),
    )
    .await?;
    let deny_id = deny["id"].as_str().ok_or("deny policy id")?;
    let (cleanup_status, _) = common::request(
        &app,
        "DELETE",
        &format!("{policies}/{deny_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(cleanup_status, StatusCode::OK);
    assert_eq!(secret_status, StatusCode::FORBIDDEN);
    assert_eq!(read_status, StatusCode::CREATED);
    Ok(())
}
