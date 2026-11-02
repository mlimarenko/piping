import { setupServer } from "msw/node";
import createClient from "openapi-fetch";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { paths } from "@/shared/api/schema.gen";
import type * as ApiClientModule from "@/shared/api/client";
import { listDataflows, resolveDataflowEndpoint } from "@/features/inventory/dataflows-api";
import { createDemoDataflows } from "../dataflows";
import { handlers } from "../handlers";

vi.mock("@/shared/api/client", async original => {
  const module = await original<typeof ApiClientModule>();
  return { ...module, apiClient: createClient<paths>({ baseUrl: window.location.origin, fetch: request => globalThis.fetch(request) }) };
});
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

describe("dataflow demo API", () => {
  it("loads the actual directed demo flows through the product pagination client", async () => {
    const flows = await listDataflows();
    expect(flows).toHaveLength(24);
    expect(new Set(flows.map(flow => flow.id)).size).toBe(flows.length);
    expect(new Set(flows.map(flow => flow.status))).toEqual(new Set(["active", "draft", "disabled"]));
    expect(flows.some(flow => flow.source.object_id === flow.destination.object_id)).toBe(true);
  });

  it("resolves a real seeded host and excludes a selected address outside it", async () => {
    const [flow] = await listDataflows();
    const result = await resolveDataflowEndpoint(flow!.source);
    expect(result.members).toHaveLength(1);
    expect(result.members[0]?.host.object_id).toBe(flow!.source.object_id);
    expect(result.addresses).toEqual([{ cidr: "192.0.2.11/32", vrf_id: null }]);
    const outside = await resolveDataflowEndpoint({ ...flow!.source, address_mode: "selected", ipaddresses: ["foreign-ip"] });
    expect(outside.addresses).toEqual([]);
  });

  it("can supply a high-cardinality synthetic dataset without customer records", () => {
    const flows = createDemoDataflows(521);
    expect(flows).toHaveLength(521);
    expect(new Set(flows.map(flow => flow.id)).size).toBe(521);
    expect(new Set(flows.map(flow => flow.destination.object_id)).size).toBeGreaterThan(490);
  });
});
