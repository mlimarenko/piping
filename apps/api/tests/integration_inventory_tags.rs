//! Native tag-definition read API, including pagination and access control.

mod common;

use axum::http::StatusCode;
use serde_json::Value;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn tags_list_reads_native_definitions_with_paging_and_sort() -> TestResult {
    let state = common::build_test_app_state().await?;
    let prefix = format!("tag-read-{}", common::unique_suffix());
    let mut ids = Vec::new();
    for index in 0..3 {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO inventory_tag (id, name, slug, color, description, weight) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(format!("{prefix}-{index}"))
        .bind(format!("{prefix}-{index}"))
        .bind("1f8b4c")
        .bind("Tag definition")
        .bind(index * 10)
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
        &app,
        "GET",
        &format!("/api/v1/inventory/tags?search={prefix}&limit=1&offset=1&sort=name&order=asc"),
        None,
        Some(&cookie),
    )
    .await?;
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
    assert_eq!(page["items"][0]["weight"], 10);

    let (status, body) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{}", ids[0]),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let tag: Value = serde_json::from_slice(&body)?;
    assert_eq!(tag["name"], format!("{prefix}-0"));
    assert_eq!(tag["slug"], format!("{prefix}-0"));
    assert_eq!(tag["color"], "1f8b4c");
    assert_eq!(tag["description"], "Tag definition");
    assert_eq!(tag["weight"], 0);
    // Imported source metadata belongs to the source binding, never the DTO.
    assert!(tag.get("external_id").is_none());
    assert!(tag.get("piping_synced_at").is_none());

    let (status, _) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{}", Uuid::now_v7()),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = common::request(
        &app,
        "GET",
        "/api/v1/inventory/tags/not-a-uuid",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = common::request(
        &app,
        "GET",
        "/api/v1/inventory/tags?sort=unapproved_column",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn tags_require_inventory_read_access() -> TestResult {
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    for uri in [
        "/api/v1/inventory/tags".to_string(),
        format!("/api/v1/inventory/tags/{}", Uuid::now_v7()),
    ] {
        let (status, _) = common::request(&app, "GET", &uri, None, None).await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    Ok(())
}
