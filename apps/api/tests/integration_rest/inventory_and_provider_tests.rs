use super::*;

async fn get_with_encoding(
    app: &Router,
    path: &str,
    cookie: Option<&str>,
    accept: &str,
) -> Result<axum::response::Response, Box<dyn std::error::Error + Send + Sync>> {
    let mut request = Request::builder()
        .uri(path)
        .header("Accept-Encoding", accept);
    if let Some(cookie) = cookie {
        request = request.header("Cookie", cookie);
    }
    Ok(app.clone().oneshot(request.body(Body::empty())?).await?)
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; exercised by the REST integration suite.
async fn connection_target_list_compression_preserves_json_and_negotiation() {
    use std::io::Read;

    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let prefix = format!("gzip-target-{}-", Uuid::now_v7().simple());
    sqlx::query(
        "INSERT INTO virtualization_virtualmachine \
         (name, status, custom_field_data, comments, description, serial, start_on_boot, \
          interface_count, virtual_disk_count) \
         SELECT $1 || value::text, 'active', '{}', '', '', '', 'unknown', 0, 0 \
         FROM generate_series(1, 200) AS value",
    )
    .bind(&prefix)
    .execute(&pool)
    .await
    .expect("seed target list");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let path = format!("/api/v1/connections/targets?search={prefix}&limit=500");
    let (status, identity) = request(&app, "GET", &path, None, Some(&cookie))
        .await
        .expect("identity response");
    assert_eq!(status, StatusCode::OK);
    let expected: serde_json::Value = serde_json::from_slice(&identity).expect("identity JSON");
    assert_eq!(expected["items"].as_array().expect("items").len(), 200);

    for (accept, compressed) in [("gzip", true), ("gzip;q=0, identity", false), ("br", false)] {
        let response = get_with_encoding(&app, &path, Some(&cookie), accept)
            .await
            .expect("target response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("Content-Encoding").is_some(),
            compressed
        );
        if compressed {
            assert_eq!(response.headers()["Content-Encoding"], "gzip");
            assert!(response.headers().get_all("Vary").iter().any(|header| {
                header
                    .to_str()
                    .expect("vary")
                    .split(',')
                    .any(|value| value.trim().eq_ignore_ascii_case("accept-encoding"))
            }));
        }
        let encoded = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("response body");
        let decoded = if compressed {
            assert!(
                encoded.len() < identity.len() / 2,
                "large read-model should shrink"
            );
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(encoded.as_ref())
                .read_to_end(&mut decoded)
                .expect("valid gzip body");
            decoded
        } else {
            encoded.to_vec()
        };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&decoded).expect("JSON"),
            expected
        );
    }

    // Authentication replies do not inherit the collection's compression layer.
    let response = get_with_encoding(&app, "/api/v1/auth/me", Some(&cookie), "gzip")
        .await
        .expect("auth response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("Content-Encoding"));
    let denied = get_with_encoding(&app, &path, None, "gzip")
        .await
        .expect("unauthenticated response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    sqlx::query("DELETE FROM virtualization_virtualmachine WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await
        .expect("remove fixture targets");
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn inventory_tenant_groups_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let create_body = serde_json::json!({
        "name": "test-tg-crud",
        "slug": "test-tg-crud"
    })
    .to_string();

    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/tenant_groups",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("id");

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tenant_groups/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let update_body = serde_json::json!({
        "name": "test-tg-updated",
        "slug": "test-tg-updated"
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/tenant_groups/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/tenant_groups/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest inventory_list_rejects_invalid_sort_keys -- --ignored
async fn inventory_list_rejects_invalid_sort_keys() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    for path in [
        "/api/v1/inventory/devices?sort=unsupported_table_column",
        "/api/v1/inventory/sites?sort=unsupported_table_column",
        "/api/v1/inventory/clusters?sort=unsupported_table_column",
        "/api/v1/inventory/device_types?sort=unsupported_table_column",
        "/api/v1/inventory/racks?sort=unsupported_table_column",
        "/api/v1/inventory/prefixes?sort=unsupported_table_column",
        "/api/v1/inventory/vlans?sort=unsupported_table_column",
        "/api/v1/inventory/virtual_machines?sort=unsupported_table_column",
        "/api/v1/inventory/regions?sort=unsupported_table_column",
        "/api/v1/inventory/tenants?sort=unsupported_table_column",
    ] {
        let (status, _) = request(&app, "GET", path, None, Some(&cookie))
            .await
            .expect("request");
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
}

// --- Inventory CRUD: tenants, sites, clusters, device_roles, devices, virtual_machines, hosts ---

#[tokio::test]
#[ignore]
async fn inventory_tenants_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = Uuid::now_v7()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();
    let name = format!("test-tenant-crud-{}", suffix);
    let slug = format!("test-tenant-crud-{}", suffix);
    let create_body = serde_json::json!({ "name": name, "slug": slug }).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/tenants",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("id");

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tenants/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let update_body =
        serde_json::json!({ "name": "test-tenant-updated", "slug": "test-tenant-updated" })
            .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/tenants/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/tenants/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn inventory_sites_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let create_body =
        serde_json::json!({ "name": "test-site-crud", "slug": "test-site-crud" }).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/sites",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("id");

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/sites/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let update_body =
        serde_json::json!({ "name": "test-site-updated", "slug": "test-site-updated" }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/sites/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/sites/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest inventory_regions_delete_returns_structured_blockers -- --ignored
async fn inventory_regions_delete_returns_structured_blockers() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let region_name = format!("delete-guard-region-{suffix}");
    let region_body = serde_json::json!({
        "name": region_name,
        "slug": region_name,
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/regions",
        Some(&region_body),
        Some(&cookie),
    )
    .await
    .expect("create region");
    assert_eq!(status, StatusCode::CREATED);
    let region_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("region json")
        .get("id")
        .and_then(|v| v.as_str())
        .expect("region id")
        .to_string();

    let site_name = format!("delete-guard-site-{suffix}");
    let site_body = serde_json::json!({
        "name": site_name,
        "slug": site_name,
        "region_id": region_id.clone(),
    })
    .to_string();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/inventory/sites",
        Some(&site_body),
        Some(&cookie),
    )
    .await
    .expect("create site");
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/regions/{region_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete region");
    assert_eq!(status, StatusCode::CONFLICT);

    let payload = serde_json::from_slice::<serde_json::Value>(&body).expect("delete-blocked json");
    assert_eq!(payload["code"], "DELETE_BLOCKED");
    assert_eq!(payload["reason_code"], "DELETE_BLOCKED");
    assert_eq!(
        payload["message"],
        "Cannot delete this record while related objects still exist"
    );
    assert_eq!(payload["details"]["total_count"], 1);
    assert_eq!(payload["details"]["blockers"][0]["table"], "dcim_site");
    assert_eq!(payload["details"]["blockers"][0]["entity_key"], "Sites");
    assert_eq!(payload["details"]["blockers"][0]["count"], 1);
    assert_eq!(
        payload["details"]["blockers"][0]["examples"][0]["label"],
        site_name
    );
}

#[tokio::test]
#[ignore]
async fn inventory_clusters_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let pool = build_test_db_pool().await.expect("db pool");
    let suffix = unique_suffix();
    let cluster_type_id = Uuid::now_v7();
    let cluster_type_name = format!("cluster-type-{}", suffix);
    let cluster_type_slug = cluster_type_name.clone();
    sqlx::query(
        r#"
        INSERT INTO virtualization_clustertype (
            id,
            custom_field_data,
            name,
            slug,
            description,
            owner_id,
            comments
        )
        VALUES ($1, '{}'::jsonb, $2, $3, '', NULL, '')
        ON CONFLICT (name) DO UPDATE SET slug = EXCLUDED.slug
        "#,
    )
    .bind(cluster_type_id)
    .bind(&cluster_type_name)
    .bind(&cluster_type_slug)
    .execute(&pool)
    .await
    .expect("insert cluster type");

    let create_body = serde_json::json!({
        "name": format!("test-cluster-crud-{suffix}"),
        "cluster_type_id": cluster_type_id.to_string(),
        "status": "planned",
        "description": "cluster create description",
        "comments": "cluster create comments"
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/clusters",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("id");
    assert_eq!(
        created.get("status").and_then(|v| v.as_str()),
        Some("planned")
    );
    assert_eq!(
        created.get("description").and_then(|v| v.as_str()),
        Some("cluster create description")
    );
    assert_eq!(
        created.get("comments").and_then(|v| v.as_str()),
        Some("cluster create comments")
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/clusters/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let fetched: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        fetched.get("description").and_then(|v| v.as_str()),
        Some("cluster create description")
    );

    let update_body = serde_json::json!({
        "name": format!("test-cluster-updated-{suffix}"),
        "cluster_type_id": cluster_type_id.to_string(),
        "status": "offline",
        "description": "cluster update description",
        "comments": "cluster update comments"
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/clusters/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let updated: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        updated.get("status").and_then(|v| v.as_str()),
        Some("offline")
    );
    assert_eq!(
        updated.get("description").and_then(|v| v.as_str()),
        Some("cluster update description")
    );
    assert_eq!(
        updated.get("comments").and_then(|v| v.as_str()),
        Some("cluster update comments")
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/clusters?search=test-cluster-updated-{suffix}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);
    let listed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let listed_cluster = listed
        .get("items")
        .and_then(|value| value.as_array())
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(|value| value.as_str()) == Some(id))
        })
        .expect("updated cluster in list");
    assert_eq!(
        listed_cluster.get("status").and_then(|v| v.as_str()),
        Some("offline")
    );
    assert_eq!(
        listed_cluster.get("description").and_then(|v| v.as_str()),
        Some("cluster update description")
    );
    assert_eq!(
        listed_cluster.get("comments").and_then(|v| v.as_str()),
        Some("cluster update comments")
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/clusters/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn inventory_device_roles_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let create_body = serde_json::json!({
        "name": format!("test-dr-crud-{suffix}"),
        "slug": format!("test-dr-crud-{suffix}")
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/device_roles",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("id");

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/device_roles/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let updated_suffix = unique_suffix();
    let update_body = serde_json::json!({
        "name": format!("test-dr-updated-{updated_suffix}"),
        "slug": format!("test-dr-updated-{updated_suffix}")
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/device_roles/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/device_roles/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn inventory_devices_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let site_body = serde_json::json!({
        "name": format!("device-site-{}", suffix),
        "slug": format!("device-site-{}", suffix)
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/sites",
        Some(&site_body),
        Some(&cookie),
    )
    .await
    .expect("create site");
    assert_eq!(status, StatusCode::CREATED);
    let site_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("site json")
        .get("id")
        .and_then(|v| v.as_str())
        .expect("site id")
        .to_string();

    let role_body = serde_json::json!({
        "name": format!("device-role-{}", suffix)
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/device_roles",
        Some(&role_body),
        Some(&cookie),
    )
    .await
    .expect("create device role");
    assert_eq!(status, StatusCode::CREATED);
    let role_id = serde_json::from_slice::<serde_json::Value>(&body)
        .expect("role json")
        .get("id")
        .and_then(|v| v.as_str())
        .expect("role id")
        .to_string();

    let pool = build_test_db_pool().await.expect("db pool");
    let manufacturer_id = Uuid::now_v7();
    let manufacturer_name = format!("manufacturer-{}", suffix);
    let manufacturer_slug = manufacturer_name.clone();
    sqlx::query(
        r#"
        INSERT INTO dcim_manufacturer (id, custom_field_data, name, slug, description, comments)
        VALUES ($1, '{}'::jsonb, $2, $3, '', '')
        "#,
    )
    .bind(manufacturer_id)
    .bind(&manufacturer_name)
    .bind(&manufacturer_slug)
    .execute(&pool)
    .await
    .expect("insert manufacturer");

    let device_type_id = Uuid::now_v7();
    let device_type_model = format!("device-type-{}", suffix);
    let device_type_slug = device_type_model.clone();
    sqlx::query(
        r#"
        INSERT INTO dcim_devicetype (
            id,
            custom_field_data,
            model,
            slug,
            part_number,
            u_height,
            is_full_depth,
            subdevice_role,
            manufacturer_id,
            description,
            comments,
            interface_template_count,
            console_port_template_count,
            console_server_port_template_count,
            power_port_template_count,
            power_outlet_template_count,
            front_port_template_count,
            rear_port_template_count,
            device_bay_template_count,
            module_bay_template_count,
            inventory_item_template_count,
            device_count,
            exclude_from_utilization,
            front_image,
            rear_image
        )
        VALUES (
            $1,
            '{}'::jsonb,
            $2,
            $3,
            '',
            1.0,
            false,
            NULL,
            $4,
            '',
            '',
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            false,
            '',
            ''
        )
        "#,
    )
    .bind(device_type_id)
    .bind(&device_type_model)
    .bind(&device_type_slug)
    .bind(manufacturer_id)
    .execute(&pool)
    .await
    .expect("insert device type");

    let create_body = serde_json::json!({
        "name": "test-device-crud",
        "site_id": site_id,
        "role_id": role_id,
        "device_type_id": device_type_id.to_string()
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/devices",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("id");

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/devices/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let update_body = serde_json::json!({
        "name": "test-device-updated",
        "site_id": site_id,
        "role_id": role_id,
        "device_type_id": device_type_id.to_string()
    })
    .to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/devices/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/devices/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn lists_connection_targets_with_pagination_and_target_kind_filter()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let site_id = create_inventory_site(&app, &cookie, &suffix).await?;
    let role_id = create_inventory_device_role(&app, &cookie, &suffix).await?;
    let pool = build_test_db_pool().await.expect("db pool");
    let device_type_id = seed_device_type(&pool, &suffix).await?;
    let prefix = format!("conn-target-{}", suffix);
    let first_name = format!("{}-a", prefix);
    let second_name = format!("{}-b", prefix);

    let first = create_inventory_device(
        &app,
        &cookie,
        NewInventoryDevice {
            name: &first_name,
            site_id: &site_id,
            role_id: &role_id,
            device_type_id,
            primary_ip: "198.18.10.1",
        },
    )
    .await?;
    let second = create_inventory_device(
        &app,
        &cookie,
        NewInventoryDevice {
            name: &second_name,
            site_id: &site_id,
            role_id: &role_id,
            device_type_id,
            primary_ip: "198.18.10.2",
        },
    )
    .await?;

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets?search={prefix}&limit=1&offset=0"),
        None,
        Some(&cookie),
    )
    .await
    .expect("list first page");
    assert_eq!(status, StatusCode::OK);
    let first_page: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(first_page["total"], 2);
    assert_eq!(first_page["limit"], 1);
    assert_eq!(first_page["offset"], 0);
    assert_eq!(first_page["items"].as_array().expect("items").len(), 1);
    assert_eq!(first_page["items"][0]["id"], first);
    assert_eq!(first_page["items"][0]["name"], first_name);
    assert_eq!(first_page["items"][0]["target_kind"], "device");
    assert_eq!(first_page["items"][0]["address"], "198.18.10.1");
    assert_eq!(first_page["items"][0]["address_bound"], true);

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets?search={prefix}&limit=1&offset=1"),
        None,
        Some(&cookie),
    )
    .await
    .expect("list second page");
    assert_eq!(status, StatusCode::OK);
    let second_page: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(second_page["total"], 2);
    assert_eq!(second_page["offset"], 1);
    assert_eq!(second_page["items"].as_array().expect("items").len(), 1);
    assert_eq!(second_page["items"][0]["id"], second);
    assert_eq!(second_page["items"][0]["name"], second_name);

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets?search={prefix}&target_kind=device"),
        None,
        Some(&cookie),
    )
    .await
    .expect("target kind filter");
    assert_eq!(status, StatusCode::OK);
    let target_kind: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(target_kind["total"], 2);
    assert!(
        target_kind["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["target_kind"] == "device")
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets?search={prefix}&target_kind=virtual_machine"),
        None,
        Some(&cookie),
    )
    .await
    .expect("target kind excludes devices");
    assert_eq!(status, StatusCode::OK);
    let vm_filter: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(vm_filter["total"], 0);
    assert!(vm_filter["items"].as_array().expect("items").is_empty());
    Ok(())
}

#[tokio::test]
#[ignore]
async fn virtual_machine_connection_targets_fall_back_to_interface_ip_when_primary_is_missing()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let config_contexts = state.repositories.config_contexts.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let site_id = create_inventory_site(&app, &cookie, &suffix).await?;
    let role_id = create_inventory_device_role(&app, &cookie, &suffix).await?;
    let vm_name = format!("conn-target-vm-{suffix}");
    let vm_id =
        create_inventory_virtual_machine(&app, &cookie, &vm_name, &site_id, &role_id).await?;
    let vm_interface_id = create_inventory_vm_interface(&app, &cookie, &vm_id, "mgmt0").await?;
    create_inventory_ip_address(&app, &cookie, "198.18.20.10/24", &vm_interface_id).await?;

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/connections/targets?search={vm_name}&target_kind=virtual_machine"),
        None,
        Some(&cookie),
    )
    .await
    .expect("list vm targets");
    assert_eq!(status, StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["name"], vm_name);
    assert_eq!(payload["items"][0]["address"], "198.18.20.10");
    assert_eq!(payload["items"][0]["address_bound"], true);

    let namespace =
        piping_backend::contexts::inventory::infrastructure::persistence::ConfigContextRepository::load_target_variable_namespace(
            &config_contexts,
            Uuid::parse_str(&vm_id).expect("vm uuid"),
        )
        .await
        .expect("load target namespace")
        .expect("vm namespace");
    assert_eq!(namespace["name"], vm_name);
    assert_eq!(namespace["primary_ip4"], "198.18.20.10");
    assert!(
        namespace.get("primary_ip6").is_none(),
        "no IPv6 fallback should be injected when only IPv4 is assigned"
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn inventory_virtual_machines_crud() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let create_body = serde_json::json!({ "name": "test-vm-crud" }).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/inventory/virtual_machines",
        Some(&create_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let id = created.get("id").and_then(|v| v.as_str()).expect("id");

    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/virtual_machines/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let update_body = serde_json::json!({ "name": "test-vm-updated" }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/virtual_machines/{}", id),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/virtual_machines/{}", id),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

async fn create_inventory_site(
    app: &Router,
    cookie: &str,
    suffix: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let name = format!("conn-target-site-{suffix}");
    let body = serde_json::json!({ "name": name, "slug": name }).to_string();
    let (status, bytes) = request(
        app,
        "POST",
        "/api/v1/inventory/sites",
        Some(&body),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create site: {e}"))?;
    assert_eq!(status, StatusCode::CREATED);
    Ok(serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("site json: {e}"))?
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "site id".to_string())?
        .to_string())
}

async fn create_inventory_device_role(
    app: &Router,
    cookie: &str,
    suffix: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let name = format!("conn-target-role-{suffix}");
    let body = serde_json::json!({ "name": name, "slug": name }).to_string();
    let (status, bytes) = request(
        app,
        "POST",
        "/api/v1/inventory/device_roles",
        Some(&body),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create device role: {e}"))?;
    assert_eq!(status, StatusCode::CREATED);
    Ok(serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("role json: {e}"))?
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "role id".to_string())?
        .to_string())
}

async fn seed_device_type(
    pool: &sqlx::PgPool,
    suffix: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let manufacturer_id = Uuid::now_v7();
    let manufacturer_name = format!("conn-target-manufacturer-{suffix}");
    sqlx::query(
        r#"
        INSERT INTO dcim_manufacturer (id, custom_field_data, name, slug, description, comments)
        VALUES ($1, '{}'::jsonb, $2, $2, '', '')
        "#,
    )
    .bind(manufacturer_id)
    .bind(&manufacturer_name)
    .execute(pool)
    .await
    .map_err(|e| format!("insert manufacturer: {e}"))?;

    let device_type_id = Uuid::now_v7();
    let device_type_model = format!("conn-target-device-type-{suffix}");
    sqlx::query(
        r#"
        INSERT INTO dcim_devicetype (
            id,
            custom_field_data,
            model,
            slug,
            part_number,
            u_height,
            is_full_depth,
            subdevice_role,
            manufacturer_id,
            description,
            comments,
            interface_template_count,
            console_port_template_count,
            console_server_port_template_count,
            power_port_template_count,
            power_outlet_template_count,
            front_port_template_count,
            rear_port_template_count,
            device_bay_template_count,
            module_bay_template_count,
            inventory_item_template_count,
            device_count,
            exclude_from_utilization,
            front_image,
            rear_image
        )
        VALUES (
            $1,
            '{}'::jsonb,
            $2,
            $2,
            '',
            1.0,
            false,
            NULL,
            $3,
            '',
            '',
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            false,
            '',
            ''
        )
        "#,
    )
    .bind(device_type_id)
    .bind(&device_type_model)
    .bind(manufacturer_id)
    .execute(pool)
    .await
    .map_err(|e| format!("insert device type: {e}"))?;

    Ok(device_type_id)
}

/// Fields for one `POST /api/v1/inventory/devices` request.
struct NewInventoryDevice<'a> {
    name: &'a str,
    site_id: &'a str,
    role_id: &'a str,
    device_type_id: Uuid,
    primary_ip: &'a str,
}

async fn create_inventory_device(
    app: &Router,
    cookie: &str,
    device: NewInventoryDevice<'_>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let NewInventoryDevice {
        name,
        site_id,
        role_id,
        device_type_id,
        primary_ip,
    } = device;
    let body = serde_json::json!({
        "name": name,
        "site_id": site_id,
        "role_id": role_id,
        "device_type_id": device_type_id.to_string(),
        "primary_ip": primary_ip
    })
    .to_string();
    let (status, bytes) = request(
        app,
        "POST",
        "/api/v1/inventory/devices",
        Some(&body),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create device: {e}"))?;
    assert_eq!(status, StatusCode::CREATED);
    let json = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("device json: {e}"))?;
    assert_eq!(json["name"], name);
    assert_eq!(json["primary_ip"], primary_ip);
    Ok(json
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "device id".to_string())?
        .to_string())
}

async fn create_inventory_virtual_machine(
    app: &Router,
    cookie: &str,
    name: &str,
    site_id: &str,
    role_id: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({
        "name": name,
        "site_id": site_id,
        "role_id": role_id
    })
    .to_string();
    let (status, bytes) = request(
        app,
        "POST",
        "/api/v1/inventory/virtual_machines",
        Some(&body),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create virtual machine: {e}"))?;
    assert_eq!(status, StatusCode::CREATED);
    let json =
        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| format!("vm json: {e}"))?;
    assert_eq!(json["name"], name);
    assert!(
        json.get("primary_ip").is_none() || json["primary_ip"].is_null(),
        "fixture must keep primary_ip unset so fallback path is exercised"
    );
    Ok(json
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "vm id".to_string())?
        .to_string())
}

async fn create_inventory_vm_interface(
    app: &Router,
    cookie: &str,
    virtual_machine_id: &str,
    name: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({
        "virtual_machine_id": virtual_machine_id,
        "name": name
    })
    .to_string();
    let (status, bytes) = request(
        app,
        "POST",
        "/api/v1/inventory/vm-interfaces",
        Some(&body),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create vm interface: {e}"))?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let json = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("vm interface json: {e}"))?;
    assert_eq!(json["name"], name);
    Ok(json
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "vm interface id".to_string())?
        .to_string())
}

async fn create_inventory_interface(
    app: &Router,
    cookie: &str,
    device_id: &str,
    name: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({
        "device_id": device_id,
        "name": name
    })
    .to_string();
    let (status, bytes) = request(
        app,
        "POST",
        "/api/v1/inventory/interfaces",
        Some(&body),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create interface: {e}"))?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let json = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("interface json: {e}"))?;
    assert_eq!(json["name"], name);
    Ok(json
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "interface id".to_string())?
        .to_string())
}

async fn create_inventory_ip_address(
    app: &Router,
    cookie: &str,
    address: &str,
    interface_id: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({
        "address": address,
        "interface_id": interface_id
    })
    .to_string();
    let (status, bytes) = request(
        app,
        "POST",
        "/api/v1/inventory/ip-addresses",
        Some(&body),
        Some(cookie),
    )
    .await
    .map_err(|e| format!("create ip address: {e}"))?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let json =
        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| format!("ip json: {e}"))?;
    assert_eq!(json["address"], address);
    Ok(json
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "ip id".to_string())?
        .to_string())
}

#[tokio::test]
#[ignore]
async fn inventory_ip_addresses_support_search_and_hydrated_interface_names()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let site_id = create_inventory_site(&app, &cookie, &suffix).await?;
    let role_id = create_inventory_device_role(&app, &cookie, &suffix).await?;

    let first_vm_name = format!("ip-search-vm-{suffix}");
    let second_vm_name = format!("ip-search-vm-other-{suffix}");
    let first_interface_name = format!("mgmt-{suffix}");
    let second_interface_name = format!("data-{suffix}");

    let first_vm_id =
        create_inventory_virtual_machine(&app, &cookie, &first_vm_name, &site_id, &role_id).await?;
    let second_vm_id =
        create_inventory_virtual_machine(&app, &cookie, &second_vm_name, &site_id, &role_id)
            .await?;

    let first_interface_id =
        create_inventory_vm_interface(&app, &cookie, &first_vm_id, &first_interface_name).await?;
    let second_interface_id =
        create_inventory_vm_interface(&app, &cookie, &second_vm_id, &second_interface_name).await?;

    // The addresses must be unique per run: the suite shares one database with
    // every other test binary, and the exact-count search assertion below
    // fails on any collision with a leftover row. The previous scheme derived
    // the two variable octets from two HEX characters of the suffix — only
    // 2 x 16 = 32 combinations, so a dirty database collided within a few
    // dozen runs (observed: total=11 duplicates). Random uuid bytes give the
    // full 2^17 host space of 198.18.0.0/15 (the reserved benchmarking
    // range), and the rows are deleted at the end so leftovers stop
    // accumulating in the first place.
    // Host octets are three-digit (100..=227) so a search for the new address
    // can never substring-match the two-digit `.10`/`.11` leftovers the old
    // scheme deposited, and the even/odd pair never substring-match each
    // other.
    let addr_entropy = Uuid::now_v7();
    let addr_bytes = addr_entropy.as_bytes();
    let octet_c = u16::from(addr_bytes[14]) % 2;
    let octet_d = u16::from(addr_bytes[15]);
    let host_octet = 100 + (u16::from(addr_bytes[13]) % 64) * 2;
    let first_address = format!("198.{}.{}.{}/32", 18 + octet_c, octet_d, host_octet);
    let second_address = format!("198.{}.{}.{}/32", 18 + octet_c, octet_d, host_octet + 1);

    let first_ip_id =
        create_inventory_ip_address(&app, &cookie, &first_address, &first_interface_id).await?;
    let second_ip_id =
        create_inventory_ip_address(&app, &cookie, &second_address, &second_interface_id).await?;

    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/inventory/ip-addresses?search={}&limit=10&offset=0",
            first_address.trim_end_matches("/32")
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("search ip by address");
    assert_eq!(status, StatusCode::OK);
    let by_address: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(by_address["total"], 1);
    assert_eq!(by_address["items"][0]["address"], first_address);
    assert_eq!(
        by_address["items"][0]["interface_name"],
        first_interface_name
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/inventory/ip-addresses?search={}&limit=10&offset=0",
            first_interface_name
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("search ip by interface name");
    assert_eq!(status, StatusCode::OK);
    let by_interface: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(by_interface["total"], 1);
    assert_eq!(by_interface["items"][0]["address"], first_address);
    assert_eq!(
        by_interface["items"][0]["interface_name"],
        first_interface_name
    );

    // Delete the created addresses so reruns against the same database do not
    // accumulate rows for the exact-count assertions above to trip over.
    for ip_id in [&first_ip_id, &second_ip_id] {
        let _ = request(
            &app,
            "DELETE",
            &format!("/api/v1/inventory/ip-addresses/{ip_id}"),
            None,
            Some(&cookie),
        )
        .await;
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn inventory_interface_lists_include_parent_names_for_filter_hydration()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let site_id = create_inventory_site(&app, &cookie, &suffix).await?;
    let role_id = create_inventory_device_role(&app, &cookie, &suffix).await?;
    let device_type_id = seed_device_type(&db_pool, &suffix).await?;
    let device_name = format!("iface-filter-device-{suffix}");
    let virtual_machine_name = format!("iface-filter-vm-{suffix}");

    let device_id = create_inventory_device(
        &app,
        &cookie,
        NewInventoryDevice {
            name: &device_name,
            site_id: &site_id,
            role_id: &role_id,
            device_type_id,
            primary_ip: "198.18.50.10",
        },
    )
    .await?;
    let virtual_machine_id =
        create_inventory_virtual_machine(&app, &cookie, &virtual_machine_name, &site_id, &role_id)
            .await?;

    let interface_name = format!("mgmt-{suffix}");
    let vm_interface_name = format!("vm-mgmt-{suffix}");
    let interface_id =
        create_inventory_interface(&app, &cookie, &device_id, &interface_name).await?;
    let vm_interface_id =
        create_inventory_vm_interface(&app, &cookie, &virtual_machine_id, &vm_interface_name)
            .await?;

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/interfaces/{interface_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get interface");
    assert_eq!(status, StatusCode::OK);
    let interface_json: serde_json::Value = serde_json::from_slice(&body).expect("interface json");
    assert_eq!(interface_json["device_name"], device_name);

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/vm-interfaces/{vm_interface_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("get vm interface");
    assert_eq!(status, StatusCode::OK);
    let vm_interface_json: serde_json::Value =
        serde_json::from_slice(&body).expect("vm interface json");
    assert_eq!(
        vm_interface_json["virtual_machine_name"],
        virtual_machine_name
    );

    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/v1/inventory/interfaces?device_id={device_id}&limit=10&offset=0"),
        None,
        Some(&cookie),
    )
    .await
    .expect("list interfaces by device");
    assert_eq!(status, StatusCode::OK);
    let interface_list_json: serde_json::Value =
        serde_json::from_slice(&body).expect("interface list json");
    // The device was created with a primary_ip, which makes ensure_device_interface
    // auto-provision a default "mgmt0" management interface alongside the one this
    // test creates explicitly below — so 2 interfaces, not 1.
    assert_eq!(interface_list_json["total"], 2);
    let interfaces = interface_list_json["items"]
        .as_array()
        .expect("items array");
    let created_interface = interfaces
        .iter()
        .find(|item| item["name"] == interface_name)
        .expect("explicitly created interface present in list");
    assert_eq!(created_interface["device_name"], device_name);

    let (status, body) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/inventory/vm-interfaces?virtual_machine_id={virtual_machine_id}&limit=10&offset=0"
        ),
        None,
        Some(&cookie),
    )
    .await
    .expect("list vm interfaces by vm");
    assert_eq!(status, StatusCode::OK);
    let vm_interface_list_json: serde_json::Value =
        serde_json::from_slice(&body).expect("vm interface list json");
    assert_eq!(vm_interface_list_json["total"], 1);
    assert_eq!(
        vm_interface_list_json["items"][0]["virtual_machine_name"],
        virtual_machine_name
    );
    Ok(())
}

// --- Sync ---

#[tokio::test]
#[ignore]
async fn post_sync_jobs_nonexistent_provider_returns_404() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let body = serde_json::json!({
        "provider_id": "00000000-0000-0000-0000-000000000000",
        "mapping_version": "v1"
    })
    .to_string();

    let (status, body_bytes) = request(
        &app,
        "POST",
        "/api/v1/sync/jobs",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::NOT_FOUND);
    // The canonical envelope, not the ad-hoc `{error: ...}` this used to
    // assert. Pinning the old shape is precisely what let eighteen handlers
    // keep emitting a body the contract never promised; `code` and `message`
    // are what a generated client actually reads.
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(
        json.get("code").and_then(serde_json::Value::as_str),
        Some("NOT_FOUND"),
        "expected the ApiErrorResponse envelope, got {json}"
    );
    assert!(
        json.get("message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "the envelope must carry a human-readable message: {json}"
    );
}

#[tokio::test]
#[ignore]
async fn get_sync_job_nonexistent_returns_404() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body_bytes) = request(
        &app,
        "GET",
        "/api/v1/sync/jobs/00000000-0000-0000-0000-000000000000",
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::NOT_FOUND);
    // The canonical envelope, not the ad-hoc `{error: ...}` this used to
    // assert. Pinning the old shape is precisely what let eighteen handlers
    // keep emitting a body the contract never promised; `code` and `message`
    // are what a generated client actually reads.
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(
        json.get("code").and_then(serde_json::Value::as_str),
        Some("NOT_FOUND"),
        "expected the ApiErrorResponse envelope, got {json}"
    );
    assert!(
        json.get("message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "the envelope must carry a human-readable message: {json}"
    );
}

// --- Providers ---

/// `POST /providers/{id}/validate` dials the provider it is given, and the test
/// provider points at the local NetBox stack (`ensure_test_provider`). It needs
/// that service up — a database alone yields 502 — so it belongs to the
/// `make netbox-test` lane, and its name carries `netbox` so the DB lane's
/// `--skip netbox` filter excludes it.
#[tokio::test]
#[ignore = "requires a live NetBox (run via `make netbox-test`)"]
async fn post_providers_validate_returns_200_against_live_netbox() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/providers/{}/validate", TEST_PROVIDER_ID),
        None,
        Some(&cookie),
    )
    .await
    .expect("request");

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json.get("valid").and_then(|v| v.as_bool()), Some(true));
}

