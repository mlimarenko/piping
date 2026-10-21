import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useNavigate } from "react-router";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import InventoryResources from "./InventoryResources";
import { ApiError } from "@/shared/api/client";
import { fieldsByTab } from "@/features/inventory/components/inventory/inventoryTypes";
import { AllPermissions } from "@/test/permissions";
import en from "@/shared/i18n/locales/en";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { RequireCapability } from "@/shared/auth/permissions";
import { useLocation } from "react-router";

const apiMocks = vi.hoisted(() => ({
  listResources: vi.fn(),
  listResourcesForTable: vi.fn(),
  getResource: vi.fn(),
  updateResource: vi.fn(),
  createResource: vi.fn(),
  deleteResource: vi.fn(),
  bulkUpdateResources: vi.fn(),
}));

vi.mock("@/features/inventory/api", () => ({
  listResources: apiMocks.listResources,
  listResourcesForTable: apiMocks.listResourcesForTable,
  getResource: apiMocks.getResource,
  updateResource: apiMocks.updateResource,
  createResource: apiMocks.createResource,
  deleteResource: apiMocks.deleteResource,
  bulkUpdateResources: apiMocks.bulkUpdateResources,
}));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn() }),
}));

vi.mock("@/shared/forms/EntityCreateEditor", () => ({
  default: ({ open }: { open: boolean }) =>
    open ? <div role="dialog">Create resource</div> : null,
}));

vi.mock("@/features/inventory/components/inventory/BulkEditDialog", () => ({
  default: ({ open, onApply }: { open: boolean; onApply: (patch: Record<string, string>) => void }) =>
    open ? (
      <div role="dialog">
        <button type="button" onClick={() => onApply({ status: "active" })}>
          Apply bulk edit
        </button>
      </div>
    ) : null,
}));

vi.mock("@/features/inventory/components/cabling-workstation/CablingWorkstation", () => ({
  default: () => <div>Cabling workstation pane</div>,
}));

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>{ui}</AllPermissions>
    </QueryClientProvider>,
  );
}

function CurrentBookmark() {
  const location = useLocation();
  return <output aria-label="Bookmark">{location.search}</output>;
}

function HistoryControls() {
  const navigate = useNavigate();
  return <>
    <button onClick={() => void navigate(-1)}>Back</button>
    <button onClick={() => void navigate(1)}>Forward</button>
  </>;
}

function CurrentPath() {
  return <output aria-label="Current route">{useLocation().pathname}</output>;
}

function collectVisibleHeaderNames(): string[] {
  return screen
    .getAllByRole("columnheader")
    .map((header) => header.textContent?.trim())
    .filter((value): value is string => Boolean(value));
}

function collectCheckedColumnSettingNames(dialog: HTMLElement): string[] {
  return within(dialog)
    .getAllByRole("menuitemcheckbox")
    .filter((entry) => entry.getAttribute("aria-checked") === "true")
    .map((entry) => entry.textContent?.replace("Fixed", "").trim())
    .filter((value): value is string => Boolean(value));
}

function expectUnique(values: string[], scope: string) {
  expect(values.length, `${scope}: values`).toBeGreaterThan(0);
  expect(new Set(values).size, `${scope}: ${values.join(", ")}`).toBe(values.length);
}

