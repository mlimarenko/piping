//! PIP-211 catalog and multi-endpoint dataflow behavior over the real router.
//!
//! Every test owns a disposable PostgreSQL database and Redis logical database;
//! the suite never touches a shared dev database. Run with
//! `cargo test --test integration_dataflow_catalog -- --ignored`.
mod common;
#[path = "common/isolated_app.rs"]
mod isolated_app;

use axum::http::StatusCode;
use isolated_app::{Harness, harness, shutdown};
use piping_backend::{
    app::AppState,
    contexts::access_control::domain::{Action, ResourceType},
};
use serde_json::{Value, json};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const BASE: &str = "/api/v1/inventory/dataflows";
const ROLES: &str = "/api/v1/inventory/application-roles";
const APPLICATIONS: &str = "/api/v1/inventory/applications";
const GROUPS: &str = "/api/v1/inventory/dataflow-groups";
const ADDRESS_GROUPS: &str = "/api/v1/inventory/address-groups";
const TAGS: &str = "/api/v1/inventory/tags";
const OWNERS: &str = "/api/v1/inventory/owners";
const TENANTS: &str = "/api/v1/inventory/tenants";
const IP_RANGES: &str = "/api/v1/inventory/ip_ranges";
const CUSTOM_FIELDS: &str = "/api/v1/custom-fields";
const CONTENT_TYPES: &str = "/api/v1/inventory/content-types";
/// Synthetic cleartext for the secret-field assertions. It must never be
/// printed: leak checks use booleans with static messages.
const SECRET_SENTINEL: &str = "pip211-secret-sentinel-9f2c";

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> TestResult<(StatusCode, Value)> {
    let body = body.map(|value| value.to_string());
    let (status, bytes) =
        common::request_with_auth(app, method, path, body.as_deref(), cookie, None).await?;
    Ok((
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        },
    ))
}

async fn created(app: &axum::Router, path: &str, body: Value, cookie: &str) -> TestResult<Value> {
    let (status, response) = call(app, "POST", path, Some(body), Some(cookie)).await?;
    assert_eq!(status, StatusCode::CREATED, "{path}: {response}");
    Ok(response)
}

async fn detail(app: &axum::Router, path: &str, cookie: &str) -> TestResult<Value> {
    let (status, response) = call(app, "GET", path, None, Some(cookie)).await?;
    assert_eq!(status, StatusCode::OK, "{path}: {response}");
    Ok(response)
}

async fn expect_status(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: &str,
    expected: StatusCode,
) -> TestResult<Value> {
    let (status, response) = call(app, method, path, body, Some(cookie)).await?;
    assert_eq!(status, expected, "{method} {path}: {response}");
    Ok(response)
}

/// Rejection helper for guards that may answer 400 (validation) or 409 (conflict).
async fn expect_rejected(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: &str,
) -> TestResult<Value> {
    let (status, response) = call(app, method, path, body, Some(cookie)).await?;
    assert!(
        matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
        "{method} {path}: expected 400/409, got {status} {response}"
    );
    Ok(response)
}

fn id_of(value: &Value) -> TestResult<Uuid> {
    Ok(Uuid::parse_str(value["id"].as_str().ok_or("missing id")?)?)
}

/// A restricted delete must answer `DELETE_BLOCKED` and name the blocking table.
fn assert_delete_blocked(body: &Value, table: &str) {
    assert_eq!(
        body["code"], "DELETE_BLOCKED",
        "a restricted delete must identify its blockers"
    );
    let blockers = body["details"]["blockers"]
        .as_array()
        .expect("DELETE_BLOCKED must carry blocker details");
    assert!(
        blockers.iter().any(|blocker| blocker["table"] == table),
        "expected a {table} blocker"
    );
}

async fn ip(pool: &sqlx::PgPool, value: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, \
         comments) VALUES ($1, '{}', $2::inet, 'active', '', '', '')",
    )
    .bind(id)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn assign(pool: &sqlx::PgPool, host: Uuid, ids: &[Uuid]) -> Result<Uuid, sqlx::Error> {
    let interface = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dcim_interface (id, custom_field_data, device_id, name, _name, label, \
         description, type, enabled, mgmt_only, mark_connected) \
         VALUES ($1, '{}', $2, $3, $3, '', '', 'virtual', true, false, false)",
    )
    .bind(interface)
    .bind(host)
    .bind(format!("if-{interface}"))
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE ipam_ipaddress SET assigned_object_id = $2, assigned_object_type_id = \
         (SELECT id FROM inventory_content_type WHERE app_label = 'dcim' AND model = 'interface') \
         WHERE id = ANY($1)",
    )
    .bind(ids)
    .bind(interface)
    .execute(pool)
    .await?;
    Ok(interface)
}

async fn set_primary(pool: &sqlx::PgPool, host: Uuid, address: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE dcim_device SET primary_ip4_id = $2 WHERE id = $1")
        .bind(host)
        .bind(address)
        .execute(pool)
        .await?;
    Ok(())
}

async fn create_tag(
    app: &axum::Router,
    cookie: &str,
    name: &str,
    object_types: &[&str],
) -> TestResult<Uuid> {
    let tag = created(
        app,
        TAGS,
        json!({
            "name": name,
            "slug": name,
            "color": "334455",
            "object_types": object_types,
        }),
        cookie,
    )
    .await?;
    assert_eq!(tag["name"], name);
    assert_eq!(tag["object_types"], json!(object_types));
    id_of(&tag)
}

async fn create_application(app: &axum::Router, cookie: &str, name: &str) -> TestResult<Uuid> {
    let application = created(app, APPLICATIONS, json!({"name": name}), cookie).await?;
    assert_eq!(application["name"], name);
    id_of(&application)
}

async fn create_group(
    app: &axum::Router,
    cookie: &str,
    name: &str,
    parent_id: Option<Uuid>,
    application_id: Option<Uuid>,
    status: Option<&str>,
) -> TestResult<Uuid> {
    let mut payload = json!({"name": name});
    if let Some(parent_id) = parent_id {
        payload["parent_id"] = json!(parent_id);
    }
    if let Some(application_id) = application_id {
        payload["application_id"] = json!(application_id);
    }
    if let Some(status) = status {
        payload["status"] = json!(status);
    }
    let group = created(app, GROUPS, payload, cookie).await?;
    id_of(&group)
}

async fn create_flow(
    app: &axum::Router,
    cookie: &str,
    name: &str,
    group_id: Uuid,
    application_id: Uuid,
    status: &str,
) -> TestResult<Uuid> {
    let flow = created(
        app,
        BASE,
        json!({
            "name": name,
            "status": status,
            "sources": [{"object_type": "all"}],
            "destinations": [{"object_type": "internet"}],
            "group_id": group_id,
            "application_id": application_id,
        }),
        cookie,
    )
    .await?;
    id_of(&flow)
}

async fn policy(harness: &Harness, host_type: &str, host_id: Uuid) -> TestResult<Value> {
    detail(
        &harness.app,
        &format!("{BASE}/firewall-policy?host_type={host_type}&host_id={host_id}"),
        &harness.cookie,
    )
    .await
}

/// Resolve one registered inventory content type by its stable compact token.
async fn content_type_id(app: &axum::Router, cookie: &str, token: &str) -> TestResult<Uuid> {
    let items = detail(app, &format!("{CONTENT_TYPES}?app_label=inventory"), cookie).await?;
    let entry = items
        .as_array()
        .ok_or("content types must be an array")?
        .iter()
        .find(|item| item["token"] == token)
        .ok_or_else(|| format!("content type {token} is not registered"))?;
    assert_eq!(
        entry["supported_for_custom_fields"], true,
        "{token} must advertise custom-field support"
    );
    id_of(entry)
}

async fn define_custom_field(
    app: &axum::Router,
    cookie: &str,
    name: &str,
    field_type: &str,
    object_type: Uuid,
    required: bool,
    is_secret: bool,
    default_value: Option<&str>,
) -> TestResult<Uuid> {
    let mut payload = json!({
        "name": name,
        "label": name,
        "field_type": field_type,
        "required": required,
        "is_secret": is_secret,
        "object_types": [object_type],
    });
    if let Some(default_value) = default_value {
        payload["default_value"] = json!(default_value);
    }
    let definition = created(app, CUSTOM_FIELDS, payload, cookie).await?;
    assert_eq!(definition["name"], name);
    assert_eq!(definition["field_type"], field_type);
    assert_eq!(definition["is_secret"], is_secret);
    assert_eq!(definition["required"], required);
    assert_eq!(definition["object_types"], json!([object_type]));
    id_of(&definition)
}

/// Canonical secret binding for one entity's secret custom field. Only counts
/// and identities are read; the cleartext never leaves the secrets store here.
struct SecretBinding {
    count: i64,
    secret_id: Option<Uuid>,
    updated_at: Option<String>,
}

async fn secret_binding(pool: &sqlx::PgPool, entity: Uuid) -> TestResult<SecretBinding> {
    let bindings: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT secret_id, updated_at::text FROM secret_usage_binding \
         WHERE consumer_type = 'inventory_custom_field' AND consumer_id = $1 \
         AND field_key = 'credential' ORDER BY id",
    )
    .bind(entity)
    .fetch_all(pool)
    .await?;
    let count = i64::try_from(bindings.len())?;
    let (secret_id, updated_at) = bindings
        .into_iter()
        .next()
        .map_or((None, None), |(id, updated_at)| {
            (Some(id), Some(updated_at))
        });
    Ok(SecretBinding {
        count,
        secret_id,
        updated_at,
    })
}

