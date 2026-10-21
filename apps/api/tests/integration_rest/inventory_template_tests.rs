use super::*;

async fn template_test_post(
    app: &Router,
    cookie: &str,
    path: &str,
    body: serde_json::Value,
    expected: StatusCode,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let body = body.to_string();
    let (status, response) = request(app, "POST", path, Some(&body), Some(cookie)).await?;
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(status, expected, "{path}: {response}");
    Ok(response)
}

// These native REST tests use the same dedicated PostgreSQL/Redis fixture as
// the other ignored integration tests; they must not run against a shared DB.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL and Redis integration fixtures"]
async fn inventory_templates_structured_references_preserve_selected_device_foreign_keys()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let suffix = unique_suffix();
    let mut types = Vec::new();
    for index in 0..2 {
        let manufacturer = template_test_post(
            &app,
            &cookie,
            "/api/v1/inventory/manufacturers",
            serde_json::json!({
                "name": format!("reference-mfr-{index}-{suffix}"),
                "slug": format!("reference-mfr-{index}-{suffix}")
            }),
            StatusCode::CREATED,
        )
        .await?;
        types.push(
            template_test_post(
                &app,
                &cookie,
                "/api/v1/inventory/device_types",
                serde_json::json!({
                    "manufacturer_id": manufacturer["id"],
                    "model": format!("same-model-{suffix}"),
                    "slug": format!("same-model-{suffix}")
                }),
                StatusCode::CREATED,
            )
            .await?,
        );
    }
    let mut roles = Vec::new();
    for (index, name) in ["Core-Switch", "Core Switch"].into_iter().enumerate() {
        roles.push(
            template_test_post(
                &app,
                &cookie,
                "/api/v1/inventory/device_roles",
                serde_json::json!({
                    "name": format!("{name}-{suffix}"),
                    "slug": format!("reference-role-{index}-{suffix}")
                }),
                StatusCode::CREATED,
            )
            .await?,
        );
    }
    let site_name = format!("reference-site-{suffix}");
    let device_name = format!("reference-device-{suffix}");
    let template = template_test_post(
        &app,
        &cookie,
        "/api/v1/inventory/templates",
        serde_json::json!({
            "name": format!("reference-template-{suffix}"),
            "status": "active",
            "variables": [
                { "key": "role", "value_type": "string", "required": true,
                  "validation_rules": { "resource": "device_role" } },
                { "key": "device_type", "value_type": "string", "required": true,
                  "validation_rules": { "resource": "device_type" } },
                { "key": "metadata", "value_type": "json", "required": true }
            ],
            "steps": [
                { "step_key": "site", "resource_type": "site", "operation": "create",
                  "payload_template": { "name": site_name, "metadata": "{{metadata}}" } },
                { "step_key": "device", "resource_type": "device", "operation": "create",
                  "depends_on_step_keys": ["site"],
                  "payload_template": { "name": device_name, "site_id": "{{steps.site.id}}",
                    "role": "{{role}}", "device_type": "{{device_type}}" } }
            ]
        }),
        StatusCode::CREATED,
    )
    .await?;
    assert_eq!(
        template.pointer("/steps/1/payload_template/role"),
        Some(&serde_json::json!("{{role}}"))
    );
    assert_eq!(
        template.pointer("/steps/1/payload_template/device_type"),
        Some(&serde_json::json!("{{device_type}}"))
    );
    let template_id = template["id"].as_str().expect("template id");
    // Reference-shaped ordinary JSON is not a selection unless the binding is
    // a reference field. Its deliberately invalid UUID must remain untouched.
    let metadata = serde_json::json!({ "resource": "device_type", "id": "ordinary-json" });
    let variables = serde_json::json!({
        "role": { "resource": "device_role", "id": roles[1]["id"] },
        "device_type": { "resource": "device_type", "id": types[1]["id"] },
        "metadata": metadata
    });
    let preview = template_test_post(
        &app,
        &cookie,
        &format!("/api/v1/inventory/templates/{template_id}/preview-run"),
        serde_json::json!({ "variables": variables }),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        preview.pointer("/steps/1/resolved_payload/role_id"),
        Some(&roles[1]["id"])
    );
    assert_eq!(
        preview.pointer("/steps/1/resolved_payload/device_type_id"),
        Some(&types[1]["id"])
    );
    assert!(preview.pointer("/steps/1/resolved_payload/role").is_none());
    assert!(
        preview
            .pointer("/steps/1/resolved_payload/device_type")
            .is_none()
    );
    assert_eq!(
        preview.pointer("/steps/0/resolved_payload/metadata"),
        Some(&metadata)
    );
    assert_eq!(preview.pointer("/variables/metadata"), Some(&metadata));

    let run = template_test_post(
        &app,
        &cookie,
        &format!("/api/v1/inventory/templates/{template_id}/runs"),
        serde_json::json!({ "variables": variables }),
        StatusCode::ACCEPTED,
    )
    .await?;
    assert_eq!(run["status"], "completed", "{run}");
    assert_eq!(
        run.pointer("/summary/succeeded_steps"),
        Some(&serde_json::json!(2))
    );
    let pool = build_test_db_pool().await.expect("db pool");
    let persisted: (Uuid, Uuid) =
        sqlx::query_as("SELECT role_id, device_type_id FROM dcim_device WHERE name = $1")
            .bind(&device_name)
            .fetch_one(&pool)
            .await
            .expect("persisted device");
    assert_eq!(
        persisted.0.to_string(),
        roles[1]["id"].as_str().expect("role id")
    );
    assert_eq!(
        persisted.1.to_string(),
        types[1]["id"].as_str().expect("type id")
    );

    // The same model is still accepted as a literal manual value, but it must
    // not choose an arbitrary manufacturer when two device types match.
    let site_id: Uuid = sqlx::query_scalar("SELECT id FROM dcim_site WHERE name = $1")
        .bind(&site_name)
        .fetch_one(&pool)
        .await
        .expect("created site");
    let ambiguous_name = format!("ambiguous-device-{suffix}");
    let manual_template = template_test_post(
        &app,
        &cookie,
        "/api/v1/inventory/templates",
        serde_json::json!({
            "name": format!("ambiguous-type-{suffix}"), "status": "active",
            "variables": [{ "key": "device_type", "value_type": "string", "required": true }],
            "steps": [{ "step_key": "device", "resource_type": "device", "operation": "create",
                "payload_template": { "name": ambiguous_name, "site_id": site_id,
                    "role_id": roles[1]["id"], "device_type": "{{device_type}}" } }]
        }),
        StatusCode::CREATED,
    )
    .await?;
    let manual_id = manual_template["id"].as_str().expect("manual template id");
    let manual_variables =
        serde_json::json!({ "variables": { "device_type": format!("same-model-{suffix}") } });
    let manual_preview = template_test_post(
        &app,
        &cookie,
        &format!("/api/v1/inventory/templates/{manual_id}/preview-run"),
        manual_variables.clone(),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        manual_preview.pointer("/steps/0/resolved_payload/device_type"),
        Some(&serde_json::json!(format!("same-model-{suffix}")))
    );
    let rejected_run = template_test_post(
        &app,
        &cookie,
        &format!("/api/v1/inventory/templates/{manual_id}/runs"),
        manual_variables,
        StatusCode::ACCEPTED,
    )
    .await?;
    assert_eq!(
        rejected_run["status"], "completed_with_errors",
        "{rejected_run}"
    );
    assert_eq!(
        rejected_run.pointer("/summary/failed_steps"),
        Some(&serde_json::json!(1))
    );
    let devices: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_device WHERE name = $1")
        .bind(&ambiguous_name)
        .fetch_one(&pool)
        .await
        .expect("ambiguous device count");
    assert_eq!(
        devices, 0,
        "ambiguous manual model must not select the first match"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL and Redis integration fixtures"]
async fn inventory_templates_invalid_references_reject_preview_and_run_before_writes()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let suffix = unique_suffix();
    let manufacturer = template_test_post(
        &app, &cookie, "/api/v1/inventory/manufacturers",
        serde_json::json!({ "name": format!("wrong-target-{suffix}"), "slug": format!("wrong-target-{suffix}") }),
        StatusCode::CREATED,
    ).await?;
    let site_name = format!("must-not-create-{suffix}");
    let template = template_test_post(
        &app,
        &cookie,
        "/api/v1/inventory/templates",
        serde_json::json!({
            "name": format!("invalid-reference-{suffix}"), "status": "active",
            "variables": [{ "key": "device_type", "value_type": "string", "required": true,
                            "validation_rules": { "resource": "device_type" } }],
            "steps": [
                { "step_key": "site", "resource_type": "site", "operation": "create",
                  "payload_template": { "name": site_name } },
                { "step_key": "device", "resource_type": "device", "operation": "create",
                  "depends_on_step_keys": ["site"],
                  "payload_template": { "name": format!("invalid-device-{suffix}"),
                    "site_id": "{{steps.site.id}}", "device_type": "{{device_type}}" } }
            ]
        }),
        StatusCode::CREATED,
    )
    .await?;
    let template_id = template["id"].as_str().expect("template id");
    let invalid_values = [
        serde_json::json!({ "resource": "device_type", "id": Uuid::now_v7() }),
        serde_json::json!({ "resource": "device_type", "id": manufacturer["id"] }),
        serde_json::json!({ "resource": "manufacturer", "id": manufacturer["id"] }),
        serde_json::json!({ "resource": "device_type", "id": "not-a-uuid" }),
        serde_json::json!({ "resource": "device_type" }),
        serde_json::json!({ "id": manufacturer["id"] }),
        serde_json::json!({ "resource": "device_type", "id": 123 }),
        serde_json::json!({ "resource": "device_type", "id": manufacturer["id"], "label": "extra" }),
    ];
    for value in invalid_values {
        for endpoint in ["preview-run", "runs"] {
            let response = template_test_post(
                &app,
                &cookie,
                &format!("/api/v1/inventory/templates/{template_id}/{endpoint}"),
                serde_json::json!({ "variables": { "device_type": value } }),
                StatusCode::BAD_REQUEST,
            )
            .await?;
            assert_eq!(response["code"], "VALIDATION_ERROR", "{value}: {response}");
        }
    }
    let pool = build_test_db_pool().await.expect("db pool");
    let sites: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_site WHERE name = $1")
        .bind(&site_name)
        .fetch_one(&pool)
        .await
        .expect("site count");
    assert_eq!(
        sites, 0,
        "reference validation must precede the first step write"
    );
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_template_run WHERE template_id = $1")
            .bind(Uuid::parse_str(template_id).expect("template UUID"))
            .fetch_one(&pool)
            .await
            .expect("run count");
    assert_eq!(runs, 0, "invalid references must not create run records");
    Ok(())
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL and Redis integration fixtures"]
async fn inventory_templates_manual_site_name_still_resolves_for_prefix_after_creation()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let suffix = unique_suffix();
    let site_name = format!("manual-site-{suffix}");
    let template = template_test_post(
        &app,
        &cookie,
        "/api/v1/inventory/templates",
        serde_json::json!({
            "name": format!("manual-site-prefix-{suffix}"), "status": "active",
            "variables": [{ "key": "site_name", "value_type": "string", "required": true }],
            "steps": [
                { "step_key": "site", "resource_type": "site", "operation": "create",
                  "payload_template": { "name": "{{site_name}}" } },
                { "step_key": "prefix", "resource_type": "prefix", "operation": "create",
                  "depends_on_step_keys": ["site"],
                  "payload_template": { "prefix": "198.18.111.0/24", "site": "{{site_name}}" } }
            ]
        }),
        StatusCode::CREATED,
    )
    .await?;
    let template_id = template["id"].as_str().expect("template id");
    let body = serde_json::json!({ "variables": { "site_name": site_name } });
    let preview = template_test_post(
        &app,
        &cookie,
        &format!("/api/v1/inventory/templates/{template_id}/preview-run"),
        body.clone(),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        preview.pointer("/steps/0/resolved_payload/name"),
        Some(&serde_json::json!(site_name))
    );
    assert_eq!(
        preview.pointer("/steps/1/resolved_payload/site"),
        Some(&serde_json::json!(site_name))
    );
    let run = template_test_post(
        &app,
        &cookie,
        &format!("/api/v1/inventory/templates/{template_id}/runs"),
        body,
        StatusCode::ACCEPTED,
    )
    .await?;
    assert_eq!(run["status"], "completed", "{run}");
    let pool = build_test_db_pool().await.expect("db pool");
    let prefixes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ipam_prefix p JOIN dcim_site s ON s.id = p._site_id WHERE s.name = $1 AND p.prefix = '198.18.111.0/24'::cidr",
    ).bind(&site_name).fetch_one(&pool).await.expect("persisted prefix site");
    assert_eq!(prefixes, 1);
    Ok(())
}

