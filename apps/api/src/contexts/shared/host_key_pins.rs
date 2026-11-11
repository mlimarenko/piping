//! SSH host key pinning: pure `known_hosts` parsing, OpenSSH-format
//! fingerprints, operator repair text, seed-file writing, and the
//! `ssh_host_key_pin` store.
//!
//! Leaf B of PWA-14 owns storage and policy only. The execution paths that
//! probe a host, write a seed file and consume the store live in the proxy,
//! the worker dispatch and the playbook runner; nothing here spawns `ssh` or
//! reaches into the delivery layer. The pin is the
//! authority: a key that does not match the stored fingerprint is never
//! accepted silently, and recovery is a deliberate operator action
//! ([`mismatch_diagnosis`]) rather than an automatic overwrite.

use std::io::Write;
use std::net::Ipv6Addr;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use crate::errors::AppError;

/// Default SSH port for a `known_hosts` token without an explicit `[host]:port`.
pub const DEFAULT_SSH_PORT: u16 = 22;

/// Proxy payload key carrying the `known_hosts` lines a connection must trust.
pub const PROXY_PAYLOAD_KNOWN_HOSTS_LINES: &str = "known_hosts_lines";

/// Why a `known_hosts` line could not be turned into a pin or fingerprint.
#[derive(Debug, Error)]
pub enum HostKeyPinError {
    /// The line was blank or only whitespace.
    #[error("known_hosts line is empty")]
    Empty,
    /// Fewer than the three mandatory fields (host, key type, key blob).
    #[error("known_hosts line is missing the key type or key blob")]
    MissingFields,
    /// A `@cert-authority` / `@revoked` marker, which is not a pinnable host key.
    #[error("known_hosts line carries an unsupported marker: {0}")]
    UnsupportedMarker(String),
    /// The host token did not match `host`, `[host]:port` or a bare IPv6 literal.
    #[error("known_hosts host field is malformed: {0}")]
    MalformedHost(String),
    /// A port outside `1..=65535`.
    #[error("known_hosts port is invalid: {0}")]
    InvalidPort(String),
    /// The key blob was not valid base64.
    #[error("known_hosts key blob is not valid base64")]
    InvalidBase64,
    /// A seed line carries an embedded newline, carriage return or NUL, which
    /// would inject additional `known_hosts` entries.
    #[error("known_hosts line contains an embedded control character")]
    EmbeddedControlCharacter,
    /// The seed file could not be written.
    #[error("failed to write the host key seed file")]
    SeedFile(#[from] std::io::Error),
}

/// A parsed `known_hosts` entry in canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownHostLine {
    /// Host with any `[brackets]` removed and lowercased.
    pub host: String,
    /// Port; [`DEFAULT_SSH_PORT`] when the line carries none.
    pub port: u16,
    /// OpenSSH key algorithm, e.g. `ssh-ed25519`.
    pub key_type: String,
    /// Base64 raw key blob, exactly as it appears in the line.
    pub blob: String,
    /// Canonical OpenSSH `known_hosts` line rebuilt from the parsed fields.
    pub line: String,
    /// `SHA256:` fingerprint over the decoded blob.
    pub fingerprint: String,
}

/// One stored pin row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyPin {
    pub id: Uuid,
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub known_hosts_line: String,
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// Parse one OpenSSH `known_hosts` line into canonical fields.
///
/// Accepts the three shapes OpenSSH writes: a bare host (default port 22), a
/// bracketed `[host]:port`, and a bracketed IPv6 literal such as
/// `[2001:db8::1]:22`. A bare IPv6 literal without brackets is treated as
/// port 22. `@`-prefixed marker lines are rejected: they are certificate
/// authorities or revocations, not host keys to pin.
pub fn parse_known_hosts_line(line: &str) -> Result<KnownHostLine, HostKeyPinError> {
    let mut fields = line.split_whitespace();
    let host_field = fields.next().ok_or(HostKeyPinError::MissingFields)?;
    let key_type = fields.next().ok_or(HostKeyPinError::MissingFields)?;
    let blob = fields.next().ok_or(HostKeyPinError::MissingFields)?;
    let (host, port) = parse_host_field(host_field)?;
    let canonical = format_known_hosts_line(&host, port, key_type, blob);
    let fingerprint = fingerprint_from_blob(blob)?;
    Ok(KnownHostLine {
        host,
        port,
        key_type: key_type.to_string(),
        blob: blob.to_string(),
        line: canonical,
        fingerprint,
    })
}

