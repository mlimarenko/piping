//! Native Interface and VMInterface assignment behavior. PostgreSQL/Redis tests
//! use the isolated database reset owned by `common`; never run on shared data.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use piping_backend::api::rest::routes::inventory::{
    native::ipam::InterfaceUpdatePayload, virtualization::vm_interfaces::VmInterfaceUpdatePayload,
};
use piping_backend::contexts::inventory::domain::{InterfaceNetworking, InterfaceNetworkingPatch};
use piping_backend::contexts::inventory::infrastructure::persistence::{
    InterfaceRepository, UpdateInterface, UpdateVmInterface, VmInterfaceRepository,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn consumer_updates_preserve_omission_and_accept_explicit_clear() {
    let policy = Uuid::now_v7();
    let current = InterfaceNetworking {
        vlan_translation_policy_id: Some(policy),
        ..Default::default()
    };
    for (wire, expected) in [
        (json!({}), Some(policy)),
        (json!({"vlan_translation_policy_id": null}), None),
    ] {
        let patch: InterfaceNetworkingPatch = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(patch.apply(&current).vlan_translation_policy_id, expected);
        let physical: InterfaceUpdatePayload = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            physical
                .networking
                .apply(&current)
                .vlan_translation_policy_id,
            expected
        );
        let vm: VmInterfaceUpdatePayload = serde_json::from_value(wire).unwrap();
        assert_eq!(
            vm.vlan_translation_policy_id.unwrap_or(Some(policy)),
            expected
        );
    }
    let target = json!({"vlan_translation_policy_id": policy});
    let vm: VmInterfaceUpdatePayload = serde_json::from_value(target.clone()).unwrap();
    assert_eq!(vm.vlan_translation_policy_id, Some(Some(policy)));
    let physical: InterfaceUpdatePayload = serde_json::from_value(target).unwrap();
    assert_eq!(
        physical.networking.vlan_translation_policy_id,
        Some(Some(policy))
    );
    for invalid in [json!(42), json!("not-a-uuid"), json!({"id":policy})] {
        let wire = json!({"vlan_translation_policy_id":invalid});
        assert!(serde_json::from_value::<InterfaceUpdatePayload>(wire.clone()).is_err());
        assert!(serde_json::from_value::<VmInterfaceUpdatePayload>(wire).is_err());
    }
}

async fn request_json(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    path: &str,
    payload: Option<Value>,
    expected: StatusCode,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let payload = payload.map(|value| value.to_string());
    let (status, body) =
        common::request(app, method, path, payload.as_deref(), Some(cookie)).await?;
    assert_eq!(status, expected, "{method} {path}");
    Ok(serde_json::from_slice(&body)?)
}

