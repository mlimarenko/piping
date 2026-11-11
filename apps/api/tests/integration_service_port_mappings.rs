//! REST and persistence regression for canonical service endpoints and IP bindings.
mod common;

use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    payload: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let payload = payload.map(|body| body.to_string());
    let (status, body) = common::request(app, method, path, payload.as_deref(), cookie)
        .await
        .expect("request");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis fixtures"]
async fn service_and_template_mixed_mappings_ip_bindings_crud_bulk_and_validation() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("admin login");
    let (host, _) = common::seed_device(&pool, "mappings").await?;
    let ip = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_ipaddress (id,custom_field_data,address,status,dns_name,description,comments) VALUES ($1,'{}','198.19.93.1/32','active','','','')")
        .bind(ip).execute(&pool).await?;
    let mappings = json!(["tcp/80", "udp/53", "sctp/65535"]);
    let payload = json!({"name":format!("mixed-{}", common::unique_suffix()),"port_mappings":mappings,"ipaddresses":[ip],"parent_object":{"object_type":"dcim.device","object_id":host}});

    let denied_user = Uuid::now_v7();
    let denied_name = format!("service-denied-{}", common::unique_suffix());
    sqlx::query("INSERT INTO identity_user (id,username,email,status,auth_source) VALUES ($1,$2,$3,'active','local')")
        .bind(denied_user).bind(&denied_name).bind(format!("{denied_name}@test.local")).execute(&pool).await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id,password_hash,password_changed_at) SELECT $1,c.password_hash,now() FROM auth_user_credential c JOIN identity_user u ON u.id=c.user_id WHERE u.username=$2")
        .bind(denied_user).bind(common::TEST_ADMIN_USERNAME).execute(&pool).await?;
    let denied_cookie =
        common::login_and_get_cookie(&app, &denied_name, common::TEST_ADMIN_PASSWORD)
            .await
            .expect("restricted login");
    for route in [
        "/api/v1/inventory/services",
        "/api/v1/inventory/service-templates",
    ] {
        let body = if route.ends_with("service-templates") {
            json!({"name":"denied","port_mappings":mappings})
        } else {
            payload.clone()
        };
        let (status, denied) = call(&app, "POST", route, Some(body), Some(&denied_cookie)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    }

    let (status, denied) = call(
        &app,
        "POST",
        "/api/v1/inventory/services",
        Some(payload.clone()),
        None,
    )
    .await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{status}: {denied}"
    );
    let (status, created) = call(
        &app,
        "POST",
        "/api/v1/inventory/services",
        Some(payload.clone()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["port_mappings"], mappings);
    assert_eq!(created["ipaddresses"], json!([ip]));
    assert!(created.get("protocol").is_none() && created.get("ports").is_none());
    let id = created["id"].as_str().unwrap();
    let path = format!("/api/v1/inventory/services/{id}");
    for method in ["PUT", "DELETE"] {
        let (status, body) = call(
            &app,
            method,
            &path,
            Some(json!({"ipaddresses":[]})),
            Some(&denied_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    let (status, loaded) = call(&app, "GET", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(loaded["port_mappings"], mappings);
    assert_eq!(loaded["ipaddresses"], json!([ip]));
    let (status, updated) = call(
        &app,
        "PUT",
        &path,
        Some(json!({"port_mappings":["UDP/053","tcp/443"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["port_mappings"], json!(["udp/53", "tcp/443"]));
    assert_eq!(updated["ipaddresses"], json!([ip]));
    let (status, cleared) = call(
        &app,
        "PUT",
        &path,
        Some(json!({"ipaddresses":[]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["ipaddresses"], json!([]));
    assert_eq!(cleared["port_mappings"], updated["port_mappings"]);

    for invalid in [
        json!([]),
        json!(["tcp/0"]),
        json!(["udp/65536"]),
        json!(["icmp/80"]),
        json!(["tcp/80", "TCP/080"]),
    ] {
        let mut bad = payload.clone();
        bad["port_mappings"] = invalid.clone();
        let (status, body) = call(
            &app,
            "POST",
            "/api/v1/inventory/services",
            Some(bad),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {body}");
        let (status, body) = call(
            &app,
            "PUT",
            &path,
            Some(json!({"port_mappings":invalid})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, body) = call(
        &app,
        "PUT",
        &path,
        Some(json!({"ipaddresses":[Uuid::now_v7()]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, bulk) = call(
        &app,
        "POST",
        "/api/v1/inventory/services/bulk",
        Some(json!({"items":[payload.clone(), {
            "name":format!("invalid-bulk-{}",common::unique_suffix()),
            "port_mappings":["tcp/0"],
            "parent_object":payload["parent_object"],
            "ipaddresses":[ip]
        }]})),
        Some(&cookie),
    )
    .await;
    assert!(status.is_success(), "{status}: {bulk}");
    assert_eq!(bulk["created"][0]["port_mappings"], mappings);
    assert_eq!(bulk["created"][0]["ipaddresses"], json!([ip]));
    assert_eq!(bulk["created"].as_array().unwrap().len(), 1);
    assert_eq!(bulk["errors"].as_array().unwrap().len(), 1);
    let persisted: Vec<Uuid> =
        sqlx::query_scalar("SELECT ipaddress_id FROM ipam_service_ipaddresses WHERE service_id=$1")
            .bind(Uuid::parse_str(bulk["created"][0]["id"].as_str().unwrap())?)
            .fetch_all(&pool)
            .await?;
    assert_eq!(persisted, vec![ip]);

    let template =
        json!({"name":format!("template-{}",common::unique_suffix()),"port_mappings":mappings});
    let (status, template) = call(
        &app,
        "POST",
        "/api/v1/inventory/service-templates",
        Some(template),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{template}");
    assert_eq!(template["port_mappings"], mappings);
    let template_path = format!(
        "/api/v1/inventory/service-templates/{}",
        template["id"].as_str().unwrap()
    );
    let (status, loaded) = call(&app, "GET", &template_path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(loaded["port_mappings"], mappings);
    let replacement = json!({"name":template["name"],"port_mappings":["SCTP/001","tcp/443"]});
    let (status, updated) = call(
        &app,
        "PUT",
        &template_path,
        Some(replacement),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["port_mappings"], json!(["sctp/1", "tcp/443"]));
    for invalid in [
        json!([]),
        json!(["udp/0"]),
        json!(["tcp/65536"]),
        json!(["icmp/80"]),
        json!(["tcp/80", "TCP/080"]),
    ] {
        let bad =
            json!({"name":format!("invalid-{}",common::unique_suffix()),"port_mappings":invalid});
        let (status, body) = call(
            &app,
            "POST",
            "/api/v1/inventory/service-templates",
            Some(bad),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, templates) = call(&app,"POST","/api/v1/inventory/service-templates/bulk",Some(json!({"items":[{"name":format!("bulk-template-{}",common::unique_suffix()),"port_mappings":mappings}]})),Some(&cookie)).await;
    assert!(status.is_success(), "{status}: {templates}");
    assert_eq!(templates["created"][0]["port_mappings"], mappings);
    for collection in [
        format!("/api/v1/inventory/services?parent_object_id={host}"),
        "/api/v1/inventory/service-templates?search=template-".to_owned(),
    ] {
        for order in ["asc", "desc"] {
            let (status, page) = call(
                &app,
                "GET",
                &format!("{collection}&sort=port_mappings&order={order}"),
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let arrays: Vec<Vec<String>> = page["items"]
                .as_array()
                .expect("sorted items")
                .iter()
                .map(|item| {
                    serde_json::from_value(item["port_mappings"].clone()).expect("text array")
                })
                .collect();
            assert!(!arrays.is_empty());
            assert!(
                arrays.windows(2).all(|pair| if order == "asc" {
                    pair[0] <= pair[1]
                } else {
                    pair[0] >= pair[1]
                }),
                "PostgreSQL text[] ordering"
            );
        }
        let (status, _) = call(
            &app,
            "GET",
            &format!("{collection}&sort=portMappings"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "only the canonical sort key is accepted"
        );
    }
    let (status, body) = call(&app, "DELETE", &template_path, None, Some(&cookie)).await;
    assert!(status.is_success(), "{status}: {body}");
    let (status, body) = call(&app, "DELETE", &path, None, Some(&cookie)).await;
    assert!(status.is_success(), "{status}: {body}");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ipam_service_ipaddresses WHERE service_id=$1")
            .bind(Uuid::parse_str(id)?)
            .fetch_one(&pool)
            .await?;
    assert_eq!(count, 0);
    Ok(())
}
