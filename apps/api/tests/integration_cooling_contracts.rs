//! REST contracts for cooling units, capability discovery and blocker privacy.
mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    login_and_get_cookie,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use utoipa::OpenApi;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn served_document() -> TestResult<Value> {
    Ok(serde_json::to_value(
        piping_backend::api::openapi::ApiDoc::openapi(),
    )?)
}

fn resolve_schema<'a>(document: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => resolve_schema(
            document,
            document
                .pointer(reference.strip_prefix('#').expect("local schema reference"))
                .expect("served schema exists"),
        ),
        None => schema,
    }
}

fn allows_null(document: &Value, schema: &Value) -> bool {
    let schema = resolve_schema(document, schema);
    schema["nullable"] == true
        || schema["type"] == "null"
        || schema["type"]
            .as_array()
            .is_some_and(|types| types.iter().any(|kind| kind == "null"))
        || ["anyOf", "oneOf"].iter().any(|key| {
            schema[*key].as_array().is_some_and(|variants| {
                variants
                    .iter()
                    .any(|variant| allows_null(document, variant))
            })
        })
}

#[test]
fn cooling_served_list_operations_expose_only_their_supported_query_parameters() -> TestResult {
    let document = served_document()?;
    for (collection, fields) in [
        (
            "cooling-sources",
            vec!["site_id", "location_id", "status", "type"],
        ),
        (
            "cooling-feeds",
            vec!["cooling_source_id", "rack_id", "tenant_id", "status"],
        ),
        (
            "cooling-intakes",
            vec!["device_id", "module_id", "cooling_outflow_id", "type"],
        ),
        (
            "cooling-outflows",
            vec!["device_id", "module_id", "cooling_intake_id", "type"],
        ),
        (
            "cooling-intake-templates",
            vec!["device_type_id", "module_type_id", "type"],
        ),
        (
            "cooling-outflow-templates",
            vec![
                "device_type_id",
                "module_type_id",
                "cooling_intake_id",
                "type",
            ],
        ),
    ] {
        let path = format!("/api/v1/inventory/{collection}");
        let parameters = document["paths"][&path]["get"]["parameters"]
            .as_array()
            .ok_or("served list parameters")?;
        let actual: std::collections::BTreeSet<_> = parameters
            .iter()
            .filter(|parameter| parameter["in"] == "query")
            .map(|parameter| parameter["name"].as_str().expect("query parameter name"))
            .collect();
        let expected: std::collections::BTreeSet<_> =
            ["id", "limit", "offset", "search", "sort", "order"]
                .into_iter()
                .chain(fields)
                .collect();
        assert_eq!(actual, expected, "served query contract for {collection}");
    }
    Ok(())
}

