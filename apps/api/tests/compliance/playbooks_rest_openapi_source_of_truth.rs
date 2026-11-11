//! The served OpenAPI and serialized response DTOs describe the workspace API.

use piping_backend::api::openapi::ApiDoc;
use piping_backend::api::rest::routes::playbooks::{
    ListExecutionEnvironmentsResponse, ListJobTemplatesResponse, ListPlaybookRunArtifactsResponse,
    ListPlaybookRunEventsResponse, ListPlaybookRunGroupsResponse, ListPlaybookRunsResponse,
    ListProjectsResponse,
};
use utoipa::OpenApi;

const CANONICAL_PAGE_FIELDS: [&str; 4] = ["items", "total", "limit", "offset"];

/// Playbook collection endpoints and the DTO each one documents as its 200 body.
const PLAYBOOK_COLLECTIONS: [(&str, &str); 7] = [
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
        "/api/v1/playbooks/run-groups",
        "ListPlaybookRunGroupsResponse",
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

#[test]
fn collection_response_dtos_serialize_the_documented_page()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let document = serde_json::to_value(ApiDoc::openapi())?;
    macro_rules! page {
        ($response:ident) => {
            (
                stringify!($response),
                serde_json::to_value($response {
                    items: Vec::new(),
                    total: 0,
                    limit: 11,
                    offset: 7,
                })?,
            )
        };
    }
    for (schema_name, serialized) in [
        page!(ListExecutionEnvironmentsResponse),
        page!(ListProjectsResponse),
        page!(ListJobTemplatesResponse),
        page!(ListPlaybookRunGroupsResponse),
        page!(ListPlaybookRunsResponse),
        page!(ListPlaybookRunArtifactsResponse),
        page!(ListPlaybookRunEventsResponse),
    ] {
        let required: std::collections::BTreeSet<&str> = document["components"]["schemas"]
            [schema_name]["required"]
            .as_array()
            .ok_or("page schema has no required fields")?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        let keys: std::collections::BTreeSet<&str> = serialized
            .as_object()
            .ok_or("page DTO did not serialize to an object")?
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, required, "{schema_name}");
        assert_eq!(serialized["items"], serde_json::json!([]));
        assert_eq!(serialized["total"], 0);
        assert_eq!(serialized["limit"], 11);
        assert_eq!(serialized["offset"], 7);
    }
    Ok(())
}

#[test]
fn workspace_launch_replaces_the_retired_playbook_and_version_api()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let document = serde_json::to_value(ApiDoc::openapi())?;
    for path in [
        "/api/v1/playbooks",
        "/api/v1/playbooks/{id}",
        "/api/v1/playbooks/{id}/versions",
        "/api/v1/playbooks/{id}/duplicate",
        "/api/v1/playbooks/{id}/runs",
    ] {
        assert!(document["paths"].get(path).is_none(), "retired path {path}");
    }
    let launch = &document["paths"]["/api/v1/playbooks/job-templates/{id}/launch"]["post"];
    assert_eq!(
        launch["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/LaunchJobTemplateRequest"
    );
    assert_eq!(
        launch["responses"]["201"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/PlaybookRunGroupCreated"
    );
    Ok(())
}
