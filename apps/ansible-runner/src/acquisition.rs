//! Canonical project acquisition inputs and verified, credential-free result transport.
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::sandbox::{SandboxLimits, SandboxSpec};
use anyhow::{Context, Result, ensure};
use piping_contracts::playbook_runner::{
    AcquireAuth, AcquireAuthKind, AcquireFailureCode, AcquireProjectRequest,
    AcquireProjectResponse, RUNNER_CONTRACT_VERSION, SnapshotContent, snapshot_tree_sha256,
};

fn destination(request: &AcquireProjectRequest) -> Result<(String, reqwest::Url)> {
    let remote = request.url.as_str();
    ensure!(
        !remote.is_empty() && !remote.chars().any(|c| c.is_control() || c.is_whitespace()),
        "invalid Git destination"
    );
    let parsed = if remote.contains("://") {
        reqwest::Url::parse(remote).map_err(|_| anyhow::anyhow!("invalid Git destination"))?
    } else {
        let (user, rest) = remote.split_once('@').context("invalid SCP remote")?;
        let (host, path) = if rest.starts_with('[') {
            let end = rest.find("]:").context("invalid SCP IPv6 remote")?;
            (&rest[..=end], &rest[end + 2..])
        } else {
            rest.split_once(':').context("invalid SCP remote")?
        };
        ensure!(
            !user.is_empty() && !path.is_empty() && !path.starts_with('-'),
            "invalid SCP remote"
        );
        reqwest::Url::parse(&format!("ssh://{user}@{host}/{path}"))
            .map_err(|_| anyhow::anyhow!("invalid SCP remote"))?
    };
    ensure!(
        matches!(parsed.scheme(), "https" | "ssh")
            && parsed.host_str().is_some()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.path() != "/"
            && !parsed.path().is_empty(),
        "unsupported Git destination"
    );
    if parsed.scheme() == "https" {
        ensure!(
            parsed.username().is_empty(),
            "credentials in HTTPS URL are forbidden"
        );
    }
    Ok((remote.into(), parsed))
}
fn auth_kind(auth: &AcquireAuth) -> AcquireAuthKind {
    match auth {
        AcquireAuth::Anonymous => AcquireAuthKind::Anonymous,
        AcquireAuth::HttpsBasic { .. } => AcquireAuthKind::HttpsBasic,
        AcquireAuth::HttpsBearer { .. } => AcquireAuthKind::HttpsBearer,
        AcquireAuth::SshKey { .. } => AcquireAuthKind::SshKey,
    }
}
fn simple_secret(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

pub(crate) fn validate(request: &AcquireProjectRequest) -> Result<()> {
    ensure!(
        request.protocol_version == RUNNER_CONTRACT_VERSION && !request.acquisition_id.is_nil(),
        "invalid acquisition identity/protocol"
    );
    let (_, url) = destination(request)?;
    ensure!(
        !request.reference.starts_with('-') && !request.reference.chars().any(char::is_control),
        "invalid Git reference"
    );
    ensure!(
        !request.image.is_empty()
            && !request.image.contains('@')
            && !request
                .image
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            && !request.image.rsplit('/').next().unwrap_or("").contains(':'),
        "invalid acquisition image"
    );
    let digest = request.image_digest.strip_prefix("sha256:").unwrap_or("");
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "acquisition image must be digest pinned"
    );
    match &request.auth {
        AcquireAuth::Anonymous => ensure!(
            url.scheme() == "https",
            "anonymous SSH acquisition is unsupported"
        ),
        AcquireAuth::HttpsBasic { username, password } => ensure!(
            url.scheme() == "https" && simple_secret(username) && simple_secret(password),
            "invalid HTTPS credentials"
        ),
        AcquireAuth::HttpsBearer { token } => ensure!(
            url.scheme() == "https" && simple_secret(token),
            "invalid HTTPS bearer"
        ),
        AcquireAuth::SshKey { connection } => {
            crate::route::validate(connection)?;
            let host = connection["host"].as_str().context("missing SSH host")?;
            let user = connection["credentials"]["username"]
                .as_str()
                .context("missing SSH user")?;
            let port = connection
                .get("port")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(22);
            ensure!(
                url.scheme() == "ssh"
                    && url.host_str().is_some_and(|value| value
                        .trim_matches(['[', ']'])
                        .eq_ignore_ascii_case(host.trim_matches(['[', ']'])))
                    && u64::from(url.port().unwrap_or(22)) == port
                    && (url.username().is_empty() || url.username() == user),
                "SSH destination differs from resolved target"
            );
        }
    }
    Ok(())
}
fn private_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

