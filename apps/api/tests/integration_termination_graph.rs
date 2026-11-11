//! Source-prepared PWA-80 product regressions. Main owns compilation and execution.
//! Each ignored case requires a separately provisioned, EMPTY, exclusively owned
//! PostgreSQL database and an explicitly selected, exclusively owned Redis DB.
//! No database creation, reset, drop, URL fallback, or shared common setup is used.
mod common;

use std::{path::Path, sync::Arc, time::Duration};

use axum::{Router, http::StatusCode};
use piping_backend::contexts::access_control::domain::{Action, ResourceType};
use piping_backend::contexts::inventory::application::{
    BackupService, FullBackupImportAction, FullBackupImportRequest, FullBackupImportRules,
    FullBackupSelection,
};
use serde_json::{Value, json};
use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const VCTS: &str = "/api/v1/inventory/virtual-circuit-terminations";
const VPNS: &str = "/api/v1/inventory/l2vpns";
const TERMS: &str = "/api/v1/inventory/l2vpn-terminations";
const OWNERS: [(&str, &str, &str); 3] = [
    (
        "circuits",
        "virtualcircuittermination",
        "circuits_virtualcircuittermination",
    ),
    ("vpn", "l2vpn", "vpn_l2vpn"),
    ("vpn", "l2vpntermination", "vpn_l2vpntermination"),
];

struct ExclusiveSelection {
    database: String,
    redis: String,
}

impl ExclusiveSelection {
    /// All environment checks precede even the first connection. Deliberately
    /// return an error, never an early successful skip, when consent is absent.
    fn from_env() -> TestResult<Self> {
        if std::env::var("PWA80_TERMINATION_TEST_EXCLUSIVE").as_deref() != Ok("1") {
            return Err("set PWA80_TERMINATION_TEST_EXCLUSIVE=1 only for exclusively owned PostgreSQL and Redis".into());
        }
        let database = std::env::var("PWA80_TERMINATION_TEST_DATABASE_URL")
            .map_err(|_| "PWA80_TERMINATION_TEST_DATABASE_URL is required; no fallback")?;
        let redis = std::env::var("PWA80_TERMINATION_TEST_REDIS_URL")
            .map_err(|_| "PWA80_TERMINATION_TEST_REDIS_URL is required; no fallback")?;
        let db = Url::parse(&database).map_err(|_| "invalid explicit PostgreSQL URL")?;
        let name = db.path().strip_prefix('/').ok_or("missing database name")?;
        let suffix = name.strip_prefix("pwa80_termination_").ok_or(
            "database name must start with pwa80_termination_ and be task-specific disposable data",
        )?;
        let database_query: Vec<_> = db.query_pairs().collect();
        let socket_selected = database_query.len() == 1
            && database_query[0].0 == "host"
            && Path::new(database_query[0].1.as_ref()).is_absolute();
        if !matches!(db.scheme(), "postgres" | "postgresql")
            || db.host_str().is_none()
            || (!database_query.is_empty() && !socket_selected)
            || db.fragment().is_some()
            || suffix.len() < 8
            || !suffix
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err("unsafe explicit PostgreSQL selection".into());
        }
        let cache = Url::parse(&redis).map_err(|_| "invalid explicit Redis URL")?;
        let cache_query: Vec<_> = cache.query_pairs().collect();
        let logical_db = match cache.scheme() {
            "redis" | "rediss" if cache.host_str().is_some() && cache_query.is_empty() => cache
                .path()
                .strip_prefix('/')
                .and_then(|s| s.parse::<u8>().ok()),
            "redis+unix"
                if cache.host_str().is_none()
                    && Path::new(cache.path()).is_absolute()
                    && cache_query.len() == 1
                    && cache_query[0].0 == "db" =>
            {
                cache_query[0].1.parse::<u8>().ok()
            }
            _ => None,
        };
        if cache.fragment().is_some() || !logical_db.is_some_and(|db| (1..=15).contains(&db)) {
            return Err("select an exclusive Redis logical DB from 1 through 15 explicitly".into());
        }
        // common setup is intentionally not called, but reject a divergent
        // selection now so later helper changes cannot reach the default DB.
        if std::env::var("PIPING_DATABASE__URL").ok().as_deref() != Some(database.as_str())
            || common::database_url() != database
            || std::env::var("PIPING_REDIS__URL").ok().as_deref() != Some(redis.as_str())
            || common::redis_url(None)? != redis
        {
            return Err("PIPING_DATABASE__URL and PIPING_REDIS__URL must exactly match the explicit PWA80 selections".into());
        }
        Ok(Self { database, redis })
    }

    async fn empty_pool(&self) -> TestResult<PgPool> {
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .acquire_timeout(Duration::from_secs(20))
            .connect(&self.database)
            .await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','S')",
        ).fetch_one(&pool).await?;
        if count != 0 {
            return Err("selected database is not empty; provision a fresh exclusive database, do not reset shared data".into());
        }
        Ok(pool)
    }
}

fn merge(mut base: Value, change: Value) -> Value {
    base.as_object_mut()
        .expect("object base")
        .extend(change.as_object().expect("object change").clone());
    base
}

#[test]
fn public_openapi_validates_real_termination_write_boundaries() -> TestResult {
    use utoipa::OpenApi;
    let doc = serde_json::to_value(piping_backend::api::openapi::ApiDoc::openapi())?;
    let id = Uuid::from_u128(0x80);
    let tag = "00000000-0000-4000-8000-000000000081";
    for (name, base, invalid) in [
        (
            "VirtualCircuitTerminationCreatePayload",
            json!({"virtual_circuit_id":id,"interface_id":id}),
            vec![
                json!({"role":"primary"}),
                json!({"interface_id":null}),
                json!({"assigned_object":endpoint("dcim.interface",id)}),
                json!({"description":"x".repeat(201)}),
            ],
        ),
        (
            "VirtualCircuitTerminationPatchPayload",
            json!({}),
            vec![
                json!({"virtual_circuit_id":null}),
                json!({"interface_id":null}),
                json!({"description":null}),
                json!({"role":null}),
            ],
        ),
        (
            "L2VpnCreatePayload",
            json!({"name":"Edge","slug":"edge","type":"vpls"}),
            vec![
                json!({"name":""}),
                json!({"name":"x".repeat(101)}),
                json!({"slug":"bad slug"}),
                json!({"type":"unknown"}),
                json!({"status":"disabled"}),
                json!({"identifier":9223372036854775807_i64}),
                json!({"identifier":"01"}),
                json!({"import_targets":null}),
                json!({"export_targets":[tag,tag]}),
            ],
        ),
        (
            "L2VpnPatchPayload",
            json!({}),
            vec![
                json!({"name":null}),
                json!({"slug":null}),
                json!({"type":null}),
                json!({"comments":null}),
                json!({"import_targets":null}),
                json!({"export_targets":null}),
            ],
        ),
        (
            "L2VpnTerminationCreatePayload",
            term(id, "dcim.interface", id),
            vec![
                json!({"l2vpn_id":null}),
                json!({"assigned_object":null}),
                json!({"assigned_object":endpoint("dcim.device",id)}),
                json!({"assigned_object":{"object_type":"ipam.vlan","object_id":id,"label":"client label"}}),
                json!({"name":"not a PrimaryModel"}),
                json!({"description":"not inherited"}),
                json!({"owner_id":id}),
                json!({"tenant_id":id}),
                json!({"comments":"not inherited"}),
            ],
        ),
        (
            "L2VpnTerminationPatchPayload",
            json!({}),
            vec![
                json!({"l2vpn_id":null}),
                json!({"assigned_object":null}),
                json!({"name":"phantom"}),
                json!({"description":"phantom"}),
                json!({"owner_id":null}),
                json!({"comments":"phantom"}),
            ],
        ),
    ] {
        let schema =
            json!({"$ref":format!("#/components/schemas/{name}"),"components":doc["components"]});
        let validator = jsonschema::validator_for(&schema)?;
        assert!(validator.is_valid(&base), "valid {name} base rejected");
        for edit in [
            json!({"tags":[]}),
            json!({"tags":[tag]}),
            json!({"custom_fields":{}}),
        ] {
            assert!(
                validator.is_valid(&merge(base.clone(), edit)),
                "valid {name} metadata rejected"
            );
        }
        for edit in invalid.into_iter().chain([
            json!({"tags":null}),
            json!({"tags":[tag,tag]}),
            json!({"custom_fields":null}),
            json!({"created":"2000-01-01T00:00:00Z"}),
            json!({"last_updated":"2000-01-01T00:00:00Z"}),
        ]) {
            assert!(
                !validator.is_valid(&merge(base.clone(), edit.clone())),
                "invalid {name}: {edit}"
            );
        }
        if name.starts_with("L2Vpn") && !name.starts_with("L2VpnTermination") {
            for edit in [
                json!({"identifier":null,"owner_id":null,"tenant_id":null}),
                json!({"identifier":"-9223372036854775808"}),
                json!({"identifier":"9223372036854775807"}),
                json!({"import_targets":[],"export_targets":[]}),
            ] {
                assert!(
                    validator.is_valid(&merge(base.clone(), edit)),
                    "valid {name} nullable/decimal/array boundary rejected"
                );
            }
        }
        if name.starts_with("L2VpnTermination") {
            for kind in ["dcim.interface", "virtualization.vminterface", "ipam.vlan"] {
                assert!(validator.is_valid(&merge(
                    base.clone(),
                    json!({"assigned_object":endpoint(kind,id)})
                )));
            }
        }
    }
    Ok(())
}

#[test]
fn identifier_parser_and_serialization_preserve_signed_i64_without_browser_rounding() -> TestResult
{
    use piping_backend::contexts::inventory::domain::vpn::l2vpn::{L2Vpn, parse_identifier};
    for number in [i64::MIN, -1, 0, 9_007_199_254_740_993, i64::MAX] {
        let text = number.to_string();
        assert_eq!(parse_identifier(&text)?, number);
        let model = L2Vpn {
            identifier: Some(number),
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(model)?["identifier"], text);
    }
    for invalid in [
        "",
        "+1",
        "01",
        "-0",
        " 1",
        "1 ",
        "1.0",
        "1e2",
        "9223372036854775808",
        "-9223372036854775809",
    ] {
        assert!(parse_identifier(invalid).is_err(), "accepted {invalid:?}");
    }
    assert!(serde_json::to_value(L2Vpn::default())?["identifier"].is_null());
    Ok(())
}