/// A real user with an explicit Inventory action set, used to exercise the
/// bulk-action permission split.
async fn limited_cookie(
    state: &AppState,
    app: &axum::Router,
    actions: &[Action],
) -> TestResult<String> {
    let id = Uuid::now_v7();
    let username = format!("pip211-{}", common::unique_suffix());
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, status, auth_source) \
         VALUES ($1, $2, $3, 'active', 'local')",
    )
    .bind(id)
    .bind(&username)
    .bind(format!("{username}@test.local"))
    .execute(&state.db_pool)
    .await?;
    sqlx::query(
        "INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at) \
         SELECT $1, c.password_hash, now() FROM auth_user_credential c \
         JOIN identity_user u ON u.id = c.user_id WHERE u.username = $2",
    )
    .bind(id)
    .bind(common::TEST_ADMIN_USERNAME)
    .execute(&state.db_pool)
    .await?;
    sqlx::query(
        "INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) \
         VALUES ($1, 'allow', $2, $3, $4, '[]')",
    )
    .bind(format!("{username}-allow"))
    .bind(json!(actions))
    .bind(json!([ResourceType::Inventory]))
    .bind(json!([format!("user:{id}")]))
    .execute(&state.db_pool)
    .await?;
    common::login_and_get_cookie(app, &username, common::TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "limited user login failed".into())
}

/// A real user with optional Inventory/ConnectionTarget read grants and an
/// optional ConnectionTarget deny on one peer, mirroring the existing dataflow
/// suite's authorization fixture.
async fn user_cookie(
    state: &AppState,
    app: &axum::Router,
    inventory: bool,
    connection_target: bool,
    denied_peer: Option<Uuid>,
) -> TestResult<String> {
    let id = Uuid::now_v7();
    let username = format!("pip211-{}", common::unique_suffix());
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, status, auth_source) \
         VALUES ($1, $2, $3, 'active', 'local')",
    )
    .bind(id)
    .bind(&username)
    .bind(format!("{username}@test.local"))
    .execute(&state.db_pool)
    .await?;
    sqlx::query(
        "INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at) \
         SELECT $1, c.password_hash, now() FROM auth_user_credential c \
         JOIN identity_user u ON u.id = c.user_id WHERE u.username = $2",
    )
    .bind(id)
    .bind(common::TEST_ADMIN_USERNAME)
    .execute(&state.db_pool)
    .await?;
    let subjects = json!([format!("user:{id}")]);
    let mut resources = Vec::new();
    if inventory {
        resources.push(ResourceType::Inventory);
    }
    if connection_target {
        resources.push(ResourceType::ConnectionTarget);
    }
    if !resources.is_empty() {
        sqlx::query(
            "INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) \
             VALUES ($1, 'allow', $2, $3, $4, '[]')",
        )
        .bind(format!("{username}-allow"))
        .bind(json!([Action::Read]))
        .bind(json!(resources))
        .bind(&subjects)
        .execute(&state.db_pool)
        .await?;
    }
    if let Some(peer) = denied_peer {
        sqlx::query(
            "INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) \
             VALUES ($1, 'deny', $2, $3, $4, $5)",
        )
        .bind(format!("{username}-deny"))
        .bind(json!([Action::Read]))
        .bind(json!([ResourceType::ConnectionTarget]))
        .bind(&subjects)
        .bind(json!([format!("connection_target:{peer}")]))
        .execute(&state.db_pool)
        .await?;
    }
    common::login_and_get_cookie(app, &username, common::TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "visibility user login failed".into())
}

