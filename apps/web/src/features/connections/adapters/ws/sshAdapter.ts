/**
 * SSH viewer built on xterm.js. Owns the terminal instance, the fit addon,
 * and the WebSocket through {@link openSessionSocket}. The wire format is
 * raw PTY bytes in both directions — a persistent bridge forwards keystrokes
 * to stdin and writes incoming messages straight to the terminal.
 *
 * The backend negotiates `Sec-WebSocket-Protocol: guacamole` on the SSH
 * endpoint (`/api/v1/ws/ssh/{session_id}`); the subprotocol name is historical — the
 * wire is still plain PTY, not the Guacamole protocol.
 */

import "@xterm/xterm/css/xterm.css";

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { attachSshTouchInput } from "./sshTouchInput";

import { openSessionSocket, RECONNECT_GIVE_UP_REASON, type SessionSocket } from "./sessionSocket";
import {
  SSH_AUTH_DENIED_PTY_RE,
  SSH_AUTH_FAILURE_CODE,
  SSH_AUTH_FAILURE_REASON,
} from "../../sshCloseMessage";

/**
 * Live-terminal connection status driven by the reconnect state machine.
 *
 * - `live`                : socket open, terminal interactive.
 * - `reconnecting-silent` : dropped < SILENT_WINDOW_MS ago; retrying quietly.
 *                           No UI, xterm keeps its frozen buffer, input is
 *                           blocked until a socket is open again.
 * - `reconnecting-visible`: dropped >= SILENT_WINDOW_MS ago and still down;
 *                           show a non-modal banner and BLOCK terminal input.
 * - `failed`              : reconnect deadline exceeded; session given up.
 */
export type SshConnectionStatus =
  | "live"
  | "reconnecting-silent"
  | "reconnecting-visible"
  | "failed";

export interface SshAdapterOptions {
  container: HTMLElement;
  /** Absolute `wss://` URL from the backend, including any existing query. */
  url: string;
  /** Per-session token issued by `POST /api/v1/connections/sessions`. */
  token: string;
  /** Called when the WebSocket successfully opens and the terminal is live. */
  onOpen?: () => void;
  /** Called when the WebSocket closes; code 1000 = normal, others = failure. */
  onClose?: (code?: number, reason?: string) => void;
  /** Called whenever the reconnect state machine transitions state. */
  onStatus?: (status: SshConnectionStatus) => void;
}

export interface SshAdapter {
  dispose(): void;
  focus(): void;
  /** Blur the terminal's hidden input so the OS soft keyboard is dismissed. */
  blur(): void;
  fit(): void;
  isLive(): boolean;
  sendText(data: string): boolean;
  sendKey(key: SshTerminalKey): boolean;
  /** Current terminal font size in px, always within [{@link SSH_FONT_SIZE_MIN}, {@link SSH_FONT_SIZE_MAX}]. */
  getFontSize(): number;
  /**
   * Set the terminal font size, clamped to a bounded range, then re-fit so the
   * new cell metrics propagate to the PTY. Never disposes or reconnects.
   * @returns the font size actually applied.
   */
  setFontSize(size: number): number;
  /**
   * Toggle the OS soft keyboard for the terminal's hidden textarea. When
   * disabled the terminal cannot be focused by a tap and no virtual keyboard
   * appears, which keeps a mobile session from hijacking the keyboard before
   * the user asks for it. Never disposes or reconnects.
   */
  setSoftKeyboardEnabled(enabled: boolean): void;
}

/** Bounded terminal font size; the UI's − / + controls clamp to this range. */
export const SSH_FONT_SIZE_MIN = 10;
export const SSH_FONT_SIZE_MAX = 24;
export const SSH_FONT_SIZE_DEFAULT = 13;

const SSH_KEYS = { enter: "\r", tab: "\t", escape: "\x1b", interrupt: "\x03", pageUp: "\x1b[5~", pageDown: "\x1b[6~" } as const;
const SSH_ARROWS = { up: "A", down: "B", right: "C", left: "D" } as const;
export type SshTerminalKey = keyof typeof SSH_KEYS | keyof typeof SSH_ARROWS;

