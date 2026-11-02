import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import { ApiError } from "@/shared/api/client";
import type { InventoryEntityKey, Paginated } from "@/shared/api/types";
import type { ContentType, CustomFieldDefinition } from "./custom-fields-api";
import EntityResourceEditorPage from "./EntityResourceEditorPage";
import EntityCreateEditor from "@/shared/forms/EntityCreateEditor";

const mocks = vi.hoisted(() => ({
  listCustomFields: vi.fn(), listChoiceSets: vi.fn(), listContentTypes: vi.fn(),
  createResource: vi.fn(), listAllResources: vi.fn(), listChildResourcesByRouteQuery: vi.fn(),
  notify: vi.fn(), onRootCreated: vi.fn(),
}));
vi.mock("@/shared/lib/notifications", () => ({ notify: mocks.notify }));
vi.mock("./custom-fields-api", async original => ({ ...(await original()),
  listCustomFields: mocks.listCustomFields, listChoiceSets: mocks.listChoiceSets, listContentTypes: mocks.listContentTypes,
}));
vi.mock("@/features/inventory/api", async original => ({ ...(await original()),
  createResource: mocks.createResource, listAllResources: mocks.listAllResources,
  listChildResourcesByRouteQuery: mocks.listChildResourcesByRouteQuery,
}));

const PROFILE_ID = "01960000-0000-7000-8000-000000000001";
const ASSIGNMENT_ID = "01960000-0000-7000-8000-000000000002";
function page<T>(items: T[]): Paginated<T> { return { items, total: items.length, limit: 100, offset: 0 }; }
function contentType(id: string, token: string): ContentType {
  const [app_label, model] = token.split(".");
  if (!app_label || !model) throw new Error(`Invalid content-type token: ${token}`);
  return { id, token, app_label, model, label: model, supported_for_custom_fields: true,
    capabilities: { custom_fields: true, tags: true, change_dates: true } };
}
function definition(overrides: Partial<CustomFieldDefinition> = {}): CustomFieldDefinition {
  return {
    id: "01960000-0000-7000-8000-000000000010", name: "approval", label: "Approval reference", field_type: "text",
    description: "", group_name: "", required: true, enforce_unique: false, is_secret: false,
    default_value: null, weight: 100, validation_minimum: null, validation_maximum: null,
    validation_regex: "", validation_schema: null, choice_set_id: null, related_object_type_id: null,
    related_object_filter: null, ui_visible: "if-set", ui_editable: "yes", is_cloneable: true,
    search_weight: 0, filter_logic: "loose", object_types: [PROFILE_ID],
    created_at: "2026-10-27T00:00:00Z", updated_at: "2026-10-27T00:00:00Z", ...overrides,
  };
}
type Host = "page" | "modal";
function renderCreate(host: Host, entity: InventoryEntityKey = "Module Type Profiles") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter
    initialEntries={[`/inventory/resources/${encodeURIComponent(entity)}/new`]}>
    {host === "page" ? <Routes>
      <Route path="/inventory/resources/:entity/new" element={<EntityResourceEditorPage />} />
      <Route path="/inventory/resources" element={<div>created-list</div>} />
    </Routes> : <EntityCreateEditor open onOpenChange={vi.fn()} rootEntity={entity}
      initialValues={entity === "Circuit Group Assignments" ? {
        groupId: "01960000-0000-7000-8000-000000000020", member: "01960000-0000-7000-8000-000000000021", member__type: "circuits.circuit",
      } : undefined}
      createEntity={mocks.createResource} loadOptions={() => []} onRootCreated={mocks.onRootCreated} />}
  </MemoryRouter></AllPermissions></QueryClientProvider>);
}
function fillName() {
  const name = screen.queryByRole("textbox", { name: /^Name/ });
  if (name) fireEvent.change(name, { target: { value: "Profile A" } });
}
function submit() { fireEvent.click(screen.getByRole("button", { name: "Create" })); }

