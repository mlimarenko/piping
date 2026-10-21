import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import TemplateRunDialog from "./TemplateRunDialog";
import type { InventoryTemplate, Resource } from "@/shared/api/types";

const apiMocks = vi.hoisted(() => ({
  listAllResources: vi.fn(),
  createResource: vi.fn(),
  previewTemplateRun: vi.fn(),
}));

vi.mock("@/features/inventory/api", () => ({
  listAllResources: apiMocks.listAllResources,
  createResource: apiMocks.createResource,
  previewTemplateRun: apiMocks.previewTemplateRun,
}));

vi.mock("@/shared/forms/EntityCreateEditor", () => ({
  default: ({ createEntity, onRootCreated }: { createEntity: (entity: string, resource: Resource) => Promise<Resource>; onRootCreated: (resource: Resource) => void }) => (
    <button onClick={async () => onRootCreated(await createEntity("Device Roles", inventoryOption("", "Fresh role", "role")))}>Confirm resource creation</button>
  ),
}));

const inventoryOption = (id: string, name: string, type: string): Resource => ({
  id,
  name,
  type,
  slug: name === "Network device" ? "network-device" : name === "Generic network appliance" ? "generic-network-appliance" : undefined,
});

const siteStep = {
  step_key: "create_site",
  title: "Create site",
  resource_type: "site" as const,
  operation: "create" as const,
  depends_on_step_keys: [],
  payload_template: { name: "{{site_name}}", status: "active" },
  on_conflict_policy: "fail" as const,
};

// The factory shape: role/device_type are resolved server-side by slug or name.
const nameReferenceTemplate: InventoryTemplate = {
  id: "tpl-network",
  name: "Create network device",
  slug: "create-network-device",
  status: "active",
  description: "",
  updatedAt: "",
  variables: [
    { key: "site_name", label: "Site name", value_type: "string", required: true, default_value: "Default site" },
    { key: "device_name", label: "Device name", value_type: "string", required: true, default_value: "switch-01" },
    {
      key: "device_role",
      label: "Device role",
      value_type: "string",
      required: true,
      default_value: "network-device",
    },
    {
      key: "device_type",
      label: "Device type",
      value_type: "string",
      required: true,
      default_value: "generic-network-appliance",
    },
  ],
  steps: [
    siteStep,
    {
      step_key: "create_device",
      title: "Create network device",
      resource_type: "device",
      operation: "create",
      depends_on_step_keys: ["create_site"],
      payload_template: {
        name: "{{device_name}}",
        site_id: "{{steps.create_site.id}}",
        role: "{{device_role}}",
        device_type: "{{device_type}}",
        status: "active",
      },
      on_conflict_policy: "fail",
    },
  ],
};

// The canonical authoring shape: the step consumes the picked object UUIDs.
const idReferenceTemplate: InventoryTemplate = {
  ...nameReferenceTemplate,
  id: "tpl-network-uuid",
  variables: nameReferenceTemplate.variables.map((variable) => ({
    ...variable,
    value_type: "resource_ref" as const,
  })),
  steps: [
    siteStep,
    {
      step_key: "create_device",
      title: "Create network device",
      resource_type: "device",
      operation: "create",
      depends_on_step_keys: ["create_site"],
      payload_template: {
        name: "{{device_name}}",
        site_id: "{{steps.create_site.id}}",
        role_id: "{{device_role}}",
        device_type_id: "{{device_type}}",
        status: "active",
      },
      on_conflict_policy: "fail",
    },
  ],
};

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

const renderDialog = (
  onRun: (variables: Record<string, unknown>) => void,
  template: InventoryTemplate = nameReferenceTemplate,
) =>
  renderWithClient(
    <TemplateRunDialog open onOpenChange={() => {}} template={template} onRun={onRun} />,
  );

