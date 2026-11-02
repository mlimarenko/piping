use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Component, Path};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use once_cell::sync::Lazy;
use piping_contracts::playbook_runner::RUNNER_PER_HOST_KEY_DIR;
use regex::Regex;
use serde_json::Value;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::timeout;

use crate::contexts::playbooks::domain::PlaybookProject;
use crate::contexts::playbooks::infrastructure::AnsibleProjectFile;
use crate::errors::AppError;

const MAX_PROJECT_FILES: usize = 2_048;
const MAX_PROJECT_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROJECT_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Matches `scheme://userinfo@` so any credential a caller embedded directly in
/// an SCM URL is scrubbed before the surrounding text can reach an error, a log
/// line, or a run-group console event.
static RE_URL_USERINFO: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"([a-zA-Z][a-zA-Z0-9+.\-]*://)[^/@\s]+@").ok());

/// Redacts `scheme://user:secret@host` userinfo down to `scheme://***@host` so
/// git output that echoes the remote URL cannot leak credentials.
fn redact_url_userinfo(text: &str) -> String {
    RE_URL_USERINFO.as_ref().map_or_else(
        || text.to_string(),
        |regex| regex.replace_all(text, "$1***@").into_owned(),
    )
}

/// A resolved SCM credential for a git checkout.
///
/// The secret material is deliberately kept off the process argv and off the
/// clone URL: HTTP tokens are supplied through a `GIT_ASKPASS` helper (the token
/// lives only in the child process environment) and SSH keys through a `0600`
/// key file referenced by `GIT_SSH_COMMAND`.
#[derive(Clone)]
pub enum ScmCredential {
    /// HTTP(S) token or password authentication (e.g. a personal access token).
    Token(String),
    /// SSH private-key material in PEM form.
    SshKey(String),
}

impl std::fmt::Debug for ScmCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render secret material, even in debug output or panic messages.
        match self {
            Self::Token(_) => f.write_str("ScmCredential::Token(<redacted>)"),
            Self::SshKey(_) => f.write_str("ScmCredential::SshKey(<redacted>)"),
        }
    }
}

