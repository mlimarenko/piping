//! Translate a resolved dispatch into job-private native Ansible inputs.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use piping_contracts::playbook_runner::{
    RUNNER_CONTRACT_VERSION, ResolvedInventory, RunnerJobRequest, SnapshotContent,
    snapshot_tree_sha256, validate_snapshot_entries,
};

use crate::native::{self, ArgumentError, Entry, EntryKind, Inventory};
use crate::sandbox::{SandboxLimits, SandboxSpec};

pub(crate) struct PreparedLaunch {
    pub spec: SandboxSpec,
    pub known_secrets: Vec<Vec<u8>>,
}

fn relative(path: &str) -> Result<PathBuf> {
    if path == "." {
        return Ok(PathBuf::new());
    }
    native::project_path(path).context("invalid relative project path")
}

fn arguments(job: &RunnerJobRequest) -> Result<Vec<String>> {
    let launch = &job.launch;
    let extra = if launch.extra_vars.is_null()
        || launch.extra_vars.as_object().is_some_and(|v| v.is_empty())
    {
        Vec::new()
    } else {
        ensure!(
            launch.extra_vars.is_object(),
            "extra_vars must be an object"
        );
        vec!["@/job/controller/extra_vars.json".to_string()]
    };
    let inventory = match &launch.inventory {
        ResolvedInventory::Managed { .. } => {
            Inventory::Managed(Path::new("/job/controller/inventory"))
        }
        ResolvedInventory::Native { sources } => Inventory::Native(sources),
    };
    native::playbook_argv(
        &launch.argv,
        &launch.playbook_paths,
        inventory,
        &extra,
        launch.limit.as_deref(),
    )
    .map_err(|error| match error {
        ArgumentError::NeedsInput => anyhow::anyhow!("needs_input: manual prompts are unsupported"),
        ArgumentError::Invalid(reason) | ArgumentError::Conflict(reason) => anyhow::anyhow!(reason),
    })?
    .into_iter()
    .map(|value| {
        value
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF-8 argument"))
    })
    .collect()
}

/// Validate the dispatch before accepting it; performs no filesystem writes.
pub(crate) fn validate(job: &RunnerJobRequest) -> Result<()> {
    ensure!(
        job.protocol_version == RUNNER_CONTRACT_VERSION,
        "unsupported runner protocol"
    );
    ensure!(job.dispatch_attempt > 0, "invalid dispatch attempt");
    validate_snapshot_entries(&job.project.entries)
        .map_err(|_| anyhow::anyhow!("invalid snapshot entries"))?;
    ensure!(
        job.project.entries.len() <= 2048,
        "too many snapshot entries"
    );
    let mut total = 0usize;
    for entry in &job.project.entries {
        if let SnapshotContent::File { bytes, .. } = &entry.content {
            ensure!(bytes.len() <= 2 * 1024 * 1024, "snapshot file too large");
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| anyhow::anyhow!("snapshot too large"))?;
        }
    }
    ensure!(total <= 64 * 1024 * 1024, "snapshot too large");
    ensure!(
        snapshot_tree_sha256(&job.project.entries)
            .map_err(|_| anyhow::anyhow!("invalid snapshot"))?
            == job.project.tree_sha256,
        "snapshot digest mismatch"
    );
    let launch = &job.launch;
    if matches!(launch.inventory, ResolvedInventory::Native { .. }) {
        if let Some(connection) = &launch.connection {
            crate::route::validate_native_defaults(connection)?;
        }
        ensure!(
            launch
                .targets
                .iter()
                .all(|target| target.connection.is_none()),
            "native inventory SSH defaults cannot silently add generated inventory"
        );
    } else {
        ensure!(
            launch.connection.is_none(),
            "record defaults require native inventory; managed routes belong to their targets"
        );
    }
    for target in &launch.targets {
        ensure!(
            !target.hostname.is_empty()
                && !target
                    .hostname
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace()),
            "invalid inventory alias"
        );
        if let Some(connection) = &target.connection {
            crate::route::validate(connection)?;
        }
    }
    relative(&launch.workdir)?;
    ensure!(
        (1..=86400).contains(&launch.timeout_seconds),
        "invalid timeout"
    );
    ensure!(
        !job.dispatch_id.is_nil() && !job.callbacks.group_token.is_empty(),
        "missing dispatch/callback identity"
    );
    ensure!(
        launch
            .targets
            .iter()
            .filter_map(|target| target.run_id)
            .collect::<BTreeSet<_>>()
            == job
                .sandbox_run_tokens
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
        "sandbox token set does not match the dispatched runs"
    );
    ensure!(
        job.sandbox_run_tokens
            .values()
            .all(|token| !token.is_empty()),
        "empty sandbox run token"
    );
    ensure!(
        !launch.image.is_empty()
            && !launch.image.contains('@')
            && !launch
                .image
                .bytes()
                .any(|b| b.is_ascii_control() || b.is_ascii_whitespace()),
        "invalid image repository"
    );
    ensure!(
        !launch.image.rsplit('/').next().unwrap_or("").contains(':'),
        "image must not carry a mutable tag"
    );
    let digest = launch.image_digest.strip_prefix("sha256:").unwrap_or("");
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid image digest"
    );
    for (key, value) in launch.env.iter().chain(&launch.secrets) {
        ensure!(
            !key.is_empty()
                && key != "HOME"
                && key != "TMPDIR"
                && key != "PIPING_RECORD_SSH_CONFIG"
                && key != "PIPING_RECORD_SSH_KEY_FILE"
                && key.bytes().enumerate().all(|(i, c)| c == b'_'
                    || c.is_ascii_alphabetic()
                    || (i > 0 && c.is_ascii_digit())),
            "invalid environment name"
        );
        ensure!(
            !value.contains(['\0', '\n', '\r']),
            "invalid environment value"
        );
    }
    ensure!(
        launch
            .env
            .keys()
            .all(|key| !launch.secrets.contains_key(key)),
        "environment and secrets overlap"
    );
    ensure!(
        !launch.env.contains_key("PIPING_API_URL")
            && !launch.env.contains_key("PIPING_RUN_TOKENS_FILE")
            && !launch.secrets.contains_key("PIPING_API_URL")
            && !launch.secrets.contains_key("PIPING_RUN_TOKENS_FILE"),
        "sandbox token delivery owns PIPING_API_URL and PIPING_RUN_TOKENS_FILE"
    );
    arguments(job)?;
    Ok(())
}

fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("cannot create private controller file")?;
    file.write_all(bytes)
        .context("cannot write private controller file")?;
    Ok(())
}

/// Prepare one fresh job workspace; callers discard it on failure.
pub(crate) fn prepare(
    job: &RunnerJobRequest,
    workspace: &Path,
    mut limits: SandboxLimits,
) -> Result<PreparedLaunch> {
    validate(job)?;
    ensure!(
        workspace.is_absolute() && workspace.is_dir(),
        "workspace must be an existing absolute directory"
    );
    let workspace = fs::canonicalize(workspace).context("cannot resolve workspace")?;
    let entries: Vec<_> = job
        .project
        .entries
        .iter()
        .map(|entry| Entry {
            path: entry.path.clone(),
            kind: match &entry.content {
                SnapshotContent::File { bytes, executable } => EntryKind::File {
                    bytes: bytes.clone(),
                    mode: if *executable { 0o700 } else { 0o600 },
                },
                SnapshotContent::Symlink { target } => EntryKind::Symlink {
                    target: target.clone(),
                },
            },
        })
        .collect();
    let project = workspace.join("project");
    native::materialize(&project, &entries).context("cannot materialize snapshot")?;
    let workdir = project.join(relative(&job.launch.workdir)?);
    let resolved_workdir =
        fs::canonicalize(&workdir).context("working directory does not exist")?;
    ensure!(
        resolved_workdir.starts_with(&project) && resolved_workdir.is_dir(),
        "working directory escapes snapshot"
    );
    for playbook in &job.launch.playbook_paths {
        let resolved = fs::canonicalize(workdir.join(relative(playbook)?))
            .context("playbook does not exist")?;
        ensure!(
            resolved.starts_with(&project) && resolved.is_file(),
            "playbook must be a file inside the snapshot"
        );
    }
    if let ResolvedInventory::Native { sources } = &job.launch.inventory {
        for source in sources {
            let resolved = fs::canonicalize(workdir.join(relative(source)?))
                .context("inventory source does not exist")?;
            ensure!(
                resolved.starts_with(&project),
                "inventory source escapes snapshot"
            );
        }
    }
    let controller = workspace.join("controller");
    fs::create_dir(&controller).context("cannot create controller")?;
    fs::set_permissions(&controller, fs::Permissions::from_mode(0o700))?;
    private_file(
        &controller.join("execute.py"),
        include_bytes!("../docker/execute.py"),
    )?;
    let callback_plugins = controller.join("callback_plugins");
    fs::create_dir(&callback_plugins).context("cannot create callback directory")?;
    fs::set_permissions(&callback_plugins, fs::Permissions::from_mode(0o700))?;
    private_file(
        &callback_plugins.join("piping_space_structured.py"),
        include_bytes!("../docker/callback_plugins/piping_space_structured.py"),
    )?;
    if let ResolvedInventory::Managed { content } = &job.launch.inventory {
        private_file(&controller.join("inventory"), content.as_bytes())?;
    }
    let mut route_secrets = Vec::new();
    let mut route_hosts = serde_json::Map::new();
    for (index, target) in job.launch.targets.iter().enumerate() {
        if let Some(connection) = &target.connection {
            let route = crate::route::render(&controller, &format!("target-{index}"), connection)?;
            route_secrets.extend(route.secrets);
            route_hosts.insert(
                target.hostname.clone(),
                serde_json::Value::Object(route.host_vars),
            );
        }
    }
    if !route_hosts.is_empty() {
        private_file(
            &controller.join("routes.json"),
            &serde_json::to_vec(&serde_json::json!({"all":{"hosts":route_hosts}}))?,
        )?;
    }
    if !job.launch.extra_vars.is_null()
        && !job
            .launch
            .extra_vars
            .as_object()
            .is_some_and(|v| v.is_empty())
    {
        private_file(
            &controller.join("extra_vars.json"),
            &serde_json::to_vec(&job.launch.extra_vars)?,
        )?;
    }
    private_file(
        &controller.join("run_tokens.json"),
        &serde_json::to_vec(&job.sandbox_run_tokens)?,
    )?;
    let container_workdir = Path::new("/job/project").join(relative(&job.launch.workdir)?);
    let mut argv = vec![
        "python3".into(),
        "-I".into(),
        "/job/controller/execute.py".into(),
        "--project".into(),
        container_workdir.to_string_lossy().into_owned(),
        "--home".into(),
        "/job/home".into(),
    ];
    if let Some(connection) = &job.launch.connection {
        let defaults = crate::route::render_native_defaults(&controller, connection)?;
        route_secrets.extend(defaults.secrets);
        argv.extend([
            "--record-ssh-config".into(),
            defaults.config_path,
            "--record-ssh-key-file".into(),
            defaults.private_key_file,
        ]);
    }
    for (name, option) in [
        ("requirements.yml", "--requirements"),
        ("requirements.yaml", "--requirements"),
        ("roles/requirements.yml", "--role-requirements"),
        ("roles/requirements.yaml", "--role-requirements"),
        ("collections/requirements.yml", "--collection-requirements"),
        ("collections/requirements.yaml", "--collection-requirements"),
    ] {
        let candidate = workdir.join(name);
        if candidate.exists() {
            let resolved = fs::canonicalize(candidate).context("cannot resolve requirements")?;
            ensure!(
                resolved.starts_with(&resolved_workdir) && resolved.is_file(),
                "requirements escape workdir"
            );
            argv.extend([option.into(), name.into()]);
        }
    }
    argv.push("--".into());
    argv.extend(arguments(job)?);
    if !route_hosts.is_empty() {
        argv.extend(["--inventory".into(), "/job/controller/routes.json".into()]);
    }
    let mut env: BTreeMap<String, String> = job.launch.env.clone();
    env.extend(job.launch.secrets.clone());
    env.insert("PIPING_API_URL".into(), job.callback_base_url.clone());
    env.insert(
        "PIPING_RUN_TOKENS_FILE".into(),
        "/job/controller/run_tokens.json".into(),
    );
    env.entry("PATH".into())
        .or_insert_with(|| "/usr/local/bin:/usr/bin:/bin".into());
    env.entry("LANG".into()).or_insert_with(|| "C.UTF-8".into());
    let mut known_secrets: Vec<Vec<u8>> = job
        .launch
        .secrets
        .values()
        .filter(|value| !value.is_empty())
        .map(|value| value.as_bytes().to_vec())
        .collect();
    known_secrets.extend(route_secrets);
    known_secrets.extend(
        job.sandbox_run_tokens
            .values()
            .filter(|value| !value.is_empty())
            .map(|value| value.as_bytes().to_vec()),
    );
    limits.timeout = limits
        .timeout
        .min(Duration::from_secs(u64::from(job.launch.timeout_seconds)));
    if limits.timeout.is_zero() {
        bail!("invalid sandbox timeout");
    }
    Ok(PreparedLaunch {
        spec: SandboxSpec {
            dispatch_id: job.dispatch_id.to_string(),
            phase: "execution".into(),
            image: format!("{}@{}", job.launch.image, job.launch.image_digest),
            workspace,
            argv,
            env,
            workdir: container_workdir,
            limits,
        },
        known_secrets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use piping_contracts::playbook_runner::{
        ProjectSnapshot, ResolvedLaunch, ResolvedTarget, SnapshotEntry, SupervisorCallbacks,
    };
    use uuid::Uuid;

    fn native_job(inputs: &[(&str, &str)]) -> Result<RunnerJobRequest> {
        let entries = inputs
            .iter()
            .map(|(path, bytes)| SnapshotEntry {
                path: (*path).into(),
                content: SnapshotContent::File {
                    bytes: bytes.as_bytes().to_vec(),
                    executable: false,
                },
            })
            .collect::<Vec<_>>();
        Ok(RunnerJobRequest {
            protocol_version: RUNNER_CONTRACT_VERSION,
            dispatch_id: Uuid::new_v4(),
            dispatch_attempt: 1,
            run_group_id: Uuid::new_v4(),
            callbacks: SupervisorCallbacks {
                group_token: "fixture".into(),
                run_tokens: BTreeMap::new(),
            },
            sandbox_run_tokens: BTreeMap::new(),
            lease_owner: "fixture".into(),
            callback_base_url: "http://example.invalid".into(),
            project: ProjectSnapshot {
                project_id: Uuid::new_v4(),
                revision_id: Uuid::new_v4(),
                tree_sha256: snapshot_tree_sha256(&entries).map_err(anyhow::Error::msg)?,
                source_commit_oid: None,
                entries,
            },
            launch: ResolvedLaunch {
                image: "example/ansible".into(),
                image_digest: format!("sha256:{}", "a".repeat(64)),
                workdir: ".".into(),
                playbook_paths: vec!["site.yml".into()],
                inventory: ResolvedInventory::Native {
                    sources: vec!["inventory.ini".into()],
                },
                connection: None,
                targets: vec![],
                argv: vec![],
                extra_vars: serde_json::Value::Null,
                limit: None,
                env: BTreeMap::new(),
                secrets: BTreeMap::new(),
                timeout_seconds: 30,
            },
        })
    }

    fn fixture_limits() -> SandboxLimits {
        SandboxLimits {
            memory_bytes: 1024 * 1024 * 1024,
            pids: 256,
            cpus_millis: 1000,
            timeout: Duration::from_secs(30),
            output_bytes: 1024 * 1024,
        }
    }

    #[test]
    fn discovers_galaxy_entrypoints_without_rewriting_the_native_project() -> Result<()> {
        let inputs = [
            (
                "site.yml",
                "- hosts: all\n  gather_facts: false\n  tasks: []\n",
            ),
            ("inventory.ini", "[all]\nlocal ansible_connection=local\n"),
            (
                "ansible.cfg",
                "[defaults]\nroles_path=./vendor_roles\ncollections_path=./vendor_collections\n",
            ),
            ("requirements.yml", "roles: []\ncollections: []\n"),
            ("requirements.yaml", "[]\n"),
            ("roles/requirements.yml", "[]\n"),
            ("collections/requirements.yaml", "collections: []\n"),
        ];
        let job = native_job(&inputs)?;

        let root = tempfile::tempdir()?;
        let prepared = prepare(&job, root.path(), fixture_limits())?;
        let boundary = prepared
            .spec
            .argv
            .iter()
            .position(|value| value == "--")
            .context("missing native argv boundary")?;
        let preparation = &prepared.spec.argv[..boundary];
        for (option, path) in [
            ("--requirements", "requirements.yml"),
            ("--requirements", "requirements.yaml"),
            ("--role-requirements", "roles/requirements.yml"),
            ("--collection-requirements", "collections/requirements.yaml"),
        ] {
            assert!(
                preparation
                    .windows(2)
                    .any(|pair| pair[0] == option && pair[1] == path)
            );
        }
        for (path, bytes) in inputs {
            assert_eq!(
                fs::read(root.path().join("project").join(path))?,
                bytes.as_bytes()
            );
        }
        assert!(!root.path().join("controller/inventory").exists());
        let plugin = root
            .path()
            .join("controller/callback_plugins/piping_space_structured.py");
        assert!(fs::metadata(&plugin)?.is_file());
        assert_eq!(fs::metadata(&plugin)?.permissions().mode() & 0o077, 0);
        Ok(())
    }

    fn record_defaults() -> serde_json::Value {
        serde_json::json!({
            "connection_type": "ssh",
            "host": "not-the-inventory-endpoint.test",
            "port": 0,
            "credentials": {"username": "record-user", "private_key": "synthetic-target-key"},
            "jump": [{
                "host": "jump.test", "port": 2222,
                "credentials": {"username": "hop-user", "private_key": "synthetic-hop-key"}
            }],
            "known_hosts_lines": [
                "[inventory.test]:2200 ssh-ed25519 synthetic-target-pin",
                "[jump.test]:2222 ssh-ed25519 synthetic-hop-pin"
            ]
        })
    }

    #[test]
    fn native_defaults_prepare_private_material_without_replacing_native_inputs() -> Result<()> {
        let inputs = [
            (
                "site.yml",
                "- hosts: all\n  gather_facts: false\n  tasks: []\n",
            ),
            (
                "inventory.ini",
                "[all]\ninventory.test ansible_port=2200 ansible_user=native-user ansible_private_key_file=./native.key\n",
            ),
            (
                "ansible.cfg",
                "[defaults]\ntransport=ssh\n[ssh_connection]\nssh_args=-o Compression=yes\n",
            ),
        ];
        let mut job = native_job(&inputs)?;
        job.launch.connection = Some(record_defaults());
        job.launch.argv = vec!["--tags=verify".into()];
        job.launch
            .env
            .insert("ANSIBLE_TRANSPORT".into(), "ssh".into());
        let root = tempfile::tempdir()?;
        let prepared = prepare(&job, root.path(), fixture_limits())?;
        let boundary = prepared
            .spec
            .argv
            .iter()
            .position(|arg| arg == "--")
            .context("missing native argv boundary")?;
        let options = &prepared.spec.argv[..boundary];
        for (name, value) in [
            ("--record-ssh-config", "/job/controller/ssh-native.conf"),
            (
                "--record-ssh-key-file",
                "/job/controller/ssh-native-target.key",
            ),
        ] {
            assert!(
                options
                    .windows(2)
                    .any(|pair| pair[0] == name && pair[1] == value)
            );
        }
        let native_argv = &prepared.spec.argv[boundary + 1..];
        assert!(native_argv.iter().any(|arg| arg == "--tags=verify"));
        assert!(
            native_argv
                .windows(2)
                .any(|pair| pair[0] == "--inventory" && pair[1] == "inventory.ini")
        );
        assert_eq!(
            prepared
                .spec
                .env
                .get("ANSIBLE_TRANSPORT")
                .map(String::as_str),
            Some("ssh")
        );
        assert!(!prepared.spec.env.contains_key("ANSIBLE_CONFIG"));
        for (path, bytes) in inputs {
            assert_eq!(
                fs::read(root.path().join("project").join(path))?,
                bytes.as_bytes()
            );
        }
        let controller = root.path().join("controller");
        assert!(!controller.join("inventory").exists());
        assert!(!controller.join("routes.json").exists());
        for (name, key) in [
            ("ssh-native-target.key", "synthetic-target-key"),
            ("ssh-native-0.key", "synthetic-hop-key"),
        ] {
            assert_eq!(fs::read(controller.join(name))?, key.as_bytes());
            assert!(
                prepared
                    .known_secrets
                    .iter()
                    .any(|secret| secret.as_slice() == key.as_bytes())
            );
            assert!(prepared.spec.argv.iter().all(|arg| !arg.contains(key)));
            assert_eq!(
                fs::metadata(controller.join(name))?.permissions().mode() & 0o777,
                0o600
            );
        }
        let mut rejected = native_job(&inputs)?;
        rejected.launch.connection = Some(record_defaults());
        rejected.launch.argv = vec!["--connection".into(), "local".into()];
        assert!(validate(&rejected).is_err());
        Ok(())
    }

    #[test]
    fn native_default_material_is_validated_before_acceptance() -> Result<()> {
        let mut job = native_job(&[])?;
        job.launch.connection = Some(record_defaults());
        validate(&job)?;
        job.launch.connection.as_mut().unwrap()["jump"] = serde_json::json!([{"host":"hop.test"}]);
        assert!(validate(&job).is_err());
        job.launch.connection = Some(record_defaults());
        job.launch.connection.as_mut().unwrap()["known_hosts_lines"] = serde_json::json!([]);
        assert!(validate(&job).is_err());
        job.launch.connection = Some(record_defaults());
        job.launch.inventory = ResolvedInventory::Managed {
            content: "[all]\nhost.test\n".into(),
        };
        assert!(validate(&job).is_err());
        Ok(())
    }

    #[test]
    fn native_default_paths_have_one_owner() -> Result<()> {
        for key in ["PIPING_RECORD_SSH_CONFIG", "PIPING_RECORD_SSH_KEY_FILE"] {
            let mut job = native_job(&[])?;
            job.launch
                .env
                .insert(key.into(), "/project/override".into());
            assert!(validate(&job).is_err());
            job.launch.env.clear();
            job.launch
                .secrets
                .insert(key.into(), "/project/override".into());
            assert!(validate(&job).is_err());
        }
        Ok(())
    }

    fn job_with_sandbox_run(run_id: Uuid, token: &str) -> Result<RunnerJobRequest> {
        let mut job = native_job(&[
            (
                "site.yml",
                "- hosts: all\n  gather_facts: false\n  tasks: []\n",
            ),
            ("inventory.ini", "[all]\nlocal ansible_connection=local\n"),
        ])?;
        job.launch.targets.push(ResolvedTarget {
            hostname: "host1".into(),
            run_id: Some(run_id),
            target_id: Some(Uuid::from_u128(0x55)),
            source_id: None,
            connection: None,
        });
        if !token.is_empty() {
            job.sandbox_run_tokens.insert(run_id, token.into());
        }
        Ok(job)
    }

    #[test]
    fn sandbox_token_set_must_match_the_dispatched_runs() -> Result<()> {
        let run_id = Uuid::from_u128(0x77);
        assert!(validate(&job_with_sandbox_run(run_id, "")?).is_err());

        let mut foreign = job_with_sandbox_run(run_id, "pjt_sandbox_token")?;
        foreign
            .sandbox_run_tokens
            .insert(Uuid::from_u128(0x78), "pjt_foreign".into());
        assert!(validate(&foreign).is_err());

        let mut empty = job_with_sandbox_run(run_id, "pjt_sandbox_token")?;
        empty.sandbox_run_tokens.insert(run_id, String::new());
        assert!(validate(&empty).is_err());

        validate(&job_with_sandbox_run(run_id, "pjt_sandbox_token")?)?;
        Ok(())
    }

    #[test]
    fn sandbox_tokens_reach_only_a_private_controller_file_and_are_redacted() -> Result<()> {
        let run_id = Uuid::from_u128(0x77);
        let token = "pjt_0123456789abcdef_sandbox";
        let job = job_with_sandbox_run(run_id, token)?;
        let root = tempfile::tempdir()?;
        let prepared = prepare(&job, root.path(), fixture_limits())?;

        let tokens_path = root.path().join("controller/run_tokens.json");
        assert_eq!(
            fs::metadata(&tokens_path)?.permissions().mode() & 0o777,
            0o600
        );
        let stored: BTreeMap<Uuid, String> = serde_json::from_slice(&fs::read(&tokens_path)?)?;
        assert_eq!(stored, job.sandbox_run_tokens);
        assert_eq!(
            prepared.spec.env.get("PIPING_API_URL").map(String::as_str),
            Some(job.callback_base_url.as_str())
        );
        assert_eq!(
            prepared
                .spec
                .env
                .get("PIPING_RUN_TOKENS_FILE")
                .map(String::as_str),
            Some("/job/controller/run_tokens.json")
        );
        assert!(
            prepared
                .known_secrets
                .iter()
                .any(|secret| secret.as_slice() == token.as_bytes())
        );
        assert!(
            prepared
                .spec
                .env
                .values()
                .all(|value| !value.contains(token))
        );
        assert!(prepared.spec.argv.iter().all(|arg| !arg.contains(token)));
        assert!(!job.launch.extra_vars.to_string().contains(token));
        for entry in &job.project.entries {
            if let SnapshotContent::File { bytes, .. } = &entry.content {
                assert!(
                    !bytes
                        .windows(token.len())
                        .any(|window| window == token.as_bytes())
                );
            }
        }
        Ok(())
    }

    #[test]
    fn sandbox_token_delivery_owns_its_environment_names() -> Result<()> {
        let mut job = native_job(&[])?;
        job.launch
            .env
            .insert("PIPING_API_URL".into(), "https://attacker.test".into());
        assert!(validate(&job).is_err());
        job.launch.env.clear();
        job.launch.secrets.insert(
            "PIPING_RUN_TOKENS_FILE".into(),
            "/job/controller/other.json".into(),
        );
        assert!(validate(&job).is_err());
        Ok(())
    }
}
