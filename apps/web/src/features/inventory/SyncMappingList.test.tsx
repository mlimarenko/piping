import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { changeAppLanguage } from "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import SyncMappingList from "./SyncMappingList";
import type { MappingConfiguration } from "@/shared/api/types";

const apiMocks = vi.hoisted(() => ({
  listMappingConfigurations: vi.fn(),
  deleteMappingConfiguration: vi.fn(),
}));

vi.mock("./sync-mapping-api", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  listMappingConfigurations: apiMocks.listMappingConfigurations,
  deleteMappingConfiguration: apiMocks.deleteMappingConfiguration,
}));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn() }),
}));

function mappingConfig(overrides: Partial<MappingConfiguration>): MappingConfiguration {
  return {
    id: "aaaaaaaa-0000-4000-8000-000000000001:1",
    logical_id: "aaaaaaaa-0000-4000-8000-000000000001",
    name: "Inventory pull",
    version: 1,
    is_latest: true,
    provider_id: "01a0f419-0000-4000-8000-000000000001",
    target: { kind: "local_inventory" },
    status: "active",
    direction: "pull",
    conflict_mode: "netbox_wins",
    entity_mappings: [],
    created_at: "2026-01-01T00:00:00Z",
    in_flight_job_count: 0,
    ...overrides,
  };
}

function renderList(path = "/inventory/sync/mappings", readOnly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  function LocationProbe() {
    const location = useLocation();
    return <output aria-label="Current URL">{location.pathname}{location.search}{location.hash}</output>;
  }
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[path]}>
        <AllPermissions>
          <PermissionsContext.Provider value={{ can: () => !readOnly, canAny: () => !readOnly, hasRole: () => false, roles: [], isAdmin: !readOnly }}>
          <Routes>
            <Route path="/inventory/sync/mappings" element={<SyncMappingList />} />
            <Route path="/inventory/sync/:providerId/mappings" element={<SyncMappingList />} />
            <Route path="/inventory/sync/mappings/:id/:mode" element={<LocationProbe />} />
          </Routes>
          </PermissionsContext.Provider>
        </AllPermissions>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return client;
}

describe("SyncMappingList in-flight badge", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    await changeAppLanguage("en");
    apiMocks.deleteMappingConfiguration.mockResolvedValue(undefined);
    apiMocks.listMappingConfigurations.mockResolvedValue({
      items: [mappingConfig({ in_flight_job_count: 2 })],
      total: 1,
      limit: 200,
      offset: 0,
    });
  });

  it("opens the latest immutable detail for a reader without losing URL context", async () => {
    renderList("/inventory/sync/provider-1/mappings?context=inventory#fields", true);
    const row = await screen.findByRole("button", { name: /Inventory pull/ });
    expect(screen.queryByRole("button", { name: en.sync.mapping.newConfig })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /actions:/i })).not.toBeInTheDocument();
    fireEvent.click(row);
    expect(await screen.findByLabelText("Current URL")).toHaveTextContent("/inventory/sync/mappings/aaaaaaaa-0000-4000-8000-000000000001:1/readonly?context=inventory#fields");
    expect(apiMocks.deleteMappingConfiguration).not.toHaveBeenCalled();
  });

  it("freezes the confirmed mapping version across refresh and restores action focus on cancel", async () => {
    const client = renderList();
    const trigger = await screen.findByRole("button", { name: `${en.actions}: Inventory pull` });
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: en.delete }));
    expect(await screen.findByRole("alertdialog")).toHaveTextContent("Inventory pull");
    fireEvent.click(screen.getByRole("button", { name: en.cancel }));
    await waitFor(() => expect(trigger).toHaveFocus());
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: en.delete }));
    await screen.findByRole("alertdialog");
    act(() => client.setQueriesData({ queryKey: ["sync", "mapping-configurations"] }, {
      items: [mappingConfig({ id: "new-version", name: "Renamed mapping", version: 2 })], total: 1,
    }));
    expect(screen.getByRole("alertdialog")).toHaveTextContent("Inventory pull");
    expect(screen.getByRole("alertdialog")).not.toHaveTextContent("Renamed mapping");
    fireEvent.click(screen.getByRole("button", { name: en.delete }));
    await waitFor(() => expect(apiMocks.deleteMappingConfiguration).toHaveBeenCalledWith("aaaaaaaa-0000-4000-8000-000000000001:1"));
  });

  it("renders the in-flight count as a non-interactive status, never a jobs link", async () => {
    renderList();

    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent(/in-flight/);
    // The badge is not a clickable control and there is no dead deep-link to the
    // non-existent /inventory/sync/jobs route.
    expect(status.closest("button")).toBeNull();
    expect(status.closest("a")).toBeNull();
    expect(document.querySelector('a[href*="/inventory/sync/jobs"]')).toBeNull();
  });
});
