import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import { inventoryResourceOptionsQuery } from "./resourceOptions";

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
      if (result.error instanceof Error) throw result.error;
      throw new Error(typeof result.error === "string" ? result.error : "API mock error");
    }
    return result.data;
  },
}));

import { tabConfig } from "./components/inventory/inventoryTypes";
import {
  buildInventoryTableParams,
  bulkUpdateResources,
  createResource,
  customFieldEntityKind,
  deleteResource,
  getResource,
  getTemplate,
  inventoryTableListPathEntities,
  inventoryTableListPathFor,
  listAllResources,
  listResources,
  listResourcesForTable,
  slugify,
  updateResource,
} from "./api";
import { inventoryTableConfigFor } from "./tableConfig";
import { buildEntityDraft, draftToResourceUpdate } from "@/shared/forms/entityForm";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("canonical Tag catalog routing", () => {
  const tagId = "00000000-0000-4000-8000-000000009401";
  const tag = { id: tagId, name: "Core", slug: "core", color: "155dfc", description: "Network", weight: 0, object_types: [] as string[] };
  it("uses the native list/detail routes and UUID-valued catalog filters", async () => {
    apiMocks.GET.mockResolvedValueOnce({ data: { items: [tag], total: 1, limit: 25, offset: 0 } });
    const config = inventoryTableConfigFor("Tags");
    const filter = config.filterColumns.find(column => column.colKey === "name")!;
    expect((await listResourcesForTable("Tags", { limit: 25, offset: 0, sort: "weight", filters: { [filter.param]: [tagId] } })).items[0]).toMatchObject({ id: tagId, name: "Core", type: "tag", weight: "0", objectTypes: [] });
    expect(apiMocks.GET).toHaveBeenLastCalledWith("/api/v1/inventory/tags", { params: { query: { limit: 25, offset: 0, sort: "weight", order: "asc", ids: tagId } } });
    apiMocks.GET.mockResolvedValueOnce({ data: { ...tag, name: "Renamed" } });
    expect(await getResource("Tags", tagId)).toMatchObject({ id: tagId, name: "Renamed" });
    expect(apiMocks.GET).toHaveBeenLastCalledWith("/api/v1/inventory/tags/{id}", { params: { path: { id: tagId } } });
  });

  it("creates through POST and renames through partial PUT without changing catalog identity", async () => {
    apiMocks.POST.mockResolvedValue({ data: tag });
    expect(await createResource("Tags", { name: "Core", type: "tag", slug: "core", color: "#155DFC", description: "Network" })).toMatchObject({ id: tagId, weight: "0" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/tags", { body: { name: "Core", slug: "core", color: "155dfc", description: "Network", weight: 0 } });

    const resource = { ...tag, type: "tag", weight: "0", objectTypes: tag.object_types };
    const initial = buildEntityDraft("Tags", resource);
    apiMocks.PUT.mockResolvedValue({ data: { ...tag, name: "Renamed", description: "" } });
    expect(await updateResource("Tags", tagId, draftToResourceUpdate("Tags", { ...initial, name: "Renamed", description: "" }, "Tag", resource, initial))).toMatchObject({ id: tagId, name: "Renamed" });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/tags/{id}", { params: { path: { id: tagId } }, body: { name: "Renamed", description: "" } });
    await updateResource("Tags", tagId, { color: "#008236", weight: "100" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ color: "008236", weight: 100 });
  });

  it("accepts a bodyless DELETE 204 and propagates native transport failures", async () => {
    // Native middleware throws on HTTP failures; 204 success has no data to unwrap.
    apiMocks.DELETE.mockResolvedValue(undefined);
    await expect(deleteResource("Tags", tagId)).resolves.toBeUndefined();
    expect(apiMocks.DELETE).toHaveBeenLastCalledWith("/api/v1/inventory/tags/{id}", { params: { path: { id: tagId } } });
    const blocked = new Error("Native deletion blocked");
    apiMocks.DELETE.mockRejectedValue(blocked);
    await expect(deleteResource("Tags", tagId)).rejects.toBe(blocked);
    apiMocks.POST.mockRejectedValue(blocked);
    await expect(createResource("Tags", { name: "Core", type: "tag", slug: "core", color: "155dfc" })).rejects.toBe(blocked);
    apiMocks.PUT.mockRejectedValue(blocked);
    await expect(updateResource("Tags", tagId, { name: "Renamed" })).rejects.toBe(blocked);
  });

  it("preserves portable scope tokens and sends [] only for an explicit clear", async () => {
    apiMocks.GET.mockResolvedValueOnce({ data: { ...tag, object_types: ["dcim.device"] } });
    const resource = await getResource("Tags", tagId);
    expect(resource.objectTypes).toEqual(["dcim.device"]);
    const initial = buildEntityDraft("Tags", resource);
    expect(draftToResourceUpdate("Tags", initial, "Tag", resource, initial)).not.toHaveProperty("objectTypes");
    apiMocks.PUT.mockResolvedValue({ data: tag });
    await updateResource("Tags", tagId, draftToResourceUpdate("Tags", { ...initial, objectTypes: "" }, "Tag", resource, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ object_types: [] });
    apiMocks.POST.mockResolvedValue({ data: { ...tag, object_types: ["dcim.device"] } });
    await createResource("Tags", { name: "Scoped", type: "tag", slug: "scoped", color: "155dfc", objectTypes: ["dcim.device"] });
    expect(apiMocks.POST.mock.lastCall?.[1].body.object_types).toEqual(["dcim.device"]);
  });

  it.each([undefined, null, "dcim.device", [null], [""], ["dcim.device", "dcim.device"]].map(object_types => ({ object_types })))("does not infer unrestricted scope from malformed metadata (%j)", async ({ object_types }) => {
    apiMocks.GET.mockResolvedValueOnce({ data: { ...tag, object_types } });
    await expect(getResource("Tags", tagId)).rejects.toThrow("Invalid native Tag object-type scope");
  });

  it("keeps bulk edits partial and maps an explicit scope clear to []", async () => {
    apiMocks.PUT.mockResolvedValue({ data: tag });
    expect(await bulkUpdateResources("Tags", [tagId], { description: null })).toEqual({ updated: [tagId], failed: [] });
    expect(apiMocks.GET).not.toHaveBeenCalled();
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ description: "" });
    await bulkUpdateResources("Tags", [tagId], { objectTypes: "dcim.device,dcim.site" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ object_types: ["dcim.device", "dcim.site"] });
    await bulkUpdateResources("Tags", [tagId], { objectTypes: null });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ object_types: [] });
  });

  it("keeps bulk attachment UUID sets typed, clears explicitly and omits untouched membership", async () => {
    const ownerId = "00000000-0000-4000-8000-000000009101";
    apiMocks.GET.mockResolvedValue({ data: { id: ownerId, name: "Campus", slug: "campus", tags: [tagId] } });
    apiMocks.PATCH.mockResolvedValue({ data: { id: ownerId, name: "Campus", tags: [tagId] } });
    await bulkUpdateResources("Wireless LAN Groups", [ownerId], { tags: tagId });
    expect(apiMocks.PATCH.mock.lastCall?.[1].body.tags).toEqual([tagId]);
    await bulkUpdateResources("Wireless LAN Groups", [ownerId], { tags: null });
    expect(apiMocks.PATCH.mock.lastCall?.[1].body.tags).toEqual([]);
    await bulkUpdateResources("Wireless LAN Groups", [ownerId], { description: "Updated" });
    expect(apiMocks.PATCH.mock.lastCall?.[1].body).not.toHaveProperty("tags");
  });

  it("sends owner membership IDs unchanged, omits untouched membership and sends [] to clear", async () => {
    const owner = { id: "group", name: "Campus", type: "wireless_lan_group", tags: [tagId] };
    const initial = buildEntityDraft("Wireless LAN Groups", owner);
    apiMocks.PATCH.mockResolvedValue({ data: { id: "group", name: "Campus", tags: [tagId] } });
    await updateResource("Wireless LAN Groups", "group", draftToResourceUpdate("Wireless LAN Groups", { ...initial, name: "Renamed" }, "Group", owner, initial));
    expect(apiMocks.PATCH.mock.lastCall?.[1].body).toEqual({ name: "Renamed" });
    await updateResource("Wireless LAN Groups", "group", draftToResourceUpdate("Wireless LAN Groups", { ...initial, tags: "" }, "Group", owner, initial));
    expect(apiMocks.PATCH.mock.lastCall?.[1].body).toEqual({ tags: [] });
    await updateResource("Wireless LAN Groups", "group", { tags: [tagId] });
    expect(apiMocks.PATCH.mock.lastCall?.[1].body).toEqual({ tags: [tagId] });
  });
});

