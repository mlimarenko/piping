use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use super::{MAX_PAYLOAD, TreeEntry, encode_tree};
use crate::sandbox::{SandboxSpec, SandboxState};

const BOOTSTRAP: &str = include_str!("../../docker/kube_bootstrap.py");
const CONTROL_LIMIT: usize = 2 * 1024 * 1024;
/// Reserved segment `node-restriction.kubernetes.io/`; kubelets cannot set or
/// modify such a label, so a node cannot select itself into the execution pool.
/// Only the infrastructure owner may set this label, and only after native
/// acceptance proves the PID ceiling on every eligible node.
const NODE_POOL_LABEL_KEY: &str = "piping.space.node-restriction.kubernetes.io/ansible-pool";
/// Supervisor-only pool contract bound, far below the 700 KiB Secret limit.
const CONTRACT_LIMIT: u64 = 16 * 1024;
/// Kernel `pid_max` maximum; a ceiling above it cannot be finite in practice.
const MAX_PIDS: u32 = 4_194_304;
const CONTRACT_FIELDS: [&str; 6] = [
    "schema_version",
    "pool",
    "pod_pids_limit",
    "node_pool_label_value",
    "runtime_class",
    "native_acceptance_sha256",
];

/// Deployment-authority assertion about the execution pool. It is never
/// self-verifying: the native acceptance digest identifies the receipt that
/// established the kubelet Pod ceiling, placement and runtime, it does not
/// prove them.
#[derive(Clone, Debug)]
struct PoolContract {
    pool: String,
    pids: u32,
    node_pool_label_value: String,
    /// `None` is the explicit cluster default handler, never an absent choice.
    runtime_class: Option<String>,
    native_acceptance_sha256: String,
}

impl PoolContract {
    fn load(path: &Path, configured_pool: &str) -> anyhow::Result<Self> {
        let bytes = read_contract_file(path)?;
        let value: Value =
            serde_json::from_slice(&bytes).context("pool contract is not valid JSON")?;
        let object = value
            .as_object()
            .context("pool contract is not a JSON object")?;
        let allowed: BTreeSet<&str> = CONTRACT_FIELDS.into_iter().collect();
        if object.len() != CONTRACT_FIELDS.len()
            || !object.keys().all(|key| allowed.contains(key.as_str()))
        {
            bail!("pool contract fields do not match schema version 1");
        }
        if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
            bail!("pool contract schema version is not supported");
        }
        let pids = object
            .get("pod_pids_limit")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| (1..=MAX_PIDS).contains(value))
            .context("pool contract PID ceiling is not a positive finite limit")?;
        let pool = object
            .get("pool")
            .and_then(Value::as_str)
            .context("pool contract identity is missing")?;
        if !valid_identifier(pool) {
            bail!("pool contract identity is malformed");
        }
        if pool != configured_pool {
            // Matching identity selects one NetworkPolicy scope; it says nothing
            // about which egress that policy actually permits.
            bail!("pool contract identity does not match the configured network pool");
        }
        let node_pool_label_value = object
            .get("node_pool_label_value")
            .and_then(Value::as_str)
            .context("pool contract node label value is missing")?;
        if !valid_label_value(node_pool_label_value) {
            bail!("pool contract node label value is malformed");
        }
        // One canonical runtime selection. A receipt for one handler does not
        // authorize another: a swapped RuntimeClass can move processes out of
        // the Pod ancestor accounting this contract asserts.
        let runtime_class = match object.get("runtime_class") {
            Some(Value::Null) => None,
            Some(Value::String(value)) if valid_runtime_class(value) => Some(value.clone()),
            Some(Value::String(_)) => bail!("pool contract runtime selection is malformed"),
            None | Some(_) => bail!("pool contract runtime selection is missing or malformed"),
        };
        let native_acceptance_sha256 = object
            .get("native_acceptance_sha256")
            .and_then(Value::as_str)
            .context("pool contract native acceptance digest is missing")?;
        if !valid_digest(native_acceptance_sha256) {
            bail!("pool contract native acceptance digest is malformed or is a placeholder");
        }
        Ok(Self {
            pool: pool.to_owned(),
            pids,
            node_pool_label_value: node_pool_label_value.to_owned(),
            runtime_class,
            native_acceptance_sha256: native_acceptance_sha256.to_owned(),
        })
    }

    /// Metadata carried in the job input Secret and reported by health checks.
    fn metadata(&self) -> Value {
        json!({
            "schema_version": 1,
            "scope": "pod_ancestor",
            "pids": self.pids,
            "pool": self.pool,
            "node_selector_key": NODE_POOL_LABEL_KEY,
            "node_selector_value": self.node_pool_label_value,
            "runtime_class": self.runtime_class,
            "native_acceptance_sha256": self.native_acceptance_sha256,
        })
    }
}

