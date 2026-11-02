//! Native DCIM component template REST contracts (PWA-80, source-prepared).
//!
//! Main owns compilation and execution. Every case is `#[ignore]`, needs an
//! owned PostgreSQL and Redis fixture, and assumes the documented backend
//! suite command (`--test-threads=1`), because each case rebuilds the shared
//! admin bootstrap.
//!
//! Fields, scopes and choices come from NetBox 4.7.1
//! `netbox/dcim/models/device_component_templates.py` and `dcim/choices.py`;
//! the persistence shape comes from `migrations/0001_init.sql`. Request bodies
//! use the native snake_case columns with `*_id` parent references. The body
//! key `type` is pinned to the served record field; a `type_` request alias
//! would be a different contract, not a supported alternative.
//!
//! Non-admin policy coverage is resource-scoped: `constraint_attribute_keys`
//! declares no object-attribute keys for `ResourceType::Inventory`, so an
//! `attributes` policy constraint on a component template cannot be written or
//! evaluated today. Parent and reference authorization is therefore pinned on
//! persisted ids (the row, its parent, and the referenced port), which is what
//! a handler can actually feed into policy evaluation.

mod common;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    build_test_redis, login_and_get_cookie, unique_suffix,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    ConsolePort,
    ConsoleServerPort,
    PowerPort,
    PowerOutlet,
    Interface,
    FrontPort,
    RearPort,
    DeviceBay,
    InventoryItem,
}

impl Kind {
    const ALL: [Self; 9] = [
        Self::ConsolePort,
        Self::ConsoleServerPort,
        Self::PowerPort,
        Self::PowerOutlet,
        Self::Interface,
        Self::FrontPort,
        Self::RearPort,
        Self::DeviceBay,
        Self::InventoryItem,
    ];

    fn slug(self) -> &'static str {
        match self {
            Self::ConsolePort => "console-port",
            Self::ConsoleServerPort => "console-server-port",
            Self::PowerPort => "power-port",
            Self::PowerOutlet => "power-outlet",
            Self::Interface => "interface",
            Self::FrontPort => "front-port",
            Self::RearPort => "rear-port",
            Self::DeviceBay => "device-bay",
            Self::InventoryItem => "inventory-item",
        }
    }

    fn collection(self) -> String {
        format!("/api/v1/inventory/{}-templates", self.slug())
    }

    fn table(self) -> &'static str {
        match self {
            Self::ConsolePort => "dcim_consoleporttemplate",
            Self::ConsoleServerPort => "dcim_consoleserverporttemplate",
            Self::PowerPort => "dcim_powerporttemplate",
            Self::PowerOutlet => "dcim_poweroutlettemplate",
            Self::Interface => "dcim_interfacetemplate",
            Self::FrontPort => "dcim_frontporttemplate",
            Self::RearPort => "dcim_rearporttemplate",
            Self::DeviceBay => "dcim_devicebaytemplate",
            Self::InventoryItem => "dcim_inventoryitemtemplate",
        }
    }

    /// Device bay and inventory item templates exist only under a device type.
    fn modular(self) -> bool {
        !matches!(self, Self::DeviceBay | Self::InventoryItem)
    }

    /// A device bay template needs a device type whose subdevice role is
    /// parent; every other template accepts any device type.
    fn default_parent(self, seed: &Seed) -> Parent {
        match self {
            Self::DeviceBay => Parent::Device(seed.parent_device_type),
            _ => Parent::Device(seed.device_type),
        }
    }

    /// Fields the upstream contract allows to be empty, so an explicit `null`
    /// is a clear rather than a validation error.
    fn nullable_fields(self) -> &'static [&'static str] {
        match self {
            Self::ConsolePort | Self::ConsoleServerPort => &["type"],
            Self::PowerPort => &["type", "maximum_draw", "allocated_draw"],
            Self::PowerOutlet => &["type", "feed_leg", "power_port_id"],
            Self::Interface => &["bridge_id", "poe_mode", "poe_type", "rf_role"],
            Self::FrontPort | Self::RearPort | Self::DeviceBay => &[],
            Self::InventoryItem => &[
                "parent_id",
                "manufacturer_id",
                "role_id",
                "component_type_id",
                "component_id",
            ],
        }
    }
}

#[derive(Clone, Copy)]
enum Parent {
    Device(Uuid),
    Module(Uuid),
}

impl Parent {
    fn scope(self) -> (&'static str, Uuid) {
        match self {
            Self::Device(id) => ("device_type_id", id),
            Self::Module(id) => ("module_type_id", id),
        }
    }
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    Anonymous,
    Cookie(&'a str),
    Bearer(&'a str),
}

struct Seed {
    manufacturer: Uuid,
    device_type: Uuid,
    sibling_device_type: Uuid,
    parent_device_type: Uuid,
    child_device_type: Uuid,
    module_type: Uuid,
    item_role: Uuid,
}

struct Fixture {
    app: Router,
    app_db: PgPool,
    cookie: String,
    db: PgPool,
    seed: Seed,
}

impl Fixture {
    async fn start() -> TestResult<Self> {
        let state = build_test_app_state().await?;
        let app_db = state.db_pool.clone();
        let app = piping_backend::app::build_router(state);
        let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
            .await
            .ok_or("admin login failed")?;
        let db = build_test_db_pool().await?;
        let seed = seed_parents(&db).await?;
        Ok(Self {
            app,
            app_db,
            cookie,
            db,
            seed,
        })
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        payload: Option<Value>,
        credential: Credential<'_>,
    ) -> TestResult<(StatusCode, Value)> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        builder = match credential {
            Credential::Anonymous => builder,
            Credential::Cookie(cookie) => builder.header("cookie", cookie),
            Credential::Bearer(token) => builder.header("authorization", format!("Bearer {token}")),
        };
        let request = builder.body(Body::from(
            payload.map(|value| value.to_string()).unwrap_or_default(),
        ))?;
        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        };
        Ok((status, value))
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        payload: Option<Value>,
        credential: Credential<'_>,
        expected: StatusCode,
    ) -> TestResult<Value> {
        let (status, value) = self.request(method, path, payload, credential).await?;
        assert_eq!(
            status, expected,
            "{method} {path} answered {status}: {value}"
        );
        if expected == StatusCode::NO_CONTENT {
            assert!(
                value.is_null(),
                "{method} {path} must answer an empty 204 body"
            );
        }
        Ok(value)
    }

    async fn admin(
        &self,
        method: &str,
        path: &str,
        payload: Option<Value>,
        expected: StatusCode,
    ) -> TestResult<Value> {
        self.call(
            method,
            path,
            payload,
            Credential::Cookie(self.cookie.as_str()),
            expected,
        )
        .await
    }

    async fn create(&self, kind: Kind, parent: Parent, name: &str) -> TestResult<Value> {
        let body = kind.create_body(parent, name, &self.seed);
        self.admin("POST", &kind.collection(), Some(body), StatusCode::CREATED)
            .await
    }

    async fn token(&self, scopes: Value) -> TestResult<String> {
        let created = self
            .admin(
                "POST",
                "/api/v1/user/settings/api-tokens",
                Some(json!({
                    "name": format!("pwa80-component-template-{}", Uuid::now_v7()),
                    "scopes": scopes,
                })),
                StatusCode::CREATED,
            )
            .await?;
        Ok(created["token_value"]
            .as_str()
            .ok_or("token value")?
            .to_owned())
    }
}

fn record_id(record: &Value) -> TestResult<String> {
    Ok(record["id"].as_str().ok_or("record id")?.to_owned())
}

fn item_path(kind: Kind, record: &Value) -> TestResult<String> {
    Ok(format!("{}/{}", kind.collection(), record_id(record)?))
}

fn assert_blocked(response: &Value, expected_ids: &[String]) -> TestResult {
    assert_eq!(response["code"], "DELETE_BLOCKED");
    assert_eq!(response["reason_code"], "DELETE_BLOCKED");
    let total = response["details"]["total_count"]
        .as_i64()
        .ok_or("blocker total")?;
    assert!(total > 0, "a blocked delete must count its references");
    let examples: Vec<&Value> = response["details"]["blockers"]
        .as_array()
        .ok_or("blockers")?
        .iter()
        .flat_map(|group| group["examples"].as_array().into_iter().flatten())
        .collect();
    assert!(
        !examples.is_empty(),
        "a blocked delete must name its references"
    );
    for id in expected_ids {
        assert!(
            examples.iter().any(|example| example["id"] == id.as_str()),
            "referencing record {id} must be named as a blocker"
        );
    }
    Ok(())
}

async fn seed_device_type(
    db: &PgPool,
    manufacturer: Uuid,
    model: &str,
    subdevice_role: Option<&str>,
) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dcim_devicetype (id,custom_field_data,model,slug,part_number,u_height,is_full_depth,subdevice_role,front_image,rear_image,comments,manufacturer_id,description,console_port_template_count,console_server_port_template_count,power_port_template_count,power_outlet_template_count,interface_template_count,front_port_template_count,rear_port_template_count,device_bay_template_count,module_bay_template_count,inventory_item_template_count,exclude_from_utilization,device_count) VALUES ($1,'{}',$2,$2,'',1.0,false,$3,'','','',$4,'',0,0,0,0,0,0,0,0,0,0,false,0)",
    )
    .bind(id)
    .bind(model)
    .bind(subdevice_role)
    .bind(manufacturer)
    .execute(db)
    .await?;
    Ok(id)
}

async fn seed_parents(db: &PgPool) -> TestResult<Seed> {
    let suffix = unique_suffix();
    let manufacturer = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_manufacturer (id,custom_field_data,name,slug,description,comments) VALUES ($1,'{}',$2,$2,'','')")
        .bind(manufacturer)
        .bind(format!("pwa80-mfr-{suffix}"))
        .execute(db)
        .await?;
    let device_type =
        seed_device_type(db, manufacturer, &format!("pwa80-dt-{suffix}"), None).await?;
    // Second parent-role type: the positive "other scope" row for every kind,
    // including device bays. Ordinary and child roles stay explicit below for
    // the negative role controls.
    let sibling_device_type = seed_device_type(
        db,
        manufacturer,
        &format!("pwa80-dt-b-{suffix}"),
        Some("parent"),
    )
    .await?;
    let parent_device_type = seed_device_type(
        db,
        manufacturer,
        &format!("pwa80-dt-p-{suffix}"),
        Some("parent"),
    )
    .await?;
    let child_device_type = seed_device_type(
        db,
        manufacturer,
        &format!("pwa80-dt-c-{suffix}"),
        Some("child"),
    )
    .await?;
    let module_type = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_moduletype (id,custom_field_data,model,part_number,comments,manufacturer_id,description,module_count) VALUES ($1,'{}',$2,'','',$3,'',0)")
        .bind(module_type)
        .bind(format!("pwa80-mt-{suffix}"))
        .bind(manufacturer)
        .execute(db)
        .await?;
    let item_role = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_inventoryitemrole (id,custom_field_data,name,slug,color,description,comments) VALUES ($1,'{}',$2,$2,'9e9e9e','','')")
        .bind(item_role)
        .bind(format!("pwa80-role-{suffix}"))
        .execute(db)
        .await?;
    Ok(Seed {
        manufacturer,
        device_type,
        sibling_device_type,
        parent_device_type,
        child_device_type,
        module_type,
        item_role,
    })
}

