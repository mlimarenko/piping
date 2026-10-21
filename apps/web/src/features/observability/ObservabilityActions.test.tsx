import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import ObservabilityPage from "./ObservabilityPage";
import type * as Api from "./api";
import { setupServer } from "msw/node";
import { handlers } from "@/demo/handlers";

const api = vi.hoisted(() => ({
  listQuerySources: vi.fn(), listMetricQueries: vi.fn(), listMetricSets: vi.fn(),
  listQuerySourceAssignments: vi.fn(), listMetricSetAssignments: vi.fn(),
  listAlertSources: vi.fn(), listAlertSourceAssignments: vi.fn(),
  listAllExporterAssignments: vi.fn(), listExporters: vi.fn(), getSdLabelSettings: vi.fn(),
  deleteQuerySource: vi.fn(), deleteExporterAssignment: vi.fn(),
  deleteMetricQuery: vi.fn(), deleteMetricSet: vi.fn(), deleteAlertSource: vi.fn(),
  deleteQuerySourceAssignment: vi.fn(), deleteMetricSetAssignment: vi.fn(), deleteAlertSourceAssignment: vi.fn(),
}));
vi.mock("./api", async original => ({ ...await original<typeof Api>(), ...api }));
vi.mock("./useScopeEntities", () => ({ useScopeEntities: () => ({ resolveName: () => "Global", options: [], loading: false }) }));
// Preview requests are independent of the list commands under test.
vi.mock("./InspectorPreviewPanel", () => ({ default: () => null }));
vi.mock("./SdPreviewPanel", () => ({ default: () => null }));
vi.mock("./QuickStartWizard", () => ({ default: () => null }));
function Location() { return <output data-testid="location">{useLocation().pathname}</output>; }
function mount(path: string, writable = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{ roles: [], isAdmin: writable, can: action => action === "READ" || writable, canAny: () => true, hasRole: () => false }}><MemoryRouter initialEntries={[path]}><ObservabilityPage /><Location /></MemoryRouter></PermissionsContext.Provider></QueryClientProvider>);
}
beforeEach(() => {
  vi.clearAllMocks();
  for (const key of ["listMetricQueries", "listMetricSets", "listQuerySourceAssignments", "listMetricSetAssignments", "listAlertSources", "listAlertSourceAssignments", "listExporters"] as const) api[key].mockResolvedValue({ items: [] });
  api.listQuerySources.mockResolvedValue({ items: [{ id: "source-1", name: "Metrics", source_type: "prometheus", enabled: true, query_url: "https://metrics.test/query" }] });
  api.listAllExporterAssignments.mockResolvedValue([{ id: "assign-1", exporter_name: "Node", exporter_id: "node", scope_type: "global", scope_id: null, enabled: true, weight: 0 }]);
  api.getSdLabelSettings.mockResolvedValue({ groups: {} });
  api.deleteQuerySource.mockResolvedValue(undefined);
  api.deleteExporterAssignment.mockResolvedValue(undefined);
  for (const key of ["deleteMetricQuery", "deleteMetricSet", "deleteAlertSource", "deleteQuerySourceAssignment", "deleteMetricSetAssignment", "deleteAlertSourceAssignment"] as const) api[key].mockResolvedValue(undefined);
});

it("keeps the workspace title and changes only active inspector actions", async () => {
  mount("/observability?tab=inspector-metrics&view=sources");
  const create = await screen.findByRole("button", { name: /^new source$/i });
  expect(create.closest('[data-slot="page-actions"]')).not.toBeNull();
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Observability");
  fireEvent.mouseDown(screen.getByRole("tab", { name: /metric cards/i }), { button: 0, ctrlKey: false });
  expect((await screen.findAllByRole("button", { name: /^new metric$/i })).some(button => button.closest('[data-slot="page-actions"]'))).toBe(true);
  expect(screen.queryByRole("button", { name: /^new source$/i })).not.toBeInTheDocument();
});

it("renders populated demo metric groups after Cards and routes their edit menu", async () => {
  const server = setupServer(...handlers);
  server.listen({ onUnhandledRequest: "error" });
  let groups: Api.MetricSetListResponse;
  try {
    const response = await fetch(`${window.location.origin}/api/v1/monitoring/metric-sets`);
    expect(response.ok).toBe(true);
    groups = await response.json();
  } finally {
    server.close();
  }
  expect(groups.items.length).toBeGreaterThan(0);
  api.listMetricSets.mockResolvedValue(groups);
  mount("/observability?tab=inspector-metrics&view=cards");
  fireEvent.mouseDown(screen.getByRole("tab", { name: /metric groups/i }), { button: 0, ctrlKey: false });
  const group = groups.items[0]!;
  const row = (await screen.findByText(group.name)).closest("tr")!;
  expect(group.items.length).toBeGreaterThan(0);
  expect(within(row).getByText(String(group.items.length))).toBeInTheDocument();
  fireEvent.click(screen.getByRole("columnheader", { name: /^metrics$/i }).querySelector("button")!);
  expect(within(row).getByText(group.name)).toBeInTheDocument();
  fireEvent.pointerDown(within(row).getByRole("button", { name: /actions:/i }), { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^edit$/i }));
  expect(screen.getByTestId("location")).toHaveTextContent(`/observability/sets/${group.id}/edit`);
});

