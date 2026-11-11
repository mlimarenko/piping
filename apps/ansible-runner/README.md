# piping-ansible-runner

Rust supervisor for disposable Ansible and Git OCI jobs. The backend resolves
an immutable project and launch; the selected execution image runs native
Ansible. Callback authorization, dispatch receipts and engine credentials belong
to the supervisor, not the job filesystem or environment.

## Runtime boundary

The sandbox module has explicit Docker and Kubernetes adapters. There is no
local Ansible or Git subprocess adapter. A job uses its registered image digest,
UID/GID 10001, dropped capabilities, no-new-privileges, a read-only image root,
private HOME/tmp/workspace and process, memory, CPU, output and time bounds.
Only that job's input directory is exposed. Its controller child is mounted
read-only; it contains delegated job credentials, never callback credentials.
The supervisor state root and sibling workspaces are not job mounts.

Project files retain their bytes and executable modes. Relative contained
symlinks are supported; traversal, special files and conflicting paths are
rejected. Native project configuration is not replaced. Generated inventory,
extra vars, SSH identities and host pins belong outside the project. Native
inventory and managed inventory are distinct inputs to the same executor.
Managed targets can carry resolved SSH routes with ordered per-hop identities
and stored host pins. Native inventory can use a resolved SSH record as a
credential and route defaults bundle, without substituting the record endpoint
for inventory hosts. Resolved record routes accept key authentication without an
interactive password or passphrase; ordinary native authentication options retain
their own precedence.

Native options are argv elements, not a shell command. The API and runner use
`piping_contracts::ansible_options` to reject unknown, conflicting and interactive
options. Inventory, extra vars, limit and entrypoints have separate owners.
Unattended stdin is closed. Custom plugins can request input in ways the runner
cannot recognize, so the job deadline still applies.

The execute bootstrap discovers requirements.yml and requirements.yaml in the
working directory and passes them to native `ansible-galaxy install`. Galaxy
interprets legacy role lists and combined role/collection mappings. The separate
roles/requirements.* and collections/requirements.* conventions remain supported.
Installation uses the private job HOME; the bootstrap asks Ansible for effective
search paths before prepending private role/collection locations. A failed
preparation stops the launch before the playbook runs. Python and system packages
belong to the selected image. There is no shared writable cache or forced JSON
stdout callback.

## Ansible CLI support

