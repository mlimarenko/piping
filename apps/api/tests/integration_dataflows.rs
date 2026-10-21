//! Native PostgreSQL/REST behavior: compact selectors, defaults, realms and auth.
mod common;
use axum::http::StatusCode;
use piping_backend::{
    app::AppState,
    contexts::access_control::domain::{Action, ResourceType},
};
use serde_json::{Value, json};
use uuid::Uuid;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis"]
async fn projections_depend_on_page_references_not_inventory_size() -> TestResult {
    use sqlx::{Postgres, migrate::MigrateDatabase};
    let mut url = url::Url::parse(&common::database_url())?;
    url.set_path(&format!("/pwa120_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let result = exercise_page_projections(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    result?;
    cleanup?;
    Ok(())
}

async fn exercise_page_projections(url: &str) -> TestResult {
    let state = projection_app_state(url).await?;
    let pool = &state.db_pool;
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    let denied = user_cookie(&state, &app, false, true, None).await?;
    let refs = seed_projection_references(pool, &app, &cookie).await?;
    let expected = create_projection_flows(&app, &cookie, &refs).await?;
    grow_unrelated_inventory(pool, &refs).await?;
    assert_projection_page_and_get(&app, &cookie, &expected).await?;
    assert_projection_filters(&app, &cookie, refs.host, &expected).await?;
    assert_projection_permissions(&app, &denied, &expected).await?;
    assert_projection_validation_caps(&app, &cookie, refs.host, &expected).await?;
    assert_projection_missing_and_null_labels(&state, refs.host, &expected).await?;
    drop(app);
    pool.close().await;
    // A closed pool proves empty pages never acquire a connection.
    assert!(
        state
            .repositories
            .dataflows
            .projections(&[])
            .await?
            .is_empty()
    );
    Ok(())
}

async fn projection_app_state(url: &str) -> TestResult<AppState> {
    use std::sync::Arc;
    let pool = sqlx::PgPool::connect(url).await?;
    sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await?
    .run(&pool)
    .await?;
    let mut settings = piping_backend::config::load_settings()?;
    settings.database.url = url.into();
    let mut redis_url = url::Url::parse(&common::redis_url())?;
    redis_url.set_path("/14");
    settings.redis.url = redis_url.to_string();
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    common::configure_test_cipher(&mut settings);
    common::ensure_test_admin(&pool, &settings.auth.password_pepper).await?;
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    Ok(AppState::build(Arc::new(settings), pool, redis).await?)
}

type ProjectionCase = (Value, Value, &'static str, &'static str);
struct ProjectionReferences {
    host: Uuid,
    site: Uuid,
    role: Uuid,
    model: Uuid,
    cases: Vec<ProjectionCase>,
}

async fn seed_projection_references(
    pool: &sqlx::PgPool,
    app: &axum::Router,
    cookie: &str,
) -> TestResult<ProjectionReferences> {
    let (host, _) = common::seed_device(pool, "projection").await?;
    let (site, role, model): (Uuid, Uuid, Uuid) =
        sqlx::query_as("SELECT site_id,role_id,device_type_id FROM dcim_device WHERE id=$1")
            .bind(host)
            .fetch_one(pool)
            .await?;
    sqlx::query("UPDATE dcim_device SET name='Device label' WHERE id=$1")
        .bind(host)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_devicetype SET model='Model label' WHERE id=$1")
        .bind(model)
        .execute(pool)
        .await?;
    // A VM and device deliberately share a UUID: identity includes the type.
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot,site_id,role_id) VALUES ($1,'{}','VM label','active','','',0,0,'','',$2,$3)").bind(host).bind(site).bind(role).execute(pool).await?;
    let address = ip(pool, "192.0.2.17/24").await?;
    assign(pool, host, &[address]).await?;
    let vrf = realm(pool, "projection", &[address]).await?;
    let prefix = active_prefix(pool, "192.0.2.0/24", vrf).await?;
    let rack = Uuid::now_v7();
    let location = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_rack (id,custom_field_data,name,status,serial,width,u_height,desc_units,comments,site_id,description,starting_unit) VALUES ($1,'{}','Rack label','active','',19,42,false,'',$2,'',1)").bind(rack).bind(site).execute(pool).await?;
    sqlx::query("INSERT INTO dcim_location (id,custom_field_data,name,slug,description,lft,rght,tree_id,level,site_id,status,facility,comments) VALUES ($1,'{}','Location label','projection-location','',0,0,0,0,$2,'active','','')").bind(location).bind(site).execute(pool).await?;
    let (status, service) = call(app, "POST", "/api/v1/inventory/services", Some(json!({"name":"Service label","protocol":"tcp","ports":[443],"parent_object":reference("dcim.device",host)})), Some(cookie)).await?;
    assert_eq!(status, StatusCode::CREATED, "{service}");
    let service = Uuid::parse_str(service["id"].as_str().ok_or("service id")?)?;
    let cases = vec![
        (
            reference("dcim.device", host),
            reference("virtualization.virtualmachine", host),
            "Device label",
            "VM label",
        ),
        (
            reference("ipam.service", service),
            reference("ipam.ipaddress", address),
            "Service label",
            "192.0.2.17/24",
        ),
        (
            reference("ipam.prefix", prefix),
            reference("dcim.devicetype", model),
            "192.0.2.0/24",
            "Model label",
        ),
        (
            reference("dcim.rack", rack),
            reference("dcim.location", location),
            "Rack label",
            "Location label",
        ),
        (
            json!({"object_type":"all"}),
            json!({"object_type":"internet"}),
            "All inventory hosts",
            "Internet (any IP)",
        ),
        (
            json!({"object_type":"dcim.device","object_id":host,"address_mode":"selected","ipaddresses":[address]}),
            reference("virtualization.virtualmachine", host),
            "Device label",
            "VM label",
        ),
    ];
    Ok(ProjectionReferences {
        host,
        site,
        role,
        model,
        cases,
    })
}

async fn create_projection_flows(
    app: &axum::Router,
    cookie: &str,
    refs: &ProjectionReferences,
) -> TestResult<Vec<Value>> {
    let mut expected = Vec::new();
    for (index, (source, destination, source_label, destination_label)) in
        refs.cases.iter().enumerate()
    {
        let mut payload = intent(source.clone(), destination.clone());
        payload["name"] = json!(format!("page-{index:02}"));
        let (status, body) = call(app, "POST", BASE, Some(payload.clone()), Some(cookie)).await?;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["source"]["label"], *source_label);
        assert_eq!(body["destination"]["label"], *destination_label);
        let id = body["id"].as_str().ok_or("flow id")?;
        let (status, updated) = call(
            app,
            "PUT",
            &format!("{BASE}/{id}"),
            Some(json!({"description":"updated"})),
            Some(cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["source"], body["source"]);
        assert_eq!(updated["destination"], body["destination"]);
        expected.push(updated);
    }
    Ok(expected)
}

async fn grow_unrelated_inventory(pool: &sqlx::PgPool, refs: &ProjectionReferences) -> TestResult {
    let ProjectionReferences {
        model, role, site, ..
    } = refs;
    // Shared dependencies keep this large fixture focused on host/IP cardinality.
    sqlx::query("INSERT INTO dcim_device (id,custom_field_data,name,status,serial,description,comments,device_type_id,role_id,site_id,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT gen_random_uuid(),'{}','unrelated-'||n,'active','','','',$1,$2,$3,0,0,0,0,0,0,0,0,0,0 FROM generate_series(1,4216) n").bind(model).bind(role).bind(site).execute(pool).await?;
    sqlx::query("INSERT INTO ipam_ipaddress (id,custom_field_data,address,status,dns_name,description,comments) SELECT gen_random_uuid(),'{}','198.18.0.0'::inet+n,'active','','','' FROM generate_series(1,9540) n").execute(pool).await?;
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM dcim_device)+(SELECT count(*) FROM virtualization_virtualmachine),(SELECT count(*) FROM ipam_ipaddress)").fetch_one(pool).await?;
    assert_eq!(counts, (4218, 9541));
    Ok(())
}

async fn assert_projection_page_and_get(
    app: &axum::Router,
    cookie: &str,
    expected: &[Value],
) -> TestResult {
    let (status, page) = call(
        app,
        "GET",
        &format!("{BASE}?limit=100&offset=0"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], expected.len());
    assert_eq!(page["items"], json!(expected));
    for item in expected {
        let id = item["id"].as_str().ok_or("flow id")?;
        let (status, body) = call(app, "GET", &format!("{BASE}/{id}"), None, Some(cookie)).await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(&body, item);
    }
    Ok(())
}

async fn assert_projection_filters(
    app: &axum::Router,
    cookie: &str,
    host: Uuid,
    expected: &[Value],
) -> TestResult {
    for (query, total, items) in [
        ("limit=2&offset=1".to_string(), 6, json!(&expected[1..3])),
        ("offset=100".to_string(), 6, json!([])),
        ("search=not-found".to_string(), 0, json!([])),
        ("search=page-02".to_string(), 1, json!([expected[2]])),
        ("status=active".to_string(), 0, json!([])),
        ("status=draft".to_string(), 6, json!(expected)),
        (
            format!("source_type=dcim.device&source_id={host}"),
            2,
            json!([expected[0], expected[5]]),
        ),
        (
            format!("destination_type=virtualization.virtualmachine&destination_id={host}"),
            2,
            json!([expected[0], expected[5]]),
        ),
    ] {
        let (status, page) =
            call(app, "GET", &format!("{BASE}?{query}"), None, Some(cookie)).await?;
        assert_eq!(status, StatusCode::OK, "{query}: {page}");
        assert_eq!(page["total"], total);
        assert_eq!(page["items"], items);
    }
    Ok(())
}