describe("Module bay templates client", () => {
  const raw = { id: "slot", name: "Uplink", label: "WAN", position: "2", enabled: false, description: "Slot specification", device_type_id: null, module_type_id: "mt", module_bay_types: ["type"] };
  it("lists by hardware parent and round trips label, false and membership without inventing metadata", async () => {
    apiMocks.GET.mockResolvedValue({ data: { items: [raw], total: 1, limit: 25, offset: 0 } });
    const page = await listResourcesForTable("Module Bay Templates", { limit: 25, offset: 0, filters: { module_type_id: ["mt"] } });
    expect(page).toMatchObject({ total: 1, items: [{ name: "Uplink", label: "WAN", position: "2", enabled: "false", moduleBayTypes: ["type"], moduleTypeId: "mt", createdAt: undefined, updatedAt: undefined }] });
    expect(apiMocks.GET).toHaveBeenLastCalledWith("/api/v1/inventory/module-bay-templates", { params: { query: { limit: 25, offset: 0, module_type_id: "mt" } } });
    expect(inventoryTableListPathFor("Module Bay Templates")).toBe("/api/v1/inventory/module-bay-templates");
    apiMocks.POST.mockResolvedValue({ data: raw });
    await createResource("Module Bay Templates", { name: "Uplink", type: "module_bay_template", deviceTypeId: null, moduleTypeId: "mt", label: "WAN", position: "2", enabled: "false", moduleBayTypes: ["type"], ownerId: "ignored", tags: ["ignored"], customFields: { ignored: true } });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/module-bay-templates", { body: { name: "Uplink", device_type_id: null, module_type_id: "mt", label: "WAN", position: "2", enabled: false, module_bay_types: ["type"] } });
    apiMocks.GET.mockResolvedValue({ data: raw });
    expect(await getResource("Module Bay Templates", "slot")).toMatchObject({ label: "WAN", enabled: "false", moduleBayTypes: ["type"] });
    expect(customFieldEntityKind("Module Bay Templates")).toBeUndefined();
  });

  it("preserves omissions and explicitly clears membership, text and a nullable parent FK", async () => {
    apiMocks.GET.mockResolvedValue({ data: raw });
    const resource = await getResource("Module Bay Templates", "slot");
    const initial = buildEntityDraft("Module Bay Templates", resource);
    expect(draftToResourceUpdate("Module Bay Templates", initial, "Module Bay Template", resource, initial)).toEqual({ id: "slot" });
    apiMocks.PUT.mockResolvedValue({ data: raw });
    await updateResource("Module Bay Templates", "slot", draftToResourceUpdate("Module Bay Templates", { ...initial, moduleTypeId: "", deviceTypeId: "dt", label: "", position: "", moduleBayTypes: "" }, "Module Bay Template", resource, initial));
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/module-bay-templates/{id}", { params: { path: { id: "slot" } }, body: { module_type_id: null, device_type_id: "dt", label: "", position: "", module_bay_types: [] } });
    await updateResource("Module Bay Templates", "slot", { description: "Renamed specification" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ description: "Renamed specification" });
  });
});

