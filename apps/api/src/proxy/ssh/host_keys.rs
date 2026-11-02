use super::*;
/// The per-operation OpenSSH `known_hosts` file every ssh/scp child this module
/// spawns pins as its only trust source. It is materialized for each operation
/// even when the seed is empty, so a first-contact key can be harvested.
pub(super) fn job_known_hosts_path(dir: &Path) -> PathBuf {
    dir.join(JOB_KNOWN_HOSTS_FILE)
}

/// Materialize the seed `known_hosts` file for one operation with mode `0600`
/// and return its path and the seed lines. A failure is fatal for the
/// operation: continuing without the file would fall back to OpenSSH defaults
/// instead of the pinned trust source.
pub(super) fn seed_job_known_hosts(
    dir: &Path,
    payload: &Value,
) -> Result<(PathBuf, Vec<String>), AppError> {
    let lines = known_hosts_lines_from_payload(payload);
    let path = job_known_hosts_path(dir);
    write_seed_file(&path, &lines).map_err(|error| {
        AppError::External(format!(
            "Failed to write SSH known_hosts seed file {}: {error}",
            path.display()
        ))
    })?;
    Ok((path, lines))
}

/// Append the sanctioned pinned host-key options. The contract returns them as
/// one whitespace-separated string; the temp paths they contain have no spaces.
pub(super) fn append_pinned_host_key_options(command: &mut Command, known_hosts: &Path) {
    for option in pinned_host_key_options(known_hosts).split_whitespace() {
        command.arg(option);
    }
}

/// Read the operation's `known_hosts` file back and return the non-comment
/// lines that were not part of the seed. These are the keys OpenSSH learned by
/// trust-on-first-use and the API must persist.
pub(super) async fn harvest_new_host_keys(path: &Path, seeded: &[String]) -> Vec<String> {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| !seeded.iter().any(|seed| seed.trim() == *line))
        .map(ToString::to_string)
        .collect()
}

/// Record any keys the process learned on first use for this connection.
pub(super) async fn record_harvested_host_keys(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    known_hosts: &Path,
    seed_lines: &[String],
) {
    let new_lines = harvest_new_host_keys(known_hosts, seed_lines).await;
    super::super::metrics::record_connection_observed_host_keys(metrics, &new_lines).await;
}

/// The fingerprint of the seed line that pins `host`/`port`, when the payload
/// carried one. Used to name the stored fingerprint in a mismatch refusal.
pub(super) fn stored_fingerprint_for(
    host: &str,
    port: u16,
    seed_lines: &[String],
) -> Option<String> {
    let normalized = host.to_ascii_lowercase();
    seed_lines.iter().find_map(|line| {
        parse_known_hosts_line(line)
            .ok()
            .filter(|parsed| parsed.host == normalized && parsed.port == port)
            .map(|parsed| parsed.fingerprint)
    })
}

/// Pure classifier for an OpenSSH host-key failure in captured stderr. Returns
/// the user-visible refusal naming `host:port`, the stored fingerprint when the
/// seed carries one, and the only sanctioned repair: verify the host out of
/// band, delete the pin, reconnect.
pub(super) fn classify_ssh_host_key_failure(
    stderr: &str,
    host: &str,
    port: u16,
    seed_lines: &[String],
) -> Option<AppError> {
    let is_host_key_failure = stderr.contains(SSH_HOST_KEY_CHANGED_MARKER)
        || stderr.contains(SSH_HOST_KEY_VERIFICATION_FAILED_MARKER);
    if !is_host_key_failure {
        return None;
    }
    let message = match stored_fingerprint_for(host, port, seed_lines) {
        Some(fingerprint) => mismatch_diagnosis(host, port, &fingerprint),
        None => format!(
            "SSH host key mismatch for {host}:{port}: the presented key did not match the pinned \
             key. The pin is authoritative and is never overwritten silently. Verify the host out \
             of band, then delete the pinned row with `{}` and reconnect to record the verified \
             key.",
            delete_pin_statement(host, port)
        ),
    };
    Some(AppError::External(message))
}

/// Classify captured stderr for a host-key refusal, but only when the child
/// process actually failed. A successful remote command can print the same
/// markers as ordinary output; only a non-zero exit or a timeout makes stderr
/// evidence. This is the non-interactive counterpart of the interactive path's
/// `!authenticated_handshake` gate.
pub(super) fn classify_failed_ssh_host_key_failure(
    timed_out: bool,
    success: bool,
    stderr: &str,
    host: &str,
    port: u16,
    seed_lines: &[String],
) -> Option<AppError> {
    if !timed_out && success {
        return None;
    }
    classify_ssh_host_key_failure(stderr, host, port, seed_lines)
}
