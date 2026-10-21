//! Runtime DB probes for the critical-status table owned by
//! `tests/compliance/response_status_ratchet.rs`.
//!
//! The ratchet proves the *declared* contract: those operations keep naming
//! their client-relevant statuses in `ApiDoc`. It reads the document only. This
//! file proves the *answered* contract on the HTTP boundary against real
//! PostgreSQL and Redis:
//!
//! 1. an anonymous caller is refused by the authentication/authorization gate
//!    on every critical mutation (401 when the route gates on authentication,
//!    403 when deny-by-default policy evaluation runs first — the probe table
//!    records which, per operation);
//! 2. malformed input answers the documented client status (400 for an
//!    unparseable body or path id, 404 for an unknown id) and never some other
//!    4XX, 405 or 5XX;
//! 3. a delete that a foreign key blocks answers 409 `DELETE_BLOCKED` carrying
//!    the blocking rows.
//!
//! The method and the path template of every probe come from
//! `ApiDoc::openapi()` — the same runtime document the ratchet reads — so a
//! probe cannot silently target a path that is not the documented operation.
//!
//! Run:
//! `cargo test -p piping-backend --test integration_rest status_contract -- --ignored --test-threads=1`

use super::common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie, request, unique_suffix,
};
use axum::http::StatusCode;
use piping_backend::api::openapi::ApiDoc;
use piping_backend::contexts::playbooks::infrastructure::persistence::repositories::playbook_repository::{
    PlaybookRepository, SqlxPlaybookRepository,
};
use std::collections::{BTreeMap, BTreeSet};
use utoipa::OpenApi;
use uuid::Uuid;

/// The critical mutations this file probes. The exact statuses each one must
/// answer live in `tests/compliance/response_status_ratchet.rs` (`REQUIRED`,
/// lines 20-47); the probes below exercise the entries a request can reach.
const CRITICAL_OPERATIONS: [&str; 11] = [
    "create_connection_session",
    "delete_user_credential_set",
    "create_ip_address",
    "update_ip_address",
    "delete_ip_address",
    "create_interface",
    "update_interface",
    "delete_interface",
    "delete_playbook",
    "delete_target_set",
    "delete_connconf_record",
];

/// A well-formed UUID that no seeded row owns.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-0000000000ff";

/// A path segment that is not a UUID at all.
const MALFORMED_ID: &str = "not-a-uuid";

/// The documented operation, resolved from `ApiDoc`.
struct OperationTarget {
    method: String,
    template: String,
    /// The `responses` keys this operation declares, as written in the document.
    declared: BTreeSet<String>,
}

/// One request sent to one documented operation.
struct Probe {
    operation: &'static str,
    /// Replacement for the `{...}` placeholder of the documented path.
    id: &'static str,
    body: Option<&'static str>,
}

/// What the answered status must be, and why that is not a choice:
/// `Exact` repeats a status the critical-status table pins for this operation,
/// `DeclaredClientStatus` covers the operations whose table entry names only
/// 409 (`delete_playbook`, `delete_target_set`, `delete_connconf_record`), so
/// the probe pins nothing more than "the answer is a client status this
/// operation declares in ApiDoc".
enum Expectation {
    Exact(StatusCode),
    DeclaredClientStatus,
}

