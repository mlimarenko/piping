import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import "@/shared/i18n";
import { changeAppLanguage } from "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import type { MappingConfiguration, SyncProvider } from "@/shared/api/types";
import SyncPage from "./SyncPage";

const apiMocks = vi.hoisted(() => ({
  listProviders: vi.fn(),
  createProvider: vi.fn(),
  updateProvider: vi.fn(),
  deleteProvider: vi.fn(),
  validateProvider: vi.fn(),
  createSyncJob: vi.fn(),
  getLatestSyncJob: vi.fn(),
}));

vi.mock("@/features/inventory/sync-api", () => ({ ...apiMocks }));

const mappingApiMocks = vi.hoisted(() => ({
  listMappingConfigurations: vi.fn(),
}));
vi.mock("@/features/inventory/sync-mapping-api", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return {
    ...actual,
    listMappingConfigurations: mappingApiMocks.listMappingConfigurations,
  };
});

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn() }),
}));

const NETBOX_PROVIDER: SyncProvider = {
  id: "01a0f419-0000-4000-8000-000000000001",
  providerId: "netbox",
  name: "Printum NetBox",
  // The wire never returns the write-only token: `secret_fields` carries
  // presence only, which is exactly what the UI must render.
  config: { base_url: "http://172.31.0.1:18001" },
  enabled: true,
  secretFields: [{ fieldKey: "api_token", hasValue: true, enabled: true }],
};

const sync = en.inventory.sync;
const URL_PLACEHOLDER = "https://netbox.example.com";

function mappingConfig(overrides: Partial<MappingConfiguration>): MappingConfiguration {
  return {
    id: "aaaaaaaa-0000-4000-8000-000000000001:1",
    logical_id: "aaaaaaaa-0000-4000-8000-000000000001",
    name: "Inventory pull",
    version: 1,
    is_latest: true,
    provider_id: NETBOX_PROVIDER.id,
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

const MAPPING_PULL = mappingConfig({});
const MAPPING_SITES = mappingConfig({
  id: "bbbbbbbb-0000-4000-8000-000000000002:3",
  logical_id: "bbbbbbbb-0000-4000-8000-000000000002",
  name: "Sites import",
  version: 3,
});

function renderPage(permissions?: PermissionsValue): void {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        {permissions ? (
          <PermissionsContext.Provider value={permissions}>
            <SyncPage />
          </PermissionsContext.Provider>
        ) : (
          <AllPermissions>
            <SyncPage />
          </AllPermissions>
        )}
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

async function renderLoaded(): Promise<void> {
  renderPage();
  await screen.findByPlaceholderText(URL_PLACEHOLDER);
}

function renderPageWithLocation(initialPath: string): () => string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  function LocationProbe() {
    const location = useLocation();
    return <output aria-label="Current URL">{location.pathname}{location.search}{location.hash}</output>;
  }
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[initialPath]}>
        <AllPermissions>
          <SyncPage />
          <LocationProbe />
        </AllPermissions>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return () => screen.getByLabelText("Current URL").textContent ?? "";
}

function urlInput(): HTMLElement {
  return screen.getByPlaceholderText(URL_PLACEHOLDER);
}

function secretInput(): HTMLInputElement {
  const input = document.querySelector<HTMLInputElement>('input[type="password"]');
  if (!input) throw new Error("Write-only secret input is not rendered");
  return input;
}

beforeEach(async () => {
  vi.clearAllMocks();
  await changeAppLanguage("en");
  apiMocks.listProviders.mockResolvedValue({
    items: [NETBOX_PROVIDER],
    total: 1,
    limit: 25,
    offset: 0,
  });
  apiMocks.updateProvider.mockResolvedValue(NETBOX_PROVIDER);
  apiMocks.validateProvider.mockResolvedValue({ ok: true });
  apiMocks.getLatestSyncJob.mockResolvedValue(null);
  mappingApiMocks.listMappingConfigurations.mockResolvedValue({
    items: [MAPPING_PULL, MAPPING_SITES],
    total: 2,
    limit: 200,
    offset: 0,
  });
});

describe("SyncPage explicit connection form", () => {
  it("exposes the provider sections as a shared heading hierarchy", async () => {
    await renderLoaded();
    expect(screen.getByRole("heading", { level: 1, name: sync.title })).toBeInTheDocument();
    for (const title of [sync.inspectorTitle, sync.connectionSettings, sync.runSync, sync.sourceEntities, sync.dangerZone]) {
      expect(screen.getByRole("heading", { level: 2, name: title })).toBeInTheDocument();
    }
  });

  it("does not persist anything while the operator types", async () => {
    await renderLoaded();

    await act(async () => {
      fireEvent.change(urlInput(), { target: { value: "http://new.example.com" } });
      fireEvent.change(secretInput(), { target: { value: "new-secret-value" } });
      await Promise.resolve();
    });

    expect(apiMocks.updateProvider).not.toHaveBeenCalled();
  });

  it("sends exactly one PATCH when Save is pressed", async () => {
    await renderLoaded();

    fireEvent.change(urlInput(), { target: { value: "http://new.example.com" } });
    fireEvent.click(screen.getByRole("button", { name: en.save }));

    await waitFor(() => expect(apiMocks.updateProvider).toHaveBeenCalledTimes(1));
    expect(apiMocks.updateProvider).toHaveBeenCalledWith(NETBOX_PROVIDER.id, {
      config: { base_url: "http://new.example.com" },
    });
    expect(notifyMock).toHaveBeenCalledWith("success", sync.configSaved);
  });

  it("keeps the stored write-only token when the secret field is left blank", async () => {
    await renderLoaded();

    fireEvent.change(urlInput(), { target: { value: "http://new.example.com" } });
    fireEvent.click(screen.getByRole("button", { name: en.save }));

    await waitFor(() => expect(apiMocks.updateProvider).toHaveBeenCalledTimes(1));
    const patch = apiMocks.updateProvider.mock.calls[0]?.[1] as {
      config: Record<string, string>;
    };
    expect(patch.config).not.toHaveProperty("api_token");
  });

  it("masks the write-only token and reports that a value is stored", async () => {
    await renderLoaded();
    const token = secretInput();

    expect(token).toHaveValue("");
    expect(screen.getByText(sync.secretHasValue)).toBeInTheDocument();
    // An empty secret field must not offer a reveal toggle that implies the
    // stored token could be shown.
    expect(screen.queryByRole("button", { name: sync.showSecret })).toBeNull();
  });

  it("associates every connection field with its visible label", async () => {
    await renderLoaded();

    expect(screen.getByLabelText(/NetBox URL/)).toBe(urlInput());
    expect(screen.getByLabelText(/API Token/)).toBe(secretInput());
  });

  it("blocks the submit and reports the error when a required field is empty", async () => {
    await renderLoaded();

    fireEvent.change(urlInput(), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: en.save }));

    expect(await screen.findByText(en.required)).toBeInTheDocument();
    expect(apiMocks.updateProvider).not.toHaveBeenCalled();
  });

  it("disables the connection check while it is in flight", async () => {
    let settle: (value: unknown) => void = () => {};
    apiMocks.validateProvider.mockReturnValue(
      new Promise((resolve) => {
        settle = resolve;
      }),
    );
    await renderLoaded();
    const button = screen.getByRole("button", { name: sync.testConnection });

    expect(button).toBeEnabled();
    fireEvent.click(button);
    await waitFor(() => expect(button).toBeDisabled());

    await act(async () => {
      settle({ valid: true });
      await Promise.resolve();
    });
  });

  it("reports the backend's own failure message instead of a fabricated result", async () => {
    apiMocks.validateProvider.mockResolvedValue({ valid: false, message: "403 Forbidden" });
    await renderLoaded();

    fireEvent.click(screen.getByRole("button", { name: sync.testConnection }));

    await waitFor(() => expect(notifyMock).toHaveBeenCalledWith("error", "403 Forbidden"));
  });
});

