import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import HostMetricOverrideDrawer from "./HostMetricOverrideDrawer";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import type * as ApiModule from "./api";
import type {
  MetricQueryHostOverrideItem,
  MonitoringMetricDescriptor,
} from "./api";

const apiMocks = vi.hoisted(() => ({
  listMetricQueryHostOverrides: vi.fn(),
  createMetricQueryHostOverride: vi.fn(),
  updateMetricQueryHostOverride: vi.fn(),
  deleteMetricQueryHostOverride: vi.fn(),
}));

vi.mock("@/features/observability/api", async () => {
  const actual = await vi.importActual<typeof ApiModule>("@/features/observability/api");
  return {
    ...actual,
    listMetricQueryHostOverrides: apiMocks.listMetricQueryHostOverrides,
    createMetricQueryHostOverride: apiMocks.createMetricQueryHostOverride,
    updateMetricQueryHostOverride: apiMocks.updateMetricQueryHostOverride,
    deleteMetricQueryHostOverride: apiMocks.deleteMetricQueryHostOverride,
  };
});

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({
  notify: notifyMock,
}));

const HOST = { id: "11111111-1111-1111-1111-111111111111", name: "edge-01" };
const METRIC_QUERY_ID = "22222222-2222-2222-2222-222222222222";

const baseMetric: MonitoringMetricDescriptor = {
  name: "cpu",
  label: "CPU",
  query: "node_cpu_seconds_total",
  unit: "%",
  value_type: "percent",
  widget_type: "stat",
  source_type: "prometheus",
  query_type: "promql",
  options: {},
  list_render: "auto",
  show_in_list: true,
};