#[tokio::test]
#[ignore]
async fn inventory_templates_create_accepts_canonical_step_shape() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let template_name = format!("inventory-template-{suffix}");
    let template_slug = format!("inventory-template-{suffix}");
    let body = serde_json::json!({
        "name": template_name.clone(),
        "slug": template_slug,
        "status": "draft",
        "variables": [
            {
                "key": "site_name",
                "label": "Site name",
                "value_type": "string",
                "required": true,
                "default_value": format!("site-{suffix}"),
                "validation_rules": {}
            }
        ],
        "steps": [
            {
                "step_key": "site",
                "title": "Create site",
                "resource_type": "site",
                "operation": "create",
                "depends_on_step_keys": [],
                "payload_template": {
                    "name": "{{site_name}}"
                },
                "on_conflict_policy": "fail"
            }
        ]
    })
    .to_string();

    let (status, response_body) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create template");
    assert_eq!(status, StatusCode::CREATED);

    let json: serde_json::Value = serde_json::from_slice(&response_body).expect("template json");
    assert_eq!(
        json.get("name").and_then(|value| value.as_str()),
        Some(template_name.as_str())
    );
    assert_eq!(
        json.pointer("/steps/0/resource_type")
            .and_then(|value| value.as_str()),
        Some("site")
    );
    assert_eq!(
        json.pointer("/variables/0/value_type")
            .and_then(|value| value.as_str()),
        Some("string")
    );
}

