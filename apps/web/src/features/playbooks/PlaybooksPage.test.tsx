import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import PlaybooksPage from "./PlaybooksPage";
import { consumeTargetSetReturnFlow, startTargetSetReturnFlow, TARGET_SET_RETURN_LABELS } from "./targetSetReturnFlow";
import type * as Api from "./api";
import type * as SecretsApi from "@/features/automation-secrets/api";

const mock = vi.hoisted(() => ({
  projects: vi.fn(), jobs: vi.fn(), environments: vi.fn(), targets: vi.fn(), schedules: vi.fn(), principals: vi.fn(), runs: vi.fn(), launch: vi.fn(), preview: vi.fn(),
}));
vi.mock("./api", async original => ({
  ...await original<typeof Api>(), listProjects: mock.projects, listJobTemplates: mock.jobs,
  listExecutionEnvironments: mock.environments, listPlaybookTargetSets: mock.targets,
  listPlaybookSchedules: mock.schedules,
  listPlaybookRunGroups: mock.runs, launchJobTemplate: mock.launch, previewPlaybookTargetSet: mock.preview,
}));
vi.mock("@/features/automation-secrets/api", async original => ({
  ...await original<typeof SecretsApi>(), listAutomationSecrets: () => Promise.resolve([]),
}));
vi.mock("./RunDetailPanel", () => ({ RunDetailPanel: ({ target, onClose }: { target: { groupId?: string }; onClose: () => void }) => <div>Run detail {target.groupId}<button onClick={onClose}>Close detail</button></div> }));
const list = (items: unknown[] = []) => ({ items, total: items.length, limit: 100, offset: 0 });
const project = { id: "project-1", name: "Ops project", source: { kind: "editable" }, current_revision_id: "revision-1", created_at: "2026-01-01", updated_at: "2026-01-01" };
const job = { id: "job-1", name: "Deploy job", project_id: "project-1", playbook_paths: ["deploy.yml"], workdir: ".", revision: { kind: "current" }, inventory: { kind: "managed", target_set_id: "set-1" }, execution_environment_id: "environment-1", options: { argv: [], env: {}, extra_vars: {}, secret_refs: {}, timeout_seconds: 60 } };
const launchPreview = { items: [{ name: "edge-1", address: "10.0.0.1", address_bound: true, source_id: "source-1", source_kind: "device", target_id: "target-1", target_kind: "connection_target" }], total: 1, limit: 500, truncated: false, target_set: { id: "set-1", name: "Edge set", mode: "dynamic", entity_kind: "connection_target", selector: {}, created_at: "", updated_at: "" } };
function HistoryProbe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <><output data-testid="route-location">{location.pathname}{location.search}</output><output data-testid="route-state">{JSON.stringify(location.state ?? null)}</output><button onClick={() => { void navigate(-1); }}>History back</button><button onClick={() => { void navigate(1); }}>History forward</button></>;
}
function show(path: string, permissions?: PermissionsValue, state?: unknown) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const entries = state === undefined ? [path] : [{ pathname: path, state }];
  const page = <QueryClientProvider client={client}><MemoryRouter initialEntries={entries}><PlaybooksPage /><HistoryProbe /></MemoryRouter></QueryClientProvider>;
  return render(permissions ? <PermissionsContext.Provider value={permissions}>{page}</PermissionsContext.Provider> : <AllPermissions>{page}</AllPermissions>);
}
function readable(resources: string[]): PermissionsValue {
  const can = (action: string, resource: string) => action === "READ" && resources.includes(resource);
  return { roles: [], isAdmin: false, can, canAny: (resource, actions) => actions.some(action => can(action, resource)), hasRole: () => false };
}
beforeEach(() => {
  vi.resetAllMocks();
  mock.projects.mockResolvedValue(list([project])); mock.jobs.mockResolvedValue(list([job]));
  mock.environments.mockResolvedValue(list()); mock.targets.mockResolvedValue(list());
  mock.schedules.mockResolvedValue(list()); mock.principals.mockResolvedValue(list()); mock.runs.mockResolvedValue(list());
  mock.preview.mockResolvedValue(launchPreview);
});
describe("automation workspace", () => {
  it("exposes four primary collections and subordinate settings", async () => {
    show("/playbooks/projects");
    expect(await screen.findByText("Ops project")).toBeInTheDocument();
    const nav = screen.getByRole("navigation", { name: "Playbook sections" });
    expect(within(nav).getByRole("link", { name: "Projects" })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("link", { name: "Jobs" })).toHaveAttribute("href", "/playbooks/jobs");
    expect(within(nav).getByRole("link", { name: "Runs" })).toHaveAttribute("href", "/playbooks/runs");
    expect(within(nav).getByRole("link", { name: "Schedules" })).toHaveAttribute("href", "/playbooks/schedules");
    expect(within(nav).getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/playbooks/settings");
    expect(within(nav).queryByRole("link", { name: /assignments/i })).toBeNull();
  });
  it("opens a job confirmation and launches the persisted job", async () => {
    mock.launch.mockResolvedValue({ run_group: { id: "group-1" }, runs: [] });
    show("/playbooks/jobs");
    expect(await screen.findByText("Deploy job")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Ops project")).toBeInTheDocument();
    expect(await within(dialog).findByText("Edge set")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Launch" }));
    await waitFor(() => expect(mock.launch).toHaveBeenCalledWith("job-1"));
    await waitFor(() => expect(screen.getByTestId("route-location")).toHaveTextContent("/playbooks/runs?group=group-1&name=Deploy+job"));
  });
  it("hides launch and unrelated sections without their capabilities", async () => {
    show("/playbooks/jobs", readable(["PLAYBOOK_JOB_TEMPLATE", "PLAYBOOK_PROJECT"]));
    expect(await screen.findByText("Deploy job")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Run" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Settings" })).toBeNull();
  });
  it("blocks launch and offers retry when projects cannot be loaded", async () => {
    mock.projects.mockRejectedValue(new Error("unavailable"));
    show("/playbooks/jobs");
    expect(await screen.findByText("Deploy job")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Projects could not be loaded.");
    expect(within(dialog).getByRole("button", { name: "Launch" })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: /retry/i })).toBeEnabled();
  });
  it("opens and closes a linked run group", async () => {
    show("/playbooks/runs?group=group-1&name=Deploy");
    expect(await screen.findByText("Run detail group-1")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close detail" }));
    expect(screen.queryByText("Run detail group-1")).toBeNull();
  });
  it("opens a run selected from history", async () => {
    mock.runs.mockResolvedValue(list([{ id: "run-1234", status: "succeeded", created_at: "2026-01-01T00:00:00Z", total_runs: 1 }]));
    show("/playbooks/runs?status=succeeded");
    const row = (await screen.findByText(/run-1234/i)).closest("tr");
    expect(row).not.toBeNull();
    fireEvent.click(row!);
    expect(await screen.findByText("Run detail run-1234")).toBeInTheDocument();
    expect(screen.getByTestId("route-location")).toHaveTextContent("status=succeeded&group=run-1234");
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    await waitFor(() => expect(screen.queryByText("Run detail run-1234")).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "History forward" }));
    expect(await screen.findByText("Run detail run-1234")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close detail" }));
    expect(screen.getByTestId("route-location")).toHaveTextContent("/playbooks/runs?status=succeeded");
    expect(screen.getByTestId("route-location")).not.toHaveTextContent("group=");
  });
  it("opens the settings overview with four navigable surfaces", async () => {
    show("/playbooks/settings");
    const overview = await screen.findByRole("region", { name: "Settings" });
    expect(within(overview).getByRole("heading", { name: "Execution environments" })).toBeInTheDocument();
    expect(within(overview).getByRole("heading", { name: "Target sets" })).toBeInTheDocument();
    expect(within(overview).getByRole("heading", { name: "Assignments" })).toBeInTheDocument();
    expect(within(overview).getByRole("link", { name: "Open Execution environments" })).toHaveAttribute("href", "/playbooks/settings/execution-environments");
    expect(within(overview).getByRole("link", { name: "Open Target sets" })).toHaveAttribute("href", "/playbooks/settings/target-sets");
    expect(within(overview).getByRole("link", { name: "Open Assignments" })).toHaveAttribute("href", "/playbooks/settings/assignments");
    const nav = screen.getByRole("navigation", { name: "Playbook sections" });
    expect(within(nav).getByRole("link", { name: "Settings" })).toHaveAttribute("aria-current", "page");
  });
  it("offers only the settings surfaces the user can read", async () => {
    show("/playbooks/settings", readable(["PLAYBOOK_TARGET_SET"]));
    const overview = await screen.findByRole("region", { name: "Settings" });
    expect(within(overview).getByRole("link", { name: "Open Target sets" })).toBeInTheDocument();
    expect(within(overview).queryByRole("link", { name: "Open Execution environments" })).toBeNull();
    expect(within(overview).getAllByText(/^No access/)).toHaveLength(2);
  });
  it("returns to the editor through its live return flow", async () => {
    window.sessionStorage.clear();
    const state = startTargetSetReturnFlow("/playbooks/jobs/job-1/edit", { form: { name: "Draft" } }, { labelKey: TARGET_SET_RETURN_LABELS.job });
    show("/playbooks/settings/target-sets", undefined, state);
    expect(await screen.findByRole("link", { name: "Back to the job" })).toHaveAttribute("href", "/playbooks/jobs/job-1/edit");
    expect(JSON.parse(screen.getByTestId("route-state").textContent ?? "null")).toEqual(state);
  });
  it("carries the live return flow from the list into target-set create", async () => {
    window.sessionStorage.clear();
    const state = startTargetSetReturnFlow("/playbooks/jobs/job-1/edit", { form: { name: "Draft" } }, { labelKey: TARGET_SET_RETURN_LABELS.job });
    show("/playbooks/settings/target-sets", undefined, state);
    fireEvent.click((await screen.findAllByRole("button", { name: "Create" }))[0]!);
    await waitFor(() => expect(screen.getByTestId("route-location")).toHaveTextContent("/playbooks/settings/target-sets/new"));
    expect(JSON.parse(screen.getByTestId("route-state").textContent ?? "null")).toEqual(state);
  });
  it("ignores a foreign return destination behind a reference", async () => {
    window.sessionStorage.clear();
    const state = startTargetSetReturnFlow("/playbooks/jobs/job-1/edit", {}, { labelKey: TARGET_SET_RETURN_LABELS.job });
    const flowId = state.targetSetReturnFlow.flowId;
    window.sessionStorage.setItem(`piping.target-set-return-flow:${flowId}`, JSON.stringify({ returnHref: "https://foreign.example/playbooks/jobs", labelKey: TARGET_SET_RETURN_LABELS.job, targetSetId: "", draft: {} }));
    show("/playbooks/settings/target-sets", undefined, state);
    expect(await screen.findByRole("navigation", { name: "Settings sections" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Back to the job" })).toBeNull();
  });
  it("does not offer a return or forward a consumed flow", async () => {
    window.sessionStorage.clear();
    const state = startTargetSetReturnFlow("/playbooks/jobs/job-1/edit", {}, { labelKey: TARGET_SET_RETURN_LABELS.job });
    consumeTargetSetReturnFlow(state);
    show("/playbooks/settings/target-sets", undefined, state);
    expect(await screen.findByRole("navigation", { name: "Settings sections" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Back to the job" })).toBeNull();
    fireEvent.click((await screen.findAllByRole("button", { name: "Create" }))[0]!);
    await waitFor(() => expect(screen.getByTestId("route-location")).toHaveTextContent("/playbooks/settings/target-sets/new"));
    expect(JSON.parse(screen.getByTestId("route-state").textContent ?? "null")).toBeNull();
  });
  it("highlights the routed settings surface, not the first readable one", async () => {
    show("/playbooks/settings/target-sets", readable(["PLAYBOOK_TARGET_SET", "PLAYBOOK_EXECUTION_ENVIRONMENT"]));
    const setup = await screen.findByRole("navigation", { name: "Settings sections" });
    expect(within(setup).getByRole("link", { name: "Target Sets" })).toHaveAttribute("aria-current", "page");
    expect(within(setup).getByRole("link", { name: "Execution Environments" })).not.toHaveAttribute("aria-current");
    expect(within(screen.getByRole("navigation", { name: "Playbook sections" })).getByRole("link", { name: "Settings" })).toHaveAttribute("aria-current", "page");
  });
  it("keeps the Settings tab and the routed surface highlighted across history", async () => {
    show("/playbooks/settings/target-sets", readable(["PLAYBOOK_TARGET_SET"]));
    expect(within(await screen.findByRole("navigation", { name: "Settings sections" })).getByRole("link", { name: "Target Sets" })).toHaveAttribute("aria-current", "page");
    expect(within(screen.getByRole("navigation", { name: "Playbook sections" })).getByRole("link", { name: "Settings" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("link", { name: "Execution Environments" })).toBeNull();
    fireEvent.click(within(screen.getByRole("navigation", { name: "Playbook sections" })).getByRole("link", { name: "Settings" }));
    await waitFor(() => expect(screen.getByTestId("route-location")).toHaveTextContent("/playbooks/settings"));
    expect(screen.queryByRole("navigation", { name: "Settings sections" })).toBeNull();
    expect(within(screen.getByRole("navigation", { name: "Playbook sections" })).getByRole("link", { name: "Settings" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    await waitFor(() => expect(within(screen.getByRole("navigation", { name: "Settings sections" })).getByRole("link", { name: "Target Sets" })).toHaveAttribute("aria-current", "page"));
    expect(within(screen.getByRole("navigation", { name: "Playbook sections" })).getByRole("link", { name: "Settings" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(screen.getByRole("button", { name: "History forward" }));
    await waitFor(() => expect(screen.queryByRole("navigation", { name: "Settings sections" })).toBeNull());
  });
  it("shows a failed jobs request with a retry action", async () => {
    mock.jobs.mockRejectedValue(new Error("unavailable"));
    show("/playbooks/jobs");
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });
  it("offers empty-state templates and a create action with CREATE permission", async () => {
    show("/playbooks/settings/target-sets");
    expect(await screen.findByText("No target sets yet.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Active hosts with an address" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "All hosts" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pick hosts manually" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Create$/ })).not.toHaveLength(0);
  });
  it("hides empty-state templates and create without CREATE permission", async () => {
    show("/playbooks/settings/target-sets", readable(["PLAYBOOK_TARGET_SET"]));
    expect(await screen.findByText("No target sets yet.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Active hosts with an address" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Create$/ })).toBeNull();
  });
  it("opens section help with purpose, related sections and documentation", async () => {
    show("/playbooks/projects");
    expect(await screen.findByText("Ops project")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Section help" }));
    expect(await screen.findByText("Ansible project trees stored in Piping or acquired from Git.")).toBeInTheDocument();
    expect(screen.getByText("Related sections: Jobs.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Documentation/ })).toHaveAttribute("href", "https://piping.space/docs/repositories");
  });
  it("keeps the section help trigger keyboard focusable", async () => {
    show("/playbooks/schedules");
    const trigger = await screen.findByRole("button", { name: "Section help" });
    trigger.focus();
    expect(trigger).toHaveFocus();
    expect(trigger).toBeEnabled();
  });
  it("explains an empty section and offers its create action", async () => {
    mock.jobs.mockResolvedValue(list());
    show("/playbooks/jobs");
    expect(await screen.findByText("No job templates yet.")).toBeInTheDocument();
    expect(screen.getByText(/A job combines a project, an entrypoint/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Create$/ }).length).toBeGreaterThan(0);
  });
  it("keeps Settings reachable with a single readable settings surface", async () => {
    show("/playbooks/settings", readable(["PLAYBOOK_TARGET_SET"]));
    const nav = await screen.findByRole("navigation", { name: "Playbook sections" });
    expect(within(nav).getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/playbooks/settings");
    const overview = await screen.findByRole("region", { name: "Settings" });
    expect(within(overview).getByRole("link", { name: "Open Target sets" })).toBeInTheDocument();
  });
});
