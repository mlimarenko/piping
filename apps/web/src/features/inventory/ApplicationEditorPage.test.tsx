import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import ApplicationEditorPage from "./ApplicationEditorPage";

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

const application = { id: "app-1", name: "Payments", role_id: "role-1", description: "Card payments", comments: "note", owner_id: "owner-1", tenant_id: "tenant-1", tags: ["tag-1"] };
const role = { id: "role-1", name: "Core", description: "", comments: "", owner_id: null };

function mount(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/inventory/dataflows/applications" element={<div>Applications list</div>} />
    <Route path="/inventory/dataflows/applications/new" element={<ApplicationEditorPage />} />
    <Route path="/inventory/dataflows/applications/:id/edit" element={<ApplicationEditorPage />} />
  </Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  catalog.listApplications.mockResolvedValue([application]);
  catalog.listApplicationRoles.mockResolvedValue([role]);
  catalog.getApplication.mockResolvedValue(application);
  catalog.updateApplication.mockResolvedValue(application);
  catalog.createApplication.mockResolvedValue(application);
  api.listAllResources.mockResolvedValue([{ id: "owner-1", name: "Alice" }, { id: "tenant-1", name: "Acme" }, { id: "tag-1", name: "prod" }]);
});

describe("application editor", () => {
  it("loads an application and updates it with the catalog payload", async () => {
    mount("/inventory/dataflows/applications/app-1/edit");
    expect(await screen.findByRole("heading", { level: 1, name: "Application — Payments" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: /^Application name/ }), { target: { value: "Payments EU" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(catalog.updateApplication).toHaveBeenCalledWith("app-1", expect.objectContaining({
      name: "Payments EU", role_id: "role-1", description: "Card payments", comments: "note",
      owner_id: "owner-1", tenant_id: "tenant-1", tags: ["tag-1"],
    })));
    expect(await screen.findByText("Applications list")).toBeInTheDocument();
  });

  it("requires a name and sends the selected role on create", async () => {
    mount("/inventory/dataflows/applications/new");
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Required");
    expect(catalog.createApplication).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: /^Application name/ }), { target: { value: "Billing" } });
    await screen.findByRole("option", { name: "Core" });
    fireEvent.change(screen.getByRole("combobox", { name: "Role" }), { target: { value: "role-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(catalog.createApplication).toHaveBeenCalledWith(expect.objectContaining({
      name: "Billing", role_id: "role-1", description: "", comments: "", owner_id: null, tenant_id: null, tags: [],
    })));
  });
});
