//! Monitoring profile REST behaviour: CRUD, apply, preview, effective profiles
//! for a target, delete-while-assigned, and job/target conflict rejection.
//!
//! Every case drives the in-process router against the disposable database, so
//! validation, repository and conflict checks execute through real requests.

use super::common::seed_device;
use super::*;

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn admin_router_and_cookie() -> (Router, String, sqlx::PgPool) {
    let state = build_test_app_state().await.expect("app state");
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("admin login");
    (app, cookie, pool)
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn exporter_id(app: &Router, cookie: &str, slug: &str) -> String {
    let body = list_monitoring(
        app,
        cookie,
        &format!("/api/v1/monitoring/exporters?sort=slug&order=asc"),
    )
    .await;
    body["items"]
        .as_array()
        .expect("exporter items")
        .iter()
        .find(|item| item["slug"].as_str() == Some(slug))
        .unwrap_or_else(|| panic!("seeded exporter `{slug}` must exist"))["id"]
        .as_str()
        .expect("exporter id")
        .to_string()
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn list_monitoring(app: &Router, cookie: &str, uri: &str) -> serde_json::Value {
    let (status, body) = request(app, "GET", uri, None, Some(cookie))
        .await
        .expect("list request");
    assert_eq!(
        status,
        StatusCode::OK,
        "list {uri} body={}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("list json")
}

#[allow(
    clippy::expect_used,
    reason = "shared fixture helper, outside the #[test] fns clippy.toml exempts"
)]
async fn create_profile(
    app: &Router,
    cookie: &str,
    body: serde_json::Value,
    expected: StatusCode,
) -> (StatusCode, serde_json::Value) {
    let (status, response) = request(
        app,
        "POST",
        "/api/v1/monitoring/profiles",
        Some(&body.to_string()),
        Some(cookie),
    )
    .await
    .expect("create profile request");
    let parsed = if response.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&response).unwrap_or(serde_json::Value::Null)
    };
    assert_eq!(
        status,
        expected,
        "create profile body={}",
        String::from_utf8_lossy(&response)
    );
    (status, parsed)
}

