use super::*;
pub(super) fn log_ssh_inbound_frame(
    connection_id: Uuid,
    frame_index: u64,
    frame_type: &'static str,
    payload_len: usize,
) {
    let should_log = frame_index <= INBOUND_FRAME_LOG_INITIAL_LIMIT
        || frame_index.is_multiple_of(INBOUND_FRAME_LOG_EVERY_NTH_EVENT);
    if !should_log {
        return;
    }

    log_event(
        LogLevel::Debug,
        "proxy.ssh.inbound.frame",
        EventFields::default()
            .with_operation("proxy_ssh_ws_receive")
            .with_resource("proxy_connection", connection_id.to_string())
            .with_context("frame_index", frame_index.to_string())
            .with_context("frame_type", frame_type.to_string())
            .with_context("payload_bytes", payload_len.to_string()),
    );
}

pub(super) fn normalize_stderr_excerpt(stderr: &str) -> Option<String> {
    let collapsed = stderr
        .replace(['\n', '\r', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }
    let excerpt: String = trimmed.chars().take(STDERR_EXCERPT_CHAR_LIMIT).collect();
    Some(excerpt)
}

pub(super) fn parse_ws_control_message(message: &str) -> Option<WsControlMessage> {
    if !message.starts_with(WS_CONTROL_PREFIX) {
        return None;
    }
    let payload = &message[WS_CONTROL_PREFIX.len()..];
    serde_json::from_str::<WsControlMessage>(payload).ok()
}

pub(super) fn classify_ssh_stderr_message(stderr: &str) -> Option<(u16, &'static str)> {
    let message = stderr.to_ascii_lowercase();
    if message.contains("connection timed out")
        || message.contains("operation timed out")
        || message.contains("i/o timeout")
        || message.contains("timed out")
    {
        return Some((WS_CLOSE_TARGET_UNAVAILABLE, "Target connection timed out"));
    }
    if message.contains("connection refused") {
        return Some((WS_CLOSE_TARGET_UNAVAILABLE, "Target refused connection"));
    }
    if message.contains("no route to host")
        || message.contains("network is unreachable")
        || message.contains("temporary failure in name resolution")
        || message.contains("could not resolve hostname")
        || message.contains("name or service not known")
    {
        return Some((WS_CLOSE_TARGET_UNAVAILABLE, "Target is unreachable"));
    }
    if message.contains("permission denied")
        || message.contains("authentication failed")
        || message.contains("too many authentication failures")
        || message.contains("publickey")
        || message.contains("error in libcrypto")
    {
        return Some((WS_CLOSE_INVALID_PAYLOAD, "SSH authentication failed"));
    }
    // Abnormal upstream loss mid-session: the established connection died (peer
    // reset, half-open drop, ServerAlive timeout, remote host killed the link).
    // Distinguished from a CLEAN logout, which prints the bare
    // "Connection to <host> closed." — we match ONLY the "closed by remote host"
    // variant so a normal exit is NOT mis-flagged.
    //
    // We deliberately match ONLY ssh-emitted, low-false-positive strings:
    // - "client_loop"        → ssh's own `client_loop: send disconnect: ...`
    //                          (covers its "Broken pipe" variant too).
    // - "closed by remote host" → the abnormal half of "closed by remote host".
    // With `-tt` ssh merges stderr into the PTY, so generic libc phrases like
    // "broken pipe" or "connection reset by peer" can appear in BENIGN program
    // output (cat-ing a log, a failed curl); because this classifier is sticky
    // (first non-NORMAL match wins and then disables the exit-255 fold-in below),
    // matching them would permanently mis-stamp normal sessions as 1013. They are
    // therefore intentionally NOT matched here — the robust, false-positive-free
    // discriminator for silent drops is the exit-code-255 fold-in at session end
    // (those exit 255 regardless of any output text).
    if message.contains("client_loop") || message.contains("closed by remote host") {
        return Some((WS_CLOSE_TARGET_UNAVAILABLE, "Upstream connection lost"));
    }
    None
}

pub(super) fn classify_ssh_startup_close_frame(error: &AppError) -> (u16, &'static str) {
    match error {
        AppError::Validation(_) => (WS_CLOSE_INVALID_PAYLOAD, "Invalid SSH connection payload"),
        AppError::NotFound(_) => (WS_CLOSE_INVALID_PAYLOAD, "Connection not found"),
        AppError::TooManyRequests(_) => {
            (WS_CLOSE_TARGET_UNAVAILABLE, "Too many active SSH sessions")
        }
        _ => classify_ssh_stderr_message(&error.to_string())
            .unwrap_or((WS_CLOSE_INTERNAL_ERROR, "SSH session failed to start")),
    }
}

/// Validate an SSH endpoint host the same way the connect path validates the
/// target: it must normalize to a bare IP address (brackets / CIDR suffix
/// stripped). This blocks jump-host SSRF — a `jump[].host` of `evil.example`,
/// `[::]`, or a CIDR string is rejected — and, combined with [`reject_option_like`],
/// closes `ssh -J` option injection. Returns the canonical IP string.
pub(super) fn validate_ssh_host(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("SSH host is required");
    }
    // An option-looking host (`-oProxyCommand=...`) must never reach the ssh arg
    // vector as a destination.
    reject_option_like(trimmed, "SSH host")?;
    let without_brackets = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    let ip_part = without_brackets
        .split('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("SSH host is not a valid IP address")?;
    IpAddr::from_str(ip_part)
        .map(|value| value.to_string())
        .map_err(|_| "SSH host is not a valid IP address")
}

/// Reject any value that OpenSSH would parse as an option flag (leading `-`).
/// Without this, a username/host of `-oProxyCommand=...` smuggles arbitrary ssh
/// options into the argv (option injection / RCE).
pub(super) fn reject_option_like(value: &str, field: &'static str) -> Result<(), &'static str> {
    if value.trim_start().starts_with('-') {
        return match field {
            "SSH host" => Err("SSH host must not start with '-'"),
            "SSH username" => Err("SSH username must not start with '-'"),
            _ => Err("SSH field must not start with '-'"),
        };
    }
    Ok(())
}

/// Validate an SSH username: non-empty and not option-looking (leading `-`).
pub(super) fn validate_ssh_username(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("SSH username is required");
    }
    reject_option_like(trimmed, "SSH username")?;
    Ok(trimmed.to_string())
}

pub(super) fn extract_ssh_launch_config(payload: &Value) -> Result<SshLaunchConfig, &'static str> {
    let (raw_host, port) = super::super::extract_host_port(payload)?;
    let host = validate_ssh_host(&raw_host)?;
    let credentials = payload
        .as_object()
        .and_then(|map| map.get("credentials"))
        .and_then(Value::as_object);

    let username = credentials
        .and_then(|map| map.get("username"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("Proxy payload credentials.username is required for SSH")
        .and_then(validate_ssh_username)?;

    let password = credentials
        .and_then(|map| map.get("password"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    let private_key = credentials
        .and_then(|map| map.get("private_key"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    let jump = extract_ssh_jump_chain(payload)?;

    Ok(SshLaunchConfig {
        host,
        port,
        username,
        password,
        private_key,
        jump,
    })
}

/// Parse the resolved `jump[]` bastion chain from the proxy payload. Each entry
/// is a flat object (`host`, `port?`, `credentials.{username,private_key?}`)
/// produced by the connection-config resolver. Absent/empty `jump` yields an
/// empty chain, leaving direct connections unaffected.
pub(super) fn extract_ssh_jump_chain(payload: &Value) -> Result<Vec<SshJumpHop>, &'static str> {
    let Some(jump_value) = payload.as_object().and_then(|map| map.get("jump")) else {
        return Ok(Vec::new());
    };
    let Some(items) = jump_value.as_array() else {
        return Err("Proxy payload jump must be an array of hop objects");
    };

    let mut hops = Vec::with_capacity(items.len());
    for item in items {
        let Some(hop) = item.as_object() else {
            return Err("Proxy payload jump hop must be a JSON object");
        };
        // Jump hosts get the SAME IP-format/allowlist validation as the target,
        // closing the jump-host SSRF gap: an unvalidated `jump[].host` could
        // otherwise pivot the proxy to an arbitrary internal address or smuggle
        // ssh options. Both host and username also reject leading `-`.
        let host = hop
            .get("host")
            .and_then(Value::as_str)
            .ok_or("Proxy payload jump hop host is required")
            .and_then(validate_ssh_host)?;
        let port = hop
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .filter(|port| *port != 0)
            .unwrap_or(22);
        let credentials = hop.get("credentials").and_then(Value::as_object);
        let username = credentials
            .and_then(|map| map.get("username"))
            .and_then(Value::as_str)
            .ok_or("Proxy payload jump hop credentials.username is required")
            .and_then(validate_ssh_username)?;
        let private_key = credentials
            .and_then(|map| map.get("private_key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        hops.push(SshJumpHop {
            host,
            port,
            username,
            private_key,
        });
    }
    Ok(hops)
}
