//! Disposable OCI lifecycle with explicit Docker and Kubernetes adapters.
//!
//! The operator must map the supervisor workspace root to the same private root
//! on the Docker host. Each direct child is owned exclusively by one job and
//! contains only files that job may read or modify. The runtime directory is a
//! separate supervisor-only directory, never mounted into a job.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use crate::console::Stream;

const OWNER: &str = "piping-ansible-runner";
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_OUTPUT_LIMIT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
/// Fixed Docker container process limit. The Kubernetes adapter takes its
/// ceiling from the trusted pool contract instead.
const DOCKER_PIDS_LIMIT: u32 = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Runtime {
    Docker,
    Kubernetes,
}

#[derive(Clone, Debug)]
pub(crate) struct SandboxConfig {
    pub runtime: Runtime,
    pub workspace_root: PathBuf,
    pub host_workspace_root: PathBuf,
    pub runtime_dir: PathBuf,
    pub installation_id: String,
    /// Optional test-resource label; never used as the ownership boundary.
    pub task_label: Option<String>,
    /// Explicit operator-approved Docker network, or none for no network.
    pub network: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SandboxLimits {
    pub memory_bytes: u64,
    pub pids: u32,
    pub cpus_millis: u32,
    pub timeout: Duration,
    pub output_bytes: usize,
}

// argv and env may contain private job data.
pub(crate) struct SandboxSpec {
    pub dispatch_id: String,
    pub phase: String,
    pub image: String,
    pub workspace: PathBuf,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub workdir: PathBuf,
    pub limits: SandboxLimits,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SandboxHandle {
    id: String,
    runtime: Runtime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SandboxState {
    Created,
    Running,
    Exited(i32),
}

struct SandboxOutput {
    stdout: Vec<u8>,
}

pub(crate) struct SandboxFrame {
    pub kind: FrameKind,
    pub bytes: Vec<u8>,
}

/// Console bytes and the untrusted structured notification share the private
/// transport but are never merged: the console collector only sees `Output`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameKind {
    Output(Stream),
    Notification,
}

pub(crate) struct SandboxStream {
    receiver: mpsc::Receiver<Result<SandboxFrame, SandboxError>>,
    cancel: Option<oneshot::Sender<()>>,
}

impl SandboxStream {
    pub(crate) async fn next(&mut self) -> Option<Result<SandboxFrame, SandboxError>> {
        self.receiver.recv().await
    }
}

impl Drop for SandboxStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            // The detached owner kills and waits for the CLI on cancellation.
            let _ = cancel.send(());
        }
    }
}

struct PrivateEnvFile(PathBuf);