#[test]
fn cooling_served_patch_operations_distinguish_omission_from_nullable_clears() -> TestResult {
    let document = served_document()?;
    for (collection, nonnullable, nullable) in [
        (
            "cooling-sources",
            vec![
                "name",
                "type",
                "status",
                "site_id",
                "description",
                "comments",
                "custom_fields",
            ],
            vec!["fluid_type", "cooling_capacity", "location_id", "owner_id"],
        ),
        (
            "cooling-feeds",
            vec![
                "name",
                "status",
                "cooling_source_id",
                "description",
                "comments",
                "custom_fields",
            ],
            vec![
                "cooling_capacity",
                "max_flow",
                "max_flow_unit",
                "rack_id",
                "tenant_id",
                "owner_id",
            ],
        ),
        (
            "cooling-intakes",
            vec!["name", "label", "device_id", "description", "custom_fields"],
            vec![
                "type",
                "diameter",
                "diameter_unit",
                "max_flow",
                "max_flow_unit",
                "module_id",
                "cooling_outflow_id",
                "owner_id",
            ],
        ),
        (
            "cooling-outflows",
            vec!["name", "label", "device_id", "description", "custom_fields"],
            vec![
                "type",
                "diameter",
                "diameter_unit",
                "module_id",
                "cooling_intake_id",
                "owner_id",
            ],
        ),
        (
            "cooling-intake-templates",
            vec!["name", "label", "description"],
            vec![
                "type",
                "diameter",
                "diameter_unit",
                "max_flow",
                "max_flow_unit",
                "device_type_id",
                "module_type_id",
            ],
        ),
        (
            "cooling-outflow-templates",
            vec!["name", "label", "description"],
            vec![
                "type",
                "diameter",
                "diameter_unit",
                "device_type_id",
                "module_type_id",
                "cooling_intake_id",
            ],
        ),
    ] {
        let path = format!("/api/v1/inventory/{collection}/{{id}}");
        let operation = &document["paths"][&path]["put"];
        let schema = resolve_schema(
            &document,
            &operation["requestBody"]["content"]["application/json"]["schema"],
        );
        assert_eq!(schema["additionalProperties"], false);
        assert!(
            schema["required"].as_array().is_none_or(Vec::is_empty),
            "all patch fields support omission"
        );
        for (fields, expected) in [(nonnullable, false), (nullable, true)] {
            for field in fields {
                let property = schema["properties"]
                    .get(field)
                    .expect("served patch property");
                assert_eq!(
                    allows_null(&document, property),
                    expected,
                    "{collection}.{field}"
                );
                if field == "custom_fields" {
                    assert_eq!(resolve_schema(&document, property)["type"], "object");
                }
            }
        }
    }
    Ok(())
}

