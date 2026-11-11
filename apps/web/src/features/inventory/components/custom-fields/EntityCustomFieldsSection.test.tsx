import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import EntityCustomFieldsSection, { EntityCustomFieldsRead, type EntityCustomFieldsHandle } from "./EntityCustomFieldsSection";
import type { CustomFieldDefinition, CustomFieldEntityKind } from "@/features/inventory/custom-fields-api";
import { customFieldEntityKind } from "@/features/inventory/api";
import * as customFieldsApi from "@/features/inventory/custom-fields-api";
import type { Paginated } from "@/shared/api/types";

vi.mock("@/features/inventory/custom-fields-api", async () => {
  const actual = await vi.importActual("@/features/inventory/custom-fields-api");
  return {
    ...actual,
    listCustomFields: vi.fn(),
    listChoiceSets: vi.fn(),
    revealEntityCustomFields: vi.fn(),
  };
});

const listCustomFieldsMock = vi.mocked(customFieldsApi.listCustomFields);
const listChoiceSetsMock = vi.mocked(customFieldsApi.listChoiceSets);

function paginated<T>(items: T[]): Paginated<T> {
  return {
    items,
    total: items.length,
    limit: items.length || 1,
    offset: 0,
  };
}

function renderSection(customFields: Record<string, unknown> | undefined,
  entityKind: CustomFieldEntityKind = "devices", onRegister: (handle: EntityCustomFieldsHandle | null) => void = () => {}) {
  const client = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
      },
    },
  });

  return render(
    <QueryClientProvider client={client}>
      <EntityCustomFieldsSection
        entityKind={entityKind}
        entityId="device-1"
        customFields={customFields}
        onRegister={onRegister}
      />
    </QueryClientProvider>,
  );
}

function buildDefinition(overrides: Partial<CustomFieldDefinition> = {}): CustomFieldDefinition {
  return {
    id: "cf-1",
    name: "artix_version",
    label: "Artix версия",
    field_type: "text",
    description: "",
    group_name: "",
    required: false,
    enforce_unique: false,
    is_secret: false,
    default_value: null,
    weight: 100,
    validation_minimum: null,
    validation_maximum: null,
    validation_regex: "",
    validation_schema: null,
    choice_set_id: null,
    related_object_type_id: null,
    related_object_filter: null,
    ui_visible: "if-set",
    ui_editable: "yes",
    is_cloneable: true,
    search_weight: 0,
    filter_logic: "loose",
    object_types: ["ct-device"],
    created_at: "2026-06-16T00:00:00Z",
    updated_at: "2026-06-16T00:00:00Z",
    ...overrides,
  };
}

