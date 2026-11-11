//! Cable bundles group cables without introducing trace edges.
mod common;

use axum::{Router, http::StatusCode};
use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, build_test_db_pool,
    build_test_redis, login_and_get_cookie, request, seed_device,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn trace_uri(id: Uuid, max_hops: u16) -> String {
    format!(
        "/api/v1/inventory/cabling/trace?endpoint_kind=interface&endpoint_id={id}&max_hops={max_hops}"
    )
}

async fn trace(app: &Router, cookie: &str, id: Uuid, max_hops: u16) -> TestResult<Value> {
    let (status, body) = request(app, "GET", &trace_uri(id, max_hops), None, Some(cookie)).await?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    Ok(serde_json::from_slice(&body)?)
}

async fn interface(
    pool: &PgPool,
    device: Uuid,
    name: &str,
    cable: Option<Uuid>,
) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dcim_interface
         (id,custom_field_data,device_id,name,_name,label,description,type,enabled,mgmt_only,mark_connected,cable_id)
         VALUES ($1,'{}',$2,$3,$3,'','','1000base-t',true,false,false,$4)",
    )
    .bind(id)
    .bind(device)
    .bind(name)
    .bind(cable)
    .execute(pool)
    .await?;
    Ok(id)
}

fn assert_path(trace: &Value, endpoints: &[Uuid], cables: &[Uuid]) {
    assert_eq!(trace["state"], "complete");
    assert_eq!(trace["summary"]["hop_count"], 3);
    assert_eq!(trace["summary"]["cable_count"], 2);
    assert_eq!(trace["summary"]["issue_count"], 0);
    assert_eq!(trace["issues"], json!([]));
    let segments = trace["segments"].as_array().expect("trace segments");
    assert_eq!(segments.len(), 3);
    for (index, segment) in segments.iter().enumerate() {
        assert_eq!(segment["index"], index);
        assert_eq!(segment["from"]["id"], endpoints[index].to_string());
        assert_eq!(segment["to"]["id"], endpoints[index + 1].to_string());
    }
    assert_eq!(segments[0]["traversal"], "cable");
    assert_eq!(segments[0]["cable"]["id"], cables[0].to_string());
    assert_eq!(segments[1]["traversal"], "front-rear-map");
    assert!(segments[1]["cable"].is_null());
    assert_eq!(segments[2]["traversal"], "cable");
    assert_eq!(segments[2]["cable"]["id"], cables[1].to_string());
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL and Redis; mounted router, persisted cable graph and scoped RBAC"]
async fn bundle_membership_is_not_a_junction_and_trace_preserves_native_graph_contract()
-> TestResult {
    let state = build_test_app_state().await?;
    let db = build_test_db_pool().await?;
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .ok_or("admin login failed")?;
    let (device, _) = seed_device(&db, "pwa78-trace").await?;
    let bundle = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dcim_cablebundle (id,custom_field_data,name,description,comments)
         VALUES ($1,'{}',$2,'','')",
    )
    .bind(bundle)
    .bind(format!("pwa78-{bundle}"))
    .execute(&db)
    .await?;
    let cables = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
    for cable in cables {
        sqlx::query(
            "INSERT INTO dcim_cable
             (id,custom_field_data,status,label,color,comments,description,profile,bundle_id)
             VALUES ($1,'{}','connected',$2,'','','','',$3)",
        )
        .bind(cable)
        .bind(format!("pwa78-{cable}"))
        .bind(bundle)
        .execute(&db)
        .await?;
    }
    let a = interface(&db, device, "a", Some(cables[0])).await?;
    let b = interface(&db, device, "b", Some(cables[1])).await?;
    let x = interface(&db, device, "x", Some(cables[2])).await?;
    let y = interface(&db, device, "y", Some(cables[2])).await?;
    let front = Uuid::now_v7();
    let rear = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dcim_frontport
         (id,custom_field_data,device_id,name,label,description,mark_connected,type,color,positions,cable_id)
         VALUES ($1,'{}',$2,'front','','',false,'8p8c','',1,$3)",
    )
    .bind(front)
    .bind(device)
    .bind(cables[0])
    .execute(&db)
    .await?;
    sqlx::query(
        "INSERT INTO dcim_rearport
         (id,custom_field_data,device_id,name,label,description,mark_connected,type,color,positions,cable_id)
         VALUES ($1,'{}',$2,'rear','','',false,'8p8c','',1,$3)",
    )
    .bind(rear)
    .bind(device)
    .bind(cables[1])
    .execute(&db)
    .await?;
    sqlx::query(
        "INSERT INTO dcim_portmapping
         (device_id,front_port_id,rear_port_id,front_port_position,rear_port_position)
         VALUES ($1,$2,$3,1,1)",
    )
    .bind(device)
    .bind(front)
    .bind(rear)
    .execute(&db)
    .await?;

    // Two cable segments join only through the front/rear map. The third
    // cable shares the bundle, but its endpoints must stay outside this path.
    assert_path(
        &trace(&app, &cookie, a, 50).await?,
        &[a, front, rear, b],
        &cables[..2],
    );
    assert_path(
        &trace(&app, &cookie, b, 50).await?,
        &[b, rear, front, a],
        &[cables[1], cables[0]],
    );
    let unrelated = trace(&app, &cookie, x, 50).await?;
    assert_eq!(unrelated["state"], "complete");
    assert_eq!(unrelated["summary"]["cable_count"], 1);
    assert_eq!(unrelated["segments"].as_array().ok_or("segments")?.len(), 1);
    assert_eq!(unrelated["segments"][0]["to"]["id"], y.to_string());

    // Removing membership changes neither connectivity nor the port map.
    sqlx::query("UPDATE dcim_cable SET bundle_id=NULL WHERE bundle_id=$1")
        .bind(bundle)
        .execute(&db)
        .await?;
    assert_path(
        &trace(&app, &cookie, a, 50).await?,
        &[a, front, rear, b],
        &cables[..2],
    );

    let truncated = trace(&app, &cookie, a, 1).await?;
    assert_eq!(truncated["state"], "incomplete");
    assert_eq!(truncated["summary"]["hop_count"], 1);
    assert_eq!(truncated["issues"][0]["code"], "trace-truncated");

    // A real extra termination on a cable, unlike a bundle sibling, splits.
    let branch = interface(&db, device, "branch", Some(cables[0])).await?;
    let split = trace(&app, &cookie, a, 50).await?;
    assert_eq!(split["state"], "split");
    assert_eq!(split["issues"][0]["code"], "multiple-next-hops");
    assert_eq!(split["summary"]["hop_count"], 1);
    assert_eq!(split["summary"]["cable_count"], 1);
    assert!(split["segments"][0]["note"].is_string());
    sqlx::query("UPDATE dcim_interface SET cable_id=NULL WHERE id=ANY($1)")
        .bind(vec![branch, b])
        .execute(&db)
        .await?;
    let open = trace(&app, &cookie, a, 50).await?;
    assert_eq!(open["state"], "incomplete");
    assert_eq!(open["summary"]["hop_count"], 2);
    assert_eq!(open["issues"][0]["code"], "open-end");
    assert_eq!(open["issues"][0]["endpoint"]["id"], rear.to_string());
    let detached = trace(&app, &cookie, b, 50).await?;
    assert_eq!(detached["state"], "incomplete");
    assert_eq!(detached["summary"]["hop_count"], 0);
    assert_eq!(detached["issues"][0]["code"], "open-end");

    for uri in [
        trace_uri(a, 0),
        trace_uri(a, 101),
        format!("/api/v1/inventory/cabling/trace?endpoint_kind=cable-bundle&endpoint_id={a}"),
        "/api/v1/inventory/cabling/trace?endpoint_kind=interface&endpoint_id=bad".into(),
    ] {
        let (status, _) = request(&app, "GET", &uri, None, Some(&cookie)).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
    let (status, _) = request(
        &app,
        "GET",
        &trace_uri(Uuid::now_v7(), 50),
        None,
        Some(&cookie),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let endpoints_uri = format!("/api/v1/inventory/cabling/endpoints?device_id={device}");
    let (status, _) = request(&app, "GET", &endpoints_uri, None, Some(&cookie)).await?;
    assert_eq!(status, StatusCode::OK);

    // Trace uses collection-level Inventory Read, not an object grant on its
    // starting endpoint. Restore policy/cache before asserting the response.
    let previous: Value =
        sqlx::query_scalar("SELECT resources FROM rbac_policy WHERE name='test-admin-policy'")
            .fetch_one(&db)
            .await?;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(json!([format!("inventory:{a}")]))
        .execute(&db)
        .await?;
    let mut redis = build_test_redis().await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let denied_start = request(&app, "GET", &trace_uri(a, 50), None, Some(&cookie)).await;
    let denied_other = request(&app, "GET", &trace_uri(x, 50), None, Some(&cookie)).await;
    let denied_list = request(&app, "GET", &endpoints_uri, None, Some(&cookie)).await;
    sqlx::query("UPDATE rbac_policy SET resources=$1 WHERE name='test-admin-policy'")
        .bind(previous)
        .execute(&db)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    assert_eq!(denied_start?.0, StatusCode::FORBIDDEN);
    assert_eq!(denied_other?.0, StatusCode::FORBIDDEN);
    assert_eq!(denied_list?.0, StatusCode::FORBIDDEN);
    Ok(())
}
