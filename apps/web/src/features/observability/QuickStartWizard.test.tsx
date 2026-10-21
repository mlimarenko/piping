import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import QuickStartWizard from "./QuickStartWizard";
import { AllPermissions } from "@/test/permissions";
import type * as ApiModule from "./api";

const apiMocks = vi.hoisted(() => ({
  listMonitoringPresets: vi.fn(),
  monitoringQuickStart: vi.fn(),
}));

vi.mock("@/features/observability/api", async () => {
  const actual = await vi.importActual<typeof ApiModule>("@/features/observability/api");
  return {
    ...actual,
    listMonitoringPresets: apiMocks.listMonitoringPresets,
    monitoringQuickStart: apiMocks.monitoringQuickStart,
  };
});

vi.mock("@/features/inventory/api", () => ({
  listAllResources: vi.fn(() => Promise.resolve([])),
}));

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({
  notify: notifyMock,
}));

function renderWizard(props: Partial<React.ComponentProps<typeof QuickStartWizard>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = props.onOpenChange ?? vi.fn();
  const onCompleted = props.onCompleted ?? vi.fn();
  return {
    onOpenChange,
    onCompleted,
    ...render(
      <QueryClientProvider client={client}>
        <AllPermissions><QuickStartWizard open onOpenChange={onOpenChange} onCompleted={onCompleted} /></AllPermissions>
      </QueryClientProvider>,
    ),
  };
}

