//! Materialization of the connection service's resolved SSH route, not a resolver.
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

pub(crate) struct RenderedRoute {
    /// Host variables this route contributes to the generated inventory.
    pub host_vars: serde_json::Map<String, Value>,
    pub secrets: Vec<Vec<u8>>,
}

pub(crate) struct NativeSshDefaults {
    pub config_path: String,
    pub private_key_file: String,
    pub secrets: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetMode {
    Endpoint,
    NativeDefaults,
}

struct SshMaterial {
    alias: Option<String>,
    config_path: String,
    private_key_file: String,
    secrets: Vec<Vec<u8>>,
}

fn port(endpoint: &Value) -> Result<u16> {
    let value = match endpoint.get("port") {
        None | Some(Value::Null) => 22,
        Some(value) => value.as_u64().context("invalid SSH port")?,
    };
    ensure!((1..=65535).contains(&value), "invalid SSH port");
    Ok(value as u16)
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    let text = value
        .get(name)
        .and_then(Value::as_str)
        .context("missing resolved SSH field")?;
    ensure!(
        !text.is_empty() && !text.chars().any(char::is_control),
        "invalid resolved SSH field"
    );
    Ok(text)
}
fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('%', "%%")
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

pub(crate) fn validate(payload: &Value) -> Result<()> {
    validate_for(payload, TargetMode::Endpoint)
}

pub(crate) fn validate_native_defaults(payload: &Value) -> Result<()> {
    validate_for(payload, TargetMode::NativeDefaults)
}

fn validate_for(payload: &Value, mode: TargetMode) -> Result<()> {
    ensure!(payload.is_object(), "invalid resolved route");
    match connection_type(payload)? {
        "ssh" => validate_ssh(payload, mode),
        "winrm" | "psrp" => {
            ensure!(
                mode == TargetMode::Endpoint,
                "native inventory defaults support SSH only"
            );
            validate_windows(payload)
        }
        other => bail!("unsupported resolved transport '{other}'"),
    }
}

/// The transport carried by a resolved connection payload. A missing value is
/// the historical SSH payload; every other value must be explicit.
fn connection_type(payload: &Value) -> Result<&str> {
    match payload.get("connection_type") {
        None => Ok("ssh"),
        Some(Value::String(value)) if matches!(value.as_str(), "ssh" | "winrm" | "psrp") => {
            Ok(value.as_str())
        }
        Some(_) => bail!("unsupported resolved transport"),
    }
}

fn validate_windows(payload: &Value) -> Result<()> {
    let host = field(payload, "host")?;
    ensure!(
        !host.starts_with('-')
            && host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".:-_".contains(&c)),
        "invalid resolved Windows host"
    );
    port(payload)?;
    ensure!(
        payload.get("jump").is_none_or(|value| value
            .as_array()
            .is_some_and(Vec::is_empty)),
        "WinRM and PSRP do not support SSH jump routes"
    );
    let credentials = payload
        .get("credentials")
        .context("missing Windows credentials")?;
    let user = field(credentials, "username")?;
    ensure!(
        user.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-@\\".contains(&c))
            && !user.starts_with('-'),
        "invalid Windows username"
    );
    ensure!(
        credentials
            .get("password")
            .and_then(Value::as_str)
            .is_some_and(|password| !password.is_empty()),
        "Windows transport requires a password credential"
    );
    ensure!(
        credentials.get("private_key").is_none_or(Value::is_null),
        "Windows transport does not accept an SSH private key"
    );
    if let Some(options) = payload.get("options") {
        let options = options.as_object().context("invalid Windows options")?;
        ensure!(
            options
                .keys()
                .all(|key| matches!(key.as_str(), "scheme" | "transport" | "ca_cert")),
            "unsupported Windows connection option"
        );
        if let Some(scheme) = options.get("scheme").and_then(Value::as_str) {
            ensure!(
                matches!(scheme, "http" | "https"),
                "invalid Windows connection scheme"
            );
            if scheme == "http" {
                ensure!(
                    port(payload)? == 5985,
                    "cleartext WinRM requires port 5985"
                );
            }
        }
        if let Some(transport) = options.get("transport").and_then(Value::as_str) {
            ensure!(
                matches!(transport, "basic" | "ntlm"),
                "unsupported Windows authentication transport"
            );
        }
        if options.get("ca_cert").is_some() {
            ensure!(
                options.get("scheme").and_then(Value::as_str) != Some("http"),
                "a CA certificate requires an https Windows endpoint"
            );
        }
    }
    Ok(())
}

