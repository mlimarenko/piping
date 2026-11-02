import { beforeEach, describe, expect, it, vi } from "vitest";
import { buildEntityDraft, draftToResource, validateEntityDraft } from "@/shared/forms/entityForm";
import { descriptorFromSpec } from "@/shared/nested-editor/descriptorFromSpec";
import { relationshipsByTab } from "./components/inventory/inventoryTypes";

const api = vi.hoisted(() => ({ GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() }));
vi.mock("@/shared/api/client", () => ({ apiClient: api, unwrap: (r: { data?: unknown; error?: unknown }) => {
  if (r.error) throw r.error instanceof Error ? r.error : new Error("API error");
  return r.data;
} }));
import { bulkUpdateResources, createResource, getResource, updateResource, deleteResource, listResources } from "./api";
import { inventoryResourceDisplayName, inventoryResourceOption } from "./resourceOptions";
import { fhrpAuthKeyIntent, revealFhrpAuthKey } from "./fhrp-auth-key";

const groupId = "a0000000-0000-4000-8000-000000000001";
const interfaceId = "a0000000-0000-4000-8000-000000000002";
const t = (key: string) => key;
beforeEach(() => { Object.values(api).forEach(mock => mock.mockReset()); });

describe("FHRP inventory workflow", () => {
  it("keeps an existing empty name through fetch, edit and read-merge-PUT without persisting its display label", async () => {
    const row = { id: groupId, name: "", protocol: "vrrp3", group_id: 10, description: "" };
    api.GET.mockResolvedValue({ data: row });
    api.PUT.mockResolvedValue({ data: row });
    const resource = await getResource("FHRP Groups", groupId);
    expect(resource.name).toBe("");
    expect(inventoryResourceDisplayName(resource)).toBe("vrrp3 10");
    expect(inventoryResourceOption("FHRP Groups", resource).name).toBe("vrrp3 10");
    const draft = buildEntityDraft("FHRP Groups", resource);
    expect(draft.name).toBe("");
    draft.description = "Edited notes";
    await updateResource("FHRP Groups", groupId, draftToResource("FHRP Groups", draft, "FHRP Group", resource));
    expect(api.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/fhrp-groups/{id}", expect.objectContaining({ body: expect.objectContaining({ name: "", description: "Edited notes" }) }));
    expect(await bulkUpdateResources("FHRP Groups", [groupId], { description: "Bulk notes" })).toEqual({ updated: [groupId], failed: [] });
    expect(api.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/fhrp-groups/{id}", expect.objectContaining({ body: expect.objectContaining({ name: "", description: "Bulk notes" }) }));
    draft.name = "Named gateway";
    await updateResource("FHRP Groups", groupId, draftToResource("FHRP Groups", draft, "FHRP Group", resource));
    expect(api.PUT).toHaveBeenLastCalledWith("/api/v1/inventory/fhrp-groups/{id}", expect.objectContaining({ body: expect.objectContaining({ name: "Named gateway" }) }));
  });
  it("creates an unnamed group using its group number as identity", async () => {
    const draft = { protocol: "vrrp3", fhrpGroupNumber: "10" };
    expect(validateEntityDraft("FHRP Groups", draft, t)).toEqual({});
    api.POST.mockResolvedValue({ data: { id: groupId, name: "", protocol: "vrrp3", group_id: 10 } });
    await createResource("FHRP Groups", draftToResource("FHRP Groups", draft, "FHRP Group"));
    expect(api.POST).toHaveBeenCalledWith("/api/v1/inventory/fhrp-groups", { body: expect.objectContaining({ name: "", protocol: "vrrp3", group_id: 10 }) });
  });
  it("keeps ordinary reads and unrelated edits free of plaintext keys", async () => {
    const row = { id: groupId, name: "Gateway", protocol: "vrrp3", group_id: 0, auth_type: "plaintext", owner_id: null, custom_fields: { note: "edge" }, tags: ["edge"], external_id: 10, piping_synced_at: null, created: "2026-10-26T00:00:00Z", last_updated: "2026-10-26T01:00:00Z" };
    api.GET.mockResolvedValue({ data: row });
    const resource = await getResource("FHRP Groups", groupId);
    expect(resource).toMatchObject({ fhrpGroupNumber: "0", protocol: "vrrp3", authType: "plaintext", customFields: { note: "edge" }, createdAt: row.created, updatedAt: row.last_updated });
    expect(resource).not.toHaveProperty("authKey");
    expect(resource).not.toHaveProperty("authKeyIntent");
    const draft = buildEntityDraft("FHRP Groups", resource);
    expect(validateEntityDraft("FHRP Groups", draft, t)).toEqual({});
    api.PUT.mockResolvedValue({ data: row });
    await updateResource("FHRP Groups", groupId, draftToResource("FHRP Groups", draft, "FHRP Group", resource));
    expect(api.PUT).toHaveBeenCalledWith(`/api/v1/inventory/fhrp-groups/{id}`, expect.objectContaining({ body: expect.objectContaining({ group_id: 0, auth_type: "plaintext", owner_id: null, custom_fields: { note: "edge" }, tags: ["edge"], external_id: 10, piping_synced_at: null }) }));
    expect(api.PUT.mock.lastCall?.[1].body).not.toHaveProperty("auth_key");
    api.GET.mockResolvedValue({ data: { items: [{ ...row, auth_key: "unexpected-plaintext" }], total: 1, limit: 25, offset: 0 } });
    const listed = await listResources("FHRP Groups");
    expect(listed.items[0]).not.toHaveProperty("authKey");
    expect(JSON.stringify(listed.items[0])).not.toContain("unexpected-plaintext");
    expect(api.POST).not.toHaveBeenCalled();
  });

  it.each(["set", "rotate", "clear"] as const)("sends only explicit %s intent from the editor", async intent => {
    const draft = buildEntityDraft("FHRP Groups");
    Object.assign(draft, { protocol: "vrrp3", fhrpGroupNumber: "10", authKeyIntent__intent: intent, authKeyIntent: "synthetic-key" });
    api.POST.mockResolvedValue({ data: { id: groupId, group_id: 10 } });
    await createResource("FHRP Groups", draftToResource("FHRP Groups", draft, "FHRP Group"));
    expect(api.POST.mock.lastCall?.[1].body.auth_key).toEqual(intent === "clear" ? { intent } : { intent, value: "synthetic-key" });
    api.GET.mockResolvedValue({ data: { id: groupId, name: "", protocol: "vrrp3", group_id: 10 } });
    api.PUT.mockResolvedValue({ data: { id: groupId, group_id: 10 } });
    await updateResource("FHRP Groups", groupId, draftToResource("FHRP Groups", draft, "FHRP Group"));
    expect(api.PUT.mock.lastCall?.[1].body.auth_key).toEqual(intent === "clear" ? { intent } : { intent, value: "synthetic-key" });
  });

  it("validates structured secret intents instead of accepting plaintext or ambiguous clears", () => {
    for (const input of ["", "****", { intent: "preserve" }, { intent: "rotate" }, { intent: "clear", value: "****" }, { intent: "set", secret_id: groupId }]) {
      expect(fhrpAuthKeyIntent.safeParse(input).success).toBe(false);
    }
    expect(validateEntityDraft("FHRP Groups", { protocol: "vrrp3", fhrpGroupNumber: "10", authKeyIntent__intent: "set" }, t)).toHaveProperty("authKeyIntent");
  });

  it("reveals only on explicit request and propagates denial", async () => {
    api.POST.mockResolvedValue({ data: { value: "synthetic-reveal" } });
    expect(await revealFhrpAuthKey(groupId)).toBe("synthetic-reveal");
    expect(api.POST).toHaveBeenCalledWith("/api/v1/inventory/fhrp-groups/{id}/auth-key/reveal", { params: { path: { id: groupId } } });
    api.POST.mockResolvedValue({ error: new Error("Forbidden") });
    await expect(revealFhrpAuthKey(groupId)).rejects.toThrow("Forbidden");
    api.POST.mockResolvedValue({ data: { value: 42 } });
    await expect(revealFhrpAuthKey(groupId)).rejects.toThrow();
  });

  it.each(["dcim.interface", "virtualization.vminterface"])("creates and reads assignments to %s", async token => {
    const row = { id: "assignment", group_id: groupId, interface_type: token, interface_id: interfaceId, priority: 0 };
    api.POST.mockResolvedValue({ data: row });
    const draft = { groupId, fhrpInterface: interfaceId, fhrpInterface__type: token, priority: "0" };
    expect(validateEntityDraft("FHRP Group Assignments", draft, t)).toEqual({});
    const result = await createResource("FHRP Group Assignments", draftToResource("FHRP Group Assignments", draft, "Assignment"));
    expect(api.POST).toHaveBeenCalledWith("/api/v1/inventory/fhrp-group-assignments", { body: rowWithoutId(row) });
    expect(result).toMatchObject({ groupId, priority: "0", fhrpInterface: { objectType: token, objectId: interfaceId } });
  });

  it.each(["-1", "256", "1.5", "NaN"])("rejects invalid assignment priority %s", async priority => {
    expect(validateEntityDraft("FHRP Group Assignments", { groupId, fhrpInterface: interfaceId, fhrpInterface__type: "dcim.interface", priority }, t)).toHaveProperty("priority");
    await expect(createResource("FHRP Group Assignments", { name: "", type: "fhrp_group_assignment", priority })).rejects.toThrow();
    expect(api.POST).not.toHaveBeenCalled();
  });

  it.each(["-1", "32768", "1.5"])("rejects invalid group number %s", async fhrpGroupNumber => {
    await expect(createResource("FHRP Groups", { name: "Gateway", type: "fhrp_group", protocol: "vrrp3", fhrpGroupNumber })).rejects.toThrow();
    expect(api.POST).not.toHaveBeenCalled();
  });

  it("lists and deletes through the owning endpoints, propagating delete failures", async () => {
    api.GET.mockResolvedValue({ data: { items: [], total: 0, limit: 25, offset: 0 } });
    expect((await listResources("FHRP Groups")).items).toEqual([]);
    expect(api.GET).toHaveBeenCalledWith("/api/v1/inventory/fhrp-groups", expect.anything());
    // The real client middleware rejects non-2xx responses before returning a result.
    api.DELETE.mockRejectedValue(new Error("Group has assignments"));
    await expect(deleteResource("FHRP Groups", groupId)).rejects.toThrow("Group has assignments");
  });
});

describe("IP polymorphic assignments", () => {
  it("writes an FHRP assignment and clears it explicitly", async () => {
    api.POST.mockResolvedValue({ data: { id: "ip", address: "192.0.2.1/24" } });
    await createResource("IP Addresses", { name: "192.0.2.1/24", type: "ip_address", assignedObject: { objectType: "ipam.fhrpgroup", objectId: groupId, label: "Gateway" } });
    expect(api.POST).toHaveBeenLastCalledWith("/api/v1/inventory/ip-addresses", { body: { address: "192.0.2.1/24", assigned_object: { object_type: "ipam.fhrpgroup", object_id: groupId } } });
    const cleared = draftToResource("IP Addresses", { prefix: "192.0.2.1/24", assignedObject: "", assignedObject__type: "" }, "IP Address");
    await createResource("IP Addresses", cleared);
    expect(api.POST).toHaveBeenLastCalledWith("/api/v1/inventory/ip-addresses", { body: { address: "192.0.2.1/24", assigned_object: null } });
  });

  it.each(["Devices", "Virtual Machines"] as const)("injects nested IP assignment from %s", entity => {
    const spec = relationshipsByTab[entity]?.find(relation => relation.childEntity === (entity === "Devices" ? "Interfaces" : "VM Interfaces"))?.child;
    if (!spec) throw new Error(`Missing nested IP relation for ${entity}`);
    const descriptor = descriptorFromSpec(spec, t);
    const body = descriptor.toPayload({ prefix: "192.0.2.1/24" }, interfaceId);
    expect(body).toMatchObject({ assignedObject: { objectId: interfaceId, objectType: entity === "Devices" ? "dcim.interface" : "virtualization.vminterface" } });
    expect(body).not.toHaveProperty("interfaceId");
  });
});

function rowWithoutId<T extends { id: string }>(row: T) {
  const { id, ...body } = row;
  void id;
  return body;
}