pub(crate) fn prepare_request(
    request: &AcquireProjectRequest,
    workspace: &Path,
    limits: SandboxLimits,
) -> Result<SandboxSpec> {
    validate(request)?;
    fs::create_dir(workspace)?;
    fs::set_permissions(workspace, fs::Permissions::from_mode(0o700))?;
    let controller = workspace.join("controller");
    fs::create_dir(&controller)?;
    fs::set_permissions(&controller, fs::Permissions::from_mode(0o700))?;
    let (remote, _) = destination(request)?;
    let script = include_bytes!("../docker/acquire.py");
    for (name, bytes, mode) in [
        ("acquire.py", script.as_slice(), 0o600),
        ("credential-helper", script.as_slice(), 0o700),
        ("remote", remote.as_bytes(), 0o600),
        ("reference", request.reference.as_bytes(), 0o600),
    ] {
        private_file(&controller.join(name), bytes, mode)?;
    }
    let kind = auth_kind(&request.auth);
    private_file(
        &controller.join("request.json"),
        &serde_json::to_vec(
            &serde_json::json!({"acquisition_id":request.acquisition_id,"destination":remote,"auth_kind":kind}),
        )?,
        0o600,
    )?;
    match &request.auth {
        AcquireAuth::Anonymous => private_file(&controller.join("auth"), b"anonymous", 0o600)?,
        AcquireAuth::HttpsBasic { username, password } => {
            private_file(&controller.join("auth"), b"https", 0o600)?;
            private_file(&controller.join("username"), username.as_bytes(), 0o600)?;
            private_file(&controller.join("secret"), password.as_bytes(), 0o600)?;
        }
        AcquireAuth::HttpsBearer { token } => {
            private_file(&controller.join("auth"), b"bearer", 0o600)?;
            private_file(&controller.join("secret"), token.as_bytes(), 0o600)?;
            let quote = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
            let config = format!(
                "[http \"{}\"]\n\textraHeader = \"Authorization: Bearer {}\"\n",
                quote(&remote),
                quote(token)
            );
            private_file(&controller.join("http-auth.conf"), config.as_bytes(), 0o600)?;
        }
        AcquireAuth::SshKey { connection } => {
            private_file(&controller.join("auth"), b"ssh", 0o600)?;
            let rendered = crate::route::render(&controller, "acquire", connection)?;
            let alias = rendered.host_vars["ansible_host"]
                .as_str()
                .context("missing acquisition SSH alias")?;
            private_file(&controller.join("ssh-alias"), alias.as_bytes(), 0o600)?;
            // The renderer owns the config path and shell-safe arguments.
            let common_args = rendered.host_vars["ansible_ssh_common_args"]
                .as_str()
                .context("missing acquisition SSH arguments")?;
            let wrapper = format!(
                "#!/bin/sh\nexec /usr/bin/ssh {} \"$@\"\n",
                common_args
            );
            private_file(&controller.join("ssh"), wrapper.as_bytes(), 0o700)?;
        }
    }
    Ok(SandboxSpec {
        dispatch_id: request.acquisition_id.to_string(),
        phase: "acquire".into(),
        image: format!("{}@{}", request.image, request.image_digest),
        workspace: workspace.to_owned(),
        argv: vec![
            "python3".into(),
            "-I".into(),
            "/job/controller/acquire.py".into(),
            "/job/controller".into(),
            "/job/project".into(),
        ],
        env: BTreeMap::from([
            ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
            ("LANG".into(), "C.UTF-8".into()),
        ]),
        workdir: "/job".into(),
        limits,
    })
}

