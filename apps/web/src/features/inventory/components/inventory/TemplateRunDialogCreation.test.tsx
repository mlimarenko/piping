import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router";
import { render } from "@/test/pageChrome";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import TemplateRunDialog from "./TemplateRunDialog";
import EntityResourceEditorPage from "@/features/inventory/EntityResourceEditorPage";
import type { InventoryTemplate, Paginated, Resource } from "@/shared/api/types";

// Only the API boundary is mocked: the modal, page editor, picker, forms and
// dirty confirmation use their real primitives and focus/inert behavior.
const api = vi.hoisted(() => ({
  listAllResources: vi.fn(), createResource: vi.fn(), previewTemplateRun: vi.fn(),
}));
vi.mock("@/features/inventory/api", async (original) => ({ ...(await original()), ...api }));

const roleId = "00000000-0000-4000-8000-000000000001";
const createdRole: Resource = { id: roleId, name: "Fresh role", type: "role" };
const template: InventoryTemplate = {
  id: "creation-wizard", name: "Creation wizard", slug: "creation-wizard", status: "active", description: "", updatedAt: "",
  variables: [
    { key: "device_name", label: "Device name", value_type: "string", required: true, default_value: "switch-01" },
    { key: "role", label: "Device role", value_type: "resource_ref", required: true, default_value: "" },
    { key: "device_type", label: "Device type", value_type: "resource_ref", required: false, default_value: "" },
  ],
  steps: [{ step_key: "device", title: "Create device", resource_type: "device", operation: "create", depends_on_step_keys: [],
    payload_template: { name: "{{device_name}}", role_id: "{{role}}", device_type_id: "{{device_type}}" }, on_conflict_policy: "fail" }],
};

const collectionTemplate: InventoryTemplate = {
  ...template,
  variables: [template.variables[0]!, {
    key: "people", label: "People", value_type: "json", default_value: [
      { contact_id: "", role_id: "", note: "First draft" },
      { contact_id: "", role_id: "", note: "Second draft" },
    ], validation_rules: { fields: [
      { key: "contact_id", label: "Contact", value_type: "resource_ref", validation_rules: { resource: "contact" } },
      { key: "role_id", label: "Contact role", value_type: "resource_ref", validation_rules: { resource: "contact_role" } },
      { key: "note", label: "Note", value_type: "string" },
    ] },
  }],
};

function setup(initialTemplate = template, allowCreate = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onRun = vi.fn(), onOpenChange = vi.fn();
  const view = (open: boolean, currentTemplate: InventoryTemplate) => <QueryClientProvider client={client}><PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: (action) => action !== "CREATE" || allowCreate, canAny: () => true, hasRole: () => true }}><TooltipProvider>
    <main id="main-content"><button>Workspace action</button>
      <TemplateRunDialog open={open} onOpenChange={onOpenChange} template={currentTemplate} onRun={onRun} />
    </main>
  </TooltipProvider></PermissionsContext.Provider></QueryClientProvider>;
  const rendered = render(view(true, initialTemplate));
  return { client, onRun, onOpenChange, rerender: (open: boolean, currentTemplate = initialTemplate, nextAllowCreate = allowCreate) => {
    allowCreate = nextAllowCreate;
    rendered.rerender(view(open, currentTemplate));
  } };
}

