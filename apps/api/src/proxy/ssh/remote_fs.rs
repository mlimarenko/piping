use super::*;
pub(super) async fn require_ssh_proxy_entry(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    feature: &'static str,
) -> Result<super::super::ProxyConnection, AppError> {
    let entry = {
        let guard = state.connections.read().await;
        guard.get(&connection_id).cloned()
    }
    .ok_or_else(|| AppError::NotFound("Proxy connection not found".to_string()))?;

    match super::super::parse_connection_type(&entry.payload) {
        Ok(ProxyConnectionType::Ssh) => Ok(entry),
        Ok(_) => Err(AppError::Validation(format!(
            "{feature} is only supported for SSH sessions"
        ))),
        Err(error) => Err(AppError::Validation(error.to_string())),
    }
}

pub(super) fn validate_remote_file_path(path: &str) -> Result<&str, AppError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "Remote path must not be empty".to_string(),
        ));
    }
    if trimmed.chars().count() > SSH_FILE_REMOTE_PATH_MAX_CHARS {
        return Err(AppError::Validation("Remote path is too long".to_string()));
    }
    if trimmed.contains('\0') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(AppError::Validation(
            "Remote path contains unsupported control characters".to_string(),
        ));
    }
    Ok(trimmed)
}

pub(super) fn validate_remote_delete_paths(paths: Vec<String>) -> Result<Vec<String>, AppError> {
    if paths.is_empty() {
        return Err(AppError::Validation(
            "At least one remote path must be selected for deletion".to_string(),
        ));
    }
    if paths.len() > SSH_FILE_DELETE_MAX_PATHS {
        return Err(AppError::Validation(format!(
            "Cannot delete more than {SSH_FILE_DELETE_MAX_PATHS} remote paths at once"
        )));
    }

    let mut unique = Vec::new();
    for path in paths {
        let trimmed = validate_remote_file_path(&path)?.to_string();
        let normalized = trimmed.trim_end_matches('/');
        let has_unsafe_segment = normalized
            .strip_prefix("~/")
            .unwrap_or(normalized)
            .split('/')
            .filter(|segment| !segment.is_empty())
            .any(|segment| segment == "." || segment == "..");
        if normalized.is_empty()
            || normalized == "/"
            || normalized == "."
            || normalized == "~"
            || normalized == ".."
            || has_unsafe_segment
        {
            return Err(AppError::Validation(format!(
                "Remote path is too dangerous to delete: {trimmed}"
            )));
        }
        if !unique.iter().any(|existing| existing == &trimmed) {
            unique.push(trimmed);
        }
    }
    Ok(unique)
}

pub(super) fn shell_single_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Render the generated, local authentication-marker pathname for OpenSSH's
/// shell-executed `LocalCommand`. The pathname is not user input, but quoting
/// remains mandatory because `TMPDIR` can contain shell metacharacters. Escape
/// `%` first because OpenSSH expands LocalCommand tokens before invoking the
/// shell.
pub(super) fn ssh_authentication_marker_local_command(marker: &Path) -> String {
    let token_escaped_path = marker.to_string_lossy().replace('%', "%%");
    format!(
        "{SSH_AUTHENTICATION_MARKER_TOUCH} -- {}",
        shell_single_quote(&token_escaped_path)
    )
}

pub(super) fn shell_quote_remote_path(value: &str) -> String {
    if value == "~" {
        return "~".to_string();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return format!("~/{}", shell_single_quote(rest));
    }
    shell_single_quote(value)
}

pub(super) fn build_remote_list_command(path: &str) -> String {
    let path = shell_quote_remote_path(path);
    format!(
        "cd -- {path} || exit 2; printf '%s\\0' \"$PWD\"; find . -mindepth 1 -maxdepth 1 -printf '%f\\0%y\\0%s\\0%T@\\0%m\\0' 2>/dev/null"
    )
}

pub(super) fn build_remote_mkdir_command(path: &str) -> String {
    format!("mkdir -p -- {}", shell_quote_remote_path(path))
}