pub(crate) fn collect_response(
    request: &AcquireProjectRequest,
    stdout: &[u8],
) -> Result<AcquireProjectResponse> {
    ensure!(
        stdout.len() <= 16 * 1024 * 1024,
        "acquisition result exceeds output bound"
    );
    let response: AcquireProjectResponse =
        serde_json::from_slice(stdout).context("invalid acquisition result")?;
    ensure!(
        response.acquisition_id == request.acquisition_id
            && response.destination == destination(request)?.0
            && response.auth_kind == auth_kind(&request.auth),
        "acquisition result identity mismatch"
    );
    ensure!(
        matches!(response.source_commit_oid.len(), 40 | 64)
            && response
                .source_commit_oid
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid source commit"
    );
    ensure!(response.entries.len() <= 2048, "too many acquired entries");
    let mut secrets: Vec<&[u8]> = Vec::new();
    match &request.auth {
        AcquireAuth::Anonymous => {}
        AcquireAuth::HttpsBasic { password, .. } => secrets.push(password.as_bytes()),
        AcquireAuth::HttpsBearer { token } => secrets.push(token.as_bytes()),
        AcquireAuth::SshKey { connection } => {
            crate::route::validate(connection)?;
            let hops = connection.get("jump").and_then(serde_json::Value::as_array);
            for endpoint in hops
                .into_iter()
                .flatten()
                .chain(std::iter::once(connection))
            {
                let key = endpoint["credentials"]["private_key"]
                    .as_str()
                    .context("missing SSH key")?;
                secrets.push(key.as_bytes());
            }
        }
    }
    ensure!(
        secrets.iter().all(|secret| !secret.is_empty()),
        "invalid issued credential"
    );
    let mut total = 0usize;
    for entry in &response.entries {
        ensure!(
            !entry
                .path
                .split('/')
                .any(|part| part == ".git" || part == ".gitmodules"),
            "Git internals are not snapshot entries"
        );
        if let SnapshotContent::File { bytes, .. } = &entry.content {
            ensure!(bytes.len() <= 2 * 1024 * 1024, "acquired file too large");
            total = total
                .checked_add(bytes.len())
                .context("acquisition too large")?;
        }
        let content = match &entry.content {
            SnapshotContent::File { bytes, .. } => bytes.as_slice(),
            SnapshotContent::Symlink { target } => target.as_bytes(),
        };
        ensure!(
            !secrets.iter().any(|secret| {
                entry
                    .path
                    .as_bytes()
                    .windows(secret.len())
                    .any(|window| window == *secret)
                    || content
                        .windows(secret.len())
                        .any(|window| window == *secret)
            }),
            "acquisition contains issued credential"
        );
    }
    ensure!(total <= 64 * 1024 * 1024, "acquisition too large");
    ensure!(
        snapshot_tree_sha256(&response.entries).map_err(anyhow::Error::msg)?
            == response.tree_sha256,
        "acquisition digest mismatch"
    );
    Ok(response)
}

