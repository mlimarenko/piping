import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import type * as Api from "./api";
import ObservabilityPage from "./ObservabilityPage";
import ProfileAssignmentDetail from "./ProfileAssignmentDetail";
import ProfileAssignmentEditor from "./ProfileAssignmentEditor";
import SdLabelSettings from "./SdLabelSettings";
import { PROFILE_ASSIGNMENTS_KEY, PROFILES_KEY } from "./keys";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

const api = vi.hoisted(() => ({ listAllProfileAssignments: vi.fn(), listAllProfiles: vi.fn(), previewProfile: vi.fn(), getSdLabelSettings: vi.fn(), updateSdLabelSettings: vi.fn(), getServiceDiscoveryPreview: vi.fn(), updateProfileAssignment: vi.fn(), createProfileAssignment: vi.fn(), deleteProfileAssignment: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof Api>(), ...api }));
vi.mock("./useScopeEntities", () => ({ useScopeEntities: () => ({ optionsByScope: { role: [], connection_target: [], site: [], tenant: [] }, resolveName: (scope: string, id: string) => scope === "global" ? "All matching targets" : `Synthetic ${id}`, isPending: false }) }));
vi.mock("./InspectorMetricsScreen", () => ({ default: () => <div>Inspector workbench</div> }));
vi.mock("./QuickStartWizard", () => ({ default: () => null }));
vi.mock("@/shared/hooks/useTablePageSize", () => ({ useTablePageSize: () => ({ value: 2, setValue: vi.fn() }) }));
vi.mock("@/shared/ui-shared/useTableColumnVisibility", () => ({ useTableColumnVisibility: () => ({ hiddenColumns: new Set(), setHidden: vi.fn(), reset: vi.fn(), source: "default" }) }));
const groups: Api.SdLabelGroups = { contacts: false, description: false, hardware: false, identifiers: false, location: false, network: false, role: false, status: false, tenant: false, device_statuses: ["active"], site_statuses: ["active"] };
const assignment = (id: string, name = id): Api.ProfileAssignment => ({ id, profile_name: name, profile_slug: "custom-profile", profile_id: "profile-1", scope_type: "global", scope_id: null, enabled: true, weight: 0, target: "old.test", created_at: "2026-10-16T00:00:00Z", updated_at: "2026-10-16T00:00:00Z" });
function Location() { const location = useLocation(); return <output data-testid="location">{location.pathname}{location.search}</output>; }
function mount(path = "/observability", readonly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const view = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/observability" element={<ObservabilityPage />} />
    <Route path="/observability/profile-assignments/new" element={<ProfileAssignmentEditor />} />
    <Route path="/observability/profile-assignments/:id/edit" element={<ProfileAssignmentEditor />} />
    <Route path="/observability/profile-assignments/:id" element={<ProfileAssignmentDetail />} />
    <Route path="/settings" element={<SdLabelSettings />} />
  </Routes><Location /></MemoryRouter>;
  render(<QueryClientProvider client={client}>{readonly ? <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => true, hasRole: () => false }}>{view}</PermissionsContext.Provider> : <AllPermissions>{view}</AllPermissions>}</QueryClientProvider>);
  return client;
}
beforeEach(async () => {
  vi.resetAllMocks();
  await i18n.changeLanguage("en");
  api.listAllProfileAssignments.mockResolvedValue([assignment("a", "Alpha"), assignment("b", "Beta"), assignment("c", "Gamma")]);
  api.listAllProfiles.mockResolvedValue([{ id: "profile-1", name: "Synthetic Node", slug: "node", enabled: true, items: [], item_count: 0, assignment_count: 0, created_at: "2026-01-01", updated_at: "2026-01-01" }]);
  api.previewProfile.mockResolvedValue({ hosts: [], groups: [], conflicts: [], targets_count: 0 });
  api.getSdLabelSettings.mockResolvedValue({ groups });
  api.updateSdLabelSettings.mockImplementation(next => Promise.resolve({ groups: next }));
  api.getServiceDiscoveryPreview.mockResolvedValue({ hosts: [], groups: [], targets_count: 0, groups_count: 0 });
});

