//! Gate: every request/response body in the OpenAPI document must have a real
//! schema (not the empty `{}` produced by `body = serde_json::Value`).
//!
//! # Scope
//!
//! Covers ALL content schemas in the document:
//! - Response bodies at every status code (2XX, 4XX, 5XX)
//! - Request bodies
//!
//! Error (4XX/5XX) responses now use `ApiErrorResponse`. The
//! 2XX response bodies and request bodies are not yet typed
//! and appear in the seed allowlist below.
//!
//! # Allowlist
//!
//! ⚠️  The allowlist **may only shrink**. Never add new entries; each empty
//! schema is a contract gap. Remove entries as typed structs are added.
//!
//! The allowlist was seeded from the complete list of offenders: 104 entries
//! for 2XX/request_body + 7 for non-envelope 422 responses.
//!
//! Keys are:
//! - `"METHOD PATH STATUS"` for response schemas
//! - `"REQUESTBODY METHOD PATH"` for request body schemas
//!
//! # How the detection works
//!
//! utoipa serialises `body = serde_json::Value` as an empty JSON Schema (`{}`).
//! A typed `$ref` (e.g. `ApiErrorResponse`) serialises as `{"$ref": "..."}`.
//! We serialise the full OpenAPI doc to JSON and flag any content whose
//! `schema` is exactly `{}`. Error responses must additionally declare an
//! `application/json` schema; a bodyless error declaration is never allowed.

use std::collections::BTreeSet;

use piping_backend::api::openapi::ApiDoc;
use utoipa::OpenApi;

/// Key for a response body: `"METHOD PATH STATUS"`.
fn response_key(method: &str, path: &str, status: &str) -> String {
    format!("{} {} {}", method.to_uppercase(), path, status)
}

/// Key for a request body: `"REQUESTBODY METHOD PATH"`.
fn request_body_key(method: &str, path: &str) -> String {
    format!("REQUESTBODY {} {}", method.to_uppercase(), path)
}

/// Seeded allowlist of operations whose schemas are still empty `{}`.
///
/// ⚠️  This list **may only shrink**. Do NOT add new entries.
/// Remove an entry only after adding a proper `#[derive(utoipa::ToSchema)]`
/// type to the handler annotation and verifying this test still passes.
fn empty_schema_allowlist() -> BTreeSet<String> {
    let mut set = BTreeSet::new();

    // ──────────────────────────────────────────────────────────────────────────
    // FLAGGED: NON-ENVELOPE 422 RESPONSES — typed schema work pending
    //
    // These handlers return a genuinely different shape from `ApiErrorResponse`
    // and must get their own typed structs before being removed from this list.
    //
    // config_contexts 422 → ValidationErrorResponse {error, message, details}
    set.insert(response_key("POST", "/api/v1/config-contexts", "422"));
    set.insert(response_key("PUT", "/api/v1/config-contexts/{id}", "422"));

    // mapping_specs 422 → ad-hoc {errors: [...], warnings: [...]}
    set.insert(response_key(
        "POST",
        "/api/v1/providers/{id}/mapping-configurations",
        "422",
    ));
    set.insert(response_key(
        "POST",
        "/api/v1/providers/{id}/mapping-configurations/{logical_id}/versions",
        "422",
    ));
    set.insert(response_key(
        "POST",
        "/api/v1/sync/mapping-configurations",
        "422",
    ));
    set.insert(response_key(
        "PATCH",
        "/api/v1/sync/mapping-configurations/{id}",
        "422",
    ));
    set.insert(response_key(
        "POST",
        "/api/v1/sync/mapping-configurations/preview",
        "422",
    ));

    // ──────────────────────────────────────────────────────────────────────────
    // 2XX RESPONSE BODIES — not yet typed
    // Seeded from the full offender list.
    set.insert(response_key(
        "GET",
        "/api/v1/providers/{id}/mapping-configurations",
        "200",
    ));
    set.insert(response_key(
        "GET",
        "/api/v1/providers/{id}/sample-record",
        "200",
    ));
    set.insert(response_key("GET", "/health", "200"));
    set.insert(response_key(
        "POST",
        "/api/v1/providers/{id}/validate",
        "200",
    ));
    set.insert(response_key(
        "POST",
        "/api/v1/sync/mapping-configurations/preview",
        "200",
    ));
    set.insert(response_key(
        "POST",
        "/api/v1/webhooks/netbox/{provider_id}",
        "200",
    ));

    // ──────────────────────────────────────────────────────────────────────────
    // REQUEST BODIES — not yet typed
    // Seeded from the full offender list.
    set.insert(request_body_key("POST", "/api/v1/mcp"));

    // ── Additional 2XX response bodies seeded from full offender scan ─────────
    set.insert(response_key(
        "GET",
        "/api/v1/sync/mapping-configurations",
        "200",
    ));
    set.insert(response_key("POST", "/api/v1/mcp", "200"));
    set.insert(response_key(
        "POST",
        "/api/v1/sync/mapping-configurations/suggest-reverse",
        "200",
    ));
    set.insert(response_key(
        "POST",
        "/api/v1/sync/mapping-configurations/validate",
        "200",
    ));

    // ── Additional request bodies seeded from full offender scan ─────────────
    set.insert(request_body_key(
        "PATCH",
        "/api/v1/sync/mapping-configurations/{id}",
    ));
    set.insert(request_body_key(
        "POST",
        "/api/v1/providers/{id}/mapping-configurations",
    ));
    set.insert(request_body_key(
        "POST",
        "/api/v1/providers/{id}/mapping-configurations/{logical_id}/versions",
    ));
    set.insert(request_body_key(
        "POST",
        "/api/v1/sync/mapping-configurations",
    ));

    set
}

