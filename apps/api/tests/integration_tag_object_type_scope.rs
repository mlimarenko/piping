//! Native Tag scope controls. Backend execution belongs to the isolated native batch.

mod common;

use axum::http::StatusCode;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::{
    NewTag, TagAuthorizer, UpdateTag,
};
use piping_backend::contexts::inventory::domain::extras::tag::Tag;
use piping_backend::errors::AppError;
use serde_json::{Value, json};
use sqlx::Connection;
use std::time::Duration;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct LockObserver<'a> {
    pool: &'a sqlx::PgPool,
    scopes: &'a mut Vec<Vec<String>>,
}

impl TagAuthorizer for LockObserver<'_> {
    fn authorize<'a>(
        &'a mut self,
        _connection: &'a mut sqlx::PgConnection,
        tag: Tag,
    ) -> futures_util::future::BoxFuture<'a, Result<(), AppError>> {
        self.scopes.push(tag.object_types);
        Box::pin(async move {
            let shared_available: bool = sqlx::query_scalar(
                "SELECT pg_try_advisory_xact_lock_shared(hashtextextended('inventory-tag-catalog',0))",
            ).fetch_one(self.pool).await?;
            assert!(
                !shared_available,
                "persisted and proposed authorization run under the exclusive catalog lock"
            );
            Ok(())
        })
    }
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn queued_tag_writer_does_not_exhaust_authorization_pool() -> TestResult {
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    let slug = format!("scope-pool-{}", common::unique_suffix());
    let tag = api(
        &app,
        "POST",
        "/api/v1/inventory/tags",
        Some(json!({"name":slug,"slug":slug,"color":"abcdef"})),
        &cookie,
        StatusCode::CREATED,
    )
    .await?;
    let id = tag["id"].as_str().ok_or("tag id")?.parse::<Uuid>()?;
    let path = format!("/api/v1/inventory/tags/{id}");

    // An independent connection holds the row while the first request owns the
    // catalog lock. The second request can then occupy the other pool slot.
    let mut observer = sqlx::PgConnection::connect(&common::database_url()).await?;
    let mut row_lock = observer.begin().await?;
    sqlx::query("SELECT id FROM inventory_tag WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *row_lock)
        .await?;
    let first_app = app.clone();
    let first_cookie = cookie.clone();
    let first_path = path.clone();
    let first = tokio::spawn(async move {
        api(
            &first_app,
            "PUT",
            &first_path,
            Some(json!({"description":"first"})),
            &first_cookie,
            StatusCode::OK,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            sqlx::query("SELECT pg_stat_clear_snapshot()").execute(&mut *row_lock).await?;
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE inventory_tag SET name=%')")
                .fetch_one(&mut *row_lock).await?;
            if waiting { return Ok::<(), sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    let second_app = app.clone();
    let second_cookie = cookie.clone();
    let second_path = path.clone();
    let second = tokio::spawn(async move {
        api(
            &second_app,
            "PUT",
            &second_path,
            Some(json!({"description":"second"})),
            &second_cookie,
            StatusCode::OK,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            sqlx::query("SELECT pg_stat_clear_snapshot()").execute(&mut *row_lock).await?;
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE '%pg_advisory_xact_lock%inventory-tag-catalog%')")
                .fetch_one(&mut *row_lock).await?;
            if waiting { return Ok::<(), sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    row_lock.rollback().await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        first.await??;
        second.await??;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await??;
    api(&app, "DELETE", &path, None, &cookie, StatusCode::NO_CONTENT).await?;
    Ok(())
}

async fn api(
    app: &axum::Router,
    method: &str,
    path: &str,
    payload: Option<Value>,
    cookie: &str,
    expected: StatusCode,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let payload = payload.map(|value| value.to_string());
    let (status, body) =
        common::request(app, method, path, payload.as_deref(), Some(cookie)).await?;
    assert_eq!(status, expected, "{method} {path}");
    Ok(if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    })
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn membership_rechecks_current_tag_scope_under_catalog_lock() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let admin = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    let suffix = common::unique_suffix();
    let name = format!("scope-read-{suffix}");
    let tag = api(&app, "POST", "/api/v1/inventory/tags", Some(json!({"name":name,"slug":name,"color":"abcdef","object_types":["wireless.wirelesslangroup"]})), &admin, StatusCode::CREATED).await?;
    let tag_id = tag["id"].as_str().ok_or("tag id")?;
    let tag_uuid = tag_id.parse::<Uuid>()?;
    let group = api(
        &app,
        "POST",
        "/api/v1/inventory/wireless_lan_groups",
        Some(json!({"name":name,"slug":name})),
        &admin,
        StatusCode::CREATED,
    )
    .await?;
    let group_id = group["id"].as_str().ok_or("group id")?;
    let group_path = format!("/api/v1/inventory/wireless_lan_groups/{group_id}");
    let role = Uuid::now_v7();
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_role (id,name,description) VALUES ($1,$2,NULL)")
        .bind(role)
        .bind(&name)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')").bind(user).bind(&name).bind(format!("{name}@test.local")).execute(&pool).await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id,password_hash,password_changed_at) SELECT $1,c.password_hash,now() FROM auth_user_credential c JOIN identity_user u ON u.id=c.user_id WHERE u.username=$2").bind(user).bind(common::TEST_ADMIN_USERNAME).execute(&pool).await?;
    sqlx::query(
        "INSERT INTO rbac_role_assignment (id,user_id,group_id,role_id) VALUES ($1,$2,NULL,$3)",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .bind(role)
    .execute(&pool)
    .await?;
    for (actions, resources, constraints) in [
        (
            json!(["UPDATE"]),
            json!([format!("inventory:{group_id}")]),
            Value::Null,
        ),
        (
            json!(["READ"]),
            json!([format!("inventory:{tag_id}")]),
            json!({"attributes":{"object_type":"extras.tag","object_types":["wireless.wirelesslangroup"]}}),
        ),
    ] {
        api(&app, "POST", "/api/v1/admin/access-control/policies", Some(json!({"name":format!("{name}-{}",Uuid::now_v7()),"description":null,"effect":"allow","actions":actions,"resource_types":["INVENTORY"],"constraints":constraints,"subjects":[format!("role:{role}")],"resources":resources,"enabled":true})), &admin, StatusCode::OK).await?;
    }
    let user_cookie = common::login_and_get_cookie(&app, &name, common::TEST_ADMIN_PASSWORD)
        .await
        .ok_or("user login failed")?;
    api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{tag_id}"),
        None,
        &user_cookie,
        StatusCode::OK,
    )
    .await?;
    api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"tags":[tag_id]})),
        &user_cookie,
        StatusCode::OK,
    )
    .await?;
    api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"tags":[]})),
        &admin,
        StatusCode::OK,
    )
    .await?;

    // Hold the same exclusive lock as a Tag writer, and observe this request
    // waiting for its shared lock before changing the compatible scope.
    let mut observer = sqlx::PgConnection::connect(&common::database_url()).await?;
    let mut catalog = observer.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('inventory-tag-catalog',0))")
        .execute(&mut *catalog)
        .await?;
    let catalog_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *catalog)
        .await?;
    let request_app = app.clone();
    let request_cookie = user_cookie.clone();
    let request_path = group_path.clone();
    let request_tag = tag_id.to_owned();
    let mut pending = tokio::spawn(async move {
        api(
            &request_app,
            "PUT",
            &request_path,
            Some(json!({"tags":[request_tag]})),
            &request_cookie,
            StatusCode::FORBIDDEN,
        )
        .await
    });
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            sqlx::query("SELECT pg_stat_clear_snapshot()").execute(&mut *catalog).await?;
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%pg_advisory_xact_lock%inventory-tag-catalog%')").bind(catalog_pid).fetch_one(&mut *catalog).await?;
            if blocked { return Ok::<(), sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await;
    if observed.is_err() || matches!(&observed, Ok(Err(_))) {
        pending.abort();
        catalog.rollback().await?;
        return observed?.map_err(Into::into);
    }
    let inserted = sqlx::query("INSERT INTO inventory_tag_object_type(tag_id,content_type_id) SELECT $1,id FROM inventory_content_type WHERE app_label='dcim' AND model='device'").bind(tag_uuid).execute(&mut *catalog).await?;
    assert_eq!(inserted.rows_affected(), 1);
    catalog.commit().await?;
    tokio::time::timeout(Duration::from_secs(10), &mut pending).await???;
    let current = api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{tag_id}"),
        None,
        &admin,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        current["object_types"],
        json!(["dcim.device", "wireless.wirelesslangroup"])
    );
    let stored = api(&app, "GET", &group_path, None, &admin, StatusCode::OK).await?;
    assert_eq!(
        stored["tags"],
        json!([]),
        "denied request must not mutate membership"
    );
    api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{tag_id}"),
        None,
        &user_cookie,
        StatusCode::FORBIDDEN,
    )
    .await?;
    api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"tags":[tag_id]})),
        &admin,
        StatusCode::OK,
    )
    .await?;
    api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"description":"omitted"})),
        &user_cookie,
        StatusCode::OK,
    )
    .await?;
    let stored = api(&app, "GET", &group_path, None, &admin, StatusCode::OK).await?;
    assert_eq!(
        stored["tags"],
        json!([tag_id]),
        "omission preserves unreadable membership"
    );
    api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"tags":[]})),
        &user_cookie,
        StatusCode::OK,
    )
    .await?;
    let stored = api(&app, "GET", &group_path, None, &admin, StatusCode::OK).await?;
    assert_eq!(
        stored["tags"],
        json!([]),
        "explicit empty removes unreadable membership"
    );

    // A PAT is a ceiling even for the administrator; granting Tag READ makes
    // the same explicit membership update succeed.
    for (grant_read, expected) in [(false, StatusCode::FORBIDDEN), (true, StatusCode::OK)] {
        let mut scopes = vec![
            json!({"action":"update","resource_type":"inventory","resource_pattern":format!("inventory:{group_id}")}),
        ];
        if grant_read {
            scopes.push(json!({"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{tag_id}")}));
        }
        let credential = api(
            &app,
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(json!({"name":format!("{name}-pat-{grant_read}"),"scopes":scopes})),
            &admin,
            StatusCode::CREATED,
        )
        .await?;
        let bearer = credential["token_value"].as_str().ok_or("token value")?;
        let body = json!({"tags":[tag_id]}).to_string();
        let (status, _) =
            common::request_with_auth(&app, "PUT", &group_path, Some(&body), None, Some(bearer))
                .await?;
        assert_eq!(status, expected, "PAT Tag READ ceiling");
        let stored = api(&app, "GET", &group_path, None, &admin, StatusCode::OK).await?;
        assert_eq!(
            stored["tags"],
            if grant_read {
                json!([tag_id])
            } else {
                json!([])
            }
        );
    }
    api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"tags":[]})),
        &admin,
        StatusCode::OK,
    )
    .await?;
    let credential = api(&app, "POST", "/api/v1/user/settings/api-tokens", Some(json!({"name":format!("{name}-pat-race"),"scopes":[
        {"action":"update","resource_type":"inventory","resource_pattern":format!("inventory:{group_id}")},
        {"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{tag_id}")}
    ]})), &admin, StatusCode::CREATED).await?;
    let token_id = credential["id"]
        .as_str()
        .ok_or("token id")?
        .parse::<Uuid>()?;
    let bearer = credential["token_value"]
        .as_str()
        .ok_or("token value")?
        .to_owned();
    let (status, _) = common::request_with_auth(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{tag_id}"),
        None,
        None,
        Some(&bearer),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "initial PAT permits Tag READ");
    let mut catalog = observer.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('inventory-tag-catalog',0))")
        .execute(&mut *catalog)
        .await?;
    let catalog_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *catalog)
        .await?;
    let request_app = app.clone();
    let request_path = group_path.clone();
    let request_body = json!({"tags":[tag_id]}).to_string();
    let mut pending = tokio::spawn(async move {
        common::request_with_auth(
            &request_app,
            "PUT",
            &request_path,
            Some(&request_body),
            None,
            Some(&bearer),
        )
        .await
    });
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            sqlx::query("SELECT pg_stat_clear_snapshot()").execute(&mut *catalog).await?;
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%pg_advisory_xact_lock%inventory-tag-catalog%')").bind(catalog_pid).fetch_one(&mut *catalog).await?;
            if blocked { return Ok::<(), sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await;
    if observed.is_err() || matches!(&observed, Ok(Err(_))) {
        pending.abort();
        catalog.rollback().await?;
        return observed?.map_err(Into::into);
    }
    let removed = sqlx::query(
        "DELETE FROM auth_user_api_token_scope WHERE token_id=$1 AND resource_pattern=$2",
    )
    .bind(token_id)
    .bind(format!("inventory:{tag_id}"))
    .execute(&mut *catalog)
    .await?;
    assert_eq!(removed.rows_affected(), 1, "narrow only the Tag READ scope");
    catalog.commit().await?;
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), &mut pending).await???;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "current PAT scope must be enforced"
    );
    let stored = api(&app, "GET", &group_path, None, &admin, StatusCode::OK).await?;
    assert_eq!(
        stored["tags"],
        json!([]),
        "narrowed PAT cannot change membership"
    );
    Ok(())
}

