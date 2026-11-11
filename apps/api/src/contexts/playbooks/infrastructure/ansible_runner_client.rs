//! Backend → Rust Ansible runner sidecar client.
//!
//! The run worker resolves one leased run group into an immutable project
//! snapshot plus a completely resolved launch, then POSTs it here. The sidecar
//! runs one native `ansible-playbook` process inside its Execution Environment
//! and streams events/completions back through supervisor-only callback tokens.
//!
//! Auth: every request carries the configured shared secret as a bearer token.
//! Credential material is part of the request body and is never logged. The
//! request type itself carries the wire contract; callers must not format it
//! with `Debug` because that would print decrypted secret values.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use piping_contracts::playbook_runner::{
    AcquireAuth, AcquireAuthKind, AcquireFailure, AcquireFailureCode, AcquireProjectRequest,
    AcquireProjectResponse, snapshot_tree_sha256,
};
pub use piping_contracts::playbook_runner::{
    RUNNER_CONTRACT_VERSION, RunnerDispatchReceipt as AnsibleDispatchReceipt,
    RunnerDispatchStatus as AnsibleDispatchStatus, RunnerJobRequest as AnsibleJobRequest,
    RunnerJobState as AnsibleJobState,
};
use uuid::Uuid;

use crate::config::Settings;
use crate::errors::AppError;
use crate::observability::inject_trace_context;
use crate::observability::metrics::{self, ExternalTarget};

/// Reqwest-backed client for the `ansible-runner` sidecar.
#[derive(Clone)]
pub struct AnsibleRunnerClient {
    http_client: reqwest::Client,
    base_url: String,
    shared_secret: String,
}

/// Result of dispatching or reconciling one durable runner acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsibleDispatchOutcome {
    /// The runner durably accepted this exact dispatch identity.
    Accepted(AnsibleDispatchReceipt),
    /// A receipt lookup proved that the runner never accepted the identity.
    NotAccepted(String),
    /// Transport failed and receipt lookup could not prove either outcome.
    AcceptanceUnknown(String),
}

/// Acquisition failure received from the runner or its transport.
#[derive(Debug)]
pub enum ProjectAcquisitionError {
    /// A validated, fixed sandbox failure category.
    Failure(AcquireFailureCode),
    /// The response or transport could not be trusted.
    Other(AppError),
}

impl From<AppError> for ProjectAcquisitionError {
    fn from(error: AppError) -> Self {
        Self::Other(error)
    }
}

impl AnsibleRunnerClient {
    /// Build a client for an explicit runner endpoint.
    ///
    /// This is also the narrow integration-test seam for transport failures;
    /// production construction remains [`Self::from_settings`].
    pub fn new(
        base_url: String,
        shared_secret: String,
        request_timeout: StdDuration,
    ) -> Result<Self, AppError> {
        let http_client = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .map_err(|error| {
                AppError::External(format!(
                    "Failed to build ansible-runner HTTP client: {error}"
                ))
            })?;
        Ok(Self {
            http_client,
            base_url: base_url.trim().to_string(),
            shared_secret,
        })
    }

    /// Build a client from settings. Fails only if the underlying reqwest client
    /// cannot be constructed; URL/secret validity is enforced at config load.
    pub fn from_settings(settings: &Arc<Settings>) -> Result<Self, AppError> {
        Self::new(
            settings.ansible_runner.base_url.clone(),
            settings.ansible_runner.shared_secret.clone(),
            StdDuration::from_millis(settings.ansible_runner.request_timeout_ms),
        )
    }

