import { fireEvent, render as renderUi, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import type { ReactElement } from "react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { AssignmentActions } from "./AssignmentsPanel";
import { TargetSetsPanel } from "./TargetSetsPanel";
import { SchedulesPanel } from "./SchedulesPanel";
import { ServicePrincipalsPanel } from "./ServicePrincipalsPanel";
import { CatalogPanel } from "./CatalogPanel";
import type { PlaybookAssignment, PlaybookCatalogItem, PlaybookSchedule, PlaybookServicePrincipal, PlaybookTargetSet } from "./api";

const tableProps = {
  loading: false, error: null, onRetry: vi.fn(), sortState: null,
  onSortChange: vi.fn(), columnFilters: {}, onColumnFilterChange: vi.fn(),
};
const targetSet: PlaybookTargetSet = {
  id: "targets-1", name: "SSH targets", mode: "dynamic", entity_kind: "connection_target",
  selector: {}, created_at: "2026-06-06T00:00:00Z", updated_at: "2026-06-06T00:00:00Z",
};
const assignment: PlaybookAssignment = {
  id: "assignment-1", name: "Deploy", playbook_id: "playbook-1", target_set_id: targetSet.id,
  enabled: true, executable: true, visible: true, quick_action_enabled: true, scope_kind: "global",
  priority: 1, default_input: {}, default_metadata: {}, metadata: {},
  created_at: targetSet.created_at, updated_at: targetSet.updated_at,
};
const schedule: PlaybookSchedule = {
  id: "schedule-1", name: "Nightly", playbook_id: "playbook-1", target_set_id: targetSet.id,
  service_principal_id: "principal-1", cron_expression: "0 3 * * *", timezone: "UTC",
  enabled: true, concurrency_policy: "skip_if_running", misfire_policy: "fire_once", metadata: {},
  created_at: targetSet.created_at, updated_at: targetSet.updated_at,
};
const principal: PlaybookServicePrincipal = {
  id: "principal-1", name: "Runner", slug: "runner", status: "active", metadata: {},
  created_at: targetSet.created_at, updated_at: targetSet.updated_at,
};

function render(ui: ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderUi(<MemoryRouter><QueryClientProvider client={client}>{ui}</QueryClientProvider></MemoryRouter>);
}

async function openMenu(name: RegExp) {
  const trigger = screen.getByRole("button", { name });
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  await screen.findByRole("menu");
  return trigger;
}

describe("playbook panel row actions", () => {
  it("keeps catalog run primary and managed playbooks read-only", async () => {
    const item: PlaybookCatalogItem = {
      playbook: { id: "playbook-1", name: "Managed deploy", slug: "managed-deploy", category: "deploy",
        connection_default: "ssh", inventory_rest_enabled: false, metadata: {}, risk: "low",
        runtime: "ansible", source_kind: "git", status: "active",
        created_at: targetSet.created_at, updated_at: targetSet.updated_at },
      current_version: { id: "version-1", playbook_id: "playbook-1", content: "---", content_sha256: "sha",
        input_schema: {}, output_schema: {}, metadata: {}, token_policy: {}, runtime: "ansible",
        language: "yaml", timeout_seconds: 60, version: 1, created_at: targetSet.created_at },
    };
    const onRun = vi.fn();
    const onEdit = vi.fn();
    const onDelete = vi.fn();
    render(<AllPermissions><CatalogPanel playbooks={[item]} loading={false} targetSetById={new Map()}
      onRun={onRun} onEdit={onEdit} onDelete={onDelete} onAssign={vi.fn()}
      onVersionHistory={vi.fn()} onUseAsTemplate={vi.fn()} onArchive={vi.fn()} /></AllPermissions>);
    expect(screen.getByRole("heading", { name: /^Catalog$/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^Run$/i }));
    expect(onRun).toHaveBeenCalledWith(item);
    await openMenu(/Actions: Managed deploy/i);
    expect(screen.queryByRole("menuitem", { name: /^Delete$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /^Archive$/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: /View playbook/i }));
    await waitFor(() => expect(onEdit).toHaveBeenCalledWith(item));
    expect(onDelete).not.toHaveBeenCalled();
  });

  it("keeps assignment run visible and suppresses an empty mutation menu", () => {
    const onRun = vi.fn();
    const props = { assignment, canReadTargetSet: true, canUpdate: false, canDelete: false, canExecute: true,
      runPending: false, onRun, onEdit: vi.fn(), onDelete: vi.fn(), onPreview: vi.fn() };
    const { rerender } = render(<AssignmentActions {...props} />);
    fireEvent.click(screen.getByRole("button", { name: /^Run$/i }));
    expect(onRun).toHaveBeenCalledWith(assignment);
    expect(screen.queryByRole("button", { name: /Actions:/i })).not.toBeInTheDocument();
    rerender(<AssignmentActions {...props} assignment={{ ...assignment, executable: false }} />);
    expect(screen.getByRole("button", { name: /^Run$/i })).toBeDisabled();
  });

  it("cancels target actions by Escape, returns focus and never selects the row", async () => {
    const onSelect = vi.fn();
    const onEdit = vi.fn();
    const onDelete = vi.fn();
    render(<AllPermissions><TargetSetsPanel {...tableProps} targetSets={[targetSet]}
      selectedId={null} onSelect={onSelect} onEdit={onEdit} onDelete={onDelete} /></AllPermissions>);
    const trigger = await openMenu(/Actions: SSH targets/i);
    const edit = screen.getByRole("menuitem", { name: /Edit target set/i });
    await waitFor(() => expect(edit).toHaveFocus());
    fireEvent.keyDown(edit, { key: "Escape" });
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onEdit).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    await openMenu(/Actions: SSH targets/i);
    fireEvent.click(screen.getByRole("menuitem", { name: /Edit target set/i }));
    await waitFor(() => expect(onEdit).toHaveBeenCalledWith(targetSet));
    expect(onSelect).not.toHaveBeenCalled();
    const name = screen.getByText("SSH targets");
    const row = name.closest("tr");
    expect(row).toBeInstanceOf(HTMLTableRowElement);
    fireEvent.click(name);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith(targetSet, row);
  });

  it("keeps schedule activation and run-now visible alongside secondary edit/delete", async () => {
    const onToggle = vi.fn();
    const onRunNow = vi.fn();
    const onDelete = vi.fn();
    render(<AllPermissions><SchedulesPanel {...tableProps} schedules={[schedule]}
      playbookById={new Map()} targetSetById={new Map()} servicePrincipalById={new Map()}
      onEdit={vi.fn()} onDelete={onDelete} onToggle={onToggle} togglingId={null}
      onRunNow={onRunNow} runNowPending={false} /></AllPermissions>);
    fireEvent.click(screen.getByRole("switch", { name: /Toggle schedule/i }));
    expect(onToggle).toHaveBeenCalledWith(schedule, false);
    fireEvent.click(screen.getByRole("button", { name: /Run now/i }));
    expect(onRunNow).toHaveBeenCalledWith(schedule);
    const trigger = await openMenu(/Actions: Nightly/i);
    fireEvent.keyDown(screen.getByRole("menuitem", { name: /^Edit$/i }), { key: "Escape" });
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(onDelete).not.toHaveBeenCalled();
    await openMenu(/Actions: Nightly/i);
    fireEvent.click(screen.getByRole("menuitem", { name: /^Delete$/i }));
    await waitFor(() => expect(onDelete).toHaveBeenCalledWith(schedule));
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("offers revoke only for active non-system principals", async () => {
    const onRevoke = vi.fn();
    render(<AllPermissions><ServicePrincipalsPanel {...tableProps} servicePrincipals={[
      principal, { ...principal, id: "system", name: "System", metadata: { system: true } },
      { ...principal, id: "revoked", name: "Revoked", status: "revoked" },
    ]} onRevoke={onRevoke} /></AllPermissions>);
    expect(screen.queryByRole("button", { name: /Actions: System/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Actions: Revoked/i })).not.toBeInTheDocument();
    await openMenu(/Actions: Runner/i);
    fireEvent.click(screen.getByRole("menuitem", { name: /Revoke/i }));
    await waitFor(() => expect(onRevoke).toHaveBeenCalledWith(principal));
  });
});
