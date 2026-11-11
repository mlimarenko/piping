/**
 * Demo-only SSH WebSocket shim.
 *
 * MSW's browser WebSocket interception does not reliably patch the global
 * `WebSocket` under this Vite + msw build, so instead we install a tiny
 * demo-only wrapper around `window.WebSocket`. For the demo SSH relay URL
 * (`wss://demo.invalid/relay/ssh/{host}…`) it returns a fake socket that streams
 * a canned shell transcript into xterm (via the adapter's terminal bridge); every
 * other URL is delegated to the real native `WebSocket` untouched.
 *
 * Installed once, before the app renders, only from the demo entry
 * (`demo-main.tsx`). The normal product build never imports this module, so the
 * real `WebSocket` is left completely intact in production.
 */
import { ansi, sshScriptForHost, type SshStep } from "./ssh-script";

const DEMO_SSH_MARKER = "/relay/ssh/";

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Minimal fake matching the WebSocket surface the SSH adapter consumes. */
class DemoSshSocket extends EventTarget {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;

  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSING = 2;
  readonly CLOSED = 3;

  readyState = 0;
  binaryType: BinaryType = "blob";
  readonly url: string;
  readonly protocol: string;

  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;

  #closed = false;

  constructor(url: string, protocols?: string | string[]) {
    super();
    this.url = url;
    this.protocol = Array.isArray(protocols) ? (protocols[0] ?? "") : (protocols ?? "");
    // Open on the next tick so listeners attached synchronously after
    // construction still receive the `open` event.
    queueMicrotask(() => this.#open());
  }

  #emit(type: "open" | "close" | "error", init?: EventInit | CloseEventInit): void {
    const ev =
      type === "close"
        ? new CloseEvent("close", { code: 1000, wasClean: true, ...(init as CloseEventInit) })
        : new Event(type, init);
    this.dispatchEvent(ev);
    const handler =
      type === "open" ? this.onopen : type === "close" ? this.onclose : this.onerror;
    handler?.call(this, ev as never);
  }

  #message(data: string): void {
    const ev = new MessageEvent("message", { data });
    this.dispatchEvent(ev);
    this.onmessage?.call(this, ev);
  }

  #open(): void {
    if (this.#closed) return;
    this.readyState = this.OPEN;
    this.#emit("open");
    void this.#stream();
  }

  async #stream(): Promise<void> {
    const hostName = decodeURIComponent(this.url.split(DEMO_SSH_MARKER)[1]?.split(/[?/]/)[0] ?? "linux-host");
    const script = sshScriptForHost(hostName);
    await sleep(200);
    for (const step of script.steps) {
      if (this.#closed) return;
      await this.#playStep(script.prompt, step);
    }
  }

  async #playStep(prompt: string, step: SshStep): Promise<void> {
    switch (step.kind) {
      case "out":
        if (step.delayMs) await sleep(step.delayMs);
        if (!this.#closed) this.#message(step.text);
        return;
      case "prompt":
        if (step.delayMs) await sleep(step.delayMs);
        if (!this.#closed) this.#message(prompt);
        return;
      case "type":
        for (const ch of step.text) {
          if (this.#closed) return;
          this.#message(ansi.bold(ansi.white(ch)));
          await sleep(step.delayMs ?? 45);
        }
        return;
      case "enter":
        if (!this.#closed) this.#message("\r\n");
        return;
    }
  }

  // The demo shell is non-interactive — swallow client keystrokes/control msgs.
  send(_data: unknown): void {}

  close(code?: number, reason?: string): void {
    if (this.#closed) return;
    this.#closed = true;
    this.readyState = this.CLOSED;
    this.#emit("close", { code: code ?? 1000, reason, wasClean: true });
  }

  // Unused by the adapter but part of the WebSocket surface.
  get bufferedAmount(): number {
    return 0;
  }
  get extensions(): string {
    return "";
  }
}

/**
 * Replace `window.WebSocket` with a router that returns {@link DemoSshSocket}
 * for the demo SSH relay URL and the native socket for everything else.
 */
export function installDemoSshSocket(): void {
  if (typeof window === "undefined") return;
  const NativeWebSocket = window.WebSocket;

  function DemoWebSocket(this: unknown, url: string | URL, protocols?: string | string[]) {
    const href = typeof url === "string" ? url : url.toString();
    if (href.includes(DEMO_SSH_MARKER)) {
      return new DemoSshSocket(href, protocols) as unknown as WebSocket;
    }
    return new NativeWebSocket(url, protocols);
  }

  // Preserve the constants/prototype callers may read off the constructor.
  DemoWebSocket.prototype = NativeWebSocket.prototype;
  (DemoWebSocket as unknown as { CONNECTING: number }).CONNECTING = NativeWebSocket.CONNECTING;
  (DemoWebSocket as unknown as { OPEN: number }).OPEN = NativeWebSocket.OPEN;
  (DemoWebSocket as unknown as { CLOSING: number }).CLOSING = NativeWebSocket.CLOSING;
  (DemoWebSocket as unknown as { CLOSED: number }).CLOSED = NativeWebSocket.CLOSED;

  window.WebSocket = DemoWebSocket as unknown as typeof WebSocket;
}
