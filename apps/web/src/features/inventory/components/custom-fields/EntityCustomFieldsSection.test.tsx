import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import EntityCustomFieldsSection from "./EntityCustomFieldsSection";
import type { CustomFieldDefinition } from "@/features/inventory/custom-fields-api";
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

function renderSection(customFields: Record<string, unknown> | undefined) {
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
        entityKind="devices"
        entityId="device-1"
        customFields={customFields}
        onRegister={() => {}}
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
    listChoiceSetsMock.mockResolvedValue(paginated([]));
    listCustomFieldsMock.mockResolvedValue(paginated([]));
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
});
