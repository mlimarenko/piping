//! Inventory + credential mapping for Ansible runs.
//!
//! Translates a resolved run target (a `ConnectionTarget` host plus the in-memory
//! SSH launch material the connection context already resolved through the
//! secrets-canonical fence) into the inputs the Ansible runner sidecar needs:
//! an ini inventory line with per-host connection vars, the ephemeral SSH key,
//! and any host-scoped SSH/WinRM/become passwords.
//!
//! The backend reuses the SAME credential resolution path as interactive SSH
//! (`resolve_ssh_execution_launch` → `proxy_payload`), so this module only reads
//! the already-fenced `host`/`port`/`credentials.{username,password,private_key}`
//! out of that payload — it never resolves secrets itself.

use std::collections::BTreeMap;

use piping_contracts::playbook_runner::RUNNER_PER_HOST_KEY_DIR;
use serde_json::Value;

use crate::contexts::playbooks::domain::PlaybookConnection;
use crate::errors::AppError;

/// Prompt regex matched against the `BECOME password:` sudo prompt by runner
/// implementations that support generic prompt responses. Prefer host-scoped
/// `ansible_become_password` for grouped jobs; this constant is kept for
/// non-host-scoped prompts that may be added later.
pub const BECOME_PASSWORD_PROMPT: &str = "^BECOME password:";

/// The fully-resolved Ansible job inputs for a single target host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsibleTargetInventory {
    /// One ini inventory line: `<name> ansible_host=.. ansible_connection=.. ..`.
    pub inventory_line: String,
    /// Ephemeral SSH private key (PEM) for ssh connections; `None` for winrm or
    /// when the credential is password-only.
    pub ssh_key: Option<String>,
    /// Prompt-regex → response map for prompts that are not host-scoped.
    /// SSH/WinRM/become credentials are emitted as inventory vars.
    pub passwords: BTreeMap<String, String>,
}

/// Build the inventory + credential mapping for one target from the resolved SSH
/// launch `proxy_payload` (the in-memory, secret-bearing launch material).
///
/// `host_name` is the inventory host alias (the target's name/slug). `connection`
/// selects the transport (`ssh`/`winrm`). The payload's `credentials.username`
/// becomes `ansible_user`; `credentials.private_key` becomes the ephemeral
/// `ssh_key`; `credentials.password` becomes either the ssh/winrm password
/// connection var (delivered as an extravar-style inventory var, never argv).
pub fn build_target_inventory(
    host_name: &str,
    connection: PlaybookConnection,
    proxy_payload: &Value,
) -> Result<AnsibleTargetInventory, AppError> {
    build_target_inventory_with_alias(
        host_name,
        host_name,
        connection,
        proxy_payload,
        &BTreeMap::new(),
    )
}

