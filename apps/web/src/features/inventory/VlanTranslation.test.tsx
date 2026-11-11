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
import { buildEntityDraft, draftToResourceUpdate, validateEntityDraft } from "@/shared/forms/entityForm";
import { createResource, deleteResource, getResource, listResourcesForTable, updateResource } from "./api";
import type { ContentType, CustomFieldDefinition } from "./custom-fields-api";

// Node's Request needs an absolute URL; keep the real client's serialization,
// session handling and error middleware while supplying the browser origin.
vi.mock("openapi-fetch", async original => {
  const module = await original<typeof OpenapiFetch>();
  return { ...module, default: ((options) => module.default({ ...options, baseUrl: "http://localhost" })) as typeof module.default };
});
const customFields = vi.hoisted(() => ({ definitions: vi.fn(), choices: vi.fn(), types: vi.fn() }));
vi.mock("./custom-fields-api", async original => ({ ...(await original()),
  listCustomFields: customFields.definitions, listChoiceSets: customFields.choices, listContentTypes: customFields.types,
}));

const id = "01960000-0000-7000-8000-000000000080";
const policyId = "01960000-0000-7000-8000-000000000081";
const tagId = "01960000-0000-7000-8000-000000000082";
const ownerId = "01960000-0000-7000-8000-000000000083";
const contentTypeId = "01960000-0000-7000-8000-000000000084";
const policy = { id: policyId, name: "Edge translation", description: "Edge", comments: "CAB", owner_id: ownerId,
  tags: [tagId], custom_fields: { approval: "CAB-42" }, created: "2026-10-27T01:00:00Z", last_updated: "2026-10-27T02:00:00Z" };
const rule = { id, policy_id: policyId, policy_name: policy.name, local_vid: 1, remote_vid: 4094, description: "Translate",
  tags: [tagId], custom_fields: { approval: "CAB-42" }, created: policy.created, last_updated: policy.last_updated };
const entities = ["VLAN Translation Policies", "VLAN Translation Rules"] as const;
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
  customFields.definitions.mockResolvedValue({ items: [], total: 0, limit: 100, offset: 0 });
  customFields.choices.mockResolvedValue({ items: [], total: 0, limit: 100, offset: 0 });
  customFields.types.mockResolvedValue([]);
  await i18n.changeLanguage("en");
});
afterEach(() => vi.unstubAllGlobals());