it("filters before pagination and preserves URL list context", async () => {
  mount("/observability?page=2");
  expect(await screen.findAllByText("Gamma")).not.toHaveLength(0);
  const link = screen.getByRole("link", { name: "All matching targets" });
  expect(link).toHaveAttribute("href", "/observability/profile-assignments/c?page=2");
  expect(screen.queryByText("Alpha")).not.toBeInTheDocument();
  fireEvent.change(screen.getByRole("textbox", { name: /search/i }), { target: { value: "Alpha" } });
  expect(await screen.findAllByText("Alpha")).not.toHaveLength(0);
  expect(screen.getByTestId("location")).toHaveTextContent("?search=Alpha");
  fireEvent.click(screen.getByRole("link", { name: "All matching targets" }));
  expect(await screen.findByRole("heading", { name: "Alpha" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /^back$/i })).toHaveAttribute("href", "/observability?search=Alpha");
  expect(api.listAllProfileAssignments).toHaveBeenCalledWith();
  expect(api.getServiceDiscoveryPreview).not.toHaveBeenCalled();
});

it("filters and sorts profiles before pagination without forwarding view params", async () => {
  api.listAllProfileAssignments.mockResolvedValue([
    assignment("a", "Zulu"), { ...assignment("b", "Beta"), enabled: false },
    assignment("c", "Alpha"), assignment("d", "Delta"),
  ]);
  const client = mount("/observability?enabled=true&sort=profile&order=desc");
  expect(await screen.findAllByText("Zulu")).not.toHaveLength(0);
  expect(screen.getAllByText("Delta")).not.toHaveLength(0);
  expect(screen.queryByText("Alpha")).not.toBeInTheDocument();
  expect(screen.queryByText("Beta")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Next page" }));
  expect(await screen.findAllByText("Alpha")).not.toHaveLength(0);
  expect(screen.getByRole("link", { name: "All matching targets" })).toHaveAttribute("href", "/observability/profile-assignments/c?sort=profile&order=desc&page=2&enabled=true");
  expect(client.getQueryData<Api.ProfileAssignment[]>(PROFILE_ASSIGNMENTS_KEY)).toHaveLength(4);
  expect(api.listAllProfileAssignments).toHaveBeenCalledWith();
});

it("mounts preview only in collector and loads explicitly, including error/retry", async () => {
  api.getServiceDiscoveryPreview.mockRejectedValueOnce(new Error("private error"));
  mount();
  await screen.findAllByText("Alpha");
  fireEvent.click(screen.getByRole("link", { name: "Discovery settings" }));
  await screen.findByLabelText("Location");
  expect(api.getServiceDiscoveryPreview).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("link", { name: "Connect collector" }));
  expect(api.getServiceDiscoveryPreview).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Load sample" }));
  await screen.findByText("Failed to load data");
  expect(screen.queryByText("private error")).not.toBeInTheDocument();
  fireEvent.click(screen.getAllByRole("button", { name: "Retry" }).at(-1)!);
  await screen.findByText("No targets");
  expect(api.getServiceDiscoveryPreview).toHaveBeenCalledWith({ limit: 200 });
});

