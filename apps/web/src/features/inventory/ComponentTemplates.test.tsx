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
    if (result.error) {
      throw result.error instanceof Error ? result.error : new Error("API mock error");
    }
    return result.data;
  },
}));

vi.mock("@/features/inventory/custom-fields-api", () => ({
  ensureContentTypeCatalog: () => Promise.resolve(),
  contentTypeTokenForId: (id: string) =>
    id === "ct-dcim-powerporttemplate" ? "dcim.powerporttemplate" : undefined,
  contentTypeIdForToken: (token: string) =>
    token === "dcim.unsupportedtemplate"
      ? Promise.reject(new Error("Unsupported content type"))
      : Promise.resolve(`ct-${token}`),
}));

import {
  createResource,
  getResource,
  inventoryTableListPathFor,
  listResourcesForTable,
  updateResource,
} from "./api";
import {
  buildEntityDraft,
  draftToResourceUpdate,
  validateEntityDraft,
} from "@/shared/forms/entityForm";
import { descriptorFromSpec } from "@/shared/nested-editor/descriptorFromSpec";
import {
  frontPortMappingsFromWire,
  parseFrontPortMappings,
  parseRearPortMappings,
  rearPortMappingsWire,
  serializeRearPortMappings,
} from "./rearPortMappings";
import {
  fieldsByTab,
  inventoryModelCapabilities,
  relationshipsByTab,
} from "./components/inventory/inventoryTypes";
import { inventoryTableConfigFor } from "./tableConfig";
import type { InventoryEntityKey } from "@/shared/api/types";

const t = (key: string) => key;

const MODULAR: InventoryEntityKey[] = [
  "Console Port Templates",
  "Console Server Port Templates",
  "Power Port Templates",
  "Power Outlet Templates",
  "Interface Templates",
  "Front Port Templates",
  "Rear Port Templates",
];
const DEVICE_ONLY: InventoryEntityKey[] = ["Device Bay Templates", "Inventory Item Templates"];
const NINE: InventoryEntityKey[] = [...MODULAR, ...DEVICE_ONLY];

