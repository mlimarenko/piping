import type { ElementType } from "react";
import type { TFunction } from "i18next";
import { Globe, Monitor, ScreenShare, Terminal, Video } from "lucide-react";

import type {
  ConnectionLaunchAccount,
  ConnectionLaunchProtocol,
  ConnectionTarget,
} from "@/shared/api/types";
import { notify } from "@/shared/lib/notifications";
import type { AdHocCredentialsContext } from "./AdHocCredentialsDialog";

export type SessionLaunchProtocol = "ssh" | "rdp" | "vnc";
/** A protocol whose launch opens a workspace tab (session or console). */
export type OpenableProtocol = SessionLaunchProtocol | "cctv";
export type LaunchProtocol = SessionLaunchProtocol | "app" | "cctv";

export const LAUNCH_PROTOCOL_ICON: Record<LaunchProtocol, ElementType> = {
  ssh: Terminal,
  rdp: Monitor,
  vnc: ScreenShare,
  app: Globe,
  cctv: Video,
};

export const LAUNCH_PROTOCOL_COLOR: Record<LaunchProtocol, string> = {
  ssh: "text-status-positive",
  rdp: "text-status-info",
  vnc: "text-status-warning",
  app: "text-primary",
  cctv: "text-status-video",
};

export const LAUNCH_PROTOCOL_LABEL: Record<LaunchProtocol, string> = {
  ssh: "SSH",
  rdp: "RDP",
  vnc: "VNC",
  app: "APP",
  cctv: "CCTV",
};

const URI_SCHEME_RE = /^([a-z][a-z0-9+.-]*):/i;
const WINDOWS_DRIVE_PATH_RE = /^[a-z]:[\\/]/i;
const BROWSER_TAB_URI_SCHEMES = new Set(["http", "https"]);
const IN_PAGE_URI_SCHEMES = new Set([
  "about",
  "blob",
  "data",
  "intent",
  "javascript",
  "vbscript",
]);
const WRAPPED_WEB_LAUNCHER_URI_SCHEMES = new Set(["microsoft-edge", "microsoft-edge-holographic"]);
const STRICT_URI_SCHEMES = new Set([
  ...BROWSER_TAB_URI_SCHEMES,
  ...WRAPPED_WEB_LAUNCHER_URI_SCHEMES,
  "file",
  "mailto",
]);

type TauriCore = {
  invoke?: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
};

export type AppLaunchResult =
  | {
      ok: true;
      mode: "browser-tab" | "browser-uri" | "desktop";
    }
  | {
      ok: false;
      reason:
        | "invalid-target"
        | "popup-blocked"
        | "desktop-required"
        | "desktop-open-failed"
        | "cancelled";
      message?: string;
    };

type AppLauncherTargetKind = "uri" | "local-command";

function isSafeHttpUrl(raw: string): boolean {
  try {
    const url = new URL(raw);
    return (url.protocol === "http:" || url.protocol === "https:")
      && Boolean(url.hostname)
      && !url.username
      && !url.password;
  } catch {
    return false;
  }
}

function hasValidPercentEncoding(raw: string): boolean {
  for (let index = raw.indexOf("%"); index !== -1; index = raw.indexOf("%", index + 1)) {
    if (!/^[0-9a-f]{2}$/i.test(raw.slice(index + 1, index + 3))) return false;
  }
  return true;
}