/// Map the sandbox's fixed stderr category to the published failure vocabulary.
///
/// `acquire.py` prints exactly one safe category line for a failed acquisition;
/// native Git diagnostics never reach this function's consumer. Anything else
/// is the generic Git failure, so an unknown line cannot invent a category.
pub(crate) fn classify_failure(stderr: &[u8]) -> AcquireFailureCode {
    let text = String::from_utf8_lossy(stderr);
    let mut code = AcquireFailureCode::GitFailed;
    for line in text.lines().map(str::trim) {
        match line {
            // Trust is actionable before credentials and cannot be shadowed.
            "host key verification failed" => return AcquireFailureCode::HostKeyRejected,
            "authentication failed" => code = AcquireFailureCode::AuthenticationFailed,
            "reference not found" if code != AcquireFailureCode::AuthenticationFailed => {
                code = AcquireFailureCode::ReferenceNotFound;
            }
            _ => {}
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use piping_contracts::playbook_runner::SnapshotEntry;
    use serde_json::json;

    fn request() -> AcquireProjectRequest {
        AcquireProjectRequest {
            protocol_version: RUNNER_CONTRACT_VERSION,
            acquisition_id: uuid::Uuid::new_v4(),
            url: "ssh://user@example.test:222/repo.git".into(),
            reference: "main".into(),
            auth: AcquireAuth::SshKey {
                connection: json!({
                    "host":"example.test", "port":222,
                    "credentials":{"username":"user", "private_key":"synthetic-key"},
                    "known_hosts_lines":["synthetic-pin"]
                }),
            },
            image: "example/image".into(),
            image_digest: format!("sha256:{}", "a".repeat(64)),
        }
    }

    #[test]
    fn ssh_destination_and_authentication_must_match_route() {
        let mut request = request();
        assert!(validate(&request).is_ok());
        for url in [
            "ssh://user@other.test:222/repo.git",
            "ssh://user@example.test:223/repo.git",
            "ssh://other@example.test:222/repo.git",
        ] {
            request.url = url.into();
            assert!(validate(&request).is_err());
        }
        request.url = "ssh://user@example.test:222/repo.git".into();
        if let AcquireAuth::SshKey { connection } = &mut request.auth {
            connection["credentials"]["passphrase"] = json!("unsupported");
        }
        assert!(validate(&request).is_err());
    }

    #[test]
    fn sandbox_diagnostics_map_to_fixed_acquisition_categories() {
        assert_eq!(
            classify_failure(b"authentication failed\n"),
            AcquireFailureCode::AuthenticationFailed
        );
        assert_eq!(
            classify_failure(b"host key verification failed\n"),
            AcquireFailureCode::HostKeyRejected
        );
        assert_eq!(
            classify_failure(b"fatal: couldn't find remote ref main\nreference not found\n"),
            AcquireFailureCode::ReferenceNotFound
        );
        assert_eq!(
            classify_failure(b"fatal: repository 'https://x.invalid/repo' not found\n"),
            AcquireFailureCode::GitFailed
        );
        assert_eq!(classify_failure(b""), AcquireFailureCode::GitFailed);
    }

    #[test]
    fn canonical_response_checks_identity_digest_and_single_document() {
        let request = request();
        let entries = vec![];
        let response = AcquireProjectResponse {
            acquisition_id: request.acquisition_id,
            destination: request.url.clone(),
            auth_kind: AcquireAuthKind::SshKey,
            source_commit_oid: "a".repeat(40),
            tree_sha256: snapshot_tree_sha256(&entries).unwrap(),
            entries,
        };
        let encoded = serde_json::to_vec(&response).unwrap();
        assert_eq!(collect_response(&request, &encoded).unwrap(), response);
        let mut extra = encoded.clone();
        extra.extend_from_slice(b" {}");
        assert!(collect_response(&request, &extra).is_err());
        let mut altered = response.clone();
        altered.tree_sha256 = "0".repeat(64);
        assert!(collect_response(&request, &serde_json::to_vec(&altered).unwrap()).is_err());
        altered = response;
        altered.destination = "ssh://user@other.test/repo.git".into();
        assert!(collect_response(&request, &serde_json::to_vec(&altered).unwrap()).is_err());
    }

    #[test]
    fn response_refuses_issued_secrets_in_paths_files_and_symlinks() {
        let mut ssh = request();
        if let AcquireAuth::SshKey { connection } = &mut ssh.auth {
            connection["jump"] = json!([{
                "host":"jump.test", "credentials":{"username":"jumpuser", "private_key":"jump-key"}
            }]);
        }
        let mut https = request();
        https.url = "https://example.test/repo.git".into();
        https.auth = AcquireAuth::HttpsBearer {
            token: "issued-token".into(),
        };
        let check = |request: &AcquireProjectRequest, entry: SnapshotEntry| {
            let entries = vec![entry];
            let response = AcquireProjectResponse {
                acquisition_id: request.acquisition_id,
                destination: request.url.clone(),
                auth_kind: auth_kind(&request.auth),
                source_commit_oid: "a".repeat(40),
                tree_sha256: snapshot_tree_sha256(&entries).unwrap(),
                entries,
            };
            collect_response(request, &serde_json::to_vec(&response).unwrap())
        };
        let file = |path: &str, bytes: &[u8]| SnapshotEntry {
            path: path.into(),
            content: SnapshotContent::File {
                bytes: bytes.into(),
                executable: false,
            },
        };
        assert!(check(&ssh, file("file", b"prefix jump-key suffix")).is_err());
        assert!(check(&ssh, file("file", b"synthetic-key")).is_err());
        assert!(check(&https, file("issued-token", b"safe")).is_err());
        assert!(
            check(
                &https,
                SnapshotEntry {
                    path: "link".into(),
                    content: SnapshotContent::Symlink {
                        target: "issued-token".into()
                    },
                }
            )
            .is_err()
        );
        assert!(check(&ssh, file("user", b"jumpuser")).is_ok());
        https.auth = AcquireAuth::HttpsBasic {
            username: "user".into(),
            password: "issued-password".into(),
        };
        assert!(check(&https, file("file", b"issued-password")).is_err());
    }
}
