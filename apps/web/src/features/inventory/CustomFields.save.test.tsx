import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import type { ChoiceSet, CustomFieldDefinition } from "./custom-fields-api";
import type * as CustomFieldsApi from "./custom-fields-api";
import CustomFields from "./CustomFields";

const api = vi.hoisted(() => ({
  listCustomFields: vi.fn(), listChoiceSets: vi.fn(), listContentTypes: vi.fn(),
  createCustomField: vi.fn(), updateCustomField: vi.fn(),
  createChoiceSet: vi.fn(), updateChoiceSet: vi.fn(),
}));
vi.mock("./custom-fields-api", async importOriginal => ({
  ...await importOriginal<typeof CustomFieldsApi>(), ...api,
}));
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

const choiceSet: ChoiceSet = {
  id: "choices-priority", name: "Priority", description: "", choice_colors: {},
  extra_choices: [{ value: "low", label: "Low" }], order_alphabetically: false,
  created_at: "2026-07-06T00:00:00Z", updated_at: "2026-07-06T00:00:00Z",
};
const definition: CustomFieldDefinition = {
  id: "def-owner", name: "cf_owner", label: "Owner", field_type: "text",
  description: "", group_name: "", required: false, enforce_unique: false,
  is_secret: false, default_value: null, weight: 100, validation_minimum: null,
  validation_maximum: null, validation_regex: "", validation_schema: null,
  choice_set_id: null, related_object_type_id: null, related_object_filter: null,
  ui_visible: "always", ui_editable: "yes", is_cloneable: true, search_weight: 0,
  filter_logic: "loose", object_types: [],
  created_at: "2026-07-06T00:00:00Z", updated_at: "2026-07-06T00:00:00Z",
};

function deferred() {
  let resolve!: (value: ChoiceSet | CustomFieldDefinition) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<ChoiceSet | CustomFieldDefinition>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

describe("CustomFields save lifetime", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listCustomFields.mockResolvedValue({ items: [definition], total: 1 });
    api.listChoiceSets.mockResolvedValue({ items: [choiceSet], total: 1 });
    api.listContentTypes.mockResolvedValue([]);
  });

  it.each([
    ["definition", "create"], ["definition", "update"],
    ["choice-set", "create"], ["choice-set", "update"],
  ] as const)("retains the %s %s draft until a successful retry", async (resource, operation) => {
    const isDefinition = resource === "definition";
    const isCreate = operation === "create";
    const mutation = isDefinition
      ? (isCreate ? api.createCustomField : api.updateCustomField)
      : (isCreate ? api.createChoiceSet : api.updateChoiceSet);
    const first = deferred();
    const retry = deferred();
    mutation.mockReturnValueOnce(first.promise).mockReturnValueOnce(retry.promise);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{
      roles: [], isAdmin: true, can: () => true, canAny: () => true, hasRole: () => true,
    }}><CustomFields /></PermissionsContext.Provider></QueryClientProvider>);
    await screen.findByText(definition.label);
    if (!isDefinition) {
      fireEvent.mouseDown(screen.getByRole("tab", { name: /choice sets/i }), { button: 0, ctrlKey: false });
      await screen.findByText(choiceSet.name);
    }
    if (isCreate) {
      fireEvent.click(screen.getByRole("button", { name: isDefinition ? /new field/i : /new choice set/i }));
    } else {
      fireEvent.click(screen.getByText(isDefinition ? definition.label : choiceSet.name));
    }
    const editor = screen.getByTestId("entity-editor-page");
    const form = within(editor);
    const draftInput = isDefinition ? form.getByPlaceholderText("Owner") : form.getByRole("textbox", { name: /name/i });
    if (isDefinition && isCreate) fireEvent.change(form.getByPlaceholderText("cf_owner"), { target: { value: "cf_owner" } });
    if (!isDefinition && isCreate) fireEvent.change(form.getAllByRole("textbox")[2]!, { target: { value: "prod" } });
    fireEvent.change(draftInput, { target: { value: "Retained draft" } });
    const submit = form.getByRole("button", { name: isCreate ? /^create$/i : /save changes/i });
    fireEvent.click(submit);
    await waitFor(() => expect(mutation).toHaveBeenCalledTimes(1));
    expect(editor).toBeInTheDocument();
    expect(draftInput).toHaveValue("Retained draft");
    await waitFor(() => expect(submit).toBeDisabled());
    expect(form.getByRole("button", { name: /^back$/i })).toBeDisabled();
    expect(form.getByRole("button", { name: /^cancel$/i })).toBeDisabled();
    expect(notify).not.toHaveBeenCalled();

    await act(async () => { first.reject(new Error("Save rejected")); await Promise.resolve(); });
    await waitFor(() => expect(submit).toBeEnabled());
    expect(editor).toBeInTheDocument();
    expect(draftInput).toHaveValue("Retained draft");
    expect(notify).toHaveBeenCalledWith("error", "Save rejected");
    expect(notify).not.toHaveBeenCalledWith("success", expect.anything());

    fireEvent.click(submit);
    await waitFor(() => expect(mutation).toHaveBeenCalledTimes(2));
    expect(mutation.mock.calls[1]).toEqual(mutation.mock.calls[0]);
    const body = mutation.mock.calls[0]![isCreate ? 0 : 1];
    expect(body).toEqual(expect.objectContaining(isDefinition ? { label: "Retained draft" } : { name: "Retained draft" }));
    if (!isCreate) expect(mutation.mock.calls[0]![0]).toBe(isDefinition ? definition.id : choiceSet.id);
    expect(editor).toBeInTheDocument();
    await act(async () => { retry.resolve(isDefinition ? definition : choiceSet); await Promise.resolve(); });
    await waitFor(() => expect(screen.queryByTestId("entity-editor-page")).not.toBeInTheDocument());
    expect(notify).toHaveBeenCalledWith("success", expect.any(String));
  });
});
