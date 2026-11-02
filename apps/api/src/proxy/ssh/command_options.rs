use super::*;
pub(super) fn append_ssh_control_options(command: &mut Command, control_path: Option<&str>) {
    if let Some(path) = control_path.filter(|value| !value.trim().is_empty()) {
        command
            // OpenSSH runs ProxyCommand through $SHELL. Pin its shell for the
            // fail-closed fallback rather than inheriting a service variable.
            .env("SHELL", "/bin/sh")
            // Sidecars use only payload-derived target data and the existing
            // master. Ignoring ambient ssh config avoids Match/ProxyCommand
            // hooks changing their fail-closed transport semantics.
            .arg("-F")
            .arg("/dev/null")
            .arg("-S")
            .arg(path)
            .arg("-o")
            .arg("ControlMaster=no")
            // `ControlMaster=no` normally falls back to a new direct
            // connection when the socket disappears. This sidecar-only
            // ProxyCommand is reached only on that fallback path and fails
            // before a target TCP connection can be made.
            .arg("-o")
            .arg(format!("ProxyCommand={SSH_MUX_ONLY_FALLBACK_BLOCKER}"))
            .arg("-o")
            .arg("ProxyUseFdpass=no")
            .arg("-o")
            .arg("BatchMode=yes");
    }
}

pub(super) fn append_scp_control_options(command: &mut Command, control_path: Option<&str>) {
    if let Some(path) = control_path.filter(|value| !value.trim().is_empty()) {
        command
            .env("SHELL", "/bin/sh")
            .arg("-F")
            .arg("/dev/null")
            .arg("-o")
            .arg(format!("ControlPath={path}"))
            .arg("-o")
            .arg("ControlMaster=no")
            .arg("-o")
            .arg(format!("ProxyCommand={SSH_MUX_ONLY_FALLBACK_BLOCKER}"))
            .arg("-o")
            .arg("ProxyUseFdpass=no")
            .arg("-o")
            .arg("BatchMode=yes");
    }
}

/// Adds durable post-connect authentication evidence to the interactive
/// ControlMaster only. Sidecars deliberately do not receive this option: they
/// must reuse the already-authenticated master and have no authority to create
/// a new session-generation marker.
pub(super) fn append_ssh_authentication_marker_options(command: &mut Command, marker: &Path) {
    let authentication_marker_command = ssh_authentication_marker_local_command(marker);
    command
        // OpenSSH executes LocalCommand through $SHELL. The marker path is
        // generated locally and shell-quoted, while pinning the shell prevents
        // an inherited service value from changing the command.
        .env("SHELL", "/bin/sh")
        // `LocalCommand` runs synchronously only after OpenSSH successfully
        // connects. Its marker survives short-lived socket removal until final
        // close classification has consumed the historical authentication fact.
        .arg("-o")
        .arg("PermitLocalCommand=yes")
        .arg("-o")
        .arg(format!("LocalCommand={authentication_marker_command}"));
}

pub(super) fn append_identities_only_when_keyed(command: &mut Command, has_private_key: bool) {
    if has_private_key {
        command.arg("-o").arg("IdentitiesOnly=yes");
    }
}
