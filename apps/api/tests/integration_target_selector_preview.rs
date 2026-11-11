//! An unsaved target selector previews exactly the hosts a saved set with that
//! selector previews, and persists nothing. Requires exclusively owned
//! PostgreSQL and Redis.

mod common;

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const PREVIEW_PATH: &str = "/api/v1/playbooks/target-sets/preview";

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn pinned_hosts_bypass_facets_but_not_exclusions_or_address_safety() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    let prefix = format!("pins-{}", common::unique_suffix());
    let (device, _) = common::seed_device(&pool, &prefix).await?;
    let original = api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({"selector":{"name":prefix}})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(original["total"], 1);
    let target = original["items"][0]["target_id"].clone();
    let pinned = json!({"name":format!("absent-{prefix}"), "include_target_ids":[target]});
    let preview = api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({"selector":pinned})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        preview["total"], 1,
        "a pin must bypass unmatched name facets"
    );
    assert_eq!(preview["items"][0]["source_id"], device.to_string());
    let excluded = api(&app, "POST", PREVIEW_PATH, Some(json!({"selector":{
        "name":format!("absent-{prefix}"), "include_target_ids":[target], "exclude_source_ids":[device]
    }})), Some(&cookie), StatusCode::OK).await?;
    assert_eq!(
        excluded["total"], 0,
        "exclusion must win over an explicit pin"
    );
    let unbound = api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({"selector":{
            "name":format!("absent-{prefix}"), "include_source_ids":[device], "address_bound":true
        }})),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(unbound["total"], 0, "pins cannot bypass address safety");
    let runtime = api(&app, "POST", PREVIEW_PATH, Some(json!({"selector":{
            "name":format!("absent-{prefix}"), "include_source_ids":[device], "runtime":["ansible"]
    }})), Some(&cookie), StatusCode::OK).await?;
    assert_eq!(
        runtime["total"], 0,
        "pins cannot bypass runtime connection requirements"
    );
    Ok(())
}

async fn api(
    app: &Router,
    method: &str,
    path: &str,
    payload: Option<Value>,
    cookie: Option<&str>,
    expected: StatusCode,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let body = payload.map(|value| value.to_string());
    let (status, body) = common::request(app, method, path, body.as_deref(), cookie).await?;
    assert_eq!(
        status,
        expected,
        "{method} {path}: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    })
}

/// Rows the request could have created under this run's unique suffix.
async fn saved_sets_named_like(
    pool: &PgPool,
    pattern: &str,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM playbook_target_set WHERE name LIKE $1")
            .bind(pattern)
            .fetch_one(pool)
            .await?,
    )
}

