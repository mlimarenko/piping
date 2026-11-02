//! Ratchet: high-traffic REST mutations whose errors originate below the
//! handler must keep declaring their client-relevant statuses in the OpenAPI
//! document.
//!
//! A conservative source scan cannot infer the status of an error raised deep
//! in a service, so this checks the declared contract instead. It reads the
//! runtime `ApiDoc` only — no source-text parsing — and lists the operations
//! and exact statuses that must not silently disappear.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use piping_backend::api::openapi::ApiDoc;
use utoipa::OpenApi;

/// Operations with errors that originate below the handler, plus the exact
/// client-relevant statuses they must declare. Exact codes must stay
/// explicit; a range response does not satisfy an entry in this table.
const REQUIRED: &[(&str, &[&str])] = &[
    (
        "create_connection_session",
        &["400", "401", "403", "404", "409", "5XX"],
    ),
    (
        "delete_user_credential_set",
        &["400", "401", "403", "404", "409", "5XX"],
    ),
    ("create_ip_address", &["400", "401", "403", "5XX"]),
    ("update_ip_address", &["400", "401", "403", "404", "5XX"]),
    // 409 here is DELETE_BLOCKED, not the generic conflict mapping. The
    // inventory repositories delete through `delete_record_guarded`, which
    // intercepts the foreign-key violation — 23503 for NO ACTION and 23001
    // for RESTRICT, both — and answers with the blocking rows instead of
    // letting classify_database_code turn it into a 400. The frontend's
    // blocked-delete dialog keys on exactly that status and payload.
    ("delete_ip_address", &["400", "401", "403", "409", "5XX"]),
    ("create_interface", &["400", "401", "403", "409", "5XX"]),
    (
        "update_interface",
        &["400", "401", "403", "404", "409", "5XX"],
    ),
    ("delete_interface", &["400", "401", "403", "409", "5XX"]),
    ("delete_playbook", &["409"]),
    ("delete_target_set", &["409"]),
    ("delete_connconf_record", &["409"]),
];

fn api_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn required_mutation_statuses_are_declared_by_api_doc() {
    let contract_statuses =
        contract_statuses_by_operation().expect("the gate needs a well-formed ApiDoc");
    let mut violations = Vec::new();

    for (operation_id, required) in REQUIRED {
        let Some(declared) = contract_statuses.get(*operation_id) else {
            violations.push(format!(
                "critical mutation {operation_id} is absent from ApiDoc"
            ));
            continue;
        };
        for status in *required {
            if !declared.contains(*status) {
                violations.push(format!(
                    "critical mutation {operation_id} is missing exact status {status}"
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "{} undocumented mutation status(es):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

#[test]
fn openapi_document_is_valid_yaml_with_required_top_level_keys() {
    let yaml_path = api_root().join("contracts/piping.openapi.yaml");
    let raw = fs::read_to_string(&yaml_path).expect("read yaml");
    let doc: serde_yml::Value = serde_yml::from_str(&raw).expect("parse yaml");
    let root = doc.as_mapping().expect("top-level must be an object");
    for key in ["openapi", "info", "paths", "components"] {
        assert!(
            root.contains_key(serde_yml::Value::String(key.to_string())),
            "missing top-level `{key}` in piping.openapi.yaml"
        );
    }
}

fn contract_statuses_by_operation() -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let document = serde_json::to_value(ApiDoc::openapi())
        .map_err(|error| format!("ApiDoc does not serialize: {error}"))?;
    let paths = document["paths"]
        .as_object()
        .ok_or_else(|| "ApiDoc has no `paths` object".to_string())?;
    let mut operations = BTreeMap::new();

    for path_item in paths.values().filter_map(serde_json::Value::as_object) {
        for operation in path_item.values().filter_map(serde_json::Value::as_object) {
            let Some(operation_id) = operation
                .get("operationId")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let statuses = operation
                .get("responses")
                .and_then(serde_json::Value::as_object)
                .into_iter()
                .flatten()
                .map(|(status, _)| status.clone())
                .collect();
            if operations
                .insert(operation_id.to_string(), statuses)
                .is_some()
            {
                return Err(format!("duplicate operationId {operation_id} in ApiDoc"));
            }
        }
    }
    Ok(operations)
}
