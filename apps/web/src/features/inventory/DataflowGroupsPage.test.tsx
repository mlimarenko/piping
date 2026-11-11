import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { ApiError } from "@/shared/api/client";
import DataflowGroupsPage from "./DataflowGroupsPage";

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

const parent = {
  id: "group-core", parent_id: null, application_id: "app-1", name: "Core", status: "disabled", effective_status: "disabled",
  blocked_by_group_id: null, tags: ["tag-2"], inherited_tags: [], effective_tags: ["tag-1", "tag-2"], description: "", comments: "", owner_id: null, tenant_id: null,
};
const child = {
  id: "group-edge", parent_id: "group-core", application_id: "app-1", name: "Edge", status: "active", effective_status: "disabled",
  blocked_by_group_id: "group-core", tags: [], inherited_tags: ["tag-1"], effective_tags: ["tag-1"], description: "", comments: "", owner_id: null, tenant_id: null,
};

function Probe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <>
    <span data-testid="url">{location.search}</span>
    <button type="button" data-testid="browser-back" onClick={() => void navigate(-1)}>History back</button>
  </>;
}

function mount(path = "/inventory/dataflows/groups", readOnly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/dataflows/groups" element={<DataflowGroupsPage />} />
  </Routes><Probe /></MemoryRouter>;
  render(<QueryClientProvider client={client}>
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
  catalog.listDataflowGroups.mockResolvedValue([child, parent]);
  catalog.listApplications.mockResolvedValue([{ id: "app-1", name: "Payments" }]);
  catalog.deleteDataflowGroup.mockResolvedValue(undefined);
  api.listAllResources.mockResolvedValue([{ id: "tag-1", name: "core-tag" }, { id: "tag-2", name: "edge-tag" }]);
});

