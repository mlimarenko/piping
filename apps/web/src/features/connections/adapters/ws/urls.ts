/**
 * Build the absolute URL the browser should use to connect to a session.
 *
 * The backend's `proxy.websocketUrl` is either:
 *   - a path like `/api/v1/ws/ssh/{session_id}` — in which case we fill
 *     it out with `wss://${location.host}`, or
 *   - an absolute `ws(s)://…` URL — in which case we respect the scheme
 *     and host the backend chose.
 *
 * The per-session `token` is always appended as `?token=…`. We use the
 * `URL` API so any existing query params on the backend-provided URL are
 * preserved instead of being overwritten by naive string concatenation.
 */
/**
 * Guard against opening a session WebSocket with degraded/stale inputs.
 *
 * A live session needs BOTH a non-empty per-session `token` and a
 * `websocketUrl` that resolves to a real relay path (e.g.
 * `/api/v1/ws/ssh/{id}` for production, `/relay/ssh/{host}` for demo). When a
 * stale `sessionStorage` handoff or a restored-but-unhydrated tab carries an
 * empty token and/or a host-only URL (no path), naively appending `?token=`
 * yields a bogus `wss://host/?token=` that the browser tries — and fails — to
 * open, spamming the console with "can't establish connection" errors.
 *
 * Returns true only when the socket is worth opening: token present and the
 * resolved pathname is something more specific than the root `/`.
 */
export function isSessionWsConnectable(websocketUrl: string, token: string): boolean {
  if (!websocketUrl || token.trim() === "") return false;
  try {
    const base = typeof window !== "undefined" ? window.location.origin : "http://localhost";
    const { pathname } = new URL(websocketUrl, base);
    // Reject host-only/root URLs (pathname "" or "/"); a real relay path is longer.
    return pathname.length > 1;
  } catch {
    // Malformed URL — definitely not connectable.
    return false;
  }
}

export function buildSessionWsUrl(websocketUrl: string, token: string): string {
  const isAbsolute = /^wss?:\/\//i.test(websocketUrl);
  if (isAbsolute) {
    const parsed = new URL(websocketUrl);
    parsed.searchParams.set("token", token);
    return parsed.toString();
  }
  if (typeof window === "undefined") {
    throw new Error(
      "buildSessionWsUrl: relative websocketUrl requires a browser window to resolve the host",
    );
  }
  const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
  const parsed = new URL(`${scheme}//${window.location.host}${websocketUrl}`);
  parsed.searchParams.set("token", token);
  return parsed.toString();
}
