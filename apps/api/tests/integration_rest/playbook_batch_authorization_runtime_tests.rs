//! Runtime coverage for the batched connection-target authorization the playbook
//! launch fan-out uses.
//!
//! This file replaces the source-reading test
//! `tests/playbook_batch_authorization.rs::resolver_and_launch_use_one_batched_authorization_path`,
//! which asserted SQL text (`WHERE id = ANY($1)` three times) and marker strings
//! inside `playbooks.rs`. What those markers were a proxy for is a behavior, and
//! the behavior is what is asserted here instead: one launch request authorizes
//! the whole fan-out, and every target in it is decided from ITS OWN inventory
//! facets (device row, VM row, and the site behind them) rather than from a
//! shared, first-row or collapsed facet set.
//!
//! Boundary — "one pass" as a statement count is NOT observed here. sqlx exposes
//! no query counter, the dev Postgres has no `pg_stat_statements` (no
//! `shared_preload_libraries`), and the `pg_stat_*` counters cannot be attributed
//! to the authorization step because the same request's target-set preview reads
//! `dcim_device`/`virtualization_virtualmachine` before it. What these tests pin
//! at runtime is the consequence a wrong resolution changes: the per-target
//! verdict. A resolver that resolved the wrong row, skipped a non-first id,
//! collapsed several targets onto one facet set, or dropped the site→region hop
//! flips the observed status of at least one case below.
//!
//! Both tests are `#[ignore]`d like the rest of the DB suite. Run them against the
//! dev stack with
//! `cargo test -p piping-backend --test integration_rest playbook_batch_authorization_runtime
//! -- --ignored --test-threads=1`.

use super::common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, build_test_app_state, login_and_get_cookie, request,
    seed_device, unique_suffix,
};
use axum::Router;
use axum::http::StatusCode;
use piping_backend::contexts::inventory::application::{
    ConnectionTargetAuthorizationFacets, resolve_connection_target_authorization_facets,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const BATCH_AUTHZ_TARGET_LIMIT: i64 = 16;
/// Status of the second device: a facet value no other fan-out member carries.
const SECOND_DEVICE_STATUS: &str = "planned";

/// The verdict a launch request must reach for its whole fan-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchDecision {
    Allowed,
    Denied,
}

/// One connection-target DENY policy belonging to a single caller.
struct DenyLeg {
    /// `rbac_policy.resources`; `["*"]` for the attribute-scoped cases.
    resources: Value,
    /// `rbac_policy.constraints`; `None` becomes SQL NULL, i.e. a deny leg with
    /// no constraint at all (the deny must then fire for every target).
    constraints: Option<Value>,
}

impl DenyLeg {
    /// Constrain the deny to targets whose own facet equals `attributes`.
    fn attributes(attributes: Value) -> Self {
        Self {
            resources: json!(["*"]),
            constraints: Some(json!({ "attributes": attributes })),
        }
    }

    /// Constrain the deny by target id only, in the canonical
    /// `connection_target:{id}` resource form.
    fn resources(resources: Vec<String>) -> Self {
        Self {
            resources: json!(resources),
            constraints: None,
        }
    }
}

struct BatchAuthorizationFixtures {
    playbook_id: Uuid,
    target_set_id: Uuid,
    /// Fan-out in preview order (`sort_order`): first device, VM, second device,
    /// third device. The VM and the third device are deliberately NOT first, so a
    /// resolver that only honors the leading id is caught by their cases.
    ordered_target_ids: Vec<Uuid>,
    first_device_id: Uuid,
    first_site_id: Uuid,
    second_region_id: Uuid,
    third_site_id: Uuid,
    virtual_machine_tenant_id: Uuid,
    unused_tenant_id: Uuid,
}

fn boxed(error: impl std::fmt::Display) -> Box<dyn std::error::Error + Send + Sync> {
    Box::from(error.to_string())
}

