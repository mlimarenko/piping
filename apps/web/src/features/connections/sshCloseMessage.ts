/**
 * Single source of truth for turning an SSH session close `(code, reason)` into
 * localized i18n keys. Both the in-terminal disconnected overlay
 * ({@link LiveTerminal}) and the connections toast ({@link ConnectionsWorkspace})
 * classify failures here — there is no second inline regex.
 *
 * The backend stamps the close frame `reason` from a fixed vocabulary (see
 * `apps/api/src/proxy/ssh.rs` `classify_ssh_stderr_message`):
 *   - 1008 "SSH authentication failed"        → credentials rejected
 *   - 1013 "Target refused connection"        → TCP refused
 *   - 1013 "Target is unreachable"            → no route / DNS / network down
 *   - 1013 "Target connection timed out"      → connect/SSH handshake timeout
 *   - 1013 "Upstream connection lost"         → established session died mid-stream
 *
 * The reason match is substring/case-insensitive so it also covers the wider
 * phrasing the backend may emit (e.g. "authentication failed", "permission
 * denied") and the frontend-synthesized auth close
 * ({@link SSH_AUTH_FAILURE_REASON}).
 */
export interface SshCloseMessageKeys {
  /** Short headline key (no interpolation). */
  titleKey: string;
  /** Detailed key; interpolates `{{host}}` when a host label is available. */
  descriptionKey: string;
}

/**
 * WS close `(code, reason)` synthesized by the adapter when it detects an SSH
 * auth failure in the PTY stream that the backend did not turn into a close
 * frame. Mirrors the backend's auth close so the rest of the pipeline is
 * source-agnostic.
 */
export const SSH_AUTH_FAILURE_CODE = 1008;
export const SSH_AUTH_FAILURE_REASON = "SSH authentication failed";

/**
 * Classify an SSH session close into localized message keys.
 *
 * Order matters: auth is checked before the generic network buckets so a
 * "permission denied" reason is never mis-bucketed as a network error.
 */
export function sshCloseMessage(
  code: number | undefined,
  reason: string | undefined,
): SshCloseMessageKeys {
  const text = reason?.trim().toLowerCase() ?? "";

  if (text && /auth|credential|password|permission denied/.test(text)) {
    return {
      titleKey: "connections.authFailed",
      descriptionKey: "connections.authFailedDescription",
    };
  }
  if (text && /upstream connection lost/.test(text)) {
    return {
      titleKey: "connections.upstreamLost",
      descriptionKey: "connections.upstreamLostDescription",
    };
  }
  if (text && /unreachable|refused|no route|timeout|timed out/.test(text)) {
    return {
      titleKey: "connections.hostUnreachable",
      descriptionKey: "connections.hostUnreachableDescription",
    };
  }
  // No usable reason text: classify by close code. Abnormal/network codes get
  // the network bucket; anything else falls back to the generic failure.
  if (code === undefined || code === 1006 || code === 1015) {
    return {
      titleKey: "connections.networkError",
      descriptionKey: "connections.networkErrorDescription",
    };
  }
  return {
    titleKey: "connections.connectionFailed",
    descriptionKey: "connections.networkErrorDescription",
  };
}

/**
 * Narrow OpenSSH "auth rejected" signature for the PTY-stream detector.
 *
 * Matches only the canonical client line OpenSSH prints when every offered auth
 * method is exhausted, e.g.:
 *   `testuser@10.0.0.1: Permission denied (publickey,password,keyboard-interactive).`
 * The method list MUST contain `password` or `keyboard-interactive` (the
 * interactive auth methods this proxy uses), so a bare `Permission denied`
 * inside benign program output (cat-ing a logfile, a failed `sudo`) does NOT
 * match. The detector additionally scopes this to the session-startup window
 * before any keystroke (see {@link attachSshTerminal}), so even this narrow
 * pattern can only fire on the real connect-time auth banner.
 */
export const SSH_AUTH_DENIED_PTY_RE =
  /Permission denied \((?=[a-z0-9,-]*\b(?:password|keyboard-interactive)\b)[a-z0-9,-]+\)/i;
