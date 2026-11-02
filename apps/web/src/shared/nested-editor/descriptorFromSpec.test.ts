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

describe("module bay parent scope", () => {
  const spec: RelationSpec = { key: "module.bays", childEntity: "Module Bays", title: "nestedEditor.relations.moduleBays", icon: "Container", listFilterKey: "module_id", listRoute: "/api/v1/inventory/module-bays", parentFkField: "moduleId", fixedFields: { deviceId: "inherited-device" }, rowForm: { kind: "config", entity: "Module Bays" } };
  it("pins inherited device and containing module on create, including default enabled", async () => {
    const desc = descriptorFromSpec(spec, t);
    const draft = { ...desc.buildDraft(), name: "Child slot", deviceId: "tampered-device" };
    expect(desc.validate(draft)).toEqual({});
    expect(desc.toPayload(draft, "containing-module")).toMatchObject({ name: "Child slot", deviceId: "inherited-device", moduleId: "containing-module", enabled: "true", moduleBayTypes: [] });
    await desc.listByParent("containing-module", { limit: 25, offset: 0 });
    expect(api.listChildResourcesByRouteQuery).toHaveBeenLastCalledWith("Module Bays", "/api/v1/inventory/module-bays", { module_id: "containing-module", limit: 25, offset: 0 });
  });
  it("sends only changed values on edit and preserves membership until explicitly cleared", () => {
    const desc = descriptorFromSpec(spec, t);
    const initial = desc.toDraft({ id: "bay", name: "Child slot", type: "module_bay", deviceId: "inherited-device", moduleId: "containing-module", enabled: "false", moduleBayTypes: ["type-id"] });
    expect(desc.toPayload({ ...initial, name: "Renamed" }, "containing-module", initial)).toEqual({ name: "Renamed" });
    expect(desc.toPayload({ ...initial, moduleBayTypes: "" }, "containing-module", initial)).toEqual({ moduleBayTypes: [] });
  });
});

/** The real IP-under-interface spec (mirrors `relationshipsByTab`). */
const IP_SPEC: RelationSpec = {
  key: "device.interfaces.ips",
  childEntity: "IP Addresses",
  title: "nestedEditor.relations.ipAddresses",
  icon: "Globe2",
  listFilterKey: "assigned_object_id",
  listRoute: "/api/v1/inventory/ip-addresses",
  parentFkField: "assignedObject",
  parentObjectType: "dcim.interface",
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

  it("toPayload injects the parent token and UUID into the canonical assignment", () => {
    const desc = descriptorFromSpec(IP_SPEC, t);
    const draft = desc.buildDraft();
    draft.prefix = "10.0.0.1/24"; // IP "address" maps to the `prefix` draft key

    const payload = desc.toPayload(draft, "iface-real-uuid");
    expect(payload.assignedObject).toEqual({ objectType: "dcim.interface", objectId: "iface-real-uuid", label: null });
    expect(payload.interface_id).toBeUndefined();
    // The address survives the draft→Resource mapping.
    expect(payload.prefix).toBe("10.0.0.1/24");
  });

  it("preserves the interface assignment when editing an existing IP", () => {
    const desc = descriptorFromSpec(IP_SPEC, t);
    const initial = desc.toDraft({ id: "ip", name: "10.0.0.1/24", type: "ip_address", prefix: "10.0.0.1/24", assignedObject: { objectType: "dcim.interface", objectId: "iface-real-uuid", label: "eth0" } });
    const payload = desc.toPayload({ ...initial, prefix: "10.0.0.2/24" }, "iface-real-uuid", initial);
    expect(payload).toMatchObject({ prefix: "10.0.0.2/24", assignedObject: { objectType: "dcim.interface", objectId: "iface-real-uuid" } });
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
    expect(body.assignedObject).toEqual({ objectType: "dcim.interface", objectId: "iface-real-uuid", label: null });
  });

  it("validation does not flag the implied parent FK as required-missing", () => {
    const desc = descriptorFromSpec(IP_SPEC, t);
    const draft = desc.buildDraft();
    draft.prefix = "10.0.0.3/24";
    const errs = desc.validate(draft);
    expect(errs.assignedObject).toBeUndefined();
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