async fn insert_tenant(
    pool: &PgPool,
    name: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tenancy_tenant (id, custom_field_data, name, slug, description, comments) \
         VALUES ($1, '{}', $2, $2, '', '')",
    )
    .bind(id)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn insert_region(
    pool: &PgPool,
    name: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dcim_region (id, custom_field_data, name, slug, description, lft, rght, \
         tree_id, level, comments) VALUES ($1, '{}', $2, $2, '', 0, 0, 0, 0, '')",
    )
    .bind(id)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn device_site(
    pool: &PgPool,
    device_id: Uuid,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    Ok(
        sqlx::query_scalar("SELECT site_id FROM dcim_device WHERE id = $1")
            .bind(device_id)
            .fetch_one(pool)
            .await?,
    )
}

async fn insert_virtual_machine(
    pool: &PgPool,
    vm_id: Uuid,
    name: &str,
    site_id: Uuid,
    role_id: Uuid,
    tenant_id: Option<Uuid>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        "INSERT INTO virtualization_virtualmachine (id, custom_field_data, name, status, comments, \
         description, interface_count, virtual_disk_count, serial, start_on_boot, site_id, role_id, \
         tenant_id) VALUES ($1, '{}', $2, 'active', '', '', 0, 0, '', '', $3, $4, $5)",
    )
    .bind(vm_id)
    .bind(name)
    .bind(site_id)
    .bind(role_id)
    .bind(tenant_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Device and site rows of the fan-out, plus the region only the second device's
/// site reaches.
struct DeviceSeeds {
    first_device_id: Uuid,
    second_device_id: Uuid,
    third_device_id: Uuid,
    first_site_id: Uuid,
    third_site_id: Uuid,
    device_role_id: Uuid,
    second_region_id: Uuid,
}

/// Seed the three fan-out devices with their sites and the region behind the
/// second device's site.
///
/// The region only exists through that site, so a region-scoped verdict can only
/// be reached if the resolution hopped site → region for that target.
async fn seed_devices_and_region(
    pool: &PgPool,
    prefix: &str,
    suffix: &str,
) -> Result<DeviceSeeds, Box<dyn std::error::Error + Send + Sync>> {
    let (first_device_id, _) = seed_device(pool, prefix).await?;
    let (second_device_id, _) = seed_device(pool, prefix).await?;
    let (third_device_id, _) = seed_device(pool, prefix).await?;
    let first_site_id = device_site(pool, first_device_id).await?;
    let second_site_id = device_site(pool, second_device_id).await?;
    let third_site_id = device_site(pool, third_device_id).await?;
    let device_role_id: Uuid = sqlx::query_scalar("SELECT role_id FROM dcim_device WHERE id = $1")
        .bind(first_device_id)
        .fetch_one(pool)
        .await?;

    let second_region_id = insert_region(pool, &format!("batch-authz-region-2-{suffix}")).await?;
    insert_region(pool, &format!("batch-authz-region-3-{suffix}")).await?;
    sqlx::query("UPDATE dcim_site SET region_id = $2 WHERE id = $1")
        .bind(second_site_id)
        .bind(second_region_id)
        .execute(pool)
        .await?;

    Ok(DeviceSeeds {
        first_device_id,
        second_device_id,
        third_device_id,
        first_site_id,
        third_site_id,
        device_role_id,
        second_region_id,
    })
}

/// Give each device its own tenant; the second device also gets the status facet
/// no other fan-out member carries, and the VM tenant is left unassigned so a
/// tenant-scoped verdict can only come from the VM's own row.
///
/// Returns `(virtual_machine_tenant_id, unused_tenant_id)`.
async fn seed_device_tenants(
    pool: &PgPool,
    suffix: &str,
    devices: &DeviceSeeds,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error + Send + Sync>> {
    let first_tenant_id = insert_tenant(pool, &format!("batch-authz-tenant-1-{suffix}")).await?;
    let second_tenant_id = insert_tenant(pool, &format!("batch-authz-tenant-2-{suffix}")).await?;
    let third_tenant_id = insert_tenant(pool, &format!("batch-authz-tenant-3-{suffix}")).await?;
    let virtual_machine_tenant_id =
        insert_tenant(pool, &format!("batch-authz-tenant-4-{suffix}")).await?;
    let unused_tenant_id =
        insert_tenant(pool, &format!("batch-authz-tenant-unused-{suffix}")).await?;

    sqlx::query("UPDATE dcim_device SET tenant_id = $2 WHERE id = $1")
        .bind(devices.first_device_id)
        .bind(first_tenant_id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_device SET tenant_id = $2, status = $3 WHERE id = $1")
        .bind(devices.second_device_id)
        .bind(second_tenant_id)
        .bind(SECOND_DEVICE_STATUS)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_device SET tenant_id = $2 WHERE id = $1")
        .bind(devices.third_device_id)
        .bind(third_tenant_id)
        .execute(pool)
        .await?;

    Ok((virtual_machine_tenant_id, unused_tenant_id))
}

/// Add the VM to the fan-out and seed an active playbook with its target set.
///
/// The playbook's run connection is `local`, which matters: the run repository
/// filters an `ssh` preview down to SSH-capable targets, which would hide members
/// from the created run group. The authorization decision under test is
/// transport-independent, so the launch is pinned to the connection that keeps
/// the created fan-out equal to the authorized one.
///
/// Returns `(playbook_id, target_set_id, ordered_target_ids)`.
async fn seed_playbook_with_target_set(
    pool: &PgPool,
    suffix: &str,
    devices: &DeviceSeeds,
    virtual_machine_tenant_id: Uuid,
) -> Result<(Uuid, Uuid, Vec<Uuid>), Box<dyn std::error::Error + Send + Sync>> {
    let virtual_machine_id = Uuid::now_v7();
    insert_virtual_machine(
        pool,
        virtual_machine_id,
        &format!("batch-authz-{suffix}-vm"),
        devices.first_site_id,
        devices.device_role_id,
        Some(virtual_machine_tenant_id),
    )
    .await?;

    // sort_order pins the preview order the authorization loop walks.
    let members = [
        (devices.first_device_id, "device", 0_i32),
        (virtual_machine_id, "virtual_machine", 1),
        (devices.second_device_id, "device", 2),
        (devices.third_device_id, "device", 3),
    ];
    let ordered_target_ids: Vec<Uuid> = members.iter().map(|(id, _, _)| *id).collect();

    let playbook_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let target_set_id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO playbook (
            id, slug, name, status, runtime, risk, category, connection_default
        )
        VALUES ($1, $2, $3, 'active', 'ansible', 'low', 'general', 'local')
        "#,
    )
    .bind(playbook_id)
    .bind(format!("batch-authz-{suffix}"))
    .bind(format!("Batch authorization {suffix}"))
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_version (
            id, playbook_id, version, language, runtime, content, content_sha256
        )
        VALUES ($1, $2, 1, 'yaml', 'ansible', '---\n- hosts: all\n', $3)
        "#,
    )
    .bind(version_id)
    .bind(playbook_id)
    .bind("0".repeat(64))
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO playbook_target_set (id, name, mode, selector) \
         VALUES ($1, $2, 'static', '{}'::jsonb)",
    )
    .bind(target_set_id)
    .bind(format!("batch-authz-{suffix}"))
    .execute(pool)
    .await?;
    for (target_id, source_kind, sort_order) in members {
        sqlx::query(
            "INSERT INTO playbook_target_set_member (target_set_id, target_kind, target_id, \
             source_kind, source_id, sort_order) \
             VALUES ($1, 'connection_target', $2, $3, $4, $5)",
        )
        .bind(target_set_id)
        .bind(target_id)
        .bind(source_kind)
        .bind(target_id)
        .bind(sort_order)
        .execute(pool)
        .await?;
    }

    sqlx::query(
        "UPDATE playbook SET current_version_id = $2, default_target_set_id = $3 WHERE id = $1",
    )
    .bind(playbook_id)
    .bind(version_id)
    .bind(target_set_id)
    .execute(pool)
    .await?;

    Ok((playbook_id, target_set_id, ordered_target_ids))
}

