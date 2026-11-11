//! PIP-211: a run-bound playbook job token reaches the firewall-policy endpoint
//! only for the concrete host its run is bound to, and only under the run
//! group's live initiating actor's permissions.
//!
//! The token itself carries no user identity and no permission of its own: the
//! `inventory:read` token ceiling narrows the caller, it never grants. Every
//! case below drives the real REST endpoint with a real `pjt_` bearer token.
mod common;
#[path = "common/isolated_app.rs"]
mod isolated_app;
#[path = "common/project_workspace.rs"]
mod project_workspace;

use axum::http::StatusCode;
use piping_backend::app::AppState;
use piping_backend::contexts::access_control::domain::{Action, ResourceType};
use piping_backend::contexts::playbooks::domain::{InventorySelection, ResolvedTarget};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const POLICY: &str = "/api/v1/inventory/dataflows/firewall-policy";
const VM_TYPES: &str = "/api/v1/inventory/virtual-machine-types";

struct Actor {
    id: Uuid,
    username: String,
    cookie: String,
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn job_token_firewall_policy_is_bound_to_its_host_and_live_actor() -> TestResult {
    // The resolver rejects duplicate ownership; prior fixtures must not share IPs.
    let harness = isolated_app::harness("pip211_job_policy", 6).await?;
    let state = harness.state.clone();
    let pool: PgPool = state.db_pool.clone();
    let app = harness.app.clone();
    let admin = harness.cookie.clone();

    let prefix = format!("pip211-job-policy-{}", common::unique_suffix());
    let (host, _) = common::seed_device(&pool, &prefix).await?;
    let (peer, _) = common::seed_device(&pool, &prefix).await?;
    let (site, role): (Uuid, Uuid) =
        sqlx::query_as("SELECT site_id, role_id FROM dcim_device WHERE id = $1")
            .bind(host)
            .fetch_one(&pool)
            .await?;
    let vm = Uuid::now_v7();
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot,site_id,role_id) VALUES ($1,'{}',$2,'active','','',0,0,'','',$3,$4)")
        .bind(vm)
        .bind(format!("{prefix}-vm"))
        .bind(site)
        .bind(role)
        .execute(&pool)
        .await?;

    // A real contributing peer: the host and the peer share an address family so
    // the policy snapshot requires both, and a peer the actor cannot see must
    // suppress the whole answer.
    let host_ip = ip(&pool, "198.51.100.10/24").await?;
    let peer_ip = ip(&pool, "198.51.100.20/24").await?;
    assign(&pool, host, &[host_ip]).await?;
    assign(&pool, peer, &[peer_ip]).await?;
    let (status, body) = common::request(
        &app,
        "POST",
        "/api/v1/inventory/dataflows",
        Some(
            &json!({
                "name": format!("{prefix}-flow"),
                "status": "active",
                "sources": [{"object_type": "dcim.device", "object_id": host, "address_mode": "all"}],
                "destinations": [{"object_type": "dcim.device", "object_id": peer, "address_mode": "all"}],
            })
            .to_string(),
        ),
        Some(&admin),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "active flow: {}",
        String::from_utf8_lossy(&body)
    );

    let allowed = seed_actor(&app, &pool, &prefix, "allowed", true, true, None).await?;
    let peer_denied =
        seed_actor(&app, &pool, &prefix, "peer-denied", true, true, Some(peer)).await?;
    let inventory_only =
        seed_actor(&app, &pool, &prefix, "inventory-only", true, false, None).await?;
    let target_only = seed_actor(&app, &pool, &prefix, "target-only", false, true, None).await?;

