//! Linux-host driver (local user administration over the SSH proxy path).
//!
//! A Linux host is not a video device: its management plane is the host's own
//! account database. This driver runs standard account tools over the existing
//! SSH exec runtime — `getent passwd` to list, `useradd`/`chpasswd` to create,
//! `chpasswd`/`usermod` to edit, `userdel -r` to remove — and normalizes them
//! onto the vendor-neutral [`DeviceDriver`] contract. It advertises exactly
//! `users`, `user_update` and `info`; every media/recording capability is absent
//! and returns [`DriverError::Unsupported`].
//!
//! # Privilege
//!
//! Account tools need root. When the launch credentials carry a fenced
//! `become_password`, commands run through `sudo -S -p ''` with that password on
//! stdin (never in argv); when the connection already logs in as `root`, no sudo
//! is used. Without a become password the commands run unprivileged and each
//! affected item fails with the tool's own permission error.
//!
//! # Secrets
//!
//! A password is only ever written to a command's stdin (`chpasswd`), never to
//! the command line, an error message, or a log. The become password is likewise
//! stdin only. A password containing a newline is rejected per item: `chpasswd`
//! reads one `name:password` line per user, so an embedded newline would inject a
//! second account line.
//!
//! # Lock/unlock
//!
//! The vendor-neutral [`UserUpdate`] has no dedicated lock facet yet, so this
//! driver accepts the account-state control tokens `lock`/`unlock` through its
//! `authorities` facet (mapped to `usermod -L`/`-U`). Any other authority token
//! is rejected per item; the field is not advertised as a catalog.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::contexts::device_management::application::ssh_exec::{
    SshExecOutput, SshExecPort, SshExecRequest,
};
use crate::contexts::device_management::domain::{
    BulkOutcome, Capabilities, DeviceDriver, DeviceInfo, DiskStatus, DriverError,
    LinkFailurePolicy, NewUser, RecordingStatus, Secret, SnapshotRef, StreamUri, UserAccount,
    UserUpdate,
};

/// Account `UID_MIN` fallback when `/etc/login.defs` is absent or unreadable.
const DEFAULT_UID_MIN: u32 = 1000;
/// The conventional `nobody` uid: never a manageable human account.
const NOBODY_UID: u32 = 65534;
/// Wall-clock deadline for one account command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// `getent passwd` listing.
const CMD_GETENT_PASSWD: &str = "getent passwd";
/// `/etc/login.defs`, read for `UID_MIN`.
const CMD_LOGIN_DEFS: &str = "cat /etc/login.defs";
/// `chpasswd`, reading `name:password` lines from stdin.
const CMD_CHPASSWD: &str = "chpasswd";
/// Host identity readout.
const CMD_UNAME: &str = "uname -srm";

/// A Linux host managed over SSH.
pub struct LinuxDriver {
    ssh_exec: Arc<dyn SshExecPort>,
    /// The connection target the SSH launch material is resolved from.
    target_id: Uuid,
    /// The SSH login username, used only to decide whether the login already is
    /// `root` (and therefore needs no `sudo`).
    username: String,
    /// Fenced `become_password` for `sudo -S`; never leaves stdin.
    become_password: Option<Secret>,
}

impl LinuxDriver {
    /// Construct a driver running commands against `target_id` over `ssh_exec`.
    pub fn new(
        ssh_exec: Arc<dyn SshExecPort>,
        target_id: Uuid,
        username: impl Into<String>,
        become_password: Option<Secret>,
    ) -> Self {
        Self {
            ssh_exec,
            target_id,
            username: username.into(),
            become_password,
        }
    }

    /// Run `command` on the host, applying the family's privilege wrapping.
    ///
    /// # Errors
    ///
    /// Returns a link-fatal [`DriverError`] when the SSH link failed or ssh
    /// itself exited 255 (its own transport/auth failure code).
    async fn exec(
        &self,
        command: String,
        stdin: Option<Vec<u8>>,
    ) -> Result<SshExecOutput, DriverError> {
        let (command, stdin) = wrap_privileged(
            &self.username,
            self.become_password.as_ref(),
            command,
            stdin,
        );
        let request = SshExecRequest {
            target_id: self.target_id,
            command,
            stdin,
            timeout: COMMAND_TIMEOUT,
        };
        let output = self.ssh_exec.exec(request).await?;
        if !output.success && output.exit_code == Some(255) {
            // ssh reserves 255 for its own connection/authentication failures;
            // no account tool returns it. Treat it as a lost link so a batch
            // aborts after its confirmed prefix.
            return Err(DriverError::Transport(format!(
                "ssh exited 255: {}",
                output.stderr_excerpt()
            )));
        }
        Ok(output)
    }

