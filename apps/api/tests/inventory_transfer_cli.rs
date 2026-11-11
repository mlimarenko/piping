//! Exercise the actual transfer entrypoint, without starting the API and
//! without a database, Redis or provider for the pin/ordering boundaries.

use std::process::Command;

use chrono::{TimeDelta, Utc};
use piping_backend::contexts::sync::domain::inventory_export::{
    CaptureEvidence, CaptureOutcome, ConsistencyStatus, EXPORT_FORMAT_VERSION, EdgeCoverage,
    ExportManifest, ObservationStatus, PayloadDescriptor, ProcessArea, ProcessCoverage,
    SourceAttestationIdentity, SourceAuthority, digest,
};
use piping_backend::contexts::sync::domain::inventory_preflight::{
    FindingCode, PreflightProfile, PreflightReport, catalog_digest, tool_digest,
};
use piping_backend::contexts::sync::domain::{
    ConflictMode, MappingConfigDocument, MappingConfigurationStatus, MappingDirection, MappingRef,
    MappingTarget, RetainAbsencePolicy, TRANSFER_FORMAT_VERSION, TransferApproval,
    TransferDescriptor, TransferOperator, TransferScope, TransferTarget,
};
use piping_backend::contexts::sync::infrastructure::providers::immutable_inventory_source::seal_bundle;
use serde_json::{Value, json};
use uuid::Uuid;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_inventory-transfer"));
    command.env_clear();
    command
}

/// A structurally valid descriptor. Every digest is synthetic; the tests only
/// exercise the byte-pin and authority-window boundaries, which never reach a
/// bundle or a database.
fn synthetic_descriptor(expires_at: chrono::DateTime<Utc>) -> TransferDescriptor {
    let operator_id = Uuid::now_v7();
    TransferDescriptor {
        format_version: TRANSFER_FORMAT_VERSION,
        run_id: Uuid::now_v7(),
        provider_id: Uuid::now_v7(),
        bundle_digest: "a".repeat(64),
        mapping_ref: MappingRef::new(Uuid::now_v7(), 1),
        mapping_digest: "b".repeat(64),
        profile_digest: "c".repeat(64),
        catalog_digest: "d".repeat(64),
        tool_digest: "e".repeat(64),
        schema_digest: "f".repeat(64),
        artifact_sha256: "0".repeat(64),
        executable_sha256: "1".repeat(64),
        target: TransferTarget {
            identity_sha256: "2".repeat(64),
            fingerprint_sha256: "3".repeat(64),
            plan_digest: "4".repeat(64),
        },
        operator: TransferOperator {
            user_id: operator_id,
        },
        authority: SourceAttestationIdentity {
            operator_id,
            provenance_sha256: "8".repeat(64),
            authority_id: Uuid::now_v7(),
            attestation_sha256: "5".repeat(64),
            issuer: "pwa-81/synthetic-fixture".into(),
            issued_at: Utc::now() - TimeDelta::hours(2),
            scope_digest: "6".repeat(64),
            expectations_sha256: "7".repeat(64),
            edge_coverage: EdgeCoverage::TypedInventoryReferences,
        },
        approval: TransferApproval {
            not_before: Utc::now() - TimeDelta::hours(1),
            expires_at,
        },
        retain_absence: RetainAbsencePolicy::RetainAndReport,
        scope: TransferScope {
            creates: true,
            updates: true,
            owner_assignments: false,
            custom_fields: false,
            object_references: false,
            secret_intents: false,
        },
    }
}

fn current_descriptor() -> TransferDescriptor {
    synthetic_descriptor(Utc::now() + TimeDelta::hours(1))
}

fn invoke_apply(descriptor: &std::path::Path, descriptor_digest: &str) -> std::process::Output {
    command()
        .args(["apply", "--descriptor"])
        .arg(descriptor)
        .args([
            "--descriptor-digest",
            descriptor_digest,
            "--bundle",
            "unused-bundle",
            "--mapping",
            "unused-mapping",
            "--profile",
            "unused-profile",
            "--endorse-provenance-digest",
            &"8".repeat(64),
        ])
        .output()
        .expect("start CLI")
}

