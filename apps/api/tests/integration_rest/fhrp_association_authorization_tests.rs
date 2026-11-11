use super::*;
use piping_backend::contexts::inventory::infrastructure::persistence::{
    IpAddressRepository, ServiceRepository, UpdateIpAddress, UpdateService,
};
use serde_json::{Value, json};

const GROUPS: &str = "/api/v1/inventory/fhrp-groups";
const MEMBERS: &str = "/api/v1/inventory/fhrp-group-assignments";
const IPS: &str = "/api/v1/inventory/ip-addresses";
const SERVICES: &str = "/api/v1/inventory/services";
const POLICIES: &str = "/api/v1/admin/access-control/policies";

async fn http(
    app: &Router,
    cookie: &str,
    method: &str,
    path: &str,
    payload: Option<Value>,
    expected: StatusCode,
) -> Value {
    let payload = payload.map(|value| value.to_string());
    let (status, body) = request(app, method, path, payload.as_deref(), Some(cookie))
        .await
        .expect("request");
    // Authentication responses can contain credentials; never print the body.
    assert_eq!(status, expected, "{method} {path}");
    if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON")
    }
}

fn path(base: &str, row: &Value) -> String {
    format!("{base}/{}", row["id"].as_str().expect("UUID"))
}

fn target(token: &str, row: &Value) -> Value {
    json!({"object_type": token, "object_id": row["id"]})
}

fn address() -> String {
    let mut bytes = *Uuid::now_v7().as_bytes();
    bytes[0] = 0xfd;
    format!("{}/128", std::net::Ipv6Addr::from(bytes))
}

async fn group(app: &Router, cookie: &str) -> Value {
    http(
        app,
        cookie,
        "POST",
        GROUPS,
        Some(json!({
            "name": format!("association-{}", unique_suffix()), "group_id": 92, "protocol": "vrrp3"
        })),
        StatusCode::CREATED,
    )
    .await
}

struct Fixture {
    state: piping_backend::app::AppState,
    app: Router,
    admin: String,
    user: Value,
    cookie: String,
    groups: Vec<Value>,
    interface: Value,
    vm_interface: Value,
}

impl Fixture {
    async fn new() -> Self {
        let state = build_test_app_state().await.expect("app state");
        let (device_id, _) = common::seed_device(&state.db_pool, "association-policy")
            .await
            .expect("device");
        let app = piping_backend::app::build_router(state.clone());
        let admin = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
            .await
            .expect("admin login");
        let name = format!("assoc-{}", unique_suffix());
        let user = http(&app, &admin, "POST", "/api/v1/admin/access-control/users", Some(json!({
            "username": name, "email": format!("{name}@test.local"), "status": "active",
            "auth_source": "local", "credential": {"mode": "manual", "password": REST_TEST_LOCAL_PASSWORD}
        })), StatusCode::OK).await;
        http(
            &app,
            &admin,
            "POST",
            POLICIES,
            Some(json!({
                "name": format!("{name}-allow"), "effect": "allow",
                "actions": ["READ", "CREATE", "UPDATE"], "resource_types": ["INVENTORY"],
                "subjects": [format!("user:{}", user["id"].as_str().expect("user"))],
                "resources": ["inventory:*"], "enabled": true
            })),
            StatusCode::OK,
        )
        .await;
        let cookie = login_and_get_cookie(&app, &name, REST_TEST_LOCAL_PASSWORD)
            .await
            .expect("user login");
        let groups = vec![
            group(&app, &admin).await,
            group(&app, &admin).await,
            group(&app, &admin).await,
        ];
        let interface = http(
            &app,
            &admin,
            "POST",
            "/api/v1/inventory/interfaces",
            Some(json!({
                "device_id": device_id, "name": "association0"
            })),
            StatusCode::CREATED,
        )
        .await;
        let vm = http(
            &app,
            &admin,
            "POST",
            "/api/v1/inventory/virtual_machines",
            Some(json!({
                "name": format!("association-vm-{}", unique_suffix())
            })),
            StatusCode::CREATED,
        )
        .await;
        let vm_interface = http(
            &app,
            &admin,
            "POST",
            "/api/v1/inventory/vm-interfaces",
            Some(json!({
                "virtual_machine_id": vm["id"], "name": "association-vm0"
            })),
            StatusCode::CREATED,
        )
        .await;
        Self {
            state,
            app,
            admin,
            user,
            cookie,
            groups,
            interface,
            vm_interface,
        }
    }

