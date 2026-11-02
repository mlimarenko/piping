/**
 * Demo-only helper: recover the host name encoded in a demo session relay URL.
 *
 * The demo create-session mock (`handlers.ts`) builds websocket URLs of the form
 * `wss://demo.invalid/relay/{protocol}/{hostName}`. The mock RDP/VNC desktops
 * use the host name for their title bars / status chips. Imported only by
 * demo-gated code paths.
 */
export function demoHostNameFromWsUrl(websocketUrl: string): string {
  try {
    const { pathname } = new URL(websocketUrl);
    const last = pathname.split("/").filter(Boolean).pop();
    if (last) return decodeURIComponent(last);
  } catch {
    // Non-URL input (shouldn't happen in demo mode) — fall through.
  }
  return "remote-host";
}
