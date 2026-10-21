use super::*;
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
