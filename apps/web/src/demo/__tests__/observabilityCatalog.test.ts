import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import type { HostLabelsResponse } from "@/features/observability/api";
import type { ListPlaybooksResponse } from "@/features/playbooks/api";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const requests: string[] = [];

beforeAll(() => {
  server.events.on("request:start", ({ request }) => requests.push(request.method));
  server.listen({ onUnhandledRequest: "error" });
});
afterEach(() => {
  requests.length = 0;
  server.resetHandlers();
});
afterAll(() => server.close());

describe("demo integration availability", () => {
  it("keeps absent alert data unknown for every requested host", async () => {
    const ids = ["00000000-0000-4000-8000-000000000147", "00000000-0000-4000-8000-000000000148"];
    const query = new URLSearchParams(ids.map(id => ["host_id", id]));
    const response = await fetch(`${window.location.origin}/api/v1/monitoring/host-labels?${query}`);
    expect(response.ok).toBe(true);
    const result = await response.json() as HostLabelsResponse;
    expect(result.hosts.map(host => host.host_id)).toEqual(ids);
    for (const host of result.hosts) {
      expect(host.alerts).toEqual([]);
      expect(host.alerts_stale).toBe(true);
      expect(host.alerts_updated_at).toBeNull();
    }
    expect(requests).toEqual(["GET"]);
  });

  it("serves the demo catalog without pretending to perform remote refresh", async () => {
    const response = await fetch(`${window.location.origin}/api/v1/playbooks?limit=100&offset=0`);
    expect(response.ok).toBe(true);
    const result = await response.json() as ListPlaybooksResponse;
    expect(result.items).toEqual([]);
    expect(result.official_catalog_refresh.state).toBe("disabled");
    expect(result.official_catalog_refresh.last_attempt_at).toBeNull();
    expect(result.official_catalog_refresh.last_success_at).toBeNull();
    expect(requests).toEqual(["GET"]);
  });
});