it("does not navigate the row when its menu opens and confirms source deletion", async () => {
  mount("/observability?tab=inspector-metrics&view=sources");
  const trigger = await screen.findByRole("button", { name: /actions: Metrics/i });
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  expect(await screen.findByRole("menuitem", { name: /^edit$/i })).toBeInTheDocument();
  expect(screen.getByTestId("location")).toHaveTextContent(/^\/observability$/);
  fireEvent.click(screen.getByRole("menuitem", { name: /^delete$/i }));
  expect(api.deleteQuerySource).not.toHaveBeenCalled();
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^cancel$/i }));
  await waitFor(() => expect(trigger).toHaveFocus());
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^delete$/i }));
  await waitFor(() => expect(api.deleteQuerySource).toHaveBeenCalledWith("source-1"));
});

it("confirms exporter removal and returns focus on cancellation", async () => {
  mount("/observability");
  const trigger = await screen.findByRole("button", { name: /actions: Node/i });
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
  expect(api.deleteExporterAssignment).not.toHaveBeenCalled();
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^cancel$/i }));
  await waitFor(() => expect(trigger).toHaveFocus());
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^delete$/i }));
  await waitFor(() => expect(api.deleteExporterAssignment).toHaveBeenCalledWith("assign-1"));
});

it.each(["/observability", "/observability?tab=inspector-metrics&view=sources"])("hides empty row menus and global create actions for read-only %s", async path => {
  mount(path, false);
  await screen.findAllByText(path.includes("inspector") ? "Metrics" : "Node");
  if (path.includes("inspector")) expect(screen.queryByRole("button", { name: /actions:/i })).not.toBeInTheDocument();
  else {
    fireEvent.pointerDown(screen.getByRole("button", { name: /actions:/i }), { button: 0, ctrlKey: false });
    expect(await screen.findByRole("menuitem", { name: /^details$/i })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /^edit|^delete/i })).not.toBeInTheDocument();
  }
  expect(screen.queryByRole("button", { name: /^new /i })).not.toBeInTheDocument();
});

it.each([
  ["cards", "listMetricQueries", "deleteMetricQuery", { id: "row-1", name: "Target", label: "Target", enabled: true, source_type: "prometheus", options: {} }],
  ["groups", "listMetricSets", "deleteMetricSet", { id: "row-1", name: "Target", enabled: true, items: [] }],
  ["alerts", "listAlertSources", "deleteAlertSource", { id: "row-1", name: "Target", enabled: true, source_type: "alertmanager", query_url: "https://alerts.test" }],
  ["profiles", "listQuerySourceAssignments", "deleteQuerySourceAssignment", { id: "row-1", query_source_name: "Target", scope_type: "global", scope_id: null, enabled: true, weight: 0 }],
  ["profiles", "listMetricSetAssignments", "deleteMetricSetAssignment", { id: "row-1", metric_set_name: "Target", scope_type: "global", scope_id: null, enabled: true, weight: 0 }],
  ["alerts", "listAlertSourceAssignments", "deleteAlertSourceAssignment", { id: "row-1", alert_source_name: "Target", scope_type: "global", scope_id: null, enabled: true, weight: 0 }],
] as const)("confirms the correct row deletion in %s via %s", async (view, query, mutation, row) => {
  api[query].mockResolvedValue({ items: [row] });
  mount(`/observability?tab=inspector-metrics&view=${view}`);
  fireEvent.pointerDown(await screen.findByRole("button", { name: /actions: Target/i }), { button: 0, ctrlKey: false });
  expect(await screen.findByRole("menuitem", { name: /^edit$/i })).toBeInTheDocument();
  const remove = screen.getByRole("menuitem", { name: /^delete$/i });
  expect(remove).toHaveAttribute("data-variant", "destructive");
  fireEvent.click(remove);
  expect(api[mutation]).not.toHaveBeenCalled();
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^delete$/i }));
  await waitFor(() => expect(api[mutation]).toHaveBeenCalledWith("row-1"));
});

it("routes the source edit command rather than activating its detail row", async () => {
  mount("/observability?tab=inspector-metrics&view=sources");
  fireEvent.pointerDown(await screen.findByRole("button", { name: /actions: Metrics/i }), { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^edit$/i }));
  expect(screen.getByTestId("location")).toHaveTextContent("/observability/sources/source-1/edit");
});