fn validate_ssh(payload: &Value, mode: TargetMode) -> Result<()> {
    ensure!(
        payload.get("jump").is_none_or(|value| value.is_array()),
        "invalid resolved jump route"
    );
    let hops = payload
        .get("jump")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    ensure!(hops.len() <= 8, "too many resolved SSH hops");
    for (index, endpoint) in hops.iter().chain(std::iter::once(payload)).enumerate() {
        // A native record delegates defaults, never the inventory's endpoint.
        if index < hops.len() || mode == TargetMode::Endpoint {
            let host = field(endpoint, "host")?;
            ensure!(
                !host.starts_with('-')
                    && host
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b".:-_".contains(&c)),
                "invalid resolved SSH host"
            );
            port(endpoint)?;
        }
        let credentials = endpoint
            .get("credentials")
            .context("missing SSH credentials")?;
        let user = field(credentials, "username")?;
        ensure!(
            user.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-@".contains(&c))
                && !user.starts_with('-'),
            "invalid SSH username"
        );
        ensure!(
            credentials
                .get("private_key")
                .and_then(Value::as_str)
                .is_some_and(|key| !key.is_empty()),
            "SSH route requires a private key"
        );
        for name in ["password", "passphrase", "private_key_passphrase"] {
            ensure!(
                credentials
                    .get(name)
                    .is_none_or(|value| value.is_null() || value.as_str() == Some("")),
                "interactive/password SSH authentication is unsupported"
            );
        }
    }
    for hop in hops {
        ensure!(
            hop.get("jump")
                .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty)),
            "nested jump routes are unsupported"
        );
    }
    ensure!(
        payload
            .get("known_hosts_lines")
            .and_then(Value::as_array)
            .is_some_and(|lines| !lines.is_empty()
                && lines.iter().all(|line| line
                    .as_str()
                    .is_some_and(|line| !line.is_empty() && !line.contains(['\n', '\r', '\0'])))),
        "stored SSH host pins are required"
    );
    if let Some(shell) = payload
        .get("options")
        .and_then(|options| options.get("shell"))
        .and_then(Value::as_str)
    {
        ensure!(
            matches!(shell, "sh" | "powershell" | "cmd"),
            "SSH shell must be sh, powershell or cmd"
        );
    }
    Ok(())
}

pub(crate) fn render(controller: &Path, name: &str, payload: &Value) -> Result<RenderedRoute> {
    validate_for(payload, TargetMode::Endpoint)?;
    match connection_type(payload)? {
        "ssh" => {
            let material = materialize(controller, name, payload, TargetMode::Endpoint)?;
            let alias = material.alias.context("missing endpoint-bound SSH alias")?;
            let mut host_vars = serde_json::Map::new();
            host_vars.insert("ansible_host".into(), Value::String(alias));
            host_vars.insert(
                "ansible_ssh_common_args".into(),
                Value::String(format!("-F {}", material.config_path)),
            );
            if let Some(shell) = payload
                .get("options")
                .and_then(|options| options.get("shell"))
                .and_then(Value::as_str)
            {
                host_vars.insert("ansible_shell_type".into(), Value::String(shell.into()));
            }
            Ok(RenderedRoute {
                host_vars,
                secrets: material.secrets,
            })
        }
        transport => render_windows(controller, name, payload, transport),
    }
}