    // --- The bound device: allowed for a live, permitted initiator. ---------
    let device_run = seed_run(&pool, "device", host, Some(allowed.id), None).await?;
    let device_token = job_token(&state, device_run, &["inventory:read"]).await?;
    let (status, body) = policy(&app, Some(&device_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::OK, "bound device policy");
    assert_eq!(body["host"]["object_id"], json!(host));
    assert!(
        body["rules"]
            .as_array()
            .is_some_and(|rules| !rules.is_empty()),
        "the bound host keeps its real policy: {body}"
    );

    // --- The bound virtual machine: the same token shape, another kind. -----
    let vm_run = seed_run(&pool, "vm", vm, Some(allowed.id), None).await?;
    let vm_token = job_token(&state, vm_run, &["inventory:read"]).await?;
    let (status, body) = policy(&app, Some(&vm_token), "virtualization.virtualmachine", vm).await?;
    assert_eq!(status, StatusCode::OK, "bound virtual machine policy");
    assert_eq!(body["host"]["object_id"], json!(vm));

    // --- A device/VM id collision is ambiguous, so the token fails closed. --
    // Host UUIDs are unique per table; the token carries no device/VM kind.
    let (collision_host, _) = common::seed_device(&pool, &prefix).await?;
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot,site_id,role_id) VALUES ($1,'{}',$2,'active','','',0,0,'','',$3,$4)")
        .bind(collision_host)
        .bind(format!("{prefix}-collision-vm"))
        .bind(site)
        .bind(role)
        .execute(&pool)
        .await?;
    let collision_run =
        seed_run(&pool, "collision", collision_host, Some(allowed.id), None).await?;
    let collision_token = job_token(&state, collision_run, &["inventory:read"]).await?;
    for host_type in ["dcim.device", "virtualization.virtualmachine"] {
        let (status, _) = policy(&app, Some(&collision_token), host_type, collision_host).await?;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "an ambiguous device/VM id must not yield a policy for {host_type}"
        );
    }

    // --- Another host, another kind of the bound id, and a non-host type. ---
    let (status, _) = policy(&app, Some(&device_token), "dcim.device", peer).await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "unbound host");
    let (status, _) = policy(
        &app,
        Some(&device_token),
        "virtualization.virtualmachine",
        host,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "kind swapped on the bound id"
    );
    let (status, _) = policy(&app, Some(&device_token), "dcim.site", site).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "non-host endpoint type");

    // Native-inventory runs may have no managed host binding. Removing a
    // previously bound target must invalidate its token rather than grant broadly.
    let unbound_run = seed_run(&pool, "unbound", host, Some(allowed.id), None).await?;
    let unbound_token = job_token(&state, unbound_run, &["inventory:read"]).await?;
    sqlx::query("UPDATE playbook_run SET target_kind = NULL, target_id = NULL WHERE id = $1")
        .bind(unbound_run)
        .execute(&pool)
        .await?;
    let (status, _) = policy(&app, Some(&unbound_token), "dcim.device", host).await?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "run lost its managed-host binding"
    );

    // --- The actor's own live permissions decide, not the token ceiling. ----
    let inventory_only_run =
        seed_run(&pool, "inventory", host, Some(inventory_only.id), None).await?;
    let inventory_only_token = job_token(&state, inventory_only_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&inventory_only_token), "dcim.device", host).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an inventory-only initiator cannot read connection-target policy"
    );

    let target_only_run = seed_run(&pool, "target", host, Some(target_only.id), None).await?;
    let target_only_token = job_token(&state, target_only_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&target_only_token), "dcim.device", host).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "connection-target visibility does not replace the endpoint's inventory read"
    );

    let peer_denied_run = seed_run(&pool, "peer", host, Some(peer_denied.id), None).await?;
    let peer_denied_token = job_token(&state, peer_denied_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&peer_denied_token), "dcim.device", host).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a peer outside the actor's visibility suppresses the policy"
    );

    // --- Request-scoped conditions apply to the job's live actor. -----------
    // The token's no-identity rule never means "no IP/time context": both the
    // job-bound decision and the actor's own interactive request evaluate the
    // constraint against THIS request.
    let ip_denied = seed_actor(&app, &pool, &prefix, "ip-denied", true, true, None).await?;
    seed_constrained_policy(
        &pool,
        &prefix,
        "ip-denied",
        ip_denied.id,
        "deny",
        json!({"ip_range": "198.51.100.0/24"}),
    )
    .await?;
    let ip_granted = seed_actor(&app, &pool, &prefix, "ip-granted", true, false, None).await?;
    seed_constrained_policy(
        &pool,
        &prefix,
        "ip-granted",
        ip_granted.id,
        "allow",
        json!({"ip_range": "198.51.100.0/24"}),
    )
    .await?;
    let window_granted =
        seed_actor(&app, &pool, &prefix, "window-granted", true, false, None).await?;
    seed_constrained_policy(
        &pool,
        &prefix,
        "window-granted",
        window_granted.id,
        "allow",
        json!({"time_window": {"start": "2020-01-01T00:00:00Z", "end": "2020-01-02T00:00:00Z"}}),
    )
    .await?;
    // The actors logged in above, so their earlier policy resolution is cached;
    // a new policy must invalidate the cached generation like the revocation
    // case above does.
    bump_policy_epoch(&state).await?;

    let ip_denied_run = seed_run(&pool, "ip-denied", host, Some(ip_denied.id), None).await?;
    let ip_denied_token = job_token(&state, ip_denied_run, &["inventory:read"]).await?;
    let (status, _) = policy_with_ip(
        &app,
        None,
        Some(&ip_denied_token),
        "dcim.device",
        host,
        "198.51.100.25",
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an in-range IP deny applies to the job-bound policy"
    );
    let (status, _) = policy_with_ip(
        &app,
        Some(&ip_denied.cookie),
        None,
        "dcim.device",
        host,
        "198.51.100.25",
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the same actor's interactive request sees the same IP deny"
    );
    // Outside its range the same DENY must not fire; otherwise an ignored IP and
    // a dropped DENY would be indistinguishable.
    let (status, _) = policy_with_ip(
        &app,
        None,
        Some(&ip_denied_token),
        "dcim.device",
        host,
        "203.0.113.8",
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "an out-of-range IP deny does not apply"
    );

    let ip_granted_run = seed_run(&pool, "ip-granted", host, Some(ip_granted.id), None).await?;
    let ip_granted_token = job_token(&state, ip_granted_run, &["inventory:read"]).await?;
    let (status, _) = policy_with_ip(
        &app,
        None,
        Some(&ip_granted_token),
        "dcim.device",
        host,
        "203.0.113.8",
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an out-of-range IP allow does not grant"
    );
    let (status, _) = policy_with_ip(
        &app,
        None,
        Some(&ip_granted_token),
        "dcim.device",
        host,
        "198.51.100.25",
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "an in-range IP allow grants");

    let window_granted_run =
        seed_run(&pool, "window-granted", host, Some(window_granted.id), None).await?;
    let window_granted_token = job_token(&state, window_granted_run, &["inventory:read"]).await?;
    let (status, _) = policy_with_ip(
        &app,
        None,
        Some(&window_granted_token),
        "dcim.device",
        host,
        "203.0.113.8",
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a grant outside its time window does not grant"
    );

    // --- Invalid token ceilings stay refused. -------------------------------
    let write_scope_run = seed_run(&pool, "write-scope", host, Some(allowed.id), None).await?;
    let write_scope_token = job_token(&state, write_scope_run, &["inventory:update"]).await?;
    let (status, _) = policy(&app, Some(&write_scope_token), "dcim.device", host).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a token without an inventory read ceiling cannot read the policy"
    );

    // --- No initiator: the run group has nobody to authorize as. ------------
    let no_actor_run = seed_run(&pool, "no-actor", host, None, None).await?;
    let no_actor_token = job_token(&state, no_actor_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&no_actor_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "run without an initiator");

    // --- Disabled and deleted initiators stop authorizing immediately. ------
    let disabled = seed_actor(&app, &pool, &prefix, "disabled", true, true, None).await?;
    let disabled_run = seed_run(&pool, "disabled", host, Some(disabled.id), None).await?;
    let disabled_token = job_token(&state, disabled_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&disabled_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::OK, "active initiator");
    sqlx::query("UPDATE identity_user SET status = 'inactive' WHERE id = $1")
        .bind(disabled.id)
        .execute(&pool)
        .await?;
    let (status, _) = policy(&app, Some(&disabled_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "deactivated initiator");

    let deleted = seed_actor(&app, &pool, &prefix, "deleted", true, true, None).await?;
    let deleted_run = seed_run(&pool, "deleted", host, Some(deleted.id), None).await?;
    let deleted_token = job_token(&state, deleted_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&deleted_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::OK, "present initiator");
    // `playbook_run_group.requested_by_user_id` is ON DELETE SET NULL, so a
    // deleted initiator leaves the run without a re-checkable subject.
    sqlx::query("DELETE FROM identity_user WHERE id = $1")
        .bind(deleted.id)
        .execute(&pool)
        .await?;
    let (status, _) = policy(&app, Some(&deleted_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "deleted initiator");

    // --- Grants removed after issuance are not restored by the token. -------
    let revoked_grants =
        seed_actor(&app, &pool, &prefix, "revoked-grants", true, true, None).await?;
    let revoked_run =
        seed_run(&pool, "revoked-grants", host, Some(revoked_grants.id), None).await?;
    let revoked_token = job_token(&state, revoked_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&revoked_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::OK, "granted initiator");
    sqlx::query("DELETE FROM rbac_policy WHERE name LIKE $1")
        .bind(format!("{}-%", revoked_grants.username))
        .execute(&pool)
        .await?;
    bump_policy_epoch(&state).await?;
    let (status, _) = policy(&app, Some(&revoked_token), "dcim.device", host).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "grants removed after token issuance"
    );

    // Sandbox tokens require a user owner, including for scheduled runs.
    let service_principal = Uuid::now_v7();
    sqlx::query("INSERT INTO playbook_service_principal (id, name, slug) VALUES ($1, $2, $3)")
        .bind(service_principal)
        .bind(format!("{prefix}-principal"))
        .bind(format!("{prefix}-principal"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) VALUES ($1, 'allow', $2, $3, $4, '[]')")
        .bind(format!("{prefix}-principal-allow"))
        .bind(json!([Action::Read]))
        .bind(json!([ResourceType::Inventory, ResourceType::ConnectionTarget]))
        .bind(json!([format!("service_principal:{service_principal}")]))
        .execute(&pool)
        .await?;
    let scheduled_run = seed_run(&pool, "scheduled", host, None, Some(service_principal)).await?;
    let scheduled_token = job_token(&state, scheduled_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&scheduled_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "service principal has no user owner");
    sqlx::query(
        "UPDATE playbook_service_principal SET status = 'revoked', revoked_at = now() WHERE id = $1",
    )
    .bind(service_principal)
    .execute(&pool)
    .await?;
    let (status, _) = policy(&app, Some(&scheduled_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked service principal");

    // --- Revoked, expired tokens and finished runs fail authentication. -----
    let revoked_token_run = seed_run(&pool, "revoked-token", host, Some(allowed.id), None).await?;
    let revoked = job_token(&state, revoked_token_run, &["inventory:read"]).await?;
    let (status, _) = policy(&app, Some(&revoked), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::OK, "token before revocation");
    sqlx::query("UPDATE playbook_job_token SET status = 'revoked' WHERE run_id = $1")
        .bind(revoked_token_run)
        .execute(&pool)
        .await?;
    let (status, _) = policy(&app, Some(&revoked), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked job token");

    let expired_token_run = seed_run(&pool, "expired-token", host, Some(allowed.id), None).await?;
    let expired = job_token(&state, expired_token_run, &["inventory:read"]).await?;
    sqlx::query(
        "UPDATE playbook_job_token SET issued_at = now() - interval '2 hours', expires_at = now() - interval '1 hour' WHERE run_id = $1",
    )
    .bind(expired_token_run)
    .execute(&pool)
    .await?;
    let (status, _) = policy(&app, Some(&expired), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "expired job token");

    let finished_run = seed_run(&pool, "finished", host, Some(allowed.id), None).await?;
    let finished_token = job_token(&state, finished_run, &["inventory:read"]).await?;
    sqlx::query("UPDATE playbook_run SET status = 'succeeded', finished_at = now() WHERE id = $1")
        .bind(finished_run)
        .execute(&pool)
        .await?;
    let (status, _) = policy(&app, Some(&finished_token), "dcim.device", host).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "finished run");

    // --- External session and PAT behavior is unchanged. --------------------
    // The same actor's interactive session is not narrowed to a run target.
    let (status, _) = policy_with(&app, Some(&allowed.cookie), None, "dcim.device", peer).await?;
    assert_eq!(status, StatusCode::OK, "external session is unchanged");
    let (status, _) = policy_with(
        &app,
        Some(&inventory_only.cookie),
        None,
        "dcim.device",
        host,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "external session keeps its connection-target visibility gate"
    );
    let (status, token_body) = common::request(
        &app,
        "POST",
        "/api/v1/user/settings/api-tokens",
        Some(
            &json!({
                "name": format!("{prefix}-pat"),
                "scopes": [{"action": "read", "resource_type": "inventory", "resource_pattern": "*"}],
            })
            .to_string(),
        ),
        Some(&admin),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "admin PAT creation must succeed"
    );
    let token_body: Value = serde_json::from_slice(&token_body)?;
    let pat = token_body["token_value"]
        .as_str()
        .ok_or("PAT token value")?
        .to_string();
    // A personal API token carries no job binding, so it is not narrowed to one
    // host: it reads both the host and the peer under its user's policies.
    for target in [host, peer] {
        let (status, _) = policy_with(&app, None, Some(&pat), "dcim.device", target).await?;
        assert_eq!(status, StatusCode::OK, "external PAT on {target}");
    }

    drop(app);
    drop(state);
    drop(pool);
    isolated_app::shutdown(harness).await
}

/// PIP-533: a job token is minted for exactly one surface. The supervisor
/// callback token is refused on inventory REST, and the sandbox inventory token
/// is refused on the run callback surface; the audience is the discriminator,
/// not the endpoint shape alone.
#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn job_token_audience_separates_supervisor_and_sandbox_surfaces() -> TestResult {
    let harness = isolated_app::harness("pip533_job_audience", 8).await?;
    let state = harness.state.clone();
    let pool: PgPool = state.db_pool.clone();
    let app = harness.app.clone();

    let prefix = format!("pip533-audience-{}", common::unique_suffix());
    let (host, _) = common::seed_device(&pool, &prefix).await?;
    let owner = seed_actor(&app, &pool, &prefix, "owner", true, true, None).await?;
    let run = seed_run(&pool, "audience", host, Some(owner.id), None).await?;

    let sandbox = job_token_with_audience(&state, run, "sandbox", &["inventory:read", "artifact:create"]).await?;
    let (status, _) = common::request_with_auth(
        &app,
        "GET",
        VM_TYPES,
        None,
        None,
        Some(&sandbox),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "sandbox token is accepted on inventory REST"
    );

    let supervisor =
        job_token_with_audience(&state, run, "supervisor", &["inventory:read"]).await?;
    let (status, _) = common::request_with_auth(
        &app,
        "GET",
        VM_TYPES,
        None,
        None,
        Some(&supervisor),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "supervisor token is refused on inventory REST"
    );

    let artifact =
        json!({"artifact_type": "log", "name": "audience-probe", "content_text": "probe"})
            .to_string();
    let callback_uri = format!("/api/v1/playbooks/runs/{run}/artifacts");
    let (status, _) = common::request_with_auth(
        &app,
        "POST",
        &callback_uri,
        Some(&artifact),
        None,
        Some(&sandbox),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "sandbox token is refused on a supervisor callback"
    );

    // The same callback succeeds only with a supervisor-audience token, so the
    // refusal above is the audience split rather than the endpoint rejecting
    // every job token.
    let supervisor_artifact =
        job_token_with_audience(&state, run, "supervisor", &["artifact:create"]).await?;
    let (status, body) = common::request_with_auth(
        &app,
        "POST",
        &callback_uri,
        Some(&artifact),
        None,
        Some(&supervisor_artifact),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "supervisor token is accepted on its own callback surface: {}",
        String::from_utf8_lossy(&body)
    );

    drop(app);
    drop(state);
    drop(pool);
    isolated_app::shutdown(harness).await
}

async fn policy(
    app: &axum::Router,
    bearer: Option<&str>,
    host_type: &str,
    host_id: Uuid,
) -> TestResult<(StatusCode, Value)> {
    policy_with(app, None, bearer, host_type, host_id).await
}

async fn policy_with(
    app: &axum::Router,
    cookie: Option<&str>,
    bearer: Option<&str>,
    host_type: &str,
    host_id: Uuid,
) -> TestResult<(StatusCode, Value)> {
    let uri = format!("{POLICY}?host_type={host_type}&host_id={host_id}");
    let (status, body) = common::request_with_auth(app, "GET", &uri, None, cookie, bearer).await?;
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    };
    Ok((status, value))
}

/// Like `policy_with`, but sends an explicit client IP so an `ip_range`
/// constraint evaluates against a controlled request scope.
async fn policy_with_ip(
    app: &axum::Router,
    cookie: Option<&str>,
    bearer: Option<&str>,
    host_type: &str,
    host_id: Uuid,
    request_ip: &str,
) -> TestResult<(StatusCode, Value)> {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let uri = format!("{POLICY}?host_type={host_type}&host_id={host_id}");
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-forwarded-for", request_ip);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if let Some(bearer) = bearer {
        builder = builder.header("authorization", format!("Bearer {bearer}"));
    }
    let request = builder
        .body(Body::empty())
        .map_err(|error| format!("build policy request: {error}"))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body[..])?
    };
    Ok((status, value))
}