const SSH_SUBPROTOCOL = "guacamole";

// Reconnect timing. The silent phase (0–10s) retries quietly with no UI; the
// visible phase (10–60s) shows a banner and blocks input; past the 60s
// wall-clock deadline the socket gives up and we surface the failed overlay.
// Backoff stays modest so the silent phase gets several quick attempts.
const SILENT_WINDOW_MS = 10_000;
const RECONNECT_DEADLINE_MS = 60_000;
// Backend close reason (code 1000) meaning "only the client link dropped, the
// server-side SSH session is still running" — reconnectable, NOT terminal.
// Must stay in sync with the proxy's WS_REASON_DETACHED_ALIVE (apps/api).
const DETACHED_ALIVE_REASON = "detached_alive";
const RECONNECT_BASE_DELAY_MS = 500;
const RECONNECT_MAX_DELAY_MS = 3_000;

// Startup auth-failure detection. The backend reliably stamps a 1008 close on
// pre-connect failures (refused/unreachable/timeout), but when the upstream ssh
// client prints "Permission denied (...)" and DOES NOT exit (some sshd auth-method
// combinations leave the PTY open), no close frame ever arrives. We detect that
// canonical auth banner in the early PTY stream and synthesize the same close the
// backend would have sent. Scoped tightly to avoid false positives: only before
// the first keystroke, only within the first few seconds of the first open, and
// only across the first few KiB of output.
const AUTH_DETECT_WINDOW_MS = 5_000;
const AUTH_DETECT_MAX_BYTES = 8 * 1024;

function isReconnectableSshClose(close: CloseEvent): boolean {
  if (close.reason === DETACHED_ALIVE_REASON) return true;
  if (close.reason === RECONNECT_GIVE_UP_REASON) return false;

  const reason = close.reason.trim().toLowerCase();
  if (
    reason.includes("connection not found") ||
    reason.includes("already attached") ||
    reason.includes("no longer active") ||
    reason.includes("session ended") ||
    reason.includes("authentication failed") ||
    reason.includes("invalid ssh") ||
    reason.includes("target") ||
    reason.includes("ssh session failed")
  ) {
    return false;
  }

  // Browser-level transport failures have no reliable close frame. Retry those
  // in place because the backend may still keep the PTY alive.
  return close.code === 1001 || close.code === 1006 || close.code === 1015;
}

function appendToken(url: string, token: string): string {
  const base = typeof window !== "undefined" ? window.location.origin : "http://localhost";
  const parsed = new URL(url, base);
  parsed.searchParams.set("token", token);
  return parsed.toString();
}

