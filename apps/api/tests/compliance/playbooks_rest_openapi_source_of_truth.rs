//! Playbook REST contract, checked against the OpenAPI document the API serves.
//!
//! Both gates here read data, never source text: `ApiDoc::openapi()` and the
//! committed `contracts/piping.openapi.yaml` it is generated into. Whether the
//! committed frontend `schema.gen.ts` matches that contract is checked by
//! `apps/web/src/shared/api/schemaGenFreshness.test.ts`, which regenerates the
//! types and compares them — a text search for a path string proved only that
//! the string occurs somewhere, not that the contract is current.

use std::fs;
use std::path::PathBuf;

use piping_backend::api::openapi::ApiDoc;
use utoipa::OpenApi;

const CANONICAL_PAGE_FIELDS: [&str; 4] = ["items", "total", "limit", "offset"];

/// Playbook collection endpoints and the DTO each one documents as its 200 body.
const PLAYBOOK_COLLECTIONS: [(&str, &str); 8] = [
    ("/api/v1/playbooks", "ListPlaybooksResponse"),
    (
        "/api/v1/playbooks/execution-environments",
        "ListExecutionEnvironmentsResponse",
    ),
    ("/api/v1/playbooks/projects", "ListProjectsResponse"),
    (
        "/api/v1/playbooks/job-templates",
        "ListJobTemplatesResponse",
    ),
    (
        "/api/v1/playbooks/{id}/versions",
        "ListPlaybookVersionsResponse",
    ),
    ("/api/v1/playbooks/runs", "ListPlaybookRunsResponse"),
    (
        "/api/v1/playbooks/runs/{run_id}/artifacts",
        "ListPlaybookRunArtifactsResponse",
    ),
    (
        "/api/v1/playbooks/runs/{run_id}/events",
        "ListPlaybookRunEventsResponse",
    ),
];

fn api_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_source(relative_path: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = api_root().join(relative_path);
    Ok(fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?)
}

#[test]
fn committed_openapi_is_generated_from_rest_annotations()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let committed = read_source("contracts/piping.openapi.yaml")?;
    let generated =
        serde_yml::to_string(&ApiDoc::openapi()).expect("generated OpenAPI should serialize");

    assert_eq!(
        committed, generated,
        "contracts/piping.openapi.yaml must be regenerated from ApiDoc::openapi()"
    );
    Ok(())
}

#[test]
fn playbook_collections_use_canonical_pagination_contract()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let document = serde_json::to_value(ApiDoc::openapi())?;

    for (path, schema_name) in PLAYBOOK_COLLECTIONS {
        let operation = &document["paths"][path]["get"];
        assert!(
            operation.is_object(),
            "OpenAPI must expose GET collection `{path}`"
        );

        let query_names: std::collections::BTreeSet<&str> = operation["parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|parameter| parameter["in"] == "query")
            .filter_map(|parameter| parameter["name"].as_str())
            .collect();
        for parameter in ["limit", "offset"] {
            assert!(
                query_names.contains(parameter),
                "`{path}` must use canonical `{parameter}` pagination"
            );
        }
        for legacy_parameter in ["page", "per_page"] {
            assert!(
                !query_names.contains(legacy_parameter),
                "`{path}` must not expose legacy `{legacy_parameter}` pagination"
            );
        }

        let expected_ref = format!("#/components/schemas/{schema_name}");
        assert_eq!(
            operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"].as_str(),
            Some(expected_ref.as_str()),
            "`{path}` must document the same response DTO its handler returns"
        );

        let component = &document["components"]["schemas"][schema_name];
        let properties = component["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("OpenAPI component `{schema_name}` must have properties"));
        let required: std::collections::BTreeSet<&str> = component["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect();

        for field in CANONICAL_PAGE_FIELDS {
            assert!(
                properties.contains_key(field),
                "OpenAPI component `{schema_name}` is missing `{field}`"
            );
            assert!(
                required.contains(field),
                "OpenAPI component `{schema_name}` must require `{field}`"
            );
        }
    }

    Ok(())
}
