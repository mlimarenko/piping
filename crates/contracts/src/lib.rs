//! Shared cross-process contracts for the piping workspace.
//!
//! Hosts values that MUST stay identical between backend, CLI, proxy and runner
//! binaries: license tier, proxy protocol discriminator, policy effect, run
//! mode, auth source, and the backend-runner wire protocol. Product DTOs and
//! HTTP schemas live in `apps/api`.

use serde::{Deserialize, Serialize};

pub mod ansible_options;

pub mod playbook_runner {
    //! Wire contract between the Piping backend and a playbook runner sidecar.
    //!
    //! The runner sidecar is an implementation detail: the current implementation
    //! is Rust and supervises native `ansible-playbook`. This module is the
    //! stable JSON contract future implementations must honor. It intentionally
    //! models the sidecar as a process supervisor, not as an Ansible
    //! reimplementation.
    //!
    //! Two request types cross the boundary: [`AcquireProjectRequest`] (fetch one
    //! project revision into an isolated workspace) and [`RunnerJobRequest`]
    //! (execute one already-resolved launch against an immutable snapshot). Both
    //! reject unknown top-level fields, so a retired inline payload field cannot
    //! be silently accepted as a legacy execution path.
    //!
    //! A failed acquisition answers with [`AcquireFailure`]: a fixed,
    //! credential-free category, never the native Git diagnostic or remote text.

    use std::collections::{BTreeMap, BTreeSet};

    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    /// Backend-runner HTTP contract version. This is a wire-sanity handshake,
    /// not a compatibility-negotiation channel: backend and runner deploy
    /// lockstep from one repository, so the runner rejects any request whose
    /// `protocol_version` is not exactly this value. Bump it on any
    /// non-additive wire change.
    ///
    /// Version 7 adds [`RunnerJobRequest::sandbox_run_tokens`], the writeback
    /// credentials delivered into the job sandbox. Backend and runner must
    /// upgrade as a pair: a v6 runner rejects the new field, and a v7 backend
    /// would otherwise dispatch a job without the tokens its playbook needs.
    pub const RUNNER_CONTRACT_VERSION: u16 = 7;

    /// Maximum number of symlink hops followed while proving that a snapshot
    /// symlink stays inside the tree. Mirrors the Linux `MAXSYMLINKS` ceiling so
    /// a chain that the kernel would refuse is also refused here.
    pub const MAX_SNAPSHOT_SYMLINK_DEPTH: usize = 40;

    /// Maximum length of one snapshot entry path, in bytes. Paths longer than
    /// this cannot be created portably and are rejected at the boundary.
    pub const MAX_SNAPSHOT_PATH_BYTES: usize = 4096;

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
        /// No valid terminal structured host result was reported, so the
        /// target's outcome is unknown. Never a substitute for a success or
        /// failure verdict.
        Unreported,
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