describe("VLAN translation native frontend boundary", () => {
  it.each(entities)("maps %s lists and detail with genuine metadata and supported sort/filter queries", async entity => {
    const isRule = entity === "VLAN Translation Rules";
    const wire = isRule ? rule : policy;
    const path = isRule ? "vlan-translation-rules" : "vlan-translation-policies";
    fetchMock.mockResolvedValueOnce(json({ items: [wire], total: 7, limit: 25, offset: 0 }));
    const page = await listResourcesForTable(entity, { limit: 25, offset: 0, search: " Edge ", sort: isRule ? "local_vid" : "name", order: "desc",
      filters: isRule ? { policy_id: [policyId] } : {} });
    expect(page).toMatchObject({ total: 7, limit: 25, offset: 0 });
    expect(page.items[0]).toMatchObject({ tags: [tagId], customFields: { approval: "CAB-42" }, createdAt: policy.created, updatedAt: policy.last_updated,
      ...(isRule ? { name: "1 → 4094", policyId, policyName: policy.name, localVid: "1", remoteVid: "4094" } : { name: policy.name, ownerId, comments: "CAB" }) });
    const query = new URL(expectRequest("GET", path).url).searchParams;
    expect(Object.fromEntries(query)).toEqual({ limit: "25", offset: "0", search: "Edge", sort: isRule ? "local_vid" : "name", order: "desc", ...(isRule ? { policy_id: policyId } : {}) });
    fetchMock.mockResolvedValueOnce(json({ ...wire, created: null, last_updated: null, piping_synced_at: "2020-01-01T00:00:00Z" }));
    expect(await getResource(entity, wire.id)).toMatchObject({ id: wire.id, createdAt: undefined, updatedAt: undefined, status: undefined });
    expectRequest("GET", `${path}/${wire.id}`);
  });

  it("creates policies with their owned metadata and keeps PATCH changed-only, including clearing tags", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json(policy)));
    await createResource("VLAN Translation Policies", { name: policy.name, type: "vlan_translation_policy", description: "Edge", comments: "CAB", ownerId, tags: [tagId], customFields: { approval: "CAB-42" } });
    expect(await expectRequest("POST", "vlan-translation-policies").json()).toEqual({ name: policy.name, description: "Edge", comments: "CAB", owner_id: ownerId, tags: [tagId], custom_fields: { approval: "CAB-42" } });
    await updateResource("VLAN Translation Policies", policyId, { description: "Changed" });
    expect(await requestAt().json()).toEqual({ description: "Changed" });
    await updateResource("VLAN Translation Policies", policyId, { tags: [], customFields: { approval: "CAB-43" } });
    expect(await expectRequest("PATCH", `vlan-translation-policies/${policyId}`).json()).toEqual({ tags: [], custom_fields: { approval: "CAB-43" } });
  });

  it("creates rules without phantom primary-model fields and preserves omitted PATCH values", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json(rule)));
    await createResource("VLAN Translation Rules", { name: "UI identity", type: "vlan_translation_rule", status: "active", ownerId, comments: "Not owned",
      policyId, localVid: "1", remoteVid: "4094", description: "Translate", tags: [tagId], customFields: { approval: "CAB-42" } });
    expect(await expectRequest("POST", "vlan-translation-rules").json()).toEqual({ policy_id: policyId, local_vid: 1, remote_vid: 4094, description: "Translate", tags: [tagId], custom_fields: { approval: "CAB-42" } });
    await updateResource("VLAN Translation Rules", id, { remoteVid: "2", tags: [] });
    expect(await expectRequest("PATCH", `vlan-translation-rules/${id}`).json()).toEqual({ remote_vid: 2, tags: [] });
    await updateResource("VLAN Translation Rules", id, { customFields: { approval: "CAB-43" } });
    expect(await requestAt().json()).toEqual({ custom_fields: { approval: "CAB-43" } });
  });

  it.each(["", "0", "4095", "1.5", "NaN", "-1"])("rejects invalid VID %j before issuing writes", async value => {
    await expect(createResource("VLAN Translation Rules", { name: "", type: "vlan_translation_rule", policyId, localVid: value, remoteVid: "1" })).rejects.toThrow("Invalid VLAN translation VID");
    await expect(updateResource("VLAN Translation Rules", id, { remoteVid: value })).rejects.toThrow("Invalid VLAN translation VID");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each(entities)("accepts bodyless DELETE 204 and propagates denied deletion for %s", async entity => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await expect(deleteResource(entity, id)).resolves.toBeUndefined();
    expectRequest("DELETE", `${entity === entities[0] ? "vlan-translation-policies" : "vlan-translation-rules"}/${id}`);
    fetchMock.mockImplementation(request => Promise.resolve(new URL(request.url).pathname === "/api/v1/auth/me"
      ? json({ id: ownerId }) : json({ code: "FORBIDDEN", message: "Translation denied" }, 403)));
    await expect(deleteResource(entity, id)).rejects.toMatchObject({ status: 403, message: "Translation denied" });
    await expect(updateResource(entity, id, { description: "Denied change" })).rejects.toMatchObject({ status: 403, message: "Translation denied" });
  });

  it.each(["Interfaces", "VM Interfaces"] as const)("round-trips %s policy assignment, explicit clear and unrelated update omission", async entity => {
    const path = entity === "Interfaces" ? "interfaces" : "vm-interfaces";
    fetchMock.mockImplementation(() => Promise.resolve(json({ id, name: "eth0", vlan_translation_policy_id: policyId })));
    const resource = await getResource(entity, id);
    expect(resource.vlanTranslationPolicyId).toBe(policyId);
    await updateResource(entity, id, { vlanTranslationPolicyId: policyId });
    expect(await expectRequest("PUT", `${path}/${id}`).json()).toEqual({ vlan_translation_policy_id: policyId });
    await updateResource(entity, id, { description: "Unrelated" });
    expect(await requestAt().json()).toEqual({ description: "Unrelated" });
    const initial = buildEntityDraft(entity, resource);
    const clear = draftToResourceUpdate(entity, { ...initial, vlanTranslationPolicyId: "" }, entity, resource, initial);
    expect(clear).toMatchObject({ id, vlanTranslationPolicyId: null });
    await updateResource(entity, id, clear);
    expect(await requestAt().json()).toEqual({ vlan_translation_policy_id: null, ...(entity === "Interfaces" ? { name: "eth0" } : {}) });
  });
});

