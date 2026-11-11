use super::*;
use serde_json::{Value, json};

const GROUPS: &str = "/api/v1/inventory/fhrp-groups";
const ASSIGNMENTS: &str = "/api/v1/inventory/fhrp-group-assignments";
const IPS: &str = "/api/v1/inventory/ip-addresses";
const SERVICES: &str = "/api/v1/inventory/services";

async fn expect_json(
    app: &Router,
    cookie: Option<&str>,
    method: &str,
    path: &str,
    payload: Option<Value>,
    expected: StatusCode,
) -> Value {
    let payload = payload.map(|value| value.to_string());
    let (status, body) = request(app, method, path, payload.as_deref(), cookie)
        .await
        .expect("HTTP request");
    // Do not include arbitrary response bodies: credentials can appear in auth replies.
    assert_eq!(status, expected, "{method} {path}");
    if expected == StatusCode::NO_CONTENT {
        assert!(body.is_empty());
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON response")
    }
}

fn entity_path(base: &str, item: &Value) -> String {
    format!("{base}/{}", item["id"].as_str().expect("entity UUID"))
}

async fn assert_fhrp_filter(app: &Router, cookie: &str, path: &str, expected: &[Value]) {
    let list = expect_json(app, Some(cookie), "GET", path, None, StatusCode::OK).await;
    assert_eq!(list["total"], json!(expected.len()));
    let mut ids: Vec<_> = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["id"].as_str().expect("UUID"))
        .collect();
    let mut expected: Vec<_> = expected
        .iter()
        .map(|id| id.as_str().expect("expected UUID"))
        .collect();
    ids.sort_unstable();
    expected.sort_unstable();
    assert_eq!(ids, expected);
    let beyond = expect_json(
        app,
        Some(cookie),
        "GET",
        &format!("{path}&offset=10000"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(beyond["total"], list["total"]);
    assert_eq!(beyond["items"], json!([]));
}

async fn assert_fhrp_sorts(
    app: &Router,
    cookie: &str,
    path: &str,
    columns: &[piping_backend::contexts::shared::list_query::ListColumn],
) {
    for column in columns.iter().filter(|column| column.sort_sql.is_some()) {
        for order in ["asc", "desc"] {
            let query = format!("{path}&sort={}&order={order}", column.key);
            let list = expect_json(app, Some(cookie), "GET", &query, None, StatusCode::OK).await;
            for (offset, item) in list["items"]
                .as_array()
                .expect("items")
                .iter()
                .take(2)
                .enumerate()
            {
                let page = expect_json(
                    app,
                    Some(cookie),
                    "GET",
                    &format!("{query}&limit=1&offset={offset}"),
                    None,
                    StatusCode::OK,
                )
                .await;
                assert_eq!(page["total"], list["total"]);
                assert_eq!(
                    page["items"][0]["id"], item["id"],
                    "stable {} pagination",
                    column.key
                );
            }
        }
    }
    for query in [
        "sort=auth_key",
        "sort=__invalid__",
        "sort=group_id&order=sideways",
    ] {
        expect_json(
            app,
            Some(cookie),
            "GET",
            &format!("{path}&{query}"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
}

fn group_payload(name: &str) -> Value {
    json!({
        "group_id": 42, "name": name, "protocol": "vrrp3",
        "description": "HTTP FHRP fixture", "comments": "round trip",
        "tags": ["fhrp-http"]
    })
}

fn assert_persisted_dates(item: &Value) {
    for field in ["created", "last_updated"] {
        chrono::DateTime::parse_from_rfc3339(item[field].as_str().expect("persisted timestamp"))
            .expect("RFC3339 timestamp");
    }
}

fn unique_ipv6_host() -> String {
    let mut octets = *Uuid::now_v7().as_bytes();
    octets[0] = 0xfd;
    format!("{}/128", std::net::Ipv6Addr::from(octets))
}

async fn admin_app() -> (Router, String) {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    (app, cookie)
}

async fn fhrp_secret_snapshot(pool: &sqlx::PgPool, secret_id: Uuid) -> Value {
    // Read-only snapshots include ciphertext, so callers compare without printing them.
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'variable', (SELECT to_jsonb(v) FROM secrets_variable v WHERE id = $1),
            'store', (SELECT to_jsonb(s) FROM secrets_store s WHERE secret_id = $1),
            'bindings', (SELECT COALESCE(jsonb_agg(to_jsonb(b) ORDER BY id), '[]'::jsonb) FROM secrets_binding b WHERE secret_id = $1),
            'usage', (SELECT COALESCE(jsonb_agg(to_jsonb(u) ORDER BY id), '[]'::jsonb) FROM secret_usage_binding u WHERE secret_id = $1))",
    )
    .bind(secret_id)
    .fetch_one(pool)
    .await
    .expect("secret lifecycle snapshot")
}

async fn create_group_field(app: &Router, cookie: &str, name: &str, is_secret: bool) -> Value {
    let types = expect_json(
        app,
        Some(cookie),
        "GET",
        "/api/v1/inventory/content-types",
        None,
        StatusCode::OK,
    )
    .await;
    let scope = types
        .as_array()
        .expect("content types")
        .iter()
        .find(|item| item["token"] == "ipam.fhrpgroup")
        .expect("FHRP content type");
    assert_eq!(scope["supported_for_custom_fields"], true);
    expect_json(
        app,
        Some(cookie),
        "POST",
        "/api/v1/custom-fields",
        Some(json!({
            "name": name, "label": name, "field_type": "text", "is_secret": is_secret,
            "object_types": [scope["id"]]
        })),
        StatusCode::CREATED,
    )
    .await
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_tests filter"]
async fn fhrp_groups_crud_bulk_search_and_persisted_metadata() {
    let (app, cookie) = admin_app().await;
    let name = format!("fhrp-http-{}", unique_suffix());
    let note_name = format!("test_note_{}", Uuid::now_v7().simple());
    let note_definition = create_group_field(&app, &cookie, &note_name, false).await;
    let supplied_id = Uuid::now_v7();
    let owner = expect_json(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/owners",
        Some(json!({"name":format!("{name}-owner")})),
        StatusCode::CREATED,
    )
    .await;
    let mut payload = group_payload(&name);
    payload["owner_id"] = owner["id"].clone();
    payload["custom_fields"] = json!({(note_name.clone()): "persisted"});
    payload["id"] = json!(supplied_id);
    payload["created"] = json!("2000-01-01T00:00:00Z");
    payload["last_updated"] = json!("2000-01-01T00:00:00Z");
    let created = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(payload),
        StatusCode::CREATED,
    )
    .await;
    assert_ne!(created["id"], json!(supplied_id));
    assert_ne!(created["created"], "2000-01-01T00:00:00Z");
    assert_persisted_dates(&created);
    assert_eq!(created["group_id"], 42);
    assert_eq!(created["tags"], json!(["fhrp-http"]));
    assert_eq!(created["custom_fields"][&note_name], "persisted");
    let path = entity_path(GROUPS, &created);
    assert_fhrp_filter(
        &app,
        &cookie,
        &format!(
            "{GROUPS}?owner_id={}&protocol=vrrp3",
            owner["id"].as_str().expect("owner UUID")
        ),
        &[created["id"].clone()],
    )
    .await;
    assert_fhrp_filter(
        &app,
        &cookie,
        &format!(
            "{GROUPS}?owner_id={}&protocol=hsrp",
            owner["id"].as_str().expect("owner UUID")
        ),
        &[],
    )
    .await;
    assert_fhrp_sorts(&app, &cookie, &format!("{GROUPS}?owner_id={}", owner["id"].as_str().expect("owner UUID")), piping_backend::contexts::inventory::infrastructure::persistence::repositories::ipam::fhrp_group_repository::FHRP_GROUP_COLUMNS).await;
    for query in ["owner_id=invalid", "protocol=invalid"] {
        expect_json(
            &app,
            Some(&cookie),
            "GET",
            &format!("{GROUPS}?{query}"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let fetched = expect_json(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await;
    assert_eq!(
        fetched, created,
        "create response must be the persisted row"
    );

    let mut replacement = group_payload(&format!("{name}-updated"));
    replacement["id"] = json!(Uuid::now_v7());
    replacement["group_id"] = json!(255);
    replacement["protocol"] = json!("hsrp");
    replacement["description"] = json!("replacement");
    let updated = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &path,
        Some(replacement),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated["id"], created["id"]);
    assert_eq!(updated["created"], created["created"]);
    assert_eq!(updated["protocol"], "hsrp");
    assert_eq!(updated["group_id"], 255);
    assert_eq!(updated["description"], "replacement");
    assert_persisted_dates(&updated);
    assert_eq!(
        expect_json(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await,
        updated
    );

    let mut invalid = group_payload(&format!("{name}-invalid"));
    invalid["protocol"] = json!("unsupported");
    let bulk = expect_json(
        &app,
        Some(&cookie),
        "POST",
        &format!("{GROUPS}/bulk"),
        Some(json!({
            "items": [group_payload(&format!("{name}-bulk")), invalid]
        })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(bulk["created"].as_array().expect("created").len(), 1);
    assert_eq!(bulk["errors"].as_array().expect("errors").len(), 1);
    assert_eq!(bulk["errors"][0]["index"], 2);
    let bulk_item = &bulk["created"][0];
    assert_persisted_dates(bulk_item);
    assert_eq!(
        expect_json(
            &app,
            Some(&cookie),
            "GET",
            &entity_path(GROUPS, bulk_item),
            None,
            StatusCode::OK
        )
        .await,
        *bulk_item
    );

    let list = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!("{GROUPS}?search={name}&sort=name&order=asc&limit=1&offset=0"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(list["total"], 2);
    assert_eq!(list["limit"], 1);
    assert_eq!(list["items"][0]["id"], bulk_item["id"]);
    let second = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!("{GROUPS}?search={name}&sort=name&order=asc&limit=1&offset=1"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(second["offset"], 1);
    assert_eq!(second["items"][0]["id"], created["id"]);
    expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!("{GROUPS}?sort=not_a_column"),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "GET",
        &path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path(GROUPS, bulk_item),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/custom-fields", &note_definition),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_tests filter"]
async fn fhrp_custom_fields_validate_names_and_store_secrets_outside_entity_json() {
    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let values = state.services.custom_field_values.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    let name = format!("fhrp-fields-{}", unique_suffix());
    let field_name = format!("fhrp_secret_{}", Uuid::now_v7().simple());
    let definition = create_group_field(&app, &cookie, &field_name, true).await;
    let secret = Uuid::now_v7().to_string();
    let mut payload = group_payload(&name);
    payload["custom_fields"] = json!({(field_name.clone()): secret});
    let created = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(payload),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(created["custom_fields"][&field_name], "****");
    let id = Uuid::parse_str(created["id"].as_str().expect("group id")).expect("UUID");
    let raw: Value = sqlx::query_scalar("SELECT custom_field_data FROM ipam_fhrpgroup WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("persisted custom fields");
    assert!(
        raw.get(&field_name).is_none(),
        "secret must not be persisted in entity JSON"
    );
    let clear = values
        .read_for_entity_token("ipam.fhrpgroup", id, &raw, true)
        .await
        .expect("stored secret")
        .to_value_map();
    assert!(
        clear[&field_name].as_str() == Some(secret.as_str()),
        "canonical secret store must contain the written value"
    );
    let path = entity_path(GROUPS, &created);
    let fetched = expect_json(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await;
    assert_eq!(fetched["custom_fields"][&field_name], "****");
    // A read-modify-write must preserve the secret, not replace it with the mask.
    let updated = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &path,
        Some(fetched),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated["custom_fields"][&field_name], "****");
    let clear = values
        .read_for_entity_token("ipam.fhrpgroup", id, &raw, true)
        .await
        .expect("unchanged secret")
        .to_value_map();
    assert!(clear[&field_name].as_str() == Some(secret.as_str()));

    let rotated_secret = Uuid::now_v7().to_string();
    let mut replacement = group_payload(&name);
    replacement["custom_fields"] = json!({(field_name.clone()): rotated_secret});
    let updated = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &path,
        Some(replacement),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated["custom_fields"][&field_name], "****");
    let raw: Value = sqlx::query_scalar("SELECT custom_field_data FROM ipam_fhrpgroup WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("updated custom fields");
    assert!(raw.get(&field_name).is_none());
    let clear = values
        .read_for_entity_token("ipam.fhrpgroup", id, &raw, true)
        .await
        .expect("rotated secret")
        .to_value_map();
    assert!(clear[&field_name].as_str() == Some(rotated_secret.as_str()));

    let reveal_path = format!("{path}/custom-fields/reveal");
    expect_json(
        &app,
        None,
        "POST",
        &reveal_path,
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "POST",
        &format!("{GROUPS}/{}/custom-fields/reveal", Uuid::now_v7()),
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    let revealed = expect_json(
        &app,
        Some(&cookie),
        "POST",
        &reveal_path,
        None,
        StatusCode::OK,
    )
    .await;
    assert!(revealed["custom_fields"][&field_name].as_str() == Some(rotated_secret.as_str()));
    let audit: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_event WHERE event_type='inventory.custom_fields.reveal' AND target_type='inventory:ipam.fhrpgroup' AND target_id=$1",
    ).bind(id).fetch_one(&pool).await.expect("reveal audit event");
    assert!(
        audit["keys"]
            .as_array()
            .expect("audited keys")
            .contains(&json!(field_name))
    );
    assert!(
        !audit.to_string().contains(&rotated_secret),
        "audit must not contain secret values"
    );

    let unknown_name = format!("undefined_{}", Uuid::now_v7().simple());
    let mut invalid = group_payload(&format!("{name}-invalid"));
    invalid["custom_fields"] = json!({(unknown_name): "reject me"});
    expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(invalid.clone()),
        StatusCode::BAD_REQUEST,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &path,
        Some(invalid.clone()),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(
        expect_json(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await,
        updated
    );

    let mut bulk_payload = group_payload(&format!("{name}-bulk"));
    bulk_payload["custom_fields"] = json!({(field_name.clone()): secret});
    let bulk = expect_json(
        &app,
        Some(&cookie),
        "POST",
        &format!("{GROUPS}/bulk"),
        Some(json!({"items": [bulk_payload, invalid]})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(bulk["created"].as_array().expect("created").len(), 1);
    assert_eq!(bulk["errors"].as_array().expect("errors").len(), 1);
    assert_eq!(bulk["errors"][0]["index"], 2);
    let bulk_item = &bulk["created"][0];
    assert_eq!(bulk_item["custom_fields"][&field_name], "****");
    let bulk_id = Uuid::parse_str(bulk_item["id"].as_str().expect("bulk id")).expect("UUID");
    let raw: Value = sqlx::query_scalar("SELECT custom_field_data FROM ipam_fhrpgroup WHERE id=$1")
        .bind(bulk_id)
        .fetch_one(&pool)
        .await
        .expect("bulk custom fields");
    assert!(raw.get(&field_name).is_none());
    let clear = values
        .read_for_entity_token("ipam.fhrpgroup", bulk_id, &raw, true)
        .await
        .expect("bulk secret")
        .to_value_map();
    assert!(clear[&field_name].as_str() == Some(secret.as_str()));
    let list = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!("{GROUPS}?search={name}"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        list["total"], 2,
        "unknown custom fields must fail before persistence"
    );
    for item in list["items"].as_array().expect("items") {
        assert_eq!(item["custom_fields"][&field_name], "****");
        expect_json(
            &app,
            Some(&cookie),
            "DELETE",
            &entity_path(GROUPS, item),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
    }
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/custom-fields", &definition),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_tests filter"]
async fn fhrp_assignments_crud_bulk_and_typed_interface_validation() {
    let state = build_test_app_state().await.expect("app state");
    let (device_id, _) = common::seed_device(&state.db_pool, "fhrp-http")
        .await
        .expect("device fixture");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    let name = format!("fhrp-members-{}", unique_suffix());
    let group = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(group_payload(&name)),
        StatusCode::CREATED,
    )
    .await;
    let interface = expect_json(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/interfaces",
        Some(json!({
            "device_id": device_id, "name": "fhrp-http0"
        })),
        StatusCode::CREATED,
    )
    .await;
    let vm = expect_json(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/virtual_machines",
        Some(json!({"name": name})),
        StatusCode::CREATED,
    )
    .await;
    let vm_interface = expect_json(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/vm-interfaces",
        Some(json!({
            "virtual_machine_id": vm["id"], "name": "fhrp-vm0"
        })),
        StatusCode::CREATED,
    )
    .await;
    let assignment = json!({
        "group_id": group["id"], "interface_type": "dcim.interface", "interface_id": interface["id"], "priority": 0
    });
    let mut supplied = assignment.clone();
    let supplied_id = Uuid::now_v7();
    supplied["id"] = json!(supplied_id);
    let created = expect_json(
        &app,
        Some(&cookie),
        "POST",
        ASSIGNMENTS,
        Some(supplied),
        StatusCode::CREATED,
    )
    .await;
    assert_ne!(created["id"], json!(supplied_id));
    assert_eq!(created["priority"], 0);
    assert_eq!(created["interface_type"], "dcim.interface");
    assert_persisted_dates(&created);
    let path = entity_path(ASSIGNMENTS, &created);
    assert_eq!(
        expect_json(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await,
        created
    );
    let mut replacement = assignment.clone();
    replacement["id"] = json!(Uuid::now_v7());
    replacement["priority"] = json!(255);
    let updated = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &path,
        Some(replacement),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated["id"], created["id"]);
    assert_eq!(updated["priority"], 255);
    assert_eq!(updated["created"], created["created"]);
    assert_eq!(
        expect_json(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await,
        updated
    );

    expect_json(
        &app,
        Some(&cookie),
        "POST",
        ASSIGNMENTS,
        Some(assignment.clone()),
        StatusCode::CONFLICT,
    )
    .await;
    for priority in [-1, 256] {
        let mut invalid = assignment.clone();
        invalid["priority"] = json!(priority);
        expect_json(
            &app,
            Some(&cookie),
            "POST",
            ASSIGNMENTS,
            Some(invalid.clone()),
            StatusCode::BAD_REQUEST,
        )
        .await;
        expect_json(
            &app,
            Some(&cookie),
            "PUT",
            &path,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    for (object_type, target) in [
        ("dcim.device", json!(device_id)),
        ("virtualization.vminterface", interface["id"].clone()),
        ("dcim.interface", json!(Uuid::now_v7())),
    ] {
        let mut invalid = assignment.clone();
        invalid["interface_type"] = json!(object_type);
        invalid["interface_id"] = target;
        expect_json(
            &app,
            Some(&cookie),
            "POST",
            ASSIGNMENTS,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    assert_eq!(
        expect_json(&app, Some(&cookie), "GET", &path, None, StatusCode::OK).await,
        updated,
        "rejected writes must not alter the assignment"
    );

    let vm_assignment = json!({
        "group_id": group["id"], "interface_type": "virtualization.vminterface", "interface_id": vm_interface["id"], "priority": 100
    });
    let mut invalid = vm_assignment.clone();
    invalid["priority"] = json!(256);
    let bulk = expect_json(
        &app,
        Some(&cookie),
        "POST",
        &format!("{ASSIGNMENTS}/bulk"),
        Some(json!({
            "items": [vm_assignment, assignment, invalid]
        })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(bulk["created"].as_array().expect("created").len(), 1);
    let errors = bulk["errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["index"], 2);
    assert_eq!(errors[1]["index"], 3);
    let vm_member = &bulk["created"][0];
    let group_filter = format!(
        "{ASSIGNMENTS}?group_id={}",
        group["id"].as_str().expect("group UUID")
    );
    assert_fhrp_filter(
        &app,
        &cookie,
        &group_filter,
        &[created["id"].clone(), vm_member["id"].clone()],
    )
    .await;
    for (token, target, member) in [
        ("dcim.interface", &interface, &created),
        ("virtualization.vminterface", &vm_interface, vm_member),
    ] {
        assert_fhrp_filter(
            &app,
            &cookie,
            &format!(
                "{group_filter}&interface_type={token}&interface_id={}",
                target["id"].as_str().expect("target UUID")
            ),
            &[member["id"].clone()],
        )
        .await;
    }
    assert_fhrp_filter(
        &app,
        &cookie,
        &format!(
            "{group_filter}&interface_type=virtualization.vminterface&interface_id={}",
            interface["id"].as_str().expect("interface UUID")
        ),
        &[],
    )
    .await;
    assert_fhrp_sorts(&app, &cookie, &group_filter, piping_backend::contexts::inventory::infrastructure::persistence::repositories::ipam::fhrp_group_assignment_repository::FHRP_GROUP_ASSIGNMENT_COLUMNS).await;
    for query in [
        "group_id=invalid",
        "interface_id=invalid",
        "interface_type=dcim.device",
    ] {
        expect_json(
            &app,
            Some(&cookie),
            "GET",
            &format!("{ASSIGNMENTS}?{query}"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    assert_eq!(vm_member["interface_type"], "virtualization.vminterface");
    assert_persisted_dates(vm_member);
    assert_eq!(
        expect_json(
            &app,
            Some(&cookie),
            "GET",
            &entity_path(ASSIGNMENTS, vm_member),
            None,
            StatusCode::OK
        )
        .await,
        *vm_member
    );
    let list = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!(
            "{ASSIGNMENTS}?search={}&sort=priority&order=asc",
            group["id"].as_str().expect("group id")
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(list["total"], 2);
    assert_eq!(list["items"][0]["id"], vm_member["id"]);
    assert_eq!(list["items"][1]["id"], created["id"]);
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "GET",
        &path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path(ASSIGNMENTS, vm_member),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path(GROUPS, &group),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/inventory/interfaces", &interface),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/inventory/vm-interfaces", &vm_interface),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/inventory/virtual_machines", &vm),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_tests filter"]
async fn fhrp_ip_assignment_and_service_parent_round_trip() {
    let (app, cookie) = admin_app().await;
    let name = format!("fhrp-parent-{}", unique_suffix());
    let group = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(group_payload(&name)),
        StatusCode::CREATED,
    )
    .await;
    let other = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(group_payload(&format!("{name}-other"))),
        StatusCode::CREATED,
    )
    .await;
    let reference = json!({"object_type": "ipam.fhrpgroup", "object_id": group["id"]});
    let other_reference = json!({"object_type": "ipam.fhrpgroup", "object_id": other["id"]});
    // A unique IPv6 host prevents collisions in a shared integration database.
    let address = unique_ipv6_host();
    let unattached = expect_json(
        &app,
        Some(&cookie),
        "POST",
        IPS,
        Some(json!({
            "address": unique_ipv6_host(), "assigned_object": null
        })),
        StatusCode::CREATED,
    )
    .await;
    assert!(unattached["assigned_object"].is_null());
    let ip = expect_json(
        &app,
        Some(&cookie),
        "POST",
        IPS,
        Some(json!({
            "address": address, "assigned_object": reference
        })),
        StatusCode::CREATED,
    )
    .await;
    let ip_path = entity_path(IPS, &ip);
    assert_eq!(ip["assigned_object"]["object_type"], "ipam.fhrpgroup");
    assert_eq!(ip["assigned_object"]["object_id"], group["id"]);
    assert_eq!(ip["assigned_object"]["label"], name);
    let fetched = expect_json(&app, Some(&cookie), "GET", &ip_path, None, StatusCode::OK).await;
    assert_eq!(fetched["assigned_object"], ip["assigned_object"]);
    let retained = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &ip_path,
        Some(json!({"description": "keep parent"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(retained["assigned_object"], ip["assigned_object"]);
    let detached = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &ip_path,
        Some(json!({"assigned_object": null})),
        StatusCode::OK,
    )
    .await;
    assert!(detached["assigned_object"].is_null());
    assert!(expect_json(&app, Some(&cookie), "GET", &ip_path, None, StatusCode::OK).await["assigned_object"].is_null());
    let reassigned = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &ip_path,
        Some(json!({"assigned_object": other_reference})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(reassigned["assigned_object"]["object_id"], other["id"]);
    assert_eq!(
        reassigned["assigned_object"]["label"],
        format!("{name}-other")
    );

    let service = expect_json(
        &app,
        Some(&cookie),
        "POST",
        SERVICES,
        Some(json!({
            "name": name, "port_mappings": ["tcp/443"], "parent_object": reference
        })),
        StatusCode::CREATED,
    )
    .await;
    let service_path = entity_path(SERVICES, &service);
    assert_eq!(service["parent_object"]["object_id"], group["id"]);
    assert_eq!(service["parent_object"]["object_type"], "ipam.fhrpgroup");
    assert_eq!(service["parent_object"]["label"], name);
    let fetched = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &service_path,
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched["parent_object"], service["parent_object"]);
    let moved = expect_json(
        &app,
        Some(&cookie),
        "PUT",
        &service_path,
        Some(json!({"parent_object": other_reference})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(moved["parent_object"]["object_id"], other["id"]);
    let list = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!(
            "{SERVICES}?parent_object_id={}",
            other["id"].as_str().expect("group id")
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["parent_object"], moved["parent_object"]);
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &service_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &ip_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path(IPS, &unattached),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    for item in [&group, &other] {
        expect_json(
            &app,
            Some(&cookie),
            "DELETE",
            &entity_path(GROUPS, item),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_tests filter"]
async fn fhrp_permissions_deny_anonymous_writes_and_unprivileged_user() {
    let (app, cookie) = admin_app().await;
    let name = format!("fhrp-denied-{}", unique_suffix());
    // Reuse the normal local-user creation flow used by settings_tests; no roles means no inventory grant.
    let user = expect_json(&app, Some(&cookie), "POST", "/api/v1/admin/access-control/users", Some(json!({
        "username": name, "email": format!("{name}@test.local"), "status": "active", "auth_source": "local",
        "credential": {"mode": "manual", "password": REST_TEST_LOCAL_PASSWORD}
    })), StatusCode::OK).await;
    let user_cookie = login_and_get_cookie(&app, &name, REST_TEST_LOCAL_PASSWORD)
        .await
        .expect("unprivileged login");
    let group = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(group_payload(&name)),
        StatusCode::CREATED,
    )
    .await;
    for (base, payload, id) in [
        (
            GROUPS,
            group_payload(&format!("{name}-unauthorized")),
            group["id"].as_str().expect("group id").to_string(),
        ),
        (
            ASSIGNMENTS,
            json!({"group_id": group["id"], "interface_type": "dcim.interface", "interface_id": Uuid::now_v7(), "priority": 100}),
            Uuid::now_v7().to_string(),
        ),
    ] {
        let path = format!("{base}/{id}");
        for (auth, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(user_cookie.as_str()), StatusCode::FORBIDDEN),
        ] {
            expect_json(&app, auth, "POST", base, Some(payload.clone()), expected).await;
            expect_json(
                &app,
                auth,
                "POST",
                &format!("{base}/bulk"),
                Some(json!({"items": [payload.clone()]})),
                expected,
            )
            .await;
            expect_json(&app, auth, "PUT", &path, Some(payload.clone()), expected).await;
            expect_json(&app, auth, "DELETE", &path, None, expected).await;
        }
        expect_json(
            &app,
            Some(&user_cookie),
            "GET",
            base,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
        expect_json(
            &app,
            Some(&user_cookie),
            "GET",
            &path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    assert_eq!(
        expect_json(
            &app,
            Some(&cookie),
            "GET",
            &entity_path(GROUPS, &group),
            None,
            StatusCode::OK
        )
        .await,
        group
    );
    let list = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!("{GROUPS}?search={name}"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        list["total"], 1,
        "denied single and bulk requests must not persist groups"
    );
    // Grant only READ through the existing policy API. The same authenticated
    // user can now fetch the group but must still fail its separate Secret gate.
    let policy = expect_json(&app, Some(&cookie), "POST", "/api/v1/admin/access-control/policies", Some(json!({
        "name": format!("{name}-read-only"), "effect": "allow", "actions": ["READ"],
        "resource_types": ["INVENTORY"], "subjects": [format!("user:{}", user["id"].as_str().expect("user id"))],
        "resources": ["*"], "enabled": true
    })), StatusCode::OK).await;
    expect_json(
        &app,
        Some(&user_cookie),
        "GET",
        &entity_path(GROUPS, &group),
        None,
        StatusCode::OK,
    )
    .await;
    expect_json(
        &app,
        Some(&user_cookie),
        "POST",
        &format!("{}/custom-fields/reveal", entity_path(GROUPS, &group)),
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/admin/access-control/policies", &policy),
        None,
        StatusCode::OK,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path(GROUPS, &group),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/admin/access-control/users", &user),
        None,
        StatusCode::OK,
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_tests filter"]
async fn fhrp_delete_blocks_references_without_retiring_secrets_then_cleans_managed_rows() {
    use piping_backend::contexts::secrets::application::consumer_type::{
        FHRP_AUTH_KEY_FIELD, INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE,
        INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
    };
    use piping_backend::contexts::secrets::infrastructure::SecretRepository;

    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let repository = state.repositories.secrets.clone();
    let store = state.services.secret_store.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    let name = format!("fhrp-delete-{}", unique_suffix());
    let field_name = format!("fhrp_delete_{}", Uuid::now_v7().simple());
    let definition = create_group_field(&app, &cookie, &field_name, true).await;
    let native_value = format!("native-fixture-{}", Uuid::now_v7());
    let custom_value = format!("custom-fixture-{}", Uuid::now_v7());
    let mut payload = group_payload(&name);
    payload["auth_type"] = json!("plaintext");
    payload["auth_key"] = json!({"intent": "set", "value": native_value});
    payload["custom_fields"] = json!({(field_name.clone()): custom_value});
    let group = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(payload),
        StatusCode::CREATED,
    )
    .await;
    assert!(group.get("auth_key").is_none());
    assert_eq!(group["custom_fields"][&field_name], "****");
    let id = Uuid::parse_str(group["id"].as_str().expect("group id")).expect("UUID");
    let group_path = entity_path(GROUPS, &group);
    let mut managed = Vec::new();
    for (consumer, field, value) in [
        (
            INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
            FHRP_AUTH_KEY_FIELD,
            native_value.as_str(),
        ),
        (
            INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE,
            field_name.as_str(),
            custom_value.as_str(),
        ),
    ] {
        let usage = repository
            .get_usage_binding(consumer, id, field)
            .await
            .expect("usage lookup")
            .expect("managed secret usage");
        assert_eq!(usage.owner_scope_type, consumer);
        assert_eq!(usage.owner_scope_id, id);
        assert!(
            store
                .get_secret(usage.secret_id)
                .await
                .expect("stored value")
                == value
        );
        let snapshot = fhrp_secret_snapshot(&pool, usage.secret_id).await;
        assert_eq!(snapshot["variable"]["kind"], "field");
        assert_eq!(snapshot["variable"]["retire_with_field_usage"], true);
        let bindings = snapshot["bindings"].as_array().expect("scope bindings");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0]["managed_by_field_usage"], true);
        managed.push((consumer, field, value, usage, snapshot));
    }

    // An unrelated named secret is created through canonical APIs, never raw inserts.
    let named = repository
        .create_named(&format!("{name}-unrelated"), None, None)
        .await
        .expect("named secret fixture");
    let named_value = format!("named-fixture-{}", Uuid::now_v7());
    store
        .set_secret(named.id, &named_value)
        .await
        .expect("named secret value");
    let named_snapshot = fhrp_secret_snapshot(&pool, named.id).await;
    let reference = json!({"object_type": "ipam.fhrpgroup", "object_id": id});
    let ip = expect_json(
        &app,
        Some(&cookie),
        "POST",
        IPS,
        Some(json!({"address": unique_ipv6_host(), "assigned_object": reference})),
        StatusCode::CREATED,
    )
    .await;

    // Test each polymorphic reference independently: one must not hide the other.
    for (table, entity_key, dependency) in [
        ("ipam_ipaddress", "IP Addresses", ip),
        ("ipam_service", "Services", Value::Null),
    ] {
        let dependency = if dependency.is_null() {
            expect_json(
                &app,
                Some(&cookie),
                "POST",
                SERVICES,
                Some(
                    json!({"name": name, "port_mappings": ["tcp/443"], "parent_object": reference}),
                ),
                StatusCode::CREATED,
            )
            .await
        } else {
            dependency
        };
        let blocked = expect_json(
            &app,
            Some(&cookie),
            "DELETE",
            &group_path,
            None,
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(blocked["code"], "DELETE_BLOCKED");
        assert_eq!(blocked["reason_code"], "DELETE_BLOCKED");
        assert_eq!(blocked["details"]["total_count"], 1);
        let blockers = blocked["details"]["blockers"]
            .as_array()
            .expect("structured blockers");
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0]["table"], table);
        assert_eq!(blockers[0]["entity_key"], entity_key);
        assert_eq!(blockers[0]["count"], 1);
        assert_eq!(blockers[0]["examples"][0]["id"], dependency["id"]);
        assert!(
            blockers[0]["examples"][0]["label"]
                .as_str()
                .is_some_and(|label| !label.is_empty())
        );
        assert_eq!(
            expect_json(
                &app,
                Some(&cookie),
                "GET",
                &group_path,
                None,
                StatusCode::OK
            )
            .await,
            group,
            "blocked deletion must preserve the group"
        );
        for (consumer, field, value, original_usage, snapshot) in &managed {
            let usage = repository
                .get_usage_binding(consumer, id, field)
                .await
                .expect("usage after rejection")
                .expect("usage retained");
            assert_eq!(usage.id, original_usage.id);
            assert_eq!(usage.secret_id, original_usage.secret_id);
            assert!(
                repository
                    .get_variable_by_id(usage.secret_id)
                    .await
                    .expect("variable lookup")
                    .is_some()
            );
            assert!(
                store
                    .get_secret(usage.secret_id)
                    .await
                    .expect("retained value")
                    == *value
            );
            assert!(
                fhrp_secret_snapshot(&pool, usage.secret_id).await == *snapshot,
                "blocked deletion changed secret lifecycle rows"
            );
        }
        let native = expect_json(
            &app,
            Some(&cookie),
            "POST",
            &format!("{group_path}/auth-key/reveal"),
            None,
            StatusCode::OK,
        )
        .await;
        assert!(native["value"].as_str() == Some(native_value.as_str()));
        let custom = expect_json(
            &app,
            Some(&cookie),
            "POST",
            &format!("{group_path}/custom-fields/reveal"),
            None,
            StatusCode::OK,
        )
        .await;
        assert!(custom["custom_fields"][&field_name].as_str() == Some(custom_value.as_str()));
        let base = if table == "ipam_ipaddress" {
            IPS
        } else {
            SERVICES
        };
        let dependency_path = entity_path(base, &dependency);
        let fetched = expect_json(
            &app,
            Some(&cookie),
            "GET",
            &dependency_path,
            None,
            StatusCode::OK,
        )
        .await;
        let association = if base == IPS {
            "assigned_object"
        } else {
            "parent_object"
        };
        assert_eq!(fetched[association]["object_id"], json!(id));
        expect_json(
            &app,
            Some(&cookie),
            "DELETE",
            &dependency_path,
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
        assert!(fhrp_secret_snapshot(&pool, named.id).await == named_snapshot);
    }

    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &group_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    expect_json(
        &app,
        Some(&cookie),
        "GET",
        &group_path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    for (consumer, field, _, usage, _) in &managed {
        assert!(
            repository
                .get_usage_binding(consumer, id, field)
                .await
                .expect("removed usage lookup")
                .is_none()
        );
        assert!(
            repository
                .list_usage_bindings_for_consumer(consumer, id)
                .await
                .expect("consumer usages")
                .is_empty()
        );
        assert!(
            repository
                .get_variable_by_id(usage.secret_id)
                .await
                .expect("retired variable lookup")
                .is_none()
        );
        assert!(
            repository
                .list_bindings_for_secret(usage.secret_id)
                .await
                .expect("retired scope bindings")
                .is_empty()
        );
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM secrets_store WHERE secret_id = $1")
                .bind(usage.secret_id)
                .fetch_one(&pool)
                .await
                .expect("retired store row count");
        assert_eq!(remaining, 0);
    }
    assert!(
        repository
            .get_variable_by_id(named.id)
            .await
            .expect("unrelated named variable")
            .is_some()
    );
    assert!(
        store
            .get_secret(named.id)
            .await
            .expect("unrelated named value")
            == named_value
    );
    assert!(
        fhrp_secret_snapshot(&pool, named.id).await == named_snapshot,
        "group cleanup must not mutate unrelated named secrets"
    );
    expect_json(
        &app,
        Some(&cookie),
        "DELETE",
        &entity_path("/api/v1/custom-fields", &definition),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    assert!(
        repository
            .delete_named(named.id)
            .await
            .expect("named fixture cleanup")
    );
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the focused integration_rest fhrp_tests filter"]
async fn fhrp_object_grants_do_not_promote_to_other_objects_or_collections() {
    let state = build_test_app_state().await.expect("app state");
    let (device_id, _) = common::seed_device(&state.db_pool, "fhrp-scoped")
        .await
        .expect("device fixture");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    let name = format!("fhrp-scoped-{}", unique_suffix());
    let group_a = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(group_payload(&format!("{name}-a"))),
        StatusCode::CREATED,
    )
    .await;
    let group_b = expect_json(
        &app,
        Some(&cookie),
        "POST",
        GROUPS,
        Some(group_payload(&format!("{name}-b"))),
        StatusCode::CREATED,
    )
    .await;
    let interface = expect_json(
        &app,
        Some(&cookie),
        "POST",
        "/api/v1/inventory/interfaces",
        Some(json!({"device_id": device_id, "name": "fhrp-scoped0"})),
        StatusCode::CREATED,
    )
    .await;
    let assignment_payload = |group: &Value| {
        json!({
            "group_id": group["id"], "interface_type": "dcim.interface", "interface_id": interface["id"], "priority": 100
        })
    };
    let assignment_a = expect_json(
        &app,
        Some(&cookie),
        "POST",
        ASSIGNMENTS,
        Some(assignment_payload(&group_a)),
        StatusCode::CREATED,
    )
    .await;
    let assignment_b = expect_json(
        &app,
        Some(&cookie),
        "POST",
        ASSIGNMENTS,
        Some(assignment_payload(&group_b)),
        StatusCode::CREATED,
    )
    .await;
    let ip_payload = |group: &Value| {
        json!({
            "address": unique_ipv6_host(),
            "assigned_object": {"object_type": "ipam.fhrpgroup", "object_id": group["id"]}
        })
    };
    let service_payload = |group: &Value, suffix: &str| {
        json!({
            "name": format!("{name}-{suffix}"), "port_mappings": ["tcp/443"],
            "parent_object": {"object_type": "ipam.fhrpgroup", "object_id": group["id"]}
        })
    };
    let ip_a = expect_json(
        &app,
        Some(&cookie),
        "POST",
        IPS,
        Some(ip_payload(&group_a)),
        StatusCode::CREATED,
    )
    .await;
    let ip_b = expect_json(
        &app,
        Some(&cookie),
        "POST",
        IPS,
        Some(ip_payload(&group_b)),
        StatusCode::CREATED,
    )
    .await;
    let service_a = expect_json(
        &app,
        Some(&cookie),
        "POST",
        SERVICES,
        Some(service_payload(&group_a, "service-a")),
        StatusCode::CREATED,
    )
    .await;
    let service_b = expect_json(
        &app,
        Some(&cookie),
        "POST",
        SERVICES,
        Some(service_payload(&group_b, "service-b")),
        StatusCode::CREATED,
    )
    .await;
    let user = expect_json(&app, Some(&cookie), "POST", "/api/v1/admin/access-control/users", Some(json!({
        "username": name, "email": format!("{name}@test.local"), "status": "active", "auth_source": "local",
        "credential": {"mode": "manual", "password": REST_TEST_LOCAL_PASSWORD}
    })), StatusCode::OK).await;
    // Include CREATE deliberately: an object-scoped create action must not
    // authorize unrelated server-generated IDs or a bulk collection mutation.
    // Assignment updates also authorize the referenced interface, not just the member row.
    let resources = [&group_a, &assignment_a, &ip_a, &service_a, &interface]
        .map(|item| format!("inventory:{}", item["id"].as_str().expect("allowed id")));
    let policy = expect_json(&app, Some(&cookie), "POST", "/api/v1/admin/access-control/policies", Some(json!({
        "name": format!("{name}-policy"), "effect": "allow", "actions": ["READ", "CREATE", "UPDATE", "DELETE", "SECRET"],
        "resource_types": ["INVENTORY"], "subjects": [format!("user:{}", user["id"].as_str().expect("user id"))],
        "resources": resources, "enabled": true
    })), StatusCode::OK).await;
    let scoped_cookie = login_and_get_cookie(&app, &name, REST_TEST_LOCAL_PASSWORD)
        .await
        .expect("scoped user login");
    let mut group_update = group_payload(&format!("{name}-a-updated"));
    // The path, not the payload UUID, determines the object authorized and written.
    group_update["id"] = group_b["id"].clone();
    let mut assignment_update = assignment_payload(&group_a);
    assignment_update["priority"] = json!(200);
    assignment_update["id"] = assignment_b["id"].clone();
    for (base, allowed, other, update, create_payload) in [
        (
            GROUPS,
            &group_a,
            &group_b,
            group_update,
            group_payload(&format!("{name}-forbidden")),
        ),
        (
            ASSIGNMENTS,
            &assignment_a,
            &assignment_b,
            assignment_update,
            assignment_payload(&group_b),
        ),
    ] {
        let allowed_path = entity_path(base, allowed);
        let other_path = entity_path(base, other);
        assert_eq!(
            expect_json(
                &app,
                Some(&scoped_cookie),
                "GET",
                &allowed_path,
                None,
                StatusCode::OK
            )
            .await,
            *allowed
        );
        expect_json(
            &app,
            Some(&scoped_cookie),
            "GET",
            &other_path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
        let updated = expect_json(
            &app,
            Some(&scoped_cookie),
            "PUT",
            &allowed_path,
            Some(update),
            StatusCode::OK,
        )
        .await;
        assert_eq!(updated["id"], allowed["id"]);
        expect_json(
            &app,
            Some(&scoped_cookie),
            "PUT",
            &other_path,
            Some(create_payload.clone()),
            StatusCode::FORBIDDEN,
        )
        .await;
        expect_json(
            &app,
            Some(&scoped_cookie),
            "DELETE",
            &other_path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
        for path in [
            base.to_string(),
            format!(
                "{base}?search={}&limit=1",
                allowed["id"].as_str().expect("allowed id")
            ),
        ] {
            let denied = expect_json(
                &app,
                Some(&scoped_cookie),
                "GET",
                &path,
                None,
                StatusCode::FORBIDDEN,
            )
            .await;
            assert!(denied.get("items").is_none());
            assert!(denied.get("total").is_none());
        }
        expect_json(
            &app,
            Some(&scoped_cookie),
            "POST",
            base,
            Some(create_payload.clone()),
            StatusCode::FORBIDDEN,
        )
        .await;
        expect_json(
            &app,
            Some(&scoped_cookie),
            "POST",
            &format!("{base}/bulk"),
            Some(json!({"items": [create_payload]})),
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(
            expect_json(
                &app,
                Some(&cookie),
                "GET",
                &other_path,
                None,
                StatusCode::OK
            )
            .await,
            *other,
            "denied writes must leave B unchanged"
        );
    }
    let reveal_a = format!("{}/custom-fields/reveal", entity_path(GROUPS, &group_a));
    for (base, allowed, other, create_payload, has_bulk) in [
        (IPS, &ip_a, &ip_b, ip_payload(&group_a), false),
        (
            SERVICES,
            &service_a,
            &service_b,
            service_payload(&group_a, "forbidden-service"),
            true,
        ),
    ] {
        let allowed_path = entity_path(base, allowed);
        let other_path = entity_path(base, other);
        let fetched = expect_json(
            &app,
            Some(&scoped_cookie),
            "GET",
            &allowed_path,
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(fetched["id"], allowed["id"]);
        expect_json(
            &app,
            Some(&scoped_cookie),
            "GET",
            &other_path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
        let update = json!({"description": "scoped association update"});
        let updated = expect_json(
            &app,
            Some(&scoped_cookie),
            "PUT",
            &allowed_path,
            Some(update.clone()),
            StatusCode::OK,
        )
        .await;
        assert_eq!(updated["description"], "scoped association update");
        expect_json(
            &app,
            Some(&scoped_cookie),
            "PUT",
            &other_path,
            Some(update),
            StatusCode::FORBIDDEN,
        )
        .await;
        expect_json(
            &app,
            Some(&scoped_cookie),
            "DELETE",
            &other_path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
        let denied = expect_json(
            &app,
            Some(&scoped_cookie),
            "GET",
            base,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
        assert!(denied.get("items").is_none());
        assert!(denied.get("total").is_none());
        expect_json(
            &app,
            Some(&scoped_cookie),
            "POST",
            base,
            Some(create_payload.clone()),
            StatusCode::FORBIDDEN,
        )
        .await;
        if has_bulk {
            expect_json(
                &app,
                Some(&scoped_cookie),
                "POST",
                &format!("{base}/bulk"),
                Some(json!({"items": [create_payload]})),
                StatusCode::FORBIDDEN,
            )
            .await;
        }
        assert_eq!(
            expect_json(
                &app,
                Some(&cookie),
                "GET",
                &other_path,
                None,
                StatusCode::OK
            )
            .await,
            *other,
            "denied association writes must leave B unchanged"
        );
    }
    let reveal_b = format!("{}/custom-fields/reveal", entity_path(GROUPS, &group_b));
    expect_json(
        &app,
        Some(&scoped_cookie),
        "POST",
        &reveal_a,
        None,
        StatusCode::OK,
    )
    .await;
    expect_json(
        &app,
        Some(&scoped_cookie),
        "POST",
        &reveal_b,
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    // Remove inbound GFK references before the group. Object-level DELETE must
    // work for each A object without using a global grant.
    for (base, item) in [
        (SERVICES, &service_a),
        (IPS, &ip_a),
        (ASSIGNMENTS, &assignment_a),
    ] {
        expect_json(
            &app,
            Some(&scoped_cookie),
            "DELETE",
            &entity_path(base, item),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
    }
    expect_json(
        &app,
        Some(&scoped_cookie),
        "DELETE",
        &entity_path(GROUPS, &group_a),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    let groups = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!("{GROUPS}?search={name}"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        groups["total"], 1,
        "denied creates must not persist extra groups"
    );
    let assignments = expect_json(
        &app,
        Some(&cookie),
        "GET",
        &format!(
            "{ASSIGNMENTS}?search={}",
            interface["id"].as_str().expect("interface id")
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(assignments["total"], 1, "only B's assignment remains");
    for (base, item) in [
        (SERVICES, &service_b),
        (IPS, &ip_b),
        (ASSIGNMENTS, &assignment_b),
        (GROUPS, &group_b),
        ("/api/v1/inventory/interfaces", &interface),
    ] {
        expect_json(
            &app,
            Some(&cookie),
            "DELETE",
            &entity_path(base, item),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
    }
    for (base, item) in [
        ("/api/v1/admin/access-control/policies", &policy),
        ("/api/v1/admin/access-control/users", &user),
    ] {
        expect_json(
            &app,
            Some(&cookie),
            "DELETE",
            &entity_path(base, item),
            None,
            StatusCode::OK,
        )
        .await;
    }
}
