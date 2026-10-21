//! Backend → Rust Ansible runner sidecar client.
//!
//! The run worker resolves a leased run into a fully-specified Ansible job
//! (inventory + extravars + ephemeral credentials), then POSTs it here. The
//! sidecar runs one native `ansible-playbook` process inside its Execution
//! Environment and streams events/completions back to the run-group callbacks
//! (`/api/v1/playbooks/run-groups/{run_group_id}/...`), so the whole Piping
//! data-plane is reused unchanged.
//!
//! Auth: every request carries the configured shared secret as a bearer token.
//! Credential material (`ssh_key`, `passwords`) is part
//! of the request body and is never logged.

use std::sync::Arc;
use std::time::Duration as StdDuration;

pub use piping_contracts::playbook_runner::{
    RUNNER_CONTRACT_VERSION, RunnerDispatchReceipt as AnsibleDispatchReceipt,
    RunnerDispatchStatus as AnsibleDispatchStatus, RunnerJobRequest as AnsibleJobRequest,
    RunnerJobState as AnsibleJobState, RunnerJobTarget as AnsibleJobTarget,
    RunnerProjectFile as AnsibleProjectFile,
};
use uuid::Uuid;

use crate::config::Settings;
use crate::errors::AppError;

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

    /// Dispatch a job to the sidecar and reconcile a lost response by its
    /// durable `dispatch_id`. This method never repeats the POST itself.
    pub async fn dispatch_job(&self, job: &AnsibleJobRequest) -> AnsibleDispatchOutcome {
        let response = self
            .http_client
            .post(self.endpoint("jobs"))
            .bearer_auth(&self.shared_secret)
            .json(job)
            .send()
            .await;

        let failure = match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<AnsibleDispatchReceipt>().await {
                    Ok(receipt) if receipt_matches_job(&receipt, job) => {
                        return AnsibleDispatchOutcome::Accepted(receipt);
                    }
                    Ok(_) => "runner returned a receipt for a different dispatch".to_string(),
                    Err(error) => format!("runner acceptance response was unreadable: {error}"),
                }
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                format!("runner rejected dispatch (status {status}): {body}")
            }
            Err(error) => format!("runner dispatch response was lost: {error}"),
        };

        match self.get_dispatch_status(job.dispatch_id).await {
            Ok(Some(status)) if receipt_matches_job(&status.receipt, job) => {
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
        let response = self
            .http_client
            .get(self.endpoint(&format!("dispatches/{dispatch_id}")))
            .bearer_auth(&self.shared_secret)
            .send()
            .await
            .map_err(|error| {
                AppError::External(format!(
                    "Failed to reconcile ansible-runner dispatch receipt: {error}"
                ))
            })?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::External(format!(
                "ansible-runner dispatch receipt lookup failed (status {status}): {body}"
            )));
        }
        response
            .json::<AnsibleDispatchStatus>()
            .await
            .map(Some)
            .map_err(|error| {
                AppError::External(format!(
                    "ansible-runner dispatch receipt response was invalid: {error}"
                ))
            })
    }

    /// Idempotently request cancellation by the durable dispatch identity.
    /// Repeating the request, including after the runner has terminalized the
    /// process, returns the current receipt. `None` proves the runner never
    /// accepted this dispatch identity at the time of the request.
    pub async fn cancel_dispatch(
        &self,
        dispatch_id: Uuid,
    ) -> Result<Option<AnsibleDispatchStatus>, AppError> {
        let response = self
            .http_client
            .post(self.endpoint(&format!("dispatches/{dispatch_id}/cancel")))
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
            return Ok(None);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::External(format!(
                "ansible-runner sidecar cancel failed (status {status}): {body}"
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
        Ok(Some(receipt))
    }
}

fn receipt_matches_job(receipt: &AnsibleDispatchReceipt, job: &AnsibleJobRequest) -> bool {
    receipt.dispatch_id == job.dispatch_id
        && receipt.run_group_id == job.run_group_id
        && receipt.dispatch_attempt == job.dispatch_attempt
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn job_request_serializes_with_sidecar_field_names() {
        let job = AnsibleJobRequest {
            protocol_version: RUNNER_CONTRACT_VERSION,
            dispatch_id: Uuid::from_u128(0x8888),
            dispatch_attempt: 1,
            run_group_id: Uuid::from_u128(0x9999),
            group_callback_token: "pjt_group".to_string(),
            lease_owner: "worker-1".to_string(),
            callback_base_url: "http://backend:7000".to_string(),
            playbook: "---\n- hosts: all\n".to_string(),
            entrypoint: None,
            project_files: Vec::new(),
            inventory: "host1 ansible_host=10.0.0.1 ansible_connection=ssh ansible_user=root\n"
                .to_string(),
            targets: vec![AnsibleJobTarget {
                alias: "host1".to_string(),
                run_id: Uuid::from_u128(0x1234),
                job_token: "pjt_test".to_string(),
                target_kind: "connection_target".to_string(),
                target_id: Uuid::from_u128(0x55),
                source_kind: "device".to_string(),
                source_id: Some(Uuid::from_u128(0x66)),
                ssh_private_key: None,
                known_hosts_lines: vec![],
            }],
            extravars: serde_json::json!({"foo": "bar"}),
            ssh_key: Some("-----BEGIN KEY-----\n".to_string()),
            passwords: BTreeMap::new(),
            settings: serde_json::json!({"job_timeout": 1800, "forks": 32}),
            playbook_env: {
                let mut map = BTreeMap::new();
                map.insert(
                    "PIPING_API_BASE_URL".to_string(),
                    "http://backend:7000".to_string(),
                );
                map
            },
        };
        let serialized = serde_json::to_value(&job).expect("serialize job");
        assert_serialized_job_identity(&serialized);
        assert_serialized_job_payload(&serialized);
    }

    fn assert_serialized_job_identity(serialized: &serde_json::Value) {
        assert_eq!(
            serialized["run_group_id"],
            "00000000-0000-0000-0000-000000009999"
        );
        assert_eq!(serialized["group_callback_token"], "pjt_group");
        assert!(serialized.get("job_token").is_none());
        assert_eq!(serialized["protocol_version"], RUNNER_CONTRACT_VERSION);
        assert_eq!(serialized["dispatch_attempt"], 1);
        assert_eq!(serialized["lease_owner"], "worker-1");
        assert_eq!(serialized["callback_base_url"], "http://backend:7000");
    }

    fn assert_serialized_job_payload(serialized: &serde_json::Value) {
        assert_eq!(
            serialized["playbook_env"]["PIPING_API_BASE_URL"],
            "http://backend:7000"
        );
        assert!(serialized["playbook"].is_string());
        assert!(serialized["inventory"].is_string());
        assert_eq!(serialized["targets"][0]["alias"], "host1");
        assert_eq!(
            serialized["targets"][0]["run_id"],
            "00000000-0000-0000-0000-000000001234"
        );
        assert_eq!(serialized["targets"][0]["job_token"], "pjt_test");
        assert_eq!(serialized["extravars"]["foo"], "bar");
        assert!(serialized["ssh_key"].is_string());
        // Empty passwords map is omitted from the wire payload.
        assert!(serialized.get("passwords").is_none());
        assert_eq!(serialized["settings"]["job_timeout"], 1800);
        assert_eq!(serialized["settings"]["forks"], 32);
    }

    #[test]
    fn job_request_omits_ssh_key_when_absent() {
        let job = AnsibleJobRequest {
            protocol_version: RUNNER_CONTRACT_VERSION,
            dispatch_id: Uuid::from_u128(8),
            dispatch_attempt: 1,
            run_group_id: Uuid::from_u128(9),
            group_callback_token: "group".to_string(),
            lease_owner: "w".to_string(),
            callback_base_url: "http://backend:7000".to_string(),
            playbook: "---\n".to_string(),
            entrypoint: None,
            project_files: Vec::new(),
            inventory: "localhost\n".to_string(),
            targets: vec![AnsibleJobTarget {
                alias: "localhost".to_string(),
                run_id: Uuid::from_u128(1),
                job_token: "t".to_string(),
                target_kind: "connection_target".to_string(),
                target_id: Uuid::from_u128(2),
                source_kind: "device".to_string(),
                source_id: None,
                ssh_private_key: None,
                known_hosts_lines: vec![],
            }],
            extravars: serde_json::json!({}),
            ssh_key: None,
            passwords: {
                let mut map = BTreeMap::new();
                map.insert("^Password:".to_string(), "secret".to_string());
                map
            },
            settings: serde_json::json!({}),
            playbook_env: BTreeMap::new(),
        };
        let serialized = serde_json::to_value(&job).expect("serialize job");
        assert!(serialized.get("ssh_key").is_none());
        assert!(serialized.get("playbook_env").is_none());
        assert_eq!(serialized["passwords"]["^Password:"], "secret");
    }
}
