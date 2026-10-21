//! DB-backed contract for the playbook collection endpoints.
//!
//! `tests/compliance/playbooks_rest_openapi_source_of_truth.rs` proves the same
//! eight collections against the OpenAPI document without a database: the
//! response DTO, the `limit`/`offset` query pair and the required
//! `items`/`total`/`limit`/`offset` fields. This suite proves the running API
//! actually answers that envelope: every `GET` returns 200 with exactly those
//! four keys and the documented JSON types, and the run-scoped collections list
//! the rows seeded for them.
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_rest playbook_collections_serve -- --ignored`

use super::*;

/// Collections whose path carries no parameter.
const PARAMETERLESS_COLLECTIONS: [&str; 5] = [
    "/api/v1/playbooks",
    "/api/v1/playbooks/execution-environments",
    "/api/v1/playbooks/projects",
    "/api/v1/playbooks/job-templates",
    "/api/v1/playbooks/runs",
];

/// The page envelope every playbook collection answers, mirroring
/// `CANONICAL_PAGE_FIELDS` in the OpenAPI source-of-truth suite.
const CANONICAL_PAGE_FIELDS: [&str; 4] = ["items", "total", "limit", "offset"];

/// Assert the canonical envelope of a collection response and return its JSON.
#[allow(
    clippy::panic,
    reason = "assertion helper outside #[test]: a body that is not the canonical page must fail the calling contract test"
)]
fn assert_canonical_page(uri: &str, body: &[u8]) -> serde_json::Value {
    let page: serde_json::Value = serde_json::from_slice(body)
        .unwrap_or_else(|error| panic!("`{uri}` must answer JSON: {error}"));
    let object = page
        .as_object()
        .unwrap_or_else(|| panic!("`{uri}` must answer a JSON object: {page}"));

    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = CANONICAL_PAGE_FIELDS;
    expected.sort_unstable();
    assert_eq!(
        keys, expected,
        "`{uri}` must answer exactly the canonical page envelope"
    );

    assert!(
        page["items"].is_array(),
        "`{uri}`: `items` must be an array"
    );
    for field in ["total", "limit", "offset"] {
        assert!(
            page[field].is_number(),
            "`{uri}`: `{field}` must be a number"
        );
    }
    page
}

/// Authenticated `GET` of one collection, checked for 200 plus the envelope.
#[allow(
    clippy::panic,
    reason = "assertion helper outside #[test]: an unanswerable GET must fail the calling contract test"
)]
async fn get_collection(app: &Router, cookie: &str, uri: &str) -> serde_json::Value {
    let (status, body) = request(app, "GET", uri, None, Some(cookie))
        .await
        .unwrap_or_else(|error| panic!("GET `{uri}`: {error}"));
    assert_eq!(
        status,
        StatusCode::OK,
        "GET `{uri}` must answer 200: {}",
        String::from_utf8_lossy(&body)
    );
    assert_canonical_page(uri, &body)
}

/// Assert a page lists the row the suite seeded under `id`.
fn assert_items_contain(page: &serde_json::Value, uri: &str, id: Uuid) {
    let expected = id.to_string();
    let found = page["items"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|item| item["id"].as_str() == Some(expected.as_str()));
    assert!(found, "`{uri}` must list seeded row {expected}: {page}");
}

/// Creates a playbook through the API — which publishes its initial version —
/// and returns `(playbook_id, version_id)`.
#[allow(
    clippy::expect_used,
    reason = "fixture helper outside #[test]: a create response missing its ids must fail the calling contract test"
)]
async fn create_playbook_with_version(app: &Router, cookie: &str) -> (Uuid, Uuid) {
    let suffix = unique_suffix();
    let body = serde_json::json!({
        "name": format!("itest collections {suffix}"),
        "content": "- hosts: all\n  tasks: []\n",
    })
    .to_string();
    let (status, body) = request(app, "POST", "/api/v1/playbooks", Some(&body), Some(cookie))
        .await
        .expect("create playbook request");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create playbook: {}",
        String::from_utf8_lossy(&body)
    );
    let item: serde_json::Value = serde_json::from_slice(&body).expect("created playbook JSON");
    let playbook_id = item["playbook"]["id"]
        .as_str()
        .expect("created playbook id")
        .parse()
        .expect("playbook uuid");
    let version_id = item["current_version"]["id"]
        .as_str()
        .expect("created playbook current version id")
        .parse()
        .expect("version uuid");
    (playbook_id, version_id)
}

/// Row ids of the minimal run chain the run-scoped collections read.
struct SeededRunChain {
    run_group_id: Uuid,
    run_id: Uuid,
    event_id: Uuid,
    artifact_id: Uuid,
}

/// Seeds `playbook_run_group` → `playbook_run` → run event and artifact, the FK
/// chain the run-scoped list endpoints read (`integration_playbook_executor.rs`
/// and `integration_delete_blocked_non_inventory.rs` seed the same tables).
#[allow(
    clippy::expect_used,
    reason = "fixture helper outside #[test]: a rejected seed INSERT must fail the calling contract test"
)]
async fn seed_run_chain(db: &sqlx::PgPool, playbook_id: Uuid, version_id: Uuid) -> SeededRunChain {
    let run_group_id = Uuid::now_v7();
    let run_id = Uuid::now_v7();
    let event_id = Uuid::now_v7();
    let artifact_id = Uuid::now_v7();

    sqlx::query(
        "INSERT INTO playbook_run_group \
         (id, playbook_id, playbook_version_id, trigger_kind, status, total_runs) \
         VALUES ($1, $2, $3, 'manual', 'running', 1)",
    )
    .bind(run_group_id)
    .bind(playbook_id)
    .bind(version_id)
    .execute(db)
    .await
    .expect("seed run group");

    sqlx::query(
        "INSERT INTO playbook_run \
         (id, run_group_id, playbook_id, playbook_version_id, target_kind, target_id, \
          source_kind, source_id, status) \
         VALUES ($1, $2, $3, $4, 'connection_target', $5, 'device', $6, 'running')",
    )
    .bind(run_id)
    .bind(run_group_id)
    .bind(playbook_id)
    .bind(version_id)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(db)
    .await
    .expect("seed run");

    sqlx::query(
        "INSERT INTO playbook_run_event \
         (id, run_group_id, run_id, event_sequence, event_kind, event_type, message) \
         VALUES ($1, $2, $3, 1, 'status', 'run.started', 'itest collections event')",
    )
    .bind(event_id)
    .bind(run_group_id)
    .bind(run_id)
    .execute(db)
    .await
    .expect("seed run event");

    sqlx::query(
        "INSERT INTO playbook_artifact \
         (id, run_id, artifact_type, name, content_json, content_sha256, content_size_bytes) \
         VALUES ($1, $2, 'result', $3, '{}'::jsonb, $4, 2)",
    )
    .bind(artifact_id)
    .bind(run_id)
    .bind(format!("itest-collections-{}", unique_suffix()))
    .bind("0".repeat(64))
    .execute(db)
    .await
    .expect("seed run artifact");

    SeededRunChain {
        run_group_id,
        run_id,
        event_id,
        artifact_id,
    }
}

/// Removes the seeded chain and playbook. The group goes first because
/// `playbook_run_group.playbook_id` is `ON DELETE RESTRICT`; runs, events and
/// artifacts cascade from the group. The playbook delete reuses the repository's
/// delete carve-out (`piping.allow_playbook_delete`), without which the
/// version-immutability trigger rejects the cascade.
#[allow(
    clippy::expect_used,
    reason = "fixture helper outside #[test]: a failed cleanup statement must fail the calling contract test"
)]
async fn remove_seeded_rows(db: &sqlx::PgPool, chain: &SeededRunChain, playbook_id: Uuid) {
    sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
        .bind(chain.run_group_id)
        .execute(db)
        .await
        .expect("cleanup run group");
    let mut tx = db.begin().await.expect("cleanup transaction");
    sqlx::query("SET LOCAL piping.allow_playbook_delete = 'on'")
        .execute(&mut *tx)
        .await
        .expect("version-delete carve-out");
    sqlx::query("DELETE FROM playbook WHERE id = $1")
        .bind(playbook_id)
        .execute(&mut *tx)
        .await
        .expect("cleanup playbook");
    tx.commit().await.expect("cleanup commit");
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn playbook_collections_serve_canonical_pages() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let db = build_test_db_pool().await.expect("db pool");
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    let (playbook_id, version_id) = create_playbook_with_version(&app, &cookie).await;
    let chain = seed_run_chain(&db, playbook_id, version_id).await;

    for path in PARAMETERLESS_COLLECTIONS {
        get_collection(&app, &cookie, path).await;
    }

    let versions_uri = format!("/api/v1/playbooks/{playbook_id}/versions");
    let versions = get_collection(&app, &cookie, &versions_uri).await;
    assert_items_contain(&versions, &versions_uri, version_id);

    // The unfiltered runs collection holds every run from every suite sharing
    // the database, so the seeded run is proven through its own `playbook_id`
    // filter instead of a page position.
    let runs_uri = format!("/api/v1/playbooks/runs?playbook_id={playbook_id}");
    let runs = get_collection(&app, &cookie, &runs_uri).await;
    assert_items_contain(&runs, &runs_uri, chain.run_id);

    let artifacts_uri = format!("/api/v1/playbooks/runs/{}/artifacts", chain.run_id);
    let artifacts = get_collection(&app, &cookie, &artifacts_uri).await;
    assert_items_contain(&artifacts, &artifacts_uri, chain.artifact_id);

    let events_uri = format!("/api/v1/playbooks/runs/{}/events", chain.run_id);
    let events = get_collection(&app, &cookie, &events_uri).await;
    assert_items_contain(&events, &events_uri, chain.event_id);

    remove_seeded_rows(&db, &chain, playbook_id).await;
}
