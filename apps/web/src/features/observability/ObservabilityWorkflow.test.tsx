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
import ExporterAssignmentDetail from "./ExporterAssignmentDetail";
import ExporterAssignmentEditor from "./ExporterAssignmentEditor";
import SdLabelSettings from "./SdLabelSettings";
import { EXPORTER_ASSIGNMENTS_KEY } from "./keys";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

const api = vi.hoisted(() => ({ listAllExporterAssignments: vi.fn(), listExporters: vi.fn(), getSdLabelSettings: vi.fn(), updateSdLabelSettings: vi.fn(), getServiceDiscoveryPreview: vi.fn(), updateExporterAssignment: vi.fn(), createExporterAssignment: vi.fn(), deleteExporterAssignment: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof Api>(), ...api }));
vi.mock("./useScopeEntities", () => ({ useScopeEntities: () => ({ optionsByScope: { role: [], connection_target: [], site: [], tenant: [] }, resolveName: (scope: string, id: string) => scope === "global" ? "All matching targets" : `Synthetic ${id}`, isPending: false }) }));
vi.mock("./InspectorMetricsScreen", () => ({ default: () => <div>Inspector workbench</div> }));
vi.mock("./QuickStartWizard", () => ({ default: () => null }));
vi.mock("@/shared/hooks/useTablePageSize", () => ({ useTablePageSize: () => ({ value: 2, setValue: vi.fn() }) }));
vi.mock("@/shared/ui-shared/useTableColumnVisibility", () => ({ useTableColumnVisibility: () => ({ hiddenColumns: new Set(), setHidden: vi.fn(), reset: vi.fn(), source: "default" }) }));
const groups: Api.SdLabelGroups = { contacts: false, description: false, hardware: false, identifiers: false, location: false, network: false, role: false, status: false, tenant: false, device_statuses: ["active"], site_statuses: ["active"] };
const assignment = (id: string, name = id): Api.ExporterAssignmentItem => ({ id, name, exporter_name: "Synthetic Node", exporter_slug: "custom-profile", exporter_id: "profile-1", scope_type: "global", scope_id: null, enabled: true, weight: 0, target: "old.test", job: "old-job", scheme: "https", port: 9443, metrics_path: "/old", labels: { fixture: "old" }, created_at: "2026-10-16T00:00:00Z", updated_at: "2026-10-16T00:00:00Z" });
function Location() { const location = useLocation(); return <output data-testid="location">{location.pathname}{location.search}</output>; }
function mount(path = "/observability", readonly = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const view = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/observability" element={<ObservabilityPage />} />
    <Route path="/observability/exporter-assignments/new" element={<ExporterAssignmentEditor />} />
    <Route path="/observability/exporter-assignments/:id/edit" element={<ExporterAssignmentEditor />} />
    <Route path="/observability/exporter-assignments/:id" element={<ExporterAssignmentDetail />} />
    <Route path="/settings" element={<SdLabelSettings />} />
  </Routes><Location /></MemoryRouter>;
  render(<QueryClientProvider client={client}>{readonly ? <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => true, hasRole: () => false }}>{view}</PermissionsContext.Provider> : <AllPermissions>{view}</AllPermissions>}</QueryClientProvider>);
  return client;
}
beforeEach(async () => {
  vi.resetAllMocks();
  await i18n.changeLanguage("en");
  api.listAllExporterAssignments.mockResolvedValue([assignment("a", "Alpha"), assignment("b", "Beta"), assignment("c", "Gamma")]);
  api.listExporters.mockResolvedValue({ items: [] });
  api.getSdLabelSettings.mockResolvedValue({ groups });
  api.updateSdLabelSettings.mockImplementation(next => Promise.resolve({ groups: next }));
  api.getServiceDiscoveryPreview.mockResolvedValue({ hosts: [], groups: [], targets_count: 0, groups_count: 0 });
});

it("uses complete canonical records, filters before pagination and preserves URL list context", async () => {
  mount("/observability?page=2");
  const link = await screen.findByRole("link", { name: "Gamma" });
  expect(link).toHaveAttribute("href", "/observability/exporter-assignments/c?page=2");
  expect(screen.queryByRole("link", { name: "Alpha" })).not.toBeInTheDocument();
  fireEvent.change(screen.getByRole("textbox", { name: /search/i }), { target: { value: "Alpha" } });
  expect(await screen.findByRole("link", { name: "Alpha" })).toBeInTheDocument();
  expect(screen.getByTestId("location")).toHaveTextContent("?search=Alpha");
  fireEvent.click(screen.getByRole("link", { name: "Alpha" }));
  expect(await screen.findByRole("heading", { name: "Alpha" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: /^back$/i })).toHaveAttribute("href", "/observability?search=Alpha");
  expect(api.listAllExporterAssignments).toHaveBeenCalledWith();
  expect(api.getServiceDiscoveryPreview).not.toHaveBeenCalled();
});