async fn assert_projection_permissions(
    app: &axum::Router,
    denied: &str,
    expected: &[Value],
) -> TestResult {
    for path in [
        BASE.to_string(),
        format!("{BASE}/{}", expected[0]["id"].as_str().ok_or("flow id")?),
    ] {
        assert_eq!(
            call(app, "GET", &path, None, Some(denied)).await?.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(app, "GET", &path, None, None).await?.0,
            StatusCode::FORBIDDEN
        );
    }
    Ok(())
}

async fn assert_projection_validation_caps(
    app: &axum::Router,
    cookie: &str,
    host: Uuid,
    expected: &[Value],
) -> TestResult {
    // Validation/preview must still fail closed, not use metadata as a policy catalog.
    let (status, _) = call(
        app,
        "POST",
        BASE,
        Some(intent(
            reference("dcim.device", host),
            json!({"object_type":"internet"}),
        )),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        call(
            app,
            "PUT",
            &format!("{BASE}/{}", expected[0]["id"].as_str().ok_or("flow id")?),
            Some(json!({"description":"blocked"})),
            Some(cookie)
        )
        .await?
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        call(
            app,
            "POST",
            &format!("{BASE}/endpoint-resolution"),
            Some(reference("dcim.device", host)),
            Some(cookie)
        )
        .await?
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    Ok(())
}

async fn assert_projection_missing_and_null_labels(
    state: &AppState,
    host: Uuid,
    expected: &[Value],
) -> TestResult {
    use piping_backend::contexts::inventory::domain::native::dataflow::{
        Dataflow, DataflowEndpointInput,
    };
    let pool = &state.db_pool;
    // Missing references are tested without persisting invalid foreign keys.
    let id = Uuid::parse_str(expected[0]["id"].as_str().ok_or("flow id")?)?;
    let flow = state
        .repositories
        .dataflows
        .get(id)
        .await?
        .ok_or("flow missing")?;
    let mut missing: Dataflow = flow.clone();
    missing.source =
        serde_json::from_value::<DataflowEndpointInput>(reference("dcim.device", Uuid::now_v7()))?;
    let projected = state.repositories.dataflows.projections(&[missing]).await?;
    assert_eq!(projected[0].0.label, None);
    sqlx::query("UPDATE dcim_device SET name=NULL WHERE id=$1")
        .bind(host)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET name='' WHERE id=$1")
        .bind(host)
        .execute(pool)
        .await?;
    let projected = state.repositories.dataflows.projections(&[flow]).await?;
    assert_eq!(projected[0].0.label.as_deref(), Some("Host"));
    assert_eq!(projected[0].1.label.as_deref(), Some(""));
    Ok(())
}

struct RegressionInventory {
    state: AppState,
    app: axum::Router,
    cookie: String,
    a: Uuid,
    b: Uuid,
    c: Uuid,
    a_ip: Uuid,
    b_ip: Uuid,
    c_ip: Uuid,
    red: Uuid,
}
async fn regression_inventory()
-> Result<RegressionInventory, Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let name = format!("regression-{}", common::unique_suffix());
    let (a, _) = common::seed_device(pool, &name).await?;
    let (b, _) = common::seed_device(pool, &name).await?;
    let (c, _) = common::seed_device(pool, &name).await?;
    let a_ip = ip(pool, "192.0.2.1/24").await?;
    let b_ip = ip(pool, "192.0.2.2/24").await?;
    let c_ip = ip(pool, "192.0.2.2/24").await?;
    let red = realm(pool, &name, &[a_ip, c_ip]).await?;
    realm(pool, &format!("{name}-blue"), &[b_ip]).await?;
    for (host, address) in [(a, a_ip), (b, b_ip), (c, c_ip)] {
        assign(pool, host, &[address]).await?;
        sqlx::query("UPDATE dcim_device SET primary_ip4_id=$2 WHERE id=$1")
            .bind(host)
            .bind(address)
            .execute(pool)
            .await?;
    }
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    Ok(RegressionInventory {
        state,
        app,
        cookie,
        a,
        b,
        c,
        a_ip,
        b_ip,
        c_ip,
        red,
    })
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn regression_service_picker_preserves_selectable_primary_only_ip() -> TestResult {
    let r = regression_inventory().await?;
    let pool = &r.state.db_pool;
    sqlx::query("UPDATE ipam_ipaddress SET assigned_object_id=NULL, assigned_object_type_id=NULL WHERE id=$1")
        .bind(r.a_ip).execute(pool).await?;
    let assigned = ip(pool, "192.0.2.50/24").await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id=$2 WHERE id=$1")
        .bind(assigned)
        .bind(r.red)
        .execute(pool)
        .await?;
    assign(pool, r.a, &[assigned]).await?;
    let (status, service) = call(
        &r.app,
        "POST",
        "/api/v1/inventory/services",
        Some(json!({
            "name": "Primary-only picker regression", "protocol": "tcp", "ports": [443],
            "parent_object": reference("dcim.device", r.a)
        })),
        Some(&r.cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{service}");
    let service_id = Uuid::parse_str(service["id"].as_str().unwrap())?;
    let uri = format!("{BASE}/endpoint-resolution");
    let (status, preview) = call(
        &r.app,
        "POST",
        &uri,
        Some(reference("ipam.service", service_id)),
        Some(&r.cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let choices: std::collections::BTreeSet<_> = preview["ipaddresses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|ip| Uuid::parse_str(ip["id"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        choices,
        [r.a_ip, assigned].into_iter().collect(),
        "selectable choices must include the primary-only IP as well as assigned fallback"
    );
    assert_eq!(preview["default_ipaddresses"], json!([assigned]));
    assert_eq!(
        preview["addresses"],
        json!([{ "cidr": "192.0.2.50/32", "vrf_id": r.red }])
    );
    let (status, all) = call(
        &r.app,
        "POST",
        &uri,
        Some(json!({
            "object_type":"ipam.service", "object_id":service_id, "address_mode":"all"
        })),
        Some(&r.cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(
        all["addresses"], preview["addresses"],
        "all fallback remains assigned-only"
    );
    assert_eq!(
        all["ipaddresses"], preview["ipaddresses"],
        "picker choices do not depend on the current mode"
    );
    let selected = json!({"object_type":"ipam.service", "object_id":service_id,
        "address_mode":"selected", "ipaddresses":[r.a_ip]});
    let (status, preview) = call(
        &r.app,
        "POST",
        &uri,
        Some(selected.clone()),
        Some(&r.cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(
        preview["addresses"],
        json!([{ "cidr": "192.0.2.1/32", "vrf_id": r.red }])
    );
    assert_eq!(preview["diagnostics"], json!([]));
    let flow_id = create_flow(
        &r.app,
        intent(selected, reference("dcim.device", r.c)),
        &r.cookie,
    )
    .await?;
    let (status, _) = call(
        &r.app,
        "PUT",
        &format!("/api/v1/inventory/services/{service_id}"),
        Some(json!({"ipaddresses":[assigned]})),
        Some(&r.cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (_, bound) = call(
        &r.app,
        "POST",
        &uri,
        Some(reference("ipam.service", service_id)),
        Some(&r.cookie),
    )
    .await?;
    assert_eq!(
        bound["ipaddresses"].as_array().unwrap().len(),
        1,
        "configured bindings must still restrict selectable choices"
    );
    assert_eq!(bound["ipaddresses"][0]["id"], json!(assigned));
    call(
        &r.app,
        "DELETE",
        &format!("{BASE}/{flow_id}"),
        None,
        Some(&r.cookie),
    )
    .await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn regression_cross_vrf_third_host_collision() -> TestResult {
    let r = regression_inventory().await?;
    let mut flow = intent(reference("dcim.device", r.a), reference("dcim.device", r.b));
    flow["status"] = json!("active");
    flow["allow_cross_vrf"] = json!(true);
    let id = create_flow(&r.app, flow, &r.cookie).await?;
    let blocked = policy(&r.app, "dcim.device", r.a, &r.cookie).await?;
    assert_eq!(
        blocked["apply_ready"], false,
        "projected B must not permit red C"
    );
    assert_eq!(blocked["rules"], json!([]));
    sqlx::query("UPDATE ipam_ipaddress SET address='192.0.2.3/24' WHERE id=$1")
        .bind(r.c_ip)
        .execute(&r.state.db_pool)
        .await?;
    assert_eq!(
        policy(&r.app, "dcim.device", r.a, &r.cookie).await?["apply_ready"],
        true
    );
    call(
        &r.app,
        "DELETE",
        &format!("{BASE}/{id}"),
        None,
        Some(&r.cookie),
    )
    .await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn regression_prefix_peer_visibility_and_local_membership() -> TestResult {
    let r = regression_inventory().await?;
    let prefix = active_prefix(&r.state.db_pool, "192.0.2.2/32", r.red).await?;
    let mut flow = intent(
        reference("dcim.device", r.a),
        reference("ipam.prefix", prefix),
    );
    flow["status"] = json!("active");
    let id = create_flow(&r.app, flow, &r.cookie).await?;
    let denied = user_cookie(&r.state, &r.app, true, true, Some(r.c)).await?;
    let uri = format!(
        "{BASE}/firewall-policy?host_type=dcim.device&host_id={}",
        r.a
    );
    assert_eq!(
        call(&r.app, "GET", &uri, None, Some(&denied)).await?.0,
        StatusCode::FORBIDDEN
    );
    let excluded = user_cookie(&r.state, &r.app, true, true, Some(r.b)).await?;
    assert_eq!(
        call(&r.app, "GET", &uri, None, Some(&excluded)).await?.0,
        StatusCode::OK
    );
    assert_eq!(
        policy(&r.app, "dcim.device", r.c, &r.cookie).await?["rules"][0]["destination"],
        "192.0.2.2/32"
    );
    sqlx::query("UPDATE ipam_prefix SET prefix='192.0.2.0/24' WHERE id=$1")
        .bind(prefix)
        .execute(&r.state.db_pool)
        .await?;
    assert_eq!(
        call(&r.app, "GET", &uri, None, Some(&denied)).await?.0,
        StatusCode::FORBIDDEN
    );
    assert!(
        !policy(&r.app, "dcim.device", r.a, &r.cookie).await?["rules"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a broader remote prefix must not be omitted just because it also contains the local host"
    );
    call(
        &r.app,
        "DELETE",
        &format!("{BASE}/{id}"),
        None,
        Some(&r.cookie),
    )
    .await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn regression_prefix_remote_ambiguity_blocks_partial_policy() -> TestResult {
    let r = regression_inventory().await?;
    let pool = &r.state.db_pool;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id=$2 WHERE id=$1")
        .bind(r.b_ip)
        .bind(r.red)
        .execute(pool)
        .await?;
    let prefix = active_prefix(pool, "192.0.2.2/32", r.red).await?;
    let mut good = intent(
        json!({"object_type":"internet"}),
        reference("dcim.device", r.a),
    );
    good["status"] = json!("active");
    let good_id = create_flow(&r.app, good, &r.cookie).await?;
    let mut flow = intent(
        reference("dcim.device", r.a),
        reference("ipam.prefix", prefix),
    );
    flow["status"] = json!("active");
    let id = create_flow(&r.app, flow, &r.cookie).await?;
    let blocked = policy(&r.app, "dcim.device", r.a, &r.cookie).await?;
    assert_eq!(
        blocked["apply_ready"], false,
        "remote Prefix must retain duplicate owner errors"
    );
    assert_eq!(blocked["rules"], json!([]));

    sqlx::query("UPDATE ipam_prefix SET prefix='192.0.2.4/32' WHERE id=$1")
        .bind(prefix)
        .execute(pool)
        .await?;
    let outside = policy(&r.app, "dcim.device", r.a, &r.cookie).await?;
    assert_eq!(
        outside["apply_ready"], true,
        "duplicates outside the selected Prefix must not block"
    );
    assert_eq!(outside["rules"].as_array().unwrap().len(), 2);
    for flow_id in [id, good_id] {
        call(
            &r.app,
            "DELETE",
            &format!("{BASE}/{flow_id}"),
            None,
            Some(&r.cookie),
        )
        .await?;
    }
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn regression_prefix_local_visibility_projects_requested_host() -> TestResult {
    let r = regression_inventory().await?;
    let pool = &r.state.db_pool;
    sqlx::query("UPDATE ipam_ipaddress SET address='192.0.2.4/24', vrf_id=$2 WHERE id=$1")
        .bind(r.b_ip)
        .bind(r.red)
        .execute(pool)
        .await?;
    let prefix = active_prefix(pool, "192.0.2.0/30", r.red).await?;
    let mut flow = intent(
        reference("dcim.device", r.b),
        reference("ipam.prefix", prefix),
    );
    flow["status"] = json!("active");
    let id = create_flow(&r.app, flow, &r.cookie).await?;
    let denied = user_cookie(&r.state, &r.app, true, true, Some(r.c)).await?;
    for (source, destination, source_ip, destination_ip) in [
        (
            reference("dcim.device", r.b),
            reference("ipam.prefix", prefix),
            "192.0.2.4/32",
            "192.0.2.1/32",
        ),
        (
            reference("ipam.prefix", prefix),
            reference("dcim.device", r.b),
            "192.0.2.1/32",
            "192.0.2.4/32",
        ),
    ] {
        assert_eq!(
            call(
                &r.app,
                "PUT",
                &format!("{BASE}/{id}"),
                Some(intent(source, destination)),
                Some(&r.cookie)
            )
            .await?
            .0,
            StatusCode::OK
        );
        let projected = policy(&r.app, "dcim.device", r.a, &denied).await?;
        assert_eq!(projected["apply_ready"], true);
        assert_eq!(projected["rules"].as_array().unwrap().len(), 1);
        assert_eq!(projected["rules"][0]["source"], source_ip);
        assert_eq!(projected["rules"][0]["destination"], destination_ip);
        let remote_uri = format!(
            "{BASE}/firewall-policy?host_type=dcim.device&host_id={}",
            r.b
        );
        assert_eq!(
            call(&r.app, "GET", &remote_uri, None, Some(&denied))
                .await?
                .0,
            StatusCode::FORBIDDEN,
            "remote Prefix still requires all contributing owners"
        );
    }
    let (other, _) =
        common::seed_device(pool, &format!("prefix-other-{}", common::unique_suffix())).await?;
    let duplicate = ip(pool, "192.0.2.2/24").await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id=$2 WHERE id=$1")
        .bind(duplicate)
        .bind(r.red)
        .execute(pool)
        .await?;
    assign(pool, other, &[duplicate]).await?;
    let projected = policy(&r.app, "dcim.device", r.a, &denied).await?;
    assert_eq!(
        projected["apply_ready"], true,
        "another local member's ambiguity must not affect the requested host"
    );
    assert_eq!(projected["rules"].as_array().unwrap().len(), 1);
    call(
        &r.app,
        "DELETE",
        &format!("{BASE}/{id}"),
        None,
        Some(&r.cookie),
    )
    .await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn regression_invalid_explicit_ip_keeps_relevance() -> TestResult {
    let r = regression_inventory().await?;
    let mut good = intent(
        json!({"object_type":"internet"}),
        reference("dcim.device", r.a),
    );
    good["status"] = json!("active");
    let good_id = create_flow(&r.app, good, &r.cookie).await?;
    let mut flow = intent(
        json!({"object_type":"internet"}),
        reference("ipam.ipaddress", r.a_ip),
    );
    flow["status"] = json!("active");
    let id = create_flow(&r.app, flow, &r.cookie).await?;
    // Keep a separate valid chosen address, so the IP flow alone determines failure.
    let healthy = ip(&r.state.db_pool, "192.0.2.10/24").await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id=$2 WHERE id=$1")
        .bind(healthy)
        .bind(r.red)
        .execute(&r.state.db_pool)
        .await?;
    assign(&r.state.db_pool, r.a, &[healthy]).await?;
    sqlx::query("UPDATE dcim_device SET primary_ip4_id=$2 WHERE id=$1")
        .bind(r.a)
        .bind(healthy)
        .execute(&r.state.db_pool)
        .await?;
    sqlx::query("UPDATE ipam_ipaddress SET status='reserved' WHERE id=$1")
        .bind(r.a_ip)
        .execute(&r.state.db_pool)
        .await?;
    let blocked = policy(&r.app, "dcim.device", r.a, &r.cookie).await?;
    assert_eq!(blocked["apply_ready"], false);
    assert_eq!(blocked["rules"], json!([]));
    assert_eq!(
        policy(&r.app, "dcim.device", r.b, &r.cookie).await?["apply_ready"],
        true
    );
    let mut other = intent(
        json!({"object_type":"internet"}),
        reference("dcim.device", r.b),
    );
    other["status"] = json!("active");
    let other_id = create_flow(&r.app, other, &r.cookie).await?;
    let unrelated = policy(&r.app, "dcim.device", r.b, &r.cookie).await?;
    assert_eq!(unrelated["apply_ready"], true);
    assert!(!unrelated["rules"].as_array().unwrap().is_empty());
    for id in [id, good_id, other_id] {
        call(
            &r.app,
            "DELETE",
            &format!("{BASE}/{id}"),
            None,
            Some(&r.cookie),
        )
        .await?;
    }
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn regression_configured_service_binding_is_strict() -> TestResult {
    let r = regression_inventory().await?;
    let extra = ip(&r.state.db_pool, "192.0.2.11/24").await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id=$2 WHERE id=$1")
        .bind(extra)
        .bind(r.red)
        .execute(&r.state.db_pool)
        .await?;
    let interface = assign(&r.state.db_pool, r.a, &[extra]).await?;
    let (status, service) = call(&r.app,"POST","/api/v1/inventory/services",Some(json!({"name":format!("strict-{}",common::unique_suffix()),"protocol":"tcp","ports":[443],"parent_object":reference("dcim.device",r.a),"ipaddresses":[r.a_ip,extra]})),Some(&r.cookie)).await?;
    assert_eq!(status, StatusCode::CREATED, "{service}");
    let service_id = Uuid::parse_str(service["id"].as_str().unwrap())?;
    let mut flow = intent(
        reference("ipam.service", service_id),
        reference("dcim.device", r.c),
    );
    flow["status"] = json!("active");
    let id = create_flow(&r.app, flow, &r.cookie).await?;
    sqlx::query("DELETE FROM dcim_interface WHERE id=$1")
        .bind(interface)
        .execute(&r.state.db_pool)
        .await?;
    let blocked = policy(&r.app, "dcim.device", r.a, &r.cookie).await?;
    assert_eq!(blocked["apply_ready"], false);
    assert_eq!(blocked["rules"], json!([]));
    let reassigned = assign(&r.state.db_pool, r.c, &[extra]).await?;
    call(&r.app,"PUT",&format!("{BASE}/{id}"),Some(json!({"source":{"object_type":"ipam.service","object_id":service_id,"address_mode":"all"}})),Some(&r.cookie)).await?;
    let blocked_all = policy(&r.app, "dcim.device", r.a, &r.cookie).await?;
    assert_eq!(blocked_all["apply_ready"], false);
    assert_eq!(blocked_all["rules"], json!([]));
    sqlx::query("DELETE FROM dcim_interface WHERE id=$1")
        .bind(reassigned)
        .execute(&r.state.db_pool)
        .await?;
    let discovered = ip(&r.state.db_pool, "192.0.2.12/24").await?;
    assign(&r.state.db_pool, r.a, &[discovered]).await?;
    sqlx::query("UPDATE ipam_ipaddress SET status='reserved',vrf_id=$2 WHERE id=$1")
        .bind(discovered)
        .bind(r.red)
        .execute(&r.state.db_pool)
        .await?;
    call(
        &r.app,
        "PUT",
        &format!("/api/v1/inventory/services/{service_id}"),
        Some(json!({"ipaddresses":[]})),
        Some(&r.cookie),
    )
    .await?;
    let fallback = policy(&r.app, "dcim.device", r.a, &r.cookie).await?;
    assert_eq!(
        fallback["apply_ready"], true,
        "fallback discovery excludes broken candidates"
    );
    assert!(
        fallback["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "inventory_excluded")
    );
    call(
        &r.app,
        "DELETE",
        &format!("{BASE}/{id}"),
        None,
        Some(&r.cookie),
    )
    .await?;
    Ok(())
}
const BASE: &str = "/api/v1/inventory/dataflows";
async fn preview(
    app: &axum::Router,
    selector: Value,
    cookie: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = call(
        app,
        "POST",
        &format!("{BASE}/endpoint-resolution"),
        Some(selector),
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    Ok(body)
}
async fn hierarchy(
    pool: &sqlx::PgPool,
    table: &str,
    parent: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let name = format!("scope-{id}");
    // Test-owned closed table list, not request input.
    assert!(["dcim_region", "dcim_sitegroup", "tenancy_tenantgroup"].contains(&table));
    sqlx::query(&format!("INSERT INTO {table} (id,custom_field_data,name,slug,description,lft,rght,tree_id,level,comments,parent_id) VALUES ($1,'{{}}',$2,$2,'',0,0,0,0,'',$3)")).bind(id).bind(name).bind(parent).execute(pool).await?;
    Ok(id)
}
async fn scope_vm(
    pool: &sqlx::PgPool,
    site: Uuid,
    role: Uuid,
    tenant: Option<Uuid>,
    address: &str,
    vrf: Uuid,
) -> Result<(Uuid, Uuid), sqlx::Error> {
    let vm = Uuid::now_v7();
    let ip = ip(pool, address).await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id=$2 WHERE id=$1")
        .bind(ip)
        .bind(vrf)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot,site_id,role_id,tenant_id,primary_ip4_id) VALUES ($1,'{}',$2,'active','','',0,0,'','',$3,$4,$5,$6)").bind(vm).bind(format!("scope-vm-{vm}")).bind(site).bind(role).bind(tenant).bind(ip).execute(pool).await?;
    Ok((vm, ip))
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn scopes_membership_composition_permissions_and_native_references() -> TestResult {
    use piping_backend::contexts::inventory::domain::native::dataflow::EndpointType;
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let name = format!("scopes-{}", common::unique_suffix());
    let (a, _) = common::seed_device(pool, &name).await?;
    let (b, _) = common::seed_device(pool, &name).await?;
    let (outside, _) = common::seed_device(pool, &name).await?;
    let (unaddressed, _) = common::seed_device(pool, &name).await?;
    let (site, role, device_type): (Uuid, Uuid, Uuid) =
        sqlx::query_as("SELECT site_id,role_id,device_type_id FROM dcim_device WHERE id=$1")
            .bind(a)
            .fetch_one(pool)
            .await?;
    let (b_site, b_role): (Uuid, Uuid) =
        sqlx::query_as("SELECT site_id,role_id FROM dcim_device WHERE id=$1")
            .bind(b)
            .fetch_one(pool)
            .await?;
    let manufacturer: Uuid =
        sqlx::query_scalar("SELECT manufacturer_id FROM dcim_devicetype WHERE id=$1")
            .bind(device_type)
            .fetch_one(pool)
            .await?;
    let region = hierarchy(pool, "dcim_region", None).await?;
    let child = hierarchy(pool, "dcim_region", Some(region)).await?;
    let sitegroup = hierarchy(pool, "dcim_sitegroup", None).await?;
    let sitegroup_child = hierarchy(pool, "dcim_sitegroup", Some(sitegroup)).await?;
    sqlx::query("UPDATE dcim_site SET region_id=$2,group_id=$3 WHERE id=$1")
        .bind(site)
        .bind(child)
        .bind(sitegroup_child)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_device SET site_id=$2 WHERE id=$1")
        .bind(unaddressed)
        .bind(site)
        .execute(pool)
        .await?;
    let tenantgroup = hierarchy(pool, "tenancy_tenantgroup", None).await?;
    let tenantgroup_child = hierarchy(pool, "tenancy_tenantgroup", Some(tenantgroup)).await?;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenancy_tenant (id,custom_field_data,name,slug,description,comments,group_id) VALUES ($1,'{}',$2,$2,'','',$3)").bind(tenant).bind(&name).bind(tenantgroup_child).execute(pool).await?;
    sqlx::query("UPDATE dcim_device SET tenant_id=$2 WHERE id=$1")
        .bind(b)
        .bind(tenant)
        .execute(pool)
        .await?;
    // A child role has deliberately stale MPTT values: only parent_id is authoritative.
    let child_role: Uuid = sqlx::query_scalar("SELECT role_id FROM dcim_device WHERE id=$1")
        .bind(unaddressed)
        .fetch_one(pool)
        .await?;
    sqlx::query("UPDATE dcim_devicerole SET parent_id=$2 WHERE id=$1")
        .bind(child_role)
        .bind(role)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_device SET role_id=$2 WHERE id=$1")
        .bind(a)
        .bind(child_role)
        .execute(pool)
        .await?;
    let a_ip = ip(pool, "198.19.10.1/24").await?;
    let b_ip = ip(pool, "198.19.10.2/24").await?;
    let extra = ip(pool, "198.19.10.11/24").await?;
    let vrf = realm(pool, &name, &[a_ip, b_ip, extra]).await?;
    assign(pool, a, &[a_ip, extra]).await?;
    assign(pool, b, &[b_ip]).await?;
    for (host, address) in [(a, a_ip), (b, b_ip)] {
        sqlx::query("UPDATE dcim_device SET primary_ip4_id=$2 WHERE id=$1")
            .bind(host)
            .bind(address)
            .execute(pool)
            .await?;
    }
    let (vm_a, vm_a_ip) = scope_vm(pool, site, child_role, None, "198.19.10.3/24", vrf).await?;
    let (vm_b, vm_b_ip) =
        scope_vm(pool, b_site, b_role, Some(tenant), "198.19.10.4/24", vrf).await?;
    let platform = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_platform (id,custom_field_data,name,slug,description,level,lft,rght,tree_id,comments) VALUES ($1,'{}',$2,$2,'',0,0,0,0,'')").bind(platform).bind(&name).execute(pool).await?;
    let cluster_group = Uuid::now_v7();
    let cluster_type = Uuid::now_v7();
    let cluster = Uuid::now_v7();
    for (table, id) in [
        ("virtualization_clustergroup", cluster_group),
        ("virtualization_clustertype", cluster_type),
    ] {
        sqlx::query(&format!("INSERT INTO {table} (id,custom_field_data,name,slug,description,comments) VALUES ($1,'{{}}',$2,$2,'','')")).bind(id).bind(&name).execute(pool).await?;
    }
    sqlx::query("INSERT INTO virtualization_cluster (id,custom_field_data,name,comments,type_id,status,description,group_id,_site_id,tenant_id) VALUES ($1,'{}',$2,'',$3,'active','',$4,$5,$6)").bind(cluster).bind(&name).bind(cluster_type).bind(cluster_group).bind(site).bind(tenant).execute(pool).await?;
    let rack = Uuid::now_v7();
    let location = Uuid::now_v7();
    let location_child = Uuid::now_v7();
    for (id, parent) in [(location, None), (location_child, Some(location))] {
        sqlx::query("INSERT INTO dcim_location (id,custom_field_data,name,slug,description,lft,rght,tree_id,level,site_id,status,facility,comments,parent_id) VALUES ($1,'{}',$2,$2,'',0,0,0,0,$3,'active','', '',$4)").bind(id).bind(format!("location-{id}")).bind(site).bind(parent).execute(pool).await?;
    }
    sqlx::query("INSERT INTO dcim_rack (id,custom_field_data,name,status,serial,width,u_height,desc_units,comments,site_id,description,starting_unit) VALUES ($1,'{}',$2,'active','',19,42,false,'',$3,'',1)").bind(rack).bind(&name).bind(site).execute(pool).await?;
    sqlx::query(
        "UPDATE dcim_device SET platform_id=$2,cluster_id=$3,rack_id=$4,location_id=$5 WHERE id=$1",
    )
    .bind(a)
    .bind(platform)
    .bind(cluster)
    .bind(rack)
    .bind(location_child)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE virtualization_virtualmachine SET platform_id=$2,cluster_id=$3 WHERE id=$1",
    )
    .bind(vm_a)
    .bind(platform)
    .bind(cluster)
    .execute(pool)
    .await?;
    // Cluster caches/tenant never turn this unrelated host into a Site/Tenant member.
    sqlx::query("UPDATE dcim_device SET cluster_id=$2 WHERE id=$1")
        .bind(outside)
        .bind(cluster)
        .execute(pool)
        .await?;
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    let source = reference("dcim.region", region);
    let destination = reference("tenancy.tenant", tenant);
    let preview_region = preview(&app, source.clone(), &cookie).await?;
    assert_eq!(preview_region["members"].as_array().unwrap().len(), 3);
    assert!(preview_region["members"].as_array().unwrap().iter().any(
        |member| member["host"]["object_id"] == json!(unaddressed)
            && member["addresses"] == json!([])
            && !member["diagnostics"].as_array().unwrap().is_empty()
    ));
    assert_eq!(preview_region["addresses"].as_array().unwrap().len(), 2);
    assert_eq!(preview_region["ipaddresses"].as_array().unwrap().len(), 3);
    let tenant_preview = preview(&app, destination.clone(), &cookie).await?;
    assert_eq!(tenant_preview["members"].as_array().unwrap().len(), 2);
    assert!(
        tenant_preview["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["host"]["object_id"] == json!(vm_b))
    );
    let scopes = [
        ("dcim.region", region, 3),
        ("dcim.site", site, 3),
        ("dcim.sitegroup", sitegroup, 3),
        ("dcim.devicerole", role, 3),
        ("tenancy.tenant", tenant, 2),
        ("tenancy.tenantgroup", tenantgroup, 2),
        ("dcim.platform", platform, 2),
        ("dcim.manufacturer", manufacturer, 1),
        ("dcim.devicetype", device_type, 1),
        ("dcim.rack", rack, 1),
        ("dcim.location", location, 1),
        ("virtualization.cluster", cluster, 3),
        ("virtualization.clustergroup", cluster_group, 3),
        ("virtualization.clustertype", cluster_type, 3),
    ];
    for (kind, id, members) in scopes {
        let resolved = preview(&app, reference(kind, id), &cookie).await?;
        assert_eq!(
            resolved["members"].as_array().unwrap().len(),
            members,
            "{kind}: {resolved}"
        );
        let flow = create_flow(
            &app,
            intent(json!({"object_type":"internet"}), reference(kind, id)),
            &cookie,
        )
        .await?;
        let (_, saved) = call(&app, "GET", &format!("{BASE}/{flow}"), None, Some(&cookie)).await?;
        assert_eq!(saved["destination"]["object_id"], json!(id));
        assert_eq!(saved["destination"]["object_type"], kind);
        let column = EndpointType::parse(kind)?.column();
        let error = sqlx::query(&format!(
            "UPDATE inventory_dataflow SET destination_{column}=$2 WHERE id=$1"
        ))
        .bind(Uuid::parse_str(&flow)?)
        .bind(Uuid::now_v7())
        .execute(pool)
        .await
        .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23503"),
            "{kind}"
        );
        call(
            &app,
            "DELETE",
            &format!("{BASE}/{flow}"),
            None,
            Some(&cookie),
        )
        .await?;
    }
    let minimal = create_flow(&app, intent(source.clone(), destination.clone()), &cookie).await?;
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"status":"active"})),
        Some(&cookie),
    )
    .await?;
    let exported = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(exported["apply_ready"], true);
    assert_eq!(exported["rules"].as_array().unwrap().len(), 2);
    assert_eq!(
        policy(&app, "virtualization.virtualmachine", vm_a, &cookie).await?["rules"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let read_only = user_cookie(&state, &app, true, false, None).await?;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{BASE}/endpoint-resolution"),
            Some(source.clone()),
            Some(&read_only)
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            BASE,
            Some(intent(source.clone(), destination.clone())),
            Some(&read_only)
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    let policy_uri = format!("{BASE}/firewall-policy?host_type=dcim.device&host_id={a}");
    assert_eq!(
        call(&app, "GET", &policy_uri, None, Some(&read_only))
            .await?
            .0,
        StatusCode::FORBIDDEN
    );
    let hidden_peer = user_cookie(&state, &app, true, true, Some(vm_b)).await?;
    assert_eq!(
        call(&app, "GET", &policy_uri, None, Some(&hidden_peer))
            .await?
            .0,
        StatusCode::FORBIDDEN
    );
    let hidden_local = user_cookie(&state, &app, true, true, Some(vm_a)).await?;
    assert_eq!(
        call(&app, "GET", &policy_uri, None, Some(&hidden_local))
            .await?
            .0,
        StatusCode::OK
    );
    let other_realm = realm(pool, &format!("{name}-excluded"), &[vm_b_ip]).await?;
    call(&app,"PUT",&format!("{BASE}/{minimal}"),Some(json!({"destination":{"object_type":"tenancy.tenant","object_id":tenant,"address_mode":"selected","ipaddresses":[b_ip,vm_b_ip]}})),Some(&cookie)).await?;
    assert_eq!(
        call(&app, "GET", &policy_uri, None, Some(&hidden_peer))
            .await?
            .0,
        StatusCode::OK,
        "selected remote owner in another VRF is excluded"
    );
    sqlx::query("UPDATE ipam_ipaddress SET status='reserved' WHERE id=$1 AND vrf_id=$2")
        .bind(vm_b_ip)
        .bind(other_realm)
        .execute(pool)
        .await?;
    assert_eq!(
        call(&app, "GET", &policy_uri, None, Some(&hidden_peer))
            .await?
            .0,
        StatusCode::OK,
        "unresolved selected ownership must not over-authorize another realm"
    );
    sqlx::query("UPDATE ipam_ipaddress SET status='active',vrf_id=$2 WHERE id=$1")
        .bind(vm_b_ip)
        .bind(vrf)
        .execute(pool)
        .await?;
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"destination":destination})),
        Some(&cookie),
    )
    .await?;
    let all_mode = json!({"object_type":"dcim.region","object_id":region,"address_mode":"all"});
    assert_eq!(
        preview(&app, all_mode.clone(), &cookie).await?["addresses"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let selected = json!({"object_type":"dcim.region","object_id":region,"address_mode":"selected","ipaddresses":[extra,a_ip]});
    let selected_preview = preview(&app, selected.clone(), &cookie).await?;
    assert_eq!(selected_preview["members"].as_array().unwrap().len(), 3);
    assert_eq!(selected_preview["addresses"].as_array().unwrap().len(), 2);
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"source":selected})),
        Some(&cookie),
    )
    .await?;
    assert_eq!(call(&app,"PUT",&format!("{BASE}/{minimal}"),Some(json!({"source":{"object_type":"dcim.region","object_id":region,"address_mode":"selected","ipaddresses":[b_ip]}})),Some(&cookie)).await?.0,StatusCode::BAD_REQUEST);
    sqlx::query("UPDATE ipam_ipaddress SET status='reserved' WHERE id=$1")
        .bind(extra)
        .execute(pool)
        .await?;
    let blocked = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(blocked["apply_ready"], false);
    assert_eq!(blocked["rules"], json!([]));
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"source":all_mode})),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        policy(&app, "dcim.device", a, &cookie).await?["apply_ready"],
        true,
        "discovered ineligible members are exclusions"
    );
    sqlx::query("UPDATE ipam_ipaddress SET status='active' WHERE id=$1")
        .bind(extra)
        .execute(pool)
        .await?;
    // Overlapping Region/All needs real ingress AND egress pairs, not suppression.
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"source":source,"destination":{"object_type":"all"}})),
        Some(&cookie),
    )
    .await?;
    let overlap = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(overlap["apply_ready"], true);
    assert!(
        overlap["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["direction"] == "ingress" && rule["source"] == "198.19.10.3/32")
    );
    assert!(
        overlap["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["direction"] == "egress" && rule["destination"] == "198.19.10.3/32")
    );
    // Same-context ambiguity blocks even if its other owner isn't in the scope.
    let ambiguous = ip(pool, "198.19.10.1/24").await?;
    assign(pool, outside, &[ambiguous]).await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id=$2 WHERE id=$1")
        .bind(ambiguous)
        .bind(vrf)
        .execute(pool)
        .await?;
    assert_eq!(
        policy(&app, "dcim.device", a, &cookie).await?["apply_ready"],
        false
    );
    sqlx::query("UPDATE ipam_ipaddress SET status='reserved' WHERE id=$1")
        .bind(ambiguous)
        .execute(pool)
        .await?;
    // Selected-branch cycle is diagnosed; unrelated cyclic role branch does not poison Region.
    sqlx::query("UPDATE dcim_region SET parent_id=$2 WHERE id=$1")
        .bind(region)
        .bind(child)
        .execute(pool)
        .await?;
    assert!(
        preview(&app, reference("dcim.region", region), &cookie).await?["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "endpoint_unresolved")
    );
    let cycle_policy = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(cycle_policy["apply_ready"], false);
    assert_eq!(cycle_policy["rules"], json!([]));
    sqlx::query("UPDATE dcim_region SET parent_id=NULL WHERE id=$1")
        .bind(region)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_devicerole SET parent_id=$2 WHERE id=$1")
        .bind(role)
        .bind(child_role)
        .execute(pool)
        .await?;
    assert_eq!(
        preview(&app, reference("dcim.region", region), &cookie).await?["members"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    sqlx::query("UPDATE dcim_devicerole SET parent_id=NULL WHERE id=$1")
        .bind(role)
        .execute(pool)
        .await?;
    let native_bad =
        sqlx::query("UPDATE inventory_dataflow SET source_kind='dcim.site' WHERE id=$1")
            .bind(Uuid::parse_str(&minimal)?)
            .execute(pool)
            .await
            .unwrap_err();
    assert_eq!(
        native_bad.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    let restrict_count:i64 = sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conrelid='inventory_dataflow'::regclass AND contype='f' AND confdeltype='r'").fetch_one(pool).await?;
    assert_eq!(restrict_count, 38);
    let restricted_delete = sqlx::query("DELETE FROM dcim_region WHERE id=$1")
        .bind(region)
        .execute(pool)
        .await
        .unwrap_err();
    assert_eq!(
        restricted_delete
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("23001")
    );
    call(
        &app,
        "DELETE",
        &format!("{BASE}/{minimal}"),
        None,
        Some(&cookie),
    )
    .await?;
    let internet = preview(&app, json!({"object_type":"internet"}), &cookie).await?;
    assert_eq!(internet["members"], json!([]));
    assert_eq!(internet["addresses"].as_array().unwrap().len(), 2);
    assert!(
        preview(&app, reference("ipam.ipaddress", vm_a_ip), &cookie).await?["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["host"]["object_id"] == json!(vm_a))
    );
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn scopes_exceed_singleton_address_limit_without_truncating_members() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let name = format!("scope-budget-{}", common::unique_suffix());
    let (seed, _) = common::seed_device(pool, &name).await?;
    let (site, role): (Uuid, Uuid) =
        sqlx::query_as("SELECT site_id,role_id FROM dcim_device WHERE id=$1")
            .bind(seed)
            .fetch_one(pool)
            .await?;
    let vrf = realm(pool, &name, &[]).await?;
    for index in 0..300 {
        scope_vm(
            pool,
            site,
            role,
            None,
            &format!("198.19.{}.{}/24", 20 + index / 250, index % 250 + 1),
            vrf,
        )
        .await?;
    }
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    let selector = reference("dcim.site", site);
    let resolved = preview(&app, selector.clone(), &cookie).await?;
    assert_eq!(resolved["members"].as_array().unwrap().len(), 301);
    assert_eq!(resolved["addresses"].as_array().unwrap().len(), 300);
    assert_eq!(
        resolved["default_ipaddresses"].as_array().unwrap().len(),
        300
    );
    assert_eq!(resolved["ipaddresses"].as_array().unwrap().len(), 300);
    assert!(
        !resolved["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "endpoint_unresolved")
    );
    let mut flow = intent(json!({"object_type":"internet"}), selector);
    flow["status"] = json!("active");
    let id = create_flow(&app, flow, &cookie).await?;
    let vm: Uuid = sqlx::query_scalar(
        "SELECT id FROM virtualization_virtualmachine WHERE site_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(site)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        policy(&app, "virtualization.virtualmachine", vm, &cookie).await?["apply_ready"],
        true
    );
    assert_eq!(
        policy(&app, "virtualization.virtualmachine", vm, &cookie).await?["rules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    call(&app, "DELETE", &format!("{BASE}/{id}"), None, Some(&cookie)).await?;
    Ok(())
}
async fn ip(pool: &sqlx::PgPool, value: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments) VALUES ($1, '{}', $2::inet, 'active', '', '', '')").bind(id).bind(value).execute(pool).await?;
    Ok(id)
}
async fn active_prefix(pool: &sqlx::PgPool, value: &str, vrf: Uuid) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_prefix (id,custom_field_data,prefix,status,is_pool,description,_children,_depth,mark_utilized,comments,vrf_id) VALUES ($1,'{}',$2::cidr,'active',false,'',0,0,false,'',$3)")
        .bind(id).bind(value).bind(vrf).execute(pool).await?;
    Ok(id)
}
async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error + Send + Sync>> {
    let body = body.map(|body| body.to_string());
    let (status, bytes) = common::request(app, method, path, body.as_deref(), cookie).await?;
    Ok((
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        },
    ))
}
fn reference(kind: &str, id: Uuid) -> Value {
    json!({"object_type": kind, "object_id": id})
}
fn intent(source: Value, destination: Value) -> Value {
    json!({"source": source, "destination": destination})
}
async fn policy(
    app: &axum::Router,
    kind: &str,
    id: Uuid,
    cookie: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = call(
        app,
        "GET",
        &format!("{BASE}/firewall-policy?host_type={kind}&host_id={id}"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    Ok(body)
}
async fn user_cookie(
    state: &AppState,
    app: &axum::Router,
    inventory: bool,
    ct: bool,
    denied_peer: Option<Uuid>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::now_v7();
    let username = format!("pwa98-{}", common::unique_suffix());
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')").bind(id).bind(&username).bind(format!("{username}@test.local")).execute(&state.db_pool).await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at) SELECT $1, c.password_hash, now() FROM auth_user_credential c JOIN identity_user u ON u.id = c.user_id WHERE u.username = $2").bind(id).bind(common::TEST_ADMIN_USERNAME).execute(&state.db_pool).await?;
    let subjects = json!([format!("user:{id}")]);
    let mut resources = Vec::new();
    if inventory {
        resources.push(ResourceType::Inventory);
    }
    if ct {
        resources.push(ResourceType::ConnectionTarget);
    }
    if !resources.is_empty() {
        sqlx::query("INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) VALUES ($1, 'allow', $2, $3, $4, '[]')").bind(format!("{username}-allow")).bind(json!([Action::Read])).bind(json!(resources)).bind(&subjects).execute(&state.db_pool).await?;
    }
    if let Some(peer) = denied_peer {
        sqlx::query("INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) VALUES ($1, 'deny', $2, $3, $4, $5)").bind(format!("{username}-deny")).bind(json!([Action::Read])).bind(json!([ResourceType::ConnectionTarget])).bind(&subjects).bind(json!([format!("connection_target:{peer}")])).execute(&state.db_pool).await?;
    }
    common::login_and_get_cookie(app, &username, common::TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| "user login failed".into())
}
async fn assign(pool: &sqlx::PgPool, host: Uuid, ids: &[Uuid]) -> Result<Uuid, sqlx::Error> {
    let interface = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_interface (id, custom_field_data, device_id, name, _name, label, description, type, enabled, mgmt_only, mark_connected) VALUES ($1, '{}', $2, $3, $3, '', '', 'virtual', true, false, false)").bind(interface).bind(host).bind(format!("if-{interface}")).execute(pool).await?;
    sqlx::query("UPDATE ipam_ipaddress SET assigned_object_id = $2, assigned_object_type_id = (SELECT id FROM inventory_content_type WHERE app_label = 'dcim' AND model = 'interface') WHERE id = ANY($1)").bind(ids).bind(interface).execute(pool).await?;
    Ok(interface)
}
async fn create_flow(
    app: &axum::Router,
    payload: Value,
    cookie: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let (status, body) = call(app, "POST", BASE, Some(payload), Some(cookie)).await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Ok(body["id"].as_str().ok_or("missing flow id")?.into())
}
async fn realm(pool: &sqlx::PgPool, name: &str, ids: &[Uuid]) -> Result<Uuid, sqlx::Error> {
    let vrf = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_vrf (id, custom_field_data, name, enforce_unique, description, comments) VALUES ($1, '{}', $2, false, '', '')").bind(vrf).bind(name).execute(pool).await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id = $2 WHERE id = ANY($1)")
        .bind(ids)
        .bind(vrf)
        .execute(pool)
        .await?;
    Ok(vrf)
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn compact_defaults_selectors_builtins_and_endpoint_choices() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let prefix = format!("pwa98-{}", common::unique_suffix());
    let (a, _) = common::seed_device(&pool, &prefix).await?;
    let (b, _) = common::seed_device(&pool, &prefix).await?;
    let (unaddressed, _) = common::seed_device(&pool, &prefix).await?;
    let a4 = ip(&pool, "192.0.2.98/24").await?;
    let a6 = ip(&pool, "2001:db8:98::1/64").await?;
    let b4 = ip(&pool, "192.0.2.99/24").await?;
    let b6 = ip(&pool, "2001:db8:98::2/64").await?;
    let extra = ip(&pool, "198.18.98.10/24").await?;
    let routing_context = realm(&pool, &prefix, &[a4, a6, b4, b6, extra]).await?;
    let _interface = assign(&pool, a, &[a4, a6, extra]).await?;
    assign(&pool, b, &[b4, b6]).await?;
    for (host, v4, v6) in [(a, a4, a6), (b, b4, b6)] {
        sqlx::query(
            "UPDATE dcim_device SET primary_ip4_id = $2, primary_ip6_id = $3 WHERE id = $1",
        )
        .bind(host)
        .bind(v4)
        .bind(v6)
        .execute(&pool)
        .await?;
    }
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    let choices_uri = format!("{BASE}/endpoint-resolution");
    let (status, choices) = call(
        &app,
        "POST",
        &choices_uri,
        Some(reference("dcim.device", a)),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(choices["ipaddresses"].as_array().unwrap().len(), 3);
    assert_eq!(choices["default_ipaddresses"], json!([a4, a6]));
    assert_eq!(choices["ipaddresses"][0]["address"], "192.0.2.98/24");
    let minimal = create_flow(
        &app,
        intent(
            json!({"object_type": "internet"}),
            reference("dcim.device", b),
        ),
        &cookie,
    )
    .await?;
    let (_, saved) = call(
        &app,
        "GET",
        &format!("{BASE}/{minimal}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(saved["status"], "draft");
    assert_eq!(saved["transport"], "any");
    assert!(saved["destination_ports"].is_null());
    assert_eq!(saved["allow_cross_vrf"], false);
    assert!(saved["name"].as_str().unwrap().contains("Internet"));
    assert!(saved["source"]["object_id"].is_null());
    assert_eq!(saved["source"]["address_mode"], "default");
    assert_eq!(
        policy(&app, "dcim.device", b, &cookie).await?["rules"],
        json!([])
    );
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"status": "active"})),
        Some(&cookie),
    )
    .await?;
    let internet = policy(&app, "dcim.device", b, &cookie).await?;
    assert_eq!(internet["apply_ready"], true);
    assert_eq!(internet["rules"].as_array().unwrap().len(), 2);
    assert_eq!(internet["rules"][0]["direction"], "ingress");
    assert_eq!(internet["rules"][0]["transport"], "any");
    assert_eq!(internet["rules"][0]["source"], "0.0.0.0/0");
    assert_eq!(internet["rules"][0]["destination"], "192.0.2.99/32");
    assert!(internet["rules"][0]["destination_ports"].is_null());
    assert_eq!(internet["rules"][1]["source"], "::/0");
    assert_eq!(internet["rules"][1]["destination"], "2001:db8:98::2/128");
    assert_eq!(internet["rules"][0]["vrf_id"], json!(routing_context));
    assert_eq!(
        policy(&app, "dcim.device", a, &cookie).await?["rules"],
        json!([]),
        "wildcard is not inventory ownership"
    );
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"destination_ports": [443, 80, 443]})),
        Some(&cookie),
    )
    .await?;
    let port_intent = policy(&app, "dcim.device", b, &cookie).await?;
    assert_eq!(port_intent["rules"].as_array().unwrap().len(), 6);
    assert!(port_intent["rules"].as_array().unwrap().iter().all(|rule| rule["transport"] != "any" && rule["destination_ports"] == json!([80, 443])));
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"destination_ports": null})),
        Some(&cookie),
    )
    .await?;
    assert!(
        call(
            &app,
            "GET",
            &format!("{BASE}/{minimal}"),
            None,
            Some(&cookie)
        )
        .await?
        .1["destination_ports"]
            .is_null()
    );
    call(
        &app,
        "PUT",
        &format!("{BASE}/{minimal}"),
        Some(json!({"status": "disabled"})),
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        policy(&app, "dcim.device", b, &cookie).await?["rules"],
        json!([])
    );
    let mut all_intent = intent(json!({"object_type": "all"}), reference("dcim.device", b));
    all_intent["status"] = json!("active");
    let all_id = create_flow(&app, all_intent, &cookie).await?;
    let all_b = policy(&app, "dcim.device", b, &cookie).await?;
    assert_eq!(all_b["apply_ready"], true);
    assert!(
        all_b["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["direction"] == "ingress" && rule["source"] != "0.0.0.0/0")
    );
    assert!(
        all_b["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["source"] == "198.18.98.10/32")
    );
    assert!(
        all_b["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "inventory_excluded")
    );
    assert!(
        policy(&app, "dcim.device", a, &cookie).await?["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["direction"] == "egress"
                && ["192.0.2.98/32", "2001:db8:98::1/128", "198.18.98.10/32"]
                    .contains(&rule["source"].as_str().unwrap()))
    );
    assert_eq!(
        policy(&app, "dcim.device", unaddressed, &cookie).await?["rules"],
        json!([])
    );
    call(
        &app,
        "DELETE",
        &format!("{BASE}/{all_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    let selected = json!({"object_type": "dcim.device", "object_id": a, "address_mode": "selected", "ipaddresses": [extra, a4, extra]});
    let mut payload = intent(selected, reference("dcim.device", b));
    payload["status"] = json!("active");
    payload["transport"] = json!("tcp");
    payload["destination_ports"] = json!([443]);
    let selected_id = create_flow(&app, payload.clone(), &cookie).await?;
    let duplicate = create_flow(&app, payload, &cookie).await?;
    let exported = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(exported["apply_ready"], true);
    assert_eq!(exported["rules"].as_array().unwrap().len(), 2);
    assert!(
        exported["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["dataflow_ids"].as_array().unwrap().len() == 2)
    );
    let selected_db_id = Uuid::parse_str(&selected_id)?;
    let selection_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inventory_dataflow_selected_ip WHERE dataflow_id = $1",
    )
    .bind(selected_db_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(selection_count, 2);
    let blocked = sqlx::query("DELETE FROM ipam_ipaddress WHERE id = $1")
        .bind(extra)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        blocked.as_database_error().unwrap().code().as_deref(),
        Some("23001")
    );
    sqlx::query("UPDATE ipam_ipaddress SET status = 'reserved' WHERE id = $1")
        .bind(extra)
        .execute(&pool)
        .await?;
    let unresolved = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(unresolved["apply_ready"], false);
    assert_eq!(unresolved["rules"], json!([]));
    sqlx::query("UPDATE ipam_ipaddress SET status = 'active' WHERE id = $1")
        .bind(extra)
        .execute(&pool)
        .await?;
    call(&app, "PUT", &format!("{BASE}/{selected_id}"), Some(json!({"source": {"object_type": "dcim.device", "object_id": a, "address_mode": "all"}})), Some(&cookie)).await?;
    assert!(
        policy(&app, "dcim.device", a, &cookie).await?["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["source"] == "2001:db8:98::1/128")
    );
    for bad in [
        json!({"source": {"object_type": "internet", "object_id": a}}),
        json!({"source": {"object_type": "dcim.device"}}),
        json!({"source": {"object_type": "dcim.device", "object_id": a, "address_mode": "selected", "ipaddresses": []}}),
        json!({"source": {"object_type": "dcim.device", "object_id": a, "address_mode": "selected", "ipaddresses": [b4]}}),
        json!({"source": {"object_type": "dcim.device", "object_id": a, "address_mode": "selected", "ipaddresses": [Uuid::now_v7()]}}),
        json!({"destination_ports": []}),
        json!({"destination_ports": [0]}),
        json!({"transport": "icmp"}),
        json!({"unexpected": true}),
    ] {
        assert_eq!(
            call(
                &app,
                "PUT",
                &format!("{BASE}/{selected_id}"),
                Some(bad),
                Some(&cookie)
            )
            .await?
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let inventory_only = user_cookie(&state, &app, true, false, None).await?;
    let denied_peer = user_cookie(&state, &app, true, true, Some(b)).await?;
    let no_inventory = user_cookie(&state, &app, false, true, None).await?;
    let policy_uri = format!("{BASE}/firewall-policy?host_type=dcim.device&host_id={a}");
    for user in [
        None,
        Some(inventory_only.as_str()),
        Some(denied_peer.as_str()),
        Some(no_inventory.as_str()),
    ] {
        assert_eq!(
            call(&app, "GET", &policy_uri, None, user).await?.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &choices_uri,
            Some(reference("dcim.device", a)),
            Some(&inventory_only)
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &choices_uri,
            Some(reference("dcim.device", a)),
            None
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            BASE,
            Some(intent(
                reference("dcim.device", a),
                reference("dcim.device", b)
            )),
            Some(&inventory_only)
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, list) = call(
        &app,
        "GET",
        &format!("{BASE}?source_type=dcim.device&source_id={a}&limit=1&offset=99"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["total"], 2);
    assert_eq!(list["items"], json!([]));
    for id in [minimal, selected_id, duplicate] {
        assert_eq!(
            call(&app, "DELETE", &format!("{BASE}/{id}"), None, Some(&cookie))
                .await?
                .0,
            StatusCode::NO_CONTENT
        );
    }
    Ok(())
}
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn service_defaults_bindings_and_routing_contexts() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let prefix = format!("pwa98-service-{}", common::unique_suffix());
    let (a, _) = common::seed_device(&pool, &prefix).await?;
    let (b, _) = common::seed_device(&pool, &prefix).await?;
    let a4 = ip(&pool, "198.18.99.10/24").await?;
    let b4 = ip(&pool, "198.18.99.20/24").await?;
    let routing_context = realm(&pool, &prefix, &[a4, b4]).await?;
    assign(&pool, a, &[a4]).await?;
    assign(&pool, b, &[b4]).await?;
    for (host, address) in [(a, a4), (b, b4)] {
        sqlx::query("UPDATE dcim_device SET primary_ip4_id = $2 WHERE id = $1")
            .bind(host)
            .bind(address)
            .execute(&pool)
            .await?;
    }
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    let (status, service) = call(&app, "POST", "/api/v1/inventory/services", Some(json!({"name": prefix, "protocol": "udp", "ports": [12345], "parent_object": reference("dcim.device", a)})), Some(&cookie)).await?;
    assert_eq!(status, StatusCode::CREATED, "{service}");
    let service_id = Uuid::parse_str(service["id"].as_str().unwrap())?;
    let uri = format!("{BASE}/endpoint-resolution");
    let choices = call(
        &app,
        "POST",
        &uri,
        Some(reference("ipam.service", service_id)),
        Some(&cookie),
    )
    .await?
    .1;
    assert_eq!(choices["default_ipaddresses"], json!([a4]));
    let mut payload = intent(
        reference("ipam.service", service_id),
        reference("dcim.device", b),
    );
    payload["status"] = json!("active");
    payload["transport"] = json!("tcp");
    payload["destination_ports"] = json!([443]);
    let id = create_flow(&app, payload, &cookie).await?;
    let resolved = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(resolved["apply_ready"], true);
    assert_eq!(resolved["rules"][0]["source"], "198.18.99.10/32");
    assert_eq!(resolved["rules"][0]["destination_ports"], json!([443]));
    let service_uri = format!("/api/v1/inventory/services/{service_id}");
    call(&app, "PUT", &format!("{BASE}/{id}"), Some(json!({"source": {"object_type": "ipam.service", "object_id": service_id, "address_mode": "all"}})), Some(&cookie)).await?;
    assert_eq!(
        policy(&app, "dcim.device", a, &cookie).await?["rules"][0]["source"],
        "198.18.99.10/32"
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &service_uri,
            Some(json!({"ipaddresses": [b4]})),
            Some(&cookie)
        )
        .await?
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &service_uri,
            Some(json!({"ipaddresses": [a4, a4]})),
            Some(&cookie)
        )
        .await?
        .1["ipaddresses"],
        json!([a4])
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &service_uri,
            Some(json!({"name": "binding retained"})),
            Some(&cookie)
        )
        .await?
        .1["ipaddresses"],
        json!([a4])
    );
    assert_eq!(
        call(&app, "DELETE", &service_uri, None, Some(&cookie))
            .await?
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "GET", &service_uri, None, Some(&cookie))
            .await?
            .1["ipaddresses"],
        json!([a4])
    );
    let vrf = Uuid::now_v7();
    let bound_choices = call(
        &app,
        "POST",
        &uri,
        Some(reference("ipam.service", service_id)),
        Some(&cookie),
    )
    .await?
    .1;
    assert_eq!(bound_choices["default_ipaddresses"], json!([a4]));
    assert_eq!(bound_choices["ipaddresses"].as_array().unwrap().len(), 1);
    call(&app, "PUT", &format!("{BASE}/{id}"), Some(json!({"source": {"object_type": "ipam.service", "object_id": service_id, "address_mode": "selected", "ipaddresses": [a4]}})), Some(&cookie)).await?;
    assert_eq!(
        policy(&app, "dcim.device", a, &cookie).await?["apply_ready"],
        true
    );
    sqlx::query("INSERT INTO ipam_vrf (id, custom_field_data, name, enforce_unique, description, comments) VALUES ($1, '{}', $2, false, '', '')").bind(vrf).bind(&prefix).execute(&pool).await?;
    let (other, _) = common::seed_device(&pool, &prefix).await?;
    let duplicate_ip = ip(&pool, "198.18.99.10/24").await?;
    assign(&pool, other, &[duplicate_ip]).await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id = $2 WHERE id = $1")
        .bind(duplicate_ip)
        .bind(vrf)
        .execute(&pool)
        .await?;
    assert_eq!(
        policy(&app, "dcim.device", a, &cookie).await?["apply_ready"],
        true,
        "other VRF address reuse is valid"
    );
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id = $2 WHERE id = $1")
        .bind(duplicate_ip)
        .bind(routing_context)
        .execute(&pool)
        .await?;
    let ambiguous = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(ambiguous["apply_ready"], false);
    assert_eq!(ambiguous["rules"], json!([]));
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id = $2 WHERE id = $1")
        .bind(duplicate_ip)
        .bind(vrf)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE ipam_ipaddress SET vrf_id = $2 WHERE id = $1")
        .bind(b4)
        .bind(vrf)
        .execute(&pool)
        .await?;
    assert_eq!(
        policy(&app, "dcim.device", a, &cookie).await?["apply_ready"],
        false
    );
    call(
        &app,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(json!({"allow_cross_vrf": true})),
        Some(&cookie),
    )
    .await?;
    let cross = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(cross["apply_ready"], true);
    assert_eq!(cross["rules"][0]["vrf_id"], json!(routing_context));
    call(&app, "DELETE", &format!("{BASE}/{id}"), None, Some(&cookie)).await?;
    let mut builtin = intent(json!({"object_type": "all"}), reference("dcim.device", a));
    builtin["status"] = json!("active");
    let all_id = create_flow(&app, builtin, &cookie).await?;
    let scoped_policy = policy(&app, "dcim.device", a, &cookie).await?;
    assert_eq!(scoped_policy["apply_ready"], true);
    assert!(
        scoped_policy["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["vrf_id"] == json!(routing_context)
                && rule["source"] != "198.18.99.20/32")
    );
    call(
        &app,
        "DELETE",
        &format!("{BASE}/{all_id}"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(
        call(&app, "DELETE", &service_uri, None, Some(&cookie))
            .await?
            .0,
        StatusCode::NO_CONTENT
    );
    // VM choices use the same catalog, including both primary and assigned IPs.
    let vm = Uuid::now_v7();
    let vm_primary = ip(&pool, "198.18.100.1/24").await?;
    let vm_extra = ip(&pool, "198.18.100.2/24").await?;
    realm(&pool, &format!("{prefix}-vm"), &[vm_primary, vm_extra]).await?;
    sqlx::query("INSERT INTO virtualization_virtualmachine (id, custom_field_data, name, status, comments, description, interface_count, virtual_disk_count, serial, start_on_boot, primary_ip4_id) VALUES ($1, '{}', $2, 'active', '', '', 1, 0, '', '', $3)")
        .bind(vm).bind(&prefix).bind(vm_primary).execute(&pool).await?;
    let interface = Uuid::now_v7();
    sqlx::query("INSERT INTO virtualization_vminterface (id, custom_field_data, virtual_machine_id, name, _name, enabled, description) VALUES ($1, '{}', $2, 'eth0', 'eth0', true, '')")
        .bind(interface).bind(vm).execute(&pool).await?;
    sqlx::query("UPDATE ipam_ipaddress SET assigned_object_id = $2, assigned_object_type_id = (SELECT id FROM inventory_content_type WHERE app_label = 'virtualization' AND model = 'vminterface') WHERE id = $1")
        .bind(vm_extra).bind(interface).execute(&pool).await?;
    let vm_choices_uri = format!("{BASE}/endpoint-resolution");
    let (status, choices) = call(
        &app,
        "POST",
        &vm_choices_uri,
        Some(reference("virtualization.virtualmachine", vm)),
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(choices["default_ipaddresses"], json!([vm_primary]));
    assert_eq!(choices["ipaddresses"].as_array().unwrap().len(), 2);
    let mut vm_intent = intent(
        json!({"object_type": "internet"}),
        json!({"object_type": "virtualization.virtualmachine", "object_id": vm, "address_mode": "all"}),
    );
    vm_intent["status"] = json!("active");
    let vm_flow = create_flow(&app, vm_intent, &cookie).await?;
    let vm_policy = policy(&app, "virtualization.virtualmachine", vm, &cookie).await?;
    assert_eq!(vm_policy["apply_ready"], true);
    assert_eq!(vm_policy["rules"].as_array().unwrap().len(), 2);
    call(
        &app,
        "PUT",
        &format!("{BASE}/{vm_flow}"),
        Some(json!({"destination": reference("ipam.ipaddress", vm_extra)})),
        Some(&cookie),
    )
    .await?;
    let fixed_ip = policy(&app, "virtualization.virtualmachine", vm, &cookie).await?;
    assert_eq!(fixed_ip["apply_ready"], true);
    assert_eq!(fixed_ip["rules"].as_array().unwrap().len(), 1);
    assert_eq!(fixed_ip["rules"][0]["destination"], "198.18.100.2/32");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{BASE}/endpoint-resolution"),
            Some(reference("ipam.ipaddress", vm_extra)),
            Some(&cookie)
        )
        .await?
        .0,
        StatusCode::OK
    );
    call(
        &app,
        "DELETE",
        &format!("{BASE}/{vm_flow}"),
        None,
        Some(&cookie),
    )
    .await?;
    Ok(())
}