async fn job_token(state: &AppState, run_id: Uuid, scopes: &[&str]) -> TestResult<String> {
    job_token_with_audience(state, run_id, "sandbox", scopes).await
}

async fn job_token_with_audience(
    state: &AppState,
    run_id: Uuid,
    audience: &str,
    scopes: &[&str],
) -> TestResult<String> {
    // Seed the persisted token directly so `issued_at` and `expires_at` share the
    // database clock; the token shape and hash are the canonical ones the
    // middleware validates (`pjt_<uuid>_<secret>`, sha256 hex).
    let token_id = Uuid::now_v7();
    let secret = Uuid::now_v7().simple().to_string();
    let raw_token = format!("pjt_{token_id}_{secret}");
    let token_prefix = format!("pjt_{}", &secret[..8]);
    let scope_values = serde_json::json!(scopes);
    sqlx::query(
        "INSERT INTO playbook_job_token (
             id, run_id, target_kind, target_id, source_kind, source_id,
             token_hash, token_prefix, scopes, issued_at, expires_at, metadata
         )
         SELECT $1, run.id, run.target_kind, run.target_id, run.source_kind, run.source_id,
                encode(sha256($2::bytea), 'hex'), $3, $4, now(), now() + interval '1 hour',
                jsonb_build_object('audience', $6, 'owner_user_id', run_group.requested_by_user_id)
         FROM playbook_run AS run
         JOIN playbook_run_group AS run_group ON run_group.id = run.run_group_id
         WHERE run.id = $5",
    )
    .bind(token_id)
    .bind(&raw_token)
    .bind(&token_prefix)
    .bind(&scope_values)
    .bind(run_id)
    .bind(audience)
    .execute(&state.db_pool)
    .await?;
    Ok(raw_token)
}