#[test]
fn scope_requests_distinguish_omission_empty_and_null() -> TestResult {
    let create: NewTag = serde_json::from_value(json!({
        "name":"Scoped", "slug":"scoped", "color":"123abc"
    }))?;
    assert!(create.object_types.is_empty());
    let omitted: UpdateTag = serde_json::from_value(json!({}))?;
    assert!(omitted.object_types.is_none());
    let clear: UpdateTag = serde_json::from_value(json!({"object_types":[]}))?;
    assert_eq!(clear.object_types, Some(vec![]));
    assert!(serde_json::from_value::<UpdateTag>(json!({"object_types":null})).is_err());
    assert!(
        serde_json::from_value::<NewTag>(json!({
            "name":"Scoped", "slug":"scoped", "color":"123abc", "object_types":null
        }))
        .is_err()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn scope_crud_attach_bulk_and_restrict_failures_preserve_native_state() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    let suffix = common::unique_suffix();
    for (index, invalid) in [
        Value::Null,
        json!("dcim.device"),
        json!(["unregistered.object"]),
        json!(["users.owner"]),
        json!(["DCIM.device"]),
        json!(["dcim.device", "dcim.device"]),
        json!([null]),
    ]
    .into_iter()
    .enumerate()
    {
        let slug = format!("invalid-scope-{suffix}-{index}");
        api(
            &app,
            "POST",
            "/api/v1/inventory/tags",
            Some(json!({"name":slug,"slug":slug,"color":"123abc","object_types":invalid})),
            &cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM inventory_tag WHERE slug=$1")
            .bind(slug)
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 0, "invalid scope must not leave a definition");
    }
    let slug = format!("device-scope-{suffix}");
    let device_only = api(
        &app,
        "POST",
        "/api/v1/inventory/tags",
        Some(json!({"name":slug,"slug":slug,"color":"123abc","object_types":["dcim.device"]})),
        &cookie,
        StatusCode::CREATED,
    )
    .await?;
    assert_eq!(device_only["object_types"], json!(["dcim.device"]));
    let tag_bulk = api(
        &app,
        "POST",
        "/api/v1/inventory/tags/bulk",
        Some(json!({"items":[
            {"name":format!("scope-bulk-tag-{suffix}"),"slug":format!("scope-bulk-tag-{suffix}"),"color":"abcdef","object_types":["dcim.device"]},
            {"name":format!("scope-bulk-invalid-{suffix}"),"slug":format!("scope-bulk-invalid-{suffix}"),"color":"abcdef","object_types":["unregistered.object"]}
        ]})),
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(tag_bulk["created"].as_array().ok_or("bulk Tags")?.len(), 1);
    assert_eq!(
        tag_bulk["created"][0]["object_types"],
        json!(["dcim.device"])
    );
    assert_eq!(
        tag_bulk["errors"]
            .as_array()
            .ok_or("bulk Tag errors")?
            .len(),
        1
    );
    let bulk_tag_id = tag_bulk["created"][0]["id"].as_str().ok_or("bulk Tag id")?;
    api(
        &app,
        "DELETE",
        &format!("/api/v1/inventory/tags/{bulk_tag_id}"),
        None,
        &cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;
    let remaining_scope: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inventory_tag_object_type WHERE tag_id=$1")
            .bind(bulk_tag_id.parse::<Uuid>()?)
            .fetch_one(&pool)
            .await?;
    assert_eq!(remaining_scope, 0, "scope is owned by its deleted Tag");
    let device_tag_id = device_only["id"].as_str().ok_or("device Tag id")?;
    let site = Uuid::now_v7();
    let device = Uuid::now_v7();
    let device_name = format!("scope-device-{suffix}");
    sqlx::query("INSERT INTO dcim_site(id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES($1,$2,$2,'active','','','','','','{}')")
        .bind(site).bind(&device_name).execute(&pool).await?;
    sqlx::query("INSERT INTO dcim_device(id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count,tags) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0,$4")
        .bind(device).bind(&device_name).bind(site).bind(vec![device_tag_id.to_string()]).execute(&pool).await?;
    let device_tags: Vec<String> = sqlx::query_scalar("SELECT tags FROM dcim_device WHERE id=$1")
        .bind(device)
        .fetch_one(&pool)
        .await?;
    assert_eq!(device_tags, vec![device_tag_id.to_string()]);
    let slug = format!("union-scope-{suffix}");
    let union = api(
        &app,
        "POST",
        "/api/v1/inventory/tags",
        Some(json!({"name":slug,"slug":slug,"color":"abcdef","object_types":["wireless.wirelesslangroup","dcim.device"]})),
        &cookie,
        StatusCode::CREATED,
    )
    .await?;
    let union_id = union["id"].as_str().ok_or("union Tag id")?;
    let union_path = format!("/api/v1/inventory/tags/{union_id}");
    assert_eq!(
        union["object_types"],
        json!(["dcim.device", "wireless.wirelesslangroup"])
    );
    let allowed = api(
        &app,
        "POST",
        "/api/v1/inventory/wireless_lan_groups",
        Some(json!({"name":format!("scope-allowed-{suffix}"),"slug":format!("scope-allowed-{suffix}"),"tags":[union_id]})),
        &cookie,
        StatusCode::CREATED,
    )
    .await?;
    let owner_path = format!(
        "/api/v1/inventory/wireless_lan_groups/{}",
        allowed["id"].as_str().ok_or("owner id")?
    );
    let omitted = api(
        &app,
        "PUT",
        &union_path,
        Some(json!({"description":"Scope omitted"})),
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(omitted["object_types"], union["object_types"]);
    let replaced_union = api(
        &app,
        "PUT",
        &union_path,
        Some(json!({"object_types":["dcim.device","wireless.wirelesslangroup"]})),
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(
        replaced_union["object_types"], union["object_types"],
        "replacement validates the final set, not a transient subset"
    );
    api(
        &app,
        "PUT",
        &union_path,
        Some(json!({"description":"Must roll back","object_types":["dcim.device"]})),
        &cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let retained = api(&app, "GET", &union_path, None, &cookie, StatusCode::OK).await?;
    assert_eq!(retained["object_types"], union["object_types"]);
    assert_eq!(retained["description"], "Scope omitted");
    let retained_owner = api(&app, "GET", &owner_path, None, &cookie, StatusCode::OK).await?;
    assert_eq!(
        retained_owner["tags"],
        json!([union_id]),
        "scope edits never detach uses"
    );
    api(
        &app,
        "PUT",
        &owner_path,
        Some(json!({"tags":[device_tag_id]})),
        &cookie,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let bulk = api(
        &app,
        "POST",
        "/api/v1/inventory/wireless_lan_groups/bulk",
        Some(json!({"items":[
            {"name":format!("scope-bulk-allowed-{suffix}"),"slug":format!("scope-bulk-allowed-{suffix}"),"tags":[union_id]},
            {"name":format!("scope-bulk-denied-{suffix}"),"slug":format!("scope-bulk-denied-{suffix}"),"tags":[device_tag_id]}
        ]})),
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(bulk["created"].as_array().ok_or("bulk created")?.len(), 1);
    assert_eq!(bulk["errors"].as_array().ok_or("bulk errors")?.len(), 1);
    let cleared = api(
        &app,
        "PUT",
        &union_path,
        Some(json!({"object_types":[]})),
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(cleared["object_types"], json!([]));
    let owner = api(&app, "GET", &owner_path, None, &cookie, StatusCode::OK).await?;
    assert_eq!(owner["tags"], json!([union_id]));
    let eligible = api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags?ids={device_tag_id},{union_id}&object_type=wireless.wirelesslangroup"),
        None,
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(eligible["total"], 1);
    assert_eq!(eligible["items"][0]["id"], union_id);
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenancy_tenant(id,name,slug,description,comments,custom_field_data,tags) VALUES($1,$2,$2,'','','{}',$3)")
        .bind(tenant).bind(format!("scope-cleared-{suffix}")).bind(vec![union_id.to_string()])
        .execute(&pool).await?;
    sqlx::query("DELETE FROM tenancy_tenant WHERE id=$1")
        .bind(tenant)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn direct_writers_and_scope_restrictions_share_the_catalog_lock_and_rollback() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let suffix = common::unique_suffix();
    let tag = state.repositories.tags.create(&serde_json::from_value(json!({
        "name":format!("scope-race-{suffix}"),"slug":format!("scope-race-{suffix}"),"color":"abcdef"
    }))?).await?;
    let owner = Uuid::now_v7();
    let mut usage = pool.begin().await?;
    sqlx::query("INSERT INTO tenancy_tenant(id,name,slug,description,comments,custom_field_data,tags) VALUES($1,$2,$2,'','','{}',$3)")
        .bind(owner).bind(format!("scope-race-owner-{suffix}")).bind(vec![tag.id.to_string()])
        .execute(&mut *usage).await?;
    let mut competing = pool.begin().await?;
    let restrict_available: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended('inventory-tag-catalog',0))",
    )
    .fetch_one(&mut *competing)
    .await?;
    assert!(
        !restrict_available,
        "an in-flight attachment excludes restriction"
    );
    competing.rollback().await?;
    usage.commit().await?;
    let update: UpdateTag = serde_json::from_value(json!({"object_types":["dcim.device"]}))?;
    let mut authorized_scopes = Vec::new();
    assert!(
        state
            .repositories
            .tags
            .update(
                tag.id,
                &update,
                &mut LockObserver {
                    pool,
                    scopes: &mut authorized_scopes
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        authorized_scopes,
        vec![Vec::<String>::new(), vec!["dcim.device".to_owned()]]
    );
    assert!(
        state
            .repositories
            .tags
            .get_by_id(tag.id)
            .await?
            .ok_or("Tag disappeared")?
            .object_types
            .is_empty()
    );
    sqlx::query("DELETE FROM tenancy_tenant WHERE id=$1")
        .bind(owner)
        .execute(pool)
        .await?;

    let mut restriction = pool.begin().await?;
    sqlx::query("INSERT INTO inventory_tag_object_type(tag_id,content_type_id) SELECT $1,id FROM inventory_content_type WHERE app_label='dcim' AND model='device'")
        .bind(tag.id).execute(&mut *restriction).await?;
    let mut competing = pool.begin().await?;
    let attach_available: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock_shared(hashtextextended('inventory-tag-catalog',0))",
    )
    .fetch_one(&mut *competing)
    .await?;
    assert!(
        !attach_available,
        "an in-flight restriction excludes attachment"
    );
    competing.rollback().await?;
    restriction.commit().await?;
    let rejected = sqlx::query("INSERT INTO tenancy_tenant(id,name,slug,description,comments,custom_field_data,tags) VALUES($1,$2,$2,'','','{}',$3)")
        .bind(owner).bind(format!("scope-race-denied-{suffix}")).bind(vec![tag.id.to_string()])
        .execute(pool).await;
    assert!(
        rejected.is_err(),
        "direct/import writers cannot bypass compatibility"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tenancy_tenant WHERE id=$1")
        .bind(owner)
        .fetch_one(pool)
        .await?;
    assert_eq!(count, 0);
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(tag.id)
        .execute(pool)
        .await?;
    Ok(())
}
