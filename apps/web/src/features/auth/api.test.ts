import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type * as AuthApi from "./api";
let api: typeof AuthApi;

beforeEach(async () => {
  // Node's Request requires an absolute URL; browser Request resolves relative
  // paths against the document before sending the same-origin login request.
  const NativeRequest = globalThis.Request;
  vi.stubGlobal("Request", class extends NativeRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(typeof input === "string" ? new URL(input, window.location.origin) : input, init);
    }
  });
  vi.resetModules();
  api = await import("./api");
});
afterEach(() => vi.unstubAllGlobals());

describe("remember-device API contract", () => {
  it.each([undefined, false, true])("sends opt-in %s as an explicit boolean", async (rememberDevice) => {
    const requests: Request[] = [];
    vi.stubGlobal("fetch", vi.fn((request: Request) => {
      requests.push(request.clone());
      return Promise.resolve(Response.json({ user_id: "fixture", username: "fixture", email: "fixture@example.test", status: "active", auth_source: "local", expires_at: "2026-12-01T00:00:00Z", roles: [], capabilities: [] }));
    }));
    await api.login({ identifier: "fixture", password: "fixture-input", rememberDevice });
    expect(requests).toHaveLength(1);
    expect(await requests[0]!.json()).toEqual({ identifier: "fixture", password: "fixture-input", remember_device: rememberDevice ?? false });
  });

  it("reads the instance's advertised remembered-session duration", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(Response.json({ local_enabled: true, oidc_enabled: true, ldap_enabled: false, ad_enabled: false, remember_device_ttl_seconds: 604800 }))));
    expect(await api.getAuthProviders()).toMatchObject({ rememberDeviceTtlSeconds: 604800 });
  });

  it("binds the selected opt-in to the SSO start request", () => {
    const remembered = new URL(api.buildSsoStartUrl("/inventory/resources?entity=Sites", true), window.location.origin);
    expect(remembered.searchParams.get("redirect_to")).toBe("/inventory/resources?entity=Sites");
    expect(remembered.searchParams.get("remember_device")).toBe("true");
    const ordinary = new URL(api.buildSsoStartUrl("/connections"), window.location.origin);
    expect(ordinary.searchParams.has("remember_device")).toBe(false);
  });
});