#[test]
fn cooling_served_membership_inputs_use_the_canonical_optional_non_null_uuid_set() -> TestResult {
    let document = served_document()?;
    for collection in [
        "cooling-sources",
        "cooling-feeds",
        "cooling-intakes",
        "cooling-outflows",
    ] {
        for (method, suffix) in [("post", ""), ("put", "/{id}")] {
            let path = format!("/api/v1/inventory/{collection}{suffix}");
            let request = &document["paths"][&path][method]["requestBody"]["content"]["application/json"]
                ["schema"];
            let request = resolve_schema(&document, request);
            assert!(
                !request["required"]
                    .as_array()
                    .is_some_and(|fields| fields.iter().any(|field| field == "tags")),
                "membership omission must remain optional"
            );
            let membership = resolve_schema(&document, &request["properties"]["tags"]);
            assert_eq!(membership["type"], "array", "{collection} {method}");
            assert!(!allows_null(&document, membership));
            assert_eq!(membership["uniqueItems"], true);
            let item = resolve_schema(&document, &membership["items"]);
            assert_eq!(item["type"], "string");
            assert_eq!(item["format"], "uuid");
            let pattern =
                regex::Regex::new(item["pattern"].as_str().ok_or("canonical UUID pattern")?)?;
            let canonical = Uuid::now_v7().to_string();
            assert!(pattern.is_match(&canonical));
            for invalid in [
                "legacy-tag-name",
                "",
                "not-a-uuid",
                "00000000-0000-7000-8000-00000000100A",
            ] {
                assert!(
                    !pattern.is_match(invalid),
                    "membership items must be canonical UUIDs"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn cooling_published_custom_field_contracts_accept_native_create_and_update_bodies() -> TestResult {
    let document = served_document()?;
    let id = Uuid::now_v7();
    for (kind, create) in [
        (
            "CoolingSource",
            json!({"name":"chiller","type":"chiller","site_id":id}),
        ),
        ("CoolingFeed", json!({"name":"loop","cooling_source_id":id})),
        ("CoolingIntake", json!({"name":"intake","device_id":id})),
        ("CoolingOutflow", json!({"name":"outflow","device_id":id})),
    ] {
        for (suffix, base) in [("Create", create), ("Patch", json!({}))] {
            let schema = json!({
                "$ref": format!("#/components/schemas/{kind}{suffix}"),
                "components": document["components"],
            });
            let validator = jsonschema::validator_for(&schema)?;
            assert!(
                validator.is_valid(&base),
                "{kind}{suffix} must allow omission"
            );
            for fields in [
                json!({}),
                json!({"note":"ok"}),
                json!({"count":2,"enabled":true,"items":["a"],"nested":{"value":null}}),
            ] {
                let mut body = base.clone();
                body["custom_fields"] = fields;
                assert!(
                    validator.is_valid(&body),
                    "{kind}{suffix} must accept custom-field maps"
                );
            }
            let mut null_body = base.clone();
            null_body["custom_fields"] = Value::Null;
            assert_eq!(
                validator.is_valid(&null_body),
                suffix == "Create",
                "{kind}{suffix} must preserve native null semantics"
            );
            for invalid in [json!("not an object"), json!(42), json!([])] {
                let mut body = base.clone();
                body["custom_fields"] = invalid;
                assert!(
                    !validator.is_valid(&body),
                    "{kind}{suffix} must reject non-object values"
                );
            }
        }
    }
    Ok(())
}

struct HttpFixture {
    app: Router,
    cookie: String,
}

impl HttpFixture {
    async fn start() -> TestResult<Self> {
        let app = piping_backend::app::build_router(build_test_app_state().await?);
        let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
            .await
            .ok_or("admin login")?;
        Ok(Self { app, cookie })
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        payload: Option<Value>,
        bearer: Option<&str>,
        expected: StatusCode,
    ) -> TestResult<Value> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        request = if let Some(bearer) = bearer {
            request.header("authorization", format!("Bearer {bearer}"))
        } else {
            request.header("cookie", &self.cookie)
        };
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(
                payload.map(|value| value.to_string()).unwrap_or_default(),
            ))?)
            .await?;
        assert_eq!(response.status(), expected, "{path}");
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok(if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        })
    }

    async fn source(&self, db: &sqlx::PgPool) -> TestResult<Value> {
        let site = seed_site(db).await?;
        self.call(
            Method::POST,
            "/api/v1/inventory/cooling-sources",
            Some(json!({"name":format!("source-{site}"),"type":"chiller","site_id":site})),
            None,
            StatusCode::CREATED,
        )
        .await
    }

    async fn token(&self, scopes: Value) -> TestResult<String> {
        let response = self
            .call(
                Method::POST,
                "/api/v1/user/settings/api-tokens",
                Some(
                    json!({"name":format!("cooling-contract-{}", Uuid::now_v7()),"scopes":scopes}),
                ),
                None,
                StatusCode::CREATED,
            )
            .await?;
        Ok(response["token_value"]
            .as_str()
            .ok_or("token value")?
            .to_owned())
    }
}

async fn seed_site(db: &sqlx::PgPool) -> TestResult<Uuid> {
    let site = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(format!("cooling-contract-{site}")).execute(db).await?;
    Ok(site)
}

async fn seed_device(db: &sqlx::PgPool, site: Uuid) -> TestResult<(Uuid, Uuid)> {
    let device = Uuid::now_v7();
    let device_type: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1")
            .fetch_one(db)
            .await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),$3,$4,'','{}',0,0,0,0,0,0,0,0,0,0")
        .bind(device).bind(format!("cooling-contract-{device}")).bind(device_type).bind(site)
        .execute(db).await?;
    Ok((device, device_type))
}

fn object_path(collection: &str, record: &Value) -> TestResult<String> {
    Ok(format!(
        "{collection}/{}",
        record["id"].as_str().ok_or("record id")?
    ))
}