/// Split the host token into a lowercased host and a port.
fn parse_host_field(field: &str) -> Result<(String, u16), HostKeyPinError> {
    if field.starts_with('@') {
        return Err(HostKeyPinError::UnsupportedMarker(field.to_string()));
    }
    if let Some(rest) = field.strip_prefix('[') {
        let close = rest
            .find(']')
            .ok_or_else(|| HostKeyPinError::MalformedHost(field.to_string()))?;
        let host = &rest[..close];
        if host.is_empty() {
            return Err(HostKeyPinError::MalformedHost(field.to_string()));
        }
        let port_token = rest[close + 1..]
            .strip_prefix(':')
            .ok_or_else(|| HostKeyPinError::MalformedHost(field.to_string()))?;
        if port_token.is_empty() || port_token.contains(':') {
            return Err(HostKeyPinError::MalformedHost(field.to_string()));
        }
        let port: u16 = port_token
            .parse()
            .map_err(|_| HostKeyPinError::InvalidPort(port_token.to_string()))?;
        if port == 0 {
            return Err(HostKeyPinError::InvalidPort(port_token.to_string()));
        }
        return Ok((host.to_ascii_lowercase(), port));
    }
    if field.contains(':') {
        if field.parse::<Ipv6Addr>().is_ok() {
            return Ok((field.to_ascii_lowercase(), DEFAULT_SSH_PORT));
        }
        return Err(HostKeyPinError::MalformedHost(field.to_string()));
    }
    if field.is_empty() {
        return Err(HostKeyPinError::MalformedHost(field.to_string()));
    }
    Ok((field.to_ascii_lowercase(), DEFAULT_SSH_PORT))
}

/// Rebuild the canonical OpenSSH line: bare host at the default port, and the
/// bracketed `[host]:port` form for any other port or any IPv6 literal.
fn format_known_hosts_line(host: &str, port: u16, key_type: &str, blob: &str) -> String {
    if port == DEFAULT_SSH_PORT && !host.contains(':') {
        format!("{host} {key_type} {blob}")
    } else {
        format!("[{host}]:{port} {key_type} {blob}")
    }
}

/// `SHA256:` fingerprint over a base64 raw key blob, in the exact form
/// `ssh-keygen -l -f` prints: standard base64 of the SHA-256 digest, without
/// padding.
pub fn fingerprint_from_blob(blob: &str) -> Result<String, HostKeyPinError> {
    let decoded = STANDARD
        .decode(blob.trim().as_bytes())
        .map_err(|_| HostKeyPinError::InvalidBase64)?;
    let digest = Sha256::digest(decoded);
    Ok(format!("SHA256:{}", STANDARD_NO_PAD.encode(digest)))
}