#[tokio::test]
#[ignore]
async fn monitoring_profiles_crud_apply_preview_and_delete_guard() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _host_name) = seed_device(&pool, &format!("itest-profile-{suffix}"))
        .await
        .expect("seed host");

    let node = exporter_id(&app, &cookie, "node").await;
    let process = exporter_id(&app, &cookie, "process").await;
    let smartctl = exporter_id(&app, &cookie, "smartctl").await;

    let profile_body = serde_json::json!({
        "name": format!("Linux baseline {suffix}"),
        "slug": format!("linux-baseline-{suffix}"),
        "description": "three exporter items",
        "enabled": true,
        "items": [
            { "exporter_id": node, "sort_order": 0 },
            { "exporter_id": process, "sort_order": 1, "job": "process" },
            { "exporter_id": smartctl, "sort_order": 2, "metrics_path": "smart" },
        ],
    });
    let (_, profile) = create_profile(&app, &cookie, profile_body, StatusCode::CREATED).await;
    let profile_id = profile["id"].as_str().expect("profile id").to_string();
    assert_eq!(profile["items"].as_array().expect("items").len(), 3);
    assert_eq!(profile["assignment_count"].as_i64(), Some(0));

    // Idempotent apply: first creates, second returns the same assignment.
    let apply_path = format!("/api/v1/monitoring/profiles/{profile_id}/apply");
    let apply_body = serde_json::json!({
        "scope_type": "connection_target",
        "scope_id": host_id,
        "weight": 3,
    })
    .to_string();
    let (status, body) = request(&app, "POST", &apply_path, Some(&apply_body), Some(&cookie))
        .await
        .expect("apply profile");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "apply body={}",
        String::from_utf8_lossy(&body)
    );
    let applied: serde_json::Value = serde_json::from_slice(&body).expect("apply json");
    let assignment_id = applied["id"].as_str().expect("assignment id").to_string();
    assert_eq!(applied["profile_id"].as_str(), Some(profile_id.as_str()));

    let (status, body) = request(&app, "POST", &apply_path, Some(&apply_body), Some(&cookie))
        .await
        .expect("re-apply profile");
    assert_eq!(
        status,
        StatusCode::OK,
        "re-apply body={}",
        String::from_utf8_lossy(&body)
    );
    let reapplied: serde_json::Value = serde_json::from_slice(&body).expect("re-apply json");
    assert_eq!(reapplied["id"].as_str(), Some(assignment_id.as_str()));

    let preview = list_monitoring(
        &app,
        &cookie,
        &format!(
            "/api/v1/monitoring/profiles/{profile_id}/preview?scope_type=connection_target&scope_id={host_id}"
        ),
    )
    .await;
    assert_eq!(
        preview["targets_count"].as_u64(),
        Some(3),
        "three items must expand to three targets: {}",
        serde_json::to_string(&preview).unwrap_or_default()
    );
    assert!(
        preview["conflicts"]
            .as_array()
            .expect("conflicts")
            .is_empty()
    );

    let effective = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/targets/{host_id}/profiles"),
    )
    .await;
    let profiles = effective["profiles"]
        .as_array()
        .expect("effective profiles");
    let found = profiles
        .iter()
        .find(|entry| entry["profile_id"].as_str() == Some(profile_id.as_str()))
        .expect("effective profile for host");
    assert_eq!(found["items"].as_array().expect("items").len(), 3);

    // Delete is refused while an assignment still references the profile.
    let (status, body) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/profiles/{profile_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete assigned profile");
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "delete assigned profile body={}",
        String::from_utf8_lossy(&body)
    );

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/profile-assignments/{assignment_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete assignment");
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/profiles/{profile_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("delete unassigned profile");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn replacing_assignment_excludes_only_that_assignment() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _) = seed_device(&pool, &format!("itest-replace-{suffix}"))
        .await
        .expect("seed host");
    let node = exporter_id(&app, &cookie, "node").await;
    let mut profiles = Vec::new();
    for prefix in ["first", "second"] {
        let (_, profile) = create_profile(
            &app,
            &cookie,
            serde_json::json!({
                "name": format!("{prefix} {suffix}"),
                "slug": format!("{prefix}-{suffix}"),
                "items": [{ "exporter_id": node, "job": "replacement-job" }],
            }),
            StatusCode::CREATED,
        )
        .await;
        profiles.push(profile["id"].as_str().expect("profile id").to_string());
    }
    let body = serde_json::json!({
        "profile_id": profiles[0], "scope_type": "connection_target", "scope_id": host_id
    })
    .to_string();
    let (status, response) = request(
        &app,
        "POST",
        "/api/v1/monitoring/profile-assignments",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create assignment");
    assert_eq!(status, StatusCode::CREATED);
    let assignment: serde_json::Value = serde_json::from_slice(&response).expect("assignment json");
    let id = assignment["id"].as_str().expect("assignment id");
    let replacement = serde_json::json!({
        "profile_id": profiles[1], "scope_type": "connection_target", "scope_id": host_id
    })
    .to_string();
    let (status, body) = request(
        &app,
        "PUT",
        &format!("/api/v1/monitoring/profile-assignments/{id}"),
        Some(&replacement),
        Some(&cookie),
    )
    .await
    .expect("replace assignment");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
#[ignore]
async fn target_profiles_excludes_assignment_for_wrong_host_kind() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _) = seed_device(&pool, &format!("itest-kind-{suffix}"))
        .await
        .expect("seed host");
    let node = exporter_id(&app, &cookie, "node").await;
    let (_, profile) = create_profile(
        &app,
        &cookie,
        serde_json::json!({
            "name": format!("Wrong kind {suffix}"), "slug": format!("wrong-kind-{suffix}"),
            "items": [{ "exporter_id": node }]
        }),
        StatusCode::CREATED,
    )
    .await;
    let profile_id = profile["id"].as_str().expect("profile id");
    let body = serde_json::json!({
        "profile_id": profile_id, "scope_type": "virtual_machine", "scope_id": host_id
    })
    .to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/profile-assignments",
        Some(&body),
        Some(&cookie),
    )
    .await
    .expect("create assignment");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let response = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/targets/{host_id}/profiles"),
    )
    .await;
    assert!(
        !response["profiles"]
            .as_array()
            .expect("profiles")
            .iter()
            .any(|entry| entry["profile_id"] == profile_id)
    );
}

