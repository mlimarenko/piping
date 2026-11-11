//! Router/OpenAPI contract: every documented operation must be mounted.
//!
//! The contract file `contracts/piping.openapi.yaml` is read at runtime, so a
//! document that drifts from the router fails here without any source-scanning
//! heuristic.

use super::*;

/// Substituted for every `{param}` placeholder in a path template.
///
/// Any syntactically valid segment works: axum matches placeholders by
/// position, and the handlers answer 400/401/404 for an unknown id. What the
/// test asserts is that a route claims the request at all.
const PATH_PARAM_PLACEHOLDER: &str = "00000000-0000-0000-0000-000000000001";

/// HTTP methods that carry an operation object inside an OpenAPI path item.
/// Everything else there (`parameters`, `servers`, `summary`, `$ref`, ...) is
/// not an operation.
const OPERATION_METHODS: [&str; 8] = [
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// Replace whole-segment placeholders with [`PATH_PARAM_PLACEHOLDER`].
///
/// A placeholder that is only part of a segment (`{id}.png`) cannot be
/// substituted this way, so it is reported instead of silently requested as a
/// literal path.
fn concrete_uri(template: &str) -> String {
    let uri = template
        .split('/')
        .map(|segment| {
            if segment.len() > 2 && segment.starts_with('{') && segment.ends_with('}') {
                PATH_PARAM_PLACEHOLDER
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    assert!(
        !uri.contains('{') && !uri.contains('}'),
        "path template {template:?} has a placeholder that is not a whole segment: {uri:?}"
    );
    uri
}

/// Collect `(METHOD, path template)` for every operation in the document.
#[allow(
    clippy::expect_used,
    reason = "fixture helper outside #[test]: a contract file that is not the documented shape fails the calling contract test"
)]
fn openapi_operations(document: &serde_yml::Value) -> Vec<(String, String)> {
    let paths = document
        .get("paths")
        .and_then(serde_yml::Value::as_mapping)
        .expect("OpenAPI document declares a `paths` mapping");

    let mut operations = Vec::new();
    for (template, item) in paths {
        let template = template
            .as_str()
            .expect("OpenAPI path key is a string")
            .to_string();
        let item = item.as_mapping().expect("OpenAPI path item is a mapping");
        for (key, _) in item {
            let Some(method) = key.as_str() else {
                continue;
            };
            if OPERATION_METHODS.contains(&method) {
                operations.push((method.to_ascii_uppercase(), template.clone()));
            }
        }
    }
    operations
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn every_openapi_operation_is_mounted() {
    let state = build_test_app_state().await.expect("app state");
    // The teapot fallback marks a request that no mounted route claimed. A
    // mounted path with an unmounted method answers 405, and the handlers
    // themselves answer 400/401/403/... without credentials, so 418 is the one
    // status that can only mean "the router does not know this path".
    let app =
        piping_backend::app::build_router(state).fallback(|| async { StatusCode::IM_A_TEAPOT });

    // Prove the marker itself: every 418 below would otherwise be unverifiable.
    let (probe_status, _) = request(&app, "GET", "/__router_contract_probe__", None, None)
        .await
        .expect("probe request");
    assert_eq!(
        probe_status,
        StatusCode::IM_A_TEAPOT,
        "a request matching no route must report the teapot marker"
    );

    let document_path = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/piping.openapi.yaml");
    let raw = std::fs::read_to_string(document_path)
        .unwrap_or_else(|error| panic!("read {document_path}: {error}"));
    let document: serde_yml::Value =
        serde_yml::from_str(&raw).unwrap_or_else(|error| panic!("parse {document_path}: {error}"));

    let operations = openapi_operations(&document);
    assert!(
        !operations.is_empty(),
        "OpenAPI contract {document_path} lists no operations"
    );

    let mut unmounted = Vec::new();
    let mut method_not_mounted = Vec::new();

    for (method, template) in &operations {
        let uri = concrete_uri(template);
        let (status, _) = request(&app, method, &uri, None, None)
            .await
            .unwrap_or_else(|error| panic!("{method} {uri} ({template}): {error}"));

        if status == StatusCode::IM_A_TEAPOT {
            unmounted.push(format!("{method} {template}"));
        } else if status == StatusCode::METHOD_NOT_ALLOWED {
            method_not_mounted.push(format!("{method} {template}"));
        }
    }

    assert!(
        unmounted.is_empty(),
        "{} documented operations are not mounted: {unmounted:#?}",
        unmounted.len()
    );
    assert!(
        method_not_mounted.is_empty(),
        "{} documented methods are missing on a mounted path: {method_not_mounted:#?}",
        method_not_mounted.len()
    );
}