it("preserves profile and nullable target when editing and returns to detail", async () => {
  const original = assignment("c", "Gamma");
  api.updateProfileAssignment.mockImplementation((id, body) => {
    const saved = { ...original, ...body, id };
    api.listAllProfileAssignments.mockResolvedValue([saved]);
    return Promise.resolve(saved);
  });
  const client = mount("/observability/profile-assignments/c/edit?page=2");
  const target = await screen.findByRole("textbox", { name: /^target override$/i });
  fireEvent.change(target, { target: { value: "" } });
  fireEvent.click(await screen.findByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
  await screen.findByRole("heading", { name: "Gamma" });
  expect(api.updateProfileAssignment).toHaveBeenCalledWith("c", expect.objectContaining({ profile_id: "profile-1", target: null, enabled: true, scope_id: null, weight: 0 }));
  expect(client.getQueryData<Api.ProfileAssignment[]>(PROFILE_ASSIGNMENTS_KEY)?.some(row => row.id === "c" && row.target === null)).toBe(true);
  expect(screen.getByTestId("location")).toHaveTextContent("/observability/profile-assignments/c?page=2");
});

it("retains failed editor draft and asks before explicit cancellation", async () => {
  api.updateProfileAssignment.mockRejectedValue(new Error("private backend detail"));
  mount("/observability/profile-assignments/c/edit");
  const target = await screen.findByRole("textbox", { name: /^target override$/i });
  fireEvent.change(target, { target: { value: "retained.example" } });
  fireEvent.click(await screen.findByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
  await screen.findByText(/Could not save/);
  expect(target).toHaveValue("retained.example");
  fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
});

it.each(["assignments", "catalogue"] as const)("retains initialized draft after deferred %s refetch failure and retry", async source => {
  const client = mount("/observability/profile-assignments/c/edit");
  const target = await screen.findByRole("textbox", { name: /^target override$/i });
  fireEvent.change(target, { target: { value: "exact-unsaved.example" } });
  const pending = deferred<never>();
  const query = source === "assignments" ? api.listAllProfileAssignments : api.listAllProfiles;
  query.mockReturnValueOnce(pending.promise);
  let refetch!: Promise<void>;
  act(() => { refetch = client.refetchQueries({ queryKey: source === "assignments" ? PROFILE_ASSIGNMENTS_KEY : PROFILES_KEY, exact: true }); });
  await act(async () => { pending.reject(new Error("synthetic reconnect failure")); await refetch; });
  expect(target).toHaveValue("exact-unsaved.example");
  fireEvent.click(await screen.findByRole("button", { name: "Retry" }));
  expect(target).toHaveValue("exact-unsaved.example");
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
});

it("shows initial catalogue failure and permits retry", async () => {
  api.listAllProfiles.mockRejectedValueOnce(new Error("synthetic initial failure"));
  mount("/observability/profile-assignments/c/edit");
  await screen.findByText("Failed to load");
  expect(screen.queryByRole("textbox", { name: /^target override$/i })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("textbox", { name: /^target override$/i })).toHaveValue("old.test");
});

it("prevents mutation if write permission is removed from a dirty editor", async () => {
  function PermissionsFixture() {
    const [allowed, setAllowed] = useState(true);
    return <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ" || allowed, canAny: () => allowed, hasRole: () => false }}><button onClick={() => setAllowed(false)}>Revoke permission</button><ProfileAssignmentEditor /></PermissionsContext.Provider>;
  }
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/observability/profile-assignments/c/edit"]}><Routes><Route path="/observability/profile-assignments/:id/edit" element={<PermissionsFixture />} /></Routes></MemoryRouter></QueryClientProvider>);
  fireEvent.change(await screen.findByRole("textbox", { name: /^target override$/i }), { target: { value: "dirty.example" } });
  fireEvent.click(await screen.findByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Revoke permission" }));
  expect(screen.getByRole("button", { name: /^save$/i })).toBeDisabled();
  expect(api.updateProfileAssignment).not.toHaveBeenCalled();
});

it.each(["before", "during"] as const)("orders a delayed settings GET started %s PUT so it cannot overwrite acknowledged fields", async timing => {
  const original = { ...groups, contacts: true, description: true, device_statuses: ["active", "planned"], site_statuses: ["active", "offline"] };
  api.getSdLabelSettings.mockResolvedValue({ groups: original });
  const client = mount("/settings");
  const location = await screen.findByLabelText("Location");
  const oldRead = deferred<{ groups: Api.SdLabelGroups }>();
  const write = deferred<{ groups: Api.SdLabelGroups }>();
  let refetch!: Promise<void>;
  const startRead = () => {
    api.getSdLabelSettings.mockReturnValueOnce(oldRead.promise);
    act(() => { refetch = client.refetchQueries({ queryKey: ["monitoring", "sd-label-settings"], exact: true }); });
  };
  if (timing === "before") startRead();
  else api.updateSdLabelSettings.mockReturnValueOnce(write.promise);
  fireEvent.click(location);
  await waitFor(() => expect(api.updateSdLabelSettings).toHaveBeenCalledTimes(1));
  if (timing === "during") {
    startRead();
    act(() => write.resolve({ groups: { ...original, location: true } }));
  }
  await screen.findByText("Label settings saved");
  await act(async () => { oldRead.resolve({ groups: original }); await oldRead.promise; await refetch; });
  expect(location).toBeChecked();
  expect(client.getQueryData<{ groups: Api.SdLabelGroups }>(["monitoring", "sd-label-settings"])?.groups.location).toBe(true);
  fireEvent.click(screen.getByLabelText("Tenant"));
  await waitFor(() => expect(api.updateSdLabelSettings).toHaveBeenLastCalledWith({ ...original, location: true, tenant: true }));
  await screen.findByText("Label settings saved");
  api.getSdLabelSettings.mockResolvedValue({ groups: { ...original, location: true, tenant: true, network: true } });
  await act(async () => { await client.refetchQueries({ queryKey: ["monitoring", "sd-label-settings"], exact: true }); });
  await waitFor(() => expect(screen.getByLabelText("Network")).toBeChecked());
});

it("awaits query cancellation before writing and retains a retryable full draft if cancellation fails", async () => {
  const client = mount("/settings");
  const location = await screen.findByLabelText("Location");
  const cancellation = deferred<void>();
  vi.spyOn(client, "cancelQueries").mockReturnValueOnce(cancellation.promise);
  fireEvent.click(location);
  expect(api.updateSdLabelSettings).not.toHaveBeenCalled();
  await act(async () => { cancellation.reject(new Error("synthetic cancellation failure")); await cancellation.promise.catch(() => {}); });
  await screen.findByText(/Could not save/);
  expect(location).toBeChecked();
  expect(api.updateSdLabelSettings).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByText("Label settings saved");
  expect(api.updateSdLabelSettings).toHaveBeenCalledWith({ ...groups, location: true });
});

it("publishes an acknowledged in-flight save after unmount without sending abandoned queued writes", async () => {
  const client = mount("/settings");
  const write = deferred<{ groups: Api.SdLabelGroups }>();
  api.updateSdLabelSettings.mockReturnValueOnce(write.promise);
  fireEvent.click(await screen.findByLabelText("Location"));
  await waitFor(() => expect(api.updateSdLabelSettings).toHaveBeenCalledTimes(1));
  fireEvent.click(screen.getByLabelText("Tenant"));
  cleanup();
  await act(async () => { write.resolve({ groups: { ...groups, location: true } }); await write.promise; });
  expect(api.updateSdLabelSettings).toHaveBeenCalledTimes(1);
  expect(client.getQueryData<{ groups: Api.SdLabelGroups }>(["monitoring", "sd-label-settings"])?.groups).toEqual({ ...groups, location: true });
  expect(client.isFetching()).toBe(0);
});

it("serializes autosaves, preserves typed status drafts across refresh and retries failed writes", async () => {
  let resolve!: (value: { groups: Api.SdLabelGroups }) => void;
  api.updateSdLabelSettings.mockImplementationOnce(() => new Promise(r => { resolve = r; }));
  const client = mount("/settings");
  fireEvent.click(await screen.findByLabelText("Location"));
  await waitFor(() => expect(api.updateSdLabelSettings).toHaveBeenCalledTimes(1));
  fireEvent.click(screen.getByLabelText("Tenant"));
  const statuses = screen.getByRole("textbox", { name: "Device statuses" });
  fireEvent.change(statuses, { target: { value: " Planned, ACTIVE, planned " } });
  await act(async () => { await client.invalidateQueries({ queryKey: ["monitoring", "sd-label-settings"] }); });
  expect(statuses).toHaveValue(" Planned, ACTIVE, planned ");
  expect(api.updateSdLabelSettings).toHaveBeenCalledTimes(1);
  api.updateSdLabelSettings.mockRejectedValueOnce(new Error("synthetic failure"));
  act(() => resolve({ groups: { ...groups, location: true } }));
  await screen.findByText(/Could not save/);
  expect(screen.getByLabelText("Tenant")).toBeChecked();
  fireEvent.blur(statuses);
  await waitFor(() => expect(api.updateSdLabelSettings).toHaveBeenLastCalledWith(expect.objectContaining({ location: true, tenant: true, device_statuses: ["active", "planned"], site_statuses: ["active"] })));
  await screen.findByText("Label settings saved");
});

it("does not write on hydration or READ-only interactions", async () => {
  mount("/observability?tab=discovery-settings", true);
  expect(await screen.findByLabelText("Location")).toBeDisabled();
  expect(screen.getByRole("textbox", { name: "Device statuses" })).toBeDisabled();
  expect(api.updateSdLabelSettings).not.toHaveBeenCalled();
});

it("keeps record after delete failure and freezes pending delete identity", async () => {
  api.deleteProfileAssignment.mockRejectedValue(new Error("synthetic rejection"));
  mount("/observability/profile-assignments/c?page=2");
  fireEvent.click(await screen.findByRole("button", { name: /^delete$/i }));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^delete$/i }));
  await screen.findByText("Failed to delete");
  expect(screen.getByRole("heading", { name: "Gamma" })).toBeInTheDocument();
  expect(api.deleteProfileAssignment).toHaveBeenCalledWith("c");
});