pub(super) fn build_remote_delete_command(paths: &[String]) -> String {
    let args = paths
        .iter()
        .map(|path| shell_quote_remote_path(path))
        .collect::<Vec<_>>()
        .join(" ");
    format!("rm -rf -- {args}")
}

pub(super) fn build_remote_move_command(paths: &[String], destination: &str) -> String {
    let destination = shell_quote_remote_path(destination);
    let args = paths
        .iter()
        .map(|path| shell_quote_remote_path(path))
        .collect::<Vec<_>>()
        .join(" ");
    format!("mkdir -p -- {destination} && mv -t {destination} -- {args}")
}

pub(super) fn build_remote_download_preflight_command(path: &str) -> String {
    let path = shell_quote_remote_path(path);
    format!(
        "[ -f {path} ] || exit 3; size=$(stat -c '%s' -- {path}) || exit 4; printf 'file\\0%s\\0' \"$size\""
    )
}

pub(super) fn build_remote_archive_preflight_command(path: &str) -> String {
    let path = shell_quote_remote_path(path);
    format!(
        "[ -d {path} ] || exit 3; bytes=$(du -sb -- {path} 2>/dev/null | awk '{{print $1}}') || exit 4; entries=$(find {path} -mindepth 0 -printf . 2>/dev/null | wc -c) || exit 4; printf 'directory\\0%s\\0%s\\0' \"$bytes\" \"$entries\""
    )
}

pub(super) fn build_remote_preview_command(path: &str, max_bytes: usize) -> String {
    let path = shell_quote_remote_path(path);
    let read_bytes = max_bytes.saturating_add(1);
    format!(
        "[ -f {path} ] || exit 3; size=$(stat -c '%s' -- {path}) || exit 4; printf 'file\\0%s\\0' \"$size\"; head -c {read_bytes} -- {path}"
    )
}

pub(super) fn build_remote_download_command(path: &str) -> String {
    format!("cat -- {}", shell_quote_remote_path(path))
}

pub(super) fn build_remote_archive_download_command(path: &str) -> String {
    let contents_archive = archive_directory_contents_requested(path);
    let path = shell_quote_remote_path(path);
    if contents_archive {
        return format!("cd -- {path} || exit 4; [ -d . ] || exit 3; tar -czf - -- .");
    }
    format!(
        "target={path}; [ -d \"$target\" ] || exit 3; parent=$(dirname -- \"$target\") || exit 4; name=$(basename -- \"$target\") || exit 4; cd -- \"$parent\" || exit 4; tar -czf - -- \"$name\""
    )
}

pub(super) fn archive_directory_contents_requested(path: &str) -> bool {
    let trimmed = path.trim();
    trimmed == "/" || trimmed.trim_end_matches('/') == "." || trimmed.trim_end_matches('/') == "~"
}

pub(super) fn enforce_download_size_limit(bytes: u64, max_bytes: u64) -> Result<(), AppError> {
    if bytes > max_bytes {
        return Err(AppError::PayloadTooLarge(format!(
            "Remote file exceeds configured download limit of {max_bytes} bytes"
        )));
    }
    Ok(())
}

pub(super) fn enforce_archive_entry_limit(entries: u64) -> Result<(), AppError> {
    if entries > SSH_FILE_ARCHIVE_MAX_ENTRIES {
        return Err(AppError::PayloadTooLarge(format!(
            "Remote directory archive exceeds configured entry limit of {SSH_FILE_ARCHIVE_MAX_ENTRIES}"
        )));
    }
    Ok(())
}

pub(in crate::proxy) fn remote_file_name(path: &str, archive: bool) -> String {
    let base = path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::trim)
        .filter(|part| !part.is_empty() && *part != "." && *part != ".." && *part != "~")
        .unwrap_or("download")
        .to_string();
    if archive && !base.ends_with(".tar.gz") {
        format!("{base}.tar.gz")
    } else {
        base
    }
}