/// Seed four fan-out members whose facets differ in every dimension the batch
/// authorization decides on, plus one active playbook whose run connection is
/// `local`.
async fn seed_batch_authorization_fixtures(
    pool: &PgPool,
    suffix: &str,
) -> Result<BatchAuthorizationFixtures, Box<dyn std::error::Error + Send + Sync>> {
    let prefix = format!("batch-authz-{suffix}");
    let devices = seed_devices_and_region(pool, &prefix, suffix).await?;
    let (virtual_machine_tenant_id, unused_tenant_id) =
        seed_device_tenants(pool, suffix, &devices).await?;
    let (playbook_id, target_set_id, ordered_target_ids) =
        seed_playbook_with_target_set(pool, suffix, &devices, virtual_machine_tenant_id).await?;

    Ok(BatchAuthorizationFixtures {
        playbook_id,
        target_set_id,
        ordered_target_ids,
        first_device_id: devices.first_device_id,
        first_site_id: devices.first_site_id,
        second_region_id: devices.second_region_id,
        third_site_id: devices.third_site_id,
        virtual_machine_tenant_id,
        unused_tenant_id,
    })
}

/// Create a fresh caller with its own subject-scoped policies and log it in.
///
/// A fresh user per launch is what keeps the cases independent: the policy set is
/// cached per user, so a new subject resolves exactly the policies inserted for
/// it, and no case can observe another case's DENY leg or `test-admin`'s grants.
///
/// The allow policy is the launch precondition (Execute on the playbook, Read on
/// its target set, Read on every connection target); the optional DENY leg is the
/// only thing that varies per case.
async fn new_caller_cookie(
    app: &Router,
    pool: &PgPool,
    deny: Option<DenyLeg>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let user_id = Uuid::now_v7();
    let username = format!("batch-authz-{}", unique_suffix());
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, status, auth_source) \
         VALUES ($1, $2, $3, 'active', 'local')",
    )
    .bind(user_id)
    .bind(&username)
    .bind(format!("{username}@test.local"))
    .execute(pool)
    .await?;
    // Reuse the seeded admin's local credential so this suite owns no password
    // hashing of its own.
    sqlx::query(
        "INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at) \
         SELECT $1, c.password_hash, now() FROM auth_user_credential c \
         JOIN identity_user u ON u.id = c.user_id WHERE u.username = $2",
    )
    .bind(user_id)
    .bind(TEST_ADMIN_USERNAME)
    .execute(pool)
    .await?;

    let subjects = json!([format!("user:{user_id}")]);
    sqlx::query(
        "INSERT INTO rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, \
         resources, enabled) \
         VALUES ($1, $2, 'allow', $3, $4, NULL, $5, $6, true)",
    )
    .bind(Uuid::now_v7())
    .bind(format!("{username}-launch-allow"))
    .bind(json!(["READ", "EXECUTE"]))
    .bind(json!(["PLAYBOOK", "PLAYBOOK_TARGET_SET", "CONNECTION_TARGET"]))
    .bind(&subjects)
    .bind(json!(["*"]))
    .execute(pool)
    .await?;

    if let Some(deny) = deny {
        let constraints = if deny.constraints.is_some() {
            "$7::jsonb"
        } else {
            "NULL::jsonb"
        };
        let sql = format!(
            "INSERT INTO rbac_policy (id, name, effect, actions, resource_types, subjects, \
             resources, constraints, enabled) \
             VALUES ($1, $2, 'deny', $3, $4, $5, $6, {constraints}, true)"
        );
        let mut query = sqlx::query(&sql)
            .bind(Uuid::now_v7())
            .bind(format!("{username}-deny"))
            .bind(json!(["READ"]))
            .bind(json!(["CONNECTION_TARGET"]))
            .bind(&subjects)
            .bind(&deny.resources);
        if let Some(constraints) = &deny.constraints {
            query = query.bind(constraints);
        }
        query.execute(pool).await?;
    }

    login_and_get_cookie(app, &username, TEST_ADMIN_PASSWORD)
        .await
        .ok_or_else(|| boxed("fresh caller login failed"))
}

