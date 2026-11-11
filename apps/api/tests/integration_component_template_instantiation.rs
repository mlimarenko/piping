//! Native component-template instantiation through actual device and module
//! creation.
//!
//! The canonical instantiator is the `dcim_instantiate_hardware_components`
//! AFTER INSERT trigger extended by migration 0013. These controls create real
//! hardware through the native REST consumers and inspect the persisted graph,
//! so they use the owned PostgreSQL fixture rather than a mock.
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
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

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
        expected: StatusCode,
    ) -> TestResult<Value> {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("cookie", &self.cookie)
            .header("content-type", "application/json")
            .body(Body::from(
                payload.map(|value| value.to_string()).unwrap_or_default(),
            ))?;
        let response = self.app.clone().oneshot(request).await?;
        assert_eq!(response.status(), expected, "{path}");
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok(if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        })
    }
}

async fn site(db: &PgPool) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
        .bind(id).bind(format!("tpl-{id}")).execute(db).await?;
    Ok(id)
}

async fn insert_interface_template(
    db: &PgPool,
    device_type: Option<Uuid>,
    module_type: Option<Uuid>,
    name: &str,
    bridge: Option<Uuid>,
) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_interfacetemplate (id,name,label,description,_name,type,mgmt_only,device_type_id,module_type_id,poe_mode,poe_type,enabled,bridge_id,rf_role) VALUES ($1,$2,'','',$2,'1000base-t',false,$3,$4,NULL,NULL,true,$5,NULL)")
        .bind(id).bind(name).bind(device_type).bind(module_type).bind(bridge)
        .execute(db).await?;
    Ok(id)
}

async fn insert_console_port_template(
    db: &PgPool,
    device_type: Option<Uuid>,
    module_type: Option<Uuid>,
    name: &str,
) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_consoleporttemplate (id,name,label,description,type,device_type_id,module_type_id) VALUES ($1,$2,'','','rj-45',$3,$4)")
        .bind(id).bind(name).bind(device_type).bind(module_type)
        .execute(db).await?;
    Ok(id)
}

async fn insert_inventory_item_template(
    db: &PgPool,
    device_type: Uuid,
    name: &str,
    parent: Option<Uuid>,
    component: Option<(Uuid, Uuid)>,
) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    let (component_type, component_id) = match component {
        Some((component_type, component_id)) => (Some(component_type), Some(component_id)),
        None => (None, None),
    };
    sqlx::query("INSERT INTO dcim_inventoryitemtemplate (id,name,label,description,component_id,part_id,lft,rght,tree_id,level,component_type_id,device_type_id,manufacturer_id,parent_id,role_id) VALUES ($1,$2,'','',$3,'',0,0,0,0,$4,$5,NULL,$6,NULL)")
        .bind(id).bind(name).bind(component_id).bind(component_type).bind(device_type).bind(parent)
        .execute(db).await?;
    Ok(id)
}