#[tokio::test]
#[ignore]
async fn profile_preview_uses_unsaved_target_and_can_clear_saved_target_without_writing() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _) = seed_device(&pool, &format!("preview-target-{suffix}"))
        .await
        .expect("seed host");
    let node = exporter_id(&app, &cookie, "node").await;
    let (_, profile) = create_profile(
        &app,
        &cookie,
        serde_json::json!({
            "name":format!("Preview target {suffix}"), "slug":format!("preview-target-{suffix}"),
            "items":[{"exporter_id":node,"port":9100}]
        }),
        StatusCode::CREATED,
    )
    .await;
    let id = profile["id"].as_str().expect("profile id");
    let payload=serde_json::json!({"scope_type":"connection_target","scope_id":host_id,"target":"saved.example"}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/monitoring/profiles/{id}/apply"),
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply profile");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let assignment: serde_json::Value = serde_json::from_slice(&body).expect("assignment");
    let preview = list_monitoring(&app,&cookie,&format!("/api/v1/monitoring/profiles/{id}/preview?scope_type=connection_target&scope_id={host_id}&target=draft.example")).await;
    assert_eq!(
        preview["hosts"][0]["targets"],
        serde_json::json!(["draft.example:9100"])
    );
    let cleared = list_monitoring(&app,&cookie,&format!("/api/v1/monitoring/profiles/{id}/preview?scope_type=connection_target&scope_id={host_id}&target=")).await;
    assert!(!cleared["hosts"].to_string().contains("saved.example"));
    let untouched: Option<String> =
        sqlx::query_scalar("SELECT target FROM monitoring_profile_assignment WHERE id=$1")
            .bind(
                uuid::Uuid::parse_str(assignment["id"].as_str().expect("assignment id"))
                    .expect("uuid"),
            )
            .fetch_one(&pool)
            .await
            .expect("saved assignment");
    assert_eq!(untouched.as_deref(), Some("saved.example"));
}

#[tokio::test]
#[ignore]
async fn monitoring_profile_job_and_target_conflict_is_rejected() {
    let (app, cookie, _pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let node = exporter_id(&app, &cookie, "node").await;
    let process = exporter_id(&app, &cookie, "process").await;

    let (_, first) = create_profile(
        &app,
        &cookie,
        serde_json::json!({
            "name": format!("First conflict {suffix}"),
            "slug": format!("first-conflict-{suffix}"),
            "items": [{ "exporter_id": node, "job": "shared-job", "port": 9100 }],
        }),
        StatusCode::CREATED,
    )
    .await;
    let first_id = first["id"].as_str().expect("first profile id").to_string();

    let (_, second) = create_profile(
        &app,
        &cookie,
        serde_json::json!({
            "name": format!("Second conflict {suffix}"),
            "slug": format!("second-conflict-{suffix}"),
            "items": [{ "exporter_id": process, "job": "shared-job", "port": 9100 }],
        }),
        StatusCode::CREATED,
    )
    .await;
    let second_id = second["id"]
        .as_str()
        .expect("second profile id")
        .to_string();

    let apply_body = serde_json::json!({
        "scope_type": "global",
        "target": "duplicate.example",
    })
    .to_string();
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/monitoring/profiles/{first_id}/apply"),
        Some(&apply_body),
        Some(&cookie),
    )
    .await
    .expect("apply first profile");
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/monitoring/profiles/{second_id}/apply"),
        Some(&apply_body),
        Some(&cookie),
    )
    .await
    .expect("apply conflicting profile");
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "conflicting apply body={}",
        String::from_utf8_lossy(&body)
    );
    let message = String::from_utf8_lossy(&body);
    assert!(
        message.contains("shared-job") && message.contains("duplicate.example"),
        "conflict must name the job and target: {message}"
    );

    for id in [first_id, second_id] {
        let (status, _) = request(
            &app,
            "DELETE",
            &format!("/api/v1/monitoring/profiles/{id}"),
            None,
            Some(&cookie),
        )
        .await
        .expect("cleanup profile");
        assert!(
            status == StatusCode::NO_CONTENT || status == StatusCode::CONFLICT,
            "unexpected cleanup status {status}"
        );
    }
}

#[tokio::test]
#[ignore]
async fn monitoring_profile_allows_two_jobs_of_one_exporter() {
    let (app, cookie, _pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let node = exporter_id(&app, &cookie, "node").await;

    let (_, profile) = create_profile(
        &app,
        &cookie,
        serde_json::json!({
            "name": format!("Two jobs {suffix}"),
            "slug": format!("two-jobs-{suffix}"),
            "items": [
                { "exporter_id": node, "job": "node-a", "port": 9100 },
                { "exporter_id": node, "job": "node-b", "port": 9101 },
            ],
        }),
        StatusCode::CREATED,
    )
    .await;
    let profile_id = profile["id"].as_str().expect("profile id").to_string();
    assert_eq!(profile["items"].as_array().expect("items").len(), 2);

    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/monitoring/profiles/{profile_id}"),
        None,
        Some(&cookie),
    )
    .await
    .expect("cleanup profile");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore]
