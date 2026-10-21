//! Native AdminModel REST/SQLx contract. Run on an isolated PostgreSQL 18 + Redis:
//! cargo test -p piping-backend --test integration_owner_admin_models -- --ignored --test-threads=1
mod common;

use axum::http::StatusCode;
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    unique_suffix,
};
use serde_json::{Value, json};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn owner_openapi_matches_admin_model_write_contracts() -> TestResult {
    use utoipa::OpenApi;
    let doc = serde_json::to_value(piping_backend::api::openapi::ApiDoc::openapi())?;
    for prefix in ["Owner", "OwnerGroup"] {
        for suffix in ["CreatePayload", "PutPayload"] {
            let schema = &doc["components"]["schemas"][format!("{prefix}{suffix}")];
            assert!(
                schema["required"]
                    .as_array()
                    .ok_or("required fields")?
                    .contains(&json!("name"))
            );
            assert_eq!(schema["properties"]["name"]["maxLength"], 100);
            assert_eq!(schema["properties"]["description"]["maxLength"], 200);
        }
        let patch = &doc["components"]["schemas"][format!("{prefix}Payload")];
        assert!(
            !patch["required"]
                .as_array()
                .is_some_and(|v| v.contains(&json!("name")))
        );
        let props = doc["components"]["schemas"][prefix]["properties"]
            .as_object()
            .ok_or("model properties")?;
        for forbidden in [
            "tags",
            "custom_fields",
            "created",
            "last_updated",
            "created_at",
            "updated_at",
        ] {
            assert!(!props.contains_key(forbidden));
        }
    }
    Ok(())
}

