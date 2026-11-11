//! Managed Ansible inventory rendering for one resolved launch.
//!
//! Credentials resolved through the canonical connection resolver travel to the
//! runner in the per-target `connection` payload, never in the inventory text.
//! The runner materializes SSH keys and host-key pins from that payload, so this
//! module renders only the non-secret host line Ansible needs to address a host.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::errors::AppError;

/// Render one managed-inventory host line.
///
/// The payload is the canonical resolver output; only its non-secret addressing
/// fields (`host`, `port`, `credentials.username`) are read here. The complete
/// payload, including `jump` and `known_hosts_lines`, is carried separately in
/// the wire target and must never be stripped or reduced to these fields.
pub fn build_managed_inventory_line(
    alias: &str,
    proxy_payload: &Value,
    host_vars: &BTreeMap<String, String>,
) -> Result<String, AppError> {
    let host = payload_str(proxy_payload, &["host"]).ok_or_else(|| {
        AppError::Validation("resolved SSH launch payload is missing host".to_string())
    })?;
    let port = proxy_payload
        .get("port")
        .and_then(Value::as_u64)
        .filter(|port| *port > 0);
    let username = proxy_payload
        .get("credentials")
        .and_then(|credentials| credentials.get("username"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|username| !username.is_empty())
        .ok_or_else(|| {
            AppError::Validation(
                "resolved SSH launch payload is missing credentials.username".to_string(),
            )
        })?;

    let alias = sanitize_host_alias(alias);
    let mut line = String::new();
    line.push_str(&alias);
    push_var(&mut line, "ansible_host", &host);
    let connection_type = payload_str(proxy_payload, &["connection_type"]).unwrap_or_else(|| "ssh".to_string());
    push_var(&mut line, "ansible_connection", &connection_type);
    // Windows over OpenSSH needs the PowerShell shell semantics stated on the
    // record; nothing is inferred from a hostname or platform name.
    if let Some(shell) = proxy_payload
        .get("options")
        .and_then(|options| options.get("shell"))
        .and_then(Value::as_str)
    {
        push_var(&mut line, "ansible_shell_type", shell);
    }
    push_var(&mut line, "ansible_user", username);
    if let Some(port) = port {
        push_var(&mut line, "ansible_port", &port.to_string());
    }
    push_vars(&mut line, host_vars);
    line.push('\n');
    Ok(line)
}

/// Read the first present non-empty string value at any of `keys`.
fn payload_str(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
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
/// names, but callback streams and temporary paths are less predictable with
/// Unicode aliases during large grouped runs.
pub fn sanitize_host_alias(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "target".to_string();
    }
    let mut alias = String::new();
    let mut previous_separator = false;
    for character in trimmed.chars() {
        if character.is_ascii_alphanumeric() || character == '.' || character == '-' {
            alias.push(character);
            previous_separator = false;
        } else if !previous_separator {
            alias.push('_');
            previous_separator = true;
        }
    }
    let alias = alias
        .trim_matches(|character| matches!(character, '_' | '.' | '-'))
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
        let mut credentials = serde_json::Map::new();
        credentials.insert("username".to_string(), json!("root"));
        if let Some(key) = private_key {
            credentials.insert("private_key".to_string(), json!(key));
        }
        if let Some(password) = password {
            credentials.insert("password".to_string(), json!(password));
        }
        json!({
            "host": "10.0.0.5",
            "port": 2222,
            "credentials": credentials,
            "jump": [{
                "host": "bastion.example.org",
                "port": 22,
                "credentials": { "username": "jump", "private_key": "JUMP-KEY" }
            }],
            "known_hosts_lines": ["10.0.0.5 ssh-ed25519 TARGET"]
        })
    }

    #[test]
    fn managed_line_carries_addressing_but_never_credentials() {
        let line = build_managed_inventory_line(
            "web-01",
            &ssh_payload(Some("PRIVATE-KEY"), Some("PASSWORD")),
            &BTreeMap::new(),
        )
        .expect("inventory line");

        assert_eq!(
            line,
            "web-01 ansible_host=10.0.0.5 ansible_connection=ssh ansible_user=root ansible_port=2222\n"
        );
        assert!(!line.contains("PRIVATE-KEY"));
        assert!(!line.contains("PASSWORD"));
        assert!(!line.contains("bastion.example.org"));
    }

    #[test]
    fn managed_line_quotes_host_vars_and_appends_them() {
        let vars = BTreeMap::from([
            ("piping_run_id".to_string(), "run-1".to_string()),
            ("piping_target_name".to_string(), "Cash desk 1".to_string()),
        ]);
        let line = build_managed_inventory_line("web-01", &ssh_payload(None, None), &vars).unwrap();
        assert!(line.contains("piping_run_id=run-1"));
        assert!(line.contains("piping_target_name=\"Cash desk 1\""));
    }

    #[test]
    fn managed_line_carries_the_resolved_windows_transport() {
        let mut payload = ssh_payload(None, None);
        payload["connection_type"] = json!("winrm");
        payload["port"] = json!(5986);
        let line = build_managed_inventory_line("win-01", &payload, &BTreeMap::new()).unwrap();
        assert!(line.contains("ansible_connection=winrm"));
        assert!(line.contains("ansible_port=5986"));
    }

    #[test]
    fn managed_line_states_windows_openssh_shell_and_never_carries_the_password() {
        let mut payload = ssh_payload(None, Some("synthetic-password"));
        payload["options"] = json!({"shell": "powershell"});
        let line = build_managed_inventory_line("win-ssh", &payload, &BTreeMap::new()).unwrap();
        assert!(line.contains("ansible_shell_type=powershell"));
        assert!(!line.contains("synthetic-password"));

        payload["options"] = json!({"shell": "cmd"});
        let cmd_line =
            build_managed_inventory_line("win-ssh", &payload, &BTreeMap::new()).unwrap();
        assert!(cmd_line.contains("ansible_shell_type=cmd"));

        let mut winrm = ssh_payload(None, Some("synthetic-password"));
        winrm["connection_type"] = json!("winrm");
        let line = build_managed_inventory_line("win-01", &winrm, &BTreeMap::new()).unwrap();
        assert!(line.contains("ansible_connection=winrm"));
        assert!(!line.contains("synthetic-password"));
    }

    #[test]
    fn managed_line_rejects_missing_host_or_username() {
        let no_host = json!({ "credentials": { "username": "root" } });
        assert!(build_managed_inventory_line("h", &no_host, &BTreeMap::new()).is_err());
        let no_user = json!({ "host": "1.2.3.4", "credentials": {} });
        assert!(build_managed_inventory_line("h", &no_user, &BTreeMap::new()).is_err());
    }

    #[test]
    fn alias_is_sanitized() {
        let line = build_managed_inventory_line(
            "weird name/with spaces",
            &ssh_payload(None, None),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(line.starts_with("weird_name_with_spaces "));
    }
}