fn assert_private_blocker(response: &Value, identifiers: &[String]) {
    assert_eq!(response["code"], "CONFLICT");
    assert_eq!(response["reason_code"], "DELETE_BLOCKED");
    assert_eq!(
        response["message"],
        "Cannot delete this record while related objects still exist"
    );
    assert!(
        response.get("details").is_none(),
        "no examples or counts may escape"
    );
    let serialized = response.to_string();
    for identifier in identifiers {
        assert!(!serialized.contains(identifier), "blocker identity escaped");
    }
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; real cooling handlers and storage"]
async fn cooling_units_create_update_omission_and_invalid_choices() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let source = http.source(&db).await?;
    let site = Uuid::parse_str(source["site_id"].as_str().ok_or("site id")?)?;
    let (device, device_type) = seed_device(&db, site).await?;

    // Independent wire values from the pinned NetBox 4.7.1 contract, exercised
    // through mounted handlers, persisted updates and subsequent GETs.
    let diameter_units = ["mm", "cm", "in"];
    let flow_units = ["lpm", "m3ph", "gpm"];
    for (collection, parent, has_diameter, has_flow) in [
        (
            "cooling-feeds",
            json!({"cooling_source_id":source["id"]}),
            false,
            true,
        ),
        ("cooling-intakes", json!({"device_id":device}), true, true),
        ("cooling-outflows", json!({"device_id":device}), true, false),
        (
            "cooling-intake-templates",
            json!({"device_type_id":device_type}),
            true,
            true,
        ),
        (
            "cooling-outflow-templates",
            json!({"device_type_id":device_type}),
            true,
            false,
        ),
    ] {
        let collection = format!("/api/v1/inventory/{collection}");
        for index in 0..3 {
            let mut payload = parent.clone();
            payload["name"] = json!(format!("unit-{}", Uuid::now_v7()));
            if has_diameter {
                payload["type"] = json!("qdc");
                payload["diameter"] = json!(1.25);
                payload["diameter_unit"] = json!(diameter_units[index]);
            }
            if has_flow {
                payload["max_flow"] = json!(1.25);
                payload["max_flow_unit"] = json!(flow_units[index]);
            }
            let created = http
                .call(
                    Method::POST,
                    &collection,
                    Some(payload.clone()),
                    None,
                    StatusCode::CREATED,
                )
                .await?;
            let path = object_path(&collection, &created)?;
            if index == 0 {
                let (reference, value) = parent
                    .as_object()
                    .ok_or("parent object")?
                    .iter()
                    .next()
                    .ok_or("parent reference")?;
                let choice_filter = if has_diameter {
                    "type=qdc"
                } else {
                    "status=active"
                };
                let query = format!(
                    "{collection}?id={}&{reference}={}&{choice_filter}",
                    created["id"].as_str().ok_or("record id")?,
                    value.as_str().ok_or("reference id")?
                );
                let page = http
                    .call(Method::GET, &query, None, None, StatusCode::OK)
                    .await?;
                assert_eq!(page["total"], 1);
                assert_eq!(page["items"][0]["id"], created["id"]);
            }
            let mut update = json!({});
            for (present, number, unit, choices) in [
                (has_diameter, "diameter", "diameter_unit", diameter_units),
                (has_flow, "max_flow", "max_flow_unit", flow_units),
            ] {
                if !present {
                    continue;
                }
                assert_eq!(created[unit], payload[unit]);
                assert_eq!(created[number].as_f64(), Some(1.25));
                update[number] = json!(2.5);
                update[unit] = json!(choices[(index + 1) % 3]);
            }
            let changed = http
                .call(
                    Method::PUT,
                    &path,
                    Some(update.clone()),
                    None,
                    StatusCode::OK,
                )
                .await?;
            let preserved = http
                .call(
                    Method::PUT,
                    &path,
                    Some(json!({"description":"unrelated metadata"})),
                    None,
                    StatusCode::OK,
                )
                .await?;
            let fetched = http
                .call(Method::GET, &path, None, None, StatusCode::OK)
                .await?;
            for (field, expected) in update.as_object().ok_or("update object")? {
                for response in [&changed, &preserved, &fetched] {
                    if expected.is_number() {
                        assert_eq!(response[field].as_f64(), expected.as_f64());
                    } else {
                        assert_eq!(
                            &response[field], expected,
                            "omitted units must survive metadata updates"
                        );
                    }
                }
            }
            for (present, unit) in [(has_diameter, "diameter_unit"), (has_flow, "max_flow_unit")] {
                if !present {
                    continue;
                }
                let mut invalid = json!({});
                invalid[unit] = json!("unsupported");
                http.call(
                    Method::PUT,
                    &path,
                    Some(invalid),
                    None,
                    StatusCode::BAD_REQUEST,
                )
                .await?;
                let mut invalid_create = payload.clone();
                invalid_create["name"] = json!(format!("invalid-{}", Uuid::now_v7()));
                invalid_create[unit] = json!("unsupported");
                http.call(
                    Method::POST,
                    &collection,
                    Some(invalid_create),
                    None,
                    StatusCode::BAD_REQUEST,
                )
                .await?;
            }
            let after_rejection = http
                .call(Method::GET, &path, None, None, StatusCode::OK)
                .await?;
            assert_eq!(
                after_rejection, fetched,
                "invalid choices must not mutate storage"
            );
            if index == 0 {
                for field in [
                    "name",
                    "description",
                    if has_diameter { "label" } else { "status" },
                ] {
                    let mut invalid = json!({});
                    invalid[field] = Value::Null;
                    http.call(
                        Method::PUT,
                        &path,
                        Some(invalid),
                        None,
                        StatusCode::BAD_REQUEST,
                    )
                    .await?;
                }
                let omitted = http
                    .call(Method::PUT, &path, Some(json!({})), None, StatusCode::OK)
                    .await?;
                assert_eq!(omitted["name"], created["name"]);
                let mut clear = json!({});
                for (field, _) in update.as_object().ok_or("update object")? {
                    clear[field] = Value::Null;
                }
                let cleared = http
                    .call(
                        Method::PUT,
                        &path,
                        Some(clear.clone()),
                        None,
                        StatusCode::OK,
                    )
                    .await?;
                let fetched = http
                    .call(Method::GET, &path, None, None, StatusCode::OK)
                    .await?;
                for (field, _) in clear.as_object().ok_or("clear object")? {
                    assert!(cleared[field].is_null());
                    assert!(
                        fetched[field].is_null(),
                        "measurement and unit clears must persist"
                    );
                }
            }
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; per-kind query extraction and empty unsupported keys"]
async fn cooling_list_handlers_reject_unsupported_keys_even_when_empty() -> TestResult {
    let http = HttpFixture::start().await?;
    for (collection, unsupported) in [
        ("cooling-sources", vec!["device_id", "cooling_source_id"]),
        ("cooling-feeds", vec!["site_id", "type"]),
        (
            "cooling-intakes",
            vec!["site_id", "status", "device_type_id"],
        ),
        (
            "cooling-outflows",
            vec!["site_id", "status", "cooling_outflow_id"],
        ),
        (
            "cooling-intake-templates",
            vec!["site_id", "status", "device_id"],
        ),
        (
            "cooling-outflow-templates",
            vec!["site_id", "status", "cooling_outflow_id"],
        ),
    ] {
        for key in unsupported.into_iter().chain(["unexpected_filter"]) {
            for value in ["", "unsupported"] {
                let path = format!("/api/v1/inventory/{collection}?{key}={value}");
                http.call(Method::GET, &path, None, None, StatusCode::BAD_REQUEST)
                    .await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; required scalar omission and genuine nullable FK clears"]
async fn cooling_source_patch_preserves_omission_rejects_scalar_null_and_clears_nullable_values()
-> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let source = http.source(&db).await?;
    let path = object_path("/api/v1/inventory/cooling-sources", &source)?;
    let location = http.call(Method::POST, "/api/v1/inventory/locations", Some(json!({"name":format!("cooling-contract-{}", Uuid::now_v7()), "site_id":source["site_id"]})), None, StatusCode::CREATED).await?;
    let set = http.call(Method::PUT, &path, Some(json!({"location_id":location["id"],"fluid_type":"water","cooling_capacity":10.25,"comments":"retain"})), None, StatusCode::OK).await?;
    let omitted = http
        .call(
            Method::PUT,
            &path,
            Some(json!({"description":"metadata"})),
            None,
            StatusCode::OK,
        )
        .await?;
    for field in [
        "name",
        "site_id",
        "type",
        "status",
        "location_id",
        "fluid_type",
        "cooling_capacity",
        "comments",
    ] {
        assert_eq!(omitted[field], set[field], "omission preserves {field}");
    }
    let list_path = format!(
        "/api/v1/inventory/cooling-sources?id={}&site_id={}&location_id={}&type=chiller&status=active",
        source["id"].as_str().ok_or("source id")?,
        source["site_id"].as_str().ok_or("site id")?,
        location["id"].as_str().ok_or("location id")?
    );
    let page = http
        .call(Method::GET, &list_path, None, None, StatusCode::OK)
        .await?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], source["id"]);
    for field in [
        "name",
        "site_id",
        "type",
        "status",
        "description",
        "comments",
        "tags",
        "custom_fields",
    ] {
        let mut invalid = json!({});
        invalid[field] = Value::Null;
        http.call(
            Method::PUT,
            &path,
            Some(invalid),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let unchanged = http
        .call(Method::GET, &path, None, None, StatusCode::OK)
        .await?;
    for field in [
        "name",
        "site_id",
        "type",
        "status",
        "location_id",
        "fluid_type",
        "cooling_capacity",
        "comments",
    ] {
        assert_eq!(
            unchanged[field], set[field],
            "rejected null must not change {field}"
        );
    }
    http.call(
        Method::PUT,
        &path,
        Some(json!({"location_id":null,"fluid_type":null,"cooling_capacity":null})),
        None,
        StatusCode::OK,
    )
    .await?;
    let cleared = http
        .call(Method::GET, &path, None, None, StatusCode::OK)
        .await?;
    for field in ["location_id", "fluid_type", "cooling_capacity"] {
        assert!(
            cleared[field].is_null(),
            "nullable {field} must clear in storage"
        );
    }
    assert_eq!(cleared["comments"], "retain");
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; served capabilities and native definition validation"]
async fn cooling_capabilities_match_native_custom_field_definition_creation() -> TestResult {
    let http = HttpFixture::start().await?;
    let types = http
        .call(
            Method::GET,
            "/api/v1/inventory/content-types",
            None,
            None,
            StatusCode::OK,
        )
        .await?;
    for (model, supported) in [
        ("coolingsource", true),
        ("coolingfeed", true),
        ("coolingintake", true),
        ("coolingoutflow", true),
        ("coolingintaketemplate", false),
        ("coolingoutflowtemplate", false),
        ("modulebaytemplate", false),
    ] {
        let token = format!("dcim.{model}");
        let content_type = types
            .as_array()
            .ok_or("content types")?
            .iter()
            .find(|item| item["token"] == token)
            .ok_or("native content type")?;
        assert_eq!(
            content_type["supported_for_custom_fields"], supported,
            "{model}"
        );
        assert_eq!(
            content_type["capabilities"]["custom_fields"], supported,
            "{model}"
        );
        let definition = http.call(
            Method::POST,
            "/api/v1/custom-fields",
            Some(json!({"name":format!("cooling_contract_{}", Uuid::now_v7().simple()),"label":"Cooling contract","field_type":"text","is_secret":true,"object_types":[content_type["id"]]})),
            None,
            if supported { StatusCode::CREATED } else { StatusCode::BAD_REQUEST },
        ).await?;
        if supported {
            assert!(
                definition["id"].as_str().is_some(),
                "native definition must be created"
            );
            assert_eq!(definition["is_secret"], true);
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; object PAT blocker privacy and complete authorization"]
async fn cooling_delete_projects_only_completely_readable_blockers() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let source = http.source(&db).await?;
    let source_path = object_path("/api/v1/inventory/cooling-sources", &source)?;
    let feed_name = format!("private-feed-{}", Uuid::now_v7());
    let feed = http
        .call(
            Method::POST,
            "/api/v1/inventory/cooling-feeds",
            Some(json!({"name":feed_name,"cooling_source_id":source["id"]})),
            None,
            StatusCode::CREATED,
        )
        .await?;
    let feed_path = object_path("/api/v1/inventory/cooling-feeds", &feed)?;
    let delete_scope = json!({"action":"delete","resource_type":"inventory","resource_pattern":format!("inventory:{}",source["id"].as_str().ok_or("source id")?)});
    let denied = http.token(json!([delete_scope])).await?;
    http.call(
        Method::GET,
        &feed_path,
        None,
        Some(&denied),
        StatusCode::FORBIDDEN,
    )
    .await?;
    let blocked = http
        .call(
            Method::DELETE,
            &source_path,
            None,
            Some(&denied),
            StatusCode::CONFLICT,
        )
        .await?;
    assert_private_blocker(
        &blocked,
        &[
            feed_name.clone(),
            feed["id"].as_str().ok_or("feed id")?.into(),
        ],
    );

    let authorized = http.token(json!([
        delete_scope,
        {"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{}",feed["id"].as_str().ok_or("feed id")?)}
    ])).await?;
    http.call(
        Method::GET,
        &feed_path,
        None,
        Some(&authorized),
        StatusCode::OK,
    )
    .await?;
    let complete = http
        .call(
            Method::DELETE,
            &source_path,
            None,
            Some(&authorized),
            StatusCode::CONFLICT,
        )
        .await?;
    assert_eq!(complete["code"], "DELETE_BLOCKED");
    assert_eq!(complete["details"]["total_count"], 1);
    let group = &complete["details"]["blockers"][0];
    assert_eq!(group["count"], 1);
    assert_eq!(group["examples"][0]["id"], feed["id"]);
    assert!(
        group["examples"][0]["label"]
            .as_str()
            .ok_or("blocker label")?
            .contains(&feed_name)
    );

    // More than the guard's three examples cannot establish READ for every
    // counted object, even for an otherwise fully authorized session.
    for index in 0..3 {
        http.call(
            Method::POST,
            "/api/v1/inventory/cooling-feeds",
            Some(json!({"name":format!("extra-{index}"),"cooling_source_id":source["id"]})),
            None,
            StatusCode::CREATED,
        )
        .await?;
    }
    let incomplete = http
        .call(
            Method::DELETE,
            &source_path,
            None,
            None,
            StatusCode::CONFLICT,
        )
        .await?;
    assert_private_blocker(
        &incomplete,
        &[feed_name, feed["id"].as_str().ok_or("feed id")?.into()],
    );
    // Integrity still holds after every refused deletion.
    http.call(Method::GET, &source_path, None, None, StatusCode::OK)
        .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_coolingfeed WHERE cooling_source_id=$1")
            .bind(Uuid::parse_str(source["id"].as_str().ok_or("source id")?)?)
            .fetch_one(&db)
            .await?;
    assert_eq!(count, 4);
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; real hardware DELETE and unreadable installed module"]
async fn module_type_delete_does_not_identify_an_unreadable_installed_module() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let site = seed_site(&db).await?;
    let (device, _) = seed_device(&db, site).await?;
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .fetch_one(&db)
            .await?;
    let category = http
        .call(
            Method::POST,
            "/api/v1/inventory/module-bay-types",
            Some(json!({"name":format!("private-category-{}", Uuid::now_v7())})),
            None,
            StatusCode::CREATED,
        )
        .await?;
    let module_type = http.call(
        Method::POST, "/api/v1/inventory/module-types",
        Some(json!({"manufacturer_id":manufacturer,"model":format!("private-type-{}", Uuid::now_v7()),"module_bay_types":[category["id"]]})),
        None, StatusCode::CREATED,
    ).await?;
    let bay_name = format!("private-bay-{}", Uuid::now_v7());
    let bay = http
        .call(
            Method::POST,
            "/api/v1/inventory/module-bays",
            Some(json!({"device_id":device,"name":bay_name,"module_bay_types":[category["id"]]})),
            None,
            StatusCode::CREATED,
        )
        .await?;
    let module = http.call(
        Method::POST, "/api/v1/inventory/modules",
        Some(json!({"device_id":device,"module_bay_id":bay["id"],"module_type_id":module_type["id"]})),
        None, StatusCode::CREATED,
    ).await?;
    let type_path = object_path("/api/v1/inventory/module-types", &module_type)?;
    let module_path = object_path("/api/v1/inventory/modules", &module)?;
    let bearer = http.token(json!([
        {"action":"delete","resource_type":"inventory","resource_pattern":format!("inventory:{}", module_type["id"].as_str().ok_or("type id")?)}
    ])).await?;
    http.call(
        Method::GET,
        &module_path,
        None,
        Some(&bearer),
        StatusCode::FORBIDDEN,
    )
    .await?;
    let blocked = http
        .call(
            Method::DELETE,
            &type_path,
            None,
            Some(&bearer),
            StatusCode::CONFLICT,
        )
        .await?;
    assert_private_blocker(
        &blocked,
        &[module["id"].as_str().ok_or("module id")?.into(), bay_name],
    );
    // The same complete native scan still returns identifying blockers to an
    // authorized session; the failed PAT deletion must preserve the installation.
    let authorized = http
        .call(Method::DELETE, &type_path, None, None, StatusCode::CONFLICT)
        .await?;
    assert_eq!(authorized["details"]["total_count"], 1);
    assert_eq!(
        authorized["details"]["blockers"][0]["examples"][0]["id"],
        module["id"]
    );
    http.call(Method::GET, &module_path, None, None, StatusCode::OK)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned disposable PostgreSQL and Redis fixtures with DDL authority; unknown blocker metadata"]
async fn cooling_delete_unknown_blocker_domain_and_missing_identity_are_private() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    for (prefix, has_id) in [("contract_unknown", true), ("dcim_contract", false)] {
        let source = http.source(&db).await?;
        let path = object_path("/api/v1/inventory/cooling-sources", &source)?;
        // UUID-generated identifiers are bounded fixture-owned SQL identifiers,
        // not request input. The real FK guard discovers each table from pg_catalog.
        let table = format!("{prefix}_{}", Uuid::now_v7().simple());
        let id_column = if has_id { "id uuid PRIMARY KEY," } else { "" };
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE TABLE ");
        query.push(&table).push(" (").push(id_column).push(" name text NOT NULL, source_id uuid NOT NULL REFERENCES dcim_coolingsource(id) ON DELETE RESTRICT)");
        query.build().execute(&db).await?;
        let id = Uuid::now_v7();
        let name = format!("undisclosable-{id}");
        if has_id {
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("INSERT INTO ");
            query
                .push(&table)
                .push(" (id,name,source_id) VALUES ($1,$2,$3)");
            query
                .build()
                .bind(id)
                .bind(&name)
                .bind(Uuid::parse_str(source["id"].as_str().ok_or("source id")?)?)
                .execute(&db)
                .await?;
        } else {
            let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("INSERT INTO ");
            query.push(&table).push(" (name,source_id) VALUES ($1,$2)");
            query
                .build()
                .bind(&name)
                .bind(Uuid::parse_str(source["id"].as_str().ok_or("source id")?)?)
                .execute(&db)
                .await?;
        }
        let blocked = http
            .call(Method::DELETE, &path, None, None, StatusCode::CONFLICT)
            .await?;
        assert_private_blocker(&blocked, &[name, id.to_string(), table]);
        http.call(Method::GET, &path, None, None, StatusCode::OK)
            .await?;
    }
    Ok(())
}