#[test]
fn help_runs_without_database_redis_or_server_configuration() {
    let result = command().arg("--help").output().expect("start CLI");
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout).expect("UTF-8 help");
    assert!(output.contains(&catalog_digest()));
    assert!(output.contains(&tool_digest()));
}

#[test]
fn mutation_and_secret_arguments_fail_without_echoing_values() {
    for args in [
        vec!["apply"],
        vec!["resume"],
        vec!["descriptor"],
        vec!["export", "--token", "CLI-PRIVATE-CANARY"],
        vec!["apply", "--token", "CLI-PRIVATE-CANARY"],
        vec!["dry-run", "--database-url", "CLI-PRIVATE-CANARY"],
    ] {
        let result = command().args(args).output().expect("start CLI");
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("CLI-PRIVATE-CANARY"));
    }
}

#[test]
fn export_has_no_http_alias_and_requires_an_exact_explicit_endorsement() {
    let old_http = command()
        .args([
            "export",
            "--url",
            "http://CLI-PRIVATE-CANARY",
            "--provider-id",
            &Uuid::now_v7().to_string(),
            "--exporter-source-sha",
            &"a".repeat(40),
            "--output",
            "unused",
        ])
        .output()
        .expect("start CLI");
    assert_eq!(old_http.status.code(), Some(1));
    assert!(old_http.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&old_http.stderr).contains("CLI-PRIVATE-CANARY"));

    let private = tempfile::tempdir().expect("private input");
    let transport = private.path().join("transport.json");
    std::fs::write(&transport, b"{}").expect("transport");
    let output = private.path().join("bundle");
    let result = command()
        .args(["export", "--snapshot-transport"])
        .arg(&transport)
        .args([
            "--provider-id",
            &Uuid::now_v7().to_string(),
            "--operator-id",
            &Uuid::now_v7().to_string(),
            "--endorse-provenance-digest",
            &"0".repeat(64),
            "--output",
        ])
        .arg(&output)
        .output()
        .expect("start CLI");
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&result.stderr),
        "inventory_transfer:snapshot_refused\n"
    );
    assert!(result.stdout.is_empty());
    assert!(!output.exists());
}

#[test]
fn apply_rejects_altered_or_unpinned_descriptor_bytes_before_any_environment() {
    let private = tempfile::tempdir().expect("private input");
    let pinned = current_descriptor()
        .pin()
        .expect("pin synthetic descriptor");
    let descriptor_path = private.path().join("descriptor.json");
    std::fs::write(&descriptor_path, pinned.bytes()).expect("descriptor file");

    // A different pin than the exact bytes.
    let wrong_pin = invoke_apply(&descriptor_path, &"9".repeat(64));
    assert_eq!(wrong_pin.status.code(), Some(2));
    assert!(wrong_pin.stderr.is_empty());
    let report: Value = serde_json::from_slice(&wrong_pin.stdout).expect("structured report");
    assert_eq!(report["state"], "blocked");
    assert_eq!(report["code"], "inventory_transfer:descriptor_invalid");
    assert!(
        report["run_id"].is_null(),
        "an unparsed descriptor has no run identity"
    );

    // A re-pinned document with an unknown field: strict decoding, no defaults.
    let mut value: Value = serde_json::from_slice(pinned.bytes()).expect("descriptor json");
    value
        .as_object_mut()
        .expect("object")
        .insert("guessed_default".into(), json!(true));
    let altered = serde_json::to_vec(&value).expect("altered bytes");
    let altered_path = private.path().join("altered.json");
    std::fs::write(&altered_path, &altered).expect("altered descriptor file");
    let guessed = invoke_apply(&altered_path, &digest(&altered));
    assert_eq!(guessed.status.code(), Some(2));
    assert!(guessed.stderr.is_empty());
    let report: Value = serde_json::from_slice(&guessed.stdout).expect("structured report");
    assert_eq!(report["code"], "inventory_transfer:descriptor_invalid");
}

