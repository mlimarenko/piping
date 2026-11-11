//! Discovery callbacks and service discovery through the real router and database.

use super::{common, isolated_app, project_workspace};
use axum::{Router, http::StatusCode};
use piping_backend::app::AppState;
use piping_backend::contexts::access_control::domain::{Action, ResourceType};
use piping_backend::contexts::monitoring::application::exporter_discovery::DISCOVERY_MAX_RAW_LISTENER_LINES;
use piping_backend::contexts::playbooks::application::{CreateArtifactInput, CreateJobTokenInput};
use piping_backend::contexts::playbooks::domain::{
    InventorySelection, JobTokenAudience, ResolvedTarget,
};
use serde_json::{Value, json};
use uuid::Uuid;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const REPORT: &str = "/api/v1/monitoring/discovery/report";
const SD_TOKEN: &str = "synthetic-pip567-sd-token";

async fn mint(
    state: &AppState,
    run_id: Uuid,
    audience: JobTokenAudience,
    scopes: &[&str],
) -> Result<(Uuid, String)> {
    let created = state
        .services
        .playbooks
        .create_job_token(
            CreateJobTokenInput {
                run_id,
                expires_in_seconds: Some(3600),
                scopes: Some(scopes.iter().map(|scope| (*scope).to_string()).collect()),
                audience,
                metadata: None,
            },
            chrono::Utc::now(),
        )
        .await?;
    Ok((created.token.id, created.raw_token))
}

fn artifact() -> CreateArtifactInput {
    CreateArtifactInput {
        artifact_type: "test-result".into(),
        name: "audience-check".into(),
        content_type: Some("application/json".into()),
        content_json: Some(json!({"ok": true})),
        content_text: None,
    }
}

async fn run(state: &AppState, host: Uuid, name: &str) -> Result<(Uuid, Uuid, Uuid, String)> {
    let pool = &state.db_pool;
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
    let target = host;
    let owner: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username = $1")
        .bind(common::TEST_ADMIN_USERNAME)
        .fetch_one(pool)
        .await?;
    let launch = workspace.launch_spec(vec![ResolvedTarget {
        hostname: name.to_string(),
        target_id: Some(target),
        source_id: Some(host),
    }]);
    sqlx::query("INSERT INTO playbook_run_group(id,template_id,launch_spec,requested_by_user_id,trigger_kind,status,total_runs) VALUES ($1,$2,$3,$4,'manual','running',1)")
        .bind(group).bind(workspace.template.id).bind(serde_json::to_value(launch)?).bind(owner).execute(pool).await?;
    sqlx::query("INSERT INTO playbook_run(id,run_group_id,hostname,target_kind,target_id,source_kind,source_id,status,started_at) VALUES ($1,$2,$3,'connection_target',$4,'virtual_machine',$5,'running',now())")
        .bind(run).bind(group).bind(name).bind(target).bind(host).execute(pool).await?;
    let created = state
        .services
        .playbooks
        .create_job_token(
            CreateJobTokenInput {
                run_id: run,
                expires_in_seconds: Some(3600),
                scopes: Some(vec!["artifact:create".into()]),
                audience: JobTokenAudience::Sandbox,
                metadata: None,
            },
            chrono::Utc::now(),
        )
        .await?;
    Ok((run, group, target, created.raw_token))
}

fn collection(
    run: Uuid,
    group: Uuid,
    target: Uuid,
    host: Uuid,
    name: &str,
    second: u32,
    listeners: &str,
) -> Value {
    json!({
        "schema_version": 1, "run_id": run, "run_group_id": group,
        "connection_target_id": target, "host_id": host, "host_name": name,
        "source_kind": "virtual_machine", "platform": "linux",
        "started_at": format!("2026-11-10T00:00:{second:02}Z"),
        "finished_at": format!("2026-11-10T00:00:{second:02}Z"),
        "sources": {"host_inspection": true, "endpoint_probe": false},
        "coverage": {"listeners":"complete", "services":"complete", "processes":"complete", "containers":"not_installed", "listener_owners":"complete"},
        "raw": {"listeners": listeners, "services":"", "processes":"", "containers":""}
    })
}