describe("Cooling inventory", () => {
  it.each([
    ["Cooling Sources", "cooling-sources", "cooling_source"],
    ["Cooling Feeds", "cooling-feeds", "cooling_feed"],
    ["Cooling Intakes", "cooling-intakes", "cooling_intake"],
    ["Cooling Outflows", "cooling-outflows", "cooling_outflow"],
    ["Cooling Intake Templates", "cooling-intake-templates", "cooling_intake_template"],
    ["Cooling Outflow Templates", "cooling-outflow-templates", "cooling_outflow_template"],
  ] as const)("round trips %s with canonical CRUD, partial clearing and native dates", async (entity, path, type) => {
    const wireType = entity === "Cooling Sources" ? "chiller" : "qdc";
    const raw = { id: "cooling", name: "Loop", type: wireType, diameter: 12.75, diameter_unit: "mm", max_flow: 1.5, max_flow_unit: "lpm", cooling_capacity: 0, device_id: "device", module_id: "module", device_type_id: null, module_type_id: "module-type", cooling_source_id: "source", cooling_intake_id: null, cooling_outflow_id: null, site_id: "site", location_id: null, tags: [], custom_fields: {}, created: null, last_updated: null };
    apiMocks.GET.mockResolvedValue({ data: { items: [raw], total: 1, limit: 25, offset: 0 } });
    expect((await listResources(entity, { limit: 25 })).items[0]).toMatchObject({ type, coolingType: wireType, diameter: "12.75", maxFlow: "1.5", coolingCapacity: "0", createdAt: undefined, updatedAt: undefined });
    expect(inventoryTableListPathFor(entity)).toBe(`/api/v1/inventory/${path}`);
    apiMocks.GET.mockResolvedValue({ data: raw });
    const resource = await getResource(entity, "cooling");
    const initial = buildEntityDraft(entity, resource);
    const untouched = draftToResourceUpdate(entity, initial, entity, resource, initial);
    expect(untouched).toEqual({ id: "cooling" });
    apiMocks.POST.mockResolvedValue({ data: raw });
    await createResource(entity, { name: "Loop", type, coolingType: wireType, diameter: "12.75", deviceId: "device", moduleTypeId: "module-type", siteId: "site", coolingSourceId: "source", coolingCapacity: "0" });
    expect(apiMocks.POST.mock.lastCall?.[0]).toBe(`/api/v1/inventory/${path}`);
    expect(apiMocks.POST.mock.lastCall?.[1].body).not.toHaveProperty("coolingType");
    expect(apiMocks.POST.mock.lastCall?.[1].body).toEqual(entity === "Cooling Sources"
      ? { name: "Loop", type: "chiller", site_id: "site", cooling_capacity: 0 }
      : entity === "Cooling Feeds" ? { name: "Loop", cooling_source_id: "source", cooling_capacity: 0 }
        : { name: "Loop", type: "qdc", diameter: 12.75, ...(entity.includes("Templates") ? { module_type_id: "module-type" } : { device_id: "device" }) });
    apiMocks.PUT.mockResolvedValue({ data: { ...raw, description: "" } });
    await updateResource(entity, "cooling", draftToResourceUpdate(entity, { ...initial, description: "" }, entity, { ...resource, description: "original" }, { ...initial, description: "original" }));
    expect(apiMocks.PUT).toHaveBeenLastCalledWith(`/api/v1/inventory/${path}/{id}`, { params: { path: { id: "cooling" } }, body: { description: "" } });
    apiMocks.DELETE.mockResolvedValue({ data: undefined });
    await deleteResource(entity, "cooling");
    expect(apiMocks.DELETE).toHaveBeenLastCalledWith(`/api/v1/inventory/${path}/{id}`, { params: { path: { id: "cooling" } } });
  });

  it("clears optional cooling measurements and FKs without transmitting omitted fields", async () => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "in", name: "Inlet" } });
    await updateResource("Cooling Intakes", "in", { diameter: null, diameterUnit: null, maxFlow: "1.25", maxFlowUnit: "m3ph", moduleId: null, coolingOutflowId: null, tags: [], customFields: {} });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ diameter: null, diameter_unit: null, max_flow: 1.25, max_flow_unit: "m3ph", module_id: null, cooling_outflow_id: null, tags: [], custom_fields: {} });
  });

  it.each(["Devices", "Device Types", "Module Types", "Racks", "Rack Types"] as const)("writes %s hardware cooling fields without inventing other hardware columns", async entity => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "hw", name: "Hardware", model: "Hardware" } });
    const rack = entity === "Racks" || entity === "Rack Types";
    await updateResource(entity, "hw", rack ? { coolingCapability: "hybrid", coolingCapacity: "0" } : { coolingMethod: "liquid", ...(entity === "Devices" ? {} : { endOfLife: "2030-06-01" }) });
    const body = apiMocks.PUT.mock.lastCall?.[1].body;
    expect(body).toMatchObject(rack ? { cooling_capability: "hybrid", cooling_capacity: 0 } : { cooling_method: "liquid", ...(entity === "Devices" ? {} : { end_of_life: "2030-06-01" }) });
    if (rack) {
      expect(body).not.toHaveProperty("cooling_method");
      expect(body).not.toHaveProperty("end_of_life");
    } else {
      expect(body).not.toHaveProperty("cooling_capability");
      if (entity === "Devices") expect(body).not.toHaveProperty("end_of_life");
    }
  });

  it("does not invent template metadata or upstream-outflow fields", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "t", name: "Inlet" } });
    await createResource("Cooling Intake Templates", { name: "Inlet", type: "cooling_intake_template", moduleTypeId: "module-type", ownerId: "owner", tags: ["tag"], customFields: { wrong: true }, coolingOutflowId: "outflow", comments: "not supported" });
    expect(apiMocks.POST.mock.lastCall?.[1].body).toEqual({ name: "Inlet", module_type_id: "module-type" });
  });

  it.each(["Cooling Intakes", "Cooling Outflows", "Cooling Intake Templates", "Cooling Outflow Templates"] as const)("retains the %s label through detail, form draft and partial update", async entity => {
    apiMocks.GET.mockResolvedValue({ data: { id: "port", name: "Port", label: "Supply label", device_id: "device", module_type_id: "module-type" } });
    const resource = await getResource(entity, "port");
    expect(resource.label).toBe("Supply label");
    const initial = buildEntityDraft(entity, resource);
    expect(initial.label).toBe("Supply label");
    apiMocks.PUT.mockResolvedValue({ data: { id: "port", name: "Port", label: "" } });
    await updateResource(entity, "port", draftToResourceUpdate(entity, { ...initial, label: "" }, entity, resource, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ label: "" });
  });

  it.each(["Device Types", "Module Types"] as const)("preserves unknown imported %s dates on unrelated changes and allows explicit clearing", async entity => {
    apiMocks.GET.mockResolvedValue({ data: { id: "hw", model: "Hardware", end_of_life: "unknown imported date", cooling_method: "liquid" } });
    const resource = await getResource(entity, "hw");
    const initial = buildEntityDraft(entity, resource);
    expect(initial.endOfLife).toBe("unknown imported date");
    apiMocks.PUT.mockResolvedValue({ data: { id: "hw", model: "Hardware" } });
    await updateResource(entity, "hw", draftToResourceUpdate(entity, { ...initial, name: "Renamed" }, entity, resource, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).not.toHaveProperty("end_of_life");
    await updateResource(entity, "hw", draftToResourceUpdate(entity, { ...initial, endOfLife: "", coolingMethod: "" }, entity, resource, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toMatchObject({ end_of_life: null, cooling_method: null });
  });
});

describe("rack groups and cable bundles", () => {
  it.each([
    ["Rack Groups", "rack-groups", "rack_group"],
    ["Cable Bundles", "cable-bundles", "cable_bundle"],
  ] as const)("round trips %s through canonical CRUD and table routes", async (entity, segment, type) => {
    const record = { id: "g", name: "Network", slug: "network", description: "Row A", comments: "Access aisle", custom_fields: { floor: 2 }, created: "2026-10-26T08:00:00Z", last_updated: null };
    apiMocks.GET.mockResolvedValue({ data: { items: [record], total: 1, limit: 25, offset: 0 } });
    expect((await listResources(entity, { limit: 25 })).items[0]).toMatchObject({ id: "g", type, comments: "Access aisle", customFields: { floor: 2 }, createdAt: record.created, updatedAt: undefined });
    expect(apiMocks.GET).toHaveBeenLastCalledWith(`/api/v1/inventory/${segment}`, { params: { query: { limit: 25, offset: 0, search: undefined } } });
    expect(inventoryTableListPathFor(entity)).toBe(`/api/v1/inventory/${segment}`);

    apiMocks.POST.mockResolvedValue({ data: record });
    const input = { type, name: "Network", slug: "network", description: "Row A", comments: "Access aisle", customFields: { floor: 2 }, status: "active" as const, color: "ffffff" };
    await createResource(entity, input);
    const body = { name: input.name, description: input.description, comments: input.comments, custom_fields: input.customFields, ...(entity === "Rack Groups" ? { slug: "network" } : {}) };
    expect(apiMocks.POST).toHaveBeenLastCalledWith(`/api/v1/inventory/${segment}`, { body });

    apiMocks.PUT.mockResolvedValue({ data: { ...record, comments: "", custom_fields: {} } });
    await updateResource(entity, "g", { comments: "", customFields: {} });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith(`/api/v1/inventory/${segment}/{id}`, { params: { path: { id: "g" } }, body: { comments: "", custom_fields: {} } });
    apiMocks.GET.mockResolvedValue({ data: record });
    expect(await getResource(entity, "g")).toMatchObject({ name: "Network", type });
    expect(apiMocks.GET).toHaveBeenLastCalledWith(`/api/v1/inventory/${segment}/{id}`, { params: { path: { id: "g" } } });
    await deleteResource(entity, "g");
    expect(apiMocks.DELETE).toHaveBeenLastCalledWith(`/api/v1/inventory/${segment}/{id}`, { params: { path: { id: "g" } } });
  });

  it.each([
    ["Racks", "groupId", "groupName", "group_id", "group_name"],
    ["Cables", "bundleId", "bundleName", "bundle_id", "bundle_name"],
  ] as const)("reads, changes and clears the %s relation without dropping an explicit clear", async (entity, idKey, nameKey, wireId, wireName) => {
    apiMocks.GET.mockResolvedValue({ data: { id: "r", name: "Rack", label: "Cable", [wireId]: "g", [wireName]: "Network" } });
    const resource = await getResource(entity, "r");
    expect(resource).toMatchObject({ [idKey]: "g", [nameKey]: "Network" });
    const draft = buildEntityDraft(entity, resource);
    const initialDraft = { ...draft };
    expect(draft[idKey]).toBe("g");
    draft[idKey] = "";
    const update = draftToResourceUpdate(entity, draft, entity, resource, initialDraft);
    apiMocks.PUT.mockResolvedValue({ data: { id: "r", [wireId]: null } });
    await updateResource(entity, "r", update);
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toMatchObject({ [wireId]: null });
    await updateResource(entity, "r", { [idKey]: "other" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ [wireId]: "other" });
    await updateResource(entity, "r", { description: "unrelated" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).not.toHaveProperty(wireId);
  });
});

describe("module bay inventory writes", () => {
  const typeId = "00000000-0000-4000-8000-000000009201";
  it("round-trips bay projections without sending them back, preserves omission and sends false/[]/null", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "bay", name: "Slot 1", device_id: "device", module_id: "container", enabled: false, module_bay_types: [typeId], parent_id: "parent", installed_module: { id: "occupant", module_type_id: "t", status: "active", serial: "S1" }, occupied: true, is_module_compatible: false } });
    const bay = await getResource("Module Bays", "bay");
    expect(bay).toMatchObject({ moduleId: "container", installedModuleId: "occupant", parentId: "parent", enabled: "false", occupied: "true", isModuleCompatible: "false", moduleBayTypes: [typeId] });
    const initial = buildEntityDraft("Module Bays", bay);
    apiMocks.PUT.mockResolvedValue({ data: { id: "bay", name: "Slot 1" } });
    await updateResource("Module Bays", "bay", draftToResourceUpdate("Module Bays", { ...initial, moduleId: "", moduleBayTypes: "", enabled: "true" }, "Module Bay", bay, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ enabled: true, module_bay_types: [] });
    await updateResource("Module Bays", "bay", { enabled: "false" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ enabled: false });
    await updateResource("Module Bays", "bay", { description: "Only this" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ description: "Only this" });
  });

  it("uses canonical bay-type routes and clears optional references without dropping metadata", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "t", name: "SFP", slug: "sfp" } });
    await createResource("Module Bay Types", { name: "SFP", type: "module_bay_type", slug: "sfp", manufacturerId: null, ownerId: null, tags: [], customFields: {}, comments: "", color: "#155dfc" });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/inventory/module-bay-types", { body: { name: "SFP", slug: "sfp", manufacturer_id: null, owner_id: null, tags: [], custom_fields: {}, comments: "", color: "155dfc" } });
    expect(inventoryTableListPathFor("Module Bay Types")).toBe("/api/v1/inventory/module-bay-types");
    const editing = { id: "t", name: "SFP", type: "module_bay_type", manufacturerId: "m", ownerId: "o", tags: ["tag"] };
    const initial = buildEntityDraft("Module Bay Types", editing);
    apiMocks.PUT.mockResolvedValue({ data: { id: "t", name: "SFP" } });
    await updateResource("Module Bay Types", "t", draftToResourceUpdate("Module Bay Types", { ...initial, manufacturerId: "", ownerId: "", tags: "" }, "Module Bay Type", editing, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ manufacturer_id: null, owner_id: null, tags: [] });
  });

  it("updates only module type membership, including an explicit clear", async () => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "t", model: "SFP" } });
    const resource = { id: "t", name: "SFP", type: "module_type", manufacturerId: "m", moduleBayTypes: [typeId] };
    const initial = buildEntityDraft("Module Types", resource);
    await updateResource("Module Types", "t", draftToResourceUpdate("Module Types", { ...initial, moduleBayTypes: "" }, "Module Type", resource, initial));
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ module_bay_types: [] });
    await updateResource("Module Types", "t", { name: "Renamed" });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ model: "Renamed" });
  });
});

