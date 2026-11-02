import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  POST: vi.fn(),
  PUT: vi.fn(),
  PATCH: vi.fn(),
}));

vi.mock("@/shared/api/client", () => ({
  apiClient: apiMocks,
  unwrap: (result: { data?: unknown; error?: unknown }) => {
    if (result.error) throw result.error instanceof Error ? result.error : new Error("API mock error");
    return result.data;
  },
}));

const rawChild = {
  id: "item-1",
  name: "PSU",
  device_type_id: "dt",
  component_type_id: "ct-dcim-powerporttemplate",
  component_id: "pp",
};

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
  vi.resetModules();
});

describe("cold content-type catalog in the nested child path", () => {
  it("waits for the shared catalog before projecting a component reference", async () => {
    const { listChildResourcesByRouteQuery } = await import("./api");
    const { buildEntityDraft, draftToResourceUpdate } = await import("@/shared/forms/entityForm");
    const { contentTypeIdForToken } = await import("./custom-fields-api");

    let releaseCatalog: () => void = () => undefined;
    const catalogGate = new Promise<void>((resolve) => { releaseCatalog = resolve; });
    const calls: string[] = [];
    apiMocks.GET.mockImplementation((path: string) => {
      calls.push(path);
      if (path === "/api/v1/inventory/content-types") {
        return catalogGate.then(() => ({
          data: [{ id: "ct-dcim-powerporttemplate", token: "dcim.powerporttemplate", app_label: "dcim", model: "powerporttemplate", label: "Power Port Template", supported_for_custom_fields: false }],
        }));
      }
      return Promise.resolve({ data: { items: [rawChild], total: 1, limit: 1000, offset: 0 } });
    });

    // The child page is requested first; its projection must not complete while
    // the catalog response is still deliberately withheld.
    const pending = listChildResourcesByRouteQuery(
      "Inventory Item Templates",
      "/api/v1/inventory/inventory-item-templates",
      { device_type_id: "dt", limit: 1000, offset: 0 },
    );
    await Promise.resolve();
    releaseCatalog();
    const page = await pending;

    const child = page.rows[0];
    if (!child) throw new Error("Expected the nested inventory template");
    expect(child.component).toEqual({ objectType: "dcim.powerporttemplate", objectId: "pp", label: null });
    expect(calls[0]).toBe("/api/v1/inventory/content-types");

    const draft = buildEntityDraft("Inventory Item Templates", child);
    expect(draft.component).toBe("pp");
    expect(draft["component__type"]).toBe("dcim.powerporttemplate");
    const cleared = draftToResourceUpdate(
      "Inventory Item Templates",
      { ...draft, component: "", "component__type": "" },
      "Inventory Item Template",
      child,
      draft,
    );
    expect(cleared.component).toBeNull();
    expect(await contentTypeIdForToken("dcim.powerporttemplate")).toBe("ct-dcim-powerporttemplate");
  });

  it("keeps the standalone list read coherent for the same entity", async () => {
    const { listResources } = await import("./api");
    let releaseCatalog: () => void = () => undefined;
    const catalogGate = new Promise<void>((resolve) => { releaseCatalog = resolve; });
    apiMocks.GET.mockImplementation((path: string) => {
      if (path === "/api/v1/inventory/content-types") {
        return catalogGate.then(() => ({
          data: [{ id: "ct-dcim-powerporttemplate", token: "dcim.powerporttemplate", app_label: "dcim", model: "powerporttemplate", label: "Power Port Template", supported_for_custom_fields: false }],
        }));
      }
      return Promise.resolve({ data: { items: [rawChild], total: 1, limit: 25, offset: 0 } });
    });
    const pending = listResources("Inventory Item Templates", { limit: 25, offset: 0 });
    await Promise.resolve();
    releaseCatalog();
    const page = await pending;
    expect(page.items[0]?.component).toEqual({ objectType: "dcim.powerporttemplate", objectId: "pp", label: null });
  });
});