async fn competing_profile_apply_keeps_one_job_target_owner() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let node = exporter_id(&app, &cookie, "node").await;
    let job = format!("race-{suffix}");
    let mut paths = Vec::new();
    for prefix in ["first", "second"] {
        let (_, profile) = create_profile(
            &app,
            &cookie,
            serde_json::json!({
                "name": format!("Race {prefix} {suffix}"),
                "slug": format!("race-{prefix}-{suffix}"),
                "items": [{ "exporter_id": node, "job": job, "port": 9100 }]
            }),
            StatusCode::CREATED,
        )
        .await;
        paths.push(format!(
            "/api/v1/monitoring/profiles/{}/apply",
            profile["id"].as_str().expect("profile id")
        ));
    }
    let payload = serde_json::json!({"scope_type":"global", "target":"race.example"}).to_string();
    let (first, second) = tokio::join!(
        request(&app, "POST", &paths[0], Some(&payload), Some(&cookie)),
        request(&app, "POST", &paths[1], Some(&payload), Some(&cookie))
    );
    let mut statuses = vec![
        first.expect("first apply").0,
        second.expect("second apply").0,
    ];
    statuses.sort();
    assert_eq!(statuses, vec![StatusCode::CREATED, StatusCode::CONFLICT]);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM monitoring_profile_assignment a JOIN monitoring_profile_item i ON i.profile_id = a.profile_id WHERE i.job = $1")
        .bind(&job).fetch_one(&pool).await.expect("assignment count");
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore]
async fn updating_assigned_profile_items_rolls_back_conflicting_job() {
    let (app, cookie, _) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let node = exporter_id(&app, &cookie, "node").await;
    let mut profiles = Vec::new();
    for prefix in ["first", "second"] {
        let (_, profile) = create_profile(
            &app,
            &cookie,
            serde_json::json!({
                "name": format!("Edit {prefix} {suffix}"),
                "slug": format!("edit-{prefix}-{suffix}"),
                "items": [{ "exporter_id": node, "job": format!("{prefix}-{suffix}"), "port":9100 }]
            }),
            StatusCode::CREATED,
        )
        .await;
        let path = format!(
            "/api/v1/monitoring/profiles/{}/apply",
            profile["id"].as_str().expect("profile id")
        );
        let payload =
            serde_json::json!({"scope_type":"global", "target":"edit.example"}).to_string();
        let (status, body) = request(&app, "POST", &path, Some(&payload), Some(&cookie))
            .await
            .expect("apply profile");
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&body)
        );
        profiles.push(profile);
    }
    let path = format!(
        "/api/v1/monitoring/profiles/{}",
        profiles[1]["id"].as_str().expect("profile id")
    );
    let payload = serde_json::json!({
        "name":profiles[1]["name"], "slug":profiles[1]["slug"],
        "items":[{"exporter_id":node,"job":format!("first-{suffix}"),"port":9100}]
    })
    .to_string();
    let (status, _) = request(&app, "PUT", &path, Some(&payload), Some(&cookie))
        .await
        .expect("edit profile");
    assert_eq!(status, StatusCode::CONFLICT);
    let profile = list_monitoring(&app, &cookie, &path).await;
    assert_eq!(
        profile["items"][0]["effective_job"],
        format!("second-{suffix}")
    );
}

#[tokio::test]
#[ignore]
async fn target_profiles_reports_conflicts_across_effective_scopes() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _) = seed_device(&pool, &format!("overlap-{suffix}"))
        .await
        .expect("seed host");
    let node = exporter_id(&app, &cookie, "node").await;
    let job = format!("overlap-{suffix}");
    for (prefix, scope, scope_id) in [
        ("global", "global", None),
        ("host", "connection_target", Some(host_id)),
    ] {
        let (_, profile) = create_profile(&app, &cookie, serde_json::json!({
            "name":format!("Overlap {prefix} {suffix}"), "slug":format!("overlap-{prefix}-{suffix}"),
            "items":[{"exporter_id":node,"job":job,"port":9100}]
        }), StatusCode::CREATED).await;
        let path = format!(
            "/api/v1/monitoring/profiles/{}/apply",
            profile["id"].as_str().expect("profile id")
        );
        let payload =
            serde_json::json!({"scope_type":scope,"scope_id":scope_id,"target":"overlap.example"})
                .to_string();
        let (status, body) = request(&app, "POST", &path, Some(&payload), Some(&cookie))
            .await
            .expect("apply profile");
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&body)
        );
    }
    let response = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/targets/{host_id}/profiles"),
    )
    .await;
    assert!(
        response["conflicts"]
            .as_array()
            .expect("conflicts")
            .iter()
            .any(|conflict| conflict["job"] == job)
    );
}