describe("SyncPage job selection", () => {
  it("preserves the run card heading without exposing execution controls to a viewer", async () => {
    renderPage({
      roles: ["viewer"], isAdmin: false,
      can: (action) => action === "READ",
      canAny: (_resource, actions) => actions.includes("READ"),
      hasRole: (role) => role === "viewer",
    });
    await screen.findByPlaceholderText(URL_PLACEHOLDER);
    expect(screen.getByText(sync.runSync)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: sync.syncNow })).toBeNull();
    expect(screen.queryByRole("combobox", { name: sync.mappingConfiguration })).toBeNull();
    expect(urlInput()).toHaveAttribute("readonly");
  });

  it("lists each configuration with its name and exact immutable version", async () => {
    await renderLoaded();

    fireEvent.click(await screen.findByRole("combobox", { name: sync.mappingConfiguration }));

    expect(screen.getByText("Inventory pull · v1")).toBeInTheDocument();
    expect(screen.getByText("Sites import · v3")).toBeInTheDocument();
  });

  it("keeps run disabled until a configuration is selected", async () => {
    await renderLoaded();

    expect(await screen.findByRole("button", { name: sync.syncNow })).toBeDisabled();
  });

  it("runs the selected configuration's exact logical_id:version reference", async () => {
    apiMocks.createSyncJob.mockResolvedValue({});
    await renderLoaded();

    fireEvent.click(await screen.findByRole("combobox", { name: sync.mappingConfiguration }));
    fireEvent.click(screen.getByText("Sites import · v3"));

    const run = screen.getByRole("button", { name: sync.syncNow });
    expect(run).toBeEnabled();
    fireEvent.click(run);

    await waitFor(() =>
      expect(apiMocks.createSyncJob).toHaveBeenCalledWith(
        NETBOX_PROVIDER.id,
        `${MAPPING_SITES.logical_id}:${MAPPING_SITES.version}`,
      ),
    );
  });

  it("links to mapping creation when the provider has no configurations", async () => {
    mappingApiMocks.listMappingConfigurations.mockResolvedValue({
      items: [],
      total: 0,
      limit: 200,
      offset: 0,
    });
    await renderLoaded();

    expect(await screen.findByText(sync.noMappingConfigurations)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: sync.createMappingConfiguration })).toHaveAttribute(
      "href",
      `/inventory/sync/${NETBOX_PROVIDER.id}/mappings/new`,
    );
  });

  it("surfaces a configuration load failure with a retry action", async () => {
    mappingApiMocks.listMappingConfigurations.mockRejectedValue(new Error("boom"));
    await renderLoaded();

    expect(await screen.findByText(sync.mappingConfigurationsLoadFailed)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.retry })).toBeInTheDocument();
  });

  it("keeps run disabled for a provider-target configuration and explains why", async () => {
    mappingApiMocks.listMappingConfigurations.mockResolvedValue({
      items: [
        mappingConfig({
          target: { kind: "provider", provider_id: "bbbbbbbb-0000-4000-8000-000000000009" },
        }),
      ],
      total: 1,
      limit: 200,
      offset: 0,
    });
    await renderLoaded();

    expect(await screen.findByText(sync.runnableConfigurationRequired)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: sync.syncNow })).toBeDisabled();
  });

  it("shows the latest job from the bounded endpoint", async () => {
    apiMocks.getLatestSyncJob.mockResolvedValue({
      id: "job-1",
      status: "completed",
      finishedAt: "2026-02-02T00:00:00Z",
      stats: {},
    });
    await renderLoaded();

    expect(await screen.findByText(new RegExp(sync.jobStatusCompleted))).toBeInTheDocument();
    // No counters were reported, so none are fabricated.
    expect(screen.queryByText(/entities_extracted=/)).toBeNull();
  });

  it("shows job counters only when the job actually reported them", async () => {
    apiMocks.getLatestSyncJob.mockResolvedValue({
      id: "job-2",
      status: "completed",
      stats: { entities_extracted: 13, entities_committed: 13, tag_memberships_committed: 4 },
    });
    await renderLoaded();

    expect(await screen.findByText(/entities_extracted=13/)).toBeInTheDocument();
    expect(screen.getByText(/tag_memberships_committed=4/)).toBeInTheDocument();
  });

  it("shows a truthful empty state when the provider has never run", async () => {
    await renderLoaded();

    expect(await screen.findByText(sync.latestJobNone)).toBeInTheDocument();
  });
});