/// Scan one operation's request body and responses, appending a line per
/// empty/undefined schema that is not allowlisted.
fn collect_operation_violations(
    operation: &serde_json::Value,
    method: &str,
    path: &str,
    allowlist: &BTreeSet<String>,
    empty_schema: &serde_json::Value,
    violations: &mut Vec<String>,
) {
    collect_request_body_violations(operation, method, path, allowlist, empty_schema, violations);
    collect_response_body_violations(operation, method, path, allowlist, empty_schema, violations);
}

fn collect_request_body_violations(
    operation: &serde_json::Value,
    method: &str,
    path: &str,
    allowlist: &BTreeSet<String>,
    empty_schema: &serde_json::Value,
    violations: &mut Vec<String>,
) {
    let Some(content) = operation
        .get("requestBody")
        .and_then(|rb| rb.get("content"))
        .and_then(|c| c.as_object())
    else {
        return;
    };
    for media_obj in content.values() {
        let Some(schema) = media_obj.get("schema") else {
            continue;
        };
        if schema == empty_schema {
            let key = request_body_key(method, path);
            if !allowlist.contains(&key) {
                violations.push(format!(
                    "  REQUESTBODY {} {} — empty schema",
                    method.to_uppercase(),
                    path
                ));
            }
        }
    }
}

fn collect_response_body_violations(
    operation: &serde_json::Value,
    method: &str,
    path: &str,
    allowlist: &BTreeSet<String>,
    empty_schema: &serde_json::Value,
    violations: &mut Vec<String>,
) {
    let Some(responses) = operation.get("responses").and_then(|r| r.as_object()) else {
        return;
    };
    for (status_code, response_obj) in responses {
        let is_error_response = matches!(status_code.as_str(), "4XX" | "5XX")
            || status_code
                .parse::<u16>()
                .is_ok_and(|status| (400..600).contains(&status));
        let has_json_schema = response_obj
            .get("content")
            .and_then(|content| content.get("application/json"))
            .and_then(|media_type| media_type.get("schema"))
            .is_some();
        if is_error_response && !has_json_schema {
            violations.push(format!(
                "  {} {} {} — missing content.application/json.schema",
                method.to_uppercase(),
                path,
                status_code
            ));
            continue;
        }
        let Some(content) = response_obj.get("content").and_then(|c| c.as_object()) else {
            continue;
        };
        for media_obj in content.values() {
            let Some(schema) = media_obj.get("schema") else {
                continue;
            };
            if schema == empty_schema {
                let key = response_key(method, path, status_code);
                if !allowlist.contains(&key) {
                    violations.push(format!(
                        "  {} {} {} — empty schema",
                        method.to_uppercase(),
                        path,
                        status_code
                    ));
                }
            }
        }
    }
}

#[test]
fn no_empty_schemas_in_openapi() {
    let doc = ApiDoc::openapi();
    let json = serde_json::to_value(&doc).expect("OpenAPI doc should serialise to JSON");

    let allowlist = empty_schema_allowlist();
    let empty_schema = serde_json::json!({});
    let paths = json
        .get("paths")
        .and_then(|p| p.as_object())
        .expect("OpenAPI doc has no `paths` map");

    let http_methods = [
        "get", "post", "put", "patch", "delete", "head", "options", "trace",
    ];

    let mut violations: Vec<String> = Vec::new();
    for (path, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };
        let operations = path_item
            .iter()
            .filter(|(method, _)| http_methods.contains(&method.as_str()));
        for (method, operation) in operations {
            collect_operation_violations(
                operation,
                method,
                path,
                &allowlist,
                &empty_schema,
                &mut violations,
            );
        }
    }

    assert!(
        violations.is_empty(),
        "Operations with empty schemas (not in allowlist):\n{}\n\n\
         Fix: use a typed struct with `#[derive(utoipa::ToSchema)]` and annotate the handler.\n\
         For error responses use `body = crate::api::rest::error_mapping::ApiErrorResponse`.\n\
         Do NOT expand the allowlist — it may only shrink.",
        violations.join("\n")
    );
}
