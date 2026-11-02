import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const NativeRequest = Request;
beforeEach(() => {
  // Browsers resolve relative URLs; Node's Request needs the browser origin.
  vi.stubGlobal("Request", class extends NativeRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(typeof input === "string" ? new URL(input, window.location.origin) : input, init);
    }
  });
});

afterEach(() => { vi.unstubAllGlobals(); vi.unstubAllEnvs(); vi.resetModules(); });

const bodylessEntities = [
  "Module Bay Types", "Module Bays", "Module Bay Templates", "Module Type Profiles",
  "Circuit Group Assignments", "Wireless LAN Groups", "Wireless LANs", "Wireless Links",
  "FHRP Groups", "FHRP Group Assignments", "Cooling Sources", "Cooling Feeds",
  "Cooling Intakes", "Cooling Outflows", "Cooling Intake Templates", "Cooling Outflow Templates",
] as const;

describe("Inventory bodyless DELETE", () => {
  it.each(bodylessEntities)("accepts a real 204 for %s", async entity => {
    vi.resetModules();
    vi.stubEnv("VITE_API_BASE_URL", "http://localhost");
    const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetch);
    const { deleteResource } = await import("./api");
    await expect(deleteResource(entity, "id")).resolves.toBeUndefined();
    expect(fetch).toHaveBeenCalledTimes(1);
    expect((fetch.mock.calls[0]?.[0] as Request).method).toBe("DELETE");
  });

  it.each(bodylessEntities)("rejects failed %s deletion and exposes only localized feedback", async entity => {
    vi.resetModules();
    vi.stubEnv("VITE_API_BASE_URL", "http://localhost");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({ error: { code: "CONFLICT", message: "private SQL detail" } }, { status: 409 })));
    const { deleteResource } = await import("./api");
    const { inventoryErrorMessage } = await import("./inventoryErrors");
    const error = await deleteResource(entity, "id").catch(error => error);
    expect(error).toMatchObject({ status: 409 });
    expect(inventoryErrorMessage(key => key, error)).toBe("inventory.resources.saveErrors.conflict");
  });

  it("propagates a forbidden FHRP deletion when the session is still valid", async () => {
    vi.resetModules();
    vi.stubEnv("VITE_API_BASE_URL", "http://localhost");
    const fetch = vi.fn()
      .mockResolvedValueOnce(Response.json({ error: { code: "FORBIDDEN", message: "private detail" } }, { status: 403 }))
      .mockResolvedValueOnce(Response.json({ id: "user" }));
    vi.stubGlobal("fetch", fetch);
    const { deleteResource } = await import("./api");
    const { inventoryErrorMessage } = await import("./inventoryErrors");
    const error = await deleteResource("FHRP Groups", "id").catch(error => error);
    expect(error).toMatchObject({ status: 403 });
    expect(inventoryErrorMessage(key => key, error)).not.toContain("private detail");
    expect(fetch).toHaveBeenCalledTimes(2);
  });
});