fn render_windows(
    controller: &Path,
    name: &str,
    payload: &Value,
    transport: &str,
) -> Result<RenderedRoute> {
    ensure!(
        !name.is_empty()
            && name.len() <= 48
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid Windows material identity"
    );
    let options = payload.get("options").and_then(Value::as_object);
    let scheme = options
        .and_then(|options| options.get("scheme"))
        .and_then(Value::as_str)
        .unwrap_or("https");
    let auth = options
        .and_then(|options| options.get("transport"))
        .and_then(Value::as_str)
        .unwrap_or("ntlm");
    let credentials = &payload["credentials"];
    let password = credentials["password"]
        .as_str()
        .context("missing Windows password")?;
    let mut host_vars = serde_json::Map::new();
    host_vars.insert("ansible_host".into(), Value::String(field(payload, "host")?.into()));
    host_vars.insert("ansible_connection".into(), Value::String(transport.into()));
    host_vars.insert(
        "ansible_port".into(),
        Value::Number(port(payload)?.into()),
    );
    host_vars.insert(
        "ansible_user".into(),
        Value::String(field(credentials, "username")?.into()),
    );
    host_vars.insert("ansible_password".into(), Value::String(password.into()));
    // Each connection plugin reads its own auth option name.
    let auth_var = if transport == "psrp" {
        "ansible_psrp_auth"
    } else {
        "ansible_winrm_transport"
    };
    host_vars.insert(auth_var.into(), Value::String(auth.into()));
    let mut secrets = vec![password.as_bytes().to_vec()];
    if scheme == "https" {
        // Certificate verification is always on: there is no ignore option.
        // Each connection plugin owns its own trust variable names: WinRM reads
        // `ansible_winrm_*`, PSRP reads `ansible_psrp_*`.
        let (validation_var, trust_var) = if transport == "psrp" {
            ("ansible_psrp_cert_validation", "ansible_psrp_cert_trust_path")
        } else {
            ("ansible_winrm_server_cert_validation", "ansible_winrm_ca_trust_path")
        };
        host_vars.insert(validation_var.into(), Value::String("validate".into()));
        if let Some(ca) = options
            .and_then(|options| options.get("ca_cert"))
            .and_then(Value::as_str)
        {
            ensure!(!ca.is_empty(), "empty Windows CA certificate");
            let ca_name = format!("winrm-{name}-ca.pem");
            write(&controller.join(&ca_name), ca.as_bytes())?;
            host_vars.insert(
                trust_var.into(),
                Value::String(format!("/job/controller/{ca_name}")),
            );
            secrets.push(ca.as_bytes().to_vec());
        }
    }
    Ok(RenderedRoute { host_vars, secrets })
}

pub(crate) fn render_native_defaults(
    controller: &Path,
    payload: &Value,
) -> Result<NativeSshDefaults> {
    let material = materialize(controller, "native", payload, TargetMode::NativeDefaults)?;
    Ok(NativeSshDefaults {
        config_path: material.config_path,
        private_key_file: material.private_key_file,
        secrets: material.secrets,
    })
}

