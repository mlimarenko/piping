import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it } from "vitest";
import "@/shared/i18n";
import { MemoryRouter } from "react-router";
import { AllPermissions } from "@/test/permissions";
import { ExporterAssignmentForm } from "./ExporterAssignmentEditor";
import type { ScopeEntities } from "./useScopeEntities";

it("describes the exporter trigger and labels JSON validation on the actual textarea", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const entities: ScopeEntities = {
    optionsByScope: { role: [], connection_target: [], site: [], tenant: [] },
    resolveName: () => "Global", isPending: false,
  };
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter><ExporterAssignmentForm
    editing={null} exporters={[{
      id: "exporter-1", slug: "node", name: "Node exporter", enabled: true,
      exporter_type: "prometheus", default_job: "node", default_metrics_path: "/metrics",
      default_scheme: "http", default_target_source: "host", labels: {},
      created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
    }]} entities={entities}
  /></MemoryRouter></AllPermissions></QueryClientProvider>);
  const exporter = screen.getByRole("combobox", { name: /^Exporter/i });
  expect(exporter).toHaveAccessibleDescription(/default Prometheus job label/);
  const labels = screen.getByRole("textbox", { name: /Labels \(JSON object\)/i });
  expect(labels).not.toHaveAttribute("aria-describedby");
  fireEvent.change(labels, { target: { value: "[1]" } });
  fireEvent.keyDown(exporter, { key: "ArrowDown" });
  fireEvent.click(await screen.findByRole("option", { name: /Node exporter/i }));
  fireEvent.click(screen.getByRole("button", { name: /^Create$/i }));
  expect(labels).toHaveAccessibleDescription("Labels must be a JSON object.");
  expect(labels).toHaveAttribute("aria-invalid", "true");
  labels.focus();
  fireEvent.change(labels, { target: { value: '{ "env": "test" }' } });
  expect(labels).toHaveFocus();
  expect(labels).toHaveValue('{ "env": "test" }');
});