function RuleForm() {
  const resource: Resource = { id, name: "1 → 4094", type: "vlan_translation_rule", policyId, localVid: "1", remoteVid: "4094", tags: [tagId] };
  const initial = buildEntityDraft("VLAN Translation Rules", resource);
  const [draft, setDraft] = useState(initial);
  return <><EntityFormBody entity="VLAN Translation Rules" draft={draft} onField={(key, value) => setDraft(previous => ({ ...previous, [key]: value }))}
    loadOptions={entity => entity === "VLAN Translation Policies" ? [{ id: policyId, name: policy.name }] : entity === "Tags" ? [{ id: tagId, name: "Edge", objectTypes: [] }] : []} onRequestCreate={() => undefined} />
    <output data-testid="rule-payload">{JSON.stringify(draftToResourceUpdate("VLAN Translation Rules", draft, "VLAN Translation Rule", resource, initial))}</output></>;
}
describe("VLAN translation shared forms", () => {
  it.each(["Interfaces", "VM Interfaces"] as const)("renders %s policy selection through the existing related picker", entity => {
    const resource: Resource = { id, name: "eth0", type: entity === "Interfaces" ? "interface" : "vm_interface", vlanTranslationPolicyId: policyId };
    render(<AllPermissions><EntityFormBody entity={entity} draft={buildEntityDraft(entity, resource)} onField={vi.fn()}
      loadOptions={related => related === "VLAN Translation Policies" ? [{ id: policyId, name: policy.name }] : []} onRequestCreate={() => undefined} /></AllPermissions>);
    expect(screen.getByRole("combobox", { name: /VLAN translation policy/i })).toHaveTextContent(policy.name);
  });

  it("renders the nameless rule relationship and VID controls, and clears genuine tags", () => {
    render(<AllPermissions><RuleForm /></AllPermissions>);
    for (const name of ["Name", "Comments"]) expect(screen.queryByRole("textbox", { name })).not.toBeInTheDocument();
    for (const name of ["Owner", "Status"]) expect(screen.queryByRole("combobox", { name })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: /VLAN translation policy/i })).toHaveTextContent(policy.name);
    expect(screen.getByRole("spinbutton", { name: /Local VLAN ID/i })).toHaveValue(1);
    expect(screen.getByRole("spinbutton", { name: /Remote VLAN ID/i })).toHaveValue(4094);
    expect(screen.getByTestId("rule-payload")).toHaveTextContent(JSON.stringify({ id }));
    fireEvent.click(screen.getByRole("button", { name: "Remove Edge" }));
    expect(screen.getByTestId("rule-payload")).toHaveTextContent(JSON.stringify({ id, tags: [] }));
    const draft = { ...buildEntityDraft("VLAN Translation Rules"), policyId, localVid: "1", remoteVid: "4094" };
    expect(validateEntityDraft("VLAN Translation Rules", draft, key => key)).toEqual({});
    expect(validateEntityDraft("VLAN Translation Rules", { ...draft, policyId: "" }, key => key)).toHaveProperty("policyId");
    expect(validateEntityDraft("VLAN Translation Rules", { ...draft, localVid: "0", remoteVid: "4095" }, key => key)).toMatchObject({ localVid: expect.any(String), remoteVid: expect.any(String) });
  });

  it.each(entities)("authors required custom fields through the real generic %s create editor and API", async entity => {
    const isRule = entity === "VLAN Translation Rules";
    const token = isRule ? "ipam.vlantranslationrule" : "ipam.vlantranslationpolicy";
    const contentType: ContentType = { id: contentTypeId, token, app_label: "ipam", model: isRule ? "vlantranslationrule" : "vlantranslationpolicy", label: entity,
      supported_for_custom_fields: true, capabilities: { custom_fields: true, tags: true, change_dates: true } };
    const definition: CustomFieldDefinition = { id: tagId, name: "approval", label: "Approval reference", field_type: "text", description: "", group_name: "", required: true,
      enforce_unique: false, is_secret: false, default_value: null, weight: 100, validation_minimum: null, validation_maximum: null, validation_regex: "", validation_schema: null,
      choice_set_id: null, related_object_type_id: null, related_object_filter: null, ui_visible: "if-set", ui_editable: "yes", is_cloneable: true, search_weight: 0, filter_logic: "loose",
      object_types: [contentTypeId], created_at: policy.created, updated_at: policy.last_updated };
    customFields.types.mockResolvedValue([contentType]);
    customFields.definitions.mockResolvedValue({ items: [definition], total: 1, limit: 100, offset: 0 });
    fetchMock.mockResolvedValue(json(isRule ? rule : policy));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    const onCreated = vi.fn();
    const initialValues = isRule ? { policyId, localVid: "1", remoteVid: "4094" } : { name: policy.name };
    render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter><EntityCreateEditor open onOpenChange={vi.fn()} rootEntity={entity}
      initialValues={initialValues} createEntity={createResource} loadOptions={(related: InventoryEntityKey) => related === entities[0] ? [{ id: policyId, name: policy.name }] : []} onRootCreated={onCreated} />
    </MemoryRouter></AllPermissions></QueryClientProvider>);
    const input = await screen.findByRole("textbox", { name: "Approval reference" });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.getByRole("group", { name: "Approval reference" })).toHaveTextContent(/required/i));
    expect(fetchMock).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "CAB-42" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(await requestAt().json()).toMatchObject({ custom_fields: { approval: "CAB-42" }, ...(isRule ? { policy_id: policyId, local_vid: 1, remote_vid: 4094 } : { name: policy.name }) });
  });
});