    fn endpoint(&self, suffix: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), suffix)
    }

    /// Acquire one project revision into the runner's isolated workspace.
    ///
    /// The response is untrusted: identity, destination, authentication kind and
    /// the recomputed snapshot digest must all match the request before the
    /// acquired tree may be persisted or dispatched.
    pub async fn acquire_project(
        &self,
        request: &AcquireProjectRequest,
    ) -> Result<AcquireProjectResponse, ProjectAcquisitionError> {
        if request.protocol_version != RUNNER_CONTRACT_VERSION {
            return Err(AppError::Validation(format!(
                "Runner acquisition requires protocol version {RUNNER_CONTRACT_VERSION}"
            ))
            .into());
        }
        let mut call =
            metrics::start_external_call(ExternalTarget::AnsibleRunner, "acquire_project");
        let mut response =
            inject_trace_context(self.http_client.post(self.endpoint("projects/acquire")))
                .bearer_auth(&self.shared_secret)
                .json(request)
                .send()
                .await
                .map_err(|error| {
                    AppError::External(format!(
                        "Failed to reach ansible-runner for project acquisition: {error}"
                    ))
                })?;
        let status = response.status();
        if !status.is_success() {
            if status == reqwest::StatusCode::BAD_GATEWAY {
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| AppError::External("Invalid acquisition failure".into()))?
                {
                    if bytes.len().saturating_add(chunk.len()) > 1024 {
                        return Err(AppError::External("Invalid acquisition failure".into()).into());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                if let Ok(failure) = serde_json::from_slice::<AcquireFailure>(&bytes) {
                    return Err(ProjectAcquisitionError::Failure(failure.code));
                }
            }
            return Err(AppError::External(format!(
                "ansible-runner project acquisition failed (status {status})"
            ))
            .into());
        }
        let receipt = response
            .json::<AcquireProjectResponse>()
            .await
            .map_err(|error| {
                AppError::External(format!(
                    "ansible-runner acquisition response was invalid: {error}"
                ))
            })?;
        validate_acquire_receipt(request, &receipt)?;
        call.succeeded();
        Ok(receipt)
    }

    /// Dispatch a job to the sidecar and reconcile a lost response by its
    /// durable `dispatch_id`. This method never repeats the POST itself.
    pub async fn dispatch_job(&self, job: &AnsibleJobRequest) -> AnsibleDispatchOutcome {
        let mut call = metrics::start_external_call(ExternalTarget::AnsibleRunner, "dispatch_job");
        let response = inject_trace_context(self.http_client.post(self.endpoint("jobs")))
            .bearer_auth(&self.shared_secret)
            .json(job)
            .send()
            .await;

        let failure = match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<AnsibleDispatchReceipt>().await {
                    Ok(receipt) if receipt_matches_job(&receipt, job) => {
                        call.succeeded();
                        return AnsibleDispatchOutcome::Accepted(receipt);
                    }
                    Ok(_) => "runner returned a receipt for a different dispatch".to_string(),
                    Err(error) => format!("runner acceptance response was unreadable: {error}"),
                }
            }
            Ok(response) => {
                let status = response.status();
                format!("runner rejected dispatch (status {status})")
            }
            Err(error) => format!("runner dispatch response was lost: {error}"),
        };

        match self.get_dispatch_status(job.dispatch_id).await {
            Ok(Some(status)) if receipt_matches_job(&status.receipt, job) => {
                call.succeeded();
                AnsibleDispatchOutcome::Accepted(status.receipt)
            }
            Ok(Some(_)) => AnsibleDispatchOutcome::AcceptanceUnknown(format!(
                "{failure}; receipt identity did not match the requested dispatch"
            )),
            Ok(None) => AnsibleDispatchOutcome::NotAccepted(failure),
            Err(error) => AnsibleDispatchOutcome::AcceptanceUnknown(format!(
                "{failure}; receipt reconciliation failed: {error}"
            )),
        }
    }

    /// Look up the durable receipt and current status for a dispatch identity.
    /// `None` is affirmative proof that this runner never accepted the ID.
    pub async fn get_dispatch_status(
        &self,
        dispatch_id: Uuid,
    ) -> Result<Option<AnsibleDispatchStatus>, AppError> {
        let mut call =
            metrics::start_external_call(ExternalTarget::AnsibleRunner, "dispatch_status");
        let response = inject_trace_context(
            self.http_client
                .get(self.endpoint(&format!("dispatches/{dispatch_id}"))),
        )
        .bearer_auth(&self.shared_secret)
        .send()
        .await
        .map_err(|error| {
            AppError::External(format!(
                "Failed to reconcile ansible-runner dispatch receipt: {error}"
            ))
        })?;
        if response.status().as_u16() == 404 {
            call.succeeded();
            return Ok(None);
        }
        let status = response.status();
        if !status.is_success() {
            return Err(AppError::External(format!(
                "ansible-runner dispatch receipt lookup failed (status {status})"
            )));
        }
        let receipt = response
            .json::<AnsibleDispatchStatus>()
            .await
            .map_err(|error| {
                AppError::External(format!(
                    "ansible-runner dispatch receipt response was invalid: {error}"
                ))
            })?;
        call.succeeded();
        Ok(Some(receipt))
    }

    /// Idempotently request cancellation by the durable dispatch identity.
    /// Repeating the request, including after the runner has terminalized the
    /// process, returns the current receipt. `None` proves the runner never
    /// accepted this dispatch identity at the time of the request.
    pub async fn cancel_dispatch(
        &self,
        dispatch_id: Uuid,
    ) -> Result<Option<AnsibleDispatchStatus>, AppError> {
        let mut call =
            metrics::start_external_call(ExternalTarget::AnsibleRunner, "cancel_dispatch");
        let response = inject_trace_context(
            self.http_client
                .post(self.endpoint(&format!("dispatches/{dispatch_id}/cancel"))),
        )
        .bearer_auth(&self.shared_secret)
        .send()
        .await
        .map_err(|error| {
            AppError::External(format!(
                "Failed to reach ansible-runner sidecar for cancel: {error}"
            ))
        })?;

        let status = response.status();
        if status.as_u16() == 404 {
            call.succeeded();
            return Ok(None);
        }
        if !status.is_success() {
            return Err(AppError::External(format!(
                "ansible-runner sidecar cancel failed (status {status})"
            )));
        }
        let receipt = response
            .json::<AnsibleDispatchStatus>()
            .await
            .map_err(|error| {
                AppError::External(format!(
                    "ansible-runner cancellation receipt was invalid: {error}"
                ))
            })?;
        if receipt.receipt.dispatch_id != dispatch_id {
            return Err(AppError::External(
                "ansible-runner returned a cancellation receipt for a different dispatch"
                    .to_string(),
            ));
        }
        call.succeeded();
        Ok(Some(receipt))
    }
}

