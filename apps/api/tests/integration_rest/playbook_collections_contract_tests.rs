//! Current workspace collections serve the documented pagination envelope.
//! Requires isolated PostgreSQL and Redis fixtures.

use super::project_workspace::{WorkspaceFixture, seed_workspace};
use super::*;
use piping_backend::contexts::playbooks::domain::{
    InventorySelection, PlaybookRunTriggerKind, ResolvedTarget,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateRunGroupRecord, PlaybookRepository, SqlxPlaybookRepository,
};

type TestError = Box<dyn std::error::Error + Send + Sync>;

const PARAMETERLESS_COLLECTIONS: [&str; 5] = [
    "/api/v1/playbooks/execution-environments",
    "/api/v1/playbooks/projects",
    "/api/v1/playbooks/job-templates",
    "/api/v1/playbooks/runs",
    "/api/v1/playbooks/run-groups",
];

async fn get_collection(
    app: &Router,
    cookie: &str,
    uri: &str,
) -> Result<serde_json::Value, TestError> {
    let (status, body) = request(app, "GET", uri, None, Some(cookie)).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET {uri}: {}",
        String::from_utf8_lossy(&body)
    );
    let page: serde_json::Value = serde_json::from_slice(&body)?;
    let object = page.as_object().ok_or("collection must return an object")?;
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["items", "limit", "offset", "total"], "GET {uri}");
    assert!(page["items"].is_array(), "GET {uri}");
    for field in ["total", "limit", "offset"] {
        assert!(page[field].as_u64().is_some(), "GET {uri}: {field}");
    }
    Ok(page)
}

fn assert_items_contain(page: &serde_json::Value, uri: &str, id: Uuid) {
    let expected = id.to_string();
    let found = page["items"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|item| item["id"].as_str() == Some(expected.as_str()));
    assert!(found, "{uri} must list seeded row {expected}: {page}");
}

struct SeededRunChain {
    workspace: WorkspaceFixture,
    target_set_id: Uuid,
    run_group_id: Uuid,
    run_id: Uuid,
    event_id: Uuid,
    artifact_id: Uuid,
}

async fn seed_run_chain(db: &sqlx::PgPool) -> Result<SeededRunChain, TestError> {
    let target_set_id = Uuid::now_v7();
    sqlx::query("INSERT INTO playbook_target_set (id, name, mode) VALUES ($1, $2, 'static')")
        .bind(target_set_id)
        .bind(format!("collections-{target_set_id}"))
        .execute(db)
        .await?;
    let workspace = seed_workspace(db, InventorySelection::Managed { target_set_id }).await?;
    let target_id = Uuid::now_v7();
    let created = SqlxPlaybookRepository::new(db.clone())
        .create_run_group(CreateRunGroupRecord {
            launch_spec: workspace.launch_spec(vec![ResolvedTarget {
                hostname: "collections-host".into(),
                target_id: Some(target_id),
                source_id: Some(target_id),
            }]),
            schedule_id: None,
            scheduled_for: None,
            trigger_kind: PlaybookRunTriggerKind::Manual,
            requested_by_user_id: None,
            service_principal_id: None,
            metadata: serde_json::json!({}),
            secret_env_authorization: None,
            ssh_record_contexts: Vec::new(),
        })
        .await?;
    let run_group_id = created.run_group.id;
    let run_id = created.runs.first().ok_or("fixture run missing")?.id;
    let event_id = Uuid::now_v7();
    let artifact_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_run_event \
         (id, run_group_id, run_id, event_sequence, event_kind, event_type, message) \
         VALUES ($1, $2, $3, 1, 'status', 'run.queued', 'collections event')",
    )
    .bind(event_id)
    .bind(run_group_id)
    .bind(run_id)
    .execute(db)
    .await?;
    sqlx::query(
        "INSERT INTO playbook_artifact \
         (id, run_id, artifact_type, name, content_json, content_sha256, content_size_bytes) \
         VALUES ($1, $2, 'result', $3, '{}'::jsonb, $4, 2)",
    )
    .bind(artifact_id)
    .bind(run_id)
    .bind(format!("collections-{artifact_id}"))
    .bind("0".repeat(64))
    .execute(db)
    .await?;
    Ok(SeededRunChain {
        workspace,
        target_set_id,
        run_group_id,
        run_id,
        event_id,
        artifact_id,
    })
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis fixtures"]
async fn playbook_collections_serve_canonical_pages() -> Result<(), TestError> {
    let state = build_test_app_state().await?;
    let db = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login failed")?;
    let chain = seed_run_chain(&db).await?;

    for path in PARAMETERLESS_COLLECTIONS {
        let page = get_collection(&app, &cookie, &format!("{path}?limit=2&offset=1")).await?;
        assert_eq!(page["limit"], 2);
        assert_eq!(page["offset"], 1);
        assert!(page["items"].as_array().ok_or("items missing")?.len() <= 2);
    }

    let template_id = chain.workspace.template.id;
    let groups_uri = format!("/api/v1/playbooks/run-groups?template_id={template_id}");
    let groups = get_collection(&app, &cookie, &groups_uri).await?;
    assert_items_contain(&groups, &groups_uri, chain.run_group_id);
    assert_eq!(groups["total"], 1);
    let runs_uri = format!("/api/v1/playbooks/runs?template_id={template_id}");
    let runs = get_collection(&app, &cookie, &runs_uri).await?;
    assert_items_contain(&runs, &runs_uri, chain.run_id);
    assert_eq!(runs["total"], 1);

    let artifacts_uri = format!("/api/v1/playbooks/runs/{}/artifacts", chain.run_id);
    let artifacts = get_collection(&app, &cookie, &artifacts_uri).await?;
    assert_items_contain(&artifacts, &artifacts_uri, chain.artifact_id);
    let events_uri = format!("/api/v1/playbooks/runs/{}/events", chain.run_id);
    let events = get_collection(&app, &cookie, &events_uri).await?;
    assert_items_contain(&events, &events_uri, chain.event_id);

    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(chain.run_group_id)
        .execute(&db)
        .await?;
    chain.workspace.cleanup(&db).await?;
    sqlx::query("DELETE FROM playbook_target_set WHERE id = $1")
        .bind(chain.target_set_id)
        .execute(&db)
        .await?;
    Ok(())
}