describe("QuickStartWizard", () => {
  beforeEach(() => {
    apiMocks.listMonitoringPresets.mockReset();
    apiMocks.listMonitoringPresets.mockResolvedValue({ items: [] });
    apiMocks.monitoringQuickStart.mockReset();
    notifyMock.mockReset();
  });

  it("describes the actual preset picker and preserves its help while choosing a preset", async () => {
    apiMocks.listMonitoringPresets.mockResolvedValue({ items: [{
      exporter_slug: "node", set_slug: "preset-node-exporter", name: "Node exporter",
      description: "Linux node exporter", query_count: 4,
    }] });
    renderWizard();
    fireEvent.change(screen.getByRole("textbox", { name: /^Query URL/i }), {
      target: { value: "http://vm.example/api/v1/query" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Next/i }));
    const picker = await screen.findByRole("combobox", { name: /^Exporter preset/i });
    await waitFor(() => expect(apiMocks.listMonitoringPresets).toHaveBeenCalled());
    const help = picker.closest('[data-slot="form-field"]')!.querySelector("p")!;
    expect(picker).toHaveAccessibleDescription(help.textContent);
    const descriptionId = picker.getAttribute("aria-describedby");
    picker.focus();
    expect(picker).toHaveFocus();
    fireEvent.click(picker);
    fireEvent.click(await screen.findByText(/Node exporter/i));
    expect(picker).toHaveTextContent("Node exporter");
    expect(picker).toHaveAttribute("aria-describedby", descriptionId);
    picker.focus();
    expect(picker).toHaveFocus();
  });

  it("requires a query URL on step 1 before letting the user advance", async () => {
    renderWizard();

    expect(await screen.findByText(/Quick start/i)).toBeInTheDocument();

    const nextButton = screen.getByRole("button", { name: /Next/i });
    expect(nextButton).toBeDisabled();
    expect(screen.getByRole("textbox", { name: /^Query URL/i })).toHaveAccessibleDescription(/Use a Prometheus-compatible/);
    expect(screen.getByRole("textbox", { name: /Bearer-token named secret ID/i })).toHaveAccessibleDescription(/UUID of an existing named secret/);

    fireEvent.change(screen.getByPlaceholderText(/victoriametrics/i), {
      target: { value: "http://vm.example/api/v1/query" },
    });

    await waitFor(() => expect(nextButton).not.toBeDisabled());
  });

  it("submits the quick-start request with the wizard fields and notifies completion", async () => {
    apiMocks.monitoringQuickStart.mockResolvedValue({
      query_source_id: "11111111-1111-1111-1111-111111111111",
      query_source_assignment_id: "22222222-2222-2222-2222-222222222222",
      applied_preset: null,
    });

    const { onCompleted, onOpenChange } = renderWizard();

    // Step 1 — fill URL and advance.
    fireEvent.change(screen.getByPlaceholderText(/victoriametrics/i), {
      target: { value: "http://vm.example/api/v1/query" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Next/i }));

    // Step 2 — skip preset and advance. The default selection is "skip".
    fireEvent.click(await screen.findByRole("button", { name: /Next/i }));

    // Step 3 — default scope is "global". Click Finish.
    fireEvent.click(await screen.findByRole("button", { name: /Finish/i }));

    await waitFor(() => expect(apiMocks.monitoringQuickStart).toHaveBeenCalledTimes(1));
    expect(apiMocks.monitoringQuickStart).toHaveBeenCalledWith(
      expect.objectContaining({
        query_url: "http://vm.example/api/v1/query",
        source_type: "prometheus",
        preset_slug: null,
        scope_type: "global",
        scope_id: null,
      }),
    );
    await waitFor(() => expect(onCompleted).toHaveBeenCalledTimes(1));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("locks scope, navigation and dismissal while pending, retaining the draft after failure", async () => {
    let reject!: (error: Error) => void;
    const pending = new Promise((_, fail) => { reject = fail; });
    apiMocks.monitoringQuickStart.mockReturnValueOnce(pending);
    const { onOpenChange } = renderWizard();
    fireEvent.change(screen.getByPlaceholderText(/victoriametrics/i), { target: { value: "http://vm.example/api/v1/query" } });
    fireEvent.click(screen.getByRole("button", { name: /Next/i }));
    fireEvent.click(await screen.findByRole("button", { name: /Next/i }));
    fireEvent.click(await screen.findByRole("button", { name: /Finish/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /Finish/i })).toBeDisabled());
    expect(screen.getByRole("combobox")).toBeDisabled();
    expect(screen.getByRole("button", { name: /^Back$/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /^Cancel$/i })).toBeDisabled();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onOpenChange).not.toHaveBeenCalled();
    await act(async () => {
      reject(new Error("save failed"));
      await pending.catch(() => undefined); // The rejected API fixture is handled by the mutation's error path.
    });
    await waitFor(() => expect(screen.getByRole("button", { name: /Finish/i })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^Back$/i }));
    fireEvent.click(screen.getByRole("button", { name: /^Back$/i }));
    expect(screen.getByPlaceholderText(/victoriametrics/i)).toHaveValue("http://vm.example/api/v1/query");
  });

  it("confirms cancellation of a changed draft before dismissing", async () => {
    const { onOpenChange } = renderWizard();
    fireEvent.change(screen.getByPlaceholderText(/victoriametrics/i), { target: { value: "http://vm.example/api/v1/query" } });
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/i }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^Discard$/i }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("does not complete an authoring session after its owner has unmounted", async () => {
    let resolve!: (response: unknown) => void;
    const pending = new Promise((done) => { resolve = done; });
    apiMocks.monitoringQuickStart.mockReturnValueOnce(pending);
    const { onCompleted, onOpenChange, unmount } = renderWizard();
    fireEvent.change(screen.getByPlaceholderText(/victoriametrics/i), { target: { value: "http://vm.example/api/v1/query" } });
    fireEvent.click(screen.getByRole("button", { name: /Next/i }));
    fireEvent.click(await screen.findByRole("button", { name: /Next/i }));
    fireEvent.click(await screen.findByRole("button", { name: /Finish/i }));
    await waitFor(() => expect(apiMocks.monitoringQuickStart).toHaveBeenCalledOnce());
    unmount();
    await act(async () => {
      resolve({ query_source_id: "source-1", query_source_assignment_id: "assignment-1", applied_preset: null });
      await pending;
    });
    expect(onCompleted).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalled();
  });
});