impl Drop for PrivateEnvFile {
    fn drop(&mut self) {
        // Best effort on cancellation; startup must remove abandoned env files.
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SandboxError {
    #[error("invalid sandbox configuration")]
    InvalidConfig,
    #[error("invalid sandbox specification")]
    InvalidSpec,
    #[error("sandbox runtime adapter is unavailable")]
    Unavailable,
    #[error("sandbox runtime operation failed")]
    Runtime,
    #[error("sandbox runtime operation timed out")]
    Timeout,
    #[error("sandbox output exceeded its bound")]
    OutputLimit,
    #[error("sandbox resource is not owned by this supervisor")]
    Ownership,
}

impl SandboxConfig {
    pub(crate) fn from_env() -> Result<Self, SandboxError> {
        let runtime = match std::env::var("PIPING_SANDBOX_RUNTIME").as_deref() {
            Ok("docker") => Runtime::Docker,
            Ok("kubernetes") => Runtime::Kubernetes,
            _ => return Err(SandboxError::InvalidConfig),
        };
        let path = |name| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .ok_or(SandboxError::InvalidConfig)
        };
        let workspace_root = std::fs::canonicalize(path("PIPING_SANDBOX_WORKSPACE_ROOT")?)
            .map_err(|_| SandboxError::InvalidConfig)?;
        let runtime_dir = std::fs::canonicalize(path("PIPING_SANDBOX_RUNTIME_DIR")?)
            .map_err(|_| SandboxError::InvalidConfig)?;
        let host_workspace_root = if runtime == Runtime::Docker {
            path("PIPING_SANDBOX_HOST_WORKSPACE_ROOT")?
        } else {
            workspace_root.clone()
        };
        let network = std::env::var("PIPING_SANDBOX_DOCKER_NETWORK")
            .ok()
            .filter(|value| !value.is_empty());
        let installation_id = std::env::var("PIPING_SANDBOX_INSTALLATION_ID")
            .map_err(|_| SandboxError::InvalidConfig)?;
        let task_label = std::env::var("PIPING_SANDBOX_TASK_LABEL").ok();
        let config = Self {
            runtime,
            workspace_root,
            host_workspace_root,
            runtime_dir,
            network,
            installation_id,
            task_label,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), SandboxError> {
        let metadata =
            std::fs::metadata(&self.runtime_dir).map_err(|_| SandboxError::InvalidConfig)?;
        if !valid_label(&self.installation_id, 24)
            || self
                .task_label
                .as_ref()
                .is_some_and(|s| !valid_label(s, 32))
            || !metadata.is_dir()
            || metadata.permissions().mode() & 0o077 != 0
            || !self.workspace_root.is_absolute()
            || !self.workspace_root.is_dir()
            || !self.host_workspace_root.is_absolute()
            || self
                .host_workspace_root
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            || self
                .host_workspace_root
                .to_str()
                .is_none_or(|s| s.contains(',') || s.contains('\n'))
            || self.runtime_dir.starts_with(&self.workspace_root)
            || self.workspace_root.starts_with(&self.runtime_dir)
            || self.network.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s == "host"
                    || s == "bridge"
                    || !s
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
            })
        {
            return Err(SandboxError::InvalidConfig);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Sandbox {
    config: SandboxConfig,
    kubernetes: Option<crate::kubernetes::Kubernetes>,
}

impl Sandbox {
    pub(crate) fn new(config: SandboxConfig) -> Result<Self, SandboxError> {
        config.validate()?;
        let kubernetes = if config.runtime == Runtime::Kubernetes {
            Some(
                crate::kubernetes::Kubernetes::from_env(&config.runtime_dir)
                    .map_err(|_| SandboxError::InvalidConfig)?,
            )
        } else {
            None
        };
        Ok(Self { config, kubernetes })
    }

    fn docker(&self) -> Result<(), SandboxError> {
        match self.config.runtime {
            Runtime::Docker => Ok(()),
            Runtime::Kubernetes => Err(SandboxError::Unavailable),
        }
    }

    pub(crate) async fn create(&self, spec: &SandboxSpec) -> Result<SandboxHandle, SandboxError> {
        let workspace_name = self.validate(spec)?;
        if let Some(kubernetes) = &self.kubernetes {
            let id = kubernetes
                .create(spec)
                .await
                .map_err(|_| SandboxError::Runtime)?;
            return Ok(SandboxHandle {
                id,
                runtime: Runtime::Kubernetes,
            });
        }
        let name = format!(
            "piping-{}-{}-{}",
            self.config.installation_id, spec.dispatch_id, spec.phase
        );
        let deadline = now()?
            .checked_add(spec.limits.timeout.as_secs())
            .ok_or(SandboxError::InvalidSpec)?;
        let host_workspace = self.config.host_workspace_root.join(workspace_name);
        let mount = format!(
            "type=bind,src={},dst=/job",
            host_workspace.to_str().ok_or(SandboxError::InvalidSpec)?
        );
        let env_path = self
            .config
            .runtime_dir
            .join(format!("env-{}", uuid::Uuid::new_v4()));
        let mut env_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&env_path)
            .map_err(|_| SandboxError::Runtime)?;
        let env_guard = PrivateEnvFile(env_path.clone());
        let write_result = (|| -> std::io::Result<()> {
            writeln!(env_file, "HOME=/home/runner\nTMPDIR=/tmp")?;
            for (key, value) in &spec.env {
                writeln!(env_file, "{key}={value}")?;
            }
            Ok(())
        })();
        drop(env_file);
        if write_result.is_err() {
            return Err(SandboxError::Runtime);
        }
        let mut args: Vec<OsString> = vec!["create".into(), "--name".into(), name.clone().into()];
        for label in [
            format!("piping.owner={OWNER}"),
            format!("piping.installation={}", self.config.installation_id),
            format!("piping.dispatch={}", spec.dispatch_id),
            format!("piping.phase={}", spec.phase),
            format!("piping.deadline={deadline}"),
            format!("piping.output-limit={}", spec.limits.output_bytes),
        ] {
            args.extend(["--label".into(), label.into()]);
        }
        if let Some(task) = &self.config.task_label {
            args.extend(["--label".into(), format!("piping.task={task}").into()]);
        }
        let cpus = format!(
            "{}.{:03}",
            spec.limits.cpus_millis / 1000,
            spec.limits.cpus_millis % 1000
        );
        args.extend([
            "--user".into(),
            "10001:10001".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges".into(),
            "--read-only".into(),
            "--network".into(),
            self.config.network.as_deref().unwrap_or("none").into(),
            "--pids-limit".into(),
            spec.limits.pids.to_string().into(),
            "--memory".into(),
            spec.limits.memory_bytes.to_string().into(),
            "--memory-swap".into(),
            spec.limits.memory_bytes.to_string().into(),
            "--cpus".into(),
            cpus.into(),
            "--init".into(),
            "--tmpfs".into(),
            "/tmp:rw,nosuid,nodev,size=64m,mode=1777".into(),
            "--tmpfs".into(),
            "/home/runner:rw,nosuid,nodev,size=64m,uid=10001,gid=10001,mode=0700".into(),
            "--mount".into(),
            mount.into(),
            "--workdir".into(),
            spec.workdir.as_os_str().into(),
            "--log-driver".into(),
            "none".into(),
            "--env-file".into(),
            env_path.as_os_str().into(),
            "--entrypoint".into(),
            spec.argv[0].clone().into(),
            spec.image.clone().into(),
        ]);
        args.extend(spec.argv.iter().skip(1).map(OsString::from));
        // A read-only submount prevents a job from replacing its credential
        // helper or SSH trust input through the writable parent workspace.
        if let Ok(metadata) = std::fs::symlink_metadata(spec.workspace.join("controller")) {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(SandboxError::InvalidSpec);
            }
            let index = args
                .iter()
                .position(|arg| arg == "--entrypoint")
                .ok_or(SandboxError::InvalidSpec)?;
            args.splice(
                index..index,
                [
                    OsString::from("--mount"),
                    OsString::from(format!(
                        "type=bind,src={},dst=/job/controller,readonly",
                        host_workspace.join("controller").display()
                    )),
                ],
            );
        }
        let result = docker_command(args, CONTROL_OUTPUT_LIMIT).await;
        let removal = std::fs::remove_file(&env_path);
        drop(env_guard);
        if removal.is_err() {
            return Err(SandboxError::Runtime);
        }
        let result = result?;
        let id = std::str::from_utf8(&result.stdout)
            .map_err(|_| SandboxError::Runtime)?
            .trim();
        if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(SandboxError::Runtime);
        }
        let handle = SandboxHandle {
            id: id.to_owned(),
            runtime: Runtime::Docker,
        };
        Ok(handle)
    }