describe("native Interface networking", () => {
  const wire = {
    mtu: 65536, mode: "q-in-q", speed: "9223372036854775807", duplex: "auto", poe_mode: "pse",
    poe_type: "type4-ieee802.3bt", rf_role: "ap", rf_channel: "5g-36-5180-20",
    rf_channel_frequency: 5180.125, rf_channel_width: 20.125, tx_power: -40,
    wwn: "aa:bb:cc:dd:ee:ff:00:11", channel_id: 1024, channels: 1,
    cable_connector: 256, cable_end: "B", cable_positions: [1, 1024],
  };

  it("reads, seeds the existing form and writes all networking wire types on create", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "i", name: "eth0", device_id: "d", ...wire } });
    const resource = await getResource("Interfaces", "i");
    expect(resource).toMatchObject({ mtu: "65536", speed: "9223372036854775807", txPower: "-40", cablePositions: "1, 1024", rfChannelFrequency: "5180.125" });
    const draft = buildEntityDraft("Interfaces", resource);
    apiMocks.POST.mockResolvedValue({ data: { id: "i", name: "eth0", ...wire } });
    const { draftToResource } = await import("@/shared/forms/entityForm");
    await createResource("Interfaces", draftToResource("Interfaces", draft, "Interface"));
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/interfaces", { body: expect.objectContaining(wire) });
    apiMocks.PUT.mockResolvedValue({ data: { id: "i", name: "eth0", ...wire } });
    await updateResource("Interfaces", "i", { speed: draft.speed });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ speed: "9223372036854775807" });
  });

  it("clears values with null, writes positions as an array and omits untouched values", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "i", name: "eth0", device_id: "d", ...wire } });
    const resource = await getResource("Interfaces", "i");
    const initial = buildEntityDraft("Interfaces", resource);
    const draft = { ...initial, mode: "", mtu: "", cablePositions: "2, 3", speed: "0" };
    apiMocks.PUT.mockResolvedValue({ data: { id: "i", name: "eth0" } });
    await updateResource("Interfaces", "i", draftToResourceUpdate("Interfaces", draft, "Interface", resource, initial));
    const body = apiMocks.PUT.mock.lastCall?.[1].body;
    expect(body).toMatchObject({ mode: null, mtu: null, cable_positions: [2, 3], speed: "0" });
    expect(body).not.toHaveProperty("poe_type");
    expect(body).not.toHaveProperty("rf_channel_frequency");
    await updateResource("Interfaces", "i", { cablePositions: null, wwn: null, txPower: null });
    expect(apiMocks.PUT.mock.lastCall?.[1].body).toEqual({ cable_positions: null, wwn: null, tx_power: null });
  });

  it.each([
    { mtu: "0" }, { mtu: "65537" }, { mode: "invalid" }, { speed: "-1" },
    { speed: "9223372036854775808" }, { speed: "1.5" }, { duplex: "invalid" },
    { poeMode: "invalid" }, { poeType: "invalid" }, { rfRole: "invalid" },
    { rfChannelFrequency: "1.0001" }, { rfChannelWidth: "10000" },
    { txPower: "128" }, { txPower: "-41" }, { wwn: "aa:bb" },
    { channelId: "0" }, { channels: "1025" }, { cableConnector: "257" },
    { cableEnd: "C" }, { cablePositions: "1,,2" }, { cablePositions: "1,1025" },
  ])("rejects malformed networking input before a mutation (%j)", async (input) => {
    await expect(updateResource("Interfaces", "i", input)).rejects.toThrow();
    expect(apiMocks.PUT).not.toHaveBeenCalled();
  });
});

describe("template presentation metadata boundary", () => {
  it.each(["active", "draft"])("accepts template status %s", async (status) => {
    apiMocks.GET.mockResolvedValue({ data: { id: "t", name: "Template", status, variables: [], steps: [] } });
    expect((await getTemplate("t")).status).toBe(status);
  });

  it.each(["archived", "", null])("rejects unsupported template status %s", async (status) => {
    apiMocks.GET.mockResolvedValue({ data: { id: "t", name: "Template", status, variables: [], steps: [] } });
    await expect(getTemplate("t")).rejects.toThrow("Unsupported inventory template status");
  });

  it.each([true, false, undefined, "true"])("retains only a boolean system marker (%s)", async (marker) => {
    apiMocks.GET.mockResolvedValue({ data: { id: "t", name: "Template", slug: "template", status: "active", is_system: marker, variables: [], steps: [] } });
    expect((await getTemplate("t")).is_system).toBe(typeof marker === "boolean" ? marker : undefined);
  });
});

describe("wireless inventory", () => {
  it.each([
    ["Wireless LAN Groups", "wireless_lan_groups"],
    ["Wireless LANs", "wireless_lans"],
    ["Wireless Links", "wireless_links"],
  ] as const)("lists and patches %s through its native endpoint", async (entity, path) => {
    apiMocks.GET.mockResolvedValue({ data: { items: [{ id: "w", name: "Campus", ssid: "Campus Wi-Fi", distance: 0, created: "created", last_updated: "updated" }], total: 1, limit: 25, offset: 0 } });
    const page = await listResourcesForTable(entity, { limit: 25, offset: 0 });
    expect(page.items[0]).toMatchObject({ id: "w", createdAt: "created", updatedAt: "updated", distance: "0" });
    if (entity !== "Wireless LAN Groups") expect(page.items[0]?.name).toBe("Campus Wi-Fi");
    expect(apiMocks.GET).toHaveBeenCalledWith(`/api/v1/inventory/${path}`, expect.objectContaining({ params: { query: { limit: 25, offset: 0 } } }));
    apiMocks.PATCH.mockResolvedValue({ data: { id: "w", description: "Changed" } });
    await updateResource(entity, "w", { description: "Changed" });
    expect(apiMocks.PATCH).toHaveBeenCalledWith(`/api/v1/inventory/${path}/{id}`, { params: { path: { id: "w" } }, body: { description: "Changed" } });
  });

  it("preserves nullable FK clears, explicit slug and numeric distances without leaking read metadata", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "g", name: "Campus", slug: "custom-slug" } });
    await createResource("Wireless LAN Groups", { name: "Campus", slug: "custom-slug", type: "wireless_lan_group", parentId: null });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/wireless_lan_groups", { body: { name: "Campus", slug: "custom-slug", parent_id: null } });
    apiMocks.PATCH.mockResolvedValue({ data: { id: "w", ssid: "Bridge" } });
    await updateResource("Wireless Links", "w", { tenantId: null, distance: "12.5", distanceUnit: "m", createdAt: "never" });
    expect(apiMocks.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/wireless_links/{id}", { params: { path: { id: "w" } }, body: { tenant_id: null, distance: 12.5, distance_unit: "m" } });
    await updateResource("Wireless Links", "w", { distance: null });
    expect(apiMocks.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/wireless_links/{id}", { params: { path: { id: "w" } }, body: { distance: null } });
    await updateResource("Wireless LANs", "w", { groupId: null, vlanId: null, scopeType: null, scopeId: null });
    expect(apiMocks.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/wireless_lans/{id}", { params: { path: { id: "w" } }, body: { group_id: null, vlan_id: null, scope_type: null, scope_id: null } });
    await expect(updateResource("Wireless Links", "w", { distance: "Infinity" })).rejects.toThrow("distance must be a finite number");
  });

  it("round-trips interface wireless associations and clears them explicitly", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "i", name: "wlan0", wireless_link_id: "link", wireless_lans: ["lan"] } });
    const resource = await getResource("Interfaces", "i");
    expect(resource).toMatchObject({ wirelessLinkId: "link", wirelessLans: ["lan"] });
    const initial = buildEntityDraft("Interfaces", resource);
    const unchanged = draftToResourceUpdate("Interfaces", initial, "Interface", resource, initial);
    expect(unchanged).not.toHaveProperty("wirelessLinkId");
    expect(unchanged).not.toHaveProperty("wirelessLans");
    apiMocks.POST.mockResolvedValue({ data: { id: "i", name: "wlan0" } });
    await createResource("Interfaces", { name: "wlan0", type: "interface", wirelessLinkId: null, wirelessLans: [] });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/interfaces", { body: { name: "wlan0", wireless_lans: [] } });
  });
});