describe("InventoryResources list", () => {
  it.each([false, true])("PWA-116 target repair gives permission-aware empty guidance (create=%s)", async create => {
    const permissions = { roles: [], isAdmin: false, can: (action: string) => action === "READ" || (create && action === "CREATE"), canAny: () => false, hasRole: () => false };
    renderWithClient(<PermissionsContext.Provider value={permissions}><MemoryRouter initialEntries={["/inventory/resources?entity=Devices"]}><InventoryResources /><CurrentPath /></MemoryRouter></PermissionsContext.Provider>);
    await screen.findByText("No Devices");
    if (create) {
      expect(screen.getByText("Create your first Device entry")).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Add Device" }));
      await waitFor(() => expect(screen.getByLabelText("Current route")).toHaveTextContent(/\/inventory\/resources\/.*\/new$/));
    } else {
      expect(screen.queryByText("Create your first Device entry")).not.toBeInTheDocument();
      expect(screen.getByText("No records are available in this view.")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Add Device" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Create" })).not.toBeInTheDocument();
    }
  });

  it("PWA-116 target repair preserves the existing READ route guard", () => {
    const permissions = { roles: [], isAdmin: false, can: () => false, canAny: () => false, hasRole: () => false };
    renderWithClient(<PermissionsContext.Provider value={permissions}><MemoryRouter><RequireCapability check={p => p.can("READ", "INVENTORY")}><InventoryResources /></RequireCapability></MemoryRouter></PermissionsContext.Provider>);
    expect(screen.queryByRole("heading", { name: "Sites" })).not.toBeInTheDocument();
    expect(apiMocks.listResourcesForTable).not.toHaveBeenCalled();
  });

  it("PWA-116 target repair returns reset focus to the surviving search control", async () => {
    apiMocks.listResourcesForTable.mockImplementation((_entity, query) => Promise.resolve({ items: query.search ? [] : [{ id: "fixture-device", name: "Fixture device", type: "device" }], total: query.search ? 0 : 1 }));
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Devices"]}><InventoryResources /></MemoryRouter>);
    const search = await screen.findByPlaceholderText("Search Devices...");
    fireEvent.change(search, { target: { value: "no-match" } });
    const reset = await screen.findByRole("button", { name: "Reset search and filters" });
    reset.focus();
    fireEvent.click(reset);
    await screen.findByText("Fixture device");
    expect(search).toHaveFocus();
    expect(search).toHaveValue("");
    expect(screen.queryByRole("button", { name: "Reset search and filters" })).not.toBeInTheDocument();
    expect(apiMocks.listResourcesForTable).toHaveBeenLastCalledWith("Devices", expect.objectContaining({ search: undefined, filters: {}, offset: 0 }));
  });

  it("PWA-116 target repair resets a nonzero page but retains sort, size and columns", async () => {
    apiMocks.listResourcesForTable.mockImplementation((_entity, query) => Promise.resolve({ items: query.search ? [] : [{ id: "fixture-device", name: "Fixture device", type: "device" }], total: 60, limit: query.limit, offset: query.offset }));
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Devices"]}><InventoryResources /></MemoryRouter>);
    await screen.findByText("Fixture device");
    fireEvent.click(screen.getByRole("button", { name: "Name" }));
    const headers = collectVisibleHeaderNames();
    const search = screen.getByPlaceholderText("Search Devices...");
    fireEvent.change(search, { target: { value: "no-match" } });
    await screen.findByRole("button", { name: "Reset search and filters" });
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    await waitFor(() => expect(apiMocks.listResourcesForTable).toHaveBeenLastCalledWith("Devices", expect.objectContaining({ offset: 25, sort: "name", order: "asc", limit: 25 })));
    fireEvent.click(screen.getByRole("button", { name: "Reset search and filters" }));
    await screen.findByText("Fixture device");
    expect(search).toHaveFocus();
    expect(collectVisibleHeaderNames()).toEqual(headers);
    await waitFor(() => expect(apiMocks.listResourcesForTable).toHaveBeenLastCalledWith("Devices", expect.objectContaining({ offset: 0, sort: "name", order: "asc", limit: 25, search: undefined, filters: {} })));
  });
  beforeEach(() => {
    apiMocks.listResources.mockResolvedValue({ items: [], total: 0 });
    apiMocks.listResourcesForTable.mockResolvedValue({ items: [], total: 0, limit: 25, offset: 0 });
    apiMocks.updateResource.mockResolvedValue({ id: "site-a", name: "Site A" });
    apiMocks.createResource.mockResolvedValue({ id: "created", name: "Created" });
    apiMocks.deleteResource.mockResolvedValue(undefined);
    apiMocks.bulkUpdateResources.mockResolvedValue({ updated: [], failed: [] });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("names the active entity in the heading and keeps categories accessible", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Devices"]}>
        <InventoryResources />
      </MemoryRouter>,
    );
    expect(await screen.findByRole("heading", { level: 1, name: "Devices" })).toBeInTheDocument();
    const navigation = screen.getByRole("navigation", { name: "Inventory Resources" });
    expect(within(navigation).getByRole("link", { name: /^DCIM/ })).toHaveAttribute("aria-current", "page");
    fireEvent.click(within(navigation).getByRole("link", { name: /^Virtualization/ }));
    expect(await screen.findByRole("heading", { level: 1, name: "Clusters" })).toBeInTheDocument();
  });

  it("shows entity names and counts directly instead of requiring a picker", async () => {
    apiMocks.listResources.mockImplementation((entity) => Promise.resolve({ items: [], total: entity === "Sites" ? 4 : 0 }));
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}>
        <InventoryResources />
      </MemoryRouter>,
    );
    const navigation = screen.getByRole("navigation", { name: "DCIM" });
    expect(await within(navigation).findByRole("link", { name: "Sites 4" })).toBeVisible();
    expect(within(navigation).getByRole("link", { name: "Devices 0" })).toBeVisible();
    expect(within(navigation).getByRole("link", { name: "Virtual Device Contexts 0" })).toBeVisible();
    expect(screen.queryByRole("combobox", { name: /DCIM|Inventory Resources/ })).not.toBeInTheDocument();
    fireEvent.click(within(navigation).getByRole("link", { name: "Locations 0" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Locations" })).toBeInTheDocument();
    expect(within(navigation).getByRole("link", { name: "Locations 0" })).toHaveAttribute("aria-current", "page");
    expect(within(navigation).getByRole("link", { name: "Sites 4" })).not.toHaveAttribute("aria-current");
    expect(apiMocks.listResources).toHaveBeenCalledWith("Devices", { limit: 1 });
  });

  it("keeps both navigation levels and search visible while updating the bookmark and history", async () => {
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Sites&offset=50#old"]}><InventoryResources /><CurrentBookmark /><HistoryControls /></MemoryRouter>);
    const toolbar = screen.getByRole("group", { name: "Inventory Resources" });
    fireEvent.click(within(toolbar).getByRole("link", { name: /^Virtualization/ }));
    expect(screen.getByLabelText("Bookmark")).toHaveTextContent("entity=Clusters");
    fireEvent.click(within(toolbar).getByRole("link", { name: /^Virtual Machines/ }));
    expect(screen.getByLabelText("Bookmark")).toHaveTextContent("entity=Virtual+Machines");
    fireEvent.change(within(toolbar).getByPlaceholderText("Search Virtual Machines..."), { target: { value: "vm-edge" } });
    await waitFor(() => expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith("Virtual Machines", expect.objectContaining({ search: "vm-edge" })));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByLabelText("Bookmark")).toHaveTextContent("entity=Clusters");
    expect(screen.getByPlaceholderText("Search Clusters...")).toHaveValue("");
    fireEvent.click(screen.getByRole("button", { name: "Forward" }));
    expect(screen.getByRole("link", { name: /^Virtual Machines/ })).toHaveAttribute("aria-current", "page");
    expect(screen.getByPlaceholderText("Search Virtual Machines...")).toHaveValue("");
  });

  it("does not report missing or failed count queries as known zero", async () => {
    apiMocks.listResources.mockImplementation((entity) => {
      if (entity === "Locations") return new Promise(() => {});
      if (entity === "Devices") return Promise.reject(new Error("Count unavailable"));
      return Promise.resolve({ items: [], total: entity === "Sites" ? 4 : 0 });
    });
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}><InventoryResources /></MemoryRouter>);
    const entities = screen.getByRole("navigation", { name: "DCIM" });
    expect(await within(entities).findByRole("link", { name: "Devices Unknown error" })).toBeVisible();
    expect(within(entities).getByRole("link", { name: "Locations Loading..." })).toBeVisible();
    expect(within(entities).getByRole("link", { name: "Sites 4" })).toBeVisible();
    expect(within(entities).queryByRole("link", { name: "Devices 0" })).not.toBeInTheDocument();
    const categories = screen.getByRole("navigation", { name: "Inventory Resources" });
    expect(within(categories).getByRole("link", { name: "DCIM Unknown error" })).toBeVisible();
    expect(within(categories).getByRole("link", { name: "Provisioning" })).toBeVisible();
  });

  it("names a bookmarked pseudo-entity without exposing generic resource actions", async () => {
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=cabling-workstation"]}><InventoryResources /></MemoryRouter>);
    expect(await screen.findByRole("heading", { level: 1, name: "Cabling Workstation" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Refresh" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Edit fields|Delete \(/ })).not.toBeInTheDocument();
  });

  it("returns focus to the named row menu after cancelling delete without mutating", async () => {
    apiMocks.listResourcesForTable.mockResolvedValue({ items: [{ id: "site-a", name: "Site A", type: "site" }], total: 1 });
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}><InventoryResources /></MemoryRouter>);
    const trigger = await screen.findByRole("button", { name: "Actions: Site A" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(apiMocks.deleteResource).not.toHaveBeenCalled();
  });

  it("withholds resource data and row actions when READ is denied", async () => {
    apiMocks.listResourcesForTable.mockResolvedValue({ items: [{ id: "site-a", name: "Site A", type: "site" }], total: 1 });
    const permissions = { roles: [], isAdmin: false, can: () => false, canAny: () => false, hasRole: () => false };
    renderWithClient(<PermissionsContext.Provider value={permissions}><MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}><InventoryResources /></MemoryRouter></PermissionsContext.Provider>);
    await screen.findByText(/Your role doesn't grant/);
    expect(apiMocks.listResourcesForTable).not.toHaveBeenCalled();
    expect(screen.queryByText("Site A")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Actions: Site A" })).not.toBeInTheDocument();
  });

  it("retains the selected entity explanation after following its visible link", async () => {
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}><InventoryResources /></MemoryRouter>);
    fireEvent.click(await screen.findByRole("link", { name: /^Locations/ }));
    expect(await screen.findByRole("heading", { level: 1, name: "Locations" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: text => text.includes("DCIM / Locations:") && text.includes(en.inventory.resources.hints.entities.Locations) }));
    expect(await screen.findByText(text => text.includes("DCIM / Locations:") && text.includes(en.inventory.resources.hints.entities.Locations))).toBeInTheDocument();
  });

  it("does not expose service binding UUIDs as a table column", async () => {
    apiMocks.listResourcesForTable.mockResolvedValue({ total: 1, items: [{ id: "service-1", name: "HTTPS", type: "service", protocol: "tcp", ports: "443", serviceIpAddressIds: ["binding-ip-id"] }] });
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Services"]}><InventoryResources /></MemoryRouter>);
    expect(await screen.findByText("HTTPS")).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: en.dataflows.serviceAddresses })).not.toBeInTheDocument();
    expect(screen.queryByText("binding-ip-id")).not.toBeInTheDocument();
  });
});

