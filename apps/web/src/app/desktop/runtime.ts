export const DESKTOP_SHELL_STORAGE_KEY = "piping.desktop.shell";
export const DESKTOP_LANGUAGE_STORAGE_KEY = "piping.desktop.language";
export const DESKTOP_SHELL_READY_EVENT = "piping:desktop-shell-ready";

export type TauriInvoke = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<unknown>;

type TauriInternals = {
  invoke?: TauriInvoke;
};

declare global {
  interface Window {
    __PIPING_DESKTOP__?: boolean;
    __PIPING_NATIVE_TITLEBAR__?: boolean;
    __TAURI_INTERNALS__?: TauriInternals;
    __setDesktopLanguage?: (language: string) => void;
    __TAURI__?: {
      core?: {
        invoke?: TauriInvoke;
      };
      window?: {
        getCurrentWindow?: () => {
          startDragging?: () => Promise<void>;
        };
      };
    };
  }
}

export function getTauriInvoke(): TauriInvoke | null {
  if (typeof window === "undefined") return null;
  const tauriInvoke = window.__TAURI__?.core?.invoke;
  if (typeof tauriInvoke === "function") return tauriInvoke;
  const internalInvoke = window.__TAURI_INTERNALS__?.invoke;
  return typeof internalInvoke === "function" ? internalInvoke : null;
}

export function isPipingDesktopShell(
  options: { allowStoredHint?: boolean } = {},
): boolean {
  if (typeof window === "undefined") return false;
  if (window.__PIPING_DESKTOP__ === true) return true;
  if (getTauriInvoke()) return true;

  if (!options.allowStoredHint) return false;
  try {
    return window.localStorage.getItem(DESKTOP_SHELL_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function invokeDesktopCommand(
  command: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  const invoke = getTauriInvoke();
  if (!invoke) {
    return Promise.reject(new Error("Piping Desktop runtime is unavailable."));
  }
  return invoke(command, args);
}

export function openExternalUrl(url: string): Promise<unknown> {
  return invokeDesktopCommand("open_external_url", { url });
}