fn receipt_matches_job(receipt: &AnsibleDispatchReceipt, job: &AnsibleJobRequest) -> bool {
    receipt.dispatch_id == job.dispatch_id
        && receipt.run_group_id == job.run_group_id
        && receipt.dispatch_attempt == job.dispatch_attempt
}

fn auth_kind(auth: &AcquireAuth) -> AcquireAuthKind {
    match auth {
        AcquireAuth::Anonymous => AcquireAuthKind::Anonymous,
        AcquireAuth::HttpsBasic { .. } => AcquireAuthKind::HttpsBasic,
        AcquireAuth::HttpsBearer { .. } => AcquireAuthKind::HttpsBearer,
        AcquireAuth::SshKey { .. } => AcquireAuthKind::SshKey,
    }
}

/// Compare destinations after the same harmless normalization the runner
/// applies: surrounding whitespace and one trailing slash are not a difference.
fn normalize_destination(value: &str) -> &str {
    value.trim().trim_end_matches('/')
}

fn validate_acquire_receipt(
    request: &AcquireProjectRequest,
    receipt: &AcquireProjectResponse,
) -> Result<(), AppError> {
    if receipt.acquisition_id != request.acquisition_id {
        return Err(AppError::External(
            "ansible-runner returned an acquisition receipt for a different request".to_string(),
        ));
    }
    if normalize_destination(&receipt.destination) != normalize_destination(&request.url) {
        return Err(AppError::External(format!(
            "ansible-runner acquired `{}` instead of the requested destination",
            receipt.destination
        )));
    }
    if receipt.auth_kind != auth_kind(&request.auth) {
        return Err(AppError::External(
            "ansible-runner acquisition used a different authentication kind".to_string(),
        ));
    }
    if receipt.source_commit_oid.trim().is_empty() {
        return Err(AppError::External(
            "ansible-runner acquisition did not report a source commit".to_string(),
        ));
    }
    let recomputed = snapshot_tree_sha256(&receipt.entries)
        .map_err(|error| AppError::External(format!("Invalid acquired snapshot: {error}")))?;
    if recomputed != receipt.tree_sha256 {
        return Err(AppError::External(
            "ansible-runner acquisition snapshot digest does not match its entries".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use piping_contracts::playbook_runner::{
        ProjectSnapshot, ResolvedInventory, ResolvedLaunch, ResolvedTarget, SnapshotContent,
        SnapshotEntry, SupervisorCallbacks,
    };
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn acquisition_failure_accepts_only_bounded_known_codes() {
        use axum::{Router, routing::post};
        use std::time::Duration;

        for (body, expected) in [
            (
                r#"{"code":"authentication_failed"}"#,
                Some(AcquireFailureCode::AuthenticationFailed),
            ),
            (
                r#"{"code":"reference_not_found"}"#,
                Some(AcquireFailureCode::ReferenceNotFound),
            ),
            (r#"{"code":"unknown","detail":"secret"}"#, None),
        ] {
            let app = Router::new().route(
                "/projects/acquire",
                post(move || async move { (axum::http::StatusCode::BAD_GATEWAY, body) }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let client = AnsibleRunnerClient::new(
                format!("http://{address}"),
                "test".into(),
                Duration::from_secs(5),
            )
            .unwrap();
            let request = AcquireProjectRequest {
                protocol_version: RUNNER_CONTRACT_VERSION,
                acquisition_id: Uuid::now_v7(),
                url: "https://example.invalid/a".into(),
                reference: "main".into(),
                auth: AcquireAuth::Anonymous,
                image: "example/image".into(),
                image_digest: format!("sha256:{}", "a".repeat(64)),
            };
            let result = client.acquire_project(&request).await;
            match (result, expected) {
                (Err(ProjectAcquisitionError::Failure(actual)), Some(expected)) => {
                    assert_eq!(actual, expected)
                }
                (Err(ProjectAcquisitionError::Other(_)), None) => {}
                other => panic!("unexpected acquisition result: {other:?}"),
            }
            server.abort();
        }
    }

    fn snapshot_entry() -> SnapshotEntry {
        SnapshotEntry {
            path: "site.yml".to_string(),
            content: SnapshotContent::File {
                bytes: b"- hosts: all\n".to_vec(),
                executable: false,
            },
        }
    }

    fn job_request() -> AnsibleJobRequest {
        let entries = vec![snapshot_entry()];
        let tree_sha256 = snapshot_tree_sha256(&entries).unwrap();
        AnsibleJobRequest {
            protocol_version: RUNNER_CONTRACT_VERSION,
            dispatch_id: Uuid::from_u128(0x8888),
            dispatch_attempt: 2,
            run_group_id: Uuid::from_u128(0x9999),
            callbacks: SupervisorCallbacks {
                group_token: "pjt_group".to_string(),
                run_tokens: BTreeMap::from([(Uuid::from_u128(0x1234), "pjt_run".to_string())]),
            },
            sandbox_run_tokens: BTreeMap::from([(
                Uuid::from_u128(0x1234),
                "pjt_sandbox".to_string(),
            )]),
            lease_owner: "worker-1".to_string(),
            callback_base_url: "http://backend:7000".to_string(),
            project: ProjectSnapshot {
                project_id: Uuid::from_u128(1),
                revision_id: Uuid::from_u128(2),
                tree_sha256,
                source_commit_oid: Some("abc123".to_string()),
                entries,
            },
            launch: ResolvedLaunch {
                image: "registry.example/ansible".to_string(),
                image_digest:
                    "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                        .to_string(),
                workdir: ".".to_string(),
                playbook_paths: vec!["site.yml".to_string()],
                inventory: ResolvedInventory::Managed {
                    content: "host1 ansible_host=10.0.0.1 ansible_connection=ssh\n".to_string(),
                },
                connection: None,
                targets: vec![ResolvedTarget {
                    hostname: "host1".to_string(),
                    run_id: Some(Uuid::from_u128(0x1234)),
                    target_id: Some(Uuid::from_u128(0x55)),
                    source_id: Some(Uuid::from_u128(0x66)),
                    connection: Some(json!({
                        "connection_type": "ssh",
                        "host": "10.0.0.1",
                        "port": 22,
                        "credentials": { "username": "root", "private_key": "KEY" },
                        "jump": [{
                            "host": "bastion.example.org",
                            "port": 22,
                            "credentials": { "username": "jump", "private_key": "JUMP" }
                        }],
                        "known_hosts_lines": ["10.0.0.1 ssh-ed25519 TARGET"]
                    })),
                }],
                argv: vec!["--tags=verify".to_string()],
                extra_vars: json!({"foo": "bar"}),
                limit: Some("host1".to_string()),
                env: BTreeMap::from([("LANG".to_string(), "C".to_string())]),
                secrets: BTreeMap::from([("TOKEN".to_string(), "secret-value".to_string())]),
                timeout_seconds: 1800,
            },
        }
    }

    #[test]
    fn job_request_serializes_protocol_five_shape_and_preserves_jump() {
        let serialized = serde_json::to_value(job_request()).expect("serialize job");

        assert_eq!(serialized["protocol_version"], RUNNER_CONTRACT_VERSION);
        assert_eq!(serialized["dispatch_attempt"], 2);
        assert_eq!(serialized["callbacks"]["group_token"], "pjt_group");
        assert_eq!(
            serialized["callbacks"]["run_tokens"]["00000000-0000-0000-0000-000000001234"],
            "pjt_run"
        );
        assert_eq!(
            serialized["project"]["revision_id"],
            "00000000-0000-0000-0000-000000000002"
        );
        assert_eq!(serialized["launch"]["image"], "registry.example/ansible");
        assert_eq!(serialized["launch"]["inventory"]["kind"], "managed");
        assert_eq!(
            serialized["launch"]["targets"][0]["connection"]["jump"][0]["host"],
            "bastion.example.org"
        );
        assert_eq!(
            serialized["launch"]["targets"][0]["connection"]["known_hosts_lines"][0],
            "10.0.0.1 ssh-ed25519 TARGET"
        );
        assert_eq!(serialized["launch"]["secrets"]["TOKEN"], "secret-value");
        // Retired inline execution fields must not reappear on the wire.
        for retired in [
            "playbook",
            "entrypoint",
            "project_files",
            "extravars",
            "settings",
        ] {
            assert!(serialized.get(retired).is_none(), "{retired} is retired");
        }
        // Supervisor callbacks never enter the launch environment.
        assert!(
            serialized["launch"]["env"]
                .as_object()
                .is_some_and(|env| !env.contains_key("PIPING_API_TOKEN"))
        );
    }

    fn acquire_request() -> AcquireProjectRequest {
        AcquireProjectRequest {
            protocol_version: RUNNER_CONTRACT_VERSION,
            acquisition_id: Uuid::from_u128(7),
            url: "https://git.example.org/team/repo.git".to_string(),
            reference: "main".to_string(),
            auth: AcquireAuth::Anonymous,
            image: "registry.example/acquire".to_string(),
            image_digest: "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                .to_string(),
        }
    }

    fn acquire_response(request: &AcquireProjectRequest) -> AcquireProjectResponse {
        let entries = vec![snapshot_entry()];
        AcquireProjectResponse {
            acquisition_id: request.acquisition_id,
            destination: "https://git.example.org/team/repo.git/".to_string(),
            auth_kind: AcquireAuthKind::Anonymous,
            source_commit_oid: "abc123".to_string(),
            tree_sha256: snapshot_tree_sha256(&entries).unwrap(),
            entries,
        }
    }

    #[test]
    fn acquire_receipt_matches_identity_destination_and_digest() {
        let request = acquire_request();
        validate_acquire_receipt(&request, &acquire_response(&request)).unwrap();
    }

    #[test]
    fn acquire_receipt_rejects_foreign_identity_or_destination() {
        let request = acquire_request();
        let mut foreign = acquire_response(&request);
        foreign.acquisition_id = Uuid::from_u128(8);
        assert!(validate_acquire_receipt(&request, &foreign).is_err());

        let mut wrong_destination = acquire_response(&request);
        wrong_destination.destination = "https://git.example.org/other/repo.git".to_string();
        assert!(validate_acquire_receipt(&request, &wrong_destination).is_err());
    }

    #[test]
    fn acquire_receipt_rejects_digest_drift_and_wrong_auth_kind() {
        let request = acquire_request();
        let mut drifted = acquire_response(&request);
        drifted.entries[0].content = SnapshotContent::File {
            bytes: b"other".to_vec(),
            executable: false,
        };
        assert!(validate_acquire_receipt(&request, &drifted).is_err());

        let mut wrong_auth = acquire_response(&request);
        wrong_auth.auth_kind = AcquireAuthKind::HttpsBearer;
        assert!(validate_acquire_receipt(&request, &wrong_auth).is_err());
    }
}
