import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ExporterPresetApplyDialog from "./ExporterPresetApplyDialog";
import type * as ApiModule from "./api";
import type { ExporterPresetCatalogItem, QuerySourceItem } from "./api";
import type { ScopeEntities } from "./useScopeEntities";

const apiMocks = vi.hoisted(() => ({
  listMonitoringPresets: vi.fn(),
  applyExporterPreset: vi.fn(),
}));

vi.mock("@/features/observability/api", async () => {
  const actual = await vi.importActual<typeof ApiModule>("@/features/observability/api");
  return {
    ...actual,
    listMonitoringPresets: apiMocks.listMonitoringPresets,
    applyExporterPreset: apiMocks.applyExporterPreset,
  };
});

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({
  notify: notifyMock,
}));

function makePreset(
  over: Partial<ExporterPresetCatalogItem> = {},
): ExporterPresetCatalogItem {
  return {
    exporter_slug: "node",
    set_slug: "preset-node-exporter",
    name: "Node exporter",
    description: "Linux node exporter",
    query_count: 4,
    ...over,
  };
}

function makeSource(over: Partial<QuerySourceItem> = {}): QuerySourceItem {
  return {
    id: "22222222-2222-2222-2222-222222222222",
    slug: "vm",
    name: "VictoriaMetrics",
    query_url: "http://victoriametrics:8428/api/v1/query",
    credential_ref: null,
    timeout_ms: 5000,
    source_type: "victoriametrics",
    enabled: true,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  } as QuerySourceItem;
}

const stubEntities: ScopeEntities = {
  optionsByScope: { site: [], tenant: [], role: [], connection_target: [] },
  resolveName: () => "—",
  isPending: false,
};

function renderDialog(props: Partial<React.ComponentProps<typeof ExporterPresetApplyDialog>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = props.onOpenChange ?? vi.fn();
  return {
    onOpenChange,
    client,
    ...render(
      <QueryClientProvider client={client}>
        <ExporterPresetApplyDialog
          open
          onOpenChange={onOpenChange}
          sources={props.sources ?? [makeSource()]}
          entities={props.entities ?? stubEntities}
        />
      </QueryClientProvider>,
    ),
  };
}

describe("ExporterPresetApplyDialog", () => {
  beforeEach(() => {
    apiMocks.listMonitoringPresets.mockReset();
    apiMocks.applyExporterPreset.mockReset();
    notifyMock.mockReset();
  });

  it("describes the empty preset catalog on the actual picker trigger", async () => {
    apiMocks.listMonitoringPresets.mockResolvedValue({ items: [] });
    renderDialog();
    const help = await screen.findByText(/No exporter catalog entries available/i);
    const picker = screen.getByRole("combobox", { name: /^Exporter preset/i });
    expect(picker).toHaveAccessibleDescription(help.textContent);
    picker.focus();
    expect(picker).toHaveFocus();
    expect(picker).toHaveAttribute("id", "apply-preset-exporter");
  });

  it("describes the source picker while retaining selection and focus", async () => {
    apiMocks.listMonitoringPresets.mockResolvedValue({ items: [makePreset()] });
    renderDialog();
    const picker = screen.getByRole("combobox", { name: /^Data source/i });
    const help = picker.closest('[data-slot="form-field"]')!.querySelector("p")!;
    expect(picker).toHaveAccessibleDescription(help.textContent);
    const descriptionId = picker.getAttribute("aria-describedby");
    picker.focus();
    expect(picker).toHaveFocus();
    fireEvent.click(picker);
    fireEvent.click(await screen.findByText(/VictoriaMetrics/i));
    expect(picker).toHaveTextContent("VictoriaMetrics");
    expect(picker).toHaveAttribute("aria-describedby", descriptionId);
    picker.focus();
    expect(picker).toHaveFocus();
  });

  it("renders the preset catalog when opened and disables the data-source picker when no sources are configured", async () => {
    apiMocks.listMonitoringPresets.mockResolvedValue({ items: [makePreset()] });

    renderDialog({ sources: [] });

    // Hint about missing sources is rendered as help text below the picker.
    expect(
      await screen.findByText(/Add a data source first/i),
    ).toBeInTheDocument();
    const picker = screen.getByRole("combobox", { name: /^Data source/i });
    expect(picker).toBeDisabled();
    expect(picker).toHaveAccessibleDescription(/Add a data source first/i);
  });

  it("calls applyExporterPreset with the selected exporter slug, source, and scope", async () => {
    apiMocks.listMonitoringPresets.mockResolvedValue({ items: [makePreset()] });
    apiMocks.applyExporterPreset.mockResolvedValue({
      exporter_id: "33333333-3333-3333-3333-333333333333",
      query_source_id: "22222222-2222-2222-2222-222222222222",
      query_source_assignment_id: "66666666-6666-6666-6666-666666666666",
      metric_set_id: "44444444-4444-4444-4444-444444444444",
      metric_set_assignment_id: "55555555-5555-5555-5555-555555555555",
      metric_query_ids: [],
    });

    const { onOpenChange } = renderDialog();

    await waitFor(() => expect(apiMocks.listMonitoringPresets).toHaveBeenCalled());

    // First two comboboxes are the SearchableSelects (exporter + data source);
    // remaining ones (e.g. the scope FormSelect) belong to ScopeFields.
    const comboboxes = await screen.findAllByRole("combobox");
    expect(comboboxes.length).toBeGreaterThanOrEqual(2);

    // Pick the exporter preset.
    fireEvent.click(comboboxes[0]!);
    fireEvent.click(await screen.findByText(/Node exporter/i));

    // Pick the source.
    fireEvent.click(comboboxes[1]!);
    fireEvent.click(await screen.findByText(/VictoriaMetrics/i));

    // Default scope is "global"; click the Apply button (the primary, lower one).
    const applyButtons = screen.getAllByRole("button", { name: /Apply preset/i });
    fireEvent.click(applyButtons[applyButtons.length - 1]!);

    await waitFor(() => expect(apiMocks.applyExporterPreset).toHaveBeenCalledTimes(1));
    expect(apiMocks.applyExporterPreset).toHaveBeenCalledWith(
      "node",
      expect.objectContaining({
        query_source_id: "22222222-2222-2222-2222-222222222222",
        scope_type: "global",
        scope_id: null,
      }),
    );
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
});
