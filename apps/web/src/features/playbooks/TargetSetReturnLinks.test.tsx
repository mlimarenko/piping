import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import JobTemplateEditorPage from "./JobTemplateEditorPage";
import ScheduleEditorPage from "./ScheduleEditorPage";
import { TARGET_SET_RETURN_LABELS } from "./targetSetReturnFlow";

const api = vi.hoisted(() => ({ list: vi.fn(), getJob: vi.fn(), updateJob: vi.fn(), createJob: vi.fn(), getSchedule: vi.fn(), updateSchedule: vi.fn(), createSchedule: vi.fn() }));
vi.mock("./api", () => ({
  listProjects: api.list, listPlaybookTargetSets: api.list, listExecutionEnvironments: api.list,
  listJobTemplates: api.list,
  getJobTemplate: api.getJob, updateJobTemplate: api.updateJob, createJobTemplate: api.createJob,
  getSchedule: api.getSchedule, updatePlaybookSchedule: api.updateSchedule, createPlaybookSchedule: api.createSchedule,
  previewPlaybookTargetSet: vi.fn(),
}));
vi.mock("@/features/connections/api", () => ({ CONNECTION_TARGETS_QUERY_KEY: ["test", "connection-targets"], listConnectionTargets: vi.fn().mockResolvedValue({ items: [] }) }));

const job = { id: "job-1", name: "Saved job", project_id: "project", workdir: ".", playbook_paths: ["site.yml"], revision: { kind: "current" }, inventory: { kind: "managed", target_set_id: "targets" }, execution_environment_id: "environment", options: { argv: [], extra_vars: {}, env: {}, secret_refs: {}, timeout_seconds: 60 } };
const schedule = { id: "schedule-1", name: "Nightly", template_id: "job-1", owner_user_id: "delegator", targets: { kind: "dynamic", selector: { target_kind: ["custom"], source_kind: ["device"], custom: true } }, cron_expression: "0 3 * * *", timezone: "UTC", concurrency_policy: "skip_if_running", misfire_policy: "fire_once", enabled: true, metadata: {}, created_at: "", updated_at: "" };

function Destination() {
  const location = useLocation();
  return <output data-testid="destination">{location.pathname}|{JSON.stringify(location.state)}</output>;
}

function mount(path: string, element: React.ReactNode, permissions?: PermissionsValue) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = <MemoryRouter initialEntries={[path]}><Routes>
    <Route path="/playbooks/jobs/:jobId/edit" element={element} />
    <Route path="/playbooks/schedules/:id/edit" element={element} />
    <Route path="*" element={<Destination />} />
  </Routes></MemoryRouter>;
  render(<QueryClientProvider client={client}>{permissions ? <PermissionsContext.Provider value={permissions}>{tree}</PermissionsContext.Provider> : <AllPermissions>{tree}</AllPermissions>}</QueryClientProvider>);
}

function flowStateOf(text: string | null): { path: string; state: { targetSetReturnFlow?: { flowId?: string } } } {
  const [path = "", state = "null"] = (text ?? "").split("|");
  return { path, state: JSON.parse(state) };
}

function storedFlow(state: { targetSetReturnFlow?: { flowId?: string } }) {
  const flowId = state.targetSetReturnFlow?.flowId ?? "";
  expect(flowId).not.toBe("");
  return JSON.parse(window.sessionStorage.getItem(`piping.target-set-return-flow:${flowId}`) ?? "null");
}

describe("target-set return flow from the automation editors", () => {
  beforeEach(() => {
    window.sessionStorage.clear();
    api.list.mockResolvedValue({ items: [] });
  });

  it("opens the target-set list from the job editor with a live flow and the edited draft", async () => {
    api.getJob.mockResolvedValue(job);
    mount("/playbooks/jobs/job-1/edit", <JobTemplateEditorPage />);
    await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.click(screen.getByRole("button", { name: "Target sets" }));
    const { path, state } = flowStateOf((await screen.findByTestId("destination")).textContent);
    expect(path).toBe("/playbooks/settings/target-sets");
    const flow = storedFlow(state);
    expect(flow.returnHref).toBe("/playbooks/jobs/job-1/edit");
    expect(flow.labelKey).toBe(TARGET_SET_RETURN_LABELS.job);
    expect(flow.draft.form.name).toBe("Saved job");
    expect(flow.draft.baseline.name).toBe("Saved job");
  });

  it("creates a target set from the job editor through the same flow", async () => {
    api.getJob.mockResolvedValue(job);
    mount("/playbooks/jobs/job-1/edit", <JobTemplateEditorPage />);
    await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.click(screen.getByRole("button", { name: "New target set" }));
    const { path, state } = flowStateOf((await screen.findByTestId("destination")).textContent);
    expect(path).toBe("/playbooks/settings/target-sets/new");
    expect(storedFlow(state).draft.form.name).toBe("Saved job");
  });

  it("hides the create action without the target-set create permission", async () => {
    api.getJob.mockResolvedValue(job);
    const readable: PermissionsValue = { roles: [], isAdmin: false, can: (action, resource) => resource !== "PLAYBOOK_TARGET_SET" || action === "READ", canAny: (resource, actions) => actions.some(action => resource !== "PLAYBOOK_TARGET_SET" || action === "READ"), hasRole: () => false };
    mount("/playbooks/jobs/job-1/edit", <JobTemplateEditorPage />, readable);
    await screen.findByRole("textbox", { name: /^name$/i });
    expect(screen.getByRole("button", { name: "Target sets" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "New target set" })).toBeNull();
  });

  it("opens the target-set list from the schedule editor with a live flow and the edited draft", async () => {
    api.getSchedule.mockResolvedValue(schedule);
    mount("/playbooks/schedules/schedule-1/edit", <ScheduleEditorPage />);
    await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.click(screen.getByRole("button", { name: "Target sets" }));
    const { path, state } = flowStateOf((await screen.findByTestId("destination")).textContent);
    expect(path).toBe("/playbooks/settings/target-sets");
    const flow = storedFlow(state);
    expect(flow.returnHref).toBe("/playbooks/schedules/schedule-1/edit");
    expect(flow.labelKey).toBe(TARGET_SET_RETURN_LABELS.schedule);
    expect(flow.draft.form.name).toBe("Nightly");
  });

  it("creates a target set from the schedule editor through the same flow", async () => {
    api.getSchedule.mockResolvedValue(schedule);
    mount("/playbooks/schedules/schedule-1/edit", <ScheduleEditorPage />);
    await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.click(screen.getByRole("button", { name: "New target set" }));
    const { path, state } = flowStateOf((await screen.findByTestId("destination")).textContent);
    expect(path).toBe("/playbooks/settings/target-sets/new");
    expect(storedFlow(state).draft.form.name).toBe("Nightly");
  });
});