async fn cleanup(
    db: &PgPool,
    site_id: Uuid,
    manufacturer_id: Uuid,
    type_names: &[String],
) -> TestResult {
    sqlx::query("DELETE FROM dcim_portmapping WHERE device_id IN (SELECT id FROM dcim_device WHERE site_id=$1)").bind(site_id).execute(db).await?;
    sqlx::query("DELETE FROM dcim_inventoryitem WHERE device_id IN (SELECT id FROM dcim_device WHERE site_id=$1) AND parent_id IS NOT NULL").bind(site_id).execute(db).await?;
    sqlx::query("DELETE FROM dcim_inventoryitem WHERE device_id IN (SELECT id FROM dcim_device WHERE site_id=$1)").bind(site_id).execute(db).await?;
    for table in [
        "dcim_interface",
        "dcim_consoleport",
        "dcim_consoleserverport",
        "dcim_poweroutlet",
        "dcim_powerport",
        "dcim_devicebay",
        "dcim_frontport",
        "dcim_rearport",
        "dcim_coolingintake",
        "dcim_coolingoutflow",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE device_id IN (SELECT id FROM dcim_device WHERE site_id=$1)"
        )))
        .bind(site_id)
        .execute(db)
        .await?;
    }
    sqlx::query("DELETE FROM dcim_modulebay WHERE device_id IN (SELECT id FROM dcim_device WHERE site_id=$1) AND module_id IS NOT NULL").bind(site_id).execute(db).await?;
    sqlx::query(
        "DELETE FROM dcim_module WHERE device_id IN (SELECT id FROM dcim_device WHERE site_id=$1)",
    )
    .bind(site_id)
    .execute(db)
    .await?;
    sqlx::query("DELETE FROM dcim_modulebay WHERE device_id IN (SELECT id FROM dcim_device WHERE site_id=$1)").bind(site_id).execute(db).await?;
    sqlx::query("DELETE FROM dcim_device WHERE site_id=$1")
        .bind(site_id)
        .execute(db)
        .await?;
    for table in [
        "dcim_porttemplatemapping",
        "dcim_inventoryitemtemplate",
        "dcim_devicebaytemplate",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE device_type_id IN (SELECT id FROM dcim_devicetype WHERE manufacturer_id=$1 AND model = ANY($2))")))
            .bind(manufacturer_id).bind(type_names).execute(db).await?;
    }
    sqlx::query("UPDATE dcim_interfacetemplate SET bridge_id=NULL WHERE device_type_id IN (SELECT id FROM dcim_devicetype WHERE manufacturer_id=$1 AND model = ANY($2))")
        .bind(manufacturer_id).bind(type_names).execute(db).await?;
    sqlx::query("DELETE FROM dcim_interfacetemplate WHERE module_type_id IN (SELECT id FROM dcim_moduletype WHERE manufacturer_id=$1 AND model = ANY($2))")
        .bind(manufacturer_id).bind(type_names).execute(db).await?;
    sqlx::query("DELETE FROM dcim_consoleporttemplate WHERE module_type_id IN (SELECT id FROM dcim_moduletype WHERE manufacturer_id=$1 AND model = ANY($2))")
        .bind(manufacturer_id).bind(type_names).execute(db).await?;
    sqlx::query("DELETE FROM dcim_interfacetemplate WHERE device_type_id IN (SELECT id FROM dcim_devicetype WHERE manufacturer_id=$1 AND model = ANY($2))")
        .bind(manufacturer_id).bind(type_names).execute(db).await?;
    for table in [
        "dcim_consoleporttemplate",
        "dcim_consoleserverporttemplate",
        "dcim_poweroutlettemplate",
        "dcim_powerporttemplate",
        "dcim_frontporttemplate",
        "dcim_rearporttemplate",
        "dcim_modulebaytemplate",
        "dcim_coolingintaketemplate",
        "dcim_coolingoutflowtemplate",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE device_type_id IN (SELECT id FROM dcim_devicetype WHERE manufacturer_id=$1 AND model = ANY($2))")))
            .bind(manufacturer_id).bind(type_names).execute(db).await?;
    }
    sqlx::query("DELETE FROM dcim_moduletype WHERE manufacturer_id=$1 AND model = ANY($2) AND NOT EXISTS (SELECT 1 FROM dcim_module WHERE dcim_module.module_type_id=dcim_moduletype.id)")
        .bind(manufacturer_id).bind(type_names).execute(db).await?;
    sqlx::query("DELETE FROM dcim_devicetype WHERE manufacturer_id=$1 AND model = ANY($2)")
        .bind(manufacturer_id)
        .bind(type_names)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site_id)
        .execute(db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL and Redis fixtures; native component-template instantiation"]
async fn device_and_module_creation_instantiate_component_templates() -> TestResult {
    let http = HttpFixture::start().await?;
    let db = build_test_db_pool().await?;
    let site_id = site(&db).await?;
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .fetch_one(&db)
            .await?;
    let role: Uuid = sqlx::query_scalar("SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1")
        .fetch_one(&db)
        .await?;
    let model = format!("tpl-device-{}", Uuid::now_v7());
    let kind = http
        .call(
            Method::POST,
            "/api/v1/inventory/device_types",
            Some(json!({"manufacturer_id":manufacturer,"model":model,"slug":model,"subdevice_role":"parent"})),
            StatusCode::CREATED,
        )
        .await?;
    let kind_id = Uuid::parse_str(kind["id"].as_str().ok_or("device type id")?)?;
    let module_model = format!("tpl-module-{}", Uuid::now_v7());
    let module_type = http
        .call(
            Method::POST,
            "/api/v1/inventory/module-types",
            Some(json!({"manufacturer_id":manufacturer,"model":module_model})),
            StatusCode::CREATED,
        )
        .await?;
    let module_type_id = Uuid::parse_str(module_type["id"].as_str().ok_or("module type id")?)?;

    let device_type_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='interfacetemplate'",
    )
    .fetch_one(&db)
    .await?;
    let power_port_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='powerporttemplate'",
    )
    .fetch_one(&db)
    .await?;

    let runtime_interface_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='interface'",
    )
    .fetch_one(&db)
    .await?;
    let runtime_powerport_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='powerport'",
    )
    .fetch_one(&db)
    .await?;
    let runtime_coolingintake_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='coolingintake'",
    )
    .fetch_one(&db)
    .await?;

    let bridge_template = insert_interface_template(&db, Some(kind_id), None, "br0", None).await?;
    let eth_template =
        insert_interface_template(&db, Some(kind_id), None, "eth0", Some(bridge_template)).await?;
    insert_console_port_template(&db, Some(kind_id), None, "con0").await?;
    let power_port_template = {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_powerporttemplate (id,name,label,description,type,maximum_draw,allocated_draw,device_type_id,module_type_id) VALUES ($1,'psu0','','','iec-60320-c14',500,300,$2,NULL)")
            .bind(id).bind(kind_id).execute(&db).await?;
        id
    };
    sqlx::query("INSERT INTO dcim_poweroutlettemplate (id,name,label,description,type,feed_leg,device_type_id,power_port_id,module_type_id,color) VALUES ($1,'out0','','','iec-60320-c13',NULL,$2,$3,NULL,'aa1409')")
        .bind(Uuid::now_v7()).bind(kind_id).bind(power_port_template).execute(&db).await?;
    let cooling_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='coolingintaketemplate'",
    )
    .fetch_one(&db)
    .await?;
    let cooling_template = {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_coolingintaketemplate (id,name,label,description,type,diameter,diameter_unit,max_flow,max_flow_unit,device_type_id) VALUES ($1,'cool0','','','uqd',NULL,NULL,NULL,NULL,$2)")
            .bind(id).bind(kind_id).execute(&db).await?;
        id
    };
    let rear_template = {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_rearporttemplate (id,name,label,description,type,positions,device_type_id,color,module_type_id) VALUES ($1,'rear0','','','8p8c',2,$2,'000000',NULL)")
            .bind(id).bind(kind_id).execute(&db).await?;
        id
    };
    let front_template = {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO dcim_frontporttemplate (id,name,label,description,type,device_type_id,color,positions,module_type_id) VALUES ($1,'front0','','','8p8c',$2,'000000',2,NULL)")
            .bind(id).bind(kind_id).execute(&db).await?;
        id
    };
    sqlx::query("INSERT INTO dcim_porttemplatemapping (id,front_port_position,rear_port_position,device_type_id,front_port_id,rear_port_id) VALUES ($1,1,2,$2,$3,$4)")
        .bind(Uuid::now_v7()).bind(kind_id).bind(front_template).bind(rear_template)
        .execute(&db).await?;
    sqlx::query("INSERT INTO dcim_devicebaytemplate (id,name,label,description,device_type_id) VALUES ($1,'bay0','','',$2)")
        .bind(Uuid::now_v7()).bind(kind_id).execute(&db).await?;
    let disabled_bay_template = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_devicebaytemplate (id,name,label,description,device_type_id,enabled) VALUES ($1,'bay-disabled','','',$2,false)")
        .bind(disabled_bay_template).bind(kind_id).execute(&db).await?;
    // Two bays at the same position expose duplicate resolved module names.
    sqlx::query("INSERT INTO dcim_modulebaytemplate (name,label,description,device_type_id,position,enabled) VALUES ('slot','','',$1,'A',true)")
        .bind(kind_id).execute(&db).await?;
    sqlx::query("INSERT INTO dcim_modulebaytemplate (name,label,description,device_type_id,position,enabled) VALUES ('slot2','','',$1,'A',true)")
        .bind(kind_id).execute(&db).await?;
    let chassis_template = insert_inventory_item_template(
        &db,
        kind_id,
        "chassis",
        None,
        Some((device_type_ctor, eth_template)),
    )
    .await?;
    insert_inventory_item_template(
        &db,
        kind_id,
        "fan",
        Some(chassis_template),
        Some((power_port_ctor, power_port_template)),
    )
    .await?;
    insert_inventory_item_template(
        &db,
        kind_id,
        "cooler",
        None,
        Some((cooling_ctor, cooling_template)),
    )
    .await?;
    let module_bridge =
        insert_interface_template(&db, None, Some(module_type_id), "m-{module}", None).await?;
    insert_interface_template(
        &db,
        None,
        Some(module_type_id),
        "b-{module}",
        Some(module_bridge),
    )
    .await?;
    insert_console_port_template(&db, None, Some(module_type_id), "c-{module}").await?;

    let device_name = format!("tpl-device-{site_id}");
    let device = http
        .call(
            Method::POST,
            "/api/v1/inventory/devices",
            Some(json!({"name":device_name,"site_id":site_id,"role_id":role,"device_type_id":kind_id,"status":"active"})),
            StatusCode::CREATED,
        )
        .await?;
    let device_id = Uuid::parse_str(device["id"].as_str().ok_or("device id")?)?;
    let device_bays: Vec<(Uuid, String, bool)> = sqlx::query_as(
        "SELECT id,name,enabled FROM dcim_devicebay WHERE device_id=$1 ORDER BY name",
    )
    .bind(device_id)
    .fetch_all(&db)
    .await?;
    assert_eq!(device_bays.len(), 2);
    assert_eq!(
        (device_bays[0].1.as_str(), device_bays[0].2),
        ("bay-disabled", false)
    );
    assert_eq!(
        (device_bays[1].1.as_str(), device_bays[1].2),
        ("bay0", true)
    );
    sqlx::query("UPDATE dcim_devicebaytemplate SET enabled=true WHERE id=$1")
        .bind(disabled_bay_template)
        .execute(&db)
        .await?;
    let unchanged: Vec<(Uuid, String, bool)> = sqlx::query_as(
        "SELECT id,name,enabled FROM dcim_devicebay WHERE device_id=$1 ORDER BY name",
    )
    .bind(device_id)
    .fetch_all(&db)
    .await?;
    assert_eq!(
        unchanged, device_bays,
        "template updates must not rematerialize bays"
    );
    let counters: (i64, i64) =
        sqlx::query_as("SELECT console_port_count, interface_count FROM dcim_device WHERE id=$1")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(counters, (1, 2), "device template counters");

    let child_model = format!("tpl-child-{}", Uuid::now_v7());
    let child_type = http
        .call(
            Method::POST,
            "/api/v1/inventory/device_types",
            Some(json!({"manufacturer_id":manufacturer,"model":child_model,"slug":child_model,"subdevice_role":"child"})),
            StatusCode::CREATED,
        )
        .await?;
    let child_type_id = Uuid::parse_str(child_type["id"].as_str().ok_or("child type id")?)?;
    let mut children = Vec::new();
    for index in 0..2 {
        let child = http
            .call(
                Method::POST,
                "/api/v1/inventory/devices",
                Some(json!({"name":format!("tpl-child-{index}-{site_id}"),"site_id":site_id,"role_id":role,"device_type_id":child_type_id,"status":"active"})),
                StatusCode::CREATED,
            )
            .await?;
        children.push(Uuid::parse_str(child["id"].as_str().ok_or("child id")?)?);
    }
    let collection = "/api/v1/inventory/device-bays";
    let default_bay = http
        .call(
            Method::POST,
            collection,
            Some(json!({"device_id":device_id,"name":"native-default"})),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(default_bay["enabled"], true);
    assert_eq!(default_bay["occupied"], false);
    let disabled = http
        .call(
            Method::POST,
            collection,
            Some(json!({"device_id":device_id,"name":"native-disabled","enabled":false})),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["occupied"], true);
    assert!(disabled["installed_device_id"].is_null());
    let disabled_path = format!("{collection}/{}", disabled["id"].as_str().ok_or("bay id")?);
    let disabled_preserved = http
        .call(
            Method::PUT,
            &disabled_path,
            Some(json!({"description":"still disabled"})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(disabled_preserved["enabled"], false);
    assert_eq!(disabled_preserved["occupied"], true);

    http.call(Method::POST, collection, Some(json!({"device_id":device_id,"name":"rejected-install","enabled":false,"installed_device_id":children[0]})), StatusCode::BAD_REQUEST).await?;
    let rejected_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dcim_devicebay WHERE device_id=$1 AND name='rejected-install'",
    )
    .bind(device_id)
    .fetch_one(&db)
    .await?;
    assert_eq!(rejected_count, 0, "failed create must not persist a bay");
    http.call(Method::PUT, &disabled_path, Some(json!({"name":"rejected-rename","description":"must roll back","installed_device_id":children[0]})), StatusCode::BAD_REQUEST).await?;
    assert_eq!(
        http.call(Method::GET, &disabled_path, None, StatusCode::OK)
            .await?,
        disabled_preserved
    );
    let direct_error = sqlx::query(
        "UPDATE dcim_devicebay SET name='direct-rejected', installed_device_id=$2 WHERE id=$1",
    )
    .bind(Uuid::parse_str(disabled["id"].as_str().ok_or("bay id")?)?)
    .bind(children[0])
    .execute(&db)
    .await
    .expect_err("disabled installation must fail at the database boundary");
    assert_eq!(
        direct_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );
    assert_eq!(
        http.call(Method::GET, &disabled_path, None, StatusCode::OK)
            .await?,
        disabled_preserved
    );

    let default_path = format!(
        "{collection}/{}",
        default_bay["id"].as_str().ok_or("bay id")?
    );
    let installed = http
        .call(
            Method::PUT,
            &default_path,
            Some(json!({"installed_device_id":children[0]})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(installed["installed_device_id"], json!(children[0]));
    assert_eq!(installed["occupied"], true);
    let retained = http
        .call(
            Method::PUT,
            &default_path,
            Some(json!({"enabled":false})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(retained["enabled"], false);
    assert_eq!(retained["installed_device_id"], json!(children[0]));
    assert_eq!(retained["occupied"], true);
    let same_child = http
        .call(
            Method::PUT,
            &default_path,
            Some(json!({"installed_device_id":children[0]})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(same_child["installed_device_id"], json!(children[0]));
    http.call(Method::PUT, &default_path, Some(json!({"name":"rejected-replacement","description":"must roll back","installed_device_id":children[1]})), StatusCode::BAD_REQUEST).await?;
    assert_eq!(
        http.call(Method::GET, &default_path, None, StatusCode::OK)
            .await?,
        same_child
    );
    let replaced = http
        .call(
            Method::PUT,
            &default_path,
            Some(json!({"enabled":true,"installed_device_id":children[1]})),
            StatusCode::OK,
        )
        .await?;
    assert_eq!(replaced["enabled"], true);
    assert_eq!(replaced["installed_device_id"], json!(children[1]));
    let uninstalled = http
        .call(
            Method::PUT,
            &default_path,
            Some(json!({"enabled":false,"installed_device_id":null})),
            StatusCode::OK,
        )
        .await?;
    assert!(uninstalled["installed_device_id"].is_null());
    assert_eq!(uninstalled["occupied"], true);
    http.call(
        Method::PUT,
        &default_path,
        Some(json!({"installed_device_id":children[0]})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    assert_eq!(
        http.call(Method::GET, &default_path, None, StatusCode::OK)
            .await?,
        uninstalled
    );
    http.call(
        Method::PUT,
        &default_path,
        Some(json!({"enabled":null,"description":"must roll back"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    assert_eq!(
        http.call(Method::GET, &default_path, None, StatusCode::OK)
            .await?,
        uninstalled
    );
    http.call(
        Method::POST,
        collection,
        Some(json!({"device_id":device_id,"name":"self-install","installed_device_id":device_id})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let self_error = sqlx::query(
        "UPDATE dcim_devicebay SET enabled=true, installed_device_id=device_id WHERE id=$1",
    )
    .bind(Uuid::parse_str(
        default_bay["id"].as_str().ok_or("bay id")?,
    )?)
    .execute(&db)
    .await
    .expect_err("self-install must fail at the database boundary");
    assert_eq!(
        self_error
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );

    let (console_type, console_mark): (Option<String>, bool) = sqlx::query_as(
        "SELECT type, mark_connected FROM dcim_consoleport WHERE device_id=$1 AND name='con0'",
    )
    .bind(device_id)
    .fetch_one(&db)
    .await?;
    assert_eq!(console_type.as_deref(), Some("rj-45"));
    assert!(!console_mark);
    let (max_draw, alloc_draw): (Option<i32>, Option<i32>) =
        sqlx::query_as("SELECT maximum_draw, allocated_draw FROM dcim_powerport WHERE device_id=$1 AND name='psu0'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!((max_draw, alloc_draw), (Some(500), Some(300)));
    let (outlet_port, outlet_status, outlet_color): (Option<Uuid>, String, String) =
        sqlx::query_as("SELECT power_port_id, status, color FROM dcim_poweroutlet WHERE device_id=$1 AND name='out0'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    let power_port_id: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_powerport WHERE device_id=$1 AND name='psu0'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(outlet_port, Some(power_port_id));
    assert_eq!(
        (outlet_status.as_str(), outlet_color.as_str()),
        ("enabled", "aa1409")
    );

    let bridge_id: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_interface WHERE device_id=$1 AND name='br0'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    let (eth_id, eth_bridge): (Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT id, bridge_id FROM dcim_interface WHERE device_id=$1 AND name='eth0'",
    )
    .bind(device_id)
    .fetch_one(&db)
    .await?;
    assert_eq!(eth_bridge, Some(bridge_id));

    let rear_id: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_rearport WHERE device_id=$1 AND name='rear0'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    let front_id: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_frontport WHERE device_id=$1 AND name='front0'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    let (map_front, map_rear): (Uuid, Uuid) =
        sqlx::query_as("SELECT front_port_id, rear_port_id FROM dcim_portmapping WHERE device_id=$1 AND front_port_position=1")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(map_front, front_id);
    assert_eq!(map_rear, rear_id);

    let (chassis_id, chassis_component, chassis_component_type): (Uuid, Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT id, component_id, component_type_id FROM dcim_inventoryitem WHERE device_id=$1 AND name='chassis'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(chassis_component, Some(eth_id));
    // component_type_id is the instantiated runtime object type, not the
    // template type supplied as the template association input above.
    assert_eq!(chassis_component_type, Some(runtime_interface_ctor));
    let (fan_parent, fan_component, fan_component_type): (Option<Uuid>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT parent_id, component_id, component_type_id FROM dcim_inventoryitem WHERE device_id=$1 AND name='fan'",
    )
    .bind(device_id)
    .fetch_one(&db)
    .await?;
    assert_eq!(fan_parent, Some(chassis_id));
    assert_eq!(fan_component, Some(power_port_id));
    assert_eq!(fan_component_type, Some(runtime_powerport_ctor));
    let (cooler_component, cooler_component_type): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT component_id, component_type_id FROM dcim_inventoryitem WHERE device_id=$1 AND name='cooler'",
    )
    .bind(device_id)
    .fetch_one(&db)
    .await?;
    let cooling_intake_id: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_coolingintake WHERE device_id=$1 AND name='cool0'")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(cooler_component, Some(cooling_intake_id));
    assert_eq!(cooler_component_type, Some(runtime_coolingintake_ctor));

    let bays: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM dcim_modulebay WHERE device_id=$1 AND module_id IS NULL ORDER BY name")
            .bind(device_id)
            .fetch_all(&db)
            .await?;
    assert_eq!(bays.len(), 2);
    let installed = http
        .call(
            Method::POST,
            "/api/v1/inventory/modules",
            Some(json!({"device_id":device_id,"module_bay_id":bays[0].0,"module_type_id":module_type_id})),
            StatusCode::CREATED,
        )
        .await?;
    let module_id = Uuid::parse_str(installed["id"].as_str().ok_or("module id")?)?;
    let module_interface_id: Uuid =
        sqlx::query_scalar("SELECT id FROM dcim_interface WHERE module_id=$1 AND name='m-A'")
            .bind(module_id)
            .fetch_one(&db)
            .await?;
    let module_bridge: Option<Uuid> = sqlx::query_scalar(
        "SELECT bridge_id FROM dcim_interface WHERE module_id=$1 AND name='b-A'",
    )
    .bind(module_id)
    .fetch_one(&db)
    .await?;
    assert_eq!(module_bridge, Some(module_interface_id));
    let module_console: String =
        sqlx::query_scalar("SELECT name FROM dcim_consoleport WHERE module_id=$1")
            .bind(module_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(module_console, "c-A");
    let counters: (i64, i64) =
        sqlx::query_as("SELECT console_port_count, interface_count FROM dcim_device WHERE id=$1")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(counters, (2, 4), "module template counters");

    let duplicate = http
        .call(
            Method::POST,
            "/api/v1/inventory/modules",
            Some(json!({"device_id":device_id,"module_bay_id":bays[1].0,"module_type_id":module_type_id})),
            StatusCode::CONFLICT,
        )
        .await;
    duplicate?;
    let second_module: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_module WHERE module_bay_id=$1")
            .bind(bays[1].0)
            .fetch_one(&db)
            .await?;
    assert_eq!(second_module, 0, "failed install must roll back atomically");
    let after_failure: (i64, i64) =
        sqlx::query_as("SELECT console_port_count, interface_count FROM dcim_device WHERE id=$1")
            .bind(device_id)
            .fetch_one(&db)
            .await?;
    assert_eq!(after_failure, counters, "failed install changed counters");

    let cycle_model = format!("tpl-cycle-{}", Uuid::now_v7());
    let cycle_kind = http
        .call(
            Method::POST,
            "/api/v1/inventory/device_types",
            Some(json!({"manufacturer_id":manufacturer,"model":cycle_model,"slug":cycle_model})),
            StatusCode::CREATED,
        )
        .await?;
    let cycle_kind_id = Uuid::parse_str(cycle_kind["id"].as_str().ok_or("cycle type id")?)?;
    let first_item = insert_inventory_item_template(&db, cycle_kind_id, "a", None, None).await?;
    let second_item =
        insert_inventory_item_template(&db, cycle_kind_id, "b", Some(first_item), None).await?;
    sqlx::query("UPDATE dcim_inventoryitemtemplate SET parent_id=$2 WHERE id=$1")
        .bind(first_item)
        .bind(second_item)
        .execute(&db)
        .await?;
    let cycle_name = format!("tpl-cycle-device-{site_id}");
    http.call(
        Method::POST,
        "/api/v1/inventory/devices",
        Some(json!({"name":cycle_name,"site_id":site_id,"role_id":role,"device_type_id":cycle_kind_id,"status":"active"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let rolled_back: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dcim_device WHERE name=$1")
        .bind(&cycle_name)
        .fetch_one(&db)
        .await?;
    assert_eq!(
        rolled_back, 0,
        "cyclic hierarchy must roll the device create back"
    );

    // An inherited module bay position that still carries {module} must fail
    // closed instead of producing an unresolved component name.
    let inherited_bay = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,enabled,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'inherited','','{module}',true,'',$2,'{}',0,1,2,0)")
        .bind(inherited_bay)
        .bind(device_id)
        .execute(&db)
        .await?;
    http.call(
        Method::POST,
        "/api/v1/inventory/modules",
        Some(json!({"device_id":device_id,"module_bay_id":inherited_bay,"module_type_id":module_type_id})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let unresolved_module: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_module WHERE module_bay_id=$1")
            .bind(inherited_bay)
            .fetch_one(&db)
            .await?;
    assert_eq!(
        unresolved_module, 0,
        "unresolved inherited bay position must roll the module install back"
    );

    // DeviceBayTemplate and InventoryItemTemplate are model identities, not
    // allowed typed component targets. A typed link to one must fail closed.
    let invalid_model = format!("tpl-invalid-{}", Uuid::now_v7());
    let invalid_kind = http
        .call(
            Method::POST,
            "/api/v1/inventory/device_types",
            Some(
                json!({"manufacturer_id":manufacturer,"model":invalid_model,"slug":invalid_model}),
            ),
            StatusCode::CREATED,
        )
        .await?;
    let invalid_kind_id = Uuid::parse_str(invalid_kind["id"].as_str().ok_or("invalid type id")?)?;
    let invalid_bay = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_devicebaytemplate (id,name,label,description,device_type_id) VALUES ($1,'invalid-bay','','',$2)")
        .bind(invalid_bay)
        .bind(invalid_kind_id)
        .execute(&db)
        .await?;
    let bay_ctor: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='devicebaytemplate'",
    )
    .fetch_one(&db)
    .await?;
    insert_inventory_item_template(
        &db,
        invalid_kind_id,
        "invalid",
        None,
        Some((bay_ctor, invalid_bay)),
    )
    .await?;
    let invalid_name = format!("tpl-invalid-device-{site_id}");
    http.call(
        Method::POST,
        "/api/v1/inventory/devices",
        Some(json!({"name":invalid_name,"site_id":site_id,"role_id":role,"device_type_id":invalid_kind_id,"status":"active"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let invalid_rolled_back: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_device WHERE name=$1")
            .bind(&invalid_name)
            .fetch_one(&db)
            .await?;
    assert_eq!(
        invalid_rolled_back, 0,
        "invalid typed component target must roll the device create back"
    );

    // Explicit materialization graphs own their component identities.
    let explicit_device = Uuid::now_v7();
    let mut tx = db.begin().await?;
    sqlx::query("SELECT set_config('piping.hardware_materialization','explicit_graph',true)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) VALUES ($1,$2,'','active','',$3,$4,$5,'','{}',0,0,0,0,0,0,0,0,0,0)")
        .bind(explicit_device)
        .bind(format!("tpl-explicit-{site_id}"))
        .bind(role)
        .bind(kind_id)
        .bind(site_id)
        .execute(&mut *tx)
        .await?;
    let explicit_interfaces: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_interface WHERE device_id=$1")
            .bind(explicit_device)
            .fetch_one(&mut *tx)
            .await?;
    let explicit_bays: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dcim_modulebay WHERE device_id=$1")
            .bind(explicit_device)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(explicit_interfaces, 0);
    assert_eq!(explicit_bays, 0);
    tx.rollback().await?;

    let type_names = vec![
        model.clone(),
        module_model,
        cycle_model,
        invalid_model,
        child_model,
    ];
    cleanup(&db, site_id, manufacturer, &type_names).await?;
    Ok(())
}