const previewFromVariables = (_id: string, variables: Record<string, unknown>) =>
  Promise.resolve({
    steps: [
      {
        step_key: "create_device",
        title: "Create network device",
        resource_type: "device",
        payload: {
          name: variables.device_name,
          role: variables.device_role,
          role_id: typeof variables.device_role === "object" ? (variables.device_role as { id: string }).id : variables.device_role,
          device_type_id: typeof variables.device_type === "object" ? (variables.device_type as { id: string }).id : variables.device_type,
        },
      },
    ],
  });

describe("TemplateRunDialog reference variables", () => {
  it("retains the wizard after rejected submission without exposing the backend error or retrying", async () => {
    const onRun = vi.fn().mockRejectedValue(new Error("secret backend trace"));
    const onOpenChange = vi.fn();
    renderWithClient(<TemplateRunDialog open template={nameReferenceTemplate} onRun={onRun} onOpenChange={onOpenChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const run = await screen.findByRole("button", { name: /^run now/i });
    await waitFor(() => expect(run).not.toBeDisabled());
    fireEvent.click(run);
    expect(await screen.findByRole("alert")).toHaveTextContent("server did not confirm");
    expect(screen.queryByText("secret backend trace")).not.toBeInTheDocument();
    expect(onRun).toHaveBeenCalledTimes(1);
    expect(onOpenChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByLabelText("Device name")).toHaveValue("switch-01");
  });

  it("retries failed option reads and preview reads while retaining input and disabling execution", async () => {
    apiMocks.listAllResources.mockRejectedValueOnce(new Error("private options error")).mockResolvedValue([]);
    apiMocks.previewTemplateRun.mockRejectedValueOnce(new Error("private preview error")).mockImplementationOnce(previewFromVariables);
    const template = { ...nameReferenceTemplate, variables: nameReferenceTemplate.variables.filter((variable) => variable.key !== "device_type") };
    renderDialog(vi.fn(), template);
    fireEvent.change(screen.getByLabelText("Device name"), { target: { value: "kept-device" } });
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load objects");
    fireEvent.click(screen.getByRole("button", { name: "Retry check" }));
    expect(await screen.findByText(/No objects yet/)).toBeInTheDocument();
    expect(screen.getByLabelText("Device name")).toHaveValue("kept-device");
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not verify the run");
    expect(screen.getByRole("button", { name: /^run now/i })).toBeDisabled();
    expect(screen.queryByText(/private .* error/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry check" }));
    await waitFor(() => expect(screen.getByRole("button", { name: /^run now/i })).not.toBeDisabled());
    expect(apiMocks.previewTemplateRun).toHaveBeenLastCalledWith(template.id, expect.objectContaining({ device_name: "kept-device" }));
  });
  it("waits for server acceptance before closing and blocks duplicate submission", async () => {
    let accept!: () => void;
    const onRun = vi.fn(() => new Promise<void>((resolve) => { accept = resolve; }));
    const onOpenChange = vi.fn();
    renderWithClient(<TemplateRunDialog open template={nameReferenceTemplate} onRun={onRun} onOpenChange={onOpenChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const run = await screen.findByRole("button", { name: /^run now/i });
    await waitFor(() => expect(run).not.toBeDisabled());
    fireEvent.click(run);
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(run).toBeDisabled();
    fireEvent.click(run);
    expect(onRun).toHaveBeenCalledTimes(1);
    accept();
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
  it.each([true, false])("preserves explicit default UUID over conflicting name rules (listed=%s)", async (listed) => {
    const explicit = "00000000-0000-4000-8000-000000000001";
    const other = "00000000-0000-4000-8000-000000000002";
    apiMocks.listAllResources.mockResolvedValue([
      ...(listed ? [inventoryOption(explicit, "Role A", "role")] : []),
      inventoryOption(other, "Role B", "role"),
    ]);
    const template = { ...idReferenceTemplate, variables: [{ key: "device_role", label: "Device role", value_type: "resource_ref" as const, required: true,
      default_value: explicit, validation_rules: { default_name: "Role B" } }] };
    renderDialog(vi.fn(), template);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent(listed ? "Role A" : explicit));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, {
      device_role: { resource: "device_role", id: explicit },
    }));
  });

  it("does not resolve an ambiguous exact model default", async () => {
    apiMocks.listAllResources.mockResolvedValue([
      inventoryOption("00000000-0000-4000-8000-000000000001", "Core Switch", "device_type"),
      inventoryOption("00000000-0000-4000-8000-000000000002", "Core Switch", "device_type"),
    ]);
    const template = { ...idReferenceTemplate, variables: [{ key: "device_type", label: "Device type", value_type: "resource_ref" as const, required: true, default_value: "Core Switch" }] };
    renderDialog(vi.fn(), template);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device type" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(await screen.findByText("Required")).toBeInTheDocument();
    expect(apiMocks.previewTemplateRun).not.toHaveBeenCalled();
  });

  it("preserves an explicit missing ID without falling through to a name", async () => {
    apiMocks.listAllResources.mockResolvedValue([inventoryOption("00000000-0000-4000-8000-000000000002", "Core Switch", "device_type")]);
    const template = { ...idReferenceTemplate, variables: [{ key: "device_type", label: "Device type", value_type: "resource_ref" as const, required: true,
      validation_rules: { default_id: "00000000-0000-4000-8000-000000000001", default_name: "Core Switch" } }] };
    renderDialog(vi.fn(), template);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device type" })).toHaveTextContent("00000000-0000-4000-8000-000000000001"));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, expect.objectContaining({ device_type: { resource: "device_type", id: "00000000-0000-4000-8000-000000000001" } })));
  });

  it("does not fuzzy-resolve Core-Switch to Core Switch", async () => {
    apiMocks.listAllResources.mockResolvedValue([inventoryOption("type-other", "Core Switch", "device_type")]);
    const template = { ...idReferenceTemplate, variables: [{ key: "device_type", label: "Device type", value_type: "resource_ref" as const, required: true, default_value: "Core-Switch" }] };
    renderDialog(vi.fn(), template);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device type" })).not.toBeDisabled());
    expect(screen.getByRole("combobox", { name: "Device type" })).not.toHaveTextContent("Core Switch");
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(await screen.findByText("Required")).toBeInTheDocument();
  });

  it("keeps duplicate model selections distinct by typed UUID", async () => {
    apiMocks.listAllResources.mockResolvedValue([
      { ...inventoryOption("00000000-0000-4000-8000-000000000001", "Core Switch", "device_type"), manufacturerName: "Maker A" },
      { ...inventoryOption("00000000-0000-4000-8000-000000000002", "Core Switch", "device_type"), manufacturerName: "Maker B" },
    ]);
    renderDialog(vi.fn());
    const picker = screen.getByRole("combobox", { name: "Device type" });
    await waitFor(() => expect(picker).not.toBeDisabled());
    fireEvent.click(picker);
    fireEvent.click(await screen.findByRole("option", { name: /Core Switch.*000002/ }));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(nameReferenceTemplate.id, expect.objectContaining({ device_type: { resource: "device_type", id: "00000000-0000-4000-8000-000000000002" } })));
  });

  it("allows manual name references without rewriting text", async () => {
    renderDialog(vi.fn());
    fireEvent.change(screen.getByRole("textbox", { name: "Device role" }), { target: { value: "Core-Switch" } });
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(nameReferenceTemplate.id, expect.objectContaining({ device_role: "Core-Switch" })));
  });

  it.each(["pending", "error"])("blocks Run when server preview is %s", async (state) => {
    apiMocks.previewTemplateRun.mockImplementation(() => state === "pending" ? new Promise(() => {}) : Promise.reject(new Error("unavailable")));
    const onRun = vi.fn();
    renderDialog(onRun);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const run = await screen.findByRole("button", { name: /run now/i });
    await waitFor(() => expect(run).toBeDisabled());
    fireEvent.click(run);
    expect(onRun).not.toHaveBeenCalled();
  });

  it("invalidates a previously successful server preview when previewing again", async () => {
    const onRun = vi.fn();
    renderDialog(onRun);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /run now/i })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
    apiMocks.previewTemplateRun.mockImplementation(() => new Promise(() => {}));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(screen.getByRole("button", { name: /run now/i })).toBeDisabled();
    expect(onRun).not.toHaveBeenCalled();
  });

  it("retains a confirmed UUID while a stale cached list refresh is delayed", async () => {
    let completeRefresh!: (resources: Resource[]) => void;
    const refreshed = new Promise<Resource[]>((resolve) => { completeRefresh = resolve; });
    apiMocks.listAllResources.mockImplementation((entity: string) => entity === "Device Roles"
      ? refreshed : Promise.resolve([inventoryOption("00000000-0000-4000-8000-000000000021", "Generic network appliance", "device_type")]));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(["inventory", "template-run-options", "Device Roles"], [inventoryOption("00000000-0000-4000-8000-000000000011", "Network device", "role")], { updatedAt: Date.now() - 31_000 });
    render(<QueryClientProvider client={client}><TemplateRunDialog open onOpenChange={() => {}} template={idReferenceTemplate} onRun={vi.fn()} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Network device"));
    expect(apiMocks.listAllResources).toHaveBeenCalledWith("Device Roles");
    fireEvent.click(screen.getByRole("button", { name: /create new.*device role/i }));
    fireEvent.click(screen.getByText("Confirm resource creation"));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Fresh role"));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(idReferenceTemplate.id, expect.objectContaining({ device_role: { resource: "device_role", id: "00000000-0000-4000-8000-000000000013" } })));
    completeRefresh([inventoryOption("00000000-0000-4000-8000-000000000011", "Network device", "role")]);
    await waitFor(() => expect(client.getQueryData<Resource[]>(["inventory", "template-run-options", "Device Roles"])).toHaveLength(1), { timeout: 500 });
    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
    expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("00000000-0000-4000-8000-000000000013");
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenLastCalledWith(idReferenceTemplate.id, expect.objectContaining({ device_role: { resource: "device_role", id: "00000000-0000-4000-8000-000000000013" } })));
  });

  beforeEach(() => {
    Object.values(apiMocks).forEach((mock) => mock.mockReset());
    apiMocks.listAllResources.mockImplementation((entity: string) => {
      if (entity === "Device Roles") {
        return Promise.resolve([
          inventoryOption("00000000-0000-4000-8000-000000000011", "Network device", "role"),
          inventoryOption("00000000-0000-4000-8000-000000000012", "Server", "role"),
        ]);
      }
      if (entity === "Device Types") {
        return Promise.resolve([
          inventoryOption("00000000-0000-4000-8000-000000000021", "Generic network appliance", "device_type"),
        ]);
      }
      return Promise.resolve([]);
    });
    apiMocks.previewTemplateRun.mockImplementation(previewFromVariables);
    apiMocks.createResource.mockResolvedValue(inventoryOption("00000000-0000-4000-8000-000000000013", "Fresh role", "role"));
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("turns step-bound name references into pickers of existing objects", async () => {
    renderDialog(vi.fn());

    // `device_role` / `device_type` feed the device step's name fields, so the
    // wizard lists the same inventory entity the engine resolves.
    const rolePicker = await screen.findByRole("combobox", { name: "Device role" });
    const typePicker = await screen.findByRole("combobox", { name: "Device type" });

    // Manual name/slug defaults stay unchanged; choosing an object is explicit.
    await waitFor(() => expect(rolePicker).not.toBeDisabled());
    await waitFor(() => expect(typePicker).not.toBeDisabled());
    expect(screen.getByRole("textbox", { name: "Device role" })).toHaveValue("network-device");
    expect(screen.getByRole("textbox", { name: "Device type" })).toHaveValue("generic-network-appliance");
    expect(apiMocks.listAllResources).toHaveBeenCalledWith("Device Roles");
    expect(apiMocks.listAllResources).toHaveBeenCalledWith("Device Types");

    // Creation inputs stay free text.
    expect(screen.getByDisplayValue("Default site")).toBeInTheDocument();
    expect(screen.getByDisplayValue("switch-01")).toBeInTheDocument();
  });

  it("runs with typed identity for picked name references", async () => {
    const onRun = vi.fn();
    renderDialog(onRun);

    const rolePicker = await screen.findByRole("combobox", { name: "Device role" });
    await waitFor(() => expect(rolePicker).not.toBeDisabled());
    fireEvent.click(rolePicker);
    fireEvent.click(await screen.findByRole("option", { name: /Server/ }));

    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /run now/i })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /run now/i }));

    await waitFor(() =>
      expect(onRun).toHaveBeenCalledWith(
        expect.objectContaining({
          site_name: "Default site",
          device_name: "switch-01",
          device_role: { resource: "device_role", id: "00000000-0000-4000-8000-000000000012" },
          device_type: "generic-network-appliance",
        }),
      ),
    );
  });

  it("runs with the picked UUID for id references", async () => {
    const onRun = vi.fn();
    renderDialog(onRun, idReferenceTemplate);

    const rolePicker = await screen.findByRole("combobox", { name: "Device role" });
    await waitFor(() => expect(rolePicker).toHaveTextContent("Network device"));
    fireEvent.click(rolePicker);
    fireEvent.click(await screen.findByRole("option", { name: /Server/ }));

    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));

    // The preview echoes the picked object; the payload keeps its UUID.
    const echoed = await screen.findByTitle("00000000-0000-4000-8000-000000000012");
    expect(echoed).toHaveTextContent("Server");

    fireEvent.click(screen.getByRole("button", { name: /run now/i }));
    await waitFor(() =>
      expect(onRun).toHaveBeenCalledWith(
        expect.objectContaining({ device_role: { resource: "device_role", id: "00000000-0000-4000-8000-000000000012" }, device_type: { resource: "device_type", id: "00000000-0000-4000-8000-000000000021" } }),
      ),
    );
  });

  it("keeps a name reference that matches no listed object runnable", async () => {
    const onRun = vi.fn();
    const customTemplate: InventoryTemplate = {
      ...nameReferenceTemplate,
      variables: nameReferenceTemplate.variables.map((variable) =>
        variable.key === "device_role" ? { ...variable, default_value: "legacy-role-slug" } : variable,
      ),
    };
    renderDialog(onRun, customTemplate);

    const rolePicker = await screen.findByRole("combobox", { name: "Device role" });
    await waitFor(() => expect(rolePicker).not.toBeDisabled());
    expect(screen.getByRole("textbox", { name: "Device role" })).toHaveValue("legacy-role-slug");

    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /run now/i })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /run now/i }));

    await waitFor(() =>
      expect(onRun).toHaveBeenCalledWith(expect.objectContaining({ device_role: "legacy-role-slug" })),
    );
  });

  it("requires a pick when a UUID reference matches no existing object", async () => {
    apiMocks.listAllResources.mockImplementation((entity: string) => {
      if (entity === "Device Roles") {
        return Promise.resolve([inventoryOption("role-9", "Only other role", "role")]);
      }
      if (entity === "Device Types") {
        return Promise.resolve([inventoryOption("00000000-0000-4000-8000-000000000021", "Generic network appliance", "device_type")]);
      }
      return Promise.resolve([]);
    });
    const onRun = vi.fn();
    renderDialog(onRun, idReferenceTemplate);

    const rolePicker = await screen.findByRole("combobox", { name: "Device role" });
    await waitFor(() => expect(rolePicker).not.toHaveTextContent("network-device"));

    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));

    expect(await screen.findByText("Required")).toBeInTheDocument();
    expect(onRun).not.toHaveBeenCalled();
  });
});
