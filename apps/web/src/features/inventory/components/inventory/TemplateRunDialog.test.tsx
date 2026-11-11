import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import TemplateRunDialog from "./TemplateRunDialog";
import { ApiError, apiErrorFromResponse } from "@/shared/api/client";
import type { InventoryTemplate, Resource } from "@/shared/api/types";

vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({ can: () => true }),
}));

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
  it("puts required inputs without defaults first across sections and keeps their order while editing", async () => {
    const defaultRoleId = "00000000-0000-4000-8000-000000000031";
    apiMocks.listAllResources.mockResolvedValue([inventoryOption(defaultRoleId, "Configured role", "contact_role")]);
    const template: InventoryTemplate = {
      ...nameReferenceTemplate,
      id: "tpl-required-first",
      variables: [
        { key: "notes", label: "Notes", value_type: "string", default_value: "Keep me" },
        { key: "site_status", label: "Site status", value_type: "string", required: true, default_value: "planned" },
        { key: "role", label: "Required role", value_type: "resource_ref", required: true, default_value: null, validation_rules: { resource: "contact_role" } },
        { key: "count", label: "Count", value_type: "integer", required: true, default_value: 0 },
        { key: "flag", label: "Feature flag", value_type: "boolean", required: true, default_value: false },
        { key: "site_name", label: "Site name", value_type: "string", required: true, default_value: null },
        { key: "manual", label: "Manual input", value_type: "string", required: true, default_value: "  " },
        { key: "default_role", label: "Default role", value_type: "resource_ref", required: true, default_value: null, validation_rules: { resource: "contact_role", default_id: defaultRoleId } },
        { key: "last", label: "Last required input", value_type: "string", required: true },
      ],
      steps: [{ ...siteStep, payload_template: { name: "{{site_name}}", status: "{{site_status}}" } }],
    };
    renderDialog(vi.fn(), template);
    const order = () => Array.from(document.querySelectorAll('[id^="run-field-"]')).map((field) => field.id.replace("run-field-", ""));
    const expected = ["site_name", "role", "manual", "last", "site_status", "default_role", "notes", "count", "flag"];
    expect(order()).toEqual(expected);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Default role" })).toHaveTextContent("Configured role"));
    expect(order()).toEqual(expected);
    expect(within(screen.getByRole("region", { name: "Related objects" })).getByRole("combobox", { name: "Default role" })).toBeInTheDocument();
    expect(screen.getByRole("spinbutton", { name: "Count" })).toHaveValue(0);
    expect(screen.getByRole("combobox", { name: "Feature flag" })).toHaveTextContent("No");

    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    const name = screen.getByRole("textbox", { name: "Site name" });
    await waitFor(() => expect(name).toHaveFocus());
    fireEvent.change(name, { target: { value: "Entered site" } });
    expect(name).toHaveValue("Entered site");
    expect(name).toHaveFocus();
    expect(order()).toEqual(expected);
  });

  it("prioritizes required collection fields without changing row values or defaults", async () => {
    const template: InventoryTemplate = {
      ...nameReferenceTemplate,
      id: "tpl-row-required-first",
      variables: [{ key: "rows", label: "Entries", value_type: "json", default_value: [{}], validation_rules: { fields: [
        { key: "notes", label: "Notes", value_type: "string", default_value: "Keep row" },
        { key: "zero", label: "Zero", value_type: "integer", required: true, default_value: 0 },
        { key: "flag", label: "Enabled", value_type: "boolean", required: true, default_value: false },
        { key: "needed", label: "Required text", value_type: "string", required: true, default_value: null },
        { key: "later", label: "Another required text", value_type: "string", required: true },
        { key: "ready", label: "Ready", value_type: "string", required: true, default_value: "Ready row" },
      ] } }],
      steps: [siteStep],
    };
    renderDialog(vi.fn(), template);
    const order = () => Array.from(document.querySelectorAll('[id^="run-field-rows.0."]')).map((field) => field.id.replace("run-field-rows.0.", ""));
    const expected = ["needed", "later", "notes", "zero", "flag", "ready"];
    expect(order()).toEqual(expected);
    fireEvent.change(screen.getByRole("textbox", { name: "Required text - row 1" }), { target: { value: "Entered row" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Another required text - row 1" }), { target: { value: "Second value" } });
    expect(order()).toEqual(expected);
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, {
      rows: [{ notes: "Keep row", zero: 0, flag: false, needed: "Entered row", later: "Second value", ready: "Ready row" }],
    }));
  });

  it.each(["", null])("submits explicit optional collection clears %j without resurrecting email or reference defaults", async (empty) => {
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [{ key: "people", label: "People", value_type: "json", default_value: [{ email: empty, role_id: empty }], validation_rules: { fields: [
      { key: "email", label: "Email", value_type: "string", default_value: "alex@example.test", validation_rules: { format: "email" } },
      { key: "role_id", label: "Role", value_type: "resource_ref", default_value: "00000000-0000-4000-8000-000000000001", validation_rules: { resource: "contact_role" } },
    ] } }] };
    renderDialog(vi.fn(), template);
    expect(screen.getByRole("textbox", { name: "Email - row 1" })).toHaveValue("");
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, { people: [{ email: empty, role_id: empty }] }));
  });

  it("submits user-cleared defaults and keeps untouched absent top-level values omitted", async () => {
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [
      { key: "title", label: "Title", value_type: "string", default_value: "Alex" },
      { key: "count", label: "Count", value_type: "integer", default_value: 3 },
      { key: "metadata", label: "Metadata", value_type: "json", default_value: { tier: "edge" } },
      { key: "absent", label: "Absent", value_type: "string" },
      { key: "people", label: "People", value_type: "json", default_value: [{}], validation_rules: { fields: [
        { key: "email", label: "Email", value_type: "string", default_value: "alex@example.test", validation_rules: { format: "email" } },
      ] } },
    ] };
    const onRun = vi.fn();
    renderDialog(onRun, template);
    expect(screen.getByRole("textbox", { name: "Email - row 1" })).toHaveValue("alex@example.test");
    for (const label of ["Title", "Metadata", "Email - row 1"]) fireEvent.change(screen.getByRole("textbox", { name: label }), { target: { value: "" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "Count" }), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const values = { title: "", count: null, metadata: null, people: [{ email: "" }] };
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, values));
    await waitFor(() => expect(screen.getByRole("button", { name: /^run now/i })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^run now/i }));
    expect(onRun).toHaveBeenCalledWith(values);
  });
  it.each([{}, { name: "" }])("applies only missing collection field defaults when opening saved rows %j", async (savedRow) => {
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [{ key: "people", label: "People", value_type: "json", default_value: [savedRow], validation_rules: { fields: [
      { key: "name", label: "Name", value_type: "string", required: true, default_value: "Alex" },
    ] } }] };
    renderDialog(vi.fn(), template);
    const hasExplicitBlank = Object.hasOwn(savedRow, "name");
    expect(screen.getByRole("textbox", { name: "Name - row 1" })).toHaveValue(hasExplicitBlank ? "" : "Alex");
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    if (hasExplicitBlank) {
      expect(screen.getByRole("textbox", { name: "Name - row 1" })).toHaveAccessibleDescription("Row 1: Required");
      expect(apiMocks.previewTemplateRun).not.toHaveBeenCalled();
    } else {
      await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, { people: [{ name: "Alex" }] }));
      fireEvent.click(screen.getByRole("button", { name: "Back" }));
      fireEvent.change(screen.getByRole("textbox", { name: "Name - row 1" }), { target: { value: "" } });
      fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
      expect(screen.getByRole("textbox", { name: "Name - row 1" })).toHaveValue("");
      expect(screen.getByRole("textbox", { name: "Name - row 1" })).toHaveAccessibleDescription("Row 1: Required");
      expect(apiMocks.previewTemplateRun).toHaveBeenCalledTimes(1);
    }
  });
  it("clears changed collection errors after correction and removal without reassigning deleted row errors", async () => {
    const firstContact = "00000000-0000-4000-8000-000000000031";
    const secondContact = "00000000-0000-4000-8000-000000000032";
    const role = "00000000-0000-4000-8000-000000000033";
    apiMocks.listAllResources.mockImplementation((entity: string) => Promise.resolve(entity === "Contacts"
      ? [inventoryOption(firstContact, "Alex", "contact"), inventoryOption(secondContact, "Sam", "contact")]
      : [inventoryOption(role, "Operations", "contact_role")]));
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [
      { key: "batch", label: "Batch", value_type: "string", required: true },
      { key: "contacts", label: "Contacts", value_type: "json", default_value: [
        { contact_id: firstContact, email: "not-email" },
        { contact_id: secondContact, role_id: role, email: "" },
      ], validation_rules: { fields: [
        { key: "contact_id", label: "Contact", value_type: "resource_ref", required: true, validation_rules: { resource: "contact" } },
        { key: "role_id", label: "Contact role", value_type: "resource_ref", required: true, validation_rules: { resource: "contact_role" } },
        { key: "email", label: "Email", value_type: "string", validation_rules: { format: "email" } },
      ] } },
    ] };
    renderDialog(vi.fn(), template);
    const preview = screen.getByRole("button", { name: /^preview/i });
    const rolePicker = screen.getByRole("combobox", { name: "Contact role - row 1" });
    await waitFor(() => expect(rolePicker).not.toBeDisabled());
    fireEvent.click(preview);
    expect(rolePicker).toHaveAttribute("aria-invalid", "true");
    fireEvent.click(rolePicker);
    fireEvent.click(await screen.findByRole("option", { name: "Operations" }));
    expect(rolePicker).toHaveAttribute("aria-invalid", "false");
    expect(rolePicker).not.toHaveAccessibleDescription(/Required/);
    expect(screen.getByRole("textbox", { name: "Batch" })).toHaveAttribute("aria-invalid", "true");
    fireEvent.click(preview);
    expect(screen.getByRole("textbox", { name: "Email - row 1" })).toHaveAccessibleDescription("Row 1: Enter a valid email address.");
    fireEvent.click(screen.getByRole("button", { name: "Remove Contacts row 1" }));
    expect(screen.getByRole("combobox", { name: "Contact - row 1" })).toHaveTextContent("Sam");
    const survivingEmail = screen.getByRole("textbox", { name: "Email - row 1" });
    expect(survivingEmail).toHaveValue("");
    expect(survivingEmail).toHaveAttribute("aria-invalid", "false");
    expect(survivingEmail).not.toHaveAccessibleDescription();
    expect(screen.queryByText(/Enter a valid email address/)).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Batch" })).toHaveAttribute("aria-invalid", "true");
  });
  it("edits repeatable object rows with UUID pickers and options, validates rows and preserves values on Back", async () => {
    const contactId = "00000000-0000-4000-8000-000000000031";
    const roleId = "00000000-0000-4000-8000-000000000032";
    apiMocks.listAllResources.mockImplementation((entity: string) => Promise.resolve(entity === "Contacts" ? [inventoryOption(contactId, "Alex", "contact")] : [inventoryOption(roleId, "Operations", "contact_role")]));
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [{ key: "people", label: "People", value_type: "json", validation_rules: { fields: [
      { key: "contact_id", label: "Contact", value_type: "resource_ref", required: true, validation_rules: { resource: "contact" } },
      { key: "role_id", label: "Contact role", value_type: "resource_ref", required: true, validation_rules: { resource: "contact_role" } },
      { key: "priority", label: "Priority", value_type: "string", default_value: "primary", validation_rules: { options: ["primary", "secondary"] } },
    ] } }] };
    const onRun = vi.fn();
    renderDialog(onRun, template);
    fireEvent.click(screen.getByRole("button", { name: "Add People row" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Contact - row 1" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(screen.getByRole("combobox", { name: "Contact - row 1" })).toHaveFocus();
    expect(screen.getByRole("combobox", { name: "Contact - row 1" })).toHaveAccessibleDescription("Row 1: Required");
    expect(apiMocks.previewTemplateRun).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("combobox", { name: "Contact - row 1" }));
    fireEvent.click(await screen.findByRole("option", { name: "Alex" }));
    fireEvent.click(screen.getByRole("combobox", { name: "Contact role - row 1" }));
    fireEvent.click(await screen.findByRole("option", { name: "Operations" }));
    fireEvent.click(screen.getByRole("combobox", { name: "Priority - row 1" }));
    fireEvent.click(await screen.findByRole("option", { name: "secondary" }));
    fireEvent.click(screen.getByRole("button", { name: "Add People row" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove People row 2" }));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const values = { people: [{ contact_id: contactId, role_id: roleId, priority: "secondary" }] };
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, values));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("combobox", { name: "Contact - row 1" })).toHaveTextContent("Alex");
    expect(screen.getByRole("combobox", { name: "Priority - row 1" })).toHaveTextContent("secondary");
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /^run now/i })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^run now/i }));
    expect(onRun).toHaveBeenCalledWith(values);
  });

  it("renders ordinary collection fields and retains malformed saved JSON until corrected", async () => {
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [{ key: "people", label: "People", value_type: "json", default_value: "{broken", validation_rules: { fields: [
      { key: "name", label: "Name", value_type: "string", required: true },
      { key: "email", label: "Email", value_type: "string", validation_rules: { format: "email" } },
      { key: "count", label: "Count", value_type: "integer" },
    ] } }] };
    renderDialog(vi.fn(), template);
    expect(screen.getByRole("textbox", { name: "People" })).toHaveValue("{broken");
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(apiMocks.previewTemplateRun).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "People" }), { target: { value: "[]" } });
    fireEvent.click(screen.getByRole("button", { name: "Add People row" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Name - row 1" }), { target: { value: "Alex" } });
    const email = screen.getByRole("textbox", { name: "Email - row 1" });
    expect(email).toHaveAttribute("type", "email");
    fireEvent.change(email, { target: { value: "invalid" } });
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(email).toHaveAccessibleDescription("Row 1: Enter a valid email address.");
    fireEvent.change(email, { target: { value: "alex@example.com" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "Count - row 1" }), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, { people: [{ name: "Alex", email: "alex@example.com", count: 2 }] }));
  });
  it("renders generic status options and blocks an invalid stored default with associated focus", async () => {
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [{ key: "status", label: "Status", value_type: "string", default_value: "unknown-status", validation_rules: { options: ["active", "planned"] } }] };
    renderDialog(vi.fn(), template);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const control = screen.getByRole("combobox", { name: "Status" });
    expect(control).toHaveFocus();
    expect(control).toHaveAttribute("aria-invalid", "true");
    expect(control).toHaveAccessibleDescription("Select one of the available values.");
    expect(apiMocks.previewTemplateRun).not.toHaveBeenCalled();
    fireEvent.click(control);
    fireEvent.click(await screen.findByRole("option", { name: "planned" }));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(template.id, { status: "planned" }));
  });

  it.each(["preview", "run"])("associates safe server field errors and focuses the rejected field after %s", async (phase) => {
    const failure = await apiErrorFromResponse(Response.json({ code: "VALIDATION_ERROR", message: "private backend trace", details: { fields: { device_name: "private database constraint", unknown_field: "private" } } }, { status: 422 }));
    const onRun = vi.fn().mockRejectedValue(failure);
    if (phase === "preview") apiMocks.previewTemplateRun.mockRejectedValueOnce(failure);
    renderDialog(onRun);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    if (phase === "run") {
      const run = await screen.findByRole("button", { name: /^run now/i });
      await waitFor(() => expect(run).not.toBeDisabled());
      fireEvent.click(run);
    }
    const field = await screen.findByRole("textbox", { name: "Device name" });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveAccessibleDescription(/Check this value, then preview again/);
    expect(screen.queryByText(/private/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^run now/i })).not.toBeInTheDocument();
  });

  it.each([[403, /permissions/], [409, /conflict policy/], [422, /template steps/]] as const)("provides safe recovery instructions for HTTP %s", async (status, message) => {
    apiMocks.previewTemplateRun.mockRejectedValueOnce(new ApiError({ status, message: "private backend trace" }));
    renderDialog(vi.fn());
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(screen.queryByText(/private backend/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^run now/i })).toBeDisabled();
  });

  it("round-trips canonical nested details.fields into row validation and focus", async () => {
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [{ key: "contacts", label: "Contacts", value_type: "json", default_value: [{ role_id: "00000000-0000-4000-8000-000000000032" }], validation_rules: { fields: [{ key: "role_id", label: "Contact role", value_type: "resource_ref", required: true, validation_rules: { resource: "contact_role" } }] } }] };
    const failure = await apiErrorFromResponse(Response.json({ code: "VALIDATION_ERROR", message: "private trace", details: { fields: { "contacts.0.role_id": "required" } } }, { status: 422 }));
    apiMocks.previewTemplateRun.mockRejectedValueOnce(failure);
    renderDialog(vi.fn(), template);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const field = await screen.findByRole("combobox", { name: "Contact role - row 1" });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveAccessibleDescription("Row 1: Required");
    expect(screen.queryByText("private trace")).not.toBeInTheDocument();
  });

  it("uses canonical range errors with localized template bounds, not unrelated top-level fields", async () => {
    const template: InventoryTemplate = { ...nameReferenceTemplate, variables: [{ key: "count", label: "Count", value_type: "integer", default_value: 11, validation_rules: { min: 1, max: 5 } }] };
    apiMocks.previewTemplateRun.mockRejectedValueOnce(new ApiError({ status: 422, message: "private trace", fields: { count: "required" }, details: { fields: { count: "invalid_range" } } }));
    renderDialog(vi.fn(), template);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    const field = await screen.findByRole("spinbutton", { name: "Count" });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveAccessibleDescription("Enter a value from 1 to 5.");
  });

  it("does not read top-level field errors as a second envelope", async () => {
    apiMocks.previewTemplateRun.mockRejectedValueOnce(new ApiError({ status: 422, message: "private trace", fields: { device_name: "required" }, details: { fields: ["invalid"] } }));
    renderDialog(vi.fn());
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("template steps");
    expect(screen.queryByRole("textbox", { name: "Device name" })).not.toBeInTheDocument();
  });
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

  it("requires a fresh preview after a rejected run instead of enabling an immediate duplicate", async () => {
    const onRun = vi.fn().mockRejectedValue(new Error("unconfirmed"));
    renderDialog(onRun);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /^run now/i })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^run now/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Check run history");
    expect(screen.getByRole("button", { name: /^run now/i })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Retry check" }));
    await waitFor(() => expect(screen.getByRole("button", { name: /^run now/i })).not.toBeDisabled());
    expect(onRun).toHaveBeenCalledTimes(1);
    expect(apiMocks.previewTemplateRun).toHaveBeenCalledTimes(2);
  });

  it("does not authorize values changed by late resource-default resolution", async () => {
    let resolveOptions!: (resources: Resource[]) => void;
    apiMocks.listAllResources.mockImplementation(() => new Promise<Resource[]>((resolve) => { resolveOptions = resolve; }));
    const template: InventoryTemplate = { ...idReferenceTemplate, variables: [{ key: "device_role", label: "Device role", value_type: "resource_ref", default_value: "Network device" }] };
    const onRun = vi.fn();
    renderDialog(onRun, template);
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /^run now/i })).not.toBeDisabled());
    await act(async () => { resolveOptions([inventoryOption("00000000-0000-4000-8000-000000000011", "Network device", "role")]); await Promise.resolve(); });
    await waitFor(() => expect(screen.getByRole("button", { name: /^run now/i })).toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^run now/i }));
    expect(onRun).not.toHaveBeenCalled();
  });

  it("retains a confirmed UUID while a stale cached list refresh is delayed", async () => {
    let completeRefresh!: (resources: Resource[]) => void;
    const refreshed = new Promise<Resource[]>((resolve) => { completeRefresh = resolve; });
    apiMocks.listAllResources.mockImplementation((entity: string) => entity === "Device Roles"
      ? refreshed : Promise.resolve([inventoryOption("00000000-0000-4000-8000-000000000021", "Generic network appliance", "device_type")]));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(["inventory", "resources", "Device Roles", "options"], { items: [inventoryOption("00000000-0000-4000-8000-000000000011", "Network device", "role")], total: 1, limit: 1000, offset: 0 }, { updatedAt: Date.now() - 31_000 });
    render(<QueryClientProvider client={client}><TemplateRunDialog open onOpenChange={() => {}} template={idReferenceTemplate} onRun={vi.fn()} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Network device"));
    expect(apiMocks.listAllResources).toHaveBeenCalledWith("Device Roles");
    fireEvent.click(screen.getByRole("button", { name: /create new.*device role/i }));
    fireEvent.click(screen.getByText("Confirm resource creation"));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Fresh role"));
    fireEvent.click(screen.getByRole("button", { name: /^preview/i }));
    await waitFor(() => expect(apiMocks.previewTemplateRun).toHaveBeenCalledWith(idReferenceTemplate.id, expect.objectContaining({ device_role: { resource: "device_role", id: "00000000-0000-4000-8000-000000000013" } })));
    completeRefresh([inventoryOption("00000000-0000-4000-8000-000000000011", "Network device", "role")]);
    await waitFor(() => expect(client.getQueryData<{ items: Resource[] }>(["inventory", "resources", "Device Roles", "options"])?.items).toHaveLength(2), { timeout: 500 });
    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
    expect(screen.getByRole("combobox", { name: "Device role" })).toHaveTextContent("Fresh role");
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