describe("InventoryResources column filters", () => {
  it("PWA-116 distinguishes filtered empty and resets the query without changing entity", async () => {
    renderWithClient(<MemoryRouter initialEntries={["/inventory/resources?entity=Devices"]}><InventoryResources /></MemoryRouter>);
    fireEvent.click(await screen.findByRole("button", { name: /filter:?\s*status/i }));
    fireEvent.click(await screen.findByRole("button", { name: /^planned$/i }));
    await waitFor(() => expect(apiMocks.listResourcesForTable).toHaveBeenLastCalledWith("Devices", expect.objectContaining({ filters: { status: ["planned"] } })));
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(await screen.findByText("No results found", {}, { timeout: 1000 })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Reset search and filters" }));
    await waitFor(() => expect(apiMocks.listResourcesForTable).toHaveBeenLastCalledWith("Devices", expect.objectContaining({ filters: {}, search: undefined, offset: 0 })));
    expect(screen.getByRole("heading", { level: 1, name: "Devices" })).toBeInTheDocument();
  });
  beforeEach(() => {
    apiMocks.listResources.mockResolvedValue({ items: [], total: 0 });
    apiMocks.listResourcesForTable.mockResolvedValue({ items: [], total: 0, limit: 25, offset: 0 });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  // BUG 1 regression: selecting a column-filter value must trigger a refetch
  // whose params carry the active filter (the funnel was updating the badge but
  // the table query previously dropped the selected values for non-devices
  // entities — and the wiring contract must stay intact for devices too).
  it("refetches with the selected status filter when a funnel value is checked", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Devices"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    // Initial load (no filter) resolves first.
    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
        "Devices",
        expect.objectContaining({ filters: {} }),
      );
    });
    apiMocks.listResourcesForTable.mockClear();

    // Open the Status column funnel (aria-label "Filter: Status") and check the
    // "Planned" value.
    const statusFunnel = await screen.findByRole("button", { name: /filter:?\s*status/i });
    fireEvent.click(statusFunnel);

    const planned = await screen.findByRole("button", { name: /^planned$/i });
    fireEvent.click(planned);

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
        "Devices",
        expect.objectContaining({ filters: { status: ["planned"] } }),
      );
    });
  });

  // BUG 2: every entity attribute is a column, so the column-settings picker
  // lists the full set while the long-tail defaults to hidden (per the
  // `table.column_visibility` default-hidden policy). `tenant`/`serial` were
  // sliced off the old 4-column window; they must now be reachable in the picker.
  it("exposes the full device attribute set in the column-settings picker", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Devices"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalled();
    });

    const columnsButton = await screen.findByRole("button", { name: /^columns$/i });
    fireEvent.click(columnsButton);

    const picker = await screen.findByRole("dialog").catch(() => null);
    const scope = picker ? within(picker) : screen;
    // `serial` and `tenant` live beyond the old slice(0,4) window; their labels
    // must appear as toggleable picker entries now that the cap is gone.
    expect(await scope.findByText(/serial/i)).toBeInTheDocument();
  });

  it("does not expose sorting for the prefix utilization meter", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Prefixes"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
        "Prefixes",
        expect.objectContaining({ sort: undefined }),
      );
    });

    expect(screen.queryByRole("button", { name: /sort by utilization/i })).toBeNull();
  });

  it("honors the Cables entity deep-link instead of falling back to Sites", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Cables"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
        "Cables",
        expect.any(Object),
      );
    });

    expect(within(screen.getByRole("navigation", { name: /Circuits/ })).getByRole("link", { name: /^Cables/ })).toHaveAttribute("aria-current", "page");
  });

  it("does not render the global create button for pseudo workspaces", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=cabling-workstation"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    expect(await screen.findByText("Cabling workstation pane")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^create$/i })).toBeNull();
  });

  it("keeps IP-address headers in parity with checked column-settings entries", async () => {
    apiMocks.listResourcesForTable.mockResolvedValue({
      items: [
        {
          id: "ip-1",
          name: "10.0.0.1/24",
          status: "active",
          prefix: "10.0.0.1/24",
          interfaceName: "xe-0/0/0",
          tenant: "Tenant A",
          vrfName: "VRF A",
          dnsName: "host-a.example.test",
          type: "ip_address",
        },
      ],
      total: 1,
      limit: 25,
      offset: 0,
    });

    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=IP%20Addresses"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
        "IP Addresses",
        expect.any(Object),
      );
    });

    fireEvent.click(await screen.findByRole("button", { name: /^columns$/i }));
    const dialog = screen.getByRole("dialog");

    const visibleHeaderNames = screen
      .getAllByRole("columnheader")
      .map((header) => header.textContent?.trim())
      .filter((value): value is string => Boolean(value));
    const checkedEntryNames = within(dialog)
      .getAllByRole("menuitemcheckbox")
      .filter((entry) => entry.getAttribute("aria-checked") === "true")
      .map((entry) => entry.textContent?.replace("Fixed", "").trim())
      .filter((value): value is string => Boolean(value));

    expect(visibleHeaderNames.sort()).toEqual(checkedEntryNames.sort());
    expect(screen.getAllByRole("columnheader", { name: /^address$/i })).toHaveLength(1);
    expect(
      within(dialog)
        .getAllByRole("menuitemcheckbox")
        .filter((entry) => entry.textContent?.replace("Fixed", "").trim() === "Address"),
    ).toHaveLength(1);
  });

  it("keeps the IP-address identity and status columns fixed in the picker", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=IP%20Addresses"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
        "IP Addresses",
        expect.any(Object),
      );
    });

    fireEvent.click(await screen.findByRole("button", { name: /^columns$/i }));
    const dialog = screen.getByRole("dialog");
    expect(dialog.querySelector('[data-column-settings-key="address"]')).toBeDisabled();
    expect(dialog.querySelector('[data-column-settings-key="status"]')).toBeDisabled();
    expect(within(dialog).queryByRole("menuitemcheckbox", { name: /^actions$/i })).toBeNull();
  });

  it("keeps the nameless-entity identity column fixed after switching tabs", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Device%20Types"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
        "Device Types",
        expect.any(Object),
      );
    });

    fireEvent.click(await screen.findByRole("button", { name: /^columns$/i }));
    const dialog = screen.getByRole("dialog");
    expect(dialog.querySelector('[data-column-settings-key="model"]')).toBeDisabled();
    expect(within(dialog).queryByRole("menuitemcheckbox", { name: /^actions$/i })).toBeNull();
  });

  it.each(Object.keys(fieldsByTab))(
    "%s keeps visible headers, checked column-settings entries, and labels in sync",
    async (entity) => {
      renderWithClient(
        <MemoryRouter initialEntries={[`/inventory/resources?entity=${encodeURIComponent(entity)}`]}>
          <InventoryResources />
        </MemoryRouter>,
      );

      await waitFor(() => {
        expect(apiMocks.listResourcesForTable).toHaveBeenCalledWith(
          entity,
          expect.any(Object),
        );
      });

      fireEvent.click(await screen.findByRole("button", { name: /^columns$/i }));
      const dialog = screen.getByRole("dialog");

      const visibleHeaderNames = collectVisibleHeaderNames();
      const checkedEntryNames = collectCheckedColumnSettingNames(dialog);

      expectUnique(visibleHeaderNames, `${entity}: visible headers`);
      expectUnique(checkedEntryNames, `${entity}: checked settings entries`);
      expect(visibleHeaderNames.sort(), entity).toEqual(checkedEntryNames.sort());
    },
  );
});