/// Launch the seeded fan-out as a fresh caller, optionally carrying one DENY leg.
async fn launch_fan_out(
    app: &Router,
    pool: &PgPool,
    fixtures: &BatchAuthorizationFixtures,
    deny: Option<DenyLeg>,
) -> Result<(StatusCode, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let cookie = new_caller_cookie(app, pool, deny).await?;
    let body = json!({
        "target_set_id": fixtures.target_set_id,
        "target_limit": BATCH_AUTHZ_TARGET_LIMIT,
    })
    .to_string();
    request(
        app,
        "POST",
        &format!("/api/v1/playbooks/{}/runs", fixtures.playbook_id),
        Some(&body),
        Some(&cookie),
    )
    .await
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis (make test / dev stack)"]
async fn bulk_authorization_decides_each_target_over_the_batched_path()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.map_err(boxed)?;
    let pool = state.db_pool.clone();
    let app = piping_backend::app::build_router(state);
    let suffix = unique_suffix();
    let fixtures = seed_batch_authorization_fixtures(&pool, &suffix).await?;

    // Positive control: the same fan-out, same fixtures, no DENY leg at all is
    // authorized and queued. Every later 403 below is therefore caused by that
    // case's connection-target DENY (the connection-target resource type is the
    // only difference), and the created runs are the authorized fan-out.
    let (status, body) = launch_fan_out(&app, &pool, &fixtures, None).await?;
    let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "all-allowed fan-out must be authorized: {payload}"
    );
    assert_ordered_fan_out(&fixtures, &payload)?;

    let fixture_tenant = fixtures.virtual_machine_tenant_id.to_string();
    let first_site = fixtures.first_site_id.to_string();
    let second_region = fixtures.second_region_id.to_string();
    let third_site = fixtures.third_site_id.to_string();
    let unused_tenant = fixtures.unused_tenant_id.to_string();

    let cases: Vec<(&str, DenyLeg, BatchDecision)> = vec![
        (
            "the virtual machine's own tenant",
            DenyLeg::attributes(json!({ "tenant_id": fixture_tenant })),
            BatchDecision::Denied,
        ),
        (
            "the fourth member's own site",
            DenyLeg::attributes(json!({ "site_id": third_site })),
            BatchDecision::Denied,
        ),
        (
            "the third member's region, resolved through its site",
            DenyLeg::attributes(json!({ "region_id": second_region })),
            BatchDecision::Denied,
        ),
        (
            "the third member's own status",
            DenyLeg::attributes(json!({ "status": SECOND_DEVICE_STATUS })),
            BatchDecision::Denied,
        ),
        (
            "the first member by canonical resource id",
            DenyLeg::resources(vec![format!(
                "connection_target:{}",
                fixtures.first_device_id
            )]),
            BatchDecision::Denied,
        ),
        (
            "a facet value no member carries",
            DenyLeg::attributes(json!({ "tenant_id": unused_tenant })),
            BatchDecision::Allowed,
        ),
        (
            "the site the first member (but not the second or third) sits on",
            DenyLeg::attributes(json!({ "site_id": first_site })),
            BatchDecision::Denied,
        ),
    ];

    for (label, deny, expected) in cases {
        let (status, body) = launch_fan_out(&app, &pool, &fixtures, Some(deny)).await?;
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        match expected {
            BatchDecision::Denied => {
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "denying {label} must deny the fan-out: {payload}"
                );
                assert_eq!(payload["code"], json!("403"), "denying {label}: {payload}");
                assert_eq!(
                    payload["message"],
                    json!("Access denied"),
                    "denying {label} must be refused by the authorization gate: {payload}"
                );
            }
            BatchDecision::Allowed => {
                assert_eq!(
                    status,
                    StatusCode::ACCEPTED,
                    "denying {label} must leave every target allowed: {payload}"
                );
                assert_ordered_fan_out(&fixtures, &payload)?;
            }
        }
    }

    // Fixtures stay in the shared dev database (as the neighbouring suites'
    // seeds do); only the deny legs would leak authority, and they are scoped to
    // the throwaway callers created above, so no cleanup can strand a deny.
    Ok(())
}