    /// Run a command that must succeed, mapping a non-zero exit to a per-item
    /// device error.
    async fn exec_checked(
        &self,
        operation: &str,
        command: String,
        stdin: Option<Vec<u8>>,
    ) -> Result<SshExecOutput, DriverError> {
        let output = self.exec(command, stdin).await?;
        if output.success {
            Ok(output)
        } else {
            Err(command_failed(operation, &output))
        }
    }

    /// Provision one user: `useradd`, then set the password via `chpasswd`.
    async fn create_one(&self, user: &NewUser) -> Result<(), DriverError> {
        if !is_valid_account_name(&user.name) {
            return Err(invalid_name(&user.name));
        }
        if !user.authorities.is_empty() {
            return Err(DriverError::Unsupported);
        }
        let groups = user
            .group
            .as_deref()
            .map(validate_group_list)
            .transpose()
            .map_err(DriverError::Device)?;
        reject_unsafe_password(&user.password)?;

        let create = build_create_user_command(&user.name, groups.as_deref());
        let output = self.exec(create, None).await?;
        if !output.success && !is_already_exists(&output) {
            return Err(command_failed("useradd", &output));
        }
        let stdin = build_chpasswd_stdin(&user.name, &user.password);
        self.exec_checked(CMD_CHPASSWD, CMD_CHPASSWD.to_string(), Some(stdin))
            .await?;
        Ok(())
    }

    /// Remove one user; a user that is already absent is an idempotent success.
    async fn delete_one(&self, name: &str) -> Result<(), DriverError> {
        if !is_valid_account_name(name) {
            return Err(invalid_name(name));
        }
        let output = self.exec(build_delete_user_command(name), None).await?;
        if output.success || is_missing_user(&output) {
            Ok(())
        } else {
            Err(command_failed("userdel", &output))
        }
    }