#[test]
fn structurally_invalid_descriptor_is_refused_before_any_environment() {
    let private = tempfile::tempdir().expect("private input");
    let mut value: Value =
        serde_json::from_slice(&current_descriptor().pin().expect("pin").bytes())
            .expect("descriptor json");
    value["format_version"] = json!(2);
    let unsupported = serde_json::to_vec(&value).expect("unsupported bytes");
    let descriptor_path = private.path().join("descriptor.json");
    std::fs::write(&descriptor_path, &unsupported).expect("descriptor file");
    let result = invoke_apply(&descriptor_path, &digest(&unsupported));
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stderr.is_empty());
    let report: Value = serde_json::from_slice(&result.stdout).expect("structured report");
    assert_eq!(report["state"], "blocked");
    assert_eq!(report["code"], "inventory_transfer:descriptor_invalid");
}

#[test]
fn descriptor_verifies_the_artifact_before_requiring_a_target_connection() {
    let private = tempfile::tempdir().expect("private input");
    let approval = private.path().join("approval.json");
    let scope = private.path().join("scope.json");
    let mapping = private.path().join("mapping.json");
    let profile = private.path().join("profile.json");
    std::fs::write(
        &approval,
        serde_json::to_vec(&TransferApproval {
            not_before: Utc::now() - TimeDelta::hours(1),
            expires_at: Utc::now() + TimeDelta::hours(1),
        })
        .expect("approval bytes"),
    )
    .expect("approval file");
    std::fs::write(
        &scope,
        serde_json::to_vec(&TransferScope {
            creates: true,
            updates: true,
            owner_assignments: false,
            custom_fields: false,
            object_references: false,
            secret_intents: false,
        })
        .expect("scope bytes"),
    )
    .expect("scope file");
    std::fs::write(&mapping, b"{}").expect("mapping file");
    std::fs::write(&profile, b"{}").expect("profile file");
    let output = private.path().join("descriptor.json");
    let result = command()
        .args(["descriptor", "--bundle"])
        .arg(private.path().join("missing-bundle"))
        .args([
            "--bundle-digest",
            &"a".repeat(64),
            "--provider-id",
            &Uuid::now_v7().to_string(),
            "--mapping",
        ])
        .arg(&mapping)
        .arg("--profile")
        .arg(&profile)
        .args([
            "--profile-digest",
            &"b".repeat(64),
            "--run-id",
            &Uuid::now_v7().to_string(),
            "--operator-id",
            &Uuid::now_v7().to_string(),
            "--approval",
        ])
        .arg(&approval)
        .arg("--scope")
        .arg(&scope)
        .args(["--endorse-provenance-digest", &"8".repeat(64)])
        .arg("--output")
        .arg(&output)
        .output()
        .expect("start CLI");
    // No INVENTORY_TRANSFER_DATABASE_URL is set: the artifact is refused first.
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stderr.is_empty());
    let report: Value = serde_json::from_slice(&result.stdout).expect("structured report");
    assert_eq!(report["code"], "inventory_transfer:bundle_unverified");
    assert!(
        !output.exists(),
        "no descriptor may be written for a refused run"
    );
}

#[test]
fn bundle_failure_precedes_any_target_configuration_or_connection() {
    let private = tempfile::tempdir().expect("private input");
    let result = command()
        .args(["dry-run", "--bundle"])
        .arg(private.path())
        .args([
            "--bundle-digest",
            &"a".repeat(64),
            "--provider-id",
            "d766fd88-e7f8-4a28-97cb-9336e9f18b68",
            "--mapping",
            "CLI-PRIVATE-CANARY",
            "--profile",
            "CLI-PRIVATE-CANARY",
            "--profile-digest",
            &"b".repeat(64),
        ])
        .output()
        .expect("start CLI");
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert_eq!(
        String::from_utf8(result.stderr).expect("UTF-8 error"),
        "inventory_transfer:bundle_verification_failed\n"
    );
}