it("filters and sorts the complete cached collection before slicing without forwarding view params to its API", async () => {
  api.listAllExporterAssignments.mockResolvedValue([
    { ...assignment("a", "Alpha"), weight: 30 },
    { ...assignment("b", "Beta"), weight: 50, enabled: false },
    { ...assignment("c", "Gamma"), weight: 10 },
    { ...assignment("d", "Delta"), weight: 20 },
  ]);
  const client = mount("/observability?enabled=true&sort=weight&order=asc");
  expect(await screen.findByRole("link", { name: "Gamma" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Delta" })).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Alpha" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Beta" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Next page" }));
  expect(await screen.findByRole("link", { name: "Alpha" })).toHaveAttribute("href", "/observability/exporter-assignments/a?sort=weight&order=asc&page=2&enabled=true");
  expect(screen.queryByRole("link", { name: "Gamma" })).not.toBeInTheDocument();
  expect(client.getQueryData<Api.ExporterAssignmentItem[]>(EXPORTER_ASSIGNMENTS_KEY)).toHaveLength(4);
  expect(api.listAllExporterAssignments).toHaveBeenCalledTimes(1);
  expect(api.listAllExporterAssignments).toHaveBeenCalledWith();
});

it("mounts preview only in collector and loads explicitly, including error/retry", async () => {
  api.getServiceDiscoveryPreview.mockRejectedValueOnce(new Error("private error"));
  mount();
  await screen.findByRole("link", { name: "Alpha" });
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

it("keeps unknown profile and all override null semantics on routed edit and updates cache", async () => {
  const original = assignment("c", "Gamma");
  api.updateExporterAssignment.mockImplementation((id, body) => {
    const saved = { ...original, ...body, id };
    api.listAllExporterAssignments.mockResolvedValue([saved]);
    return Promise.resolve(saved);
  });
  const client = mount("/observability/exporter-assignments/c/edit?page=2");
  const name = await screen.findByRole("textbox", { name: /assignment name/i });
  expect(screen.getByRole("combobox", { name: /^exporter profile/i })).toHaveTextContent("custom-profile");
  fireEvent.change(name, { target: { value: "Saved synthetic" } });
  for (const label of [/job label/i, /target override/i, /^port$/i, /metrics path/i, /labels \(JSON object\)/i]) fireEvent.change(screen.getByRole(label.source === "^port$" ? "spinbutton" : "textbox", { name: label }), { target: { value: "" } });
  fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
  await screen.findByRole("heading", { name: "Saved synthetic" });
  expect(api.updateExporterAssignment).toHaveBeenCalledWith("c", expect.objectContaining({ exporter_slug: "custom-profile", target: null, job: null, port: null, metrics_path: null, labels: null, scheme: "https", enabled: true, weight: 0, scope_id: null }));
  expect(client.getQueryData<Api.ExporterAssignmentItem[]>(EXPORTER_ASSIGNMENTS_KEY)?.some(row => row.name === "Saved synthetic")).toBe(true);
  expect(screen.getByTestId("location")).toHaveTextContent("/observability/exporter-assignments/c?page=2");
});

it("retains failed editor draft and asks before explicit cancellation", async () => {
  api.updateExporterAssignment.mockRejectedValue(new Error("private backend detail"));
  mount("/observability/exporter-assignments/c/edit");
  const name = await screen.findByRole("textbox", { name: /assignment name/i });
  fireEvent.change(name, { target: { value: "Retained draft" } });
  fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
  await screen.findByText(/Could not save/);
  expect(name).toHaveValue("Retained draft");
  fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
});

it.each(["assignments", "catalogue"] as const)("retains initialized editor draft and dirty guard after deferred %s refetch failure and retry", async source => {
  const client = mount("/observability/exporter-assignments/c/edit");
  const name = await screen.findByRole("textbox", { name: /assignment name/i });
  const labels = screen.getByRole("textbox", { name: /labels \(JSON object\)/i });
  fireEvent.change(name, { target: { value: "  Exact unsaved draft  " } });
  fireEvent.change(labels, { target: { value: '{ "custom": "retained", "n": 155 }' } });
  const pending = deferred<never>();
  const query = source === "assignments" ? api.listAllExporterAssignments : api.listExporters;
  query.mockReturnValueOnce(pending.promise);
  let refetch!: Promise<void>;
  act(() => { refetch = client.refetchQueries({ queryKey: source === "assignments" ? EXPORTER_ASSIGNMENTS_KEY : ["observability", "exporters"], exact: true }); });
  await act(async () => { pending.reject(new Error("synthetic reconnect failure")); await refetch; });
  expect(screen.getByRole("textbox", { name: /assignment name/i })).toBe(name);
  expect(name).toHaveValue("  Exact unsaved draft  ");
  expect(labels).toHaveValue('{ "custom": "retained", "n": 155 }');
  fireEvent.click(await screen.findByRole("button", { name: "Retry" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument());
  expect(name).toHaveValue("  Exact unsaved draft  ");
  expect(labels).toHaveValue('{ "custom": "retained", "n": 155 }');
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
});

it("shows initial catalogue failure truthfully and permits retry without claiming an initialized draft", async () => {
  api.listExporters.mockRejectedValueOnce(new Error("synthetic initial failure"));
  mount("/observability/exporter-assignments/c/edit");
  await screen.findByText("Failed to load");
  expect(screen.queryByRole("textbox", { name: /assignment name/i })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("textbox", { name: /assignment name/i })).toHaveValue("Gamma");
});

it("denies an initialized dirty editor when its write permission is removed", async () => {
  function PermissionsFixture() {
    const [allowed, setAllowed] = useState(true);
    return <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ" || allowed, canAny: () => allowed, hasRole: () => false }}><button onClick={() => setAllowed(false)}>Revoke permission</button><ExporterAssignmentEditor /></PermissionsContext.Provider>;
  }
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/observability/exporter-assignments/c/edit"]}><Routes><Route path="/observability/exporter-assignments/:id/edit" element={<PermissionsFixture />} /></Routes></MemoryRouter></QueryClientProvider>);
  fireEvent.change(await screen.findByRole("textbox", { name: /assignment name/i }), { target: { value: "Dirty before revocation" } });
  fireEvent.click(screen.getByRole("button", { name: "Revoke permission" }));
  await screen.findByText(/Your role doesn't grant/);
  expect(screen.queryByRole("textbox", { name: /assignment name/i })).not.toBeInTheDocument();
  expect(api.updateExporterAssignment).not.toHaveBeenCalled();
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
  api.deleteExporterAssignment.mockRejectedValue(new Error("synthetic rejection"));
  mount("/observability/exporter-assignments/c?page=2");
  fireEvent.click(await screen.findByRole("button", { name: /^delete$/i }));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^delete$/i }));
  await screen.findByText("Failed to delete");
  expect(screen.getByRole("heading", { name: "Gamma" })).toBeInTheDocument();
  expect(api.deleteExporterAssignment).toHaveBeenCalledWith("c");
});

for (const view of ["detail", "editor"] as const) {
  it.each(["loading", "error", "missing"])(`exporter ${view} distinguishes %s and retains its canonical exit`, async state => {
    if (state === "loading") api.listAllExporterAssignments.mockReturnValue(new Promise(() => {}));
    else if (state === "error") api.listAllExporterAssignments.mockRejectedValue(new Error("synthetic unavailable"));
    else api.listAllExporterAssignments.mockResolvedValue([]);
    mount(`/observability/exporter-assignments/missing${view === "editor" ? "/edit" : ""}?search=synthetic&page=2`);
    if (state === "loading") await screen.findByText(i18n.t("loading"));
    else await screen.findByText(i18n.t(state === "error" ? "observability.entityLoadFailed" : "observability.entityNotFound"));
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    const back = screen.queryByRole("link", { name: "Back" }) ?? screen.getByRole("button", { name: "Back" });
    fireEvent.click(back);
    expect(screen.getByTestId("location")).toHaveTextContent("/observability?search=synthetic");
  });
}

it("creates every request field, freezes pending inputs and returns saved detail", async () => {
  api.listExporters.mockResolvedValue({ items: [{ id: "profile-1", slug: "node", name: "Synthetic Node", enabled: true }] });
  let resolve!: (value: Api.ExporterAssignmentItem) => void;
  api.createExporterAssignment.mockImplementation(() => new Promise(r => { resolve = r; }));
  mount("/observability/exporter-assignments/new?search=synthetic");
  const exporter = await screen.findByRole("combobox", { name: /^exporter profile/i });
  fireEvent.keyDown(exporter, { key: "ArrowDown" });
  fireEvent.click(await screen.findByRole("option", { name: /Synthetic Node/i }));
  for (const [label, value] of [[/assignment name/i, "Synthetic created"], [/job label/i, "fixture-job"], [/target override/i, "192.0.2.155"], [/metrics path/i, "/fixture"], [/labels \(JSON object\)/i, '{"fixture":"PWA-155"}']] as const) fireEvent.change(screen.getByRole("textbox", { name: label }), { target: { value } });
  fireEvent.change(screen.getByRole("spinbutton", { name: "Port" }), { target: { value: "9155" } });
  fireEvent.change(screen.getByRole("spinbutton", { name: "Priority" }), { target: { value: "155" } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));
  await waitFor(() => expect(api.createExporterAssignment).toHaveBeenCalledWith(expect.objectContaining({ exporter_id: "profile-1", name: "Synthetic created", scope_type: "global", scope_id: null, weight: 155, enabled: true, target: "192.0.2.155", job: "fixture-job", scheme: null, port: 9155, metrics_path: "/fixture", labels: { fixture: "PWA-155" } })));
  expect(screen.getByRole("textbox", { name: /assignment name/i })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  const saved = assignment("created", "Synthetic created");
  api.listAllExporterAssignments.mockResolvedValue([saved]);
  act(() => resolve(saved));
  await screen.findByRole("heading", { name: saved.name! });
  expect(screen.getByTestId("location")).toHaveTextContent("/observability/exporter-assignments/created?search=synthetic");
});
