import { afterEach, describe, expect, it, vi } from "vitest";

import {
  isSafeConnectionAppLauncherTarget,
  isSafeConnectionAppUri,
  launchConnectionAccount,
  openConnectionAppUri,
} from "./launchPresentation";
import type { ConnectionLaunchAccount, ConnectionTarget } from "@/shared/api/types";

describe("launchConnectionAccount", () => {
  const host = { id: "t1", name: "nvr-1" } as ConnectionTarget;
  const account = {
    entryId: "config:connconf:cctv:0",
    label: "Lab DVRIP",
    kind: "shared",
    username: "admin",
    requiresInput: false,
  } as unknown as ConnectionLaunchAccount;

  it("routes a cctv launch to onConnect with the cctv console protocol", () => {
    // Regression guard for the video->ssh coercion: a cctv (formerly video)
    // account must open the CCTV console — never a phantom SSH session.
    const onConnect = vi.fn();
    void launchConnectionAccount(host, "cctv", account, onConnect);
    expect(onConnect).toHaveBeenCalledWith(host, "cctv", "config:connconf:cctv:0", "admin");
  });

  it("does not call onConnect for an app launch (it opens a launcher target)", () => {
    const onConnect = vi.fn();
    void launchConnectionAccount(host, "app", { ...account, uri: undefined }, onConnect);
    expect(onConnect).not.toHaveBeenCalled();
  });
});

describe("isSafeConnectionAppUri", () => {
  it("allows web and mail launch targets", () => {
    expect(isSafeConnectionAppUri("https://10.20.0.1")).toBe(true);
    expect(isSafeConnectionAppUri("http://10.20.0.1")).toBe(true);
    expect(isSafeConnectionAppUri("mailto:ops@example.com?subject=x&body=y")).toBe(true);
  });

  it("allows custom launcher schemes handled by the client OS", () => {
    expect(isSafeConnectionAppUri("microsoft-edge:http://10.20.0.1")).toBe(true);
    expect(isSafeConnectionAppUri("microsoft-edge:https://example.com")).toBe(true);
    expect(isSafeConnectionAppUri("notepad:")).toBe(true);
    expect(isSafeConnectionAppUri("notepad:foo")).toBe(true);
    expect(isSafeConnectionAppUri("ms-settings:display")).toBe(true);
    expect(isSafeConnectionAppUri("file:///tmp/report.txt")).toBe(true);
    expect(isSafeConnectionAppUri("iexplore:http://10.20.0.1")).toBe(true);
  });

  it("blocks in-page scripts, whitespace, and schemeless targets", () => {
    expect(isSafeConnectionAppUri("javascript:alert(1)")).toBe(false);
    expect(isSafeConnectionAppUri("data:text/html,<script>alert(1)</script>")).toBe(false);
    expect(isSafeConnectionAppUri("blob:https://example.com/123")).toBe(false);
    expect(isSafeConnectionAppUri("about:blank")).toBe(false);
    expect(isSafeConnectionAppUri("vbscript:msgbox(1)")).toBe(false);
    expect(isSafeConnectionAppUri("microsoft-edge:file:///etc/passwd")).toBe(false);
    expect(isSafeConnectionAppUri("microsoft-edge:javascript:alert(1)")).toBe(false);
    expect(isSafeConnectionAppUri("microsoft-edge:http://user:pass@example.com")).toBe(false);
    expect(isSafeConnectionAppUri("microsoft-edge:http:")).toBe(false);
    expect(isSafeConnectionAppUri("ms-msdt:/id PCWDiagnostic")).toBe(false);
    expect(isSafeConnectionAppUri("search-ms:query=secret")).toBe(true);
    expect(isSafeConnectionAppUri("intent://scan/#Intent;scheme=zxing;end")).toBe(false);
    expect(isSafeConnectionAppUri("shell:AppsFolder")).toBe(true);
    expect(isSafeConnectionAppUri("https://example.com/a b")).toBe(false);
    expect(isSafeConnectionAppUri("mailto:ops@example.com?subject=bad%ZZ")).toBe(false);
    expect(isSafeConnectionAppUri("custom:bad\"quote")).toBe(false);
    expect(isSafeConnectionAppUri("10.20.0.1")).toBe(false);
  });
});

describe("isSafeConnectionAppLauncherTarget", () => {
  it("allows trusted local commands for the desktop client", () => {
    expect(isSafeConnectionAppLauncherTarget("notepad")).toBe(true);
    expect(isSafeConnectionAppLauncherTarget("calc.exe")).toBe(true);
    expect(isSafeConnectionAppLauncherTarget('"C:\\Program Files\\Vendor\\App.exe" --host 10.20.0.1')).toBe(true);
    expect(isSafeConnectionAppLauncherTarget("/opt/vendor/client --target 10.20.0.1")).toBe(true);
  });

  it("keeps strict URI schemes strict and blocks in-page schemes", () => {
    expect(isSafeConnectionAppLauncherTarget("https://example.com/a b")).toBe(false);
    expect(isSafeConnectionAppLauncherTarget("microsoft-edge:file:///etc/passwd")).toBe(false);
    expect(isSafeConnectionAppLauncherTarget("javascript:alert(1)")).toBe(false);
    expect(isSafeConnectionAppLauncherTarget("notepad\u0000")).toBe(false);
  });
});

describe("openConnectionAppUri", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    delete (window as Window & { __TAURI__?: unknown }).__TAURI__;
  });

  it("opens http(s) launch targets in a new browser tab", async () => {
    const openSpy = vi.spyOn(window, "open").mockReturnValue({} as Window);

    await expect(openConnectionAppUri("http://10.20.0.1")).resolves.toEqual({
      ok: true,
      mode: "browser-tab",
    });
    expect(openSpy).toHaveBeenCalledWith("http://10.20.0.1", "_blank", "noopener,noreferrer");
  });

  it("dispatches a custom OS-scheme URI through a transient tab, not a top-frame navigation", async () => {
    // window.open is the browser-blessed protocol-dispatch path; failure (no
    // registered handler) surfaces in the throwaway tab, never over the app.
    const openSpy = vi.spyOn(window, "open").mockReturnValue(null);

    const result = await openConnectionAppUri("microsoft-edge:http://10.20.0.1");

    expect(result).toEqual({ ok: true, mode: "browser-uri" });
    expect(openSpy).toHaveBeenCalledWith(
      "microsoft-edge:http://10.20.0.1",
      "_blank",
      "noopener,noreferrer",
    );
  });

  it("reports that local commands need the desktop client in a plain browser", async () => {
    await expect(openConnectionAppUri("notepad")).resolves.toEqual({
      ok: false,
      reason: "desktop-required",
    });
  });

  it("hands local commands to the desktop client when Tauri is available", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    (window as Window & { __TAURI__?: { core: { invoke: typeof invoke } } }).__TAURI__ = {
      core: { invoke },
    };

    await expect(openConnectionAppUri("notepad")).resolves.toEqual({
      ok: true,
      mode: "desktop",
    });
    expect(invoke).toHaveBeenCalledWith("open_app_uri", { uri: "notepad" });
  });

  it("reports desktop cancellation without treating it as a launch failure", async () => {
    const invoke = vi.fn().mockRejectedValue("app launch cancelled by user");
    (window as Window & { __TAURI__?: { core: { invoke: typeof invoke } } }).__TAURI__ = {
      core: { invoke },
    };

    await expect(openConnectionAppUri("notepad")).resolves.toEqual({
      ok: false,
      reason: "cancelled",
      message: "app launch cancelled by user",
    });
  });
});
