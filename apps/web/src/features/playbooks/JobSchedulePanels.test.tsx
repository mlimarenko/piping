import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { JobTemplatesPanel } from "./JobTemplatesPanel";
import { SchedulesPanel } from "./SchedulesPanel";
import type { JobTemplate, PlaybookSchedule } from "./api";

const job: JobTemplate = { id: "job", name: "Incomplete job", project_id: "project", playbook_paths: [], workdir: ".", inventory: { kind: "managed", target_set_id: "targets" }, revision: { kind: "current" }, execution_environment_id: "environment", options: { argv: [], env: {}, extra_vars: {}, secret_refs: {}, timeout_seconds: 3600 } };
const schedule: PlaybookSchedule = { id: "schedule", name: "Nightly", template_id: "job-identity", targets: { kind: "fixed", targets: [{ hostname: "host" }] }, owner_user_id: "delegator", cron_expression: "0 3 * * *", timezone: "UTC", enabled: true, concurrency_policy: "skip_if_running", misfire_policy: "fire_once", metadata: {}, created_at: "", updated_at: "" };
function wrapper(ui: React.ReactNode) {
  return render(<MemoryRouter><QueryClientProvider client={new QueryClient()}><AllPermissions>{ui}</AllPermissions></QueryClientProvider></MemoryRouter>);
}

describe("job and schedule panels", () => {
  it("marks incomplete jobs unavailable and prevents both button and row launch", () => {
    const run = vi.fn();
    wrapper(<JobTemplatesPanel jobTemplates={[job]} loading={false} error={null} onRetry={vi.fn()} projectById={new Map()} targetSetById={new Map()} executionEnvironmentById={new Map()} onRun={run} runPending={false} />);
    expect(screen.getByText(/unavailable: project or entrypoint missing/i)).toBeInTheDocument();
    expect(screen.queryByText("legacy-playbook")).not.toBeInTheDocument();
    const button = screen.getByRole("button", { name: /^run$/i });
    expect(button).toBeDisabled();
    fireEvent.click(screen.getByText("Incomplete job"));
    expect(run).not.toHaveBeenCalled();
  });
  it("shows the saved job identity and fixed target count", () => {
    wrapper(<SchedulesPanel schedules={[schedule]} loading={false} error={null} onRetry={vi.fn()} sortState={null} onSortChange={vi.fn()} columnFilters={{}} onColumnFilterChange={vi.fn()} targetSetById={new Map()} onEdit={vi.fn()} onToggle={vi.fn()} onDelete={vi.fn()} togglingId={null} />);
    expect(screen.getByText("job-identity")).toBeInTheDocument();
    expect(screen.getByText(/hosts in snapshot: 1/i)).toBeInTheDocument();
  });
  it("allows an enabled project job with an entrypoint to run", () => {
    const run = vi.fn();
    const complete = { ...job, name: "Project job", playbook_paths: ["site.yml"] };
    wrapper(<JobTemplatesPanel jobTemplates={[complete]} loading={false} error={null} onRetry={vi.fn()} projectById={new Map()} targetSetById={new Map()} executionEnvironmentById={new Map()} onRun={run} runPending={false} />);
    fireEvent.click(screen.getByRole("button", { name: /^run$/i }));
    expect(run).toHaveBeenCalledWith(complete);
  });
});
