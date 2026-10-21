declare module "@novnc/novnc" {
  export default class RFB {
    constructor(target: HTMLElement, url: string, opts?: Record<string, unknown>);
    scaleViewport: boolean;
    resizeSession: boolean;
    viewOnly: boolean;
    clipboardPasteFrom(text: string): void;
    focus(options?: FocusOptions): void;
    sendKey(keysym: number, code: string, down?: boolean): void;
    disconnect(): void;
    addEventListener(type: string, listener: EventListener): void;
    removeEventListener(type: string, listener: EventListener): void;
    sendCredentials(creds: { password?: string; username?: string }): void;
  }
}