const ROUTE: Record<string, string> = {
  "Console Port Templates": "console-port-templates",
  "Console Server Port Templates": "console-server-port-templates",
  "Power Port Templates": "power-port-templates",
  "Power Outlet Templates": "power-outlet-templates",
  "Interface Templates": "interface-templates",
  "Front Port Templates": "front-port-templates",
  "Rear Port Templates": "rear-port-templates",
  "Device Bay Templates": "device-bay-templates",
  "Inventory Item Templates": "inventory-item-templates",
};

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("Native component-template form contract", () => {
  it("requires exactly one hardware parent for the modular models", () => {
    for (const entity of MODULAR) {
      const bare = { ...buildEntityDraft(entity), name: "Port" };
      expect(validateEntityDraft(entity, bare, t), entity).toHaveProperty("deviceTypeId");
      expect(
        validateEntityDraft(entity, { ...bare, deviceTypeId: "dt" }, t),
        entity,
      ).not.toHaveProperty("deviceTypeId");
      expect(
        validateEntityDraft(entity, { ...bare, moduleTypeId: "mt" }, t),
        entity,
      ).not.toHaveProperty("deviceTypeId");
      expect(
        validateEntityDraft(entity, { ...bare, deviceTypeId: "dt", moduleTypeId: "mt" }, t),
        entity,
      ).toHaveProperty("deviceTypeId");
    }
  });

  it("requires a device type for device bays and inventory item templates", () => {
    for (const entity of DEVICE_ONLY) {
      const bare = { ...buildEntityDraft(entity), name: "Item" };
      expect(validateEntityDraft(entity, bare, t), entity).toHaveProperty("deviceTypeId");
      expect(
        validateEntityDraft(entity, { ...bare, deviceTypeId: "dt" }, t),
        entity,
      ).not.toHaveProperty("deviceTypeId");
    }
  });

  it("defaults interface toggles and front/rear positions on create", () => {
    const iface = buildEntityDraft("Interface Templates");
    expect(iface.enabled).toBe("true");
    expect(iface.mgmtOnly).toBe("false");
    expect(buildEntityDraft("Device Bay Templates").enabled).toBe("true");
    expect(buildEntityDraft("Front Port Templates").positions).toBe("1");
    expect(buildEntityDraft("Rear Port Templates").positions).toBe("1");
  });

  it("rejects a 65-character template name and an allocated draw above the maximum", () => {
    const base = { ...buildEntityDraft("Power Port Templates"), name: "PSU", deviceTypeId: "dt" };
    expect(validateEntityDraft("Power Port Templates", { ...base, name: "x".repeat(65) }, t)).toHaveProperty("name");
    const over = validateEntityDraft("Power Port Templates", { ...base, maximumDraw: "300", allocatedDraw: "450" }, t);
    expect(over).toHaveProperty("allocatedDraw");
    expect(over).not.toHaveProperty("deviceTypeId");
    expect(validateEntityDraft("Power Port Templates", { ...base, maximumDraw: "450", allocatedDraw: "300" }, t)).toEqual({});
  });

  it("declares no tags, custom fields, status or comments on any of the nine models", () => {
    for (const entity of NINE) {
      expect(inventoryModelCapabilities(entity), entity).toEqual({ tags: false, custom_fields: false, change_dates: false });
      const keys = (fieldsByTab[entity] ?? []).map((field) => field.key);
      expect(keys, entity).not.toContain("tags");
      expect(keys, entity).not.toContain("ownerId");
      expect(keys, entity).not.toContain("comments");
      expect(keys, entity).not.toContain("status");
    }
  });

  it("lists every model under the correct hardware parent in the nested editor", () => {
    const deviceSpecs = relationshipsByTab["Device Types"]!.filter((spec) => spec.rowForm.kind === "config" && NINE.includes(spec.childEntity));
    expect(deviceSpecs.map((spec) => spec.childEntity)).toEqual(NINE);
    for (const spec of deviceSpecs) {
      expect(spec.parentFkField).toBe("deviceTypeId");
      expect(spec.listFilterKey).toBe("device_type_id");
      expect(spec.listRoute).toBe(`/api/v1/inventory/${ROUTE[spec.childEntity]}`);
      const desc = descriptorFromSpec(spec, t);
      const draft = { ...desc.buildDraft(), name: "Nested", componentType: "lc" };
      expect(desc.validate(draft)).toEqual({});
      expect(desc.toPayload(draft, "hardware")).toMatchObject({ deviceTypeId: "hardware", name: "Nested" });
    }
    const moduleSpecs = relationshipsByTab["Module Types"]!.filter((spec) => spec.rowForm.kind === "config" && NINE.includes(spec.childEntity));
    expect(moduleSpecs.map((spec) => spec.childEntity)).toEqual(MODULAR);
    for (const spec of moduleSpecs) {
      expect(spec.parentFkField).toBe("moduleTypeId");
      expect(spec.listFilterKey).toBe("module_type_id");
    }
  });

  it("keeps the parent funnel mapped to a declared backend column", () => {
    for (const entity of NINE) {
      const config = inventoryTableConfigFor(entity);
      expect(config.sortKeys, entity).toEqual(["name", "description"]);
      expect(config.filterColumns.some((column) => column.param === "device_type_id"), entity).toBe(true);
      for (const column of config.filterColumns) {
        expect(
          config.sortKeys.includes(column.colKey) || config.unsortableKeys?.includes(column.colKey),
          `${entity}: ${column.colKey}`,
        ).toBe(true);
      }
    }
    // The specialized Module Bay Templates list query has no sort/order support:
    // its table must not request them, and its columns stay filter-only.
    const moduleBay = inventoryTableConfigFor("Module Bay Templates");
    expect(moduleBay.sortKeys).toEqual([]);
    expect(moduleBay.unsortableKeys).toEqual(expect.arrayContaining(["device_type_id", "module_type_id", "enabled"]));

    // Native query DTOs accept only these filters; unsupported funnels stay
    // documented as unfilterable while their columns remain visible.
    expect(inventoryTableConfigFor("Power Outlet Templates").filterColumns.map((column) => column.param)).toEqual(["device_type_id", "module_type_id", "power_port_id"]);
    expect(inventoryTableConfigFor("Power Outlet Templates").unfilterableKeys).toContain("feed_leg");
    expect(inventoryTableConfigFor("Interface Templates").filterColumns.map((column) => column.param)).toEqual(["device_type_id", "module_type_id"]);
    expect(inventoryTableConfigFor("Interface Templates").unfilterableKeys).toEqual(expect.arrayContaining(["bridge_id", "enabled", "mgmt_only"]));
    expect(inventoryTableConfigFor("Inventory Item Templates").filterColumns.map((column) => column.param)).toEqual(["device_type_id", "parent_id"]);
    expect(inventoryTableConfigFor("Inventory Item Templates").unfilterableKeys).toEqual(expect.arrayContaining(["manufacturer_id", "role_id"]));
  });

  it("sends only the native-accepted sort and filter params for template tables", async () => {
    apiMocks.GET.mockResolvedValue({ data: { items: [], total: 0, limit: 25, offset: 0 } });
    await listResourcesForTable("Power Outlet Templates", {
      limit: 25, offset: 0, sort: "name", order: "asc",
      filters: { device_type_id: ["dt"], power_port_id: ["pp"] },
    });
    expect(apiMocks.GET).toHaveBeenLastCalledWith("/api/v1/inventory/power-outlet-templates", {
      params: { query: { limit: 25, offset: 0, sort: "name", order: "asc", device_type_id: "dt", power_port_id: "pp" } },
    });
    expect(inventoryTableListPathFor("Power Outlet Templates")).toBe("/api/v1/inventory/power-outlet-templates");

    await listResourcesForTable("Module Bay Templates", { limit: 25, offset: 0, filters: { module_type_id: ["mt"] } });
    expect(apiMocks.GET).toHaveBeenLastCalledWith("/api/v1/inventory/module-bay-templates", {
      params: { query: { limit: 25, offset: 0, module_type_id: "mt" } },
    });
    expect(inventoryTableListPathFor("Module Bay Templates")).toBe("/api/v1/inventory/module-bay-templates");
  });

  it("offers exactly the native component-target allowlist, scoped to the same device type", () => {
    const component = (fieldsByTab["Inventory Item Templates"] ?? []).find((field) => field.key === "component")!;
    expect(component.type).toBe("objectRef");
    expect(component.emptyValue).toBeNull();
    expect(component.optionsScope).toEqual({ deviceTypeId: "deviceTypeId" });
    expect(component.objectTypes?.map((target) => target.token)).toEqual([
      "dcim.consoleporttemplate",
      "dcim.consoleserverporttemplate",
      "dcim.coolingintaketemplate",
      "dcim.coolingoutflowtemplate",
      "dcim.frontporttemplate",
      "dcim.interfacetemplate",
      "dcim.poweroutlettemplate",
      "dcim.powerporttemplate",
      "dcim.rearporttemplate",
    ]);
    expect(component.objectTypes?.map((target) => target.token)).not.toEqual(expect.arrayContaining(["dcim.devicebaytemplate", "dcim.inventoryitemtemplate", "dcim.modulebaytemplate"]));
    const deviceBayParent = (fieldsByTab["Device Bay Templates"] ?? []).find((field) => field.key === "deviceTypeId")!;
    expect(deviceBayParent.optionsSubdeviceRole).toBe("parent");
    const relation = relationshipsByTab["Device Types"]!.find((spec) => spec.childEntity === "Device Bay Templates")!;
    expect(relation.parentSubdeviceRole).toBe("parent");
  });

  it("writes both native component refs and clears the pair atomically", async () => {
    await expect(createResource("Inventory Item Templates", { name: "PSU", type: "inventory_item_template", deviceTypeId: "dt", component: { objectType: "dcim.unsupportedtemplate", objectId: "x", label: null } })).rejects.toThrow("Unsupported content type");
    apiMocks.POST.mockResolvedValue({ data: { id: "item-1" } });
    await createResource("Inventory Item Templates", {
      name: "PSU", type: "inventory_item_template", deviceTypeId: "dt",
      component: { objectType: "dcim.powerporttemplate", objectId: "pp", label: null },
    });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/inventory-item-templates", {
      body: expect.objectContaining({ component_type_id: "ct-dcim.powerporttemplate", component_id: "pp" }),
    });

    const row = {
      id: "item-1", name: "PSU", type: "inventory_item_template", deviceTypeId: "dt",
      component: { objectType: "dcim.powerporttemplate", objectId: "pp", label: "PSU 1" },
    };
    const initial = buildEntityDraft("Inventory Item Templates", row);
    expect(initial.component).toBe("pp");
    expect(initial["component__type"]).toBe("dcim.powerporttemplate");
    apiMocks.PUT.mockResolvedValue({ data: row });
    await updateResource("Inventory Item Templates", "item-1", draftToResourceUpdate(
      "Inventory Item Templates",
      { ...initial, component: "", "component__type": "" },
      "Inventory Item Template",
      row,
      initial,
    ));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ component_type_id: null, component_id: null });
  });

  it("hydrates a backend-shaped component pair into the typed reference", async () => {
    apiMocks.GET.mockResolvedValueOnce({ data: {
      id: "item-2", name: "PSU", device_type_id: "dt",
      component_type_id: "ct-dcim-powerporttemplate", component_id: "pp",
    } });
    const resource = await getResource("Inventory Item Templates", "item-2");
    expect(resource.component).toEqual({ objectType: "dcim.powerporttemplate", objectId: "pp", label: null });
    const draft = buildEntityDraft("Inventory Item Templates", resource);
    expect(draft.component).toBe("pp");
    expect(draft["component__type"]).toBe("dcim.powerporttemplate");
    apiMocks.PUT.mockResolvedValue({ data: { id: "item-2", name: "PSU" } });
    await updateResource("Inventory Item Templates", "item-2", draftToResourceUpdate(
      "Inventory Item Templates",
      { ...draft, component: "", "component__type": "" },
      "Inventory Item Template",
      resource,
      draft,
    ));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ component_type_id: null, component_id: null });
  });
});