#[test]
fn real_offline_assessment_is_deterministic_and_invalid_dry_run_needs_no_database() {
    let private = tempfile::tempdir().expect("private input");
    let bundle = private.path().join("bundle");
    let mut payloads = std::collections::BTreeMap::new();
    let mut descriptor = |path: &str| {
        let bytes = b"{}".to_vec();
        payloads.insert(path.to_owned(), bytes.clone());
        PayloadDescriptor {
            path: path.into(),
            byte_length: bytes.len() as u64,
            sha256: digest(&bytes),
        }
    };
    let now = Utc::now();
    let provider_id = Uuid::now_v7();
    let manifest = ExportManifest {
        format_version: EXPORT_FORMAT_VERSION,
        source_provider_id: provider_id,
        observed_netbox_version: "4.7.1".into(),
        exporter_source_sha: "a".repeat(40),
        authority: SourceAuthority::live_capture(),
        capture: CaptureEvidence {
            started_at: now,
            ended_at: now,
            outcome: CaptureOutcome::Incomplete,
            consistency: ConsistencyStatus::Unproven,
            consistency_evidence: descriptor("consistency.json"),
            discovery_complete: false,
            discovery_evidence: descriptor("discovery.json"),
        },
        schema: descriptor("schema.json"),
        resources: Vec::new(),
        processes: [
            ProcessArea::Plugins,
            ProcessArea::ScriptsAndJobs,
            ProcessArea::Webhooks,
            ProcessArea::EventRules,
            ProcessArea::Permissions,
        ]
        .into_iter()
        .map(|area| ProcessCoverage {
            area,
            status: ObservationStatus::Unobservable,
            evidence: None,
        })
        .collect(),
    };
    let bundle_digest = seal_bundle(&bundle, &manifest, &payloads).expect("seal synthetic bundle");
    let config = MappingConfigDocument {
        absence_policy: Default::default(),
        reference: MappingRef::new(Uuid::now_v7(), 1),
        name: "synthetic".into(),
        description: None,
        source_provider_id: provider_id,
        target: MappingTarget::LocalInventory,
        status: MappingConfigurationStatus::Active,
        direction: MappingDirection::Pull,
        conflict_mode: ConflictMode::NetboxWins,
        created_by: None,
        created_at: now,
        retired_at: None,
        entity_mappings: Vec::new(),
        field_mappings: Vec::new(),
    };
    let mapping = serde_json::to_vec(&config).expect("mapping bytes");
    let profile = serde_json::to_vec(&PreflightProfile {
        format_version: 1,
        bundle_digest: bundle_digest.clone(),
        source_provider_id: provider_id,
        mapping_ref: config.reference,
        mapping_digest: digest(&mapping),
        catalog_digest: catalog_digest(),
        tool_digest: tool_digest(),
        snapshot_authority: None,
    })
    .expect("profile bytes");
    let mapping_path = private.path().join("mapping.json");
    let profile_path = private.path().join("profile.json");
    std::fs::write(&mapping_path, mapping).expect("mapping file");
    std::fs::write(&profile_path, &profile).expect("profile file");
    let invoke = |mode: &str, profile_digest: &str| {
        command()
            .arg(mode)
            .arg("--bundle")
            .arg(&bundle)
            .args([
                "--bundle-digest",
                &bundle_digest,
                "--provider-id",
                &provider_id.to_string(),
            ])
            .arg("--mapping")
            .arg(&mapping_path)
            .arg("--profile")
            .arg(&profile_path)
            .args(["--profile-digest", profile_digest])
            .output()
            .expect("start CLI")
    };
    let first = invoke("preflight", &digest(&profile));
    let second = invoke("preflight", &digest(&profile));
    assert_eq!(first.status.code(), Some(2));
    assert!(first.stderr.is_empty() && second.stderr.is_empty());
    assert!(
        first.stdout == second.stdout,
        "offline report changed with unchanged inputs"
    );
    let report: PreflightReport = serde_json::from_slice(&first.stdout).expect("structured report");
    assert!(!report.source_coverage_complete);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == FindingCode::SourceConsistencyUnproven)
    );
    let invalid = invoke("dry-run", &"b".repeat(64));
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stderr.is_empty());
    let report: PreflightReport =
        serde_json::from_slice(&invalid.stdout).expect("structured report");
    assert!(report.target.is_none());
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == FindingCode::ProfileDigestMismatch)
    );
}