fn assert_ordered_fan_out(
    fixtures: &BatchAuthorizationFixtures,
    payload: &Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let created: Vec<String> = payload["runs"]
        .as_array()
        .ok_or_else(|| boxed("created run group without runs"))?
        .iter()
        .filter_map(|run| run["target_id"].as_str().map(str::to_string))
        .collect();
    let expected: Vec<String> = fixtures
        .ordered_target_ids
        .iter()
        .map(Uuid::to_string)
        .collect();
    assert_eq!(
        created, expected,
        "one run per authorized target, in preview order, and only those targets"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis (make test / dev stack)"]
async fn batched_facet_resolver_reports_each_targets_own_facet_set()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = build_test_app_state().await.map_err(boxed)?;
    let pool = state.db_pool.clone();
    let suffix = unique_suffix();
    let prefix = format!("batch-authz-resolver-{suffix}");

    let (device_id, _) = seed_device(&pool, &prefix).await?;
    let site_id = device_site(&pool, device_id).await?;
    let region_id = insert_region(&pool, &format!("batch-authz-resolver-region-{suffix}")).await?;
    sqlx::query("UPDATE dcim_site SET region_id = $2 WHERE id = $1")
        .bind(site_id)
        .bind(region_id)
        .execute(&pool)
        .await?;
    let device_tenant_id =
        insert_tenant(&pool, &format!("batch-authz-resolver-dev-{suffix}")).await?;
    sqlx::query("UPDATE dcim_device SET tenant_id = $2, status = $3 WHERE id = $1")
        .bind(device_id)
        .bind(device_tenant_id)
        .bind(SECOND_DEVICE_STATUS)
        .execute(&pool)
        .await?;

    let device_role_id: Uuid = sqlx::query_scalar("SELECT role_id FROM dcim_device WHERE id = $1")
        .bind(device_id)
        .fetch_one(&pool)
        .await?;
    let virtual_machine_tenant_id =
        insert_tenant(&pool, &format!("batch-authz-resolver-vm-{suffix}")).await?;
    let virtual_machine_id = Uuid::now_v7();
    insert_virtual_machine(
        &pool,
        virtual_machine_id,
        &format!("{prefix}-vm"),
        site_id,
        device_role_id,
        Some(virtual_machine_tenant_id),
    )
    .await?;
    // Same UUID in both target tables: the resolver documents that the device row
    // wins, and the VM facet must not overwrite it.
    let shadowed_vm_tenant_id =
        insert_tenant(&pool, &format!("batch-authz-resolver-shadow-{suffix}")).await?;
    insert_virtual_machine(
        &pool,
        device_id,
        &format!("{prefix}-shadow-vm"),
        site_id,
        device_role_id,
        Some(shadowed_vm_tenant_id),
    )
    .await?;
    let unknown_target_id = Uuid::now_v7();

    let facets = resolve_connection_target_authorization_facets(
        &pool,
        &[device_id, virtual_machine_id, unknown_target_id],
    )
    .await?;

    assert_eq!(
        facets.len(),
        2,
        "known targets are reported and unknown ids stay absent so callers fail closed"
    );
    let device = facets
        .get(&device_id)
        .ok_or_else(|| boxed("device facets missing"))?;
    assert_eq!(
        device.tenant_id,
        Some(device_tenant_id),
        "the device row wins over the VM row sharing its UUID"
    );

    let virtual_machine = facets
        .get(&virtual_machine_id)
        .ok_or_else(|| boxed("virtual machine facets missing"))?;
    assert_eq!(
        virtual_machine.tenant_id,
        Some(virtual_machine_tenant_id),
        "a VM target keeps its own facets rather than the device's"
    );
    assert_eq!(virtual_machine.site_id, Some(site_id));

    let empty: Vec<Uuid> = Vec::new();
    assert!(
        resolve_connection_target_authorization_facets(&pool, &empty)
            .await?
            .is_empty(),
        "an empty fan-out resolves nothing"
    );
    let expected = ConnectionTargetAuthorizationFacets {
        role_id: Some(device_role_id),
        site_id: Some(site_id),
        region_id: Some(region_id),
        tenant_id: Some(device_tenant_id),
        platform_id: None,
        cluster_id: None,
        status: Some(SECOND_DEVICE_STATUS.to_string()),
    };
    assert_eq!(
        *device, expected,
        "one resolver pass must report this target's own row facets"
    );
    Ok(())
}