#[tokio::test]
#[ignore]
async fn inventory_templates_delete_removes_template() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let body = serde_json::json!({
        "name": format!("inventory-template-delete-{suffix}"),
        "slug": format!("inventory-template-delete-{suffix}"),
        "status": "draft",
        "steps": []
    })
    .to_string();

    let (status, response_body) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create template");
    assert_eq!(status, StatusCode::CREATED);

    let json: serde_json::Value = serde_json::from_slice(&response_body).expect("template json");
    let template_id = json
        .get("id")
        .and_then(|value| value.as_str())
        .expect("template id");
    let template_path = format!("/api/v1/inventory/templates/{template_id}");

    let (status, _) = request(&app, "DELETE", &template_path, None, Some(&cookie))
        .await
        .expect("delete template");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = request(&app, "GET", &template_path, None, Some(&cookie))
        .await
        .expect("get deleted template");
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn inventory_templates_create_rejects_legacy_resource_type_aliases() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let body = serde_json::json!({
        "name": format!("inventory-template-alias-{suffix}"),
        "slug": format!("inventory-template-alias-{suffix}"),
        "status": "draft",
        "steps": [
            {
                "step_key": "site",
                "title": "Create site",
                "resource_type": "sites",
                "operation": "create",
                "depends_on_step_keys": [],
                "payload_template": {
                    "name": "site"
                },
                "on_conflict_policy": "fail"
            }
        ]
    })
    .to_string();

    let (status, response_body) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create template with legacy alias");
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let json: serde_json::Value = serde_json::from_slice(&response_body).expect("validation json");
    assert_eq!(
        json.get("code").and_then(|value| value.as_str()),
        Some("VALIDATION_ERROR")
    );
    assert!(
        json.get("message")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .contains("steps[0].resource_type"),
        "expected resource_type validation error, got: {json}"
    );
}

