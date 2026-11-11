use super::*;

/// Write one private OpenSSH configuration shared by the target and every
/// recursively spawned ProxyJump client. Each hop gets only its own identity.
pub(super) async fn write_ssh_route_config(
    dir: &Path,
    launch: &SshLaunchConfig,
    target_key: Option<&str>,
    jump_keys: &[String],
    known_hosts: &Path,
    connect_timeout_secs: u64,
) -> Result<PathBuf, AppError> {
    fn quote(value: &str) -> Result<String, AppError> {
        if value.chars().any(|c| c.is_control()) {
            return Err(AppError::Validation(
                "Invalid SSH configuration path".to_string(),
            ));
        }
        Ok(format!(
            "\"{}\"",
            value
                .replace('%', "%%")
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        ))
    }
    struct Endpoint<'a> {
        alias: &'a str,
        host: &'a str,
        port: u16,
        user: &'a str,
        key: Option<&'a str>,
        previous: Option<&'a str>,
        jump: bool,
    }
    fn stanza(
        output: &mut String,
        endpoint: Endpoint<'_>,
        known_hosts: &str,
        seeds: &[String],
        connect_timeout_secs: u64,
    ) -> Result<(), AppError> {
        use std::fmt::Write as _;
        let Endpoint {
            alias,
            host,
            port,
            user,
            key,
            previous,
            jump,
        } = endpoint;
        // A stored pin is mandatory for that endpoint. Empty seeds retain the
        // proxy's first-contact TOFU policy, never a disabled host-key check.
        let checking = if stored_fingerprint_for(host, port, seeds).is_some() {
            "yes"
        } else {
            "accept-new"
        };
        write!(output, "Host {alias}\n  HostName {host}\n  Port {port}\n  User {user}\n  ConnectTimeout {connect_timeout_secs}\n  UserKnownHostsFile {known_hosts}\n  GlobalKnownHostsFile /dev/null\n  StrictHostKeyChecking {checking}\n  HashKnownHosts no\n").expect("writing to String cannot fail");
        if let Some(key) = key {
            writeln!(output, "  IdentityFile {}", quote(key)?)
                .expect("writing to String cannot fail");
        }
        if jump || key.is_some() {
            output.push_str("  IdentitiesOnly yes\n");
        }
        if jump {
            output.push_str("  BatchMode yes\n");
        }
        if let Some(previous) = previous {
            writeln!(output, "  ProxyJump {previous}").expect("writing to String cannot fail");
        }
        Ok(())
    }
    let seeds: Vec<String> = tokio::fs::read_to_string(known_hosts)
        .await
        .map_err(|error| {
            AppError::External(format!("Failed to read SSH known_hosts seed: {error}"))
        })?
        .lines()
        .map(str::to_string)
        .collect();
    let known_hosts = known_hosts
        .to_str()
        .ok_or_else(|| AppError::Validation("Invalid SSH known_hosts path".to_string()))?;
    let known_hosts = quote(known_hosts)?;
    let mut config = String::new();
    let mut keys = jump_keys.iter();
    for (index, hop) in launch.jump.iter().enumerate() {
        if hop
            .private_key
            .as_deref()
            .is_none_or(|key| key.trim().is_empty())
        {
            return Err(AppError::Validation(
                "SSH jump hop requires a resolved private key".to_string(),
            ));
        }
        let key = Some(
            keys.next()
                .ok_or_else(|| AppError::Internal("Missing SSH jump key material".to_string()))?
                .as_str(),
        );
        let previous = index.checked_sub(1).map(|i| format!("piping-jump-{i}"));
        stanza(
            &mut config,
            Endpoint {
                alias: &format!("piping-jump-{index}"),
                host: &hop.host,
                port: hop.port,
                user: &hop.username,
                key,
                previous: previous.as_deref(),
                jump: true,
            },
            &known_hosts,
            &seeds,
            connect_timeout_secs,
        )?;
    }
    let previous = launch
        .jump
        .len()
        .checked_sub(1)
        .map(|i| format!("piping-jump-{i}"));
    stanza(
        &mut config,
        Endpoint {
            alias: "piping-target",
            host: &launch.host,
            port: launch.port,
            user: &launch.username,
            key: target_key,
            previous: previous.as_deref(),
            jump: false,
        },
        &known_hosts,
        &seeds,
        connect_timeout_secs,
    )?;
    let path = dir.join("ssh_config");
    let write_path = path.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(write_path)?;
        file.write_all(config.as_bytes())
    })
    .await
    .map_err(|error| AppError::External(format!("SSH config task failed: {error}")))?
    .map_err(|error| AppError::External(format!("Failed to write SSH config: {error}")))?;
    Ok(path)
}
/// Allocates fresh, owner-only material for exactly one interactive SSH
/// runtime. The path contains only the generation UUID rather than the
/// connection UUID so the control socket stays safely below Unix `sun_path`
/// limits. Existing directories are never reused: UUID collision retries are
/// bounded and a prior generation's socket is never unlinked by a successor.
pub(super) async fn create_ssh_session_material() -> Result<SshSessionMaterial, AppError> {
    const CREATE_ATTEMPTS: usize = 4;

    for _ in 0..CREATE_ATTEMPTS {
        let generation = Uuid::now_v7().simple().to_string();
        let dir = std::env::temp_dir().join(format!("piping-ssh-session-{generation}"));
        let create_dir = {
            let dir = dir.clone();
            move || -> std::io::Result<()> {
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&dir)
            }
        };
        match tokio::task::spawn_blocking(create_dir)
            .await
            .map_err(|error| {
                AppError::External(format!("SSH session material task failed: {error}"))
            })? {
            Ok(()) => {
                let control_path = dir.join("control.sock");
                return Ok(SshSessionMaterial {
                    authentication_marker: dir.join(SSH_AUTHENTICATION_MARKER_FILE),
                    control_path: control_path.to_string_lossy().into_owned(),
                    dir,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(AppError::External(format!(
                    "Failed to create SSH session material directory: {error}"
                )));
            }
        }
    }

    Err(AppError::External(
        "Failed to allocate a unique SSH session material directory".to_string(),
    ))
}

pub(super) fn ssh_transfer_material_dir(connection_id: Uuid, operation_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "piping-ssh-transfer-{connection_id}-{operation_id}"
    ))
}