    async fn deny(&self, row: &Value, action: &str) -> Value {
        http(&self.app, &self.admin, "POST", POLICIES, Some(json!({
            "name": format!("association-deny-{}", unique_suffix()), "effect": "deny",
            "actions": [action], "resource_types": ["INVENTORY"],
            "subjects": [format!("user:{}", self.user["id"].as_str().expect("user"))],
            "resources": [format!("inventory:{}", row["id"].as_str().expect("target"))], "enabled": true
        })), StatusCode::OK).await
    }

    async fn remove_deny(&self, policy: &Value) {
        http(
            &self.app,
            &self.admin,
            "DELETE",
            &path(POLICIES, policy),
            None,
            StatusCode::OK,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the integration_rest fhrp_association_authorization_tests filter"]
async fn fhrp_association_targets_gate_create_reassociation_retained_updates_and_bulk_items() {
    let f = Fixture::new().await;
    for (token, denied) in [
        ("ipam.fhrpgroup", &f.groups[1]),
        ("dcim.interface", &f.interface),
        ("virtualization.vminterface", &f.vm_interface),
    ] {
        let (group_id, interface_type, interface_id) = if token == "ipam.fhrpgroup" {
            (&denied["id"], "dcim.interface", &f.interface["id"])
        } else {
            (&f.groups[0]["id"], token, &denied["id"])
        };
        let member = json!({"group_id": group_id, "interface_type": interface_type, "interface_id": interface_id, "priority": 100});
        // This association does not already exist: without the target gate the
        // create/reassociation can persist, rather than merely hit uniqueness.
        let bad_create_member = if token == "ipam.fhrpgroup" {
            json!({"group_id": denied["id"], "interface_type": "virtualization.vminterface",
                "interface_id": f.vm_interface["id"], "priority": 100})
        } else {
            json!({"group_id": f.groups[2]["id"], "interface_type": token,
                "interface_id": denied["id"], "priority": 100})
        };
        let existing = http(
            &f.app,
            &f.admin,
            "POST",
            MEMBERS,
            Some(member.clone()),
            StatusCode::CREATED,
        )
        .await;
        let existing_ip = http(
            &f.app,
            &f.admin,
            "POST",
            IPS,
            Some(json!({
                "address": address(), "assigned_object": target(token, denied)
            })),
            StatusCode::CREATED,
        )
        .await;
        let unassigned = http(
            &f.app,
            &f.admin,
            "POST",
            IPS,
            Some(json!({"address": address()})),
            StatusCode::CREATED,
        )
        .await;
        let other_group = group(&f.app, &f.admin).await;
        let (other_type, other_interface) = if token == "virtualization.vminterface" {
            ("dcim.interface", &f.interface)
        } else {
            ("virtualization.vminterface", &f.vm_interface)
        };
        let other_member = http(
            &f.app,
            &f.admin,
            "POST",
            MEMBERS,
            Some(json!({
                "group_id": other_group["id"], "interface_type": other_type,
                "interface_id": other_interface["id"], "priority": 100
            })),
            StatusCode::CREATED,
        )
        .await;
        let allowed_member = json!({"group_id": f.groups[2]["id"], "interface_type": "dcim.interface", "interface_id": f.interface["id"], "priority": 100});
        let policy = f.deny(denied, "UPDATE").await;

        http(
            &f.app,
            &f.cookie,
            "POST",
            MEMBERS,
            Some(bad_create_member.clone()),
            StatusCode::FORBIDDEN,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(MEMBERS, &existing),
            Some(member.clone()),
            StatusCode::FORBIDDEN,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(MEMBERS, &other_member),
            Some(bad_create_member.clone()),
            StatusCode::FORBIDDEN,
        )
        .await;
        let unchanged = http(
            &f.app,
            &f.admin,
            "GET",
            &path(MEMBERS, &other_member),
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(unchanged["group_id"], other_group["id"]);
        assert_eq!(unchanged["interface_id"], other_interface["id"]);
        let bulk = http(
            &f.app,
            &f.cookie,
            "POST",
            &format!("{MEMBERS}/bulk"),
            Some(json!({
                "items": [bad_create_member.clone(), allowed_member.clone()]
            })),
            StatusCode::OK,
        )
        .await;
        let expected_allowed = usize::from(token != "dcim.interface");
        assert_eq!(
            bulk["created"].as_array().expect("created").len(),
            expected_allowed
        );
        assert_eq!(
            bulk["errors"].as_array().expect("errors").len(),
            2 - expected_allowed
        );
        for row in bulk["created"].as_array().expect("created") {
            http(
                &f.app,
                &f.admin,
                "DELETE",
                &path(MEMBERS, row),
                None,
                StatusCode::NO_CONTENT,
            )
            .await;
        }
        http(
            &f.app,
            &f.cookie,
            "POST",
            IPS,
            Some(json!({
                "address": address(), "assigned_object": target(token, denied)
            })),
            StatusCode::FORBIDDEN,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(IPS, &unassigned),
            Some(json!({
                "assigned_object": target(token, denied)
            })),
            StatusCode::FORBIDDEN,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(IPS, &existing_ip),
            Some(json!({
                "description": "retains forbidden target"
            })),
            StatusCode::FORBIDDEN,
        )
        .await;
        let unchanged = http(
            &f.app,
            &f.admin,
            "GET",
            &path(IPS, &unassigned),
            None,
            StatusCode::OK,
        )
        .await;
        assert!(unchanged["assigned_object"].is_null());
        let unchanged = http(
            &f.app,
            &f.admin,
            "GET",
            &path(IPS, &existing_ip),
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(unchanged["assigned_object"]["object_id"], denied["id"]);
        assert_ne!(unchanged["description"], "retains forbidden target");
        f.remove_deny(&policy).await;
        // A permitted target still works; source permission alone was not the issue.
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(IPS, &unassigned),
            Some(json!({
                "assigned_object": target(token, denied)
            })),
            StatusCode::OK,
        )
        .await;
        http(
            &f.app,
            &f.admin,
            "DELETE",
            &path(MEMBERS, &existing),
            None,
            StatusCode::NO_CONTENT,
        )
        .await;
    }
}

fn service(group: &Value, ips: Vec<Value>) -> Value {
    json!({"name": format!("association-service-{}", unique_suffix()), "port_mappings": ["tcp/443"],
        "parent_object": target("ipam.fhrpgroup", group), "ipaddresses": ips})
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the integration_rest fhrp_association_authorization_tests filter"]
async fn fhrp_ip_update_rejects_stale_authorized_assignment_including_null_transitions() {
    let f = Fixture::new().await;
    let denied = f.deny(&f.groups[1], "UPDATE").await;
    for (initial, concurrent) in [
        (Some(&f.groups[0]), Some(&f.groups[1])),
        (None, Some(&f.groups[1])),
        (Some(&f.groups[0]), None),
    ] {
        let source = http(
            &f.app,
            &f.admin,
            "POST",
            IPS,
            Some(json!({
                "address": address(), "description": "before",
                "assigned_object": initial.map(|group| target("ipam.fhrpgroup", group))
            })),
            StatusCode::CREATED,
        )
        .await;
        // Establish that the caller can update this source and its initial
        // effective target before taking the handler's authorization snapshot.
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(IPS, &source),
            Some(json!({"description": "authorized before race"})),
            StatusCode::OK,
        )
        .await;
        let id = Uuid::parse_str(source["id"].as_str().expect("IP UUID")).expect("UUID");
        let snapshot = f
            .state
            .repositories
            .ip_addresses
            .get_by_id(id)
            .await
            .expect("snapshot query")
            .expect("snapshot");
        let expected = (
            snapshot.assigned_object_type_id,
            snapshot.assigned_object_id,
        );

        // Awaited HTTP completion commits the competing writer. There are no
        // timing assumptions: the stale DTO is submitted only after that commit.
        let newer = http(
            &f.app,
            &f.admin,
            "PUT",
            &path(IPS, &source),
            Some(json!({
                "assigned_object": concurrent.map(|group| target("ipam.fhrpgroup", group)),
                "description": "concurrent committed"
            })),
            StatusCode::OK,
        )
        .await;
        for assignment in [None, Some(None)] {
            let result = f
                .state
                .repositories
                .ip_addresses
                .update(
                    id,
                    &UpdateIpAddress {
                        expected_assigned_object: expected,
                        assigned_object: assignment,
                        description: Some("stale unauthorized mutation".into()),
                        ..Default::default()
                    },
                )
                .await;
            assert!(matches!(
                result,
                Err(piping_backend::errors::AppError::Conflict(_))
            ));
            let after = http(
                &f.app,
                &f.admin,
                "GET",
                &path(IPS, &source),
                None,
                StatusCode::OK,
            )
            .await;
            assert_eq!(after["description"], newer["description"]);
            assert_eq!(after["assigned_object"], newer["assigned_object"]);
        }
        // Explicit null remains a supported mutation when its compare snapshot
        // is current, including when both stored columns are already NULL.
        let current = f
            .state
            .repositories
            .ip_addresses
            .get_by_id(id)
            .await
            .expect("current query")
            .expect("current");
        let cleared = f
            .state
            .repositories
            .ip_addresses
            .update(
                id,
                &UpdateIpAddress {
                    expected_assigned_object: (
                        current.assigned_object_type_id,
                        current.assigned_object_id,
                    ),
                    assigned_object: Some(None),
                    ..Default::default()
                },
            )
            .await
            .expect("fresh explicit detach")
            .expect("updated IP");
        assert!(cleared.assigned_object_type_id.is_none());
        assert!(cleared.assigned_object_id.is_none());
    }
    f.remove_deny(&denied).await;
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the integration_rest fhrp_association_authorization_tests filter"]
async fn fhrp_service_update_rejects_stale_authorized_parent_and_binding_snapshots() {
    let f = Fixture::new().await;
    let ip_a = http(
        &f.app,
        &f.admin,
        "POST",
        IPS,
        Some(json!({
            "address": address(), "assigned_object": target("ipam.fhrpgroup", &f.groups[0])
        })),
        StatusCode::CREATED,
    )
    .await;
    let ip_b = http(
        &f.app,
        &f.admin,
        "POST",
        IPS,
        Some(json!({
            "address": address(), "assigned_object": target("ipam.fhrpgroup", &f.groups[0])
        })),
        StatusCode::CREATED,
    )
    .await;
    let denied_group = f.deny(&f.groups[1], "UPDATE").await;
    let denied_ip = f.deny(&ip_b, "UPDATE").await;
    for change_parent in [true, false] {
        let ips = if change_parent {
            vec![]
        } else {
            vec![ip_a["id"].clone()]
        };
        let source = http(
            &f.app,
            &f.admin,
            "POST",
            SERVICES,
            Some(service(&f.groups[0], ips)),
            StatusCode::CREATED,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(SERVICES, &source),
            Some(json!({"description": "authorized before race"})),
            StatusCode::OK,
        )
        .await;
        let id = Uuid::parse_str(source["id"].as_str().expect("service UUID")).expect("UUID");
        let snapshot = f
            .state
            .repositories
            .services
            .get_by_id(id)
            .await
            .expect("snapshot query")
            .expect("snapshot");
        let authorized_parent_object = (snapshot.parent_object_type_id, snapshot.parent_object_id);
        let authorized_ipaddresses = snapshot.ipaddresses.clone();
        let concurrent = if change_parent {
            json!({"parent_object": target("ipam.fhrpgroup", &f.groups[1]), "description": "concurrent committed"})
        } else {
            json!({"ipaddresses": [ip_b["id"]], "description": "concurrent committed"})
        };
        let newer = http(
            &f.app,
            &f.admin,
            "PUT",
            &path(SERVICES, &source),
            Some(concurrent),
            StatusCode::OK,
        )
        .await;
        let stale = UpdateService {
            authorized_parent_object,
            authorized_ipaddresses: authorized_ipaddresses.clone(),
            name: None,
            port_mappings: None,
            parent_object_type_id: None,
            parent_object_id: None,
            description: Some("stale unauthorized mutation".into()),
            ipaddresses: None,
        };
        let result = f.state.repositories.services.update(id, &stale).await;
        assert!(matches!(
            result,
            Err(piping_backend::errors::AppError::Conflict(_))
        ));
        let after = http(
            &f.app,
            &f.admin,
            "GET",
            &path(SERVICES, &source),
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(after["description"], newer["description"]);
        assert_eq!(after["parent_object"], newer["parent_object"]);
        assert_eq!(after["ipaddresses"], newer["ipaddresses"]);

        // Explicit authorized values may replace the competing writer's values;
        // the locked snapshot must not substitute retained newer associations.
        let explicit = UpdateService {
            parent_object_type_id: Some(authorized_parent_object.0),
            parent_object_id: Some(authorized_parent_object.1),
            ipaddresses: Some(authorized_ipaddresses.clone()),
            description: Some("explicit authorized mutation".into()),
            ..stale
        };
        let updated = f
            .state
            .repositories
            .services
            .update(id, &explicit)
            .await
            .expect("explicit authorized associations")
            .expect("service");
        assert_eq!(
            (updated.parent_object_type_id, updated.parent_object_id),
            authorized_parent_object
        );
        assert_eq!(updated.ipaddresses, authorized_ipaddresses);
    }
    f.remove_deny(&denied_ip).await;
    f.remove_deny(&denied_group).await;
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the integration_rest fhrp_association_authorization_tests filter"]
async fn fhrp_projection_withholds_group_labels_without_target_read_for_sessions_and_tokens() {
    let f = Fixture::new().await;
    let group = &f.groups[0];
    let ip = http(
        &f.app,
        &f.admin,
        "POST",
        IPS,
        Some(json!({
            "address": address(), "assigned_object": target("ipam.fhrpgroup", group)
        })),
        StatusCode::CREATED,
    )
    .await;
    let service = http(
        &f.app,
        &f.admin,
        "POST",
        SERVICES,
        Some(service(group, vec![])),
        StatusCode::CREATED,
    )
    .await;
    let policy = f.deny(group, "READ").await;
    for (base, source, field) in [
        (IPS, &ip, "assigned_object"),
        (SERVICES, &service, "parent_object"),
    ] {
        let source_path = path(base, source);
        let allowed = http(&f.app, &f.admin, "GET", &source_path, None, StatusCode::OK).await;
        let group_label = allowed[field]["label"]
            .as_str()
            .expect("existing group has a real label");
        let masked = http(&f.app, &f.cookie, "GET", &source_path, None, StatusCode::OK).await;
        assert_eq!(masked[field]["object_id"], group["id"]);
        assert_eq!(masked[field]["object_type"], "ipam.fhrpgroup");
        assert!(masked[field]["label"].is_null());
        assert!(!masked.to_string().contains(group_label));
        http(
            &f.app,
            &f.cookie,
            "GET",
            &path(GROUPS, group),
            None,
            StatusCode::FORBIDDEN,
        )
        .await;

        for include_target in [false, true] {
            // The owner is an administrator. Only the READ-only bearer scopes
            // distinguish source-only access from access to the referenced group.
            let mut scopes = vec![json!({"action": "read", "resource_type": "inventory",
                "resource_pattern": format!("inventory:{}", source["id"].as_str().expect("source UUID"))})];
            if include_target {
                scopes.push(json!({"action": "read", "resource_type": "inventory",
                    "resource_pattern": format!("inventory:{}", group["id"].as_str().expect("group UUID"))}));
            }
            let token = http(
                &f.app,
                &f.admin,
                "POST",
                "/api/v1/user/settings/api-tokens",
                Some(json!({
                    "name": format!("association-label-{}", unique_suffix()), "scopes": scopes
                })),
                StatusCode::CREATED,
            )
            .await;
            let bearer = token["token_value"].as_str().expect("bearer");
            let (status, body) =
                request_with_auth(&f.app, "GET", &source_path, None, None, Some(bearer))
                    .await
                    .expect("token source read");
            assert_eq!(status, StatusCode::OK);
            let projection: Value = serde_json::from_slice(&body).expect("source JSON");
            assert_eq!(projection[field]["object_id"], group["id"]);
            assert_eq!(projection[field]["object_type"], "ipam.fhrpgroup");
            if include_target {
                assert_eq!(projection[field]["label"], allowed[field]["label"]);
            } else {
                assert!(projection[field]["label"].is_null());
                assert!(!String::from_utf8_lossy(&body).contains(group_label));
            }
            let (status, _) = request_with_auth(
                &f.app,
                "GET",
                &path(GROUPS, group),
                None,
                None,
                Some(bearer),
            )
            .await
            .expect("token target read");
            assert_eq!(
                status,
                if include_target {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            http(
                &f.app,
                &f.admin,
                "POST",
                &format!(
                    "{}/revoke",
                    path("/api/v1/user/settings/api-tokens", &token)
                ),
                None,
                StatusCode::OK,
            )
            .await;
        }
    }
    f.remove_deny(&policy).await;
    for (base, source, field) in [
        (IPS, &ip, "assigned_object"),
        (SERVICES, &service, "parent_object"),
    ] {
        let restored = http(
            &f.app,
            &f.cookie,
            "GET",
            &path(base, source),
            None,
            StatusCode::OK,
        )
        .await;
        assert!(restored[field]["label"].as_str().is_some());
    }
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the integration_rest fhrp_association_authorization_tests filter"]
async fn fhrp_service_effective_group_and_ip_bindings_require_target_permission() {
    let f = Fixture::new().await;
    let ip = http(
        &f.app,
        &f.admin,
        "POST",
        IPS,
        Some(json!({"address": address()})),
        StatusCode::CREATED,
    )
    .await;
    for denied in [&f.groups[1], &ip] {
        let payload = service(&f.groups[1], vec![ip["id"].clone()]);
        let existing = http(
            &f.app,
            &f.admin,
            "POST",
            SERVICES,
            Some(payload.clone()),
            StatusCode::CREATED,
        )
        .await;
        let other = http(
            &f.app,
            &f.admin,
            "POST",
            SERVICES,
            Some(service(&f.groups[0], vec![])),
            StatusCode::CREATED,
        )
        .await;
        let policy = f.deny(denied, "UPDATE").await;
        let mut forbidden_create = payload.clone();
        forbidden_create["name"] = json!(format!("forbidden-create-{}", unique_suffix()));
        http(
            &f.app,
            &f.cookie,
            "POST",
            SERVICES,
            Some(forbidden_create.clone()),
            StatusCode::FORBIDDEN,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(SERVICES, &other),
            Some(json!({
                "parent_object": target("ipam.fhrpgroup", &f.groups[1]), "ipaddresses": [ip["id"]]
            })),
            StatusCode::FORBIDDEN,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(SERVICES, &existing),
            Some(json!({
                "description": "forbidden retained binding"
            })),
            StatusCode::FORBIDDEN,
        )
        .await;
        http(
            &f.app,
            &f.cookie,
            "POST",
            &format!("{SERVICES}/bulk"),
            Some(json!({
                "items": [service(&f.groups[0], vec![]), forbidden_create]
            })),
            StatusCode::FORBIDDEN,
        )
        .await;
        let unchanged = http(
            &f.app,
            &f.admin,
            "GET",
            &path(SERVICES, &other),
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(unchanged["parent_object"]["object_id"], f.groups[0]["id"]);
        assert_eq!(unchanged["ipaddresses"], json!([]));
        let unchanged = http(
            &f.app,
            &f.admin,
            "GET",
            &path(SERVICES, &existing),
            None,
            StatusCode::OK,
        )
        .await;
        assert_ne!(unchanged["description"], "forbidden retained binding");
        f.remove_deny(&policy).await;
        http(
            &f.app,
            &f.cookie,
            "PUT",
            &path(SERVICES, &existing),
            Some(json!({"description": "permitted"})),
            StatusCode::OK,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "Requires PostgreSQL and Redis; run the integration_rest fhrp_association_authorization_tests filter"]
async fn fhrp_association_token_scope_must_include_effective_targets_not_only_source() {
    let f = Fixture::new().await;
    let ip = http(
        &f.app,
        &f.admin,
        "POST",
        IPS,
        Some(json!({
            "address": address(), "assigned_object": target("ipam.fhrpgroup", &f.groups[0])
        })),
        StatusCode::CREATED,
    )
    .await;
    for include_target in [false, true] {
        let mut scopes = vec![json!({"action": "update", "resource_type": "inventory",
            "resource_pattern": format!("inventory:{}", ip["id"].as_str().expect("ip"))})];
        if include_target {
            scopes.push(json!({"action": "update", "resource_type": "inventory",
                "resource_pattern": format!("inventory:{}", f.groups[0]["id"].as_str().expect("group"))}));
        }
        let token = http(
            &f.app,
            &f.admin,
            "POST",
            "/api/v1/user/settings/api-tokens",
            Some(json!({
                "name": format!("association-token-{}", unique_suffix()), "scopes": scopes
            })),
            StatusCode::CREATED,
        )
        .await;
        let payload = json!({"description": "token effective reference"}).to_string();
        let (status, body) = request_with_auth(
            &f.app,
            "PUT",
            &path(IPS, &ip),
            Some(&payload),
            None,
            Some(token["token_value"].as_str().expect("bearer")),
        )
        .await
        .expect("token request");
        assert_eq!(
            status,
            if include_target {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        if !include_target {
            let response: Value = serde_json::from_slice(&body).expect("denial");
            assert_eq!(response["code"], "TOKEN_SCOPE_DENIED");
        } else {
            // UPDATE on the effective target permits persistence, not disclosure
            // of its label in the write response without a separate READ scope.
            let response: Value = serde_json::from_slice(&body).expect("updated IP");
            assert_eq!(response["assigned_object"]["object_id"], f.groups[0]["id"]);
            assert!(response["assigned_object"]["label"].is_null());
        }
        http(
            &f.app,
            &f.admin,
            "POST",
            &format!(
                "{}/revoke",
                path("/api/v1/user/settings/api-tokens", &token)
            ),
            None,
            StatusCode::OK,
        )
        .await;
    }
}