#[tokio::test]
#[ignore]
async fn inventory_templates_create_accepts_newly_supported_resource_types() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    // `manufacturer`, `cluster_type`, and `region` were NOT in the original
    // closed 7-arm set; authoring must now accept them via the shared allow-list.
    let body = serde_json::json!({
        "name": format!("inventory-template-newtypes-{suffix}"),
        "slug": format!("inventory-template-newtypes-{suffix}"),
        "status": "draft",
        "steps": [
            {
                "step_key": "manufacturer",
                "title": "Create manufacturer",
                "resource_type": "manufacturer",
                "operation": "create",
                "depends_on_step_keys": [],
                "payload_template": { "name": format!("mfr-{suffix}") },
                "on_conflict_policy": "fail"
            },
            {
                "step_key": "cluster_type",
                "title": "Create cluster type",
                "resource_type": "cluster_type",
                "operation": "create",
                "depends_on_step_keys": [],
                "payload_template": { "name": format!("ctype-{suffix}") },
                "on_conflict_policy": "fail"
            }
        ]
    })
    .to_string();

    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create template with newly supported types");
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
#[ignore]
async fn inventory_templates_run_creates_newly_supported_entities() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    // Active template so it can run; two new entity types with no required FKs.
    let body = serde_json::json!({
        "name": format!("inventory-template-run-{suffix}"),
        "slug": format!("inventory-template-run-{suffix}"),
        "status": "active",
        "steps": [
            {
                "step_key": "manufacturer",
                "title": "Create manufacturer",
                "resource_type": "manufacturer",
                "operation": "create",
                "depends_on_step_keys": [],
                "payload_template": { "name": format!("run-mfr-{suffix}") },
                "on_conflict_policy": "fail"
            },
            {
                "step_key": "cluster_type",
                "title": "Create cluster type",
                "resource_type": "cluster_type",
                "operation": "create",
                "depends_on_step_keys": [],
                "payload_template": { "name": format!("run-ctype-{suffix}") },
                "on_conflict_policy": "fail"
            }
        ]
    })
    .to_string();

    let (status, response_body) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create active template");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&response_body).expect("template json");
    let template_id = created
        .get("id")
        .and_then(|value| value.as_str())
        .expect("template id");

    let run_path = format!("/api/v1/inventory/templates/{template_id}/runs");
    let run_body = serde_json::json!({ "variables": {} }).to_string();
    let (run_status, run_response) =
        request(&app, "POST", &run_path, Some(&run_body), Some(&cookie))
            .await
            .expect("create template run");
    assert_eq!(run_status, StatusCode::ACCEPTED);

    let run_json: serde_json::Value = serde_json::from_slice(&run_response).expect("run json");
    assert_eq!(
        run_json.get("status").and_then(|value| value.as_str()),
        Some("completed"),
        "run should complete cleanly, got: {run_json}"
    );
    assert_eq!(
        run_json
            .pointer("/summary/succeeded_steps")
            .and_then(serde_json::Value::as_u64),
        Some(2),
        "both new-entity steps should succeed, got: {run_json}"
    );
    assert_eq!(
        run_json
            .pointer("/summary/failed_steps")
            .and_then(serde_json::Value::as_u64),
        Some(0),
        "no step should fail, got: {run_json}"
    );
}