#[test]
fn serde_write_contract_distinguishes_omission_clear_and_inherited_fields() -> TestResult {
    use piping_backend::api::rest::routes::inventory::{
        circuits::virtual_circuit_terminations::{
            VirtualCircuitTerminationCreatePayload, VirtualCircuitTerminationPatchPayload,
        },
        vpn::{l2vpn_terminations::L2VpnTerminationPatchPayload, l2vpns::L2VpnPatchPayload},
    };
    let id = Uuid::from_u128(0x80);
    let vct: VirtualCircuitTerminationCreatePayload =
        serde_json::from_value(json!({"virtual_circuit_id":id,"interface_id":id}))?;
    assert_eq!(vct.role, "peer");
    let absent: L2VpnPatchPayload = serde_json::from_value(json!({}))?;
    assert_eq!(absent.identifier, None);
    assert_eq!(absent.owner_id, None);
    assert_eq!(absent.tenant_id, None);
    assert_eq!(absent.import_targets, None);
    let clear: L2VpnPatchPayload = serde_json::from_value(
        json!({"identifier":null,"owner_id":null,"tenant_id":null,"import_targets":[],"export_targets":[]}),
    )?;
    assert_eq!(clear.identifier, Some(None));
    assert_eq!(clear.owner_id, Some(None));
    assert_eq!(clear.tenant_id, Some(None));
    assert_eq!(clear.import_targets, Some(vec![]));
    assert_eq!(clear.export_targets, Some(vec![]));
    for invalid in [
        json!({"role":null}),
        json!({"interface_id":null}),
        json!({"virtual_circuit_id":null}),
        json!({"tags":null}),
        json!({"comments":"not inherited"}),
    ] {
        assert!(serde_json::from_value::<VirtualCircuitTerminationPatchPayload>(invalid).is_err());
    }
    for invalid in [
        json!({"l2vpn_id":null}),
        json!({"assigned_object":null}),
        json!({"assigned_object":{"object_type":"ipam.vlan","object_id":id,"label":"injected"}}),
        json!({"description":"not inherited"}),
        json!({"owner_id":null}),
        json!({"custom_fields":null}),
    ] {
        assert!(serde_json::from_value::<L2VpnTerminationPatchPayload>(invalid).is_err());
    }
    Ok(())
}

fn endpoint(kind: &str, id: Uuid) -> Value {
    json!({"object_type":kind,"object_id":id})
}
fn term(parent: Uuid, kind: &str, id: Uuid) -> Value {
    json!({"l2vpn_id":parent,"assigned_object":endpoint(kind,id)})
}
fn row_id(row: &Value) -> TestResult<Uuid> {
    Ok(Uuid::parse_str(
        row["id"].as_str().ok_or("response UUID missing")?,
    )?)
}
async fn call(
    app: &Router,
    cookie: Option<&str>,
    method: &str,
    path: &str,
    payload: Option<Value>,
    expected: StatusCode,
) -> TestResult<Value> {
    let body = payload.map(|v| v.to_string());
    let (status, bytes) = common::request(app, method, path, body.as_deref(), cookie).await?;
    // Do not print response bodies: reveal endpoints and token creation contain secrets.
    assert_eq!(status, expected, "{method} {path}");
    if expected == StatusCode::NO_CONTENT {
        assert!(bytes.is_empty(), "204 must have no response body");
    }
    Ok(if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    })
}
async fn post(app: &Router, cookie: &str, path: &str, payload: Value) -> TestResult<Value> {
    call(
        app,
        Some(cookie),
        "POST",
        path,
        Some(payload),
        StatusCode::CREATED,
    )
    .await
}
async fn vpn(app: &Router, cookie: &str, kind: &str) -> TestResult<Uuid> {
    let name = format!("termination-{}", Uuid::now_v7());
    row_id(
        &post(
            app,
            cookie,
            VPNS,
            json!({"name":name,"slug":name,"type":kind}),
        )
        .await?,
    )
}
async fn dcim_interface(pool: &PgPool, kind: &str) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_interface(id,custom_field_data,name,label,description,mark_connected,enabled,_name,type,mgmt_only) VALUES($1,'{}',$2,'','',false,true,$2,$3,false)")
        .bind(id).bind(id.to_string()).bind(kind).execute(pool).await?;
    Ok(id)
}
async fn virtual_circuit(pool: &PgPool) -> TestResult<Uuid> {
    let provider = Uuid::now_v7();
    let network = Uuid::now_v7();
    let kind = Uuid::now_v7();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO circuits_provider(id,name,slug,description,comments,custom_field_data) VALUES($1,$2,$2,'','','{}')").bind(provider).bind(provider.to_string()).execute(pool).await?;
    sqlx::query("INSERT INTO circuits_providernetwork(id,provider_id,name,service_id,description,comments,custom_field_data) VALUES($1,$2,$3,'','','','{}')").bind(network).bind(provider).bind(network.to_string()).execute(pool).await?;
    sqlx::query("INSERT INTO circuits_virtualcircuittype(id,name,slug,color,description,comments,custom_field_data) VALUES($1,$2,$2,'155dfc','','','{}')").bind(kind).bind(kind.to_string()).execute(pool).await?;
    sqlx::query("INSERT INTO circuits_virtualcircuit(id,provider_network_id,type_id,cid,status,description,comments,custom_field_data) VALUES($1,$2,$3,$4,'active','','','{}')").bind(id).bind(network).bind(kind).bind(id.to_string()).execute(pool).await?;
    Ok(id)
}
async fn equal_uuid_endpoints(pool: &PgPool) -> TestResult<Uuid> {
    let id = dcim_interface(pool, "virtual").await?;
    let vm = Uuid::now_v7();
    sqlx::query("INSERT INTO virtualization_virtualmachine(id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot) VALUES($1,'{}',$2,'active','','',0,0,'','enabled')").bind(vm).bind(vm.to_string()).execute(pool).await?;
    sqlx::query("INSERT INTO virtualization_vminterface(id,custom_field_data,name,_name,enabled,description,mtu,virtual_machine_id) VALUES($1,'{}','eth0','eth0',true,'',1500,$2)").bind(id).bind(vm).execute(pool).await?;
    sqlx::query("INSERT INTO ipam_vlan(id,name,vid,status,description,comments,custom_field_data) VALUES($1,$2,80,'active','','','{}')").bind(id).bind(id.to_string()).execute(pool).await?;
    Ok(id)
}