async function startCreation(label = "Device role") {
  await waitFor(() => expect(screen.getByRole("combobox", { name: label })).not.toBeDisabled());
  fireEvent.change(screen.getByLabelText(/Device name/), { target: { value: "edited-switch" } });
  fireEvent.click(screen.getByRole("button", { name: `Create new ${label}` }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  const create = screen.getByRole("button", { name: "Create"  });
  expect(create.closest("[inert], [aria-hidden=true]")).toBeNull();
  expect(screen.getByRole("heading", { level: 2 })).toHaveFocus();
}

async function expectRestored(label = "Device role") {
  expect(await screen.findByRole("dialog")).toBeInTheDocument();
  expect(screen.getByLabelText(/Device name/)).toHaveValue("edited-switch");
  await waitFor(() => expect(screen.getByRole("combobox", { name: label })).toHaveFocus());
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listAllResources.mockResolvedValue([]);
  api.createResource.mockImplementation(() => {
    api.listAllResources.mockImplementation((entity: string) => Promise.resolve(entity === "Device Roles" ? [createdRole] : []));
    return Promise.resolve(createdRole);
  });
  api.previewTemplateRun.mockImplementation((_id: string, variables: Record<string, unknown>) => Promise.resolve({
    steps: [{ step_key: "device", title: "Create device", resource_type: "device", payload: { name: variables.device_name, role_id: roleId } }],
  }));
});

describe("TemplateRunDialog page-based creation with native primitives", () => {
  it.each(["inventory-first", "wizard-first"])("shares complete inventory envelopes across both real editors (%s), including an older in-flight list", async (order) => {
    const existing: Resource = { id: "00000000-0000-4000-8000-000000000051", name: "Existing contact", type: "contact" };
    const created: Resource = { id: roleId, name: "Created contact", type: "contact" };
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const key = ["inventory", "resources", "Contacts", "options"];
    let finishOldList!: (resources: Resource[]) => void;
    if (order === "inventory-first") {
      // The real inventory cache contract, including the server page metadata.
      client.setQueryData(key, { items: [existing], total: 1, limit: 1000, offset: 0 }, { updatedAt: Date.now() - 31_000 });
      api.listAllResources.mockImplementation((entity: string) => entity === "Contacts" ? new Promise<Resource[]>((resolve) => { finishOldList = resolve; }) : Promise.resolve([]));
    }
    api.createResource.mockResolvedValue(created);
    const view = (inventory: boolean) => <QueryClientProvider client={client}><PermissionsContext.Provider value={{ roles: ["admin"], isAdmin: true, can: () => true, canAny: () => true, hasRole: () => true }}><TooltipProvider><MemoryRouter initialEntries={["/inventory/resources/Contact%20Assignments/new"]}>
      <main id="main-content">{inventory ? <Routes><Route path="/inventory/resources/:entity/new" element={<EntityResourceEditorPage />} /></Routes> : <TemplateRunDialog open template={collectionTemplate} onOpenChange={vi.fn()} onRun={vi.fn()} />}</main>
    </MemoryRouter></TooltipProvider></PermissionsContext.Provider></QueryClientProvider>;
    const rendered = render(view(order === "inventory-first"));
    if (order === "inventory-first") {
      await waitFor(() => expect(api.listAllResources).toHaveBeenCalledWith("Contacts"));
      fireEvent.click(screen.getByRole("combobox", { name: "Contact" }));
      expect(await screen.findByRole("option", { name: "Existing contact" })).toBeInTheDocument();
      rendered.rerender(view(false));
    }
    // Plus remains usable while the stale option read is pending.
    fireEvent.change(screen.getByLabelText(/Device name/), { target: { value: "Cross-flow draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Create new Contact - row 2" }));
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: created.name } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Contact - row 2" })).toHaveTextContent(created.name));
    if (order === "inventory-first") await act(async () => {
      finishOldList([existing]);
      await Promise.resolve();
    });
    const expected = order === "inventory-first" ? [existing, created] : [created];
    expect(client.getQueryData<Paginated<Resource>>(key)?.items).toEqual(expected);
    rendered.rerender(view(true));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Contact" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("combobox", { name: "Contact" }));
    fireEvent.click(await screen.findByRole("option", { name: created.name }));
    expect(screen.getByRole("combobox", { name: "Contact" })).toHaveTextContent(created.name);
    expect(client.getQueryData<Paginated<Resource>>(key)).toEqual({ items: expected, total: expected.length, limit: expected.length, offset: 0 });
  });

  it("resolves recursive contact groups into their callers before selecting only the root contact in the collection", async () => {
    const parentGroupId = "00000000-0000-4000-8000-000000000041";
    const childGroupId = "00000000-0000-4000-8000-000000000042";
    api.createResource.mockImplementation((entity: string, payload: Resource) => Promise.resolve({ ...payload, id: payload.name === "Parent group" ? parentGroupId : entity === "Contact Groups" ? childGroupId : roleId }));
    const { client } = setup(collectionTemplate);
    await startCreation("Contact - row 2");
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Root contact" } });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Contact Group" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "Create new Contact Group" }));
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Child group" } });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Parent Group" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "Create new Parent Group" }));
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Parent group" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: /^Name/ })).toHaveValue("Child group"));
    expect(screen.getByRole("combobox", { name: "Parent Group" })).toHaveTextContent("Parent group");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: /^Name/ })).toHaveValue("Root contact"));
    expect(screen.getByRole("combobox", { name: "Contact Group" })).toHaveTextContent("Child group");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await expectRestored("Contact - row 2");
    expect(api.createResource).toHaveBeenNthCalledWith(2, "Contact Groups", expect.objectContaining({ name: "Child group", parentId: parentGroupId }));
    expect(api.createResource).toHaveBeenNthCalledWith(3, "Contacts", expect.objectContaining({ name: "Root contact", groupId: childGroupId }));
    expect(screen.getByRole("combobox", { name: "Contact - row 2" })).toHaveTextContent("Root contact");
    expect(screen.getByRole("combobox", { name: "Contact - row 1" })).not.toHaveTextContent("Root contact");
    expect(client.getQueryData<Paginated<Resource>>(["inventory", "resources", "Contact Groups", "options"])?.items.map((item) => item.id)).toEqual([parentGroupId, childGroupId]);
  });

  it.each([
    ["Contact", "Contacts", "contact", "contact_id"],
    ["Contact role", "Contact Roles", "contact_role", "role_id"],
  ])("creates a collection %s into the exact calling row and preserves sibling fields through preview", async (label, entity, type, fieldKey) => {
    const created = { id: roleId, name: `Fresh ${label}`, type };
    api.createResource.mockResolvedValue(created);
    const { client } = setup(collectionTemplate);
    const pickerLabel = `${label} - row 2`;
    await startCreation(pickerLabel);
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: created.name } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await expectRestored(pickerLabel);
    expect(screen.getByRole("combobox", { name: pickerLabel })).toHaveTextContent(created.name);
    expect(screen.getByRole("combobox", { name: `${label} - row 1` })).not.toHaveTextContent(created.name);
    expect(screen.getByRole("textbox", { name: "Note - row 1" })).toHaveValue("First draft");
    expect(screen.getByRole("textbox", { name: "Note - row 2" })).toHaveValue("Second draft");
    expect(client.getQueryData(["inventory", "resources", entity, "options"])).toEqual({ items: [created], total: 1, limit: 1, offset: 0 });
    fireEvent.click(screen.getByRole("button", { name: /^Preview/ }));
    await waitFor(() => expect(api.previewTemplateRun).toHaveBeenCalledWith(collectionTemplate.id, {
      device_name: "edited-switch", people: [
        { contact_id: "", role_id: "", note: "First draft" },
        { contact_id: "", role_id: "", note: "Second draft", [fieldKey]: roleId },
      ],
    }));
  });

  it.each(["Cancel", "Escape", "Back"])("preserves collection drafts and restores the calling picker after dirty %s", async (action) => {
    setup(collectionTemplate);
    await startCreation("Contact - row 2");
    const name = screen.getByRole("textbox", { name: /^Name/ });
    fireEvent.change(name, { target: { value: "Unsaved contact" } });
    const navigate = () => action === "Escape" ? fireEvent.keyDown(name, { key: "Escape" }) : fireEvent.click(screen.getByRole("button", { name: action }));
    navigate();
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Continue editing" }));
    expect(name).toHaveValue("Unsaved contact");
    navigate();
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard" }));
    await expectRestored("Contact - row 2");
    expect(screen.getByRole("textbox", { name: "Note - row 2" })).toHaveValue("Second draft");
    expect(api.createResource).not.toHaveBeenCalled();
  });

  it("keeps RUN and ordinary selection available without exposing CREATE on either kind of reference", async () => {
    setup({ ...collectionTemplate, variables: [...collectionTemplate.variables, template.variables[1]!] }, false);
    expect(await screen.findByRole("combobox", { name: "Contact - row 1" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Device role" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Create new/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Preview/ })).not.toBeDisabled();
    expect(api.createResource).not.toHaveBeenCalled();
  });

  it("does not select an invalid identity returned by creation and keeps the editor draft available for recovery", async () => {
    api.createResource.mockResolvedValue({ id: "not-a-uuid", name: "Invalid contact", type: "contact" });
    setup(collectionTemplate);
    await startCreation("Contact - row 2");
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Invalid contact" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createResource).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByRole("button", { name: "Create" })).not.toBeDisabled());
    expect(screen.getByRole("textbox", { name: /^Name/ })).toHaveValue("Invalid contact");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("checks CREATE again at persistence if permission changes while an editor is open", async () => {
    const { rerender } = setup(collectionTemplate);
    await startCreation("Contact - row 2");
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Retained contact" } });
    rerender(true, collectionTemplate, false);
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Create" })).not.toBeDisabled());
    expect(api.createResource).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: /^Name/ })).toHaveValue("Retained contact");
    expect(screen.queryByRole("button", { name: /^Create new/ })).not.toBeInTheDocument();
  });

  it.each(["reopen", "template"])("ignores a pending collection creation after %s, including when a new editor is already open", async (change) => {
    let resolve!: (resource: Resource) => void;
    api.createResource.mockReturnValue(new Promise<Resource>((done) => { resolve = done; }));
    const { rerender } = setup(collectionTemplate);
    await startCreation("Contact - row 2");
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Old contact" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createResource).toHaveBeenCalledTimes(1));
    if (change === "reopen") { rerender(false); rerender(true); }
    else rerender(true, { ...collectionTemplate, id: "other-template", variables: [collectionTemplate.variables[0]!, { ...collectionTemplate.variables[1]!, default_value: [{ note: "Replacement row" }] }] });
    await startCreation("Contact - row 1");
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Current draft" } });
    await act(async () => {
      resolve({ id: roleId, name: "Old contact", type: "contact" });
      await Promise.resolve();
    });
    expect(screen.getByRole("textbox", { name: /^Name/ })).toHaveValue("Current draft");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard" }));
    await expectRestored("Contact - row 1");
    expect(screen.getByRole("combobox", { name: "Contact - row 1" })).not.toHaveTextContent("Old contact");
  });

  it("exposes the create page, caches and selects the persisted typed UUID, and keeps wizard inputs through preview and run", async () => {
    const { client, onRun, onOpenChange } = setup();
    await startCreation();
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Fresh role" } });
    fireEvent.click(screen.getByRole("button", { name: "Create"  }));
    await expectRestored();
    expect(api.createResource).toHaveBeenCalledWith("Device Roles", expect.objectContaining({ name: "Fresh role" }));
    expect(client.getQueryData(["inventory", "resources", "Device Roles", "options"])).toEqual({ items: [createdRole], total: 1, limit: 1, offset: 0 });
    expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Fresh role");
    expect(onOpenChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^Preview/ }));
    const values = { device_name: "edited-switch", role: { resource: "device_role", id: roleId }, device_type: "" };
    await waitFor(() => expect(api.previewTemplateRun).toHaveBeenCalledWith(template.id, values));
    await waitFor(() => expect(screen.getByRole("button", { name: /Run now/ })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "Back"  }));
    expect(screen.getByLabelText(/Device name/)).toHaveValue("edited-switch");
    expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Fresh role");
    fireEvent.click(screen.getByRole("button", { name: /^Preview/ }));
    await waitFor(() => expect(screen.getByRole("button", { name: /Run now/ })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /Run now/ }));
    expect(onRun).toHaveBeenCalledWith(values);
  });

  it.each(["Cancel", "Escape", "Back"])("returns from a clean create page via %s without resetting values or closing the wizard", async (action) => {
    const { onOpenChange } = setup();
    await startCreation();
    if (action === "Escape") fireEvent.keyDown(screen.getByRole("heading", { level: 2 }), { key: "Escape" });
    else fireEvent.click(screen.getByRole("button", { name: action  }));
    await expectRestored();
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(api.createResource).not.toHaveBeenCalled();
  });

  it.each(["Cancel", "Escape", "Back"])("guards dirty %s navigation and keeps the draft when discard is cancelled", async (action) => {
    setup();
    await startCreation();
    const name = screen.getByRole("textbox", { name: /^Name/ });
    fireEvent.change(name, { target: { value: "Unsaved role" } });
    const navigate = () => {
      if (action === "Escape") fireEvent.keyDown(name, { key: "Escape" });
      else fireEvent.click(screen.getByRole("button", { name: action  }));
    };
    navigate();
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Continue editing" }));
    expect(name).toHaveValue("Unsaved role");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    navigate();
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard"  }));
    await expectRestored();
    expect(api.createResource).not.toHaveBeenCalled();
  });

  it("lets picker Escape close only the picker and nested dirty Back/Escape pop only one create frame", async () => {
    setup();
    await startCreation("Device type");
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Parent type" } });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Manufacturer" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("combobox", { name: "Manufacturer" }));
    const search = await screen.findByPlaceholderText(/Search.*manufacturer/i);
    fireEvent.keyDown(search, { key: "Escape" });
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Manufacturer" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "Create new Manufacturer" }));
    const childName = screen.getByRole("textbox", { name: /^Name/ });
    fireEvent.change(childName, { target: { value: "Child maker" } });
    // Header Back and footer Back both keep the canonical dirty guard.
    const [headerBack] = screen.getAllByRole("button", { name: "Back" });
    if (!headerBack) throw new Error("Missing nested editor Back action");
    fireEvent.click(headerBack);
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Continue editing" }));
    expect(childName).toHaveValue("Child maker");
    fireEvent.keyDown(childName, { key: "Escape" });
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard"  }));
    expect(screen.getByRole("textbox", { name: /^Name/ })).toHaveValue("Parent type");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel"  }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard"  }));
    await expectRestored("Device type");
  });
});