#[tokio::test]
async fn actual_export_cli_seals_incomplete_evidence_without_default_record_reads() {
    use std::sync::{Arc, Mutex};

    use axum::{
        Json, Router,
        extract::State,
        http::{Method, Uri},
    };
    use piping_backend::contexts::sync::domain::inventory_export::CaptureStatus;
    use piping_backend::contexts::sync::infrastructure::providers::ImmutableInventorySource;

    type Requests = Arc<Mutex<Vec<(String, String)>>>;
    async fn respond(State(requests): State<Requests>, method: Method, uri: Uri) -> Json<Value> {
        requests
            .lock()
            .expect("request log")
            .push((method.to_string(), uri.path().into()));
        Json(match uri.path() {
            "/api/status/" => json!({"netbox-version": "4.7.1"}),
            "/api/schema/" => json!({
                "openapi": "3.0.3",
                "paths": {"/api/dcim/sites/": {"get": {"responses": {"200": {"content": {
                    "application/json": {"schema": {"type": "object", "properties": {
                        "count": {"type": "integer"},
                        "results": {"type": "array", "items": {"type": "object", "properties": {
                            "id": {"type": "integer"}
                        }}}
                    }}}
                }}}}}}
            }),
            _ => json!({"unexpected_request": true}),
        })
    }
    let requests = Requests::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock listener");
    let address = listener.local_addr().expect("mock address");
    let router = Router::new().fallback(respond).with_state(requests.clone());
    let server =
        tokio::spawn(async move { axum::serve(listener, router).await.expect("mock server") });
    let private = tempfile::tempdir().expect("private export");
    let bundle = private.path().join("bundle");
    let provider_id = Uuid::now_v7();
    let mut cli = tokio::process::Command::new(env!("CARGO_BIN_EXE_inventory-transfer"));
    cli.env_clear()
        .env("INVENTORY_TRANSFER_NETBOX_TOKEN", "synthetic-test-token")
        .kill_on_drop(true)
        .args([
            "capture-http",
            "--url",
            &format!("http://{address}"),
            "--provider-id",
            &provider_id.to_string(),
            "--exporter-source-sha",
            &"a".repeat(40),
            "--output",
        ])
        .arg(&bundle);
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), cli.output()).await;
    server.abort();
    let output = result.expect("bounded export").expect("start CLI");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).expect("export report");
    let source = ImmutableInventorySource::open(
        &bundle,
        report["bundle_digest"].as_str().expect("manifest digest"),
        provider_id,
        100,
    )
    .expect("replay real CLI artifact");
    assert!(source.manifest().capture.outcome == CaptureOutcome::Incomplete);
    assert!(source.census().iter().any(|r| {
        r.entity
            .as_ref()
            .is_some_and(|e| e.as_str() == "dcim.sites")
            && r.status == CaptureStatus::NotAttempted
            && r.declared_rows.is_none()
            && r.payload.is_none()
    }));
    assert_eq!(
        *requests.lock().expect("request log"),
        vec![
            ("GET".into(), "/api/status/".into()),
            ("GET".into(), "/api/schema/".into()),
            ("GET".into(), "/api/status/".into()),
        ]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&bundle)
                .expect("bundle permissions")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(bundle.join("manifest.json"))
                .expect("manifest permissions")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