fn read_contract_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    if !path.is_absolute() {
        bail!("pool contract path must be absolute");
    }
    let link = fs::symlink_metadata(path).context("pool contract is unavailable")?;
    if link.file_type().is_symlink() || !link.is_file() {
        bail!("pool contract must be a regular file");
    }
    if link.permissions().mode() & 0o022 != 0 {
        // The file is mounted read-only and is not a secret: health reports and
        // job metadata carry the same values. Only group or world *write*
        // authority could let another identity change the trusted contract.
        bail!("pool contract must not be writable outside its owner");
    }
    if link.len() == 0 || link.len() > CONTRACT_LIMIT {
        bail!("pool contract size is outside its bound");
    }
    let mut file = fs::File::open(path).context("pool contract is unavailable")?;
    // Compare the descriptor with the inspected path so a swapped symlink cannot
    // redirect the trusted read after the initial check.
    let opened = file.metadata().context("pool contract is unavailable")?;
    if !opened.is_file() || opened.dev() != link.dev() || opened.ino() != link.ino() {
        bail!("pool contract changed while it was opened");
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(CONTRACT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .context("pool contract is unreadable")?;
    if bytes.len() as u64 > CONTRACT_LIMIT {
        bail!("pool contract size is outside its bound");
    }
    Ok(bytes)
}

fn field(name: &str) -> anyhow::Result<String> {
    let value = std::env::var(name).with_context(|| format!("missing {name}"))?;
    if !valid_identifier(&value) {
        bail!("invalid Kubernetes adapter identifier: {name}");
    }
    Ok(value)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_label_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && matches!(bytes.first(), Some(byte) if byte.is_ascii_alphanumeric())
        && matches!(bytes.last(), Some(byte) if byte.is_ascii_alphanumeric())
}

/// RuntimeClass names are DNS-1123 subdomains; the name never proves a bound.
fn valid_runtime_class(value: &str) -> bool {
    let bytes = value.as_bytes();
    !value.is_empty()
        && value.len() <= 253
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && matches!(bytes.first(), Some(byte) if byte.is_ascii_alphanumeric())
        && matches!(bytes.last(), Some(byte) if byte.is_ascii_alphanumeric())
}

fn valid_digest(value: &str) -> bool {
    let mut rest = value.bytes();
    let Some(first) = rest.next() else {
        return false;
    };
    // A repeated single character is a placeholder, not a receipt identity.
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && rest.any(|byte| byte != first)
}

#[derive(Clone)]
pub(crate) struct Kubernetes {
    namespace: String,
    installation: String,
    pool: String,
    service_account: String,
    runtime_dir: PathBuf,
    kubeconfig: PathBuf,
    cluster_env: BTreeMap<String, OsString>,
    contract: PoolContract,
}

impl Kubernetes {
    pub(crate) fn from_env(runtime_dir: &Path) -> anyhow::Result<Self> {
        let namespace = field("PIPING_SANDBOX_KUBERNETES_NAMESPACE")?;
        let installation = field("PIPING_SANDBOX_INSTALLATION_ID")?;
        let pool = field("PIPING_SANDBOX_KUBERNETES_NETWORK_POOL")?;
        // The pool contract owns the runtime selection. A second selector input
        // could authorize a handler the acceptance receipt never covered.
        if std::env::var_os("PIPING_SANDBOX_KUBERNETES_RUNTIME_CLASS")
            .is_some_and(|v| !v.is_empty())
        {
            bail!(
                "PIPING_SANDBOX_KUBERNETES_RUNTIME_CLASS is not supported; declare runtime_class in the pool contract"
            );
        }
        let service_account = field("PIPING_SANDBOX_KUBERNETES_SERVICE_ACCOUNT")?;
        let contract_path = PathBuf::from(
            std::env::var_os("PIPING_SANDBOX_KUBERNETES_POOL_CONTRACT")
                .context("missing PIPING_SANDBOX_KUBERNETES_POOL_CONTRACT")?,
        );
        let contract = PoolContract::load(&contract_path, &pool)?;
        let kubeconfig = std::env::var_os("PIPING_SANDBOX_KUBECONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/dev/null"));
        if !kubeconfig.is_absolute() {
            bail!("Kubernetes configuration path must be absolute");
        }
        // Only these client-go discovery variables reach kubectl. No process
        // environment, proxy setting or backend credential reaches the job Pod.
        let cluster_env = ["KUBERNETES_SERVICE_HOST", "KUBERNETES_SERVICE_PORT"]
            .into_iter()
            .filter_map(|name| std::env::var_os(name).map(|value| (name.to_owned(), value)))
            .collect();
        Ok(Self {
            namespace,
            installation,
            pool,
            service_account,
            runtime_dir: runtime_dir.to_owned(),
            kubeconfig,
            cluster_env,
            contract,
        })
    }

    /// The contracted Pod ancestor ceiling; the caller must request exactly it.
    pub(crate) fn process_limit(&self) -> u32 {
        self.contract.pids
    }

    /// Resource scope reported by the supervisor health endpoint.
    pub(crate) fn resource_contract(&self) -> Value {
        let mut contract = self.contract.metadata();
        if let Some(object) = contract.as_object_mut() {
            object.insert("runtime".into(), Value::from("kubernetes"));
        }
        contract
    }

    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new("/usr/bin/kubectl");
        command
            .args(["--namespace", &self.namespace, "--request-timeout=30s"])
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.runtime_dir)
            .env("KUBECONFIG", &self.kubeconfig)
            .envs(&self.cluster_env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    async fn call(&self, args: Vec<String>, input: Option<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
        let mut command = self.command(&args);
        if input.is_some() {
            command.stdin(Stdio::piped());
        }
        let child = command.spawn().context("start Kubernetes adapter CLI")?;
        tokio::time::timeout(
            Duration::from_secs(90),
            exchange(child, input, CONTROL_LIMIT),
        )
        .await
        .context("Kubernetes adapter operation timed out")?
    }

    fn selector(&self) -> String {
        format!(
            "piping.owner=ansible-job,piping.installation={}",
            self.installation
        )
    }

    fn labels(&self, spec: &SandboxSpec) -> Value {
        json!({"piping.owner":"ansible-job","piping.installation":self.installation,"piping.dispatch":spec.dispatch_id,"piping.phase":spec.phase,"piping.network-pool":self.pool})
    }

    /// Input Secret payload. The PID contract travels with the immutable job
    /// input so both containers see the same asserted scope and ceiling.
    fn input_payload(&self, spec: &SandboxSpec, tree: &[u8]) -> anyhow::Result<String> {
        let mut environment = spec.env.clone();
        environment.insert("HOME".into(), "/home/runner".into());
        environment.insert("TMPDIR".into(), "/tmp".into());
        let payload = json!({
            "tree": serde_json::from_slice::<Value>(tree)?, "argv":spec.argv,
            "env":environment, "workdir":spec.workdir, "pids":spec.limits.pids,
            "output_limit":spec.limits.output_bytes, "contract":self.contract.metadata()
        });
        let payload = serde_json::to_string(&payload)?;
        if payload.len() > MAX_PAYLOAD {
            bail!("job exceeds the Kubernetes Secret transport limit");
        }
        Ok(payload)
    }

    fn job_manifest(&self, spec: &SandboxSpec, name: &str) -> Value {
        let labels = self.labels(spec);
        let security = json!({"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}});
        let resources = json!({"requests":{"cpu":"100m","memory":"64Mi"},"limits":{"cpu":format!("{}m",spec.limits.cpus_millis),"memory":spec.limits.memory_bytes.to_string()}});
        let mut node_selector = serde_json::Map::new();
        node_selector.insert(
            NODE_POOL_LABEL_KEY.to_owned(),
            Value::from(self.contract.node_pool_label_value.clone()),
        );
        let mut job = json!({
            "apiVersion":"batch/v1","kind":"Job","metadata":{"name":name,"labels":labels},
            "spec":{"suspend":true,"backoffLimit":0,"activeDeadlineSeconds":spec.limits.timeout.as_secs(),"ttlSecondsAfterFinished":86400,
                "template":{"metadata":{"labels":labels},"spec":{
                    "restartPolicy":"Never","automountServiceAccountToken":false,"serviceAccountName":self.service_account,
                    "nodeSelector":node_selector,"enableServiceLinks":false,"hostNetwork":false,"hostPID":false,"hostIPC":false,
                    "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,"fsGroup":10001,"seccompProfile":{"type":"RuntimeDefault"}},
                    "initContainers":[{"name":"materialize","image":spec.image,"imagePullPolicy":"IfNotPresent","securityContext":security,"resources":resources,
                        "command":["python3","-I","-c",BOOTSTRAP,"unpack"],
                        "volumeMounts":[{"name":"workspace","mountPath":"/job"},{"name":"input","mountPath":"/bootstrap","readOnly":true}]}],
                    "containers":[{"name":"execute","image":spec.image,"imagePullPolicy":"IfNotPresent","securityContext":security,"resources":resources,
                        "command":["python3","-I","-c",BOOTSTRAP,"hold"],
                        "readinessProbe":{"exec":{"command":["python3","-I","-c","import os; import sys; sys.exit(0 if os.path.isfile('/tmp/.piping-control.ready') else 1)"]},"periodSeconds":1},
                        "volumeMounts":[{"name":"workspace","mountPath":"/job"},{"name":"workspace","mountPath":"/job/controller","subPath":"controller","readOnly":true},{"name":"home","mountPath":"/home/runner"},{"name":"tmp","mountPath":"/tmp"}]}],
                    "volumes":[{"name":"input","secret":{"secretName":name,"defaultMode":288}},
                        {"name":"workspace","emptyDir":{"sizeLimit":"256Mi"}},
                        {"name":"home","emptyDir":{"sizeLimit":"128Mi"}},
                        {"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":"64Mi"}}]
                }}}
        });
        let pod = job
            .pointer_mut("/spec/template/spec")
            .and_then(Value::as_object_mut);
        if let (Some(runtime_class), Some(pod)) = (&self.contract.runtime_class, pod) {
            pod.insert(
                "runtimeClassName".into(),
                Value::from(runtime_class.clone()),
            );
        }
        job
    }

    pub(crate) async fn create(&self, spec: &SandboxSpec) -> anyhow::Result<String> {
        if spec.limits.pids != self.contract.pids {
            // The requested bound must be the contracted Pod ancestor ceiling.
            bail!("requested PID limit does not match the trusted pool contract");
        }
        let name = format!("piping-{}-{}", spec.dispatch_id, spec.phase);
        if name.len() > 63 {
            bail!("Kubernetes job name exceeds limit");
        }
        let tree = collect_tree(&spec.workspace)?;
        let bytes = encode_tree(&tree).map_err(anyhow::Error::msg)?;
        let payload = self.input_payload(spec, &bytes)?;
        let secret = json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":name,"labels":self.labels(spec)},"type":"Opaque","immutable":true,"stringData":{"payload.json":payload}});
        self.call(
            vec!["create".into(), "-f".into(), "-".into()],
            Some(serde_json::to_vec(&secret)?),
        )
        .await?;
        let job = self.job_manifest(spec, &name);
        let result = self
            .call(
                vec!["create".into(), "-f".into(), "-".into()],
                Some(serde_json::to_vec(&job)?),
            )
            .await;
        if result.is_err() {
            // A lost create acknowledgement may still have created the Job. Keep
            // its input Secret until label-based restart reconciliation can stop it.
            return result.map(|_| name);
        }
        Ok(name)
    }

    async fn owned(&self, kind: &str, name: &str) -> anyhow::Result<Option<Value>> {
        if !valid_name(name) {
            bail!("invalid Kubernetes resource identity");
        }
        let bytes = self
            .call(
                vec![
                    "get".into(),
                    kind.into(),
                    name.into(),
                    "--ignore-not-found".into(),
                    "-o".into(),
                    "json".into(),
                ],
                None,
            )
            .await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let object: Value = serde_json::from_slice(&bytes)?;
        if object
            .pointer("/metadata/labels/piping.owner")
            .and_then(Value::as_str)
            != Some("ansible-job")
            || object
                .pointer("/metadata/labels/piping.installation")
                .and_then(Value::as_str)
                != Some(self.installation.as_str())
        {
            bail!("Kubernetes resource is not owned by this supervisor");
        }
        Ok(Some(object))
    }

    pub(crate) async fn start(&self, name: &str) -> anyhow::Result<()> {
        if self.owned("job", name).await?.is_none() {
            bail!("Kubernetes job no longer exists");
        }
        self.call(
            vec![
                "patch".into(),
                "job".into(),
                name.into(),
                "--type=merge".into(),
                "--patch".into(),
                r#"{"spec":{"suspend":false}}"#.into(),
            ],
            None,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn observe(&self, name: &str) -> anyhow::Result<SandboxState> {
        let job = self
            .owned("job", name)
            .await?
            .context("Kubernetes job no longer exists")?;
        if job.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true) {
            return Ok(SandboxState::Created);
        }
        let selector = format!("{},job-name={name}", self.selector());
        let bytes = self
            .call(
                vec![
                    "get".into(),
                    "pods".into(),
                    "-l".into(),
                    selector,
                    "-o".into(),
                    "json".into(),
                ],
                None,
            )
            .await?;
        let pods: Value = serde_json::from_slice(&bytes)?;
        for pod in pods["items"]
            .as_array()
            .context("invalid Kubernetes pod listing")?
        {
            if let Some(code) = pod
                .pointer("/status/containerStatuses/0/state/terminated/exitCode")
                .and_then(Value::as_i64)
            {
                return Ok(SandboxState::Exited(i32::try_from(code)?));
            }
            if let Some(code) = pod
                .pointer("/status/initContainerStatuses/0/state/terminated/exitCode")
                .and_then(Value::as_i64)
                .filter(|code| *code != 0)
            {
                return Ok(SandboxState::Exited(i32::try_from(code)?));
            }
        }
        if job
            .pointer("/status/failed")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            return Ok(SandboxState::Exited(125));
        }
        Ok(SandboxState::Running)
    }

    pub(crate) async fn exec_stream(&self, name: &str) -> anyhow::Result<Command> {
        let job = self
            .owned("job", name)
            .await?
            .context("Kubernetes job no longer exists")?;
        let uid = job
            .pointer("/metadata/uid")
            .and_then(Value::as_str)
            .context("Kubernetes job has no UID")?;
        let selector = format!("{},job-name={name}", self.selector());
        let mut watch = self
            .command(&[
                "--request-timeout=0".into(),
                "get".into(),
                "pods".into(),
                "-l".into(),
                selector,
                "--watch".into(),
                "-o".into(),
                "name".into(),
            ])
            .spawn()
            .context("start Kubernetes pod watch")?;
        let output = watch.stdout.take().context("missing pod watch output")?;
        let mut lines = BufReader::new(output.take(1025)).lines();
        let line = tokio::time::timeout(Duration::from_secs(60), lines.next_line())
            .await
            .context("Kubernetes pod wait timed out")??
            .context("Kubernetes pod watch ended")?;
        watch.kill().await.context("stop Kubernetes pod watch")?;
        let pod_name = line
            .strip_prefix("pod/")
            .context("invalid Kubernetes pod identity")?;
        let pod = self
            .owned("pod", pod_name)
            .await?
            .context("Kubernetes pod disappeared")?;
        let owned_by_job = pod
            .pointer("/metadata/ownerReferences")
            .and_then(Value::as_array)
            .is_some_and(|owners| {
                owners.iter().any(|owner| {
                    owner.get("uid").and_then(Value::as_str) == Some(uid)
                        && owner.get("kind").and_then(Value::as_str) == Some("Job")
                })
            });
        if !owned_by_job {
            bail!("Kubernetes pod is not owned by the job");
        }
        self.call(
            vec![
                "wait".into(),
                "--for=condition=Ready".into(),
                format!("pod/{pod_name}"),
                "--timeout=60s".into(),
            ],
            None,
        )
        .await?;
        Ok(self.exec_command(pod_name))
    }

    fn exec_command(&self, pod_name: &str) -> Command {
        self.command(&[
            "--request-timeout=0".into(),
            "exec".into(),
            pod_name.into(),
            "--container=execute".into(),
            "--".into(),
            "python3".into(),
            "-I".into(),
            "-c".into(),
            BOOTSTRAP.into(),
            "run".into(),
        ])
    }

    pub(crate) async fn terminate(&self, name: &str) -> anyhow::Result<()> {
        if !valid_name(name) {
            bail!("invalid Kubernetes resource identity");
        }
        if self.owned("job", name).await?.is_some() {
            self.call(
                vec![
                    "delete".into(),
                    "job".into(),
                    name.into(),
                    "--cascade=foreground".into(),
                    "--wait=true".into(),
                    "--timeout=60s".into(),
                    "--ignore-not-found".into(),
                ],
                None,
            )
            .await?;
        }
        // Scoped orphan cleanup also covers an interrupted create/delete call.
        self.call(
            vec![
                "delete".into(),
                "pod".into(),
                "-l".into(),
                format!("{},job-name={name}", self.selector()),
                "--wait=true".into(),
                "--timeout=60s".into(),
            ],
            None,
        )
        .await?;
        if self.owned("secret", name).await?.is_some() {
            self.call(
                vec![
                    "delete".into(),
                    "secret".into(),
                    name.into(),
                    "--wait=true".into(),
                    "--timeout=30s".into(),
                    "--ignore-not-found".into(),
                ],
                None,
            )
            .await?;
        }
        if self.owned("job", name).await?.is_some() {
            bail!("Kubernetes job deletion is not confirmed");
        }
        Ok(())
    }

    pub(crate) async fn reconcile(&self) -> anyhow::Result<Vec<String>> {
        let bytes = self
            .call(
                vec![
                    "get".into(),
                    "jobs,secrets".into(),
                    "-l".into(),
                    self.selector(),
                    "-o".into(),
                    "json".into(),
                ],
                None,
            )
            .await?;
        let list: Value = serde_json::from_slice(&bytes)?;
        let mut names = BTreeSet::new();
        for item in list["items"]
            .as_array()
            .context("invalid Kubernetes resource listing")?
        {
            let name = item
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .context("Kubernetes resource has no name")?;
            if !valid_name(name) {
                bail!("invalid Kubernetes resource name");
            }
            names.insert(name.to_owned());
        }
        Ok(names.into_iter().collect())
    }
}

/// One adapter CLI exchange: write the optional payload, close the input pipe,
/// and bound both response streams.
///
/// The input future owns `ChildStdin` because tokio's `ChildStdin::shutdown` is
/// a no-op for pipes: the write half closes only when the handle is dropped.
/// Retaining it in the caller while awaiting responses would keep the CLI
/// blocked reading stdin until the outer deadline expires.
async fn exchange(
    mut child: Child,
    input: Option<Vec<u8>>,
    control_limit: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut stdout = child.stdout.take().context("missing Kubernetes stdout")?;
    let mut stderr = child.stderr.take().context("missing Kubernetes stderr")?;
    let stdin = child.stdin.take();
    let write = async move {
        if let Some(bytes) = input {
            let mut pipe = stdin.context("missing Kubernetes stdin")?;
            pipe.write_all(&bytes).await?;
            pipe.flush().await?;
            // Drop the write half so the CLI can finish reading its input.
        }
        Ok::<_, anyhow::Error>(())
    };
    let read_out = async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(control_limit as u64 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > control_limit {
            bail!("Kubernetes response exceeded its bound");
        }
        Ok::<_, anyhow::Error>(bytes)
    };
    let read_err = async {
        let mut bytes = Vec::new();
        (&mut stderr).take(65537).read_to_end(&mut bytes).await?;
        if bytes.len() > 65536 {
            bail!("Kubernetes diagnostic exceeded its bound");
        }
        Ok::<_, anyhow::Error>(())
    };
    let (_, bytes, _) = tokio::try_join!(write, read_out, read_err)?;
    if !child.wait().await?.success() {
        // kubectl errors can include Secret data or private project paths.
        bail!("Kubernetes adapter operation failed");
    }
    Ok(bytes)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn collect_tree(root: &Path) -> anyhow::Result<Vec<TreeEntry>> {
    fn visit(
        root: &Path,
        directory: &Path,
        entries: &mut Vec<TreeEntry>,
        bytes: &mut u64,
    ) -> anyhow::Result<()> {
        let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            if entries.len() >= 4096 {
                bail!("too many Kubernetes input entries");
            }
            let path = child.path();
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .context("non UTF-8 input path")?
                .to_owned();
            if relative == ".piping-launch.json" {
                bail!("reserved job transport path");
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                entries.push(TreeEntry::Directory {
                    path: relative,
                    mode: metadata.permissions().mode() & 0o777,
                });
                visit(root, &path, entries, bytes)?;
            } else if metadata.is_file() {
                *bytes = bytes
                    .checked_add(metadata.len())
                    .context("Kubernetes input too large")?;
                if *bytes > MAX_PAYLOAD as u64 {
                    bail!("Kubernetes input exceeds Secret transport limit");
                }
                entries.push(TreeEntry::File {
                    path: relative,
                    mode: metadata.permissions().mode() & 0o777,
                    bytes: fs::read(&path)?,
                });
            } else if metadata.file_type().is_symlink() {
                entries.push(TreeEntry::Symlink {
                    path: relative,
                    target: fs::read_link(&path)?
                        .to_str()
                        .context("non UTF-8 symlink")?
                        .to_owned(),
                });
            } else {
                bail!("unsupported Kubernetes input file type");
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries, &mut 0)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCEPTANCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn valid_contract() -> String {
        format!(
            r#"{{"schema_version":1,"pool":"piping-app-sandbox","pod_pids_limit":2048,"node_pool_label_value":"ansible-pool","runtime_class":null,"native_acceptance_sha256":"{ACCEPTANCE}"}}"#
        )
    }

    fn load(text: &str) -> anyhow::Result<PoolContract> {
        let root = tempfile::tempdir().expect("private contract directory");
        let path = root.path().join("pool.json");
        fs::write(&path, text).expect("write contract");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private contract");
        PoolContract::load(&path, "piping-app-sandbox")
    }

    #[test]
    fn contract_rejects_missing_unknown_and_placeholder_values() {
        let valid = valid_contract();
        assert_eq!(load(&valid).expect("valid contract").pids, 2048);
        for broken in [
            valid.replace("\"schema_version\":1,", ""),
            valid.replace("\"schema_version\":1", "\"schema_version\":2"),
            valid.replace(
                "\"schema_version\":1,",
                "\"schema_version\":1,\"extra\":true,",
            ),
            valid.replace("\"pod_pids_limit\":2048,", ""),
            valid.replace("\"pod_pids_limit\":2048", "\"pod_pids_limit\":0"),
            valid.replace("piping-app-sandbox", "other-pool"),
            valid.replace(
                "\"node_pool_label_value\":\"ansible-pool\"",
                "\"node_pool_label_value\":\"Bad Value\"",
            ),
            valid.replace(",\"runtime_class\":null", ""),
            valid.replace("\"runtime_class\":null", "\"runtime_class\":\"Bad Class\""),
            valid.replace("\"runtime_class\":null", "\"runtime_class\":\"\""),
            valid.replace("\"runtime_class\":null", "\"runtime_class\":1"),
            valid.replace(ACCEPTANCE, &"0".repeat(64)),
            valid.replace(ACCEPTANCE, &ACCEPTANCE.to_uppercase()),
            valid.replace(ACCEPTANCE, "abc"),
        ] {
            assert!(load(&broken).is_err(), "{broken} must be rejected");
        }
    }

    #[test]
    fn contract_file_must_be_regular_and_not_writable_by_others() {
        let root = tempfile::tempdir().expect("private contract directory");
        let path = root.path().join("pool.json");
        fs::write(&path, valid_contract()).expect("write contract");
        // A projected ConfigMap or Secret normally appears group-readable.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("group-readable mode");
        assert!(PoolContract::load(&path, "piping-app-sandbox").is_ok());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("read-only shared mode");
        assert!(PoolContract::load(&path, "piping-app-sandbox").is_ok());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).expect("group-writable mode");
        assert!(PoolContract::load(&path, "piping-app-sandbox").is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o602)).expect("world-writable mode");
        assert!(PoolContract::load(&path, "piping-app-sandbox").is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private mode");
        assert!(PoolContract::load(&path, "piping-app-sandbox").is_ok());
        let link = root.path().join("pool-link.json");
        std::os::unix::fs::symlink(&path, &link).expect("symlink");
        assert!(PoolContract::load(&link, "piping-app-sandbox").is_err());
        assert!(PoolContract::load(Path::new("pool.json"), "piping-app-sandbox").is_err());
        assert!(PoolContract::load(root.path(), "piping-app-sandbox").is_err());
    }

    fn pool_adapter(runtime_class: Option<&str>) -> Kubernetes {
        Kubernetes {
            namespace: "piping-app-jobs".into(),
            installation: "piping-eu".into(),
            pool: "piping-app-sandbox".into(),
            service_account: "ansible-job".into(),
            runtime_dir: PathBuf::from("/run/piping"),
            kubeconfig: PathBuf::from("/dev/null"),
            cluster_env: BTreeMap::new(),
            contract: PoolContract {
                pool: "piping-app-sandbox".into(),
                pids: 2048,
                node_pool_label_value: "ansible-pool".into(),
                runtime_class: runtime_class.map(str::to_owned),
                native_acceptance_sha256: ACCEPTANCE.into(),
            },
        }
    }

    fn sandbox_spec() -> SandboxSpec {
        SandboxSpec {
            dispatch_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
            phase: "execute".into(),
            image: format!("example/image@sha256:{}", "a".repeat(64)),
            workspace: PathBuf::from("/job"),
            argv: vec!["ansible-playbook".into()],
            env: BTreeMap::new(),
            workdir: PathBuf::from("/job"),
            limits: crate::sandbox::SandboxLimits {
                memory_bytes: 8 * 1024 * 1024,
                pids: 2048,
                cpus_millis: 500,
                timeout: Duration::from_secs(60),
                output_bytes: 1024,
            },
        }
    }

    #[test]
    fn job_manifest_pins_the_protected_pool_label() {
        let spec = sandbox_spec();
        let job = pool_adapter(None).job_manifest(&spec, "piping-execute");
        let pod = job.pointer("/spec/template/spec").expect("pod template");
        let selector = pod.get("nodeSelector").expect("mandatory node selector");
        assert_eq!(selector.as_object().map(|object| object.len()), Some(1));
        assert_eq!(
            selector.get(NODE_POOL_LABEL_KEY).and_then(Value::as_str),
            Some("ansible-pool")
        );
        assert!(pod.get("runtimeClassName").is_none());
        let job = pool_adapter(Some("piping-app-sandbox")).job_manifest(&spec, "piping-execute");
        let pod = job.pointer("/spec/template/spec").expect("pod template");
        assert_eq!(
            pod.get("runtimeClassName").and_then(Value::as_str),
            Some("piping-app-sandbox")
        );
    }

    #[test]
    fn exec_stream_targets_selected_container() {
        let adapter = pool_adapter(None);
        let command = adapter.exec_command("selected-pod");
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let exec = args
            .iter()
            .position(|arg| arg == "exec")
            .expect("exec command");
        assert_eq!(
            &args[exec + 1..exec + 4],
            ["selected-pod", "--container=execute", "--"]
        );
        assert_eq!(args.last().map(String::as_str), Some("run"));
    }

    #[test]
    fn input_supplies_the_scope_and_ceiling_to_the_job() {
        let adapter = pool_adapter(None);
        let payload: Value = serde_json::from_str(
            &adapter
                .input_payload(&sandbox_spec(), b"[]")
                .expect("payload"),
        )
        .expect("json payload");
        assert_eq!(payload["pids"].as_u64(), Some(2048));
        let contract = &payload["contract"];
        assert_eq!(contract["schema_version"].as_u64(), Some(1));
        assert_eq!(contract["scope"], "pod_ancestor");
        assert_eq!(contract["pids"].as_u64(), Some(2048));
        assert_eq!(contract["pool"], "piping-app-sandbox");
        assert_eq!(contract["node_selector_key"], NODE_POOL_LABEL_KEY);
        assert_eq!(contract["node_selector_value"], "ansible-pool");
        assert!(contract["runtime_class"].is_null());
        assert_eq!(contract["native_acceptance_sha256"], ACCEPTANCE);
        assert_eq!(adapter.process_limit(), 2048);
        let reported = adapter.resource_contract();
        assert_eq!(reported["runtime"], "kubernetes");
        assert_eq!(reported["scope"], contract["scope"]);
        assert_eq!(reported["pids"], contract["pids"]);
        assert_eq!(contract["runtime_class"], reported["runtime_class"]);
        assert_eq!(
            reported["native_acceptance_sha256"],
            contract["native_acceptance_sha256"]
        );
    }

    fn test_child(script: &str) -> Child {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn test child")
    }

    async fn wait_for_exit(pid: u32) {
        loop {
            match fs::read_to_string(format!("/proc/{pid}/stat")) {
                Err(_) => return,
                Ok(stat) => {
                    let state = stat
                        .rsplit_once(')')
                        .and_then(|(_, rest)| rest.split_whitespace().next());
                    if state == Some("Z") {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn input_task_closes_stdin_so_the_child_observes_eof() {
        // `wc -c` exits only after stdin reaches EOF, so a retained write end
        // keeps this exchange pending for the whole test deadline.
        let child = test_child("exec wc -c");
        let payload = vec![b'x'; 64 * 1024 + 7];
        let expected_bytes = payload.len();
        let stdout = tokio::time::timeout(
            Duration::from_secs(10),
            exchange(child, Some(payload), 1024),
        )
        .await
        .expect("stdin must reach EOF")
        .expect("exchange");
        let received_bytes = String::from_utf8(stdout)
            .expect("byte count output")
            .trim()
            .parse::<usize>()
            .expect("numeric byte count");
        assert_eq!(received_bytes, expected_bytes);
    }

    #[tokio::test]
    async fn no_input_still_collects_the_child_response() {
        let child = test_child("printf ready");
        let stdout = tokio::time::timeout(Duration::from_secs(10), exchange(child, None, 1024))
            .await
            .expect("exchange must finish")
            .expect("exchange");
        assert_eq!(stdout, b"ready");
    }

    #[tokio::test]
    async fn input_without_a_pipe_is_rejected() {
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn test child");
        let error = exchange(child, Some(vec![1]), 1024)
            .await
            .expect_err("missing stdin pipe");
        assert!(
            error.to_string().contains("missing Kubernetes stdin"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn response_above_the_control_bound_fails() {
        let child = test_child("head -c 4096 /dev/zero");
        let error = exchange(child, None, 1024)
            .await
            .expect_err("response must be bounded");
        assert!(error.to_string().contains("exceeded its bound"), "{error}");
    }

    #[tokio::test]
    async fn diagnostics_above_the_control_bound_fail() {
        let child = test_child("head -c 70000 /dev/zero >&2");
        let error = exchange(child, None, 1 << 20)
            .await
            .expect_err("diagnostics must be bounded");
        assert!(
            error.to_string().contains("diagnostic exceeded its bound"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn child_failure_is_reported_without_its_diagnostics() {
        let child = test_child("printf 'private-bytes' >&2; exit 7");
        let error = exchange(child, None, 1024)
            .await
            .expect_err("nonzero exit must fail the exchange");
        assert_eq!(error.to_string(), "Kubernetes adapter operation failed");
    }

    #[tokio::test]
    async fn cancelling_the_exchange_kills_the_child() {
        let child = test_child("exec sleep 30");
        let pid = child.id().expect("child id");
        let result =
            tokio::time::timeout(Duration::from_millis(100), exchange(child, None, 1024)).await;
        assert!(result.is_err(), "a blocked child must not complete");
        tokio::time::timeout(Duration::from_secs(5), wait_for_exit(pid))
            .await
            .expect("dropping the exchange must kill the child");
    }
}
