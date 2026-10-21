import { describe, expect, it, vi, beforeEach } from "vitest";

// Mock the inventory API module so the descriptor's real list/CRUD wiring is
// exercised against spies — this is the one piece of non-mocked logic in the
// nested editor (FK injection + the parent filter key). The spies are created
// via `vi.hoisted` so they exist before the hoisted `vi.mock` factory runs.
const api = vi.hoisted(() => ({
  listChildResourcesByRouteQuery: vi.fn(() =>
    Promise.resolve({ rows: [], total: 0, limit: 50, offset: 0 }),
  ),
  createResource: vi.fn((_entity: string, body: Record<string, unknown>) =>
    Promise.resolve({ id: "new-ip-id", name: typeof body.name === "string" ? body.name : "", type: "ip" }),
  ),
  updateResource: vi.fn((_e: string, id: string, body: Record<string, unknown>) =>
    Promise.resolve({ id, name: typeof body.name === "string" ? body.name : "", type: "ip" }),
  ),
  deleteResource: vi.fn(() => Promise.resolve()),
}));
const { listChildResourcesByRouteQuery, createResource } = api;

vi.mock("@/features/inventory/api", () => ({
  ...api,
  INVENTORY_MAX_PAGE_SIZE: 1000,
}));

// Compiled after the mock is registered.
import { descriptorFromSpec, type RelationSpec } from "./descriptorFromSpec";

const t = (key: string) => key;

/** The real IP-under-interface spec (mirrors `relationshipsByTab`). */
const IP_SPEC: RelationSpec = {
  key: "device.interfaces.ips",
  childEntity: "IP Addresses",
  title: "nestedEditor.relations.ipAddresses",
  icon: "Globe2",
  listFilterKey: "assigned_object_id",
  listRoute: "/api/v1/inventory/ip-addresses",
  parentFkField: "interfaceId",
  rowForm: { kind: "config", entity: "IP Addresses" },
};

describe("descriptorFromSpec — real IP-under-interface wiring", () => {
  beforeEach(() => {
    api.listChildResourcesByRouteQuery.mockClear();
    api.createResource.mockClear();
    api.updateResource.mockClear();
    api.deleteResource.mockClear();
  });

  it("lists children with the assigned_object_id filter (not interface_id)", async () => {
    const desc = descriptorFromSpec(IP_SPEC, t);
    await desc.listByParent("iface-uuid", { limit: 100, offset: 0 });

    expect(listChildResourcesByRouteQuery).toHaveBeenCalledTimes(1);
    const [, route, query] = listChildResourcesByRouteQuery.mock.calls[0] as unknown as [
      string,
      string,
      Record<string, unknown>,
    ];
    expect(route).toBe("/api/v1/inventory/ip-addresses");
    expect(query.assigned_object_id).toBe("iface-uuid");
    expect(query.interface_id).toBeUndefined();
    expect(query.limit).toBe(100);
  });

  it("toPayload injects the parent real id under the camelCase FK idKey (interfaceId)", () => {
    const desc = descriptorFromSpec(IP_SPEC, t);
    const draft = desc.buildDraft();
    draft.prefix = "10.0.0.1/24"; // IP "address" maps to the `prefix` draft key

    const payload = desc.toPayload(draft, "iface-real-uuid");
    // The FK rides under the camelCase Resource idKey so the api layer's
    // `toBackendPayload` converts it to `interface_id` exactly once.
    expect(payload.interfaceId).toBe("iface-real-uuid");
    expect(payload.interface_id).toBeUndefined();
    // The address survives the draft→Resource mapping.
    expect(payload.prefix).toBe("10.0.0.1/24");
  });

  it("create routes through createResource with the IP entity and FK-injected body", async () => {
    const desc = descriptorFromSpec(IP_SPEC, t);
    const draft = desc.buildDraft();
    draft.prefix = "10.0.0.2/24";

    const { id } = await desc.create(desc.toPayload(draft, "iface-real-uuid"));
    expect(id).toBe("new-ip-id");
    expect(createResource).toHaveBeenCalledTimes(1);
    const [entity, body] = createResource.mock.calls[0] as unknown as [string, Record<string, unknown>];
    expect(entity).toBe("IP Addresses");
    expect(body.interfaceId).toBe("iface-real-uuid");
  });

  it("validation does not flag the implied parent FK as required-missing", () => {
    const desc = descriptorFromSpec(IP_SPEC, t);
    const draft = desc.buildDraft();
    draft.prefix = "10.0.0.3/24"; // address present; interfaceId intentionally absent
    const errs = desc.validate(draft);
    expect(errs.interfaceId).toBeUndefined();
  });

  it("compiles a nested child relation (Interface → IP)", () => {
    const ifaceSpec: RelationSpec = {
      key: "device.interfaces",
      childEntity: "Interfaces",
      title: "nestedEditor.relations.interfaces",
      icon: "Network",
      listFilterKey: "device_id",
      listRoute: "/api/v1/inventory/interfaces",
      parentFkField: "deviceId",
      rowForm: { kind: "config", entity: "Interfaces" },
      child: IP_SPEC,
    };
    const desc = descriptorFromSpec(ifaceSpec, t);
    expect(desc.childRelation).toBeDefined();
    expect(desc.childRelation?.key).toBe("device.interfaces.ips");
    expect(desc.parentFkField).toBe("deviceId");
  });
});
