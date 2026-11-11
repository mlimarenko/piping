import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { ApiError } from "@/shared/api/client";
import ApplicationsPage from "./ApplicationsPage";

const catalog = vi.hoisted(() => ({
  listApplicationRoles: vi.fn(), createApplicationRole: vi.fn(), updateApplicationRole: vi.fn(), deleteApplicationRole: vi.fn(),
  listApplications: vi.fn(), getApplication: vi.fn(), createApplication: vi.fn(), updateApplication: vi.fn(), deleteApplication: vi.fn(),
  listDataflowGroups: vi.fn(), getDataflowGroup: vi.fn(), createDataflowGroup: vi.fn(), updateDataflowGroup: vi.fn(), deleteDataflowGroup: vi.fn(),
  listAddressGroups: vi.fn(), getAddressGroup: vi.fn(), createAddressGroup: vi.fn(), updateAddressGroup: vi.fn(), deleteAddressGroup: vi.fn(),
  bulkDataflows: vi.fn(),
}));
const api = vi.hoisted(() => ({ listAllResources: vi.fn() }));
vi.mock("./dataflowCatalog-api", () => catalog);
vi.mock("./api", () => api);

const application = { id: "app-1", name: "Payments", role_id: "role-1", description: "Card payments", comments: "Renew yearly", owner_id: "owner-1", tenant_id: "tenant-1", tags: ["tag-1"] };
const role = { id: "role-1", name: "Core", description: "Core role", comments: "", owner_id: "owner-1" };

function Probe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <>
    <span data-testid="url">{location.pathname}{location.search}</span>
    <button type="button" data-testid="browser-back" onClick={() => void navigate(-1)}>History back</button>
  </>;
}

function mount(path = "/inventory/dataflows/applications", readOnly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/dataflows/applications" element={<ApplicationsPage />} />
  </Routes><Probe /></MemoryRouter>;
  return render(<QueryClientProvider client={client}>
    {readOnly
      ? <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false }}>{content}</PermissionsContext.Provider>
      : <AllPermissions>{content}</AllPermissions>}
  </QueryClientProvider>);
}

async function openDetail(name: string) {
  const opener = await screen.findByRole("button", { name: `Inspect ${name}` });
  fireEvent.click(opener);
  const dialog = await screen.findByRole("dialog", { name });
  return { opener, dialog };
}

beforeEach(() => {
  vi.clearAllMocks();
  catalog.listApplications.mockResolvedValue([application]);
  catalog.listApplicationRoles.mockResolvedValue([role]);
  catalog.createApplicationRole.mockResolvedValue(role);
  catalog.updateApplicationRole.mockResolvedValue(role);
  catalog.deleteApplicationRole.mockResolvedValue(undefined);
  catalog.deleteApplication.mockResolvedValue(undefined);
  api.listAllResources.mockResolvedValue([
    { id: "owner-1", name: "Alice" },
    { id: "tenant-1", name: "Acme" },
    { id: "tag-1", name: "prod" },
  ]);
});

