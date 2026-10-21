import type { ReactNode } from "react";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@/shared/i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type * as ApiModule from "./api";
import type {
  ConnectionTargetObservabilityResponse,
  LatestMetricsResponse,
  MonitoringMetricDescriptor,
} from "./api";

const apiMock = vi.hoisted(() => ({
  getConnectionTargetObservability: vi.fn(),
  getLatestMetrics: vi.fn(),
  getMetricHistory: vi.fn(),
  listMetricQueryHostOverrides: vi.fn(),
  deleteMetricQueryHostOverride: vi.fn(),
  createMetricQueryHostOverride: vi.fn(),
  updateMetricQueryHostOverride: vi.fn(),
}));

vi.mock("@/features/observability/api", async () => {
  const actual = await vi.importActual<typeof ApiModule>("@/features/observability/api");
  return {
    ...actual,
    getConnectionTargetObservability: apiMock.getConnectionTargetObservability,
    getLatestMetrics: apiMock.getLatestMetrics,
    getMetricHistory: apiMock.getMetricHistory,
    listMetricQueryHostOverrides: apiMock.listMetricQueryHostOverrides,
    deleteMetricQueryHostOverride: apiMock.deleteMetricQueryHostOverride,
    createMetricQueryHostOverride: apiMock.createMetricQueryHostOverride,
    updateMetricQueryHostOverride: apiMock.updateMetricQueryHostOverride,
  };
});

import HostMetricsPreviewSection from "./HostMetricsPreviewSection";

const HOST = { id: "11111111-1111-1111-1111-111111111111", name: "edge-01" };

function descriptor(over: Partial<MonitoringMetricDescriptor>): MonitoringMetricDescriptor {
  return {
    name: over.name ?? "metric",
    label: over.label ?? "Metric",
    query: over.query ?? "metric_q",
    unit: null,
    value_type: "number",
    widget_type: "stat",
    source_type: "prometheus",
    query_type: "promql",
    options: {},
    show_in_list: false,
    list_render: "auto",
    ...over,
  };
}

function obsResponse(metrics: MonitoringMetricDescriptor[]): ConnectionTargetObservabilityResponse {
  return {
    groups: [],
    targets_count: 1,
    target: {
      host_address: "10.0.0.1",
      host_id: HOST.id,
      host_name: HOST.name,
      kind: "device",
      metrics,
      inspector_metrics: metrics,
      metric_sources: [{ source_type: "prometheus", source_id: null, weight: 0 } as never],
      scrapes: [],
    },
  };
}

function latestResponse(metrics: MonitoringMetricDescriptor[]): LatestMetricsResponse {
  return {
    configured: true,
    items: metrics.map((m) => ({
      query: m.query,
      executed_query: m.query,
      name: m.name,
      label: m.label ?? m.name,
      unit: m.unit ?? null,
      value_type: m.value_type ?? null,
      widget_type: m.widget_type ?? null,
      source_type: m.source_type ?? "prometheus",
      status: "success",
      value: {
        status: "success",
        data: { resultType: "vector", result: [{ metric: {}, value: [1, "1"] }] },
      },
      error: null,
    })),
  };
}

function Wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}><AllPermissions>{children}</AllPermissions></QueryClientProvider>;
}

