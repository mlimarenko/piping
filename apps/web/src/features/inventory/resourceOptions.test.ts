import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { cacheCreatedInventoryResource, inventoryResourceOption, inventoryResourceOptions, inventoryResourceOptionsQuery } from "./resourceOptions";
import type { Resource } from "@/shared/api/types";

const api = vi.hoisted(() => ({ listAllResources: vi.fn() }));
vi.mock("./api", () => api);

describe("inventory resource options during first-load creation", () => {
  it.each(["success", "failure"])("retains the created option immediately and refreshes the missing full list independently (%s)", async (outcome) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const options = inventoryResourceOptionsQuery("Contacts");
    const created: Resource = { id: "created", name: "Created contact", type: "contact" };
    const existing: Resource = { id: "existing", name: "Existing contact", type: "contact" };
    const deleted: Resource = { id: "deleted", name: "Deleted contact", type: "contact" };
    let finishOld!: (resources: Resource[]) => void;
    let finishFresh!: (resources: Resource[]) => void;
    let failFresh!: (error: Error) => void;
    api.listAllResources.mockReset();
    api.listAllResources.mockReturnValueOnce(new Promise<Resource[]>((resolve) => { finishOld = resolve; }));
    api.listAllResources.mockReturnValueOnce(new Promise<Resource[]>((resolve, reject) => { finishFresh = resolve; failFresh = reject; }));
    const observer = new QueryObserver(client, options);
    const unsubscribe = observer.subscribe(() => {});
    try {
      expect(api.listAllResources).toHaveBeenCalledTimes(1);
      expect(client.getQueryData(options.queryKey)).toBeUndefined();
      // Cache completion must not wait on the background list or report the
      // already-successful POST as failed when that read fails.
      await expect(cacheCreatedInventoryResource(client, "Contacts", created)).resolves.toBeUndefined();
      expect(client.getQueryData(options.queryKey)?.items).toEqual([created]);
      expect(api.listAllResources).toHaveBeenCalledTimes(2);
      expect(client.getQueryState(options.queryKey)?.isInvalidated).toBe(true);
      finishOld([existing, deleted]);
      if (outcome === "success") finishFresh([existing, created]);
      else failFresh(new Error("Options temporarily unavailable"));
      await vi.waitFor(() => expect(client.getQueryState(options.queryKey)?.fetchStatus).toBe("idle"));
      expect(client.getQueryData(options.queryKey)?.items).toEqual(outcome === "success" ? [existing, created] : [created]);
      expect(client.getQueryData(options.queryKey)?.items).not.toContainEqual(deleted);
      expect(client.getQueryState(options.queryKey)?.status).toBe(outcome === "success" ? "success" : "error");
    } finally {
      unsubscribe();
      client.clear();
    }
  });
});

describe("inventoryResourceOptions", () => {
  const device = (id: string, name: string, site: string): Resource => ({
    id, name, type: "device", site, role: "access-switch", primaryIp: "10.0.0.1", tenant: "core",
  });

  it("reuses one list while the page is unchanged and follows its replacement", () => {
    const page = [device("a", "switch-a", "Amsterdam"), device("b", "switch-b", "Berlin")];
    const first = inventoryResourceOptions("Devices", page);
    expect(inventoryResourceOptions("Devices", page)).toBe(first);

    const edited = [{ ...page[0]!, name: "switch-a-renamed", site: "Utrecht" }, page[1]!];
    const replaced = inventoryResourceOptions("Devices", edited);
    expect(replaced).not.toBe(first);
    expect(replaced[0]).toMatchObject({ name: "switch-a-renamed", subtitle: "Utrecht · access-switch · 10.0.0.1" });

    const created = inventoryResourceOptions("Devices", [...edited, device("c", "switch-c", "Delft")]);
    expect(created.map((option) => option.id)).toEqual(["a", "b", "c"]);
    expect(inventoryResourceOptions("Devices", [edited[1]!]).map((option) => option.id)).toEqual(["b"]);
  });

  it("keeps locale case folding for collapsed parts and the first spelling", () => {
    const racks = [{ id: "r", name: "rack-1", type: "rack", site: "Amsterdam DC1", tenant: "amsterdam dc1" }];
    expect(inventoryResourceOptions("Racks", racks)[0]?.subtitle).toBe("Amsterdam DC1");
  });

  it("keeps entity-specific labels separate when callers share a page", () => {
    const page = [{ ...device("a", "node-a", "Amsterdam"), cluster: "cluster-a" }];
    const devices = inventoryResourceOptions("Devices", page);
    const virtualMachines = inventoryResourceOptions("Virtual Machines", page);
    expect(devices[0]?.subtitle).toBe("Amsterdam · access-switch · 10.0.0.1");
    expect(virtualMachines[0]?.subtitle).toBe("Amsterdam · cluster-a · access-switch · 10.0.0.1");
    expect(inventoryResourceOptions("Devices", page)).toBe(devices);
  });
});

describe("inventoryResourceOption", () => {
  it("keeps canonical Tag identity separate from authorized label, slug and color", () => {
    expect(inventoryResourceOption("Tags", { id: "tag-id", name: "Core", slug: "core", color: "155dfc", type: "tag" })).toMatchObject({ id: "tag-id", name: "Core", slug: "core", color: "155dfc" });
  });
  it("disambiguates device models by manufacturer and keeps slug separate from search text", () => {
    const option = inventoryResourceOption("Device Types", {
      id: "type-a", name: "Core Switch", type: "device_type", slug: "core-switch",
      manufacturerName: "Maker A", manufacturerId: "maker-a",
    });
    expect(option.subtitle).toBe("Maker A · maker-a");
    expect(option.searchText).toContain("Maker A");
    expect(option.searchText).toContain("maker-a");
    expect(option.slug).toBe("core-switch");
  });
});
