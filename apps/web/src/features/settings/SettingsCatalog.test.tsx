import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { SERVER_SETTING_DEFINITIONS } from "@/shared/settings/registry";
import SettingsCatalog from "./SettingsCatalog";
import AdminSettingsPage from "./AdminSettingsPage";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import type * as SettingsHooks from "@/shared/settings/hooks";
import type * as SettingsApi from "@/shared/settings/api";
import type * as ReactI18next from "react-i18next";

const hooks = vi.hoisted(() => ({
  useAdminSettingDefaults: vi.fn(),
  useSetting: vi.fn(),
}));
const defaultsApi = vi.hoisted(() => ({
  fetchAdminSettingDefaults: vi.fn(),
  upsertAdminSettingDefault: vi.fn(),
}));
vi.mock("@/shared/settings/api", async original => ({
  ...(await original<typeof SettingsApi>()), ...defaultsApi,
}));
vi.mock("@/shared/settings/hooks", async (original) => ({
  ...(await original<typeof SettingsHooks>()), ...hooks,
}));
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));
// Keys stand in for translated copy so category labels are deterministic.
vi.mock("react-i18next", async (original) => ({
  ...(await original<typeof ReactI18next>()),
  useTranslation: () => ({ t: (key: string) => key }),
}));

function renderWithRouter(ui: ReactNode, initialEntries: string[] = ["/administration/settings"]) {
  return render(ui, { wrapper: ({ children }) => (
    <MemoryRouter initialEntries={initialEntries}>{children}</MemoryRouter>
  ) });
}

function renderAdminCatalog(initialEntries?: string[]) {
  return renderWithRouter(
    <AllPermissions>
      <SettingsCatalog mode="admin" />
    </AllPermissions>,
    initialEntries,
  );
}

function CategoryHistoryProbe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <>
    <span data-testid="category-location">{location.pathname}{location.search}{location.hash}</span>
    <button onClick={() => { void navigate(-1); }}>Previous category</button>
  </>;
}

