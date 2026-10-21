//! Native module-profile read API, including pagination and access control.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn module_profiles_preserve_schema_and_page_native_rows() -> TestResult {
    let state = common::build_test_app_state().await?;
    let prefix = format!("profile-read-{}", common::unique_suffix());
    let mut ids = Vec::new();
    let schema = json!({
        "type": "object", "properties": {"ports": {"type": "integer", "minimum": 1}},
        "required": ["ports"],
    });
    for index in 0..3 {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO dcim_moduletypeprofile (id, name, description, comments, schema, custom_field_data) VALUES ($1, $2, $3, $4, $5, '{}'::jsonb)",
        )
        .bind(id)
        .bind(format!("{prefix}-{index}"))
        .bind("Module attribute schema")
        .bind("Imported catalog metadata")
        .bind(&schema)
        .execute(&state.db_pool)
        .await?;
        ids.push(id);
    }
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("test login failed")?;
    let (status, body) = common::request(
        &app, "GET",
        &format!("/api/v1/inventory/module-type-profiles?search={prefix}&limit=1&offset=1&sort=name&order=asc"),
        None, Some(&cookie),
    ).await?;
    assert_eq!(status, StatusCode::OK);
    let page: Value = serde_json::from_slice(&body)?;
    assert_eq!(page["total"], 3);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 1);
    assert_eq!(
        page["items"].as_array().ok_or("items array missing")?.len(),
        1
    );
    assert_eq!(page["items"][0]["id"], ids[1].to_string());
    assert_eq!(page["items"][0]["schema"], schema);

    let (status, body) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/module-type-profiles/{}", ids[0]),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let profile: Value = serde_json::from_slice(&body)?;
    assert_eq!(profile["schema"], schema);
    assert_eq!(profile["description"], "Module attribute schema");
    assert_eq!(profile["comments"], "Imported catalog metadata");
    assert_eq!(profile["owner_id"], Value::Null);

    let (status, _) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/module-type-profiles/{}", Uuid::now_v7()),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = common::request(
        &app,
        "GET",
        "/api/v1/inventory/module-type-profiles?sort=unapproved_column",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn module_profiles_require_inventory_read_access() -> TestResult {
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    for uri in [
        "/api/v1/inventory/module-type-profiles".to_string(),
        format!("/api/v1/inventory/module-type-profiles/{}", Uuid::now_v7()),
    ] {
        let (status, _) = common::request(&app, "GET", &uri, None, None).await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    Ok(())
}
