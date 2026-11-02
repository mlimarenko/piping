//! Native Tag CRUD, canonical memberships and object/token authorization.

mod common;

use axum::http::StatusCode;
use serde_json::Value;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn attribute_denied_tags_do_not_leak_through_memberships_or_pages() -> TestResult {
    use serde_json::json;
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let suffix = common::unique_suffix();
    let prefix = format!("tag-policy-{suffix}");
    let role = Uuid::now_v7();
    let user = Uuid::now_v7();
    let username = format!("tag-policy-{suffix}");
    sqlx::query("INSERT INTO rbac_role (id,name,description) VALUES ($1,$2,NULL)")
        .bind(role)
        .bind(&username)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')")
        .bind(user).bind(&username).bind(format!("{username}@test.local")).execute(&pool).await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id,password_hash,password_changed_at) SELECT $1,c.password_hash,now() FROM auth_user_credential c JOIN identity_user u ON u.id=c.user_id WHERE u.username=$2")
        .bind(user).bind(common::TEST_ADMIN_USERNAME).execute(&pool).await?;
    sqlx::query(
        "INSERT INTO rbac_role_assignment (id,user_id,group_id,role_id) VALUES ($1,$2,NULL,$3)",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .bind(role)
    .execute(&pool)
    .await?;
    let mut ids = Vec::new();
    for index in 0..3 {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO inventory_tag (id,name,slug,color,description,weight) VALUES ($1,$2,$2,'1f8b4c','',0)")
            .bind(id).bind(format!("{prefix}-{index}")).execute(&pool).await?;
        ids.push(id);
    }
    for id in &ids[1..] {
        sqlx::query("INSERT INTO inventory_tag_object_type(tag_id,content_type_id) SELECT $1,id FROM inventory_content_type WHERE app_label='dcim' AND model='site'")
            .bind(id).execute(&pool).await?;
    }
    let app = piping_backend::app::build_router(state);
    let admin = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login failed")?;
    for (effect, actions, resources, constraints) in [
        (
            "allow",
            json!(["READ", "CREATE", "UPDATE"]),
            json!(["inventory:*"]),
            Value::Null,
        ),
        (
            "deny",
            json!(["READ"]),
            json!([format!("inventory:{}", ids[1])]),
            json!({"attributes":{"object_type":"extras.tag","object_types":["dcim.site"]}}),
        ),
        (
            "deny",
            json!(["UPDATE"]),
            json!([
                format!("inventory:{}", ids[0]),
                format!("inventory:{}", ids[2])
            ]),
            json!({"attributes":{"object_type":"extras.tag","object_types":["dcim.site"]}}),
        ),
    ] {
        api(
            &app,
            "POST",
            "/api/v1/admin/access-control/policies",
            Some(json!({
                "name":format!("{prefix}-policy-{}", Uuid::now_v7()), "description":null,
                "effect":effect, "actions":actions, "resource_types":["INVENTORY"],
                "constraints":constraints, "subjects":[format!("role:{role}")],
                "resources":resources, "enabled":true
            })),
            &admin,
            StatusCode::OK,
        )
        .await?;
    }
    for constraints in [
        json!({"attributes":{"object_type":"dcim.consoleporttemplate","object_types":["dcim.site"]}}),
        json!({"or":[{"attributes":{"object_type":"extras.tag"}}, {"and":[{"attributes":{"object_type":"dcim.consoleporttemplate"}},{"attributes":{"object_types":["dcim.site"]}}]}]}),
    ] {
        api(
            &app,
            "POST",
            "/api/v1/admin/access-control/policies",
            Some(json!({
                "name":format!("{prefix}-invalid-{}", Uuid::now_v7()), "description":null,
                "effect":"deny", "actions":["READ"], "resource_types":["INVENTORY"],
                "constraints":constraints, "subjects":[format!("role:{role}")],
                "resources":["inventory:*"], "enabled":true
            })),
            &admin,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let cookie = common::login_and_get_cookie(&app, &username, common::TEST_ADMIN_PASSWORD)
        .await
        .ok_or("login failed")?;
    let page = api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags?search={prefix}&sort=name&order=asc&limit=1&offset=1"),
        None,
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(page["total"], 2);
    assert_eq!(page["items"][0]["id"], ids[2].to_string());
    let denied_reference = api(
        &app,
        "POST",
        "/api/v1/inventory/wireless_lan_groups",
        Some(json!({"name":prefix,"slug":prefix,"tags":[ids[1]]})),
        &cookie,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let denied_tag = api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{}", ids[1]),
        None,
        &cookie,
        StatusCode::FORBIDDEN,
    )
    .await?;
    for denial in [denied_reference, denied_tag] {
        let serialized = serde_json::to_string(&denial)?;
        for private_property in [
            "object_types".to_owned(),
            "dcim.site".to_owned(),
            "external_id".to_owned(),
            "piping_synced_at".to_owned(),
            format!("{prefix}-1"),
        ] {
            assert!(
                !serialized.contains(&private_property),
                "denied Tag properties must remain private"
            );
        }
    }
    api(
        &app,
        "PUT",
        &format!("/api/v1/inventory/tags/{}", ids[0]),
        Some(json!({"object_types":["dcim.site"]})),
        &cookie,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let unchanged = api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{}", ids[0]),
        None,
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(unchanged["object_types"], json!([]));
    api(
        &app,
        "PUT",
        &format!("/api/v1/inventory/tags/{}", ids[2]),
        Some(json!({"object_types":[]})),
        &cookie,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let old_denied = api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{}", ids[2]),
        None,
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(old_denied["object_types"], json!(["dcim.site"]));
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn tags_list_reads_native_definitions_with_paging_and_sort() -> TestResult {
    let state = common::build_test_app_state().await?;
    let prefix = format!("tag-read-{}", common::unique_suffix());
    let mut ids = Vec::new();
    for index in 0..3 {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO inventory_tag (id, name, slug, color, description, weight) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(format!("{prefix}-{index}"))
        .bind(format!("{prefix}-{index}"))
        .bind("1f8b4c")
        .bind("Tag definition")
        .bind(index * 10)
        .execute(&state.db_pool)
        .await?;
        ids.push(id);
    }
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("test login failed")?;

    let (status, body) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags?search={prefix}&limit=1&offset=1&sort=name&order=asc"),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let page: Value = serde_json::from_slice(&body)?;
    assert_eq!(page["total"], 3);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 1);
    assert_eq!(
        page["items"].as_array().ok_or("items array missing")?.len(),
        1
    );
    assert_eq!(page["items"][0]["id"], ids[1].to_string());
    assert_eq!(page["items"][0]["weight"], 10);

    let (status, body) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{}", ids[0]),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let tag: Value = serde_json::from_slice(&body)?;
    assert_eq!(tag["name"], format!("{prefix}-0"));
    assert_eq!(tag["slug"], format!("{prefix}-0"));
    assert_eq!(tag["color"], "1f8b4c");
    assert_eq!(tag["description"], "Tag definition");
    assert_eq!(tag["weight"], 0);
    // Imported source metadata belongs to the source binding, never the DTO.
    assert!(tag.get("external_id").is_none());
    assert!(tag.get("piping_synced_at").is_none());

    let (status, _) = common::request(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags/{}", Uuid::now_v7()),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = common::request(
        &app,
        "GET",
        "/api/v1/inventory/tags/not-a-uuid",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = common::request(
        &app,
        "GET",
        "/api/v1/inventory/tags?sort=unapproved_column",
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn tags_require_inventory_read_access() -> TestResult {
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    for uri in [
        "/api/v1/inventory/tags".to_string(),
        format!("/api/v1/inventory/tags/{}", Uuid::now_v7()),
    ] {
        let (status, _) = common::request(&app, "GET", &uri, None, None).await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
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
    let body = payload.map(|v| v.to_string());
    let (status, body) = common::request(app, method, path, body.as_deref(), Some(cookie)).await?;
    assert_eq!(status, expected, "{method} {path}");
    Ok(if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    })
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn tag_crud_rename_and_owner_membership_boundary_are_canonical() -> TestResult {
    use serde_json::json;
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
    let tag = api(&app,"POST","/api/v1/inventory/tags",Some(json!({"name":format!("Initial-{suffix}"),"slug":format!("initial-{suffix}"),"color":"1f8b4c","weight":12})),&cookie,StatusCode::CREATED).await?;
    let id = tag["id"].as_str().ok_or("tag id missing")?;
    let tag_path = format!("/api/v1/inventory/tags/{id}");
    let group = api(
        &app,
        "POST",
        "/api/v1/inventory/wireless_lan_groups",
        Some(
            json!({"name":format!("Group-{suffix}"),"slug":format!("group-{suffix}"),"tags":[id]}),
        ),
        &cookie,
        StatusCode::CREATED,
    )
    .await?;
    let group_id = group["id"].as_str().ok_or("group id missing")?;
    let group_path = format!("/api/v1/inventory/wireless_lan_groups/{group_id}");
    let renamed = api(&app,"PUT",&tag_path,Some(json!({"name":format!("Renamed-{suffix}"),"slug":format!("renamed-{suffix}"),"color":"abcdef","weight":0})),&cookie,StatusCode::OK).await?;
    assert_eq!(renamed["id"], id);
    let preserved = api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"description":"omitted tags preserve identity"})),
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(preserved["tags"], json!([id]));
    for tags in [
        Value::Null,
        json!(["renamed"]),
        json!([Uuid::now_v7().to_string()]),
        json!([id, id]),
        json!([null]),
    ] {
        api(
            &app,
            "PUT",
            &group_path,
            Some(json!({"tags":tags})),
            &cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let unchanged = api(&app, "GET", &group_path, None, &cookie, StatusCode::OK).await?;
        assert_eq!(
            unchanged["tags"],
            json!([id]),
            "rejected replacement must be atomic"
        );
    }
    for patch in [
        json!({"id":Uuid::now_v7()}),
        json!({"name":null}),
        json!({"slug":null}),
        json!({"color":null}),
        json!({"description":null}),
        json!({"weight":null}),
        json!({"color":"invalid"}),
        json!({"weight":-1}),
        json!({"description":"x".repeat(201)}),
    ] {
        api(
            &app,
            "PUT",
            &tag_path,
            Some(patch),
            &cookie,
            StatusCode::BAD_REQUEST,
        )
        .await?;
    }
    let blocked = api(
        &app,
        "DELETE",
        &tag_path,
        None,
        &cookie,
        StatusCode::CONFLICT,
    )
    .await?;
    assert_eq!(blocked["code"], "DELETE_BLOCKED");
    assert!(
        blocked["details"]["blockers"]
            .as_array()
            .ok_or("blockers missing")?
            .iter()
            .any(|v| v["table"] == "wireless_wirelesslangroup")
    );
    let cleared = api(
        &app,
        "PUT",
        &group_path,
        Some(json!({"tags":[]})),
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(cleared["tags"], json!([]));
    let set = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_target_set(id,name,mode,selector) VALUES($1,$2,'dynamic',$3)",
    )
    .bind(set)
    .bind(format!("tag-set-{suffix}"))
    .bind(json!({"tag_ids":[id]}))
    .execute(&pool)
    .await?;
    let blocked = api(
        &app,
        "DELETE",
        &tag_path,
        None,
        &cookie,
        StatusCode::CONFLICT,
    )
    .await?;
    assert!(
        blocked["details"]["blockers"]
            .as_array()
            .ok_or("selector blockers missing")?
            .iter()
            .any(|v| v["table"] == "playbook_target_set")
    );
    sqlx::query("DELETE FROM playbook_target_set WHERE id=$1")
        .bind(set)
        .execute(&pool)
        .await?;
    api(
        &app,
        "DELETE",
        &tag_path,
        None,
        &cookie,
        StatusCode::NO_CONTENT,
    )
    .await?;
    api(&app, "GET", &tag_path, None, &cookie, StatusCode::NOT_FOUND).await?;
    let bulk = api(&app,"POST","/api/v1/inventory/tags/bulk",Some(json!({"items":[{"name":format!("Bulk-{suffix}"),"slug":format!("bulk-{suffix}"),"color":"123abc"},{"name":"invalid","slug":"bad slug","color":"123abc"}]})),&cookie,StatusCode::OK).await?;
    assert_eq!(bulk["created"].as_array().ok_or("bulk created")?.len(), 1);
    assert_eq!(bulk["errors"].as_array().ok_or("bulk errors")?.len(), 1);
    let bulk_id = bulk["created"][0]["id"].as_str().ok_or("bulk id")?;
    let filtered = api(
        &app,
        "GET",
        &format!("/api/v1/inventory/tags?ids={bulk_id}&color=123abc"),
        None,
        &cookie,
        StatusCode::OK,
    )
    .await?;
    assert_eq!(filtered["total"], 1);
    assert_eq!(filtered["items"][0]["id"], bulk_id);
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn tag_object_grants_and_membership_references_respect_token_ceiling() -> TestResult {
    use serde_json::json;
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    let suffix = common::unique_suffix();
    let tag = api(&app,"POST","/api/v1/inventory/tags",Some(json!({"name":format!("Scoped-{suffix}"),"slug":format!("scoped-{suffix}"),"color":"123abc"})),&cookie,StatusCode::CREATED).await?;
    let id = tag["id"].as_str().ok_or("tag id")?;
    let group = api(&app,"POST","/api/v1/inventory/wireless_lan_groups",Some(json!({"name":format!("Scoped-group-{suffix}"),"slug":format!("scoped-group-{suffix}"),"tags":[id]})),&cookie,StatusCode::CREATED).await?;
    let group_id = group["id"].as_str().ok_or("group id")?;
    // Synthetic credential stays in memory; neither response nor value is logged.
    let credential = api(&app,"POST","/api/v1/user/settings/api-tokens",Some(json!({"name":format!("tag-ceiling-{suffix}"),"scopes":[{"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{id}")},{"action":"update","resource_type":"inventory","resource_pattern":format!("inventory:{group_id}")}]})),&cookie,StatusCode::CREATED).await?;
    let bearer = credential["token_value"].as_str().ok_or("token missing")?;
    for (method, path, payload, expected) in [
        (
            "GET",
            format!("/api/v1/inventory/tags/{id}"),
            None,
            StatusCode::OK,
        ),
        (
            "GET",
            "/api/v1/inventory/tags".into(),
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            format!("/api/v1/inventory/tags/{}", Uuid::now_v7()),
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            format!("/api/v1/inventory/tags/{id}"),
            Some(json!({"name":"denied"})),
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            format!("/api/v1/inventory/wireless_lan_groups/{group_id}"),
            Some(json!({"tags":[Uuid::now_v7()]})),
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            format!("/api/v1/inventory/wireless_lan_groups/{group_id}"),
            Some(json!({"description":"omission allowed"})),
            StatusCode::OK,
        ),
        (
            "PUT",
            format!("/api/v1/inventory/wireless_lan_groups/{group_id}"),
            Some(json!({"tags":[]})),
            StatusCode::OK,
        ),
    ] {
        let body = payload.map(|v| v.to_string());
        let (status, _) =
            common::request_with_auth(&app, method, &path, body.as_deref(), None, Some(bearer))
                .await?;
        assert_eq!(status, expected, "{method} {path}");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn tag_delete_blockers_require_read_on_every_inventory_owner_and_target_set() -> TestResult {
    use serde_json::json;
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
    let tag = api(&app,"POST","/api/v1/inventory/tags",Some(json!({"name":format!("Privacy-{suffix}"),"slug":format!("privacy-{suffix}"),"color":"123abc"})),&cookie,StatusCode::CREATED).await?;
    let id = tag["id"].as_str().ok_or("tag id")?;
    let owner_name = format!("Hidden-owner-{suffix}");
    let group = api(
        &app,
        "POST",
        "/api/v1/inventory/wireless_lan_groups",
        Some(json!({"name":owner_name,"slug":format!("hidden-owner-{suffix}"),"tags":[id]})),
        &cookie,
        StatusCode::CREATED,
    )
    .await?;
    let owner = group["id"].as_str().ok_or("owner id")?;
    let set = Uuid::now_v7();
    let set_id = set.to_string();
    let set_name = format!("Hidden-target-set-{suffix}");
    sqlx::query(
        "INSERT INTO playbook_target_set(id,name,mode,selector) VALUES($1,$2,'dynamic',$3)",
    )
    .bind(set)
    .bind(&set_name)
    .bind(json!({"tag_ids":[id]}))
    .execute(&pool)
    .await?;
    let delete_scope = json!({"action":"delete","resource_type":"inventory","resource_pattern":format!("inventory:{id}")});
    let owner_read = json!({"action":"read","resource_type":"inventory","resource_pattern":format!("inventory:{owner}")});
    let set_read = json!({"action":"read","resource_type":"playbook_target_set","resource_pattern":set.to_string()});
    for (label, scopes, read_owner, read_set) in [
        ("delete-only", vec![delete_scope.clone()], false, false),
        (
            "owner-only",
            vec![delete_scope.clone(), owner_read.clone()],
            true,
            false,
        ),
        (
            "set-only",
            vec![delete_scope.clone(), set_read.clone()],
            false,
            true,
        ),
        ("both", vec![delete_scope, owner_read, set_read], true, true),
    ] {
        // Synthetic credential values stay in memory and are never logged.
        let credential = api(
            &app,
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(json!({"name":format!("tag-blockers-{label}-{suffix}"),"scopes":scopes})),
            &cookie,
            StatusCode::CREATED,
        )
        .await?;
        let bearer = credential["token_value"].as_str().ok_or("token value")?;
        for (path, allowed) in [
            (
                format!("/api/v1/inventory/wireless_lan_groups/{owner}"),
                read_owner,
            ),
            (format!("/api/v1/playbooks/target-sets/{set}"), read_set),
        ] {
            let (status, _) =
                common::request_with_auth(&app, "GET", &path, None, None, Some(bearer)).await?;
            assert_eq!(
                status,
                if allowed {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
                "READ ceiling {label}"
            );
        }
        let (status, body) = common::request_with_auth(
            &app,
            "DELETE",
            &format!("/api/v1/inventory/tags/{id}"),
            None,
            None,
            Some(bearer),
        )
        .await?;
        assert_eq!(status, StatusCode::CONFLICT);
        let body: Value = serde_json::from_slice(&body)?;
        if read_owner && read_set {
            assert_eq!(body["code"], "DELETE_BLOCKED");
            assert_eq!(body["details"]["total_count"], 2);
            let blockers = body["details"]["blockers"]
                .as_array()
                .ok_or("structured blockers")?;
            assert!(
                blockers
                    .iter()
                    .any(|group| group["table"] == "wireless_wirelesslangroup"
                        && group["examples"][0]["id"] == owner)
            );
            assert!(
                blockers
                    .iter()
                    .any(|group| group["table"] == "playbook_target_set"
                        && group["examples"][0]["id"] == set.to_string())
            );
        } else {
            assert_eq!(body["reason_code"], "DELETE_BLOCKED");
            assert!(body.get("details").is_none_or(Value::is_null));
            let serialized = serde_json::to_string(&body)?;
            for forbidden in [
                owner,
                &owner_name,
                &set_id,
                &set_name,
                "wireless_wirelesslangroup",
                "playbook_target_set",
                "total_count",
                "blockers",
            ] {
                assert!(
                    !serialized.contains(forbidden),
                    "generic refusal must not identify unreadable references ({label})"
                );
            }
        }
        let retained: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inventory_tag WHERE id=$1)")
                .bind(id.parse::<Uuid>()?)
                .fetch_one(&pool)
                .await?;
        assert!(
            retained,
            "projection must never weaken the locked integrity refusal"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis"]
async fn declared_native_membership_sets_match_create_update_and_bulk_acceptance() -> TestResult {
    use serde_json::json;
    let state = common::build_test_app_state().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("login failed")?;
    let suffix = common::unique_suffix();
    let tag = api(&app,"POST","/api/v1/inventory/tags",Some(json!({"name":format!("Contract-{suffix}"),"slug":format!("contract-{suffix}"),"color":"123abc"})),&cookie,StatusCode::CREATED).await?;
    let tag_id = tag["id"].as_str().ok_or("Tag id")?;
    for collection in ["module-bay-types", "rack-groups", "cable-bundles"] {
        let path = format!("/api/v1/inventory/{collection}");
        let name = format!("Contract-{collection}-{suffix}");
        let created = api(
            &app,
            "POST",
            &path,
            Some(json!({"name":name})),
            &cookie,
            StatusCode::CREATED,
        )
        .await?;
        assert_eq!(
            created["tags"],
            json!([]),
            "omitted create memberships remain optional"
        );
        let id = created["id"].as_str().ok_or("Owner id")?;
        let detail = format!("{path}/{id}");
        let assigned = api(
            &app,
            "PUT",
            &detail,
            Some(json!({"tags":[tag_id]})),
            &cookie,
            StatusCode::OK,
        )
        .await?;
        assert_eq!(assigned["tags"], json!([tag_id]));
        let omitted = api(
            &app,
            "PUT",
            &detail,
            Some(json!({"description":"Tags omitted"})),
            &cookie,
            StatusCode::OK,
        )
        .await?;
        assert_eq!(
            omitted["tags"],
            json!([tag_id]),
            "omitted sparse memberships preserve the existing set"
        );
        for (index, rejected) in [
            Value::Null,
            json!(["slug"]),
            json!([tag_id, tag_id]),
            json!([null]),
            json!([format!("{{{tag_id}}}")]),
            json!([tag_id.replace('-', "")]),
        ]
        .into_iter()
        .enumerate()
        {
            let mut invalid_create =
                json!({"name":format!("Rejected-{index}-{collection}-{suffix}")});
            invalid_create["tags"] = rejected.clone();
            api(
                &app,
                "POST",
                &path,
                Some(invalid_create.clone()),
                &cookie,
                StatusCode::BAD_REQUEST,
            )
            .await?;
            api(
                &app,
                "POST",
                &format!("{path}/bulk"),
                Some(json!({"items":[invalid_create]})),
                &cookie,
                StatusCode::BAD_REQUEST,
            )
            .await?;
            api(
                &app,
                "PUT",
                &detail,
                Some(json!({"tags":rejected})),
                &cookie,
                StatusCode::BAD_REQUEST,
            )
            .await?;
        }
        let cleared = api(
            &app,
            "PUT",
            &detail,
            Some(json!({"tags":[]})),
            &cookie,
            StatusCode::OK,
        )
        .await?;
        assert_eq!(cleared["tags"], json!([]), "[] remains the clear operation");
    }
    Ok(())
}