/// Read the `known_hosts` seed lines from a proxy payload value.
///
/// Lenient by design: a missing key, a non-array value, non-string entries and
/// blank entries all contribute nothing rather than failing the caller. Both
/// the API worker and the proxy read the payload key through this one parser,
/// so they agree on its type handling.
pub fn known_hosts_lines_from_payload(payload: &serde_json::Value) -> Vec<String> {
    payload
        .get(PROXY_PAYLOAD_KNOWN_HOSTS_LINES)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The exact SQL an operator runs to remove the pin that refused a host key.
/// The host is single-quote-escaped because it arrives from a payload.
pub fn delete_pin_statement(host: &str, port: u16) -> String {
    let escaped = host.replace('\'', "''");
    format!("DELETE FROM public.ssh_host_key_pin WHERE host = '{escaped}' AND port = {port};")
}

/// Operator-facing text for a host key that does not match its stored pin.
/// Names the exact `host:port` and stored fingerprint, and the only sanctioned
/// repair: verify the host out of band, delete the pin, reconnect.
pub fn mismatch_diagnosis(host: &str, port: u16, stored_fingerprint: &str) -> String {
    format!(
        "SSH host key mismatch for {host}:{port}: the presented key does not match the pinned \
         fingerprint {stored_fingerprint}. The pin is authoritative and is never overwritten \
         silently. Verify the host out of band, then delete the pinned row with `{}` and \
         reconnect to record the verified key. The sanctioned repair is `delete_pins`.",
        delete_pin_statement(host, port)
    )
}

/// Write `lines` to `path` as a `known_hosts` seed file with mode `0o600`.
///
/// The file is opened with the mode and then forced to `0o600` before any
/// content is written, so a pre-existing file with broader permissions never
/// exposes a key while it is being rewritten.
pub fn write_seed_file(path: &Path, lines: &[String]) -> Result<(), HostKeyPinError> {
    // A line with an embedded newline, carriage return or NUL would append a
    // second `known_hosts` entry, letting a malicious payload pin a key it does
    // not own. Reject before opening so a refused seed leaves no file behind.
    if lines
        .iter()
        .any(|line| line.contains('\n') || line.contains('\r') || line.contains('\0'))
    {
        return Err(HostKeyPinError::EmbeddedControlCharacter);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(Permissions::from_mode(0o600))?;
    }
    for line in lines {
        writeln!(file, "{line}")?;
    }
    file.flush()?;
    Ok(())
}

/// Read every pin for `host`/`port`, ordered by key type. The host is
/// normalized to lowercase to match the stored form.
pub async fn fetch_pins(pool: &PgPool, host: &str, port: u16) -> Result<Vec<HostKeyPin>, AppError> {
    let normalized = host.to_ascii_lowercase();
    let rows = sqlx::query(
        "SELECT id, host, port, key_type, known_hosts_line, fingerprint, created_at, last_seen_at \
         FROM public.ssh_host_key_pin \
         WHERE host = $1 AND port = $2 \
         ORDER BY key_type",
    )
    .bind(&normalized)
    .bind(i32::from(port))
    .fetch_all(pool)
    .await
    .map_err(|error| database_error("fetch SSH host key pins", error))?;

    rows.iter().map(row_to_pin).collect()
}

/// Build the seed-file lines for `host`/`port` from the stored pins, in the
/// same order [`fetch_pins`] returns them.
pub async fn resolve_seed_lines(
    pool: &PgPool,
    host: &str,
    port: u16,
) -> Result<Vec<String>, AppError> {
    let pins = fetch_pins(pool, host, port).await?;
    Ok(seed_lines_from_pins(&pins))
}

/// Project stored pins onto their canonical `known_hosts` lines. Pure, so the
/// seed-file shape is testable without a database.
pub fn seed_lines_from_pins(pins: &[HostKeyPin]) -> Vec<String> {
    pins.iter()
        .map(|pin| pin.known_hosts_line.clone())
        .collect()
}

/// Parse, fingerprint and upsert every line. All lines land in one
/// transaction: a half-written pin set would leave the store inconsistent
/// about which keys are trusted.
///
/// Pinning is trust-on-first-use: an existing `(host, port, key_type)` row
/// keeps its `known_hosts_line` and `fingerprint`; a later observation of a
/// different key only bumps `last_seen_at`. The stored key is therefore never
/// overwritten silently, and recovery stays the deliberate operator action
/// [`delete_pins`].
pub async fn upsert_lines(pool: &PgPool, lines: &[String]) -> Result<(), AppError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| database_error("start SSH host key pin transaction", error))?;

    for line in lines {
        let parsed = parse_known_hosts_line(line).map_err(|error| {
            AppError::Validation(format!("invalid SSH known_hosts line: {error}"))
        })?;
        sqlx::query(
            "INSERT INTO public.ssh_host_key_pin \
                 (host, port, key_type, known_hosts_line, fingerprint) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (host, port, key_type) DO UPDATE SET \
                 last_seen_at = now()",
        )
        .bind(&parsed.host)
        .bind(i32::from(parsed.port))
        .bind(&parsed.key_type)
        .bind(&parsed.line)
        .bind(&parsed.fingerprint)
        .execute(&mut *transaction)
        .await
        .map_err(|error| database_error("upsert SSH host key pin", error))?;
    }

    transaction
        .commit()
        .await
        .map_err(|error| database_error("commit SSH host key pins", error))?;
    Ok(())
}