pub(super) fn parse_download_preflight_output(
    remote_path: &str,
    archive: bool,
    output: &[u8],
    max_bytes: u64,
) -> Result<AgentSshFileDownloadPreflight, AppError> {
    let parts: Vec<&[u8]> = output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect();
    if archive {
        if parts.len() != 3 || parts[0] != b"directory" {
            return Err(AppError::External(
                "Remote directory archive metadata is not readable".to_string(),
            ));
        }
        let logical_bytes = parse_ascii_u64(parts[1], "Remote directory size is not readable")?;
        let entry_count =
            parse_ascii_u64(parts[2], "Remote directory entry count is not readable")?;
        enforce_download_size_limit(logical_bytes, max_bytes)?;
        enforce_archive_entry_limit(entry_count)?;
        return Ok(AgentSshFileDownloadPreflight {
            file_name: remote_file_name(remote_path, true),
            path: remote_path.to_string(),
            bytes: None,
            logical_bytes: Some(logical_bytes),
            entry_count: Some(entry_count),
            kind: "directory".to_string(),
            archive: true,
        });
    }

    if parts.len() != 2 || parts[0] != b"file" {
        return Err(AppError::External(
            "Remote file size is not readable".to_string(),
        ));
    }
    let bytes = parse_ascii_u64(parts[1], "Remote file size is not readable")?;
    enforce_download_size_limit(bytes, max_bytes)?;
    Ok(AgentSshFileDownloadPreflight {
        file_name: remote_file_name(remote_path, false),
        path: remote_path.to_string(),
        bytes: Some(bytes),
        logical_bytes: Some(bytes),
        entry_count: Some(1),
        kind: "file".to_string(),
        archive: false,
    })
}

pub(super) fn parse_remote_preview_output(
    remote_path: &str,
    output: &[u8],
    max_preview_bytes: usize,
    stream_truncated: bool,
) -> Result<AgentSshFilePreview, AppError> {
    let first_nul = output.iter().position(|byte| *byte == 0).ok_or_else(|| {
        AppError::External("Remote file preview metadata is not readable".to_string())
    })?;
    if &output[..first_nul] != b"file" {
        return Err(AppError::External(
            "Remote file preview metadata is not readable".to_string(),
        ));
    }
    let rest = &output[first_nul + 1..];
    let second_nul = rest.iter().position(|byte| *byte == 0).ok_or_else(|| {
        AppError::External("Remote file preview size is not readable".to_string())
    })?;
    let bytes = parse_ascii_u64(
        &rest[..second_nul],
        "Remote file preview size is not readable",
    )?;
    let mut preview_bytes = rest[second_nul + 1..].to_vec();
    let limit_truncated = preview_bytes.len() > max_preview_bytes;
    if limit_truncated {
        preview_bytes.truncate(max_preview_bytes);
    }
    if preview_bytes.contains(&0) {
        return Err(AppError::Validation(
            "Remote file does not look like a text file".to_string(),
        ));
    }
    let preview_bytes_len = preview_bytes.len() as u64;
    let content = String::from_utf8(preview_bytes)
        .map_err(|_| AppError::Validation("Remote file is not valid UTF-8 text".to_string()))?;
    Ok(AgentSshFilePreview {
        path: remote_path.to_string(),
        bytes,
        preview_bytes: preview_bytes_len,
        truncated: stream_truncated || limit_truncated || bytes > preview_bytes_len,
        content,
    })
}

pub(super) fn parse_ascii_u64(raw: &[u8], message: &'static str) -> Result<u64, AppError> {
    let value = std::str::from_utf8(raw)
        .map_err(|_| AppError::External(message.to_string()))?
        .trim()
        .parse::<u64>()
        .map_err(|_| AppError::External(message.to_string()))?;
    Ok(value)
}

