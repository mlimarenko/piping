import { beforeEach, describe, expect, it, vi } from "vitest";
const client = vi.hoisted(() => ({ GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() }));
vi.mock("@/shared/api/client", () => ({ apiClient: client, unwrap: (response: { data?: unknown }) => {
  if (response.data === undefined) throw new Error("Empty response body");
  return response.data;
} }));
import { createDataflow, deleteDataflow, getDataflow, resolveDataflowEndpoint, getDataflowPolicy, listDataflows, updateDataflow } from "./dataflows-api";
import type { DataflowCreate, DataflowEndpointResolution } from "./dataflows-api";

beforeEach(() => vi.clearAllMocks());
describe("dataflows typed API domain", () => {
  it("accepts the endpoint-only create contract without duplicating server defaults", async () => {
    const body = { source: { object_type: "internet" }, destination: { object_type: "all" } } satisfies DataflowCreate;
    client.POST.mockResolvedValue({ data: { id: "flow", ...body } });
    await createDataflow(body);
    expect(client.POST).toHaveBeenCalledWith("/api/v1/inventory/dataflows", { body });
  });
  it("posts the complete selector to the canonical read-only resolution route", async () => {
    const body = { object_type: "dcim.region", object_id: "region", address_mode: "selected", ipaddresses: ["ip-1"] } as const;
    const resolution = { endpoint: { ...body, ipaddresses: ["ip-1"], label: "Region A" }, members: [{ host: { object_type: "dcim.device", object_id: "host", label: "Host" }, addresses: [{ cidr: "192.0.2.1/32", vrf_id: null }], diagnostics: [] }], addresses: [{ cidr: "192.0.2.1/32", vrf_id: null }], diagnostics: [], ipaddresses: [{ id: "ip-1", address: "192.0.2.1/24", vrf_id: null }], default_ipaddresses: ["ip-1"] } satisfies DataflowEndpointResolution;
    client.POST.mockResolvedValue({ data: resolution });
    expect(await resolveDataflowEndpoint({ ...body, ipaddresses: ["ip-1"] })).toEqual(resolution);
    expect(client.POST).toHaveBeenCalledWith("/api/v1/inventory/dataflows/endpoint-resolution", { body });
    expect(client.GET).not.toHaveBeenCalled();
  });
  it("collects the canonical list envelope across pages", async () => {
    client.GET.mockResolvedValueOnce({ data: { items: [{ id: "a" }], total: 2, limit: 1, offset: 0 } })
      .mockResolvedValueOnce({ data: { items: [{ id: "b" }], total: 2, limit: 1, offset: 1 } });
    expect(await listDataflows()).toEqual([{ id: "a" }, { id: "b" }]);
    expect(client.GET).toHaveBeenNthCalledWith(2, "/api/v1/inventory/dataflows", { params: { query: { limit: 100, offset: 1 } } });
  });
  it("uses the owning client for create, detail, PUT, DELETE and host-policy query", async () => {
    const body = { name: "HTTPS", description: "", status: "draft", transport: "tcp", source: { object_type: "dcim.device", object_id: "d" }, destination: { object_type: "ipam.service", object_id: "s" }, destination_ports: [443] } satisfies DataflowCreate;
    client.POST.mockResolvedValue({ data: { id: "flow", ...body } });
    client.GET.mockResolvedValue({ data: { id: "flow", ...body } });
    client.PUT.mockResolvedValue({ data: { id: "flow", ...body, status: "disabled" } });
    client.DELETE.mockResolvedValue({ response: new Response(null, { status: 204 }) });
    await createDataflow(body);
    expect(client.POST).toHaveBeenCalledWith("/api/v1/inventory/dataflows", { body });
    await getDataflow("flow");
    expect(client.GET).toHaveBeenCalledWith("/api/v1/inventory/dataflows/{id}", { params: { path: { id: "flow" } } });
    await updateDataflow("flow", { status: "disabled" });
    expect(client.PUT).toHaveBeenCalledWith("/api/v1/inventory/dataflows/{id}", { params: { path: { id: "flow" } }, body: { status: "disabled" } });
    await expect(deleteDataflow("flow")).resolves.toBeUndefined();
    expect(client.DELETE).toHaveBeenCalledWith("/api/v1/inventory/dataflows/{id}", { params: { path: { id: "flow" } } });
    await getDataflowPolicy({ object_type: "virtualization.virtualmachine", object_id: "vm" });
    expect(client.GET).toHaveBeenCalledWith("/api/v1/inventory/dataflows/firewall-policy", { params: { query: { host_type: "virtualization.virtualmachine", host_id: "vm" } } });
  });
});