#[tokio::test]
#[ignore]
async fn inventory_templates_create_rejects_unknown_step_dependencies() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let body = serde_json::json!({
        "name": format!("inventory-template-deps-{suffix}"),
        "slug": format!("inventory-template-deps-{suffix}"),
        "status": "draft",
        "steps": [
            {
                "step_key": "device",
                "title": "Create device",
                "resource_type": "device",
                "operation": "create",
                "depends_on_step_keys": ["site"],
                "payload_template": {
                    "name": "device",
                    "site_id": "00000000-0000-7000-8000-000000000111",
                    "role_id": "00000000-0000-7000-8000-000000000112",
                    "device_type_id": "00000000-0000-7000-8000-000000000113"
                },
                "on_conflict_policy": "fail"
            }
        ]
    })
    .to_string();

    let (status, response_body) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create template with unknown dependency");
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let json: serde_json::Value = serde_json::from_slice(&response_body).expect("validation json");
    assert_eq!(
        json.get("code").and_then(|value| value.as_str()),
        Some("VALIDATION_ERROR")
    );
    assert!(
        json.get("message")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .contains("depends_on_step_keys references unknown step"),
        "expected dependency validation error, got: {json}"
    );
}

#[tokio::test]
#[ignore]
async fn inventory_templates_partial_update_keeps_variables_and_steps() {
    // A PUT that only changes the name used to arrive at the repository with
    // `variables: vec![]` and `steps: vec![]`, because the service turned an
    // absent field into an empty one. Both replacements start with a DELETE, so
    // the template came back stripped of everything it was made of.
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let body = serde_json::json!({
        "name": format!("partial-update-{suffix}"),
        "slug": format!("partial-update-{suffix}"),
        "status": "draft",
        "variables": [{
            "key": "site_name",
            "label": "Site name",
            "value_type": "string",
            "required": true,
            "default_value": format!("site-{suffix}"),
            "validation_rules": {}
        }],
        "steps": [{
            "step_key": "site",
            "title": "Create site",
            "resource_type": "site",
            "operation": "create",
            "depends_on_step_keys": [],
            "payload_template": {"name": "{{site_name}}"},
            "on_conflict_policy": "fail"
        }]
    })
    .to_string();

    let (status, created) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create template");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&created).expect("template json");
    let template_id = created
        .get("id")
        .and_then(|value| value.as_str())
        .expect("template id")
        .to_string();

    // Rename only. Nothing is said about variables, steps or the system flag.
    let rename = serde_json::json!({
        "name": format!("partial-update-renamed-{suffix}"),
        "status": "draft"
    })
    .to_string();
    let (status, updated) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/templates/{template_id}"),
        Some(&rename),
        Some(&cookie),
    )
    .await
    .expect("update template");
    assert_eq!(status, StatusCode::OK);

    let updated: serde_json::Value = serde_json::from_slice(&updated).expect("template json");
    assert_eq!(
        updated.get("name").and_then(|value| value.as_str()),
        Some(format!("partial-update-renamed-{suffix}").as_str()),
        "the field that was sent must change"
    );
    assert_eq!(
        updated
            .get("variables")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(1),
        "a rename must not delete the template's variables"
    );
    assert_eq!(
        updated
            .get("steps")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(1),
        "a rename must not delete the template's steps"
    );
    assert_eq!(
        updated.pointer("/variables/0/key").and_then(|v| v.as_str()),
        Some("site_name"),
        "the surviving variable must be the one that was there"
    );

    // And an explicit empty list still means empty - absent and empty are
    // different requests, which is the whole point of the fix.
    let clear = serde_json::json!({
        "name": format!("partial-update-renamed-{suffix}"),
        "status": "draft",
        "variables": [],
        "steps": []
    })
    .to_string();
    let (status, cleared) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/templates/{template_id}"),
        Some(&clear),
        Some(&cookie),
    )
    .await
    .expect("clear template contents");
    assert_eq!(status, StatusCode::OK);
    let cleared: serde_json::Value = serde_json::from_slice(&cleared).expect("template json");
    assert_eq!(
        cleared
            .get("variables")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(0),
        "an explicit empty list must still clear them"
    );
}