describe("custom fields through inventory create hosts", () => {
  afterEach(() => vi.unstubAllEnvs());

  beforeEach(() => {
    vi.resetAllMocks();
    mocks.listContentTypes.mockResolvedValue([
      contentType(PROFILE_ID, "dcim.moduletypeprofile"), contentType(ASSIGNMENT_ID, "circuits.circuitgroupassignment"),
    ]);
    mocks.listCustomFields.mockResolvedValue(page([definition()]));
    mocks.listChoiceSets.mockResolvedValue(page([]));
    mocks.listAllResources.mockResolvedValue([]);
    mocks.listChildResourcesByRouteQuery.mockResolvedValue({ rows: [], total: 0, limit: 50, offset: 0 });
    mocks.createResource.mockImplementation((_entity, resource) => Promise.resolve({ ...resource, id: "created" }));
  });

  it.each<Host>(["page", "modal"])("%s blocks missing required fields and submits canonical UUID-scoped typed input", async host => {
    mocks.listCustomFields.mockResolvedValue(page([
      definition({ field_type: "integer" }),
      definition({ id: "other", name: "other_scope", label: "Wrong scope", object_types: [ASSIGNMENT_ID] }),
      definition({ id: "token", name: "token_scope", label: "Token is not a UUID scope", object_types: ["dcim.moduletypeprofile"] }),
    ]));
    renderCreate(host);
    const input = await screen.findByRole("spinbutton", { name: "Approval reference" });
    expect(screen.queryByText("Wrong scope")).not.toBeInTheDocument();
    expect(screen.queryByText("Token is not a UUID scope")).not.toBeInTheDocument();
    fillName(); submit();
    await waitFor(() => expect(within(screen.getByRole("group", { name: "Approval reference" })).getByRole("alert")).toBeInTheDocument());
    expect(mocks.createResource).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "42" } }); submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ customFields: { approval: 42 } })));
  });

  it("requires and submits custom fields for Circuit Group Assignments in the shared modal", async () => {
    mocks.listCustomFields.mockResolvedValue(page([definition({ object_types: [ASSIGNMENT_ID] })]));
    renderCreate("modal", "Circuit Group Assignments");
    const input = await screen.findByRole("textbox", { name: "Approval reference" });
    submit();
    await waitFor(() => expect(mocks.notify).toHaveBeenCalled());
    expect(mocks.createResource).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "CAB-42" } }); submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledWith("Circuit Group Assignments", expect.objectContaining({ customFields: { approval: "CAB-42" } })));
  });

  it.each<Host>(["page", "modal"])("%s preserves an untouched required datetime instant across a DST overlap", async host => {
    vi.stubEnv("TZ", "America/New_York");
    expect(new Date("2026-11-01T01:30:00").toISOString()).toBe("2026-11-01T05:30:00.000Z");
    const original = "2026-11-01T06:30:00Z";
    mocks.listCustomFields.mockResolvedValue(page([
      definition({ name: "observed_at", label: "Observed at", field_type: "datetime", default_value: original }),
    ]));
    renderCreate(host);
    const input = await screen.findByLabelText(/Observed at/, { selector: "input" });
    expect(input).toHaveValue("2026-11-01T01:30");
    fillName(); submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledTimes(1));
    expect(mocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ customFields: { observed_at: original } }));
  });

  it("parses an edited datetime wall time after a DST overlap", async () => {
    vi.stubEnv("TZ", "America/New_York");
    mocks.listCustomFields.mockResolvedValue(page([
      definition({ name: "observed_at", label: "Observed at", field_type: "datetime", default_value: "2026-11-01T06:30:00Z" }),
    ]));
    renderCreate("page");
    const input = await screen.findByLabelText(/Observed at/, { selector: "input" });
    fillName();
    fireEvent.change(input, { target: { value: "2026-11-01T02:30" } });
    submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ customFields: { observed_at: "2026-11-01T07:30:00.000Z" } })));
  });

  it("submits typed required defaults and omits untouched optional defaults", async () => {
    mocks.listCustomFields.mockResolvedValue(page([
      definition({ field_type: "boolean", default_value: false }),
      definition({ id: "json", name: "configuration", label: "Configuration", field_type: "json", default_value: "string default" }),
      definition({ id: "date", name: "observed_at", label: "Observed at", field_type: "datetime", default_value: "2026-10-27T12:00:00.000Z" }),
      definition({ id: "optional", name: "optional", label: "Optional", required: false, default_value: "server default" }),
    ]));
    renderCreate("modal");
    await screen.findByRole("textbox", { name: "Configuration" });
    expect(screen.getByLabelText("Observed at", { selector: "input" })).not.toHaveValue("");
    fillName(); submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ customFields: { approval: false, configuration: "string default", observed_at: "2026-10-27T12:00:00.000Z" } })));
  });

  it("keeps required JSON validation local until an actual value is supplied", async () => {
    mocks.listCustomFields.mockResolvedValue(page([definition({ field_type: "json" })]));
    renderCreate("page");
    const input = await screen.findByRole("textbox", { name: "Approval reference" });
    fillName();
    for (const value of ["{", "null", "[]"]) {
      fireEvent.change(input, { target: { value } }); submit();
      await waitFor(() => expect(within(screen.getByRole("group", { name: "Approval reference" })).getByRole("alert")).toBeInTheDocument());
      expect(mocks.createResource).not.toHaveBeenCalled();
    }
    fireEvent.change(input, { target: { value: '{"approved":false}' } }); submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ customFields: { approval: { approved: false } } })));
  });

  it("blocks a required hidden field without exposing it as editable metadata", async () => {
    mocks.listCustomFields.mockResolvedValue(page([definition({ ui_editable: "hidden" })]));
    renderCreate("modal");
    await screen.findByRole("alert");
    expect(screen.queryByRole("textbox", { name: "Approval reference" })).not.toBeInTheDocument();
    fillName(); submit();
    await waitFor(() => expect(mocks.notify).toHaveBeenCalled());
    expect(mocks.createResource).not.toHaveBeenCalled();
  });

  it.each<Host>(["page", "modal"])("%s omits untouched secrets and submits only explicitly edited secrets", async host => {
    mocks.listCustomFields.mockResolvedValue(page([
      definition({ required: false, is_secret: true, name: "edited_secret", label: "Edited secret" }),
      definition({ id: "untouched", required: false, is_secret: true, name: "untouched_secret", label: "Untouched secret" }),
    ]));
    renderCreate(host);
    const input = await screen.findByLabelText("Edited secret", { selector: "input" });
    expect(input).toHaveAttribute("type", "password");
    expect(input).toHaveValue("");
    expect(screen.queryByRole("button", { name: "Reveal" })).not.toBeInTheDocument();
    fillName(); fireEvent.change(input, { target: { value: "operator-entered-value" } }); submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ customFields: { edited_secret: "operator-entered-value" } })));
  });

  it.each([403, 503])("definitions HTTP %s prevents actual create submission", async status => {
    mocks.listCustomFields.mockRejectedValue(new ApiError({ status, message: "private definition diagnostic" }));
    renderCreate("page"); fillName();
    await screen.findByRole("alert"); submit();
    await waitFor(() => expect(mocks.notify).toHaveBeenCalled());
    expect(mocks.createResource).not.toHaveBeenCalled();
    expect(screen.queryByText("private definition diagnostic")).not.toBeInTheDocument();
  });

  it.each<Host>(["page", "modal"])("%s preserves ordinary creation when no definitions apply", async host => {
    mocks.listCustomFields.mockResolvedValue(page([definition({ object_types: [ASSIGNMENT_ID] })]));
    renderCreate(host);
    await screen.findByText("No custom fields apply to this entity type.");
    fillName(); submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledTimes(1));
    const call = mocks.createResource.mock.calls[0];
    if (!call) throw new Error("Expected a create submission");
    expect(call[1].customFields).toBeUndefined();
  });

  it.each(["Owners", "Owner Groups", "Service Templates"] satisfies InventoryEntityKey[])("does not load or render custom fields for unsupported %s", async entity => {
    renderCreate("modal", entity);
    await screen.findByRole("button", { name: "Create" });
    expect(screen.queryByText("Approval reference")).not.toBeInTheDocument();
    expect(screen.queryByText("Custom Fields", { exact: true })).not.toBeInTheDocument();
    expect(mocks.listCustomFields).not.toHaveBeenCalled();
    expect(mocks.listContentTypes).not.toHaveBeenCalled();
    expect(mocks.listChoiceSets).not.toHaveBeenCalled();
  });

  it("loads later definition pages before allowing creation", async () => {
    mocks.listCustomFields.mockImplementation(({ offset }: { offset: number }) => Promise.resolve({
      items: offset === 0 ? Array.from({ length: 100 }, (_, i) => definition({ id: `other-${i}`, name: `other_${i}`, object_types: [ASSIGNMENT_ID] })) : [definition()],
      total: 101, limit: 100, offset,
    }));
    renderCreate("page");
    await screen.findByRole("textbox", { name: "Approval reference" }); fillName(); submit();
    await waitFor(() => expect(mocks.notify).toHaveBeenCalled());
    expect(mocks.listCustomFields).toHaveBeenCalledWith({ limit: 100, offset: 100 });
    expect(mocks.createResource).not.toHaveBeenCalled();
  });

  it.each(["unsupported", "disabled capability", "catalogue unavailable"])("blocks create when content-type scope is %s", async state => {
    const type = contentType(PROFILE_ID, "dcim.moduletypeprofile");
    if (state === "unsupported") type.supported_for_custom_fields = false;
    if (state === "disabled capability") type.capabilities = { custom_fields: false, tags: true, change_dates: true };
    if (state === "catalogue unavailable") mocks.listContentTypes.mockRejectedValue(new ApiError({ status: 503, message: "private catalogue diagnostic" }));
    else mocks.listContentTypes.mockResolvedValue([type]);
    renderCreate("modal"); fillName();
    await screen.findByRole("alert"); submit();
    await waitFor(() => expect(mocks.notify).toHaveBeenCalled());
    expect(mocks.createResource).not.toHaveBeenCalled();
    expect(screen.queryByText("private catalogue diagnostic")).not.toBeInTheDocument();
  });

  it("does not create while definitions are still loading", async () => {
    mocks.listCustomFields.mockReturnValue(new Promise(() => {}));
    renderCreate("modal"); fillName(); submit();
    await waitFor(() => expect(mocks.notify).toHaveBeenCalled());
    expect(mocks.createResource).not.toHaveBeenCalled();
  });

  it("retains the parent custom-field draft after returning from nested owner creation", async () => {
    renderCreate("modal");
    const input = await screen.findByRole("textbox", { name: "Approval reference" });
    fillName(); fireEvent.change(input, { target: { value: "Parent approval" } });
    fireEvent.click(screen.getByRole("button", { name: "Create new Owner" }));
    expect(screen.getByRole("heading", { name: /Create Owner/ })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Approval reference" })).not.toBeInTheDocument();
    const back = screen.getAllByRole("button", { name: "Back" }).find(button => button.hasAttribute("data-editor-cancel"));
    expect(back).toBeDefined();
    fireEvent.click(back!);
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(await screen.findByRole("textbox", { name: "Approval reference" })).toHaveValue("Parent approval");
    submit();
    await waitFor(() => expect(mocks.createResource).toHaveBeenCalledWith("Module Type Profiles", expect.objectContaining({ customFields: { approval: "Parent approval" } })));
  });
});
