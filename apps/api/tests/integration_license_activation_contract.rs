//! Integration test target.

#[path = "common/mod.rs"]
mod common;
#[path = "support/license_activation_support.rs"]
mod support;

use common::request;
use support::{
    LICENSE_ACTIVATE_PATH, LICENSE_MAX_SEATS_DEFAULT, MALICIOUS_CLIENT_INSTANCE_FINGERPRINT,
    MOCK_MODE_ACTIVE, active_assignment_count, build_license_admin_harness,
    latest_license_instance_id, latest_timeline_metadata, reset_license_tables,
};

use axum::http::StatusCode;
use piping_backend::observability::metrics;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_license_activation_contract -- --ignored
async fn license_activation_returns_canonical_entitlement_state_and_records_success()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A harness that will not build is a failure to report, not a reason to
    // claim success. This used to `return Ok(())`, so the lane ran the test,
    // never reached the activation request, and counted it as a pass.
    let harness = build_license_admin_harness(MOCK_MODE_ACTIVE)
        .await
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
            format!("license admin harness could not be built: {error}").into()
        })?;
    reset_license_tables(&harness.db_pool).await?;

    let activate_body = serde_json::json!({
        "activationToken": "integration-token",
    })
    .to_string();
    let (status, body) = request(
        &harness.app,
        "POST",
        LICENSE_ACTIVATE_PATH,
        Some(&activate_body),
        Some(&harness.admin_cookie),
    )
    .await
    .expect("activate");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(payload["status"], "active");
    assert_eq!(payload["seatLimit"], LICENSE_MAX_SEATS_DEFAULT);
    assert!(payload["validFrom"].as_str().is_some());
    assert!(payload["validUntil"].as_str().is_some());
    assert!(payload["version"].as_i64().is_some());

    assert_eq!(active_assignment_count(&harness.db_pool).await?, 1);

    let instance_id = latest_license_instance_id(&harness.db_pool).await?;
    assert!(instance_id.starts_with("piping-"));

    let metadata =
        latest_timeline_metadata(&harness.db_pool, "timeline.activation_completed").await?;
    assert_eq!(metadata["action_kind"], "activation_completed");
    assert_eq!(metadata["result"], "success");
    assert_eq!(metadata["instance_fingerprint"], instance_id);

    let metrics_snapshot = metrics::render_prometheus_text().expect("metrics");
    assert!(
        metrics_snapshot
            .contains("piping_license_activation_total{reason=\"activated\",result=\"success\"}"),
        "activation success counter must be exposed in metrics",
    );
    Ok(())
}

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_license_activation_contract -- --ignored
async fn license_activation_ignores_client_supplied_instance_fingerprint()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A harness that will not build is a failure to report, not a reason to
    // claim success. This used to `return Ok(())`, so the lane ran the test,
    // never reached the activation request, and counted it as a pass.
    let harness = build_license_admin_harness(MOCK_MODE_ACTIVE)
        .await
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
            format!("license admin harness could not be built: {error}").into()
        })?;
    reset_license_tables(&harness.db_pool).await?;

    let activate_body = serde_json::json!({
        "activationToken": "integration-token",
        "instanceFingerprint": MALICIOUS_CLIENT_INSTANCE_FINGERPRINT,
    })
    .to_string();
    let (status, body) = request(
        &harness.app,
        "POST",
        LICENSE_ACTIVATE_PATH,
        Some(&activate_body),
        Some(&harness.admin_cookie),
    )
    .await
    .expect("activate");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let instance_id = latest_license_instance_id(&harness.db_pool).await?;
    assert_ne!(instance_id, MALICIOUS_CLIENT_INSTANCE_FINGERPRINT);
    assert!(instance_id.starts_with("piping-"));

    let metadata =
        latest_timeline_metadata(&harness.db_pool, "timeline.activation_completed").await?;
    assert_eq!(metadata["instance_fingerprint"], instance_id);
    Ok(())
}