function existingOverride(
  options_override: unknown,
): MetricQueryHostOverrideItem {
  return {
    id: "33333333-3333-3333-3333-333333333333",
    host_id: HOST.id,
    metric_query_id: METRIC_QUERY_ID,
    label_override: "CPU on host",
    unit_override: "%",
    options_override,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

function renderDrawer(overrides: MetricQueryHostOverrideItem[] = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = vi.fn();
  apiMocks.listMetricQueryHostOverrides.mockResolvedValue({
    items: overrides,
  });
  return {
    onOpenChange,
    client,
    ...render(
      <QueryClientProvider client={client}>
        <AllPermissions>
        <HostMetricOverrideDrawer
          open
          onOpenChange={onOpenChange}
          host={HOST}
          metric={baseMetric}
          metricQueryId={METRIC_QUERY_ID}
        />
        </AllPermissions>
      </QueryClientProvider>,
    ),
  };
}

describe("HostMetricOverrideDrawer", () => {
  it("describes the actual override inputs without moving focus or losing edits", async () => {
    renderDrawer();
    for (const name of [/^Label override/i, /^Unit override/i]) {
      const input = await screen.findByRole("textbox", { name });
      const help = input.closest('[data-slot="form-field"]')!.querySelector("p")!;
      expect(input).toHaveAccessibleDescription(help.textContent);
      const descriptionId = input.getAttribute("aria-describedby");
      input.focus();
      fireEvent.change(input, { target: { value: "Custom override" } });
      expect(input).toHaveValue("Custom override");
      expect(input).toHaveFocus();
      expect(input).toHaveAttribute("aria-describedby", descriptionId);
    }
  });
  beforeEach(() => {
    apiMocks.listMetricQueryHostOverrides.mockReset();
    apiMocks.createMetricQueryHostOverride.mockReset();
    apiMocks.updateMetricQueryHostOverride.mockReset();
    apiMocks.deleteMetricQueryHostOverride.mockReset();
    notifyMock.mockReset();
  });

  it("isolates drafts and late save completion when the edited metric changes", async () => {
    let resolve!: (value: unknown) => void;
    const pending = new Promise((done) => { resolve = done; });
    apiMocks.createMetricQueryHostOverride.mockReturnValueOnce(pending);
    const { client, rerender, onOpenChange } = renderDrawer();
    const label = (await screen.findAllByRole("textbox"))[0]!;
    fireEvent.change(label, { target: { value: "First metric draft" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(apiMocks.createMetricQueryHostOverride).toHaveBeenCalledOnce());
    rerender(
      <QueryClientProvider client={client}>
        <AllPermissions>
        <HostMetricOverrideDrawer open onOpenChange={onOpenChange} host={HOST} metric={baseMetric} metricQueryId="second-metric" />
        </AllPermissions>
      </QueryClientProvider>,
    );
    expect((await screen.findAllByRole("textbox"))[0]).toHaveValue("");
    await act(async () => { resolve({ id: "saved-first-metric" }); await pending; });
    expect(onOpenChange).not.toHaveBeenCalled();
  });

  it("locks every draft control and dismissal while saving and retains edits on failure", async () => {
    let reject!: (error: Error) => void;
    const pending = new Promise((_, fail) => { reject = fail; });
    apiMocks.createMetricQueryHostOverride.mockReturnValueOnce(pending);
    const { onOpenChange } = renderDrawer();
    const label = await screen.findByRole("textbox", { name: /^Label override/i });
    fireEvent.change(label, { target: { value: "Unsaved override" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(label).toBeDisabled());
    for (const role of ["textbox", "spinbutton", "combobox"] as const) {
      for (const control of screen.queryAllByRole(role)) expect(control).toBeDisabled();
    }
    fireEvent.change(label, { target: { value: "Rejected edit" } });
    expect(label).toHaveValue("Unsaved override");
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onOpenChange).not.toHaveBeenCalled();
    await act(async () => {
      reject(new Error("save failed"));
      await pending.catch(() => undefined); // The rejected API fixture is handled by the mutation's error path.
    });
    await waitFor(() => expect(label).not.toBeDisabled());
    expect(label).toHaveValue("Unsaved override");
  });

  it("uses discard confirmation for a changed override", async () => {
    const { onOpenChange } = renderDrawer();
    const label = await screen.findByRole("textbox", { name: /^Label override/i });
    fireEvent.change(label, { target: { value: "Unsaved override" } });
    fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^discard$/i }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("does not offer save or revert to a read-only monitoring viewer", async () => {
    apiMocks.listMetricQueryHostOverrides.mockResolvedValue({ items: [existingOverride(null)] });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: (action) => action === "READ", canAny: () => false, hasRole: () => false }}>
          <HostMetricOverrideDrawer open onOpenChange={vi.fn()} host={HOST} metric={baseMetric} metricQueryId={METRIC_QUERY_ID} />
        </PermissionsContext.Provider>
      </QueryClientProvider>,
    );
    await screen.findByDisplayValue("CPU on host");
    expect(screen.queryByRole("button", { name: /^Save$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /revert/i })).not.toBeInTheDocument();
  });

  it("creates a new override with options_override as the display subtree", async () => {
    apiMocks.createMetricQueryHostOverride.mockResolvedValue({
      id: "new-id",
      host_id: HOST.id,
      metric_query_id: METRIC_QUERY_ID,
      enabled: true,
      label_override: "Edge CPU",
      unit_override: null,
      options_override: null,
      created_at: "",
      updated_at: "",
    });
    const { onOpenChange } = renderDrawer([]);

    await waitFor(() => {
      expect(apiMocks.listMetricQueryHostOverrides).toHaveBeenCalledWith(HOST.id);
    });

    // Wait for the body to swap in once the list resolves.
    const labelInput = await waitFor<HTMLElement>(() => {
      const [first] = screen.getAllByRole("textbox");
      if (!first) throw new Error("inputs not rendered yet");
      return first;
    });
    fireEvent.change(labelInput, { target: { value: "Edge CPU" } });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(apiMocks.createMetricQueryHostOverride).toHaveBeenCalled();
    });
    const call = apiMocks.createMetricQueryHostOverride.mock.calls[0];
    if (!call) throw new Error("create override not called");
    const body = call[0] as {
      host_id: string;
      metric_query_id: string;
      label_override: string | null;
      options_override: unknown;
    };
    expect(body.host_id).toBe(HOST.id);
    expect(body.metric_query_id).toBe(METRIC_QUERY_ID);
    expect(body.label_override).toBe("Edge CPU");
    // Label-only edit: the threshold was untouched, so options_override is
    // null and the descriptor's own display rules are preserved. Sending
    // the seeded threshold here would clobber sibling `display.*` keys
    // (apply_host_overrides replaces options.display wholesale).
    expect(body.options_override).toBeNull();
    await waitFor(() => {
      expect(onOpenChange).toHaveBeenCalledWith(false);
    });
  });

  it("seeds the form from an existing override and updates it", async () => {
    apiMocks.updateMetricQueryHostOverride.mockResolvedValue(
      existingOverride({ status: { mode: "auto" } }),
    );
    renderDrawer([
      existingOverride({
        status: { mode: "numeric", direction: "higher_is_worse", warning: 50, critical: 60 },
      }),
    ]);

    expect(await screen.findByDisplayValue("CPU on host")).toBeInTheDocument();
    // Warning input pre-populated from existing override.
    expect(screen.getByDisplayValue("50")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(apiMocks.updateMetricQueryHostOverride).toHaveBeenCalled();
    });
    const updateCall = apiMocks.updateMetricQueryHostOverride.mock.calls[0];
    if (!updateCall) throw new Error("update override not called");
    const [overrideId, body] = updateCall as [string, { options_override: unknown }];
    expect(overrideId).toBe("33333333-3333-3333-3333-333333333333");
    expect(body.options_override).toMatchObject({
      status: { mode: "numeric", direction: "higher_is_worse" },
    });
  });

  it("offers a revert button only when an override exists", async () => {
    apiMocks.deleteMetricQueryHostOverride.mockResolvedValue(undefined);
    renderDrawer([existingOverride(null)]);

    // Two-stage confirm: first click arms the destructive action, second
    // click commits. Operators should never lose an override on one click.
    // Same button element gets reused for the confirm — find by initial
    // label, click twice, assert the click guard fires between them.
    const revertBtn = await screen.findByRole("button", { name: /revert/i });
    fireEvent.click(revertBtn);
    expect(apiMocks.deleteMetricQueryHostOverride).not.toHaveBeenCalled();

    fireEvent.click(revertBtn);
    await waitFor(() => {
      expect(apiMocks.deleteMetricQueryHostOverride).toHaveBeenCalledWith(
        "33333333-3333-3333-3333-333333333333",
      );
    });
  });

  it("does not show revert when no override exists yet", async () => {
    renderDrawer([]);
    await waitFor(() => {
      expect(apiMocks.listMetricQueryHostOverrides).toHaveBeenCalled();
    });
    expect(screen.queryByRole("button", { name: /revert/i })).toBeNull();
  });
});