The structured job editor projects onto `options.argv`; it does not introduce
another executor or precedence layer. This matrix follows the
[Ansible playbook CLI](https://docs.ansible.com/ansible/latest/cli/ansible-playbook.html)
for the runner's `ansible-core>=2.22,<2.23` dependency. Saved jobs containing
unsupported arguments must be corrected before saving or launching.

| Capability | Input and status | Semantics and restrictions |
| --- | --- | --- |
| `--tags`, `-t` | Supported; Tags field | Ansible interprets the comma-separated value, including its special tags. |
| `--skip-tags` | Supported; Skip tags field | Excludes matching tasks according to native Ansible precedence. |
| `--check`, `-C` | Supported; Check mode switch | Requests simulation. Modules may lack check support, and tasks may override check mode; this is not a no-change guarantee. |
| `--diff`, `-D` | Supported; Show differences switch | Module-dependent before/after output; file contents may be exposed in the run log. |
| `--verbose`, `-v` through `-vvvv` | Supported; Verbosity 0-4 | Zero omits the flag. Set verbosity once. Higher levels are rejected. |
| `--become`, `-b` | Supported; explicit become switch | Does not imply a password prompt or grant access to a credential. |
| `--become-user`, `--become-method` | Supported; fields shown when become is enabled | Require explicit become. Ansible validates the installed method and user; a missing credential fails unattended. |
| `--limit`, `-l` | Platform-owned; Limit field | `options.limit` is the only owner; raw forms are rejected even when the field is empty. |
| `--inventory`, `--inventory-file`, `-i` | Platform-owned; inventory selection | Managed inventory or native project-relative sources. Raw forms cannot replace either selection. |
| `--extra-vars`, `-e` | Platform-owned; job parameters | JSON values are materialized in the controller directory, preserving booleans, numbers, null, arrays and objects. Raw forms are rejected. |
| Playbook paths | Platform-owned; entrypoint selection | Validated project-relative paths; no positional raw arguments. |
| `-k`, `-K`, `-J`, `--ask-pass`, `--ask-become-pass`, `--ask-vault-pass`, `--ask-vault-password`, `--step` | Interactive; rejected | The runner returns needs-input rather than waiting for a prompt. |
| `--vault-id`, `--vault-password-file`, `--vault-pass-file` | Security-restricted; no UI | File and executable password sources are not accepted through raw argv. No Vault CLI path is enabled. |
| User, connection, SSH/key/password-file options (`-u`, `-c`, `--private-key`, `--ssh-*`, password-file aliases) | Security-restricted; no raw CLI controls | Use the existing inventory, delegated connection and secret-reference gates. Project/playbook configuration retains native semantics within the sandbox. |
| Forks, connection timeout, module paths, start-at-task, list/syntax/help/version/flush-cache modes and other options | Not supported by this launch contract | No raw escape hatch. The job deadline remains the separate Timeout field, not Ansible's connection timeout. |

Options and values are never shell-split. Accepted aliases normalize to canonical
long options with attached values. Repeated options, empty values and control
characters are rejected. Disabling become clears its user and method in the UI.
Secret references are delivered by the existing environment gate, not by raw
Vault/password-file flags. These option restrictions do not turn authorized
project code into an untrusted-code security boundary.

## Native SSH record defaults

The connection-service payload supplies target `credentials`, an ordered `jump`
chain with each hop's own endpoint and credentials, and `known_hosts_lines`.
The root record's `host` and `port` are not used in native mode. Inventory sources,
project configuration, effective target host/port, and native user or identity
selections remain Ansible-owned. The runner materializes credentials and pins
under the private controller directory, never inside the project or a generated
native inventory overlay.

An administrator can explicitly authorize pin projection for native inventory
endpoints on the delegated SSH record through `PUT /api/v1/connection-config/records/{id}`:

```json
{"params":{"host":"192.0.2.10","credentials":{"username":"ansible","private_key":"00000000-0000-7000-8000-000000000001"},"native_host_key_endpoints":[{"host":"inventory.example.test","port":2222},{"host":"192.0.2.25"}]}}
```

Use an existing bound private-key secret UUID and retain the record's other
parameters when updating it. Each endpoint must already have a stored host-key
pin; a missing pin rejects dispatch before private keys are read. Port defaults
to 22. The list is limited to 256 exact hosts, never patterns or templates.
Missing or empty means no additional pins. Inheritance replaces the entire array;
`{"unset":true}` removes an inherited list. Delegation exports only these
declared public pins, not other entries in the global pin store. This list does
not set inventory, target host, port, or route. The example is API-managed; the
record form preserves the field but has no dedicated editor for it.

When native `DEFAULT_TRANSPORT` has origin `default`, the bootstrap selects
`piping.space.record_ssh`. The execution image must provide the Piping collection
on its configured Ansible collection path; the default runner image includes it.
The plugin inherits stock Ansible SSH execution, transfers and retries, and adapts
only resolved options through `set_options()`. A target identity explicitly
selected by native inputs replaces the record identity rather than adding it.
Concrete hop aliases retain only their own credentials; the `piping-native-*`
namespace is reserved and cannot be used as a native target name.

Explicit transport selection in cfg, environment, CLI, play/task or inventory
remains effective and can opt out of the defaults bundle. This is intentional
Ansible precedence, not automatic fallback. While the record transport is
selected, conflicting SSH config/route/trust options or custom SSH executables
fail explicitly. Hop, authentication and pin failures never switch to a direct
route. Canonical pins keep their original names and ports; an uncovered or
changed destination fails strict OpenSSH verification.

The defaults bundle is not a job-wide security boundary against authorized
project code. Credential delegation authorization and the execution pool's
network isolation remain separate requirements.

## Supported transports and authentication

The transport is not inferred from a hostname, address or platform name. It is
the protocol of the connection-config record assigned to the target; exactly one
of SSH, WinRM or PSRP must participate, and an ambiguous or empty set is refused.

| Target | Transport | Authentication | TLS / certificate | Jump chain |
| --- | --- | --- | --- | --- |
| Linux | SSH (`ssh`) | private key only; password and passphrase refused | strict host-key pins from the stored trust store | ordered chain, at most 8 hops, each hop key-only |
| Windows | OpenSSH (`ssh`) | private key only, as above | strict host-key pins | supported like Linux |
| Windows | WinRM (`winrm`) | username + password with `basic` or `ntlm` | HTTPS on 5986 with validation always on; cleartext only with explicit `scheme: http` on port 5985 | not supported; a record with a jump route is refused |
| Windows | PSRP (`psrp`) | username + password with `basic` or `ntlm` (`ansible_psrp_auth`) | as WinRM | not supported; refused |

Windows OpenSSH states its shell semantics explicitly on the record with
`options.shell = "powershell"` or `options.shell = "cmd"` (Windows OpenSSH with the
default cmd.exe shell); `sh` is the default and nothing is guessed. The
runner renders the shell type into the generated host variables, so a Windows
OpenSSH target never falls back to a POSIX shell by accident.

Secrets stay out of the project, extra vars and the inventory text: the API
resolves them through the existing connection secret gate, the runner writes
Windows passwords and CA material into the private controller directory
(`routes.json`, mode 0600) and registers every value for log scrubbing. There is
no "ignore certificate" option; an unreachable or untrusted API/endpoint fails
with an explicit error instead of a silent downgrade. Server certificate trust for
WinRM/PSRP is supplied as `options.ca_cert` on the record; client-certificate
authentication is not offered.

Dependencies are exact pins in `requirements.txt` and commit-pinned collections
in `requirements.yml`; update them only together with a verified image build.

## Docker Compose prerequisites

The supervisor requires Docker Engine access through /var/run/docker.sock. This
socket is mounted only in the supervisor. Set these installation variables:

- PIPING_DOCKER_GID: socket group ID for the supervisor's supplementary group
- PIPING_ANSIBLE_STATE_ROOT: absolute persistent supervisor-private directory
- PIPING_ANSIBLE_JOB_ROOT: separate absolute directory for per-job workspaces
- PIPING_ANSIBLE_JOB_NETWORK: explicitly authorized job network; empty means none

The state and workspace directories must be owned by UID/GID 10001. Do not put
one inside the other. The host workspace path and supervisor workspace path must
refer to the same directory; the engine resolves bind mounts on the host, not
inside the supervisor container. Each container mounts only its own child.
Do not use the control-plane Compose network as the job network.

The adapter attaches to Docker's multiplexed byte stream before starting the
container. Docker logging is disabled for these containers; the supervisor
redacts known secrets before its durable console storage. Cancellation removes
the entire container and verifies its absence. Docker deadlines depend on the
supervisor; startup reconciliation must stop abandoned jobs before accepting
new work. A supervisor outage is not a daemon-enforced Docker deadline.

## Kubernetes prerequisites

Kubernetes uses native Jobs, not Docker. Configure PIPING_SANDBOX_RUNTIME as
kubernetes, PIPING_SANDBOX_INSTALLATION_ID, PIPING_SANDBOX_WORKSPACE_ROOT and
PIPING_SANDBOX_RUNTIME_DIR, plus:

- PIPING_SANDBOX_KUBERNETES_NAMESPACE: dedicated job namespace
- PIPING_SANDBOX_KUBERNETES_NETWORK_POOL: pool selected by its enforced network policy
- PIPING_SANDBOX_KUBERNETES_POOL_CONTRACT: absolute path of the supervisor-only pool contract file
- PIPING_SANDBOX_KUBERNETES_SERVICE_ACCOUNT: job identity without API permissions
- PIPING_SANDBOX_KUBECONFIG: optional supervisor-only configuration file;
  otherwise kubectl uses the supervisor's in-cluster identity

The trusted execution pool is the only supported PID enforcement mode. Its
contract file is a JSON object with schema_version 1 and exactly these fields:

- pod_pids_limit: finite positive Pod ancestor PID ceiling, at most 4194304
- pool: pool identity, which must equal PIPING_SANDBOX_KUBERNETES_NETWORK_POOL
- node_pool_label_value: value of the pool node label described below
- runtime_class: `null` for the cluster default native handler, or the exact
  RuntimeClass name the acceptance receipt covered
- native_acceptance_sha256: SHA256 of the native acceptance receipt as 64
  lowercase hex digits, not a repeated placeholder

A missing, malformed, unknown or extra field, a nonpositive ceiling, a pool
mismatch, an empty or placeholder acceptance digest, a relative path, a symlink,
a non-regular file, a file above 16 KiB, or group or world *write* permission all
fail configuration. The file is mounted read-only and is not a job mount; a
projected ConfigMap or Secret normally appears group-readable and is accepted,
because the same pool and acceptance values already appear in health output and
job metadata. Only write authority matters: the deployment owner writes the file
and the supervisor never does. Its values are a trust assertion by that owner,
which also owns the acceptance receipt; the supervisor validates their shape and
neither recomputes nor verifies the receipt behind the digest. No active receipt
is distributed here.

Every job Pod carries a mandatory nodeSelector on the fixed label
`piping.space.node-restriction.kubernetes.io/ansible-pool`. The
`node-restriction.kubernetes.io/` prefix segment is reserved for administrators,
so a kubelet cannot label its own Node into the pool; only the infrastructure
owner can place a node there, and only after native acceptance establishes the
ceiling on every eligible node. The label must be removed before the ceiling is
weakened. An unlabelled cluster leaves jobs Pending instead of running them
without the contracted placement.
This protection requires the Node authorizer and NodeRestriction admission.
Native acceptance must also verify that a kubelet identity cannot set or change
the protected pool label.

The contract is also the only runtime selection: the supervisor sets
runtimeClassName from `runtime_class` and rejects a set
PIPING_SANDBOX_KUBERNETES_RUNTIME_CLASS instead of choosing between two inputs.
A RuntimeClass name proves no bound by itself, and a receipt for one handler does
not authorize another, because a different handler can move guest processes
outside the Pod ancestor accounting. The acceptance therefore binds the node set,
runtime, kubelet configuration and cgroup policy together. Changing any of them,
including swapping the RuntimeClass for a VM-based or otherwise different
handler, requires draining the pool or removing its node label and repeating
native acceptance before the label returns.

The supervisor needs namespace-scoped create/get/list/patch/delete access to
Jobs, get/list/watch/delete access to Pods, exec access to Pods, and
create/get/list/delete access to Secrets. The exact `pods/exec` verbs depend on
the deployed kubectl streaming client and API server; they must be reviewed and
granted before deploying this adapter. Job Pods use a distinct label and
service account, automountServiceAccountToken=false and enableServiceLinks=false.
They must not inherit the runner Deployment's network identity or backend access.
Pool identity selects a policy scope; it says nothing about which egress that
policy permits. The installation owner supplies the Cilium/NetworkPolicy rules
and persistent supervisor state. An emptyDir is suitable for job scratch, not
dispatch receipts.

Execution input crosses into a Pod through an immutable, job-scoped Secret and
an init container, then lives in private emptyDir volumes. The serialized input
limit is 700 KiB; larger input is rejected, not truncated or fetched using a
controller credential. The Secret also carries the asserted scope, ceiling, pool,
runtime selection and acceptance identity to both containers. Bootstrap rejects
inconsistent or placeholder metadata and records the tighter of the contracted
ceiling and any observed finite container-local limit. That record is the
materialisation container's own observation: the execution container checks its
shape but does not inherit it as its own bound. The guaranteed ceiling stays the
contracted value. The Pod ancestor `pids.max` is not required to be visible, and
no container-local value proves it either. A requested `spec.limits.pids` that
differs from the contracted ceiling stops creation. No shared host/PVC mount is
required for execution. Kubernetes activeDeadlineSeconds bounds the Job
independently of the supervisor. Cancellation waits for foreground Job/Pod
deletion before removing its Secret.

The execution container's PID1 waits on a private Unix socket in its writable
`/tmp`. Once the Pod is Ready, the supervisor executes the bootstrap in that
container; child output travels only over the exec stream, not container logs.
The wrapper sends only the terminal status to PID1 and PID1 waits for the wrapper
to exit before terminating. The image and runtime must support Python's Linux
`os.pidfd_open`, Unix peer credentials and the corresponding syscalls under the
configured seccomp profile. Verify this and exec-stream completion in the selected
runtime as part of native pool acceptance.

The health endpoint reports the configured contract: container and 256 for
Docker, Pod ancestor and the contracted ceiling for Kubernetes, with the pool
identity, runtime selection and acceptance digest and no credentials or private
paths. These are contracted guarantees, not a fresh kernel observation.

The launcher frames native stdout/stderr bytes for the CRI log transport; the
supervisor decodes stream identity without changing Ansible's stdout callback.
Kubernetes node logs remain sensitive execution data and require corresponding
access and retention policy. An acquisition job emits one canonical project
response document on stdout; the supervisor carries it through the same bounded
framed stream, stops the sandbox, confirms its removal, and only then validates
that shared response and its digest.

Native acceptance is a separate obligation. It must demonstrate the Pod ancestor
cgroup ceiling on every eligible node, refusal of a spawn beyond it, placement
through the protected label, and no escape to a worker outside the pool.
RuntimeClass, configuration parsing, a mocked engine and these source checks
establish none of that.

## Git acquisition and console

Acquisition uses a private HOME, no system/global Git configuration, disabled
hooks and inherited credential helpers, strict TLS checks and no HTTP redirects.
HTTPS authentication uses a credential helper over stdin/stdout, scoped to the
exact protocol/host/port/path. SSH requires trusted route configuration and
stored host pins; changed keys fail. Encrypted SSH keys needing a prompt,
unresolved submodules and LFS pointer files are refused explicitly.

Git credentials, .git and controller files are not execution snapshot entries.
The acquisition job emits one bounded canonical response through stdout. After
the sandbox and descendants have stopped, the supervisor validates identity,
entries and the shared tree digest. It does not collect a mutable host checkout.
The collector bounds file count and bytes and refuses escaping links.
Project-writable known_hosts files and callbacks never enroll host trust.

Console collection preserves ANSI and arbitrary bytes, assigns ordered sequence
numbers and bounds bytes/chunks. Exact known-secret redaction spans input chunk
boundaries before product persistence. It cannot hide deliberate exfiltration,
unknown secrets or transformed values. Reconnection reads persisted sequences,
not a second execution or a fresh runtime log cursor.

Authenticated dispatches return a durable receipt before launch. Console callbacks
use the shared byte/stream/sequence contract and retry the same stored prefix until
the backend acknowledges it; completion names the final prefix and is sent only
after the structured notification batch.

A trusted aggregate callback plugin is loaded from the read-only job controller
directory on top of the effective Ansible configuration. The configured stdout
callback and project callback paths are preserved; nothing parses stdout. The
plugin writes a bounded per-host stats spool and a bounded per-task result spool
(host alias, task name, result only, never module results or arguments). The
trusted bootstrap seals them into one bounded `result.ndjson`; the supervisor
reads it after the sandbox has terminated (Docker host workspace) or from a
private `notify` frame on the job exec stream (Kubernetes). The data is untrusted:
duplicate stats accumulate, out-of-dispatch hosts and malformed records are
refused, known secrets are redacted, and run, host and batch identities are
supervisor-derived. One idempotent `events/batch` is delivered before completion.

Native exit status is authoritative for the group process, including launches
without Piping target rows, and a nonzero exit is not per-host evidence. A host
with a valid terminal stats summary keeps its own succeeded/failed verdict even
when the group failed or was canceled. A host without one is `unreported`;
cancellation and deadline are lifecycle facts only for targets that never
produced a verdict. If the backend permanently refuses the event batch, all
observed verdicts and the group outcome are kept and the loss is appended to the
completion's operator-facing error message; authentication/identity refusals keep
the outbox pending and never bless a completion without the required data.

One supervisor exclusively locks each persistent state root. Startup stops all
owned abandoned sandboxes before accepting work and never replays a launch with
credentials omitted from its receipt. Receipts, acquisition results and console
records do not expire automatically: a previously accepted dispatch must never
be reported as absent. SIGINT and SIGTERM stop active work before exit;
an unclean outage still requires startup reconciliation.

All HTTP routes, including health, require the shared runner bearer token. Use
the binary's `--health-check` mode for container probes; it reads the configured
secret from the supervisor environment without putting it in argv.

## Checks

Run the package's product tests when the Rust build slot is available:


    cargo test -p piping-ansible-runner

Native runtime acceptance must also exercise the selected adapter, image digest,
network policy, cancellation and restart behavior. Formatting and a mocked
engine do not establish isolation.