describe("InventoryResources delete blockers", () => {
  beforeEach(() => {
    apiMocks.listResources.mockResolvedValue({ items: [], total: 0 });
    apiMocks.listResourcesForTable.mockResolvedValue({
      items: [{ id: "site-a", name: "Site A", type: "site" }],
      total: 1,
      limit: 25,
      offset: 0,
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("keeps the delete dialog open and renders related blockers from a structured 409", async () => {
    apiMocks.deleteResource.mockRejectedValue(
      new ApiError({
        status: 409,
        code: "DELETE_BLOCKED",
        reasonCode: "DELETE_BLOCKED",
        message: "Cannot delete this record while related objects still exist",
        details: {
          total_count: 2,
          blockers: [
            {
              table: "dcim_device",
              entity_key: "Devices",
              count: 2,
              examples: [{ id: "dev-a", label: "router01" }],
            },
          ],
        },
      }),
    );

    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalled();
    });

    fireEvent.keyDown(await screen.findByRole("button", { name: /Actions: Site A/ }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: /^delete$/i }));

    expect(await within(dialog).findByText("Delete is blocked")).toBeInTheDocument();
    expect(within(dialog).getByText(/related records/i)).toBeInTheDocument();
    expect(within(dialog).getByText("router01")).toBeInTheDocument();
    expect(within(dialog).getByText("Devices")).toBeInTheDocument();
  });

  it("reuses the detailed blocker dialog when a single selected row fails bulk delete", async () => {
    apiMocks.deleteResource.mockRejectedValue(
      new ApiError({
        status: 409,
        code: "DELETE_BLOCKED",
        reasonCode: "DELETE_BLOCKED",
        message: "Cannot delete this record while related objects still exist",
        details: {
          total_count: 2,
          blockers: [
            {
              table: "dcim_device",
              entity_key: "Devices",
              count: 2,
              examples: [{ id: "dev-a", label: "router01" }],
            },
          ],
        },
      }),
    );

    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalled();
    });

    await screen.findByRole("button", { name: "Actions: Site A" });
    const rowSelectionCheckbox = screen.getAllByRole("checkbox").at(1);
    expect(rowSelectionCheckbox).toBeDefined();
    if (!rowSelectionCheckbox) {
      throw new Error("Expected at least one row selection checkbox");
    }
    fireEvent.click(rowSelectionCheckbox);
    fireEvent.click(screen.getByRole("button", { name: /delete \(1\)/i }));

    const confirmDialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(confirmDialog).getByRole("button", { name: /delete 1/i }));

    const blockedDialog = await screen.findByRole("alertdialog");
    expect(await within(blockedDialog).findByText("Delete is blocked")).toBeInTheDocument();
    expect(within(blockedDialog).getByText("router01")).toBeInTheDocument();
    expect(within(blockedDialog).getByText("Devices")).toBeInTheDocument();
  });

  it("shows related blocker details for multi-row bulk delete failures", async () => {
    apiMocks.listResourcesForTable.mockResolvedValue({
      items: [
        { id: "site-a", name: "Site A", type: "site" },
        { id: "site-b", name: "Site B", type: "site" },
      ],
      total: 2,
      limit: 25,
      offset: 0,
    });
    apiMocks.deleteResource.mockRejectedValue(
      new ApiError({
        status: 409,
        code: "DELETE_BLOCKED",
        message: "Cannot delete this record while related objects still exist",
        details: {
          totalCount: 1,
          blockers: [
            {
              table: "dcim_device",
              entityKey: "Devices",
              count: 1,
              examples: [{ id: "dev-a", label: "router01" }],
            },
          ],
        },
      }),
    );

    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalled();
    });
    expect(await screen.findByText("Site A")).toBeInTheDocument();
    expect(await screen.findByText("Site B")).toBeInTheDocument();

    const rowSelectionCheckboxes = screen.getAllByRole("checkbox").slice(1);
    expect(rowSelectionCheckboxes).toHaveLength(2);
    rowSelectionCheckboxes.forEach((checkbox) => fireEvent.click(checkbox));
    fireEvent.click(screen.getByRole("button", { name: /delete \(2\)/i }));

    const confirmDialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(confirmDialog).getByRole("button", { name: /delete 2/i }));

    const failureDialog = await screen.findByRole("alertdialog");
    expect(await within(failureDialog).findByText("Some items were not deleted")).toBeInTheDocument();
    expect(within(failureDialog).getByText("Site A")).toBeInTheDocument();
    expect(within(failureDialog).getByText("Site B")).toBeInTheDocument();
    expect(within(failureDialog).getAllByText("router01")).toHaveLength(2);
    expect(within(failureDialog).getAllByText("Devices")).toHaveLength(2);
  });

  it.each([
    { error: new ApiError({ status: 409, code: "CONFLICT", message: "Internal conflict detail" }), message: "Delete is blocked by related records" },
    { error: new Error("Delete request failed"), message: "Delete request failed" },
    { error: { unexpected: true }, message: "Unknown error" },
  ])("preserves bulk-delete message decisions: $message", async ({ error, message }) => {
    apiMocks.deleteResource.mockRejectedValue(error);
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}>
        <InventoryResources />
      </MemoryRouter>,
    );
    await screen.findByText("Site A");
    fireEvent.click(screen.getAllByRole("checkbox")[1]!);
    fireEvent.click(screen.getByRole("button", { name: /delete \(1\)/i }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /delete 1/i }));
    const failureDialog = await screen.findByRole("alertdialog");
    expect(await within(failureDialog).findByText(message)).toBeInTheDocument();
    expect(within(failureDialog).queryByText("Internal conflict detail")).not.toBeInTheDocument();
  });

  it("shows per-row reasons for bulk edit failures", async () => {
    apiMocks.listResourcesForTable.mockResolvedValue({
      items: [
        { id: "site-a", name: "Site A", type: "site" },
        { id: "site-b", name: "Site B", type: "site" },
      ],
      total: 2,
      limit: 25,
      offset: 0,
    });
    apiMocks.bulkUpdateResources.mockResolvedValue({
      updated: ["site-a"],
      failed: [{ id: "site-b", reason: "Region is required" }],
    });

    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}>
        <InventoryResources />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(apiMocks.listResourcesForTable).toHaveBeenCalled();
    });
    expect(await screen.findByText("Site A")).toBeInTheDocument();
    expect(await screen.findByText("Site B")).toBeInTheDocument();

    const rowSelectionCheckboxes = screen.getAllByRole("checkbox").slice(1);
    expect(rowSelectionCheckboxes).toHaveLength(2);
    rowSelectionCheckboxes.forEach((checkbox) => fireEvent.click(checkbox));
    fireEvent.click(screen.getByRole("button", { name: /edit fields \(2\)/i }));

    const editDialog = await screen.findByRole("dialog");
    fireEvent.click(within(editDialog).getByRole("button", { name: /apply bulk edit/i }));

    const failureDialog = await screen.findByRole("alertdialog");
    expect(await within(failureDialog).findByText("Some items were not updated")).toBeInTheDocument();
    expect(within(failureDialog).getByText("Site B")).toBeInTheDocument();
    expect(within(failureDialog).getByText("Region is required")).toBeInTheDocument();
    expect(within(failureDialog).queryByText("Site A")).not.toBeInTheDocument();
  });
});