/// Build target inventory with an explicit already-unique inventory alias.
pub fn build_target_inventory_with_alias(
    _host_name: &str,
    host_alias: &str,
    connection: PlaybookConnection,
    proxy_payload: &Value,
    host_vars: &BTreeMap<String, String>,
) -> Result<AnsibleTargetInventory, AppError> {
    if matches!(connection, PlaybookConnection::Local) {
        let alias = sanitize_host_alias(host_alias);
        let mut line = String::new();
        line.push_str(&alias);
        push_var(&mut line, "ansible_connection", connection.as_str());
        push_vars(&mut line, host_vars);
        line.push('\n');
        return Ok(AnsibleTargetInventory {
            inventory_line: line,
            ssh_key: None,
            passwords: BTreeMap::new(),
        });
    }

    let host = payload_str(proxy_payload, &["host"]).ok_or_else(|| {
        AppError::Validation("resolved SSH launch payload is missing host".to_string())
    })?;
    let port = proxy_payload
        .get("port")
        .and_then(Value::as_u64)
        .filter(|p| *p > 0);

    let credentials = proxy_payload.get("credentials");
    let username = credentials
        .and_then(|c| c.get("username"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::Validation(
                "resolved SSH launch payload is missing credentials.username".to_string(),
            )
        })?;
    // Preserve the PEM body verbatim (including its trailing newline) — OpenSSH
    // key parsers are newline-sensitive, so only blank keys are dropped.
    let private_key = credentials
        .and_then(|c| c.get("private_key"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    let password = credentials
        .and_then(|c| c.get("password"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let alias = sanitize_host_alias(host_alias);
    let mut line = String::new();
    line.push_str(&alias);
    push_var(&mut line, "ansible_host", &host);
    push_var(&mut line, "ansible_connection", connection.as_str());
    push_var(&mut line, "ansible_user", username);
    if let Some(port) = port {
        // `Local` returned above; both ssh and winrm map the port to
        // `ansible_port`.
        push_var(&mut line, "ansible_port", &port.to_string());
    }
    push_vars(&mut line, host_vars);

    let mut ssh_key = None;
    let passwords = BTreeMap::new();
    match connection {
        PlaybookConnection::Ssh => {
            // Key auth is preferred; the PEM is materialized by the sidecar into
            // `env/ssh_key` (0600). A password-only credential falls back to
            // host-scoped inventory vars. Do not put SSH/become passwords in the
            // runner prompt-response map: one grouped ansible-playbook command
            // may contain many hosts with different credentials.
            ssh_key = private_key;
            if ssh_key.is_none()
                && let Some(password) = &password
            {
                push_var(&mut line, "ansible_ssh_pass", password);
                push_var(&mut line, "ansible_become_password", password);
            }
        }
        PlaybookConnection::Winrm => {
            // WinRM authenticates with a password; pywinrm in the EE handles the
            // transport. (Windows credential resolution is sequenced in P4; the
            // mapping is wired now so the contract is canonical.)
            if let Some(password) = password {
                push_var(&mut line, "ansible_password", &password);
            }
            push_var(&mut line, "ansible_winrm_transport", "ntlm");
        }
        PlaybookConnection::Local => {
            return Err(AppError::Internal(
                "local connection returns before credential mapping".to_string(),
            ));
        }
    }
    line.push('\n');

    Ok(AnsibleTargetInventory {
        inventory_line: line,
        ssh_key,
        passwords,
    })
}

/// Rewrite a target's inventory line to point its SSH key auth at the per-host
/// key file the runner materializes at `.piping_keys/<alias>`
/// (`RUNNER_PER_HOST_KEY_DIR`). Used for grouped runs whose hosts carry
/// distinct key material, where the group-wide ssh-agent key cannot serve every
/// host. The path is relative to the play working directory (the project root),
/// which is the seam the runner honors. `alias` must be the same
/// already-sanitized alias used for the inventory line and the target entry.
pub fn with_per_host_key_file(inventory_line: &str, alias: &str) -> String {
    let base = inventory_line.strip_suffix('\n').unwrap_or(inventory_line);
    format!("{base} ansible_ssh_private_key_file={RUNNER_PER_HOST_KEY_DIR}/{alias}\n")
}

/// Read the first present string value at any of `keys` from a JSON object.
fn payload_str(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

/// Append ` key=value` to an inventory line. Values with whitespace are quoted.
fn push_var(line: &mut String, key: &str, value: &str) {
    line.push(' ');
    line.push_str(key);
    line.push('=');
    if value.chars().any(char::is_whitespace) {
        line.push('"');
        line.push_str(&value.replace('"', "\\\""));
        line.push('"');
    } else {
        line.push_str(value);
    }
}

fn push_vars(line: &mut String, vars: &BTreeMap<String, String>) {
    for (key, value) in vars {
        push_var(line, key, value);
    }
}

/// Reduce an arbitrary target name to a safe Ansible inventory host alias.
/// Keep the transport alias ASCII-only: Ansible itself supports broader host
/// names, but legacy shells, callback streams and temporary paths are less
/// predictable with Unicode aliases during large grouped runs.
pub fn sanitize_host_alias(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "target".to_string();
    }
    let mut alias = String::new();
    let mut previous_separator = false;
    for c in trimmed.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            alias.push(c);
            previous_separator = false;
        } else if !previous_separator {
            alias.push('_');
            previous_separator = true;
        }
    }
    let alias = alias
        .trim_matches(|c| matches!(c, '_' | '.' | '-'))
        .to_string();
    if alias.is_empty() {
        "target".to_string()
    } else {
        alias
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ssh_payload(private_key: Option<&str>, password: Option<&str>) -> Value {
        let mut creds = serde_json::Map::new();
        creds.insert("username".to_string(), json!("root"));
        if let Some(key) = private_key {
            creds.insert("private_key".to_string(), json!(key));
        }
        if let Some(pass) = password {
            creds.insert("password".to_string(), json!(pass));
        }
        json!({ "host": "10.0.0.5", "port": 2222, "credentials": creds })
    }

    #[test]
    fn ssh_key_target_builds_inventory_line_and_key() {
        let result = build_target_inventory(
            "web-01",
            PlaybookConnection::Ssh,
            &ssh_payload(Some("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n"), None),
        )
        .expect("inventory");
        assert_eq!(
            result.inventory_line,
            "web-01 ansible_host=10.0.0.5 ansible_connection=ssh ansible_user=root ansible_port=2222\n"
        );
        assert_eq!(
            result.ssh_key.as_deref(),
            Some("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n")
        );
        assert!(result.passwords.is_empty());
    }

    #[test]
    fn ssh_password_target_falls_back_to_password_var_and_become() {
        let result = build_target_inventory(
            "db-01",
            PlaybookConnection::Ssh,
            &ssh_payload(None, Some("hunter2")),
        )
        .expect("inventory");
        assert!(result.inventory_line.contains("ansible_connection=ssh"));
        assert!(result.inventory_line.contains("ansible_ssh_pass=hunter2"));
        assert!(
            result
                .inventory_line
                .contains("ansible_become_password=hunter2")
        );
        assert!(result.ssh_key.is_none());
        assert!(result.passwords.is_empty());
    }

    #[test]
    fn winrm_target_uses_password_and_transport() {
        let payload = json!({
            "host": "win-01.example.com",
            "port": 5986,
            "credentials": { "username": "Administrator", "password": "P@ss" }
        });
        let result = build_target_inventory("win-01", PlaybookConnection::Winrm, &payload)
            .expect("inventory");
        assert!(result.inventory_line.contains("ansible_connection=winrm"));
        assert!(result.inventory_line.contains("ansible_user=Administrator"));
        assert!(result.inventory_line.contains("ansible_password=P@ss"));
        assert!(
            result
                .inventory_line
                .contains("ansible_winrm_transport=ntlm")
        );
        assert!(result.ssh_key.is_none());
    }

    #[test]
    fn local_target_does_not_require_managed_host_credentials() {
        let result =
            build_target_inventory("controller job", PlaybookConnection::Local, &json!({}))
                .expect("inventory");
        assert_eq!(
            result.inventory_line,
            "controller_job ansible_connection=local\n"
        );
        assert!(result.ssh_key.is_none());
        assert!(result.passwords.is_empty());
    }

    #[test]
    fn host_scoped_piping_vars_are_added_to_inventory_line() {
        let mut vars = BTreeMap::new();
        vars.insert("piping_run_id".to_string(), "run-1".to_string());
        vars.insert("piping_source_kind".to_string(), "device".to_string());
        vars.insert("piping_source_id".to_string(), "source-1".to_string());
        vars.insert(
            "piping_target_kind".to_string(),
            "connection_target".to_string(),
        );
        vars.insert("piping_target_id".to_string(), "target-1".to_string());
        vars.insert("piping_target_name".to_string(), "Cash desk 1".to_string());

        let result = build_target_inventory_with_alias(
            "web-01",
            "web-01",
            PlaybookConnection::Ssh,
            &ssh_payload(Some("k\n"), None),
            &vars,
        )
        .expect("inventory");

        for expected in [
            "piping_run_id=run-1",
            "piping_source_kind=device",
            "piping_source_id=source-1",
            "piping_target_kind=connection_target",
            "piping_target_id=target-1",
            "piping_target_name=\"Cash desk 1\"",
        ] {
            assert!(result.inventory_line.contains(expected), "{expected}");
        }
    }

    #[test]
    fn missing_host_or_username_is_rejected() {
        let no_host = json!({ "credentials": { "username": "root" } });
        assert!(build_target_inventory("h", PlaybookConnection::Ssh, &no_host).is_err());
        let no_user = json!({ "host": "1.2.3.4", "credentials": {} });
        assert!(build_target_inventory("h", PlaybookConnection::Ssh, &no_user).is_err());
    }

    #[test]
    fn host_alias_is_sanitized() {
        let result = build_target_inventory(
            "weird name/with spaces",
            PlaybookConnection::Ssh,
            &ssh_payload(Some("k\n"), None),
        )
        .expect("inventory");
        assert!(result.inventory_line.starts_with("weird_name_with_spaces "));
    }

    #[test]
    fn per_host_key_file_var_points_at_runner_key_dir() {
        let base = "web-01 ansible_host=10.0.0.5 ansible_connection=ssh ansible_user=root\n";
        let augmented = with_per_host_key_file(base, "web-01");
        assert_eq!(
            augmented,
            "web-01 ansible_host=10.0.0.5 ansible_connection=ssh ansible_user=root ansible_ssh_private_key_file=.piping_keys/web-01\n"
        );
        // The var is inserted before the single trailing newline, never doubled.
        assert_eq!(augmented.matches('\n').count(), 1);
    }

    #[test]
    fn cyrillic_host_alias_uses_ascii_transport_name() {
        let result = build_target_inventory(
            "Касса 1-1",
            PlaybookConnection::Ssh,
            &ssh_payload(Some("k\n"), None),
        )
        .expect("inventory");
        assert!(result.inventory_line.starts_with("1-1 "));
    }
}