#[tokio::test]
#[ignore]
async fn inventory_templates_partial_update_keeps_status() {
    // `parse_template_status(None)` resolved to "draft", which is the right
    // default for a template being created and the wrong one for a template
    // being renamed: a PUT that said nothing about status demoted an active
    // template back to a draft.
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let suffix = unique_suffix();
    let body = serde_json::json!({
        "name": format!("status-keep-{suffix}"),
        "slug": format!("status-keep-{suffix}"),
        "status": "active"
    })
    .to_string();
    let (status, created) = request(
        &app,
        "POST",
        "/api/v1/inventory/templates",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create template");
    assert_eq!(status, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&created).expect("template json");
    assert_eq!(
        created.get("status").and_then(|v| v.as_str()),
        Some("active"),
        "the fixture must actually start active, or this test proves nothing"
    );
    let template_id = created
        .get("id")
        .and_then(|value| value.as_str())
        .expect("template id")
        .to_string();

    // Rename only. Nothing said about status.
    let rename = serde_json::json!({ "name": format!("status-keep-renamed-{suffix}") }).to_string();
    let (status, updated) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/templates/{template_id}"),
        Some(&rename),
        Some(&cookie),
    )
    .await
    .expect("update template");
    assert_eq!(status, StatusCode::OK);

    let updated: serde_json::Value = serde_json::from_slice(&updated).expect("template json");
    assert_eq!(
        updated.get("status").and_then(|v| v.as_str()),
        Some("active"),
        "a rename must not demote an active template to a draft"
    );
    assert_eq!(
        updated.get("name").and_then(|v| v.as_str()),
        Some(format!("status-keep-renamed-{suffix}").as_str()),
        "the field that was sent must still change"
    );

    // An explicit status is still honoured - absent and stated are different.
    let demote = serde_json::json!({
        "name": format!("status-keep-renamed-{suffix}"),
        "status": "draft"
    })
    .to_string();
    let (status, demoted) = request(
        &app,
        "PUT",
        &format!("/api/v1/inventory/templates/{template_id}"),
        Some(&demote),
        Some(&cookie),
    )
    .await
    .expect("demote template");
    assert_eq!(status, StatusCode::OK);
    let demoted: serde_json::Value = serde_json::from_slice(&demoted).expect("template json");
    assert_eq!(
        demoted.get("status").and_then(|v| v.as_str()),
        Some("draft"),
        "an explicit status must still be applied"
    );
}