pub(super) fn parse_remote_list_output(output: &[u8]) -> Result<AgentSshFileList, AppError> {
    let parts: Vec<&[u8]> = output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect();
    let Some((cwd_raw, fields)) = parts.split_first() else {
        return Err(AppError::External(
            "Remote directory listing returned no path".to_string(),
        ));
    };
    let (chunks, remainder) = fields.as_chunks::<5>();
    if !remainder.is_empty() {
        return Err(AppError::External(
            "Remote directory listing returned malformed metadata".to_string(),
        ));
    }

    let cwd = String::from_utf8_lossy(cwd_raw).into_owned();
    let mut entries = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let name = String::from_utf8_lossy(chunk[0]).into_owned();
        let kind_raw = String::from_utf8_lossy(chunk[1]);
        let size_raw = String::from_utf8_lossy(chunk[2]);
        let mtime_raw = String::from_utf8_lossy(chunk[3]);
        let mode_raw = String::from_utf8_lossy(chunk[4]);
        let kind = match kind_raw.as_ref() {
            "d" => "directory",
            "f" => "file",
            "l" => "symlink",
            _ => "other",
        }
        .to_string();
        let path = join_remote_path(&cwd, &name);
        entries.push(AgentSshFileEntry {
            name,
            path,
            kind,
            size: size_raw.parse::<u64>().ok(),
            modified_at: parse_remote_mtime(&mtime_raw),
            permissions: if mode_raw.is_empty() {
                None
            } else {
                Some(format!("0{mode_raw}"))
            },
        });
    }

    entries.sort_by(|left, right| {
        let left_dir = left.kind == "directory";
        let right_dir = right.kind == "directory";
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    Ok(AgentSshFileList {
        parent: remote_parent_path(&cwd),
        path: cwd,
        entries,
    })
}

pub(super) fn parse_remote_mtime(raw: &str) -> Option<String> {
    let timestamp = raw.parse::<f64>().ok()?;
    if !timestamp.is_finite() {
        return None;
    }
    let seconds = timestamp.trunc() as i64;
    let nanos = (timestamp.fract().abs() * 1_000_000_000.0).round() as u32;
    Utc.timestamp_opt(seconds, nanos.min(999_999_999))
        .single()
        .map(|value| value.to_rfc3339())
}

pub(super) fn join_remote_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}

pub(super) fn remote_parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(index) => Some(trimmed[..index].to_string()),
        None => Some(".".to_string()),
    }
}

pub(super) fn ssh_stream_excerpt(bytes: &[u8]) -> String {
    let value = String::from_utf8_lossy(bytes);
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "remote command failed".to_string();
    }
    trimmed.chars().take(STDERR_EXCERPT_CHAR_LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nul_joined(parts: &[&str]) -> Vec<u8> {
        parts.join("\0").into_bytes()
    }

    #[test]
    fn parse_remote_list_output_groups_fields_in_order() {
        let payload = nul_joined(&[
            "/srv/data",
            "a.txt",
            "f",
            "12",
            "1700000000.0",
            "644",
            "b.txt",
            "f",
            "34",
            "1700000001.5",
            "755",
        ]);

        let listing = parse_remote_list_output(&payload).expect("valid listing");

        assert_eq!(listing.path, "/srv/data");
        assert_eq!(listing.entries.len(), 2);
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
        assert!(listing.entries.iter().all(|entry| entry.kind == "file"));
        assert_eq!(listing.entries[0].size, Some(12));
        assert_eq!(listing.entries[1].size, Some(34));
        assert!(listing.entries[0].modified_at.is_some());
        assert!(listing.entries[1].modified_at.is_some());
        assert_eq!(listing.entries[0].permissions.as_deref(), Some("0644"));
        assert_eq!(listing.entries[1].permissions.as_deref(), Some("0755"));
    }

    #[test]
    fn parse_remote_list_output_rejects_partial_group() {
        let payload = nul_joined(&[
            "/srv/data",
            "a.txt",
            "f",
            "12",
            "1700000000.0",
            "644",
            "extra",
        ]);

        let error = parse_remote_list_output(&payload).expect_err("partial group must be rejected");
        assert!(
            error.to_string().contains("malformed metadata"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn parse_remote_list_output_with_only_cwd_returns_no_entries() {
        let payload = nul_joined(&["/srv/data"]);

        let listing = parse_remote_list_output(&payload).expect("cwd-only listing");
        assert_eq!(listing.path, "/srv/data");
        assert!(listing.entries.is_empty());
    }
}
