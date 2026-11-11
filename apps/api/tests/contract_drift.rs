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
fn every_inline_operation_response_has_a_description() {
    let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI serializes");
    let paths = document["paths"].as_object().expect("OpenAPI paths");
    let mut missing = Vec::new();
    for (path, item) in paths {
        for method in [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ] {
            let Some(responses) = item[method]["responses"].as_object() else {
                continue;
            };
            for (status, response) in responses {
                if response.get("$ref").is_none()
                    && !response
                        .get("description")
                        .is_some_and(|value| value.is_string())
                {
                    missing.push(format!("{method} {path} {status}"));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "responses without description: {missing:?}"
    );
}

#[test]
fn target_set_detail_exposes_typed_members() {
    let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI serializes");
    let schemas = &document["components"]["schemas"];
    assert_eq!(
        schemas["PlaybookTargetSet"]["properties"]["members"]["items"]["$ref"],
        "#/components/schemas/PlaybookTargetSetMember"
    );
    let required: std::collections::BTreeSet<&str> = schemas["PlaybookTargetSetMember"]["required"]
        .as_array()
        .expect("member schema required fields")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    for field in [
        "target_kind",
        "target_id",
        "source_kind",
        "source_id",
        "sort_order",
    ] {
        assert!(
            required.contains(field),
            "member schema must require {field}"
        );
    }
}

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

/// NetBox 4.7.1 PWA-75/PWA-80 gap cases, not the full model census.
/// This checks the published document, not routing, authorization or CRUD behavior.
#[test]
fn netbox_gap_models_have_native_openapi_owners() {
    let doc = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI serializes");
    let mut failures = Vec::new();
    // Direct native collections: item GET must actually return the model schema,
    // and the corresponding collection must expose a GET operation.
    let direct = [
        (
            "CoolingIntakeTemplate",
            "cooling-intake-templates",
            "CoolingIntakeTemplate",
        ),
        (
            "CoolingOutflowTemplate",
            "cooling-outflow-templates",
            "CoolingOutflowTemplate",
        ),
        (
            "ModuleBayTemplate",
            "module-bay-templates",
            "ModuleBayTemplate",
        ),
        (
            "ConsolePortTemplate",
            "console-port-templates",
            "ConsolePortTemplate",
        ),
        (
            "ConsoleServerPortTemplate",
            "console-server-port-templates",
            "ConsoleServerPortTemplate",
        ),
        (
            "PowerPortTemplate",
            "power-port-templates",
            "PowerPortTemplate",
        ),
        (
            "PowerOutletTemplate",
            "power-outlet-templates",
            "PowerOutletTemplate",
        ),
        (
            "InterfaceTemplate",
            "interface-templates",
            "InterfaceTemplate",
        ),
        (
            "FrontPortTemplate",
            "front-port-templates",
            "FrontPortTemplate",
        ),
        (
            "RearPortTemplate",
            "rear-port-templates",
            "RearPortTemplate",
        ),
        (
            "DeviceBayTemplate",
            "device-bay-templates",
            "DeviceBayTemplate",
        ),
        (
            "InventoryItemTemplate",
            "inventory-item-templates",
            "InventoryItemTemplate",
        ),
        (
            "ModuleTypeProfile",
            "module-type-profiles",
            "ModuleTypeProfile",
        ),
        (
            "VLANTranslationPolicy",
            "vlan-translation-policies",
            "VlanTranslationPolicy",
        ),
        (
            "VLANTranslationRule",
            "vlan-translation-rules",
            "VlanTranslationRule",
        ),
        (
            "CircuitGroupAssignment",
            "circuit-group-assignments",
            "CircuitGroupAssignment",
        ),
        (
            "VirtualCircuitTermination",
            "virtual-circuit-terminations",
            "VirtualCircuitTermination",
        ),
        ("L2VPN", "l2vpns", "L2Vpn"),
        ("L2VPNTermination", "l2vpn-terminations", "L2VpnTermination"),
        ("RackGroup", "rack-groups", "RackGroupResponse"),
        ("CableBundle", "cable-bundles", "CableBundleResponse"),
        ("ModuleBayType", "module-bay-types", "ModuleBayType"),
        ("CoolingSource", "cooling-sources", "CoolingSource"),
        ("CoolingFeed", "cooling-feeds", "CoolingFeed"),
        ("CoolingIntake", "cooling-intakes", "CoolingIntake"),
        ("CoolingOutflow", "cooling-outflows", "CoolingOutflow"),
        ("FHRPGroup", "fhrp-groups", "FhrpGroup"),
        (
            "FHRPGroupAssignment",
            "fhrp-group-assignments",
            "FhrpGroupAssignment",
        ),
        (
            "VirtualMachineType",
            "virtual-machine-types",
            "VirtualMachineType",
        ),
        (
            "WirelessLANGroup",
            "wireless_lan_groups",
            "WirelessLanGroup",
        ),
        ("WirelessLAN", "wireless_lans", "WirelessLan"),
        ("WirelessLink", "wireless_links", "WirelessLink"),
    ];
    for (model, collection, schema) in direct {
        let path = format!("/api/v1/inventory/{collection}");
        let item = format!("{path}/{{id}}");
        let reference = format!("#/components/schemas/{schema}");
        if !doc["paths"][&path]["get"].is_object()
            || doc["paths"][&item]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
                ["$ref"]
                != reference.as_str()
            || !doc["components"]["schemas"][schema].is_object()
        {
            failures.push(format!(
                "{model}: missing collection GET or typed item GET ({path}, {schema})"
            ));
        }
    }

    // CableTermination is an internal cable-to-endpoint relation, not a native
    // standalone resource: trace returns segments connecting typed endpoints.
    let trace = &doc["paths"]["/api/v1/inventory/cabling/trace"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"]["$ref"];
    let schemas = &doc["components"]["schemas"];
    if trace != "#/components/schemas/CablingTrace"
        || schemas["CablingTrace"]["properties"]["segments"]["items"]["$ref"]
            != "#/components/schemas/CablingTraceSegment"
        || schemas["CablingTraceSegment"]["properties"]["from"]["$ref"]
            != "#/components/schemas/CablingEndpointRef"
        || !schemas["CablingTraceSegment"]["properties"]["cable"]["oneOf"]
            .as_array()
            .is_some_and(|variants| {
                variants
                    .iter()
                    .any(|variant| variant["$ref"] == "#/components/schemas/CablingCableSummary")
            })
    {
        failures.push("CableTermination: missing typed cable/endpoint trace owner".to_string());
    }

    assert!(
        failures.is_empty(),
        "native model coverage gaps:\n{}",
        failures.join("\n")
    );
}