#[tokio::test]
#[ignore = "Requires exclusively owned PostgreSQL/Redis; common resets the database"]
async fn native_consumers_assign_project_preserve_clear_and_enforce_reference_pat_read()
-> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = state.db_pool.clone();
    let interfaces = state.repositories.interfaces.clone();
    let vm_interfaces = state.repositories.vm_interfaces.clone();
    let (device, _) = common::seed_device(&state.db_pool, "translation-consumers").await?;
    let app = piping_backend::app::build_router(state);
    let admin = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .expect("admin session");
    let policy = request_json(
        &app,
        &admin,
        "POST",
        "/api/v1/inventory/vlan-translation-policies",
        Some(json!({"name":format!("consumer-policy-{}", Uuid::now_v7())})),
        StatusCode::CREATED,
    )
    .await?;
    let policy = policy["id"].as_str().expect("policy id");
    let vm = request_json(
        &app,
        &admin,
        "POST",
        "/api/v1/inventory/virtual_machines",
        Some(json!({"name":format!("consumer-vm-{}",Uuid::now_v7()),"status":"active"})),
        StatusCode::CREATED,
    )
    .await?;
    let vm = vm["id"].as_str().expect("VM id");
    for (collection, payload) in [
        (
            "interfaces",
            json!({"device_id":device,"name":"eth-translation","type":"virtual","mtu":1500,"vlan_translation_policy_id":policy}),
        ),
        (
            "vm-interfaces",
            json!({"virtual_machine_id":vm,"name":"eth-translation","mtu":1500,"vlan_translation_policy_id":policy}),
        ),
    ] {
        let collection = format!("/api/v1/inventory/{collection}");
        let created = request_json(
            &app,
            &admin,
            "POST",
            &collection,
            Some(payload),
            StatusCode::CREATED,
        )
        .await?;
        assert_eq!(created["vlan_translation_policy_id"], policy);
        let id = created["id"].as_str().expect("consumer id");
        let path = format!("{collection}/{id}");
        let projected = request_json(&app, &admin, "GET", &path, None, StatusCode::OK).await?;
        assert_eq!(projected["vlan_translation_policy_id"], policy);
        let listed = request_json(
            &app,
            &admin,
            "GET",
            &format!("{collection}?search=eth-translation"),
            None,
            StatusCode::OK,
        )
        .await?;
        assert!(
            listed["items"]
                .as_array()
                .expect("list items")
                .iter()
                .any(|item| item["id"] == id && item["vlan_translation_policy_id"] == policy)
        );

        // Hold the canonical graph. Once the native writer is observed waiting
        // on it, the graph owner must still be able to lock the consumer row.
        // Reversed row -> graph order fails NOWAIT instead of hanging the test.
        // The owner and writer occupy the app's two connections; observation
        // must not queue behind the writer in that same pool.
        let mut observer = PgConnection::connect(&common::database_url()).await?;
        let mut graph = pool.begin().await?;
        sqlx::query("SELECT dcim_acquire_hardware_graph()")
            .execute(&mut *graph)
            .await?;
        let graph_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *graph)
            .await?;
        let writer_app = app.clone();
        let writer_cookie = admin.clone();
        let writer_path = path.clone();
        let mut writer = tokio::spawn(async move {
            common::request(
                &writer_app,
                "PUT",
                &writer_path,
                Some(r#"{"description":"graph-ordered"}"#),
                Some(&writer_cookie),
            )
            .await
        });
        let observed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND query = 'SELECT dcim_acquire_hardware_graph()')")
                    .bind(graph_pid).fetch_one(&mut observer).await?;
                if waiting {
                    return Ok::<(), sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        });
        tokio::pin!(observed);
        tokio::select! {
            result = &mut writer => {
                graph.rollback().await?;
                let (status, _) = result??;
                return Err(format!("{collection}: writer completed before graph wait (status {status})").into());
            }
            result = &mut observed => {
                match result {
                    Ok(Ok(())) => {},
                    failure => {
                        graph.rollback().await?;
                        writer.abort();
                        // Cancellation is expected here; the original observation error is returned.
                        let _ = writer.await;
                        let stage = if failure.is_err() { "timed out" } else { "query failed" };
                        return Err(format!("{collection}: graph wait observation {stage}").into());
                    }
                }
            }
        }
        let table = if collection.ends_with("/vm-interfaces") {
            "virtualization_vminterface"
        } else {
            "dcim_interface"
        };
        sqlx::query(&format!(
            "SELECT id FROM {table} WHERE id=$1 FOR UPDATE NOWAIT"
        ))
        .bind(Uuid::parse_str(id)?)
        .fetch_one(&mut *graph)
        .await?;
        graph.rollback().await?;
        let (status, _) = writer.await??;
        assert_eq!(status, StatusCode::OK);

        // A token may update consumers but cannot read any policy. Supplying a
        // reference must fail; omitting it or clearing it needs no target read.
        let token = request_json(&app, &admin, "POST", "/api/v1/user/settings/api-tokens", Some(json!({
            "name":format!("consumer-update-{}",Uuid::now_v7()),
            "scopes":[{"action":"update","resource_type":"inventory","resource_pattern":"inventory:*"}]
        })), StatusCode::CREATED).await?;
        let bearer = token["token_value"].as_str().expect("token value");
        for (stage, payload, expected) in [
            (
                "supplied",
                json!({"vlan_translation_policy_id":policy,"description":"denied"}),
                StatusCode::FORBIDDEN,
            ),
            ("omitted", json!({"description":"retained"}), StatusCode::OK),
            (
                "clear",
                json!({"vlan_translation_policy_id":null}),
                StatusCode::OK,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri(&path)
                        .header("Authorization", format!("Bearer {bearer}"))
                        .header("Content-Type", "application/json")
                        .body(Body::from(payload.to_string()))?,
                )
                .await?;
            assert_eq!(response.status(), expected, "{collection}: {stage}");
            let saved = request_json(&app, &admin, "GET", &path, None, StatusCode::OK).await?;
            assert_eq!(saved["mtu"], 1500);
            if expected == StatusCode::FORBIDDEN {
                assert_ne!(saved["description"], "denied");
                assert_eq!(saved["vlan_translation_policy_id"], policy);
            } else if payload.get("vlan_translation_policy_id").is_some() {
                assert!(saved["vlan_translation_policy_id"].is_null());
            } else {
                assert_eq!(saved["vlan_translation_policy_id"], policy);
            }
        }
        // A UUID identifying the VM is not a policy; no partial metadata write.
        request_json(
            &app,
            &admin,
            "PUT",
            &path,
            Some(json!({"vlan_translation_policy_id":vm,"description":"invalid-target"})),
            StatusCode::BAD_REQUEST,
        )
        .await?;
        let saved = request_json(&app, &admin, "GET", &path, None, StatusCode::OK).await?;
        assert!(saved["vlan_translation_policy_id"].is_null());
        assert_eq!(saved["description"], "retained");

        // Bypassing HTTP validation still cannot persist a bad policy FK or
        // unrelated metadata from the same transaction.
        let missing = Uuid::now_v7();
        let consumer_id = Uuid::parse_str(id)?;
        let result = if collection.ends_with("/vm-interfaces") {
            vm_interfaces
                .update(
                    consumer_id,
                    &UpdateVmInterface {
                        vlan_translation_policy_id: Some(Some(missing)),
                        description: Some("must-rollback".into()),
                        name: None,
                        enabled: None,
                        mtu: None,
                        mode: None,
                    },
                )
                .await
                .map(|_| ())
        } else {
            interfaces
                .update(
                    consumer_id,
                    &UpdateInterface {
                        networking: InterfaceNetworkingPatch {
                            vlan_translation_policy_id: Some(Some(missing)),
                            ..Default::default()
                        },
                        description: Some("must-rollback".into()),
                        name: None,
                        type_: None,
                        label: None,
                        enabled: None,
                        mgmt_only: None,
                        wireless_link_id: None,
                        wireless_lans: None,
                    },
                )
                .await
                .map(|_| ())
        };
        assert!(result.is_err());
        let saved = request_json(&app, &admin, "GET", &path, None, StatusCode::OK).await?;
        assert!(saved["vlan_translation_policy_id"].is_null());
        assert_eq!(saved["description"], "retained");
    }
    Ok(())
}