#[tokio::test]
#[ignore]
async fn preset_apply_reuses_operator_profile_without_reenabling_assignment() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _) = seed_device(&pool, &format!("preset-reuse-{suffix}"))
        .await
        .expect("seed host");
    let node = exporter_id(&app, &cookie, "node").await;
    let (_, profile) = create_profile(&app, &cookie, serde_json::json!({
        "name":format!("Migrated operator profile {suffix}"),"slug":format!("operator-{suffix}"),
        "items":[{"exporter_id":node,"job":format!("custom-{suffix}"),"port":9101,"metrics_path":"/custom","labels":{"owner":"operator"}}]
    }),StatusCode::CREATED).await;
    let payload=serde_json::json!({"profile_id":profile["id"],"scope_type":"connection_target","scope_id":host_id,"enabled":false,"target":"operator.example"}).to_string();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/monitoring/profile-assignments",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("create disabled assignment");
    assert_eq!(status, StatusCode::CREATED);
    let assignment: serde_json::Value = serde_json::from_slice(&body).expect("assignment");
    let source_id:uuid::Uuid=sqlx::query_scalar("INSERT INTO monitoring_query_source(name,slug,query_url,source_type) VALUES($1,$2,'http://metrics.example/api/v1/query','prometheus') RETURNING id")
        .bind(format!("Preset source {suffix}")).bind(format!("preset-source-{suffix}")).fetch_one(&pool).await.expect("query source");
    let payload=serde_json::json!({"query_source_id":source_id,"scope_type":"connection_target","scope_id":host_id}).to_string();
    for _ in 0..2 {
        let (status, body) = request(
            &app,
            "POST",
            "/api/v1/monitoring/exporters/node/apply",
            Some(&payload),
            Some(&cookie),
        )
        .await
        .expect("apply preset");
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let applied: serde_json::Value = serde_json::from_slice(&body).expect("applied preset");
        assert_eq!(applied["scrape_assignment_id"], assignment["id"]);
    }
    let rows:Vec<(uuid::Uuid,bool,Option<String>)>=sqlx::query_as("SELECT id,enabled,target FROM monitoring_profile_assignment WHERE scope_type='connection_target' AND scope_id=$1")
        .bind(host_id).fetch_all(&pool).await.expect("assignments");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].1);
    assert_eq!(rows[0].2.as_deref(), Some("operator.example"));
    let current = list_monitoring(
        &app,
        &cookie,
        &format!(
            "/api/v1/monitoring/profiles/{}",
            profile["id"].as_str().expect("profile id")
        ),
    )
    .await;
    assert_eq!(current["items"][0]["job"], format!("custom-{suffix}"));
    assert_eq!(current["items"][0]["port"], 9101);
    assert_eq!(current["items"][0]["metrics_path"], "/custom");
}

#[tokio::test]
#[ignore]
async fn label_job_collision_is_rejected_and_reported_across_scopes() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _) = seed_device(&pool, &format!("label-job-{suffix}"))
        .await
        .expect("seed host");
    let node = exporter_id(&app, &cookie, "node").await;
    let shared_job = format!("label-shared-{suffix}");
    let mut ids = Vec::new();
    for prefix in ["first", "second"] {
        let (_, profile) = create_profile(&app, &cookie, serde_json::json!({
            "name":format!("Label {prefix} {suffix}"),"slug":format!("label-{prefix}-{suffix}"),
            "items":[{"exporter_id":node,"job":format!("raw-{prefix}-{suffix}"),"port":9100,"labels":{"job":shared_job}}]
        }),StatusCode::CREATED).await;
        ids.push(profile["id"].as_str().expect("profile id").to_string());
    }
    let payload = serde_json::json!({"scope_type":"global","target":"labels.example"}).to_string();
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/monitoring/profiles/{}/apply", ids[0]),
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("first apply");
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/monitoring/profiles/{}/apply", ids[1]),
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("conflicting apply");
    assert_eq!(status, StatusCode::CONFLICT);
    let payload=serde_json::json!({"scope_type":"connection_target","scope_id":host_id,"target":"labels.example"}).to_string();
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/v1/monitoring/profiles/{}/apply", ids[1]),
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("host apply");
    assert_eq!(status, StatusCode::CREATED);
    let response = list_monitoring(
        &app,
        &cookie,
        &format!("/api/v1/monitoring/targets/{host_id}/profiles"),
    )
    .await;
    assert!(
        response["conflicts"]
            .as_array()
            .expect("conflicts")
            .iter()
            .any(|entry| entry["job"] == shared_job)
    );
}