/// Classifies resolved secret material into an SCM credential kind. PEM
/// private-key markers select SSH-key authentication; anything else is treated
/// as an HTTP token/password.
#[must_use]
pub fn classify_scm_credential(value: &str) -> ScmCredential {
    let trimmed = value.trim_start();
    if trimmed.contains("-----BEGIN") && trimmed.contains("PRIVATE KEY-----") {
        ScmCredential::SshKey(value.to_string())
    } else {
        ScmCredential::Token(value.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct ProjectBundle {
    pub entrypoint: String,
    pub files: Vec<AnsibleProjectFile>,
}

pub async fn build_project_bundle(
    project: &PlaybookProject,
    playbook_path: &str,
    credential: Option<ScmCredential>,
) -> Result<ProjectBundle, AppError> {
    let entrypoint =
        join_project_playbook_path(project.playbook_base_path.as_deref(), playbook_path)?;
    let files = match project.scm_type.as_str() {
        "manual" => manual_project_files(&project.metadata)?,
        "git" => {
            let checkout = checkout_git_project(project, credential.as_ref()).await?;
            let root = checkout.path().join("repo");
            tokio::task::spawn_blocking(move || collect_project_files(&root))
                .await
                .map_err(|error| {
                    AppError::External(format!("Project bundle task failed: {error}"))
                })??
        }
        other => {
            return Err(AppError::Validation(format!(
                "Unsupported project scm_type for launch: {other}"
            )));
        }
    };
    validate_project_bundle(&files)?;
    if !files.iter().any(|file| file.path == entrypoint) {
        return Err(AppError::Validation(format!(
            "Project playbook entrypoint {entrypoint} is not present in project files"
        )));
    }
    Ok(ProjectBundle { entrypoint, files })
}

/// Validates and normalizes a caller-supplied set of manual project files
/// (e.g. an upload) before it is persisted into `metadata.project_files`.
///
/// This reuses the exact same guards the launch path enforces on a project
/// bundle: relative POSIX paths only, no parent traversal, no controller-owned
/// `ansible.cfg`, no file inside the runner-reserved `.piping_keys/` directory,
/// no duplicate paths, and the per-file / total byte ceilings.
/// Symlinks are structurally impossible in a JSON payload so there is nothing to
/// skip. On success each returned file carries the normalized (POSIX-joined)
/// path so what is stored round-trips cleanly back through the launch reader.
pub fn validate_manual_project_files(
    files: Vec<AnsibleProjectFile>,
) -> Result<Vec<AnsibleProjectFile>, AppError> {
    if files.len() > MAX_PROJECT_FILES {
        return Err(AppError::Validation(
            "Project bundle has too many files".to_string(),
        ));
    }
    let mut seen = HashSet::new();
    let mut total_bytes = 0usize;
    let mut normalized = Vec::with_capacity(files.len());
    for file in files {
        let path = validate_project_file_path(&file.path)?;
        if !seen.insert(path.clone()) {
            return Err(AppError::Validation(format!(
                "Project bundle contains duplicate path {path}"
            )));
        }
        if file.content.len() > MAX_PROJECT_FILE_BYTES {
            return Err(AppError::Validation(format!(
                "Project file {path} is too large"
            )));
        }
        total_bytes = total_bytes.saturating_add(file.content.len());
        if total_bytes > MAX_PROJECT_TOTAL_BYTES {
            return Err(AppError::Validation(
                "Project bundle is too large".to_string(),
            ));
        }
        normalized.push(AnsibleProjectFile {
            path,
            content: file.content,
            executable: file.executable,
        });
    }
    Ok(normalized)
}

pub fn join_project_playbook_path(
    base_path: Option<&str>,
    playbook_path: &str,
) -> Result<String, AppError> {
    let joined = match base_path.map(str::trim).filter(|value| !value.is_empty()) {
        Some(base_path) => format!("{base_path}/{playbook_path}"),
        None => playbook_path.to_string(),
    };
    validate_project_path(&joined, "project playbook path")
}

fn manual_project_files(metadata: &Value) -> Result<Vec<AnsibleProjectFile>, AppError> {
    let files = metadata.get("project_files").ok_or_else(|| {
        AppError::Validation(
            "manual playbook projects require metadata.project_files for launch".to_string(),
        )
    })?;
    parse_project_files(files)
}

/// Reads the stored `metadata.project_files` set for the read-back endpoint.
///
/// Unlike the launch reader, an absent set is not an error: a project that never
/// received a manual upload (including a git project) legitimately has no stored
/// files and reads back as an empty list rather than a 4xx.
pub fn stored_project_files(metadata: &Value) -> Result<Vec<AnsibleProjectFile>, AppError> {
    match metadata.get("project_files") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(files) => parse_project_files(files),
    }
}

fn parse_project_files(files: &Value) -> Result<Vec<AnsibleProjectFile>, AppError> {
    serde_json::from_value(files.clone()).map_err(|error| {
        AppError::Validation(format!(
            "manual project metadata.project_files is invalid: {error}"
        ))
    })
}

async fn checkout_git_project(
    project: &PlaybookProject,
    credential: Option<&ScmCredential>,
) -> Result<TempDir, AppError> {
    let scm_url = project.scm_url.as_deref().ok_or_else(|| {
        AppError::Validation("git playbook project is missing scm_url".to_string())
    })?;
    checkout_git_source(
        scm_url,
        project.scm_branch.as_deref(),
        project.scm_ref.as_deref(),
        credential,
    )
    .await
}

/// Shallow-checks-out `scm_url` into a fresh TempDir (under `repo/`), applying
/// the shared non-interactive git env, timeout, and off-argv credential
/// plumbing. Shared by user git projects and the official catalog refresh.
pub(crate) async fn checkout_git_source(
    scm_url: &str,
    scm_branch: Option<&str>,
    scm_ref: Option<&str>,
    credential: Option<&ScmCredential>,
) -> Result<TempDir, AppError> {
    let temp_dir = tempfile::Builder::new()
        .prefix("piping-playbook-project-")
        .tempdir()
        .map_err(|error| {
            AppError::External(format!("Failed to create project checkout dir: {error}"))
        })?;
    let repo_dir = temp_dir.path().join("repo");
    // Secret material is materialized once, inside the checkout TempDir, so it is
    // torn down with the checkout and is referenced only via the child env.
    let credential_env = prepare_git_credential(temp_dir.path(), credential)?;

    let scm_ref = scm_ref.map(str::trim).filter(|value| !value.is_empty());
    let mut clone = Command::new("git");
    clone.arg("clone").arg("--no-tags").arg("--depth").arg("1");
    if scm_ref.is_none()
        && let Some(branch) = scm_branch.map(str::trim).filter(|value| !value.is_empty())
    {
        clone.arg("--branch").arg(branch);
    }
    clone.arg(scm_url).arg(&repo_dir);
    apply_git_env(&mut clone, &credential_env);
    run_git_command(clone, "clone playbook project").await?;

    if let Some(revision) = scm_ref {
        let mut fetch = Command::new("git");
        fetch
            .arg("-C")
            .arg(&repo_dir)
            .arg("fetch")
            .arg("--depth")
            .arg("1")
            .arg("origin")
            .arg(revision);
        apply_git_env(&mut fetch, &credential_env);
        run_git_command(fetch, "fetch playbook project revision").await?;

        let mut checkout = Command::new("git");
        checkout
            .arg("-C")
            .arg(&repo_dir)
            .arg("checkout")
            .arg("--detach")
            .arg("FETCH_HEAD");
        apply_git_env(&mut checkout, &credential_env);
        run_git_command(checkout, "checkout playbook project revision").await?;
    }

    Ok(temp_dir)
}

/// Applies the non-interactive base git environment plus any credential env vars
/// to `command`. `GIT_TERMINAL_PROMPT=0` guarantees git never blocks on an
/// interactive prompt; `GIT_CONFIG_NOSYSTEM=1` ignores host-wide git config.
fn apply_git_env(command: &mut Command, credential_env: &[(String, String)]) {
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for (key, value) in credential_env {
        command.env(key, value);
    }
}

/// Materializes credential helpers inside `temp_dir` and returns the environment
/// variables that wire git to them. Returns an empty vector when no credential
/// is supplied (anonymous checkout).
///
/// Tokens never touch argv or the URL: a `GIT_ASKPASS` shell helper reads the
/// token from the child environment. SSH keys are written `0600` and referenced
/// through `GIT_SSH_COMMAND` with an explicit, non-interactive host-key policy.
fn prepare_git_credential(
    temp_dir: &Path,
    credential: Option<&ScmCredential>,
) -> Result<Vec<(String, String)>, AppError> {
    let mut env = Vec::new();
    match credential {
        None => {}
        Some(ScmCredential::Token(token)) => {
            let askpass = temp_dir.join("git-askpass.sh");
            write_askpass_script(&askpass)?;
            env.push((
                "GIT_ASKPASS".to_string(),
                askpass.to_string_lossy().into_owned(),
            ));
            // Deterministic, non-interactive username for token auth. The token
            // itself is only ever read by the askpass helper from this env var.
            env.push((
                "PIPING_GIT_USERNAME".to_string(),
                "x-access-token".to_string(),
            ));
            env.push(("PIPING_GIT_TOKEN".to_string(), token.clone()));
        }
        Some(ScmCredential::SshKey(key)) => {
            let key_path = temp_dir.join("scm_key");
            write_private_key(&key_path, key)?;
            let known_hosts = temp_dir.join("known_hosts");
            std::fs::File::create(&known_hosts).map_err(|error| {
                AppError::External(format!("Failed to create git known_hosts store: {error}"))
            })?;
            // `accept-new` trusts a host on first contact but rejects a changed
            // key thereafter — the only workable non-interactive policy for
            // arbitrary git hosts under GIT_TERMINAL_PROMPT=0.
            let ssh_command = format!(
                "ssh -i \"{key}\" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=\"{known_hosts}\"",
                key = key_path.to_string_lossy(),
                known_hosts = known_hosts.to_string_lossy(),
            );
            env.push(("GIT_SSH_COMMAND".to_string(), ssh_command));
        }
    }
    Ok(env)
}

/// Writes an executable askpass helper that answers git's username/password
/// prompts from the child environment, keeping the token off argv.
fn write_askpass_script(path: &Path) -> Result<(), AppError> {
    // git invokes the helper with the prompt text as $1; usernames get the
    // configured principal, every other prompt (password) gets the token.
    let script = "#!/bin/sh\ncase \"$1\" in\n  Username*|username*) printf '%s' \"${PIPING_GIT_USERNAME:-x-access-token}\" ;;\n  *) printf '%s' \"$PIPING_GIT_TOKEN\" ;;\nesac\n";
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o700);
    let mut file = options.open(path).map_err(|error| {
        AppError::External(format!("Failed to create git askpass helper: {error}"))
    })?;
    file.write_all(script.as_bytes()).map_err(|error| {
        AppError::External(format!("Failed to write git askpass helper: {error}"))
    })?;
    Ok(())
}

/// Writes SSH private-key material to a `0600` file created with restrictive
/// permissions up front (never world-readable, even briefly).
fn write_private_key(path: &Path, key: &str) -> Result<(), AppError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| AppError::External(format!("Failed to create SCM key file: {error}")))?;
    file.write_all(key.as_bytes())
        .map_err(|error| AppError::External(format!("Failed to write SCM key file: {error}")))?;
    if !key.ends_with('\n') {
        file.write_all(b"\n").map_err(|error| {
            AppError::External(format!("Failed to finalize SCM key file: {error}"))
        })?;
    }
    Ok(())
}