describe("EntityCustomFieldsSection", () => {
  beforeEach(() => {
    vi.mocked(customFieldsApi.revealEntityCustomFields).mockReset();
    listChoiceSetsMock.mockResolvedValue(paginated([]));
    listCustomFieldsMock.mockResolvedValue(paginated([]));
  });

  it("retains an untouched datetime instant when another custom field is edited", async () => {
    listCustomFieldsMock.mockResolvedValue(paginated([
      buildDefinition({ field_type: "integer" }),
      buildDefinition({ id: "date", name: "observed_at", label: "Observed at", field_type: "datetime" }),
    ]));
    const instant = "2026-10-25T02:30:00+01:00";
    let handle: EntityCustomFieldsHandle | null = null;
    renderSection({ artix_version: 1, observed_at: instant }, "devices", value => { handle = value; });
    const input = await screen.findByRole("spinbutton");
    fireEvent.change(input, { target: { value: "2" } });
    await waitFor(() => expect(handle?.buildPayload()).toEqual({ artix_version: 2, observed_at: instant }));
  });

  it("renders unset if-set fields in the edit form so they can be filled", async () => {
    listCustomFieldsMock.mockResolvedValue(paginated([buildDefinition()]));

    renderSection({ artix_version: null });

    expect(await screen.findByText("Artix версия")).toBeInTheDocument();
    expect(screen.getByRole("textbox")).toBeInTheDocument();
  });

  it("still hides fields that are explicitly hidden from editing", async () => {
    listCustomFieldsMock.mockResolvedValue(
      paginated([buildDefinition({ ui_editable: "hidden" })]),
    );

    renderSection({ artix_version: null });

    expect(await screen.findByText("No custom fields apply to this entity type.")).toBeInTheDocument();
    expect(screen.queryByText("Artix версия")).not.toBeInTheDocument();
  });

  it.each([
    "FHRP Groups", "Wireless LAN Groups", "Wireless LANs", "Wireless Links",
  ] as const)("uses the %s registration for typed edits without writing the secret mask", async (entity) => {
    listCustomFieldsMock.mockResolvedValue(paginated([
      buildDefinition({ name: "priority_note", label: "Priority note", field_type: "integer", ui_visible: "always" }),
      buildDefinition({ id: "cf-secret", name: "shared_secret", label: "Shared secret", is_secret: true, ui_visible: "always" }),
    ]));
    const kind = customFieldEntityKind(entity);
    if (!kind) throw new Error(`Missing ${entity} custom-field registration`);
    let handle: EntityCustomFieldsHandle | null = null;
    renderSection({ priority_note: 10, shared_secret: "****" }, kind, value => { handle = value; });
    const input = await screen.findByRole("spinbutton");
    await waitFor(() => expect(handle?.buildPayload()).toBeUndefined());
    fireEvent.change(input, { target: { value: "20" } });
    await waitFor(() => expect(handle?.buildPayload()).toEqual({ priority_note: 20 }));
    expect(customFieldsApi.revealEntityCustomFields).not.toHaveBeenCalled();
    const secret = screen.getByPlaceholderText("****");
    expect(secret).toHaveValue("");
    fireEvent.change(secret, { target: { value: "synthetic-replacement" } });
    await waitFor(() => expect(handle?.buildPayload()).toEqual({
      priority_note: 20, shared_secret: "synthetic-replacement",
    }));
  });

  it.each([
    ["FHRP Groups", "fhrp-groups"],
    ["Wireless LAN Groups", "wireless_lan_groups"],
    ["Wireless LANs", "wireless_lans"],
    ["Wireless Links", "wireless_links"],
  ] as const)("reveals %s secrets only through an explicit canonical reveal action", async (entity, kind) => {
    listCustomFieldsMock.mockResolvedValue(paginated([
      buildDefinition({ name: "shared_secret", label: "Shared secret", is_secret: true, ui_visible: "always" }),
    ]));
    vi.mocked(customFieldsApi.revealEntityCustomFields).mockResolvedValue({ shared_secret: "synthetic-reveal-value" });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><EntityCustomFieldsRead
      entityKind={customFieldEntityKind(entity) ?? null} entityId="entity-1"
      customFields={{ shared_secret: "****" }} /></QueryClientProvider>);
    expect(await screen.findByText("****")).toBeInTheDocument();
    expect(screen.queryByText("synthetic-reveal-value")).not.toBeInTheDocument();
    expect(customFieldsApi.revealEntityCustomFields).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    expect(await screen.findByText("synthetic-reveal-value")).toBeInTheDocument();
    expect(customFieldsApi.revealEntityCustomFields).toHaveBeenCalledExactlyOnceWith(kind, "entity-1");
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByText("synthetic-reveal-value")).not.toBeInTheDocument();
    expect(screen.getByText("****")).toBeInTheDocument();
    expect(customFieldsApi.revealEntityCustomFields).toHaveBeenCalledTimes(1);
  });

  it.each(["Wireless LAN Groups", "Wireless LANs", "Wireless Links"] as const)(
    "keeps %s secrets masked when explicit reveal is denied", async (entity) => {
      listCustomFieldsMock.mockResolvedValue(paginated([
        buildDefinition({ name: "shared_secret", label: "Shared secret", is_secret: true, ui_visible: "always" }),
      ]));
      vi.mocked(customFieldsApi.revealEntityCustomFields).mockRejectedValue({ status: 403 });
      const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      render(<QueryClientProvider client={client}><EntityCustomFieldsRead
        entityKind={customFieldEntityKind(entity) ?? null} entityId="entity-1"
        customFields={{ shared_secret: "****" }} /></QueryClientProvider>);
      expect(await screen.findByText("****")).toBeInTheDocument();
      expect(customFieldsApi.revealEntityCustomFields).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
      expect(await screen.findByRole("alert")).toBeInTheDocument();
      expect(screen.getByText("****")).toBeInTheDocument();
    },
  );
});
