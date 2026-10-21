//! Integration test target.

#[path = "common/mod.rs"]
mod common;
#[path = "support/license_activation_support.rs"]
mod support;

use common::request;
use support::{
    LICENSE_ACTIVATE_PATH, MOCK_MODE_REVOKED, build_license_admin_harness,
    latest_timeline_metadata, reset_license_tables,
};

use axum::http::StatusCode;
use piping_backend::observability::metrics;

#[tokio::test]
#[ignore] // Requires PostgreSQL and Redis; run with: cargo test --test integration_activation_rejects_revoked_token -- --ignored
async fn activation_rejects_revoked_token_with_canonical_reason_and_timeline()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A harness that will not build is a failure to report, not a reason to
    // claim success. This used to `return Ok(())`, so the lane ran the test,
    // never reached the activation request, and counted it as a pass.
    let harness = build_license_admin_harness(MOCK_MODE_REVOKED)
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
    .await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let payload: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(payload["code"], "TOKEN_REVOKED");
    assert_eq!(payload["reason_code"], "TOKEN_REVOKED");

    let metadata =
        latest_timeline_metadata(&harness.db_pool, "timeline.activation_rejected").await?;
    assert_eq!(metadata["action_kind"], "activation_rejected");
    assert_eq!(metadata["result"], "rejected");
    assert_eq!(metadata["reason_code"], "TOKEN_REVOKED");
    assert!(
        metadata["instance_fingerprint"]
            .as_str()
            .is_some_and(|value| value.starts_with("piping-")),
        "revoked activation should record the server-owned instance id"
    );

    let metrics_snapshot = metrics::render_prometheus_text()?;
    assert!(
        metrics_snapshot.contains(
            "piping_license_activation_total{reason=\"TOKEN_REVOKED\",result=\"failed\"}"
        ),
        "revoked token failures must be exposed in metrics",
    );
    Ok(())
}