async fn post(app: &Router, token: Option<&str>, body: &Value) -> Result<(StatusCode, Value)> {
    let (status, bytes) =
        common::request_with_auth(app, "POST", REPORT, Some(&body.to_string()), None, token)
            .await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

async fn sd(app: &Router) -> Result<Value> {
    let (status, bytes) = common::request_with_auth(
        app,
        "GET",
        "/api/v1/monitoring/sd",
        None,
        None,
        Some(SD_TOKEN),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    Ok(serde_json::from_slice(&bytes)?)
}

async fn assert_owner_sees_vm(app: &Router, cookie: &str, host: Uuid) -> Result {
    let (status, bytes) = common::request(
        app,
        "GET",
        &format!("/api/v1/inventory/virtual_machines/{host}"),
        None,
        Some(cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "VM detail: {}",
        String::from_utf8_lossy(&bytes)
    );
    let vm: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(vm["id"], json!(host));
    Ok(())
}

fn target_count(groups: &Value, target: &str) -> Result<usize> {
    let groups = groups.as_array().ok_or("SD groups array")?;
    Ok(groups
        .iter()
        .filter_map(|group| group["targets"].as_array())
        .flat_map(|targets| targets.iter())
        .filter(|value| value.as_str() == Some(target))
        .count())
}

fn target_has_label(groups: &Value, target: &str, key: &str, expected: &str) -> Result<bool> {
    let groups = groups.as_array().ok_or("SD groups array")?;
    Ok(groups.iter().any(|group| {
        group["targets"]
            .as_array()
            .is_some_and(|targets| targets.iter().any(|value| value.as_str() == Some(target)))
            && group["labels"][key].as_str() == Some(expected)
    }))
}

async fn fresh_report(
    harness: &isolated_app::Harness,
    host: Uuid,
    name: &str,
    second: u32,
    listeners: &str,
    options: Value,
    coverage: Option<&str>,
) -> Result<Value> {
    let (run, group, target, token) = run(&harness.state, host, name).await?;
    let mut body = collection(run, group, target, host, name, second, listeners);
    body["options"] = options;
    if let Some(value) = coverage {
        body["coverage"]["listeners"] = json!(value);
    }
    let (status, response) = post(&harness.app, Some(&token), &body).await?;
    assert_eq!(status, StatusCode::CREATED, "{response}");
    Ok(response)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis"]
async fn vm_discovery_reaches_sd_and_checks_run_binding() -> Result {
    let mut harness = isolated_app::harness("pip567_discovery", 6).await?;
    let mut settings = (*harness.state.settings).clone();
    settings.monitoring.sd_token = Some(SD_TOKEN.to_string());
    settings.monitoring.sd_public_enabled = false;
    harness.state.settings = std::sync::Arc::new(settings);
    harness.app = piping_backend::app::build_router(harness.state.clone());
    let (status, _) =
        common::request(&harness.app, "GET", "/api/v1/monitoring/sd", None, None).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "SD must not be public");
    let pool = &harness.state.db_pool;
    let (device, _) = common::seed_device(pool, "pip567").await?;
    let (site, role): (Uuid, Uuid) =
        sqlx::query_as("SELECT site_id, role_id FROM dcim_device WHERE id=$1")
            .bind(device)
            .fetch_one(pool)
            .await?;
    let host = Uuid::now_v7();
    let name = format!("pip567-{}", host.simple());
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot,site_id,role_id) VALUES ($1,'{}',$2,'active','','',0,0,'','',$3,$4)")
        .bind(host).bind(&name).bind(site).bind(role).execute(pool).await?;
    let address = "192.0.2.10";
    let primary_id = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_ipaddress(id,custom_field_data,address,status,dns_name,description,comments) VALUES ($1,'{}',$2::inet,'active','','','')")
        .bind(primary_id).bind(format!("{address}/32")).execute(pool).await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET primary_ip4_id=$2 WHERE id=$1")
        .bind(host)
        .bind(primary_id)
        .execute(pool)
        .await?;
    // The migrated built-in global node assignment covers 9100 on every host.
    // Disable only this known seed in the disposable test database.
    let disabled_seed = sqlx::query("UPDATE monitoring_profile_assignment SET enabled=false WHERE id=$1 AND scope_type='global' AND enabled")
        .bind(Uuid::parse_str("019ee0e0-69fb-795f-9bc3-4f26240efac1")?)
        .execute(pool).await?;
    assert_eq!(
        disabled_seed.rows_affected(),
        1,
        "migrated default node assignment"
    );
    let exporter: Uuid =
        sqlx::query_scalar("SELECT id FROM monitoring_exporter WHERE exporter_type='node' LIMIT 1")
            .fetch_one(pool)
            .await?;
    let profile: Uuid = sqlx::query_scalar(
        "INSERT INTO monitoring_profile(name,slug) VALUES ('manual','pip567-manual') RETURNING id",
    )
    .fetch_one(pool)
    .await?;
    sqlx::query("INSERT INTO monitoring_profile_item(profile_id,exporter_id,job,scheme,port,metrics_path,labels,enabled,sort_order) VALUES ($1,$2,'manual-job','http',9300,'/metrics','{\"team\":\"manual\"}',true,0)")
        .bind(profile).bind(exporter).execute(pool).await?;
    sqlx::query("INSERT INTO monitoring_profile_assignment(profile_id,scope_type,scope_id,weight,enabled,target) VALUES ($1,'connection_target',$2,0,true,NULL)")
        .bind(profile).bind(host).execute(pool).await?;
    let (run_id, group, target, token) = run(&harness.state, host, &name).await?;
    let listener =
        r#"tcp LISTEN 0 4096 0.0.0.0:9100 0.0.0.0:* users:(("node_exporter",pid=1,fd=3))"#;
    let body = collection(run_id, group, target, host, &name, 1, listener);
    assert_eq!(
        post(&harness.app, None, &body).await?.0,
        StatusCode::UNAUTHORIZED
    );
    for field in ["run_id", "run_group_id", "connection_target_id", "host_id"] {
        let mut foreign = body.clone();
        foreign[field] = json!(Uuid::now_v7());
        assert_eq!(
            post(&harness.app, Some(&token), &foreign).await?.0,
            StatusCode::FORBIDDEN,
            "foreign {field}"
        );
    }
    let (_, supervisor) = mint(
        &harness.state,
        run_id,
        JobTokenAudience::Supervisor,
        &["artifact:create"],
    )
    .await?;
    assert_eq!(
        post(&harness.app, Some(&supervisor), &body).await?.0,
        StatusCode::UNAUTHORIZED,
        "supervisor cannot submit discovery"
    );
    let (_, sandbox_callbacks) = mint(
        &harness.state,
        run_id,
        JobTokenAudience::Sandbox,
        &["artifact:create", "event:append", "status:update"],
    )
    .await?;
    let artifact_payload = json!({"artifact_type":"test-result", "name":"audience-check", "content_type":"application/json", "content_json":{"ok":true}}).to_string();
    let event_payload = json!({"event_kind":"system", "event_type":"audience-check"}).to_string();
    let completion_payload =
        json!({"lease_owner":"fixture", "status":"succeeded", "exit_code":0}).to_string();
    for (suffix, payload) in [
        ("artifacts", &artifact_payload),
        ("events", &event_payload),
        ("completion", &completion_payload),
    ] {
        let uri = format!("/api/v1/playbooks/runs/{run_id}/{suffix}");
        let (status, _) = common::request_with_auth(
            &harness.app,
            "POST",
            &uri,
            Some(payload),
            None,
            Some(&sandbox_callbacks),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "sandbox cannot call {suffix}"
        );
    }
    let uri = format!("/api/v1/playbooks/runs/{run_id}/artifacts");
    let (status, response) = common::request_with_auth(
        &harness.app,
        "POST",
        &uri,
        Some(&artifact_payload),
        None,
        Some(&supervisor),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "supervisor artifact: {}",
        String::from_utf8_lossy(&response)
    );
    let (status, result) = post(&harness.app, Some(&token), &body).await?;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    assert_eq!(result["reconcile"]["created"], 1, "{result}");
    assert!(
        result["report"]["endpoints"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item["endpoint"]["address"] == address
                    && item["endpoint"]["port"] == 9100
                    && item["status"] == "confirmed_discovered"
                    && item["exporter_type"] == "node")),
        "expected typed discovered endpoint: {result}"
    );
    let report_id = Uuid::parse_str(result["artifact_id"].as_str().ok_or("report artifact id")?)?;
    let artifact_run: Uuid = sqlx::query_scalar("SELECT run_id FROM playbook_artifact WHERE id=$1 AND artifact_type='exporter-discovery-report'")
        .bind(report_id).fetch_one(pool).await?;
    assert_eq!(artifact_run, run_id);
    let first = sd(&harness.app).await?;
    assert!(
        target_count(&first, &format!("{address}:9100"))? == 1
            && target_has_label(
                &first,
                &format!("{address}:9100"),
                "piping_host_name",
                &name
            )?,
        "{first}"
    );
    let (status, replay_report) = post(&harness.app, Some(&token), &body).await?;
    assert_eq!(status, StatusCode::CREATED, "{replay_report}");
    let replay = sd(&harness.app).await?;
    assert_eq!(
        target_count(&replay, &format!("{address}:9100"))?,
        1,
        "SD={replay} report={replay_report}"
    );
    let changed_listener = listener.replace(":9100", ":9200");
    let changed_report =
        fresh_report(&harness, host, &name, 3, &changed_listener, json!({}), None).await?;
    let changed_sd = sd(&harness.app).await?.to_string();
    assert!(
        changed_sd.contains(&format!("{address}:9200")),
        "SD={changed_sd} report={changed_report}"
    );
    assert!(
        changed_sd.contains(&format!("{address}:9100")),
        "manual absence retains the original target: SD={changed_sd} report={changed_report}"
    );
    let (_, stale) = post(&harness.app, Some(&token), &body).await?;
    assert_eq!(stale["reconcile"]["stale"], true, "{stale}");
    assert!(
        sd(&harness.app)
            .await?
            .to_string()
            .contains(&format!("{address}:9200"))
    );
    let automatic = json!({"absence_policy":"automatic", "absence_confirm_runs":2, "restore_on_reappearance":false});
    let present = &changed_listener;
    let partial = fresh_report(
        &harness,
        host,
        &name,
        4,
        present,
        automatic.clone(),
        Some("partial"),
    )
    .await?;
    assert_eq!(partial["reconcile"]["disabled"], 0, "{partial}");
    let first_absence =
        fresh_report(&harness, host, &name, 5, present, automatic.clone(), None).await?;
    assert_eq!(first_absence["reconcile"]["disabled"], 0, "{first_absence}");
    let error = fresh_report(
        &harness,
        host,
        &name,
        6,
        "unparseable listener",
        automatic.clone(),
        None,
    )
    .await?;
    assert_eq!(error["reconcile"]["disabled"], 0, "{error}");
    let truncated_lines = format!("{present}\n").repeat(DISCOVERY_MAX_RAW_LISTENER_LINES);
    let truncated = fresh_report(
        &harness,
        host,
        &name,
        7,
        &truncated_lines,
        automatic.clone(),
        None,
    )
    .await?;
    assert_eq!(truncated["reconcile"]["disabled"], 0, "{truncated}");
    assert!(
        truncated["report"]["endpoints"]
            .as_array()
            .is_some_and(
                |items| items.iter().any(|item| item["endpoint"]["port"] == 9100
                    && item["status"] == "error"
                    && item["reason"] == "partial_inspection")
            ),
        "truncated listener evidence must report partial inspection: {truncated}"
    );
    let first_absence =
        fresh_report(&harness, host, &name, 8, present, automatic.clone(), None).await?;
    assert_eq!(first_absence["reconcile"]["disabled"], 0, "{first_absence}");
    let second_absence =
        fresh_report(&harness, host, &name, 9, present, automatic.clone(), None).await?;
    assert_eq!(
        second_absence["reconcile"]["disabled"], 1,
        "{second_absence}"
    );
    let after_disable = sd(&harness.app).await?;
    assert!(
        target_count(&after_disable, &format!("{address}:9100"))? == 0,
        "{after_disable}"
    );
    assert!(
        target_has_label(&after_disable, &format!("{address}:9300"), "team", "manual")?
            && target_has_label(
                &after_disable,
                &format!("{address}:9300"),
                "job",
                "manual-job"
            )?,
        "{after_disable}"
    );
    fresh_report(&harness, host, &name, 10, listener, automatic.clone(), None).await?;
    assert!(
        !sd(&harness.app)
            .await?
            .to_string()
            .contains(&format!("{address}:9100"))
    );
    fresh_report(&harness, host, &name, 11, listener, json!({}), None).await?;
    assert!(
        !sd(&harness.app)
            .await?
            .to_string()
            .contains(&format!("{address}:9100"))
    );
    let restore = json!({"absence_policy":"automatic", "absence_confirm_runs":2, "restore_on_reappearance":true});
    fresh_report(&harness, host, &name, 12, listener, restore, None).await?;
    let restored = sd(&harness.app).await?;
    assert!(
        target_count(&restored, &format!("{address}:9100"))? == 1
            && target_has_label(&restored, &format!("{address}:9300"), "team", "manual")?
            && target_has_label(&restored, &format!("{address}:9300"), "job", "manual-job")?,
        "{restored}"
    );

    // A different host has a manual node item on the exact discovered endpoint.
    let manual_host = Uuid::now_v7();
    let manual_name = format!("pip567-manual-{}", manual_host.simple());
    let manual_ip = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_ipaddress(id,custom_field_data,address,status,dns_name,description,comments) VALUES ($1,'{}','192.0.2.11/32'::inet,'active','','','')")
        .bind(manual_ip).execute(pool).await?;
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot,site_id,role_id,primary_ip4_id) VALUES ($1,'{}',$2,'active','','',0,0,'','',$3,$4,$5)")
        .bind(manual_host).bind(&manual_name).bind(site).bind(role).bind(manual_ip).execute(pool).await?;
    let node_exporter: Uuid =
        sqlx::query_scalar("SELECT id FROM monitoring_exporter WHERE exporter_type='node' LIMIT 1")
            .fetch_one(pool)
            .await?;
    let node_profile: Uuid = sqlx::query_scalar("INSERT INTO monitoring_profile(name,slug) VALUES ('manual node','pip567-manual-node') RETURNING id")
        .fetch_one(pool).await?;
    sqlx::query("INSERT INTO monitoring_profile_item(profile_id,exporter_id,job,scheme,port,metrics_path,labels,enabled,sort_order) VALUES ($1,$2,'manual-node','http',9100,'/metrics','{\"team\":\"manual-node\"}',true,0)")
        .bind(node_profile).bind(node_exporter).execute(pool).await?;
    sqlx::query("INSERT INTO monitoring_profile_assignment(profile_id,scope_type,scope_id,weight,enabled,target) VALUES ($1,'connection_target',$2,0,true,NULL)")
        .bind(node_profile).bind(manual_host).execute(pool).await?;
    let manual_policy = json!({"absence_policy":"automatic", "absence_confirm_runs":1, "restore_on_reappearance":true});
    let manual_report = fresh_report(
        &harness,
        manual_host,
        &manual_name,
        1,
        listener,
        manual_policy.clone(),
        None,
    )
    .await?;
    assert_eq!(
        manual_report["reconcile"]["confirmed"], 1,
        "{manual_report}"
    );
    assert_eq!(
        manual_report["reconcile"]["manual_preserved"], 1,
        "{manual_report}"
    );
    assert_eq!(manual_report["reconcile"]["created"], 0, "{manual_report}");
    assert_eq!(manual_report["reconcile"]["updated"], 0, "{manual_report}");
    let absent_manual = fresh_report(
        &harness,
        manual_host,
        &manual_name,
        2,
        "",
        manual_policy,
        None,
    )
    .await?;
    assert_eq!(absent_manual["reconcile"]["absent"], 1, "{absent_manual}");
    assert_eq!(absent_manual["reconcile"]["disabled"], 0, "{absent_manual}");
    let source_profiles: i64 = sqlx::query_scalar("SELECT count(*) FROM monitoring_profile WHERE slug=$1")
        .bind(piping_backend::contexts::monitoring::infrastructure::discovery_store::discovery_profile_slug("virtual_machine", manual_host))
        .fetch_one(pool).await?;
    assert_eq!(
        source_profiles, 0,
        "manual coverage must not create a discovery profile: {manual_report}"
    );
    let manual_sd = sd(&harness.app).await?;
    assert_eq!(
        target_count(&manual_sd, "192.0.2.11:9100")?,
        1,
        "SD={manual_sd} report={manual_report}"
    );
    assert!(
        target_has_label(&manual_sd, "192.0.2.11:9100", "team", "manual-node")?
            && target_has_label(&manual_sd, "192.0.2.11:9100", "job", "manual-node")?,
        "SD={manual_sd} report={manual_report}"
    );

    // The only network destination is this local, controlled HTTP fixture.
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = server.local_addr()?.port();
    let address_id = Uuid::now_v7();
    sqlx::query("INSERT INTO ipam_ipaddress(id,custom_field_data,address,status,dns_name,description,comments) VALUES ($1,'{}','127.0.0.1/32'::inet,'active','','','')")
        .bind(address_id).execute(pool).await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET primary_ip4_id=$2 WHERE id=$1")
        .bind(host)
        .bind(address_id)
        .execute(pool)
        .await?;
    let local_listener = format!(
        "tcp LISTEN 0 4096 0.0.0.0:{port} 0.0.0.0:* users:((\"node_exporter\",pid=1,fd=3))"
    );
    let local_report =
        fresh_report(&harness, host, &name, 13, &local_listener, json!({}), None).await?;
    assert_eq!(local_report["reconcile"]["created"], 1, "{local_report}");
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM monitoring_profile_item i JOIN monitoring_profile p ON p.id=i.profile_id JOIN monitoring_profile_assignment a ON a.profile_id=p.id WHERE p.slug=$1 AND a.scope_type='connection_target' AND a.scope_id=$2 AND a.enabled AND i.enabled AND i.port=$3")
        .bind(piping_backend::contexts::monitoring::infrastructure::discovery_store::discovery_profile_slug("virtual_machine", host))
        .bind(host).bind(i32::from(port)).fetch_one(pool).await?;
    assert_eq!(stored, 1, "local probe target must be persisted");
    assert_eq!(
        target_count(&sd(&harness.app).await?, &format!("127.0.0.1:{port}"))?,
        1
    );
    let (probe_run, probe_group, probe_target, probe_token) =
        run(&harness.state, host, &name).await?;
    let mut probe = collection(
        probe_run,
        probe_group,
        probe_target,
        host,
        &name,
        15,
        &local_listener,
    );
    probe["sources"]["endpoint_probe"] = json!(true);
    probe["sources"]["host_inspection"] = json!(false);
    probe["options"] = json!({"absence_policy":"automatic", "absence_confirm_runs":1, "excluded_ports":[9100,9200,9300], "probe_timeout_ms":1000});
    let mut excluded = probe.clone();
    excluded["options"]["excluded_hosts"] = json!([name]);
    let (status, skipped) = post(&harness.app, Some(&probe_token), &excluded).await?;
    assert_eq!(status, StatusCode::CREATED, "{skipped}");
    assert_eq!(skipped["probe_target_count"], 0, "{skipped}");
    let mut skipped_host = probe.clone();
    skipped_host["skipped"] = json!(true);
    let (status, skipped) = post(&harness.app, Some(&probe_token), &skipped_host).await?;
    assert_eq!(status, StatusCode::CREATED, "{skipped}");
    assert_eq!(skipped["probe_target_count"], 0, "{skipped}");
    let (response, responder) = tokio::join!(
        post(&harness.app, Some(&probe_token), &probe),
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = server.accept().await?;
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await?;
            stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
            Ok::<(), std::io::Error>(())
        })
    );
    responder??;
    let (status, refusal) = response?;
    assert_eq!(status, StatusCode::CREATED, "{refusal}");
    assert!(refusal["report"].to_string().contains("503"), "{refusal}");
    assert_eq!(refusal["reconcile"]["disabled"], 0, "{refusal}");
    let observations = refusal["report"]["endpoints"]
        .as_array()
        .ok_or("report endpoints")?;
    assert!(
        observations
            .iter()
            .any(|item| item["source"] == "host_inspection"
                && item["status"] == "skipped"
                && item["reason"] == "source_disabled"),
        "{refusal}"
    );
    assert!(
        observations
            .iter()
            .any(|item| item["source"] == "endpoint_probe"
                && item["provenance"]["observed_from"] == "backend"
                && item["proof"]["connect"]["http_status"] == 503),
        "{refusal}"
    );
    assert!(
        !observations
            .iter()
            .any(|item| item["status"] == "confirmed_absent"),
        "{refusal}"
    );
    assert!(
        sd(&harness.app)
            .await?
            .to_string()
            .contains(&format!("127.0.0.1:{port}"))
    );
    let exporter_payload = json!({
        "name":"Sandbox denied exporter", "slug":format!("pip567-denied-{host}"),
        "exporter_type":"node", "default_job":"node", "default_scheme":"http",
        "default_port":9100, "default_metrics_path":"/metrics",
        "default_target_source":"host_address", "labels":{}, "enabled":true
    })
    .to_string();
    let (status, _) = common::request_with_auth(
        &harness.app,
        "POST",
        "/api/v1/monitoring/exporters",
        Some(&exporter_payload),
        None,
        Some(&token),
    )
    .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, response) = common::request(
        &harness.app,
        "POST",
        "/api/v1/monitoring/exporters",
        Some(&exporter_payload),
        Some(&harness.cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "valid catalog payload: {}",
        String::from_utf8_lossy(&response)
    );
    let assignment = json!({"profile_id":profile,"scope_type":"global"}).to_string();
    let (status, _) = common::request_with_auth(
        &harness.app,
        "POST",
        "/api/v1/monitoring/profile-assignments",
        Some(&assignment),
        None,
        Some(&token),
    )
    .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, response) = common::request(
        &harness.app,
        "POST",
        "/api/v1/monitoring/profile-assignments",
        Some(&assignment),
        Some(&harness.cookie),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "valid assignment payload: {}",
        String::from_utf8_lossy(&response)
    );

    let owner: Uuid = sqlx::query_scalar("SELECT id FROM identity_user WHERE username=$1")
        .bind(common::TEST_ADMIN_USERNAME)
        .fetch_one(pool)
        .await?;
    let deny_id = Uuid::now_v7();
    sqlx::query("INSERT INTO rbac_policy(id,name,effect,actions,resource_types,subjects,resources,enabled) VALUES ($1,$2,'deny',$3,$4,$5,'[]',true)")
        .bind(deny_id).bind(format!("pip567-monitoring-deny-{host}"))
        .bind(json!([Action::Create])).bind(json!([ResourceType::Monitoring]))
        .bind(json!([format!("user:{owner}")])).execute(pool).await?;
    let mut redis = harness.state.redis.clone();
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    assert_owner_sees_vm(&harness.app, &harness.cookie, host).await?;
    assert_eq!(
        post(&harness.app, Some(&token), &body).await?.0,
        StatusCode::FORBIDDEN,
        "monitoring Create revoked"
    );
    sqlx::query("UPDATE rbac_policy SET actions=$2 WHERE id=$1")
        .bind(deny_id)
        .bind(json!([Action::Update]))
        .execute(pool)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    assert_owner_sees_vm(&harness.app, &harness.cookie, host).await?;
    assert_eq!(
        post(&harness.app, Some(&token), &body).await?.0,
        StatusCode::FORBIDDEN,
        "monitoring Update revoked"
    );
    sqlx::query("DELETE FROM rbac_policy WHERE id=$1")
        .bind(deny_id)
        .execute(pool)
        .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    assert_eq!(
        post(&harness.app, Some(&token), &body).await?.0,
        StatusCode::CREATED,
        "restored owner permissions"
    );
    let (revoked_id, revoked_token) = mint(
        &harness.state,
        run_id,
        JobTokenAudience::Sandbox,
        &["artifact:create"],
    )
    .await?;
    sqlx::query("UPDATE playbook_job_token SET status='revoked', revoked_at=now() WHERE id=$1")
        .bind(revoked_id)
        .execute(pool)
        .await?;
    assert_eq!(
        post(&harness.app, Some(&revoked_token), &body).await?.0,
        StatusCode::UNAUTHORIZED
    );
    let result = harness
        .state
        .services
        .playbooks
        .create_artifact_with_job_token(
            run_id,
            &revoked_token,
            artifact(),
            JobTokenAudience::Sandbox,
            chrono::Utc::now(),
        )
        .await;
    assert!(
        matches!(result, Err(piping_backend::errors::AppError::Auth(_))),
        "transaction must reject revoked token"
    );
    let (mismatched_id, mismatched_token) = mint(
        &harness.state,
        run_id,
        JobTokenAudience::Sandbox,
        &["artifact:create"],
    )
    .await?;
    sqlx::query("UPDATE playbook_job_token SET metadata=jsonb_set(metadata, '{owner_user_id}', to_jsonb($2::text)) WHERE id=$1")
        .bind(mismatched_id).bind(Uuid::now_v7().to_string()).execute(pool).await?;
    let result = harness
        .state
        .services
        .playbooks
        .create_artifact_with_job_token(
            run_id,
            &mismatched_token,
            artifact(),
            JobTokenAudience::Sandbox,
            chrono::Utc::now(),
        )
        .await;
    assert!(
        matches!(result, Err(piping_backend::errors::AppError::Auth(_))),
        "transaction must reject a mismatched owner binding"
    );
    sqlx::query("UPDATE identity_user SET status='inactive' WHERE username=$1")
        .bind(common::TEST_ADMIN_USERNAME)
        .execute(pool)
        .await?;
    assert_eq!(
        post(&harness.app, Some(&token), &body).await?.0,
        StatusCode::UNAUTHORIZED
    );
    let result = harness
        .state
        .services
        .playbooks
        .create_artifact_with_job_token(
            run_id,
            &token,
            artifact(),
            JobTokenAudience::Sandbox,
            chrono::Utc::now(),
        )
        .await;
    assert!(
        matches!(result, Err(piping_backend::errors::AppError::Auth(_))),
        "transaction must reject inactive owner"
    );
    isolated_app::shutdown(harness).await?;
    Ok(())
}
