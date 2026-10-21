/**
 * Thin, reconnecting wrapper over the browser `WebSocket` used by every
 * live-session adapter (SSH/RDP/VNC). Exposes an EventTarget-like API on top
 * of the underlying socket so callers can swap attempts transparently.
 *
 * Wire facts:
 *   - Backend asks for a specific `Sec-WebSocket-Protocol` per session type
 *     (SSH and RDP → "guacamole", VNC → "binary" or "base64"). The protocol
 *     is negotiated on the very first upgrade and is reused on every
 *     reconnect attempt.
 *   - Reconnect uses exponential backoff (base 500ms, doubled each attempt).
 *     `maxAttempts` bounds how many reconnects are tried before the socket
 *     is declared closed for good.
 *   - Passing an `AbortSignal` guarantees that the socket is torn down and
 *     any scheduled reconnect is cancelled synchronously.
 */

export type SessionSocketReadyState = "connecting" | "open" | "closing" | "closed";

export type SessionSocketEventType = "open" | "message" | "close" | "error" | "reconnect";

export type SessionSocketEventListener = (event: Event | MessageEvent | CloseEvent) => void;

export interface ReconnectOptions {
  /** Maximum number of reconnect attempts after the initial connect. */
  maxAttempts?: number;
  /** Base delay (ms) for exponential backoff; attempt N waits base * 2^(N-1). */
  baseDelayMs?: number;
  /** Upper bound (ms) for a single backoff wait. */
  maxDelayMs?: number;
  /**
   * Wall-clock budget (ms) for reconnecting, measured from the first drop after
   * the last successful open. Once this deadline passes we stop scheduling
   * retries and emit a terminal `close` (so callers can "give up after N s"
   * cleanly rather than counting attempts). A successful `open` resets the
   * window. Unset (default) means no wall-clock limit — only `maxAttempts`.
   */
  deadlineMs?: number;
}

export interface SessionSocketOptions {
  /** Absolute `wss://` URL including any query params (e.g. `?token=...`). */
  url: string;
  /** Sec-WebSocket-Protocol values to negotiate. */
  protocols?: string[];
  /** Auto-reconnect policy. Defaults: base 500ms, up to 8 attempts, capped at 30s. */
  reconnect?: ReconnectOptions;
  /**
   * Optional policy hook for protocol-level close semantics. Return false when
   * a CloseEvent is terminal for the session and auto-reconnect must stop.
   */
  shouldReconnect?: (event: CloseEvent) => boolean;
  /** Tearing-down signal; on abort the socket is closed and reconnect is cancelled. */
  signal?: AbortSignal;
}

export interface SessionSocket {
  /**
   * Write to the underlying socket. If the socket is not OPEN this throws
   * — callers must wait for the `open` event before sending. We deliberately
   * don't buffer frames because the Guacamole and xterm handshakes are
   * ordering-sensitive and silently queueing would desynchronize them.
   */
  send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void;
  /** Close the socket and abort any pending reconnect. */
  close(code?: number, reason?: string): void;
  readonly readyState: SessionSocketReadyState;
  /** Subscribe to lifecycle events; returns an unsubscribe function. */
  addEventListener(event: SessionSocketEventType, cb: SessionSocketEventListener): () => void;
}

/** `WebSocket.send` rejects `SharedArrayBuffer`; normalize views backed by one. */
function webSocketSendPayload(data: string | ArrayBufferLike | Blob | ArrayBufferView): string | Blob | BufferSource {
  if (typeof data === "string" || data instanceof Blob) {
    return data;
  }
  if (data instanceof ArrayBuffer) {
    return data;
  }
  if (data instanceof SharedArrayBuffer) {
    const copy = new Uint8Array(data.byteLength);
    copy.set(new Uint8Array(data));
    return copy.buffer;
  }
  const view = data;
  if (view.buffer instanceof SharedArrayBuffer) {
    const copy = new Uint8Array(view.byteLength);
    copy.set(new Uint8Array(view.buffer, view.byteOffset, view.byteLength));
    return copy.buffer;
  }
  return view as BufferSource;
}

const DEFAULT_MAX_ATTEMPTS = 8;
const DEFAULT_BASE_DELAY_MS = 500;
const DEFAULT_MAX_DELAY_MS = 30_000;

/**
 * `reason` carried by the synthetic terminal `close` emitted when the
 * wall-clock `deadlineMs` is exceeded. Callers can match on this to tell a
 * permanent give-up apart from an ordinary per-attempt close.
 */
export const RECONNECT_GIVE_UP_REASON = "reconnect deadline exceeded";

/** Computes `base * 2^(attempt-1)` clamped to `[0, maxDelay]`. */
export function computeReconnectDelayMs(
  attempt: number,
  baseDelayMs: number,
  maxDelayMs: number,
): number {
  const exponent = Math.max(0, attempt - 1);
  const raw = baseDelayMs * Math.pow(2, exponent);
  return Math.min(Math.max(0, raw), maxDelayMs);
}