describe("service explicit IP address bindings", () => {
  it("reads bindings into the owning resource projection", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "s", name: "HTTPS", ipaddresses: ["ip-1", "ip-2"] } });
    expect(await getResource("Services", "s")).toMatchObject({ serviceIpAddressIds: ["ip-1", "ip-2"] });
  });

  it("creates bindings and supports explicit clear without modifying untouched bindings", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "s", name: "HTTPS", ipaddresses: ["ip-1"] } });
    await createResource("Services", { name: "HTTPS", type: "service", serviceIpAddressIds: ["ip-1"] });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/services", { body: { name: "HTTPS", ipaddresses: ["ip-1"] } });
    apiMocks.PUT.mockResolvedValue({ data: { id: "s", name: "HTTPS", ipaddresses: [] } });
    await updateResource("Services", "s", { serviceIpAddressIds: [] });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/services/{id}", { params: { path: { id: "s" } }, body: { ipaddresses: [] } });
    await updateResource("Services", "s", { description: "changed" });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/services/{id}", { params: { path: { id: "s" } }, body: { description: "changed" } });
  });
});

describe.each(["Services", "Service Templates"] as const)("%s mapping payloads", (entity) => {
  const path = entity === "Services" ? "/api/v1/inventory/services" : "/api/v1/inventory/service-templates";
  const mappings = ["tcp/80", "udp/53", "sctp/123"];

  it("reads and writes canonical mappings without legacy fields", async () => {
    const wire = { id: "s", name: "Mixed", port_mappings: mappings, ...(entity === "Services" ? { ipaddresses: ["ip-1"] } : {}) };
    apiMocks.GET.mockResolvedValue({ data: wire });
    const resource = await getResource(entity, "s");
    expect(resource.portMappings).toEqual(mappings);
    apiMocks.POST.mockResolvedValue({ data: wire });
    await createResource(entity, resource);
    expect(apiMocks.POST).toHaveBeenCalledWith(path, { body: { name: "Mixed", port_mappings: mappings, ...(entity === "Services" ? { ipaddresses: ["ip-1"] } : {}) } });
    apiMocks.PUT.mockResolvedValue({ data: wire });
    await updateResource(entity, "s", { portMappings: ["UDP/065535"] });
    expect(apiMocks.PUT).toHaveBeenCalledWith(`${path}/{id}`, { params: { path: { id: "s" } }, body: { port_mappings: ["udp/65535"] } });
  });

  it.each([[], ["tcp/0"], ["tcp/65536"], ["TCP/080", "tcp/80"], ["icmp/80"]].map((portMappings) => ({ portMappings })))("rejects invalid API-bound mappings $portMappings", async ({ portMappings }) => {
    await expect(updateResource(entity, "s", { portMappings })).rejects.toThrow();
    expect(apiMocks.PUT).not.toHaveBeenCalled();
  });
});

describe("native Owner AdminModels", () => {
  it.each(["Owners", "Owner Groups"] as const)("projects %s without unsupported AdminModel metadata", async (entity) => {
    apiMocks.GET.mockResolvedValue({ data: { id: "o", name: "Operations", description: "Team", group_id: "g", users: ["u"], user_groups: ["ug"], tags: ["t"], custom_fields: { x: 1 }, created_at: "today", updated_at: "today" } });
    const result = await getResource(entity, "o");
    expect(result).not.toHaveProperty("customFields");
    expect(result).not.toHaveProperty("createdAt");
    expect(result).not.toHaveProperty("updatedAt");
    if (entity === "Owners") expect(result).toMatchObject({ groupId: "g", users: ["u"], userGroups: ["ug"] });
    else expect(Object.keys(result).sort()).toEqual(["description", "id", "name", "type"]);
  });

  it("creates owners against the native typed route with only supported fields", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "o", name: "Ops", description: "", users: ["u"], user_groups: ["g"] } });
    await createResource("Owners", { name: "Ops", type: "owner", users: ["u"], userGroups: ["g"], customFields: { rejected: true }, createdAt: "never" });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/owners", { body: { name: "Ops", users: ["u"], user_groups: ["g"] } });
  });

  it("omits untouched assignments and preserves explicit empty replacements and group null", async () => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "o", name: "Ops", description: "", users: [], user_groups: [] } });
    apiMocks.PATCH.mockResolvedValue({ data: { id: "o", name: "Ops", description: "", users: [], user_groups: [] } });
    await updateResource("Owners", "o", { name: "Ops" });
    expect(apiMocks.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/owners/{id}", { params: { path: { id: "o" } }, body: { name: "Ops" } });
    const resource = { name: "Ops", users: [], userGroups: [], groupId: null };
    await updateResource("Owners", "o", resource);
    expect(apiMocks.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/owners/{id}", { params: { path: { id: "o" } }, body: { name: "Ops", users: [], user_groups: [], group_id: null } });
  });

  it.each(["Owners", "Owner Groups"] as const)("bulk edits %s without resending a stale record", async (entity) => {
    const stale = { id: "o", name: "Ops", description: "before", group_id: "g", users: ["u"], user_groups: ["ug"] };
    let current = { ...stale, name: "Peer renamed", group_id: "peer-group", users: ["u", "v"], user_groups: ["ug", "peer-ug"] };
    apiMocks.GET.mockResolvedValue({ data: stale });
    const save = (_path: string, options: { body: Partial<typeof stale> }) => {
      current = { ...current, ...options.body };
      return Promise.resolve({ data: current });
    };
    apiMocks.PUT.mockImplementation(save);
    apiMocks.PATCH.mockImplementation(save);
    expect(await bulkUpdateResources(entity, ["o"], { description: "changed" })).toEqual({ updated: ["o"], failed: [] });
    expect(current).toEqual({ ...stale, name: "Peer renamed", description: "changed", group_id: "peer-group", users: ["u", "v"], user_groups: ["ug", "peer-ug"] });
    expect(apiMocks.GET).not.toHaveBeenCalled();
    expect(apiMocks.PUT).not.toHaveBeenCalled();
    expect(apiMocks.PATCH.mock.calls[0]?.[1]?.body).toEqual({ description: "changed" });
  });

  it.each(["Owners", "Owner Groups"] as const)("bulk-clears supported %s scalar fields explicitly", async (entity) => {
    apiMocks.PATCH.mockResolvedValue({ data: { id: "o", name: "Ops", description: "", group_id: null, users: [], user_groups: [] } });
    expect(await bulkUpdateResources(entity, ["o"], { description: null, groupId: null })).toEqual({ updated: ["o"], failed: [] });
    expect(apiMocks.PATCH.mock.calls[0]?.[1]?.body).toEqual(entity === "Owners" ? { description: "", group_id: null } : { description: "" });
    expect(apiMocks.GET).not.toHaveBeenCalled();
  });

  it("creates and deletes flat owner groups without identity assignments", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "g", name: "Ops", description: "" } });
    await createResource("Owner Groups", { name: "Ops", type: "owner_group", users: ["not-allowed"] });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/owner_groups", { body: { name: "Ops" } });
    await deleteResource("Owner Groups", "g");
    expect(apiMocks.DELETE).toHaveBeenCalledWith("/api/v1/inventory/owner_groups/{id}", { params: { path: { id: "g" } } });
  });
});

