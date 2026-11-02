import { useState } from "react";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import type * as OpenapiFetch from "openapi-fetch";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import i18n from "@/shared/i18n";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import EntityCreateEditor from "@/shared/forms/EntityCreateEditor";
import { buildEntityDraft, draftToResourceUpdate, objectRefTypeKey, validateEntityDraft } from "@/shared/forms/entityForm";
import { createResource, deleteResource, getResource, listResourcesForTable, updateResource } from "./api";
import type { ContentType, CustomFieldDefinition } from "./custom-fields-api";

// Preserve the real client and middleware; Node's Request needs a browser origin.
vi.mock("openapi-fetch", async original => {
  const module = await original<typeof OpenapiFetch>();
  return { ...module, default: ((options) => module.default({ ...options, baseUrl: "http://localhost" })) as typeof module.default };
});
const customFields = vi.hoisted(() => ({ definitions: vi.fn(), choices: vi.fn(), types: vi.fn() }));
vi.mock("./custom-fields-api", async original => ({ ...(await original()),
  listCustomFields: customFields.definitions, listChoiceSets: customFields.choices, listContentTypes: customFields.types,
}));

const id = "01960000-0000-7000-8000-000000000080";
const parentId = "01960000-0000-7000-8000-000000000081";
const endpointId = "01960000-0000-7000-8000-000000000082";
const tagId = "01960000-0000-7000-8000-000000000083";
const ownerId = "01960000-0000-7000-8000-000000000084";
const contentTypeId = "01960000-0000-7000-8000-000000000085";
const dates = { created: "2026-10-27T01:00:00Z", last_updated: "2026-10-27T02:00:00Z" };
const metadata = { tags: [tagId], custom_fields: { approval: "CAB-42" }, ...dates };
const vct = { id, virtual_circuit_id: parentId, virtual_circuit_name: "Metro VC", interface_id: endpointId, interface_name: "bridge0", role: "peer", description: "Edge", ...metadata };
const vpn = { id, name: "Metro VPN", slug: "metro-vpn", type: "vpws", status: "active", identifier: "9223372036854775807",
  tenant_id: parentId, owner_id: ownerId, import_targets: [parentId], export_targets: [endpointId], description: "Edge", comments: "CAB", ...metadata };
const term = { id, l2vpn_id: parentId, l2vpn_name: "Metro VPN", assigned_object: { object_type: "dcim.interface", object_id: endpointId, label: "bridge0" }, ...metadata };
const cases: { entity: InventoryEntityKey; path: string; wire: Record<string, unknown>; type: string; token: string; sort: string; filters: Record<string, string[]> }[] = [
  { entity: "Virtual Circuit Terminations", path: "virtual-circuit-terminations", wire: vct, type: "virtual_circuit_termination", token: "circuits.virtualcircuittermination", sort: "role", filters: { virtual_circuit_id: [parentId], interface_id: [endpointId], role: ["peer"] } },
  { entity: "L2VPNs", path: "l2vpns", wire: vpn, type: "l2vpn", token: "vpn.l2vpn", sort: "identifier", filters: { type: ["vpws"], status: ["active"], tenant_id: [parentId] } },
  { entity: "L2VPN Terminations", path: "l2vpn-terminations", wire: term, type: "l2vpn_termination", token: "vpn.l2vpntermination", sort: "l2vpn_name", filters: { l2vpn_id: [parentId] } },
];
const fetchMock = vi.fn<(request: Request) => Promise<Response>>();
function json(value: unknown, status = 200) { return new Response(JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } }); }
function requestAt(index = fetchMock.mock.calls.length - 1): Request {
  const call = fetchMock.mock.calls[index];
  if (!call) throw new Error(`Missing request ${index}`);
  return call[0];
}
function expectRequest(method: string, path: string) {
  const request = requestAt();
  expect(request.method).toBe(method);
  expect(new URL(request.url).pathname).toBe(`/api/v1/inventory/${path}`);
  return request;
}
beforeEach(async () => {
  vi.resetAllMocks();
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockImplementation(() => Promise.resolve(json(vpn)));
  customFields.definitions.mockResolvedValue({ items: [], total: 0, limit: 100, offset: 0 });
  customFields.choices.mockResolvedValue({ items: [], total: 0, limit: 100, offset: 0 });
  customFields.types.mockResolvedValue([]);
  await i18n.changeLanguage("en");
});
afterEach(() => vi.unstubAllGlobals());