    fn validate<'a>(&self, spec: &'a SandboxSpec) -> Result<&'a std::ffi::OsStr, SandboxError> {
        let id = uuid::Uuid::parse_str(&spec.dispatch_id).map_err(|_| SandboxError::InvalidSpec)?;
        if id.to_string() != spec.dispatch_id
            || spec.phase.is_empty()
            || spec.phase.len() > 32
            || !spec
                .phase
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err(SandboxError::InvalidSpec);
        }
        let (repo, digest) = spec
            .image
            .rsplit_once("@sha256:")
            .ok_or(SandboxError::InvalidSpec)?;
        if repo.is_empty()
            || repo
                .bytes()
                .any(|c| c.is_ascii_whitespace() || c.is_ascii_control() || c == b'@')
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(SandboxError::InvalidSpec);
        }
        let root = std::fs::canonicalize(&self.config.workspace_root)
            .map_err(|_| SandboxError::InvalidConfig)?;
        let workspace =
            std::fs::canonicalize(&spec.workspace).map_err(|_| SandboxError::InvalidSpec)?;
        if workspace != spec.workspace
            || workspace.parent() != Some(root.as_path())
            || !workspace.is_dir()
        {
            return Err(SandboxError::InvalidSpec);
        }
        let name = spec
            .workspace
            .file_name()
            .ok_or(SandboxError::InvalidSpec)?;
        if name
            .to_str()
            .is_none_or(|s| s.contains(',') || s.contains('\n'))
            || !spec.workdir.is_absolute()
            || !spec.workdir.starts_with(Path::new("/job"))
            || spec
                .workdir
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            || spec.argv.is_empty()
            || spec.argv[0].is_empty()
            || spec.argv.iter().any(|s| s.contains('\0'))
            || spec.env.iter().any(|(k, v)| {
                k.is_empty()
                    || k == "HOME"
                    || k == "TMPDIR"
                    || !k.bytes().enumerate().all(|(i, c)| {
                        c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                    })
                    || v.bytes().any(|c| c == b'\0' || c == b'\n' || c == b'\r')
            })
            || spec.limits.memory_bytes < 6 * 1024 * 1024
            || spec.limits.pids == 0
            || spec.limits.cpus_millis == 0
            || spec.limits.timeout.as_secs() == 0
            || spec.limits.output_bytes == 0
            || spec.limits.output_bytes > MAX_OUTPUT
        {
            return Err(SandboxError::InvalidSpec);
        }
        Ok(name)
    }

    async fn inspect(&self, handle: &SandboxHandle) -> Result<Value, SandboxError> {
        self.docker()?;
        validate_handle(handle)?;
        let output = docker_command(
            vec!["inspect".into(), handle.id.clone().into()],
            CONTROL_OUTPUT_LIMIT,
        )
        .await?;
        let value: Value =
            serde_json::from_slice(&output.stdout).map_err(|_| SandboxError::Runtime)?;
        let object = value
            .as_array()
            .and_then(|a| a.first())
            .ok_or(SandboxError::Runtime)?;
        if object
            .pointer("/Config/Labels/piping.owner")
            .and_then(Value::as_str)
            != Some(OWNER)
            || object
                .pointer("/Config/Labels/piping.installation")
                .and_then(Value::as_str)
                != Some(self.config.installation_id.as_str())
            || object.pointer("/Id").and_then(Value::as_str) != Some(handle.id.as_str())
        {
            return Err(SandboxError::Ownership);
        }
        Ok(object.clone())
    }

    pub(crate) async fn start(&self, handle: &SandboxHandle) -> Result<(), SandboxError> {
        if handle.runtime != self.config.runtime {
            return Err(SandboxError::Ownership);
        }
        if let Some(kubernetes) = &self.kubernetes {
            return kubernetes
                .start(&handle.id)
                .await
                .map_err(|_| SandboxError::Runtime);
        }
        let object = self.inspect(handle).await?;
        if deadline(&object)? <= now()? {
            self.terminate(handle).await?;
            return Err(SandboxError::Timeout);
        }
        docker_command(
            vec!["start".into(), handle.id.clone().into()],
            CONTROL_OUTPUT_LIMIT,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn observe(
        &self,
        handle: &SandboxHandle,
    ) -> Result<SandboxState, SandboxError> {
        if handle.runtime != self.config.runtime {
            return Err(SandboxError::Ownership);
        }
        if let Some(kubernetes) = &self.kubernetes {
            return kubernetes
                .observe(&handle.id)
                .await
                .map_err(|_| SandboxError::Runtime);
        }
        let object = self.inspect(handle).await?;
        match object.pointer("/State/Status").and_then(Value::as_str) {
            Some("created") => Ok(SandboxState::Created),
            Some("running" | "restarting") => Ok(SandboxState::Running),
            Some("exited" | "dead") => Ok(SandboxState::Exited(
                object
                    .pointer("/State/ExitCode")
                    .and_then(Value::as_i64)
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or(SandboxError::Runtime)?,
            )),
            _ => Err(SandboxError::Runtime),
        }
    }

    /// Attach Docker before start so no byte can be lost to log rotation. For
    /// Kubernetes starts its private exec stream after the Job is ready.
    pub(crate) async fn stream(
        &self,
        handle: &SandboxHandle,
        limit: usize,
    ) -> Result<SandboxStream, SandboxError> {
        if handle.runtime != self.config.runtime {
            return Err(SandboxError::Ownership);
        }
        if limit == 0 || limit > MAX_OUTPUT {
            return Err(SandboxError::InvalidSpec);
        }
        if let Some(kubernetes) = &self.kubernetes {
            let command = kubernetes
                .exec_stream(&handle.id)
                .await
                .map_err(|_| SandboxError::Runtime)?;
            return kubernetes_stream(command, limit);
        }
        let object = self.inspect(handle).await?;
        let configured = object
            .pointer("/Config/Labels/piping.output-limit")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or(SandboxError::Ownership)?;
        if limit > configured
            || object.pointer("/State/Status").and_then(Value::as_str) != Some("created")
        {
            return Err(SandboxError::InvalidSpec);
        }
        let client = reqwest::Client::builder()
            .unix_socket("/var/run/docker.sock")
            .no_proxy()
            .connect_timeout(CLI_TIMEOUT)
            .build()
            .map_err(|_| SandboxError::Runtime)?;
        let response = tokio::time::timeout(
            CLI_TIMEOUT,
            client
                .post(format!(
                    "http://docker/v1.52/containers/{}/attach?stream=1&stdout=1&stderr=1&logs=0",
                    handle.id
                ))
                .header("Connection", "Upgrade")
                .header("Upgrade", "tcp")
                .send(),
        )
        .await
        .map_err(|_| SandboxError::Timeout)?
        .map_err(|_| SandboxError::Runtime)?;
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            return Err(SandboxError::Runtime);
        }
        let upgraded = response
            .upgrade()
            .await
            .map_err(|_| SandboxError::Runtime)?;
        let (sender, receiver) = mpsc::channel(16);
        let (cancel, mut cancelled) = oneshot::channel();
        tokio::spawn(async move {
            let result = tokio::select! {
                result = follow_docker_mux(upgraded, &sender, limit) => Some(result),
                _ = &mut cancelled => None,
            };
            if let Some(Err(error)) = result {
                let _ = sender.send(Err(error)).await;
            }
        });
        Ok(SandboxStream {
            receiver,
            cancel: Some(cancel),
        })
    }

    pub(crate) fn attach_before_start(&self) -> bool {
        self.config.runtime == Runtime::Docker
    }

    /// Docker jobs expose their sealed notification through the host workspace;
    /// Kubernetes delivers it on the private exec stream.
    pub(crate) fn reads_workspace_notification(&self) -> bool {
        self.config.runtime == Runtime::Docker
    }

    pub(crate) fn workspace_root(&self) -> &Path {
        &self.config.workspace_root
    }

    /// Process ceiling this supervisor enforces for one job. The caller must
    /// request exactly this value in `SandboxLimits`: Docker keeps its fixed
    /// container limit, Kubernetes uses the contracted Pod ancestor ceiling.
    pub(crate) fn process_limit(&self) -> u32 {
        match &self.kubernetes {
            Some(kubernetes) => kubernetes.process_limit(),
            None => DOCKER_PIDS_LIMIT,
        }
    }

    /// Truthful resource scope for health reporting, without credentials or
    /// private paths. Kubernetes reports the asserted Pod ancestor contract.
    pub(crate) fn resource_contract(&self) -> Value {
        match &self.kubernetes {
            Some(kubernetes) => kubernetes.resource_contract(),
            None => serde_json::json!({
                "schema_version": 1,
                "runtime": "docker",
                "scope": "container",
                "pids": DOCKER_PIDS_LIMIT,
            }),
        }
    }

    /// Clean an exact accepted identity even if create's acknowledgement was lost.
    pub(crate) async fn cleanup_dispatch(
        &self,
        dispatch: &str,
        phase: &str,
    ) -> Result<(), SandboxError> {
        let id = uuid::Uuid::parse_str(dispatch).map_err(|_| SandboxError::InvalidSpec)?;
        if id.to_string() != dispatch || !valid_label(phase, 32) {
            return Err(SandboxError::InvalidSpec);
        }
        if let Some(kubernetes) = &self.kubernetes {
            return kubernetes
                .terminate(&format!("piping-{dispatch}-{phase}"))
                .await
                .map_err(|_| SandboxError::Runtime);
        }
        let listed = docker_command(
            vec![
                "ps".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--filter".into(),
                format!("label=piping.owner={OWNER}").into(),
                "--filter".into(),
                format!("label=piping.installation={}", self.config.installation_id).into(),
                "--filter".into(),
                format!("label=piping.dispatch={dispatch}").into(),
                "--filter".into(),
                format!("label=piping.phase={phase}").into(),
                "--format".into(),
                "{{.ID}}".into(),
            ],
            CONTROL_OUTPUT_LIMIT,
        )
        .await?;
        for id in std::str::from_utf8(&listed.stdout)
            .map_err(|_| SandboxError::Runtime)?
            .lines()
        {
            self.terminate(&SandboxHandle {
                id: id.into(),
                runtime: Runtime::Docker,
            })
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn terminate(&self, handle: &SandboxHandle) -> Result<(), SandboxError> {
        if handle.runtime != self.config.runtime {
            return Err(SandboxError::Ownership);
        }
        if let Some(kubernetes) = &self.kubernetes {
            return kubernetes
                .terminate(&handle.id)
                .await
                .map_err(|_| SandboxError::Runtime);
        }
        self.docker()?;
        validate_handle(handle)?;
        if !self.exists(handle).await? {
            return Ok(());
        }
        if let Err(error) = self.inspect(handle).await {
            if !self.exists(handle).await? {
                return Ok(());
            }
            return Err(error);
        }
        let removal = docker_command(
            vec!["rm".into(), "--force".into(), handle.id.clone().into()],
            CONTROL_OUTPUT_LIMIT,
        )
        .await;
        if !self.exists(handle).await? {
            return Ok(());
        }
        removal?;
        Err(SandboxError::Runtime)
    }

    async fn exists(&self, handle: &SandboxHandle) -> Result<bool, SandboxError> {
        let result = docker_command(
            vec![
                "ps".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--filter".into(),
                format!("id={}", handle.id).into(),
                "--format".into(),
                "{{.ID}}".into(),
            ],
            CONTROL_OUTPUT_LIMIT,
        )
        .await?;
        Ok(!result.stdout.is_empty())
    }

    /// Call at supervisor startup before accepting work; expired resources are removed.
    pub(crate) async fn reconcile(&self) -> Result<Vec<SandboxHandle>, SandboxError> {
        if let Some(kubernetes) = &self.kubernetes {
            return kubernetes
                .reconcile()
                .await
                .map(|names| {
                    names
                        .into_iter()
                        .map(|id| SandboxHandle {
                            id,
                            runtime: Runtime::Kubernetes,
                        })
                        .collect()
                })
                .map_err(|_| SandboxError::Runtime);
        }
        self.docker()?;
        let list = docker_command(
            vec![
                "ps".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--filter".into(),
                format!("label=piping.owner={OWNER}").into(),
                "--filter".into(),
                format!("label=piping.installation={}", self.config.installation_id).into(),
                "--format".into(),
                "{{.ID}}".into(),
            ],
            CONTROL_OUTPUT_LIMIT,
        )
        .await?;
        let names = std::str::from_utf8(&list.stdout).map_err(|_| SandboxError::Runtime)?;
        let mut active = Vec::new();
        for name in names.lines() {
            let handle = SandboxHandle {
                id: name.to_string(),
                runtime: Runtime::Docker,
            };
            self.inspect(&handle).await?;
            active.push(handle);
        }
        Ok(active)
    }
}

fn valid_label(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

fn validate_handle(handle: &SandboxHandle) -> Result<(), SandboxError> {
    if handle.id.len() == 64
        && handle
            .id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        Ok(())
    } else {
        Err(SandboxError::Ownership)
    }
}

async fn follow_docker_mux(
    mut input: impl AsyncRead + Unpin,
    sender: &mpsc::Sender<Result<SandboxFrame, SandboxError>>,
    limit: usize,
) -> Result<(), SandboxError> {
    let mut count = 0usize;
    loop {
        let mut header = [0u8; 8];
        let first = input
            .read(&mut header[..1])
            .await
            .map_err(|_| SandboxError::Runtime)?;
        if first == 0 {
            return Ok(());
        }
        input
            .read_exact(&mut header[1..])
            .await
            .map_err(|_| SandboxError::Runtime)?;
        let stream = match header[0] {
            1 => Stream::Stdout,
            2 => Stream::Stderr,
            _ => return Err(SandboxError::Runtime),
        };
        if header[1..4] != [0, 0, 0] {
            return Err(SandboxError::Runtime);
        }
        let length = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        count = count
            .checked_add(length)
            .filter(|size| *size <= limit)
            .ok_or(SandboxError::OutputLimit)?;
        let mut remaining = length;
        while remaining > 0 {
            let mut bytes = vec![0; remaining.min(8192)];
            input
                .read_exact(&mut bytes)
                .await
                .map_err(|_| SandboxError::Runtime)?;
            remaining -= bytes.len();
            sender
                .send(Ok(SandboxFrame {
                    kind: FrameKind::Output(stream),
                    bytes,
                }))
                .await
                .map_err(|_| SandboxError::Runtime)?;
        }
    }
}

fn kubernetes_stream(mut command: Command, limit: usize) -> Result<SandboxStream, SandboxError> {
    let mut child = command.spawn().map_err(|_| SandboxError::Runtime)?;
    let stdout = child.stdout.take().ok_or(SandboxError::Runtime)?;
    let stderr = child.stderr.take().ok_or(SandboxError::Runtime)?;
    let (sender, receiver) = mpsc::channel(16);
    let (cancel, mut cancelled) = oneshot::channel();
    tokio::spawn(async move {
        let operation = async {
            let ((), (_, overflow)) = tokio::try_join!(
                follow_kubernetes_frames(stdout, &sender, limit),
                read_bounded(stderr, 65536)
            )?;
            if overflow
                || !child
                    .wait()
                    .await
                    .map_err(|_| SandboxError::Runtime)?
                    .success()
            {
                return Err(SandboxError::Runtime);
            }
            Ok(())
        };
        let result =
            tokio::select! { result = operation => Some(result), _ = &mut cancelled => None };
        if !matches!(result, Some(Ok(()))) {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Some(Err(error)) = result {
            let _ = sender.send(Err(error)).await;
        }
    });
    Ok(SandboxStream {
        receiver,
        cancel: Some(cancel),
    })
}

async fn follow_kubernetes_frames(
    mut input: impl AsyncRead + Unpin,
    sender: &mpsc::Sender<Result<SandboxFrame, SandboxError>>,
    limit: usize,
) -> Result<(), SandboxError> {
    let mut buffer = [0u8; 4096];
    let mut pending = Vec::new();
    let mut count = 0usize;
    loop {
        let length = input
            .read(&mut buffer)
            .await
            .map_err(|_| SandboxError::Runtime)?;
        if length == 0 {
            return if pending.is_empty() {
                Ok(())
            } else {
                Err(SandboxError::Runtime)
            };
        }
        pending.extend_from_slice(&buffer[..length]);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let (stdout, stderr, notify) = crate::kubernetes::decode_frames(&pending[..end], 8192)
                .map_err(|_| SandboxError::Runtime)?;
            pending.drain(..=end);
            for (stream, bytes) in [(Stream::Stdout, stdout), (Stream::Stderr, stderr)] {
                if bytes.is_empty() {
                    continue;
                }
                count = count
                    .checked_add(bytes.len())
                    .filter(|size| *size <= limit)
                    .ok_or(SandboxError::OutputLimit)?;
                sender
                    .send(Ok(SandboxFrame {
                        kind: FrameKind::Output(stream),
                        bytes,
                    }))
                    .await
                    .map_err(|_| SandboxError::Runtime)?;
            }
            if !notify.is_empty() {
                sender
                    .send(Ok(SandboxFrame {
                        kind: FrameKind::Notification,
                        bytes: notify,
                    }))
                    .await
                    .map_err(|_| SandboxError::Runtime)?;
            }
        }
        if pending.len() > 32768 {
            return Err(SandboxError::OutputLimit);
        }
    }
}

fn now() -> Result<u64, SandboxError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SandboxError::Runtime)?
        .as_secs())
}