describe("native VM write projection", () => {
  it.each(["not-a-number", "Infinity", "-Infinity"])("rejects malformed compute input %s instead of clearing it", async (value) => {
    await expect(createResource("Virtual Machines", { type: "vm", name: "VM", vcpus: value })).rejects.toThrow("vcpus must be a finite number");
    await expect(updateResource("Virtual Machines", "vm", { name: "VM", memory: value })).rejects.toThrow("memory must be a finite number");
    await expect(updateResource("Virtual Machine Types", "type", { defaultVcpus: value })).rejects.toThrow("default_vcpus must be a finite number");
    await expect(updateResource("Virtual Machine Types", "type", { defaultMemory: value })).rejects.toThrow("default_memory must be a finite number");
    expect(apiMocks.POST).not.toHaveBeenCalled();
    expect(apiMocks.PUT).not.toHaveBeenCalled();
    expect(apiMocks.PATCH).not.toHaveBeenCalled();
  });
  it("creates a VM using only the complete backend-known payload", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "vm", name: "Native VM" } });
    await createResource("Virtual Machines", { type: "vm", name: "Native VM", slug: "unsupported", siteId: "site", clusterId: "cluster", tenantId: "tenant", roleId: "role", platformId: "platform", virtualMachineTypeId: "type", status: "active", primaryIp: "192.0.2.1", vcpus: "0.25", memory: "0", disk: "8", description: "desc", comments: "comments", customFields: { plain: "kept" }, localContextData: { config: true }, serial: "unsupported", regionId: "unsupported" });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/virtual_machines", { body: { name: "Native VM", site_id: "site", cluster_id: "cluster", tenant_id: "tenant", role_id: "role", platform_id: "platform", virtual_machine_type_id: "type", status: "active", primary_ip: "192.0.2.1", vcpus: 0.25, memory: 0, disk: 8, description: "desc", comments: "comments", custom_fields: { plain: "kept" }, local_context_data: { config: true } } });
  });
  it("preserves omission and sends explicit single-edit compute/platform clears", async () => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "vm", name: "VM" } });
    await updateResource("Virtual Machines", "vm", { name: "VM" });
    expect(apiMocks.PUT.mock.calls[0]?.[1]?.body).toEqual({ name: "VM" });
    await updateResource("Virtual Machines", "vm", { name: "VM", platformId: "", vcpus: "", memory: "", virtualMachineTypeId: "" });
    expect(apiMocks.PUT.mock.calls[1]?.[1]?.body).toEqual({ name: "VM", platform_id: null, vcpus: null, memory: null, virtual_machine_type_id: null });
  });
  it("projects real form clears instead of dropping empty compute/platform fields", async () => {
    const editing = { id: "vm", name: "VM", type: "vm", platformId: "platform", vcpus: "1.5", memory: "2048" };
    const initial = buildEntityDraft("Virtual Machines", editing);
    apiMocks.PUT.mockResolvedValue({ data: editing });
    const draft = { ...initial, platformId: "", vcpus: "", memory: "" };
    await updateResource("Virtual Machines", "vm", draftToResourceUpdate("Virtual Machines", draft, "VM", editing, initial));
    expect(apiMocks.PUT.mock.calls[0]?.[1]?.body).toEqual({ name: "VM", platform_id: null, vcpus: null, memory: null });
    await updateResource("Virtual Machines", "vm", draftToResourceUpdate("Virtual Machines", initial, "VM", editing, initial));
    expect(apiMocks.PUT.mock.calls[1]?.[1]?.body).toEqual({ name: "VM" });
  });
  it("bulk edits use the same projection and do not resend omitted compute fields", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "vm", name: "VM", platform_id: "platform", vcpus: 1.5, memory: 2048 } });
    apiMocks.PUT.mockResolvedValue({ data: { id: "vm", name: "VM" } });
    expect(await bulkUpdateResources("Virtual Machines", ["vm"], { description: "changed" })).toEqual({ updated: ["vm"], failed: [] });
    expect(apiMocks.PUT.mock.calls[0]?.[1]?.body).toEqual({ name: "VM", description: "changed" });
    expect(await bulkUpdateResources("Virtual Machines", ["vm"], { platformId: null, vcpus: null, memory: null })).toEqual({ updated: ["vm"], failed: [] });
    expect(apiMocks.PUT.mock.calls[1]?.[1]?.body).toEqual({ name: "VM", platform_id: null, vcpus: null, memory: null });
    await bulkUpdateResources("Virtual Machines", ["vm"], { vcpus: "0.25", memory: "0" });
    expect(apiMocks.PUT.mock.calls[2]?.[1]?.body).toEqual({ name: "VM", vcpus: 0.25, memory: 0 });
  });
  it("preserves the VM's other PUT attributes without resending masked custom fields", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "vm", name: "VM", site_id: "site", cluster_id: "cluster", tenant_id: "tenant", role_id: "role", status: "active", disk: 8, description: "before", comments: "kept", custom_fields: { protected: "****" }, local_context_data: { config: true }, vcpus: 1.5, memory: 2048 } });
    apiMocks.PUT.mockResolvedValue({ data: { id: "vm", name: "VM" } });
    await bulkUpdateResources("Virtual Machines", ["vm"], { description: "changed" });
    expect(apiMocks.PUT.mock.calls[0]?.[1]?.body).toEqual({ name: "VM", site_id: "site", cluster_id: "cluster", tenant_id: "tenant", role_id: "role", status: "active", disk: 8, description: "changed", comments: "kept", local_context_data: { config: true } });
  });
});

describe("virtual machine types", () => {
  it("maps the hydrated VM type name together with its edit UUID", async () => {
    apiMocks.GET.mockResolvedValue({data:{items:[{id:"vm-1", name:"VM", virtual_machine_type_id:"type-1", virtual_machine_type_name:"Small VM"}], total:1, limit:25, offset:0}});
    const result = await listResourcesForTable("Virtual Machines", {limit:25,offset:0});
    expect(result.items[0]).toMatchObject({virtualMachineTypeId:"type-1",virtualMachineType:"Small VM"});
  });
  it("round-trips fractional defaults, independent slug and zero memory", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "type-1", name: "Small", slug: "stable-slug", default_vcpus: 0.25, default_memory: 0, default_platform_id: "platform-1", virtual_machine_count: 0 } });
    const resource = await createResource("Virtual Machine Types", { type: "virtual_machine_type", name: "Small", slug: "stable-slug", defaultVcpus: "0.25", defaultMemory: "0", defaultPlatformId: "platform-1" });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/virtual-machine-types", { body: { name: "Small", slug: "stable-slug", default_vcpus: 0.25, default_memory: 0, default_platform_id: "platform-1" } });
    expect(resource).toMatchObject({ slug: "stable-slug", defaultVcpus: "0.25", defaultMemory: "0", defaultPlatformId: "platform-1", virtualMachineCount: "0" });
  });

  it("clears empty defaults rather than retaining their previous values", async () => {
    apiMocks.PATCH.mockResolvedValue({ data: { id: "type-1", name: "Small", slug: "stable-slug" } });
    await updateResource("Virtual Machine Types", "type-1", { name: "Small", slug: "stable-slug", defaultVcpus: "", defaultMemory: "", defaultPlatformId: "" });
    expect(apiMocks.PATCH).toHaveBeenCalledWith("/api/v1/inventory/virtual-machine-types/{id}", { params: { path: { id: "type-1" } }, body: { name: "Small", slug: "stable-slug", default_vcpus: null, default_memory: null, default_platform_id: null } });
  });

  it("omits untouched defaults on PATCH", async () => {
    apiMocks.PATCH.mockResolvedValue({ data: { id: "type-1", name: "Small", slug: "stable-slug" } });
    await updateResource("Virtual Machine Types", "type-1", { description: "changed" });
    expect(apiMocks.PATCH.mock.calls[0]?.[1].body).toEqual({ description: "changed" });
  });

  it("bulk-clears type defaults without reading and resending stale fields", async () => {
    apiMocks.PATCH.mockResolvedValue({ data: { id: "type-1", name: "Small", slug: "small", default_memory: 512 } });
    expect(await bulkUpdateResources("Virtual Machine Types", ["type-1"], { defaultVcpus: null })).toEqual({ updated: ["type-1"], failed: [] });
    expect(apiMocks.GET).not.toHaveBeenCalled();
    expect(apiMocks.PATCH.mock.calls[0]?.[1].body).toEqual({ default_vcpus: null });
  });

  it("preserves omitted VM type and sends explicit null when its picker is cleared", async () => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "vm-1", name: "VM" } });
    await updateResource("Virtual Machines", "vm-1", { name: "VM" });
    expect(apiMocks.PUT.mock.calls[0]?.[1].body).not.toHaveProperty("virtual_machine_type_id");
    await updateResource("Virtual Machines", "vm-1", { name: "VM", virtualMachineTypeId: "" });
    expect(apiMocks.PUT.mock.calls[1]?.[1].body).toHaveProperty("virtual_machine_type_id", null);
    await updateResource("Virtual Machines", "vm-1", { name: "VM", virtualMachineTypeId: "type-1" });
    expect(apiMocks.PUT.mock.calls[2]?.[1].body).toHaveProperty("virtual_machine_type_id", "type-1");
  });
});