    /// Apply one edit, in the order password → groups → lock state.
    async fn update_one(&self, update: &UserUpdate) -> Result<(), DriverError> {
        if !is_valid_account_name(&update.name) {
            return Err(invalid_name(&update.name));
        }
        if let Some(password) = &update.password {
            reject_unsafe_password(password)?;
            let stdin = build_chpasswd_stdin(&update.name, password);
            self.exec_checked(CMD_CHPASSWD, CMD_CHPASSWD.to_string(), Some(stdin))
                .await?;
        }
        if let Some(group) = &update.group {
            let groups = validate_group_list(group).map_err(DriverError::Device)?;
            let command = build_usermod_groups_command(&update.name, &groups);
            self.exec_checked("usermod", command, None).await?;
        }
        if let Some(authorities) = &update.authorities {
            match parse_lock_action(authorities)? {
                LockAction::Lock => {
                    let command = build_lock_command(&update.name);
                    self.exec_checked("usermod -L", command, None).await?;
                }
                LockAction::Unlock => {
                    let command = build_unlock_command(&update.name);
                    self.exec_checked("usermod -U", command, None).await?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl DeviceDriver for LinuxDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            info: true,
            users: true,
            user_update: true,
            ..Capabilities::default()
        }
    }

    async fn get_info(&self) -> Result<DeviceInfo, DriverError> {
        let output = self
            .exec_checked("uname", CMD_UNAME.to_string(), None)
            .await?;
        let model = output.stdout_text().trim().to_owned();
        Ok(DeviceInfo {
            model: (!model.is_empty()).then_some(model),
            firmware: None,
            serial: None,
            channels: None,
            device_time: None,
        })
    }

    async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError> {
        // `/etc/login.defs` is advisory: an absent/unreadable file is not an
        // error, it falls back to the conventional UID_MIN.
        let defs = self.exec(CMD_LOGIN_DEFS.to_string(), None).await?;
        let uid_min = parse_uid_min(&defs.stdout_text());
        let passwd = self
            .exec_checked("getent passwd", CMD_GETENT_PASSWD.to_string(), None)
            .await?;
        Ok(parse_getent_passwd(&passwd.stdout_text(), uid_min))
    }

    async fn create_users(&self, users: &[NewUser]) -> Result<BulkOutcome, DriverError> {
        let mut outcome = BulkOutcome::default();
        for (index, user) in users.iter().enumerate() {
            match self.create_one(user).await {
                Ok(()) => outcome.succeeded.push(user.name.clone()),
                Err(error) if is_link_fatal(&error) => {
                    return outcome.abort_after_link_failure(
                        user.name.clone(),
                        error,
                        users[index + 1..].iter().map(|user| user.name.clone()),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((user.name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn delete_users(&self, usernames: &[String]) -> Result<BulkOutcome, DriverError> {
        if usernames.is_empty() {
            return Ok(BulkOutcome::default());
        }
        let mut outcome = BulkOutcome::default();
        for (index, name) in usernames.iter().enumerate() {
            match self.delete_one(name).await {
                Ok(()) => outcome.succeeded.push(name.clone()),
                Err(error) if is_link_fatal(&error) => {
                    return outcome.abort_after_link_failure(
                        name.clone(),
                        error,
                        usernames[index + 1..].iter().cloned(),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn update_users(&self, updates: &[UserUpdate]) -> Result<BulkOutcome, DriverError> {
        if updates.is_empty() {
            return Ok(BulkOutcome::default());
        }
        let mut outcome = BulkOutcome::default();
        for (index, update) in updates.iter().enumerate() {
            match self.update_one(update).await {
                Ok(()) => outcome.succeeded.push(update.name.clone()),
                Err(error) if is_link_fatal(&error) => {
                    return outcome.abort_after_link_failure(
                        update.name.clone(),
                        error,
                        updates[index + 1..]
                            .iter()
                            .map(|update| update.name.clone()),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((update.name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError> {
        Err(DriverError::Unsupported)
    }

    async fn recording_status(&self) -> Result<RecordingStatus, DriverError> {
        Err(DriverError::Unsupported)
    }

    async fn snapshot(&self, _channel: Option<u16>) -> Result<SnapshotRef, DriverError> {
        Err(DriverError::Unsupported)
    }

    async fn stream_uri(&self, _channel: Option<u16>) -> Result<StreamUri, DriverError> {
        Err(DriverError::Unsupported)
    }
}

/// A batch-item error is link-fatal when the link or its credentials are broken;
/// a `Device`/`Protocol`/`Unsupported` error is specific to one item and does
/// not stop the batch.
fn is_link_fatal(error: &DriverError) -> bool {
    matches!(
        error,
        DriverError::Auth
            | DriverError::ClockSkew
            | DriverError::Transport(_)
            | DriverError::Timeout(_)
            | DriverError::Unreachable(_)
    )
}

/// Account-state change selected by the `authorities` control tokens.
enum LockAction {
    Lock,
    Unlock,
}

/// Map the `authorities` facet onto a lock-state action.
///
/// The vendor-neutral contract has no lock field yet, so exactly the single
/// tokens `lock`/`unlock` (case-insensitive) are accepted; anything else is a
/// per-item rejection rather than a silent drop.
fn parse_lock_action(authorities: &[String]) -> Result<LockAction, DriverError> {
    if authorities.len() == 1 {
        match authorities[0].trim().to_ascii_lowercase().as_str() {
            "lock" => return Ok(LockAction::Lock),
            "unlock" => return Ok(LockAction::Unlock),
            _ => {}
        }
    }
    Err(DriverError::Device(format!(
        "Linux accounts accept only the lock/unlock authority tokens, got {authorities:?}"
    )))
}

/// Wrap `command` for privilege escalation, returning the command and its stdin.
///
/// Root needs no sudo. Otherwise, when a become password is present, the command
/// is prefixed with `sudo -S -p ''` and the password line is prepended to stdin:
/// `sudo -S` consumes that one line and leaves the rest of stdin for the command
/// (`chpasswd`). Without a become password the command runs unprivileged.
fn wrap_privileged(
    username: &str,
    become_password: Option<&Secret>,
    command: String,
    payload: Option<Vec<u8>>,
) -> (String, Option<Vec<u8>>) {
    if username == "root" {
        return (command, payload);
    }
    let Some(escalation) = become_password else {
        return (command, payload);
    };
    let wrapped = format!("sudo -S -p '' {command}");
    let mut stdin =
        Vec::with_capacity(escalation.expose().len() + 1 + payload.as_ref().map_or(0, Vec::len));
    stdin.extend_from_slice(escalation.expose().as_bytes());
    stdin.push(b'\n');
    if let Some(payload) = payload {
        stdin.extend_from_slice(&payload);
    }
    (wrapped, Some(stdin))
}

/// Parse `UID_MIN` from `/etc/login.defs` content, defaulting to
/// [`DEFAULT_UID_MIN`] when absent.
fn parse_uid_min(login_defs: &str) -> u32 {
    login_defs
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some("UID_MIN"), Some(value)) => value.parse::<u32>().ok(),
                _ => None,
            }
        })
        .unwrap_or(DEFAULT_UID_MIN)
}

/// Parse `getent passwd` output into the human accounts: uid at or above
/// `uid_min`, excluding the conventional `nobody` uid. Malformed and NSS
/// netgroup lines (`+`/`-`) are skipped.
fn parse_getent_passwd(stdout: &str, uid_min: u32) -> Vec<UserAccount> {
    stdout
        .lines()
        .filter_map(parse_passwd_line)
        .filter(|(_, uid)| *uid >= uid_min && *uid < NOBODY_UID)
        .map(|(name, _uid)| UserAccount {
            name: name.to_owned(),
            // The passwd line carries the numeric gid, not a group name; group
            // membership is not attributed from this read.
            group: None,
            authorities: Vec::new(),
        })
        .collect()
}

/// Parse one `getent passwd` line (`name:passwd:uid:gid:gecos:home:shell`).
fn parse_passwd_line(line: &str) -> Option<(&str, u32)> {
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 7 {
        return None;
    }
    let name = fields[0];
    if name.is_empty() || name.starts_with('+') || name.starts_with('-') {
        return None;
    }
    let uid = fields[2].parse::<u32>().ok()?;
    Some((name, uid))
}

/// Whether `name` matches the strict account/group name grammar
/// `^[a-z_][a-z0-9_-]{0,31}$`.
fn is_valid_account_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_lowercase() || first == b'_' => {}
        _ => return false,
    }
    let mut length = 1;
    for byte in bytes {
        if !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-') {
            return false;
        }
        length += 1;
    }
    length <= 32
}

/// Validate and normalize a comma-separated supplementary-group list.
fn validate_group_list(raw: &str) -> Result<String, String> {
    let mut groups = Vec::new();
    for group in raw.split(',') {
        let group = group.trim();
        if group.is_empty() {
            return Err("group list contains an empty name".to_owned());
        }
        if !is_valid_account_name(group) {
            return Err(format!("invalid group name: {group}"));
        }
        groups.push(group);
    }
    Ok(groups.join(","))
}

/// Reject a password that cannot be safely carried as one `chpasswd` line.
///
/// `chpasswd` reads one `name:password` line per account; an embedded newline
/// would inject a second account line and is therefore refused.
fn reject_unsafe_password(password: &Secret) -> Result<(), DriverError> {
    if password.expose().contains(['\n', '\r']) {
        return Err(DriverError::Device(
            "password must not contain a newline".to_owned(),
        ));
    }
    Ok(())
}

/// Single-quote `value` for safe interpolation into a shell command line.
fn shell_single_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// `useradd -m -s /bin/bash [-G groups] -- <name>`.
fn build_create_user_command(name: &str, groups: Option<&str>) -> String {
    let name = shell_single_quote(name);
    match groups {
        Some(groups) => format!(
            "useradd -m -s /bin/bash -G {} -- {}",
            shell_single_quote(groups),
            name
        ),
        None => format!("useradd -m -s /bin/bash -- {name}"),
    }
}

/// `usermod -G <groups> -- <name>`.
fn build_usermod_groups_command(name: &str, groups: &str) -> String {
    format!(
        "usermod -G {} -- {}",
        shell_single_quote(groups),
        shell_single_quote(name)
    )
}

/// `usermod -L -- <name>`.
fn build_lock_command(name: &str) -> String {
    format!("usermod -L -- {}", shell_single_quote(name))
}

/// `usermod -U -- <name>`.
fn build_unlock_command(name: &str) -> String {
    format!("usermod -U -- {}", shell_single_quote(name))
}

/// `userdel -r -- <name>`.
fn build_delete_user_command(name: &str) -> String {
    format!("userdel -r -- {}", shell_single_quote(name))
}

/// The `chpasswd` stdin line `name:password`.
fn build_chpasswd_stdin(name: &str, password: &Secret) -> Vec<u8> {
    format!("{}:{}\n", name, password.expose()).into_bytes()
}

/// Build a per-item error for an invalid account name.
fn invalid_name(name: &str) -> DriverError {
    DriverError::Device(format!("invalid Linux account name: {name}"))
}

/// Build a per-item error from a failed command.
fn command_failed(operation: &str, output: &SshExecOutput) -> DriverError {
    let excerpt = output.stderr_excerpt();
    if excerpt.is_empty() {
        DriverError::Device(format!(
            "{operation} failed with exit code {:?}",
            output.exit_code
        ))
    } else {
        DriverError::Device(format!(
            "{operation} failed with exit code {:?}: {excerpt}",
            output.exit_code
        ))
    }
}

/// Whether `useradd` failed because the account already exists (idempotent
/// create).
fn is_already_exists(output: &SshExecOutput) -> bool {
    if output.exit_code == Some(9) {
        return true;
    }
    let stderr = output.stderr_text().to_ascii_lowercase();
    stderr.contains("already exists") || stderr.contains("already in use")
}

/// Whether `userdel` failed because the account does not exist (idempotent
/// delete).
fn is_missing_user(output: &SshExecOutput) -> bool {
    if output.exit_code == Some(6) {
        return true;
    }
    let stderr = output.stderr_text().to_ascii_lowercase();
    stderr.contains("does not exist") || stderr.contains("no such user")
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// A scripted [`SshExecPort`] that records every request and returns queued
    /// responses in order.
    struct FakeSsh {
        responses: Mutex<VecDeque<Result<SshExecOutput, DriverError>>>,
        requests: Mutex<Vec<SshExecRequest>>,
    }

    impl FakeSsh {
        fn new(responses: Vec<Result<SshExecOutput, DriverError>>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            })
        }

        fn requests(&self) -> Vec<SshExecRequest> {
            self.requests.lock().expect("requests lock").clone()
        }
    }

    #[async_trait]
    impl SshExecPort for FakeSsh {
        async fn exec(&self, request: SshExecRequest) -> Result<SshExecOutput, DriverError> {
            self.requests.lock().expect("requests lock").push(request);
            self.responses
                .lock()
                .expect("responses lock")
                .pop_front()
                .unwrap_or_else(|| Ok(ok_output("")))
        }
    }

    fn ok_output(stdout: &str) -> SshExecOutput {
        SshExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            success: true,
            timed_out: false,
        }
    }

    fn fail_output(exit_code: i32, stderr: &str) -> SshExecOutput {
        SshExecOutput {
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code: Some(exit_code),
            success: false,
            timed_out: false,
        }
    }

    /// A stable, non-nil target id so tests prove the driver propagates it.
    const TEST_TARGET_ID: Uuid = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);

    fn root_driver(ssh: Arc<dyn SshExecPort>) -> LinuxDriver {
        LinuxDriver::new(ssh, TEST_TARGET_ID, "root", None)
    }

    fn sudo_driver(ssh: Arc<dyn SshExecPort>) -> LinuxDriver {
        LinuxDriver::new(
            ssh,
            TEST_TARGET_ID,
            "deploy",
            Some(Secret::new("become-pw")),
        )
    }

    #[test]
    fn parse_uid_min_reads_the_directive_and_defaults() {
        assert_eq!(parse_uid_min("UID_MIN 2000\nUID_MAX 60000\n"), 2000);
        assert_eq!(parse_uid_min("   UID_MIN\t5000\n"), 5000);
        assert_eq!(parse_uid_min("UID_MIN not-a-number\n"), DEFAULT_UID_MIN);
        assert_eq!(parse_uid_min(""), DEFAULT_UID_MIN);
        assert_eq!(parse_uid_min("UID_MAX 60000\n"), DEFAULT_UID_MIN);
    }

    #[test]
    fn parse_getent_passwd_filters_below_uid_min_and_nobody() {
        let stdout = "\
root:x:0:0:root:/root:/bin/bash
daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin
alice:x:1000:1000:Alice:/home/alice:/bin/bash
bob:x:2001:2001:Bob:/home/bob:/bin/sh
nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin
malformed
";
        let users = parse_getent_passwd(stdout, 1000);
        let names: Vec<&str> = users.iter().map(|user| user.name.as_str()).collect();
        assert_eq!(names, vec!["alice", "bob"]);
    }

    #[test]
    fn account_name_grammar_is_strict() {
        for valid in ["alice", "a", "_svc", "user_1", "a-b", &"a".repeat(32)] {
            assert!(is_valid_account_name(valid), "{valid} should be valid");
        }
        for invalid in [
            "",
            "Alice",
            "1alice",
            "-alice",
            "al ice",
            "alice;rm -rf /",
            "alice'quote",
            "üser",
            &"a".repeat(33),
        ] {
            assert!(
                !is_valid_account_name(invalid),
                "{invalid} should be invalid"
            );
        }
    }

    #[test]
    fn group_lists_are_validated_and_normalized() {
        assert_eq!(validate_group_list("sudo").unwrap(), "sudo");
        assert_eq!(validate_group_list("sudo,adm").unwrap(), "sudo,adm");
        assert_eq!(validate_group_list(" sudo , adm ").unwrap(), "sudo,adm");
        assert!(validate_group_list("sudo,,adm").is_err());
        assert!(validate_group_list("sudo;rm").is_err());
    }

    #[test]
    fn single_quote_escapes_embedded_quotes() {
        assert_eq!(shell_single_quote("bob"), "'bob'");
        assert_eq!(shell_single_quote("o'brien"), "'o'\\''brien'");
    }

    #[test]
    fn wrap_privileged_puts_the_become_password_on_stdin_only() {
        let escalation = Secret::new("become-pw");
        let (command, stdin) = wrap_privileged(
            "deploy",
            Some(&escalation),
            "getent passwd".to_owned(),
            None,
        );
        assert_eq!(command, "sudo -S -p '' getent passwd");
        assert!(!command.contains("become-pw"));
        assert_eq!(stdin.as_deref(), Some(b"become-pw\n".as_slice()));
    }

    #[test]
    fn wrap_privileged_prepends_the_password_before_the_payload() {
        let escalation = Secret::new("become-pw");
        let (command, stdin) = wrap_privileged(
            "deploy",
            Some(&escalation),
            "chpasswd".to_owned(),
            Some(b"alice:secret\n".to_vec()),
        );
        assert_eq!(command, "sudo -S -p '' chpasswd");
        assert_eq!(
            stdin.as_deref(),
            Some(b"become-pw\nalice:secret\n".as_slice())
        );
    }

    #[test]
    fn wrap_privileged_skips_sudo_for_root_and_without_a_become_password() {
        assert_eq!(
            wrap_privileged("root", Some(&Secret::new("x")), "id".to_owned(), None),
            ("id".to_owned(), None)
        );
        assert_eq!(
            wrap_privileged("deploy", None, "id".to_owned(), None),
            ("id".to_owned(), None)
        );
    }

    #[test]
    fn create_command_and_chpasswd_payload_are_well_formed() {
        assert_eq!(
            build_create_user_command("alice", None),
            "useradd -m -s /bin/bash -- 'alice'"
        );
        assert_eq!(
            build_create_user_command("alice", Some("sudo,adm")),
            "useradd -m -s /bin/bash -G 'sudo,adm' -- 'alice'"
        );
        assert_eq!(build_delete_user_command("alice"), "userdel -r -- 'alice'");
        assert_eq!(build_lock_command("alice"), "usermod -L -- 'alice'");
        assert_eq!(build_unlock_command("alice"), "usermod -U -- 'alice'");
        assert_eq!(
            build_usermod_groups_command("alice", "sudo"),
            "usermod -G 'sudo' -- 'alice'"
        );
        assert_eq!(
            build_chpasswd_stdin("alice", &Secret::new("s3cret")),
            b"alice:s3cret\n".to_vec()
        );
    }

    #[tokio::test]
    async fn create_sends_the_password_on_stdin_and_never_in_argv() {
        let ssh = FakeSsh::new(vec![Ok(ok_output("")), Ok(ok_output(""))]);
        let driver = root_driver(ssh.clone());
        let users = vec![NewUser {
            name: "alice".to_owned(),
            password: Secret::new("s3cret"),
            group: Some("sudo".to_owned()),
            authorities: Vec::new(),
        }];
        let outcome = driver.create_users(&users).await.expect("create succeeds");
        assert!(outcome.all_succeeded());

        let requests = ssh.requests();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert_eq!(
                request.target_id, TEST_TARGET_ID,
                "every command runs against the driver's target"
            );
        }
        assert_eq!(
            requests[0].command,
            "useradd -m -s /bin/bash -G 'sudo' -- 'alice'"
        );
        assert!(requests[0].stdin.is_none());
        assert_eq!(requests[1].command, "chpasswd");
        assert_eq!(
            requests[1].stdin.as_deref(),
            Some(b"alice:s3cret\n".as_slice())
        );
        for request in &requests {
            assert!(
                !request.command.contains("s3cret"),
                "password must never appear in argv: {}",
                request.command
            );
        }
    }

    #[tokio::test]
    async fn create_rejects_a_newline_password_per_item() {
        let ssh = FakeSsh::new(vec![]);
        let driver = root_driver(ssh);
        let users = vec![NewUser {
            name: "alice".to_owned(),
            password: Secret::new("x\nroot:pwned"),
            group: None,
            authorities: Vec::new(),
        }];
        let outcome = driver.create_users(&users).await.expect("batch returns");
        assert_eq!(outcome.succeeded, Vec::<String>::new());
        assert_eq!(outcome.failed.len(), 1);
        assert!(outcome.failed[0].1.to_string().contains("newline"));
    }

    #[tokio::test]
    async fn create_treats_already_exists_as_idempotent_success() {
        let ssh = FakeSsh::new(vec![
            Ok(fail_output(9, "useradd: user 'alice' already exists\n")),
            Ok(ok_output("")),
        ]);
        let driver = root_driver(ssh.clone());
        let outcome = driver
            .create_users(&[NewUser {
                name: "alice".to_owned(),
                password: Secret::new("pw"),
                group: None,
                authorities: Vec::new(),
            }])
            .await
            .expect("create returns");
        assert!(outcome.all_succeeded());
        // The password is still reconciled on the existing account.
        assert_eq!(ssh.requests().len(), 2);
    }

    #[tokio::test]
    async fn update_password_groups_and_lock_use_the_expected_commands() {
        let ssh = FakeSsh::new(vec![
            Ok(ok_output("")),
            Ok(ok_output("")),
            Ok(ok_output("")),
        ]);
        let driver = root_driver(ssh.clone());
        let outcome = driver
            .update_users(&[UserUpdate {
                name: "alice".to_owned(),
                password: Some(Secret::new("new-pw")),
                group: Some("sudo".to_owned()),
                authorities: Some(vec!["lock".to_owned()]),
            }])
            .await
            .expect("update returns");
        assert!(outcome.all_succeeded());
        let requests = ssh.requests();
        assert_eq!(requests[0].command, "chpasswd");
        assert_eq!(
            requests[0].stdin.as_deref(),
            Some(b"alice:new-pw\n".as_slice())
        );
        assert_eq!(requests[1].command, "usermod -G 'sudo' -- 'alice'");
        assert_eq!(requests[2].command, "usermod -L -- 'alice'");
    }

    #[tokio::test]
    async fn update_unlock_and_rejects_unknown_authorities() {
        let ssh = FakeSsh::new(vec![Ok(ok_output(""))]);
        let driver = root_driver(ssh.clone());
        let outcome = driver
            .update_users(&[UserUpdate {
                name: "alice".to_owned(),
                password: None,
                group: None,
                authorities: Some(vec!["unlock".to_owned()]),
            }])
            .await
            .expect("update returns");
        assert!(outcome.all_succeeded());
        assert_eq!(ssh.requests()[0].command, "usermod -U -- 'alice'");

        let ssh = FakeSsh::new(vec![]);
        let driver = root_driver(ssh);
        let outcome = driver
            .update_users(&[UserUpdate {
                name: "alice".to_owned(),
                password: None,
                group: None,
                authorities: Some(vec!["Monitor".to_owned()]),
            }])
            .await
            .expect("update returns");
        assert_eq!(outcome.failed.len(), 1);
    }

    #[tokio::test]
    async fn delete_missing_user_is_idempotent_success() {
        let ssh = FakeSsh::new(vec![Ok(fail_output(
            6,
            "userdel: user 'alice' does not exist\n",
        ))]);
        let driver = root_driver(ssh);
        let outcome = driver
            .delete_users(&["alice".to_owned()])
            .await
            .expect("delete returns");
        assert!(outcome.all_succeeded());
    }

    #[tokio::test]
    async fn list_users_reads_uid_min_then_getent() {
        let defs = "UID_MIN 1500\n";
        let passwd =
            "root:x:0:0:root:/root:/bin/bash\ncarol:x:1600:1600:Carol:/home/carol:/bin/bash\n";
        let ssh = FakeSsh::new(vec![Ok(ok_output(defs)), Ok(ok_output(passwd))]);
        let driver = root_driver(ssh.clone());
        let users = driver.list_users().await.expect("list succeeds");
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].name, "carol");
        let requests = ssh.requests();
        assert_eq!(requests[0].command, CMD_LOGIN_DEFS);
        assert_eq!(requests[1].command, CMD_GETENT_PASSWD);
    }

    #[tokio::test]
    async fn batch_aborts_on_link_loss_after_a_confirmed_prefix() {
        let ssh = FakeSsh::new(vec![
            Ok(ok_output("")),
            Ok(ok_output("")),
            Err(DriverError::Transport("link lost".to_owned())),
        ]);
        let driver = root_driver(ssh);
        let users = vec![
            NewUser {
                name: "alice".to_owned(),
                password: Secret::new("pw"),
                group: None,
                authorities: Vec::new(),
            },
            NewUser {
                name: "bob".to_owned(),
                password: Secret::new("pw"),
                group: None,
                authorities: Vec::new(),
            },
            NewUser {
                name: "carol".to_owned(),
                password: Secret::new("pw"),
                group: None,
                authorities: Vec::new(),
            },
        ];
        let outcome = driver.create_users(&users).await.expect("partial outcome");
        assert_eq!(outcome.succeeded, vec!["alice".to_owned()]);
        assert_eq!(outcome.failed.len(), 2);
        assert!(matches!(outcome.failed[0].1, DriverError::Transport(_)));
        assert!(matches!(outcome.failed[1].1, DriverError::Unreachable(_)));
    }

    #[tokio::test]
    async fn ssh_exit_255_is_treated_as_a_lost_link() {
        let ssh = FakeSsh::new(vec![Ok(SshExecOutput {
            stdout: Vec::new(),
            stderr: b"Connection to 10.0.0.5 closed.\n".to_vec(),
            exit_code: Some(255),
            success: false,
            timed_out: false,
        })]);
        let driver = root_driver(ssh);
        let outcome = driver.delete_users(&["alice".to_owned()]).await;
        // Nothing acknowledged + ErrorWithoutAcknowledged => top-level Err.
        assert!(matches!(outcome, Err(DriverError::Transport(_))));
    }

    #[test]
    fn capabilities_are_users_user_update_and_info_only() {
        let ssh = FakeSsh::new(vec![]);
        let capabilities = root_driver(ssh).capabilities();
        assert!(capabilities.info);
        assert!(capabilities.users);
        assert!(capabilities.user_update);
        assert!(!capabilities.disk);
        assert!(!capabilities.recording);
        assert!(!capabilities.snapshot);
        assert!(!capabilities.streaming);
        assert!(!capabilities.channels);
        assert!(!capabilities.ptz);
        assert!(!capabilities.export);
        assert!(!capabilities.authority_catalog);
        assert!(!capabilities.recording_search);
    }

    #[tokio::test]
    async fn get_info_reports_the_uname_readout() {
        let ssh = FakeSsh::new(vec![Ok(ok_output("Linux 6.1.0 x86_64\n"))]);
        let driver = root_driver(ssh);
        let info = driver.get_info().await.expect("info succeeds");
        assert_eq!(info.model.as_deref(), Some("Linux 6.1.0 x86_64"));
    }

    #[tokio::test]
    async fn sudo_driver_wraps_commands_and_feeds_the_become_password() {
        let ssh = FakeSsh::new(vec![Ok(ok_output(""))]);
        let driver = sudo_driver(ssh.clone());
        let outcome = driver
            .delete_users(&["alice".to_owned()])
            .await
            .expect("delete returns");
        assert!(outcome.all_succeeded());
        let requests = ssh.requests();
        assert_eq!(requests[0].command, "sudo -S -p '' userdel -r -- 'alice'");
        assert_eq!(
            requests[0].stdin.as_deref(),
            Some(b"become-pw\n".as_slice())
        );
    }
}