describe("termination graph native frontend boundary", () => {
  it.each(cases)("maps $entity lists, filters, search, sorting and concrete detail", async ({ entity, path, wire, sort, filters }) => {
    fetchMock.mockResolvedValueOnce(json({ items: [wire], total: 7, limit: 25, offset: 0 }));
    const page = await listResourcesForTable(entity, { limit: 25, offset: 0, search: " Metro ", sort, order: "desc", filters });
    expect(page).toMatchObject({ total: 7, limit: 25, offset: 0 });
    expect(page.items[0]).toMatchObject({ id, tags: [tagId], customFields: { approval: "CAB-42" }, createdAt: dates.created, updatedAt: dates.last_updated });
    const query = new URL(expectRequest("GET", path).url).searchParams;
    expect(Object.fromEntries(query)).toEqual({ limit: "25", offset: "0", search: "Metro", sort, order: "desc", ...Object.fromEntries(Object.entries(filters).map(([key, value]) => [key, value.join(",")])) });
    fetchMock.mockResolvedValueOnce(json(wire));
    const resource = await getResource(entity, id);
    expectRequest("GET", `${path}/${id}`);
    if (entity === "Virtual Circuit Terminations") expect(resource).toMatchObject({ virtualCircuitId: parentId, interfaceId: endpointId, role: "peer", description: "Edge", status: undefined, ownerId: undefined, comments: undefined });
    else if (entity === "L2VPNs") expect(resource).toMatchObject({ name: vpn.name, slug: vpn.slug, l2vpnType: "vpws", status: "active", identifier: vpn.identifier, tenantId: parentId, ownerId, importTargets: [parentId], exportTargets: [endpointId] });
    else expect(resource).toMatchObject({ l2vpnId: parentId, assignedObject: { objectType: "dcim.interface", objectId: endpointId, label: "bridge0" }, description: undefined, comments: undefined, ownerId: undefined, status: undefined });
    fetchMock.mockResolvedValueOnce(json({ ...wire, created: null, last_updated: null, piping_synced_at: "2020-01-01T00:00:00Z" }));
    expect(await getResource(entity, id)).toMatchObject({ createdAt: undefined, updatedAt: undefined });
  });

  it("creates VCTs with owned fields only, default peer, and changed-only PATCH", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json(vct)));
    const draft: ReturnType<typeof buildEntityDraft> = { ...buildEntityDraft("Virtual Circuit Terminations"), virtualCircuitId: parentId, interfaceId: endpointId };
    expect(draft.role).toBe("peer");
    expect(validateEntityDraft("Virtual Circuit Terminations", draft, key => key)).toEqual({});
    await createResource("Virtual Circuit Terminations", { name: "Not owned", type: "virtual_circuit_termination", virtualCircuitId: parentId, interfaceId: endpointId,
      role: draft.role, status: "active", ownerId, comments: "Not owned", description: "Edge", tags: [tagId], customFields: { approval: "CAB-42" } });
    expect(await expectRequest("POST", "virtual-circuit-terminations").json()).toEqual({ virtual_circuit_id: parentId, interface_id: endpointId, role: "peer", description: "Edge", tags: [tagId], custom_fields: { approval: "CAB-42" } });
    await updateResource("Virtual Circuit Terminations", id, { role: "hub", tags: [] });
    expect(await expectRequest("PATCH", `virtual-circuit-terminations/${id}`).json()).toEqual({ role: "hub", tags: [] });
  });

  it.each(["-9223372036854775808", "9223372036854775807", "0"])("preserves identifier %s through create, detail and PATCH", async identifier => {
    fetchMock.mockImplementation(() => Promise.resolve(json({ ...vpn, identifier })));
    const created = await createResource("L2VPNs", { name: vpn.name, type: "l2vpn", slug: vpn.slug, l2vpnType: "vpws", status: "active", identifier,
      tenantId: parentId, ownerId, importTargets: [parentId], exportTargets: [endpointId], description: "Edge", comments: "CAB", tags: [tagId], customFields: { approval: "CAB-42" } });
    expect(created.identifier).toBe(identifier);
    expect(await expectRequest("POST", "l2vpns").json()).toEqual({ name: vpn.name, slug: vpn.slug, type: "vpws", status: "active", identifier,
      tenant_id: parentId, owner_id: ownerId, import_targets: [parentId], export_targets: [endpointId], description: "Edge", comments: "CAB", tags: [tagId], custom_fields: { approval: "CAB-42" } });
    expect((await getResource("L2VPNs", id)).identifier).toBe(identifier);
    await updateResource("L2VPNs", id, { identifier });
    expect(await expectRequest("PATCH", `l2vpns/${id}`).json()).toEqual({ identifier });
  });

  it.each(["9223372036854775808", "-9223372036854775809", "-0", "1.5", "1e3", "NaN", "01", "+1", " 1 "])("rejects invalid identifier %j at write and response boundaries", async identifier => {
    await expect(createResource("L2VPNs", { name: "Metro", type: "l2vpn", identifier })).rejects.toThrow("Invalid L2VPN identifier");
    await expect(updateResource("L2VPNs", id, { identifier })).rejects.toThrow("Invalid L2VPN identifier");
    expect(fetchMock).not.toHaveBeenCalled();
    fetchMock.mockResolvedValue(json({ ...vpn, identifier }));
    await expect(getResource("L2VPNs", id)).rejects.toThrow("Invalid L2VPN identifier");
  });

  it.each([0, 42, Number("9223372036854775807")])("rejects numeric identifier wire value %s rather than silently rounding it", async identifier => {
    fetchMock.mockImplementation(() => Promise.resolve(json({ ...vpn, identifier })));
    await expect(getResource("L2VPNs", id)).rejects.toThrow("Invalid L2VPN identifier");
    fetchMock.mockResolvedValue(json({ items: [{ ...vpn, identifier }], total: 1, limit: 25, offset: 0 }));
    await expect(listResourcesForTable("L2VPNs", { limit: 25, offset: 0 })).rejects.toThrow("Invalid L2VPN identifier");
  });

  it("requests native L2VPN termination parent-ID sorting without inventing an endpoint-ID sort", async () => {
    fetchMock.mockResolvedValueOnce(json({ items: [term], total: 1, limit: 25, offset: 0 }));
    await listResourcesForTable("L2VPN Terminations", { limit: 25, offset: 0, sort: "l2vpn_id", order: "asc", filters: { l2vpn_id: [parentId] } });
    const query = new URL(expectRequest("GET", "l2vpn-terminations").url).searchParams;
    expect(Object.fromEntries(query)).toEqual({ limit: "25", offset: "0", sort: "l2vpn_id", order: "asc", l2vpn_id: parentId });
  });

  it("keeps nullable omission distinct from explicit clear and clears both target arrays", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json({ ...vpn, identifier: null, tenant_id: null, owner_id: null, import_targets: [], export_targets: [] })));
    const resource = await getResource("L2VPNs", id);
    expect(resource).toMatchObject({ identifier: undefined, tenantId: undefined, ownerId: undefined, importTargets: [], exportTargets: [] });
    await createResource("L2VPNs", { name: vpn.name, type: "l2vpn", slug: vpn.slug, l2vpnType: "vpws" });
    expect(await requestAt().json()).toEqual({ name: vpn.name, slug: vpn.slug, type: "vpws" });
    await updateResource("L2VPNs", id, { description: "Changed" });
    expect(await expectRequest("PATCH", `l2vpns/${id}`).json()).toEqual({ description: "Changed" });
    await updateResource("L2VPNs", id, { identifier: null, tenantId: null, ownerId: null, importTargets: [], exportTargets: [], tags: [] });
    expect(await requestAt().json()).toEqual({ identifier: null, tenant_id: null, owner_id: null, import_targets: [], export_targets: [], tags: [] });
  });

  it.each(cases)("keeps $entity PATCH changed-only and preserves custom field edits", async ({ entity, path, wire }) => {
    fetchMock.mockImplementation(() => Promise.resolve(json(wire)));
    await updateResource(entity, id, { tags: [], customFields: { approval: "CAB-43" } });
    expect(await expectRequest("PATCH", `${path}/${id}`).json()).toEqual({ tags: [], custom_fields: { approval: "CAB-43" } });
  });

  it.each(["dcim.interface", "virtualization.vminterface", "ipam.vlan"])("creates and PATCHes typed %s endpoints without phantom metadata", async objectType => {
    fetchMock.mockImplementation(() => Promise.resolve(json(term)));
    await createResource("L2VPN Terminations", { name: "Not owned", type: "l2vpn_termination", l2vpnId: parentId,
      assignedObject: { objectType, objectId: endpointId, label: "Display only" }, description: "Not owned", comments: "Not owned", ownerId, status: "active", tags: [tagId], customFields: { approval: "CAB-42" } });
    expect(await expectRequest("POST", "l2vpn-terminations").json()).toEqual({ l2vpn_id: parentId, assigned_object: { object_type: objectType, object_id: endpointId }, tags: [tagId], custom_fields: { approval: "CAB-42" } });
    await updateResource("L2VPN Terminations", id, { assignedObject: { objectType, objectId: endpointId, label: null }, tags: [] });
    expect(await expectRequest("PATCH", `l2vpn-terminations/${id}`).json()).toEqual({ assigned_object: { object_type: objectType, object_id: endpointId }, tags: [] });
    await updateResource("L2VPN Terminations", id, { customFields: { approval: "CAB-43" } });
    expect(await requestAt().json()).toEqual({ custom_fields: { approval: "CAB-43" } });
  });

  it("rejects unsupported endpoint kinds and enum values before network writes", async () => {
    await expect(updateResource("L2VPN Terminations", id, { assignedObject: { objectType: "dcim.device", objectId: endpointId, label: null } })).rejects.toThrow();
    await expect(updateResource("L2VPNs", id, { l2vpnType: "invalid" })).rejects.toThrow();
    await expect(updateResource("L2VPNs", id, { status: "offline" })).rejects.toThrow();
    await expect(updateResource("Virtual Circuit Terminations", id, { role: "invalid" })).rejects.toThrow();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each(cases)("rejects malformed concrete $entity IDs before GET/PATCH/DELETE", async ({ entity }) => {
    fetchMock.mockImplementation(() => Promise.resolve(json({})));
    for (const malformed of ["not-a-uuid", `${id}/other`, "", "123"]) {
      await expect(getResource(entity, malformed)).rejects.toThrow();
      await expect(updateResource(entity, malformed, { tags: [] })).rejects.toThrow();
      await expect(deleteResource(entity, malformed)).rejects.toThrow();
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each([
    { entity: "Virtual Circuit Terminations", body: { virtualCircuitId: "bad", interfaceId: endpointId } },
    { entity: "Virtual Circuit Terminations", body: { virtualCircuitId: parentId, interfaceId: "bad" } },
    { entity: "L2VPNs", body: { tenantId: "bad" } },
    { entity: "L2VPNs", body: { ownerId: "bad" } },
    { entity: "L2VPNs", body: { importTargets: ["bad"] } },
    { entity: "L2VPNs", body: { exportTargets: ["bad"] } },
    { entity: "L2VPN Terminations", body: { l2vpnId: "bad" } },
    { entity: "L2VPN Terminations", body: { assignedObject: { objectType: "dcim.interface", objectId: "bad", label: null } } },
  ] satisfies { entity: InventoryEntityKey; body: Partial<Resource> }[])("rejects malformed $entity relationship input $body", async ({ entity, body }) => {
    fetchMock.mockImplementation(() => Promise.resolve(json({})));
    await expect(createResource(entity, { name: "Metro", type: "test", ...body })).rejects.toThrow();
    await expect(updateResource(entity, id, body)).rejects.toThrow();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each(cases)("accepts bodyless $entity DELETE 204 and preserves actual 403 errors", async ({ entity, path }) => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await expect(deleteResource(entity, id)).resolves.toBeUndefined();
    expectRequest("DELETE", `${path}/${id}`);
    fetchMock.mockImplementation(request => Promise.resolve(new URL(request.url).pathname === "/api/v1/auth/me"
      ? json({ id: ownerId }) : json({ code: "FORBIDDEN", message: "Graph denied" }, 403)));
    await expect(deleteResource(entity, id)).rejects.toMatchObject({ status: 403, message: "Graph denied" });
    await expect(updateResource(entity, id, { tags: [] })).rejects.toMatchObject({ status: 403, message: "Graph denied" });
  });
});

function VctForm() {
  const [draft, setDraft] = useState(buildEntityDraft("Virtual Circuit Terminations"));
  return <EntityFormBody entity="Virtual Circuit Terminations" draft={draft} onField={(key, value) => setDraft(previous => ({ ...previous, [key]: value }))}
    loadOptions={entity => entity === "Interfaces" ? [
      { id: endpointId, name: "Physical port", interfaceType: "1000base-t" },
      { id: parentId, name: "Virtual port", interfaceType: "virtual" },
      { id: ownerId, name: "LAG port", interfaceType: "lag" },
      { id: tagId, name: "Bridge port", interfaceType: "bridge" },
      { id, name: "Unknown port" },
    ] : []} onRequestCreate={() => undefined} />;
}

describe("termination graph shared forms", () => {
  it("filters canonical interface options to virtual/lag/bridge and uses the shared picker", () => {
    render(<AllPermissions><VctForm /></AllPermissions>);
    for (const name of ["Name", "Comments"]) expect(screen.queryByRole("textbox", { name })).not.toBeInTheDocument();
    for (const name of ["Owner", "Status"]) expect(screen.queryByRole("combobox", { name })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Role" })).toHaveTextContent(/peer/i);
    fireEvent.click(screen.getByRole("combobox", { name: "Interface" }));
    for (const name of ["Virtual port", "LAG port", "Bridge port"]) expect(screen.getByRole("option", { name })).toBeInTheDocument();
    for (const name of ["Physical port", "Unknown port"]) expect(screen.queryByRole("option", { name })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "Bridge port" }));
    expect(screen.getByRole("combobox", { name: "Interface" })).toHaveTextContent("Bridge port");
  });

  it("renders endpoint-only termination controls with genuine tags and no primary-model metadata", () => {
    render(<AllPermissions><EntityFormBody entity="L2VPN Terminations" draft={buildEntityDraft("L2VPN Terminations")}
      onField={vi.fn()} loadOptions={() => []} onRequestCreate={() => undefined} /></AllPermissions>);
    expect(screen.getByRole("combobox", { name: "L2VPN" })).toBeInTheDocument();
    for (const name of ["Name", "Description", "Comments"]) expect(screen.queryByRole("textbox", { name })).not.toBeInTheDocument();
    for (const name of ["Owner", "Status"]) expect(screen.queryByRole("combobox", { name })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Tags" })).toBeInTheDocument();
  });

  it("keeps equal-UUID, type-only ObjectRef edits and validates both required halves", async () => {
    const resource: Resource = { id, name: "bridge0", type: "l2vpn_termination", l2vpnId: parentId, assignedObject: { objectType: "dcim.interface", objectId: endpointId, label: "bridge0" } };
    const initial = buildEntityDraft("L2VPN Terminations", resource);
    const changed = { ...initial, [objectRefTypeKey("assignedObject")]: "virtualization.vminterface" };
    expect(validateEntityDraft("L2VPN Terminations", changed, key => key)).toEqual({});
    expect(validateEntityDraft("L2VPN Terminations", { ...changed, assignedObject: "" }, key => key)).toHaveProperty("assignedObject");
    expect(validateEntityDraft("L2VPN Terminations", { ...changed, [objectRefTypeKey("assignedObject")]: "" }, key => key)).toHaveProperty("assignedObject");
    const payload = draftToResourceUpdate("L2VPN Terminations", changed, "L2VPN Termination", resource, initial);
    expect(payload).toEqual({ id, assignedObject: { objectType: "virtualization.vminterface", objectId: endpointId, label: null } });
    fetchMock.mockResolvedValue(json(term));
    await updateResource("L2VPN Terminations", id, payload);
    expect(await requestAt().json()).toEqual({ assigned_object: { object_type: "virtualization.vminterface", object_id: endpointId } });
  });

  it("maps nullable and M2M form clears to explicit PATCH values without resending unchanged fields", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json(vpn)));
    const resource = await getResource("L2VPNs", id);
    const initial = buildEntityDraft("L2VPNs", resource);
    const changed = { ...initial, identifier: "", tenantId: "", ownerId: "", importTargets: "", exportTargets: "", tags: "" };
    const payload = draftToResourceUpdate("L2VPNs", changed, "L2VPN", resource, initial);
    expect(payload).toEqual({ id, identifier: null, tenantId: null, ownerId: null, importTargets: [], exportTargets: [], tags: [] });
    await updateResource("L2VPNs", id, payload);
    expect(await requestAt().json()).toEqual({ identifier: null, tenant_id: null, owner_id: null, import_targets: [], export_targets: [], tags: [] });
  });

  it.each(cases)("authors required custom fields through the generic $entity create editor and real API", async ({ entity, wire, token, path }) => {
    const [appLabel, model] = token.split(".");
    if (!appLabel || !model) throw new Error("Invalid content type token");
    const contentType: ContentType = { id: contentTypeId, token, app_label: appLabel, model, label: entity,
      supported_for_custom_fields: true, capabilities: { custom_fields: true, tags: true, change_dates: true } };
    const definition: CustomFieldDefinition = { id: tagId, name: "approval", label: "Approval reference", field_type: "text", description: "", group_name: "", required: true,
      enforce_unique: false, is_secret: false, default_value: null, weight: 100, validation_minimum: null, validation_maximum: null, validation_regex: "", validation_schema: null,
      choice_set_id: null, related_object_type_id: null, related_object_filter: null, ui_visible: "if-set", ui_editable: "yes", is_cloneable: true, search_weight: 0, filter_logic: "loose",
      object_types: [contentTypeId], created_at: dates.created, updated_at: dates.last_updated };
    customFields.types.mockResolvedValue([contentType]);
    customFields.definitions.mockResolvedValue({ items: [definition], total: 1, limit: 100, offset: 0 });
    fetchMock.mockResolvedValue(json(wire));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    const onCreated = vi.fn();
    const initialValues = entity === "Virtual Circuit Terminations" ? { virtualCircuitId: parentId, interfaceId: endpointId }
      : entity === "L2VPNs" ? { name: vpn.name, slug: vpn.slug, l2vpnType: "vpws" }
        : { l2vpnId: parentId, assignedObject: endpointId, [objectRefTypeKey("assignedObject")]: "dcim.interface" };
    render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter><EntityCreateEditor open onOpenChange={vi.fn()} rootEntity={entity}
      initialValues={initialValues} createEntity={createResource} loadOptions={related => related === "Interfaces" ? [{ id: endpointId, name: "bridge0", interfaceType: "bridge" }]
        : related === "Virtual Circuits" || related === "L2VPNs" ? [{ id: parentId, name: "Metro" }] : []} onRootCreated={onCreated} />
    </MemoryRouter></AllPermissions></QueryClientProvider>);
    const input = await screen.findByRole("textbox", { name: "Approval reference" });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.getByRole("group", { name: "Approval reference" })).toHaveTextContent(/required/i));
    expect(fetchMock).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "CAB-42" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(await expectRequest("POST", path).json()).toMatchObject({ custom_fields: { approval: "CAB-42" }, ...(entity === "Virtual Circuit Terminations"
      ? { virtual_circuit_id: parentId, interface_id: endpointId, role: "peer" }
      : entity === "L2VPNs" ? { name: vpn.name, slug: vpn.slug, type: "vpws" }
        : { l2vpn_id: parentId, assigned_object: { object_type: "dcim.interface", object_id: endpointId } }) });
  });
});