fn deadline(object: &Value) -> Result<u64, SandboxError> {
    object
        .pointer("/Config/Labels/piping.deadline")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or(SandboxError::Ownership)
}

async fn docker_command(args: Vec<OsString>, limit: usize) -> Result<SandboxOutput, SandboxError> {
    let mut command = Command::new("/usr/bin/docker");
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| SandboxError::Runtime)?;
    let stdout = child.stdout.take().ok_or(SandboxError::Runtime)?;
    let stderr = child.stderr.take().ok_or(SandboxError::Runtime)?;
    let result = tokio::time::timeout(CLI_TIMEOUT, async {
        let (a, b, status) = tokio::join!(
            read_bounded(stdout, limit),
            read_bounded(stderr, limit),
            child.wait()
        );
        let (out, out_overflow) = a?;
        let (err, err_overflow) = b?;
        let status = status.map_err(|_| SandboxError::Runtime)?;
        if out_overflow || err_overflow || out.len().saturating_add(err.len()) > limit {
            return Err(SandboxError::OutputLimit);
        }
        if !status.success() {
            return Err(SandboxError::Runtime);
        }
        Ok(SandboxOutput { stdout: out })
    })
    .await
    .map_err(|_| SandboxError::Timeout)?;
    result
}

async fn read_bounded(
    mut input: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<(Vec<u8>, bool), SandboxError> {
    let mut retained = Vec::new();
    let mut exceeded = false;
    let mut chunk = [0u8; 8192];
    loop {
        let count = input
            .read(&mut chunk)
            .await
            .map_err(|_| SandboxError::Runtime)?;
        if count == 0 {
            break;
        }
        let keep = count.min(limit.saturating_sub(retained.len()));
        retained.extend_from_slice(&chunk[..keep]);
        exceeded |= keep < count;
    }
    Ok((retained, exceeded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unpinned_and_escaping_jobs() {
        let (_root, sandbox) = docker_fixture();
        let mut spec = SandboxSpec {
            dispatch_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
            phase: "execute".into(),
            image: format!("example/image@sha256:{}", "a".repeat(64)),
            workspace: sandbox.config.workspace_root.join("job"),
            argv: vec!["ansible-playbook".into()],
            env: BTreeMap::new(),
            workdir: "/job".into(),
            limits: SandboxLimits {
                memory_bytes: 8 * 1024 * 1024,
                pids: 8,
                cpus_millis: 500,
                timeout: Duration::from_secs(1),
                output_bytes: 1024,
            },
        };
        assert!(sandbox.validate(&spec).is_ok());
        spec.image = "example/image:latest".into();
        assert!(matches!(
            sandbox.validate(&spec),
            Err(SandboxError::InvalidSpec)
        ));
        spec.image = format!("example/image@sha256:{}", "a".repeat(64));
        spec.workdir = "/job/../outside".into();
        assert!(matches!(
            sandbox.validate(&spec),
            Err(SandboxError::InvalidSpec)
        ));
        spec.workdir = "/job".into();
        spec.env.insert("TOKEN".into(), "line\nbreak".into());
        assert!(matches!(
            sandbox.validate(&spec),
            Err(SandboxError::InvalidSpec)
        ));
    }

    #[test]
    fn docker_contract_reports_the_container_process_limit() {
        let (_root, sandbox) = docker_fixture();
        assert_eq!(sandbox.process_limit(), 256);
        let contract = sandbox.resource_contract();
        assert_eq!(contract["runtime"], "docker");
        assert_eq!(contract["scope"], "container");
        assert_eq!(contract["pids"].as_u64(), Some(256));
        assert!(contract.get("pool").is_none());
        assert!(contract.get("native_acceptance_sha256").is_none());
    }

    fn docker_fixture() -> (tempfile::TempDir, Sandbox) {
        let root = tempfile::tempdir().expect("private test root");
        let workspace_root = root.path().join("jobs");
        let runtime_dir = root.path().join("runtime");
        std::fs::create_dir_all(workspace_root.join("job")).expect("job directory");
        std::fs::create_dir(&runtime_dir).expect("runtime directory");
        std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))
            .expect("private runtime");
        let sandbox = Sandbox::new(SandboxConfig {
            runtime: Runtime::Docker,
            workspace_root: workspace_root.clone(),
            host_workspace_root: workspace_root,
            runtime_dir,
            installation_id: "test-installation".into(),
            task_label: None,
            network: None,
        })
        .expect("valid configuration");
        (root, sandbox)
    }

    #[tokio::test]
    async fn follow_frames_are_tagged_and_bounded() {
        let (sender, mut receiver) = mpsc::channel(8);
        let input = b"\x01\0\0\0\0\0\0\x03out\x02\0\0\0\0\0\0\x03err";
        follow_docker_mux(&input[..], &sender, 6)
            .await
            .expect("within limit");
        drop(sender);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        while let Some(frame) = receiver.recv().await {
            let frame = frame.expect("frame");
            match frame.kind {
                FrameKind::Output(Stream::Stdout) => stdout.extend(frame.bytes),
                FrameKind::Output(Stream::Stderr) => stderr.extend(frame.bytes),
                FrameKind::Notification => panic!("docker console never carries notifications"),
            }
        }
        assert_eq!(stdout, b"out");
        assert_eq!(stderr, b"err");
        let (sender, _receiver) = mpsc::channel(8);
        assert!(matches!(
            follow_docker_mux(&input[..], &sender, 5).await,
            Err(SandboxError::OutputLimit)
        ));
    }
}
