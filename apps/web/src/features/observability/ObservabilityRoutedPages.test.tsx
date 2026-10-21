import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import type * as ApiModule from "./api";
import type { AlertSourceItem, MetricQueryItem, MetricSetDetail, QuerySourceItem } from "./api";
import QuerySourceEditor from "./QuerySourceEditor";
import QuerySourceDetail from "./QuerySourceDetail";
import MetricQueryEditor from "./MetricQueryEditor";
import MetricQueryDetail from "./MetricQueryDetail";
import MetricSetEditor from "./MetricSetEditor";
import MetricSetDetailPage from "./MetricSetDetail";
import AlertSourceEditor from "./AlertSourceEditor";
import AlertSourceDetail from "./AlertSourceDetail";

const api = vi.hoisted(() => ({
  listQuerySources: vi.fn(),
  createQuerySource: vi.fn(),
  updateQuerySource: vi.fn(),
  deleteQuerySource: vi.fn(),
  listMetricQueries: vi.fn(),
  createMetricQuery: vi.fn(),
  updateMetricQuery: vi.fn(),
  deleteMetricQuery: vi.fn(),
  listMetricSets: vi.fn(),
  createMetricSet: vi.fn(),
  updateMetricSet: vi.fn(),
  deleteMetricSet: vi.fn(),
  listAlertSources: vi.fn(),
  createAlertSource: vi.fn(),
  updateAlertSource: vi.fn(),
  deleteAlertSource: vi.fn(),
}));
vi.mock("./api", async (original) => ({ ...(await original<typeof ApiModule>()), ...api }));
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));
vi.mock("@/features/connections/api", () => ({
  searchConnectionTargets: vi.fn().mockResolvedValue([]),
}));

const source: QuerySourceItem = {
  id: "qs-1", slug: "vm", name: "VictoriaMetrics", query_url: "http://vm/api/v1/query",
  credential_ref: null, timeout_ms: 5000, source_type: "prometheus", enabled: true,
  options: {}, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
};
const metric: MetricQueryItem = {
  id: "mq-1", slug: "up", name: "Up", label: "Up", query: "up", unit: null, description: null,
  sort_order: 0, default_show_in_inspector: true, default_show_in_list: true, enabled: true,
  source_type: "prometheus", query_type: "promql", value_type: "auto", widget_type: "stat",
  options: {}, category: null, semantic: null, exporter_id: null,
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
};
const set: MetricSetDetail = {
  id: "ms-1", name: "Core set", slug: "core", description: "d", enabled: true,
  items: [{
    id: "it-1", metric_query_id: "mq-1", metric_query_name: "Up", metric_query_slug: "up",
    list_render: "auto", show_in_inspector: true, show_in_list: true, show_in_hover: false,
    sort_order: 0, label_override: null, unit_override: null, metric_set_id: "ms-1",
    created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  }],
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
};
const alert: AlertSourceItem = {
  id: "as-1", name: "Alertmanager", query_url: "http://am/api/v2/alerts", credential_ref: null,
  timeout_ms: 5000, source_type: "alertmanager", enabled: true, options: {},
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
};

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function renderPages(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/observability/sources/new" element={<QuerySourceEditor />} />
            <Route path="/observability/sources/:id/edit" element={<QuerySourceEditor />} />
            <Route path="/observability/sources/:id" element={<QuerySourceDetail />} />
            <Route path="/observability/metrics/new" element={<MetricQueryEditor />} />
            <Route path="/observability/metrics/:id/edit" element={<MetricQueryEditor />} />
            <Route path="/observability/metrics/:id" element={<MetricQueryDetail />} />
            <Route path="/observability/sets/new" element={<MetricSetEditor />} />
            <Route path="/observability/sets/:id/edit" element={<MetricSetEditor />} />
            <Route path="/observability/sets/:id" element={<MetricSetDetailPage />} />
            <Route path="/observability/alert-sources/new" element={<AlertSourceEditor />} />
            <Route path="/observability/alert-sources/:id/edit" element={<AlertSourceEditor />} />
            <Route path="/observability/alert-sources/:id" element={<AlertSourceDetail />} />
            <Route path="/observability" element={<div>observability list</div>} />
          </Routes>
          <Location />
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listQuerySources.mockResolvedValue({ items: [source], total: 1 });
  api.listMetricQueries.mockResolvedValue({ items: [metric], total: 1 });
  api.listMetricSets.mockResolvedValue({ items: [set], total: 1 });
  api.listAlertSources.mockResolvedValue({ items: [alert], total: 1 });
  api.createQuerySource.mockImplementation((body) => Promise.resolve({ ...source, ...body, id: "qs-new" }));
  api.updateQuerySource.mockImplementation((id, body) => Promise.resolve({ ...source, ...body, id }));
  api.createMetricQuery.mockImplementation((body) => Promise.resolve({ ...metric, ...body, id: "mq-new" }));
  api.updateMetricQuery.mockImplementation((id, body) => Promise.resolve({ ...metric, ...body, id }));
  api.createMetricSet.mockImplementation((body) => Promise.resolve({ ...set, ...body, id: "ms-new" }));
  api.updateMetricSet.mockImplementation((id, body) => Promise.resolve({ ...set, ...body, id }));
  api.createAlertSource.mockImplementation((body) => Promise.resolve({ ...alert, ...body, id: "as-new" }));
  api.updateAlertSource.mockImplementation((id, body) => Promise.resolve({ ...alert, ...body, id }));
  api.deleteQuerySource.mockResolvedValue(undefined);
  api.deleteMetricQuery.mockResolvedValue(undefined);
  api.deleteMetricSet.mockResolvedValue(undefined);
  api.deleteAlertSource.mockResolvedValue(undefined);
});