describe("SyncPage provider URL selection", () => {
  const OTHER_PROVIDER: SyncProvider = {
    ...NETBOX_PROVIDER,
    id: "02b0f419-0000-4000-8000-000000000002",
    name: "Second Provider",
  };

  function mockTwoProviders(): void {
    apiMocks.listProviders.mockResolvedValue({
      items: [NETBOX_PROVIDER, OTHER_PROVIDER],
      total: 2,
      limit: 25,
      offset: 0,
    });
  }

  it("restores the selected provider from the ?provider= URL", async () => {
    mockTwoProviders();
    renderPageWithLocation(`/inventory/sync?provider=${OTHER_PROVIDER.id}`);
    expect(await screen.findByRole("heading", { level: 3, name: OTHER_PROVIDER.name })).toBeInTheDocument();
  });

  it("writes the selected provider id to the URL on row click", async () => {
    mockTwoProviders();
    const url = renderPageWithLocation("/inventory/sync");
    await screen.findByText(OTHER_PROVIDER.name);
    fireEvent.click(screen.getByText(OTHER_PROVIDER.name).closest("tr")!);
    expect(url()).toContain(`provider=${OTHER_PROVIDER.id}`);
  });

  it("fails closed (no first-provider fallback) when the URL provider is absent from the results", async () => {
    // Only NETBOX_PROVIDER is returned; the URL points at an off-page/deleted id.
    const url = renderPageWithLocation("/inventory/sync?provider=ffffffff-0000-4000-8000-000000000000");

    expect(await screen.findByText(sync.providerUnavailable)).toBeInTheDocument();
    // The inspector must not silently resolve X→first provider.
    expect(screen.queryByRole("heading", { level: 3, name: NETBOX_PROVIDER.name })).toBeNull();
    // The URL keeps the explicit selection so it recovers after page/filter clear.
    expect(url()).toContain("provider=ffffffff-0000-4000-8000-000000000000");
  });

  it("clears only the provider param, preserving context query and hash", async () => {
    const url = renderPageWithLocation(
      "/inventory/sync?provider=ffffffff-0000-4000-8000-000000000000&context=fixture#selection",
    );

    fireEvent.click(await screen.findByRole("button", { name: sync.clearProviderSelection }));

    expect(url()).toBe("/inventory/sync?context=fixture#selection");
  });

  it("replaces only the provider on row click, keeping context query and hash", async () => {
    mockTwoProviders();
    const url = renderPageWithLocation("/inventory/sync?context=fixture#selection");
    await screen.findByText(OTHER_PROVIDER.name);

    fireEvent.click(screen.getByText(OTHER_PROVIDER.name).closest("tr")!);

    expect(url()).toContain(`provider=${OTHER_PROVIDER.id}`);
    expect(url()).toContain("context=fixture");
    expect(url()).toContain("#selection");
  });
});