export function openSessionSocket(opts: SessionSocketOptions): SessionSocket {
  const {
    url,
    protocols,
    reconnect: reconnectOpts = {},
    shouldReconnect,
    signal,
  } = opts;
  const baseDelayMs = reconnectOpts.baseDelayMs ?? DEFAULT_BASE_DELAY_MS;
  const maxDelayMs = reconnectOpts.maxDelayMs ?? DEFAULT_MAX_DELAY_MS;
  const deadlineMs = reconnectOpts.deadlineMs;
  // When a wall-clock `deadlineMs` governs reconnection, the attempt count must
  // not cut it short — default to unbounded attempts so the deadline alone
  // decides when to give up. An explicit `maxAttempts` still wins if provided.
  const maxAttempts =
    reconnectOpts.maxAttempts ??
    (deadlineMs !== undefined ? Number.POSITIVE_INFINITY : DEFAULT_MAX_ATTEMPTS);

  const listeners: { [K in SessionSocketEventType]: Set<SessionSocketEventListener> } = {
    open: new Set(),
    message: new Set(),
    close: new Set(),
    error: new Set(),
    reconnect: new Set(),
  };

  let socket: WebSocket | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let attempts = 0;
  let disposed = false;
  let explicitClose = false;
  // Wall-clock timestamp (ms) of the first drop in the current reconnect window.
  // Reset to null on a successful open so each fresh outage gets the full budget.
  let reconnectWindowStartedAt: number | null = null;
  // Latched once the wall-clock deadline fires so we emit the terminal `close`
  // exactly once and never re-arm retries afterwards.
  let gaveUp = false;

  function emit(type: SessionSocketEventType, event: Event | MessageEvent | CloseEvent): void {
    for (const cb of listeners[type]) {
      try {
        cb(event);
      } catch (err) {
        // Never let a listener throw poison the dispatch loop.
        // Errors in user callbacks are their own problem.
        console.error("[sessionSocket] listener threw for", type, err);
      }
    }
  }

  function clearReconnectTimer(): void {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
  }

  /**
   * Emits a synthetic terminal `close` once, signalling that auto-reconnect has
   * permanently given up (wall-clock deadline hit). Carries WS code 1006 so
   * callers treat it as an abnormal/failed close. Subsequent calls are no-ops.
   */
  function emitGiveUp(reason: string): void {
    if (gaveUp) return;
    gaveUp = true;
    emit("close", new CloseEvent("close", { code: 1006, reason }));
  }

  function scheduleReconnect(): void {
    if (disposed || explicitClose) return;
    // Start (or keep) the wall-clock reconnect window at the first drop.
    if (reconnectWindowStartedAt === null) {
      reconnectWindowStartedAt = Date.now();
    }
    // Wall-clock budget: once exceeded, stop retrying and give up terminally.
    if (deadlineMs !== undefined && Date.now() - reconnectWindowStartedAt >= deadlineMs) {
      emitGiveUp(RECONNECT_GIVE_UP_REASON);
      return;
    }
    attempts += 1;
    if (attempts > maxAttempts) return;
    const delay = computeReconnectDelayMs(attempts, baseDelayMs, maxDelayMs);
    // Clamp the wait so we never schedule a retry that would wake up past the
    // deadline — fire one last attempt exactly at the deadline instead.
    let waitMs = delay;
    if (deadlineMs !== undefined) {
      const remaining = deadlineMs - (Date.now() - reconnectWindowStartedAt);
      if (remaining <= 0) {
        emitGiveUp(RECONNECT_GIVE_UP_REASON);
        return;
      }
      waitMs = Math.min(delay, remaining);
    }
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      if (disposed || explicitClose) return;
      emit("reconnect", new Event("reconnect"));
      connect();
    }, waitMs);
  }

  function connect(): void {
    if (disposed || explicitClose) return;
    const next = new WebSocket(url, protocols);
    next.binaryType = "arraybuffer";
    socket = next;

    next.addEventListener("open", (ev) => {
      // A successful connect resets the attempt counter so a later drop
      // restarts backoff from 500ms rather than 30s, and resets the wall-clock
      // reconnect window so each fresh outage gets the full deadline budget.
      attempts = 0;
      reconnectWindowStartedAt = null;
      emit("open", ev);
    });
    next.addEventListener("message", (ev) => {
      emit("message", ev);
    });
    next.addEventListener("error", (ev) => {
      emit("error", ev);
    });
    next.addEventListener("close", (ev) => {
      emit("close", ev);
      if (socket === next) socket = null;
      if (shouldReconnect && !shouldReconnect(ev)) return;
      scheduleReconnect();
    });
  }

  function derivedReadyState(): SessionSocketReadyState {
    if (disposed || explicitClose) return "closed";
    if (socket === null) {
      // Either waiting for the next reconnect tick, or exhausted.
      return reconnectTimer !== null ? "connecting" : "closed";
    }
    switch (socket.readyState) {
      case WebSocket.CONNECTING:
        return "connecting";
      case WebSocket.OPEN:
        return "open";
      case WebSocket.CLOSING:
        return "closing";
      default:
        return "closed";
    }
  }

  function disposeInternal(code?: number, reason?: string): void {
    if (disposed) return;
    disposed = true;
    explicitClose = true;
    clearReconnectTimer();
    if (socket !== null) {
      try {
        socket.close(code, reason);
      } catch {
        // ignore: some browsers throw when close() is called on a
        // socket that is already in CLOSING/CLOSED.
      }
      socket = null;
    }
  }

  const api: SessionSocket = {
    send(data) {
      if (socket === null || socket.readyState !== WebSocket.OPEN) {
        throw new Error(
          `[sessionSocket] cannot send while readyState=${derivedReadyState()}`,
        );
      }
      socket.send(webSocketSendPayload(data));
    },
    close(code, reason) {
      disposeInternal(code, reason);
    },
    get readyState() {
      return derivedReadyState();
    },
    addEventListener(event, cb) {
      listeners[event].add(cb);
      return () => {
        listeners[event].delete(cb);
      };
    },
  };

  // If the caller already aborted before we got here, don't open at all.
  if (signal?.aborted) {
    disposed = true;
    explicitClose = true;
    return api;
  }

  signal?.addEventListener(
    "abort",
    () => {
      disposeInternal(1000, "aborted");
    },
    { once: true },
  );

  connect();

  return api;
}