describe("observability routed entity pages", () => {
  it.each([
    ["/observability/sources/qs-1/edit", "updateQuerySource"],
    ["/observability/metrics/mq-1/edit", "updateMetricQuery"],
    ["/observability/sets/ms-1/edit", "updateMetricSet"],
    ["/observability/alert-sources/as-1/edit", "updateAlertSource"],
  ] as const)("freezes the whole draft during save and unlocks it after failure at %s", async (path, mutation) => {
    let reject!: (error: Error) => void;
    const pending = new Promise((_, fail) => { reject = fail; });
    api[mutation].mockReturnValueOnce(pending);
    renderPages(path);
    const name = await screen.findByRole("textbox", { name: /^name\b/i });
    fireEvent.change(name, { target: { value: "Unsaved draft" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(api[mutation]).toHaveBeenCalledOnce());
    for (const role of ["textbox", "spinbutton", "combobox", "switch"] as const) {
      for (const input of screen.queryAllByRole(role)) expect(input).toBeDisabled();
    }
    expect(screen.getByRole("button", { name: /^cancel$/i })).toBeDisabled();
    fireEvent.change(name, { target: { value: "Must not overwrite pending draft" } });
    expect(name).toHaveValue("Unsaved draft");
    await act(async () => {
      reject(new Error("save failed"));
      await pending.catch(() => undefined); // The rejected API fixture is handled by the mutation's error path.
    });
    await waitFor(() => expect(name).not.toBeDisabled());
    expect(name).toHaveValue("Unsaved draft");
    expect(screen.getByTestId("location")).toHaveTextContent(path);
  });

  it.each([
    ["/observability/sources/new", "New data source"],
    ["/observability/metrics/new", "New metric query"],
    ["/observability/sets/new", "New metric set"],
    ["/observability/alert-sources/new", "New alert source"],
  ])("renders the create editor chrome at %s", async (path, title) => {
    renderPages(path);
    expect(await screen.findByRole("heading", { name: title })).toBeInTheDocument();
    expect(screen.getByTestId("entity-editor-page")).toBeInTheDocument();
  });

  it.each([
    ["/observability/sources/qs-1/edit", "Edit data source"],
    ["/observability/metrics/mq-1/edit", "Edit metric query"],
    ["/observability/sets/ms-1/edit", "Edit metric set"],
    ["/observability/alert-sources/as-1/edit", "Edit alert source"],
  ])("renders the edit editor chrome at %s", async (path, title) => {
    renderPages(path);
    expect(await screen.findByRole("heading", { name: title })).toBeInTheDocument();
  });

  it.each([
    ["/observability/sources/qs-1", "VictoriaMetrics", "/observability/sources/qs-1/edit"],
    ["/observability/metrics/mq-1", "Up", "/observability/metrics/mq-1/edit"],
    ["/observability/sets/ms-1", "Core set", "/observability/sets/ms-1/edit"],
    ["/observability/alert-sources/as-1", "Alertmanager", "/observability/alert-sources/as-1/edit"],
  ])("shows a read-only detail at %s linking to edit", async (path, heading, editHref) => {
    renderPages(path);
    expect(await screen.findByRole("heading", { name: heading })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Edit" })).toHaveAttribute("href", editHref);
  });

  it("creates a query source and opens its detail page", async () => {
    renderPages("/observability/sources/new");
    await screen.findByRole("heading", { name: "New data source" });

    const nameInput = await screen.findByRole("textbox", { name: /^name\b/i });
    fireEvent.change(nameInput, { target: { value: "New source" } });
    fireEvent.change(screen.getByPlaceholderText("http://victoriametrics:8428/api/v1/query"), {
      target: { value: "http://x/api/v1/query" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => expect(api.createQuerySource).toHaveBeenCalled());
    expect(api.createQuerySource).toHaveBeenCalledWith(
      expect.objectContaining({ name: "New source", query_url: "http://x/api/v1/query" }),
    );
    expect(await screen.findByTestId("location")).toHaveTextContent("/observability/sources/qs-new");
  });

  it("updates a metric query from its edit page", async () => {
    renderPages("/observability/metrics/mq-1/edit");
    await screen.findByRole("heading", { name: "Edit metric query" });

    const textarea = await screen.findByPlaceholderText(/up\{/);
    fireEvent.change(textarea, { target: { value: "up == 0" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(api.updateMetricQuery).toHaveBeenCalledWith("mq-1", expect.objectContaining({ query: "up == 0" })));
    expect(await screen.findByTestId("location")).toHaveTextContent("/observability/metrics/mq-1");
  });

  it("deletes an alert source from its detail page and returns to the list", async () => {
    renderPages("/observability/alert-sources/as-1");
    await screen.findByRole("heading", { name: "Alertmanager" });

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(api.deleteAlertSource).toHaveBeenCalledWith("as-1"));
    expect(await screen.findByTestId("location")).toHaveTextContent(
      "/observability?tab=inspector-metrics&view=alerts",
    );
  });

  it("confirms metric query deletion using the display label, not the raw name", async () => {
    api.listMetricQueries.mockResolvedValue({
      items: [{ ...metric, name: "cpu_usage_pct", label: "CPU Usage %" }],
      total: 1,
    });
    renderPages("/observability/metrics/mq-1");
    expect(await screen.findByRole("heading", { name: "CPU Usage %" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/CPU Usage %/)).toBeInTheDocument();
    expect(within(dialog).queryByText(/cpu_usage_pct/)).not.toBeInTheDocument();
  });
});