function hasControlCharacter(raw: string): boolean {
  for (let index = 0; index < raw.length; index += 1) {
    const code = raw.charCodeAt(index);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}

function hasUriUnsafeCharacter(raw: string): boolean {
  for (const character of raw) {
    if (character === "\"" || character.trim() === "" || hasControlCharacter(character)) {
      return true;
    }
  }
  return false;
}

function appUriScheme(raw: string): string | undefined {
  if (WINDOWS_DRIVE_PATH_RE.test(raw)) return undefined;
  return raw.match(URI_SCHEME_RE)?.[1]?.toLowerCase();
}

export function isSafeConnectionAppUri(raw: string): boolean {
  const target = raw.trim();
  if (!target || hasUriUnsafeCharacter(target)) return false;
  if (!hasValidPercentEncoding(target)) return false;

  const scheme = appUriScheme(target);
  if (!scheme) return false;
  if (IN_PAGE_URI_SCHEMES.has(scheme)) return false;

  if (BROWSER_TAB_URI_SCHEMES.has(scheme)) {
    return isSafeHttpUrl(target);
  }

  if (WRAPPED_WEB_LAUNCHER_URI_SCHEMES.has(scheme)) {
    return isSafeHttpUrl(target.slice(scheme.length + 1));
  }

  return true;
}

function appLauncherTargetKind(raw: string): AppLauncherTargetKind | undefined {
  const target = raw.trim();
  if (!target || hasControlCharacter(target)) return undefined;

  const scheme = appUriScheme(target);
  if (!scheme) return "local-command";
  if (IN_PAGE_URI_SCHEMES.has(scheme)) return undefined;
  if (STRICT_URI_SCHEMES.has(scheme) || !/[\s"]/.test(target)) {
    return isSafeConnectionAppUri(target) ? "uri" : undefined;
  }

  return "local-command";
}

export function isSafeConnectionAppLauncherTarget(raw: string): boolean {
  return appLauncherTargetKind(raw) !== undefined;
}

// Best-effort launch of a non-http(s) URI scheme (custom OS protocol handlers
// such as `microsoft-edge:`, `iexplore:`, vendor clients for cameras/DVRs, etc.)
// from a plain browser. A transient `_blank` navigation is the browser-blessed
// path that actually dispatches a registered handler — a hidden iframe is
// refused for external protocols by modern Chromium, so it would silently fail
// to launch even where the handler exists. For a registered scheme the throwaway
// tab hands off to the OS and closes; an unregistered scheme surfaces the
// browser's "protocol is unknown" notice in THAT tab instead of over the app.
// The return value is unreliable for protocol dispatch (and null under
// `noopener`), so this is fire-and-forget.
function openViaExternalProtocolTab(target: string): void {
  window.open(target, "_blank", "noopener,noreferrer");
}

function errorMessage(error: unknown): string | undefined {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return undefined;
}

export async function openConnectionAppUri(uri: string): Promise<AppLaunchResult> {
  const target = uri.trim();
  const targetKind = appLauncherTargetKind(target);
  if (!targetKind) {
    return { ok: false, reason: "invalid-target" };
  }
  if (targetKind === "uri" && /^https?:\/\//i.test(target)) {
    const opened = window.open(target, "_blank", "noopener,noreferrer");
    return opened === null
      ? { ok: false, reason: "popup-blocked" }
      : { ok: true, mode: "browser-tab" };
  }

  const invoke = (window as Window & { __TAURI__?: { core?: TauriCore } }).__TAURI__?.core
    ?.invoke;
  if (invoke) {
    try {
      await invoke("open_app_uri", { uri: target });
      return { ok: true, mode: "desktop" };
    } catch (error) {
      const message = errorMessage(error);
      if (message?.toLowerCase().includes("cancelled")) {
        return { ok: false, reason: "cancelled", message };
      }
      return {
        ok: false,
        reason: "desktop-open-failed",
        message,
      };
    }
  }

  if (targetKind === "local-command") {
    return { ok: false, reason: "desktop-required" };
  }

  openViaExternalProtocolTab(target);
  return { ok: true, mode: "browser-uri" };
}

export function notifyAppLaunchResult(
  result: AppLaunchResult | void,
  t: TFunction,
): void {
  if (!result) return;
  if (result.ok) {
    if (result.mode === "browser-uri" || result.mode === "desktop") {
      notify("info", t("connections.appLaunch.requested"), {
        description: result.mode === "desktop"
          ? t("connections.appLaunch.desktopDescription")
          : t("connections.appLaunch.browserUriDescription"),
        source: "connection",
      });
    }
    return;
  }

  if (result.reason === "popup-blocked") {
    notify("warning", t("connections.appLaunch.popupBlocked"), {
      description: t("connections.popupBlocked"),
      source: "connection",
    });
    return;
  }

  if (result.reason === "desktop-required") {
    notify("warning", t("connections.appLaunch.desktopRequired"), {
      description: t("connections.appLaunch.desktopRequiredDescription"),
      source: "connection",
    });
    return;
  }

  if (result.reason === "cancelled") return;

  if (result.reason === "invalid-target") {
    notify("error", t("connections.appLaunch.invalidTarget"), {
      description: t("connections.appLaunch.invalidTargetDescription"),
      source: "connection",
    });
    return;
  }

  notify("error", t("connections.appLaunch.failed"), {
    description: result.message || t("connections.appLaunch.failedDescription"),
    source: "connection",
  });
}

const isAutoLabel = (label: string): boolean =>
  /\bconnections\s*\[/i.test(label) || /^role\s/i.test(label);

export function connectionAccountText(
  account: ConnectionLaunchAccount,
  protocol: string,
): string {
  if (account.label && !isAutoLabel(account.label)) return account.label;
  const username = account.username?.trim();
  if (username) return username;
  return LAUNCH_PROTOCOL_LABEL[protocol as LaunchProtocol] ?? protocol.toUpperCase();
}

export function connectionAccountTitle(account: ConnectionLaunchAccount): string {
  if (account.uri) return `${account.label} (${account.uri})`;
  if (account.username) return `${account.label} (${account.username})`;
  return account.label;
}

export function uniqueLaunchAccounts(
  proto: ConnectionLaunchProtocol,
): ConnectionLaunchAccount[] {
  const isApp = proto.protocol === "app";
  const seen = new Set<string>();

  return proto.accounts.filter((account) => {
    if (isApp && !account.uri?.trim()) return false;
    const key = isApp
      ? `uri:${account.uri?.trim().toLowerCase()}`
      : account.username?.trim()
        ? `u:${account.username.trim().toLowerCase()}`
        : `e:${account.entryId}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function preferredPersonalAccount(accounts: ConnectionLaunchAccount[]): ConnectionLaunchAccount | undefined {
  return accounts.find((account) => account.kind === "personal" && account.isDefault)
    ?? accounts.find((account) => account.kind === "personal");
}

function presentRdpAccounts(accounts: ConnectionLaunchAccount[]): ConnectionLaunchAccount[] {
  const sharedAccounts = accounts.filter((account) => account.kind === "shared");
  if (sharedAccounts.length === 0) return [];

  const personal = preferredPersonalAccount(accounts);
  if (!personal) return sharedAccounts;

  let usedPersonalOverlay = false;

  // RDP availability belongs to connection-config; personal records are only
  // credential overlays. A launch-ready shared account is authoritative; only
  // a shared action that still requires credentials is backed by the selected
  // personal entry id.
  return sharedAccounts.map((account) => {
    if (!account.requiresInput || usedPersonalOverlay) return account;
    usedPersonalOverlay = true;

    return {
      ...account,
      entryId: personal.entryId,
      username: personal.username ?? account.username,
      requiresInput: personal.requiresInput,
      isDefault: personal.isDefault || account.isDefault,
    };
  });
}

export function presentLaunchAccounts(
  proto: ConnectionLaunchProtocol,
): ConnectionLaunchAccount[] {
  const accounts = proto.protocol === "rdp"
    ? presentRdpAccounts(proto.accounts)
    : proto.accounts;
  return uniqueLaunchAccounts({ ...proto, accounts });
}

export function sessionAccountInput(
  host: { id: string; name: string },
  protocol: LaunchProtocol,
  account: ConnectionLaunchAccount,
): AdHocCredentialsContext | null {
  if ((protocol !== "ssh" && protocol !== "rdp" && protocol !== "vnc") || !account.requiresInput) return null;
  return { protocol, host, account: { entryId: account.entryId, name: connectionAccountText(account, protocol), username: account.username ?? undefined } };
}

export function launchConnectionAccount(
  host: ConnectionTarget,
  protocol: LaunchProtocol,
  account: ConnectionLaunchAccount,
  onConnect: (
    host: ConnectionTarget,
    protocol: OpenableProtocol,
    entryId?: string,
    username?: string,
  ) => void,
): Promise<AppLaunchResult | void> {
  if (protocol === "app") {
    return account.uri ? openConnectionAppUri(account.uri) : Promise.resolve();
  }
  // `cctv` opens the device-management console tab; ssh/rdp/vnc open a proxied
  // session. Both go through `onConnect` (the workspace opener), which branches
  // on the protocol — the CCTV path mints a client-only tab with no websocket.
  // Forward the account's remote login so it can surface in the detached-session
  // tab title (`host · PROTOCOL · user`). Only account-picker launches carry it;
  // the plain connect button passes no account, so the title degrades to
  // `host · PROTOCOL` there ("user only if applicable").
  onConnect(host, protocol, account.entryId, account.username ?? undefined);
  return Promise.resolve();
}