async fn run_git_command(mut command: Command, operation: &str) -> Result<(), AppError> {
    let output = timeout(GIT_COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| AppError::External(format!("{operation} timed out")))?
        .map_err(|error| AppError::External(format!("{operation} failed to start: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = redact_url_userinfo(stderr.trim());
    let message = message.trim();
    Err(AppError::External(if message.is_empty() {
        format!("{operation} failed with status {}", output.status)
    } else {
        format!("{operation} failed: {message}")
    }))
}

fn collect_project_files(root: &Path) -> Result<Vec<AnsibleProjectFile>, AppError> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let mut total_bytes = 0usize;
    collect_project_files_inner(root, root, &mut files, &mut seen, &mut total_bytes)?;
    Ok(files)
}

fn collect_project_files_inner(
    root: &Path,
    current: &Path,
    files: &mut Vec<AnsibleProjectFile>,
    seen: &mut HashSet<String>,
    total_bytes: &mut usize,
) -> Result<(), AppError> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| AppError::External(format!("Failed to read project dir: {error}")))?
    {
        let entry = entry.map_err(|error| {
            AppError::External(format!("Failed to read project entry: {error}"))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            AppError::External(format!("Failed to inspect project entry: {error}"))
        })?;
        if entry.file_name() == OsStr::new(".git") {
            continue;
        }
        if file_type.is_symlink() {
            return Err(AppError::Validation(format!(
                "Project bundle must not contain symlink {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_project_files_inner(root, &path, files, seen, total_bytes)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        collect_regular_project_file(root, &path, files, seen, total_bytes)?;
    }
    Ok(())
}

/// Validates a single regular file discovered under the checkout and, if it
/// passes every guard (path normalization, dedup, count/byte ceilings, UTF-8),
/// appends it to `files`. Extracted from the directory walk to keep that walk's
/// control flow shallow.
fn collect_regular_project_file(
    root: &Path,
    path: &Path,
    files: &mut Vec<AnsibleProjectFile>,
    seen: &mut HashSet<String>,
    total_bytes: &mut usize,
) -> Result<(), AppError> {
    let relative = path.strip_prefix(root).map_err(|error| {
        AppError::External(format!("Failed to relativize project file: {error}"))
    })?;
    let project_path = path_to_project_string(relative)?;
    validate_project_file_path(&project_path)?;
    if !seen.insert(project_path.clone()) {
        return Err(AppError::Validation(format!(
            "Project bundle contains duplicate path {project_path}"
        )));
    }
    if files.len() >= MAX_PROJECT_FILES {
        return Err(AppError::Validation(
            "Project bundle has too many files".to_string(),
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        AppError::External(format!(
            "Failed to read project file {}: {error}",
            path.display()
        ))
    })?;
    if bytes.len() > MAX_PROJECT_FILE_BYTES {
        return Err(AppError::Validation(format!(
            "Project file {project_path} is too large"
        )));
    }
    *total_bytes = total_bytes.saturating_add(bytes.len());
    if *total_bytes > MAX_PROJECT_TOTAL_BYTES {
        return Err(AppError::Validation(
            "Project bundle is too large".to_string(),
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| AppError::Validation(format!("Project file {project_path} is not UTF-8")))?;
    files.push(AnsibleProjectFile {
        path: project_path,
        content,
        executable: is_executable(path)?,
    });
    Ok(())
}

fn validate_project_bundle(files: &[AnsibleProjectFile]) -> Result<(), AppError> {
    if files.len() > MAX_PROJECT_FILES {
        return Err(AppError::Validation(
            "Project bundle has too many files".to_string(),
        ));
    }
    let mut seen = HashSet::new();
    let mut total_bytes = 0usize;
    for file in files {
        validate_project_file_path(&file.path)?;
        if !seen.insert(file.path.clone()) {
            return Err(AppError::Validation(format!(
                "Project bundle contains duplicate path {}",
                file.path
            )));
        }
        if file.content.len() > MAX_PROJECT_FILE_BYTES {
            return Err(AppError::Validation(format!(
                "Project file {} is too large",
                file.path
            )));
        }
        total_bytes = total_bytes.saturating_add(file.content.len());
        if total_bytes > MAX_PROJECT_TOTAL_BYTES {
            return Err(AppError::Validation(
                "Project bundle is too large".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_project_file_path(value: &str) -> Result<String, AppError> {
    let normalized = validate_project_path(value, "project file")?;
    if normalized == "ansible.cfg" {
        return Err(AppError::Validation(
            "Project bundle must not include ansible.cfg; use typed runner settings".to_string(),
        ));
    }
    // Mirror the runner's own guard (`path.starts_with(RUNNER_PER_HOST_KEY_DIR)`):
    // the reserved directory itself and everything below it are controller-owned
    // key material, so a caller-supplied bundle must never carry either. The
    // `{dir}/` boundary keeps a sibling such as `.piping_keys.example` accepted.
    let reserved_prefix = format!("{RUNNER_PER_HOST_KEY_DIR}/");
    if normalized == RUNNER_PER_HOST_KEY_DIR || normalized.starts_with(&reserved_prefix) {
        return Err(AppError::Validation(format!(
            "Project bundle must not include files under the runner-reserved {RUNNER_PER_HOST_KEY_DIR}/ directory"
        )));
    }
    Ok(normalized)
}

pub(crate) fn validate_project_path(value: &str, field: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!(
            "{field} path must not be empty"
        )));
    }
    if trimmed.contains('\\') || trimmed.contains('\0') {
        return Err(AppError::Validation(format!(
            "{field} path must be a relative POSIX path"
        )));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(AppError::Validation(format!(
            "{field} path must be relative"
        )));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let Some(part) = part.to_str() else {
                    return Err(AppError::Validation(format!("{field} path must be UTF-8")));
                };
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::Validation(format!(
                    "{field} path must not contain traversal"
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(AppError::Validation(format!(
            "{field} path must contain a file name"
        )));
    }
    Ok(parts.join("/"))
}

fn path_to_project_string(path: &Path) -> Result<String, AppError> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            return Err(AppError::Validation(
                "Project file path must be relative".to_string(),
            ));
        };
        let Some(part) = part.to_str() else {
            return Err(AppError::Validation(
                "Project file path must be UTF-8".to_string(),
            ));
        };
        parts.push(part.to_string());
    }
    validate_project_path(&parts.join("/"), "project file")
}

fn is_executable(path: &Path) -> Result<bool, AppError> {
    #[cfg(unix)]
    {
        let metadata = std::fs::metadata(path).map_err(|error| {
            AppError::External(format!(
                "Failed to read project file mode {}: {error}",
                path.display()
            ))
        })?;
        Ok(metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn manual_project(metadata: Value) -> PlaybookProject {
        PlaybookProject {
            id: Uuid::now_v7(),
            name: "Manual project".to_string(),
            description: None,
            scm_type: "manual".to_string(),
            scm_url: None,
            scm_branch: None,
            scm_ref: None,
            playbook_base_path: Some("playbooks".to_string()),
            credential_id: None,
            default_execution_environment_id: None,
            sync_policy: "manual".to_string(),
            sync_status: "never_synced".to_string(),
            last_sync_revision: None,
            last_sync_error: None,
            last_synced_at: None,
            metadata,
            created_by_user_id: None,
            updated_by_user_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn manual_project_bundle_uses_base_path_entrypoint() {
        let project = manual_project(json!({
            "project_files": [
                {
                    "path": "playbooks/site.yml",
                    "content": "---\n- hosts: all\n  tasks: []\n"
                },
                {
                    "path": "roles/common/tasks/main.yml",
                    "content": "---\n[]\n"
                }
            ]
        }));
        let bundle = build_project_bundle(&project, "site.yml", None)
            .await
            .unwrap();
        assert_eq!(bundle.entrypoint, "playbooks/site.yml");
        assert_eq!(bundle.files.len(), 2);
    }

    #[tokio::test]
    async fn manual_project_bundle_rejects_missing_entrypoint() {
        let project = manual_project(json!({
            "project_files": [
                {
                    "path": "playbooks/other.yml",
                    "content": "---\n- hosts: all\n"
                }
            ]
        }));
        let error = build_project_bundle(&project, "site.yml", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("entrypoint playbooks/site.yml"));
    }

    #[tokio::test]
    async fn manual_project_bundle_rejects_ansible_cfg() {
        let project = manual_project(json!({
            "project_files": [
                {
                    "path": "playbooks/site.yml",
                    "content": "---\n- hosts: all\n"
                },
                {
                    "path": "ansible.cfg",
                    "content": "[defaults]\n"
                }
            ]
        }));
        let error = build_project_bundle(&project, "site.yml", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ansible.cfg"));
    }

    fn git_project(scm_url: &str) -> PlaybookProject {
        PlaybookProject {
            scm_type: "git".to_string(),
            scm_url: Some(scm_url.to_string()),
            playbook_base_path: Some("playbooks".to_string()),
            ..manual_project(json!({}))
        }
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("git command spawns");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo_with_playbook(dir: &Path) {
        run_git(dir, &["init", "-q", "-b", "main"]);
        run_git(dir, &["config", "user.email", "test@example.com"]);
        run_git(dir, &["config", "user.name", "Test Runner"]);
        std::fs::create_dir_all(dir.join("playbooks")).unwrap();
        std::fs::write(
            dir.join("playbooks/site.yml"),
            "---\n- hosts: all\n  tasks: []\n",
        )
        .unwrap();
        run_git(dir, &["add", "."]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn classify_scm_credential_detects_ssh_key() {
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n";
        assert!(matches!(
            classify_scm_credential(pem),
            ScmCredential::SshKey(_)
        ));
        assert!(matches!(
            classify_scm_credential("  \n-----BEGIN RSA PRIVATE KEY-----\nx\n"),
            ScmCredential::SshKey(_)
        ));
        assert!(matches!(
            classify_scm_credential("ghp_exampletoken"),
            ScmCredential::Token(_)
        ));
    }

    #[test]
    fn redact_url_userinfo_scrubs_credentials() {
        let text = "fatal: could not read from 'https://alice:s3cret@git.example.com/repo.git'";
        let redacted = redact_url_userinfo(text);
        assert!(!redacted.contains("s3cret"));
        assert!(!redacted.contains("alice"));
        assert!(redacted.contains("https://***@git.example.com"));
        // URLs without userinfo are left intact.
        assert_eq!(
            redact_url_userinfo("clone https://git.example.com/repo.git"),
            "clone https://git.example.com/repo.git"
        );
    }

    #[test]
    fn scm_credential_debug_never_leaks_secret() {
        let token = format!("{:?}", ScmCredential::Token("ghp_supersecret".to_string()));
        assert!(!token.contains("ghp_supersecret"));
        let key = format!("{:?}", ScmCredential::SshKey("PRIVATE".to_string()));
        assert!(!key.contains("PRIVATE"));
    }

    #[test]
    fn write_private_key_uses_restrictive_mode() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("scm_key");
        write_private_key(&path, "keydata").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "keydata\n");
        #[cfg(unix)]
        {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // create_new rejects an existing key file rather than overwriting it.
        assert!(write_private_key(&path, "other").is_err());
    }

    #[tokio::test]
    async fn git_project_bundle_clones_with_token_credential() {
        let remote = tempfile::tempdir().unwrap();
        init_repo_with_playbook(remote.path());
        let url = format!("file://{}", remote.path().display());
        let project = git_project(&url);
        // The token credential is materialized (askpass helper written) even
        // though a local file transport does not exercise it; this proves the
        // credential plumbing does not break an otherwise valid checkout.
        let bundle = build_project_bundle(
            &project,
            "site.yml",
            Some(ScmCredential::Token("ghp_example".to_string())),
        )
        .await
        .unwrap();
        assert_eq!(bundle.entrypoint, "playbooks/site.yml");
        assert!(bundle.files.iter().any(|f| f.path == "playbooks/site.yml"));
    }

    #[tokio::test]
    async fn git_project_bundle_rejects_missing_scm_url() {
        let mut project = git_project("file:///tmp/does-not-matter");
        project.scm_url = None;
        let error = build_project_bundle(&project, "site.yml", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing scm_url"));
    }

    fn upload_file(path: &str, content: &str) -> AnsibleProjectFile {
        AnsibleProjectFile {
            path: path.to_string(),
            content: content.to_string(),
            executable: false,
        }
    }

    #[test]
    fn validate_manual_project_files_normalizes_and_accepts_relative_paths() {
        let files = validate_manual_project_files(vec![
            upload_file("./playbooks/site.yml", "---\n- hosts: all\n"),
            upload_file("roles/common/tasks/main.yml", "---\n[]\n"),
        ])
        .expect("relative project files are accepted");
        assert_eq!(files.len(), 2);
        // Leading `./` is normalized away so the stored path round-trips.
        assert_eq!(files[0].path, "playbooks/site.yml");
    }

    #[test]
    fn validate_manual_project_files_rejects_parent_traversal() {
        let error =
            validate_manual_project_files(vec![upload_file("../../etc/passwd", "root:x:0:0")])
                .unwrap_err();
        assert!(matches!(error, AppError::Validation(_)));
        assert!(error.to_string().contains("traversal"));
    }

    #[test]
    fn validate_manual_project_files_rejects_absolute_path() {
        let error =
            validate_manual_project_files(vec![upload_file("/etc/hosts", "x")]).unwrap_err();
        assert!(error.to_string().contains("relative"));
    }

    #[test]
    fn validate_manual_project_files_rejects_ansible_cfg() {
        let error = validate_manual_project_files(vec![upload_file("ansible.cfg", "[defaults]\n")])
            .unwrap_err();
        assert!(error.to_string().contains("ansible.cfg"));
    }

    #[test]
    fn validate_manual_project_files_rejects_reserved_key_directory() {
        // Mirrors the runner guard `path.starts_with(".piping_keys")`: both the
        // reserved directory itself and any file below it are rejected.
        for path in [".piping_keys", ".piping_keys/host1", "./.piping_keys/db-02"] {
            let error =
                validate_manual_project_files(vec![upload_file(path, "key-material")]).unwrap_err();
            assert!(
                matches!(error, AppError::Validation(_)),
                "{path} must be rejected as a reserved key path"
            );
            assert!(error.to_string().contains(".piping_keys"));
        }
    }

    #[test]
    fn validate_manual_project_files_accepts_piping_keys_prefixed_sibling() {
        // A file whose name merely starts with the reserved directory name is
        // not *inside* it; the runner accepts it too (component-wise prefix).
        let files = validate_manual_project_files(vec![upload_file(".piping_keys.example", "x")])
            .expect("a similarly named sibling is not inside the reserved directory");
        assert_eq!(files[0].path, ".piping_keys.example");
    }

    #[test]
    fn validate_manual_project_files_rejects_duplicate_paths() {
        let error = validate_manual_project_files(vec![
            upload_file("playbooks/site.yml", "a"),
            upload_file("./playbooks/site.yml", "b"),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("duplicate"));
    }

    #[test]
    fn validate_manual_project_files_rejects_oversized_file() {
        let big = "x".repeat(MAX_PROJECT_FILE_BYTES + 1);
        let error = validate_manual_project_files(vec![upload_file("big.txt", &big)]).unwrap_err();
        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn uploaded_project_files_round_trip_through_stored_metadata() {
        // The exact encoding `set_project_files` writes and the read-back
        // endpoint parses: validate -> serialize into metadata -> parse back.
        let uploaded = vec![
            upload_file("./playbooks/site.yml", "---\n- hosts: all\n"),
            AnsibleProjectFile {
                path: "roles/common/tasks/main.yml".to_string(),
                content: "---\n[]\n".to_string(),
                executable: true,
            },
        ];
        let normalized = validate_manual_project_files(uploaded).unwrap();
        let metadata = json!({ "project_files": serde_json::to_value(&normalized).unwrap() });
        let read_back = stored_project_files(&metadata).expect("stored files parse back");
        assert_eq!(read_back, normalized);
        // Normalization is persisted, so the path round-trips without `./`.
        assert_eq!(read_back[0].path, "playbooks/site.yml");
        assert!(read_back[1].executable);
    }

    #[test]
    fn stored_project_files_reads_empty_when_absent_or_null() {
        // A project that never received an upload (or a git project) must read
        // back as an empty list, not a 4xx.
        assert!(stored_project_files(&json!({})).unwrap().is_empty());
        assert!(
            stored_project_files(&json!({ "project_files": null }))
                .unwrap()
                .is_empty()
        );
    }
}