/// Delete every pin for `host`/`port` and return how many rows were removed.
/// This is the only sanctioned recovery from a key mismatch.
pub async fn delete_pins(pool: &PgPool, host: &str, port: u16) -> Result<u64, AppError> {
    let normalized = host.to_ascii_lowercase();
    let result = sqlx::query("DELETE FROM public.ssh_host_key_pin WHERE host = $1 AND port = $2")
        .bind(&normalized)
        .bind(i32::from(port))
        .execute(pool)
        .await
        .map_err(|error| database_error("delete SSH host key pins", error))?;
    Ok(result.rows_affected())
}

fn row_to_pin(row: &PgRow) -> Result<HostKeyPin, AppError> {
    let raw_port: i32 = row
        .try_get("port")
        .map_err(|error| database_error("read SSH host key pin port", error))?;
    let port = u16::try_from(raw_port).map_err(|_| {
        AppError::Database(format!(
            "stored SSH host key pin has out-of-range port {raw_port}"
        ))
    })?;
    Ok(HostKeyPin {
        id: row
            .try_get("id")
            .map_err(|error| database_error("read SSH host key pin id", error))?,
        host: row
            .try_get("host")
            .map_err(|error| database_error("read SSH host key pin host", error))?,
        port,
        key_type: row
            .try_get("key_type")
            .map_err(|error| database_error("read SSH host key pin key type", error))?,
        known_hosts_line: row
            .try_get("known_hosts_line")
            .map_err(|error| database_error("read SSH host key pin line", error))?,
        fingerprint: row
            .try_get("fingerprint")
            .map_err(|error| database_error("read SSH host key pin fingerprint", error))?,
        created_at: row
            .try_get("created_at")
            .map_err(|error| database_error("read SSH host key pin created_at", error))?,
        last_seen_at: row
            .try_get("last_seen_at")
            .map_err(|error| database_error("read SSH host key pin last_seen_at", error))?,
    })
}