fn item_names(preview: &Value) -> Vec<&str> {
    preview["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["name"].as_str())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn unsaved_selector_preview_matches_saved_set_and_writes_nothing() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;

    let suffix = common::unique_suffix();
    let prefix = format!("selector-preview-{suffix}");
    let (_, first_name) = common::seed_device(&pool, &prefix).await?;
    let (_, second_name) = common::seed_device(&pool, &prefix).await?;
    let selector = json!({ "name": prefix });
    let name_pattern = format!("%{suffix}%");

    // A selector that matches two hosts answers with both, and creates no set.
    let preview = api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({ "selector": selector, "limit": 50 })),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    let mut keys: Vec<&str> = preview
        .as_object()
        .ok_or("preview must be an object")?
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["items", "limit", "total", "truncated"]);
    assert_eq!(preview["total"], 2);
    assert_eq!(preview["limit"], 50);
    assert_eq!(preview["truncated"], false);
    let names = item_names(&preview);
    assert!(
        names.contains(&first_name.as_str()) && names.contains(&second_name.as_str()),
        "both seeded hosts must be previewed: {preview}"
    );
    assert_eq!(
        saved_sets_named_like(&pool, &name_pattern).await?,
        0,
        "the preview must not create a playbook_target_set row"
    );

    // The same selector, once saved, previews the same hosts, total and
    // truncation.
    let created = api(
        &app,
        "POST",
        "/api/v1/playbooks/target-sets",
        Some(json!({
            "name": format!("selector-preview-set-{suffix}"),
            "mode": "dynamic",
            "selector": selector,
        })),
        Some(&cookie),
        StatusCode::CREATED,
    )
    .await?;
    let target_set_id = created["target_set"]["id"]
        .as_str()
        .ok_or("created target set id")?
        .to_string();
    assert_eq!(saved_sets_named_like(&pool, &name_pattern).await?, 1);
    let saved = api(
        &app,
        "GET",
        &format!("/api/v1/playbooks/target-sets/{target_set_id}/preview?limit=50"),
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        saved["items"], preview["items"],
        "same selector, same hosts"
    );
    assert_eq!(saved["total"], preview["total"]);
    assert_eq!(saved["limit"], preview["limit"]);
    assert_eq!(saved["truncated"], preview["truncated"]);

    // A selector that matches more hosts than the page reports a different
    // answer than the empty and the complete one: a truncated first page.
    let bounded = api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({ "selector": selector, "limit": 1 })),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(bounded["total"], 2);
    assert_eq!(bounded["truncated"], true);
    assert_eq!(
        bounded["items"].as_array().map(Vec::len),
        Some(1),
        "a truncated page carries exactly the limit: {bounded}"
    );
    assert_eq!(
        bounded["items"][0], preview["items"][0],
        "truncation keeps the canonical order"
    );

    // A selector that matches nothing is a resolved zero, not an error and not
    // a truncation.
    let empty = api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({ "selector": { "name": format!("{prefix}-absent") }, "limit": 50 })),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["items"], json!([]));
    assert_eq!(empty["truncated"], false);

    // An unusable selector fails loudly instead of previewing nothing.
    for invalid in [
        json!({ "selector": { "tag_slugs": ["retired-slug"] } }),
        json!({ "selector": ["not", "an", "object"] }),
        json!({ "selector": { "sort": "unknown-column" } }),
    ] {
        api(
            &app,
            "POST",
            PREVIEW_PATH,
            Some(invalid),
            Some(&cookie),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    assert_eq!(
        saved_sets_named_like(&pool, &name_pattern).await?,
        1,
        "no preview request may add another target set"
    );

    api(
        &app,
        "DELETE",
        &format!("/api/v1/playbooks/target-sets/{target_set_id}"),
        None,
        Some(&cookie),
        StatusCode::NO_CONTENT,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn unsaved_selector_preview_requires_authentication_and_target_set_read() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let admin_cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    let suffix = common::unique_suffix();

    api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({ "selector": {} })),
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await?;

    // A signed-in principal without playbook target-set read sees no host list.
    let username = format!("selector-preview-{suffix}");
    let created_user = api(
        &app,
        "POST",
        "/api/v1/admin/access-control/users",
        Some(json!({
            "username": username,
            "email": format!("{username}@test.local"),
            "status": "active",
            "auth_source": "local",
            "credential": {
                "mode": "manual",
                "password": common::TEST_ADMIN_PASSWORD,
            },
        })),
        Some(&admin_cookie),
        StatusCode::OK,
    )
    .await?;
    let user = created_user["id"]
        .as_str()
        .ok_or("created user id")?
        .to_owned();
    let role = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_role (id,name,description) VALUES ($1,$2,NULL)")
        .bind(role)
        .bind(&username)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO rbac_role_assignment (id,user_id,group_id,role_id) VALUES ($1,$2,NULL,$3)",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::parse_str(&user)?)
    .bind(role)
    .execute(&pool)
    .await?;
    api(
        &app,
        "POST",
        "/api/v1/admin/access-control/policies",
        Some(json!({
            "name": format!("{username}-deny"),
            "description": null,
            "effect": "deny",
            "actions": ["READ"],
            "resource_types": ["PLAYBOOK_TARGET_SET"],
            "constraints": null,
            "subjects": [format!("role:{role}")],
            "resources": [],
            "enabled": true,
        })),
        Some(&admin_cookie),
        StatusCode::OK,
    )
    .await?;
    let restricted_cookie =
        common::login_and_get_cookie(&app, &username, common::TEST_ADMIN_PASSWORD)
            .await
            .ok_or("restricted login failed")?;
    api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({ "selector": {} })),
        Some(&restricted_cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;
    api(
        &app,
        "GET",
        "/api/v1/playbooks/target-sets/facets?kind=role",
        None,
        Some(&restricted_cookie),
        StatusCode::FORBIDDEN,
    )
    .await?;

    // The read gate is the only difference: the same body succeeds for an
    // operator who may read target sets.
    api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({ "selector": {} })),
        Some(&admin_cookie),
        StatusCode::OK,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn selector_facets_count_page_and_keep_identifiers_after_rename() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    api(
        &app,
        "GET",
        "/api/v1/playbooks/target-sets/facets?kind=role",
        None,
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await?;
    let prefix = format!("facets-{}", common::unique_suffix());
    let (first, _) = common::seed_device(&pool, &prefix).await?;
    let (second, _) = common::seed_device(&pool, &prefix).await?;
    let role: Uuid = sqlx::query_scalar("SELECT role_id FROM dcim_device WHERE id=$1")
        .bind(first)
        .fetch_one(&pool)
        .await?;
    sqlx::query("UPDATE dcim_device SET role_id=$1 WHERE id=$2")
        .bind(role)
        .bind(second)
        .execute(&pool)
        .await?;
    let selector = json!({ "name": prefix });
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("selector", &selector.to_string())
        .finish();
    let facets_path = format!("/api/v1/playbooks/target-sets/facets?{query}");
    let roles = api(
        &app,
        "GET",
        &format!("{facets_path}&kind=role"),
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(roles["total"], 1);
    assert_eq!(roles["items"][0]["id"], role.to_string());
    assert_eq!(roles["items"][0]["count"], 2);
    let sites = api(
        &app,
        "GET",
        &format!("{facets_path}&kind=site&limit=1"),
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(sites["total"], 2);
    assert_eq!(sites["items"].as_array().ok_or("facet items")?.len(), 1);
    let beyond = api(
        &app,
        "GET",
        &format!("{facets_path}&kind=site&limit=1&offset=50"),
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(beyond["total"], 2);
    assert_eq!(beyond["items"], json!([]));
    let renamed = format!("renamed-{prefix}");
    sqlx::query("UPDATE dcim_devicerole SET name=$1 WHERE id=$2")
        .bind(&renamed)
        .bind(role)
        .execute(&pool)
        .await?;
    let renamed_roles = api(
        &app,
        "GET",
        &format!("{facets_path}&kind=role&search=renamed"),
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(renamed_roles["items"][0]["id"], role.to_string());
    assert_eq!(renamed_roles["items"][0]["name"], renamed);
    let selected = api(
        &app,
        "POST",
        PREVIEW_PATH,
        Some(json!({ "selector": { "name": prefix, "role_ids": [role] } })),
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        selected["total"], 2,
        "renaming must not change ID selection"
    );
    let tag = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_tag (id,name,slug) VALUES ($1,$2,$2)")
        .bind(tag)
        .bind(format!("tag-{prefix}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO inventory_tag_object_type (tag_id,content_type_id) SELECT $1,id FROM inventory_content_type WHERE app_label='dcim' AND model='device'")
        .bind(tag).execute(&pool).await?;
    sqlx::query("UPDATE dcim_device SET tags=$1 WHERE id=$2")
        .bind(vec![tag.to_string()])
        .bind(first)
        .execute(&pool)
        .await?;
    let tags = api(
        &app,
        "GET",
        &format!("{facets_path}&kind=tag"),
        None,
        Some(&cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(tags["total"], 1);
    assert_eq!(tags["items"][0]["id"], tag.to_string());
    assert_eq!(tags["items"][0]["count"], 1);
    Ok(())
}
