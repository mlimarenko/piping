import { expect, it, vi } from "vitest";
import { listConnectionTargets } from "@/features/connections/api";
import { listAllResources } from "@/features/inventory/api";

const api = vi.hoisted(() => ({ GET: vi.fn() }));
vi.mock("@/shared/api/client", () => ({ apiClient: api, unwrap: (result: { data: unknown }) => result.data }));

it("identity loaders include site and connection-target names beyond their transport first page", async () => {
  api.GET.mockImplementation((path, options) => {
    const { limit, offset } = options.params.query;
    const targets = path === "/api/v1/connections/targets";
    const total = targets ? 50_001 : 1001;
    return Promise.resolve({ data: { items: Array.from({ length: Math.min(limit, total - offset) }, (_, index) => ({ id: `synthetic-${offset + index}`, name: `Synthetic ${offset + index}`, target_kind: "device", status: "active", address: "192.0.2.155" })), total, limit, offset } });
  });
  const sites = await listAllResources("Sites");
  expect(sites).toHaveLength(1001);
  expect(sites.at(-1)?.name).toBe("Synthetic 1000");
  const targets = await listConnectionTargets();
  expect(targets.items).toHaveLength(50_001);
  expect(targets.items.at(-1)?.name).toBe("Synthetic 50000");
  expect(api.GET).toHaveBeenCalledWith("/api/v1/connections/targets", expect.objectContaining({ params: { query: { limit: 50_000, offset: 50_000 } } }));
});
