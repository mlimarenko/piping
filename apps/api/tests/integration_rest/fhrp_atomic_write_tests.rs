use super::*;
use serde_json::{Value, json};

use piping_backend::contexts::secrets::application::SecretStore;
use piping_backend::contexts::secrets::infrastructure::{
    DbSecretStore, SecretRepository, SqlxSecretRepository,
};

const GROUPS: &str = "/api/v1/inventory/fhrp-groups";

async fn json_request(
    app: &Router,
    cookie: &str,
    method: &str,
    path: &str,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let payload = payload.map(|value| value.to_string());
    let (status, body) = request(app, method, path, payload.as_deref(), Some(cookie))
        .await
        .expect("HTTP request");
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON response")
    };
    (status, body)
}

fn group(name: &str) -> Value {
    json!({"group_id": 42, "name": name, "protocol": "vrrp3"})
}

async fn definition(app: &Router, cookie: &str, content_type: &Value, name: &str, secret: bool) {
    let (status, _) = json_request(
        app,
        cookie,
        "POST",
        "/api/v1/custom-fields",
        Some(json!({
            "name": name, "label": name, "field_type": "text", "is_secret": secret,
            "object_types": [content_type]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
#[ignore = "Requires the isolated PostgreSQL/Redis stack; run integration_rest fhrp_atomic_write_tests serially"]
async fn native_and_custom_secrets_commit_or_roll_back_with_the_fhrp_entity() {
    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let repository = SqlxSecretRepository::new(pool.clone());
    let store = DbSecretStore::new(pool.clone());
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    let (status, types) = json_request(
        &app,
        &cookie,
        "GET",
        "/api/v1/inventory/content-types",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let content_type = &types
        .as_array()
        .expect("content types")
        .iter()
        .find(|item| item["token"] == "ipam.fhrpgroup")
        .expect("FHRP type")["id"];
    let suffix = Uuid::now_v7().simple().to_string();
    let first = format!("atomic_first_{suffix}");
    let failing = "atomic_injected_failure";
    let note = format!("atomic_note_{suffix}");
    let other_note = format!("atomic_other_{suffix}");
    definition(&app, &cookie, content_type, &first, true).await;
    definition(&app, &cookie, content_type, failing, true).await;
    definition(&app, &cookie, content_type, &note, false).await;
    definition(&app, &cookie, content_type, &other_note, false).await;

    // Both native and custom-field whitespace are rejected before a row is created.
    let rejected_name = format!("atomic_rejected_{suffix}");
    for native in [true, false] {
        let mut payload = group(&rejected_name);
        if native {
            payload["auth_key"] = json!({"intent": "set", "value": " \t "});
        } else {
            payload["auth_key"] = json!({"intent": "set", "value": "synthetic-native"});
            payload["custom_fields"] = json!({(first.clone()): " \t "});
        }
        let (status, _) = json_request(&app, &cookie, "POST", GROUPS, Some(payload.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, bulk) = json_request(
            &app,
            &cookie,
            "POST",
            &format!("{GROUPS}/bulk"),
            Some(json!({"items": [payload]})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            bulk["created"]
                .as_array()
                .expect("created items")
                .is_empty()
        );
        assert_eq!(bulk["errors"].as_array().expect("errors").len(), 1);
    }
    let rejected: i64 = sqlx::query_scalar("SELECT count(*) FROM ipam_fhrpgroup WHERE name = $1")
        .bind(&rejected_name)
        .fetch_one(&pool)
        .await
        .expect("rejected row count");
    assert_eq!(rejected, 0);

    let name = format!("atomic_group_{suffix}");
    let mut payload = group(&name);
    payload["auth_key"] = json!({"intent": "set", "value": "synthetic-native-original"});
    payload["custom_fields"] = json!({(first.clone()): "synthetic-custom-original", (note.clone()): "original-note", (other_note.clone()): "preserved-note"});
    let (status, created) = json_request(&app, &cookie, "POST", GROUPS, Some(payload)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(created.get("auth_key").is_none());
    let id = Uuid::parse_str(created["id"].as_str().expect("group id")).expect("UUID");
    let path = format!("{GROUPS}/{id}");
    let native = repository
        .get_usage_binding("inventory_fhrp_group", id, "auth_key")
        .await
        .expect("binding lookup")
        .expect("native binding");
    let custom = repository
        .get_usage_binding("inventory_custom_field", id, &first)
        .await
        .expect("binding lookup")
        .expect("custom binding");

    // A failing entity FK must not eagerly rotate either secret or merge JSONB.
    let mut invalid_update = group("must-not-persist");
    invalid_update["owner_id"] = json!(Uuid::now_v7());
    invalid_update["auth_key"] =
        json!({"intent": "rotate", "value": "synthetic-native-replacement"});
    invalid_update["custom_fields"] = json!({(first.clone()): "synthetic-custom-replacement", (note.clone()): "replacement-note"});
    let (status, _) = json_request(&app, &cookie, "PUT", &path, Some(invalid_update)).await;
    assert!(!status.is_success());

    // Inject a native PostgreSQL failure after the native and first custom intent.
    // The fixture owns this trigger and removes it before continuing HTTP checks.
    sqlx::query("CREATE FUNCTION fhrp_atomic_reject_usage() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected atomic secret failure'; END; $$").execute(&pool).await.expect("failure fixture function");
    sqlx::query("CREATE TRIGGER fhrp_atomic_reject_usage BEFORE INSERT ON secret_usage_binding FOR EACH ROW WHEN (NEW.field_key = 'atomic_injected_failure') EXECUTE FUNCTION fhrp_atomic_reject_usage()").execute(&pool).await.expect("failure fixture trigger");
    let mut late_failure = group("must-also-not-persist");
    late_failure["auth_key"] = json!({"intent": "rotate", "value": "synthetic-native-replacement"});
    late_failure["custom_fields"] = json!({(first.clone()): "synthetic-custom-replacement", (failing): "synthetic-late-failure", (note.clone()): "replacement-note"});
    let (failed_status, _) = json_request(&app, &cookie, "PUT", &path, Some(late_failure)).await;
    sqlx::query("DROP TRIGGER fhrp_atomic_reject_usage ON secret_usage_binding")
        .execute(&pool)
        .await
        .expect("remove failure trigger");
    sqlx::query("DROP FUNCTION fhrp_atomic_reject_usage()")
        .execute(&pool)
        .await
        .expect("remove failure function");
    assert!(!failed_status.is_success());
    let (status, unchanged) = json_request(&app, &cookie, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unchanged, created);
    assert!(
        store
            .get_secret(native.secret_id)
            .await
            .expect("native secret")
            == "synthetic-native-original"
    );
    assert!(
        store
            .get_secret(custom.secret_id)
            .await
            .expect("custom secret")
            == "synthetic-custom-original"
    );
    assert_eq!(
        repository
            .get_usage_binding("inventory_fhrp_group", id, "auth_key")
            .await
            .expect("binding lookup")
            .expect("native binding")
            .secret_id,
        native.secret_id
    );
    assert!(
        repository
            .get_usage_binding("inventory_custom_field", id, failing)
            .await
            .expect("binding lookup")
            .is_none()
    );

    // An ordinary edit omits the native credential and merges only supplied plain fields.
    let mut edit = group(&name);
    edit["description"] = json!("ordinary edit");
    edit["custom_fields"] = json!({(note.clone()): "edited-note"});
    let (status, edited) = json_request(&app, &cookie, "PUT", &path, Some(edit)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(edited.get("auth_key").is_none());
    assert_eq!(edited["custom_fields"][&note], "edited-note");
    assert_eq!(edited["custom_fields"][&other_note], "preserved-note");
    assert_eq!(
        repository
            .get_usage_binding("inventory_fhrp_group", id, "auth_key")
            .await
            .expect("binding lookup")
            .expect("native binding")
            .secret_id,
        native.secret_id
    );
    assert!(
        store
            .get_secret(native.secret_id)
            .await
            .expect("native secret")
            == "synthetic-native-original"
    );
    let (status, list) = json_request(
        &app,
        &cookie,
        "GET",
        &format!("{GROUPS}?search={name}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        list["items"]
            .as_array()
            .expect("groups")
            .iter()
            .all(|item| item.get("auth_key").is_none())
    );

    let mut clear = group(&name);
    clear["auth_key"] = json!({"intent": "clear"});
    let (status, cleared) = json_request(&app, &cookie, "PUT", &path, Some(clear)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cleared.get("auth_key").is_none());
    assert_eq!(cleared["custom_fields"], edited["custom_fields"]);
    assert!(
        repository
            .get_usage_binding("inventory_fhrp_group", id, "auth_key")
            .await
            .expect("binding lookup")
            .is_none()
    );
    assert!(
        repository
            .get_usage_binding("inventory_custom_field", id, &first)
            .await
            .expect("binding lookup")
            .is_some()
    );
}