async fn seed_run(
    pool: &PgPool,
    label: &str,
    target: Uuid,
    requested_by: Option<Uuid>,
    service_principal: Option<Uuid>,
) -> TestResult<Uuid> {
    let workspace = project_workspace::seed_workspace(
        pool,
        InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
    )
    .await?;
    let group = Uuid::now_v7();
    let run = Uuid::now_v7();
    let source = Uuid::now_v7();
    let hostname = format!("policy-{label}-{target}");
    let launch = workspace.launch_spec(vec![ResolvedTarget {
        hostname: hostname.clone(),
        target_id: Some(target),
        source_id: Some(source),
    }]);
    let scheduled_for = service_principal.map(|_| chrono::Utc::now());
    // A service-principal run group is a real scheduled occurrence, so the
    // schedule binding constraint holds rather than being bypassed.
    let schedule = match service_principal {
        Some(_service_principal) => {
            let schedule = Uuid::now_v7();
            let delegated_by: Uuid =
                sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
                    .bind(common::TEST_ADMIN_USERNAME)
                    .fetch_one(pool)
                    .await?;
            sqlx::query("INSERT INTO playbook_schedule (id, template_id, targets, owner_user_id, name, cron_expression, enabled) VALUES ($1, $2, $3, $4, $5, '0 * * * * *', false)")
                .bind(schedule)
                .bind(workspace.template.id)
                .bind(serde_json::to_value(&workspace.template.inventory)?)
                .bind(delegated_by)
                .bind(format!("PIP-211 job policy {label} schedule"))
                .execute(pool)
                .await?;
            sqlx::query("INSERT INTO playbook_schedule_occurrence(schedule_id,scheduled_for,fired_at,created_at,updated_at) VALUES ($1,$2,$2,now(),now())")
                .bind(schedule)
                .bind(scheduled_for)
                .execute(pool)
                .await?;
            Some(schedule)
        }
        None => None,
    };
    let trigger_kind = if schedule.is_some() {
        "schedule"
    } else {
        "manual"
    };
    sqlx::query("INSERT INTO playbook_run_group(id,template_id,launch_spec,requested_by_user_id,service_principal_id,trigger_kind,schedule_id,scheduled_for,status,total_runs) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'running',1)")
        .bind(group)
        .bind(workspace.template.id)
        .bind(serde_json::to_value(&launch)?)
        .bind(requested_by)
        .bind(service_principal)
        .bind(trigger_kind)
        .bind(schedule)
        .bind(scheduled_for)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO playbook_run(id,run_group_id,hostname,target_kind,target_id,source_kind,source_id,status,started_at) VALUES ($1,$2,$3,'connection_target',$4,'device',$5,'running',now())")
        .bind(run)
        .bind(group)
        .bind(hostname)
        .bind(target)
        .bind(source)
        .execute(pool)
        .await?;
    Ok(run)
}