fn database_error(action: &str, error: sqlx::Error) -> AppError {
    AppError::Database(format!("failed to {action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ED25519_BLOB: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIKdwFDb7uXDNTayo3mt/sJyW1Hut13OuWSjKUWvi64be";

    #[test]
    fn parse_known_hosts_line_extracts_host_port_and_key_type() {
        let bracketed = format!("[Example.COM]:2222 ssh-ed25519 {ED25519_BLOB}");
        let parsed = parse_known_hosts_line(&bracketed).expect("bracketed line parses");
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 2222);
        assert_eq!(parsed.key_type, "ssh-ed25519");
        assert_eq!(parsed.blob, ED25519_BLOB);
        assert_eq!(
            parsed.line,
            format!("[example.com]:2222 ssh-ed25519 {ED25519_BLOB}")
        );

        let bare = format!("Host.Example.Org ssh-ed25519 {ED25519_BLOB}");
        let parsed = parse_known_hosts_line(&bare).expect("bare line parses");
        assert_eq!(parsed.host, "host.example.org");
        assert_eq!(parsed.port, 22);
        assert_eq!(
            parsed.line,
            format!("host.example.org ssh-ed25519 {ED25519_BLOB}")
        );

        let ipv6 = format!("[2001:db8::1]:22 ssh-ed25519 {ED25519_BLOB}");
        let parsed = parse_known_hosts_line(&ipv6).expect("ipv6 line parses");
        assert_eq!(parsed.host, "2001:db8::1");
        assert_eq!(parsed.port, 22);
        assert_eq!(
            parsed.line,
            format!("[2001:db8::1]:22 ssh-ed25519 {ED25519_BLOB}")
        );

        let ipv6_port = format!("[2001:db8::1]:2222 ssh-ed25519 {ED25519_BLOB}");
        let parsed = parse_known_hosts_line(&ipv6_port).expect("ipv6 with port parses");
        assert_eq!(parsed.port, 2222);
    }

    #[test]
    fn fingerprint_matches_ssh_keygen_sha256_form() {
        // Stable ed25519 vector generated for this test:
        //   ssh-keygen -t ed25519 -N '' -C 'pwa-14-test-vector' -f /tmp/opencode/pwa14-key
        // Public line:
        //   ssh-ed25519 <ED25519_BLOB> pwa-14-test-vector
        // `ssh-keygen -l -f` reports:
        //   256 SHA256:sG0SMy82lt8Ahb2R9zfKRotu1WVm/6kQ5rbiOcJ/CYg pwa-14-test-vector (ED25519)
        let fingerprint = fingerprint_from_blob(ED25519_BLOB).expect("blob decodes and hashes");
        assert_eq!(
            fingerprint,
            "SHA256:sG0SMy82lt8Ahb2R9zfKRotu1WVm/6kQ5rbiOcJ/CYg"
        );
        assert!(
            !fingerprint.ends_with('='),
            "OpenSSH prints the fingerprint without padding: {fingerprint}"
        );
    }

    #[test]
    fn mismatch_diagnosis_names_host_port_stored_fingerprint_and_repair() {
        let diagnosis = mismatch_diagnosis(
            "host.example.org",
            2222,
            "SHA256:sG0SMy82lt8Ahb2R9zfKRotu1WVm/6kQ5rbiOcJ/CYg",
        );
        assert!(
            diagnosis.contains("host.example.org:2222"),
            "diagnosis must name host:port: {diagnosis}"
        );
        assert!(
            diagnosis.contains("SHA256:sG0SMy82lt8Ahb2R9zfKRotu1WVm/6kQ5rbiOcJ/CYg"),
            "diagnosis must name the stored fingerprint: {diagnosis}"
        );
        assert!(
            diagnosis.contains(
                "DELETE FROM public.ssh_host_key_pin WHERE host = 'host.example.org' AND port = 2222;"
            ),
            "diagnosis must name the executable repair SQL: {diagnosis}"
        );
        assert!(
            diagnosis.contains("delete_pins"),
            "diagnosis must name the repair (`delete_pins`): {diagnosis}"
        );
        assert!(
            diagnosis.contains("reconnect"),
            "diagnosis must say to reconnect after verifying the host: {diagnosis}"
        );
    }

    #[test]
    fn known_hosts_lines_from_payload_is_lenient_and_trims() {
        let payload = json!({
            PROXY_PAYLOAD_KNOWN_HOSTS_LINES: [
                "  host.example.org ssh-ed25519 AAAA  ",
                "host2.example.org ssh-rsa BBBB",
                7,
                "",
                "   ",
            ],
        });
        assert_eq!(
            known_hosts_lines_from_payload(&payload),
            vec![
                "host.example.org ssh-ed25519 AAAA".to_string(),
                "host2.example.org ssh-rsa BBBB".to_string(),
            ],
            "array entries are trimmed strings; non-strings and blanks are skipped"
        );
        assert!(
            known_hosts_lines_from_payload(&json!({})).is_empty(),
            "an absent key contributes no pins"
        );
        assert!(
            known_hosts_lines_from_payload(&json!({ PROXY_PAYLOAD_KNOWN_HOSTS_LINES: null }))
                .is_empty(),
            "a null value contributes no pins"
        );
        assert!(
            known_hosts_lines_from_payload(&json!({ PROXY_PAYLOAD_KNOWN_HOSTS_LINES: "nope" }))
                .is_empty(),
            "a non-array value contributes no pins"
        );
    }

    #[test]
    fn write_seed_file_rejects_embedded_newlines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("known_hosts");
        let injected = format!(
            "host.example.org ssh-ed25519 {ED25519_BLOB}\nevil.example.org ssh-ed25519 {ED25519_BLOB}"
        );

        let error =
            write_seed_file(&path, &[injected]).expect_err("an embedded newline must be rejected");

        assert!(
            error.to_string().contains("embedded control character"),
            "the rejection must name the embedded control character: {error}"
        );
        assert!(
            !path.exists(),
            "a rejected seed must not leave a partially written known_hosts file"
        );
    }

    #[test]
    fn write_seed_file_writes_normal_line_with_owner_only_mode() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("known_hosts");
        let line = format!("host.example.org ssh-ed25519 {ED25519_BLOB}");

        write_seed_file(&path, std::slice::from_ref(&line)).expect("a normal line writes");

        let contents = std::fs::read_to_string(&path).expect("read back the seed file");
        assert_eq!(contents, format!("{line}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("seed file metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the seed file must be owner-only");
        }
    }
}