export function attachSshTerminal(opts: SshAdapterOptions): SshAdapter {
  const { container, url, token, onOpen, onClose, onStatus } = opts;

  let disposed = false;
  let raf1: number | undefined;
  let raf2: number | undefined;

  const term = new Terminal({
    convertEol: true,
    fontSize: SSH_FONT_SIZE_DEFAULT,
    theme: { background: "#0b0d10" },
  });

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.open(container);
  // Whether the hidden textarea may summon the OS soft keyboard. Desktop keeps
  // this on (the component never disables it there); a mobile session turns it
  // off until the user explicitly opts into direct terminal input.
  let softKeyboardEnabled = true;
  if (term.textarea) {
    term.textarea.inputMode = "text";
    term.textarea.autocapitalize = "none";
    term.textarea.autocomplete = "off";
    term.textarea.setAttribute("autocorrect", "off");
    term.textarea.spellcheck = false;
    // iOS zooms focused inputs below 16px, including xterm's hidden IME input.
    term.textarea.style.fontSize = "16px";
  }

  // Control messages to the backend must be prefixed with "__piping_ws__:" so
  // the backend can distinguish them from raw PTY stdin bytes.
  const WS_CONTROL_PREFIX = "__piping_ws__:";

  // --- Reconnect state machine -------------------------------------------
  // The SSH PTY/process lives on the backend independently of this WebSocket,
  // so a client drop does NOT kill the running command. We reconnect inside
  // this same adapter instance (sessionSocket swaps the underlying socket),
  // keeping the SAME Terminal so its rendered scrollback survives the gap.
  let status: SshConnectionStatus = "live";
  let hasOpenedOnce = false;
  let openReportPending = false;
  // True between a drop and the next successful open; gates the reset-on-
  // reconnect and the silent→visible escalation timer.
  let reconnecting = false;
  let silentTimer: number | undefined;

  // --- Startup auth-failure detector -------------------------------------
  // See AUTH_DETECT_* above. `authDetectActive` is true only during the bounded
  // startup window before the first keystroke; once the user types, the upstream
  // is interactive and a "Permission denied" line is ordinary shell output, not a
  // connect-time failure, so we stop scanning.
  let authDetectActive = true;
  let authDetectBytes = 0;
  let authDetectBuf = "";
  let authClosed = false;
  let openedAtMs: number | undefined;

  function stopAuthDetect(): void {
    authDetectActive = false;
    authDetectBuf = "";
  }

  // Feed inbound terminal text to the auth detector. On a confident match,
  // synthesize the close the backend should have sent (1008) so the normal
  // terminal-close path (status → failed, onClose) fires exactly once.
  function inspectForAuthFailure(text: string): void {
    if (!authDetectActive || disposed || authClosed) return;
    if (
      openedAtMs !== undefined &&
      typeof performance !== "undefined" &&
      performance.now() - openedAtMs > AUTH_DETECT_WINDOW_MS
    ) {
      stopAuthDetect();
      return;
    }
    // Keep only enough recent text to span a banner that straddles two frames,
    // and stop scanning once past the byte budget.
    authDetectBytes += text.length;
    authDetectBuf = (authDetectBuf + text).slice(-2048);
    if (authDetectBytes > AUTH_DETECT_MAX_BYTES) {
      stopAuthDetect();
      return;
    }
    if (SSH_AUTH_DENIED_PTY_RE.test(authDetectBuf)) {
      synthesizeAuthFailureClose();
    }
  }

  function synthesizeAuthFailureClose(): void {
    if (authClosed || disposed) return;
    authClosed = true;
    stopAuthDetect();
    clearSilentTimer();
    setStatus("failed");
    // Tear down the live socket so it cannot reconnect or keep streaming after
    // the failure; mirror the backend's intentional teardown.
    try {
      socket.close(SSH_AUTH_FAILURE_CODE, SSH_AUTH_FAILURE_REASON);
    } catch {
      // ignore — close is best-effort; onClose below is the authoritative signal.
    }
    onClose?.(SSH_AUTH_FAILURE_CODE, SSH_AUTH_FAILURE_REASON);
  }

  function setStatus(next: SshConnectionStatus): void {
    if (status === next) return;
    status = next;
    // Block stdin whenever the relay is not live. We do not queue terminal
    // input across reconnects because PTY/Guacamole streams are ordering
    // sensitive; sending stale keystrokes after a reattach is worse than
    // dropping input typed during the outage.
    term.options.disableStdin = next !== "live";
    onStatus?.(next);
  }

  function reportOpenIfReady(): void {
    if (!openReportPending) return;
    openReportPending = false;
    onOpen?.();
  }

  function clearSilentTimer(): void {
    if (silentTimer !== undefined) {
      if (typeof window !== "undefined") window.clearTimeout(silentTimer);
      silentTimer = undefined;
    }
  }

  const socket: SessionSocket = openSessionSocket({
    url: appendToken(url, token),
    protocols: [SSH_SUBPROTOCOL],
    // The backend keeps the SSH session alive across a client WS drop and
    // re-attaches by session_id, replaying history. Reconnect quietly for the
    // first few seconds, then give up at the wall-clock deadline (~60s).
    reconnect: {
      baseDelayMs: RECONNECT_BASE_DELAY_MS,
      maxDelayMs: RECONNECT_MAX_DELAY_MS,
      deadlineMs: RECONNECT_DEADLINE_MS,
    },
    shouldReconnect: isReconnectableSshClose,
  });

  function sendInput(data: string, focus: boolean): boolean {
    if (disposed || status !== "live" || socket.readyState !== "open") return false;
    try {
      stopAuthDetect();
      socket.send(data);
      if (focus) term.focus();
      return true;
    } catch { return false; }
  }
  const detachTouch = attachSshTouchInput(term.element ?? container, () => {
    // A tap only focuses the terminal (summoning the soft keyboard) when soft
    // input is enabled. In a mobile command-input session it is disabled, so a
    // stray tap must not steal the keyboard from the command field.
    if (softKeyboardEnabled && !disposed && status === "live" && socket.readyState === "open") {
      term.focus();
    }
  });

  // Wire the reconnecting socket to xterm with our own persistent bridge.
  // @xterm/addon-attach disposes itself on any `close`/`error`; that is correct
  // for a one-shot WebSocket but wrong here because SessionSocket emits
  // per-attempt close events while it keeps reconnecting. If the addon disposes
  // after the first transient drop, the next successful reattach receives the
  // backend replay but nothing writes it to xterm, leaving a blank/frozen tab
  // until the user refreshes. The bridge below stays subscribed until adapter
  // disposal and only writes terminal data while this adapter is alive.
  const terminalBridge = attachTerminalBridge(term, socket, WS_CONTROL_PREFIX, {
    onReady: reportOpenIfReady,
    onInboundText: inspectForAuthFailure,
    shouldInspectInbound: () => {
      if (!authDetectActive) return false;
      if (
        openedAtMs !== undefined &&
        typeof performance !== "undefined" &&
        performance.now() - openedAtMs > AUTH_DETECT_WINDOW_MS
      ) {
        stopAuthDetect();
        return false;
      }
      return true;
    },
    onUserInput: stopAuthDetect,
  });

  // Track the last dims sent to backend to avoid re-sending identical values.
  // xterm deduplicates term.resize() no-ops, so term.onResize won't fire when
  // cols/rows haven't changed — we must check explicitly after every fit().
  let lastSent: { cols: number; rows: number } | null = null;

  // Coalesce resize dispatch to the backend. A drag-to-resize or window resize
  // fires the ResizeObserver/onResize on every grid-cell step, which would
  // flood the PTY with winsize+SIGWINCH and make the remote ssh client send a
  // window-change per step. Fullscreen TUIs (mc, htop, vim) then redraw on
  // every round-trip and render torn/garbled mid-drag. We fit() xterm locally
  // on every step (so the grid tracks the cursor smoothly) but debounce the
  // wire dispatch so the remote sees only the settled size and redraws once.
  const RESIZE_DISPATCH_DEBOUNCE_MS = 90;
  let resizeDispatchTimer: number | undefined;

  // `force` bypasses the lastSent dedup. A layout change (maximize/expand,
  // dock switch, Suspense reveal) can settle on a size that an earlier
  // intermediate fit already recorded in `lastSent`, or fire its final
  // ResizeObserver callback after the debounced flush already ran — in both
  // cases the deduped path would never re-send the settled size and the PTY
  // stays at the wrong (smaller) winsize, so fullscreen TUIs render narrow.
  // The authoritative re-sync below always force-sends the settled size.
  function flushResize(force = false) {
    if (disposed) return;
    const cols = term.cols;
    const rows = term.rows;
    if (cols <= 0 || rows <= 0) return;
    if (!force && lastSent && lastSent.cols === cols && lastSent.rows === rows) return;
    if (socket.readyState !== "open") return;
    try {
      socket.send(WS_CONTROL_PREFIX + JSON.stringify({ type: "resize", cols, rows }));
      lastSent = { cols, rows };
    } catch {
      // ignore
    }
  }

  function dispatchResizeIfChanged() {
    if (disposed) return;
    if (typeof window === "undefined") {
      // No event loop to debounce against (tests/SSR) — send synchronously.
      flushResize();
      return;
    }
    if (resizeDispatchTimer !== undefined) window.clearTimeout(resizeDispatchTimer);
    resizeDispatchTimer = window.setTimeout(() => {
      resizeDispatchTimer = undefined;
      flushResize();
    }, RESIZE_DISPATCH_DEBOUNCE_MS);
  }

  // Trailing authoritative re-sync: after the size could have changed, re-fit
  // and force-dispatch once layout has fully settled. The two delays cover (a)
  // an instant relayout (maximize toggles a parent box synchronously) and (b)
  // a slower settle (Suspense reveal, CSS animation, font metrics). This is the
  // guaranteed final dispatch the debounced/deduped path can otherwise drop.
  const authoritativeResyncTimers: number[] = [];
  function scheduleAuthoritativeResync() {
    if (disposed || typeof window === "undefined") return;
    while (authoritativeResyncTimers.length) {
      window.clearTimeout(authoritativeResyncTimers.pop());
    }
    for (const delay of [130, 320]) {
      authoritativeResyncTimers.push(
        window.setTimeout(() => {
          if (disposed) return;
          try {
            fitAddon.fit();
          } catch {
            return;
          }
          flushResize(true);
        }, delay),
      );
    }
  }

  const safeFit = (authoritative = false) => {
    if (disposed) return;
    try {
      fitAddon.fit();
    } catch {
      // jsdom or transiently zero-sized container — ResizeObserver retries.
      return;
    }
    dispatchResizeIfChanged();
    if (authoritative) scheduleAuthoritativeResync();
  };

  // Double-RAF guarantees the browser has done a layout pass + paint after the
  // React commit before we measure. Belt-and-suspenders setTimeout covers
  // slow WS open races.
  safeFit();
  let initialFitTimer: number | undefined;
  if (typeof window !== "undefined") {
    raf1 = window.requestAnimationFrame(() => {
      raf2 = window.requestAnimationFrame(() => {
        if (disposed) return;
        safeFit();
      });
    });
    initialFitTimer = window.setTimeout(safeFit, 100);
  }

  const unsubOpen = socket.addEventListener("open", () => {
    const isReconnect = hasOpenedOnce;
    hasOpenedOnce = true;
    openReportPending = true;
    // Record the first-open timestamp to bound the auth-detection window. A
    // reconnect re-attaches an already-authenticated session, so don't re-arm.
    if (!isReconnect && typeof performance !== "undefined") {
      openedAtMs = performance.now();
    } else if (isReconnect) {
      stopAuthDetect();
    }

    if (isReconnect && reconnecting) {
      // ROBUST-CORE reconnect: the backend replays the FULL history buffer on
      // re-attach. Reset xterm (clear screen + scrollback + cursor) IMMEDIATELY
      // before the bridge writes the replayed bytes that follow this open, so
      // the redraw is clean instead of duplicating the pre-drop scrollback.
      // The very first open must NOT reset — there is nothing to redraw yet.
      try {
        term.reset();
      } catch {
        // ignore — a reset failure must not abort the reconnect.
      }
    }

    reconnecting = false;
    clearSilentTimer();
    setStatus("live");
  });

  const unsubClose = socket.addEventListener("close", (ev) => {
    const close = ev as CloseEvent;
    if (disposed) return;
    // A synthesized auth-failure close already drove status → failed and called
    // onClose; the socket's own close echo must not fire the path a second time.
    if (authClosed) return;
    stopAuthDetect();

    // The sessionSocket emits two kinds of `close`:
    //   1. Per-attempt closes — every underlying socket drop while it still has
    //      retries left. These are NOT terminal; the wrapper reconnects.
    //   2. A single synthetic give-up close at the wall-clock deadline
    //      (reason GIVE_UP_REASON). This IS terminal.
    const isGiveUp = close.reason === RECONNECT_GIVE_UP_REASON;
    openReportPending = false;
    // Backend closes the proxy leg with code 1000 reason "detached_alive" when
    // only the client link dropped but the server-side SSH session is still
    // running — reconnectable, not a teardown.
    // A clean close (code 1000) that is NOT a detached-alive signal is an
    // intentional teardown (dispose / session ended) — surface it immediately,
    // no reconnect dance.
    if (!isReconnectableSshClose(close)) {
      clearSilentTimer();
      setStatus("failed");
      onClose?.(close.code, close.reason);
      return;
    }

    if (isGiveUp) {
      // Reconnect deadline exceeded — give up for good.
      clearSilentTimer();
      setStatus("failed");
      onClose?.(close.code, close.reason);
      return;
    }

    // Per-attempt abnormal close. Enter (or stay in) the reconnect window. Only
    // arm the silent→visible escalation timer once, on the first drop.
    if (!reconnecting) {
      reconnecting = true;
      setStatus("reconnecting-silent");
      // Escalate to the visible (banner + input-blocked) phase if still down
      // after the silent window. The give-up close (above) flips us to failed.
      if (typeof window !== "undefined") {
        silentTimer = window.setTimeout(() => {
          silentTimer = undefined;
          if (disposed || !reconnecting) return;
          setStatus("reconnecting-visible");
        }, SILENT_WINDOW_MS);
      }
    }
    // Do NOT call onClose for per-attempt closes — the session may still
    // recover. onClose only fires on the terminal give-up or a clean 1000.
  });

  // Forward terminal dimension changes to the backend PTY so fullscreen TUI
  // apps (htop, mc, vim) fill the whole terminal. Uses dispatchResizeIfChanged
  // to avoid redundant sends when xterm deduplicates same-size resize calls.
  const unsubResize = term.onResize(() => {
    dispatchResizeIfChanged();
  });

  // Re-fit and force-dispatch initial dims once the socket opens (in case
  // earlier fits ran before the WS was ready). Flush synchronously here so the
  // session starts at the correct size immediately, bypassing the resize
  // debounce that only matters for interactive drag/window resizes.
  const unsubOpenForResize = socket.addEventListener("open", () => {
    window.requestAnimationFrame(() => {
      if (disposed) return;
      try {
        fitAddon.fit();
      } catch {
        return;
      }
      flushResize();
    });
  });

  let resizeObserver: ResizeObserver | null = null;
  if (typeof ResizeObserver !== "undefined") {
    // Container size changes (maximize/expand, dock switch, drag-resize) flow
    // through here. Schedule an authoritative trailing re-sync so the settled
    // size always reaches the PTY even if the debounced dispatch dropped it.
    resizeObserver = new ResizeObserver(() => safeFit(true));
    resizeObserver.observe(container);
  }

  // ResizeObserver covers container size changes, but in some Firefox layouts
  // a window-only resize that doesn't change the container's offsetParent
  // chain bypasses the observer. A window listener guarantees re-fit on every
  // viewport change.
  const onWindowResize = () => safeFit(true);
  if (typeof window !== "undefined") {
    window.addEventListener("resize", onWindowResize);
  }

  return {
    dispose(): void {
      disposed = true;
      if (typeof window !== "undefined") {
        if (raf1 !== undefined) window.cancelAnimationFrame(raf1);
        if (raf2 !== undefined) window.cancelAnimationFrame(raf2);
        if (initialFitTimer !== undefined) window.clearTimeout(initialFitTimer);
        if (resizeDispatchTimer !== undefined) window.clearTimeout(resizeDispatchTimer);
        while (authoritativeResyncTimers.length) {
          window.clearTimeout(authoritativeResyncTimers.pop());
        }
      }
      clearSilentTimer();
      unsubOpen();
      unsubClose();
      unsubResize.dispose();
      unsubOpenForResize();
      resizeObserver?.disconnect();
      resizeObserver = null;
      if (typeof window !== "undefined") {
        window.removeEventListener("resize", onWindowResize);
      }
      terminalBridge.dispose();
      detachTouch();
      try {
        socket.close(1000, "dispose");
      } catch {
        // ignore
      }
      try {
        term.dispose();
      } catch {
        // ignore
      }
    },
    focus(): void {
      term.focus();
    },
    blur(): void {
      // term.blur() clears xterm's helper textarea; the browser then dismisses
      // the OS soft keyboard for whichever field held focus.
      term.blur();
    },
    fit(): void {
      // Use safeFit (not bare fitAddon.fit) so the new cols/rows are also
      // dispatched to the backend PTY. Otherwise fullscreen TUIs (mc, htop)
      // keep their old COLUMNS/LINES after a tab/panel resize and render
      // narrower than the available area. authoritative=true guarantees a
      // final force-dispatch at the settled size, since an explicit fit() is
      // triggered by exactly the layout changes (maximize/expand, tab activate)
      // that the deduped/debounced path is prone to drop.
      safeFit(true);
    },
    isLive(): boolean {
      return !disposed && status === "live" && socket.readyState === "open";
    },
    sendText(data: string): boolean {
      return sendInput(data, softKeyboardEnabled);
    },
    sendKey(key: SshTerminalKey): boolean {
      if (key in SSH_ARROWS) {
        const prefix = term.modes.applicationCursorKeysMode ? "\x1bO" : "\x1b[";
        return sendInput(prefix + SSH_ARROWS[key as keyof typeof SSH_ARROWS], false);
      }
      return sendInput(SSH_KEYS[key as keyof typeof SSH_KEYS], false);
    },
    getFontSize(): number {
      return term.options.fontSize ?? SSH_FONT_SIZE_DEFAULT;
    },
    setFontSize(size: number): number {
      const clamped = Math.min(SSH_FONT_SIZE_MAX, Math.max(SSH_FONT_SIZE_MIN, Math.round(size)));
      if (term.options.fontSize !== clamped) {
        term.options.fontSize = clamped;
        // Font metrics change the cell size, so the grid must be re-measured and
        // the new cols/rows re-sent to the PTY. safeFit(true) keeps the visible
        // scrollback and the live socket — only the geometry is recomputed.
        safeFit(true);
      }
      return clamped;
    },
    setSoftKeyboardEnabled(enabled: boolean): void {
      if (softKeyboardEnabled === enabled) return;
      softKeyboardEnabled = enabled;
      const textarea = term.textarea;
      if (!textarea) return;
      if (enabled) {
        textarea.inputMode = "text";
        return;
      }
      // inputMode="none" suppresses the virtual keyboard while still allowing
      // programmatic focus (used only to read the caret); blur if it currently
      // holds focus so an already-open keyboard closes.
      textarea.inputMode = "none";
      if (typeof document !== "undefined" && document.activeElement === textarea) {
        textarea.blur();
      }
    },
  };
}