describe("SettingsCatalog admin section navigation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.values(defaultsApi).forEach(mock => mock.mockReset());
    hooks.useAdminSettingDefaults.mockReturnValue({
      data: [],
      isLoading: false,
      isSaving: false,
      upsertDefault: vi.fn(),
      deleteDefault: vi.fn(),
    });
  });

  it("navigates URL categories with native links and restores query/hash context on Back", async () => {
    renderWithRouter(<AllPermissions><SettingsCatalog mode="admin" /><CategoryHistoryProbe /></AllPermissions>, [
      "/administration/settings?origin=return#catalog",
    ]);

    const categoryCounts = new Map<string, number>();
    for (const definition of SERVER_SETTING_DEFINITIONS) {
      categoryCounts.set(
        definition.category,
        (categoryCounts.get(definition.category) ?? 0) + 1,
      );
    }
    const expectedLabels = ["appearance", "connections", "workspace", "assistant", "data"].map(
      (category) => `settingsCatalog.category.${category} (${categoryCounts.get(category)})`,
    );

    const navigation = screen.getByRole("navigation", { name: "settingsCatalog.sections" });
    const links = within(navigation).getAllByRole("link");
    expect(links).toHaveLength(expectedLabels.length);
    expect(links.map((link) => link.textContent)).toEqual(expectedLabels);
    expect(links[0]).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "settingsCatalog.category.appearance" }),
    ).toBeInTheDocument();

    expect(links[1]).toHaveAttribute("href", "/administration/settings?origin=return&category=connections#catalog");
    fireEvent.click(links[1]!);

    expect(links[1]).toHaveAttribute("aria-current", "page");
    expect(links[0]).not.toHaveAttribute("aria-current");
    expect(
      screen.getByRole("heading", { name: "settingsCatalog.category.connections" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Previous category" }));
    await waitFor(() => expect(screen.getByTestId("category-location")).toHaveTextContent(
      "/administration/settings?origin=return#catalog",
    ));
    expect(links[0]).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "settingsCatalog.category.appearance" })).toBeInTheDocument();
  });

  it("deep-links to a category from the URL", () => {
    renderAdminCatalog(["/administration/settings?category=workspace"]);
    expect(
      screen.getByRole("heading", { name: "settingsCatalog.category.workspace" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "settingsCatalog.category.appearance" })).not.toBeInTheDocument();
  });

  it("falls back to the first rendered category on an unknown URL param", () => {
    renderAdminCatalog(["/administration/settings?category=nonexistent"]);
    expect(
      screen.getByRole("heading", { name: "settingsCatalog.category.appearance" }),
    ).toBeInTheDocument();
  });

  it("validates labeled JSON and reports success only after the write succeeds", () => {
    const upsertDefault = vi.fn();
    hooks.useAdminSettingDefaults.mockReturnValue({ data: [], isLoading: false, isSaving: false, upsertDefault, deleteDefault: vi.fn() });
    renderWithRouter(<AllPermissions><SettingsCatalog mode="admin" keys={["table.column_visibility"]} /></AllPermissions>);
    const field = screen.getByRole("textbox", { name: "settingsRegistry.tableColumnVisibility.title" });
    fireEvent.change(field, { target: { value: "invalid JSON" } });
    fireEvent.click(screen.getByRole("button", { name: "save" }));
    expect(upsertDefault).not.toHaveBeenCalled();
    fireEvent.change(field, { target: { value: '{"devices":["name"]}' } });
    fireEvent.click(screen.getByRole("button", { name: "save" }));
    expect(upsertDefault).toHaveBeenCalledTimes(1);
    expect(notify).not.toHaveBeenCalled();
    upsertDefault.mock.calls[0]![1].onSuccess();
    expect(notify).toHaveBeenCalledWith("success", "settingsCatalog.saved");
  });

  it("renders the routed page with one primary heading and named controls", () => {
    renderWithRouter(<AllPermissions><AdminSettingsPage /></AllPermissions>);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("combobox", { name: "settingsRegistry.appTheme.title" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "settingsCatalog.adminTitle" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "settingsCatalog.category.appearance" })).toBeInTheDocument();
  });

  it("allows an embedded admin catalog to opt into its own title", () => {
    renderWithRouter(<AllPermissions><SettingsCatalog mode="admin" showHeader /></AllPermissions>);
    expect(screen.getByRole("heading", { name: "settingsCatalog.adminTitle" })).toBeInTheDocument();
    expect(screen.getByText("settingsCatalog.adminDescription")).toBeInTheDocument();
  });

  it("resets the selected override through the existing mutation", () => {
    const deleteDefault = vi.fn();
    hooks.useAdminSettingDefaults.mockReturnValue({ data: [{ key: "table.column_visibility", value: { devices: ["name"] } }], isLoading: false, isSaving: false, upsertDefault: vi.fn(), deleteDefault });
    renderWithRouter(<AllPermissions><SettingsCatalog mode="admin" keys={["table.column_visibility"]} /></AllPermissions>);
    fireEvent.click(screen.getByRole("button", { name: "settingsCatalog.reset" }));
    expect(deleteDefault).toHaveBeenCalledWith("table.column_visibility", expect.any(Object));
  });

  it("keeps READ-only JSON visible without save or reset controls", () => {
    renderWithRouter(<PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false }}>
      <SettingsCatalog mode="admin" keys={["table.column_visibility"]} />
    </PermissionsContext.Provider>);
    expect(screen.getByRole("textbox", { name: "settingsRegistry.tableColumnVisibility.title" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "save" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "settingsCatalog.reset" })).not.toBeInTheDocument();
  });

  it("keeps an unsaved JSON editor mounted when another setting saves and the cached defaults refetch fails", async () => {
    const actualHooks = await vi.importActual<typeof SettingsHooks>("@/shared/settings/hooks");
    hooks.useAdminSettingDefaults.mockImplementation(actualHooks.useAdminSettingDefaults);
    const columns = { key: "table.column_visibility", value: {}, updated_at: "2026-10-09T00:00:00Z" };
    const pageSize = { key: "table.page_size", value: 50, updated_at: "2026-10-09T00:00:00Z" };
    const savedPageSize = { ...pageSize, value: 100 };
    defaultsApi.upsertAdminSettingDefault.mockResolvedValue(savedPageSize);
    defaultsApi.fetchAdminSettingDefaults
      .mockRejectedValueOnce(new Error("Mock background refetch failed"))
      .mockResolvedValueOnce([columns, savedPageSize]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    client.setQueryData(["settings", "admin-defaults"], [columns, pageSize]);
    renderWithRouter(<QueryClientProvider client={client}><AllPermissions>
      <SettingsCatalog mode="admin" keys={["table.column_visibility", "table.page_size"]} />
    </AllPermissions></QueryClientProvider>);
    const editor = screen.getByRole("textbox", { name: "settingsRegistry.tableColumnVisibility.title" });
    const draft = '{"devices":["name"]}';
    fireEvent.change(editor, { target: { value: draft } });
    fireEvent.keyDown(screen.getByRole("combobox", { name: "settingsRegistry.tablePageSize.title" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("option", { name: "settingsRegistry.tablePageSizeOptions.100" }));
    await waitFor(() => expect(defaultsApi.upsertAdminSettingDefault).toHaveBeenCalledWith("table.page_size", 100));
    const alert = await screen.findByRole("alert");
    expect(defaultsApi.fetchAdminSettingDefaults).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("textbox", { name: "settingsRegistry.tableColumnVisibility.title" })).toBe(editor);
    expect(editor).toHaveValue(draft);
    fireEvent.click(within(alert).getByRole("button", { name: "retry" }));
    await waitFor(() => expect(defaultsApi.fetchAdminSettingDefaults).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(screen.getByRole("textbox", { name: "settingsRegistry.tableColumnVisibility.title" })).toBe(editor);
    expect(editor).toHaveValue(draft);
    expect(defaultsApi.upsertAdminSettingDefault).toHaveBeenCalledTimes(1);
  });

  it("replaces editors on an initial defaults error without cached data and restores them on retry", async () => {
    const actualHooks = await vi.importActual<typeof SettingsHooks>("@/shared/settings/hooks");
    hooks.useAdminSettingDefaults.mockImplementation(actualHooks.useAdminSettingDefaults);
    defaultsApi.fetchAdminSettingDefaults
      .mockRejectedValueOnce(new Error("Mock initial defaults failure"))
      .mockResolvedValueOnce([]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    renderWithRouter(<QueryClientProvider client={client}><AllPermissions>
      <SettingsCatalog mode="admin" keys={["table.column_visibility"]} />
    </AllPermissions></QueryClientProvider>);
    const alert = await screen.findByRole("alert");
    expect(screen.queryByRole("textbox", { name: "settingsRegistry.tableColumnVisibility.title" })).not.toBeInTheDocument();
    fireEvent.click(within(alert).getByRole("button", { name: "retry" }));
    expect(await screen.findByRole("textbox", { name: "settingsRegistry.tableColumnVisibility.title" })).toHaveValue("{}");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(defaultsApi.upsertAdminSettingDefault).not.toHaveBeenCalled();
  });
});