describe("Front port rear-port mappings", () => {
  const rows = [
    { position: 1, rearPortId: "rear", rearPortPosition: 2 },
    { position: 2, rearPortId: "rear", rearPortPosition: 1 },
  ];

  it("round-trips rows and rejects null while [] clears", async () => {
    expect(parseRearPortMappings(undefined)).toBeUndefined();
    expect(parseRearPortMappings("")).toEqual([]);
    expect(parseRearPortMappings(serializeRearPortMappings(rows))).toEqual(rows);
    expect(() => parseRearPortMappings(null)).toThrow();
    expect(() => parseRearPortMappings('[{"position":1,"rearPortId":"rear","rearPortPosition":0}]')).toThrow();
    expect(rearPortMappingsWire(rows)).toEqual([
      { position: 1, rear_port: "rear", rear_port_position: 2 },
      { position: 2, rear_port: "rear", rear_port_position: 1 },
    ]);

    apiMocks.POST.mockResolvedValue({ data: { id: "f2" } });
    await createResource("Front Port Templates", { name: "Front", type: "front_port_template", deviceTypeId: "dt", componentType: "lc", positions: "2", rearPorts: serializeRearPortMappings(rows) });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/front-port-templates", {
      body: expect.objectContaining({
        rear_ports: [
          { position: 1, rear_port: "rear", rear_port_position: 2 },
          { position: 2, rear_port: "rear", rear_port_position: 1 },
        ],
      }),
    });
    await createResource("Front Port Templates", { name: "Front", type: "front_port_template", deviceTypeId: "dt", componentType: "lc", positions: "2", rearPorts: "" });
    expect(apiMocks.POST.mock.lastCall?.[1].body).toMatchObject({ rear_ports: [] });
    await createResource("Front Port Templates", { name: "Front", type: "front_port_template", deviceTypeId: "dt", componentType: "lc", positions: "2" });
    expect(apiMocks.POST.mock.lastCall?.[1].body).not.toHaveProperty("rear_ports");
    await expect(createResource("Front Port Templates", { name: "Front", type: "front_port_template", deviceTypeId: "dt", componentType: "lc", positions: "2", rearPorts: null as unknown as string })).rejects.toThrow();
  });

  it("maps wire rows on read and preserves omission on partial update", async () => {
    apiMocks.GET.mockResolvedValueOnce({ data: { id: "f1", name: "Front", type: "lc", positions: 2, device_type_id: "dt", module_type_id: null, rear_ports: [{ position: 1, rear_port: "rear", rear_port_position: 2 }] } });
    const resource = await getResource("Front Port Templates", "f1");
    expect(resource.rearPorts).toBe(serializeRearPortMappings([{ position: 1, rearPortId: "rear", rearPortPosition: 2 }]));
    const initial = buildEntityDraft("Front Port Templates", resource);
    expect(parseRearPortMappings(initial.rearPorts)).toEqual([{ position: 1, rearPortId: "rear", rearPortPosition: 2 }]);
    apiMocks.PUT.mockResolvedValue({ data: { id: "f1", name: "Front renamed", type: "lc", positions: 2, device_type_id: "dt", module_type_id: null } });
    await updateResource("Front Port Templates", "f1", draftToResourceUpdate("Front Port Templates", { ...initial, name: "Front renamed" }, "Front Port Template", resource, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ name: "Front renamed" });
  });

  it("rejects duplicate and out-of-range front positions in the shared validator", () => {
    const base = { ...buildEntityDraft("Front Port Templates"), name: "Front", deviceTypeId: "dt", componentType: "lc", positions: "2" };
    const duplicate = serializeRearPortMappings([
      { position: 1, rearPortId: "rear", rearPortPosition: 1 },
      { position: 1, rearPortId: "rear", rearPortPosition: 2 },
    ]);
    expect(validateEntityDraft("Front Port Templates", { ...base, rearPorts: duplicate }, t)).toHaveProperty("rearPorts");
    const outOfRange = serializeRearPortMappings([{ position: 3, rearPortId: "rear", rearPortPosition: 1 }]);
    expect(validateEntityDraft("Front Port Templates", { ...base, rearPorts: outOfRange }, t)).toHaveProperty("rearPorts");
    expect(validateEntityDraft("Front Port Templates", { ...base, rearPorts: serializeRearPortMappings(rows) }, t)).toEqual({});
  });

  it("declares the reverse projection read-only on rear port templates", () => {
    const front = (fieldsByTab["Front Port Templates"] ?? []).find((field) => field.key === "rearPorts")!;
    expect(front.type).toBe("rear-port-mappings");
    expect(front.optionsScope).toEqual({ deviceTypeId: "deviceTypeId", moduleTypeId: "moduleTypeId" });
    const rear = (fieldsByTab["Rear Port Templates"] ?? []).find((field) => field.key === "frontPorts")!;
    expect(rear.readOnly).toBe(true);
    expect(rear.tableColumn).toBe(false);
  });

  it("maps the asymmetric reverse projection by side", () => {
    const serialized = frontPortMappingsFromWire([{ position: 2, front_port: "front", front_port_position: 1 }]);
    expect(parseFrontPortMappings(serialized)).toEqual([{ frontPosition: 1, frontPortId: "front", rearPosition: 2 }]);
  });
});