describe("buildInventoryTableParams", () => {
  it("passes limit/offset and trims search", () => {
    expect(buildInventoryTableParams({ limit: 25, offset: 50, search: "  cash  " })).toEqual({
      limit: 25,
      offset: 50,
      search: "cash",
    });
  });

  it("omits blank/whitespace search", () => {
    expect(buildInventoryTableParams({ limit: 25, offset: 0, search: "   " })).toEqual({
      limit: 25,
      offset: 0,
    });
  });

  it("forwards sort with default asc order", () => {
    expect(buildInventoryTableParams({ limit: 25, offset: 0, sort: "device_type" })).toEqual({
      limit: 25,
      offset: 0,
      sort: "device_type",
      order: "asc",
    });
  });

  it("forwards explicit desc order", () => {
    expect(
      buildInventoryTableParams({ limit: 25, offset: 0, sort: "name", order: "desc" }),
    ).toMatchObject({ sort: "name", order: "desc" });
  });

  it("never emits an order without a sort key", () => {
    const params = buildInventoryTableParams({ limit: 25, offset: 0, order: "desc" });
    expect(params).not.toHaveProperty("sort");
    expect(params).not.toHaveProperty("order");
  });

  it("comma-joins multi-value filters and drops empty lists", () => {
    expect(
      buildInventoryTableParams({
        limit: 25,
        offset: 0,
        filters: { status: ["active", "planned"], site_id: ["a", "b"], tenant_id: [] },
      }),
    ).toEqual({
      limit: 25,
      offset: 0,
      status: "active,planned",
      site_id: "a,b",
    });
  });
});

describe("listResourcesForTable", () => {
  it("sends sort + comma-joined filters to the devices route and returns the full envelope", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [{ id: "dev-1", name: "Cash 1", status: "active" }],
        total: 42,
        limit: 25,
        offset: 0,
      },
    });

    const result = await listResourcesForTable("Devices", {
      limit: 25,
      offset: 0,
      sort: "device_type",
      order: "desc",
      filters: { status: ["active"], site_id: ["s1", "s2"] },
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/inventory/devices", {
      params: {
        query: {
          limit: 25,
          offset: 0,
          sort: "device_type",
          order: "desc",
          status: "active",
          site_id: "s1,s2",
        },
      },
    });
    expect(result.total).toBe(42);
    expect(result.items).toHaveLength(1);
    expect(result.items[0]).toMatchObject({ id: "dev-1", name: "Cash 1" });
  });

  const standardizedInventoryTables = tabConfig
    .map((tab) => tab.key)
    .filter((entity) => {
      const config = inventoryTableConfigFor(entity);
      return config.sortKeys.length > 0 || config.filterColumns.length > 0;
    });

  it("keeps table list routes in exact parity with standardized inventory tables", () => {
    expect([...inventoryTableListPathEntities()].sort()).toEqual(
      [...standardizedInventoryTables].sort(),
    );
  });

  it.each(standardizedInventoryTables)(
    "uses the sort/filter-aware route for every standardized table: %s",
    async (entity) => {
      const config = inventoryTableConfigFor(entity);
      const path = inventoryTableListPathFor(entity);
      expect(path, `${entity}: missing ENTITY_LIST_PATH entry`).toBeTruthy();

      const filters = Object.fromEntries(
        config.filterColumns.slice(0, 2).map((column, index) => [
          column.param,
          [`value-${index}`],
        ]),
      );

      apiMocks.GET.mockResolvedValue({
        data: { items: [], total: 0, limit: 25, offset: 0 },
      });
      if (entity === "Inventory Item Templates") {
        // The read path preloads the content-type catalog before mapping the
        // native component_type_id/component_id pair.
        apiMocks.GET.mockResolvedValueOnce({ data: [] });
      }

      await listResourcesForTable(entity, {
        limit: 25,
        offset: 0,
        sort: config.sortKeys[0],
        order: "asc",
        filters,
      });

      expect(apiMocks.GET).toHaveBeenCalledWith(path, {
        params: {
          query: {
            limit: 25,
            offset: 0,
            ...(config.sortKeys[0] ? { sort: config.sortKeys[0], order: "asc" } : {}),
            ...Object.fromEntries(
              Object.entries(filters).map(([param, values]) => [param, values.join(",")]),
            ),
          },
        },
      });
    },
  );

  it("keeps the devices route contract explicit", async () => {
    apiMocks.GET.mockResolvedValue({
      data: { items: [], total: 0, limit: 25, offset: 0 },
    });

    await listResourcesForTable("Devices", {
      limit: 25,
      offset: 0,
      sort: "device_type",
      order: "asc",
      filters: { status: ["planned", "offline"], site_id: ["s1"] },
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/inventory/devices", {
      params: {
        query: {
          limit: 25,
          offset: 0,
          sort: "device_type",
          order: "asc",
          status: "planned,offline",
          site_id: "s1",
        },
      },
    });
  });

  it("falls back to the raw parent id when site-group rows are not name-hydrated", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [{ id: "group-1", name: "North", slug: "north", parent_id: "parent-1" }],
        total: 1,
        limit: 25,
        offset: 0,
      },
    });

    const result = await listResourcesForTable("Site Groups", { limit: 25, offset: 0 });

    expect(result.items[0]).toMatchObject({ parentId: "parent-1", parentName: "parent-1" });
  });

  it("falls back to the raw vrf id when IP-address rows are not name-hydrated", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [{ id: "ip-1", address: "10.0.0.1/24", vrf_id: "vrf-1" }],
        total: 1,
        limit: 25,
        offset: 0,
      },
    });

    const result = await listResourcesForTable("IP Addresses", { limit: 25, offset: 0 });

    expect(result.items[0]).toMatchObject({ vrfId: "vrf-1", vrfName: "vrf-1" });
  });
});

describe("listAllResources", () => {
  it.each(["list", "shared options"])("walks all pages so FK filter options are not truncated to the first page (%s)", async (consumer) => {
    apiMocks.GET
      .mockResolvedValueOnce({
        data: {
          items: [
            { id: "dev-1", name: "Cash 1" },
            { id: "dev-2", name: "Cash 2" },
          ],
          total: 3,
          limit: 2,
          offset: 0,
        },
      })
      .mockResolvedValueOnce({
        data: {
          items: [{ id: "dev-3", name: "Cash 3" }],
          total: 3,
          limit: 2,
          offset: 2,
        },
      });

    const result = consumer === "list" ? await listAllResources("Devices") : (await new QueryClient().fetchQuery(inventoryResourceOptionsQuery("Devices"))).items;

    expect(apiMocks.GET).toHaveBeenNthCalledWith(1, "/api/v1/inventory/devices", {
      params: { query: { limit: 1000, offset: 0, search: undefined } },
    });
    expect(apiMocks.GET).toHaveBeenNthCalledWith(2, "/api/v1/inventory/devices", {
      params: { query: { limit: 1000, offset: 2, search: undefined } },
    });
    expect(result.map((resource) => resource.id)).toEqual(["dev-1", "dev-2", "dev-3"]);
  });
});

describe("inventory slugify", () => {
  it("transliterates cyrillic names instead of collapsing them to a shared fallback", () => {
    expect(slugify("Площадка Бегемота")).toBe("ploschadka-begemota");
  });

  it("keeps latin slugs stable", () => {
    expect(slugify("Edge Site 01")).toBe("edge-site-01");
  });
});

describe("inventory cluster payloads", () => {
  it("sends writable status, notes, and custom fields when updating a cluster", async () => {
    apiMocks.PUT.mockResolvedValue({
      data: {
        id: "cluster-1",
        name: "Cluster A",
        status: "offline",
        cluster_type_id: "cluster-type-1",
        description: "Updated description",
        comments: "Updated comments",
      },
    });

    await updateResource("Clusters", "cluster-1", {
      name: "Cluster A",
      type: "cluster",
      clusterTypeId: "cluster-type-1",
      status: "offline",
      description: "Updated description",
      comments: "Updated comments",
      customFields: { owner: "platform" },
    });

    expect(apiMocks.PUT).toHaveBeenCalledWith("/api/v1/inventory/clusters/{id}", {
      params: { path: { id: "cluster-1" } },
      body: {
        name: "Cluster A",
        cluster_type_id: "cluster-type-1",
        status: "offline",
        description: "Updated description",
        comments: "Updated comments",
        custom_fields: { owner: "platform" },
      },
    });
  });
});

