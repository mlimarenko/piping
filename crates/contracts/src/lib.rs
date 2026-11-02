//! Shared cross-process contracts for the piping workspace.
//!
//! Hosts only values that MUST stay identical between backend, CLI and proxy
//! binaries: license tier, proxy protocol discriminator, policy effect, run
//! mode, auth source. DTOs and wire-format schemas live in `apps/api`.

use serde::{Deserialize, Serialize};

pub mod playbook_runner {
    //! Wire contract between the Piping backend and a playbook runner sidecar.
    //!
    //! The runner sidecar is an implementation detail: the current implementation
    //! is Rust and supervises native `ansible-playbook`. This module is the
    //! stable JSON contract future implementations must honor. It intentionally
    //! models the sidecar as a process supervisor, not as an Ansible
    //! reimplementation.

    use std::collections::BTreeMap;

    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use uuid::Uuid;

    /// Backend-runner HTTP contract version. This is a wire-sanity handshake,
    /// not a compatibility-negotiation channel: backend and runner deploy
    /// lockstep from one repository, so the runner rejects any request whose
    /// `protocol_version` is not exactly this value. Bump it on any
    /// non-additive wire change.
    pub const RUNNER_CONTRACT_VERSION: u16 = 4;

    /// Logical status returned by the runner control API.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum RunnerJobState {
        /// The runner accepted the job and will start it asynchronously.
        Accepted,
        /// The job is currently executing.
        Running,
        /// Cancellation was requested and the runner is terminating the process tree.
        Canceling,
        /// The job completed successfully.
        Succeeded,
        /// The job completed with a failure.
        Failed,
        /// The job reached its execution timeout.
        TimedOut,
        /// The job was canceled by an operator.
        Canceled,
    }

    /// Terminal status for one Piping playbook run in a grouped Ansible job.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum RunnerRunStatus {
        /// The target run completed successfully.
        Succeeded,
        /// The target run failed.
        Failed,
        /// The target run was canceled.
        Canceled,
        /// The target run exceeded its timeout.
        TimedOut,
    }

    /// Event classification consumed by the playbook run UI.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum RunnerEventKind {
        /// Lifecycle or progress status.
        Status,
        /// Standard output line or rendered Ansible callback output.
        Stdout,
        /// Standard error line.
        Stderr,
        /// Runner-side diagnostic event.
        System,
        /// Structured Ansible task result.
        Result,
    }

    /// One backend-resolved target inside a grouped Ansible launch.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RunnerJobTarget {
        /// Inventory alias used by Ansible for this target.
        pub alias: String,
        /// Piping playbook run ID for the target.
        pub run_id: Uuid,
        /// Run-bound callback token.
        pub job_token: String,
        /// Piping target kind, for example `connection_target`.
        pub target_kind: String,
        /// Piping target ID.
        pub target_id: Uuid,
        /// Inventory source kind, for example `device` or `virtual_machine`.
        pub source_kind: String,
        /// Inventory source ID, when the target is backed by a source entity.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub source_id: Option<Uuid>,
        /// Per-host ephemeral SSH private key (PEM), when this target uses key
        /// material distinct from the rest of the group. The runner writes it to
        /// `<RUNNER_PER_HOST_KEY_DIR>/<alias>` (0600) under the project root, and
        /// the backend inventory line for this host carries
        /// `ansible_ssh_private_key_file=.piping_keys/<alias>` (relative to the
        /// play working directory). This is the fan-out path: a grouped run whose
        /// hosts use different keys sets a key here per host instead of the
        /// group-wide `RunnerJobRequest.ssh_key`. A run where every host shares
        /// one key leaves this `None` and uses the group-wide ssh-agent key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub ssh_private_key: Option<String>,
        /// OpenSSH `known_hosts` lines the runner must seed into the job-level
        /// known_hosts file. Each entry is one complete line such as
        /// `host.example.com ssh-ed25519 AAAA...`. The runner unions every
        /// target's lines into one file; an empty list still materializes an
        /// empty seed so first-use keys can be harvested.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub known_hosts_lines: Vec<String>,
    }

    /// Runner-reserved directory, relative to the Ansible project root (which is
    /// the `ansible-playbook` working directory), that the runner materializes
    /// per-host SSH key files into — one file per target alias
    /// (`.piping_keys/<alias>`, mode 0600). The backend inventory builder and
    /// the runner MUST agree on this path: the inventory line references it
    /// through `ansible_ssh_private_key_file`, and Ansible resolves that
    /// relative path against the play working directory. Project bundles may not
    /// place files under this reserved prefix.
    pub const RUNNER_PER_HOST_KEY_DIR: &str = ".piping_keys";

    /// Job-level OpenSSH `known_hosts` file inside the materialized project
    /// directory (which is the `ansible-playbook` working directory). The runner
    /// writes the union of every target's `known_hosts_lines` here and points
    /// every ssh child at it; the backend reads it back to harvest keys learned
    /// on first use.
    pub const JOB_KNOWN_HOSTS_FILE: &str = "known_hosts";

    /// The only sanctioned OpenSSH host-key options. Every ssh child must get
    /// exactly these pins with `<known_hosts>` replaced by the job-level file
    /// from [`JOB_KNOWN_HOSTS_FILE`]:
    ///
    /// * `StrictHostKeyChecking=accept-new` trusts an unseen host on first use
    ///   (TOFU) but refuses a host whose key later changes. `StrictHostKeyChecking=no`
    ///   is never emitted: it would silently accept a changed key.
    /// * `UserKnownHostsFile=<known_hosts>` is the only trust source for the job,
    ///   with `GlobalKnownHostsFile=/dev/null` suppressing the image's global
    ///   file. `UserKnownHostsFile=/dev/null` is never emitted: it would disable
    ///   host-key pinning altogether.
    /// * `HashKnownHosts=no` keeps harvested lines readable so the backend can
    ///   persist them verbatim.
    pub fn pinned_host_key_options(known_hosts: &std::path::Path) -> String {
        format!(
            "-o UserKnownHostsFile={} -o StrictHostKeyChecking=accept-new -o GlobalKnownHostsFile=/dev/null -o HashKnownHosts=no",
            known_hosts.display()
        )
    }

    /// One file in an Ansible project bundle materialized by the runner.
    ///
    /// Paths are POSIX-style relative project paths. Runner implementations must
    /// reject absolute paths, parent traversal and controller-owned files such as
    /// the generated `ansible.cfg` before writing anything to disk.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RunnerProjectFile {
        /// Relative path inside the project directory, for example
        /// `roles/web/tasks/main.yml` or `templates/config.j2`.
        pub path: String,
        /// UTF-8 file contents.
        pub content: String,
        /// Whether the file should be executable in the private data dir.
        #[serde(default, skip_serializing_if = "is_false")]
        pub executable: bool,
    }

    /// Backend-to-runner launch request accepted by `POST /jobs`.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct RunnerJobRequest {
        /// Wire contract version. The runner rejects any value that is not
        /// exactly `RUNNER_CONTRACT_VERSION` (lockstep deploy, no negotiation).
        pub protocol_version: u16,
        /// Stable identity for this dispatch attempt. Retries MUST reuse it;
        /// only a definitively unaccepted attempt may receive a new ID.
        pub dispatch_id: Uuid,
        /// Monotonic run-group dispatch attempt number, starting at one.
        pub dispatch_attempt: i32,
        /// Piping run-group ID; also used as the private data directory name.
        pub run_group_id: Uuid,
        /// Dedicated bearer token for run-group event and completion callbacks.
        /// Per-run tokens remain on [`RunnerJobTarget`] and cannot authorize a
        /// mutation for any sibling run.
        pub group_callback_token: String,
        /// Backend worker lease owner that started the run.
        pub lease_owner: String,
        /// Backend base URL the sidecar POSTs callbacks back to.
        pub callback_base_url: String,
        /// Inline single-file Ansible playbook YAML. When `project_files` is
        /// empty the runner writes this content to `entrypoint` (default
        /// `play.yml`). Multi-file / AWX-like launches send `project_files`
        /// instead; the two are alternate inputs, not versions.
        #[serde(default)]
        pub playbook: String,
        /// Entrypoint playbook path relative to the materialized project root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub entrypoint: Option<String>,
        /// Multi-file Ansible project bundle. This is the forward-compatible path
        /// for roles, files, templates, `group_vars`, `host_vars` and synced projects.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub project_files: Vec<RunnerProjectFile>,
        /// Backend-generated Ansible inventory text.
        pub inventory: String,
        /// Host aliases and their run-bound callback tokens.
        #[serde(default)]
        pub targets: Vec<RunnerJobTarget>,
        /// Extra variables passed to Ansible.
        #[serde(default)]
        pub extravars: Value,
        /// Ephemeral SSH private key for the process.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub ssh_key: Option<String>,
        /// Prompt regex to response map for become/connection passwords.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub passwords: BTreeMap<String, String>,
        /// Runner settings such as backend, forks, timeout and callback policy.
        #[serde(default)]
        pub settings: Value,
        /// Explicit environment variables exposed to the playbook process.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub playbook_env: BTreeMap<String, String>,
    }

    /// Durable receipt returned by `POST /jobs` after the runner accepts a job.
    /// Repeating the POST with the same `dispatch_id` returns this same value.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RunnerDispatchReceipt {
        /// Stable dispatch identity persisted before process spawn.
        pub dispatch_id: Uuid,
        /// Run-group ID accepted by the runner.
        pub run_group_id: Uuid,
        /// Attempt number supplied by the backend for this run group.
        pub dispatch_attempt: i32,
    }

    /// Response returned by `GET /dispatches/{dispatch_id}`.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RunnerDispatchStatus {
        /// Immutable acceptance receipt.
        #[serde(flatten)]
        pub receipt: RunnerDispatchReceipt,
        /// Current runner-side state.
        pub status: RunnerJobState,
        /// Native `ansible-playbook` exit code, when known.
        pub rc: Option<i32>,
    }

    /// One event callback sent by the runner to the backend.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct RunnerGroupEvent {
        /// Piping run ID. Absent for group-level output that is not host-specific.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub run_id: Option<Uuid>,
        /// Ansible host alias, when the event can be attributed to one host.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub host: Option<String>,
        /// UI/storage event class.
        pub event_kind: RunnerEventKind,
        /// Raw or normalized Ansible event type.
        pub event_type: String,
        /// Human-readable event message.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub message: Option<String>,
        /// Structured event payload. Keep large facts out unless explicitly needed.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub payload: Option<Value>,
    }

    /// Batched event callback sent by the runner to the backend.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct RunnerGroupEventBatch {
        /// Idempotency key for this batch. The runner assigns a fresh value once
        /// and reuses it across the batch's delivery retries; the backend ingests
        /// a given `(run_group_id, batch_id)` at most once, so a lost-ACK retry
        /// appends no duplicate events. Required — a nil/shared value would make
        /// distinct batches collide and drop events.
        pub batch_id: Uuid,
        /// Events in append order.
        pub events: Vec<RunnerGroupEvent>,
    }

    /// Completion result for one Piping run inside a grouped Ansible launch.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RunnerGroupCompletionItem {
        /// Piping playbook run ID.
        pub run_id: Uuid,
        /// Terminal run status.
        pub status: RunnerRunStatus,
        /// Native exit code, when applicable.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub exit_code: Option<i32>,
        /// Short operator-facing error summary.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub error_message: Option<String>,
    }

    /// Batched completion callback sent by the runner to the backend.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RunnerGroupCompletionBatch {
        /// Backend worker lease owner that started the group.
        pub lease_owner: String,
        /// Per-run terminal results.
        pub runs: Vec<RunnerGroupCompletionItem>,
        /// `known_hosts` lines observed during the job that were not part of the
        /// seed (TOFU harvest). Empty when the job learned no new host keys.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub new_host_keys: Vec<String>,
    }

    // `skip_serializing_if` callbacks receive the field by reference, so the
    // `&bool` signature is required — by-value would not compile with serde.
    const fn is_false(value: &bool) -> bool {
        !*value
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn sample_job_json() -> Value {
            serde_json::json!({
                "run_group_id": "00000000-0000-0000-0000-000000000001",
                "dispatch_id": "00000000-0000-0000-0000-000000000010",
                "dispatch_attempt": 1,
                "protocol_version": RUNNER_CONTRACT_VERSION,
                "group_callback_token": "group-token",
                "lease_owner": "worker-1",
                "callback_base_url": "http://backend:7000",
                "playbook": "---\n- hosts: all\n",
                "inventory": "host1 ansible_connection=local\n",
                "targets": [{
                    "alias": "host1",
                    "run_id": "00000000-0000-0000-0000-000000000002",
                    "job_token": "target-token",
                    "target_kind": "connection_target",
                    "target_id": "00000000-0000-0000-0000-000000000003",
                    "source_kind": "device",
                    "source_id": "00000000-0000-0000-0000-000000000004"
                }],
                "extravars": {"example": true},
                "settings": {"forks": 32}
            })
        }

        #[test]
        fn runner_job_request_requires_dispatch_identity() {
            let job: RunnerJobRequest =
                serde_json::from_value(sample_job_json()).expect("deserialize runner job");

            assert_eq!(job.protocol_version, RUNNER_CONTRACT_VERSION);
            assert_eq!(job.dispatch_id, Uuid::from_u128(0x10));
            assert_eq!(job.dispatch_attempt, 1);
            assert_eq!(job.targets[0].alias, "host1");
            assert_eq!(job.settings["forks"], 32);
        }

        #[test]
        fn runner_job_target_carries_optional_per_host_ssh_key() {
            let mut job: RunnerJobRequest =
                serde_json::from_value(sample_job_json()).expect("deserialize runner job");
            assert_eq!(job.targets[0].ssh_private_key, None);

            job.targets[0].ssh_private_key = Some("-----BEGIN KEY-----\n".to_string());
            let serialized = serde_json::to_value(&job).expect("serialize runner job");
            assert_eq!(
                serialized["targets"][0]["ssh_private_key"],
                "-----BEGIN KEY-----\n"
            );
            // Absent per-host key must stay off the wire so shared-key runs are
            // unchanged.
            job.targets[0].ssh_private_key = None;
            let serialized = serde_json::to_value(&job).expect("serialize runner job");
            assert!(serialized["targets"][0].get("ssh_private_key").is_none());
        }

        #[test]
        fn runner_job_target_carries_known_hosts_lines() {
            let mut job: RunnerJobRequest =
                serde_json::from_value(sample_job_json()).expect("deserialize runner job");
            // Absent seed stays off the wire so existing callers are unchanged.
            assert!(job.targets[0].known_hosts_lines.is_empty());

            job.targets[0].known_hosts_lines = vec![
                "host1 ssh-ed25519 AAAASEEDONE".to_string(),
                "host1 ssh-rsa AAAASEEDTWO".to_string(),
            ];
            let serialized = serde_json::to_value(&job).expect("serialize runner job");
            assert_eq!(
                serialized["targets"][0]["known_hosts_lines"],
                serde_json::json!(["host1 ssh-ed25519 AAAASEEDONE", "host1 ssh-rsa AAAASEEDTWO"])
            );

            let round_tripped: RunnerJobRequest =
                serde_json::from_value(serialized).expect("deserialize round-trip");
            assert_eq!(
                round_tripped.targets[0].known_hosts_lines,
                vec![
                    "host1 ssh-ed25519 AAAASEEDONE".to_string(),
                    "host1 ssh-rsa AAAASEEDTWO".to_string(),
                ]
            );
        }

        #[test]
        fn runner_contract_version_pins_the_wire_version() {
            assert_eq!(RUNNER_CONTRACT_VERSION, 4);
        }

        #[test]
        fn pinned_host_key_options_are_the_canonical_pins() {
            let options = pinned_host_key_options(std::path::Path::new("/tmp/job/known_hosts"));
            assert_eq!(
                options,
                "-o UserKnownHostsFile=/tmp/job/known_hosts -o StrictHostKeyChecking=accept-new -o GlobalKnownHostsFile=/dev/null -o HashKnownHosts=no"
            );
            assert!(!options.contains("StrictHostKeyChecking=no"));
            assert!(!options.contains("UserKnownHostsFile=/dev/null"));
        }

        #[test]
        fn runner_job_request_serializes_protocol_version_and_snake_case_status() {
            let mut job: RunnerJobRequest =
                serde_json::from_value(sample_job_json()).expect("deserialize runner job");
            job.protocol_version = RUNNER_CONTRACT_VERSION;

            let serialized = serde_json::to_value(&job).expect("serialize runner job");

            assert_eq!(serialized["protocol_version"], RUNNER_CONTRACT_VERSION);
            assert_eq!(
                serde_json::to_value(RunnerJobState::TimedOut).expect("serialize state"),
                "timed_out"
            );
            assert_eq!(
                serde_json::to_value(RunnerRunStatus::TimedOut).expect("serialize status"),
                "timed_out"
            );
        }
    }
}

/// Runtime mode selected for the backend process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    /// Full backend API and worker runtime.
    Backend,
    /// Proxy-only runtime.
    Proxy,
}

/// Authentication authority that issued or validates a user session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthSource {
    /// Local username and password authentication.
    Local,
    /// `OpenID` Connect authentication.
    Oidc,
}

/// Remote-access protocol handled through the proxy layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    /// Secure Shell session.
    Ssh,
    /// Remote Desktop Protocol session.
    Rdp,
    /// Virtual Network Computing session.
    Vnc,
    /// Application-specific proxy session.
    App,
}

/// Authorization decision produced by policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyEffect {
    /// Permit the requested action.
    Allow,
    /// Reject the requested action.
    Deny,
}

/// Product entitlement level attached to a license.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenseTier {
    /// Free community entitlement.
    Community,
    /// Standard commercial entitlement.
    Standard,
    /// Enterprise commercial entitlement.
    Enterprise,
}
