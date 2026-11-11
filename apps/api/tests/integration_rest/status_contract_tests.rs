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
    login_and_get_cookie, request,
};
use axum::http::StatusCode;
use piping_backend::api::openapi::ApiDoc;
use piping_backend::contexts::playbooks::domain::InventorySelection;
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
    "delete_job_template",
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
/// 409 (`delete_job_template`, `delete_target_set`, `delete_connconf_record`), so
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
///   `create_interface`, `update_interface`, `delete_job_template`,
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
                r#"{"address":"10.255.255.1/32","assigned_object":{"object_type":"dcim.interface","object_id":"00000000-0000-0000-0000-0000000000ff"}}"#,
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
            operation: "delete_job_template",
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
            operation: "delete_job_template",
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
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn deleting_a_blocked_job_template_over_http_returns_409() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let pool = build_test_db_pool().await.expect("db pool");
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");
    let workspace = super::project_workspace::seed_workspace(
        &pool,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await
    .expect("workspace");
    let group_id = Uuid::now_v7();
    let run_id = Uuid::now_v7();
    let spec = serde_json::to_value(workspace.launch_spec(vec![])).expect("launch spec");
    sqlx::query("INSERT INTO playbook_run_group (id, template_id, launch_spec, trigger_kind, status, total_runs) VALUES ($1, $2, $3, 'manual', 'running', 1)")
        .bind(group_id).bind(workspace.template.id).bind(&spec).execute(&pool).await.expect("seed run group");
    sqlx::query("INSERT INTO playbook_run (id, run_group_id, hostname, status) VALUES ($1, $2, 'test-host', 'running')")
        .bind(run_id).bind(group_id).execute(&pool).await.expect("seed run");
    let targets = operation_targets();
    let target = target_for(&targets, "delete_job_template");
    let uri = concrete_uri(&target.template, &workspace.template.id.to_string());
    let (status, body) = request(&app, &target.method, &uri, None, Some(&cookie))
        .await
        .expect("delete template");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("delete-blocked JSON");
    let template_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM playbook_job_template WHERE id=$1)")
            .bind(workspace.template.id)
            .fetch_one(&pool)
            .await
            .expect("template existence");
    let revision_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM playbook_project_revision WHERE revision_id=$1)",
    )
    .bind(workspace.revision.revision_id)
    .fetch_one(&pool)
    .await
    .expect("revision existence");
    let run_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM playbook_run WHERE id=$1)")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("run existence");
    let saved_spec: serde_json::Value =
        sqlx::query_scalar("SELECT launch_spec FROM playbook_run_group WHERE id=$1")
            .bind(group_id)
            .fetch_one(&pool)
            .await
            .expect("run group spec");
    let mut violations = Vec::new();
    if status != StatusCode::CONFLICT {
        violations.push(format!("DELETE {uri} answered {status}, expected 409"));
    }
    if payload["code"] != "DELETE_BLOCKED" {
        violations.push(format!("unexpected code: {}", payload["code"]));
    }
    if payload["reason_code"] != "DELETE_BLOCKED" {
        violations.push(format!(
            "unexpected reason_code: {}",
            payload["reason_code"]
        ));
    }
    if payload["message"] != "Cannot delete this record while related objects still exist" {
        violations.push(format!("unexpected message: {}", payload["message"]));
    }
    if payload["details"]["total_count"] != 1 {
        violations.push(format!(
            "expected one blocker, got {}",
            payload["details"]["total_count"]
        ));
    }
    let group = payload["details"]["blockers"]
        .as_array()
        .and_then(|groups| {
            groups
                .iter()
                .find(|group| group["table"] == "playbook_run_group")
        });
    if !group.is_some_and(|group| group["count"] == 1) {
        violations.push(format!("missing run group blocker: {payload}"));
    }
    if !template_exists || !revision_exists || !run_exists || saved_spec != spec {
        violations.push("blocked deletion changed template, revision, run or launch spec".into());
    }
    sqlx::query("DELETE FROM playbook_run WHERE id=$1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("cleanup run");
    sqlx::query("DELETE FROM playbook_run_group WHERE id=$1")
        .bind(group_id)
        .execute(&pool)
        .await
        .expect("cleanup run group");
    workspace.cleanup(&pool).await.expect("cleanup workspace");
    assert!(
        violations.is_empty(),
        "{} blocked-delete contract violation(s):\n{}",
        violations.len(),
        violations.join("\n")
    );
}
