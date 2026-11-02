//! Generated request schemas agree with native Tag scalar deserialization and
//! the shared inventory membership boundary, not remote labels or read DTOs.
use piping_backend::api::openapi::ApiDoc;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::UpdateTag;
use serde_json::json;
use utoipa::OpenApi;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn update_tag_schema_and_deserializer_allow_omission_but_reject_null() -> TestResult {
    let document = serde_json::to_value(ApiDoc::openapi())?;
    let schema = &document["components"]["schemas"]["UpdateTag"];
    serde_json::from_value::<UpdateTag>(json!({}))?;
    for (field, value, kind) in [
        ("name", json!("Catalog name"), "string"),
        ("slug", json!("catalog-name"), "string"),
        ("color", json!("abcdef"), "string"),
        ("description", json!(""), "string"),
        ("weight", json!(0), "integer"),
    ] {
        assert_eq!(
            schema["properties"][field]["type"], kind,
            "{field} must not advertise null"
        );
        assert!(
            !schema["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|value| value == field)),
            "{field} remains optional"
        );
        let mut supplied = json!({});
        supplied[field] = value;
        serde_json::from_value::<UpdateTag>(supplied.clone())?;
        supplied[field] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<UpdateTag>(supplied).is_err(),
            "{field}: schema and actual request deserializer must both reject null"
        );
    }
    Ok(())
}

#[test]
fn native_membership_request_schemas_expose_optional_nonnull_uuid_sets() -> TestResult {
    let document = serde_json::to_value(ApiDoc::openapi())?;
    let id = Uuid::parse_str("abcdef00-1234-7000-8000-abcdef012345")?.to_string();
    for name in [
        "ModuleBayTypeCreatePayload",
        "ModuleBayTypeUpdatePayload",
        "CableBundleCreatePayload",
        "CableBundleUpdatePayload",
        "RackGroupCreatePayload",
        "RackGroupUpdatePayload",
        "WirelessLanGroupCreate",
        "WirelessLanGroupWrite",
        "WirelessLanCreate",
        "WirelessLanWrite",
        "WirelessLinkCreate",
        "WirelessLinkWrite",
        "FhrpGroupRequest",
    ] {
        let schema = &document["components"]["schemas"][name];
        assert!(schema.is_object(), "public request schema missing: {name}");
        assert!(
            !schema["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|value| value == "tags")),
            "{name}: tags omission must remain valid"
        );
        let tags = &schema["properties"]["tags"];
        assert_eq!(
            tags["type"], "array",
            "{name}: null is not a membership array"
        );
        assert_eq!(
            tags["uniqueItems"], true,
            "{name}: duplicate memberships are rejected"
        );
        assert!(
            tags["minItems"].as_u64().is_none_or(|minimum| minimum == 0),
            "{name}: [] must remain a valid clear operation"
        );
        assert_eq!(tags["items"]["type"], "string");
        assert_eq!(tags["items"]["format"], "uuid");
        let pattern = regex::Regex::new(
            tags["items"]["pattern"]
                .as_str()
                .ok_or("canonical UUID pattern missing")?,
        )?;
        assert!(
            pattern.is_match(&id),
            "{name}: canonical UUID must be advertised"
        );
        let uppercase = id.to_uppercase();
        let compact = id.replace('-', "");
        for value in ["slug", uppercase.as_str(), compact.as_str()] {
            assert!(
                !pattern.is_match(value),
                "{name}: noncanonical token must not match the published item pattern"
            );
        }
    }
    Ok(())
}