describe("flow groups tree", () => {
  it("renders a deterministic parent-before-child tree with direct and effective state", async () => {
    mount();
    await screen.findByRole("button", { name: "Inspect Edge" });
    expect(screen.getAllByRole("button", { name: /^Inspect / }).map(node => node.textContent)).toEqual(["Core", "Edge"]);
    expect(screen.getByRole("link", { name: "Flow groups" })).toHaveAttribute("aria-current", "page");

    const childRow = screen.getByRole("button", { name: "Inspect Edge" }).closest("tr") as HTMLElement;
    expect(within(childRow).getByText("Active")).toBeInTheDocument();
    expect(within(childRow).getByText("Disabled")).toBeInTheDocument();
    expect(within(childRow).getByText("Payments")).toBeInTheDocument();
    expect(within(childRow).getByText("core-tag")).toBeInTheDocument();
    expect(screen.getAllByRole("columnheader").map(header => header.textContent)).toEqual(
      expect.arrayContaining(["Group name", "Application", "Group status", "Effective status", "Blocked by", "Tags", "Actions"]),
    );
  });

  it("indents children and keeps the ancestor breadcrumb visible", async () => {
    mount();
    const parentOpener = await screen.findByRole("button", { name: "Inspect Core" });
    const childOpener = screen.getByRole("button", { name: "Inspect Edge" });
    expect(parentOpener.parentElement?.style.paddingInlineStart).toBe("0rem");
    expect(childOpener.parentElement?.style.paddingInlineStart).toBe("1.25rem");
    expect(within(childOpener.parentElement as HTMLElement).getByText("Core")).toBeInTheDocument();
  });

  it("filters by exact application and parent with an explicit root choice", async () => {
    mount("/inventory/dataflows/groups?application=app-1&parent=group-core");
    await screen.findByRole("button", { name: "Inspect Edge" });
    expect(screen.queryByRole("button", { name: "Inspect Core" })).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Parent group"), { target: { value: "root" } });
    await waitFor(() => expect(screen.getByTestId("url")).toHaveTextContent("parent=root"));
    expect(screen.getByTestId("url")).toHaveTextContent("application=app-1");
    expect(screen.getByRole("button", { name: "Inspect Core" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect Edge" })).not.toBeInTheDocument();
  });

  it("keeps the read action for a long name", async () => {
    const longName = "Core network services owned by the platform reliability group for every production region";
    catalog.listDataflowGroups.mockResolvedValue([{ ...parent, name: longName }]);
    mount();
    const opener = await screen.findByRole("button", { name: `Inspect ${longName}` });
    expect(opener).toBeVisible();
    expect(screen.getByRole("button", { name: `Actions: ${longName}` })).toBeInTheDocument();
  });

  it("lets a read-only user inspect hierarchy context and effective state", async () => {
    mount("/inventory/dataflows/groups", true);
    const { dialog } = await openDetail("Edge");
    expect(within(dialog).getAllByText("Core").length).toBeGreaterThan(0);
    expect(within(dialog).getByText("Payments")).toBeInTheDocument();
    expect(within(dialog).getByText("Active")).toBeInTheDocument();
    expect(within(dialog).getByText("Disabled")).toBeInTheDocument();
    expect(within(dialog).getByText("Blocked by")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Inherited tags" })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Effective tags" })).toBeInTheDocument();
    expect(within(dialog).getAllByText("core-tag").length).toBeGreaterThan(0);
    expect(within(dialog).queryByRole("link", { name: "Edit" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });

  it("links directly to the flows of this group and its descendants", async () => {
    mount();
    const { dialog } = await openDetail("Core");
    expect(within(dialog).getByRole("link", { name: "Flows" })).toHaveAttribute("href", "/inventory/dataflows?group=group-core");
  });

  it("closes with browser back and restores name focus without losing search and filters", async () => {
    mount("/inventory/dataflows/groups?q=edge&application=app-1&parent=group-core");
    const { opener } = await openDetail("Edge");
    expect(screen.getByTestId("url")).toHaveTextContent("selected=group-edge");
    fireEvent.click(screen.getByTestId("browser-back"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edge" })).not.toBeInTheDocument());
    await waitFor(() => expect(opener).toHaveFocus());
    expect(screen.getByPlaceholderText("Search")).toHaveValue("edge");
    expect(screen.getByLabelText("Application")).toHaveValue("app-1");
    expect(screen.getByLabelText("Parent group")).toHaveValue("group-core");
    expect(screen.getByTestId("url")).toHaveTextContent("q=edge");
    expect(screen.getByTestId("url")).toHaveTextContent("application=app-1");
    expect(screen.getByTestId("url")).toHaveTextContent("parent=group-core");
  });

  it("keeps search, filters and the open detail when a referenced delete is blocked", async () => {
    catalog.deleteDataflowGroup.mockRejectedValue(new ApiError({ status: 409, message: "children", reasonCode: "DELETE_BLOCKED" }));
    mount("/inventory/dataflows/groups?q=edge&application=app-1&parent=group-core");
    const { dialog } = await openDetail("Edge");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    const confirm = await within(dialog).findByRole("group", { name: "Delete" });
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Move its flows and child groups first.");
    expect(catalog.deleteDataflowGroup).toHaveBeenCalledWith("group-edge", expect.anything());
    expect(screen.getByRole("dialog", { name: "Edge" })).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Search")).toHaveValue("edge");
    expect(screen.getByTestId("url")).toHaveTextContent("q=edge");
    expect(screen.getByTestId("url")).toHaveTextContent("selected=group-edge");
  });

  it("shows a referenced group delete as blocked from the row actions", async () => {
    catalog.deleteDataflowGroup.mockRejectedValue(new ApiError({ status: 409, message: "children", reasonCode: "DELETE_BLOCKED" }));
    mount();
    await screen.findByRole("button", { name: "Inspect Core" });
    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Core" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Move its flows and child groups first.");
    expect(catalog.deleteDataflowGroup).toHaveBeenCalledWith("group-core", expect.anything());
  });
});
