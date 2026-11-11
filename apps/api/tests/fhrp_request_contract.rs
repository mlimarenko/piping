//! In-memory request/response contract checks; no database or generated files.

use piping_backend::api::openapi::ApiDoc;
use piping_backend::api::rest::routes::inventory::BulkCreateRequest;
use piping_backend::contexts::inventory::domain::ipam::fhrp_group::FhrpGroupRequest;
use piping_backend::contexts::secrets::application::SecretFieldIntent;
use serde_json::{Value, json};
use utoipa::OpenApi;

fn resolve<'a>(document: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => resolve(
            document,
            document
                .pointer(reference.strip_prefix('#').expect("local schema reference"))
                .expect("referenced schema exists"),
        ),
        None => schema,
    }
}

fn object_schema<'a>(document: &'a Value, schema: &'a Value) -> &'a Value {
    let schema = resolve(document, schema);
    if schema.get("properties").is_some() {
        return schema;
    }
    schema["oneOf"]
        .as_array()
        .expect("nullable schema variants")
        .iter()
        .map(|variant| resolve(document, variant))
        .find(|variant| variant.get("properties").is_some())
        .expect("object variant exists")
}

#[test]
fn write_operations_expose_structured_intents_but_read_schema_does_not() {
    let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI value");
    for (path, method, bulk) in [
        ("/api/v1/inventory/fhrp-groups", "post", false),
        ("/api/v1/inventory/fhrp-groups/{id}", "put", false),
        ("/api/v1/inventory/fhrp-groups/bulk", "post", true),
    ] {
        let request = &document["paths"][path][method]["requestBody"]["content"]["application/json"]
            ["schema"];
        let request = object_schema(&document, request);
        let group = if bulk {
            object_schema(&document, &request["properties"]["items"]["items"])
        } else {
            request
        };
        assert_eq!(group["additionalProperties"], false);
        let key = object_schema(&document, &group["properties"]["auth_key"]);
        assert_eq!(key["additionalProperties"], false);
        let intent = resolve(&document, &key["properties"]["intent"]);
        assert_eq!(intent["enum"], json!(["set", "rotate", "clear"]));
        assert!(key["properties"].get("value").is_some());
        assert!(key["properties"].get("secret_id").is_none());
        assert!(
            !group["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == "auth_key"))
        );
    }
    let read = &document["paths"]["/api/v1/inventory/fhrp-groups/{id}"]["get"]["responses"]["200"]
        ["content"]["application/json"]["schema"];
    assert!(
        object_schema(&document, read)["properties"]
            .get("auth_key")
            .is_none()
    );
}

#[test]
fn native_single_and_bulk_requests_accept_clear_and_keep_closed_input_shape() {
    let group = json!({"group_id":42,"protocol":"vrrp3","auth_key":{"intent":"clear"}});
    let request: FhrpGroupRequest = serde_json::from_value(group.clone()).expect("clear request");
    assert!(matches!(
        request.0.auth_key.as_ref().map(|key| key.intent),
        Some(SecretFieldIntent::Clear)
    ));
    assert!(request.0.validate().is_ok());
    let bulk: BulkCreateRequest<FhrpGroupRequest> =
        serde_json::from_value(json!({"items":[group.clone()]})).expect("bulk clear request");
    assert_eq!(bulk.items.len(), 1);
    assert!(bulk.items[0].0.validate().is_ok());
    let omitted: FhrpGroupRequest =
        serde_json::from_value(json!({"group_id":42,"protocol":"vrrp3"}))
            .expect("omission preserves credential");
    assert!(omitted.0.auth_key.is_none());
    for invalid in [
        json!({"group_id":42,"protocol":"vrrp3","unexpected":true}),
        json!({"group_id":42,"protocol":"vrrp3","auth_key":{"intent":"invalid"}}),
        json!({"group_id":42,"protocol":"vrrp3","auth_key":{"intent":"clear","secret_id":null}}),
    ] {
        assert!(serde_json::from_value::<FhrpGroupRequest>(invalid).is_err());
    }
}

#[test]
fn ordinary_serialization_omits_credentials_even_when_request_contains_value() {
    for intent in ["set", "rotate", "clear"] {
        let mut key = json!({"intent":intent});
        if intent != "clear" {
            key["value"] = json!(uuid::Uuid::now_v7().to_string());
        }
        let request: FhrpGroupRequest = serde_json::from_value(json!({
            "group_id":42,"protocol":"vrrp3","auth_key":key
        }))
        .expect("structured request");
        let response = serde_json::to_value(&request.0).expect("ordinary response");
        assert!(response.get("auth_key").is_none());
        let list = serde_json::to_value([&request.0]).expect("ordinary list");
        assert!(list[0].get("auth_key").is_none());
    }
}
