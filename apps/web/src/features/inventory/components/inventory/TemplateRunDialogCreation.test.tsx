import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import TemplateRunDialog from "./TemplateRunDialog";
import type { InventoryTemplate, Resource } from "@/shared/api/types";

// Only the API boundary is mocked: the modal, page editor, picker, forms and
// dirty confirmation use their real primitives and focus/inert behavior.
const api = vi.hoisted(() => ({
  listAllResources: vi.fn(), createResource: vi.fn(), previewTemplateRun: vi.fn(),
}));
vi.mock("@/features/inventory/api", () => api);

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

function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onRun = vi.fn(), onOpenChange = vi.fn();
  render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{ roles: ["admin"], isAdmin: true, can: () => true, canAny: () => true, hasRole: () => true }}><TooltipProvider>
    <main id="main-content"><button>Workspace action</button>
      <TemplateRunDialog open onOpenChange={onOpenChange} template={template} onRun={onRun} />
    </main>
  </TooltipProvider></PermissionsContext.Provider></QueryClientProvider>);
  return { client, onRun, onOpenChange };
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
  it("exposes the create page, caches and selects the persisted typed UUID, and keeps wizard inputs through preview and run", async () => {
    const { client, onRun, onOpenChange } = setup();
    await startCreation();
    fireEvent.change(screen.getByRole("textbox", { name: /^Name/ }), { target: { value: "Fresh role" } });
    fireEvent.click(screen.getByRole("button", { name: "Create"  }));
    await expectRestored();
    expect(api.createResource).toHaveBeenCalledWith("Device Roles", expect.objectContaining({ name: "Fresh role" }));
    expect(client.getQueryData(["inventory", "template-run-options", "Device Roles"])).toEqual([createdRole]);
    expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Fresh role");
    expect(onOpenChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^Preview/ }));
    const values = { device_name: "edited-switch", role: { resource: "device_role", id: roleId } };
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
