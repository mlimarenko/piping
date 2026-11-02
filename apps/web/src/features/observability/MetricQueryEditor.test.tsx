import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AllPermissions } from "@/test/permissions";
import MetricQueryEditor from "./MetricQueryEditor";
import type * as ApiModule from "./api";
import type { QuerySourceItem, MetricQueryItem, MetricQueryTestResponse } from "./api";

const apiMocks = vi.hoisted(() => ({
  createMetricQuery: vi.fn(),
  updateMetricQuery: vi.fn(),
  listMetricQueries: vi.fn(),
  listQuerySources: vi.fn(),
  testMetricQuery: vi.fn(),
}));

vi.mock("@/features/observability/api", async () => {
  const actual = await vi.importActual<typeof ApiModule>("@/features/observability/api");
  return {
    ...actual,
    createMetricQuery: apiMocks.createMetricQuery,
    updateMetricQuery: apiMocks.updateMetricQuery,
    listMetricQueries: apiMocks.listMetricQueries,
    listQuerySources: apiMocks.listQuerySources,
    testMetricQuery: apiMocks.testMetricQuery,
  };
});

const searchConnectionTargetsMock = vi.hoisted(() => vi.fn());
vi.mock("@/features/connections/api", () => ({
  searchConnectionTargets: searchConnectionTargetsMock,
}));

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({
  notify: notifyMock,
}));

function makeSource(): QuerySourceItem {
  return {
    id: "src-1",
    slug: "vm",
    name: "VictoriaMetrics",
    query_url: "http://victoriametrics:8428/api/v1/query",
    credential_ref: null,
    timeout_ms: 5000,
    source_type: "prometheus",
    enabled: true,
    options: {},
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

function makeMetric(): MetricQueryItem {
  return {
    id: "metric-1", name: "Availability", slug: "availability", query: "up",
    query_type: "promql", source_type: "prometheus", value_type: "auto", widget_type: "stat",
    default_show_in_inspector: true, default_show_in_list: true, enabled: true,
    sort_order: 0, options: {}, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  };
}

function renderPage(path = "/observability/metrics/new") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    client,
    ...render(
      <QueryClientProvider client={client}>
        <AllPermissions>
          <MemoryRouter initialEntries={[path]}>
            <Routes>
              <Route path="/observability" element={<h1>Metric query collection</h1>} />
              <Route path="/observability/metrics/new" element={<MetricQueryEditor />} />
              <Route path="/observability/metrics/:id/edit" element={<MetricQueryEditor />} />
            </Routes>
          </MemoryRouter>
        </AllPermissions>
      </QueryClientProvider>,
    ),
  };
}