    /// One immutable snapshot entry.
    ///
    /// Only regular files and symlinks are representable, so a device, socket,
    /// FIFO or directory node can never be smuggled onto the wire: the `kind`
    /// tag has no such variant and serde rejects it.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum SnapshotContent {
        /// Regular file. `bytes` is arbitrary binary data with no text-only
        /// restriction; `executable` carries the owner-execute bit.
        File {
            /// Raw file contents, exactly as checked out.
            bytes: Vec<u8>,
            /// Whether the file is executable.
            executable: bool,
        },
        /// Symbolic link. `target` is the link text, not the resolved path.
        Symlink {
            /// Link text; resolved and containment-checked by
            /// [`validate_snapshot_entries`].
            target: String,
        },
    }

    /// One entry of an immutable project snapshot.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct SnapshotEntry {
        /// Relative, POSIX-normalized path inside the snapshot root.
        pub path: String,
        /// Immutable file or symlink content.
        pub content: SnapshotContent,
    }

    /// Immutable project revision dispatched to the runner.
    ///
    /// Carries no credentials: `secrets` live only in the transient dispatch
    /// inputs of [`ResolvedLaunch`], never in the snapshot.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ProjectSnapshot {
        /// Piping project ID.
        pub project_id: Uuid,
        /// Piping project revision ID.
        pub revision_id: Uuid,
        /// Lowercase hex SHA-256 returned by [`snapshot_tree_sha256`] for
        /// `entries`. The runner recomputes it and refuses a mismatch.
        pub tree_sha256: String,
        /// Source commit the snapshot was materialized from, when known.
        pub source_commit_oid: Option<String>,
        /// Complete tree, validated by [`validate_snapshot_entries`].
        pub entries: Vec<SnapshotEntry>,
    }

    /// Completely resolved execution inputs for one launch.
    ///
    /// This is a transient dispatch input. `secrets` and the `connection`
    /// payloads, which carry resolved credentials, are ephemeral: they are
    /// handed to the supervisor for this dispatch only and are never persisted
    /// in a launch spec or a snapshot.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct ResolvedLaunch {
        /// Repository reference without a tag, for example
        /// `registry.example/ansible`. The runner pulls `{image}@{image_digest}`.
        pub image: String,
        /// Content digest of `image`, for example `sha256:<64 lowercase hex>`.
        /// A mutable tag is not an acceptable value.
        pub image_digest: String,
        /// Working directory inside the runner sandbox.
        pub workdir: String,
        /// Playbook paths relative to `workdir`, in execution order.
        pub playbook_paths: Vec<String>,
        /// Inventory to execute against.
        pub inventory: ResolvedInventory,
        /// Optional native inventory record defaults: the exact canonical
        /// resolved connection payload produced by the backend connection
        /// resolver, applied as per-host defaults when `inventory` is
        /// [`ResolvedInventory::Native`]. `None` for a managed inventory, which
        /// already carries fully resolved host variables.
        ///
        /// Transient and sensitive: the backend supplies it only after an
        /// explicit `DelegateCredentials` authorization, it is never persisted
        /// as a launch secret, and its schema is owned by the connection
        /// resolver, not by this contract.
        pub connection: Option<Value>,
        /// Per-host targets with their resolved connection payload.
        pub targets: Vec<ResolvedTarget>,
        /// Exact argv passed to `ansible-playbook`.
        pub argv: Vec<String>,
        /// Extra variables merged into the launch.
        pub extra_vars: Value,
        /// Optional Ansible host limit.
        pub limit: Option<String>,
        /// Non-secret environment variables.
        pub env: BTreeMap<String, String>,
        /// Ephemeral secret material for this dispatch only. Never persisted and
        /// never part of a [`ProjectSnapshot`].
        pub secrets: BTreeMap<String, String>,
        /// Execution timeout in seconds.
        pub timeout_seconds: u32,
    }

    /// One Piping target resolved into an Ansible host.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct ResolvedTarget {
        /// Ansible host alias used in the inventory.
        pub hostname: String,
        /// Piping playbook run ID, when this target belongs to one run.
        pub run_id: Option<Uuid>,
        /// Connection target ID, when this host is a managed target.
        pub target_id: Option<Uuid>,
        /// Source device ID, when this host came from a device record.
        pub source_id: Option<Uuid>,
        /// Exact canonical resolved connection payload for this host, carried
        /// verbatim from the backend connection resolver: the target's resolved
        /// `host`/`port`/`credentials`, the ordered `jump[]` bastion chain of
        /// `{host, port, credentials}`, and the `known_hosts_lines` covering the
        /// target and every hop. `None` only when the target does not connect
        /// over SSH.
        ///
        /// The runner consumes this payload as-is and MUST preserve the whole
        /// resolved route, including `jump`: stripping the bastion chain or
        /// falling back to a direct connection is never acceptable. This is a
        /// transient resolver output supplied only after an explicit
        /// `DelegateCredentials` authorization and is never persisted as a
        /// launch secret. The schema is owned by the connection resolver, so
        /// this contract carries it verbatim rather than defining a competing
        /// type.
        pub connection: Option<Value>,
    }

    /// Inventory source for a launch.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum ResolvedInventory {
        /// Backend-generated inventory content, verbatim.
        Managed {
            /// Inventory file contents.
            content: String,
        },
        /// Native inventory sources shipped in the project snapshot.
        Native {
            /// Inventory source paths relative to the launch workdir.
            sources: Vec<String>,
        },
    }

    /// Callback credentials for one run group.
    ///
    /// Supervisor-only: retained by the runner supervisor and never passed into
    /// the job sandbox, which is why it is not part of [`ResolvedLaunch`].
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct SupervisorCallbacks {
        /// Callback token for group-level events and completions.
        pub group_token: String,
        /// Per-run callback token, keyed by Piping run ID.
        pub run_tokens: BTreeMap<Uuid, String>,
    }

    /// Authentication for project acquisition, scoped to the request destination.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum AcquireAuth {
        /// No credentials.
        Anonymous,
        /// HTTPS basic authentication.
        HttpsBasic {
            /// Basic-auth user name.
            username: String,
            /// Ephemeral basic-auth password.
            password: String,
        },
        /// HTTPS bearer authentication.
        HttpsBearer {
            /// Ephemeral bearer token.
            token: String,
        },
        /// SSH public-key authentication.
        SshKey {
            /// The canonical connection-service payload, including ordered
            /// jumps, per-hop credentials and stored host-key pins. Transient
            /// sensitive material; never persist in project metadata or logs.
            connection: Value,
        },
    }

    /// Acquire one project revision into an isolated runner workspace.
    ///
    /// The runner acquires Git in an isolated HOME with system/global Git config
    /// and hooks disabled. Credentials are destination-scoped; SSH host keys use
    /// a persistent supervisor-owned trust store, never a job-writable pin file.
    /// `image`/`image_digest` select the isolated acquisition image; this
    /// contract only carries that input and makes no claim that a runner
    /// enforces it yet.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AcquireProjectRequest {
        /// Wire contract version; the runner requires
        /// [`RUNNER_CONTRACT_VERSION`].
        pub protocol_version: u16,
        /// Stable identity for this acquisition attempt.
        pub acquisition_id: Uuid,
        /// Generic HTTPS, SSH or `scp`-style remote repository URL.
        pub url: String,
        /// Branch, tag or commit to acquire.
        pub reference: String,
        /// Destination-scoped authentication.
        pub auth: AcquireAuth,
        /// Repository reference of the isolated acquisition image, without a tag.
        pub image: String,
        /// Content digest of `image`, for example `sha256:<64 lowercase hex>`.
        /// A mutable tag is not an acceptable value.
        pub image_digest: String,
    }

    /// Result of a successful acquisition.
    ///
    /// `tree_sha256` is the lowercase hex digest returned by
    /// [`snapshot_tree_sha256`] for `entries`, so the backend verifies the same
    /// bytes it will later dispatch.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AcquireProjectResponse {
        /// Acquisition identity echoed from the request.
        pub acquisition_id: Uuid,
        /// Acquired destination, normalized by the runner.
        pub destination: String,
        /// Authentication kind used, without any credential material.
        pub auth_kind: AcquireAuthKind,
        /// Commit the workspace was checked out at.
        pub source_commit_oid: String,
        /// Lowercase hex SHA-256 of `entries` from [`snapshot_tree_sha256`].
        pub tree_sha256: String,
        /// Complete validated tree.
        pub entries: Vec<SnapshotEntry>,
    }

    /// Credential-free authentication discriminator for acquisition responses.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AcquireAuthKind {
        /// No credentials.
        Anonymous,
        /// HTTPS basic authentication.
        HttpsBasic,
        /// HTTPS bearer authentication.
        HttpsBearer,
        /// SSH public-key authentication.
        SshKey,
    }

    /// Fixed reason an acquisition produced no revision.
    ///
    /// The runner derives this from its sandbox's fixed diagnostics; native Git
    /// output, URLs and credentials never cross this boundary. Consumers map
    /// these categories to operator actions, so an unknown value is not a new
    /// category: [`AcquireFailureCode::parse`] rejects it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AcquireFailureCode {
        /// The remote rejected the project credentials.
        AuthenticationFailed,
        /// The remote host key does not match the trusted endpoint.
        HostKeyRejected,
        /// The configured branch, tag or commit is absent from the remote.
        ReferenceNotFound,
        /// Git itself or the isolated sandbox failed without a narrower category.
        GitFailed,
    }

    impl AcquireFailureCode {
        /// Canonical wire spelling.
        #[must_use]
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::AuthenticationFailed => "authentication_failed",
                Self::HostKeyRejected => "host_key_rejected",
                Self::ReferenceNotFound => "reference_not_found",
                Self::GitFailed => "git_failed",
            }
        }

        /// Parse only the exact published spellings.
        #[must_use]
        pub fn parse(value: &str) -> Option<Self> {
            match value {
                "authentication_failed" => Some(Self::AuthenticationFailed),
                "host_key_rejected" => Some(Self::HostKeyRejected),
                "reference_not_found" => Some(Self::ReferenceNotFound),
                "git_failed" => Some(Self::GitFailed),
                _ => None,
            }
        }
    }

    /// Response body of a failed [`AcquireProjectRequest`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AcquireFailure {
        /// Fixed failure category.
        pub code: AcquireFailureCode,
    }

    /// Backend-to-runner launch request accepted by `POST /jobs`.
    ///
    /// Retired inline payload fields (`playbook`, `inventory` as text, the old
    /// `targets` shape, `extravars`, `settings`) are rejected rather than
    /// ignored, so a stale producer fails loudly instead of silently taking a
    /// legacy execution path.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
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
        /// Scoped callback credentials, retained only by the supervisor.
        pub callbacks: SupervisorCallbacks,
        /// Per-run writeback credentials delivered into the job sandbox, keyed
        /// by Piping run ID. Deliberately separate from
        /// [`SupervisorCallbacks`]: the supervisor keeps its own tokens, and
        /// these values only ever reach the job through its private controller
        /// directory.
        pub sandbox_run_tokens: BTreeMap<Uuid, String>,
        /// Backend worker lease owner that started the run.
        pub lease_owner: String,
        /// Backend base URL the sidecar POSTs callbacks back to.
        pub callback_base_url: String,
        /// Immutable snapshot and completely resolved execution inputs.
        pub project: ProjectSnapshot,
        /// A digest-pinned image and inputs resolved before dispatch.
        pub launch: ResolvedLaunch,
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

    /// Largest console chunk, after exact known-secret redaction.
    pub const MAX_CONSOLE_CHUNK_BYTES: usize = 65_536;
    /// Maximum decoded console bytes in one callback.
    pub const MAX_CONSOLE_BATCH_BYTES: usize = 262_144;
    /// Maximum number of chunks in one callback or replay page.
    pub const MAX_CONSOLE_BATCH_CHUNKS: usize = 64;
    /// Sequence values must remain exact in JSON consumers using JavaScript.
    pub const MAX_CONSOLE_SEQUENCE: u64 = 9_007_199_254_740_991;

    /// Stream label; ordering is the supervisor's observed multiplex order,
    /// not an assertion about simultaneous writes in separate OS streams.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConsoleStream {
        Stdout,
        Stderr,
    }

    /// Exact redacted bytes, not a line, UTF-8 string or derived Ansible event.
    /// Sequence starts at zero and is contiguous across stdout and stderr for
    /// one dispatch. JSON represents `bytes` as an array of integers 0..255.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ConsoleChunk {
        pub sequence: u64,
        pub stream: ConsoleStream,
        pub bytes: Vec<u8>,
    }

    /// POST to `/api/v1/playbooks/run-groups/{group_id}/console/batch`
    /// using the supervisor-only group callback token. Exact retries are
    /// idempotent; conflicting bytes or sequence gaps fail with HTTP 409.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct RunnerConsoleBatch {
        pub dispatch_id: Uuid,
        pub dispatch_attempt: i32,
        pub chunks: Vec<ConsoleChunk>,
    }

    /// Acknowledges a durable contiguous prefix [0, next_sequence).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ConsoleAck {
        pub dispatch_id: Uuid,
        pub dispatch_attempt: i32,
        pub next_sequence: u64,
    }

    /// Authorized user replay from an inclusive `from_sequence` cursor.
    /// `next_sequence` is the next page cursor, not a timestamp or DB event ID.
    /// `complete` means this page reaches the supervisor's sealed final prefix.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ConsolePage {
        pub dispatch_id: Option<Uuid>,
        pub dispatch_attempt: i32,
        pub chunks: Vec<ConsoleChunk>,
        pub next_sequence: u64,
        pub complete: bool,
    }

    /// Check transport bounds and intra-batch order before taking DB locks.
    /// The storage owner additionally checks the durable prefix and exact replay.
    pub fn validate_console_batch(batch: &RunnerConsoleBatch) -> Result<(), String> {
        if batch.dispatch_id.is_nil() || batch.dispatch_attempt < 1 {
            return Err("invalid console dispatch identity".into());
        }
        if batch.chunks.is_empty() || batch.chunks.len() > MAX_CONSOLE_BATCH_CHUNKS {
            return Err("invalid console chunk count".into());
        }
        let mut total = 0_usize;
        let mut previous = None;
        for chunk in &batch.chunks {
            if chunk.sequence >= MAX_CONSOLE_SEQUENCE
                || previous.is_some_and(|value| chunk.sequence != value + 1)
                || chunk.bytes.is_empty()
                || chunk.bytes.len() > MAX_CONSOLE_CHUNK_BYTES
            {
                return Err("invalid console chunk sequence or size".into());
            }
            total += chunk.bytes.len();
            if total > MAX_CONSOLE_BATCH_BYTES {
                return Err("console batch exceeds byte limit".into());
            }
            previous = Some(chunk.sequence);
        }
        Ok(())
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
    ///
    /// `group_status` is the authoritative outcome of the whole group, so a
    /// launch with zero per-host rows (a Native inventory that matched no host,
    /// or a failure before any host started) still reports its real
    /// success/failure/timeout. An empty `runs` list MUST NOT be read as
    /// success; consumers take the group outcome from `group_status`.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RunnerGroupCompletionBatch {
        /// Receipt identity must match both the group and callback token.
        pub dispatch_id: Uuid,
        pub dispatch_attempt: i32,
        /// Backend worker lease owner that started the group.
        pub lease_owner: String,
        /// Final durable console prefix length. Completion is rejected until
        /// the backend has acknowledged exactly this many contiguous chunks.
        pub console_next_sequence: u64,
        /// Terminal status of the group as a whole, including zero-host groups.
        pub group_status: RunnerRunStatus,
        /// Native exit code of the group launch, when applicable.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub exit_code: Option<i32>,
        /// Short operator-facing error summary for the group.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub error_message: Option<String>,
        /// Per-run terminal results. May be empty; it never overrides
        /// `group_status`.
        pub runs: Vec<RunnerGroupCompletionItem>,
    }

    /// Validate an untrusted snapshot entry list.
    ///
    /// Rejects, with an operator-readable reason:
    ///
    /// - empty, absolute, drive-prefixed, backslash or non-normalized paths
    ///   (empty, `.` or `..` segments) and paths longer than
    ///   [`MAX_SNAPSHOT_PATH_BYTES`];
    /// - duplicate paths and paths nested under a file or symlink entry;
    /// - symlink targets that are empty, absolute, drive-prefixed, malformed, or
    ///   that resolve outside the snapshot root through `..` segments;
    /// - symlink chains that leave the root, exceed
    ///   [`MAX_SNAPSHOT_SYMLINK_DEPTH`] hops, or form a cycle.
    ///
    /// Binary file contents, executable files and configuration files are all
    /// valid: there is no text-only file restriction. Device and other special
    /// nodes cannot be represented by [`SnapshotContent`] and are rejected by
    /// serde.
    pub fn validate_snapshot_entries(entries: &[SnapshotEntry]) -> Result<(), String> {
        let mut contents: BTreeMap<&str, &SnapshotContent> = BTreeMap::new();
        for entry in entries {
            validate_snapshot_path(&entry.path)?;
            if contents
                .insert(entry.path.as_str(), &entry.content)
                .is_some()
            {
                return Err(format!("duplicate snapshot path: {}", entry.path));
            }
        }

        // A file or symlink is a leaf: nothing may live underneath it.
        for &path in contents.keys() {
            for (index, _) in path.match_indices('/') {
                let ancestor = &path[..index];
                if contents.contains_key(ancestor) {
                    return Err(format!(
                        "snapshot path {path} is nested under entry {ancestor}"
                    ));
                }
            }
        }

        let symlinks: BTreeMap<&str, &str> = contents
            .iter()
            .filter_map(|(path, content)| match content {
                SnapshotContent::Symlink { target } => Some((*path, target.as_str())),
                SnapshotContent::File { .. } => None,
            })
            .collect();
        for (&path, &target) in &symlinks {
            validate_symlink_target(path, target)?;
            let mut visited = BTreeSet::new();
            resolve_symlink_chain(path, link_parent(path), target, &symlinks, &mut visited, 0)?;
        }
        Ok(())
    }

    /// Canonical tree digest for [`ProjectSnapshot::entries`].
    ///
    /// Both the backend and the runner call this function, so a digest mismatch
    /// means the two trees differ. The encoding is documented here because it is
    /// the digest contract, not an implementation detail:
    ///
    /// ```text
    /// u64 entry_count
    /// for each entry in ascending byte order of `path`:
    ///   u64 path_len, path bytes
    ///   u8  kind            0 = file, 1 = symlink
    ///   file:    u8 executable (0 or 1), u64 content_len, content bytes
    ///   symlink: u64 target_len, target bytes
    /// ```
    ///
    /// Every integer is little-endian `u64`; paths, targets and file contents are
    /// framed by their byte length, so no two distinct trees can concatenate to
    /// the same byte stream. The digest is the lowercase hex SHA-256 of that
    /// stream. `entries` is validated by [`validate_snapshot_entries`] first, so
    /// an invalid tree has no digest.
    pub fn snapshot_tree_sha256(entries: &[SnapshotEntry]) -> Result<String, String> {
        validate_snapshot_entries(entries)?;
        let mut sorted: Vec<&SnapshotEntry> = entries.iter().collect();
        sorted.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));

        let mut digest = Sha256::new();
        digest.update((sorted.len() as u64).to_le_bytes());
        for entry in sorted {
            let path = entry.path.as_bytes();
            digest.update((path.len() as u64).to_le_bytes());
            digest.update(path);
            match &entry.content {
                SnapshotContent::File { bytes, executable } => {
                    digest.update([0u8]);
                    digest.update([u8::from(*executable)]);
                    digest.update((bytes.len() as u64).to_le_bytes());
                    digest.update(bytes);
                }
                SnapshotContent::Symlink { target } => {
                    digest.update([1u8]);
                    let target = target.as_bytes();
                    digest.update((target.len() as u64).to_le_bytes());
                    digest.update(target);
                }
            }
        }
        Ok(hex_lower(&digest.finalize()))
    }

    fn hex_lower(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }

    fn validate_snapshot_path(path: &str) -> Result<(), String> {
        if path.is_empty() {
            return Err("snapshot path is empty".to_string());
        }
        if path.len() > MAX_SNAPSHOT_PATH_BYTES {
            return Err(format!(
                "snapshot path exceeds {MAX_SNAPSHOT_PATH_BYTES} bytes"
            ));
        }
        if path.contains('\0') {
            return Err("snapshot path contains a NUL byte".to_string());
        }
        if path.contains('\\') {
            return Err(format!("snapshot path is not POSIX: {path}"));
        }
        if path.starts_with('/') {
            return Err(format!("snapshot path is absolute: {path}"));
        }
        if has_drive_prefix(path) {
            return Err(format!("snapshot path has a drive prefix: {path}"));
        }
        for segment in path.split('/') {
            if segment.is_empty() {
                return Err(format!("snapshot path has an empty segment: {path}"));
            }
            if segment == "." || segment == ".." {
                return Err(format!("snapshot path is not normalized: {path}"));
            }
        }
        Ok(())
    }

    fn validate_symlink_target(path: &str, target: &str) -> Result<(), String> {
        if target.is_empty() {
            return Err(format!("symlink {path} has an empty target"));
        }
        if target.contains('\0') {
            return Err(format!("symlink {path} target contains a NUL byte"));
        }
        if target.contains('\\') {
            return Err(format!("symlink {path} target is not POSIX"));
        }
        if target.starts_with('/') {
            return Err(format!("symlink {path} has an absolute target"));
        }
        if has_drive_prefix(target) {
            return Err(format!("symlink {path} target has a drive prefix"));
        }
        if target.split('/').any(str::is_empty) {
            return Err(format!("symlink {path} target has an empty segment"));
        }
        Ok(())
    }

    fn has_drive_prefix(value: &str) -> bool {
        let bytes = value.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
    }

    /// Directory segments containing `path`.
    fn link_parent(path: &str) -> Vec<String> {
        let mut segments: Vec<String> = path.split('/').map(str::to_string).collect();
        segments.pop();
        segments
    }

    /// Resolve `target` from `base`, following symlinks in `symlinks`, and prove
    /// the result stays inside the snapshot root.
    ///
    /// `visited` holds the symlink paths already followed in this resolution
    /// chain; re-entering one is a cycle. `depth` counts followed hops.
    fn resolve_symlink_chain(
        link_path: &str,
        base: Vec<String>,
        target: &str,
        symlinks: &BTreeMap<&str, &str>,
        visited: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<Vec<String>, String> {
        let mut resolved = base;
        let mut pending: std::collections::VecDeque<String> =
            target.split('/').map(str::to_string).collect();
        while let Some(segment) = pending.pop_front() {
            match segment.as_str() {
                "." => {}
                ".." => {
                    if resolved.pop().is_none() {
                        return Err(format!(
                            "symlink {link_path} target escapes the snapshot root"
                        ));
                    }
                }
                name => {
                    resolved.push(name.to_string());
                    let current = resolved.join("/");
                    let Some(&hop_target) = symlinks.get(current.as_str()) else {
                        continue;
                    };
                    if !visited.insert(current.clone()) {
                        return Err(format!("symlink {link_path} participates in a cycle"));
                    }
                    if depth >= MAX_SNAPSHOT_SYMLINK_DEPTH {
                        return Err(format!(
                            "symlink {link_path} chain exceeds {MAX_SNAPSHOT_SYMLINK_DEPTH} hops"
                        ));
                    }
                    validate_symlink_target(&current, hop_target)?;
                    let parent = link_parent(&current);
                    resolved = resolve_symlink_chain(
                        link_path,
                        parent,
                        hop_target,
                        symlinks,
                        visited,
                        depth + 1,
                    )?;
                }
            }
        }
        Ok(resolved)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn file(path: &str, bytes: &[u8], executable: bool) -> SnapshotEntry {
            SnapshotEntry {
                path: path.to_string(),
                content: SnapshotContent::File {
                    bytes: bytes.to_vec(),
                    executable,
                },
            }
        }

        fn symlink(path: &str, target: &str) -> SnapshotEntry {
            SnapshotEntry {
                path: path.to_string(),
                content: SnapshotContent::Symlink {
                    target: target.to_string(),
                },
            }
        }

        fn sample_entries() -> Vec<SnapshotEntry> {
            vec![
                file("site.yml", b"---\n- hosts: all\n", false),
                file("group_vars/all.cfg", b"ansible_user=deploy\n", false),
                file("bin/run", b"\x7fELF\x02\x01\x01", true),
                symlink("current", "site.yml"),
            ]
        }

        fn sample_snapshot() -> ProjectSnapshot {
            let entries = sample_entries();
            let tree_sha256 = snapshot_tree_sha256(&entries).expect("digest sample entries");
            ProjectSnapshot {
                project_id: Uuid::from_u128(0x20),
                revision_id: Uuid::from_u128(0x21),
                tree_sha256,
                source_commit_oid: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
                entries,
            }
        }

        fn sample_job() -> RunnerJobRequest {
            RunnerJobRequest {
                protocol_version: RUNNER_CONTRACT_VERSION,
                dispatch_id: Uuid::from_u128(0x10),
                dispatch_attempt: 1,
                run_group_id: Uuid::from_u128(0x01),
                callbacks: SupervisorCallbacks {
                    group_token: "group-token".to_string(),
                    run_tokens: BTreeMap::from([(Uuid::from_u128(0x02), "run-token".to_string())]),
                },
                sandbox_run_tokens: BTreeMap::from([(
                    Uuid::from_u128(0x02),
                    "sandbox-token".to_string(),
                )]),
                lease_owner: "worker-1".to_string(),
                callback_base_url: "http://backend:7000".to_string(),
                project: sample_snapshot(),
                launch: ResolvedLaunch {
                    image: "registry.example/ansible".to_string(),
                    image_digest: format!("sha256:{}", "a".repeat(64)),
                    workdir: "/job/project".to_string(),
                    playbook_paths: vec!["site.yml".to_string()],
                    inventory: ResolvedInventory::Managed {
                        content: "host1 ansible_host=10.0.0.5\n".to_string(),
                    },
                    connection: Some(serde_json::json!({
                        "ansible_user": "deploy",
                        "ansible_port": 22,
                    })),
                    targets: vec![ResolvedTarget {
                        hostname: "host1".to_string(),
                        run_id: Some(Uuid::from_u128(0x02)),
                        target_id: Some(Uuid::from_u128(0x03)),
                        source_id: Some(Uuid::from_u128(0x04)),
                        connection: Some(serde_json::json!({
                            "host": "10.0.0.5",
                            "port": 22,
                            "credentials": {
                                "username": "deploy",
                                "private_key": "-----BEGIN KEY-----",
                            },
                            "jump": [{
                                "host": "10.0.0.1",
                                "port": 22,
                                "credentials": {
                                    "username": "jump",
                                    "private_key": "-----BEGIN HOP KEY-----",
                                },
                            }],
                            "known_hosts_lines": ["host1 ssh-ed25519 AAAAPIN"],
                        })),
                    }],
                    argv: vec!["ansible-playbook".to_string(), "site.yml".to_string()],
                    extra_vars: serde_json::json!({"example": true}),
                    limit: None,
                    env: BTreeMap::from([(
                        "ANSIBLE_HOST_KEY_CHECKING".to_string(),
                        "yes".to_string(),
                    )]),
                    secrets: BTreeMap::from([(
                        "vault_password".to_string(),
                        "ephemeral".to_string(),
                    )]),
                    timeout_seconds: 600,
                },
            }
        }

        fn keys(value: &Value) -> BTreeSet<String> {
            value.as_object().expect("object").keys().cloned().collect()
        }

        fn expect_rejection(entries: Vec<SnapshotEntry>) -> String {
            validate_snapshot_entries(&entries).expect_err("entry list must be rejected")
        }

        #[test]
        fn runner_contract_version_pins_the_wire_version() {
            assert_eq!(RUNNER_CONTRACT_VERSION, 7);
        }

        #[test]
        fn acquisition_failure_codes_have_exact_wire_spellings() {
            for code in [
                AcquireFailureCode::AuthenticationFailed,
                AcquireFailureCode::HostKeyRejected,
                AcquireFailureCode::ReferenceNotFound,
                AcquireFailureCode::GitFailed,
            ] {
                assert_eq!(
                    serde_json::to_value(code).expect("serialize failure code"),
                    serde_json::json!(code.as_str())
                );
                assert_eq!(AcquireFailureCode::parse(code.as_str()), Some(code));
            }
            assert_eq!(AcquireFailureCode::parse("detail"), None);
            assert_eq!(
                serde_json::to_value(AcquireFailure {
                    code: AcquireFailureCode::ReferenceNotFound,
                })
                .expect("serialize failure"),
                serde_json::json!({"code": "reference_not_found"})
            );
        }

        #[test]
        fn unreported_run_status_is_a_distinct_wire_value() {
            assert_eq!(
                serde_json::to_value(RunnerRunStatus::Unreported).expect("serialize status"),
                serde_json::json!("unreported")
            );
            let round_tripped: RunnerRunStatus =
                serde_json::from_value(serde_json::json!("unreported")).expect("deserialize");
            assert_eq!(round_tripped, RunnerRunStatus::Unreported);
        }

        #[test]
        fn runner_job_request_round_trips_with_exact_top_level_fields() {
            let job = sample_job();
            let serialized = serde_json::to_value(&job).expect("serialize runner job");

            assert_eq!(
                keys(&serialized),
                BTreeSet::from(
                    [
                        "callback_base_url",
                        "callbacks",
                        "dispatch_attempt",
                        "dispatch_id",
                        "launch",
                        "lease_owner",
                        "project",
                        "protocol_version",
                        "run_group_id",
                        "sandbox_run_tokens",
                    ]
                    .map(str::to_string)
                )
            );
            assert_eq!(serialized["protocol_version"], RUNNER_CONTRACT_VERSION);
            assert_eq!(serialized["dispatch_attempt"], 1);

            let round_tripped: RunnerJobRequest =
                serde_json::from_value(serialized).expect("deserialize runner job");
            assert_eq!(round_tripped, job);
        }

        #[test]
        fn callbacks_stay_out_of_the_execution_launch() {
            let serialized = serde_json::to_value(sample_job()).expect("serialize runner job");
            let launch = serialized["launch"].as_object().expect("launch object");
            assert!(!launch.contains_key("callbacks"));
            assert!(launch.contains_key("secrets"));
            assert_eq!(
                serialized["callbacks"]["group_token"],
                Value::String("group-token".to_string())
            );
        }

        #[test]
        fn snapshot_carries_no_secret_material() {
            let serialized = serde_json::to_value(sample_job()).expect("serialize runner job");
            assert_eq!(
                keys(&serialized["project"]),
                BTreeSet::from(
                    [
                        "entries",
                        "project_id",
                        "revision_id",
                        "source_commit_oid",
                        "tree_sha256",
                    ]
                    .map(str::to_string)
                )
            );
            let snapshot_text = serialized["project"].to_string();
            assert!(!snapshot_text.contains("ephemeral"));
            assert!(!snapshot_text.contains("BEGIN KEY"));
        }

        #[test]
        fn launch_serializes_inventory_and_connection_payloads() {
            let serialized = serde_json::to_value(sample_job()).expect("serialize runner job");
            assert_eq!(serialized["launch"]["inventory"]["kind"], "managed");
            assert_eq!(
                serialized["launch"]["connection"],
                serde_json::json!({"ansible_user": "deploy", "ansible_port": 22})
            );

            let target = &serialized["launch"]["targets"][0];
            let connection = &target["connection"];
            assert_eq!(connection["host"], "10.0.0.5");
            assert_eq!(connection["port"], 22);
            assert_eq!(connection["credentials"]["username"], "deploy");
            assert_eq!(connection["jump"][0]["credentials"]["username"], "jump");
            assert!(connection["jump"][0]["credentials"]["private_key"].is_string());
            assert!(connection["known_hosts_lines"].is_array());
            assert_eq!(serialized["launch"]["limit"], Value::Null);
        }

        #[test]
        fn connection_payloads_are_optional_and_serialize_as_null() {
            let mut job = sample_job();
            job.launch.connection = None;
            job.launch.targets[0].connection = None;
            let serialized = serde_json::to_value(&job).expect("serialize runner job");
            assert_eq!(serialized["launch"]["connection"], Value::Null);
            assert_eq!(
                serialized["launch"]["targets"][0]["connection"],
                Value::Null
            );
            let round_tripped: RunnerJobRequest =
                serde_json::from_value(serialized).expect("deserialize runner job");
            assert_eq!(round_tripped, job);
        }

        #[test]
        fn launch_and_target_have_exact_fields() {
            let serialized = serde_json::to_value(sample_job()).expect("serialize runner job");
            assert_eq!(
                keys(&serialized["launch"]),
                BTreeSet::from(
                    [
                        "argv",
                        "connection",
                        "env",
                        "extra_vars",
                        "image",
                        "image_digest",
                        "inventory",
                        "limit",
                        "playbook_paths",
                        "secrets",
                        "targets",
                        "timeout_seconds",
                        "workdir",
                    ]
                    .map(str::to_string)
                )
            );
            assert_eq!(
                keys(&serialized["launch"]["targets"][0]),
                BTreeSet::from(
                    ["connection", "hostname", "run_id", "source_id", "target_id"]
                        .map(str::to_string)
                )
            );
        }

        #[test]
        fn native_inventory_serializes_sources() {
            let inventory = ResolvedInventory::Native {
                sources: vec!["inventory/hosts.yml".to_string()],
            };
            assert_eq!(
                serde_json::to_value(&inventory).expect("serialize inventory"),
                serde_json::json!({"kind": "native", "sources": ["inventory/hosts.yml"]})
            );
        }

        #[test]
        fn group_completion_batch_serializes_group_outcome() {
            let batch = RunnerGroupCompletionBatch {
                dispatch_id: Uuid::from_u128(1),
                dispatch_attempt: 1,
                lease_owner: "worker-1".to_string(),
                console_next_sequence: 2,
                group_status: RunnerRunStatus::Failed,
                exit_code: Some(2),
                error_message: Some("host unreachable".to_string()),
                runs: vec![RunnerGroupCompletionItem {
                    run_id: Uuid::from_u128(0x02),
                    status: RunnerRunStatus::Failed,
                    exit_code: Some(2),
                    error_message: None,
                }],
            };
            let serialized = serde_json::to_value(&batch).expect("serialize completion batch");
            assert_eq!(
                keys(&serialized),
                BTreeSet::from(
                    [
                        "console_next_sequence",
                        "dispatch_attempt",
                        "dispatch_id",
                        "error_message",
                        "exit_code",
                        "group_status",
                        "lease_owner",
                        "runs",
                    ]
                    .map(str::to_string)
                )
            );
            assert_eq!(serialized["group_status"], "failed");
            assert_eq!(serialized["exit_code"], 2);

            let round_tripped: RunnerGroupCompletionBatch =
                serde_json::from_value(serialized).expect("deserialize completion batch");
            assert_eq!(round_tripped, batch);
        }

        #[test]
        fn zero_run_completion_batch_preserves_group_failure_and_timeout() {
            for status in [RunnerRunStatus::Failed, RunnerRunStatus::TimedOut] {
                let batch = RunnerGroupCompletionBatch {
                    dispatch_id: Uuid::from_u128(1),
                    dispatch_attempt: 1,
                    lease_owner: "worker-1".to_string(),
                    console_next_sequence: 0,
                    group_status: status,
                    exit_code: None,
                    error_message: Some("group failed before any host started".to_string()),
                    runs: Vec::new(),
                };
                let serialized = serde_json::to_value(&batch).expect("serialize completion batch");
                let body = serialized.as_object().expect("object");
                // Unknown exit code stays off the wire; the group status stays explicit.
                assert!(!body.contains_key("exit_code"));
                assert_eq!(body["runs"], serde_json::json!([]));
                assert_eq!(
                    body["group_status"],
                    serde_json::to_value(status).expect("serialize status")
                );

                let round_tripped: RunnerGroupCompletionBatch =
                    serde_json::from_value(serialized).expect("deserialize completion batch");
                assert!(round_tripped.runs.is_empty());
                assert_eq!(round_tripped.group_status, status);
                assert_eq!(round_tripped, batch);
            }
        }

        #[test]
        fn console_round_trip_preserves_binary_partial_lines_and_stream_order() {
            let batch = RunnerConsoleBatch {
                dispatch_id: Uuid::from_u128(1),
                dispatch_attempt: 1,
                chunks: vec![
                    ConsoleChunk {
                        sequence: 0,
                        stream: ConsoleStream::Stdout,
                        bytes: vec![0x1b, b'[', b'3', b'1', b'm', 0xf0],
                    },
                    ConsoleChunk {
                        sequence: 1,
                        stream: ConsoleStream::Stderr,
                        bytes: vec![0x9f, 0x92, 0xa9, b'\r', 0, 255],
                    },
                ],
            };
            validate_console_batch(&batch).expect("valid byte stream");
            let decoded: RunnerConsoleBatch =
                serde_json::from_slice(&serde_json::to_vec(&batch).unwrap()).unwrap();
            assert_eq!(decoded, batch);
            let mut invalid = batch.clone();
            invalid.chunks[1].sequence = 2;
            assert!(validate_console_batch(&invalid).is_err());
            invalid = batch.clone();
            invalid.chunks[1].sequence = 0;
            assert!(validate_console_batch(&invalid).is_err());
            invalid = batch;
            invalid.chunks[1].bytes = vec![0; MAX_CONSOLE_CHUNK_BYTES + 1];
            assert!(validate_console_batch(&invalid).is_err());
        }

        #[test]
        fn acquire_auth_serializes_every_explicit_kind() {
            for auth in [
                AcquireAuth::Anonymous,
                AcquireAuth::HttpsBasic {
                    username: "user".into(),
                    password: "synthetic".into(),
                },
                AcquireAuth::HttpsBearer {
                    token: "synthetic".into(),
                },
                AcquireAuth::SshKey {
                    connection: serde_json::json!({"host":"git.example.test","port":22,"jump":[{"host":"jump.example.test","port":22}],"known_hosts_lines":["git.example.test ssh-ed25519 AAAAPIN"]}),
                },
            ] {
                let encoded = serde_json::to_vec(&auth).expect("encode auth");
                let decoded: AcquireAuth = serde_json::from_slice(&encoded).expect("decode auth");
                assert_eq!(decoded, auth);
            }
        }

        fn sample_acquire_request() -> AcquireProjectRequest {
            AcquireProjectRequest {
                protocol_version: RUNNER_CONTRACT_VERSION,
                acquisition_id: Uuid::from_u128(0x30),
                url: "ssh://git@example.test/piping/project.git".into(),
                reference: "main".into(),
                auth: AcquireAuth::SshKey {
                    connection: serde_json::json!({"host":"example.test","port":22,"credentials":{"username":"git","private_key":"synthetic"},"jump":[],"known_hosts_lines":["example.test ssh-ed25519 AAAAPIN"]}),
                },
                image: "registry.example/git".into(),
                image_digest: format!("sha256:{}", "b".repeat(64)),
            }
        }

        #[test]
        fn acquire_request_round_trips_with_exact_top_level_fields() {
            let request = sample_acquire_request();
            let serialized = serde_json::to_value(&request).expect("serialize acquire request");
            assert_eq!(
                keys(&serialized),
                BTreeSet::from(
                    [
                        "acquisition_id",
                        "auth",
                        "image",
                        "image_digest",
                        "protocol_version",
                        "reference",
                        "url",
                    ]
                    .map(str::to_string)
                )
            );
            let round_tripped: AcquireProjectRequest =
                serde_json::from_value(serialized).expect("deserialize acquire request");
            assert_eq!(round_tripped, request);
        }

        #[test]
        fn acquire_response_carries_commit_tree_and_entries() {
            let snapshot = sample_snapshot();
            let response = AcquireProjectResponse {
                acquisition_id: Uuid::from_u128(0x30),
                destination: "ssh://git@example.test/piping/project.git".to_string(),
                auth_kind: AcquireAuthKind::SshKey,
                source_commit_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
                tree_sha256: snapshot.tree_sha256,
                entries: snapshot.entries,
            };
            let serialized = serde_json::to_value(&response).expect("serialize acquire response");
            assert_eq!(
                keys(&serialized),
                BTreeSet::from(
                    [
                        "acquisition_id",
                        "auth_kind",
                        "destination",
                        "entries",
                        "source_commit_oid",
                        "tree_sha256",
                    ]
                    .map(str::to_string)
                )
            );
            assert_eq!(
                snapshot_tree_sha256(&response.entries).expect("digest response entries"),
                response.tree_sha256
            );
        }

        #[test]
        fn runner_job_request_rejects_retired_inline_payload_fields() {
            for retired in [
                ("playbook", serde_json::json!("---\n- hosts: all\n")),
                (
                    "inventory",
                    serde_json::json!("host1 ansible_connection=local\n"),
                ),
                ("extravars", serde_json::json!({"example": true})),
                ("settings", serde_json::json!({"forks": 32})),
            ] {
                let mut value = serde_json::to_value(sample_job()).expect("serialize runner job");
                value[retired.0] = retired.1;
                let error = serde_json::from_value::<RunnerJobRequest>(value)
                    .expect_err("retired inline field must be rejected");
                assert!(
                    error.to_string().contains("unknown field"),
                    "unexpected error for {}: {error}",
                    retired.0
                );
            }
        }

        #[test]
        fn acquire_request_rejects_retired_inline_payload_fields() {
            for retired in ["playbook", "inventory", "targets"] {
                let mut value =
                    serde_json::to_value(sample_acquire_request()).expect("serialize acquire");
                value[retired] = serde_json::json!("legacy");
                let error = serde_json::from_value::<AcquireProjectRequest>(value)
                    .expect_err("retired inline field must be rejected");
                assert!(
                    error.to_string().contains("unknown field"),
                    "unexpected error for {retired}: {error}"
                );
            }
        }

        #[test]
        fn snapshot_entry_rejects_device_and_unknown_kinds() {
            for kind in ["device", "directory", "fifo", "socket"] {
                let value = serde_json::json!({"path": "dev/sda", "content": {"kind": kind}});
                let error = serde_json::from_value::<SnapshotEntry>(value)
                    .expect_err("non-file entry must be rejected");
                assert!(
                    error.to_string().contains("unknown variant"),
                    "unexpected error for {kind}: {error}"
                );
            }
        }

        #[test]
        fn valid_snapshot_accepts_binary_executable_and_cfg_files() {
            let entries = sample_entries();
            validate_snapshot_entries(&entries)
                .expect("binary, executable and cfg files are valid");
            assert!(matches!(
                entries[2].content,
                SnapshotContent::File {
                    executable: true,
                    ..
                }
            ));
        }

        #[test]
        fn snapshot_rejects_absolute_and_traversal_paths() {
            for path in [
                "/etc/passwd",
                "../escape",
                "a/../../escape",
                "a/../b",
                "./a",
            ] {
                let error = expect_rejection(vec![file(path, b"x", false)]);
                assert!(!error.is_empty(), "path {path} must be rejected");
            }
        }

        #[test]
        fn snapshot_rejects_malformed_paths() {
            for path in ["", "a//b", "a/", "a\\b", "C:/windows", "dir/\0name"] {
                let error = expect_rejection(vec![file(path, b"x", false)]);
                assert!(!error.is_empty(), "path {path:?} must be rejected");
            }
        }

        #[test]
        fn snapshot_rejects_duplicate_and_nested_paths() {
            expect_rejection(vec![file("a.yml", b"1", false), file("a.yml", b"2", false)]);

            let under_file =
                expect_rejection(vec![file("a", b"1", false), file("a/b", b"2", false)]);
            assert!(under_file.contains("nested under entry a"));

            expect_rejection(vec![symlink("link", "a"), file("link/child", b"2", false)]);
        }

        #[test]
        fn snapshot_rejects_absolute_and_escaping_symlink_targets() {
            // A root-level link has no parent, so any leading `..` leaves the tree.
            for target in ["/etc/passwd", "C:/windows", "../outside", "a/../../outside"] {
                let error = expect_rejection(vec![symlink("link", target)]);
                assert!(!error.is_empty(), "target {target} must be rejected");
            }
        }

        #[test]
        fn snapshot_rejects_symlink_cycles() {
            let error = expect_rejection(vec![symlink("a", "b"), symlink("b", "a")]);
            assert!(error.contains("cycle"), "unexpected error: {error}");

            let self_link = expect_rejection(vec![symlink("self", "self")]);
            assert!(self_link.contains("cycle"), "unexpected error: {self_link}");
        }

        #[test]
        fn snapshot_rejects_symlink_chains_through_an_escaping_link() {
            let error = expect_rejection(vec![symlink("a", "b/inner"), symlink("b", "../outside")]);
            assert!(!error.is_empty(), "escaping chain must be rejected");
        }

        #[test]
        fn snapshot_rejects_overlong_symlink_chains() {
            let mut entries = Vec::new();
            for index in 0..=MAX_SNAPSHOT_SYMLINK_DEPTH {
                entries.push(symlink(&format!("l{index}"), &format!("l{}", index + 1)));
            }
            entries.push(symlink(
                &format!("l{}", MAX_SNAPSHOT_SYMLINK_DEPTH + 1),
                "terminal",
            ));
            let error = expect_rejection(entries);
            assert!(error.contains("chain exceeds"), "unexpected error: {error}");
        }

        #[test]
        fn snapshot_accepts_a_contained_symlink_chain() {
            // current -> releases/v1 -> v2 -> releases/v2/site.yml, all in-tree.
            let entries = vec![
                symlink("current", "releases/v1"),
                symlink("releases/v1", "v2"),
                file("releases/v2/site.yml", b"- hosts: all\n", false),
            ];
            validate_snapshot_entries(&entries).expect("contained chain is valid");
        }

        #[test]
        fn digest_is_independent_of_entry_order() {
            let entries = sample_entries();
            let mut reversed = entries.clone();
            reversed.reverse();
            assert_eq!(
                snapshot_tree_sha256(&entries).expect("digest"),
                snapshot_tree_sha256(&reversed).expect("digest")
            );
        }

        #[test]
        fn digest_tracks_path_bytes_executable_bit_and_symlink_target() {
            let base = snapshot_tree_sha256(&[file("a", b"x", false), symlink("l", "a")])
                .expect("digest base");
            let content = snapshot_tree_sha256(&[file("a", b"y", false), symlink("l", "a")])
                .expect("digest content");
            let executable = snapshot_tree_sha256(&[file("a", b"x", true), symlink("l", "a")])
                .expect("digest executable");
            let target = snapshot_tree_sha256(&[file("a", b"x", false), symlink("l", "b")])
                .expect("digest target");
            let path = snapshot_tree_sha256(&[file("b", b"x", false), symlink("l", "a")])
                .expect("digest path");

            let mut digests = vec![base, content, executable, target, path];
            digests.sort();
            digests.dedup();
            assert_eq!(digests.len(), 5, "every component must change the digest");
        }

        #[test]
        fn digest_framing_cannot_collide_two_different_trees() {
            // Without length framing, "ab"+"c" and "a"+"bc" would hash the same.
            let split_left = snapshot_tree_sha256(&[file("ab", b"", false), file("c", b"", false)])
                .expect("digest left");
            let split_right =
                snapshot_tree_sha256(&[file("a", b"", false), file("bc", b"", false)])
                    .expect("digest right");
            assert_ne!(split_left, split_right);
        }

        #[test]
        fn digest_matches_the_documented_encoding_vector() {
            let entries = vec![
                file("README.md", b"hello\n", false),
                file("bin/run", b"\x00\xff", true),
                symlink("link", "README.md"),
            ];
            assert_eq!(
                snapshot_tree_sha256(&entries).expect("digest"),
                "bd47f39e17d271cf05c1cd487a3f9c22f3139b727c91e879769d954574f5aa13"
            );
        }

        #[test]
        fn digest_refuses_an_invalid_tree() {
            let error = snapshot_tree_sha256(&[file("../escape", b"x", false)])
                .expect_err("invalid tree has no digest");
            assert!(!error.is_empty());
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