// ---------------------------------------------------------------------------
// Catalog CRUD, filters and restricted deletion
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn catalog_entities_lists_filters_and_restrict_deletion() -> TestResult {
    let harness = harness("pip211cat1", 11).await?;
    let result = catalog_entities(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn catalog_entities(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();
    let pool = &harness.state.db_pool;

    let tag_application = create_tag(app, cookie, "app-scope", &["inventory.application"]).await?;
    let tag_group = create_tag(app, cookie, "group-scope", &["inventory.dataflowgroup"]).await?;
    let tag_flow = create_tag(app, cookie, "flow-scope", &["inventory.dataflow"]).await?;
    let tag_device = create_tag(app, cookie, "device-scope", &["dcim.device"]).await?;

    let owner = created(app, OWNERS, json!({"name": "platform-team"}), cookie).await?;
    assert_eq!(owner["name"], "platform-team");
    let owner_id = id_of(&owner)?;
    let tenant = created(
        app,
        TENANTS,
        json!({"name": "tenant-alpha", "slug": "tenant-alpha"}),
        cookie,
    )
    .await?;
    let tenant_id = id_of(&tenant)?;

    // --- application role: create, detail, list order, filters ------------
    let role = created(
        app,
        ROLES,
        json!({"name": "edge-role", "description": "edge role", "comments": "role comments"}),
        cookie,
    )
    .await?;
    assert_eq!(role["name"], "edge-role");
    assert_eq!(role["description"], "edge role");
    assert_eq!(role["comments"], "role comments");
    assert_eq!(role["owner_id"], Value::Null);
    let role_id = id_of(&role)?;
    assert_eq!(
        detail(app, &format!("{ROLES}/{role_id}"), cookie).await?,
        role
    );
    for name in ["aaa-role", "zzz-role"] {
        created(app, ROLES, json!({"name": name}), cookie).await?;
    }
    let page = detail(app, &format!("{ROLES}?limit=2&offset=1"), cookie).await?;
    assert_eq!(page["total"], 3);
    assert_eq!(page["limit"], 2);
    assert_eq!(page["offset"], 1);
    let names: Vec<&str> = page["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["edge-role", "zzz-role"],
        "deterministic name order"
    );
    let searched = detail(app, &format!("{ROLES}?search=edge"), cookie).await?;
    assert_eq!(searched["total"], 1);
    assert_eq!(searched["items"][0]["id"], json!(role_id));

    // --- application: create carries every declared field -----------------
    let application = created(
        app,
        APPLICATIONS,
        json!({
            "name": "payments",
            "role_id": role_id,
            "owner_id": owner_id,
            "tenant_id": tenant_id,
            "description": "payments app",
            "comments": "app comments",
            "tags": [tag_application.to_string()],
        }),
        cookie,
    )
    .await?;
    assert_eq!(application["name"], "payments");
    assert_eq!(application["role_id"], json!(role_id));
    assert_eq!(application["owner_id"], json!(owner_id));
    assert_eq!(application["tenant_id"], json!(tenant_id));
    assert_eq!(application["description"], "payments app");
    assert_eq!(application["comments"], "app comments");
    assert_eq!(application["tags"], json!([tag_application.to_string()]));
    // No definitions are scoped to inventory.application in this fixture, so the
    // canonical custom-field owner materializes an empty map.
    assert_eq!(application["custom_field_data"], json!({}));
    let application_id = id_of(&application)?;
    assert_eq!(
        detail(app, &format!("{APPLICATIONS}/{application_id}"), cookie).await?,
        application
    );
    let page = detail(app, &format!("{APPLICATIONS}?limit=1&offset=0"), cookie).await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 0);
    assert_eq!(page["items"], json!([application]));
    for (query, total) in [
        ("search=payments".to_string(), 1),
        (format!("role_id={role_id}"), 1),
        (format!("role_id={}", Uuid::now_v7()), 0),
        (format!("tenant_id={tenant_id}"), 1),
        (format!("owner_id={owner_id}"), 1),
        (format!("tag_id={tag_application}"), 1),
        ("search=absent".to_string(), 0),
    ] {
        let page = detail(app, &format!("{APPLICATIONS}?{query}"), cookie).await?;
        assert_eq!(page["total"], total, "{query}");
    }

    created(app, APPLICATIONS, json!({"name": "without-role"}), cookie).await?;
    created(
        app,
        APPLICATIONS,
        json!({"name": "same-role", "role_id": role_id}),
        cookie,
    )
    .await?;
    let role_page = detail(
        app,
        &format!("{APPLICATIONS}?role_id={role_id}&limit=1&offset=1"),
        cookie,
    )
    .await?;
    assert_eq!(
        role_page["total"], 2,
        "role filter counts the whole matching collection"
    );
    assert_eq!(role_page["items"][0]["name"], "same-role");
    expect_status(
        app,
        "GET",
        &format!("{APPLICATIONS}?role_id=invalid"),
        None,
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    // --- hierarchical flow groups -----------------------------------------
    let root = created(
        app,
        GROUPS,
        json!({
            "name": "core",
            "application_id": application_id,
            "description": "root group",
            "comments": "root comments",
            "tags": [tag_group.to_string()],
        }),
        cookie,
    )
    .await?;
    assert_eq!(root["status"], "active", "new groups default to active");
    assert_eq!(root["effective_status"], "active");
    assert_eq!(root["blocked_by_group_id"], Value::Null);
    assert_eq!(root["parent_id"], Value::Null);
    assert_eq!(root["application_id"], json!(application_id));
    assert_eq!(root["tags"], json!([tag_group.to_string()]));
    assert_eq!(root["inherited_tags"], json!([]));
    assert_eq!(root["effective_tags"], json!([tag_group.to_string()]));
    let root_id = id_of(&root)?;
    assert_eq!(
        detail(app, &format!("{GROUPS}/{root_id}"), cookie).await?,
        root
    );

    let child = created(
        app,
        GROUPS,
        json!({"name": "edge", "parent_id": root_id, "application_id": application_id}),
        cookie,
    )
    .await?;
    assert_eq!(child["parent_id"], json!(root_id));
    assert_eq!(child["status"], "active");
    assert_eq!(child["effective_status"], "active");
    assert_eq!(child["inherited_tags"], json!([tag_group.to_string()]));
    assert_eq!(child["effective_tags"], json!([tag_group.to_string()]));
    let child_id = id_of(&child)?;

    let page = detail(app, &format!("{GROUPS}?limit=10"), cookie).await?;
    assert_eq!(page["total"], 2);
    let names: Vec<&str> = page["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, vec!["core", "edge"]);
    let roots = detail(app, &format!("{GROUPS}?root=true"), cookie).await?;
    assert_eq!(roots["total"], 1);
    assert_eq!(roots["items"][0]["id"], json!(root_id));
    let children = detail(app, &format!("{GROUPS}?parent_id={root_id}"), cookie).await?;
    assert_eq!(children["total"], 1);
    assert_eq!(children["items"][0]["id"], json!(child_id));
    let by_application = detail(
        app,
        &format!("{GROUPS}?application_id={application_id}"),
        cookie,
    )
    .await?;
    assert_eq!(by_application["total"], 2);
    let by_tag = detail(app, &format!("{GROUPS}?tag_id={tag_group}"), cookie).await?;
    assert_eq!(by_tag["total"], 1, "only the root carries the group tag");
    let by_status = detail(app, &format!("{GROUPS}?status=active"), cookie).await?;
    assert_eq!(by_status["total"], 2);
    expect_status(
        app,
        "GET",
        &format!("{GROUPS}?root=true&parent_id={root_id}"),
        None,
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    // --- address group with ordered typed members -------------------------
    let (device, device_name) = common::seed_device(pool, "pip211cat1").await?;
    let selected_ip = ip(pool, "192.0.2.31/24").await?;
    assign(pool, device, &[selected_ip]).await?;
    set_primary(pool, device, selected_ip).await?;
    let range = created(
        app,
        IP_RANGES,
        json!({
            "start_address": "198.51.100.20",
            "end_address": "198.51.100.22",
            "status": "active",
            "description": "",
        }),
        cookie,
    )
    .await?;
    let range_id = id_of(&range)?;
    assert_eq!(range["start_address"], "198.51.100.20/32");
    assert_eq!(range["end_address"], "198.51.100.22/32");
    sqlx::query("UPDATE dcim_device SET tags = ARRAY[$2] WHERE id = $1")
        .bind(device)
        .bind(tag_device.to_string())
        .execute(pool)
        .await?;

    let address_group = created(
        app,
        ADDRESS_GROUPS,
        json!({
            "name": "edge-addresses",
            "description": "address group",
            "comments": "ag comments",
            "members": [
                {
                    "object_type": "dcim.device",
                    "object_id": device,
                    "address_mode": "selected",
                    "ipaddresses": [selected_ip],
                },
                {"object_type": "ipam.iprange", "object_id": range_id},
                {
                    "object_type": "extras.tag",
                    "object_id": tag_device,
                    "tag_object_type": "dcim.device",
                },
            ],
        }),
        cookie,
    )
    .await?;
    assert_eq!(address_group["name"], "edge-addresses");
    assert_eq!(address_group["description"], "address group");
    assert_eq!(address_group["comments"], "ag comments");
    assert_eq!(address_group["owner_id"], Value::Null);
    assert_eq!(address_group["tenant_id"], Value::Null);
    assert_eq!(address_group["tags"], json!([]));
    assert_eq!(address_group["custom_field_data"], json!({}));
    let members = address_group["members"].as_array().ok_or("members")?;
    assert_eq!(members.len(), 3);
    assert_eq!(members[0]["position"], 1);
    assert_eq!(members[0]["object_type"], "dcim.device");
    assert_eq!(members[0]["object_id"], json!(device));
    assert_eq!(members[0]["address_mode"], "selected");
    assert_eq!(members[0]["ipaddresses"], json!([selected_ip]));
    assert_eq!(members[0]["label"], json!(device_name));
    assert!(members[0].get("tag_object_type").is_none());
    assert_eq!(members[1]["position"], 2);
    assert_eq!(members[1]["object_type"], "ipam.iprange");
    assert_eq!(members[1]["object_id"], json!(range_id));
    assert_eq!(members[1]["address_mode"], "default");
    assert_eq!(members[1]["ipaddresses"], json!([]));
    let range_label = members[1]["label"].as_str().ok_or("range member label")?;
    assert!(
        range_label.starts_with("198.51.100.20") && range_label.ends_with("198.51.100.22"),
        "range member label must render both endpoints"
    );
    assert_eq!(members[2]["position"], 3);
    assert_eq!(members[2]["object_type"], "extras.tag");
    assert_eq!(members[2]["object_id"], json!(tag_device));
    assert_eq!(members[2]["tag_object_type"], "dcim.device");
    assert_eq!(members[2]["label"], json!("device-scope"));
    let address_group_id = id_of(&address_group)?;
    assert_eq!(
        detail(app, &format!("{ADDRESS_GROUPS}/{address_group_id}"), cookie).await?,
        address_group
    );
    let page = detail(
        app,
        &format!("{ADDRESS_GROUPS}?search=edge-addresses"),
        cookie,
    )
    .await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"], json!([address_group]));

    // --- restricted deletion ----------------------------------------------
    let blocked = expect_status(
        app,
        "DELETE",
        &format!("{ROLES}/{role_id}"),
        None,
        cookie,
        StatusCode::CONFLICT,
    )
    .await?;
    assert_delete_blocked(&blocked, "inventory_application");
    let blocked = expect_status(
        app,
        "DELETE",
        &format!("{APPLICATIONS}/{application_id}"),
        None,
        cookie,
        StatusCode::CONFLICT,
    )
    .await?;
    assert_delete_blocked(&blocked, "inventory_dataflow_group");
    let blocked = expect_status(
        app,
        "DELETE",
        &format!("{GROUPS}/{root_id}"),
        None,
        cookie,
        StatusCode::CONFLICT,
    )
    .await?;
    assert_delete_blocked(&blocked, "inventory_dataflow_group");

    let flow = created(
        app,
        BASE,
        json!({
            "name": "address-group-flow",
            "sources": [{"object_type": "inventory.addressgroup", "object_id": address_group_id}],
            "destinations": [{"object_type": "internet"}],
        }),
        cookie,
    )
    .await?;
    let flow_id = id_of(&flow)?;
    let blocked = expect_status(
        app,
        "DELETE",
        &format!("{ADDRESS_GROUPS}/{address_group_id}"),
        None,
        cookie,
        StatusCode::CONFLICT,
    )
    .await?;
    // The blocker is a flow selector (`inventory_dataflow_selector`), which is
    // not a registered content type, so the guard cannot load its attributes and
    // withholds `details` behind a private CONFLICT. A DELETE_BLOCKED body must
    // still name the selector blocker; both shapes keep the delete closed.
    match blocked["code"].as_str() {
        Some("DELETE_BLOCKED") => {
            let blockers = blocked["details"]["blockers"]
                .as_array()
                .ok_or("blocker details")?;
            assert!(
                blockers
                    .iter()
                    .any(|blocker| blocker["table"] == "inventory_dataflow_selector"),
                "{blocked}"
            );
        }
        Some("CONFLICT") => {}
        other => panic!("unexpected delete-blocked shape {other:?}: {blocked}"),
    }
    // The blocked delete must not remove the address group.
    detail(app, &format!("{ADDRESS_GROUPS}/{address_group_id}"), cookie).await?;
    expect_status(
        app,
        "DELETE",
        &format!("{BASE}/{flow_id}"),
        None,
        cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;
    expect_status(
        app,
        "DELETE",
        &format!("{ADDRESS_GROUPS}/{address_group_id}"),
        None,
        cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;

    let spare_role = created(app, ROLES, json!({"name": "spare-role"}), cookie).await?;
    expect_status(
        app,
        "DELETE",
        &format!("{ROLES}/{}", id_of(&spare_role)?),
        None,
        cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;
    let spare_group =
        create_group(app, cookie, "spare-group", None, Some(application_id), None).await?;
    expect_status(
        app,
        "DELETE",
        &format!("{GROUPS}/{spare_group}"),
        None,
        cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;
    let spare_application = create_application(app, cookie, "spare-app").await?;
    expect_status(
        app,
        "DELETE",
        &format!("{APPLICATIONS}/{spare_application}"),
        None,
        cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;
    // The flow tag scope is registered even though no flow used it here.
    let tag_page = detail(app, &format!("{TAGS}?ids={tag_flow}"), cookie).await?;
    assert_eq!(tag_page["total"], 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-endpoint flows: derived projection, validation and tag filters
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn flow_arrays_projection_validation_and_tag_filters() -> TestResult {
    let harness = harness("pip211cat2", 12).await?;
    let result = flows_and_tags(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn flows_and_tags(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();
    let pool = &harness.state.db_pool;

    let application_id = create_application(app, cookie, "flow-app").await?;
    let group_id =
        create_group(app, cookie, "flow-group", None, Some(application_id), None).await?;
    let tag_device = create_tag(app, cookie, "flow-device-tag", &["dcim.device"]).await?;
    let tag_vm = create_tag(
        app,
        cookie,
        "flow-vm-tag",
        &["virtualization.virtualmachine"],
    )
    .await?;
    let tag_flow = create_tag(app, cookie, "flow-direct-tag", &["inventory.dataflow"]).await?;

    let (a, a_name) = common::seed_device(pool, "pip211cat2").await?;
    let (b, b_name) = common::seed_device(pool, "pip211cat2").await?;
    let a_ip = ip(pool, "192.0.2.41/24").await?;
    assign(pool, a, &[a_ip]).await?;
    set_primary(pool, a, a_ip).await?;
    let b_ip = ip(pool, "192.0.2.42/24").await?;
    assign(pool, b, &[b_ip]).await?;
    set_primary(pool, b, b_ip).await?;
    let range = created(
        app,
        IP_RANGES,
        json!({
            "start_address": "198.51.100.20",
            "end_address": "198.51.100.22",
            "status": "active",
            "description": "",
        }),
        cookie,
    )
    .await?;
    let range_id = id_of(&range)?;
    let address_group = created(
        app,
        ADDRESS_GROUPS,
        json!({
            "name": "flow-addresses",
            "members": [{"object_type": "dcim.device", "object_id": a}],
        }),
        cookie,
    )
    .await?;
    let address_group_id = id_of(&address_group)?;

    let payload = json!({
        "name": "web-to-db",
        "description": "web tier to database",
        "comments": "change 211",
        "status": "active",
        "sources": [
            {"object_type": "dcim.device", "object_id": a},
            {
                "object_type": "extras.tag",
                "object_id": tag_device,
                "tag_object_type": "dcim.device",
            },
        ],
        "destinations": [
            {"object_type": "dcim.device", "object_id": b, "address_mode": "all"},
            {"object_type": "ipam.iprange", "object_id": range_id},
        ],
        "transport": "tcp",
        "source_ports": [1025, 1024],
        "destination_ports": [443],
        "group_id": group_id,
        "application_id": application_id,
        "tags": [tag_flow.to_string()],
    });
    let flow = created(app, BASE, payload.clone(), cookie).await?;
    assert_eq!(flow["name"], "web-to-db");
    assert_eq!(flow["description"], "web tier to database");
    assert_eq!(flow["comments"], "change 211");
    assert_eq!(flow["status"], "active");
    assert_eq!(flow["effective_status"], "active");
    assert_eq!(flow["blocked_by_group_id"], Value::Null);
    assert_eq!(flow["transport"], "tcp");
    assert_eq!(flow["source_ports"], json!([1024, 1025]));
    assert_eq!(flow["destination_ports"], json!([443]));
    assert_eq!(flow["icmp_types"], Value::Null);
    assert_eq!(flow["allow_cross_vrf"], false);
    assert_eq!(flow["group_id"], json!(group_id));
    assert_eq!(flow["application_id"], json!(application_id));
    assert_eq!(flow["tags"], json!([tag_flow.to_string()]));
    assert_eq!(flow["inherited_tags"], json!([]));
    assert_eq!(flow["effective_tags"], json!([tag_flow.to_string()]));
    // No definitions are scoped to inventory.dataflow in this fixture.
    assert_eq!(flow["custom_field_data"], json!({}));

    let sources = flow["sources"].as_array().ok_or("sources")?;
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0]["object_type"], "dcim.device");
    assert_eq!(sources[0]["object_id"], json!(a));
    assert_eq!(sources[0]["address_mode"], "default");
    assert_eq!(sources[0]["ipaddresses"], json!([]));
    assert_eq!(sources[0]["label"], json!(a_name));
    assert!(sources[0].get("tag_object_type").is_none());
    assert_eq!(sources[1]["object_type"], "extras.tag");
    assert_eq!(sources[1]["object_id"], json!(tag_device));
    assert_eq!(sources[1]["tag_object_type"], "dcim.device");
    assert_eq!(sources[1]["label"], json!("flow-device-tag"));

    let destinations = flow["destinations"].as_array().ok_or("destinations")?;
    assert_eq!(destinations.len(), 2);
    assert_eq!(destinations[0]["object_type"], "dcim.device");
    assert_eq!(destinations[0]["address_mode"], "all");
    assert_eq!(destinations[0]["label"], json!(b_name));
    assert_eq!(destinations[1]["object_type"], "ipam.iprange");
    assert_eq!(destinations[1]["object_id"], json!(range_id));
    let range_label = destinations[1]["label"].as_str().ok_or("range label")?;
    assert!(
        range_label.starts_with("198.51.100.20") && range_label.ends_with("198.51.100.22"),
        "{range_label}"
    );

    let flow_id = id_of(&flow)?;
    assert_eq!(
        detail(app, &format!("{BASE}/{flow_id}"), cookie).await?,
        flow
    );

    for (query, total) in [
        ("search=web-to-db".to_string(), 1),
        ("status=active".to_string(), 1),
        ("effective_status=active".to_string(), 1),
        (format!("group_id={group_id}"), 1),
        (format!("application_id={application_id}"), 1),
        (format!("tag_id={tag_flow}"), 1),
        ("transport=tcp".to_string(), 1),
        (format!("source_type=dcim.device&source_id={a}"), 1),
        (
            format!("destination_type=ipam.iprange&destination_id={range_id}"),
            1,
        ),
        ("search=absent".to_string(), 0),
    ] {
        let page = detail(app, &format!("{BASE}?{query}&limit=100"), cookie).await?;
        assert_eq!(page["total"], total, "{query}");
    }

    // --- rejections -------------------------------------------------------
    let mut invalid = payload.clone();
    invalid
        .as_object_mut()
        .ok_or("payload object")?
        .remove("sources");
    invalid["source"] = json!({"object_type": "dcim.device", "object_id": a});
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["sources"] = json!([]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["sources"] = Value::Array(vec![
        json!({"object_type": "dcim.device", "object_id": a});
        33
    ]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["sources"] = json!([{"object_type": "extras.tag", "object_id": tag_device}]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["sources"] = json!([{
        "object_type": "extras.tag",
        "object_id": tag_vm,
        "tag_object_type": "virtualization.virtualmachine",
        "address_mode": "oob",
    }]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["sources"] = json!([{
        "object_type": "extras.tag",
        "object_id": tag_device,
        "tag_object_type": "dcim.device",
        "address_mode": "selected",
        "ipaddresses": [a_ip],
    }]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["sources"] = json!([{
        "object_type": "ipam.iprange",
        "object_id": range_id,
        "address_mode": "selected",
        "ipaddresses": [a_ip],
    }]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["sources"] = json!([{
        "object_type": "inventory.addressgroup",
        "object_id": address_group_id,
        "address_mode": "selected",
        "ipaddresses": [a_ip],
    }]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    for ports in [json!([0]), json!([65536])] {
        let mut invalid = payload.clone();
        invalid["source_ports"] = ports;
        expect_status(
            app,
            "POST",
            BASE,
            Some(invalid),
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }

    let mut invalid = payload.clone();
    invalid["transport"] = json!("icmp");
    invalid["icmp_types"] = json!([256]);
    invalid["source_ports"] = Value::Null;
    invalid["destination_ports"] = Value::Null;
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let mut invalid = payload.clone();
    invalid["transport"] = json!("icmp");
    invalid["source_ports"] = Value::Null;
    invalid["destination_ports"] = json!([443]);
    expect_status(
        app,
        "POST",
        BASE,
        Some(invalid),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let page = detail(app, &format!("{BASE}?search=web-to-db"), cookie).await?;
    assert_eq!(page["total"], 1, "rejected payloads must not persist");

    // --- tag scope filters ------------------------------------------------
    // A dedicated tag keeps the filter fixture free of the main flow above.
    let tag_scope = create_tag(app, cookie, "scope-filter-tag", &["dcim.device"]).await?;
    let device_flow = created(
        app,
        BASE,
        json!({
            "name": "tag-device-flow",
            "sources": [{
                "object_type": "extras.tag",
                "object_id": tag_scope,
                "tag_object_type": "dcim.device",
            }],
            "destinations": [{"object_type": "internet"}],
        }),
        cookie,
    )
    .await?;
    let device_flow_id = id_of(&device_flow)?;
    let vm_flow = created(
        app,
        BASE,
        json!({
            "name": "tag-vm-flow",
            "sources": [{
                "object_type": "extras.tag",
                "object_id": tag_scope,
                "tag_object_type": "virtualization.virtualmachine",
            }],
            "destinations": [{"object_type": "internet"}],
        }),
        cookie,
    )
    .await?;
    let vm_flow_id = id_of(&vm_flow)?;

    let both = detail(
        app,
        &format!("{BASE}?source_type=extras.tag&source_id={tag_scope}&limit=100"),
        cookie,
    )
    .await?;
    assert_eq!(both["total"], 2, "omitted scope matches both host kinds");
    let device_only = detail(
        app,
        &format!(
            "{BASE}?source_type=extras.tag&source_id={tag_scope}\
             &source_tag_object_type=dcim.device&limit=100"
        ),
        cookie,
    )
    .await?;
    assert_eq!(device_only["total"], 1);
    assert_eq!(device_only["items"][0]["id"], json!(device_flow_id));
    let vm_only = detail(
        app,
        &format!(
            "{BASE}?source_type=extras.tag&source_id={tag_scope}\
             &source_tag_object_type=virtualization.virtualmachine&limit=100"
        ),
        cookie,
    )
    .await?;
    assert_eq!(vm_only["total"], 1);
    assert_eq!(vm_only["items"][0]["id"], json!(vm_flow_id));

    for query in [
        "source_tag_object_type=dcim.device".to_string(),
        format!("source_type=dcim.device&source_id={a}&source_tag_object_type=dcim.device"),
        format!("source_type=extras.tag&source_id={tag_scope}&source_tag_object_type=ipam.prefix"),
        "source_type=extras.tag".to_string(),
    ] {
        expect_status(
            app,
            "GET",
            &format!("{BASE}?{query}"),
            None,
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Contract wire spelling for the combined transport
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn flow_tcp_udp_wire_spelling_round_trips() -> TestResult {
    let harness = harness("pip211cat6", 10).await?;
    let result = tcp_udp_round_trip(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn tcp_udp_round_trip(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();
    // The contract's single wire spelling is `tcp_udp`; the DB CHECK,
    // `as_str()` and the web client agree.
    let flow = created(
        app,
        BASE,
        json!({
            "name": "tcp-udp-flow",
            "status": "draft",
            "sources": [{"object_type": "all"}],
            "destinations": [{"object_type": "internet"}],
            "transport": "tcp_udp",
            "source_ports": [1024],
            "destination_ports": [443],
        }),
        cookie,
    )
    .await?;
    assert_eq!(flow["transport"], "tcp_udp");
    let flow_id = id_of(&flow)?;
    assert_eq!(
        detail(app, &format!("{BASE}/{flow_id}"), cookie).await?,
        flow
    );
    let page = detail(app, &format!("{BASE}?transport=tcp_udp&limit=100"), cookie).await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], json!(flow_id));
    Ok(())
}

// ---------------------------------------------------------------------------
// Custom fields: scope validation, masking and secret storage separation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn custom_fields_validate_scope_and_keep_secrets_out_of_entity_json() -> TestResult {
    let harness = harness("pip211cat7", 9).await?;
    let result = custom_field_behavior(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn custom_field_behavior(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();
    let pool = &harness.state.db_pool;

    // --- definitions scoped to the four catalog content types -------------
    let application_type = content_type_id(app, cookie, "inventory.application").await?;
    let group_type = content_type_id(app, cookie, "inventory.dataflowgroup").await?;
    let address_group_type = content_type_id(app, cookie, "inventory.addressgroup").await?;
    let dataflow_type = content_type_id(app, cookie, "inventory.dataflow").await?;
    // ApplicationRole is a real content type but has no custom-field surface.
    let items = detail(app, &format!("{CONTENT_TYPES}?app_label=inventory"), cookie).await?;
    let role_type = items
        .as_array()
        .ok_or("content types must be an array")?
        .iter()
        .find(|item| item["token"] == "inventory.applicationrole")
        .ok_or("application role content type is not registered")?;
    assert_eq!(role_type["supported_for_custom_fields"], false);
    id_of(role_type)?;

    define_custom_field(
        app,
        cookie,
        "tier",
        "text",
        application_type,
        false,
        false,
        Some("bronze"),
    )
    .await?;
    define_custom_field(
        app,
        cookie,
        "required_note",
        "text",
        application_type,
        true,
        false,
        None,
    )
    .await?;
    define_custom_field(
        app,
        cookie,
        "credential",
        "text",
        application_type,
        false,
        true,
        None,
    )
    .await?;
    define_custom_field(
        app,
        cookie,
        "cost_center",
        "text",
        group_type,
        false,
        false,
        None,
    )
    .await?;
    define_custom_field(
        app,
        cookie,
        "site_code",
        "text",
        address_group_type,
        false,
        false,
        None,
    )
    .await?;
    define_custom_field(
        app,
        cookie,
        "owner_note",
        "text",
        dataflow_type,
        false,
        false,
        None,
    )
    .await?;

    // --- create rejects unknown, out-of-scope, missing and wrong-typed ----
    for (name, custom_field_data) in [
        ("cf-unknown", json!({"nope": "x", "required_note": "ok"})),
        (
            "cf-out-of-scope",
            json!({"cost_center": "cc-0", "required_note": "ok"}),
        ),
        ("cf-missing-required", json!({"tier": "gold"})),
        ("cf-wrong-type", json!({"tier": 42, "required_note": "ok"})),
    ] {
        expect_status(
            app,
            "POST",
            APPLICATIONS,
            Some(json!({"name": name, "custom_field_data": custom_field_data})),
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let page = detail(app, &format!("{APPLICATIONS}?search={name}"), cookie).await?;
        assert_eq!(
            page["total"], 0,
            "a rejected application ({name}) must not persist"
        );
    }

    // --- non-secret round-trip, omission, replacement and clear -----------
    let application = created(
        app,
        APPLICATIONS,
        json!({"name": "cf-application", "custom_field_data": {"required_note": "ok"}}),
        cookie,
    )
    .await?;
    let application_id = id_of(&application)?;
    assert_eq!(
        application["custom_field_data"]["tier"], "bronze",
        "an omitted field surfaces its definition default"
    );
    assert_eq!(application["custom_field_data"]["required_note"], "ok");
    assert!(
        application["custom_field_data"]["credential"].is_null(),
        "an unset secret has no value"
    );

    let updated = expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{application_id}"),
        Some(json!({"custom_field_data": {"tier": "gold", "required_note": "ok"}})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(updated["custom_field_data"]["tier"], "gold");

    // Omitting custom_field_data on update preserves the stored map.
    let updated = expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{application_id}"),
        Some(json!({"description": "omitted custom fields"})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        updated["custom_field_data"]["tier"], "gold",
        "omission preserves the stored value"
    );
    assert_eq!(updated["custom_field_data"]["required_note"], "ok");

    // An explicit null clears and outranks the definition default.
    let updated = expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{application_id}"),
        Some(json!({"custom_field_data": {"tier": null}})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert!(
        updated["custom_field_data"]["tier"].is_null(),
        "an explicit null must not fall back to the default"
    );
    assert!(
        updated["custom_field_data"]["required_note"].is_null(),
        "supplying a custom-field map replaces the stored keys"
    );

    let updated = expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{application_id}"),
        Some(json!({"custom_field_data": {"tier": "silver"}})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(updated["custom_field_data"]["tier"], "silver");
    assert_eq!(
        detail(app, &format!("{APPLICATIONS}/{application_id}"), cookie).await?,
        updated
    );

    // --- the same owner is wired for groups, address groups and flows -----
    let group = created(
        app,
        GROUPS,
        json!({"name": "cf-group", "custom_field_data": {"cost_center": "cc-1"}}),
        cookie,
    )
    .await?;
    assert_eq!(group["custom_field_data"]["cost_center"], "cc-1");

    let (device, device_name) = common::seed_device(pool, "pip211cat7").await?;
    let address_group = created(
        app,
        ADDRESS_GROUPS,
        json!({
            "name": "cf-addresses",
            "custom_field_data": {"site_code": "S1"},
            "members": [{"object_type": "dcim.device", "object_id": device}],
        }),
        cookie,
    )
    .await?;
    assert_eq!(address_group["custom_field_data"]["site_code"], "S1");
    assert_eq!(address_group["members"][0]["label"], json!(device_name));

    let flow = created(
        app,
        BASE,
        json!({
            "name": "cf-flow",
            "sources": [{"object_type": "all"}],
            "destinations": [{"object_type": "internet"}],
            "custom_field_data": {"owner_note": "note-1"},
        }),
        cookie,
    )
    .await?;
    assert_eq!(flow["custom_field_data"]["owner_note"], "note-1");

    // --- secret field: masked responses, separated storage ----------------
    let secret_application = created(
        app,
        APPLICATIONS,
        json!({
            "name": "cf-secret-application",
            "custom_field_data": {"required_note": "ok", "credential": SECRET_SENTINEL},
        }),
        cookie,
    )
    .await?;
    let secret_entity = id_of(&secret_application)?;
    assert_eq!(
        secret_application["custom_field_data"]["credential"], "****",
        "a set secret is masked"
    );
    assert!(
        !serde_json::to_string(&secret_application)?.contains(SECRET_SENTINEL),
        "secret cleartext leaked into the create response"
    );

    let key_stored: bool = sqlx::query_scalar(
        "SELECT custom_field_data ? 'credential' FROM inventory_application WHERE id = $1",
    )
    .bind(secret_entity)
    .fetch_one(pool)
    .await?;
    assert!(
        !key_stored,
        "the secret key must be stripped from the entity JSONB"
    );
    let value_stored: bool = sqlx::query_scalar(
        "SELECT custom_field_data::text LIKE '%' || $2 || '%' \
         FROM inventory_application WHERE id = $1",
    )
    .bind(secret_entity)
    .bind(SECRET_SENTINEL)
    .fetch_one(pool)
    .await?;
    assert!(
        !value_stored,
        "secret cleartext must not be stored on the entity row"
    );

    let first = secret_binding(pool, secret_entity).await?;
    assert_eq!(first.count, 1, "a secret write creates one usage binding");
    let first_secret = first.secret_id.ok_or("secret binding id")?;

    let read = detail(app, &format!("{APPLICATIONS}/{secret_entity}"), cookie).await?;
    assert!(
        !serde_json::to_string(&read)?.contains(SECRET_SENTINEL),
        "secret cleartext leaked into the detail response"
    );
    let page = detail(
        app,
        &format!("{APPLICATIONS}?search=cf-secret-application"),
        cookie,
    )
    .await?;
    assert!(
        !serde_json::to_string(&page)?.contains(SECRET_SENTINEL),
        "secret cleartext leaked into the list response"
    );
    assert_eq!(page["items"][0]["custom_field_data"]["credential"], "****");

    // Omitting custom fields keeps the binding untouched.
    expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{secret_entity}"),
        Some(json!({"description": "retain"})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(secret_binding(pool, secret_entity).await?.count, 1);

    // A masked round-trip means "keep existing", never "overwrite with ****".
    expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{secret_entity}"),
        Some(json!({"custom_field_data": {"credential": "****"}})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    let retained = secret_binding(pool, secret_entity).await?;
    assert_eq!(retained.count, 1);
    assert_eq!(
        retained.secret_id,
        Some(first_secret),
        "a mask round-trip keeps the secret identity"
    );

    // A blank value clears the binding.
    expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{secret_entity}"),
        Some(json!({"custom_field_data": {"credential": ""}})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        secret_binding(pool, secret_entity).await?.count,
        0,
        "a blank secret clears the usage binding"
    );
    let read = detail(app, &format!("{APPLICATIONS}/{secret_entity}"), cookie).await?;
    assert!(read["custom_field_data"]["credential"].is_null());

    // --- failure injection: a rejected update must not touch the binding --
    expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{secret_entity}"),
        Some(json!({"custom_field_data": {"credential": SECRET_SENTINEL}})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    let before = secret_binding(pool, secret_entity).await?;
    assert_eq!(before.count, 1);
    let before_json: String = sqlx::query_scalar(
        "SELECT custom_field_data::text FROM inventory_application WHERE id = $1",
    )
    .bind(secret_entity)
    .fetch_one(pool)
    .await?;

    expect_status(
        app,
        "PUT",
        &format!("{APPLICATIONS}/{secret_entity}"),
        Some(json!({
            "custom_field_data": {"credential": "replacement-value", "tier": 123},
        })),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let after = secret_binding(pool, secret_entity).await?;
    assert_eq!(
        after.count, before.count,
        "a rejected update must not add a secret binding"
    );
    assert_eq!(
        after.secret_id, before.secret_id,
        "a rejected update must not replace the stored secret"
    );
    assert_eq!(
        after.updated_at, before.updated_at,
        "a rejected update must not rewrite the binding"
    );
    let after_json: String = sqlx::query_scalar(
        "SELECT custom_field_data::text FROM inventory_application WHERE id = $1",
    )
    .bind(secret_entity)
    .fetch_one(pool)
    .await?;
    assert!(
        before_json == after_json,
        "a rejected update must not rewrite entity custom fields"
    );

    // --- ApplicationRole has no custom-field payload ----------------------
    expect_status(
        app,
        "POST",
        ROLES,
        Some(json!({"name": "cf-role", "custom_field_data": {"tier": "gold"}})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let page = detail(app, &format!("{ROLES}?search=cf-role"), cookie).await?;
    assert_eq!(
        page["total"], 0,
        "an application role must reject custom_field_data"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Hierarchy: effective status, nearest blocker and application guards
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn group_hierarchy_effective_status_and_application_guards() -> TestResult {
    let harness = harness("pip211cat3", 13).await?;
    let result = hierarchy(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn hierarchy(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();

    let app_a = create_application(app, cookie, "hierarchy-a").await?;
    let app_b = create_application(app, cookie, "hierarchy-b").await?;

    let root = create_group(app, cookie, "root", None, Some(app_a), None).await?;
    let child = create_group(app, cookie, "child", Some(root), Some(app_a), Some("draft")).await?;
    let grandchild = create_group(
        app,
        cookie,
        "grandchild",
        Some(child),
        Some(app_a),
        Some("disabled"),
    )
    .await?;

    let root_group = detail(app, &format!("{GROUPS}/{root}"), cookie).await?;
    assert_eq!(root_group["effective_status"], "active");
    assert_eq!(root_group["blocked_by_group_id"], Value::Null);
    let child_group = detail(app, &format!("{GROUPS}/{child}"), cookie).await?;
    assert_eq!(child_group["effective_status"], "draft");
    assert_eq!(child_group["blocked_by_group_id"], Value::Null);
    let grandchild_group = detail(app, &format!("{GROUPS}/{grandchild}"), cookie).await?;
    assert_eq!(grandchild_group["effective_status"], "disabled");
    assert_eq!(
        grandchild_group["blocked_by_group_id"],
        Value::Null,
        "the row's own status explains its effective state"
    );

    let root2 = create_group(app, cookie, "root2", None, Some(app_a), Some("disabled")).await?;
    let mid = create_group(app, cookie, "mid", Some(root2), Some(app_a), None).await?;
    let leaf = create_group(app, cookie, "leaf", Some(mid), Some(app_a), None).await?;
    let leaf_group = detail(app, &format!("{GROUPS}/{leaf}"), cookie).await?;
    assert_eq!(leaf_group["effective_status"], "disabled");
    assert_eq!(leaf_group["blocked_by_group_id"], json!(root2));
    let mid_group = detail(app, &format!("{GROUPS}/{mid}"), cookie).await?;
    assert_eq!(mid_group["effective_status"], "disabled");
    assert_eq!(mid_group["blocked_by_group_id"], json!(root2));

    let flow_root = create_flow(app, cookie, "flow-root", root, app_a, "draft").await?;
    let flow_child = create_flow(app, cookie, "flow-child", child, app_a, "active").await?;
    let flow_grandchild =
        create_flow(app, cookie, "flow-grandchild", grandchild, app_a, "active").await?;
    let flow_leaf = create_flow(app, cookie, "flow-leaf", leaf, app_a, "active").await?;
    let flow_disabled = create_flow(app, cookie, "flow-disabled", root, app_a, "disabled").await?;

    let saved = detail(app, &format!("{BASE}/{flow_root}"), cookie).await?;
    assert_eq!(saved["effective_status"], "draft");
    assert_eq!(saved["blocked_by_group_id"], Value::Null);
    let saved = detail(app, &format!("{BASE}/{flow_child}"), cookie).await?;
    assert_eq!(saved["effective_status"], "draft");
    assert_eq!(saved["blocked_by_group_id"], json!(child));
    let saved = detail(app, &format!("{BASE}/{flow_grandchild}"), cookie).await?;
    assert_eq!(saved["effective_status"], "disabled");
    assert_eq!(saved["blocked_by_group_id"], json!(grandchild));
    let saved = detail(app, &format!("{BASE}/{flow_leaf}"), cookie).await?;
    assert_eq!(saved["effective_status"], "disabled");
    assert_eq!(saved["blocked_by_group_id"], json!(root2));
    let saved = detail(app, &format!("{BASE}/{flow_disabled}"), cookie).await?;
    assert_eq!(saved["effective_status"], "disabled");
    assert_eq!(saved["blocked_by_group_id"], Value::Null);

    // Descendant filter is inclusive; effective status is a separate filter.
    let descendants = detail(app, &format!("{BASE}?group_id={root}&limit=100"), cookie).await?;
    assert_eq!(descendants["total"], 4, "root, child and grandchild flows");
    let child_descendants =
        detail(app, &format!("{BASE}?group_id={child}&limit=100"), cookie).await?;
    assert_eq!(child_descendants["total"], 2);
    let disabled = detail(
        app,
        &format!("{BASE}?effective_status=disabled&limit=100"),
        cookie,
    )
    .await?;
    assert_eq!(disabled["total"], 3);
    let stored_disabled = detail(app, &format!("{BASE}?status=disabled&limit=100"), cookie).await?;
    assert_eq!(stored_disabled["total"], 1);

    // Child must share its parent's application.
    expect_status(
        app,
        "POST",
        GROUPS,
        Some(json!({"name": "mismatch", "parent_id": root, "application_id": app_b})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let page = detail(app, &format!("{GROUPS}?search=mismatch"), cookie).await?;
    assert_eq!(page["total"], 0, "rejected child must not persist");

    // A reparent that would close a cycle is rejected and leaves the row intact.
    expect_rejected(
        app,
        "PUT",
        &format!("{GROUPS}/{root}"),
        Some(json!({"parent_id": grandchild})),
        cookie,
    )
    .await?;
    let unchanged = detail(app, &format!("{GROUPS}/{root}"), cookie).await?;
    assert_eq!(unchanged["parent_id"], Value::Null);

    // Changing a populated group's application must reject until references move.
    expect_rejected(
        app,
        "PUT",
        &format!("{GROUPS}/{root}"),
        Some(json!({"application_id": app_b})),
        cookie,
    )
    .await?;
    let unchanged = detail(app, &format!("{GROUPS}/{root}"), cookie).await?;
    assert_eq!(unchanged["application_id"], json!(app_a));

    // A flow in an application-bound group must match that application.
    expect_status(
        app,
        "POST",
        BASE,
        Some(json!({
            "name": "conflicting-application",
            "sources": [{"object_type": "all"}],
            "destinations": [{"object_type": "internet"}],
            "group_id": root,
            "application_id": app_b,
        })),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let page = detail(
        app,
        &format!("{BASE}?search=conflicting-application"),
        cookie,
    )
    .await?;
    assert_eq!(page["total"], 0, "conflicting flow must not persist");
    Ok(())
}

// ---------------------------------------------------------------------------
// Bulk actions: atomicity and the Update/Delete permission split
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn bulk_actions_are_atomic_and_require_the_matching_permission() -> TestResult {
    let harness = harness("pip211cat4", 14).await?;
    let result = bulk_actions(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn bulk_actions(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();

    let app_a = create_application(app, cookie, "bulk-a").await?;
    let app_b = create_application(app, cookie, "bulk-b").await?;
    let group_a = create_group(app, cookie, "bulk-group-a", None, Some(app_a), None).await?;
    let group_b = create_group(app, cookie, "bulk-group-b", None, Some(app_b), None).await?;
    let first = create_flow(app, cookie, "bulk-one", group_a, app_a, "draft").await?;
    let second = create_flow(app, cookie, "bulk-two", group_a, app_a, "draft").await?;

    let response = expect_status(
        app,
        "POST",
        &format!("{BASE}/bulk-action"),
        Some(json!({"ids": [first, second], "action": "set_status", "status": "active"})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(response["action"], "set_status");
    let mut affected: Vec<Uuid> = response["affected"]
        .as_array()
        .ok_or("affected")?
        .iter()
        .map(|item| Uuid::parse_str(item["id"].as_str().unwrap_or_default()))
        .collect::<Result<Vec<Uuid>, uuid::Error>>()?;
    affected.sort_unstable();
    let mut expected = vec![first, second];
    expected.sort_unstable();
    assert_eq!(affected, expected);
    for id in [first, second] {
        let saved = detail(app, &format!("{BASE}/{id}"), cookie).await?;
        assert_eq!(saved["status"], "active");
    }

    // Move one flow across groups and applications in one transaction.
    expect_status(
        app,
        "POST",
        &format!("{BASE}/bulk-action"),
        Some(json!({
            "ids": [first],
            "action": "move",
            "group_id": group_b,
            "application_id": app_b,
        })),
        cookie,
        StatusCode::OK,
    )
    .await?;
    let moved = detail(app, &format!("{BASE}/{first}"), cookie).await?;
    assert_eq!(moved["group_id"], json!(group_b));
    assert_eq!(moved["application_id"], json!(app_b));

    // Duplicate ids deduplicate instead of double-applying.
    let response = expect_status(
        app,
        "POST",
        &format!("{BASE}/bulk-action"),
        Some(json!({"ids": [second, second], "action": "set_status", "status": "draft"})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(response["affected"].as_array().ok_or("affected")?.len(), 1);

    // An unknown id rolls the whole set back.
    let unknown = Uuid::now_v7();
    expect_rejected(
        app,
        "POST",
        &format!("{BASE}/bulk-action"),
        Some(json!({"ids": [second, unknown], "action": "set_status", "status": "active"})),
        cookie,
    )
    .await?;
    let unchanged = detail(app, &format!("{BASE}/{second}"), cookie).await?;
    assert_eq!(unchanged["status"], "draft", "unknown id must roll back");
    assert_eq!(unchanged["group_id"], json!(group_a));
    assert_eq!(unchanged["application_id"], json!(app_a));

    // Unrelated action fields and empty id sets are rejected.
    for body in [
        json!({"ids": [second], "action": "set_status", "status": "active", "group_id": group_a}),
        json!({"ids": [second], "action": "move", "status": "active", "group_id": group_a}),
        json!({"ids": [second], "action": "move"}),
        json!({"ids": [second], "action": "delete", "status": "active"}),
        json!({"ids": [], "action": "set_status", "status": "active"}),
    ] {
        expect_status(
            app,
            "POST",
            &format!("{BASE}/bulk-action"),
            Some(body.clone()),
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }

    // A user with Inventory Update but no Delete may set status but not delete.
    let updater = limited_cookie(&harness.state, app, &[Action::Read, Action::Update]).await?;
    expect_status(
        app,
        "POST",
        &format!("{BASE}/bulk-action"),
        Some(json!({"ids": [second], "action": "set_status", "status": "active"})),
        &updater,
        StatusCode::OK,
    )
    .await?;
    expect_status(
        app,
        "POST",
        &format!("{BASE}/bulk-action"),
        Some(json!({"ids": [second], "action": "delete"})),
        &updater,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let still_there = detail(app, &format!("{BASE}/{second}"), cookie).await?;
    assert_eq!(still_there["status"], "active");

    let response = expect_status(
        app,
        "POST",
        &format!("{BASE}/bulk-action"),
        Some(json!({"ids": [second], "action": "delete"})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(response["action"], "delete");
    expect_status(
        app,
        "GET",
        &format!("{BASE}/{second}"),
        None,
        cookie,
        StatusCode::NOT_FOUND,
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolution, address groups, OOB and connection-target authorization
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn resolution_policy_and_connection_target_authorization() -> TestResult {
    let harness = harness("pip211cat5", 15).await?;
    let result = resolution_and_policy(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn application_contact_assignment_is_typed_and_blocks_referenced_delete() -> TestResult {
    let harness = harness("pip211cat8", 8).await?;
    let result = application_contacts(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn application_contacts(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();
    let application = created(
        app,
        APPLICATIONS,
        json!({"name": format!("pip211-contacts-{}", common::unique_suffix())}),
        cookie,
    )
    .await?;
    let application_id = id_of(&application)?;
    let contact = created(
        app,
        "/api/v1/inventory/contacts",
        json!({"name": "PIP-211 application contact"}),
        cookie,
    )
    .await?;
    let contact_id = id_of(&contact)?;
    let role = created(
        app,
        "/api/v1/inventory/contact-roles",
        json!({
            "name": "PIP-211 application contact role",
            "slug": format!("pip211-app-contact-{}", common::unique_suffix()),
        }),
        cookie,
    )
    .await?;
    let role_id = id_of(&role)?;

    let assignment = created(
        app,
        "/api/v1/inventory/contact-assignments",
        json!({
            "object_type": "inventory.application",
            "object_id": application_id,
            "contact_id": contact_id,
            "role_id": role_id,
            "priority": "primary",
        }),
        cookie,
    )
    .await?;
    assert_eq!(assignment["object"]["object_type"], "inventory.application");
    assert_eq!(assignment["object"]["object_id"], json!(application_id));
    assert_eq!(assignment["object"]["label"], application["name"]);
    let assignment_id = id_of(&assignment)?;

    let application_type: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label = 'inventory' AND model = 'application'",
    )
    .fetch_one(&harness.state.db_pool)
    .await?;

    let (status, page) = call(
        app,
        "GET",
        &format!(
            "/api/v1/inventory/contact-assignments?content_type={application_type}&object_id={application_id}"
        ),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["id"] == json!(assignment_id) && item["object"]["label"] == application["name"]
        })
    }));

    let (status, blocked) = call(
        app,
        "DELETE",
        &format!("{APPLICATIONS}/{application_id}"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked}");
    // The shared projection withholds unregistered blocker attributes instead
    // of disclosing a contact's identity under an unproven READ decision.
    assert_eq!(blocked["reason_code"], "DELETE_BLOCKED");

    let deletion = sqlx::query("DELETE FROM inventory_application WHERE id = $1")
        .bind(application_id)
        .execute(&harness.state.db_pool)
        .await;
    assert!(
        deletion.is_err(),
        "native deletion must preserve typed contacts"
    );

    let (status, _) = call(
        app,
        "DELETE",
        &format!("/api/v1/inventory/contact-assignments/{assignment_id}"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        app,
        "DELETE",
        &format!("{APPLICATIONS}/{application_id}"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A writer that validated before the deletion must not insert an orphan
    // after it, even when it bypasses the REST pre-validation.
    let stale_insert = sqlx::query(
        "INSERT INTO tenancy_contactassignment (id, object_type_id, object_id, contact_id, role_id, priority) VALUES ($1, $2, $3, $4, $5, 'primary')",
    )
    .bind(Uuid::now_v7())
    .bind(application_type)
    .bind(application_id)
    .bind(contact_id)
    .bind(role_id)
    .execute(&harness.state.db_pool)
    .await;
    assert!(
        stale_insert.is_err(),
        "stale contact creation must be rejected"
    );

    // ApplicationRole has no contract-level contact target.
    let application_role = created(
        app,
        ROLES,
        json!({"name": format!("pip211-role-{}", common::unique_suffix())}),
        cookie,
    )
    .await?;
    let (status, body) = call(
        app,
        "POST",
        "/api/v1/inventory/contact-assignments",
        Some(json!({
            "object_type": "inventory.applicationrole",
            "object_id": id_of(&application_role)?,
            "contact_id": contact_id,
            "role_id": role_id,
        })),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    Ok(())
}

async fn resolution_and_policy(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();
    let pool = &harness.state.db_pool;

    let (host, _) = common::seed_device(pool, "pip211cat5").await?;
    let (peer, _) = common::seed_device(pool, "pip211cat5").await?;
    let host_ip = ip(pool, "192.0.2.51/24").await?;
    assign(pool, host, &[host_ip]).await?;
    set_primary(pool, host, host_ip).await?;
    let peer_ip = ip(pool, "192.0.2.52/24").await?;
    assign(pool, peer, &[peer_ip]).await?;
    set_primary(pool, peer, peer_ip).await?;

    // An IP range resolves to the exact CIDRs covering the whole range.
    let range = created(
        app,
        IP_RANGES,
        json!({
            "start_address": "192.0.2.100",
            "end_address": "192.0.2.102",
            "status": "active",
            "description": "",
        }),
        cookie,
    )
    .await?;
    let range_id = id_of(&range)?;
    let resolved = expect_status(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(json!({"object_type": "ipam.iprange", "object_id": range_id})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    let cidrs: Vec<&str> = resolved["addresses"]
        .as_array()
        .ok_or("addresses")?
        .iter()
        .map(|address| address["cidr"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        cidrs,
        vec!["192.0.2.100/31", "192.0.2.102/32"],
        "an explicit range must not be widened or truncated"
    );

    // Device tag selectors: default -> primary, all -> eligible, oob -> OOB.
    let tag = create_tag(app, cookie, "resolve-devices", &["dcim.device"]).await?;
    let extra = ip(pool, "192.0.2.61/24").await?;
    assign(pool, host, &[extra]).await?;
    let oob_ip = ip(pool, "198.51.100.61/24").await?;
    assign(pool, host, &[oob_ip]).await?;
    sqlx::query("UPDATE dcim_device SET tags = ARRAY[$2], oob_ip_id = $3 WHERE id = $1")
        .bind(host)
        .bind(tag.to_string())
        .bind(oob_ip)
        .execute(pool)
        .await?;

    let default = expect_status(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(json!({
            "object_type": "extras.tag",
            "object_id": tag,
            "tag_object_type": "dcim.device",
        })),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        default["addresses"],
        json!([{"cidr": "192.0.2.51/32", "vrf_id": Value::Null}]),
        "{default}"
    );

    let all = expect_status(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(json!({
            "object_type": "extras.tag",
            "object_id": tag,
            "tag_object_type": "dcim.device",
            "address_mode": "all",
        })),
        cookie,
        StatusCode::OK,
    )
    .await?;
    let mut all_cidrs: Vec<&str> = all["addresses"]
        .as_array()
        .ok_or("addresses")?
        .iter()
        .map(|address| address["cidr"].as_str().unwrap_or_default())
        .collect();
    all_cidrs.sort_unstable();
    assert_eq!(
        all_cidrs,
        vec!["192.0.2.51/32", "192.0.2.61/32", "198.51.100.61/32"],
        "{all}"
    );

    let oob = expect_status(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(json!({
            "object_type": "extras.tag",
            "object_id": tag,
            "tag_object_type": "dcim.device",
            "address_mode": "oob",
        })),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        oob["addresses"],
        json!([{"cidr": "198.51.100.61/32", "vrf_id": Value::Null}]),
        "{oob}"
    );

    let vm_tag = create_tag(
        app,
        cookie,
        "resolve-vms",
        &["virtualization.virtualmachine"],
    )
    .await?;
    expect_status(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(json!({
            "object_type": "extras.tag",
            "object_id": vm_tag,
            "tag_object_type": "virtualization.virtualmachine",
            "address_mode": "oob",
        })),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    // An unresolvable address-group member blocks the whole policy.
    let reserved_range = created(
        app,
        IP_RANGES,
        json!({
            "start_address": "192.0.2.200",
            "end_address": "192.0.2.201",
            "status": "reserved",
            "description": "",
        }),
        cookie,
    )
    .await?;
    let reserved_range_id = id_of(&reserved_range)?;
    let address_group = created(
        app,
        ADDRESS_GROUPS,
        json!({
            "name": "mixed-addresses",
            "members": [
                {"object_type": "dcim.device", "object_id": host},
                {"object_type": "ipam.iprange", "object_id": reserved_range_id},
            ],
        }),
        cookie,
    )
    .await?;
    let address_group_id = id_of(&address_group)?;
    let group_resolution = expect_status(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(json!({
            "object_type": "inventory.addressgroup",
            "object_id": address_group_id,
        })),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert!(
        !group_resolution["diagnostics"]
            .as_array()
            .ok_or("diagnostics")?
            .is_empty(),
        "an unresolvable member must be reported: {group_resolution}"
    );
    let group_flow = created(
        app,
        BASE,
        json!({
            "name": "group-policy",
            "status": "active",
            "sources": [{"object_type": "inventory.addressgroup", "object_id": address_group_id}],
            "destinations": [{"object_type": "internet"}],
        }),
        cookie,
    )
    .await?;
    let exported = policy(harness, "dcim.device", host).await?;
    assert_eq!(
        exported["apply_ready"], false,
        "an unresolvable member must not yield a partial union: {exported}"
    );
    assert_eq!(exported["rules"], json!([]));
    assert!(
        exported["diagnostics"]
            .as_array()
            .ok_or("diagnostics")?
            .iter()
            .any(|diagnostic| diagnostic["code"] == "endpoint_unresolved"),
        "{exported}"
    );
    expect_status(
        app,
        "DELETE",
        &format!("{BASE}/{}", id_of(&group_flow)?),
        None,
        cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;

    // Missing OOB on an explicit device is unresolved, never a primary fallback.
    let (oobless, _) = common::seed_device(pool, "pip211cat5").await?;
    let oobless_ip = ip(pool, "192.0.2.71/24").await?;
    assign(pool, oobless, &[oobless_ip]).await?;
    set_primary(pool, oobless, oobless_ip).await?;
    let oob_resolution = expect_status(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(json!({
            "object_type": "dcim.device",
            "object_id": oobless,
            "address_mode": "oob",
        })),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(oob_resolution["addresses"], json!([]));
    assert!(
        !oob_resolution["default_ipaddresses"]
            .as_array()
            .ok_or("default choices")?
            .is_empty(),
        "the primary exists but must not be used as an OOB fallback"
    );
    created(
        app,
        BASE,
        json!({
            "name": "oob-flow",
            "status": "active",
            "sources": [{
                "object_type": "dcim.device",
                "object_id": oobless,
                "address_mode": "oob",
            }],
            "destinations": [{"object_type": "internet"}],
        }),
        cookie,
    )
    .await?;
    let exported = policy(harness, "dcim.device", oobless).await?;
    assert_eq!(exported["apply_ready"], false, "{exported}");
    assert_eq!(exported["rules"], json!([]));
    assert!(
        exported["diagnostics"]
            .as_array()
            .ok_or("diagnostics")?
            .iter()
            .any(|diagnostic| diagnostic["code"] == "endpoint_unresolved"),
        "{exported}"
    );

    // Host policy requires connection-target row visibility for every peer.
    created(
        app,
        BASE,
        json!({
            "name": "visibility-flow",
            "status": "active",
            "sources": [{"object_type": "dcim.device", "object_id": host}],
            "destinations": [{"object_type": "dcim.device", "object_id": peer}],
        }),
        cookie,
    )
    .await?;
    let uri = format!("{BASE}/firewall-policy?host_type=dcim.device&host_id={host}");
    let exported = detail(app, &uri, cookie).await?;
    assert_eq!(exported["apply_ready"], true, "{exported}");
    assert!(
        !exported["rules"].as_array().ok_or("rules")?.is_empty(),
        "{exported}"
    );

    let no_visibility = user_cookie(&harness.state, app, true, false, None).await?;
    expect_status(
        app,
        "GET",
        &uri,
        None,
        &no_visibility,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let hidden_peer = user_cookie(&harness.state, app, true, true, Some(peer)).await?;
    expect_status(app, "GET", &uri, None, &hidden_peer, StatusCode::FORBIDDEN).await?;

    // An external read:inventory PAT reads the policy under its actor's live
    // visibility; no separate connection-target token scope is required.
    let (status, token) = call(
        app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(json!({
            "name": format!("pip211-policy-{}", common::unique_suffix()),
            "scopes": [{"action": "read", "resource_type": "inventory"}],
        })),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let bearer = token["token_value"].as_str().ok_or("token value")?;
    let (status, bytes) =
        common::request_with_auth(app, "GET", &uri, None, None, Some(bearer)).await?;
    assert_eq!(status, StatusCode::OK);
    let exported: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(exported["apply_ready"], true, "{exported}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Address-group members share the flow selector mutation owner
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn address_group_members_reject_foreign_selected_ip() -> TestResult {
    let harness = harness("pip211ag1", 7).await?;
    let result = address_group_member_validation(&harness).await;
    let cleanup = shutdown(harness).await;
    result?;
    cleanup?;
    Ok(())
}

async fn address_group_member_validation(harness: &Harness) -> TestResult {
    let app = &harness.app;
    let cookie = harness.cookie.as_str();
    let pool = &harness.state.db_pool;
    let (device, _) = common::seed_device(pool, "pip211ag1").await?;
    let (other, _) = common::seed_device(pool, "pip211ag1").await?;
    let device_ip = ip(pool, "192.0.2.81/24").await?;
    let foreign_ip = ip(pool, "192.0.2.82/24").await?;
    assign(pool, device, &[device_ip]).await?;
    assign(pool, other, &[foreign_ip]).await?;
    set_primary(pool, device, device_ip).await?;
    set_primary(pool, other, foreign_ip).await?;

    let foreign_member = json!({
        "object_type": "dcim.device",
        "object_id": device,
        "address_mode": "selected",
        "ipaddresses": [foreign_ip],
    });
    expect_status(
        app,
        "POST",
        ADDRESS_GROUPS,
        Some(json!({
            "name": "foreign-selected",
            "members": [foreign_member.clone()],
        })),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let group = created(
        app,
        ADDRESS_GROUPS,
        json!({
            "name": "valid-members",
            "description": "kept on rejected update",
            "comments": "kept comment",
            "members": [{
                "object_type": "dcim.device",
                "object_id": device,
                "address_mode": "selected",
                "ipaddresses": [device_ip],
            }],
        }),
        cookie,
    )
    .await?;
    let group_id = id_of(&group)?;

    expect_status(
        app,
        "PUT",
        &format!("{ADDRESS_GROUPS}/{group_id}"),
        Some(json!({
            "name": "rejected-name",
            "description": "rejected-description",
            "comments": "rejected-comments",
            "members": [foreign_member],
        })),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    assert_eq!(
        detail(app, &format!("{ADDRESS_GROUPS}/{group_id}"), cookie).await?,
        group,
        "a rejected member replacement must preserve existing members and metadata"
    );

    expect_status(
        app,
        "PUT",
        &format!("{ADDRESS_GROUPS}/{group_id}"),
        Some(json!({"members": [{
            "object_type": "ipam.ipaddress",
            "object_id": Uuid::now_v7(),
        }]})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    assert_eq!(
        detail(app, &format!("{ADDRESS_GROUPS}/{group_id}"), cookie).await?,
        group,
        "a rejected direct reference must not replace the stored members"
    );
    Ok(())
}
