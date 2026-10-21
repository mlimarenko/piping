import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import "@/shared/i18n";
import { changeAppLanguage } from "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import type { EntityMapping, MappingTarget } from "@/shared/api/types";

import AddEntityDialog from "./AddEntityDialog";

const mappingApiMocks = vi.hoisted(() => ({
  listLocalInventoryResources: vi.fn(),
}));
vi.mock("@/features/inventory/sync-mapping-api", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return {
    ...actual,
    listLocalInventoryResources: mappingApiMocks.listLocalInventoryResources,
  };
});

const LOCAL_TARGET: MappingTarget = { kind: "local_inventory" };
const PROVIDER_TARGET: MappingTarget = {
  kind: "provider",
  provider_id: "11111111-1111-4111-8111-111111111111",
};

function renderDialog(
  target: MappingTarget,
  onAdded: (em: EntityMapping) => boolean,
): void {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <AddEntityDialog
        open
        onOpenChange={() => {}}
        providerId="provider-1"
        target={target}
        onAdded={onAdded}
      />
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  vi.clearAllMocks();
  await changeAppLanguage("en");
  mappingApiMocks.listLocalInventoryResources.mockResolvedValue({
    resources: [
      { sourceEntity: "dcim.sites", targetEntity: "sites", fields: [] },
      { sourceEntity: "dcim.devices", targetEntity: "devices", fields: [] },
    ],
    providerEnabled: true,
  });
});

describe("AddEntityDialog catalog source", () => {
  it("lists entities from the local inventory catalogue and adds the canonical pair", async () => {
    const onAdded = vi.fn(() => true);
    renderDialog(LOCAL_TARGET, onAdded);

    expect(await screen.findByText("dcim.devices")).toBeInTheDocument();
    fireEvent.click(screen.getByText("dcim.devices"));
    fireEvent.click(screen.getByRole("button", { name: en.add }));

    expect(onAdded).toHaveBeenCalledWith(
      expect.objectContaining({
        source_entity: "dcim.devices",
        target_entity: "devices",
      }),
    );
  });

  it("does not offer a fake registry for a remote provider target", async () => {
    renderDialog(PROVIDER_TARGET, vi.fn());

    expect(
      await screen.findByText(en.sync.mapping.localTargetRequired),
    ).toBeInTheDocument();
    expect(mappingApiMocks.listLocalInventoryResources).not.toHaveBeenCalled();
  });
});