describe("Native component-template client", () => {
  it("round-trips device bay enabled and leaves it out of unrelated updates", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "bay" } });
    const created = buildEntityDraft("Device Bay Templates");
    await createResource("Device Bay Templates", { name: "Bay", type: "device_bay_template", deviceTypeId: "dt", enabled: created.enabled });
    expect(apiMocks.POST.mock.lastCall?.[1].body).toMatchObject({ enabled: true });

    apiMocks.GET.mockResolvedValueOnce({ data: { id: "bay", name: "Bay", device_type_id: "dt", enabled: false } });
    const resource = await getResource("Device Bay Templates", "bay");
    const initial = buildEntityDraft("Device Bay Templates", resource);
    expect(initial.enabled).toBe("false");
    apiMocks.PUT.mockResolvedValue({ data: { id: "bay", name: "Bay" } });
    await updateResource("Device Bay Templates", "bay", draftToResourceUpdate("Device Bay Templates", { ...initial, name: "Renamed" }, "Device Bay Template", resource, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ name: "Renamed" });
    await updateResource("Device Bay Templates", "bay", { enabled: "false" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ enabled: false });
  });
  it("maps read rows and keeps nullable template members", async () => {
    apiMocks.GET.mockResolvedValueOnce({ data: { id: "c1", name: "Console", label: "OOB", type: "usb-c", description: "Out-of-band", device_type_id: "dt", module_type_id: null } });
    await expect(getResource("Console Port Templates", "c1")).resolves.toMatchObject({
      name: "Console", label: "OOB", componentType: "usb-c", deviceTypeId: "dt", type: "console_port_template",
      createdAt: undefined, updatedAt: undefined,
    });
    expect(apiMocks.GET).toHaveBeenLastCalledWith("/api/v1/inventory/console-port-templates/{id}", { params: { path: { id: "c1" } } });

    apiMocks.GET.mockResolvedValueOnce({ data: { id: "f1", name: "Front 1", type: "lc", color: "", positions: 2, device_type_id: "dt", module_type_id: null } });
    await expect(getResource("Front Port Templates", "f1")).resolves.toMatchObject({ componentType: "lc", positions: "2", color: "" });

    apiMocks.GET.mockResolvedValueOnce({ data: { id: "o1", name: "Outlet", type: "iec-60320-c13", color: "155dfc", power_port_id: "p1", feed_leg: "A", device_type_id: "dt", module_type_id: null } });
    await expect(getResource("Power Outlet Templates", "o1")).resolves.toMatchObject({ powerPortId: "p1", feedLeg: "A", componentType: "iec-60320-c13" });
  });

  it("writes snake_case fields with explicit nulls and preserved false values", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "x" } });
    await createResource("Power Port Templates", { name: "PSU", type: "power_port_template", deviceTypeId: "dt", moduleTypeId: null, componentType: "iec-60320-c14", maximumDraw: "450", allocatedDraw: "300", label: "PSU A", description: "Inlet" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/power-port-templates", {
      body: { name: "PSU", label: "PSU A", description: "Inlet", device_type_id: "dt", module_type_id: null, type: "iec-60320-c14", maximum_draw: 450, allocated_draw: 300 },
    });

    await createResource("Power Outlet Templates", { name: "Outlet", type: "power_outlet_template", moduleTypeId: "mt", deviceTypeId: null, componentType: "", color: "#155DFC", powerPortId: "pp", feedLeg: "B" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/power-outlet-templates", {
      body: { name: "Outlet", device_type_id: null, module_type_id: "mt", type: null, color: "155dfc", power_port_id: "pp", feed_leg: "B" },
    });

    await createResource("Interface Templates", { name: "mgmt0", type: "interface_template", deviceTypeId: "dt", componentType: "1000base-t", enabled: "false", mgmtOnly: "true", bridgeId: "br", poeMode: "pd", poeType: "type2-ieee802.3at", rfRole: "ap" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/interface-templates", {
      body: { name: "mgmt0", device_type_id: "dt", type: "1000base-t", enabled: false, mgmt_only: true, bridge_id: "br", poe_mode: "pd", poe_type: "type2-ieee802.3at", rf_role: "ap" },
    });

    await createResource("Device Bay Templates", { name: "Bay 1", type: "device_bay_template", deviceTypeId: "dt", label: "GPU" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/device-bay-templates", {
      body: { name: "Bay 1", label: "GPU", device_type_id: "dt" },
    });

    await createResource("Inventory Item Templates", { name: "PSU", type: "inventory_item_template", deviceTypeId: "dt", parentId: "parent", manufacturerId: "mf", roleId: "role", partId: "74-1" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/inventory-item-templates", {
      body: { name: "PSU", device_type_id: "dt", parent_id: "parent", manufacturer_id: "mf", role_id: "role", part_id: "74-1" },
    });

    await createResource("Front Port Templates", { name: "Front", type: "front_port_template", deviceTypeId: "dt", moduleTypeId: null, componentType: "lc", color: "", positions: "2" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/front-port-templates", {
      body: { name: "Front", device_type_id: "dt", module_type_id: null, type: "lc", color: "", positions: 2 },
    });
  });

  it.each<InventoryEntityKey>([
    "Interface Templates", "Front Port Templates", "Rear Port Templates",
  ])("preserves explicit null type intent for %s", async (entity) => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "t1", name: "Template" } });
    await updateResource(entity, "t1", { componentType: null });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith(`/api/v1/inventory/${ROUTE[entity]}/{id}`, {
      params: { path: { id: "t1" } },
      body: { type: null },
    });
  });

  it("sends only operator changes on update and never invents metadata", async () => {
    const row = { id: "p1", name: "PSU", type: "power_port_template", deviceTypeId: "dt", moduleTypeId: null, componentType: "iec-60320-c14", maximumDraw: "450", label: "PSU A" };
    const initial = buildEntityDraft("Power Port Templates", row);
    apiMocks.PUT.mockResolvedValue({ data: row });
    await updateResource("Power Port Templates", "p1", draftToResourceUpdate("Power Port Templates", { ...initial, name: "PSU renamed" }, "Power Port Template", row, initial));
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/power-port-templates/{id}", {
      params: { path: { id: "p1" } },
      body: { name: "PSU renamed" },
    });
    await updateResource("Power Port Templates", "p1", draftToResourceUpdate("Power Port Templates", { ...initial, deviceTypeId: "" }, "Power Port Template", row, initial));
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/power-port-templates/{id}", {
      params: { path: { id: "p1" } },
      body: { device_type_id: null },
    });
  });
});
