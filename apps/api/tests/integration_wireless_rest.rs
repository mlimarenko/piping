//! Wireless API, baseline constraints and Interface M2M on a disposable child database.
mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{
    PgPool, Postgres,
    migrate::{MigrateDatabase, Migrator},
};
use std::{sync::Arc, time::Duration};
use url::Url;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const GROUPS: &str = "/api/v1/inventory/wireless_lan_groups";
const LANS: &str = "/api/v1/inventory/wireless_lans";
const LINKS: &str = "/api/v1/inventory/wireless_links";
const IFACES: &str = "/api/v1/inventory/interfaces";

async fn api(
    app: &axum::Router,
    method: &str,
    path: &str,
    payload: Option<Value>,
    cookie: Option<&str>,
    expected: StatusCode,
) -> TestResult<Value> {
    let payload = payload.map(|v| v.to_string());
    let (status, body) = common::request(app, method, path, payload.as_deref(), cookie).await?;
    // Do not echo response bodies: Wireless authentication fields can contain credentials.
    assert_eq!(status, expected, "{method} {path}: unexpected status");
    Ok(if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    })
}

fn id(value: &Value) -> TestResult<&str> {
    value["id"].as_str().ok_or_else(|| "Missing ID".into())
}

async fn assert_filtered_ids(
    app: &axum::Router,
    cookie: Option<&str>,
    path: &str,
    expected: &[&str],
) -> TestResult {
    let page = api(app, "GET", path, None, cookie, StatusCode::OK).await?;
    assert_eq!(page["total"], json!(expected.len()));
    let mut actual = page["items"]
        .as_array()
        .ok_or("Missing list items")?
        .iter()
        .map(id)
        .collect::<TestResult<Vec<_>>>()?;
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    let beyond = api(
        app,
        "GET",
        &format!("{path}&offset=10000"),
        None,
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(beyond["total"], page["total"]);
    assert_eq!(beyond["items"], json!([]));
    Ok(())
}

async fn assert_list_sort_contract(
    app: &axum::Router,
    cookie: Option<&str>,
    path: &str,
    columns: &[piping_backend::contexts::shared::list_query::ListColumn],
) -> TestResult {
    for column in columns.iter().filter(|column| column.sort_sql.is_some()) {
        for order in ["asc", "desc"] {
            let query = format!("{path}?sort={}&order={order}", column.key);
            let page = api(app, "GET", &query, None, cookie, StatusCode::OK).await?;
            let items = page["items"].as_array().ok_or("Missing sorted list")?;
            for (offset, item) in items.iter().take(3).enumerate() {
                let single = api(
                    app,
                    "GET",
                    &format!("{query}&limit=1&offset={offset}"),
                    None,
                    cookie,
                    StatusCode::OK,
                )
                .await?;
                assert_eq!(single["total"], page["total"]);
                assert_eq!(
                    single["items"][0]["id"], item["id"],
                    "stable {} pagination",
                    column.key
                );
            }
        }
    }
    for query in [
        "sort=__invalid__",
        "sort=auth_psk",
        "sort=auth_key",
        "sort=name&order=sideways",
    ] {
        api(
            app,
            "GET",
            &format!("{path}?{query}"),
            None,
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    Ok(())
}

async fn observe_interface_lock_wait(pool: &PgPool, blocker_pid: i32) -> TestResult {
    // Only the spawned Interface request competes with this transaction in the
    // isolated database. Commit the writer only after PostgreSQL reports its waiter.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker_pid)
            .fetch_one(pool)
            .await?;
            if waiting {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
}

async fn update_behind_lan_writer(
    app: &axum::Router,
    cookie: &str,
    pool: &PgPool,
    interface_id: Uuid,
    committed_lan: Uuid,
    payload: Value,
    expected: StatusCode,
) -> TestResult<Value> {
    let mut writer = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *writer)
        .await?;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await?;
    sqlx::query("UPDATE dcim_interface SET description = description WHERE id = $1")
        .bind(interface_id)
        .execute(&mut *writer)
        .await?;
    sqlx::query("DELETE FROM dcim_interface_wirelesslans WHERE interface_id = $1")
        .bind(interface_id)
        .execute(&mut *writer)
        .await?;
    let committed_membership_id = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_interface_wirelesslans (id, interface_id, wirelesslan_id) VALUES ($1, $2, $3)")
        .bind(committed_membership_id)
        .bind(interface_id)
        .bind(committed_lan)
        .execute(&mut *writer)
        .await?;

    let app = app.clone();
    let cookie = cookie.to_owned();
    let replaces_lans = payload.get("wireless_lans").is_some();
    // JoinSet aborts the request on an early observer/commit failure instead of
    // leaving a detached task holding a pooled database connection.
    let mut requests = tokio::task::JoinSet::new();
    requests.spawn(async move {
        api(
            &app,
            "PUT",
            &format!("{IFACES}/{interface_id}"),
            Some(payload),
            Some(&cookie),
            expected,
        )
        .await
    });
    observe_interface_lock_wait(pool, blocker_pid).await?;
    writer.commit().await?;
    let completed = tokio::time::timeout(Duration::from_secs(30), requests.join_next())
        .await?
        .ok_or("Interface waiter task disappeared")?;
    let response = completed??;
    if !replaces_lans {
        // Keeping the junction row ID also proves omission did not delete and
        // reinsert the same effective LAN after reading it correctly.
        let memberships: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, wirelesslan_id FROM dcim_interface_wirelesslans WHERE interface_id = $1 ORDER BY wirelesslan_id",
        )
        .bind(interface_id)
        .fetch_all(pool)
        .await?;
        assert_eq!(memberships, vec![(committed_membership_id, committed_lan)]);
    }
    Ok(response)
}

async fn interface_lan_lock_wait_regression(
    app: &axum::Router,
    cookie: &str,
    pool: &PgPool,
    device: Uuid,
    lan_a: Uuid,
) -> TestResult {
    let isolation: String = sqlx::query_scalar("SHOW default_transaction_isolation")
        .fetch_one(pool)
        .await?;
    assert_eq!(isolation, "read committed");
    let lan_b = api(
        app,
        "POST",
        LANS,
        Some(json!({"ssid":"Concurrent LAN B"})),
        Some(cookie),
        StatusCode::CREATED,
    )
    .await?;
    let lan_b = Uuid::parse_str(id(&lan_b)?)?;
    let interface = api(
        app, "POST", IFACES,
        Some(json!({"device_id":device, "name":"wlan-lock-wait", "type":"ieee802.11ax", "speed":"9007199254740993", "wireless_lans":[lan_a]})),
        Some(cookie), StatusCode::CREATED,
    ).await?;
    let interface_id = Uuid::parse_str(id(&interface)?)?;
    let path = format!("{IFACES}/{interface_id}");

    let changed = update_behind_lan_writer(
        app,
        cookie,
        pool,
        interface_id,
        lan_b,
        json!({"description":"metadata after committed replacement"}),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(changed["wireless_lans"], json!([lan_b]));
    assert_eq!(
        changed["description"],
        "metadata after committed replacement"
    );
    assert_eq!(changed["speed"], "9007199254740993");
    let memberships: Vec<Uuid> = sqlx::query_scalar("SELECT wirelesslan_id FROM dcim_interface_wirelesslans WHERE interface_id = $1 ORDER BY wirelesslan_id")
        .bind(interface_id).fetch_all(pool).await?;
    assert_eq!(memberships, vec![lan_b]);

    // Starting empty makes a stale membership snapshot miss the association
    // entirely: even an omitted list must validate the newly committed LAN B.
    let empty = api(
        app,
        "PUT",
        &path,
        Some(json!({"wireless_lans":[]})),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(empty["wireless_lans"], json!([]));
    update_behind_lan_writer(
        app,
        cookie,
        pool,
        interface_id,
        lan_b,
        json!({"type":"1000base-t"}),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let retained = api(app, "GET", &path, None, Some(cookie), StatusCode::OK).await?;
    assert_eq!(retained["wireless_lans"], json!([lan_b]));
    assert_eq!(retained["type"], "ieee802.11ax");
    assert_eq!(
        retained["description"],
        "metadata after committed replacement"
    );
    assert_eq!(retained["speed"], "9007199254740993");

    // The authenticated administrator can still explicitly replace the list
    // after the same lock wait; preserving omission must not disable writes.
    api(
        app,
        "PUT",
        &path,
        Some(json!({"wireless_lans":[lan_a]})),
        Some(cookie),
        StatusCode::OK,
    )
    .await?;
    let replaced = update_behind_lan_writer(
        app,
        cookie,
        pool,
        interface_id,
        lan_b,
        json!({"wireless_lans":[lan_a]}),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(replaced["wireless_lans"], json!([lan_a]));
    assert_eq!(replaced["speed"], "9007199254740993");
    Ok(())
}

#[test]
fn wireless_openapi_registers_native_operations() -> TestResult {
    use utoipa::OpenApi;
    let doc = serde_json::to_value(piping_backend::api::openapi::ApiDoc::openapi())?;
    for collection in [GROUPS, LANS, LINKS] {
        for method in ["get", "post"] {
            assert!(doc["paths"][collection][method].is_object());
        }
        for method in ["get", "put", "patch", "delete"] {
            assert!(doc["paths"][format!("{collection}/{{id}}")][method].is_object());
        }
        assert!(doc["paths"][format!("{collection}/bulk")]["post"].is_object());
    }
    for schema in [
        "WirelessLanGroupWrite",
        "WirelessLanWrite",
        "WirelessLinkWrite",
    ] {
        assert_eq!(
            doc["components"]["schemas"][schema]["additionalProperties"],
            false
        );
    }
    Ok(())
}

#[test]
fn wireless_list_query_parameters_match_each_kind_registry() -> TestResult {
    use piping_backend::contexts::inventory::infrastructure::persistence::repositories::wireless::WirelessKind;
    use std::collections::BTreeSet;
    use utoipa::OpenApi;

    let doc = serde_json::to_value(piping_backend::api::openapi::ApiDoc::openapi())?;
    for (collection, kind) in [
        (GROUPS, WirelessKind::Group),
        (LANS, WirelessKind::Lan),
        (LINKS, WirelessKind::Link),
    ] {
        let expected: BTreeSet<_> = ["limit", "offset", "search", "sort", "order"]
            .into_iter()
            .chain(
                kind.columns()
                    .iter()
                    .filter_map(|column| column.filter.map(|filter| filter.param)),
            )
            .collect();
        let parameters = doc["paths"][collection]["get"]["parameters"]
            .as_array()
            .ok_or("Missing Wireless list parameters")?;
        let declared: Vec<_> = parameters
            .iter()
            .filter(|parameter| parameter["in"] == "query")
            .map(|parameter| {
                parameter["name"]
                    .as_str()
                    .ok_or("Missing query parameter name")
            })
            .collect::<Result<_, _>>()?;
        let declared_names: BTreeSet<_> = declared.iter().copied().collect();
        assert_eq!(
            declared.len(),
            declared_names.len(),
            "duplicate parameters for {collection}"
        );
        assert_eq!(
            declared_names, expected,
            "GET parameters must match this kind's compiled list/filter contract: {collection}"
        );
    }
    Ok(())
}

async fn exercise(url: &str) -> TestResult {
    let pool = PgPool::connect(url).await?;
    Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await?
    .run(&pool)
    .await?;
    let mut settings = piping_backend::config::load_settings()?;
    settings.database.url = url.into();
    settings.redis.url = common::redis_url(Some(14))?;
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    common::configure_test_cipher(&mut settings);
    common::ensure_test_admin(&pool, &settings.auth.password_pepper).await?;
    let state =
        piping_backend::app::AppState::build(Arc::new(settings), pool.clone(), redis.clone())
            .await?;
    let app = piping_backend::app::build_router(state.clone());
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("Test login failed")?;
    let cookie = Some(cookie.as_str());

    // These are valid filter values on another Wireless kind, not malformed UUIDs.
    // Strict kind-specific decoding must reject them, including explicit empties.
    let unrelated_id = Uuid::now_v7().to_string();
    for (collection, param, value) in [
        (GROUPS, "group_id", unrelated_id.as_str()),
        (GROUPS, "scope_type", "dcim.site"),
        (LANS, "interface_a_id", unrelated_id.as_str()),
        (LANS, "distance_unit", "m"),
        (LINKS, "group_id", unrelated_id.as_str()),
        (LINKS, "scope_type", "dcim.site"),
    ] {
        for value in [value, ""] {
            api(
                &app,
                "GET",
                &format!("{collection}?{param}={value}"),
                None,
                cookie,
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
    }

    let owner = api(
        &app,
        "POST",
        "/api/v1/inventory/owners",
        Some(json!({"name":"Wireless list owner"})),
        cookie,
        StatusCode::CREATED,
    )
    .await?;
    let tenant = api(
        &app,
        "POST",
        "/api/v1/inventory/tenants",
        Some(json!({"name":"Wireless list tenant", "slug":"wireless-list-tenant"})),
        cookie,
        StatusCode::CREATED,
    )
    .await?;
    let vlan = api(
        &app,
        "POST",
        "/api/v1/inventory/vlans",
        Some(json!({"name":"Wireless list VLAN", "vid":177})),
        cookie,
        StatusCode::CREATED,
    )
    .await?;

    let tag = api(
        &app,
        "POST",
        "/api/v1/inventory/tags",
        Some(json!({"name":"Wireless list Tag", "slug":"wireless-list-tag", "color":"336699"})),
        cookie,
        StatusCode::CREATED,
    )
    .await?;
    let group = api(
        &app,
        "POST",
        GROUPS,
        Some(
            json!({"name":"Office", "slug":"office", "comments":"group comment", "tags":[id(&tag)?], "owner_id":id(&owner)?}),
        ),
        cookie,
        StatusCode::CREATED,
    )
    .await?;
    let group_path = format!("{GROUPS}/{}", id(&group)?);
    assert_filtered_ids(
        &app,
        cookie,
        &format!("{GROUPS}?owner_id={}", id(&owner)?),
        &[id(&group)?],
    )
    .await?;
    let child = api(
        &app,
        "POST",
        GROUPS,
        Some(json!({"name":"Floor", "slug":"floor", "parent_id":id(&group)?})),
        cookie,
        StatusCode::CREATED,
    )
    .await?;
    api(
        &app,
        "PATCH",
        &group_path,
        Some(json!({"parent_id":id(&child)?})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    api(
        &app,
        "POST",
        GROUPS,
        Some(json!({"name":"Office", "slug":"duplicate"})),
        cookie,
        StatusCode::CONFLICT,
    )
    .await?;

    let lan = api(&app, "POST", LANS, Some(json!({"ssid":"Office", "group_id":id(&group)?, "status":"reserved", "auth_type":"wpa-enterprise", "auth_cipher":"aes", "description":"LAN description", "comments":"LAN comment", "tags":[id(&tag)?]})), cookie, StatusCode::CREATED).await?;
    let lan_path = format!("{LANS}/{}", id(&lan)?);
    assert_filtered_ids(
        &app,
        cookie,
        &format!("{GROUPS}?parent_id={}", id(&group)?),
        &[id(&child)?],
    )
    .await?;
    assert_filtered_ids(
        &app,
        cookie,
        &format!(
            "{LANS}?group_id={}&status=reserved&auth_type=wpa-enterprise&auth_cipher=aes",
            id(&group)?
        ),
        &[id(&lan)?],
    )
    .await?;
    for query in [
        "group_id=not-a-uuid",
        "scope_type=users.owner",
        "status=invalid",
        "auth_type=invalid",
        "auth_cipher=invalid",
        "interface_a_id=not-a-uuid",
    ] {
        api(
            &app,
            "GET",
            &format!("{LANS}?{query}"),
            None,
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    api(
        &app,
        "GET",
        &format!("{LANS}?scope_id={}", Uuid::now_v7()),
        None,
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    for param in ["group_id", "vlan_id", "tenant_id", "owner_id"] {
        assert_filtered_ids(
            &app,
            cookie,
            &format!("{LANS}?{param}={}", Uuid::now_v7()),
            &[],
        )
        .await?;
    }
    let changed = api(
        &app,
        "PATCH",
        &lan_path,
        Some(json!({"description":"changed"})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(changed["comments"], "LAN comment");
    assert_eq!(changed["group_id"], group["id"]);
    assert_eq!(changed["auth_type"], "wpa-enterprise");
    let cleared = api(
        &app,
        "PATCH",
        &lan_path,
        Some(json!({"group_id":null})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert!(cleared["group_id"].is_null());
    for invalid in [
        json!({"status":"invalid"}),
        json!({"scope_type":"dcim.site"}),
        json!({"vlan_id":Uuid::now_v7()}),
        json!({"ssid":null}),
        json!({"scope_type_id":Uuid::now_v7()}),
    ] {
        api(
            &app,
            "PATCH",
            &lan_path,
            Some(invalid),
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let bulk = api(
        &app,
        "POST",
        &format!("{LANS}/bulk"),
        Some(json!({"items":[{"ssid":"Guest"},{"ssid":"Bad", "status":"bogus"}]})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        bulk["created"]
            .as_array()
            .ok_or("Missing bulk created")?
            .len(),
        1
    );
    assert_eq!(
        bulk["errors"]
            .as_array()
            .ok_or("Missing bulk errors")?
            .len(),
        1
    );
    api(
        &app,
        "POST",
        &format!("{LANS}/bulk"),
        Some(json!({"items":[]})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;

    let (device, _) = common::seed_device(&pool, "wireless").await?;
    let site: Uuid = sqlx::query_scalar("SELECT site_id FROM dcim_device WHERE id = $1")
        .bind(device)
        .fetch_one(&pool)
        .await?;
    let filtered = api(&app, "POST", LANS, Some(json!({"ssid":"Filtered LAN", "group_id":id(&group)?, "vlan_id":id(&vlan)?, "tenant_id":id(&tenant)?, "owner_id":id(&owner)?, "scope_type":"dcim.site", "scope_id":site})), cookie, StatusCode::CREATED).await?;
    for (param, value) in [
        ("group_id", id(&group)?.to_owned()),
        ("vlan_id", id(&vlan)?.to_owned()),
        ("tenant_id", id(&tenant)?.to_owned()),
        ("owner_id", id(&owner)?.to_owned()),
        ("scope_type=dcim.site&scope_id", site.to_string()),
    ] {
        assert_filtered_ids(
            &app,
            cookie,
            &format!("{LANS}?{param}={value}"),
            &[id(&filtered)?],
        )
        .await?;
    }
    assert_filtered_ids(
        &app,
        cookie,
        &format!("{LANS}?scope_type=dcim.location&scope_id={site}"),
        &[],
    )
    .await?;
    interface_lan_lock_wait_regression(
        &app,
        cookie.ok_or("Missing administrator cookie")?,
        &pool,
        device,
        Uuid::parse_str(id(&lan)?)?,
    )
    .await?;
    let a = api(&app, "POST", IFACES, Some(json!({"device_id":device, "name":"wlan-a", "type":"ieee802.11ax", "wireless_lans":[id(&lan)?]})), cookie, StatusCode::CREATED).await?;
    let b = api(
        &app,
        "POST",
        IFACES,
        Some(json!({"device_id":device, "name":"wlan-b", "type":"ieee802.11ax"})),
        cookie,
        StatusCode::CREATED,
    )
    .await?;
    let wired = api(
        &app,
        "POST",
        IFACES,
        Some(json!({"device_id":device, "name":"eth0", "type":"1000base-t"})),
        cookie,
        StatusCode::CREATED,
    )
    .await?;
    assert_eq!(a["wireless_lans"], json!([id(&lan)?]));
    let a_path = format!("{IFACES}/{}", id(&a)?);
    let preserved = api(
        &app,
        "PUT",
        &a_path,
        Some(json!({"description":"keep LANs"})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(preserved["wireless_lans"], a["wireless_lans"]);
    api(
        &app,
        "PUT",
        &a_path,
        Some(json!({"wireless_lans":[Uuid::now_v7()]})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    assert_eq!(
        api(&app, "GET", &a_path, None, cookie, StatusCode::OK).await?["wireless_lans"],
        a["wireless_lans"]
    );
    api(
        &app,
        "PUT",
        &format!("{IFACES}/{}", id(&wired)?),
        Some(json!({"wireless_lans":[id(&lan)?]})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    api(
        &app,
        "POST",
        LINKS,
        Some(json!({"interface_a_id":id(&a)?, "interface_b_id":id(&wired)?})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let link = api(&app, "POST", LINKS, Some(json!({"interface_a_id":id(&a)?, "interface_b_id":id(&b)?, "ssid":"Backhaul", "distance":12.5, "distance_unit":"m", "comments":"link comment", "tenant_id":id(&tenant)?, "owner_id":id(&owner)?})), cookie, StatusCode::CREATED).await?;
    let link_path = format!("{LINKS}/{}", id(&link)?);
    assert_filtered_ids(
        &app,
        cookie,
        &format!(
            "{LINKS}?tenant_id={}&owner_id={}",
            id(&tenant)?,
            id(&owner)?
        ),
        &[id(&link)?],
    )
    .await?;
    assert_filtered_ids(
        &app,
        cookie,
        &format!(
            "{LINKS}?interface_a_id={}&interface_b_id={}&distance_unit=m",
            id(&a)?,
            id(&b)?
        ),
        &[id(&link)?],
    )
    .await?;
    assert_filtered_ids(
        &app,
        cookie,
        &format!("{IFACES}?wireless_link_id={}", id(&link)?),
        &[id(&a)?, id(&b)?],
    )
    .await?;
    assert_filtered_ids(
        &app,
        cookie,
        &format!("{IFACES}?wireless_lans={}&search=wlan-a", id(&lan)?),
        &[id(&a)?],
    )
    .await?;
    for param in ["interface_a_id", "interface_b_id", "tenant_id", "owner_id"] {
        assert_filtered_ids(
            &app,
            cookie,
            &format!("{LINKS}?{param}={}", Uuid::now_v7()),
            &[],
        )
        .await?;
        api(
            &app,
            "GET",
            &format!("{LINKS}?{param}=invalid"),
            None,
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    for param in ["wireless_link_id", "wireless_lans"] {
        api(
            &app,
            "GET",
            &format!("{IFACES}?{param}=invalid"),
            None,
            cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    use piping_backend::contexts::inventory::infrastructure::persistence::repositories::{
        dcim::interface_repository, wireless,
    };
    for (path, columns) in [
        (GROUPS, wireless::WIRELESS_GROUP_COLUMNS),
        (LANS, wireless::WIRELESS_LAN_COLUMNS),
        (LINKS, wireless::WIRELESS_LINK_COLUMNS),
        (IFACES, interface_repository::INTERFACE_COLUMNS),
    ] {
        assert_list_sort_contract(&app, cookie, path, columns).await?;
    }
    assert_eq!(link["distance"], json!(12.5));
    assert_eq!(
        api(&app, "GET", &a_path, None, cookie, StatusCode::OK).await?["wireless_link_id"],
        link["id"]
    );
    api(
        &app,
        "POST",
        LINKS,
        Some(json!({"interface_a_id":id(&a)?, "interface_b_id":id(&b)?})),
        cookie,
        StatusCode::CONFLICT,
    )
    .await?;
    api(
        &app,
        "PATCH",
        &link_path,
        Some(json!({"interface_b_id":id(&a)?})),
        cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let link = api(
        &app,
        "PATCH",
        &link_path,
        Some(json!({"distance":null, "distance_unit":null})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert!(link["distance"].is_null());
    assert_eq!(link["comments"], "link comment");

    // Exercise the actual unique/FK baseline constraints, not only REST validators.
    let error = sqlx::query("INSERT INTO dcim_interface_wirelesslans (id,interface_id,wirelesslan_id) VALUES ($1,$2,$3)").bind(Uuid::now_v7()).bind(Uuid::parse_str(id(&a)?)?).bind(Uuid::parse_str(id(&lan)?)?).execute(&pool).await.unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23505")
    );
    let error = sqlx::query("INSERT INTO dcim_interface_wirelesslans (id,interface_id,wirelesslan_id) VALUES ($1,$2,$3)").bind(Uuid::now_v7()).bind(Uuid::parse_str(id(&a)?)?).bind(Uuid::now_v7()).execute(&pool).await.unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23503")
    );

    api(
        &app,
        "DELETE",
        &link_path,
        None,
        cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;
    assert!(
        api(&app, "GET", &a_path, None, cookie, StatusCode::OK).await?["wireless_link_id"]
            .is_null()
    );
    let empty = api(
        &app,
        "PUT",
        &a_path,
        Some(json!({"wireless_lans":[]})),
        cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(empty["wireless_lans"], json!([]));

    sqlx::query("INSERT INTO rbac_policy (id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) VALUES ($1,'PWA-91 deny mutations','deny',$2,$3,NULL,$4,'[]'::jsonb,true)")
        .bind(Uuid::now_v7()).bind(json!(["CREATE","UPDATE","DELETE"])).bind(json!(["INVENTORY"])).bind(json!(["role:00000000-0000-7000-8000-000000000002"])).execute(&pool).await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    for (collection, object, payload) in [
        (
            GROUPS,
            group_path.as_str(),
            json!({"name":"denied","slug":"denied"}),
        ),
        (LANS, lan_path.as_str(), json!({"ssid":"denied"})),
        (
            LINKS,
            link_path.as_str(),
            json!({"interface_a_id":id(&a)?,"interface_b_id":id(&b)?}),
        ),
    ] {
        api(
            &app,
            "POST",
            collection,
            Some(payload.clone()),
            cookie,
            StatusCode::FORBIDDEN,
        )
        .await?;
        api(
            &app,
            "POST",
            &format!("{collection}/bulk"),
            Some(json!({"items":[payload.clone()]})),
            cookie,
            StatusCode::FORBIDDEN,
        )
        .await?;
        for method in ["PUT", "PATCH"] {
            api(
                &app,
                method,
                object,
                Some(payload.clone()),
                cookie,
                StatusCode::FORBIDDEN,
            )
            .await?;
        }
        api(&app, "DELETE", object, None, cookie, StatusCode::FORBIDDEN).await?;
    }
    assert_eq!(
        api(&app, "GET", &group_path, None, cookie, StatusCode::OK).await?["name"],
        "Office"
    );
    drop(app);
    drop(state);
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis via PIPING_DATABASE__URL/PIPING_REDIS__URL"]
async fn wireless_crud_m2m_constraints_and_denied_mutations() -> TestResult {
    let mut url = Url::parse(&common::database_url())?;
    url.set_path(&format!("/pwa91_http_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    let result = exercise(&url).await;
    let cleanup = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
    result?;
    cleanup?;
    Ok(())
}