for (const view of ["detail", "editor"] as const) {
  it.each(["loading", "error", "missing"])(`exporter ${view} distinguishes %s and retains its canonical exit`, async state => {
    if (state === "loading") api.listAllProfileAssignments.mockReturnValue(new Promise(() => {}));
    else if (state === "error") api.listAllProfileAssignments.mockRejectedValue(new Error("synthetic unavailable"));
    else api.listAllProfileAssignments.mockResolvedValue([]);
    mount(`/observability/profile-assignments/missing${view === "editor" ? "/edit" : ""}?search=synthetic&page=2`);
    if (state === "loading") await screen.findByText(i18n.t("loading"));
    else await screen.findByText(i18n.t(state === "error" ? "observability.entityLoadFailed" : "observability.entityNotFound"));
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    const back = screen.queryByRole("link", { name: "Back" }) ?? screen.getByRole("button", { name: "Back" });
    fireEvent.click(back);
    expect(screen.getByTestId("location")).toHaveTextContent("/observability?search=synthetic");
  });
}

it("creates an assignment, freezes pending inputs and returns saved detail", async () => {
  let resolve!: (value: Api.ProfileAssignment) => void;
  api.createProfileAssignment.mockImplementation(() => new Promise(r => { resolve = r; }));
  mount("/observability/profile-assignments/new?search=synthetic");
  const profile = await screen.findByRole("combobox", { name: /^profile/i });
  fireEvent.keyDown(profile, { key: "ArrowDown" });
  fireEvent.click(await screen.findByRole("option", { name: /Synthetic Node/i }));
  fireEvent.change(screen.getByRole("textbox", { name: /^target override$/i }), { target: { value: "192.0.2.155" } });
  fireEvent.change(screen.getByRole("spinbutton", { name: /weight/i }), { target: { value: "7" } });
  fireEvent.click(await screen.findByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Create" }));
  await waitFor(() => expect(api.createProfileAssignment).toHaveBeenCalledWith(expect.objectContaining({ profile_id: "profile-1", scope_type: "global", scope_id: null, enabled: true, target: "192.0.2.155", weight: 7 })));
  expect(screen.getByRole("textbox", { name: /^target override$/i })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  const saved = assignment("created", "Synthetic Node");
  api.listAllProfileAssignments.mockResolvedValue([saved]);
  act(() => resolve(saved));
  await screen.findByRole("heading", { name: saved.profile_name });
  expect(screen.getByTestId("location")).toHaveTextContent("/observability/profile-assignments/created?search=synthetic");
});
