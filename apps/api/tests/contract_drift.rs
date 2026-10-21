//! No-DB contract checks for the generated OpenAPI document.
//!
//! These assertions inspect `ApiDoc::openapi()` — the document the
//! `dump-openapi` binary writes to `contracts/piping.openapi.yaml` and the
//! `/api/v1/openapi/piping.openapi.yaml` route serves — so a response type that
//! loses a typed schema reference fails here without a database and without a
//! running server.

use piping_backend::api::openapi::ApiDoc;
use utoipa::OpenApi;

#[test]
fn sync_contract_exposes_typed_targets_and_catalog_resources() {
    let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI serializes");
    let schemas = &document["components"]["schemas"];
    for response in [
        "MappingConfigurationDraft",
        "MappingConfigurationResponse",
        "SyncMappingConfigurationResponse",
    ] {
        assert_eq!(
            schemas[response]["properties"]["target"]["$ref"], "#/components/schemas/MappingTarget",
            "{response} must expose the canonical target discriminator, not an untyped Object"
        );
    }
    assert_eq!(
        schemas["MappingTarget"]["oneOf"]
            .as_array()
            .expect("tagged target variants")
            .len(),
        2
    );
    assert_eq!(
        schemas["LocalInventoryResourcesResponse"]["properties"]["resources"]["items"]["$ref"],
        "#/components/schemas/LocalInventoryResource"
    );
    assert_eq!(
        schemas["LocalInventoryResourceResponse"]["properties"]["resource"]["$ref"],
        "#/components/schemas/LocalInventoryResource"
    );
    assert_eq!(
        schemas["LocalInventoryResource"]["properties"]["fields"]["items"]["$ref"],
        "#/components/schemas/LocalInventoryResourceField"
    );
}