impl Kind {
    /// A valid body that supplies every writable field the local schema keeps,
    /// using upstream values rather than invented ones.
    fn create_body(self, parent: Parent, name: &str, seed: &Seed) -> Value {
        let (scope_field, parent_id) = parent.scope();
        let mut body = json!({ "name": name });
        body[scope_field] = json!(parent_id);
        match self {
            Self::ConsolePort => {
                body["label"] = json!("console port label");
                body["description"] = json!("console port template");
                body["type"] = json!("rj-45");
            }
            Self::ConsoleServerPort => {
                body["label"] = json!("console server port label");
                body["description"] = json!("console server port template");
                body["type"] = json!("rj-45");
            }
            Self::PowerPort => {
                body["label"] = json!("power port label");
                body["description"] = json!("power port template");
                body["type"] = json!("iec-60320-c14");
                body["maximum_draw"] = json!(500);
                body["allocated_draw"] = json!(250);
            }
            Self::PowerOutlet => {
                body["label"] = json!("power outlet label");
                body["description"] = json!("power outlet template");
                body["type"] = json!("iec-60320-c13");
                body["color"] = json!("1a2b3c");
                body["feed_leg"] = json!("A");
            }
            Self::Interface => {
                body["label"] = json!("interface label");
                body["description"] = json!("interface template");
                body["type"] = json!("1000base-t");
                body["enabled"] = json!(true);
                body["mgmt_only"] = json!(true);
                body["poe_mode"] = json!("pd");
                body["poe_type"] = json!("type1-ieee802.3af");
                body["rf_role"] = json!("ap");
            }
            Self::FrontPort => {
                body["label"] = json!("front port label");
                body["description"] = json!("front port template");
                body["type"] = json!("8p8c");
                body["color"] = json!("aabbcc");
                body["positions"] = json!(4);
            }
            Self::RearPort => {
                body["label"] = json!("rear port label");
                body["description"] = json!("rear port template");
                body["type"] = json!("8p8c");
                body["color"] = json!("aabbcc");
                body["positions"] = json!(4);
            }
            Self::DeviceBay => {
                body["label"] = json!("device bay label");
                body["description"] = json!("device bay template");
            }
            Self::InventoryItem => {
                body["label"] = json!("inventory item label");
                body["description"] = json!("inventory item template");
                body["part_id"] = json!("PN-1");
                body["manufacturer_id"] = json!(seed.manufacturer);
                body["role_id"] = json!(seed.item_role);
            }
        }
        body
    }
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template routes"]
async fn component_template_routes_deny_anonymous_callers() -> TestResult {
    let f = Fixture::start().await?;
    let missing = Uuid::now_v7();
    for kind in Kind::ALL {
        let collection = kind.collection();
        f.call(
            "GET",
            &collection,
            None,
            Credential::Anonymous,
            StatusCode::FORBIDDEN,
        )
        .await?;
        let body = json!({"name":"anonymous","device_type_id":f.seed.device_type});
        f.call(
            "POST",
            &collection,
            Some(body),
            Credential::Anonymous,
            StatusCode::UNAUTHORIZED,
        )
        .await?;
        let path = format!("{collection}/{missing}");
        f.call(
            "GET",
            &path,
            None,
            Credential::Anonymous,
            StatusCode::FORBIDDEN,
        )
        .await?;
        f.call(
            "PUT",
            &path,
            Some(json!({"label":"anonymous"})),
            Credential::Anonymous,
            StatusCode::UNAUTHORIZED,
        )
        .await?;
        f.call(
            "DELETE",
            &path,
            None,
            Credential::Anonymous,
            StatusCode::UNAUTHORIZED,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template storage"]
async fn component_template_crud_persists_representative_fields() -> TestResult {
    let f = Fixture::start().await?;
    for kind in Kind::ALL {
        let mut scopes = vec![kind.default_parent(&f.seed)];
        if kind.modular() {
            scopes.push(Parent::Module(f.seed.module_type));
        }
        for (index, parent) in scopes.iter().copied().enumerate() {
            let name = format!("crud-{}-{index}-{}", kind.slug(), unique_suffix());
            let body = kind.create_body(parent, &name, &f.seed);
            let created = f
                .admin(
                    "POST",
                    &kind.collection(),
                    Some(body.clone()),
                    StatusCode::CREATED,
                )
                .await?;
            for (field, value) in body.as_object().ok_or("create body")? {
                assert_eq!(
                    &created[field.as_str()],
                    value,
                    "{} must echo {field}",
                    kind.slug()
                );
            }
            let path = item_path(kind, &created)?;
            let fetched = f.admin("GET", &path, None, StatusCode::OK).await?;
            assert_eq!(
                fetched,
                created,
                "{} create must survive a read",
                kind.slug()
            );

            // An unrelated metadata edit preserves every omitted hardware field.
            let updated = f
                .admin(
                    "PUT",
                    &path,
                    Some(json!({"description": format!("updated {}", kind.slug())})),
                    StatusCode::OK,
                )
                .await?;
            assert_eq!(updated["description"], format!("updated {}", kind.slug()));
            for (field, value) in body.as_object().ok_or("create body")? {
                if field.as_str() != "description" {
                    assert_eq!(
                        &updated[field.as_str()],
                        value,
                        "{} update must preserve {field}",
                        kind.slug()
                    );
                }
            }

            // Nullable fields clear with an explicit null and stay cleared.
            let nullable: Vec<&str> = kind
                .nullable_fields()
                .iter()
                .copied()
                .filter(|field| body.get(*field).is_some_and(|value| !value.is_null()))
                .collect();
            if !nullable.is_empty() {
                let mut clear = serde_json::Map::new();
                for field in &nullable {
                    clear.insert((*field).to_owned(), Value::Null);
                }
                let cleared = f
                    .admin("PUT", &path, Some(Value::Object(clear)), StatusCode::OK)
                    .await?;
                let replayed = f.admin("GET", &path, None, StatusCode::OK).await?;
                for field in nullable {
                    assert!(
                        cleared[field].is_null(),
                        "{} must clear {field}",
                        kind.slug()
                    );
                    assert!(
                        replayed[field].is_null(),
                        "{} must persist the clear of {field}",
                        kind.slug()
                    );
                }
            }

            f.admin("DELETE", &path, None, StatusCode::NO_CONTENT)
                .await?;
            f.admin("GET", &path, None, StatusCode::NOT_FOUND).await?;
            f.admin("DELETE", &path, None, StatusCode::NOT_FOUND)
                .await?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; device bay template enabled contract"]
async fn device_bay_template_enabled_preserves_values_and_rejects_invalid_writes() -> TestResult {
    let f = Fixture::start().await?;
    let kind = Kind::DeviceBay;
    let collection = kind.collection();
    let name = format!("enabled-bay-{}", unique_suffix());
    let base = kind.create_body(kind.default_parent(&f.seed), &name, &f.seed);
    for invalid in [Value::Null, json!("false"), json!(0)] {
        let mut body = base.clone();
        body["enabled"] = invalid;
        f.admin("POST", &collection, Some(body), StatusCode::BAD_REQUEST)
            .await?;
    }
    let created = f
        .admin("POST", &collection, Some(base), StatusCode::CREATED)
        .await?;
    assert_eq!(created["enabled"], true);
    let path = item_path(kind, &created)?;
    for enabled in [false, true, false] {
        let updated = f
            .admin(
                "PUT",
                &path,
                Some(json!({"enabled": enabled})),
                StatusCode::OK,
            )
            .await?;
        assert_eq!(updated["enabled"], enabled);
        let preserved = f
            .admin(
                "PUT",
                &path,
                Some(json!({"description":"preserved"})),
                StatusCode::OK,
            )
            .await?;
        assert_eq!(preserved["enabled"], enabled);
        let stable = f.admin("GET", &path, None, StatusCode::OK).await?;
        for invalid in [Value::Null, json!("false"), json!(0)] {
            f.admin(
                "PUT",
                &path,
                Some(json!({"enabled":invalid,"description":"must roll back"})),
                StatusCode::BAD_REQUEST,
            )
            .await?;
            assert_eq!(f.admin("GET", &path, None, StatusCode::OK).await?, stable);
        }
    }
    let mut disabled = kind.create_body(
        kind.default_parent(&f.seed),
        &format!("disabled-bay-{}", unique_suffix()),
        &f.seed,
    );
    disabled["enabled"] = json!(false);
    let created_disabled = f
        .admin("POST", &collection, Some(disabled), StatusCode::CREATED)
        .await?;
    assert_eq!(created_disabled["enabled"], false);
    let mut enabled = kind.create_body(
        kind.default_parent(&f.seed),
        &format!("explicit-enabled-bay-{}", unique_suffix()),
        &f.seed,
    );
    enabled["enabled"] = json!(true);
    let created_enabled = f
        .admin("POST", &collection, Some(enabled), StatusCode::CREATED)
        .await?;
    assert_eq!(created_enabled["enabled"], true);
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template defaults"]
async fn component_template_create_defaults_and_required_type() -> TestResult {
    let f = Fixture::start().await?;
    for kind in Kind::ALL {
        let name = format!("minimal-{}-{}", kind.slug(), unique_suffix());
        let (scope_field, parent_id) = kind.default_parent(&f.seed).scope();
        let mut body = json!({"name": name});
        body[scope_field] = json!(parent_id);
        match kind {
            Kind::Interface => body["type"] = json!("1000base-t"),
            Kind::FrontPort | Kind::RearPort => body["type"] = json!("8p8c"),
            Kind::InventoryItem => body["part_id"] = json!(""),
            _ => {}
        }
        let created = f
            .admin("POST", &kind.collection(), Some(body), StatusCode::CREATED)
            .await?;
        assert_eq!(
            created["label"],
            "",
            "{} label defaults to empty",
            kind.slug()
        );
        assert_eq!(
            created["description"],
            "",
            "{} description defaults to empty",
            kind.slug()
        );
        match kind {
            Kind::ConsolePort | Kind::ConsoleServerPort | Kind::PowerPort | Kind::PowerOutlet => {
                assert!(
                    created["type"].is_null(),
                    "{} type defaults to null",
                    kind.slug()
                );
            }
            Kind::FrontPort | Kind::RearPort => {
                assert_eq!(
                    created["positions"],
                    1,
                    "{} positions default to 1",
                    kind.slug()
                );
            }
            _ => {}
        }
    }

    // Upstream requires a port type for these templates, so omission and an
    // explicit null are both refused rather than stored as an empty string.
    for kind in [Kind::Interface, Kind::FrontPort, Kind::RearPort] {
        let name = format!("missing-type-{}", unique_suffix());
        let (scope_field, parent_id) = kind.default_parent(&f.seed).scope();
        let mut body = json!({"name": name});
        body[scope_field] = json!(parent_id);
        f.admin(
            "POST",
            &kind.collection(),
            Some(body),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template validation"]
async fn component_template_bodies_reject_unknown_and_persistence_only_fields() -> TestResult {
    let f = Fixture::start().await?;
    for kind in Kind::ALL {
        let device = kind.default_parent(&f.seed);
        let name = format!("strict-{}-{}", kind.slug(), unique_suffix());
        let base = kind.create_body(device, &name, &f.seed);
        // Tags, custom fields, comments and persistence columns are not public
        // component template fields.
        for field in ["tags", "custom_fields", "comments", "custom_field_data"] {
            let mut body = base.clone();
            body[field] = json!("rejected");
            f.admin(
                "POST",
                &kind.collection(),
                Some(body),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
        let created = f
            .admin("POST", &kind.collection(), Some(base), StatusCode::CREATED)
            .await?;
        let path = item_path(kind, &created)?;
        let stable = f.admin("GET", &path, None, StatusCode::OK).await?;
        for field in ["status", "lft", "tree_id", "custom_field_data"] {
            let mut body = json!({});
            body[field] = json!(1);
            f.admin("PUT", &path, Some(body), StatusCode::BAD_REQUEST)
                .await?;
        }
        // Required scalars cannot be replaced with null.
        for field in ["name", "label", "description"] {
            let mut body = json!({});
            body[field] = Value::Null;
            f.admin("PUT", &path, Some(body), StatusCode::BAD_REQUEST)
                .await?;
        }
        for (field, value) in [
            ("name", json!("   ")),
            ("name", json!("x".repeat(65))),
            ("label", json!("x".repeat(65))),
            ("description", json!("x".repeat(201))),
        ] {
            let mut body = json!({});
            body[field] = value;
            f.admin("PUT", &path, Some(body), StatusCode::BAD_REQUEST)
                .await?;
        }
        let unchanged = f.admin("GET", &path, None, StatusCode::OK).await?;
        assert_eq!(
            unchanged,
            stable,
            "{} rejected updates must not mutate storage",
            kind.slug()
        );
        // An empty string is a value, not an omitted field.
        let emptied = f
            .admin(
                "PUT",
                &path,
                Some(json!({"label":"","description":""})),
                StatusCode::OK,
            )
            .await?;
        assert_eq!(emptied["label"], "");
        assert_eq!(emptied["description"], "");
        let fetched = f.admin("GET", &path, None, StatusCode::OK).await?;
        assert_eq!(fetched["label"], "");
        assert_eq!(fetched["description"], "");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template bounds"]
async fn component_template_shape_and_choice_validation() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let cases: Vec<(Kind, Value)> = vec![
        (Kind::ConsolePort, json!({"type": "serial"})),
        (Kind::ConsoleServerPort, json!({"type": "serial"})),
        (Kind::PowerPort, json!({"type": "iec-9999"})),
        (Kind::PowerPort, json!({"maximum_draw": 0})),
        (Kind::PowerPort, json!({"allocated_draw": -1})),
        (Kind::PowerPort, json!({"allocated_draw": 501})),
        (Kind::PowerOutlet, json!({"feed_leg": "D"})),
        (Kind::PowerOutlet, json!({"color": "#ff0000"})),
        (Kind::PowerOutlet, json!({"color": "red"})),
        (Kind::Interface, json!({"type": "ethernet"})),
        (Kind::Interface, json!({"poe_mode": "on"})),
        (Kind::Interface, json!({"rf_role": "client"})),
        (Kind::FrontPort, json!({"type": "rj-45"})),
        (Kind::RearPort, json!({"color": "12345"})),
        (Kind::FrontPort, json!({"positions": 0})),
        (Kind::FrontPort, json!({"positions": 1025})),
        (Kind::RearPort, json!({"positions": -1})),
        (Kind::InventoryItem, json!({"part_id": "x".repeat(51)})),
    ];
    for (kind, invalid) in cases {
        let name = format!("invalid-{}-{}", kind.slug(), unique_suffix());
        let mut body = kind.create_body(device, &name, &f.seed);
        for (field, value) in invalid.as_object().ok_or("invalid case")? {
            body[field.as_str()] = value.clone();
        }
        f.admin(
            "POST",
            &kind.collection(),
            Some(body),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let rows: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {} WHERE name=$1",
            kind.table()
        ))
        .bind(&name)
        .fetch_one(&f.db)
        .await?;
        assert_eq!(
            rows,
            0,
            "{} rejected an invalid body but persisted a row",
            kind.slug()
        );
    }

    // Inclusive upstream boundaries are accepted.
    let mut front =
        Kind::FrontPort.create_body(device, &format!("max-{}", unique_suffix()), &f.seed);
    front["positions"] = json!(1024);
    let created = f
        .admin(
            "POST",
            &Kind::FrontPort.collection(),
            Some(front),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(created["positions"], 1024);

    let mut port =
        Kind::PowerPort.create_body(device, &format!("max-{}", unique_suffix()), &f.seed);
    port["maximum_draw"] = json!(500);
    port["allocated_draw"] = json!(500);
    let created = f
        .admin(
            "POST",
            &Kind::PowerPort.collection(),
            Some(port),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(created["maximum_draw"], 500);
    assert_eq!(created["allocated_draw"], 500);

    // A null clear for a required scalar is refused on an existing record.
    let iface = f
        .create(
            Kind::Interface,
            device,
            &format!("null-{}", unique_suffix()),
        )
        .await?;
    f.admin(
        "PUT",
        &item_path(Kind::Interface, &iface)?,
        Some(json!({"type": null})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let front = f
        .create(
            Kind::FrontPort,
            device,
            &format!("null-{}", unique_suffix()),
        )
        .await?;
    f.admin(
        "PUT",
        &item_path(Kind::FrontPort, &front)?,
        Some(json!({"positions": null})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template graph"]
async fn component_template_parent_scopes_and_references_stay_in_scope() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);

    // A modular template belongs to exactly one of device type or module type.
    for kind in Kind::ALL.into_iter().filter(|kind| kind.modular()) {
        let name = format!("scope-{}-{}", kind.slug(), unique_suffix());
        let mut both = kind.create_body(device, &name, &f.seed);
        both["module_type_id"] = json!(f.seed.module_type);
        f.admin(
            "POST",
            &kind.collection(),
            Some(both),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let mut neither = kind.create_body(device, &name, &f.seed);
        neither
            .as_object_mut()
            .ok_or("scope body")?
            .remove("device_type_id");
        f.admin(
            "POST",
            &kind.collection(),
            Some(neither),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }

    // Device-only templates refuse a module scope and a null device type.
    for kind in [Kind::DeviceBay, Kind::InventoryItem] {
        let name = format!("device-only-{}-{}", kind.slug(), unique_suffix());
        let mut module = kind.create_body(device, &name, &f.seed);
        module["module_type_id"] = json!(f.seed.module_type);
        f.admin(
            "POST",
            &kind.collection(),
            Some(module),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let mut missing = kind.create_body(device, &name, &f.seed);
        missing["device_type_id"] = Value::Null;
        f.admin(
            "POST",
            &kind.collection(),
            Some(missing),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }

    // A device bay template needs a device type whose subdevice role is parent.
    for device_type in [f.seed.child_device_type, f.seed.device_type] {
        let mut body = Kind::DeviceBay.create_body(
            Parent::Device(device_type),
            &format!("bay-{}", unique_suffix()),
            &f.seed,
        );
        body["device_type_id"] = json!(device_type);
        f.admin(
            "POST",
            &Kind::DeviceBay.collection(),
            Some(body),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let bay = f
        .create(
            Kind::DeviceBay,
            Parent::Device(f.seed.parent_device_type),
            &format!("bay-{}", unique_suffix()),
        )
        .await?;
    assert_eq!(bay["device_type_id"], json!(f.seed.parent_device_type));

    // A power outlet's parent power port must share its device or module type.
    let port = f
        .create(Kind::PowerPort, device, &format!("pp-{}", unique_suffix()))
        .await?;
    let mut foreign = Kind::PowerOutlet.create_body(
        Parent::Device(f.seed.sibling_device_type),
        &format!("out-{}", unique_suffix()),
        &f.seed,
    );
    foreign["power_port_id"] = port["id"].clone();
    f.admin(
        "POST",
        &Kind::PowerOutlet.collection(),
        Some(foreign),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let mut same =
        Kind::PowerOutlet.create_body(device, &format!("out-{}", unique_suffix()), &f.seed);
    same["power_port_id"] = port["id"].clone();
    let outlet = f
        .admin(
            "POST",
            &Kind::PowerOutlet.collection(),
            Some(same),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(outlet["power_port_id"], port["id"]);
    let module = Parent::Module(f.seed.module_type);
    let module_port = f
        .create(Kind::PowerPort, module, &format!("pp-{}", unique_suffix()))
        .await?;
    let mut module_outlet =
        Kind::PowerOutlet.create_body(module, &format!("out-{}", unique_suffix()), &f.seed);
    module_outlet["power_port_id"] = module_port["id"].clone();
    f.admin(
        "POST",
        &Kind::PowerOutlet.collection(),
        Some(module_outlet),
        StatusCode::CREATED,
    )
    .await?;
    let mut cross =
        Kind::PowerOutlet.create_body(module, &format!("out-{}", unique_suffix()), &f.seed);
    cross["power_port_id"] = port["id"].clone();
    f.admin(
        "POST",
        &Kind::PowerOutlet.collection(),
        Some(cross),
        StatusCode::BAD_REQUEST,
    )
    .await?;

    // An interface bridge must share its parent scope and clear as a real null.
    let bridge = f
        .create(
            Kind::Interface,
            device,
            &format!("bridge-{}", unique_suffix()),
        )
        .await?;
    let mut foreign_iface = Kind::Interface.create_body(
        Parent::Device(f.seed.sibling_device_type),
        &format!("if-{}", unique_suffix()),
        &f.seed,
    );
    foreign_iface["bridge_id"] = bridge["id"].clone();
    f.admin(
        "POST",
        &Kind::Interface.collection(),
        Some(foreign_iface),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let mut same_iface =
        Kind::Interface.create_body(device, &format!("if-{}", unique_suffix()), &f.seed);
    same_iface["bridge_id"] = bridge["id"].clone();
    let iface = f
        .admin(
            "POST",
            &Kind::Interface.collection(),
            Some(same_iface),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(iface["bridge_id"], bridge["id"]);
    let cleared = f
        .admin(
            "PUT",
            &item_path(Kind::Interface, &iface)?,
            Some(json!({"bridge_id": null})),
            StatusCode::OK,
        )
        .await?;
    assert!(cleared["bridge_id"].is_null());

    // An inventory item parent must share its device type and stay acyclic.
    let root_name = format!("root-{}", unique_suffix());
    let root = f.create(Kind::InventoryItem, device, &root_name).await?;
    let stranger = f
        .create(
            Kind::InventoryItem,
            Parent::Device(f.seed.sibling_device_type),
            &format!("stranger-{}", unique_suffix()),
        )
        .await?;
    let mut foreign_child =
        Kind::InventoryItem.create_body(device, &format!("child-{}", unique_suffix()), &f.seed);
    foreign_child["parent_id"] = stranger["id"].clone();
    f.admin(
        "POST",
        &Kind::InventoryItem.collection(),
        Some(foreign_child),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let mut child_body =
        Kind::InventoryItem.create_body(device, &format!("child-{}", unique_suffix()), &f.seed);
    child_body["parent_id"] = root["id"].clone();
    let child = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(child_body),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(child["parent_id"], root["id"]);
    // Duplicate names are scoped by parent.
    let mut same_name = Kind::InventoryItem.create_body(device, &root_name, &f.seed);
    same_name["parent_id"] = root["id"].clone();
    f.admin(
        "POST",
        &Kind::InventoryItem.collection(),
        Some(same_name),
        StatusCode::CREATED,
    )
    .await?;
    // Self-parent and an ancestor cycle are refused without mutating the tree.
    let child_path = item_path(Kind::InventoryItem, &child)?;
    let root_path = item_path(Kind::InventoryItem, &root)?;
    f.admin(
        "PUT",
        &child_path,
        Some(json!({"parent_id": child["id"]})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    f.admin(
        "PUT",
        &root_path,
        Some(json!({"parent_id": child["id"]})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let unchanged = f.admin("GET", &root_path, None, StatusCode::OK).await?;
    assert!(unchanged["parent_id"].is_null());
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template uniqueness"]
async fn component_template_duplicate_names_are_scoped_and_concurrent_creates_settle() -> TestResult
{
    let f = Fixture::start().await?;
    let kind = Kind::ConsolePort;
    let device = Parent::Device(f.seed.device_type);
    let name = format!("duplicate-{}", unique_suffix());
    f.create(kind, device, &name).await?;
    f.admin(
        "POST",
        &kind.collection(),
        Some(kind.create_body(device, &name, &f.seed)),
        StatusCode::CONFLICT,
    )
    .await?;
    // The same name is free under another device type and under a module type.
    f.create(kind, Parent::Device(f.seed.sibling_device_type), &name)
        .await?;
    f.create(kind, Parent::Module(f.seed.module_type), &name)
        .await?;

    // Concurrent identical creates settle on exactly one row.
    let contested = format!("contested-{}", unique_suffix());
    let body = kind.create_body(device, &contested, &f.seed);
    let collection = kind.collection();
    let left = f.request(
        "POST",
        &collection,
        Some(body.clone()),
        Credential::Cookie(f.cookie.as_str()),
    );
    let right = f.request(
        "POST",
        &collection,
        Some(body),
        Credential::Cookie(f.cookie.as_str()),
    );
    let (left, right) = tokio::join!(left, right);
    let (left_status, left_body) = left?;
    let (right_status, right_body) = right?;
    let statuses = [left_status, right_status];
    let reasons = [&left_body["reason_code"], &right_body["reason_code"]];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CREATED)
            .count(),
        1,
        "concurrent create outcomes {statuses:?}, reason codes {reasons:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1,
        "concurrent create outcomes {statuses:?}, reason codes {reasons:?}"
    );
    let rows: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {} WHERE name=$1",
        kind.table()
    ))
    .bind(&contested)
    .fetch_one(&f.db)
    .await?;
    assert_eq!(rows, 1, "a duplicate concurrent create left extra rows");
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template delete guards"]
async fn component_template_delete_guards_name_readable_blockers() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);

    // An unreferenced template is deletable and then absent.
    let loose = f
        .create(
            Kind::RearPort,
            device,
            &format!("loose-{}", unique_suffix()),
        )
        .await?;
    let loose_path = item_path(Kind::RearPort, &loose)?;
    f.admin("DELETE", &loose_path, None, StatusCode::NO_CONTENT)
        .await?;
    f.admin("GET", &loose_path, None, StatusCode::NOT_FOUND)
        .await?;

    // A power port template referenced by an outlet template is blocked.
    let port = f
        .create(Kind::PowerPort, device, &format!("pp-{}", unique_suffix()))
        .await?;
    let mut outlet_body =
        Kind::PowerOutlet.create_body(device, &format!("outlet-{}", unique_suffix()), &f.seed);
    outlet_body["power_port_id"] = port["id"].clone();
    let outlet = f
        .admin(
            "POST",
            &Kind::PowerOutlet.collection(),
            Some(outlet_body),
            StatusCode::CREATED,
        )
        .await?;
    let port_path = item_path(Kind::PowerPort, &port)?;
    let blocked = f
        .admin("DELETE", &port_path, None, StatusCode::CONFLICT)
        .await?;
    assert_blocked(&blocked, &[record_id(&outlet)?])?;
    // Deleting the outlet releases the port.
    f.admin(
        "DELETE",
        &item_path(Kind::PowerOutlet, &outlet)?,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    f.admin("DELETE", &port_path, None, StatusCode::NO_CONTENT)
        .await?;

    // Front and rear template mappings (seeded directly: no REST surface of
    // their own) block deletion, but the mapping table has no canonical content
    // type, so the refusal stays generic: no details, ids, labels or table name.
    let front = f
        .create(
            Kind::FrontPort,
            device,
            &format!("front-{}", unique_suffix()),
        )
        .await?;
    let rear = f
        .create(Kind::RearPort, device, &format!("rear-{}", unique_suffix()))
        .await?;
    let mapping = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_porttemplatemapping (id,front_port_position,rear_port_position,device_type_id,module_type_id,front_port_id,rear_port_id) VALUES ($1,1,1,$2,NULL,$3,$4)")
        .bind(mapping)
        .bind(f.seed.device_type)
        .bind(Uuid::parse_str(&record_id(&front)?)?)
        .bind(Uuid::parse_str(&record_id(&rear)?)?)
        .execute(&f.db)
        .await?;
    for (kind, record) in [(Kind::FrontPort, &front), (Kind::RearPort, &rear)] {
        let blocked = f
            .admin(
                "DELETE",
                &item_path(kind, record)?,
                None,
                StatusCode::CONFLICT,
            )
            .await?;
        assert_eq!(blocked["code"], "CONFLICT");
        assert_eq!(blocked["reason_code"], "DELETE_BLOCKED");
        assert!(
            blocked.get("details").is_none(),
            "an unregistered relation table must not be projected"
        );
        let serialized = blocked.to_string();
        assert!(!serialized.contains(&record_id(&front)?));
        assert!(!serialized.contains(&record_id(&rear)?));
        assert!(!serialized.contains(&mapping.to_string()));
        assert!(!serialized.contains("dcim_porttemplatemapping"));
    }
    let mappings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_porttemplatemapping WHERE id=$1")
            .bind(mapping)
            .fetch_one(&f.db)
            .await?;
    assert_eq!(
        mappings, 1,
        "a refused delete must leave the mapping untouched"
    );
    sqlx::query("DELETE FROM dcim_porttemplatemapping WHERE id=$1")
        .bind(mapping)
        .execute(&f.db)
        .await?;
    for (kind, record) in [(Kind::FrontPort, &front), (Kind::RearPort, &rear)] {
        f.admin(
            "DELETE",
            &item_path(kind, record)?,
            None,
            StatusCode::NO_CONTENT,
        )
        .await?;
    }

    // Blocker identities stay private without READ on the referencing record.
    let port = f
        .create(Kind::PowerPort, device, &format!("pp-{}", unique_suffix()))
        .await?;
    let mut outlet_body =
        Kind::PowerOutlet.create_body(device, &format!("outlet-{}", unique_suffix()), &f.seed);
    outlet_body["power_port_id"] = port["id"].clone();
    let outlet = f
        .admin(
            "POST",
            &Kind::PowerOutlet.collection(),
            Some(outlet_body),
            StatusCode::CREATED,
        )
        .await?;
    let scope = json!({
        "action": "delete",
        "resource_type": "inventory",
        "resource_pattern": format!("inventory:{}", record_id(&port)?),
    });
    let token = f.token(json!([scope])).await?;
    let hidden = f
        .call(
            "DELETE",
            &item_path(Kind::PowerPort, &port)?,
            None,
            Credential::Bearer(&token),
            StatusCode::CONFLICT,
        )
        .await?;
    assert_eq!(hidden["code"], "CONFLICT");
    assert_eq!(hidden["reason_code"], "DELETE_BLOCKED");
    assert!(
        hidden.get("details").is_none(),
        "unreadable blockers must not be projected"
    );
    let serialized = hidden.to_string();
    assert!(!serialized.contains(record_id(&outlet)?.as_str()));
    assert!(!serialized.contains(record_id(&port)?.as_str()));
    f.admin(
        "GET",
        &item_path(Kind::PowerPort, &port)?,
        None,
        StatusCode::OK,
    )
    .await?;

    // A parent inventory item template with children is fail-closed: the local
    // owner refuses the delete with a structured blocker instead of cascading.
    let root = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("root-{}", unique_suffix()),
        )
        .await?;
    let mut child_body =
        Kind::InventoryItem.create_body(device, &format!("child-{}", unique_suffix()), &f.seed);
    child_body["parent_id"] = root["id"].clone();
    let child = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(child_body),
            StatusCode::CREATED,
        )
        .await?;
    let mut grandchild_body = Kind::InventoryItem.create_body(
        device,
        &format!("grandchild-{}", unique_suffix()),
        &f.seed,
    );
    grandchild_body["parent_id"] = child["id"].clone();
    let grandchild = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(grandchild_body),
            StatusCode::CREATED,
        )
        .await?;
    let root_path = item_path(Kind::InventoryItem, &root)?;
    let child_path = item_path(Kind::InventoryItem, &child)?;
    let grandchild_path = item_path(Kind::InventoryItem, &grandchild)?;
    let before = [
        f.admin("GET", &root_path, None, StatusCode::OK).await?,
        f.admin("GET", &child_path, None, StatusCode::OK).await?,
        f.admin("GET", &grandchild_path, None, StatusCode::OK)
            .await?,
    ];

    let blocked = f
        .admin("DELETE", &root_path, None, StatusCode::CONFLICT)
        .await?;
    assert_blocked(&blocked, &[record_id(&child)?])?;
    let after = [
        f.admin("GET", &root_path, None, StatusCode::OK).await?,
        f.admin("GET", &child_path, None, StatusCode::OK).await?,
        f.admin("GET", &grandchild_path, None, StatusCode::OK)
            .await?,
    ];
    assert_eq!(
        after, before,
        "a refused parent delete must leave the hierarchy untouched"
    );
    let root_id = Uuid::parse_str(&record_id(&root)?)?;
    let child_id = Uuid::parse_str(&record_id(&child)?)?;
    let grandchild_id = Uuid::parse_str(&record_id(&grandchild)?)?;
    let orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dcim_inventoryitemtemplate child \
         LEFT JOIN dcim_inventoryitemtemplate parent ON parent.id = child.parent_id \
         WHERE child.id IN ($1,$2,$3) AND child.parent_id IS NOT NULL AND parent.id IS NULL",
    )
    .bind(root_id)
    .bind(child_id)
    .bind(grandchild_id)
    .fetch_one(&f.db)
    .await?;
    assert_eq!(orphans, 0, "a refused delete must not orphan any row");

    // Leaf-first deletion is the supported way to remove the hierarchy.
    for path in [&grandchild_path, &child_path, &root_path] {
        f.admin("DELETE", path.as_str(), None, StatusCode::NO_CONTENT)
            .await?;
    }
    let survivors: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dcim_inventoryitemtemplate \
         WHERE id IN ($1,$2,$3) OR parent_id IN ($1,$2,$3)",
    )
    .bind(root_id)
    .bind(child_id)
    .bind(grandchild_id)
    .fetch_one(&f.db)
    .await?;
    assert_eq!(
        survivors, 0,
        "leaf-first deletion must remove the whole set"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template tokens"]
async fn component_template_pat_ceiling_does_not_inherit_parent_scope() -> TestResult {
    let f = Fixture::start().await?;
    let kind = Kind::ConsolePort;
    let collection = kind.collection();
    let device = Parent::Device(f.seed.device_type);
    let record = f
        .create(kind, device, &format!("pat-{}", unique_suffix()))
        .await?;
    let path = item_path(kind, &record)?;

    // A parent grant neither reads a child row nor lists the collection.
    let parent_read = f
        .token(json!([{
            "action": "read",
            "resource_type": "inventory",
            "resource_pattern": format!("inventory:{}", f.seed.device_type),
        }]))
        .await?;
    f.call(
        "GET",
        &path,
        None,
        Credential::Bearer(&parent_read),
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "GET",
        &collection,
        None,
        Credential::Bearer(&parent_read),
        StatusCode::FORBIDDEN,
    )
    .await?;

    // A parent grant cannot create: collection writes need the global scope.
    let parent_create = f
        .token(json!([{
            "action": "create",
            "resource_type": "inventory",
            "resource_pattern": format!("inventory:{}", f.seed.device_type),
        }]))
        .await?;
    f.call(
        "POST",
        &collection,
        Some(kind.create_body(device, &format!("denied-{}", unique_suffix()), &f.seed)),
        Credential::Bearer(&parent_create),
        StatusCode::FORBIDDEN,
    )
    .await?;

    // Create does not imply read, and global read reaches the created row.
    let global_create = f
        .token(json!([{
            "action": "create",
            "resource_type": "inventory",
            "resource_pattern": "inventory:*",
        }]))
        .await?;
    let created = f
        .call(
            "POST",
            &collection,
            Some(kind.create_body(device, &format!("token-{}", unique_suffix()), &f.seed)),
            Credential::Bearer(&global_create),
            StatusCode::CREATED,
        )
        .await?;
    f.call(
        "GET",
        &item_path(kind, &created)?,
        None,
        Credential::Bearer(&global_create),
        StatusCode::FORBIDDEN,
    )
    .await?;
    let global_read = f
        .token(json!([{
            "action": "read",
            "resource_type": "inventory",
            "resource_pattern": "inventory:*",
        }]))
        .await?;
    f.call(
        "GET",
        &item_path(kind, &created)?,
        None,
        Credential::Bearer(&global_read),
        StatusCode::OK,
    )
    .await?;
    f.call(
        "GET",
        &collection,
        None,
        Credential::Bearer(&global_read),
        StatusCode::OK,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component template lists"]
async fn component_template_lists_scope_by_parent_and_reject_unknown_filters() -> TestResult {
    let f = Fixture::start().await?;
    for kind in Kind::ALL {
        let collection = kind.collection();
        let device = kind.default_parent(&f.seed);
        // Rows and every positive scope filter below use the same per-kind
        // parent: device bays live under the parent-role type.
        let parent_id = device.scope().1;
        let prefix = format!("list-{}-{}", kind.slug(), unique_suffix());
        let first = f.create(kind, device, &format!("{prefix}-a")).await?;
        let last = f.create(kind, device, &format!("{prefix}-z")).await?;
        let module_record = if kind.modular() {
            Some(
                f.create(
                    kind,
                    Parent::Module(f.seed.module_type),
                    &format!("list-mod-{}", unique_suffix()),
                )
                .await?,
            )
        } else {
            None
        };
        f.create(
            kind,
            Parent::Device(f.seed.sibling_device_type),
            &format!("list-other-{}", unique_suffix()),
        )
        .await?;

        let page = f
            .admin(
                "GET",
                &format!("{collection}?device_type_id={parent_id}"),
                None,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(
            page["total"],
            2,
            "{} list must count only the two rows of this device type",
            kind.slug()
        );
        let items = page["items"].as_array().ok_or("items")?;
        assert!(
            items
                .iter()
                .all(|item| item["device_type_id"] == json!(parent_id)),
            "{} list must stay inside the requested device type",
            kind.slug()
        );
        assert!(items.iter().any(|item| item["id"] == first["id"]));

        let searched = f
            .admin(
                "GET",
                &format!("{collection}?search={prefix}-a"),
                None,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(searched["total"], 1);
        assert_eq!(searched["items"][0]["id"], first["id"]);

        // Sorting is allowlisted to name/description with asc/desc, and equal
        // keys still resolve deterministically (name, then id).
        let first_id = first["id"].as_str().ok_or("first id")?;
        let last_id = last["id"].as_str().ok_or("last id")?;
        let ascending = f
            .admin(
                "GET",
                &format!(
                    "{collection}?device_type_id={}&sort=name&order=asc",
                    parent_id
                ),
                None,
                StatusCode::OK,
            )
            .await?;
        let ids: Vec<&str> = ascending["items"]
            .as_array()
            .ok_or("items")?
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec![first_id, last_id],
            "{} ascending name sort",
            kind.slug()
        );
        let descending = f
            .admin(
                "GET",
                &format!(
                    "{collection}?device_type_id={}&sort=name&order=desc",
                    parent_id
                ),
                None,
                StatusCode::OK,
            )
            .await?;
        let ids: Vec<&str> = descending["items"]
            .as_array()
            .ok_or("items")?
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec![last_id, first_id],
            "{} descending name sort",
            kind.slug()
        );
        let repeated = f
            .admin(
                "GET",
                &format!(
                    "{collection}?device_type_id={}&sort=name&order=asc",
                    parent_id
                ),
                None,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(
            repeated,
            ascending,
            "{} sorted list must be stable",
            kind.slug()
        );
        let by_description = f
            .admin(
                "GET",
                &format!(
                    "{collection}?device_type_id={}&sort=description&order=desc",
                    parent_id
                ),
                None,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(by_description["total"], 2);

        if let Some(module_record) = module_record {
            let modules = f
                .admin(
                    "GET",
                    &format!("{collection}?module_type_id={}", f.seed.module_type),
                    None,
                    StatusCode::OK,
                )
                .await?;
            assert!(
                modules["items"]
                    .as_array()
                    .ok_or("items")?
                    .iter()
                    .any(|item| item["id"] == module_record["id"]),
                "{} module scope filter must select the module template",
                kind.slug()
            );
        }

        // Unsupported keys are refused even when empty, and a malformed parent
        // filter is a validation error rather than an empty page.
        for query in [
            "unexpected_filter=".to_owned(),
            "unexpected_filter=1".to_owned(),
            format!("device_id={}", f.seed.device_type),
            "sort=model".to_owned(),
            "sort=name&order=sideways".to_owned(),
        ] {
            f.admin(
                "GET",
                &format!("{collection}?{query}"),
                None,
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
        f.admin(
            "GET",
            &format!("{collection}?device_type_id=not-a-uuid"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let limited = f
            .admin(
                "GET",
                &format!("{collection}?device_type_id={parent_id}&limit=1"),
                None,
                StatusCode::OK,
            )
            .await?;
        assert!(limited["items"].as_array().ok_or("items")?.len() <= 1);

        let missing = format!("{collection}/{}", Uuid::now_v7());
        f.admin("GET", &missing, None, StatusCode::NOT_FOUND)
            .await?;
        f.admin(
            "PUT",
            &missing,
            Some(json!({"label":"missing"})),
            StatusCode::NOT_FOUND,
        )
        .await?;
        f.admin("DELETE", &missing, None, StatusCode::NOT_FOUND)
            .await?;
    }
    Ok(())
}

/// Policies are cached in Redis behind a generation key; a direct SQL seed
/// bypasses the admin API's invalidation.
async fn bump_policy_cache() -> TestResult {
    let mut redis = build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    Ok(())
}

/// Seed a non-admin role, user and assignment, then log in. Policies for the
/// role are attached separately, so a case can mix resource-scoped SQL seeds
/// with canonically validated attribute policies created through the admin API.
async fn seed_restricted_principal(f: &Fixture) -> TestResult<(Uuid, String)> {
    let role = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_role (id,name,description) VALUES ($1,$2,NULL)")
        .bind(role)
        .bind(format!("pwa80-component-template-role-{}", unique_suffix()))
        .execute(&f.db)
        .await?;
    let username = format!("pwa80-component-template-{}", unique_suffix());
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')")
        .bind(user)
        .bind(&username)
        .bind(format!("{username}@test.local"))
        .execute(&f.db)
        .await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id,password_hash,password_changed_at) SELECT $1,c.password_hash,now() FROM auth_user_credential c JOIN identity_user u ON u.id=c.user_id WHERE u.username=$2")
        .bind(user)
        .bind(TEST_ADMIN_USERNAME)
        .execute(&f.db)
        .await?;
    sqlx::query(
        "INSERT INTO rbac_role_assignment (id,user_id,group_id,role_id) VALUES ($1,$2,NULL,$3)",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .bind(role)
    .execute(&f.db)
    .await?;
    let cookie = login_and_get_cookie(&f.app, &username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("restricted login failed")?;
    Ok((role, cookie))
}

/// Insert one unconstrained resource-scoped allow policy (direct SQL). Used by
/// the controls that only need `inventory:<id>` grants.
async fn insert_resource_policy(
    f: &Fixture,
    role: Uuid,
    actions: Value,
    resources: Value,
) -> TestResult {
    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) VALUES ($1,$2,'allow',$3,$4,NULL,$5,$6,true)")
        .bind(Uuid::now_v7())
        .bind(format!(
            "pwa80-component-template-policy-{}",
            unique_suffix()
        ))
        .bind(actions)
        .bind(json!(["INVENTORY"]))
        .bind(json!([format!("role:{role}")]))
        .bind(resources)
        .execute(&f.db)
        .await?;
    Ok(())
}

/// Canonical policy input, created through the admin API so the access-control
/// write path validates the constraint vocabulary (including the mandatory
/// `object_type` discriminator for Inventory attributes).
fn policy_payload(effect: &str, actions: Value, constraints: Option<Value>, role: Uuid) -> Value {
    policy_payload_for_resources(effect, actions, constraints, json!(["inventory:*"]), role)
}

fn policy_payload_for_resources(
    effect: &str,
    actions: Value,
    constraints: Option<Value>,
    resources: Value,
    role: Uuid,
) -> Value {
    json!({
        "name": format!("pwa80-component-template-{}", unique_suffix()),
        "description": null,
        "effect": effect,
        "actions": actions,
        "resource_types": ["INVENTORY"],
        "resources": resources,
        "constraints": constraints,
        "subjects": [format!("role:{role}")],
        "enabled": true,
    })
}

async fn create_policy(f: &Fixture, payload: Value) -> TestResult {
    f.admin(
        "POST",
        "/api/v1/admin/access-control/policies",
        Some(payload),
        StatusCode::OK,
    )
    .await?;
    Ok(())
}

/// Seed a non-admin session principal whose only grants are the given
/// `(actions, resource patterns)` pairs over `INVENTORY`. Each pair becomes one
/// allow policy attached to a fresh role assigned to the new user.
async fn seed_restricted_user(f: &Fixture, grants: &[(Value, Value)]) -> TestResult<String> {
    let (role, cookie) = seed_restricted_principal(f).await?;
    for (actions, resources) in grants {
        insert_resource_policy(f, role, actions.clone(), resources.clone()).await?;
    }
    bump_policy_cache().await?;
    Ok(cookie)
}

#[derive(Clone, Copy, Debug)]
enum WaitingAuthorityChange {
    DirectRole,
    DirectRoleRepeatableRead,
    AccountDeactivated,
    GroupRole,
    GroupSubject,
    TokenRevoked,
    TokenNarrowed,
}

/// Hold the application's own pool connection and graph lock. The other pool
/// connection must reach the advisory wait before authority changes, proving
/// that the request's initial authentication and route gate already ran.
async fn assert_waiting_writer_rechecks_authority(change: WaitingAuthorityChange) -> TestResult {
    let f = Fixture::start().await?;
    let kind = Kind::ConsolePort;
    let parent = Parent::Device(f.seed.device_type);
    let (header, credential, user, role, group, token_id) = match change {
        WaitingAuthorityChange::TokenRevoked | WaitingAuthorityChange::TokenNarrowed => {
            let token = f.token(json!([{
                "action": "create", "resource_type": "inventory", "resource_pattern": "inventory:*"
            }])).await?;
            let id = Uuid::parse_str(token.split('_').nth(1).ok_or("token id")?)?;
            (
                "authorization",
                format!("Bearer {token}"),
                None,
                None,
                None,
                Some(id),
            )
        }
        _ => {
            let (role, cookie) = seed_restricted_principal(&f).await?;
            let user: Uuid = sqlx::query_scalar("SELECT user_id FROM rbac_role_assignment WHERE role_id = $1 AND user_id IS NOT NULL")
                .bind(role).fetch_one(&f.db).await?;
            insert_resource_policy(&f, role, json!(["CREATE"]), json!(["inventory:*"])).await?;
            let group = if matches!(
                change,
                WaitingAuthorityChange::GroupRole | WaitingAuthorityChange::GroupSubject
            ) {
                let group = Uuid::now_v7();
                sqlx::query("INSERT INTO identity_group (id,name) VALUES ($1,$2)")
                    .bind(group)
                    .bind(format!("component-wait-{}", unique_suffix()))
                    .execute(&f.db)
                    .await?;
                sqlx::query("INSERT INTO identity_group_membership (id,user_id,group_id,source) VALUES ($1,$2,$3,'manual')")
                    .bind(Uuid::now_v7()).bind(user).bind(group).execute(&f.db).await?;
                sqlx::query("DELETE FROM rbac_role_assignment WHERE role_id=$1 AND user_id=$2")
                    .bind(role)
                    .bind(user)
                    .execute(&f.db)
                    .await?;
                if matches!(change, WaitingAuthorityChange::GroupRole) {
                    sqlx::query("INSERT INTO rbac_role_assignment (id,user_id,group_id,role_id) VALUES ($1,NULL,$2,$3)")
                        .bind(Uuid::now_v7()).bind(group).bind(role).execute(&f.db).await?;
                } else {
                    sqlx::query("UPDATE rbac_policy SET subjects=$1 WHERE subjects @> $2")
                        .bind(json!([format!("group:{group}")]))
                        .bind(json!([format!("role:{role}")]))
                        .execute(&f.db)
                        .await?;
                }
                Some(group)
            } else {
                None
            };
            bump_policy_cache().await?;
            ("cookie", cookie, Some(user), Some(role), group, None)
        }
    };

    // Establish the initial grant and populate the ordinary policy cache.
    let preflight = Request::builder()
        .method("POST")
        .uri(kind.collection())
        .header("content-type", "application/json")
        .header(header, credential.as_str())
        .body(Body::from(
            kind.create_body(parent, &format!("before-{}", unique_suffix()), &f.seed)
                .to_string(),
        ))?;
    let response = f.app.clone().oneshot(preflight).await?;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "initial grant for {change:?}"
    );

    let repeatable_read_default =
        matches!(change, WaitingAuthorityChange::DirectRoleRepeatableRead);
    if repeatable_read_default {
        // Hold both distinct connections while configuring only this fixture's
        // application sessions, not the shared server or database default.
        let mut first = f.app_db.acquire().await?;
        let mut second = f.app_db.acquire().await?;
        for connection in [&mut *first, &mut *second] {
            sqlx::query(
                "SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL REPEATABLE READ",
            )
            .execute(&mut *connection)
            .await?;
            let isolation: String = sqlx::query_scalar("SHOW default_transaction_isolation")
                .fetch_one(&mut *connection)
                .await?;
            assert_eq!(isolation, "repeatable read");
        }
    }

    let mut holder = f.app_db.begin().await?;
    sqlx::query("SELECT dcim_acquire_hardware_graph()")
        .execute(&mut *holder)
        .await?;
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await?;
    if repeatable_read_default {
        let isolation: String = sqlx::query_scalar("SHOW transaction_isolation")
            .fetch_one(&mut *holder)
            .await?;
        assert_eq!(isolation, "repeatable read");
    }
    // build_test_app_state uses a two-connection pool. With the holder checked
    // out, this is the only connection available to the request's transaction.
    let mut writer_connection = f.app_db.acquire().await?;
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer_connection)
        .await?;
    assert_ne!(holder_pid, writer_pid);
    if repeatable_read_default {
        let isolation: String = sqlx::query_scalar("SHOW default_transaction_isolation")
            .fetch_one(&mut *writer_connection)
            .await?;
        assert_eq!(isolation, "repeatable read");
    }
    drop(writer_connection);
    let name = format!("after-{}", unique_suffix());
    let request = Request::builder()
        .method("POST")
        .uri(kind.collection())
        .header("content-type", "application/json")
        .header(header, credential.as_str())
        .body(Body::from(
            kind.create_body(parent, &name, &f.seed).to_string(),
        ))?;
    let app = f.app.clone();
    let writer = tokio::spawn(async move { app.oneshot(request).await });
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_locks l WHERE l.pid=$1 AND l.locktype='advisory' AND NOT l.granted AND $2 = ANY(pg_blocking_pids(l.pid)))")
                .bind(writer_pid).bind(holder_pid).fetch_one(&mut *holder).await?;
            if waiting { return Ok::<(), sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;

    let affected = match change {
        WaitingAuthorityChange::DirectRole | WaitingAuthorityChange::DirectRoleRepeatableRead => sqlx::query("DELETE FROM rbac_role_assignment WHERE user_id=$1 AND role_id=$2")
            .bind(user).bind(role).execute(&mut *holder).await?.rows_affected(),
        WaitingAuthorityChange::GroupRole | WaitingAuthorityChange::GroupSubject => sqlx::query("DELETE FROM identity_group_membership WHERE user_id=$1 AND group_id=$2")
            .bind(user).bind(group).execute(&mut *holder).await?.rows_affected(),
        WaitingAuthorityChange::AccountDeactivated => sqlx::query("UPDATE identity_user SET status='inactive' WHERE id=$1")
            .bind(user).execute(&mut *holder).await?.rows_affected(),
        WaitingAuthorityChange::TokenRevoked => sqlx::query("UPDATE auth_user_api_token SET status='revoked', revoked_at=now() WHERE id=$1")
            .bind(token_id).execute(&mut *holder).await?.rows_affected(),
        WaitingAuthorityChange::TokenNarrowed => sqlx::query("UPDATE auth_user_api_token_scope SET resource_pattern=$2 WHERE token_id=$1 AND lower(action)='create' AND lower(resource_type)='inventory'")
            .bind(token_id).bind(format!("inventory:{}", f.seed.device_type)).execute(&mut *holder).await?.rows_affected(),
    };
    assert_eq!(affected, 1, "authority mutation for {change:?}");
    if matches!(change, WaitingAuthorityChange::AccountDeactivated) {
        let retained_grants: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rbac_role_assignment WHERE user_id=$1 AND role_id=$2) AND EXISTS (SELECT 1 FROM rbac_policy WHERE subjects @> $3 AND enabled=true)")
            .bind(user)
            .bind(role)
            .bind(json!([format!("role:{}", role.ok_or("session role missing")?)]))
            .fetch_one(&mut *holder)
            .await?;
        assert!(
            retained_grants,
            "account deactivation removed the policy grant"
        );
    }
    holder.commit().await?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(20), writer).await???;
    let expected = if matches!(
        change,
        WaitingAuthorityChange::TokenRevoked | WaitingAuthorityChange::AccountDeactivated
    ) {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::FORBIDDEN
    };
    assert_eq!(
        response.status(),
        expected,
        "locked authorization for {change:?}"
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_consoleporttemplate WHERE name=$1")
            .bind(&name)
            .fetch_one(&f.db)
            .await?;
    assert_eq!(rows, 0, "denied writer persisted a row for {change:?}");
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; graph lock authority recheck"]
async fn component_template_waiting_writer_rechecks_direct_role() -> TestResult {
    assert_waiting_writer_rechecks_authority(WaitingAuthorityChange::DirectRole).await
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; graph lock account activity recheck"]
async fn component_template_waiting_writer_rechecks_deactivated_session_account() -> TestResult {
    assert_waiting_writer_rechecks_authority(WaitingAuthorityChange::AccountDeactivated).await
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; repeatable-read session authority recheck"]
async fn component_template_waiting_writer_rechecks_role_with_repeatable_read_default() -> TestResult
{
    assert_waiting_writer_rechecks_authority(WaitingAuthorityChange::DirectRoleRepeatableRead).await
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; graph lock authority recheck"]
async fn component_template_waiting_writer_rechecks_group_role() -> TestResult {
    assert_waiting_writer_rechecks_authority(WaitingAuthorityChange::GroupRole).await
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; graph lock authority recheck"]
async fn component_template_waiting_writer_rechecks_group_subject() -> TestResult {
    assert_waiting_writer_rechecks_authority(WaitingAuthorityChange::GroupSubject).await
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; graph lock token recheck"]
async fn component_template_waiting_writer_rechecks_revoked_pat() -> TestResult {
    assert_waiting_writer_rechecks_authority(WaitingAuthorityChange::TokenRevoked).await
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; graph lock token recheck"]
async fn component_template_waiting_writer_rechecks_narrowed_pat() -> TestResult {
    assert_waiting_writer_rechecks_authority(WaitingAuthorityChange::TokenNarrowed).await
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; non-admin component template policies"]
async fn component_template_policies_scope_persisted_rows_and_parents() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let template_a = f
        .create(
            Kind::ConsolePort,
            device,
            &format!("policy-{}", unique_suffix()),
        )
        .await?;
    let template_b = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("policy-{}", unique_suffix()),
        )
        .await?;
    let port_allowed = f
        .create(
            Kind::PowerPort,
            device,
            &format!("port-{}", unique_suffix()),
        )
        .await?;
    let port_denied = f
        .create(
            Kind::PowerPort,
            device,
            &format!("port-{}", unique_suffix()),
        )
        .await?;
    let outlet = f
        .create(
            Kind::PowerOutlet,
            device,
            &format!("outlet-{}", unique_suffix()),
        )
        .await?;

    // The caller can act on one persisted template row, on its parent device
    // type, on one power port and on its own outlet; nothing else.
    let grants = [
        (
            json!(["READ", "UPDATE", "DELETE"]),
            json!([format!("inventory:{}", record_id(&template_a)?)]),
        ),
        (
            json!(["READ", "UPDATE"]),
            json!([format!("inventory:{}", f.seed.device_type)]),
        ),
        (
            json!(["READ"]),
            json!([format!("inventory:{}", record_id(&port_allowed)?)]),
        ),
        (
            json!(["READ", "UPDATE"]),
            json!([format!("inventory:{}", record_id(&outlet)?)]),
        ),
    ];
    let restricted = seed_restricted_user(&f, &grants).await?;
    let caller = Credential::Cookie(&restricted);

    // Allowed: the granted persisted row reads and updates in place.
    let template_a_path = item_path(Kind::ConsolePort, &template_a)?;
    f.call("GET", &template_a_path, None, caller, StatusCode::OK)
        .await?;
    let updated = f
        .call(
            "PUT",
            &template_a_path,
            Some(json!({"label":"in scope"})),
            caller,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(updated["label"], "in scope");
    assert_eq!(updated["device_type_id"], json!(f.seed.device_type));

    // Denied: a body-supplied parent the caller cannot access must not move the
    // persisted row. A refusal for any reason is acceptable; a mutation is not.
    let before = f
        .admin("GET", &template_a_path, None, StatusCode::OK)
        .await?;
    let (status, refused) = f
        .request(
            "PUT",
            &template_a_path,
            Some(json!({"device_type_id": f.seed.sibling_device_type})),
            caller,
        )
        .await?;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::BAD_REQUEST,
        "moving to an unauthorized parent answered {status}: {refused}"
    );
    let after = f
        .admin("GET", &template_a_path, None, StatusCode::OK)
        .await?;
    assert_eq!(
        after, before,
        "a refused parent move must not mutate the row"
    );
    assert_eq!(after["device_type_id"], json!(f.seed.device_type));

    // Denied: an ungranted existing row cannot be read, updated or deleted.
    // Naming a granted parent in the body must not substitute for the
    // persisted row's own parent: this request must be refused by authorization
    // (403) before any immutable-parent rule can answer instead.
    let template_b_path = item_path(Kind::ConsolePort, &template_b)?;
    let template_b_before = f
        .admin("GET", &template_b_path, None, StatusCode::OK)
        .await?;
    f.call("GET", &template_b_path, None, caller, StatusCode::FORBIDDEN)
        .await?;
    f.call(
        "PUT",
        &template_b_path,
        Some(json!({"label":"stolen"})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "PUT",
        &template_b_path,
        Some(json!({"label":"bypass","device_type_id":f.seed.device_type})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "DELETE",
        &template_b_path,
        None,
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let template_b_after = f
        .admin("GET", &template_b_path, None, StatusCode::OK)
        .await?;
    assert_eq!(
        template_b_after, template_b_before,
        "denied writes must not mutate an ungranted row"
    );
    assert_eq!(
        template_b_after["device_type_id"],
        json!(f.seed.sibling_device_type)
    );

    // Denied: naming a known, granted parent does not authorize the collection.
    for device_type in [f.seed.device_type, f.seed.sibling_device_type] {
        let name = format!("policy-create-{}", unique_suffix());
        let body = json!({"name": name.as_str(), "device_type_id": device_type});
        f.call(
            "POST",
            &Kind::ConsolePort.collection(),
            Some(body),
            caller,
            StatusCode::FORBIDDEN,
        )
        .await?;
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM dcim_consoleporttemplate WHERE name=$1")
                .bind(&name)
                .fetch_one(&f.db)
                .await?;
        assert_eq!(rows, 0, "a denied create must not persist");
    }

    // Denied: the update grant on the outlet does not extend to an unreadable
    // power port reference; the persisted reference stays unchanged.
    let outlet_path = item_path(Kind::PowerOutlet, &outlet)?;
    f.call(
        "GET",
        &item_path(Kind::PowerPort, &port_denied)?,
        None,
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "PUT",
        &outlet_path,
        Some(json!({"power_port_id": port_denied["id"]})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let outlet_after = f.admin("GET", &outlet_path, None, StatusCode::OK).await?;
    assert!(
        outlet_after["power_port_id"].is_null(),
        "a refused reference change must not persist"
    );

    // Allowed: the same change to a readable, same-scope port succeeds.
    let changed = f
        .call(
            "PUT",
            &outlet_path,
            Some(json!({"power_port_id": port_allowed["id"]})),
            caller,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(changed["power_port_id"], port_allowed["id"]);
    let persisted = f.admin("GET", &outlet_path, None, StatusCode::OK).await?;
    assert_eq!(persisted["power_port_id"], port_allowed["id"]);

    // Allowed: the granted row is deletable; the ungranted one is not.
    f.call(
        "DELETE",
        &template_a_path,
        None,
        caller,
        StatusCode::NO_CONTENT,
    )
    .await?;
    f.admin("GET", &template_a_path, None, StatusCode::NOT_FOUND)
        .await?;
    f.call(
        "DELETE",
        &template_b_path,
        None,
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; attribute-scoped component template policies"]
async fn component_template_attribute_policies_gate_parent_and_object_type() -> TestResult {
    let f = Fixture::start().await?;
    let (role, cookie) = seed_restricted_principal(&f).await?;
    let caller = Credential::Cookie(&cookie);
    let device = Parent::Device(f.seed.device_type);

    // The collection gates need unconstrained grants; the canonical attribute
    // denies then narrow one parent and one object class.
    for actions in [json!(["READ"]), json!(["CREATE"]), json!(["UPDATE"])] {
        create_policy(&f, policy_payload("allow", actions, None, role)).await?;
    }
    create_policy(
        &f,
        policy_payload(
            "deny",
            json!(["READ", "CREATE", "UPDATE"]),
            Some(json!({"attributes": {
                "object_type": "dcim.consoleporttemplate",
                "device_type_id": f.seed.sibling_device_type,
            }})),
            role,
        ),
    )
    .await?;
    create_policy(
        &f,
        policy_payload(
            "deny",
            json!(["READ", "UPDATE"]),
            Some(json!({"attributes": {
                "object_type": "dcim.powerporttemplate",
                "device_type_id": f.seed.device_type,
            }})),
            role,
        ),
    )
    .await?;

    // Allowed create, read and update inside the granted device type.
    let created = f
        .call(
            "POST",
            &Kind::ConsolePort.collection(),
            Some(Kind::ConsolePort.create_body(
                device,
                &format!("attr-{}", unique_suffix()),
                &f.seed,
            )),
            caller,
            StatusCode::CREATED,
        )
        .await?;
    let path = item_path(Kind::ConsolePort, &created)?;
    f.call("GET", &path, None, caller, StatusCode::OK).await?;
    let updated = f
        .call(
            "PUT",
            &path,
            Some(json!({"label":"attribute scope"})),
            caller,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(updated["label"], "attribute scope");

    // Denied create in the other device type, with no persisted row.
    let denied_name = format!("attr-denied-{}", unique_suffix());
    f.call(
        "POST",
        &Kind::ConsolePort.collection(),
        Some(Kind::ConsolePort.create_body(
            Parent::Device(f.seed.sibling_device_type),
            &denied_name,
            &f.seed,
        )),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_consoleporttemplate WHERE name=$1")
            .bind(&denied_name)
            .fetch_one(&f.db)
            .await?;
    assert_eq!(rows, 0, "an attribute-denied create must not persist");

    // Denied read and update of a persisted row in the other device type.
    let foreign = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("attr-foreign-{}", unique_suffix()),
        )
        .await?;
    let foreign_path = item_path(Kind::ConsolePort, &foreign)?;
    let foreign_before = f.admin("GET", &foreign_path, None, StatusCode::OK).await?;
    f.call("GET", &foreign_path, None, caller, StatusCode::FORBIDDEN)
        .await?;
    f.call(
        "PUT",
        &foreign_path,
        Some(json!({"label":"stolen"})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let foreign_after = f.admin("GET", &foreign_path, None, StatusCode::OK).await?;
    assert_eq!(
        foreign_after, foreign_before,
        "a denied attribute update must not persist"
    );

    // object_type discrimination: the unconstrained allow would serve this
    // power port template, so only the class-scoped deny can explain the 403.
    let port = f
        .create(
            Kind::PowerPort,
            device,
            &format!("attr-port-{}", unique_suffix()),
        )
        .await?;
    let port_path = item_path(Kind::PowerPort, &port)?;
    let port_before = f.admin("GET", &port_path, None, StatusCode::OK).await?;
    f.call("GET", &port_path, None, caller, StatusCode::FORBIDDEN)
        .await?;
    f.call(
        "PUT",
        &port_path,
        Some(json!({"label":"stolen"})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let port_after = f.admin("GET", &port_path, None, StatusCode::OK).await?;
    assert_eq!(
        port_after, port_before,
        "a class-scoped deny must block the write"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; policy write-time object_type gating"]
async fn component_template_attribute_policies_reject_unenforced_object_types() -> TestResult {
    let f = Fixture::start().await?;
    let (role, cookie) = seed_restricted_principal(&f).await?;
    let caller = Credential::Cookie(&cookie);
    let policies = "/api/v1/admin/access-control/policies";

    // A DeviceType read route passes no attributes, so a deny naming
    // `dcim.devicetype` would be inert. The write path must refuse it instead of
    // accepting a policy that reads as a deny but never fires.
    for constraints in [
        json!({"attributes": {"object_type": "dcim.devicetype"}}),
        json!({"attributes": {"object_type": "dcim.devicetype", "device_type_id": f.seed.device_type}}),
        json!({"attributes": {"object_type": "dcim.coolingintaketemplate"}}),
        json!({"attributes": {"object_type": "dcim.widgettemplate"}}),
        // A valid token nested in `or` does not excuse an unsupported sibling.
        json!({"or": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"attributes": {"object_type": "dcim.devicetype"}}
        ]}),
        // A guarded `or` branch does not guard an unguarded sibling branch.
        json!({"or": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"attributes": {"manufacturer_id": f.seed.manufacturer}}
        ]}),
        json!({"or": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"attributes": {}}
        ]}),
        // The discriminator must be a canonical string token.
        json!({"attributes": {"object_type": 7}}),
    ] {
        let response = f
            .admin(
                "POST",
                policies,
                Some(policy_payload(
                    "deny",
                    json!(["READ"]),
                    Some(constraints.clone()),
                    role,
                )),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        assert!(
            response["message"]
                .as_str()
                .is_some_and(|message| message.contains("object_type")),
            "rejection must name object_type, got {response}"
        );
    }

    // The supported condition-expression forms still validate and enforce.
    for constraints in [
        json!({"and": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"attributes": {"device_type_id": f.seed.sibling_device_type}}
        ]}),
        json!([{"attributes": {
            "object_type": "dcim.consoleporttemplate",
            "device_type_id": f.seed.sibling_device_type
        }}]),
        // Split `and` guard: the discriminator sibling covers a plain attribute.
        json!({"and": [
            {"attributes": {"object_type": "dcim.consoleporttemplate"}},
            {"attributes": {"manufacturer_id": f.seed.manufacturer}}
        ]}),
        // Every `or` branch carries its own supported discriminator.
        json!({"or": [
            {"attributes": {
                "object_type": "dcim.consoleporttemplate",
                "device_type_id": f.seed.sibling_device_type
            }},
            {"attributes": {
                "object_type": "dcim.consoleporttemplate",
                "parent_id": Uuid::now_v7()
            }}
        ]}),
    ] {
        create_policy(
            &f,
            policy_payload("deny", json!(["READ"]), Some(constraints), role),
        )
        .await?;
    }
    create_policy(&f, policy_payload("allow", json!(["READ"]), None, role)).await?;

    let allowed = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.device_type),
            &format!("expr-allowed-{}", unique_suffix()),
        )
        .await?;
    let denied = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("expr-denied-{}", unique_suffix()),
        )
        .await?;

    f.call(
        "GET",
        &item_path(Kind::ConsolePort, &allowed)?,
        None,
        caller,
        StatusCode::OK,
    )
    .await?;
    f.call(
        "GET",
        &item_path(Kind::ConsolePort, &denied)?,
        None,
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; hidden reference privacy"]
async fn component_template_hidden_references_report_only_forbidden() -> TestResult {
    let f = Fixture::start().await?;
    let (role, cookie) = seed_restricted_principal(&f).await?;
    let caller = Credential::Cookie(&cookie);

    let same_port = f
        .create(
            Kind::PowerPort,
            Parent::Device(f.seed.device_type),
            &format!("hide-port-same-{}", unique_suffix()),
        )
        .await?;
    let other_port = f
        .create(
            Kind::PowerPort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("hide-port-other-{}", unique_suffix()),
        )
        .await?;
    let same_rear = f
        .create(
            Kind::RearPort,
            Parent::Device(f.seed.device_type),
            &format!("hide-rear-same-{}", unique_suffix()),
        )
        .await?;
    let other_rear = f
        .create(
            Kind::RearPort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("hide-rear-other-{}", unique_suffix()),
        )
        .await?;
    let other_bridge = f
        .create(
            Kind::Interface,
            Parent::Device(f.seed.sibling_device_type),
            &format!("hide-bridge-{}", unique_suffix()),
        )
        .await?;
    let other_component = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("hide-component-{}", unique_suffix()),
        )
        .await?;

    for actions in [json!(["READ"]), json!(["UPDATE"]), json!(["CREATE"])] {
        create_policy(&f, policy_payload("allow", actions, None, role)).await?;
    }
    // Exact-id READ denies hide each reference without constraining the row the
    // caller legitimately updates.
    let hidden_paths: Vec<String> = [
        &same_port,
        &other_port,
        &same_rear,
        &other_rear,
        &other_bridge,
        &other_component,
    ]
    .iter()
    .map(|record| record_id(record).map(|id| format!("inventory:{id}")))
    .collect::<Result<_, _>>()?;
    create_policy(
        &f,
        policy_payload_for_resources("deny", json!(["READ"]), None, json!(hidden_paths), role),
    )
    .await?;

    let outlet = f
        .create(
            Kind::PowerOutlet,
            Parent::Device(f.seed.device_type),
            &format!("hide-outlet-{}", unique_suffix()),
        )
        .await?;
    let outlet_path = item_path(Kind::PowerOutlet, &outlet)?;

    // The hidden power port is a valid same-parent reference and an invalid
    // other-parent one. Both must answer Forbidden: a Validation answer would
    // disclose the parent property of an unreadable row.
    f.call(
        "PUT",
        &outlet_path,
        Some(json!({"power_port_id": same_port["id"]})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "PUT",
        &outlet_path,
        Some(json!({"power_port_id": other_port["id"]})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let outlet_after = f.admin("GET", &outlet_path, None, StatusCode::OK).await?;
    assert!(
        outlet_after["power_port_id"].is_null(),
        "a hidden reference must not be persisted"
    );

    let interface = f
        .create(
            Kind::Interface,
            Parent::Device(f.seed.device_type),
            &format!("hide-iface-{}", unique_suffix()),
        )
        .await?;
    f.call(
        "PUT",
        &item_path(Kind::Interface, &interface)?,
        Some(json!({"bridge_id": other_bridge["id"]})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;

    let front = f
        .create(
            Kind::FrontPort,
            Parent::Device(f.seed.device_type),
            &format!("hide-front-{}", unique_suffix()),
        )
        .await?;
    let front_path = item_path(Kind::FrontPort, &front)?;
    for (rear, position) in [(&same_rear, 5), (&other_rear, 1)] {
        // 5 exceeds the readable front capacity, and the other-parent rear is
        // out of scope; capacity and same-parent checks must not answer before
        // the hidden rear port is authorized.
        f.call(
            "PUT",
            &front_path,
            Some(json!({"rear_ports": [{
                "position": 1,
                "rear_port": rear["id"],
                "rear_port_position": position,
            }]})),
            caller,
            StatusCode::FORBIDDEN,
        )
        .await?;
    }
    let mappings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_porttemplatemapping WHERE front_port_id=$1")
            .bind(Uuid::parse_str(&record_id(&front)?)?)
            .fetch_one(&f.db)
            .await?;
    assert_eq!(mappings, 0, "a hidden rear reference must not be mapped");

    let item = f
        .create(
            Kind::InventoryItem,
            Parent::Device(f.seed.device_type),
            &format!("hide-item-{}", unique_suffix()),
        )
        .await?;
    let console_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='consoleporttemplate'",
    )
    .fetch_one(&f.db)
    .await?;
    f.call(
        "PUT",
        &item_path(Kind::InventoryItem, &item)?,
        Some(json!({
            "component_type_id": console_ctor,
            "component_id": other_component["id"],
        })),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let item_after = f
        .admin(
            "GET",
            &item_path(Kind::InventoryItem, &item)?,
            None,
            StatusCode::OK,
        )
        .await?;
    assert!(
        item_after["component_id"].is_null(),
        "a hidden component must not be linked"
    );

    // Create-scoped PAT semantics are unchanged: the object-level CREATE gate
    // does not demand READ on every reference named in the body.
    let created = f
        .call(
            "POST",
            &Kind::PowerOutlet.collection(),
            Some(json!({
                "name": format!("hide-create-{}", unique_suffix()),
                "device_type_id": f.seed.device_type,
                "type": "iec-60320-c13",
                "color": "1a2b3c",
                "power_port_id": same_port["id"],
            })),
            caller,
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(created["power_port_id"], same_port["id"]);
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; component template list visibility and counts"]
async fn component_template_list_visibility_counts_only_authorized_rows() -> TestResult {
    let f = Fixture::start().await?;
    let prefix = format!("vis-{}", unique_suffix());
    let first = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.device_type),
            &format!("{prefix}-a"),
        )
        .await?;
    let second = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.device_type),
            &format!("{prefix}-c"),
        )
        .await?;
    let denied = f
        .create(
            Kind::ConsolePort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("{prefix}-b"),
        )
        .await?;
    let collection = Kind::ConsolePort.collection();

    // Unconstrained collection READ plus one canonical attribute deny for the
    // same object type under the other device type.
    let (role, cookie) = seed_restricted_principal(&f).await?;
    create_policy(&f, policy_payload("allow", json!(["READ"]), None, role)).await?;
    create_policy(
        &f,
        policy_payload(
            "deny",
            json!(["READ"]),
            Some(json!({"attributes": {
                "object_type": "dcim.consoleporttemplate",
                "device_type_id": f.seed.sibling_device_type,
            }})),
            role,
        ),
    )
    .await?;
    let caller = Credential::Cookie(&cookie);

    let page = f
        .call(
            "GET",
            &format!("{collection}?search={prefix}"),
            None,
            caller,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(page["total"], 2, "total must count only visible rows");
    let ids: Vec<&str> = page["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            first["id"].as_str().ok_or("first id")?,
            second["id"].as_str().ok_or("second id")?,
        ]
    );
    assert!(!ids.contains(&denied["id"].as_str().ok_or("denied id")?));

    // A filter that selects only denied rows is an empty authorized page.
    let hidden = f
        .call(
            "GET",
            &format!(
                "{collection}?search={prefix}&device_type_id={}",
                f.seed.sibling_device_type
            ),
            None,
            caller,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(hidden["total"], 0);
    assert!(hidden["items"].as_array().ok_or("items")?.is_empty());

    // Pagination never changes the authorized total and never pages in a
    // denied row.
    for (offset, expected) in [(0usize, first["id"].clone()), (1, second["id"].clone())] {
        let page = f
            .call(
                "GET",
                &format!("{collection}?search={prefix}&limit=1&offset={offset}"),
                None,
                caller,
                StatusCode::OK,
            )
            .await?;
        assert_eq!(page["total"], 2);
        assert_eq!(page["items"].as_array().ok_or("items")?.len(), 1);
        assert_eq!(page["items"][0]["id"], expected);
    }
    let beyond = f
        .call(
            "GET",
            &format!("{collection}?search={prefix}&limit=1&offset=2"),
            None,
            caller,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(beyond["total"], 2);
    assert!(beyond["items"].as_array().ok_or("items")?.is_empty());
    f.call(
        "GET",
        &item_path(Kind::ConsolePort, &denied)?,
        None,
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;

    // A constrained allow alone is not a collection grant: the list is refused
    // instead of served as a partial page, while item reads honour the scope.
    let (scoped_role, scoped_cookie) = seed_restricted_principal(&f).await?;
    create_policy(
        &f,
        policy_payload(
            "allow",
            json!(["READ"]),
            Some(json!({"attributes": {
                "object_type": "dcim.consoleporttemplate",
                "device_type_id": f.seed.device_type,
            }})),
            scoped_role,
        ),
    )
    .await?;
    let scoped = Credential::Cookie(&scoped_cookie);
    f.call(
        "GET",
        &format!("{collection}?search={prefix}"),
        None,
        scoped,
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "GET",
        &item_path(Kind::ConsolePort, &first)?,
        None,
        scoped,
        StatusCode::OK,
    )
    .await?;
    f.call(
        "GET",
        &item_path(Kind::ConsolePort, &denied)?,
        None,
        scoped,
        StatusCode::FORBIDDEN,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; front port mapping replacement"]
async fn component_template_front_port_mappings_replace_and_roll_back() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let first_rear = f
        .create(
            Kind::RearPort,
            device,
            &format!("rear-a-{}", unique_suffix()),
        )
        .await?;
    let second_rear = f
        .create(
            Kind::RearPort,
            device,
            &format!("rear-b-{}", unique_suffix()),
        )
        .await?;
    let front = f
        .create(
            Kind::FrontPort,
            device,
            &format!("front-{}", unique_suffix()),
        )
        .await?;
    let front_path = item_path(Kind::FrontPort, &front)?;
    let mapping = |position: i16, rear: &Value, rear_position: i16| json!({"position": position, "rear_port": rear["id"], "rear_port_position": rear_position});

    // Replace the complete mapping set.
    let replaced = f
        .admin(
            "PUT",
            &front_path,
            Some(json!({"rear_ports": [mapping(1, &first_rear, 2), mapping(2, &second_rear, 2)]})),
            StatusCode::OK,
        )
        .await?;
    let mappings = replaced["rear_ports"].as_array().ok_or("rear_ports")?;
    assert_eq!(mappings.len(), 2);
    assert_eq!(mappings[0]["position"], 1);
    assert_eq!(mappings[0]["rear_port"], first_rear["id"]);
    assert_eq!(mappings[0]["rear_port_position"], 2);
    assert_eq!(mappings[1]["rear_port"], second_rear["id"]);
    let reverse = f
        .admin(
            "GET",
            &item_path(Kind::RearPort, &first_rear)?,
            None,
            StatusCode::OK,
        )
        .await?;
    let entries = reverse["front_ports"].as_array().ok_or("front_ports")?;
    let entry = entries
        .iter()
        .find(|entry| entry["front_port"] == front["id"])
        .ok_or("reverse mapping")?;
    // The two sides are asymmetric: front position 1 wires rear position 2.
    assert_eq!(
        entry["position"], 2,
        "the rear side reports its own position"
    );
    assert_eq!(
        entry["front_port_position"], 1,
        "the rear side reports the front position"
    );

    // Replace the set again, unwiring the first rear port.
    let replaced = f
        .admin(
            "PUT",
            &front_path,
            Some(json!({"rear_ports": [mapping(1, &second_rear, 2)]})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(
        replaced["rear_ports"].as_array().ok_or("rear_ports")?.len(),
        1
    );
    let reverse = f
        .admin(
            "GET",
            &item_path(Kind::RearPort, &first_rear)?,
            None,
            StatusCode::OK,
        )
        .await?;
    assert!(
        reverse["front_ports"]
            .as_array()
            .ok_or("front_ports")?
            .is_empty()
    );

    // Cross-parent, out-of-range, duplicate and unknown mapping fields are
    // refused atomically: the previous single mapping survives.
    let other_rear = f
        .create(
            Kind::RearPort,
            Parent::Device(f.seed.sibling_device_type),
            &format!("rear-other-{}", unique_suffix()),
        )
        .await?;
    for invalid in [
        json!({"rear_ports": [mapping(1, &other_rear, 1)]}),
        json!({"rear_ports": [mapping(1, &first_rear, 1), mapping(5, &second_rear, 1)]}),
        json!({"rear_ports": [mapping(1, &first_rear, 1), mapping(1, &second_rear, 2)]}),
        json!({"rear_ports": [{"position": 1, "rear_port": first_rear["id"], "rear_port_position": 1, "label": "x"}]}),
    ] {
        f.admin("PUT", &front_path, Some(invalid), StatusCode::BAD_REQUEST)
            .await?;
        let unchanged = f.admin("GET", &front_path, None, StatusCode::OK).await?;
        let surviving = unchanged["rear_ports"].as_array().ok_or("rear_ports")?;
        assert_eq!(surviving.len(), 1);
        assert_eq!(surviving[0]["rear_port"], second_rear["id"]);
    }

    // Clears the complete set.
    let cleared = f
        .admin(
            "PUT",
            &front_path,
            Some(json!({"rear_ports": []})),
            StatusCode::OK,
        )
        .await?;
    assert!(
        cleared["rear_ports"]
            .as_array()
            .ok_or("rear_ports")?
            .is_empty()
    );
    let reverse = f
        .admin(
            "GET",
            &item_path(Kind::RearPort, &second_rear)?,
            None,
            StatusCode::OK,
        )
        .await?;
    assert!(
        reverse["front_ports"]
            .as_array()
            .ok_or("front_ports")?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; attribute-denied delete blockers"]
async fn component_template_delete_blockers_stay_private_when_attributes_deny_them() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let port = f
        .create(
            Kind::PowerPort,
            device,
            &format!("privacy-port-{}", unique_suffix()),
        )
        .await?;
    let outlet_name = format!("privacy-outlet-{}", unique_suffix());
    let outlet_label = format!("hidden-outlet-{}", unique_suffix());
    let mut outlet_body = Kind::PowerOutlet.create_body(device, &outlet_name, &f.seed);
    outlet_body["power_port_id"] = port["id"].clone();
    outlet_body["label"] = json!(outlet_label);
    let outlet = f
        .admin(
            "POST",
            &Kind::PowerOutlet.collection(),
            Some(outlet_body),
            StatusCode::CREATED,
        )
        .await?;
    let port_path = item_path(Kind::PowerPort, &port)?;
    let outlet_path = item_path(Kind::PowerOutlet, &outlet)?;
    let outlet_id = record_id(&outlet)?;

    // The caller may read and delete Inventory globally, but an attribute deny
    // hides this outlet class in this device type.
    let (role, cookie) = seed_restricted_principal(&f).await?;
    create_policy(&f, policy_payload("allow", json!(["READ"]), None, role)).await?;
    create_policy(&f, policy_payload("allow", json!(["DELETE"]), None, role)).await?;
    create_policy(
        &f,
        policy_payload(
            "deny",
            json!(["READ"]),
            Some(json!({"attributes": {
                "object_type": "dcim.poweroutlettemplate",
                "device_type_id": f.seed.device_type,
            }})),
            role,
        ),
    )
    .await?;
    let hidden_caller = Credential::Cookie(&cookie);
    f.call(
        "GET",
        &outlet_path,
        None,
        hidden_caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let hidden_page = f
        .call(
            "GET",
            &format!("{}?search={outlet_name}", Kind::PowerOutlet.collection()),
            None,
            hidden_caller,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(hidden_page["total"], 0);
    assert!(hidden_page["items"].as_array().ok_or("items")?.is_empty());

    // The blocked delete must not project the hidden referrer.
    let blocked = f
        .call(
            "DELETE",
            &port_path,
            None,
            hidden_caller,
            StatusCode::CONFLICT,
        )
        .await?;
    assert_eq!(blocked["code"], "CONFLICT");
    assert_eq!(blocked["reason_code"], "DELETE_BLOCKED");
    assert!(
        blocked.get("details").is_none(),
        "an unreadable blocker must not be projected"
    );
    let serialized = blocked.to_string();
    assert!(!serialized.contains(&outlet_id));
    assert!(!serialized.contains(&outlet_name));
    assert!(!serialized.contains(&outlet_label));
    f.admin("GET", &port_path, None, StatusCode::OK).await?;

    // A caller that may read the blocker still receives usable details.
    let (reader_role, reader_cookie) = seed_restricted_principal(&f).await?;
    create_policy(
        &f,
        policy_payload("allow", json!(["READ"]), None, reader_role),
    )
    .await?;
    create_policy(
        &f,
        policy_payload("allow", json!(["DELETE"]), None, reader_role),
    )
    .await?;
    let readable = f
        .call(
            "DELETE",
            &port_path,
            None,
            Credential::Cookie(&reader_cookie),
            StatusCode::CONFLICT,
        )
        .await?;
    assert_eq!(readable["reason_code"], "DELETE_BLOCKED");
    assert!(
        readable["details"]["total_count"]
            .as_i64()
            .ok_or("blocker total")?
            >= 1
    );
    let examples: Vec<&Value> = readable["details"]["blockers"]
        .as_array()
        .ok_or("blockers")?
        .iter()
        .flat_map(|group| group["examples"].as_array().into_iter().flatten())
        .collect();
    let example = examples
        .iter()
        .find(|example| example["id"] == outlet_id.as_str())
        .ok_or("blocker example")?;
    let label = example["label"].as_str().ok_or("blocker label")?;
    assert!(
        label.contains(outlet_name.as_str()) || label.contains(outlet_label.as_str()),
        "unexpected blocker label {label}"
    );

    // The port was otherwise deletable: clearing the reference releases it.
    f.admin(
        "PUT",
        &outlet_path,
        Some(json!({"power_port_id": null})),
        StatusCode::OK,
    )
    .await?;
    f.admin("DELETE", &port_path, None, StatusCode::NO_CONTENT)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; typed component target deletion"]
async fn component_template_typed_component_targets_block_deletion_until_cleared() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let console_type: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='consoleporttemplate'",
    )
    .fetch_one(&f.db)
    .await?;
    let cooling_type: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='coolingintaketemplate'",
    )
    .fetch_one(&f.db)
    .await?;

    // A console-port template target referenced from an inventory item.
    let console_port = f
        .create(
            Kind::ConsolePort,
            device,
            &format!("typed-console-{}", unique_suffix()),
        )
        .await?;
    let mut item_body = Kind::InventoryItem.create_body(
        device,
        &format!("typed-item-{}", unique_suffix()),
        &f.seed,
    );
    item_body["component_type_id"] = json!(console_type);
    item_body["component_id"] = console_port["id"].clone();
    let item = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(item_body),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(item["component_type_id"], json!(console_type));
    assert_eq!(item["component_id"], console_port["id"]);
    let target_path = item_path(Kind::ConsolePort, &console_port)?;
    let inventory_path = item_path(Kind::InventoryItem, &item)?;
    let item_before = f
        .admin("GET", &inventory_path, None, StatusCode::OK)
        .await?;
    let blocked = f
        .admin("DELETE", &target_path, None, StatusCode::CONFLICT)
        .await?;
    assert_blocked(&blocked, &[record_id(&item)?])?;
    assert!(
        blocked["details"]["blockers"]
            .as_array()
            .ok_or("blockers")?
            .iter()
            .any(|group| group["table"] == "dcim_inventoryitemtemplate"),
        "the typed reference must be named as a blocker"
    );
    f.admin("GET", &target_path, None, StatusCode::OK).await?;
    assert_eq!(
        f.admin("GET", &inventory_path, None, StatusCode::OK)
            .await?,
        item_before
    );
    // Clearing the typed reference releases the target.
    f.admin(
        "PUT",
        &inventory_path,
        Some(json!({"component_type_id": null, "component_id": null})),
        StatusCode::OK,
    )
    .await?;
    let cleared = f
        .admin("GET", &inventory_path, None, StatusCode::OK)
        .await?;
    assert!(cleared["component_type_id"].is_null() && cleared["component_id"].is_null());
    f.admin("DELETE", &target_path, None, StatusCode::NO_CONTENT)
        .await?;
    f.admin("GET", &target_path, None, StatusCode::NOT_FOUND)
        .await?;

    // A specialized cooling target must be protected by the same guard.
    let cooling = f
        .admin(
            "POST",
            "/api/v1/inventory/cooling-intake-templates",
            Some(json!({
                "name": format!("typed-intake-{}", unique_suffix()),
                "device_type_id": f.seed.device_type,
                "type": "qdc",
                "diameter": 1.25,
                "diameter_unit": "mm",
                "max_flow": 1.25,
                "max_flow_unit": "lpm",
            })),
            StatusCode::CREATED,
        )
        .await?;
    let cooling_path = format!(
        "/api/v1/inventory/cooling-intake-templates/{}",
        record_id(&cooling)?
    );
    let mut cooling_body = Kind::InventoryItem.create_body(
        device,
        &format!("typed-cooling-{}", unique_suffix()),
        &f.seed,
    );
    cooling_body["component_type_id"] = json!(cooling_type);
    cooling_body["component_id"] = cooling["id"].clone();
    let cooling_item = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(cooling_body),
            StatusCode::CREATED,
        )
        .await?;
    let cooling_item_path = item_path(Kind::InventoryItem, &cooling_item)?;
    let blocked = f
        .admin("DELETE", &cooling_path, None, StatusCode::CONFLICT)
        .await?;
    assert_blocked(&blocked, &[record_id(&cooling_item)?])?;
    f.admin("GET", &cooling_path, None, StatusCode::OK).await?;
    f.admin(
        "PUT",
        &cooling_item_path,
        Some(json!({"component_type_id": null, "component_id": null})),
        StatusCode::OK,
    )
    .await?;
    f.admin("DELETE", &cooling_path, None, StatusCode::NO_CONTENT)
        .await?;
    f.admin("GET", &cooling_path, None, StatusCode::NOT_FOUND)
        .await?;
    Ok(())
}

#[derive(Clone, Copy)]
struct TreeNode {
    id: Uuid,
    lft: i32,
    rght: i32,
    level: i32,
    tree: i32,
}

async fn load_tree(db: &PgPool, device_type_id: Uuid) -> TestResult<Vec<TreeNode>> {
    let rows = sqlx::query(
        "SELECT id,lft,rght,level,tree_id FROM dcim_inventoryitemtemplate \
         WHERE device_type_id=$1 ORDER BY lft",
    )
    .bind(device_type_id)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(TreeNode {
                id: row.try_get("id")?,
                lft: row.try_get("lft")?,
                rght: row.try_get("rght")?,
                level: row.try_get("level")?,
                tree: row.try_get("tree_id")?,
            })
        })
        .collect()
}

fn tree_node(nodes: &[TreeNode], id: Uuid) -> TestResult<TreeNode> {
    nodes
        .iter()
        .copied()
        .find(|node| node.id == id)
        .ok_or_else(|| "tree node missing".into())
}

fn assert_nested_set(
    nodes: &[TreeNode],
    parentage: &[(Uuid, Uuid)],
    levels: &[(Uuid, i32)],
) -> TestResult {
    assert!(!nodes.is_empty(), "tree must not be empty");
    let tree = nodes[0].tree;
    let mut endpoints = Vec::with_capacity(nodes.len() * 2);
    for node in nodes {
        assert_eq!(node.tree, tree, "one tree per device type");
        assert!(node.lft > 0, "left endpoint must be positive");
        assert!(node.lft < node.rght, "left endpoint must precede right");
        endpoints.push(node.lft);
        endpoints.push(node.rght);
    }
    let unique: std::collections::BTreeSet<i32> = endpoints.iter().copied().collect();
    assert_eq!(
        unique.len(),
        endpoints.len(),
        "left/right endpoints must be unique"
    );
    for (child, parent) in parentage {
        let child = tree_node(nodes, *child)?;
        let parent = tree_node(nodes, *parent)?;
        assert!(
            parent.lft < child.lft && child.rght < parent.rght,
            "a descendant must nest strictly inside its parent"
        );
    }
    for (id, level) in levels {
        assert_eq!(tree_node(nodes, *id)?.level, *level, "unexpected depth");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; inventory template nested sets"]
async fn component_template_inventory_tree_nested_sets_stay_consistent() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let root = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("tree-root-{}", unique_suffix()),
        )
        .await?;
    let mut first_body = Kind::InventoryItem.create_body(
        device,
        &format!("tree-child-a-{}", unique_suffix()),
        &f.seed,
    );
    first_body["parent_id"] = root["id"].clone();
    let first_child = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(first_body),
            StatusCode::CREATED,
        )
        .await?;
    let mut second_body = Kind::InventoryItem.create_body(
        device,
        &format!("tree-child-b-{}", unique_suffix()),
        &f.seed,
    );
    second_body["parent_id"] = root["id"].clone();
    let second_child = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(second_body),
            StatusCode::CREATED,
        )
        .await?;
    let mut leaf_body =
        Kind::InventoryItem.create_body(device, &format!("tree-leaf-{}", unique_suffix()), &f.seed);
    leaf_body["parent_id"] = first_child["id"].clone();
    let leaf = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(leaf_body),
            StatusCode::CREATED,
        )
        .await?;
    let root_id = Uuid::parse_str(&record_id(&root)?)?;
    let first_id = Uuid::parse_str(&record_id(&first_child)?)?;
    let second_id = Uuid::parse_str(&record_id(&second_child)?)?;
    let leaf_id = Uuid::parse_str(&record_id(&leaf)?)?;

    let levels = [(root_id, 0), (first_id, 1), (second_id, 1), (leaf_id, 2)];
    let created = load_tree(&f.db, f.seed.device_type).await?;
    assert_eq!(created.len(), 4);
    assert_nested_set(
        &created,
        &[
            (first_id, root_id),
            (second_id, root_id),
            (leaf_id, first_id),
        ],
        &levels,
    )?;
    assert_siblings_disjoint(&created, first_id, second_id)?;

    // A rename renumbers the tree but keeps every invariant.
    f.admin(
        "PUT",
        &item_path(Kind::InventoryItem, &second_child)?,
        Some(json!({"name": format!("tree-renamed-{}", unique_suffix())})),
        StatusCode::OK,
    )
    .await?;
    let renamed = load_tree(&f.db, f.seed.device_type).await?;
    assert_nested_set(
        &renamed,
        &[
            (first_id, root_id),
            (second_id, root_id),
            (leaf_id, first_id),
        ],
        &levels,
    )?;
    assert_siblings_disjoint(&renamed, first_id, second_id)?;

    // Reparenting the leaf to the root makes three disjoint siblings.
    f.admin(
        "PUT",
        &item_path(Kind::InventoryItem, &leaf)?,
        Some(json!({"parent_id": root["id"]})),
        StatusCode::OK,
    )
    .await?;
    let reparented = load_tree(&f.db, f.seed.device_type).await?;
    assert_nested_set(
        &reparented,
        &[
            (first_id, root_id),
            (second_id, root_id),
            (leaf_id, root_id),
        ],
        &[(root_id, 0), (first_id, 1), (second_id, 1), (leaf_id, 1)],
    )?;
    assert_siblings_disjoint(&reparented, first_id, second_id)?;
    assert_siblings_disjoint(&reparented, first_id, leaf_id)?;
    assert_siblings_disjoint(&reparented, second_id, leaf_id)?;

    // Deleting the leaf keeps the remaining nested set consistent.
    f.admin(
        "DELETE",
        &item_path(Kind::InventoryItem, &leaf)?,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    let remaining = load_tree(&f.db, f.seed.device_type).await?;
    assert_eq!(remaining.len(), 3);
    assert_nested_set(
        &remaining,
        &[(first_id, root_id), (second_id, root_id)],
        &[(root_id, 0), (first_id, 1), (second_id, 1)],
    )?;
    assert_siblings_disjoint(&remaining, first_id, second_id)?;
    Ok(())
}

fn assert_siblings_disjoint(nodes: &[TreeNode], left: Uuid, right: Uuid) -> TestResult {
    let left = tree_node(nodes, left)?;
    let right = tree_node(nodes, right)?;
    assert!(
        left.rght < right.lft || right.rght < left.lft,
        "sibling subtrees must not overlap"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; logical root name uniqueness"]
async fn component_template_root_names_are_unique_under_one_device_type() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let name = format!("root-name-{}", unique_suffix());
    let root = f.create(Kind::InventoryItem, device, &name).await?;
    let root_path = item_path(Kind::InventoryItem, &root)?;
    let root_before = f.admin("GET", &root_path, None, StatusCode::OK).await?;

    // Create conflict: duplicate roots are refused and no row is persisted.
    f.admin(
        "POST",
        &Kind::InventoryItem.collection(),
        Some(Kind::InventoryItem.create_body(device, &name, &f.seed)),
        StatusCode::CONFLICT,
    )
    .await?;
    let roots: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dcim_inventoryitemtemplate \
         WHERE device_type_id=$1 AND parent_id IS NULL AND name=$2",
    )
    .bind(f.seed.device_type)
    .bind(&name)
    .fetch_one(&f.db)
    .await?;
    assert_eq!(roots, 1, "a refused duplicate root must not persist");
    assert_eq!(
        f.admin("GET", &root_path, None, StatusCode::OK).await?,
        root_before
    );

    // Rename conflict: another root cannot take the existing root name.
    let other = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("root-other-{}", unique_suffix()),
        )
        .await?;
    let other_path = item_path(Kind::InventoryItem, &other)?;
    let other_before = f.admin("GET", &other_path, None, StatusCode::OK).await?;
    f.admin(
        "PUT",
        &other_path,
        Some(json!({"name": name})),
        StatusCode::CONFLICT,
    )
    .await?;
    assert_eq!(
        f.admin("GET", &other_path, None, StatusCode::OK).await?,
        other_before
    );

    // Reparent conflict: a child named like the root would become a duplicate
    // root sibling, so the reparent is refused and the row keeps its parent.
    let mut child_body = Kind::InventoryItem.create_body(device, &name, &f.seed);
    child_body["parent_id"] = root["id"].clone();
    let child = f
        .admin(
            "POST",
            &Kind::InventoryItem.collection(),
            Some(child_body),
            StatusCode::CREATED,
        )
        .await?;
    let child_path = item_path(Kind::InventoryItem, &child)?;
    let child_before = f.admin("GET", &child_path, None, StatusCode::OK).await?;
    f.admin(
        "PUT",
        &child_path,
        Some(json!({"parent_id": null})),
        StatusCode::CONFLICT,
    )
    .await?;
    assert_eq!(child_before["parent_id"], root["id"]);
    assert_eq!(
        f.admin("GET", &child_path, None, StatusCode::OK).await?,
        child_before
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; sibling conflict authorization"]
async fn component_template_sibling_conflict_authorizes_only_the_conflict() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let (role, cookie) = seed_restricted_principal(&f).await?;
    let caller = Credential::Cookie(&cookie);

    let taken = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("sibling-taken-{}", unique_suffix()),
        )
        .await?;
    let taken_id = record_id(&taken)?;
    // An unrelated hidden sibling in the same device type must not block a
    // legitimate write; only the exact conflicting row is loaded.
    let unrelated_hidden = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("sibling-unrelated-{}", unique_suffix()),
        )
        .await?;
    let target = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("sibling-target-{}", unique_suffix()),
        )
        .await?;
    let target_path = item_path(Kind::InventoryItem, &target)?;

    for actions in [json!(["READ"]), json!(["UPDATE"]), json!(["CREATE"])] {
        create_policy(&f, policy_payload("allow", actions, None, role)).await?;
    }
    create_policy(
        &f,
        policy_payload_for_resources(
            "deny",
            json!(["READ"]),
            None,
            json!([
                format!("inventory:{taken_id}"),
                format!("inventory:{}", record_id(&unrelated_hidden)?),
            ]),
            role,
        ),
    )
    .await?;

    // A rename onto a hidden sibling must answer Forbidden, not a Conflict that
    // would confirm the name exists.
    f.call(
        "PUT",
        &target_path,
        Some(json!({"name": taken["name"]})),
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let target_after = f.admin("GET", &target_path, None, StatusCode::OK).await?;
    assert_eq!(target_after["name"], target["name"]);

    // The unrelated hidden row does not participate in the predicate.
    let renamed = f
        .call(
            "PUT",
            &target_path,
            Some(json!({"name": format!("sibling-free-{}", unique_suffix())})),
            caller,
            StatusCode::OK,
        )
        .await?;
    assert_ne!(renamed["name"], target["name"]);

    // A readable conflict still reports the ordinary Conflict.
    let readable_conflict = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("sibling-readable-{}", unique_suffix()),
        )
        .await?;
    let second_target = f
        .create(
            Kind::InventoryItem,
            device,
            &format!("sibling-second-{}", unique_suffix()),
        )
        .await?;
    f.call(
        "PUT",
        &item_path(Kind::InventoryItem, &second_target)?,
        Some(json!({"name": readable_conflict["name"]})),
        caller,
        StatusCode::CONFLICT,
    )
    .await?;

    // Create keeps the create-scoped PAT rule: no READ grant on the retained
    // conflicting row is demanded, and the duplicate is still refused.
    let create_name = format!("sibling-create-{}", unique_suffix());
    f.create(Kind::InventoryItem, device, &create_name).await?;
    let body = Kind::InventoryItem.create_body(device, &create_name, &f.seed);
    f.call(
        "POST",
        &Kind::InventoryItem.collection(),
        Some(body),
        caller,
        StatusCode::CONFLICT,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; capped delete blocker examples"]
async fn component_template_delete_blockers_withhold_capped_unprovable_examples() -> TestResult {
    let f = Fixture::start().await?;
    let device = Parent::Device(f.seed.device_type);
    let port = f
        .create(
            Kind::PowerPort,
            device,
            &format!("agg-port-{}", unique_suffix()),
        )
        .await?;
    // The guard orders examples by display label (the template name, natural
    // sort) and caps them at three, so the denied fourth referrer sorts last
    // and never reaches the example set.
    let prefix = format!("agg-outlet-{}", unique_suffix());
    let mut outlets = Vec::new();
    for index in 1..=4 {
        let mut body = Kind::PowerOutlet.create_body(device, &format!("{prefix}-{index}"), &f.seed);
        body["power_port_id"] = port["id"].clone();
        let outlet = f
            .admin(
                "POST",
                &Kind::PowerOutlet.collection(),
                Some(body),
                StatusCode::CREATED,
            )
            .await?;
        outlets.push(outlet);
    }
    let hidden_name = format!("{prefix}-4");
    let hidden_id = record_id(&outlets[3])?;

    // Global READ and DELETE, with READ denied for exactly one referrer.
    let (role, cookie) = seed_restricted_principal(&f).await?;
    create_policy(&f, policy_payload("allow", json!(["READ"]), None, role)).await?;
    create_policy(&f, policy_payload("allow", json!(["DELETE"]), None, role)).await?;
    create_policy(
        &f,
        policy_payload_for_resources(
            "deny",
            json!(["READ"]),
            None,
            json!([format!("inventory:{hidden_id}")]),
            role,
        ),
    )
    .await?;
    let caller = Credential::Cookie(&cookie);

    // The scoped deny also denies the collection-wide policy trace, which has
    // no concrete resource id. The individual item remains forbidden.
    f.call(
        "GET",
        &item_path(Kind::PowerOutlet, &outlets[3])?,
        None,
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "GET",
        &format!("{}?search={hidden_name}", Kind::PowerOutlet.collection()),
        None,
        caller,
        StatusCode::FORBIDDEN,
    )
    .await?;
    f.call(
        "GET",
        &item_path(Kind::PowerOutlet, &outlets[0])?,
        None,
        caller,
        StatusCode::OK,
    )
    .await?;

    // The guard counts four references but can prove only the capped examples,
    // so the whole projection is withheld instead of revealing a total.
    let port_path = item_path(Kind::PowerPort, &port)?;
    let blocked = f
        .call("DELETE", &port_path, None, caller, StatusCode::CONFLICT)
        .await?;
    assert_eq!(blocked["code"], "CONFLICT");
    assert_eq!(blocked["reason_code"], "DELETE_BLOCKED");
    assert!(
        blocked.get("details").is_none(),
        "unprovable capped examples must not be projected"
    );
    let serialized = blocked.to_string();
    for index in 1..=4 {
        let name = format!("{prefix}-{index}");
        assert!(!serialized.contains(&name), "blocker name leaked: {name}");
        assert!(!serialized.contains(&record_id(&outlets[index - 1])?));
    }
    f.admin("GET", &port_path, None, StatusCode::OK).await?;
    let references: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_poweroutlettemplate WHERE power_port_id=$1")
            .bind(Uuid::parse_str(&record_id(&port)?)?)
            .fetch_one(&f.db)
            .await?;
    assert_eq!(references, 4, "the guard must count every reference");
    Ok(())
}
