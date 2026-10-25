import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { cacheCreatedInventoryResource, inventoryResourceOption, inventoryResourceOptionsQuery } from "./resourceOptions";
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

describe("inventoryResourceOption", () => {
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