describe("HostMetricsPreviewSection category groups", () => {
  beforeEach(() => {
    apiMock.getConnectionTargetObservability.mockReset();
    apiMock.getLatestMetrics.mockReset();
    apiMock.getMetricHistory.mockReset();
    apiMock.getMetricHistory.mockResolvedValue({ configured: true, query: "", series: [] });
    apiMock.listMetricQueryHostOverrides.mockReset();
    apiMock.deleteMetricQueryHostOverride.mockReset();
    apiMock.createMetricQueryHostOverride.mockReset();
    apiMock.updateMetricQueryHostOverride.mockReset();
    window.localStorage.clear();
  });

  afterEach(() => {
    window.localStorage.clear();
  });

  it("lets a delete-only operator reach Revert without offering Save", async () => {
    const metric = descriptor({ name: "cpu", label: "CPU", query: "cpu_q", metric_query_id: "metric-query" });
    apiMock.getConnectionTargetObservability.mockResolvedValue(obsResponse([metric]));
    apiMock.getLatestMetrics.mockResolvedValue(latestResponse([metric]));
    apiMock.listMetricQueryHostOverrides.mockResolvedValue({ items: [{
      id: "existing-override", host_id: HOST.id, metric_query_id: "metric-query",
      label_override: "Host CPU", unit_override: null, options_override: {},
      created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
    }] });
    apiMock.deleteMetricQueryHostOverride.mockResolvedValue(undefined);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}>
      <PermissionsContext.Provider value={{ roles: [], isAdmin: false,
        can: action => action === "READ" || action === "DELETE", canAny: () => false, hasRole: () => false }}>
        <HostMetricsPreviewSection host={HOST} kind="device" />
      </PermissionsContext.Provider>
    </QueryClientProvider>);
    await screen.findByText("CPU");
    fireEvent.click(await screen.findByRole("button", { name: /Override metric for this host/i }, { timeout: 1000 }));
    const revert = await screen.findByRole("button", { name: /revert/i }, { timeout: 1000 });
    expect(revert).toBeEnabled();
    expect(screen.queryByRole("button", { name: /^Save$/ })).not.toBeInTheDocument();
    fireEvent.click(revert);
    fireEvent.click(screen.getByRole("button", { name: /click again to confirm/i }));
    await waitFor(() => expect(apiMock.deleteMetricQueryHostOverride).toHaveBeenCalledTimes(1));
    expect(apiMock.createMetricQueryHostOverride).not.toHaveBeenCalled();
    expect(apiMock.updateMetricQueryHostOverride).not.toHaveBeenCalled();
  });

  it("groups metrics by category, persists fold state in localStorage by kind, and restores it on remount", async () => {
    const metrics = [
      descriptor({ name: "cpu", label: "CPU", query: "cpu_q", category: "Compute" }),
      descriptor({ name: "mem", label: "Memory", query: "mem_q", category: "Compute" }),
      descriptor({ name: "net", label: "Network", query: "net_q", category: "Network" }),
    ];
    apiMock.getConnectionTargetObservability.mockResolvedValue(obsResponse(metrics));
    apiMock.getLatestMetrics.mockResolvedValue(latestResponse(metrics));

    const { unmount } = render(
      <Wrapper>
        <HostMetricsPreviewSection host={HOST} kind="device" />
      </Wrapper>,
    );

    // Both groups + their metrics should be visible by default.
    expect(await screen.findByRole("button", { name: /Compute/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Network/ })).toBeInTheDocument();
    expect(screen.getByText("CPU")).toBeInTheDocument();
    expect(screen.getByText("Memory")).toBeInTheDocument();
    // "Network" is both a group title and a metric label here — disambiguate
    // with a query that returns the metric row regardless of the group.
    expect(screen.getAllByText("Network").length).toBeGreaterThanOrEqual(2);

    // Collapse the Compute group. Its two metric rows must hide; the Network
    // group stays expanded.
    const computeHeader = screen.getByRole("button", { name: /Compute/ });
    expect(computeHeader).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(computeHeader);
    expect(computeHeader).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("CPU")).not.toBeInTheDocument();
    expect(screen.queryByText("Memory")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Network/ })).toBeInTheDocument();

    // The collapsed state lands in localStorage keyed by the host kind, so a
    // device inspector and a VM inspector keep separate preferences.
    const stored = JSON.parse(window.localStorage.getItem("obs:inspector:groups:device") ?? "{}");
    expect(stored).toEqual({ Compute: true });

    // Re-mount: the persisted fold state is restored.
    unmount();
    render(
      <Wrapper>
        <HostMetricsPreviewSection host={HOST} kind="device" />
      </Wrapper>,
    );
    expect(await screen.findByRole("button", { name: /Compute/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Compute/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(screen.queryByText("CPU")).not.toBeInTheDocument();

    // A VM inspector for the same host honours its own bucket (untouched →
    // expanded by default). The previous render is still mounted; the new
    // VM tree must produce its own Compute button that is expanded even
    // though the device bucket recorded a collapse.
    render(
      <Wrapper>
        <HostMetricsPreviewSection
          host={{ id: "22222222-2222-2222-2222-222222222222", name: "vm-1" }}
          kind="vm"
        />
      </Wrapper>,
    );
    await waitFor(() =>
      expect(screen.getAllByRole("button", { name: /Compute/ }).length).toBe(2),
    );
    const [, vmComputeHeader] = screen.getAllByRole("button", { name: /Compute/ });
    expect(vmComputeHeader).toHaveAttribute("aria-expanded", "true");
  });

  it("renders the flat list (no group shell) when there is only one category", async () => {
    const metrics = [
      descriptor({ name: "cpu", label: "CPU", query: "cpu_q", category: "Compute" }),
      descriptor({ name: "mem", label: "Memory", query: "mem_q", category: "Compute" }),
    ];
    apiMock.getConnectionTargetObservability.mockResolvedValue(obsResponse(metrics));
    apiMock.getLatestMetrics.mockResolvedValue(latestResponse(metrics));

    render(
      <Wrapper>
        <HostMetricsPreviewSection host={HOST} kind="device" />
      </Wrapper>,
    );

    // Metric labels render at the row level, no group header / collapse button.
    expect(await screen.findByText("CPU")).toBeInTheDocument();
    expect(screen.getByText("Memory")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Compute/ })).not.toBeInTheDocument();
  });

  it("re-syncs collapse state when the host kind changes in place (no remount)", async () => {
    const metrics = [
      descriptor({ name: "cpu", label: "CPU", query: "cpu_q", category: "Compute" }),
      descriptor({ name: "net", label: "Network", query: "net_q", category: "Network" }),
    ];
    apiMock.getConnectionTargetObservability.mockResolvedValue(obsResponse(metrics));
    apiMock.getLatestMetrics.mockResolvedValue(latestResponse(metrics));
    // Seed the VM bucket with a pre-existing collapse so we can prove the
    // switch reads from the new key, not the device key.
    window.localStorage.setItem(
      "obs:inspector:groups:vm",
      JSON.stringify({ Compute: true }),
    );

    const { rerender } = render(
      <Wrapper>
        <HostMetricsPreviewSection host={HOST} kind="device" />
      </Wrapper>,
    );

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Compute/ })).toHaveAttribute(
        "aria-expanded",
        "true",
      ),
    );

    // Same instance switches from a device to a VM (e.g. selection moved).
    // The collapse state must come from the VM bucket, not the device bucket.
    rerender(
      <Wrapper>
        <HostMetricsPreviewSection host={HOST} kind="vm" />
      </Wrapper>,
    );

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Compute/ })).toHaveAttribute(
        "aria-expanded",
        "false",
      ),
    );
  });

  it("renders the flat list when every metric is uncategorized", async () => {
    const metrics = [
      descriptor({ name: "cpu", label: "CPU", query: "cpu_q" }),
      descriptor({ name: "mem", label: "Memory", query: "mem_q", category: null }),
    ];
    apiMock.getConnectionTargetObservability.mockResolvedValue(obsResponse(metrics));
    apiMock.getLatestMetrics.mockResolvedValue(latestResponse(metrics));

    render(
      <Wrapper>
        <HostMetricsPreviewSection host={HOST} kind="device" />
      </Wrapper>,
    );

    expect(await screen.findByText("CPU")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Other/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Прочее/ })).not.toBeInTheDocument();
  });
});