/// Reads the columns that carry a site's link to its upstream NetBox object.
///
/// `external_id` and `piping_synced_at` are absent from every REST request
/// body, so they can only be observed at the database level.
async fn read_site_sync_link(
    pool: &sqlx::PgPool,
    site_id: Uuid,
) -> Result<
    (String, Option<i64>, Option<chrono::DateTime<Utc>>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let name: String = sqlx::query_scalar("SELECT name FROM dcim_site WHERE id = $1")
        .bind(site_id)
        .fetch_one(pool)
        .await?;
    let external_id: Option<i64> =
        sqlx::query_scalar("SELECT external_id FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_one(pool)
            .await?;
    let synced_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT piping_synced_at FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_one(pool)
            .await?;
    Ok((name, external_id, synced_at))
}

/// Editing an unrelated field through REST must not sever a synced record's
/// link to its upstream NetBox object.
///
/// The sync link is seeded with SQL on purpose. `external_id` and
/// `piping_synced_at` cannot be expressed in a REST payload, so a fixture
/// created through POST alone carries NULL in both columns before *and* after
/// the update — such a test passes whether or not the defect is present. Only a
/// row that actually holds a sync link can observe the update erasing it.
#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_rest -- --ignored
async fn inventory_site_update_preserves_netbox_sync_link()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let db_pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let site_id = Uuid::parse_str(&create_inventory_site(&app, &cookie, &suffix).await?)?;

    // Whole seconds keep the value exact across the timestamptz round trip.
    let seeded_external_id: i64 = 424_242;
    let seeded_synced_at: chrono::DateTime<Utc> = "2026-01-02T03:04:05Z".parse()?;
    sqlx::query("UPDATE dcim_site SET external_id = $2, piping_synced_at = $3 WHERE id = $1")
        .bind(site_id)
        .bind(seeded_external_id)
        .bind(seeded_synced_at)
        .execute(&db_pool)
        .await?;

    // Guard: prove the seed landed, so a green run cannot mean "nothing was set".
    let (seeded_name, external_id, synced_at) = read_site_sync_link(&db_pool, site_id).await?;
    assert_eq!(
        external_id,
        Some(seeded_external_id),
        "seed did not land: external_id must hold a sync link before the update"
    );
    assert_eq!(
        synced_at,
        Some(seeded_synced_at),
        "seed did not land: piping_synced_at must hold a sync link before the update"
    );

    let renamed = format!("synced-site-renamed-{suffix}");
    assert_ne!(seeded_name, renamed, "rename must be an observable change");
    let update_body = serde_json::json!({ "name": renamed }).to_string();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/sites/{site_id}"),
        Some(&update_body),
        Some(&cookie),
    )
    .await
    .expect("update site");
    assert_eq!(status, StatusCode::OK);

    let (name, external_id, synced_at) = read_site_sync_link(&db_pool, site_id).await?;
    assert_eq!(
        external_id,
        Some(seeded_external_id),
        "updating a site through REST erased external_id, breaking its NetBox link"
    );
    assert_eq!(
        synced_at,
        Some(seeded_synced_at),
        "updating a site through REST erased piping_synced_at, breaking its NetBox link"
    );
    // The update must still apply what the caller actually sent.
    assert_eq!(name, renamed, "the requested rename was not applied");

    let _ = request(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/sites/{site_id}"),
        None,
        Some(&cookie),
    )
    .await;
    Ok(())
}

// --- Admin: role-assignments, policy-bindings ---