async fn seed_actor(
    app: &axum::Router,
    pool: &PgPool,
    prefix: &str,
    tag: &str,
    inventory: bool,
    connection_target: bool,
    denied_peer: Option<Uuid>,
) -> TestResult<Actor> {
    let id = Uuid::now_v7();
    let username = format!("{prefix}-{tag}-{}", common::unique_suffix());
    sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1, $2, $3, 'active', 'local')")
        .bind(id)
        .bind(&username)
        .bind(format!("{username}@test.local"))
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at) SELECT $1, c.password_hash, now() FROM auth_user_credential c JOIN identity_user u ON u.id = c.user_id WHERE u.username = $2")
        .bind(id)
        .bind(common::TEST_ADMIN_USERNAME)
        .execute(pool)
        .await?;
    let subjects = json!([format!("user:{id}")]);
    let mut resources = Vec::new();
    if inventory {
        resources.push(ResourceType::Inventory);
    }
    if connection_target {
        resources.push(ResourceType::ConnectionTarget);
    }
    if !resources.is_empty() {
        sqlx::query("INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) VALUES ($1, 'allow', $2, $3, $4, '[]')")
            .bind(format!("{username}-allow"))
            .bind(json!([Action::Read]))
            .bind(json!(resources))
            .bind(&subjects)
            .execute(pool)
            .await?;
    }
    if let Some(peer) = denied_peer {
        sqlx::query("INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources) VALUES ($1, 'deny', $2, $3, $4, $5)")
            .bind(format!("{username}-deny"))
            .bind(json!([Action::Read]))
            .bind(json!([ResourceType::ConnectionTarget]))
            .bind(&subjects)
            .bind(json!([format!("connection_target:{peer}")]))
            .execute(pool)
            .await?;
    }
    let cookie = common::login_and_get_cookie(app, &username, common::TEST_ADMIN_PASSWORD)
        .await
        .ok_or("actor login failed")?;
    Ok(Actor {
        id,
        username,
        cookie,
    })
}