fn materialize(
    controller: &Path,
    name: &str,
    payload: &Value,
    mode: TargetMode,
) -> Result<SshMaterial> {
    validate_for(payload, mode)?;
    ensure!(
        !name.is_empty()
            && name.len() <= 48
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid SSH material identity"
    );
    let config_name = format!("ssh-{name}.conf");
    let pins_name = format!("ssh-{name}.known_hosts");
    let pins = payload["known_hosts_lines"]
        .as_array()
        .context("missing pins")?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write(&controller.join(&pins_name), pins.as_bytes())?;
    let hops = payload
        .get("jump")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut config = String::new();
    let mut secrets = Vec::new();
    let mut previous: Option<String> = None;
    let mut target_key_file = String::new();
    for (index, endpoint) in hops.iter().chain(std::iter::once(payload)).enumerate() {
        let native_target = mode == TargetMode::NativeDefaults && index == hops.len();
        let alias = format!("piping-{name}-{index}");
        let key_name = if native_target {
            format!("ssh-{name}-target.key")
        } else {
            format!("ssh-{name}-{index}.key")
        };
        let credentials = &endpoint["credentials"];
        let key = credentials["private_key"]
            .as_str()
            .context("missing SSH key")?;
        write(&controller.join(&key_name), key.as_bytes())?;
        secrets.push(key.as_bytes().to_vec());
        target_key_file = format!("/job/controller/{key_name}");
        if native_target {
            config.push_str(&format!("Host * !piping-{name}-*\n"));
        } else {
            config.push_str(&format!(
                "Host {alias}\n  HostName {}\n  Port {}\n  IdentityFile {target_key_file}\n  IdentitiesOnly yes\n  IdentityAgent none\n  BatchMode yes\n  PreferredAuthentications publickey\n  PasswordAuthentication no\n  KbdInteractiveAuthentication no\n",
                quote(field(endpoint, "host")?), port(endpoint)?
            ));
        }
        config.push_str(&format!(
            "  User {}\n  UserKnownHostsFile /job/controller/{pins_name}\n  GlobalKnownHostsFile /dev/null\n  StrictHostKeyChecking yes\n  UpdateHostKeys no\n  VerifyHostKeyDNS no\n  KnownHostsCommand none\n  ForwardAgent no\n  PermitLocalCommand no\n  ConnectTimeout 30\n",
            quote(field(credentials, "username")?)
        ));
        if let Some(previous) = &previous {
            config.push_str(&format!("  ProxyJump {previous}\n"));
        }
        if !native_target {
            previous = Some(alias);
        }
    }
    // The target key is passed as an Ansible option only when native credentials
    // did not select an identity. Putting it in Host * would make -i additive.
    write(&controller.join(&config_name), config.as_bytes())?;
    Ok(SshMaterial {
        alias: if mode == TargetMode::Endpoint {
            previous
        } else {
            None
        },
        config_path: format!("/job/controller/{config_name}"),
        private_key_file: target_key_file,
        secrets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn windows_transport_renders_private_credentials_and_rejects_jump_routes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let payload = json!({
            "connection_type": "winrm",
            "host": "win.example.test",
            "port": 5986,
            "credentials": {"username": "svc", "password": "synthetic-secret"},
            "options": {"transport": "ntlm"}
        });
        let route = render(root.path(), "win", &payload)?;
        assert_eq!(route.host_vars["ansible_connection"], json!("winrm"));
        assert_eq!(route.host_vars["ansible_port"], json!(5986));
        assert_eq!(route.host_vars["ansible_user"], json!("svc"));
        assert_eq!(route.host_vars["ansible_password"], json!("synthetic-secret"));
        assert_eq!(route.host_vars["ansible_winrm_transport"], json!("ntlm"));
        assert_eq!(
            route.host_vars["ansible_winrm_server_cert_validation"],
            json!("validate")
        );
        assert!(route.secrets.iter().any(|value| value == b"synthetic-secret"));

        let mut jumped = payload.clone();
        jumped["jump"] = json!([{"host": "bastion.test"}]);
        let error = validate(&jumped).expect_err("WinRM must refuse an SSH jump route");
        assert!(error.to_string().contains("jump"));
        Ok(())
    }

    #[test]
    fn windows_openssh_states_its_shell_explicitly() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut payload = json!({
            "connection_type": "ssh",
            "host": "win-ssh.example.test",
            "credentials": {"username": "svc", "private_key": "synthetic-key"},
            "known_hosts_lines": ["win-ssh.example.test ssh-ed25519 SYNTHETIC"],
            "options": {"shell": "powershell"}
        });
        let route = render(root.path(), "winssh", &payload)?;
        assert_eq!(route.host_vars["ansible_shell_type"], json!("powershell"));
        assert!(!route
            .host_vars
            .values()
            .any(|value| value.as_str() == Some("synthetic-key")));

        payload["options"] = json!({"shell": "cmd"});
        let cmd_route = render(root.path(), "winssh-cmd", &payload)?;
        assert_eq!(cmd_route.host_vars["ansible_shell_type"], json!("cmd"));

        payload["options"] = json!({"shell": "bash"});
        assert!(validate(&payload).is_err());
        Ok(())
    }

    #[test]
    fn psrp_uses_its_own_certificate_trust_variables() -> Result<()> {
        let root = tempfile::tempdir()?;
        let payload = json!({
            "connection_type": "psrp",
            "host": "win.example.test",
            "port": 5986,
            "credentials": {"username": "svc", "password": "synthetic-secret"},
            "options": {"transport": "ntlm", "ca_cert": "SYNTHETIC-CA"}
        });
        let route = render(root.path(), "psrp", &payload)?;
        assert_eq!(
            route.host_vars["ansible_psrp_cert_validation"],
            json!("validate")
        );
        assert_eq!(
            route.host_vars["ansible_psrp_cert_trust_path"],
            json!("/job/controller/winrm-psrp-ca.pem")
        );
        assert_eq!(route.host_vars["ansible_psrp_auth"], json!("ntlm"));
        assert!(route.host_vars.get("ansible_winrm_transport").is_none());
        assert!(route.host_vars.get("ansible_winrm_ca_trust_path").is_none());
        Ok(())
    }

    #[test]
    fn windows_transport_requires_password_and_cleartext_uses_5985() -> Result<()> {
        let no_password = json!({
            "connection_type": "psrp",
            "host": "win.example.test",
            "credentials": {"username": "svc"}
        });
        assert!(validate(&no_password).is_err());

        let cleartext_wrong_port = json!({
            "connection_type": "winrm",
            "host": "win.example.test",
            "port": 5986,
            "credentials": {"username": "svc", "password": "synthetic-secret"},
            "options": {"scheme": "http"}
        });
        assert!(validate(&cleartext_wrong_port).is_err());

        let cert_auth = json!({
            "connection_type": "winrm",
            "host": "win.example.test",
            "credentials": {"username": "svc", "password": "synthetic-secret"},
            "options": {"transport": "cert"}
        });
        assert!(validate(&cert_auth).is_err());

        let ca_on_cleartext = json!({
            "connection_type": "winrm",
            "host": "win.example.test",
            "port": 5985,
            "credentials": {"username": "svc", "password": "synthetic-secret"},
            "options": {"scheme": "http", "ca_cert": "SYNTHETIC-CA"}
        });
        assert!(validate(&ca_on_cleartext).is_err());

        let unsupported = json!({
            "connection_type": "rdp",
            "host": "win.example.test",
            "credentials": {"username": "svc", "password": "synthetic-secret"}
        });
        assert!(validate(&unsupported).is_err());
        Ok(())
    }

    #[test]
    fn refuses_malformed_chain_instead_of_direct_fallback() {
        let payload = json!({"host":"target.test","credentials":{"username":"user","private_key":"synthetic"},"known_hosts_lines":["synthetic-pin"],"jump":"not-an-array"});
        assert!(validate(&payload).is_err());
    }
    #[test]
    fn native_openssh_resolves_each_hop_alias_and_private_identity() -> Result<()> {
        let root = tempfile::tempdir()?;
        let payload = json!({"host":"target.test","port":222,"credentials":{"username":"target-user","private_key":"synthetic-target"},"known_hosts_lines":["synthetic-pin"],"jump":[{"host":"first.test","credentials":{"username":"first-user","private_key":"synthetic-first"}},{"host":"second.test","port":2200,"credentials":{"username":"second-user","private_key":"synthetic-second"}}]});
        let route = render(root.path(), "fixture", &payload)?;
        for (alias, host, previous, key) in [
            ("piping-fixture-0", "first.test", None, "ssh-fixture-0.key"),
            (
                "piping-fixture-1",
                "second.test",
                Some("piping-fixture-0"),
                "ssh-fixture-1.key",
            ),
            (
                route.host_vars["ansible_host"].as_str().unwrap(),
                "target.test",
                Some("piping-fixture-1"),
                "ssh-fixture-2.key",
            ),
        ] {
            let output = std::process::Command::new("/usr/bin/ssh")
                .args(["-G", "-F"])
                .arg(root.path().join("ssh-fixture.conf"))
                .arg(alias)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .output()?;
            ensure!(output.status.success(), "OpenSSH rejected rendered route");
            let parsed = String::from_utf8(output.stdout)?;
            assert!(
                parsed
                    .lines()
                    .any(|line| line == format!("hostname {host}"))
            );
            assert!(
                parsed
                    .lines()
                    .any(|line| line == format!("identityfile /job/controller/{key}"))
            );
            if let Some(previous) = previous {
                assert!(
                    parsed
                        .lines()
                        .any(|line| line == format!("proxyjump {previous}"))
                );
            }
        }
        Ok(())
    }

    fn native_payload() -> Value {
        json!({
            "connection_type": "ssh",
            "host": "unused-record-endpoint.test",
            "port": 9999,
            "credentials": {"username": "record-user", "private_key": "synthetic-target-key"},
            "jump": [
                {"host": "first.test", "port": 2222, "credentials": {"username": "first-user", "private_key": "synthetic-first-key"}},
                {"host": "second.test", "port": 2200, "credentials": {"username": "second-user", "private_key": "synthetic-second-key"}}
            ],
            "known_hosts_lines": [
                "[inventory.test]:2022 ssh-ed25519 synthetic-target-pin",
                "[first.test]:2222 ssh-ed25519 synthetic-first-pin",
                "[second.test]:2200 ssh-ed25519 synthetic-second-pin"
            ]
        })
    }

    fn native_options(root: &Path, args: &[&str]) -> Result<Vec<String>> {
        let output = std::process::Command::new("/usr/bin/ssh")
            .args(["-G", "-F"])
            .arg(root.join("ssh-native.conf"))
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()?;
        ensure!(
            output.status.success(),
            "OpenSSH rejected native SSH defaults"
        );
        Ok(String::from_utf8(output.stdout)?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    #[test]
    fn native_openssh_defaults_preserve_endpoints_and_separate_hop_identities() -> Result<()> {
        let root = tempfile::tempdir()?;
        let payload = native_payload();
        let rendered = render_native_defaults(root.path(), &payload)?;
        assert_eq!(rendered.config_path, "/job/controller/ssh-native.conf");
        assert_eq!(
            rendered.private_key_file,
            "/job/controller/ssh-native-target.key"
        );
        assert_eq!(rendered.secrets.len(), 3);
        let defaults = native_options(root.path(), &["inventory.test"])?;
        assert!(defaults.contains(&"hostname inventory.test".into()));
        assert!(defaults.contains(&"port 22".into()));
        assert!(defaults.contains(&"user record-user".into()));
        assert!(defaults.contains(&"proxyjump piping-native-1".into()));
        assert!(
            !defaults
                .iter()
                .any(|line| line.contains("ssh-native-target.key"))
        );
        let selected_key = root.path().join("selected.key");
        write(&selected_key, b"synthetic-native-key")?;
        let selected_key = selected_key.to_str().context("non-UTF-8 fixture path")?;
        let explicit = native_options(
            root.path(),
            &[
                "-l",
                "native-user",
                "-p",
                "2022",
                "-i",
                selected_key,
                "inventory.test",
            ],
        )?;
        assert!(explicit.contains(&"hostname inventory.test".into()));
        assert!(explicit.contains(&"port 2022".into()));
        assert!(explicit.contains(&"user native-user".into()));
        let identities: Vec<_> = explicit
            .iter()
            .filter(|line| line.starts_with("identityfile "))
            .collect();
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0], &format!("identityfile {selected_key}"));
        assert!(
            !explicit
                .iter()
                .any(|line| line.contains("ssh-native-target.key"))
        );
        for (index, host, port, user) in [
            (0, "first.test", 2222, "first-user"),
            (1, "second.test", 2200, "second-user"),
        ] {
            let alias = format!("piping-native-{index}");
            let hop = native_options(root.path(), &[&alias])?;
            assert!(hop.contains(&format!("hostname {host}")));
            assert!(hop.contains(&format!("port {port}")));
            assert!(hop.contains(&format!("user {user}")));
            assert_eq!(
                hop.iter()
                    .filter(|line| line.starts_with("identityfile "))
                    .count(),
                1
            );
            assert!(hop.contains(&format!(
                "identityfile /job/controller/ssh-native-{index}.key"
            )));
            if index == 1 {
                assert!(hop.contains(&"proxyjump piping-native-0".into()));
            }
        }
        let pins = payload["known_hosts_lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(
            std::fs::read_to_string(root.path().join("ssh-native.known_hosts"))?,
            pins
        );
        Ok(())
    }

    #[test]
    fn native_record_endpoint_is_nonoperative_but_hop_endpoints_remain_required() -> Result<()> {
        let mut payload = native_payload();
        payload.as_object_mut().unwrap().remove("host");
        payload.as_object_mut().unwrap().remove("port");
        validate_native_defaults(&payload)?;
        assert!(validate(&payload).is_err());
        payload["host"] = json!("-invalid\nendpoint");
        payload["port"] = json!(0);
        validate_native_defaults(&payload)?;
        payload["jump"][0]["port"] = json!(0);
        assert!(validate_native_defaults(&payload).is_err());
        Ok(())
    }

    #[test]
    fn malformed_native_defaults_are_refused_before_materialization() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mutations: [fn(&mut Value); 5] = [
            |value: &mut Value| value["jump"] = json!("not-an-array"),
            |value: &mut Value| value["jump"][0]["jump"] = json!([{"host": "hidden.test"}]),
            |value: &mut Value| value["jump"][0]["credentials"] = Value::Null,
            |value: &mut Value| value["known_hosts_lines"] = json!([]),
            |value: &mut Value| value["credentials"]["password"] = json!("unsupported-password"),
        ];
        for mutate in mutations {
            let mut payload = native_payload();
            mutate(&mut payload);
            assert!(render_native_defaults(root.path(), &payload).is_err());
            assert_eq!(std::fs::read_dir(root.path())?.count(), 0);
        }
        Ok(())
    }

    #[test]
    fn explicitly_direct_native_defaults_do_not_require_a_record_endpoint() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut payload = native_payload();
        payload.as_object_mut().unwrap().remove("host");
        payload.as_object_mut().unwrap().remove("port");
        payload.as_object_mut().unwrap().remove("jump");
        render_native_defaults(root.path(), &payload)?;
        let options = native_options(root.path(), &["inventory.test"])?;
        assert!(options.contains(&"hostname inventory.test".into()));
        assert!(!options.iter().any(|line| (line.starts_with("proxyjump ")
            && line != "proxyjump none")
            || (line.starts_with("proxycommand ") && line != "proxycommand none")));
        Ok(())
    }
}