/**
 * Returns true if a WS message payload is a backend control frame
 * (`__piping_ws__:` prefix). Only string/text frames can be control frames;
 * binary frames are always raw PTY bytes.
 */
function isControlFrameData(data: unknown, controlPrefix: string): boolean {
  return typeof data === "string" && data.startsWith(controlPrefix);
}

function controlFrameType(data: unknown, controlPrefix: string): string | null {
  if (!isControlFrameData(data, controlPrefix)) return null;
  const payload = (data as string).slice(controlPrefix.length);
  try {
    const parsed = JSON.parse(payload) as { type?: unknown };
    return typeof parsed.type === "string" ? parsed.type : null;
  } catch {
    return null;
  }
}

/**
 * Persistent xterm bridge for a reconnecting SessionSocket.
 *
 * This intentionally does not listen to `close`/`error` for cleanup. Those
 * events are per-attempt lifecycle noise while auto-reconnect is active; the
 * bridge must survive them and only be disposed with the owning adapter.
 *
 * Inbound control frames (`controlPrefix`-prefixed text) are dropped so backend
 * control messages can never corrupt the visible terminal screen.
 */
function attachTerminalBridge(
  term: Terminal,
  sock: SessionSocket,
  controlPrefix: string,
  hooks: {
    onReady?: () => void;
    /** Inbound terminal text (decoded), excluding backend control frames. */
    onInboundText?: (text: string) => void;
    /** Return false to skip inbound byte decoding for optional inspection. */
    shouldInspectInbound?: () => boolean;
    /** The user typed into the terminal (first keystroke gates auth detection). */
    onUserInput?: () => void;
  } = {},
): { dispose(): void } {
  let disposed = false;
  const disposables: Array<() => void> = [];
  const decoder = typeof TextDecoder !== "undefined" ? new TextDecoder() : null;

  const shouldReportInbound = (): boolean =>
    Boolean(hooks.onInboundText && (!hooks.shouldInspectInbound || hooks.shouldInspectInbound()));

  const reportInbound = (bytes: Uint8Array): void => {
    if (!shouldReportInbound() || !decoder) return;
    try {
      hooks.onInboundText?.(decoder.decode(bytes));
    } catch {
      // A decode failure must never break terminal rendering.
    }
  };

  const writePayload = (data: unknown): void => {
    if (disposed) return;

    if (isControlFrameData(data, controlPrefix)) {
      if (controlFrameType(data, controlPrefix) === "ready") {
        hooks.onReady?.();
      }
      return;
    }

    hooks.onReady?.();

    if (typeof data === "string") {
      term.write(data);
      if (shouldReportInbound()) hooks.onInboundText?.(data);
      return;
    }

    if (data instanceof ArrayBuffer) {
      const bytes = new Uint8Array(data);
      term.write(bytes);
      reportInbound(bytes);
      return;
    }

    if (ArrayBuffer.isView(data)) {
      const view = data;
      const bytes = new Uint8Array(view.byteLength);
      bytes.set(new Uint8Array(view.buffer, view.byteOffset, view.byteLength));
      term.write(bytes);
      reportInbound(bytes);
      return;
    }

    if (data instanceof Blob) {
      void data.arrayBuffer()
        .then((buffer) => {
          if (disposed) return;
          const bytes = new Uint8Array(buffer);
          term.write(bytes);
          reportInbound(bytes);
        })
        .catch(() => {
          // Ignore malformed browser payloads; the next frame can still render.
        });
    }
  };

  const sendIfOpen = (data: string | ArrayBufferLike | Blob | ArrayBufferView): void => {
    if (disposed || sock.readyState !== "open") return;
    try {
      sock.send(data);
    } catch {
      // The socket can close between readyState and send(); reconnect will
      // re-establish the stream, but this keystroke must not poison xterm.
    }
  };

  disposables.push(sock.addEventListener("message", (ev) => {
    writePayload((ev as MessageEvent).data);
  }));

  const dataDisposable = term.onData((data) => {
    hooks.onUserInput?.();
    sendIfOpen(data);
  });
  disposables.push(() => dataDisposable.dispose());

  const binaryDisposable = term.onBinary((data) => {
    hooks.onUserInput?.();
    const bytes = new Uint8Array(data.length);
    for (let i = 0; i < data.length; i += 1) {
      bytes[i] = data.charCodeAt(i) & 255;
    }
    sendIfOpen(bytes);
  });
  disposables.push(() => binaryDisposable.dispose());

  return {
    dispose(): void {
      if (disposed) return;
      disposed = true;
      while (disposables.length) {
        try {
          disposables.pop()?.();
        } catch {
          // ignore cleanup errors
        }
      }
    },
  };
}