/// Add one extra `Read × CONNECTION_TARGET` policy for an already-seeded actor.
/// The constraint is stored verbatim, so the real `ip_range`/`time_window`
/// evaluator runs instead of a hand-rolled shortcut.
async fn seed_constrained_policy(
    pool: &PgPool,
    prefix: &str,
    tag: &str,
    actor_id: Uuid,
    effect: &str,
    constraints: Value,
) -> TestResult {
    sqlx::query(
        "INSERT INTO rbac_policy (name, effect, actions, resource_types, subjects, resources, constraints) VALUES ($1, $2, $3, $4, $5, '[]', $6)",
    )
    .bind(format!("{prefix}-{tag}-constrained"))
    .bind(effect)
    .bind(json!([Action::Read]))
    .bind(json!([ResourceType::ConnectionTarget]))
    .bind(json!([format!("user:{actor_id}")]))
    .bind(constraints)
    .execute(pool)
    .await?;
    Ok(())
}

async fn bump_policy_epoch(state: &AppState) -> TestResult {
    let mut redis = state.redis.clone();
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    Ok(())
}

async fn ip(pool: &PgPool, value: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments) VALUES ($1, '{}', $2::inet, 'active', '', '', '')")
        .bind(id)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(id)
}

async fn assign(pool: &PgPool, host: Uuid, ids: &[Uuid]) -> Result<Uuid, sqlx::Error> {
    let interface = Uuid::now_v7();
    sqlx::query("INSERT INTO dcim_interface (id, custom_field_data, device_id, name, _name, label, description, type, enabled, mgmt_only, mark_connected) VALUES ($1, '{}', $2, $3, $3, '', '', 'virtual', true, false, false)")
        .bind(interface)
        .bind(host)
        .bind(format!("if-{interface}"))
        .execute(pool)
        .await?;
    sqlx::query("UPDATE ipam_ipaddress SET assigned_object_id = $2, assigned_object_type_id = (SELECT id FROM inventory_content_type WHERE app_label = 'dcim' AND model = 'interface') WHERE id = ANY($1)")
        .bind(ids)
        .bind(interface)
        .execute(pool)
        .await?;
    Ok(interface)
}
