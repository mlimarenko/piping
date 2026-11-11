import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import type { EntityMapping, FieldMapping, Transform } from "@/shared/api/types";
import { changeAppLanguage } from "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import ReverseSlot from "./ReverseSlot";

const suggestReverse = vi.hoisted(() => vi.fn());
vi.mock("@/features/inventory/sync-mapping-api", async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(), suggestReverse,
}));

it("delivers a deferred suggestion to its originating authoring callback, not a newer render", async () => {
  await changeAppLanguage("en");
  const field: FieldMapping = { id: "field-1", source_field: "name", target_field: "name", enabled: true,
    forward_transform: { operator: "trim", args: { field: "name" } }, reverse_transform: null };
  const entity: EntityMapping = { id: "entity-1", source_entity: "dcim.devices", target_entity: "Devices", enabled: true, field_mappings: [field] };
  let resolveSuggestion!: (result: { reverse_transform: Transform }) => void;
  const pending = new Promise<{ reverse_transform: Transform }>(resolve => { resolveSuggestion = resolve; });
  suggestReverse.mockReturnValue(pending);
  const original = vi.fn();
  const replacement = vi.fn();
  const client = new QueryClient();
  const view = (onUpdate: (reverse: Transform | null) => void) => <QueryClientProvider client={client}>
    <ReverseSlot fieldMapping={field} entityMapping={entity} onUpdate={onUpdate} />
  </QueryClientProvider>;
  const { rerender } = render(view(original));
  fireEvent.click(screen.getByRole("button", { name: en.sync.mapping.suggestReverse }));
  await waitFor(() => expect(suggestReverse).toHaveBeenCalledTimes(1));
  rerender(view(replacement));
  const reverse: Transform = { operator: "identity", args: { field: "name" } };
  await act(async () => { resolveSuggestion({ reverse_transform: reverse }); await pending; });
  await waitFor(() => expect(original).toHaveBeenCalledWith(reverse));
  expect(replacement).not.toHaveBeenCalled();
});