#[test]
fn owner_update_contracts_match_null_rejection() -> TestResult {
    use utoipa::OpenApi;
    let doc = serde_json::to_value(piping_backend::api::openapi::ApiDoc::openapi())?;
    let owner_fields = &[
        ("name", "string"),
        ("description", "string"),
        ("users", "array"),
        ("user_groups", "array"),
    ][..];
    let group_fields = &[("name", "string"), ("description", "string")][..];
    for (name, fields) in [
        ("OwnerPayload", owner_fields),
        ("OwnerPutPayload", owner_fields),
        ("OwnerGroupPayload", group_fields),
        ("OwnerGroupPutPayload", group_fields),
    ] {
        let schema = &doc["components"]["schemas"][name];
        for (field, expected) in fields {
            assert_eq!(
                schema["properties"][*field]["type"], *expected,
                "{name}.{field}: omission is allowed, null is not"
            );
            let required = *field == "name" && name.ends_with("PutPayload");
            assert_eq!(
                schema["required"]
                    .as_array()
                    .is_some_and(|v| v.contains(&json!(field))),
                required,
                "{name}.{field}: only PUT name is required"
            );
        }
    }
    for name in ["OwnerPayload", "OwnerPutPayload"] {
        let schema = &doc["components"]["schemas"][name];
        assert_eq!(
            schema["properties"]["group_id"]["type"],
            json!(["string", "null"])
        );
    }
    Ok(())
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: StatusCode,
) -> TestResult<Value> {
    let body = body.map(|v| v.to_string());
    let (status, bytes) = request(app, method, path, body.as_deref(), Some(cookie)).await?;
    assert_eq!(status, expected, "{method} {path}");
    if bytes.is_empty() {
        Ok(Value::Null)
    } else {
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn owner_bulk_responses_redact_database_failures() -> TestResult {
    let state = build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    sqlx::query("CREATE FUNCTION pwa79_owner_bulk_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic-private-driver-detail' USING ERRCODE='XX000'; END $$")
        .execute(&pool).await?;
    for table in ["users_owner", "users_ownergroup"] {
        sqlx::query(&format!("CREATE TRIGGER pwa79_owner_bulk_fault BEFORE INSERT ON {table} FOR EACH ROW EXECUTE FUNCTION pwa79_owner_bulk_fault()"))
            .execute(&pool).await?;
    }
    let responses: TestResult<Vec<Value>> = async {
        let mut responses = Vec::new();
        for entity in ["owners", "owner_groups"] {
            responses.push(
                call(
                    &app,
                    &cookie,
                    "POST",
                    &format!("/api/v1/inventory/{entity}/bulk"),
                    Some(json!({"items":[{"name":format!("fault-{}",unique_suffix())}]})),
                    StatusCode::OK,
                )
                .await?,
            );
        }
        Ok(responses)
    }
    .await;
    // Remove the isolated fault even when the HTTP call itself failed.
    for table in ["users_owner", "users_ownergroup"] {
        sqlx::query(&format!("DROP TRIGGER pwa79_owner_bulk_fault ON {table}"))
            .execute(&pool)
            .await?;
    }
    sqlx::query("DROP FUNCTION pwa79_owner_bulk_fault()")
        .execute(&pool)
        .await?;
    for response in responses? {
        assert_eq!(response["created"], json!([]));
        assert_eq!(response["errors"][0]["index"], 1);
        assert_eq!(response["errors"][0]["error"], "Database error");
        assert!(
            !response
                .to_string()
                .contains("synthetic-private-driver-detail")
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn admin_models_reject_custom_field_scopes() -> TestResult {
    let state = build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let definition=call(&app,&cookie,"POST","/api/v1/custom-fields",Some(json!({"name":format!("plain_scope_{}",unique_suffix()),"label":"Plain scope","field_type":"text"})),StatusCode::CREATED).await?;
    let definition_id = definition["id"].as_str().ok_or("definition id")?;
    for model in ["owner", "ownergroup"] {
        let id: Uuid = sqlx::query_scalar(
            "SELECT id FROM inventory_content_type WHERE app_label='users' AND model=$1",
        )
        .bind(model)
        .fetch_one(&pool)
        .await?;
        call(&app,&cookie,"POST","/api/v1/custom-fields",Some(json!({"name":format!("owner_scope_{}",unique_suffix()),"label":"Owner scope","field_type":"text","object_types":[id]})),StatusCode::BAD_REQUEST).await?;
        call(
            &app,
            &cookie,
            "PATCH",
            &format!("/api/v1/custom-fields/{definition_id}"),
            Some(json!({"label":"must rollback","object_types":[id]})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let stored = call(
            &app,
            &cookie,
            "GET",
            &format!("/api/v1/custom-fields/{definition_id}"),
            None,
            StatusCode::OK,
        )
        .await?;
        assert_eq!(stored["object_types"], json!([]));
        assert_eq!(stored["label"], "Plain scope");
        let reader=piping_backend::contexts::inventory::infrastructure::persistence::repositories::custom_field_data_repository::SqlxCustomFieldDataRepository::new(pool.clone());
        assert!(
            reader
                .read_page(&format!("users.{model}"), &[])
                .await
                .is_err()
        );
    }
    Ok(())
}

async fn assert_read_only_owner_routes(
    app: &axum::Router,
    read_token: &str,
    owner_id: &str,
    group_id: &str,
) -> TestResult {
    let owners = "/api/v1/inventory/owners";
    let groups = "/api/v1/inventory/owner_groups";
    for entity in [owners, groups] {
        let (status, _) =
            common::request_with_auth(app, "GET", entity, None, None, Some(read_token)).await?;
        assert_eq!(status, StatusCode::OK);
        let target = if entity == owners {
            format!("{entity}/{owner_id}")
        } else {
            format!("{entity}/{group_id}")
        };
        for (method, uri, body) in [
            (
                "POST",
                entity.to_string(),
                Some(json!({"name":"forbidden"})),
            ),
            ("PUT", target.clone(), Some(json!({"name":"forbidden"}))),
            (
                "PATCH",
                target.clone(),
                Some(json!({"description":"forbidden"})),
            ),
            ("DELETE", target, None),
            (
                "POST",
                format!("{entity}/bulk"),
                Some(json!({"items":[{"name":"forbidden"}]})),
            ),
            (
                "POST",
                format!(
                    "/api/v1/inventory/resources/{}/bulk",
                    entity.rsplit('/').next().ok_or("entity key")?
                ),
                Some(json!({"operation":"delete","items":[{"id":owner_id}]})),
            ),
        ] {
            let body = body.map(|b| b.to_string());
            let (status, _) = common::request_with_auth(
                app,
                method,
                &uri,
                body.as_deref(),
                None,
                Some(read_token),
            )
            .await?;
            assert_eq!(status, StatusCode::FORBIDDEN, "read-only {method} {uri}");
        }
    }
    Ok(())
}

async fn assert_rejected_null_updates_preserve_state(
    app: &axum::Router,
    cookie: &str,
    uri: &str,
    fields: &[&str],
) -> TestResult {
    let before = call(app, cookie, "GET", uri, None, StatusCode::OK).await?;
    for method in ["PUT", "PATCH"] {
        for field in fields {
            let mut payload = json!({"name": before["name"]});
            payload[*field] = Value::Null;
            call(
                app,
                cookie,
                method,
                uri,
                Some(payload),
                StatusCode::BAD_REQUEST,
            )
            .await?;
            let after = call(app, cookie, "GET", uri, None, StatusCode::OK).await?;
            assert_eq!(
                after, before,
                "rejected {method} {field}=null must not mutate state"
            );
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn owner_crud_bulk_associations_and_lossless_updates() -> TestResult {
    let state = build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login")?;
    let suffix = unique_suffix();
    let groups = "/api/v1/inventory/owner_groups";
    let owners = "/api/v1/inventory/owners";
    let content_types = call(
        &app,
        &cookie,
        "GET",
        "/api/v1/inventory/content-types?app_label=users",
        None,
        StatusCode::OK,
    )
    .await?;
    for token in ["users.owner", "users.ownergroup"] {
        let item = content_types
            .as_array()
            .ok_or("content types")?
            .iter()
            .find(|v| v["token"] == token)
            .ok_or("owner registry entry")?;
        assert_eq!(item["supported_for_custom_fields"], false);
        assert_eq!(
            item["capabilities"],
            json!({"tags":false,"custom_fields":false,"change_dates":false})
        );
    }
    let group = call(
        &app,
        &cookie,
        "POST",
        groups,
        Some(json!({"name":format!("group-{suffix}"),"description":"kept group"})),
        StatusCode::CREATED,
    )
    .await?;
    let gid = group["id"].as_str().ok_or("group id")?;
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username=$1")
        .bind(TEST_ADMIN_USERNAME)
        .fetch_one(&pool)
        .await?;
    let identity_gid = Uuid::now_v7();
    sqlx::query("INSERT INTO identity_group (id,name) VALUES ($1,$2)")
        .bind(identity_gid)
        .bind(format!("identity-{suffix}"))
        .execute(&pool)
        .await?;
    let owner = call(&app,&cookie,"POST",owners,Some(json!({"name":format!("owner-{suffix}"),"description":"kept owner","group_id":gid,"users":[uid,uid],"user_groups":[identity_gid]})),StatusCode::CREATED).await?;
    let oid = owner["id"].as_str().ok_or("owner id")?;
    assert_eq!(owner["users"], json!([uid]));
    assert_eq!(owner.as_object().ok_or("owner object")?.len(), 6);
    assert_eq!(group.as_object().ok_or("group object")?.len(), 3);
    let path = format!("{owners}/{oid}");
    let token_response=call(&app,&cookie,"POST","/api/v1/user/settings/api-tokens",Some(json!({"name":format!("owner-read-{suffix}"),"scopes":[{"action":"read","resource_type":"inventory"}]})),StatusCode::CREATED).await?;
    let read_token = token_response["token_value"].as_str().ok_or("read token")?;
    assert_read_only_owner_routes(&app, read_token, oid, gid).await?;
    let fetched = call(&app, &cookie, "GET", &path, None, StatusCode::OK).await?;
    assert_eq!(owner, fetched);
    for method in ["PUT", "PATCH"] {
        let updated = call(
            &app,
            &cookie,
            method,
            &path,
            Some(json!({"name":format!("renamed-{suffix}")})),
            StatusCode::OK,
        )
        .await?;
        assert_eq!(updated["group_id"], json!(gid));
        assert_eq!(updated["description"], "kept owner");
        assert_eq!(updated["users"], json!([uid]));
        assert_eq!(updated["user_groups"], json!([identity_gid]));
        let cleared = call(
            &app,
            &cookie,
            method,
            &path,
            Some(json!({"name":format!("renamed-{suffix}"),"group_id":null})),
            StatusCode::OK,
        )
        .await?;
        assert!(cleared["group_id"].is_null());
        call(
            &app,
            &cookie,
            "PATCH",
            &path,
            Some(json!({"group_id":gid})),
            StatusCode::OK,
        )
        .await?;
    }
    let listed = call(&app,&cookie,"GET",&format!("{owners}?group_id={gid}&user_id={uid}&user_group_id={identity_gid}&sort=name&order=desc"),None,StatusCode::OK).await?;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["items"][0]["id"], oid);
    let empty_page = call(
        &app,
        &cookie,
        "GET",
        &format!("{owners}?group_id={gid}&offset=100"),
        None,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(empty_page["total"], 1);
    assert_eq!(empty_page["items"], json!([]));
    let group_path = format!("{groups}/{gid}");
    for (uri, fields) in [
        (&path, &["name", "description", "users", "user_groups"][..]),
        (&group_path, &["name", "description"][..]),
    ] {
        assert_rejected_null_updates_preserve_state(&app, &cookie, uri, fields).await?;
    }
    for target in [&path, &group_path] {
        call(
            &app,
            &cookie,
            "PUT",
            target,
            Some(json!({"description":"requires name"})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        for body in [
            json!({"name":null}),
            json!({"description":null}),
            json!({"tags":[]}),
            json!({"custom_fields":{}}),
            json!({"created":"2026-01-01"}),
            json!({"last_updated":"2026-01-01"}),
        ] {
            call(
                &app,
                &cookie,
                "PATCH",
                target,
                Some(body),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
    }
    let updated = call(
        &app,
        &cookie,
        "PUT",
        &group_path,
        Some(json!({"name":format!("group-renamed-{suffix}")})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(updated["description"], "kept group");
    call(
        &app,
        &cookie,
        "PATCH",
        &group_path,
        Some(json!({"description":""})),
        StatusCode::OK,
    )
    .await?;
    let blocked = call(
        &app,
        &cookie,
        "DELETE",
        &group_path,
        None,
        StatusCode::CONFLICT,
    )
    .await?;
    assert_eq!(blocked["code"], "DELETE_BLOCKED");
    let invalid = Uuid::now_v7();
    call(
        &app,
        &cookie,
        "PATCH",
        &path,
        Some(json!({"group_id":invalid})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    call(
        &app,
        &cookie,
        "PATCH",
        &path,
        Some(json!({"group_id":"bad-id"})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    call(
        &app,
        &cookie,
        "GET",
        &format!("{owners}?group_id=bad-id"),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let protected_bulk = call(
        &app,
        &cookie,
        "POST",
        "/api/v1/inventory/resources/owner_groups/bulk",
        Some(json!({"operation":"delete","items":[{"id":gid}]})),
        StatusCode::OK,
    )
    .await?;
    assert_eq!(protected_bulk["failure_count"], 1);
    call(
        &app,
        &cookie,
        "PATCH",
        &path,
        Some(json!({"description":"should rollback","users":[invalid]})),
        StatusCode::BAD_REQUEST,
    )
    .await?;
    let intact = call(&app, &cookie, "GET", &path, None, StatusCode::OK).await?;
    assert_eq!(intact["description"], "kept owner");
    assert_eq!(intact["users"], json!([uid]));
    let (device_id, _) = common::seed_device(&pool, &format!("owner-protect-{suffix}")).await?;
    sqlx::query("UPDATE dcim_device SET owner_id=$1 WHERE id=$2")
        .bind(Uuid::parse_str(oid)?)
        .bind(device_id)
        .execute(&pool)
        .await?;
    let blocked = call(&app, &cookie, "DELETE", &path, None, StatusCode::CONFLICT).await?;
    assert_eq!(blocked["code"], "DELETE_BLOCKED");
    sqlx::query("UPDATE dcim_device SET owner_id=NULL WHERE id=$1")
        .bind(device_id)
        .execute(&pool)
        .await?;
    for entity in [owners, groups] {
        call(
            &app,
            &cookie,
            "POST",
            &format!("{entity}/bulk"),
            Some(json!({"items":[]})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        for body in [
            json!({}),
            json!({"name":""}),
            json!({"name":null}),
            json!({"name":"x","description":null}),
            json!({"name":"x","tags":[]}),
            json!({"name":"x","custom_fields":{}}),
            json!({"name":"x","created":"2026-01-01"}),
            json!({"name":"x","last_updated":"2026-01-01"}),
            json!({"name":"x".repeat(101)}),
            json!({"name":"x","description":"x".repeat(201)}),
        ] {
            call(
                &app,
                &cookie,
                "POST",
                entity,
                Some(body),
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
        call(
            &app,
            &cookie,
            "GET",
            &format!("{entity}?sort=created"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        call(
            &app,
            &cookie,
            "GET",
            &format!("{entity}?order=invalid"),
            None,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        call(
            &app,
            &cookie,
            "GET",
            &format!("{entity}/{invalid}"),
            None,
            StatusCode::NOT_FOUND,
        )
        .await?;
        call(
            &app,
            &cookie,
            "PATCH",
            &format!("{entity}/{invalid}"),
            Some(json!({"name":"x"})),
            StatusCode::NOT_FOUND,
        )
        .await?;
        call(
            &app,
            &cookie,
            "DELETE",
            &format!("{entity}/{invalid}"),
            None,
            StatusCode::NOT_FOUND,
        )
        .await?;
        let bulk=call(&app,&cookie,"POST",&format!("{entity}/bulk"),Some(json!({"items":[{"name":format!("bulk-{suffix}-b")},{"name":""},{"name":format!("bulk-{suffix}-b")},{"name":format!("bulk-{suffix}-a")}]})),StatusCode::OK).await?;
        assert_eq!(bulk["created"].as_array().ok_or("created")?.len(), 2);
        assert_eq!(bulk["errors"].as_array().ok_or("errors")?.len(), 2);
        for (order, first) in [("asc", "a"), ("desc", "b")] {
            let sorted = call(
                &app,
                &cookie,
                "GET",
                &format!("{entity}?search=bulk-{suffix}&sort=name&order={order}"),
                None,
                StatusCode::OK,
            )
            .await?;
            assert_eq!(sorted["total"], 2);
            assert_eq!(sorted["items"][0]["name"], format!("bulk-{suffix}-{first}"));
        }
        let key = entity.rsplit('/').next().ok_or("entity key")?;
        let deleted=call(&app,&cookie,"POST",&format!("/api/v1/inventory/resources/{key}/bulk"),Some(json!({"operation":"delete","items":[{"id":bulk["created"][0]["id"]},{"id":bulk["created"][1]["id"]},{"id":invalid}]})),StatusCode::OK).await?;
        assert_eq!(deleted["success_count"], 2);
        assert_eq!(deleted["failure_count"], 1);
    }
    call(
        &app,
        &cookie,
        "PATCH",
        &path,
        Some(json!({"users":[],"user_groups":[]})),
        StatusCode::OK,
    )
    .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users_owner_identity_user WHERE owner_id=$1")
            .bind(Uuid::parse_str(oid)?)
            .fetch_one(&pool)
            .await?;
    assert_eq!(count, 0);
    call(
        &app,
        &cookie,
        "PATCH",
        &path,
        Some(json!({"users":[uid],"user_groups":[identity_gid]})),
        StatusCode::OK,
    )
    .await?;
    sqlx::query("DELETE FROM identity_group WHERE id=$1")
        .bind(identity_gid)
        .execute(&pool)
        .await?;
    let after_identity_delete = call(&app, &cookie, "GET", &path, None, StatusCode::OK).await?;
    assert_eq!(after_identity_delete["user_groups"], json!([]));
    assert_eq!(after_identity_delete["users"], json!([uid]));
    call(&app, &cookie, "DELETE", &path, None, StatusCode::NO_CONTENT).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users_owner_identity_user WHERE owner_id=$1")
            .bind(Uuid::parse_str(oid)?)
            .fetch_one(&pool)
            .await?;
    assert_eq!(count, 0);
    call(
        &app,
        &cookie,
        "DELETE",
        &group_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await?;
    sqlx::query("DELETE FROM identity_group WHERE id=$1")
        .bind(identity_gid)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn owner_sqlx_concurrent_updates_and_identity_cascades() -> TestResult {
    use piping_backend::contexts::inventory::{
        domain::Owner,
        infrastructure::persistence::{OwnerChanges, OwnerRepository, SqlxOwnerRepository},
    };
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let uid = Uuid::now_v7();
    let oid = Uuid::now_v7();
    let suffix = unique_suffix();
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')").bind(uid).bind(format!("owner-user-{suffix}")).bind(format!("owner-{suffix}@test.local")).execute(&pool).await?;
    let repo = SqlxOwnerRepository::new(pool.clone());
    repo.create(&Owner {
        id: oid,
        name: format!("concurrent-{suffix}"),
        description: "seeded".into(),
        group_id: None,
        users: vec![uid],
        user_groups: vec![],
    })
    .await?;
    let name = OwnerChanges {
        name: Some(format!("changed-{suffix}")),
        ..Default::default()
    };
    let description = OwnerChanges {
        description: Some("concurrent description".into()),
        ..Default::default()
    };
    let (a, b) = tokio::join!(repo.update(oid, &name), repo.update(oid, &description));
    assert!(a?.is_some());
    assert!(b?.is_some());
    let owner = repo.get_by_id(oid).await?.ok_or("concurrent owner")?;
    assert_eq!(owner.name, format!("changed-{suffix}"));
    assert_eq!(owner.description, "concurrent description");
    assert_eq!(owner.users, vec![uid]);
    sqlx::query("DELETE FROM identity_user WHERE id=$1")
        .bind(uid)
        .execute(&pool)
        .await?;
    assert!(
        repo.get_by_id(oid)
            .await?
            .ok_or("owner after user deletion")?
            .users
            .is_empty()
    );
    repo.delete(oid).await?;
    Ok(())
}