describe("custom field entity kind mapping", () => {
  it("keeps frontend reveal path segments in sync with supported inventory entities", () => {
    expect(customFieldEntityKind("Sites")).toBe("sites");
    expect(customFieldEntityKind("Devices")).toBe("devices");
    expect(customFieldEntityKind("Regions")).toBe("regions");
    expect(customFieldEntityKind("Clusters")).toBe("clusters");
    expect(customFieldEntityKind("Tenants")).toBe("tenants");
    expect(customFieldEntityKind("Virtual Machines")).toBe("virtual-machines");
  });
});

describe("inventory item projection and payloads", () => {
  it("projects every Inventory Items field, stringifying discovered and nesting component", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [
          {
            id: "item-1",
            name: "PSU 1",
            label: "Power supply",
            part_id: "part-1",
            serial: "SN-1",
            asset_tag: "AT-1",
            discovered: true,
            status: "active",
            device_id: "dev-1",
            parent_id: "item-parent",
            role_id: "role-1",
            manufacturer_id: "mfr-1",
            description: "desc",
            component: { object_type: "dcim.interface", object_id: "if-1", label: "eth0" },
          },
        ],
        total: 1,
        limit: 25,
        offset: 0,
      },
    });

    const result = await listResources("Inventory Items", { limit: 25, offset: 0 });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/inventory/inventory-items", {
      params: { query: { limit: 25, offset: 0, search: undefined } },
    });
    expect(result.items[0]).toMatchObject({
      id: "item-1",
      name: "PSU 1",
      label: "Power supply",
      partId: "part-1",
      serial: "SN-1",
      assetTag: "AT-1",
      discovered: "true",
      status: "active",
      deviceId: "dev-1",
      parentId: "item-parent",
      roleId: "role-1",
      manufacturerId: "mfr-1",
      description: "desc",
      component: { objectType: "dcim.interface", objectId: "if-1", label: "eth0" },
    });
  });

  it("sends device_id when creating an Inventory Item", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "item-1", name: "PSU 1" } });

    await createResource("Inventory Items", {
      name: "PSU 1",
      type: "inventory_item",
      deviceId: "dev-1",
      discovered: "true",
      component: { objectType: "dcim.interface", objectId: "if-1", label: null },
    });

    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/inventory-items", {
      body: {
        device_id: "dev-1",
        name: "PSU 1",
        discovered: true,
        component: { object_type: "dcim.interface", object_id: "if-1" },
      },
    });
  });

  it("omits device_id when updating an Inventory Item (update schema denies unknown fields)", async () => {
    apiMocks.PUT.mockResolvedValue({ data: { id: "item-1", name: "PSU 1" } });

    await updateResource("Inventory Items", "item-1", {
      name: "PSU 1",
      deviceId: "dev-1",
      discovered: "false",
    });

    expect(apiMocks.PUT).toHaveBeenCalledWith("/api/v1/inventory/inventory-items/{id}", {
      params: { path: { id: "item-1" } },
      body: { name: "PSU 1", discovered: false },
    });
  });
});

describe("module type profiles", () => {
  it("round-trips ModuleType profile attributes and preserves omission versus clearing", async () => {
    apiMocks.GET.mockResolvedValue({ data: { id: "mt", model: "Fiber", manufacturer_id: "m", profile_id: "p", attribute_data: { ports: 4 } } });
    const loaded = await getResource("Module Types", "mt");
    expect(loaded).toMatchObject({ profileId: "p", attributeDataSummary: '{"ports":4}' });
    apiMocks.PUT.mockResolvedValue({ data: { id: "mt", model: "Fiber" } });
    await updateResource("Module Types", "mt", { profileId: "p", attributeDataSummary: '{"ports":8}' });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/module-types/{id}", { params: { path: { id: "mt" } }, body: { profile_id: "p", attribute_data: { ports: 8 } } });
    await updateResource("Module Types", "mt", { name: "Renamed" });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/module-types/{id}", { params: { path: { id: "mt" } }, body: { model: "Renamed" } });
    await updateResource("Module Types", "mt", { profileId: null, attributeDataSummary: null });
    expect(apiMocks.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/module-types/{id}", { params: { path: { id: "mt" } }, body: { profile_id: null, attribute_data: null } });
  });
  it("projects the profile with a readable schema summary", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [
          {
            id: "profile-1",
            name: "Fiber module",
            description: "Attributes for fiber modules",
            comments: "managed",
            schema: { properties: { wavelength: { type: "integer", required: true } } },
            owner_id: "owner-1",
          },
        ],
        total: 1,
        limit: 25,
        offset: 0,
      },
    });

    const result = await listResources("Module Type Profiles", { limit: 25, offset: 0 });

    expect(apiMocks.GET).toHaveBeenCalledWith(
      "/api/v1/inventory/module-type-profiles",
      { params: { query: { limit: 25, offset: 0, search: undefined } } },
    );
    expect(result.items[0]).toMatchObject({
      id: "profile-1",
      name: "Fiber module",
      description: "Attributes for fiber modules",
      comments: "managed",
      ownerId: "owner-1",
    });
    expect(result.items[0]?.schemaSummary).toBe(
      JSON.stringify({ properties: { wavelength: { type: "integer", required: true } } }),
    );
  });

  it("creates, patches and deletes through native routes without phantom fields", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "profile-1", name: "Fiber" } });
    apiMocks.PATCH.mockResolvedValue({ data: { id: "profile-1", name: "Fiber" } });
    apiMocks.DELETE.mockResolvedValue({ data: undefined });
    await createResource("Module Type Profiles", { name: "Fiber", type: "module_type_profile", schemaSummary: '{"properties":{}}', ownerId: "owner-1" });
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/inventory/module-type-profiles", { body: { name: "Fiber", schema: { properties: {} }, owner_id: "owner-1" } });
    await updateResource("Module Type Profiles", "profile-1", { description: "changed" });
    expect(apiMocks.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/module-type-profiles/{id}", { params: { path: { id: "profile-1" } }, body: { description: "changed" } });
    await updateResource("Module Type Profiles", "profile-1", { schemaSummary: "", ownerId: null });
    expect(apiMocks.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/module-type-profiles/{id}", { params: { path: { id: "profile-1" } }, body: { schema: null, owner_id: null } });
    await deleteResource("Module Type Profiles", "profile-1");
    expect(apiMocks.DELETE).toHaveBeenCalledWith("/api/v1/inventory/module-type-profiles/{id}", { params: { path: { id: "profile-1" } } });
    expect(apiMocks.PUT).not.toHaveBeenCalled();
  });
  it.each(["[]", "true", "42", "broken"])("rejects invalid schema %s before a network mutation", async schemaSummary => {
    await expect(createResource("Module Type Profiles", { name: "Fiber", type: "module_type_profile", schemaSummary })).rejects.toThrow();
    expect(apiMocks.POST).not.toHaveBeenCalled();
  });
  // Bodyless DELETE failure propagation is covered by the real-transport
  // `apiDelete.test.ts` (204 resolves, 409 rejects). Here the client is mocked,
  // so a synthetic `{ error }` never passes through an unwrap.
});

describe("imported tag and service projections", () => {
  it("projects canonical service endpoints without inventing an optional protocol", async () => {
    apiMocks.GET.mockResolvedValue({ data: { items: [{
      id: "service-1", name: "Multi-protocol service",
      port_mappings: ["tcp/443", "udp/53"],
      parent_object: { object_type: "dcim.device", object_id: "device-1", label: "Router" },
    }], total: 1, limit: 25, offset: 0 } });
    const result = await listResources("Services", { limit: 25, offset: 0 });
    expect(result.items[0]).toMatchObject({ name: "Multi-protocol service", portMappings: ["tcp/443", "udp/53"] });
    expect(result.items[0]).not.toHaveProperty("protocol");
    expect(result.items[0]).not.toHaveProperty("ports");
  });

  it("lists native tag definitions", async () => {
    apiMocks.GET.mockResolvedValue({ data: { items: [{
      id: "tag-1", name: "Core", slug: "core", color: "ff0000", weight: 100, description: "Managed tag", object_types: [],
    }], total: 1, limit: 25, offset: 0 } });
    const result = await listResources("Tags", { limit: 25, offset: 0 });
    expect(result.items[0]).toMatchObject({ name: "Core", slug: "core", color: "ff0000", weight: "100", objectTypes: [] });
    expect(apiMocks.POST).not.toHaveBeenCalled();
    expect(apiMocks.PUT).not.toHaveBeenCalled();
    expect(apiMocks.DELETE).not.toHaveBeenCalled();
  });
});