describe("applications catalog", () => {
  it("lists applications with role and only the metadata the rows carry", async () => {
    mount();
    expect(await screen.findByRole("button", { name: "Inspect Payments" })).toBeInTheDocument();
    const row = screen.getByRole("button", { name: "Inspect Payments" }).closest("tr");
    expect(row).not.toBeNull();
    expect(within(row as HTMLElement).getByText("Core")).toBeInTheDocument();
    expect(within(row as HTMLElement).getByText("Alice")).toBeInTheDocument();
    expect(within(row as HTMLElement).getByText("Acme")).toBeInTheDocument();
    expect(within(row as HTMLElement).getByText("prod")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Applications" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Flows" })).not.toHaveAttribute("aria-current");
  });

  it("omits empty owner, tenant and tag columns instead of showing compulsory blanks", async () => {
    catalog.listApplications.mockResolvedValue([{ ...application, owner_id: null, tenant_id: null, tags: [] }]);
    mount();
    await screen.findByRole("button", { name: "Inspect Payments" });
    const headers = screen.getAllByRole("columnheader").map(header => header.textContent);
    expect(headers).toContain("Name");
    expect(headers).toContain("Role");
    expect(headers).toContain("Description");
    expect(headers).toContain("Actions");
    expect(headers).not.toContain("Owner");
    expect(headers).not.toContain("Tenant");
    expect(headers).not.toContain("Tags");
  });

  it("keeps search in the URL and filters the list", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Payments" });
    fireEvent.change(screen.getByPlaceholderText("Search"), { target: { value: "card" } });
    await waitFor(() => expect(screen.getByTestId("url")).toHaveTextContent("?q=card"));
    expect(screen.getByRole("button", { name: "Inspect Payments" })).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText("Search"), { target: { value: "nothing" } });
    expect(await screen.findByText("No results found")).toBeInTheDocument();
  });

  it("keeps the complete read action for a long name", async () => {
    const longName = "Payments settlement reconciliation and chargeback processing for the international card network";
    catalog.listApplications.mockResolvedValue([{ ...application, name: longName }]);
    mount();
    const opener = await screen.findByRole("button", { name: `Inspect ${longName}` });
    expect(opener).toBeVisible();
    expect(opener).toHaveTextContent(longName);
    expect(screen.getByRole("button", { name: `Actions: ${longName}` })).toBeInTheDocument();
  });

  it("lets a read-only user inspect the full application metadata", async () => {
    mount("/inventory/dataflows/applications", true);
    const { dialog } = await openDetail("Payments");
    expect(within(dialog).getByText("Core")).toBeInTheDocument();
    expect(within(dialog).getByText("Card payments")).toBeInTheDocument();
    expect(within(dialog).getByText("Renew yearly")).toBeInTheDocument();
    expect(within(dialog).getByText("Alice")).toBeInTheDocument();
    expect(within(dialog).getByText("Acme")).toBeInTheDocument();
    expect(within(dialog).getByText("prod")).toBeInTheDocument();
    expect(within(dialog).queryByRole("link", { name: "Edit" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });

  it("links directly to the flows scoped by this application", async () => {
    mount();
    const { dialog } = await openDetail("Payments");
    expect(within(dialog).getByRole("link", { name: "Flows" })).toHaveAttribute("href", "/inventory/dataflows?application=app-1");
  });

  it("closes with browser back and restores name focus without losing the search", async () => {
    mount("/inventory/dataflows/applications?q=pay");
    const { opener } = await openDetail("Payments");
    expect(screen.getByTestId("url")).toHaveTextContent("selected=app-1");
    fireEvent.click(screen.getByTestId("browser-back"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Payments" })).not.toBeInTheDocument());
    await waitFor(() => expect(opener).toHaveFocus());
    expect(screen.getByPlaceholderText("Search")).toHaveValue("pay");
    expect(screen.getByTestId("url")).toHaveTextContent("q=pay");
    expect(screen.getByTestId("url")).not.toHaveTextContent("selected");
  });

  it("restores the selected name after flow-link navigation, Back and Escape", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Payments" });
    fireEvent.click(screen.getByRole("button", { name: "Inspect Payments" }));
    const dialog = await screen.findByRole("dialog", { name: "Payments" });
    fireEvent.click(within(dialog).getByRole("link", { name: "Flows" }));
    expect(screen.getByTestId("url")).toHaveTextContent("application=app-1");
    fireEvent.click(screen.getByTestId("browser-back"));
    await screen.findByRole("dialog", { name: "Payments" });
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Payments" })).not.toBeInTheDocument());
    const restored = await screen.findByRole("button", { name: "Inspect Payments" });
    await waitFor(() => expect(restored).toHaveFocus());
  });

  it("keeps search and the open detail when a referenced delete is blocked", async () => {
    catalog.deleteApplication.mockRejectedValue(new ApiError({ status: 409, message: "related flows", reasonCode: "DELETE_BLOCKED" }));
    mount("/inventory/dataflows/applications?q=pay");
    const { dialog } = await openDetail("Payments");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    const confirm = await within(dialog).findByRole("group", { name: "Delete" });
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Delete is blocked by related records");
    expect(catalog.deleteApplication).toHaveBeenCalledWith("app-1", expect.anything());
    expect(screen.getByRole("dialog", { name: "Payments" })).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Search")).toHaveValue("pay");
    expect(screen.getByTestId("url")).toHaveTextContent("q=pay");
    expect(screen.getByTestId("url")).toHaveTextContent("selected=app-1");
  });

  it("creates a role from the compact dialog with the contract payload", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Payments" });
    fireEvent.click(screen.getByRole("button", { name: "New role" }));
    const dialog = await screen.findByRole("dialog", { name: "New role" });
    fireEvent.change(within(dialog).getByRole("textbox", { name: /^Role name/ }), { target: { value: "Billing" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: /^Role description/ }), { target: { value: "Money" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(catalog.createApplicationRole).toHaveBeenCalledWith(
      { name: "Billing", description: "Money", comments: "", owner_id: null },
    ));
  });

  it("updates a role with only the canonical role fields", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Payments" });
    fireEvent.click(screen.getByRole("button", { name: "Edit: Core" }));
    const dialog = await screen.findByRole("dialog", { name: "Role — Core" });
    fireEvent.change(within(dialog).getByRole("textbox", { name: /^Role description/ }), { target: { value: "Updated role" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(catalog.updateApplicationRole).toHaveBeenCalledWith(
      "role-1", { name: "Core", description: "Updated role", comments: "", owner_id: "owner-1" },
    ));
  });

  it("reports a role that is still used instead of the backend message", async () => {
    catalog.deleteApplicationRole.mockRejectedValue(new ApiError({ status: 409, message: "SQL detail", reasonCode: "DELETE_BLOCKED" }));
    mount();
    await screen.findByRole("button", { name: "Inspect Payments" });
    fireEvent.click(screen.getByRole("button", { name: "Delete: Core" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The role is used by applications.");
    expect(catalog.deleteApplicationRole).toHaveBeenCalledWith("role-1", expect.anything());
  });

  it("reports a referenced application delete as blocked from the row actions", async () => {
    catalog.deleteApplication.mockRejectedValue(new ApiError({ status: 409, message: "related flows", reasonCode: "DELETE_BLOCKED" }));
    mount();
    await screen.findByRole("button", { name: "Inspect Payments" });
    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Payments" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Delete is blocked by related records");
    expect(catalog.deleteApplication).toHaveBeenCalledWith("app-1", expect.anything());
  });

  it("gates create and row actions behind inventory permissions", async () => {
    mount("/inventory/dataflows/applications", true);
    await screen.findByRole("button", { name: "Inspect Payments" });
    expect(screen.queryByRole("link", { name: "New application" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Actions: Payments" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete: Core" })).not.toBeInTheDocument();
  });
});