/// Anonymous probes: a syntactically valid request, no cookie, and the status
/// the operation's own authentication/authorization boundary answers.
///
/// Two gates produce it, and which one a route uses is visible in its handler
/// signature:
///
/// * an authentication gate — `RequireAuthenticated` (extraction phase, before
///   the body is read) or the handler-level `require_authenticated_user_id` —
///   answers 401. That covers `create_connection_session`,
///   `delete_user_credential_set`, `create_ip_address`, `update_ip_address`,
///   `create_interface`, `update_interface`, `delete_playbook`,
///   `delete_target_set`;
/// * deny-by-default policy evaluation inside the handler (`ensure_access`,
///   `ensure_config_mutation`) answers 403, because a caller with no identity
///   reaches the policy check instead of a pre-gate. That covers
///   `delete_ip_address`, `delete_interface`, `delete_connconf_record`. The
///   critical-status table names 401 for the first two as well; the 403 here is
///   what the gate actually answers, recorded in the PIP-130 report.
///
/// Either way the caller is refused before the write path: what must never
/// happen is that an anonymous request is served.
const ANONYMOUS_PROBES: &[(Probe, StatusCode)] = &[
    (
        Probe {
            operation: "create_connection_session",
            id: ABSENT_UUID,
            body: Some(r#"{"target_id":"00000000-0000-0000-0000-0000000000ff","protocol":"ssh"}"#),
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "delete_user_credential_set",
            id: ABSENT_UUID,
            body: None,
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "create_ip_address",
            id: ABSENT_UUID,
            body: Some(
                r#"{"address":"10.255.255.1/32","interface_id":"00000000-0000-0000-0000-0000000000ff"}"#,
            ),
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "update_ip_address",
            id: ABSENT_UUID,
            body: Some(r#"{"address":"10.255.255.1/32"}"#),
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "delete_ip_address",
            id: ABSENT_UUID,
            body: None,
        },
        StatusCode::FORBIDDEN,
    ),
    (
        Probe {
            operation: "create_interface",
            id: ABSENT_UUID,
            body: Some(
                r#"{"device_id":"00000000-0000-0000-0000-0000000000ff","name":"status-contract-probe"}"#,
            ),
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "update_interface",
            id: ABSENT_UUID,
            body: Some(r#"{"name":"status-contract-probe"}"#),
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "delete_interface",
            id: ABSENT_UUID,
            body: None,
        },
        StatusCode::FORBIDDEN,
    ),
    (
        Probe {
            operation: "delete_playbook",
            id: ABSENT_UUID,
            body: None,
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "delete_target_set",
            id: ABSENT_UUID,
            body: None,
        },
        StatusCode::UNAUTHORIZED,
    ),
    (
        Probe {
            operation: "delete_connconf_record",
            id: ABSENT_UUID,
            body: None,
        },
        StatusCode::FORBIDDEN,
    ),
];

/// Authenticated probes for rejected input, paired with the status the
/// operation answers it with.
const INPUT_PROBES: &[(Probe, Expectation)] = &[
    // 400 in the critical-status table: a body that cannot be parsed, a path id
    // that cannot be parsed, or an address value that cannot be normalized.
    (
        Probe {
            operation: "create_connection_session",
            id: ABSENT_UUID,
            body: Some("{}"),
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    (
        Probe {
            operation: "delete_user_credential_set",
            id: MALFORMED_ID,
            body: None,
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    (
        Probe {
            operation: "create_ip_address",
            id: ABSENT_UUID,
            body: Some("{}"),
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    (
        Probe {
            operation: "update_ip_address",
            id: ABSENT_UUID,
            body: Some(r#"{"address":"not-a-cidr"}"#),
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    (
        Probe {
            operation: "delete_ip_address",
            id: MALFORMED_ID,
            body: None,
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    (
        Probe {
            operation: "create_interface",
            id: ABSENT_UUID,
            body: Some("{}"),
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    (
        Probe {
            operation: "update_interface",
            id: MALFORMED_ID,
            body: Some("{}"),
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    (
        Probe {
            operation: "delete_interface",
            id: MALFORMED_ID,
            body: None,
        },
        Expectation::Exact(StatusCode::BAD_REQUEST),
    ),
    // 404 in the critical-status table: an unknown but well-formed id.
    (
        Probe {
            operation: "create_connection_session",
            id: ABSENT_UUID,
            body: Some(r#"{"target_id":"00000000-0000-0000-0000-0000000000ff","protocol":"ssh"}"#),
        },
        Expectation::Exact(StatusCode::NOT_FOUND),
    ),
    (
        Probe {
            operation: "delete_user_credential_set",
            id: ABSENT_UUID,
            body: None,
        },
        Expectation::Exact(StatusCode::NOT_FOUND),
    ),
    (
        Probe {
            operation: "update_ip_address",
            id: ABSENT_UUID,
            body: Some("{}"),
        },
        Expectation::Exact(StatusCode::NOT_FOUND),
    ),
    (
        Probe {
            operation: "update_interface",
            id: ABSENT_UUID,
            body: Some("{}"),
        },
        Expectation::Exact(StatusCode::NOT_FOUND),
    ),
    // Neither 400 nor 404 is in the table for these three, so the probe only
    // requires the documented client status of an unknown id.
    (
        Probe {
            operation: "delete_playbook",
            id: ABSENT_UUID,
            body: None,
        },
        Expectation::DeclaredClientStatus,
    ),
    (
        Probe {
            operation: "delete_target_set",
            id: ABSENT_UUID,
            body: None,
        },
        Expectation::DeclaredClientStatus,
    ),
    (
        Probe {
            operation: "delete_connconf_record",
            id: ABSENT_UUID,
            body: None,
        },
        Expectation::DeclaredClientStatus,
    ),
];

/// Read method, path template and declared statuses per operationId from the
/// runtime document. Mirrors `contract_statuses_by_operation` in the ratchet.
#[allow(
    clippy::expect_used,
    reason = "fixture helper outside #[test]: a runtime ApiDoc that is not the documented shape fails the calling contract test"
)]
fn operation_targets() -> BTreeMap<String, OperationTarget> {
    let document = serde_json::to_value(ApiDoc::openapi()).expect("ApiDoc serializes to JSON");
    let paths = document["paths"]
        .as_object()
        .expect("ApiDoc declares a `paths` object");
    let mut targets = BTreeMap::new();

    for (template, path_item) in paths {
        let Some(item) = path_item.as_object() else {
            continue;
        };
        for (method, operation) in item {
            let Some(operation_id) = operation
                .get("operationId")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let declared: BTreeSet<String> = operation
                .get("responses")
                .and_then(serde_json::Value::as_object)
                .into_iter()
                .flatten()
                .map(|(status, _)| status.clone())
                .collect();
            let previous = targets.insert(
                operation_id.to_string(),
                OperationTarget {
                    method: method.to_ascii_uppercase(),
                    template: template.clone(),
                    declared,
                },
            );
            assert!(
                previous.is_none(),
                "duplicate operationId {operation_id} in ApiDoc"
            );
        }
    }

    for operation in CRITICAL_OPERATIONS {
        assert!(
            targets.contains_key(operation),
            "critical operation {operation} is absent from ApiDoc"
        );
    }

    targets
}

#[allow(
    clippy::panic,
    reason = "fixture helper outside #[test]: an operation missing from the runtime ApiDoc must fail the calling contract test"
)]
fn target_for<'a>(
    targets: &'a BTreeMap<String, OperationTarget>,
    operation: &str,
) -> &'a OperationTarget {
    targets
        .get(operation)
        .unwrap_or_else(|| panic!("critical operation {operation} is absent from ApiDoc"))
}

/// Substitute the documented `{...}` placeholder with a concrete path segment.
///
/// Every critical mutation's documented path carries at most one placeholder,
/// and a collection `POST` (`create_connection_session`) carries none — hence
/// the optional substitution instead of a positional list of values.
#[allow(
    clippy::panic,
    reason = "fixture helper outside #[test]: a malformed documented path template must fail the calling contract test"
)]
fn concrete_uri(template: &str, id: &str) -> String {
    let Some((head, tail)) = template.split_once('{') else {
        return template.to_string();
    };
    let Some((_, rest)) = tail.split_once('}') else {
        panic!("path template {template:?} has an unclosed placeholder");
    };
    assert!(
        !rest.contains('{'),
        "path template {template:?} has more than one placeholder"
    );
    format!("{head}{id}{rest}")
}

/// A status that only a broken boundary produces: the router not claiming a
/// mounted method (405), the teapot fallback of "no route matched" (418), or a
/// server-side failure (5XX).
fn is_boundary_failure(status: StatusCode) -> bool {
    status == StatusCode::METHOD_NOT_ALLOWED
        || status == StatusCode::IM_A_TEAPOT
        || status.is_server_error()
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn critical_mutations_require_authentication() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let targets = operation_targets();

    let mut violations = Vec::new();

    for (probe, expected) in ANONYMOUS_PROBES {
        let target = target_for(&targets, probe.operation);
        let uri = concrete_uri(&target.template, probe.id);
        let (status, _) = request(&app, &target.method, &uri, probe.body, None)
            .await
            .unwrap_or_else(|error| panic!("anonymous {} {uri}: {error}", target.method));

        if status != *expected {
            violations.push(format!(
                "anonymous {} {uri} ({}) answered {status}, expected {expected}",
                target.method, probe.operation
            ));
        }
    }

    assert_eq!(
        ANONYMOUS_PROBES.len(),
        CRITICAL_OPERATIONS.len(),
        "every critical mutation needs an anonymous probe"
    );
    let probed: BTreeSet<&str> = ANONYMOUS_PROBES
        .iter()
        .map(|(probe, _)| probe.operation)
        .collect();
    for operation in CRITICAL_OPERATIONS {
        assert!(
            probed.contains(operation),
            "critical mutation {operation} has no anonymous probe"
        );
    }

    assert!(
        violations.is_empty(),
        "{} anonymous probe(s) were not rejected by the authentication gate:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn critical_mutations_reject_invalid_input_without_server_error() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let targets = operation_targets();

    let mut violations = Vec::new();

    for (probe, expectation) in INPUT_PROBES {
        let target = target_for(&targets, probe.operation);
        let uri = concrete_uri(&target.template, probe.id);
        let (status, _) = request(&app, &target.method, &uri, probe.body, Some(&cookie))
            .await
            .unwrap_or_else(|error| panic!("{} {uri}: {error}", target.method));

        if is_boundary_failure(status) {
            violations.push(format!(
                "{} {uri} ({}) answered {status}: a rejected input must not reach the router boundary or the server",
                target.method, probe.operation
            ));
            continue;
        }

        match expectation {
            Expectation::Exact(expected) => {
                let declared = target.declared.contains(&expected.as_u16().to_string());
                if !declared {
                    violations.push(format!(
                        "critical mutation {} does not declare {expected} in ApiDoc",
                        probe.operation
                    ));
                }
                if status != *expected {
                    violations.push(format!(
                        "{} {uri} ({}) answered {status}, expected {expected}",
                        target.method, probe.operation
                    ));
                }
            }
            Expectation::DeclaredClientStatus => {
                if !status.is_client_error()
                    || !target.declared.contains(&status.as_u16().to_string())
                {
                    violations.push(format!(
                        "{} {uri} ({}) answered {status}, which is not a client status the operation declares ({:?})",
                        target.method, probe.operation, target.declared
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "{} rejected-input probe(s) broke the documented contract:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn deleting_a_blocked_playbook_over_http_returns_409() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let pool = build_test_db_pool().await.expect("db pool");

    // The FK chain a blocked playbook delete needs: a run history that
    // references the playbook. Seeded directly — the HTTP playbook-version and
    // run APIs would have to produce the same rows through several calls.
    let suffix = unique_suffix();
    let playbook_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let run_group_id = Uuid::now_v7();
    let playbook_run_id = Uuid::now_v7();
    let playbook_name = format!("status-contract-blocked-playbook-{suffix}");

    sqlx::query("INSERT INTO playbook (id, slug, name) VALUES ($1, $2, $3)")
        .bind(playbook_id)
        .bind(format!("status-contract-{suffix}"))
        .bind(&playbook_name)
        .execute(&pool)
        .await
        .expect("seed playbook");
    sqlx::query(
        "INSERT INTO playbook_version \
         (id, playbook_id, version, content, content_sha256) \
         VALUES ($1, $2, 1, '---', $3)",
    )
    .bind(version_id)
    .bind(playbook_id)
    .bind("0".repeat(64))
    .execute(&pool)
    .await
    .expect("seed playbook version");
    sqlx::query(
        "INSERT INTO playbook_run_group \
         (id, playbook_id, playbook_version_id, trigger_kind) \
         VALUES ($1, $2, $3, 'manual')",
    )
    .bind(run_group_id)
    .bind(playbook_id)
    .bind(version_id)
    .execute(&pool)
    .await
    .expect("seed playbook run group");
    sqlx::query(
        "INSERT INTO playbook_run \
         (id, run_group_id, playbook_id, playbook_version_id, target_kind, target_id, \
          source_kind, source_id) \
         VALUES ($1, $2, $3, $4, 'connection_target', $5, 'device', $6)",
    )
    .bind(playbook_run_id)
    .bind(run_group_id)
    .bind(playbook_id)
    .bind(version_id)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .expect("seed playbook run");

    let targets = operation_targets();
    let target = target_for(&targets, "delete_playbook");
    let uri = concrete_uri(&target.template, &playbook_id.to_string());
    let (status, body) = request(&app, &target.method, &uri, None, Some(&cookie))
        .await
        .expect("delete playbook");
    let payload = serde_json::from_slice::<serde_json::Value>(&body)
        .unwrap_or_else(|error| panic!("delete-blocked body is not JSON ({error}): {body:?}"));

    // A blocked delete must not have cascaded the version, and the playbook
    // itself must still be there. Both are read before the cleanup below, and
    // every check appends to `violations` so the cleanup always runs.
    let playbook_still_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM playbook WHERE id = $1)")
            .bind(playbook_id)
            .fetch_one(&pool)
            .await
            .expect("playbook existence query");
    let version_still_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM playbook_version WHERE id = $1)")
            .bind(version_id)
            .fetch_one(&pool)
            .await
            .expect("version existence query");

    let mut violations = Vec::new();
    if status != StatusCode::CONFLICT {
        violations.push(format!("DELETE {uri} answered {status}, expected 409"));
    }
    if payload["code"] != "DELETE_BLOCKED" {
        violations.push(format!(
            "expected code DELETE_BLOCKED, got {}",
            payload["code"]
        ));
    }
    if payload["reason_code"] != "DELETE_BLOCKED" {
        violations.push(format!(
            "expected reason_code DELETE_BLOCKED, got {}",
            payload["reason_code"]
        ));
    }
    if payload["message"] != "Cannot delete this record while related objects still exist" {
        violations.push(format!("unexpected message: {}", payload["message"]));
    }
    if payload["details"]["total_count"] != 2 {
        violations.push(format!(
            "expected 2 blocker groups, got {}",
            payload["details"]["total_count"]
        ));
    }
    let blockers = payload["details"]["blockers"].as_array();
    for (table, entity_key) in [
        ("playbook_run_group", "Playbook Run Groups"),
        ("playbook_run", "Playbook Runs"),
    ] {
        let group =
            blockers.and_then(|blockers| blockers.iter().find(|group| group["table"] == table));
        let Some(group) = group else {
            violations.push(format!("missing {table} blocker: {payload}"));
            continue;
        };
        if group["entity_key"] != entity_key {
            violations.push(format!(
                "{table} entity_key is {}, expected {entity_key}",
                group["entity_key"]
            ));
        }
        if group["count"] != 1 {
            violations.push(format!("{table} count is {}, expected 1", group["count"]));
        }
        let names_the_playbook = group["examples"].as_array().is_some_and(|examples| {
            examples.iter().any(|example| {
                example["label"]
                    .as_str()
                    .is_some_and(|label| label.ends_with(&format!(" @ {playbook_name}")))
            })
        });
        if !names_the_playbook {
            violations.push(format!("{table} must name the blocking playbook: {group}"));
        }
    }
    if !playbook_still_exists {
        violations.push("blocked delete removed the playbook".to_string());
    }
    if !version_still_exists {
        violations.push("blocked delete must roll back the version cascade".to_string());
    }

    // `playbook_version` rows are immutable: a trigger rejects direct deletes
    // unless the canonical delete carve-out is active, so the version goes away
    // with its parent through the repository's own delete (the same guarded
    // path the product uses). The run history is what has to be cleared first.
    let cleanup = async {
        sqlx::query("DELETE FROM playbook_run WHERE id = $1")
            .bind(playbook_run_id)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM playbook_run_group WHERE id = $1")
            .bind(run_group_id)
            .execute(&pool)
            .await?;
        SqlxPlaybookRepository::new(pool.clone())
            .delete_playbook(playbook_id)
            .await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    cleanup.expect("cleanup seeded playbook rows");

    assert!(
        violations.is_empty(),
        "{} blocked-delete contract violation(s):\n{}",
        violations.len(),
        violations.join("\n")
    );
}