struct Native {
    pool: PgPool,
    app: Router,
    cookie: String,
    redis: redis::aio::ConnectionManager,
}
async fn native() -> TestResult<Native> {
    let selection = ExclusiveSelection::from_env()?;
    let pool = selection.empty_pool().await?;
    // Even the settings/cipher bootstrap is after the one fail-closed guard.
    let mut settings = piping_backend::config::load_settings()?;
    settings.database.url = selection.database;
    settings.redis.url = selection.redis;
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    common::configure_test_cipher(&mut settings);
    Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await?
    .run(&pool)
    .await?;
    common::ensure_test_admin(&pool, &settings.auth.password_pepper).await?;
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    invalidate(&mut redis).await?;
    let state =
        piping_backend::app::AppState::build(Arc::new(settings), pool.clone(), redis.clone())
            .await?;
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    Ok(Native {
        pool,
        app,
        cookie,
        redis,
    })
}
async fn invalidate(redis: &mut redis::aio::ConnectionManager) -> TestResult {
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(redis)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "Main only: requires explicit exclusive empty PWA80 PostgreSQL and Redis selections; mutates both"]
async fn native_termination_crud_graph_atomicity_authorization_and_restore() -> TestResult {
    let mut n = native().await?;
    for collection in [VCTS, VPNS, TERMS] {
        call(&n.app, None, "GET", collection, None, StatusCode::FORBIDDEN).await?;
    }
    let parent = virtual_circuit(&n.pool).await?;
    let mut paths = Vec::new();
    // More than two peers and repeated hubs are legal: there is no role cap.
    for (kind, role) in [
        ("virtual", None),
        ("lag", Some("peer")),
        ("bridge", Some("peer")),
        ("virtual", Some("hub")),
        ("virtual", Some("hub")),
        ("virtual", Some("spoke")),
    ] {
        let interface = dcim_interface(&n.pool, kind).await?;
        let mut body = json!({"virtual_circuit_id":parent,"interface_id":interface});
        if let Some(role) = role {
            body["role"] = json!(role);
        }
        let row = post(&n.app, &n.cookie, VCTS, body.clone()).await?;
        assert_eq!(row["role"], role.unwrap_or("peer"));
        assert!(row["created"].is_string());
        let path = format!("{VCTS}/{}", row_id(&row)?);
        call(
            &n.app,
            Some(&n.cookie),
            "POST",
            VCTS,
            Some(body),
            StatusCode::CONFLICT,
        )
        .await?;
        paths.push(path);
    }
    let physical = dcim_interface(&n.pool, "1000base-t").await?;
    call(
        &n.app,
        Some(&n.cookie),
        "POST",
        VCTS,
        Some(json!({"virtual_circuit_id":parent,"interface_id":physical})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let only_vm = equal_uuid_endpoints(&n.pool).await?;
    // Remove only the DCIM identity; a same-UUID VMInterface must not substitute.
    sqlx::query("DELETE FROM dcim_interface WHERE id=$1")
        .bind(only_vm)
        .execute(&n.pool)
        .await?;
    call(
        &n.app,
        Some(&n.cookie),
        "POST",
        VCTS,
        Some(json!({"virtual_circuit_id":parent,"interface_id":only_vm})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let vpn_id = vpn(&n.app, &n.cookie, "vpls").await?;
    let vpn_path = format!("{VPNS}/{vpn_id}");
    let same = equal_uuid_endpoints(&n.pool).await?;
    let mut typed = Vec::new();
    for kind in ["dcim.interface", "virtualization.vminterface", "ipam.vlan"] {
        let row = post(&n.app, &n.cookie, TERMS, term(vpn_id, kind, same)).await?;
        assert_eq!(row["assigned_object"]["object_type"], kind);
        assert_eq!(row["assigned_object"]["object_id"], same.to_string());
        assert!(row["assigned_object"]["label"].is_string());
        for phantom in [
            "name",
            "slug",
            "owner_id",
            "tenant_id",
            "description",
            "comments",
        ] {
            assert!(
                row.get(phantom).is_none(),
                "termination leaked PrimaryModel field {phantom}"
            );
        }
        let path = format!("{TERMS}/{}", row_id(&row)?);
        paths.push(path.clone());
        typed.push(path);
        let other = vpn(&n.app, &n.cookie, "vpls").await?;
        call(
            &n.app,
            Some(&n.cookie),
            "POST",
            TERMS,
            Some(term(other, kind, same)),
            StatusCode::CONFLICT,
        )
        .await?;
    }
    paths.push(vpn_path.clone());
    for (collection, query, expected) in [
        (
            VCTS,
            format!("virtual_circuit_id={parent}&sort=role"),
            6_u64,
        ),
        (TERMS, format!("l2vpn_id={vpn_id}"), 3_u64),
        (
            VPNS,
            format!(
                "name={}",
                call(
                    &n.app,
                    Some(&n.cookie),
                    "GET",
                    &vpn_path,
                    None,
                    StatusCode::OK
                )
                .await?["name"]
                    .as_str()
                    .ok_or("VPN name")?
            ),
            1_u64,
        ),
    ] {
        let page = call(
            &n.app,
            Some(&n.cookie),
            "GET",
            &format!("{collection}?{query}&limit=1&offset=100"),
            None,
            StatusCode::OK,
        )
        .await?;
        assert_eq!(page["total"], expected);
        assert_eq!(page["items"], json!([]));
    }
    for path in &paths {
        let before = call(&n.app, Some(&n.cookie), "GET", path, None, StatusCode::OK).await?;
        let after = call(
            &n.app,
            Some(&n.cookie),
            "PATCH",
            path,
            Some(json!({})),
            StatusCode::OK,
        )
        .await?;
        assert_eq!(before["id"], after["id"]);
        assert_eq!(before["created"], after["created"]);
        assert_eq!(before["custom_fields"], after["custom_fields"]);
        metadata_contract(&n, path).await?;
    }
    vpn_nullable_and_targets(&n, &vpn_path).await?;
    concurrency(&n).await?;
    direct_graph_constraints(&n).await?;
    object_and_reference_permissions(&mut n, &paths[0], &vpn_path, &typed[0], parent, same).await?;
    conditional_reference_attributes(&mut n, &paths[0], &typed[0], same).await?;
    secret_atomicity_and_retirement(&n).await?;
    restore_cascade_and_rollback(&n).await?;
    // Typed cascade must not delete same UUID endpoints or their other children.
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &format!("/api/v1/inventory/interfaces/{same}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    call(
        &n.app,
        Some(&n.cookie),
        "GET",
        &typed[0],
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    for path in &typed[1..] {
        call(&n.app, Some(&n.cookie), "GET", path, None, StatusCode::OK).await?;
    }
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &vpn_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    for path in &typed[1..] {
        call(
            &n.app,
            Some(&n.cookie),
            "GET",
            path,
            None,
            StatusCode::NOT_FOUND,
        )
        .await?;
    }
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &vpn_path,
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &paths[0],
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    call(
        &n.app,
        Some(&n.cookie),
        "GET",
        &paths[0],
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    n.pool.close().await;
    Ok(())
}

async fn metadata_contract(n: &Native, path: &str) -> TestResult {
    let suffix = Uuid::now_v7().to_string();
    let tag = post(
        &n.app,
        &n.cookie,
        "/api/v1/inventory/tags",
        json!({"name":suffix,"slug":suffix,"color":"155dfc","weight":0}),
    )
    .await?;
    let id = row_id(&tag)?;
    let row = call(
        &n.app,
        Some(&n.cookie),
        "PATCH",
        path,
        Some(json!({"tags":[id],"custom_fields":{}})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(row["tags"], json!([id]));
    for invalid in [
        json!({"tags":null}),
        json!({"tags":[id,id]}),
        json!({"tags":[Uuid::now_v7()]}),
        json!({"custom_fields":null}),
        json!({"created":"2000-01-01T00:00:00Z"}),
    ] {
        call(
            &n.app,
            Some(&n.cookie),
            "PATCH",
            path,
            Some(invalid),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    if path.starts_with(TERMS) {
        for invalid in [
            json!({"name":"phantom"}),
            json!({"comments":"phantom"}),
            json!({"description":"phantom"}),
            json!({"owner_id":null}),
            json!({"assigned_object":null}),
            json!({"assigned_object":{"object_type":"ipam.vlan","object_id":id,"label":"injected"}}),
        ] {
            call(
                &n.app,
                Some(&n.cookie),
                "PATCH",
                path,
                Some(invalid),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
    }
    let kept = call(
        &n.app,
        Some(&n.cookie),
        "PATCH",
        path,
        Some(json!({})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(kept["tags"], json!([id]));
    let cleared = call(
        &n.app,
        Some(&n.cookie),
        "PATCH",
        path,
        Some(json!({"tags":[]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(cleared["tags"], json!([]));
    Ok(())
}

async fn vpn_nullable_and_targets(n: &Native, path: &str) -> TestResult {
    let owner = post(
        &n.app,
        &n.cookie,
        "/api/v1/inventory/owners",
        json!({"name":"Termination owner"}),
    )
    .await?;
    let tenant = post(
        &n.app,
        &n.cookie,
        "/api/v1/inventory/tenants",
        json!({"name":"Termination tenant","slug":"termination-tenant"}),
    )
    .await?;
    let rt = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_routetarget(id,name,description,comments,custom_field_data) VALUES($1,'65000:80','','','{}')").bind(rt).execute(&n.pool).await?;
    let body = json!({"identifier":"-9223372036854775808","owner_id":row_id(&owner)?,"tenant_id":row_id(&tenant)?,"import_targets":[rt],"export_targets":[rt],"comments":"retained"});
    let row = call(
        &n.app,
        Some(&n.cookie),
        "PATCH",
        path,
        Some(body),
        StatusCode::OK,
    )
    .await?;
    let kept = call(
        &n.app,
        Some(&n.cookie),
        "PATCH",
        path,
        Some(json!({"description":"changed"})),
        StatusCode::OK,
    )
    .await?;
    for key in [
        "identifier",
        "owner_id",
        "tenant_id",
        "import_targets",
        "export_targets",
        "comments",
    ] {
        assert_eq!(kept[key], row[key], "omission lost {key}");
    }
    for identifier in [
        json!(42),
        json!("01"),
        json!("-0"),
        json!("9223372036854775808"),
    ] {
        call(
            &n.app,
            Some(&n.cookie),
            "PATCH",
            path,
            Some(json!({"identifier":identifier})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    for key in ["import_targets", "export_targets"] {
        for value in [Value::Null, json!([rt, rt]), json!([Uuid::now_v7()])] {
            call(
                &n.app,
                Some(&n.cookie),
                "PATCH",
                path,
                Some(json!({key:value})),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
    }
    let row = call(&n.app,Some(&n.cookie),"PATCH",path,Some(json!({"identifier":null,"owner_id":null,"tenant_id":null,"import_targets":[],"export_targets":[]})),StatusCode::OK).await?;
    for key in [
        "identifier",
        "owner_id",
        "owner_name",
        "tenant_id",
        "tenant_name",
    ] {
        assert!(row[key].is_null());
    }
    assert_eq!(row["import_targets"], json!([]));
    assert_eq!(row["export_targets"], json!([]));
    Ok(())
}

async fn race(
    n: &Native,
    first_path: &str,
    first_method: &str,
    first: Value,
    second_path: &str,
    second_method: &str,
    second: Value,
) -> TestResult<[StatusCode; 2]> {
    let first = first.to_string();
    let second = second.to_string();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(
            common::request(
                &n.app,
                first_method,
                first_path,
                Some(&first),
                Some(&n.cookie)
            ),
            common::request(
                &n.app,
                second_method,
                second_path,
                Some(&second),
                Some(&n.cookie)
            )
        )
    })
    .await?;
    Ok([result.0?.0, result.1?.0])
}
async fn concurrency(n: &Native) -> TestResult {
    let vc = virtual_circuit(&n.pool).await?;
    let interface = dcim_interface(&n.pool, "virtual").await?;
    let payload = json!({"virtual_circuit_id":vc,"interface_id":interface});
    let statuses = race(n, VCTS, "POST", payload.clone(), VCTS, "POST", payload).await?;
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::CONFLICT));
    for kind in ["vpws", "epl", "ep-lan", "ep-tree"] {
        let parent = vpn(&n.app, &n.cookie, kind).await?;
        let a = dcim_interface(&n.pool, "virtual").await?;
        let b = dcim_interface(&n.pool, "virtual").await?;
        let c = dcim_interface(&n.pool, "virtual").await?;
        post(&n.app, &n.cookie, TERMS, term(parent, "dcim.interface", a)).await?;
        let statuses = race(
            n,
            TERMS,
            "POST",
            term(parent, "dcim.interface", b),
            TERMS,
            "POST",
            term(parent, "dcim.interface", c),
        )
        .await?;
        assert_eq!(
            statuses
                .iter()
                .filter(|s| **s == StatusCode::CREATED)
                .count(),
            1,
            "P2P race {kind}"
        );
        assert!(statuses.iter().all(|s| matches!(
            *s,
            StatusCode::CREATED | StatusCode::BAD_REQUEST | StatusCode::CONFLICT
        )));
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM vpn_l2vpntermination WHERE l2vpn_id=$1")
                .bind(parent)
                .fetch_one(&n.pool)
                .await?;
        assert_eq!(count, 2);
    }
    let parent = vpn(&n.app, &n.cookie, "vpls").await?;
    let other = vpn(&n.app, &n.cookie, "vpls").await?;
    let target = dcim_interface(&n.pool, "virtual").await?;
    let statuses = race(
        n,
        TERMS,
        "POST",
        term(parent, "dcim.interface", target),
        TERMS,
        "POST",
        term(other, "dcim.interface", target),
    )
    .await?;
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::CONFLICT));
    for _ in 0..2 {
        let id = dcim_interface(&n.pool, "virtual").await?;
        post(&n.app, &n.cookie, TERMS, term(parent, "dcim.interface", id)).await?;
    }
    // Make exactly two children before racing a third against type reduction.
    sqlx::query("DELETE FROM vpn_l2vpntermination WHERE l2vpn_id=$1 AND assigned_object_id=$2")
        .bind(parent)
        .bind(target)
        .execute(&n.pool)
        .await?;
    let path = format!("{VPNS}/{parent}");
    let third = dcim_interface(&n.pool, "virtual").await?;
    let statuses = race(
        n,
        &path,
        "PATCH",
        json!({"type":"vpws"}),
        TERMS,
        "POST",
        term(parent, "dcim.interface", third),
    )
    .await?;
    assert!(
        !(statuses[0] == StatusCode::OK && statuses[1] == StatusCode::CREATED),
        "both conflicting graph transitions committed"
    );
    assert!(
        statuses[0] == StatusCode::OK || statuses[1] == StatusCode::CREATED,
        "neither valid serialized transition committed"
    );
    let (kind,count):(String,i64)=sqlx::query_as("SELECT p.type,count(t.id) FROM vpn_l2vpn p LEFT JOIN vpn_l2vpntermination t ON t.l2vpn_id=p.id WHERE p.id=$1 GROUP BY p.type").bind(parent).fetch_one(&n.pool).await?;
    assert!(kind == "vpls" || count <= 2);
    // evpl is NOT in the upstream P2P set despite its descriptive label.
    let parent = vpn(&n.app, &n.cookie, "evpl").await?;
    for _ in 0..3 {
        let id = dcim_interface(&n.pool, "virtual").await?;
        post(&n.app, &n.cookie, TERMS, term(parent, "dcim.interface", id)).await?;
    }
    call(
        &n.app,
        Some(&n.cookie),
        "PATCH",
        &format!("{VPNS}/{parent}"),
        Some(json!({"type":"epl"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    Ok(())
}

async fn object_and_reference_permissions(
    n: &mut Native,
    vct: &str,
    vpn: &str,
    term_path: &str,
    vc: Uuid,
    interface: Uuid,
) -> TestResult {
    let saved: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(&n.pool)
            .await?;
    let paths = [vct, vpn, term_path];
    let resources: Vec<_> = paths
        .iter()
        .map(|p| format!("inventory:{}", p.rsplit('/').next().expect("id")))
        .collect();
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(json!(resources))
        .execute(&n.pool)
        .await?;
    invalidate(&mut n.redis).await?;
    let result: TestResult = async {
        for path in paths {
            call(&n.app, Some(&n.cookie), "GET", path, None, StatusCode::OK).await?;
            call(
                &n.app,
                Some(&n.cookie),
                "PATCH",
                path,
                Some(json!({"tags":[]})),
                StatusCode::OK,
            )
            .await?;
        }
        for collection in [VCTS, VPNS, TERMS] {
            call(
                &n.app,
                Some(&n.cookie),
                "GET",
                collection,
                None,
                StatusCode::FORBIDDEN,
            )
            .await?;
        }
        for (stage, path, body) in [
            (
                "virtual circuit parent",
                vct,
                json!({"virtual_circuit_id":vc}),
            ),
            (
                "virtual circuit interface",
                vct,
                json!({"interface_id":interface}),
            ),
            (
                "L2VPN endpoint",
                term_path,
                json!({"assigned_object":endpoint("dcim.interface",interface)}),
            ),
        ] {
            eprintln!("termination scoped reference check: {stage}");
            call(
                &n.app,
                Some(&n.cookie),
                "PATCH",
                path,
                Some(body),
                StatusCode::FORBIDDEN,
            )
            .await?;
        }
        Ok(())
    }
    .await;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(saved)
        .execute(&n.pool)
        .await?;
    invalidate(&mut n.redis).await?;
    result?;
    for path in paths {
        let id = path.rsplit('/').next().ok_or("resource id")?;
        for actions in [vec!["read"], vec!["update"], vec!["read", "update"]] {
            let scopes:Vec<_>=actions.iter().map(|a|json!({"action":a,"resource_type":"inventory","resource_pattern":format!("inventory:{id}")})).collect();
            let token = post(
                &n.app,
                &n.cookie,
                "/api/v1/user/settings/api-tokens",
                json!({"name":Uuid::now_v7().to_string(),"scopes":scopes}),
            )
            .await?;
            let bearer = token["token_value"].as_str().ok_or("PAT value")?;
            for (method, body, allowed) in [
                ("GET", None, actions.contains(&"read")),
                (
                    "PATCH",
                    Some(json!({"tags":[]})),
                    actions.contains(&"update"),
                ),
                ("DELETE", None, false),
            ] {
                let body = body.map(|v| v.to_string());
                let (status, _) = common::request_with_auth(
                    &n.app,
                    method,
                    path,
                    body.as_deref(),
                    None,
                    Some(bearer),
                )
                .await?;
                assert_eq!(
                    status,
                    if allowed {
                        StatusCode::OK
                    } else {
                        StatusCode::FORBIDDEN
                    },
                    "PAT ceiling {method}"
                );
            }
            // UPDATE on the child does not grant READ on a supplied endpoint.
            if actions.contains(&"update") && path != vpn {
                let body = if path == vct {
                    json!({"interface_id":interface})
                } else {
                    json!({"assigned_object":endpoint("dcim.interface",interface)})
                };
                let (status, _) = common::request_with_auth(
                    &n.app,
                    "PATCH",
                    path,
                    Some(&body.to_string()),
                    None,
                    Some(bearer),
                )
                .await?;
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "reference READ must remain inside PAT ceiling"
                );
            }
        }
    }
    Ok(())
}

async fn snapshot(pool: &PgPool) -> TestResult<Value> {
    let mut state = serde_json::Map::new();
    for table in [
        "circuits_virtualcircuit",
        "circuits_virtualcircuittermination",
        "vpn_l2vpn",
        "vpn_l2vpntermination",
        "vpn_l2vpn_import_targets",
        "vpn_l2vpn_export_targets",
        "dcim_interface",
        "virtualization_vminterface",
        "ipam_vlan",
        "secret_usage_binding",
        "secrets_binding",
        "secrets_variable",
        "secrets_store",
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb) FROM ",
        );
        query.push(table).push(" t");
        let rows: Value = query.build_query_scalar().fetch_one(pool).await?;
        state.insert(table.into(), rows);
    }
    Ok(Value::Object(state))
}
async fn define_secrets(n: &Native) -> TestResult {
    for (app, model, _) in OWNERS {
        let scope: Uuid = sqlx::query_scalar(
            "SELECT id FROM inventory_content_type WHERE app_label=$1 AND model=$2",
        )
        .bind(app)
        .bind(model)
        .fetch_one(&n.pool)
        .await?;
        post(&n.app,&n.cookie,"/api/v1/custom-fields",json!({"name":format!("termination_{model}_credential"),"label":"Synthetic graph credential","field_type":"text","is_secret":true,"object_types":[scope]})).await?;
    }
    Ok(())
}
fn secret_body(model: &str) -> Value {
    json!({"custom_fields":{format!("termination_{model}_credential"):Uuid::now_v7().to_string()}})
}
async fn owned_secrets(pool: &PgPool, consumers: &[Uuid]) -> TestResult<Vec<Uuid>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT secret_id FROM secret_usage_binding WHERE consumer_id=ANY($1) ORDER BY secret_id",
    )
    .bind(consumers)
    .fetch_all(pool)
    .await?;
    assert_eq!(ids.len(), consumers.len(), "one owned secret per consumer");
    Ok(ids)
}
async fn retired(pool: &PgPool, consumers: &[Uuid], secrets: &[Uuid]) -> TestResult {
    for (_, _, table) in OWNERS {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM ");
        query.push(table).push(" WHERE id=ANY($1)");
        let count: i64 = query
            .build_query_scalar()
            .bind(consumers)
            .fetch_one(pool)
            .await?;
        assert_eq!(count, 0, "native cascade {table}");
    }
    for (table, column) in [
        ("secret_usage_binding", "secret_id"),
        ("secrets_binding", "secret_id"),
        ("secrets_variable", "id"),
        ("secrets_store", "secret_id"),
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM ");
        query
            .push(table)
            .push(" WHERE ")
            .push(column)
            .push("=ANY($1)");
        let count: i64 = query
            .build_query_scalar()
            .bind(secrets)
            .fetch_one(pool)
            .await?;
        assert_eq!(count, 0, "secret cascade {table}");
    }
    Ok(())
}
async fn secret_atomicity_and_retirement(n: &Native) -> TestResult {
    define_secrets(n).await?;
    let parent = virtual_circuit(&n.pool).await?;
    let interface = dcim_interface(&n.pool, "virtual").await?;
    let vpn_id = vpn(&n.app, &n.cookie, "vpls").await?;
    let vct = post(
        &n.app,
        &n.cookie,
        VCTS,
        merge(
            json!({"virtual_circuit_id":parent,"interface_id":interface}),
            secret_body("virtualcircuittermination"),
        ),
    )
    .await?;
    let term_row = post(
        &n.app,
        &n.cookie,
        TERMS,
        merge(
            term(vpn_id, "dcim.interface", interface),
            secret_body("l2vpntermination"),
        ),
    )
    .await?;
    let vpn_path = format!("{VPNS}/{vpn_id}");
    call(
        &n.app,
        Some(&n.cookie),
        "PATCH",
        &vpn_path,
        Some(secret_body("l2vpn")),
        StatusCode::OK,
    )
    .await?;
    let vct_path = format!("{VCTS}/{}", row_id(&vct)?);
    let term_path = format!("{TERMS}/{}", row_id(&term_row)?);
    for (path, model) in [
        (&vct_path, "virtualcircuittermination"),
        (&vpn_path, "l2vpn"),
        (&term_path, "l2vpntermination"),
    ] {
        let row = call(&n.app, Some(&n.cookie), "GET", path, None, StatusCode::OK).await?;
        assert!(
            row["custom_fields"][format!("termination_{model}_credential")] == "****",
            "secret was not masked"
        );
        let token=post(&n.app,&n.cookie,"/api/v1/user/settings/api-tokens",json!({"name":Uuid::now_v7().to_string(),"scopes":[{"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{}",row_id(&row)?)}]})).await?;
        let bearer = token["token_value"].as_str().ok_or("read PAT")?;
        let (status, _) = common::request_with_auth(
            &n.app,
            "POST",
            &format!("{path}/custom-fields/reveal"),
            None,
            None,
            Some(bearer),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "inventory READ alone cannot reveal a secret"
        );
    }
    let consumers = [row_id(&vct)?, vpn_id, row_id(&term_row)?];
    let secrets = owned_secrets(&n.pool, &consumers).await?;
    let endpoint_secrets = [
        secret_for(&n.pool, consumers[0]).await?,
        secret_for(&n.pool, consumers[2]).await?,
    ];
    let fresh = dcim_interface(&n.pool, "virtual").await?;
    sqlx::raw_sql("CREATE FUNCTION pwa80_termination_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic graph secret failure'; END $$; CREATE TRIGGER pwa80_termination_fault BEFORE INSERT OR UPDATE ON secrets_store FOR EACH ROW EXECUTE FUNCTION pwa80_termination_fault();").execute(&n.pool).await?;
    let before = snapshot(&n.pool).await?;
    for (path, model) in [
        (&vct_path, "virtualcircuittermination"),
        (&vpn_path, "l2vpn"),
        (&term_path, "l2vpntermination"),
    ] {
        call(
            &n.app,
            Some(&n.cookie),
            "PATCH",
            path,
            Some(secret_body(model)),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await?;
        assert!(
            snapshot(&n.pool).await? == before,
            "failed secret patch changed graph/bindings/ciphertext"
        );
    }
    for (path, body) in [
        (
            VCTS,
            merge(
                json!({"virtual_circuit_id":parent,"interface_id":fresh}),
                secret_body("virtualcircuittermination"),
            ),
        ),
        (
            TERMS,
            merge(
                term(vpn_id, "dcim.interface", fresh),
                secret_body("l2vpntermination"),
            ),
        ),
        (
            VPNS,
            merge(
                json!({"name":"Faulted VPN","slug":"faulted-vpn","type":"vpls"}),
                secret_body("l2vpn"),
            ),
        ),
    ] {
        call(
            &n.app,
            Some(&n.cookie),
            "POST",
            path,
            Some(body),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await?;
        assert!(
            snapshot(&n.pool).await? == before,
            "failed secret create leaked state"
        );
    }
    sqlx::raw_sql("DROP TRIGGER pwa80_termination_fault ON secrets_store; DROP FUNCTION pwa80_termination_fault();").execute(&n.pool).await?;
    // Inject a real FK dependency from a registered inventory table so both
    // readable and private blocker projection can be exercised on each handler.
    for (path, table, id) in [
        (&vct_path, OWNERS[0].2, consumers[0]),
        (&vpn_path, OWNERS[1].2, consumers[1]),
        (&term_path, OWNERS[2].2, consumers[2]),
    ] {
        blocker_projection_and_rollback(n, path, table, id).await?;
    }
    sqlx::raw_sql("CREATE FUNCTION pwa80_endpoint_delete_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic endpoint delete failure'; END $$; CREATE TRIGGER pwa80_endpoint_delete_fault BEFORE DELETE ON dcim_interface FOR EACH ROW EXECUTE FUNCTION pwa80_endpoint_delete_fault();").execute(&n.pool).await?;
    let before = snapshot(&n.pool).await?;
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &format!("/api/v1/inventory/interfaces/{interface}"),
        None,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await?;
    assert!(
        snapshot(&n.pool).await? == before,
        "failed endpoint delete changed native rows or secret bindings"
    );
    sqlx::raw_sql("DROP TRIGGER pwa80_endpoint_delete_fault ON dcim_interface; DROP FUNCTION pwa80_endpoint_delete_fault();").execute(&n.pool).await?;
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &format!("/api/v1/inventory/interfaces/{interface}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    retired(&n.pool, &[consumers[0], consumers[2]], &endpoint_secrets).await?;
    // Parent remains, with its own live secret, after endpoint cascade.
    owned_secrets(&n.pool, &[vpn_id]).await?;
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &vpn_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    retired(&n.pool, &consumers, &secrets).await?;
    // Exercise actual native parent and direct-child deletion, not just restore.
    for target in [
        "virtual-circuit",
        "l2vpn",
        "termination",
        "vm-interface",
        "vlan",
    ] {
        let same = equal_uuid_endpoints(&n.pool).await?;
        let parent = if target == "virtual-circuit" {
            virtual_circuit(&n.pool).await?
        } else {
            vpn(&n.app, &n.cookie, "vpls").await?
        };
        let (collection, body) = if target == "virtual-circuit" {
            (
                VCTS,
                json!({"virtual_circuit_id":parent,"interface_id":same}),
            )
        } else {
            (
                TERMS,
                term(
                    parent,
                    if target == "vm-interface" {
                        "virtualization.vminterface"
                    } else if target == "vlan" {
                        "ipam.vlan"
                    } else {
                        "dcim.interface"
                    },
                    same,
                ),
            )
        };
        let child = post(
            &n.app,
            &n.cookie,
            collection,
            merge(
                body,
                secret_body(if target == "virtual-circuit" {
                    "virtualcircuittermination"
                } else {
                    "l2vpntermination"
                }),
            ),
        )
        .await?;
        let child = row_id(&child)?;
        let secrets = owned_secrets(&n.pool, &[child]).await?;
        let path = match target {
            "virtual-circuit" => format!("/api/v1/inventory/virtual-circuits/{parent}"),
            "l2vpn" => format!("{VPNS}/{parent}"),
            "vm-interface" => format!("/api/v1/inventory/vm-interfaces/{same}"),
            "vlan" => format!("/api/v1/inventory/vlans/{same}"),
            _ => format!("{TERMS}/{child}"),
        };
        call(
            &n.app,
            Some(&n.cookie),
            "DELETE",
            &path,
            None,
            StatusCode::NO_CONTENT,
        )
        .await?;
        retired(&n.pool, &[child], &secrets).await?;
    }
    Ok(())
}

async fn direct_graph_constraints(n: &Native) -> TestResult {
    let vc = virtual_circuit(&n.pool).await?;
    let interface = dcim_interface(&n.pool, "virtual").await?;
    post(
        &n.app,
        &n.cookie,
        VCTS,
        json!({"virtual_circuit_id":vc,"interface_id":interface}),
    )
    .await?;
    let before = snapshot(&n.pool).await?;
    let mut tx = n.pool.begin().await?;
    sqlx::query("UPDATE dcim_interface SET type='1000base-t' WHERE id=$1")
        .bind(interface)
        .execute(&mut *tx)
        .await?;
    assert!(
        tx.commit().await.is_err(),
        "direct endpoint type change bypassed graph validation"
    );
    assert!(
        snapshot(&n.pool).await? == before,
        "invalid endpoint transition was not rolled back"
    );
    let parent = vpn(&n.app, &n.cookie, "vpls").await?;
    for _ in 0..3 {
        let interface = dcim_interface(&n.pool, "virtual").await?;
        post(
            &n.app,
            &n.cookie,
            TERMS,
            term(parent, "dcim.interface", interface),
        )
        .await?;
    }
    let before = snapshot(&n.pool).await?;
    let mut tx = n.pool.begin().await?;
    sqlx::query("UPDATE vpn_l2vpn SET type='vpws' WHERE id=$1")
        .bind(parent)
        .execute(&mut *tx)
        .await?;
    assert!(
        tx.commit().await.is_err(),
        "direct parent type change bypassed P2P cap"
    );
    assert!(snapshot(&n.pool).await? == before);
    // The shared graph must cover statement-level direct writers even when no
    // row matches; a row lock cannot account for these expected lock timeouts.
    let mut owner = n.pool.begin().await?;
    sqlx::query("SELECT dcim_acquire_hardware_graph()")
        .execute(&mut *owner)
        .await?;
    for (table, column) in [
        ("circuits_virtualcircuittermination", "role"),
        ("vpn_l2vpn", "name"),
        ("vpn_l2vpntermination", "l2vpn_id"),
    ] {
        let mut writer = n.pool.begin().await?;
        sqlx::query("SET LOCAL lock_timeout='100ms'")
            .execute(&mut *writer)
            .await?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE ");
        query
            .push(table)
            .push(" SET ")
            .push(column)
            .push("=")
            .push(column)
            .push(" WHERE false");
        let error = query
            .build()
            .execute(&mut *writer)
            .await
            .expect_err("direct writer bypassed graph owner");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        );
        writer.rollback().await?;
    }
    owner.rollback().await?;
    Ok(())
}

async fn conditional_reference_attributes(
    n: &mut Native,
    vct: &str,
    term_path: &str,
    interface: Uuid,
) -> TestResult {
    let policy = Uuid::now_v7();
    // Deny only READ of this persisted virtual Interface. Child UPDATE remains
    // allowed, so a 403 must come from supplied-reference authorization.
    sqlx::query("INSERT INTO rbac_policy(id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) SELECT $1,$2,'deny',$3,$4,$5,subjects,$6,true FROM rbac_policy WHERE name='test-admin-policy'")
        .bind(policy).bind(policy.to_string()).bind(json!([Action::Read])).bind(json!([ResourceType::Inventory])).bind(json!({"attributes":{"object_type":"dcim.interface","type":"virtual"}})).bind(json!([format!("inventory:{interface}")])).execute(&n.pool).await?;
    invalidate(&mut n.redis).await?;
    let result: TestResult = async {
        for (stage, path, body) in [
            (
                "virtual circuit interface",
                vct,
                json!({"interface_id":interface}),
            ),
            (
                "L2VPN endpoint",
                term_path,
                json!({"assigned_object":endpoint("dcim.interface",interface)}),
            ),
        ] {
            eprintln!("termination conditional reference check: {stage}");
            call(
                &n.app,
                Some(&n.cookie),
                "PATCH",
                path,
                Some(body),
                StatusCode::FORBIDDEN,
            )
            .await?;
        }
        sqlx::query("UPDATE rbac_policy SET constraints=$1 WHERE id=$2")
            .bind(json!({"attributes":{"object_type":"dcim.interface","type":"bridge"}}))
            .bind(policy)
            .execute(&n.pool)
            .await?;
        invalidate(&mut n.redis).await?;
        // Same request with a nonmatching persisted-type condition is allowed.
        call(
            &n.app,
            Some(&n.cookie),
            "PATCH",
            term_path,
            Some(json!({"assigned_object":endpoint("dcim.interface",interface)})),
            StatusCode::OK,
        )
        .await?;
        Ok(())
    }
    .await;
    sqlx::query("DELETE FROM rbac_policy WHERE id=$1")
        .bind(policy)
        .execute(&n.pool)
        .await?;
    invalidate(&mut n.redis).await?;
    result
}

async fn secret_for(pool: &PgPool, consumer: Uuid) -> TestResult<Uuid> {
    Ok(
        sqlx::query_scalar("SELECT secret_id FROM secret_usage_binding WHERE consumer_id=$1")
            .bind(consumer)
            .fetch_one(pool)
            .await?,
    )
}

async fn blocker_projection_and_rollback(
    n: &Native,
    path: &str,
    table: &str,
    id: Uuid,
) -> TestResult {
    let blocker = dcim_interface(&n.pool, "virtual").await?;
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
        "ALTER TABLE dcim_interface ADD COLUMN pwa80_termination_blocker uuid REFERENCES ",
    );
    query.push(table).push("(id) DEFERRABLE INITIALLY DEFERRED");
    query.build().execute(&n.pool).await?;
    sqlx::query("UPDATE dcim_interface SET pwa80_termination_blocker=$1 WHERE id=$2")
        .bind(id)
        .bind(blocker)
        .execute(&n.pool)
        .await?;
    let before = snapshot(&n.pool).await?;
    let visible = call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        path,
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    assert_eq!(visible["code"], "DELETE_BLOCKED");
    assert!(
        visible["details"]["blockers"]
            .as_array()
            .ok_or("blockers")?
            .iter()
            .any(|g| g["table"] == "dcim_interface"
                && g["examples"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|e| e["id"] == blocker.to_string())))
    );
    assert!(
        snapshot(&n.pool).await? == before,
        "blocked delete changed graph or secret state"
    );
    let saved: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(&n.pool)
            .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(json!([format!("inventory:{id}")]))
        .execute(&n.pool)
        .await?;
    let mut redis = n.redis.clone();
    invalidate(&mut redis).await?;
    let hidden = call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        path,
        None,
        StatusCode::CONFLICT,
    )
    .await;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(saved)
        .execute(&n.pool)
        .await?;
    invalidate(&mut redis).await?;
    let hidden = hidden?;
    assert_eq!(hidden["code"], "CONFLICT");
    assert_eq!(hidden["reason_code"], "DELETE_BLOCKED");
    assert!(hidden.get("details").is_none_or(Value::is_null));
    let encoded = hidden.to_string();
    assert!(
        !encoded.contains(&blocker.to_string()),
        "private blocker UUID disclosed"
    );
    assert!(
        !encoded.contains("dcim_interface"),
        "private blocker table disclosed"
    );
    assert!(
        snapshot(&n.pool).await? == before,
        "private blocked delete changed secrets"
    );
    sqlx::query("ALTER TABLE dcim_interface DROP COLUMN pwa80_termination_blocker")
        .execute(&n.pool)
        .await?;
    Ok(())
}

async fn restore_cascade_and_rollback(n: &Native) -> TestResult {
    let service = BackupService::new(n.pool.clone());
    for (table, collection, model) in [
        (
            "circuits_virtualcircuit",
            "/api/v1/inventory/virtual-circuits",
            "virtualcircuittermination",
        ),
        ("vpn_l2vpn", VPNS, "l2vpntermination"),
        (
            "dcim_interface",
            "/api/v1/inventory/interfaces",
            "l2vpntermination",
        ),
    ] {
        let selection = FullBackupSelection {
            sections: vec![],
            tables: vec![table.into()],
        };
        // Export before creating the missing parent. Never clear the archive:
        // existing rows and unselected tables must survive selective restore.
        let archive = service
            .export_full_backup(true, Some(selection.clone()))
            .await?;
        let interface = dcim_interface(&n.pool, "virtual").await?;
        let (parent, child, child_path) = if table == "circuits_virtualcircuit" {
            let parent = virtual_circuit(&n.pool).await?;
            let child = post(
                &n.app,
                &n.cookie,
                VCTS,
                merge(
                    json!({"virtual_circuit_id":parent,"interface_id":interface}),
                    secret_body(model),
                ),
            )
            .await?;
            (
                parent,
                row_id(&child)?,
                format!("{VCTS}/{}", row_id(&child)?),
            )
        } else {
            let parent = vpn(&n.app, &n.cookie, "vpls").await?;
            let child = post(
                &n.app,
                &n.cookie,
                TERMS,
                merge(
                    term(parent, "dcim.interface", interface),
                    secret_body(model),
                ),
            )
            .await?;
            (
                if table == "dcim_interface" {
                    interface
                } else {
                    parent
                },
                row_id(&child)?,
                format!("{TERMS}/{}", row_id(&child)?),
            )
        };
        let secrets = owned_secrets(&n.pool, &[child]).await?;
        let request = |dry_run| FullBackupImportRequest {
            payload: archive.clone(),
            confirm_full_restore: true,
            dry_run,
            selection: Some(selection.clone()),
            rules: Some(FullBackupImportRules {
                default_action: FullBackupImportAction::Delete,
                ..Default::default()
            }),
        };
        let before = snapshot(&n.pool).await?;
        assert!(!service.import_full_backup(request(true)).await?.applied);
        assert!(
            snapshot(&n.pool).await? == before,
            "dry-run changed graph/secrets"
        );
        // Fail after retirement starts; the whole restore must roll back.
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "CREATE FUNCTION pwa80_restore_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.id='",
        );
        query.push(parent).push(
            "'::uuid THEN RAISE EXCEPTION 'synthetic restore failure'; END IF; RETURN OLD; END $$",
        );
        query.build().execute(&n.pool).await?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "CREATE TRIGGER pwa80_restore_fault BEFORE DELETE ON ",
        );
        query
            .push(table)
            .push(" FOR EACH ROW EXECUTE FUNCTION pwa80_restore_fault()");
        query.build().execute(&n.pool).await?;
        assert!(service.import_full_backup(request(false)).await.is_err());
        assert!(
            snapshot(&n.pool).await? == before,
            "failed selective restore changed ciphertext or graph"
        );
        let mut query =
            sqlx::QueryBuilder::<sqlx::Postgres>::new("DROP TRIGGER pwa80_restore_fault ON ");
        query.push(table);
        query.build().execute(&n.pool).await?;
        sqlx::query("DROP FUNCTION pwa80_restore_fault()")
            .execute(&n.pool)
            .await?;
        assert!(service.import_full_backup(request(false)).await?.applied);
        retired(&n.pool, &[child], &secrets).await?;
        call(
            &n.app,
            Some(&n.cookie),
            "GET",
            &child_path,
            None,
            StatusCode::NOT_FOUND,
        )
        .await?;
        call(
            &n.app,
            Some(&n.cookie),
            "GET",
            &format!("{collection}/{parent}"),
            None,
            StatusCode::NOT_FOUND,
        )
        .await?;
    }
    Ok(())
}

async fn managed_secret_identity(pool: &PgPool, consumer: Uuid, secret: Uuid) -> TestResult<Value> {
    // Compare identity and encrypted bytes without including either in assertion
    // diagnostics. Usage timestamps may legitimately change during replay.
    let identity: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(\
         'usage',(SELECT jsonb_agg(jsonb_build_object('id',id,'consumer_type',consumer_type,'consumer_id',consumer_id,'field_key',field_key,'secret_id',secret_id,'owner_scope_type',owner_scope_type,'owner_scope_id',owner_scope_id)) FROM secret_usage_binding WHERE consumer_id=$1 AND secret_id=$2),\
         'bindings',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM secrets_binding b WHERE secret_id=$2),\
         'variable',(SELECT jsonb_build_object('id',id,'key',key,'kind',kind,'created_at',created_at,'retire_with_field_usage',retire_with_field_usage) FROM secrets_variable WHERE id=$2),\
         'store',(SELECT to_jsonb(s) FROM secrets_store s WHERE secret_id=$2))",
    ).bind(consumer).bind(secret).fetch_one(pool).await?;
    assert_eq!(
        identity["usage"]
            .as_array()
            .ok_or("managed usage missing")?
            .len(),
        1
    );
    assert!(
        !identity["bindings"]
            .as_array()
            .ok_or("managed binding missing")?
            .is_empty()
    );
    assert!(identity["variable"].is_object(), "managed variable missing");
    assert!(
        identity["store"]["encrypted_value"].is_string(),
        "managed ciphertext missing"
    );
    Ok(identity)
}

#[tokio::test]
#[ignore = "Main only: requires its own explicit exclusive empty PostgreSQL/Redis selections; restores secret-bearing typed graph"]
async fn selective_restore_reparented_termination_preserves_only_final_live_secret() -> TestResult {
    let n = native().await?;
    define_secrets(&n).await?;
    let service = BackupService::new(n.pool.clone());
    for (replay, occupy_original) in [
        (FullBackupImportAction::Replace, false),
        (FullBackupImportAction::Delete, false),
        (FullBackupImportAction::Skip, false),
        (FullBackupImportAction::Delete, true),
    ] {
        let parent = vpn(&n.app, &n.cookie, "vpls").await?;
        let e1 = dcim_interface(&n.pool, "virtual").await?;
        let value = Uuid::now_v7().to_string();
        let row = post(
            &n.app,
            &n.cookie,
            TERMS,
            merge(
                term(parent, "dcim.interface", e1),
                json!({"custom_fields":{"termination_l2vpntermination_credential":value}}),
            ),
        )
        .await?;
        let termination = row_id(&row)?;
        let path = format!("{TERMS}/{termination}");
        let secret = secret_for(&n.pool, termination).await?;
        let identity = managed_secret_identity(&n.pool, termination, secret).await?;
        let selection = FullBackupSelection {
            sections: vec![],
            tables: vec!["dcim_interface".into(), "vpn_l2vpntermination".into()],
        };
        let archive = service
            .export_full_backup(true, Some(selection.clone()))
            .await?;
        assert_eq!(
            archive.tables.len(),
            2,
            "secret tables must remain unselected"
        );
        assert!(archive.tables.iter().all(|table| matches!(
            table.name.as_str(),
            "dcim_interface" | "vpn_l2vpntermination"
        )));
        let archived = archive
            .tables
            .iter()
            .find(|table| table.name == "vpn_l2vpntermination")
            .ok_or("termination table absent")?
            .rows
            .iter()
            .find(|row| row["id"] == termination.to_string())
            .ok_or("termination absent from archive")?;
        assert_eq!(archived["assigned_object_id"], e1.to_string());

        let e2 = dcim_interface(&n.pool, "virtual").await?;
        call(
            &n.app,
            Some(&n.cookie),
            "PATCH",
            &path,
            Some(json!({"assigned_object":endpoint("dcim.interface",e2)})),
            StatusCode::OK,
        )
        .await?;
        assert!(
            managed_secret_identity(&n.pool, termination, secret).await? == identity,
            "reparent changed managed secret identity"
        );

        // Only Delete removes absent archive rows. Replace must retain them,
        // so the immediate unique-slot regression is a separate Delete case.
        let occupant = if occupy_original {
            let row = post(
                &n.app,
                &n.cookie,
                TERMS,
                merge(
                    term(parent, "dcim.interface", e1),
                    secret_body("l2vpntermination"),
                ),
            )
            .await?;
            let id = row_id(&row)?;
            let secret = secret_for(&n.pool, id).await?;
            let identity = managed_secret_identity(&n.pool, id, secret).await?;
            assert!(
                archive
                    .tables
                    .iter()
                    .filter(|table| table.name == "vpn_l2vpntermination")
                    .all(|table| table.rows.iter().all(|row| row["id"] != id.to_string())),
                "new endpoint occupant must be absent from the archive"
            );
            Some((id, secret, identity))
        } else {
            None
        };

        // UUID ordering puts this real unselected FK blocker after E2 for
        // ordinary missing-row deletion. Rollback must also undo any early
        // retirement of the absent occupant before retained-row replay.
        let blocked = Uuid::from_u128(
            e2.as_u128()
                .checked_add(1)
                .ok_or("endpoint UUID overflow")?,
        );
        sqlx::query("INSERT INTO dcim_interface(id,custom_field_data,name,label,description,mark_connected,enabled,_name,type,mgmt_only) VALUES($1,'{}',$2,'','',false,true,$2,'virtual',false)").bind(blocked).bind(blocked.to_string()).execute(&n.pool).await?;
        sqlx::query("CREATE TABLE pwa80_termination_restore_blocker(id uuid PRIMARY KEY, endpoint_id uuid NOT NULL REFERENCES dcim_interface(id) DEFERRABLE INITIALLY DEFERRED)").execute(&n.pool).await?;
        sqlx::query("INSERT INTO pwa80_termination_restore_blocker(id,endpoint_id) VALUES($1,$2)")
            .bind(Uuid::now_v7())
            .bind(blocked)
            .execute(&n.pool)
            .await?;
        let request = |dry_run| FullBackupImportRequest {
            payload: archive.clone(),
            confirm_full_restore: true,
            dry_run,
            selection: Some(selection.clone()),
            rules: Some(FullBackupImportRules {
                default_action: FullBackupImportAction::Delete,
                tables: [("vpn_l2vpntermination".into(), replay)]
                    .into_iter()
                    .collect(),
                ..Default::default()
            }),
        };
        let before = snapshot(&n.pool).await?;
        assert!(!service.import_full_backup(request(true)).await?.applied);
        assert!(
            snapshot(&n.pool).await? == before,
            "restore dry-run changed graph or secrets"
        );
        let error = service
            .import_full_backup(request(false))
            .await
            .expect_err("later endpoint FK must block restore");
        assert!(
            matches!(error, piping_backend::errors::AppError::DeleteBlocked(_)),
            "restore must retain canonical blocker classification"
        );
        assert!(
            snapshot(&n.pool).await? == before,
            "later blocked delete failed to roll back graph and managed secret state"
        );
        assert!(
            managed_secret_identity(&n.pool, termination, secret).await? == identity,
            "rollback changed managed secret identity"
        );
        if let Some((id, secret, identity)) = &occupant {
            assert!(
                managed_secret_identity(&n.pool, *id, *secret).await? == *identity,
                "blocked restore did not roll back the absent occupant's secret retirement"
            );
            let row = call(
                &n.app,
                Some(&n.cookie),
                "GET",
                &format!("{TERMS}/{id}"),
                None,
                StatusCode::OK,
            )
            .await?;
            assert_eq!(row["assigned_object"]["object_id"], e1.to_string());
        }
        sqlx::query("DROP TABLE pwa80_termination_restore_blocker")
            .execute(&n.pool)
            .await?;

        assert!(service.import_full_backup(request(false)).await?.applied);
        if let Some((id, secret, _)) = &occupant {
            // T2 must leave E1's immediate globally unique slot before T is
            // replayed, while only T's original managed secret survives.
            call(
                &n.app,
                Some(&n.cookie),
                "GET",
                &format!("{TERMS}/{id}"),
                None,
                StatusCode::NOT_FOUND,
            )
            .await?;
            retired(&n.pool, &[*id], &[*secret]).await?;
        }
        for absent in [e2, blocked] {
            call(
                &n.app,
                Some(&n.cookie),
                "GET",
                &format!("/api/v1/inventory/interfaces/{absent}"),
                None,
                StatusCode::NOT_FOUND,
            )
            .await?;
        }
        call(
            &n.app,
            Some(&n.cookie),
            "GET",
            &format!("/api/v1/inventory/interfaces/{e1}"),
            None,
            StatusCode::OK,
        )
        .await?;
        if replay == FullBackupImportAction::Skip {
            // Archive membership alone is insufficient: no termination replay
            // means E2's cascade removes T from the final graph.
            call(
                &n.app,
                Some(&n.cookie),
                "GET",
                &path,
                None,
                StatusCode::NOT_FOUND,
            )
            .await?;
            retired(&n.pool, &[termination], &[secret]).await?;
        } else {
            let restored =
                call(&n.app, Some(&n.cookie), "GET", &path, None, StatusCode::OK).await?;
            assert_eq!(row_id(&restored)?, termination);
            assert_eq!(restored["assigned_object"]["object_type"], "dcim.interface");
            assert_eq!(restored["assigned_object"]["object_id"], e1.to_string());
            assert!(
                managed_secret_identity(&n.pool, termination, secret).await? == identity,
                "replayed termination lost original binding, variable or ciphertext"
            );
            let revealed = call(
                &n.app,
                Some(&n.cookie),
                "POST",
                &format!("{path}/custom-fields/reveal"),
                None,
                StatusCode::OK,
            )
            .await?;
            assert!(
                revealed["custom_fields"]["termination_l2vpntermination_credential"] == value,
                "restored secret no longer reveals the original value"
            );
        }
    }
    n.pool.close().await;
    Ok(())
}

async fn contact_assignment(
    n: &Native,
    parent: Uuid,
    kind: &str,
    contact: Uuid,
    role: Uuid,
) -> TestResult<Value> {
    post(&n.app,&n.cookie,"/api/v1/inventory/contact-assignments",json!({"object_type":kind,"object_id":parent,"contact_id":contact,"role_id":role,"priority":"primary"})).await
}

#[tokio::test]
#[ignore = "Main only: requires its own explicit exclusive empty PostgreSQL/Redis selections; races real contact attach against L2VPN delete"]
async fn l2vpn_contacts_cascade_is_typed_and_attach_delete_share_hardware_graph() -> TestResult {
    let n = native().await?;
    let contact = row_id(
        &post(
            &n.app,
            &n.cookie,
            "/api/v1/inventory/contacts",
            json!({"name":"Termination graph contact"}),
        )
        .await?,
    )?;
    let role = row_id(
        &post(
            &n.app,
            &n.cookie,
            "/api/v1/inventory/contact-roles",
            json!({"name":"Termination graph role","slug":"termination-graph-role"}),
        )
        .await?,
    )?;
    let parent = vpn(&n.app, &n.cookie, "vpls").await?;
    // Equal object UUIDs in two existing contactable domains must not turn the
    // GenericRelation cascade into an untyped object_id delete.
    sqlx::query("INSERT INTO tenancy_tenant(id,name,slug,description,comments,custom_field_data) VALUES($1,$2,$2,'','','{}')").bind(parent).bind(parent.to_string()).execute(&n.pool).await?;
    let child = contact_assignment(&n, parent, "vpn.l2vpn", contact, role).await?;
    let unrelated = contact_assignment(&n, parent, "tenancy.tenant", contact, role).await?;
    assert_eq!(child["object"]["object_type"], "vpn.l2vpn");
    assert_eq!(unrelated["object"]["object_type"], "tenancy.tenant");
    call(
        &n.app,
        Some(&n.cookie),
        "DELETE",
        &format!("{VPNS}/{parent}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    call(
        &n.app,
        Some(&n.cookie),
        "GET",
        &format!("/api/v1/inventory/contact-assignments/{}", row_id(&child)?),
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    let kept = call(
        &n.app,
        Some(&n.cookie),
        "GET",
        &format!(
            "/api/v1/inventory/contact-assignments/{}",
            row_id(&unrelated)?
        ),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(kept["object"], unrelated["object"]);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM tenancy_contactassignment a JOIN inventory_content_type ct ON ct.id=a.object_type_id WHERE a.object_id=$1 AND ct.app_label='vpn' AND ct.model='l2vpn'").bind(parent).fetch_one(&n.pool).await?;
    assert_eq!(
        count, 0,
        "typed contact child persisted after parent deletion"
    );

    let parent = vpn(&n.app, &n.cookie, "vpls").await?;
    let mut owner = n.pool.begin().await?;
    sqlx::query("SELECT dcim_acquire_hardware_graph()")
        .execute(&mut *owner)
        .await?;
    let owner_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *owner)
        .await?;
    let app = n.app.clone();
    let cookie = n.cookie.clone();
    let attach = tokio::spawn(async move {
        let body=json!({"object_type":"vpn.l2vpn","object_id":parent,"contact_id":contact,"role_id":role,"priority":"primary"}).to_string();
        common::request(
            &app,
            "POST",
            "/api/v1/inventory/contact-assignments",
            Some(&body),
            Some(&cookie),
        )
        .await
    });
    let app = n.app.clone();
    let cookie = n.cookie.clone();
    let delete = tokio::spawn(async move {
        common::request(
            &app,
            "DELETE",
            &format!("{VPNS}/{parent}"),
            None,
            Some(&cookie),
        )
        .await
    });
    // Require both concrete consumers to wait behind the existing hardware
    // owner. A request pair that happens to run sequentially is not race proof.
    let queued=tokio::time::timeout(Duration::from_secs(15),async {
        loop {
            let waiters:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND $1=ANY(pg_blocking_pids(pid))").bind(owner_pid).fetch_one(&n.pool).await?;
            if waiters>=2 {return Ok::<(),sqlx::Error>(());}
            tokio::task::yield_now().await;
        }
    }).await;
    // Always release the graph before propagating a missing-lock failure.
    owner.rollback().await?;
    let (attached, deleted) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(attach, delete)
    })
    .await?;
    queued??;
    let (attach_status, attach_body) = attached??;
    assert_eq!(deleted??.0, StatusCode::NO_CONTENT);
    assert!(
        matches!(
            attach_status,
            StatusCode::CREATED
                | StatusCode::BAD_REQUEST
                | StatusCode::NOT_FOUND
                | StatusCode::CONFLICT
        ),
        "contact attach must either commit before cascade or reject a missing parent; got {attach_status}"
    );
    call(
        &n.app,
        Some(&n.cookie),
        "GET",
        &format!("{VPNS}/{parent}"),
        None,
        StatusCode::NOT_FOUND,
    )
    .await?;
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM tenancy_contactassignment a JOIN inventory_content_type ct ON ct.id=a.object_type_id WHERE a.object_id=$1 AND ct.app_label='vpn' AND ct.model='l2vpn'").bind(parent).fetch_one(&n.pool).await?;
    assert_eq!(count, 0, "concurrent contact attach orphaned a typed child");
    if attach_status == StatusCode::CREATED {
        let row: Value = serde_json::from_slice(&attach_body)?;
        call(
            &n.app,
            Some(&n.cookie),
            "GET",
            &format!("/api/v1/inventory/contact-assignments/{}", row_id(&row)?),
            None,
            StatusCode::NOT_FOUND,
        )
        .await?;
    }
    call(
        &n.app,
        Some(&n.cookie),
        "GET",
        &format!(
            "/api/v1/inventory/contact-assignments/{}",
            row_id(&unrelated)?
        ),
        None,
        StatusCode::OK,
    )
    .await?;
    n.pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "Main only: requires a separately provisioned empty exclusive database; exercises raw 0011 upgrade/refusal"]
async fn upgrade_0011_preserves_history_and_refuses_invalid_existing_graph() -> TestResult {
    let selection = ExclusiveSelection::from_env()?;
    let pool = selection.empty_pool().await?;
    for sql in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_interface_networking.sql"),
        include_str!("../migrations/0003_service_port_mappings.sql"),
        include_str!("../migrations/0004_fhrp_auth_key_secret.sql"),
        include_str!("../migrations/0005_module_bay_types.sql"),
        include_str!("../migrations/0006_canonical_tag_ids.sql"),
        include_str!("../migrations/0007_cooling_hardware.sql"),
        include_str!("../migrations/0008_module_type_profile_scope.sql"),
        include_str!("../migrations/0009_circuit_group_assignments.sql"),
        include_str!("../migrations/0010_vlan_translation.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await?;
    }
    let parent = virtual_circuit(&pool).await?;
    let interface = dcim_interface(&pool, "1000base-t").await?;
    let vct = Uuid::now_v7();
    sqlx::query("INSERT INTO circuits_virtualcircuittermination(id,virtual_circuit_id,interface_id,role,description,custom_field_data) VALUES($1,$2,$3,'peer','historical','{\"retained\":true}')").bind(vct).bind(parent).bind(interface).execute(&pool).await?;
    let vpn = Uuid::now_v7();
    sqlx::query("INSERT INTO vpn_l2vpn(id,name,slug,type,status,identifier,description,comments,custom_field_data) VALUES($1,'Historical VPN','historical-vpn','vpws','active',$2,'historical','retained','{\"retained\":true}')").bind(vpn).bind(i64::MIN).execute(&pool).await?;
    sqlx::query("INSERT INTO inventory_content_type(app_label,model) VALUES('dcim','interface'),('circuits','virtualcircuittermination'),('vpn','l2vpn'),('vpn','l2vpntermination') ON CONFLICT(app_label,model) DO NOTHING").execute(&pool).await?;
    let ct: Uuid = sqlx::query_scalar(
        "SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model='interface'",
    )
    .fetch_one(&pool)
    .await?;
    let mut terms = Vec::new();
    for _ in 0..3 {
        let endpoint = dcim_interface(&pool, "virtual").await?;
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO vpn_l2vpntermination(id,l2vpn_id,assigned_object_type_id,assigned_object_id,custom_field_data) VALUES($1,$2,$3,$4,'{\"retained\":true}')").bind(id).bind(vpn).bind(ct).bind(endpoint).execute(&pool).await?;
        terms.push(id);
    }
    let tag = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_tag(id,name,slug,color,description,weight) VALUES($1,'Historical termination tag','historical-termination-tag','155dfc','retained',0)").bind(tag).execute(&pool).await?;
    for (_, _, table) in OWNERS {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE ");
        query.push(table).push(" SET tags=$1");
        query
            .build()
            .bind(vec![tag.to_string()])
            .execute(&pool)
            .await?;
    }
    let target = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_routetarget(id,name,description,comments,custom_field_data) VALUES($1,'65000:81','retained','','{}')").bind(target).execute(&pool).await?;
    for table in ["vpn_l2vpn_import_targets", "vpn_l2vpn_export_targets"] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("INSERT INTO ");
        query
            .push(table)
            .push("(l2vpn_id,routetarget_id) VALUES($1,$2)");
        query.build().bind(vpn).bind(target).execute(&pool).await?;
    }
    let identities: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id,app_label,model FROM inventory_content_type ORDER BY id")
            .fetch_all(&pool)
            .await?;
    let forward = include_str!("../migrations/0011_termination_graph.sql");
    for repair in [false, true] {
        if repair {
            sqlx::query("UPDATE dcim_interface SET type='virtual' WHERE id=$1")
                .bind(interface)
                .execute(&pool)
                .await?;
        }
        let before = snapshot(&pool).await?;
        let mut tx = pool.begin().await?;
        assert!(
            sqlx::raw_sql(forward).execute(&mut *tx).await.is_err(),
            "invalid existing graph was silently repaired"
        );
        tx.rollback().await?;
        assert!(
            snapshot(&pool).await? == before,
            "refused upgrade changed imported history"
        );
        let dates_exist:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='vpn_l2vpn' AND column_name='created')").fetch_one(&pool).await?;
        assert!(!dates_exist, "failed migration partially applied");
    }
    // An explicit operator-side correction, not an upgrade fallback.
    sqlx::query("UPDATE vpn_l2vpn SET type='vpls' WHERE id=$1")
        .bind(vpn)
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(forward).execute(&mut *tx).await?;
    tx.commit().await?;
    let after: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id,app_label,model FROM inventory_content_type ORDER BY id")
            .fetch_all(&pool)
            .await?;
    assert!(
        identities.iter().all(|row| after.contains(row)),
        "catalog identity changed"
    );
    for (table, ids) in [
        (OWNERS[0].2, vec![vct]),
        (OWNERS[1].2, vec![vpn]),
        (OWNERS[2].2, terms),
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT id,custom_field_data,created,last_updated FROM ",
        );
        query.push(table).push(" WHERE id=ANY($1) ORDER BY id");
        let rows: Vec<(
            Uuid,
            Value,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<chrono::DateTime<chrono::Utc>>,
        )> = query.build_query_as().bind(&ids).fetch_all(&pool).await?;
        assert_eq!(rows.len(), ids.len());
        for (_, fields, created, updated) in rows {
            assert_eq!(fields, json!({"retained":true}));
            assert!(created.is_none());
            assert!(updated.is_none());
        }
    }
    let identifier: i64 = sqlx::query_scalar("SELECT identifier FROM vpn_l2vpn WHERE id=$1")
        .bind(vpn)
        .fetch_one(&pool)
        .await?;
    assert_eq!(identifier, i64::MIN);
    for (_, _, table) in OWNERS {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT tags FROM ");
        query.push(table);
        let memberships: Vec<Vec<String>> = query.build_query_scalar().fetch_all(&pool).await?;
        assert!(
            memberships
                .iter()
                .all(|tags| tags == &vec![tag.to_string()]),
            "upgrade lost historical tag membership"
        );
    }
    for table in ["vpn_l2vpn_import_targets", "vpn_l2vpn_export_targets"] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT routetarget_id FROM ");
        query.push(table).push(" WHERE l2vpn_id=$1");
        let targets: Vec<Uuid> = query
            .build_query_scalar()
            .bind(vpn)
            .fetch_all(&pool)
            .await?;
        assert_eq!(targets, vec![target]);
    }
    pool.close().await;
    Ok(())
}
