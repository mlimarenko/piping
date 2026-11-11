import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { ApiError } from "@/shared/api/client";
import DataflowGroupEditorPage from "./DataflowGroupEditorPage";

const catalog = vi.hoisted(() => ({
  listApplicationRoles: vi.fn(), createApplicationRole: vi.fn(), updateApplicationRole: vi.fn(), deleteApplicationRole: vi.fn(),
  listApplications: vi.fn(), getApplication: vi.fn(), createApplication: vi.fn(), updateApplication: vi.fn(), deleteApplication: vi.fn(),
  listDataflowGroups: vi.fn(), getDataflowGroup: vi.fn(), createDataflowGroup: vi.fn(), updateDataflowGroup: vi.fn(), deleteDataflowGroup: vi.fn(),
  listAddressGroups: vi.fn(), getAddressGroup: vi.fn(), createAddressGroup: vi.fn(), updateAddressGroup: vi.fn(), deleteAddressGroup: vi.fn(),
  bulkDataflows: vi.fn(),
  dataflowGroupDescendants: (groups: { id: string; parent_id?: string | null }[], id: string) => {
    const children = new Map<string, string[]>();
    for (const group of groups) if (group.parent_id) children.set(group.parent_id, [...(children.get(group.parent_id) ?? []), group.id]);
    const result = new Set([id]);
    const queue = [...(children.get(id) ?? [])];
    while (queue.length) { const next = queue.shift()!; if (result.has(next)) continue; result.add(next); queue.push(...(children.get(next) ?? [])); }
    return result;
  },
}));
const api = vi.hoisted(() => ({ listAllResources: vi.fn() }));
vi.mock("./dataflowCatalog-api", () => catalog);
vi.mock("./api", () => api);

const alpha = {
  id: "alpha", parent_id: null, application_id: "app-1", name: "Alpha", status: "active", effective_status: "disabled",
  blocked_by_group_id: null, tags: ["tag-2"], inherited_tags: ["tag-1"], effective_tags: ["tag-1", "tag-2"], description: "", comments: "", owner_id: null, tenant_id: null,
};
const beta = { ...alpha, id: "beta", parent_id: "alpha", name: "Beta", status: "draft", effective_status: "disabled", blocked_by_group_id: "alpha" };
const gamma = { ...alpha, id: "gamma", parent_id: null, application_id: "app-2", name: "Gamma", status: "active", effective_status: "active", inherited_tags: [], effective_tags: [], tags: [] };

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={["/inventory/dataflows/groups/alpha/edit"]}><Routes>
    <Route path="/inventory/dataflows/groups" element={<div>Groups list</div>} />
    <Route path="/inventory/dataflows/groups/:id/edit" element={<DataflowGroupEditorPage />} />
  </Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  catalog.getDataflowGroup.mockResolvedValue(alpha);
  catalog.listDataflowGroups.mockResolvedValue([alpha, beta, gamma]);
  catalog.listApplications.mockResolvedValue([{ id: "app-1", name: "Payments" }, { id: "app-2", name: "Billing" }]);
  catalog.updateDataflowGroup.mockResolvedValue(alpha);
  api.listAllResources.mockResolvedValue([{ id: "tag-1", name: "core-tag" }, { id: "tag-2", name: "edge-tag" }]);
});

describe("flow group editor", () => {
  it("never offers a descendant as the parent and shows effective status and inherited tags read-only", async () => {
    mount();
    expect(await screen.findByRole("heading", { level: 1, name: "Group — Alpha" })).toBeInTheDocument();
    const parentSelect = screen.getByRole("combobox", { name: "Parent group" });
    expect(await within(parentSelect).findByRole("option", { name: /^Gamma/ })).toBeInTheDocument();
    expect(within(parentSelect).queryByRole("option", { name: /^Beta/ })).not.toBeInTheDocument();

    const facts = screen.getByRole("group", { name: "Effective status" });
    expect(within(facts).getAllByText("Disabled").length).toBeGreaterThan(0);
    expect(within(facts).getByText("core-tag")).toBeInTheDocument();
  });

  it("blocks a parent from another application without silently changing the group", async () => {
    mount();
    await screen.findByRole("heading", { level: 1, name: "Group — Alpha" });
    await screen.findByRole("option", { name: /^Gamma/ });
    fireEvent.change(screen.getByRole("combobox", { name: "Parent group" }), { target: { value: "gamma" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    expect(screen.getByRole("alert")).toHaveTextContent("A child group must stay in the same application.");
    expect(catalog.updateDataflowGroup).not.toHaveBeenCalled();
  });

  it("surfaces a backend reference conflict for an application or parent change", async () => {
    catalog.updateDataflowGroup.mockRejectedValue(new ApiError({ status: 409, message: "flows reference this group", reasonCode: "CONFLICT" }));
    mount();
    await screen.findByRole("heading", { level: 1, name: "Group — Alpha" });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(catalog.updateDataflowGroup).toHaveBeenCalledWith("alpha", expect.objectContaining({ name: "Alpha", status: "active" })));
    expect(await screen.findByText("Move its flows and child groups first.")).toBeInTheDocument();
  });
});