describe("Metric query editor test button", () => {
  beforeEach(() => {
    apiMocks.createMetricQuery.mockReset();
    apiMocks.updateMetricQuery.mockReset();
    apiMocks.listMetricQueries.mockReset();
    apiMocks.listQuerySources.mockReset();
    apiMocks.testMetricQuery.mockReset();
    searchConnectionTargetsMock.mockReset();
    notifyMock.mockReset();

    apiMocks.listMetricQueries.mockResolvedValue({ items: [], total: 0 });
    apiMocks.listQuerySources.mockResolvedValue({
      items: [makeSource()],
      total: 1,
    });
    searchConnectionTargetsMock.mockResolvedValue([
      { id: "target-1", name: "host-a" },
    ]);
  });

  function expectFormOwner() {
    const owner = screen.getByTestId("entity-editor-page");
    expect(document.querySelectorAll('[data-slot="page-layout"], [data-slot="entity-editor-page"]')).toHaveLength(1);
    expect(owner).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  }

  it.each(["new", "edit"] as const)("uses the form owner for a loaded %s query without changing its draft", async mode => {
    apiMocks.listMetricQueries.mockResolvedValue({ items: mode === "edit" ? [makeMetric()] : [], total: mode === "edit" ? 1 : 0 });
    renderPage(mode === "edit" ? "/observability/metrics/metric-1/edit" : "/observability/metrics/new");
    const query = await screen.findByPlaceholderText(/up\{/);
    expectFormOwner();
    expect(query).toHaveValue(mode === "edit" ? "up" : "");
    expect(screen.getByRole("button", { name: mode === "edit" ? /^save$/i : /^create$/i })).toBeInTheDocument();
    expect(apiMocks.createMetricQuery).not.toHaveBeenCalled();
    expect(apiMocks.updateMetricQuery).not.toHaveBeenCalled();
  });

  it.each(["pending", "error", "missing"] as const)("keeps one form owner and safe exit while the query is %s", async state => {
    if (state === "pending") apiMocks.listMetricQueries.mockReturnValue(new Promise(() => {}));
    if (state === "error") apiMocks.listMetricQueries.mockRejectedValue(new Error("Unavailable"));
    renderPage("/observability/metrics/metric-1/edit");
    if (state === "pending") expect(await screen.findByRole("status")).toHaveTextContent(/loading/i);
    else await waitFor(() => expect(screen.queryByText(/loading/i)).not.toBeInTheDocument());
    expectFormOwner();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^(save|create)$/i })).not.toBeInTheDocument();
    if (state === "error") expect(screen.getByRole("button", { name: /^retry$/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
    expect(await screen.findByRole("heading", { name: "Metric query collection" })).toBeInTheDocument();
    expect(apiMocks.createMetricQuery).not.toHaveBeenCalled();
    expect(apiMocks.updateMetricQuery).not.toHaveBeenCalled();
  });

  it("runs a live test against the first available source and target", async () => {
    const response: MetricQueryTestResponse = {
      executed_query: 'up{piping_host_name="host-a"}',
      value: { status: "success", data: { resultType: "vector", result: [{ metric: {}, value: [1, "1"] }] } },
      source_resolved: true,
    };
    apiMocks.testMetricQuery.mockResolvedValue(response);

    renderPage();
    await waitFor(() => {
      expect(apiMocks.listQuerySources).toHaveBeenCalled();
    });

    const textarea = await screen.findByPlaceholderText(/up\{/);
    expect(textarea).toHaveAccessibleDescription(/Use \$host as the connection target placeholder/);
    expect(screen.getByRole("textbox", { name: /^Unit$/i })).toHaveAccessibleDescription(/Used only for display formatting/);
    textarea.focus();
    fireEvent.change(textarea, { target: { value: "up" } });
    expect(textarea).toHaveFocus();

    const testButton = await screen.findByRole("button", { name: /^test$/i });
    await waitFor(() => {
      expect(testButton).not.toBeDisabled();
    });
    fireEvent.click(testButton);

    await waitFor(() => {
      expect(apiMocks.testMetricQuery).toHaveBeenCalled();
    });
    const call = apiMocks.testMetricQuery.mock.calls[0];
    if (!call) throw new Error("testMetricQuery not called");
    const body = call[0] as {
      query: string;
      query_url: string;
      source_type: string;
      target_id: string | null;
    };
    expect(body.query).toBe("up");
    expect(body.query_url).toBe("http://victoriametrics:8428/api/v1/query");
    expect(body.source_type).toBe("prometheus");
    // No target picked → first searchConnectionTargets result is used.
    expect(body.target_id).toBe("target-1");

    expect(await screen.findByText(/test result/i)).toBeInTheDocument();
    expect(screen.getByText(/up\{piping_host_name="host-a"\}/)).toBeInTheDocument();
  });

  it("renders an upstream error in red without destroying the form", async () => {
    const errorResp: MetricQueryTestResponse = {
      executed_query: "up",
      value: null,
      error: "connection refused",
      source_resolved: true,
    };
    apiMocks.testMetricQuery.mockResolvedValue(errorResp);

    renderPage();
    await waitFor(() => {
      expect(apiMocks.listQuerySources).toHaveBeenCalled();
    });

    const textarea = await screen.findByPlaceholderText(/up\{/);
    fireEvent.change(textarea, { target: { value: "up" } });

    const testButton = await screen.findByRole("button", { name: /^test$/i });
    fireEvent.click(testButton);

    expect(await screen.findByText(/connection refused/)).toBeInTheDocument();
    // Form fields persist after error.
    expect((textarea as HTMLTextAreaElement).value).toBe("up");
  });

  it("disables the test button when no query source is configured", async () => {
    apiMocks.listQuerySources.mockResolvedValueOnce({ items: [], total: 0 });
    renderPage();
    await waitFor(() => {
      expect(apiMocks.listQuerySources).toHaveBeenCalled();
    });

    const textarea = await screen.findByPlaceholderText(/up\{/);
    fireEvent.change(textarea, { target: { value: "up" } });

    const testButton = await screen.findByRole("button", { name: /^test$/i });
    expect(testButton).toBeDisabled();
  });

  it("does not show a deferred preview after its query has changed", async () => {
    let resolve!: (value: MetricQueryTestResponse) => void;
    const pending = new Promise<MetricQueryTestResponse>((done) => { resolve = done; });
    apiMocks.testMetricQuery.mockReturnValue(pending);
    renderPage();
    const textarea = await screen.findByPlaceholderText(/up\{/);
    fireEvent.change(textarea, { target: { value: "old_query" } });
    const button = screen.getByRole("button", { name: /^test$/i });
    await waitFor(() => expect(button).not.toBeDisabled());
    fireEvent.click(button);
    await waitFor(() => expect(apiMocks.testMetricQuery).toHaveBeenCalledOnce());
    fireEvent.change(textarea, { target: { value: "new_query" } });
    await act(async () => {
      resolve({ executed_query: "old_query", value: null, error: "old preview error", source_resolved: true });
      await pending;
    });
    await waitFor(() => expect(button).not.toBeDisabled());
    expect(screen.queryByText("old preview error")).not.toBeInTheDocument();
    expect(textarea).toHaveValue("new_query");
  });
});