pub(super) fn ssh_exec_material_dir(connection_id: Uuid, operation_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("piping-ssh-exec-{connection_id}-{operation_id}"))
}

pub(super) async fn cleanup_ssh_session_material(material: &SshSessionMaterial) {
    let _ = tokio::fs::remove_dir_all(&material.dir).await;
}

/// Completes every old-runtime side effect before publishing terminal state.
/// The cached-session registry uses terminal state as replacement eligibility,
/// so this ordering prevents old metric writes or shared-directory cleanup from
/// overwriting a newly-created runtime's state or SSH material.
pub(super) async fn publish_ssh_terminal_after_finalization<F>(
    session: &SshSession,
    finalization: F,
    close_code: u16,
    close_reason: String,
) where
    F: Future<Output = ()>,
{
    finalization.await;
    session.mark_ended(close_code, close_reason).await;
}

pub(super) async fn cleanup_spawned_ssh_startup_failure(
    child: &mut tokio::process::Child,
    material: &SshSessionMaterial,
) {
    let _ = child.kill().await;
    let _ = child.wait().await;
    cleanup_ssh_session_material(material).await;
}

pub(super) async fn ensure_ssh_material_dir(dir: PathBuf) -> Result<PathBuf, AppError> {
    let create_dir = {
        let dir = dir.clone();
        move || -> std::io::Result<()> {
            match std::fs::create_dir(&dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
    };
    tokio::task::spawn_blocking(create_dir)
        .await
        .map_err(|error| AppError::External(format!("SSH key dir task failed: {}", error)))?
        .map_err(|error| {
            AppError::External(format!("Failed to create SSH key directory: {}", error))
        })?;
    Ok(dir)
}

/// Write a private key to `dir/name` with create-with-mode `0600` and `O_EXCL`,
/// so the file is never world-readable for an instant and a predictable path can
/// never be pre-created/symlinked by another user (finding 4a).
pub(super) async fn write_key_file_excl(
    dir: PathBuf,
    name: String,
    private_key: &str,
) -> Result<String, AppError> {
    let normalized_private_key = normalize_ssh_private_key(private_key);
    let path = dir.join(&name);
    let write_path = path.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&write_path)?;
        file.write_all(normalized_private_key.as_bytes())?;
        file.flush()?;
        Ok(())
    })
    .await
    .map_err(|error| AppError::External(format!("SSH key write task failed: {}", error)))?
    .map_err(|error| AppError::External(format!("Failed to write SSH private key: {}", error)))?;
    Ok(path.to_string_lossy().into_owned())
}

pub(super) async fn write_private_key_file(
    dir: &Path,
    private_key: &str,
) -> Result<String, AppError> {
    write_key_file_excl(dir.to_path_buf(), "target.key".to_string(), private_key).await
}

pub(super) async fn write_jump_private_key_file(
    dir: &Path,
    hop_index: usize,
    private_key: &str,
) -> Result<String, AppError> {
    write_key_file_excl(
        dir.to_path_buf(),
        format!("jump-{}.key", hop_index),
        private_key,
    )
    .await
}

/// Write an `SSH_ASKPASS` helper script into the generation-scoped temp directory
/// and return its path. The script prints the password that `ssh` looks up via
/// the askpass mechanism, replacing the former `sshpass` wrapper (which broke
/// terminal-resize propagation by interposing its own pty). The password is
/// never written to disk: the helper reads it from the `PIPING_SSH_ASKPASS_SECRET`
/// environment variable, which is set only on the spawned `ssh` process. The
/// script is created `0700` (owner-only) in a `0700` directory and removed with
/// the rest of the generation-scoped material on teardown.
pub(super) async fn write_askpass_helper(dir: PathBuf) -> Result<String, AppError> {
    write_askpass_helper_in_dir(dir, "askpass.sh".to_string()).await
}

pub(super) async fn write_askpass_helper_in_dir(
    dir: PathBuf,
    name: String,
) -> Result<String, AppError> {
    const ASKPASS_SCRIPT: &str = "#!/bin/sh\nprintf '%s\\n' \"$PIPING_SSH_ASKPASS_SECRET\"\n";
    let path = dir.join(name);
    let write_path = path.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        let mut file = options.open(&write_path)?;
        file.write_all(ASKPASS_SCRIPT.as_bytes())?;
        file.flush()?;
        Ok(())
    })
    .await
    .map_err(|error| AppError::External(format!("SSH askpass write task failed: {}", error)))?
    .map_err(|error| {
        AppError::External(format!("Failed to write SSH askpass helper: {}", error))
    })?;
    Ok(path.to_string_lossy().into_owned())
}

pub(super) fn normalize_ssh_private_key(value: &str) -> String {
    let normalized_line_endings = value.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized_line_endings.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut normalized = trimmed.to_string();
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    normalized
}