#[tokio::test]
#[ignore]
async fn distinct_ports_allow_assignments_and_exporter_metadata_update() {
    let (app, cookie, _) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let node = exporter_id(&app, &cookie, "node").await;
    let job = format!("ports-{suffix}");
    for port in [9100, 9101] {
        let (_, profile) = create_profile(
            &app,
            &cookie,
            serde_json::json!({
                "name":format!("Port {port} {suffix}"),"slug":format!("port-{port}-{suffix}"),
                "items":[{"exporter_id":node,"job":job,"port":port}]
            }),
            StatusCode::CREATED,
        )
        .await;
        let payload =
            serde_json::json!({"scope_type":"global","target":"ports.example"}).to_string();
        let (status, body) = request(
            &app,
            "POST",
            &format!(
                "/api/v1/monitoring/profiles/{}/apply",
                profile["id"].as_str().expect("profile id")
            ),
            Some(&payload),
            Some(&cookie),
        )
        .await
        .expect("apply");
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&body)
        );
    }
    let path = format!("/api/v1/monitoring/exporters/{node}");
    let catalog = list_monitoring(&app, &cookie, "/api/v1/monitoring/exporters").await;
    let mut exporter = catalog["items"]
        .as_array()
        .expect("exporter catalog")
        .iter()
        .find(|exporter| exporter["id"].as_str() == Some(node.as_str()))
        .expect("node exporter")
        .clone();
    exporter["description"] = serde_json::json!(format!("Updated metadata {suffix}"));
    let payload = exporter.to_string();
    let (status, body) = request(&app, "PUT", &path, Some(&payload), Some(&cookie))
        .await
        .expect("metadata update");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
#[ignore]
async fn preset_item_addition_validates_existing_assignment_before_commit() {
    let (app, cookie, pool) = admin_router_and_cookie().await;
    let suffix = unique_suffix();
    let (host_id, _) = seed_device(&pool, &format!("preset-add-{suffix}"))
        .await
        .expect("seed host");
    let process = exporter_id(&app, &cookie, "process").await;
    let smart = exporter_id(&app, &cookie, "smartctl").await;
    let (_, profile) = create_profile(
        &app,
        &cookie,
        serde_json::json!({
            "name":format!("Customized preset {suffix}"),"slug":"preset-node-exporter",
            "items":[{"exporter_id":process,"job":"node","port":9100,"labels":{"owner":"operator"}},
                     {"exporter_id":smart,"job":format!("smart-{suffix}")}]
        }),
        StatusCode::CREATED,
    )
    .await;
    let payload=serde_json::json!({"scope_type":"connection_target","scope_id":host_id,"target":"preset-add.example"}).to_string();
    let (status, _) = request(
        &app,
        "POST",
        &format!(
            "/api/v1/monitoring/profiles/{}/apply",
            profile["id"].as_str().expect("profile id")
        ),
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("apply custom preset");
    assert_eq!(status, StatusCode::CREATED);
    let source_id:uuid::Uuid=sqlx::query_scalar("INSERT INTO monitoring_query_source(name,slug,query_url,source_type) VALUES($1,$2,'http://metrics.example/api/v1/query','prometheus') RETURNING id")
        .bind(format!("Add source {suffix}")).bind(format!("add-source-{suffix}")).fetch_one(&pool).await.expect("query source");
    let payload=serde_json::json!({"scope_type":"connection_target","scope_id":host_id,"query_source_id":source_id}).to_string();
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/monitoring/exporters/node/apply",
        Some(&payload),
        Some(&cookie),
    )
    .await
    .expect("preset apply");
    assert_eq!(status, StatusCode::CONFLICT);
    let current = list_monitoring(
        &app,
        &cookie,
        &format!(
            "/api/v1/monitoring/profiles/{}",
            profile["id"].as_str().expect("profile id")
        ),
    )
    .await;
    assert_eq!(current["items"].as_array().expect("profile items").len(), 2);
}
