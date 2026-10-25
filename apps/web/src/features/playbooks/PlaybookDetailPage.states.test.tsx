import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import { ApiError } from "@/shared/api/client";
import PageLayout from "@/shared/ui-shared/PageLayout";
import PlaybookDetailPage from "./PlaybookDetailPage";
import type * as PlaybookApi from "./api";
import "@/shared/i18n";

const api = vi.hoisted(() => ({ get: vi.fn(), versions: vi.fn(), group: vi.fn(), target: vi.fn(), preview: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof PlaybookApi>(), getPlaybook: api.get, listPlaybookVersions: api.versions, getPlaybookRunGroup: api.group, getTargetSet: api.target, previewPlaybookTargetSet: api.preview }));

const id = "00000000-0000-4000-8000-000000000129";
const returnTo = "/playbooks?filter=active#selected";
const permissions: PermissionsValue = { roles: [], isAdmin: false, can: () => true, canAny: () => true, hasRole: () => false };
function Destination() { const location = useLocation(); return <p>Returned: {location.pathname + location.search + location.hash}</p>; }
function mount(entityId = id, allowed = true, destination = returnTo) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{ ...permissions, can: () => allowed }}>
    <MemoryRouter initialEntries={[`/playbooks/${entityId}?${new URLSearchParams({ returnTo: destination })}`]}><Routes>
      <Route path="/playbooks/:id" element={<PageLayout><PlaybookDetailPage /></PageLayout>} />
      <Route path="/playbooks" element={<Destination />} />
    </Routes></MemoryRouter>
  </PermissionsContext.Provider></QueryClientProvider>);
}
function assertState() {
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Playbooks");
  expect(screen.getAllByRole("link", { name: "Back" })).toHaveLength(1);
  expect(screen.getByRole("link", { name: "Back" })).toHaveAttribute("href", returnTo);
  expect(document.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
  expect(document.querySelector('[data-slot="entity-editor-page"]')).toBeNull();
}
describe("PlaybookDetailPage conditional state chrome", () => {
  beforeEach(() => { Object.values(api).forEach((mock) => mock.mockReset()); api.versions.mockResolvedValue({ items: [] }); });
  it("keeps category title and exact query/hash Back while pending", () => {
    api.get.mockImplementation(() => new Promise(() => {}));
    mount();
    assertState();
    expect(screen.getByRole("status")).toHaveTextContent("Loading...");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Back" }));
    expect(screen.getByText(`Returned: ${returnTo}`)).toBeInTheDocument();
  });
  it.each([[404, "Page Not Found"], [500, "Unknown error"], [403, "You don't have access to this section"]] as const)("keeps one title/Back/error announcement for API %s", async (status, message) => {
    api.get.mockRejectedValue(new ApiError({ status, message: "internal backend diagnostic" }));
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    assertState();
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.queryByText("internal backend diagnostic")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Back" }));
    expect(screen.getByText(`Returned: ${returnTo}`)).toBeInTheDocument();
  });
  it("retries a missing record through the same chrome then restores unchanged entity detail", async () => {
    let resolve!: (value: unknown) => void;
    api.get.mockImplementationOnce(() => Promise.reject(new ApiError({ status: 404, message: "missing" })))
      .mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    mount();
    await screen.findByRole("alert");
    assertState();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(api.get).toHaveBeenCalledTimes(2));
    assertState();
    act(() => { resolve({ playbook: { id, name: "Loaded identity", runtime: "ansible", source_kind: "in_app", status: "active" }, current_version: null }); });
    await screen.findByRole("heading", { name: "Loaded identity" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getAllByRole("link", { name: "Back" })).toHaveLength(1);
    expect(screen.getByRole("link", { name: "Back" })).toHaveAttribute("href", returnTo);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(document.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
  });
  it.each([["invalid-id", true, "Page Not Found"], [id, false, "You don't have access to this section"]] as const)("retains chrome without a request for invalid/denied input %s", (entityId, allowed, message) => {
    mount(entityId, allowed);
    assertState();
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    expect(api.get).not.toHaveBeenCalled();
    expect(api.group).not.toHaveBeenCalled();
    expect(api.target).not.toHaveBeenCalled();
    expect(api.preview).not.toHaveBeenCalled();
  });
  it("treats an empty record as unavailable and validates the state Back destination", async () => {
    api.get.mockResolvedValue(null);
    mount(id, true, "https://foreign.example");
    expect(await screen.findByRole("alert")).toHaveTextContent("Page Not Found");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("link", { name: "Back" })).toHaveAttribute("href", "/playbooks");
  });
});
