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

describe("template presentation metadata boundary", () => {
  it.each([true, false, undefined, "true"])("retains only a boolean system marker (%s)", async (marker) => {
    apiMocks.GET.mockResolvedValue({ data: { id: "t", name: "Template", slug: "template", status: "active", is_system: marker, variables: [], steps: [] } });
    expect((await getTemplate("t")).is_system).toBe(typeof marker === "boolean" ? marker : undefined);
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
    .filter((entity) => inventoryTableConfigFor(entity).sortKeys.length > 0);

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
            sort: config.sortKeys[0],
            order: "asc",
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
  it("walks all pages so FK filter options are not truncated to the first page", async () => {
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

    const result = await listAllResources("Devices");

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

describe("module type profiles (read-only)", () => {
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

  it("rejects every write because the backend exposes no write routes", async () => {
    await expect(
      createResource("Module Type Profiles", { name: "x", type: "module_type_profile" }),
    ).rejects.toThrow(/read-only/);
    await expect(
      updateResource("Module Type Profiles", "profile-1", { name: "y" }),
    ).rejects.toThrow(/read-only/);
    await expect(deleteResource("Module Type Profiles", "profile-1")).rejects.toThrow(
      /read-only/,
    );
    expect(apiMocks.POST).not.toHaveBeenCalled();
    expect(apiMocks.PUT).not.toHaveBeenCalled();
    expect(apiMocks.DELETE).not.toHaveBeenCalled();
  });
});

describe("imported tag and service projections", () => {
  it("projects canonical service endpoints without inventing an optional protocol", async () => {
    apiMocks.GET.mockResolvedValue({ data: { items: [{
      id: "service-1", name: "Multi-protocol service", protocol: null, ports: null,
      port_mappings: ["tcp/443", "udp/53"],
      parent_object: { object_type: "dcim.device", object_id: "device-1", label: "Router" },
    }], total: 1, limit: 25, offset: 0 } });
    const result = await listResources("Services", { limit: 25, offset: 0 });
    expect(result.items[0]).toMatchObject({ name: "Multi-protocol service", portMappings: "tcp/443, udp/53" });
    expect(result.items[0]?.protocol).toBeUndefined();
    expect(result.items[0]?.ports).toBeUndefined();
  });

  it("lists native tag definitions and rejects writes", async () => {
    apiMocks.GET.mockResolvedValue({ data: { items: [{
      id: "tag-1", name: "Core", slug: "core", color: "ff0000", weight: 100, description: "Managed tag",
    }], total: 1, limit: 25, offset: 0 } });
    const result = await listResources("Tags", { limit: 25, offset: 0 });
    expect(result.items[0]).toMatchObject({ name: "Core", slug: "core", color: "ff0000", weight: "100" });
    await expect(createResource("Tags", { name: "x", type: "tag" })).rejects.toThrow(/read-only/);
    await expect(updateResource("Tags", "tag-1", { name: "y" })).rejects.toThrow(/read-only/);
    await expect(deleteResource("Tags", "tag-1")).rejects.toThrow(/read-only/);
    expect(apiMocks.POST).not.toHaveBeenCalled();
    expect(apiMocks.PUT).not.toHaveBeenCalled();
    expect(apiMocks.DELETE).not.toHaveBeenCalled();
  });
});
