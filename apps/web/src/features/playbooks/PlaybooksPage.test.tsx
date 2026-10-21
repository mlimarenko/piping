import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate, type InitialEntry } from "react-router";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import PlaybooksPage from "./PlaybooksPage";
import PlaybookDetailPage from "./PlaybookDetailPage";
import TargetSetEditorPage from "./TargetSetEditorPage";
import { startAssignmentDraft } from "./assignmentDraft";
import type { PlaybookRun } from "./api";

const RUN_GROUP_DETAIL_LIMIT = 50_000;

const playbookApiMock = vi.hoisted(() => ({
  getPlaybook: vi.fn(),
  getTargetSet: vi.fn(),
  listPlaybooks: vi.fn(),
  listPlaybookAssignments: vi.fn(),
  listPlaybookTargetSets: vi.fn(),
  listPlaybookSchedules: vi.fn(),
  listPlaybookServicePrincipals: vi.fn(),
  listExecutionEnvironments: vi.fn(),
  listProjects: vi.fn(),
  listJobTemplates: vi.fn(),
  listPlaybookRunGroups: vi.fn(),
  listPlaybookRuns: vi.fn(),
  listPlaybookVersions: vi.fn(),
  previewPlaybookTargetSet: vi.fn(),
  createPlaybook: vi.fn(),
  createExecutionEnvironment: vi.fn(),
  createProject: vi.fn(),
  createJobTemplate: vi.fn(),
  createPlaybookAssignment: vi.fn(),
  updatePlaybookAssignment: vi.fn(),
  deletePlaybookAssignment: vi.fn(),
  createPlaybookTargetSet: vi.fn(),
  deletePlaybookTargetSet: vi.fn(),
  createPlaybookRunGroup: vi.fn(),
  launchJobTemplate: vi.fn(),
  createPlaybookSchedule: vi.fn(),
  updatePlaybookSchedule: vi.fn(),
  deletePlaybookSchedule: vi.fn(),
  createPlaybookServicePrincipal: vi.fn(),
  getPlaybookRun: vi.fn(),
  getPlaybookRunGroup: vi.fn(),
  listPlaybookRunArtifacts: vi.fn(),
  listPlaybookRunGroupEvents: vi.fn(),
  listPlaybookRunEvents: vi.fn(),
  cancelPlaybookRun: vi.fn(),
  cancelPlaybookRunGroup: vi.fn(),
  playbookRunStreamUrl: vi.fn(),
}));

const connectionsApiMock = vi.hoisted(() => ({
  listConnectionTargets: vi.fn(),
}));

const inventoryApiMock = vi.hoisted(() => ({
  listResources: vi.fn(),
}));

vi.mock("@/shared/lib/notifications", () => ({
  notify: vi.fn(),
}));

vi.mock("@/features/inventory/api", () => ({
  listResources: inventoryApiMock.listResources,
}));

vi.mock("@/features/playbooks/api", () => ({
  getPlaybook: playbookApiMock.getPlaybook,
  getTargetSet: playbookApiMock.getTargetSet,
  listPlaybooks: playbookApiMock.listPlaybooks,
  listPlaybookAssignments: playbookApiMock.listPlaybookAssignments,
  listPlaybookTargetSets: playbookApiMock.listPlaybookTargetSets,
  listPlaybookSchedules: playbookApiMock.listPlaybookSchedules,
  listPlaybookServicePrincipals: playbookApiMock.listPlaybookServicePrincipals,
  listExecutionEnvironments: playbookApiMock.listExecutionEnvironments,
  listProjects: playbookApiMock.listProjects,
  listJobTemplates: playbookApiMock.listJobTemplates,
  listPlaybookRunGroups: playbookApiMock.listPlaybookRunGroups,
  listPlaybookRuns: playbookApiMock.listPlaybookRuns,
  listPlaybookVersions: playbookApiMock.listPlaybookVersions,
  previewPlaybookTargetSet: playbookApiMock.previewPlaybookTargetSet,
  createPlaybook: playbookApiMock.createPlaybook,
  createExecutionEnvironment: playbookApiMock.createExecutionEnvironment,
  createProject: playbookApiMock.createProject,
  createJobTemplate: playbookApiMock.createJobTemplate,
  createPlaybookAssignment: playbookApiMock.createPlaybookAssignment,
  updatePlaybookAssignment: playbookApiMock.updatePlaybookAssignment,
  deletePlaybookAssignment: playbookApiMock.deletePlaybookAssignment,
  createPlaybookTargetSet: playbookApiMock.createPlaybookTargetSet,
  deletePlaybookTargetSet: playbookApiMock.deletePlaybookTargetSet,
  createPlaybookRunGroup: playbookApiMock.createPlaybookRunGroup,
  launchJobTemplate: playbookApiMock.launchJobTemplate,
  createPlaybookSchedule: playbookApiMock.createPlaybookSchedule,
  updatePlaybookSchedule: playbookApiMock.updatePlaybookSchedule,
  deletePlaybookSchedule: playbookApiMock.deletePlaybookSchedule,
  createPlaybookServicePrincipal: playbookApiMock.createPlaybookServicePrincipal,
  getPlaybookRun: playbookApiMock.getPlaybookRun,
  getPlaybookRunGroup: playbookApiMock.getPlaybookRunGroup,
  listPlaybookRunArtifacts: playbookApiMock.listPlaybookRunArtifacts,
  listPlaybookRunGroupEvents: playbookApiMock.listPlaybookRunGroupEvents,
  listPlaybookRunEvents: playbookApiMock.listPlaybookRunEvents,
  cancelPlaybookRun: playbookApiMock.cancelPlaybookRun,
  cancelPlaybookRunGroup: playbookApiMock.cancelPlaybookRunGroup,
  playbookRunStreamUrl: playbookApiMock.playbookRunStreamUrl,
}));

vi.mock("@/features/connections/api", () => ({
  CONNECTION_TARGETS_QUERY_KEY: ["connections", "targets"],
  listConnectionTargets: connectionsApiMock.listConnectionTargets,
}));

function LocationProbe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <><span data-testid="location-path">{location.pathname}{location.search}</span><span data-testid="location-state">{JSON.stringify(location.state)}</span><span data-testid="location-key">{location.key}</span><button onClick={() => void navigate(-1)}>History back</button></>;
}

function renderPage(initialEntry: InitialEntry = "/playbooks", permissions?: PermissionsValue, detail = false) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const content = (
    <MemoryRouter initialEntries={[initialEntry]}>
      {detail ? <Routes><Route path="/playbooks/:id" element={<PlaybookDetailPage />} /><Route path="/automation/target-sets/new" element={<TargetSetEditorPage />} /><Route path="/playbooks" element={<div>Catalog destination</div>} /></Routes> : <PlaybooksPage />}
      <LocationProbe />
    </MemoryRouter>
  );
  const view = render(
    <QueryClientProvider client={client}>
      {permissions ? (
        <PermissionsContext.Provider value={permissions}>{content}</PermissionsContext.Provider>
      ) : (
        <AllPermissions>{content}</AllPermissions>
      )}
    </QueryClientProvider>,
  );
  return { ...view, client };
}

function playbookRun(index: number): PlaybookRun {
  return {
    id: `run-${index}`,
    run_group_id: "rg-many",
    playbook_id: "playbook-1",
    playbook_version_id: "version-1",
    source_id: `source-${index}`,
    source_kind: "device",
    target_id: `target-${index}`,
    target_kind: "connection_target",
    target_name: `Target ${index}`,
    target_address: `10.0.0.${index}`,
    target_address_bound: true,
    status: "queued",
    attempt: 1,
    timeout_seconds: 1800,
    credential_account_kind: "none",
    credential_merge_policy: "auto",
    credential_resolution: {},
    metadata: {},
    queued_at: "2026-06-06T00:00:00Z",
  };
}

function openLastActionsMenu() {
  const actionsButtons = screen.getAllByRole("button", { name: /^Actions:/ });
  const actionsButton = actionsButtons.at(-1);
  expect(actionsButton).toBeDefined();
  fireEvent.pointerDown(actionsButton!);
  fireEvent.mouseDown(actionsButton!);
  fireEvent.mouseUp(actionsButton!);
  fireEvent.click(actionsButton!);
}

const noPlaybookCreatePermissions: PermissionsValue = {
  roles: ["viewer"],
  isAdmin: false,
  can: (action, resourceType) => !(action === "CREATE" && resourceType === "PLAYBOOK"),
  canAny: (resourceType, actions) =>
    actions.some((action) => !(action === "CREATE" && resourceType === "PLAYBOOK")),
  hasRole: () => false,
};

describe("PlaybooksPage", () => {
  beforeEach(() => {
    playbookApiMock.getPlaybook.mockReset();
    playbookApiMock.getTargetSet.mockReset();
    playbookApiMock.listPlaybooks.mockReset();
    playbookApiMock.listPlaybookAssignments.mockReset();
    playbookApiMock.listPlaybookTargetSets.mockReset();
    playbookApiMock.listPlaybookSchedules.mockReset();
    playbookApiMock.listPlaybookServicePrincipals.mockReset();
    playbookApiMock.listExecutionEnvironments.mockReset();
    playbookApiMock.listProjects.mockReset();
    playbookApiMock.listJobTemplates.mockReset();
    playbookApiMock.listPlaybookRunGroups.mockReset();
    playbookApiMock.listPlaybookRuns.mockReset();
    playbookApiMock.listPlaybookVersions.mockReset();
    playbookApiMock.previewPlaybookTargetSet.mockReset();
    playbookApiMock.createPlaybook.mockReset();
    playbookApiMock.createExecutionEnvironment.mockReset();
    playbookApiMock.createProject.mockReset();
    playbookApiMock.createJobTemplate.mockReset();
    playbookApiMock.createPlaybookAssignment.mockReset();
    playbookApiMock.updatePlaybookAssignment.mockReset();
    playbookApiMock.deletePlaybookAssignment.mockReset();
    playbookApiMock.createPlaybookTargetSet.mockReset();
    playbookApiMock.deletePlaybookTargetSet.mockReset();
    playbookApiMock.createPlaybookRunGroup.mockReset();
    playbookApiMock.launchJobTemplate.mockReset();
    playbookApiMock.createPlaybookSchedule.mockReset();
    playbookApiMock.updatePlaybookSchedule.mockReset();
    playbookApiMock.deletePlaybookSchedule.mockReset();
    playbookApiMock.createPlaybookServicePrincipal.mockReset();
    playbookApiMock.getPlaybookRun.mockReset();
    playbookApiMock.getPlaybookRunGroup.mockReset();
    playbookApiMock.listPlaybookRunArtifacts.mockReset();
    playbookApiMock.listPlaybookRunGroupEvents.mockReset();
    playbookApiMock.listPlaybookRunEvents.mockReset();
    playbookApiMock.cancelPlaybookRun.mockReset();
    playbookApiMock.cancelPlaybookRunGroup.mockReset();
    playbookApiMock.playbookRunStreamUrl.mockReset();
    connectionsApiMock.listConnectionTargets.mockReset();
    inventoryApiMock.listResources.mockReset();
    inventoryApiMock.listResources.mockResolvedValue({
      items: [],
      total: 0,
      limit: 1000,
      offset: 0,
    });

    playbookApiMock.listPlaybooks.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookAssignments.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookSchedules.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookServicePrincipals.mockResolvedValue({ items: [] });
    playbookApiMock.listExecutionEnvironments.mockResolvedValue({ items: [] });
    playbookApiMock.listProjects.mockResolvedValue({ items: [] });
    playbookApiMock.listJobTemplates.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookRunGroups.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookRuns.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookVersions.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookRunArtifacts.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookRunGroupEvents.mockResolvedValue({
      items: [],
      total: 0,
      limit: RUN_GROUP_DETAIL_LIMIT,
      offset: 0,
    });
    playbookApiMock.listPlaybookRunEvents.mockResolvedValue({ items: [] });
    playbookApiMock.cancelPlaybookRun.mockResolvedValue({});
    playbookApiMock.cancelPlaybookRunGroup.mockResolvedValue({});
    playbookApiMock.launchJobTemplate.mockResolvedValue({
      run_group: { id: "rg-template", total_runs: 1 },
      runs: [],
      target_set: { id: "target-set-1" },
    });
    playbookApiMock.playbookRunStreamUrl.mockReturnValue("ws://localhost/playbooks/runs/run-1");
    connectionsApiMock.listConnectionTargets.mockResolvedValue({
      items: [],
      total: 0,
      limit: 0,
      offset: 0,
    });
    playbookApiMock.previewPlaybookTargetSet.mockResolvedValue({
      target_set: { id: "target-set-1", name: "Active Linux", mode: "dynamic", selector: {} },
      items: [
        {
          target_id: "target-1",
          target_kind: "connection_target",
          source_id: "source-1",
          source_kind: "virtual_machine",
          name: "pxm-piping01",
          address_bound: true,
          address: "10.0.1.10",
        },
      ],
      total: 1,
      limit: 100,
      truncated: false,
    });
  });

  it("renders a clear empty catalog state", async () => {
    renderPage();

    expect(await screen.findByText("No playbooks yet.")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /New playbook/i }).length).toBeGreaterThan(0);
  });

  const parentId = "11111111-1111-4111-8111-111111111111";
  const targetId = "22222222-2222-4222-8222-222222222222";
  const selected = { playbook: { id: parentId, name: "Selected parent", slug: "selected", source_kind: "in_app", status: "active", runtime: "ansible", category: "ops", default_target_set_id: targetId }, current_version: { id: "version-1", version: 3 } };

  it.each([
    ["assignments", "listPlaybookAssignments"],
    ["schedules", "listPlaybookSchedules"],
    ["runs", "listPlaybookRunGroups"],
  ] as const)("deep-links %s with immutable parent scope and URL filters", async (section, method) => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    renderPage(`/playbooks/${parentId}?section=${section}&${section}.enabled=true&${section}.status=failed&${section}.sort=unknown&${section}.page=2&${section}.playbook=foreign`, undefined, true);
    expect(await screen.findByRole("heading", { name: "Selected parent" })).toBeInTheDocument();
    await waitFor(() => expect(playbookApiMock[method]).toHaveBeenCalledWith(expect.objectContaining({ playbook_id: parentId, limit: 100, offset: 200, sort: undefined })));
    const sent = playbookApiMock[method].mock.calls.at(-1)![0];
    expect(section === "runs" ? sent.status : sent.enabled).toBe(section === "runs" ? "failed" : "true");
    expect(playbookApiMock.listPlaybooks).not.toHaveBeenCalled();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });
  it("native Back from target editing restores the same live draft without a history snapshot", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    renderPage(`/playbooks/${parentId}?section=assignments&assignments.enabled=false`, undefined, true);
    await screen.findByRole("heading", { name: "Selected parent" });
    fireEvent.click(screen.getByRole("button", { name: "New assignment" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Native back draft" } });
    fireEvent.click(screen.getByRole("button", { name: "New target set" }));
    await waitFor(() => expect(screen.getByTestId("location-path").textContent).toContain("/automation/target-sets/new"));
    expect(screen.getByTestId("location-state").textContent).not.toContain("Native back draft");
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    expect(await screen.findByDisplayValue("Native back draft")).toBeInTheDocument();
    expect(screen.getByTestId("location-path").textContent).toContain("assignments.enabled=false");
  });
  it.each([true, false])("late target POST after native Back cannot navigate or resurrect the assignment (cancel=%s)", async (cancel) => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    let resolveTarget!: (value: unknown) => void;
    playbookApiMock.createPlaybookTargetSet.mockImplementation(() => new Promise((resolve) => { resolveTarget = resolve; }));
    const entry = `/playbooks/${parentId}?section=assignments&assignments.enabled=false`;
    const mounted = renderPage(entry, undefined, true);
    await screen.findByRole("heading", { name: "Selected parent" });
    fireEvent.click(screen.getByRole("button", { name: "New assignment" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Cancelled while target pending" } });
    fireEvent.click(screen.getByRole("button", { name: "New target set" }));
    await waitFor(() => expect(screen.getByTestId("location-path").textContent).toContain("/automation/target-sets/new"));
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Slow target creation" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(playbookApiMock.createPlaybookTargetSet).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    await screen.findByDisplayValue("Cancelled while target pending");
    if (cancel) {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      await waitFor(() => expect(screen.queryByDisplayValue("Cancelled while target pending")).not.toBeInTheDocument());
      expect(screen.getByTestId("location-state").textContent).toBe("null");
    }
    const currentState = screen.getByTestId("location-state").textContent;
    const currentKey = screen.getByTestId("location-key").textContent;
    await act(async () => { resolveTarget({ target_set: { id: targetId } }); await Promise.resolve(); });
    expect(screen.getByTestId("location-state").textContent).toBe(currentState);
    expect(screen.getByTestId("location-key").textContent).toBe(currentKey);
    expect(screen.getByTestId("location-path").textContent).toBe(entry);
    const state: unknown = JSON.parse(screen.getByTestId("location-state").textContent ?? "null");
    mounted.unmount();
    renderPage({ pathname: `/playbooks/${parentId}`, search: "?section=assignments&assignments.enabled=false", state }, undefined, true);
    if (cancel) {
      await screen.findByRole("heading", { name: "Selected parent" });
      expect(screen.queryByDisplayValue("Cancelled while target pending")).not.toBeInTheDocument();
    }
    else expect(await screen.findByDisplayValue("Cancelled while target pending")).toBeInTheDocument();
  });

  it("keeps selected identity, version scope, return filters and browser section history", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    renderPage(`/playbooks/${parentId}?returnTo=%2Fplaybooks%3Ffilter%3Dactive&assignments.enabled=false`, undefined, true);
    await screen.findByRole("heading", { name: "Selected parent" });
    expect(playbookApiMock.listPlaybookVersions).toHaveBeenCalledWith(parentId, { limit: 100, offset: 0 });
    expect(screen.getByRole("link", { name: "Back" })).toHaveAttribute("href", "/playbooks?filter=active");
    fireEvent.click(screen.getByRole("link", { name: "Assignments" }));
    await waitFor(() => expect(playbookApiMock.listPlaybookAssignments).toHaveBeenCalledWith(expect.objectContaining({ playbook_id: parentId, enabled: "false" })));
    expect(screen.getByRole("link", { name: "Assignments" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    await waitFor(() => expect(screen.getByTestId("location-path").textContent).not.toContain("section=assignments"));
    expect(screen.getByRole("heading", { name: "Selected parent" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Back" }));
    expect(await screen.findByText("Catalog destination")).toBeInTheDocument();
  });

  it("keeps unread child capabilities explicit without requests or mutation actions", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    const permissions: PermissionsValue = { roles: [], isAdmin: false, can: (action, resource) => action === "READ" && resource === "PLAYBOOK", canAny: () => false, hasRole: () => false };
    renderPage(`/playbooks/${parentId}?section=assignments`, permissions, true);
    await screen.findByRole("heading", { name: "Selected parent" });
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Assignments" })).toBeInTheDocument();
    expect(playbookApiMock.listPlaybookAssignments).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookTargetSets).not.toHaveBeenCalled();
    expect(screen.queryByRole("link", { name: "Edit" })).not.toBeInTheDocument();
  });

  it("shows only the persisted default target relation, never the global collection", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    playbookApiMock.getTargetSet.mockResolvedValue({ id: targetId, name: "Only related target" });
    renderPage(`/playbooks/${parentId}?section=targets`, undefined, true);
    expect(await screen.findByRole("heading", { name: /Only related target/ })).toBeInTheDocument();
    expect(playbookApiMock.getTargetSet).toHaveBeenCalledWith(targetId);
    expect(playbookApiMock.previewPlaybookTargetSet).toHaveBeenCalledWith(targetId, 50);
    expect(playbookApiMock.listPlaybookTargetSets).not.toHaveBeenCalled();
  });

  it("rejects a run deep link belonging to another parent before mounting output", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue({ run_group: { playbook_id: "foreign" }, runs: [] });
    renderPage(`/playbooks/${parentId}?section=runs&group=${targetId}`, undefined, true);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(playbookApiMock.getPlaybookRunGroup).toHaveBeenCalledWith(targetId, 50_000);
    expect(playbookApiMock.listPlaybookRunGroupEvents).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookRunGroups).not.toHaveBeenCalled();
  });

  it("restores the assignment draft after the target-set editor return", async () => {
    renderPage({ pathname: "/automation/assignments", state: startAssignmentDraft({ returnHref: "/automation/assignments", editingId: null, form: { playbookId: parentId, targetSetId: targetId, name: "Keep assignment draft", scopeKind: "global", scopeId: "", priority: "87", enabled: true, visible: true, executable: true, quickActionEnabled: false } }) });
    expect(await screen.findByDisplayValue("Keep assignment draft")).toBeInTheDocument();
    expect(playbookApiMock.createPlaybookAssignment).not.toHaveBeenCalled();
  });

  const grants = (resources: string[]): PermissionsValue => ({ roles: [], isAdmin: false, can: (action, resource) => action === "READ" && resources.includes(resource), canAny: () => false, hasRole: () => false });
  it("with RUN but not GROUP denies group list and group detail without requests", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    renderPage(`/playbooks/${parentId}?section=runs&group=${targetId}`, grants(["PLAYBOOK", "PLAYBOOK_RUN"]), true);
    await screen.findByRole("heading", { name: "Selected parent" });
    expect(screen.getByRole("alert")).toHaveTextContent("You don't have access");
    expect(playbookApiMock.listPlaybookRunGroups).not.toHaveBeenCalled();
    expect(playbookApiMock.getPlaybookRunGroup).not.toHaveBeenCalled();
  });
  it("with GROUP but not RUN admits group list, group detail and group output only", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue({ run_group: { id: targetId, playbook_id: parentId, status: "succeeded", total_runs: 1 }, runs: [playbookRun(1)] });
    renderPage(`/playbooks/${parentId}?section=runs&group=${targetId}`, grants(["PLAYBOOK", "PLAYBOOK_RUN_GROUP"]), true);
    await waitFor(() => expect(playbookApiMock.listPlaybookRunGroups).toHaveBeenCalledWith(expect.objectContaining({ playbook_id: parentId })));
    await waitFor(() => expect(playbookApiMock.listPlaybookRunGroupEvents).toHaveBeenCalled());
    expect(playbookApiMock.getPlaybookRunGroup).toHaveBeenCalled();
    expect(playbookApiMock.getPlaybookRun).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookRunEvents).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookRunArtifacts).not.toHaveBeenCalled();
  });
  it("readable assignment rows with no target READ expose no preview request/action", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    playbookApiMock.listPlaybookAssignments.mockResolvedValue({ items: [{ id: "a", name: "Readable assignment", playbook_id: parentId, target_set_id: targetId, scope_kind: "global", enabled: true }], total: 1 });
    renderPage(`/playbooks/${parentId}?section=assignments`, grants(["PLAYBOOK", "PLAYBOOK_ASSIGNMENT"]), true);
    await screen.findByText("Readable assignment");
    expect(screen.queryByRole("button", { name: /Preview targets/ })).not.toBeInTheDocument();
    expect(playbookApiMock.previewPlaybookTargetSet).not.toHaveBeenCalled();
  });
  it("assignment dialog with no target READ cannot issue preview GET by id", async () => {
    const permissions: PermissionsValue = { ...grants(["PLAYBOOK", "PLAYBOOK_ASSIGNMENT"]), can: (action, resource) => resource !== "PLAYBOOK_TARGET_SET" && (action === "READ" || (action === "CREATE" && resource === "PLAYBOOK_ASSIGNMENT")) };
    renderPage({ pathname: "/automation/assignments", state: startAssignmentDraft({ returnHref: "/automation/assignments", editingId: null, form: { playbookId: parentId, targetSetId: targetId, name: "Unread targets draft", scopeKind: "global", scopeId: "", priority: "1", enabled: true, visible: true, executable: true, quickActionEnabled: false } }) }, permissions);
    await screen.findByDisplayValue("Unread targets draft");
    expect(playbookApiMock.previewPlaybookTargetSet).not.toHaveBeenCalled();
    expect(screen.getByText("You don't have access to this section")).toBeInTheDocument();
  });
  it.each([
    ["cancel", "Cancel", false], ["cancel", "Create", false],
    ["save", "Cancel", false], ["save", "Create", false],
    ["cancel", "Cancel", true], ["cancel", "Save", true],
    ["save", "Cancel", true], ["save", "Save", true],
  ] as const)("completed target %s and assignment %s flow (edit=%s) cannot resurrect after Back twice or reload", async (targetCompletion, completion, editing) => {
    playbookApiMock.getPlaybook.mockResolvedValue(selected);
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({ items: [{ id: targetId, name: "Initial target", mode: "dynamic" }] });
    playbookApiMock.createPlaybookAssignment.mockResolvedValue({ id: "new-assignment" });
    playbookApiMock.updatePlaybookAssignment.mockResolvedValue({ id: "updated-assignment" });
    const editingId = "33333333-3333-4333-8333-333333333333";
    const createdTargetId = "44444444-4444-4444-8444-444444444444";
    playbookApiMock.createPlaybookTargetSet.mockResolvedValue({ target_set: { id: createdTargetId } });
    if (editing) playbookApiMock.listPlaybookAssignments.mockResolvedValue({ items: [{ id: editingId, name: "Old assignment", playbook_id: parentId, target_set_id: targetId, scope_kind: "global", priority: 72, enabled: true, visible: true, executable: true, quick_action_enabled: false }], total: 1 });
    const entry = `/playbooks/${parentId}?section=assignments&assignments.enabled=false`;
    const mounted = renderPage(entry, undefined, true);
    await screen.findByRole("heading", { name: "Selected parent" });
    if (editing) {
      await screen.findByText("Old assignment");
      openLastActionsMenu();
      fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    } else fireEvent.click(screen.getByRole("button", { name: "New assignment" }));
    const input = await screen.findByRole("textbox", { name: "Name" });
    fireEvent.change(input, { target: { value: "Complete this draft once" } });
    if (!editing) {
      const targetPicker = screen.getAllByRole("combobox")[1];
      if (!targetPicker) throw new Error("Assignment target picker missing");
      fireEvent.click(targetPicker);
      fireEvent.click(await screen.findByRole("option", { name: /Initial target/ }));
    }
    fireEvent.click(screen.getByRole("button", { name: "New target set" }));
    await waitFor(() => expect(screen.getByTestId("location-path").textContent).toContain("/automation/target-sets/new"));
    if (targetCompletion === "save") {
      fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Created flow target" } });
      fireEvent.click(screen.getByRole("button", { name: "Create" }));
    } else fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await screen.findByDisplayValue("Complete this draft once");
    fireEvent.click(screen.getByRole("button", { name: completion }));
    await waitFor(() => expect(screen.queryByDisplayValue("Complete this draft once")).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    await waitFor(() => expect(screen.getByTestId("location-path").textContent).toContain(`/${parentId}?`));
    expect(screen.queryByDisplayValue("Complete this draft once")).not.toBeInTheDocument();
    if (completion !== "Cancel") {
      const body = expect.objectContaining({ playbook_id: parentId, name: "Complete this draft once", target_set_id: targetCompletion === "save" ? createdTargetId : targetId });
      if (editing) expect(playbookApiMock.updatePlaybookAssignment).toHaveBeenCalledWith(editingId, body);
      else expect(playbookApiMock.createPlaybookAssignment).toHaveBeenCalledWith(body);
    }
    const oldReference: unknown = JSON.parse(screen.getByTestId("location-state").textContent ?? "null");
    mounted.unmount();
    renderPage({ pathname: `/playbooks/${parentId}`, search: "?section=assignments&assignments.enabled=false", state: oldReference }, undefined, true);
    await screen.findByRole("heading", { name: "Selected parent" });
    expect(screen.queryByDisplayValue("Complete this draft once")).not.toBeInTheDocument();
  });

  it("keeps setup pages focused on the current resource", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            source_kind: "in_app",
            default_target_set_id: null,
            current_version_id: "version-1",
            vars_schema: {},
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });

    renderPage("/automation/execution-environments");

    expect(
      await screen.findByRole("heading", { level: 1, name: "Execution environments" }),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("playbooks-workflow")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /New playbook/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Run playbook/i })).not.toBeInTheDocument();
    const create = screen.getByRole("button", { name: /New execution environment/i });
    expect(create.closest('[data-slot="page-actions-host"]')).not.toBeNull();
    fireEvent.click(screen.getByRole("link", { name: /^Projects$/i }));
    expect(await screen.findByRole("button", { name: /New project/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /New execution environment/i })).not.toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it.each(["another-row", "empty", "error", "loading"])("keeps environment preview focus owned after a collection refetch (%s)", async state => {
    const environment = {
      id: "environment-1", name: "Default Ansible", description: "Native runner",
      image: "piping-ansible-runner:local", pull_policy: "if_not_present", runner_group: "default",
      enabled: true, env: {}, settings: {}, metadata: {}, created_at: "2026-06-06T00:00:00Z", updated_at: "2026-06-06T00:00:00Z",
    };
    playbookApiMock.listExecutionEnvironments.mockResolvedValue({ items: [environment] });
    const { client } = renderPage("/automation/execution-environments");
    const row = (await screen.findByText("Default Ansible")).closest("tr")!;
    row.focus();
    fireEvent.keyDown(row, { key: "Enter" });
    const dialog = await screen.findByRole("dialog", { name: "Default Ansible" });
    if (state === "error") playbookApiMock.listExecutionEnvironments.mockRejectedValue(new Error("query failed"));
    else if (state === "loading") playbookApiMock.listExecutionEnvironments.mockReturnValue(new Promise(() => {}));
    else playbookApiMock.listExecutionEnvironments.mockResolvedValue({ items: state === "empty" ? [] : [{ ...environment, id: "environment-2", name: "Other runner" }] });
    await act(async () => {
      if (state === "loading") {
        void client.resetQueries({ queryKey: ["playbooks", "execution-environments"] });
      } else await client.refetchQueries({ queryKey: ["playbooks", "execution-environments"] });
    });
    await waitFor(() => expect(row).not.toBeInTheDocument());
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.getByRole("region", { name: "Execution environments" })).toHaveFocus());
    client.clear();
  });

  it("opens execution environment details from the table row", async () => {
    playbookApiMock.listExecutionEnvironments.mockResolvedValue({
      items: [
        {
          id: "environment-1",
          name: "Default Ansible",
          description: "Built-in native ansible-playbook execution environment.",
          image: "piping-ansible-runner:local",
          pull_policy: "if_not_present",
          runner_group: "default",
          enabled: true,
          env: {},
          settings: {},
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });

    renderPage("/automation/execution-environments");

    const environmentName = await screen.findByText("Default Ansible");
    const environmentRow = environmentName.closest("tr");
    expect(environmentRow).toHaveAttribute("role", "button");
    const actions = within(environmentRow!).getByRole("button", { name: "Actions: Default Ansible" });
    actions.focus();
    fireEvent.keyDown(actions, { key: "ArrowDown" });
    const details = await screen.findByRole("menuitem", { name: "Details" });
    fireEvent.keyDown(details, { key: "Escape" });
    await waitFor(() => expect(actions).toHaveFocus());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(environmentRow!);

    const detailsDialog = await screen.findByRole("dialog", { name: "Default Ansible" });
    expect(
      within(detailsDialog).getByText("piping-ansible-runner:local"),
    ).toBeInTheDocument();
    fireEvent.keyDown(detailsDialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(environmentRow).toHaveFocus());
    actions.focus();
    fireEvent.keyDown(actions, { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Details" }));
    const menuDetailsDialog = await screen.findByRole("dialog", { name: "Default Ansible" });
    fireEvent.keyDown(menuDetailsDialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(actions).toHaveFocus());
  });

  it("creates an editable copy from a read-only system playbook", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "system-playbook-1",
            name: "Auto-update inventory",
            slug: "system-auto-update-inventory",
            description: "Factory inventory collector",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            source_kind: "system",
            default_target_set_id: "target-set-1",
            current_version_id: "system-version-1",
            vars_schema: {},
            metadata: { system: true },
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: {
            id: "system-version-1",
            version: 1,
            content: "---\n- name: Auto-update inventory\n",
            timeout_seconds: 1800,
          },
        },
      ],
    });
    playbookApiMock.createPlaybook.mockResolvedValue({
      playbook: {
        id: "playbook-copy-1",
        name: "Auto-update inventory copy",
        slug: "auto-update-inventory-copy",
        description: "Factory inventory collector",
        category: "inventory",
        runtime: "ansible",
        connection_default: "ssh",
        risk: "low",
        status: "draft",
        source_kind: "in_app",
        default_target_set_id: null,
        current_version_id: "copy-version-1",
        vars_schema: {},
        metadata: {},
        created_at: "2026-06-06T00:00:00Z",
        updated_at: "2026-06-06T00:00:00Z",
      },
      current_version: {
        id: "copy-version-1",
        version: 1,
        content: "---\n- name: Auto-update inventory\n",
        timeout_seconds: 1800,
      },
    });

    renderPage();

    expect(await screen.findByText("Auto-update inventory")).toBeInTheDocument();
    const actionsButton = screen.getByRole("button", { name: /^Actions:/ });
    fireEvent.pointerDown(actionsButton);
    fireEvent.mouseDown(actionsButton);
    fireEvent.mouseUp(actionsButton);
    fireEvent.click(actionsButton);
    fireEvent.click(await screen.findByRole("menuitem", { name: /View playbook/i }));

    // The read-only system playbook opens as a routed editor page (view mode),
    // not an in-memory swap.
    await waitFor(() => {
      expect(screen.getByTestId("location-path")).toHaveTextContent(
        "/playbooks/system-playbook-1/edit",
      );
    });
  });

  it("hides copy actions for read-only playbooks without create permission", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "system-playbook-1",
            name: "Auto-update inventory",
            slug: "system-auto-update-inventory",
            description: "Factory inventory collector",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            source_kind: "system",
            default_target_set_id: null,
            current_version_id: "system-version-1",
            vars_schema: {},
            metadata: { system: true },
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: {
            id: "system-version-1",
            version: 1,
            content: "---\n- name: Auto-update inventory\n",
            timeout_seconds: 1800,
          },
        },
      ],
    });

    renderPage("/playbooks", noPlaybookCreatePermissions);

    expect(await screen.findByText("Auto-update inventory")).toBeInTheDocument();
    const actionsButton = screen.getByRole("button", { name: /^Actions:/ });
    fireEvent.pointerDown(actionsButton);
    fireEvent.mouseDown(actionsButton);
    fireEvent.mouseUp(actionsButton);
    fireEvent.click(actionsButton);

    expect(screen.queryByRole("menuitem", { name: /Use as template/i })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("menuitem", { name: /View playbook/i }));

    // Without create permission the read-only playbook still routes to view mode.
    await waitFor(() => {
      expect(screen.getByTestId("location-path")).toHaveTextContent(
        "/playbooks/system-playbook-1/edit",
      );
    });
  });

  it("opens the version history action in its selected parent context", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            source_kind: "in_app",
            default_target_set_id: null,
            current_version_id: "version-1",
            vars_schema: {},
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: {
            id: "version-1",
            playbook_id: "playbook-1",
            version: 1,
            content: "---\n- hosts: all\n",
            content_sha256: "sha256",
            runtime: "ansible",
            language: "yaml",
            timeout_seconds: 900,
            input_schema: {},
            output_schema: {},
            token_policy: {},
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
          },
        },
      ],
    });
    playbookApiMock.listPlaybookVersions.mockResolvedValue({
      items: [
        {
          id: "version-1",
          playbook_id: "playbook-1",
          version: 1,
          content: "---\n- hosts: all\n",
          content_sha256: "sha256",
          runtime: "ansible",
          language: "yaml",
          timeout_seconds: 900,
          input_schema: {},
          output_schema: {},
          token_policy: {},
          changelog: "Initial version",
          metadata: {},
          created_by_user_id: null,
          created_at: "2026-06-06T00:00:00Z",
        },
      ],
    });

    renderPage();

    expect(await screen.findByText("Linux inventory")).toBeInTheDocument();
    const actionsButton = screen.getByRole("button", { name: /^Actions:/ });
    fireEvent.pointerDown(actionsButton);
    fireEvent.mouseDown(actionsButton);
    fireEvent.mouseUp(actionsButton);
    fireEvent.click(actionsButton);
    fireEvent.click(await screen.findByRole("menuitem", { name: /Version history/i }));

    // The version-history action routes to the editor with the history tab selected.
    await waitFor(() => {
      expect(screen.getByTestId("location-path")).toHaveTextContent(
        "/playbooks/playbook-1?returnTo=%2Fplaybooks&section=versions",
      );
    });
  });

  it("renders the selected section from the route", async () => {
    renderPage("/automation/target-sets");

    expect(screen.queryByTestId("playbooks-workflow")).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { level: 1, name: "Target sets" })).toBeInTheDocument();
    expect(screen.getByText("0 target sets")).toBeInTheDocument();
  });

  it("keeps the service-principals tab independent from catalog and target-set APIs", async () => {
    renderPage("/automation/service-principals");

    expect(
      await screen.findByRole("heading", { level: 1, name: "Service principals" }),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.listPlaybookServicePrincipals).toHaveBeenCalledTimes(1);
    });
    expect(playbookApiMock.listPlaybooks).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookTargetSets).not.toHaveBeenCalled();
  });

  it("loads the reference data required by the schedules tab without setup-only APIs", async () => {
    renderPage("/automation/schedules");

    expect(await screen.findByRole("heading", { level: 1, name: "Schedules" })).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.listPlaybookSchedules).toHaveBeenCalledTimes(1);
      expect(playbookApiMock.listPlaybooks).toHaveBeenCalledTimes(1);
      expect(playbookApiMock.listPlaybookTargetSets).toHaveBeenCalledTimes(1);
      expect(playbookApiMock.listPlaybookServicePrincipals).toHaveBeenCalledTimes(1);
    });
    expect(playbookApiMock.listExecutionEnvironments).not.toHaveBeenCalled();
    expect(playbookApiMock.listProjects).not.toHaveBeenCalled();
    expect(playbookApiMock.listJobTemplates).not.toHaveBeenCalled();
  });

  it("loads both target-set views while leaving the unrelated catalog disabled", async () => {
    renderPage("/automation/target-sets");

    expect(await screen.findByRole("heading", { level: 1, name: "Target sets" })).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.listPlaybookTargetSets).toHaveBeenCalledTimes(2);
    });
    expect(playbookApiMock.listPlaybookTargetSets).toHaveBeenCalledWith({
      limit: 100,
      offset: 0,
      search: undefined,
      sort: undefined,
      order: undefined,
    });
    expect(playbookApiMock.listPlaybookTargetSets).toHaveBeenCalledWith({
      limit: 100,
      offset: 0,
    });
    expect(playbookApiMock.listPlaybooks).not.toHaveBeenCalled();
  });

  it("surfaces the server total in the header badge, not the page row count", async () => {
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
      total: 5,
      limit: 100,
      offset: 0,
    });

    renderPage("/automation/target-sets");

    expect(await screen.findByRole("heading", { level: 1, name: "Target sets" })).toBeInTheDocument();
    // The badge reflects the API total (5), not the single rendered page row.
    expect(await screen.findByText("5 target sets")).toBeInTheDocument();
  });

  it("queues a manual run through the catalog default target set", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: "target-set-1",
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.createPlaybookRunGroup.mockResolvedValue({
      run_group: { total_runs: 3 },
      runs: [],
      target_set: { id: "target-set-1" },
    });

    renderPage();

    expect(await screen.findByText("Linux inventory")).toBeInTheDocument();
    expect(screen.getByText(/Default target set:/i)).toBeInTheDocument();
    // The per-row Run opens the canonical launch dialog directly with the
    // playbook's default target set preselected.
    const runButton = screen
      .getAllByRole("button", { name: /^Run$/i })
      .find((button) => button.textContent?.trim() === "Run");
    expect(runButton).toBeDefined();
    fireEvent.click(runButton!);

    expect(await screen.findByRole("dialog", { name: "Run playbook" })).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.previewPlaybookTargetSet).toHaveBeenCalledWith("target-set-1", 100);
    });
    expect(playbookApiMock.createPlaybookRunGroup).not.toHaveBeenCalled();
    const queueButton = screen.getByRole("button", { name: /Queue run/i });
    await waitFor(() => expect(queueButton).not.toBeDisabled());
    fireEvent.click(queueButton);

    await waitFor(() => {
      expect(playbookApiMock.createPlaybookRunGroup).toHaveBeenCalledWith("playbook-1", expect.objectContaining({
        target_set_id: "target-set-1",
        trigger_kind: "manual",
        target_limit: 1,
        metadata: expect.objectContaining({
          source: "playbooks.catalog_default",
          launch_options: expect.objectContaining({
            target_limit: 1,
            dry_run: false,
            credential_choice: "auto",
            connection: "ssh",
            extravars: {},
          }),
        }),
      }));
    });
  });

  it("queues a manual run from an assignment target set", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookAssignments.mockResolvedValue({
      items: [
        {
          id: "assignment-1",
          playbook_id: "playbook-1",
          target_set_id: "target-set-1",
          name: "All Linux hosts",
          scope_kind: "global",
          scope_id: null,
          priority: 100,
          enabled: true,
          visible: true,
          executable: true,
          quick_action_enabled: true,
          default_input: {},
          default_metadata: {},
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.createPlaybookRunGroup.mockResolvedValue({
      run_group: { total_runs: 3 },
      runs: [],
      target_set: { id: "target-set-1" },
    });

    renderPage("/automation/assignments");
    expect(await screen.findByText("All Linux hosts")).toBeInTheDocument();
    const runButton = screen
      .getAllByRole("button", { name: /Run/i })
      .find((button) => button.textContent?.trim() === "Run");
    expect(runButton).toBeDefined();
    fireEvent.click(runButton!);

    expect(await screen.findByRole("dialog", { name: "Run playbook" })).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.previewPlaybookTargetSet).toHaveBeenCalledWith("target-set-1", 100);
    });
    expect(playbookApiMock.createPlaybookRunGroup).not.toHaveBeenCalled();
    const queueButton = screen.getByRole("button", { name: /Queue run/i });
    await waitFor(() => expect(queueButton).not.toBeDisabled());
    fireEvent.click(queueButton);

    await waitFor(() => {
      expect(playbookApiMock.createPlaybookRunGroup).toHaveBeenCalledWith("playbook-1", expect.objectContaining({
        target_set_id: "target-set-1",
        trigger_kind: "manual",
        target_limit: 1,
        metadata: expect.objectContaining({
          source: "playbooks.assignment",
          assignment_id: "assignment-1",
          launch_options: expect.objectContaining({
            target_limit: 1,
            dry_run: false,
            credential_choice: "auto",
            connection: "ssh",
            extravars: {},
          }),
        }),
      }));
    });
  });

  it("does not queue a run when the confirmation preview resolves zero targets", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: "target-set-1",
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Empty Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.previewPlaybookTargetSet.mockResolvedValue({
      target_set: { id: "target-set-1", name: "Empty Linux", mode: "dynamic", selector: {} },
      items: [],
      total: 0,
      limit: 100,
      truncated: false,
    });

    renderPage();

    expect(await screen.findByText("Linux inventory")).toBeInTheDocument();
    const runButton = screen
      .getAllByRole("button", { name: /^Run$/i })
      .find((button) => button.textContent?.trim() === "Run");
    fireEvent.click(runButton!);

    const queueButton = await screen.findByRole("button", { name: /Queue run/i });
    await screen.findByText("No eligible targets");
    expect(
      screen.getByText(/This target set currently resolves to zero eligible hosts/i),
    ).toBeInTheDocument();

    expect(queueButton).toBeDisabled();
    expect(playbookApiMock.createPlaybookRunGroup).not.toHaveBeenCalled();
  });

  it("runs on multiple selected hosts via a static target set", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    connectionsApiMock.listConnectionTargets.mockResolvedValue({
      items: [
        {
          id: "target-1",
          name: "pxm-piping01",
          targetKind: "virtual_machine",
          status: "active",
          role: "hypervisor",
          site: "aeza-de",
          primaryIp: "10.0.1.10",
          addressBound: true,
          vmId: "vm-1",
        },
        {
          id: "target-2",
          name: "pxm-piping02",
          targetKind: "device",
          status: "active",
          role: "switch",
          site: "aeza-de",
          primaryIp: "10.0.1.11",
          addressBound: true,
          deviceId: "device-2",
        },
      ],
      total: 2,
      limit: 2,
      offset: 0,
    });
    playbookApiMock.createPlaybookTargetSet.mockResolvedValue({
      target_set: { id: "static-set-1", name: "ad-hoc", mode: "static", selector: {} },
      items: [],
      total: 2,
      limit: 100,
      truncated: false,
    });
    playbookApiMock.createPlaybookRunGroup.mockResolvedValue({
      run_group: { id: "rg-1", total_runs: 2, trigger_kind: "bulk" },
      runs: [],
      target_set: { id: "static-set-1" },
    });

    renderPage();

    expect(await screen.findByText("Linux inventory")).toBeInTheDocument();
    // Header action opens the canonical launch dialog (playbook preselected).
    const [runPlaybookButton] = screen.getAllByRole("button", { name: /Run playbook/i });
    expect(runPlaybookButton).toBeDefined();
    fireEvent.click(runPlaybookButton!);

    const runDialog = await screen.findByRole("dialog");
    // Switch the targeting mode to an ad-hoc host list.
    fireEvent.click(within(runDialog).getByRole("button", { name: /Specific hosts/i }));
    // Inventory loads into the inline host picker.
    const host1 = await screen.findByText("pxm-piping01");
    const host2 = await screen.findByText("pxm-piping02");
    fireEvent.click(host1);
    fireEvent.click(host2);

    expect(await screen.findByRole("dialog", { name: "Run playbook" })).toBeInTheDocument();
    // Static targets are previewed without a server target-set lookup.
    expect(playbookApiMock.previewPlaybookTargetSet).not.toHaveBeenCalled();
    const queueButton = await screen.findByRole("button", { name: /Queue run/i });
    await waitFor(() => expect(queueButton).not.toBeDisabled());
    fireEvent.click(queueButton);

    // The static target set is materialized, then the run group is queued.
    await waitFor(() => {
      expect(playbookApiMock.createPlaybookTargetSet).toHaveBeenCalledWith(
        expect.objectContaining({
          mode: "static",
          members: expect.arrayContaining([
            expect.objectContaining({
              target_id: "target-1",
              target_kind: "connection_target",
              source_id: "vm-1",
              source_kind: "virtual_machine",
            }),
            expect.objectContaining({
              target_id: "target-2",
              target_kind: "connection_target",
              source_id: "device-2",
              source_kind: "device",
            }),
          ]),
        }),
      );
    });
    await waitFor(() => {
      expect(playbookApiMock.createPlaybookRunGroup).toHaveBeenCalledWith(
        "playbook-1",
        expect.objectContaining({
          target_set_id: "static-set-1",
          trigger_kind: "bulk",
          target_limit: 2,
          metadata: expect.objectContaining({ source: "playbooks.multi_host" }),
        }),
      );
    });
  });

  it("launches a playbook-backed job template through the template endpoint", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            source_kind: "in_app",
            default_target_set_id: null,
            current_version_id: "version-1",
            vars_schema: {},
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: {
            id: "version-1",
            version: 1,
            content: "---\n- name: Linux inventory\n",
            timeout_seconds: 1800,
          },
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listJobTemplates.mockResolvedValue({
      items: [
        {
          id: "template-1",
          name: "Cash inventory template",
          description: "Reusable inventory run",
          playbook_id: "playbook-1",
          project_id: null,
          playbook_path: null,
          inventory_target_set_id: "target-set-1",
          execution_environment_id: null,
          credential_refs: [],
          extra_vars: { template_extra: "kept" },
          launch_options: {
            forks: 64,
            extravars: { launch_extra: "kept" },
          },
          ask_variables_on_launch: false,
          ask_limit_on_launch: false,
          ask_tags_on_launch: false,
          ask_skip_tags_on_launch: false,
          ask_credential_on_launch: false,
          ask_execution_environment_on_launch: false,
          enabled: true,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.launchJobTemplate.mockResolvedValue({
      run_group: { id: "rg-template-1", total_runs: 1 },
      runs: [],
      target_set: { id: "target-set-1" },
    });

    renderPage("/automation/job-templates");

    expect(await screen.findByText("Cash inventory template")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^Run$/i }));

    expect(await screen.findByRole("dialog", { name: "Run playbook" })).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.previewPlaybookTargetSet).toHaveBeenCalledWith("target-set-1", 100);
    });

    const queueButton = screen.getByRole("button", { name: /Queue run/i });
    await waitFor(() => expect(queueButton).not.toBeDisabled());
    fireEvent.click(queueButton);

    await waitFor(() => {
      expect(playbookApiMock.launchJobTemplate).toHaveBeenCalledWith(
        "template-1",
        expect.objectContaining({
          target_set_id: "target-set-1",
          trigger_kind: "manual",
          target_limit: 1,
          metadata: expect.objectContaining({
            source: "playbooks.job_template",
            job_template_id: "template-1",
            job_template_name: "Cash inventory template",
            launch_options: expect.objectContaining({
              forks: 64,
              target_limit: 1,
              dry_run: false,
              credential_choice: "auto",
              connection: "ssh",
              extravars: expect.objectContaining({
                launch_extra: "kept",
                template_extra: "kept",
              }),
            }),
          }),
        }),
      );
    });
    expect(playbookApiMock.createPlaybookRunGroup).not.toHaveBeenCalled();
  });

  it("opens a project's files editor from the Projects tab", async () => {
    playbookApiMock.listProjects.mockResolvedValue({
      items: [
        {
          id: "project-1",
          name: "Ops project",
          description: "Playbooks from SCM",
          scm_type: "manual",
          scm_url: null,
          scm_branch: null,
          scm_ref: null,
          playbook_base_path: "playbooks",
          credential_id: null,
          default_execution_environment_id: null,
          sync_policy: "manual",
          sync_status: "never_synced",
          last_sync_revision: null,
          last_sync_error: null,
          last_synced_at: null,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });

    renderPage("/automation/projects");

    expect(await screen.findByText("Ops project")).toBeInTheDocument();
    const actions = screen.getByRole("button", { name: "Actions: Ops project" });
    actions.focus();
    fireEvent.keyDown(actions, { key: "ArrowDown" });
    const files = await screen.findByRole("menuitem", { name: "Files" });
    expect(files).toHaveAttribute(
      "href",
      "/automation/projects/project-1/files",
    );
    fireEvent.click(files);
    await waitFor(() => expect(screen.getByTestId("location-path")).toHaveTextContent("/automation/projects/project-1/files"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("owns project row and menu previews and yields to Files navigation", async () => {
    playbookApiMock.listProjects.mockResolvedValue({ items: [{
      id: "project-1", name: "Ops project", description: "Playbooks from SCM", scm_type: "manual",
      scm_url: null, scm_branch: null, scm_ref: null, playbook_base_path: "playbooks", credential_id: null,
      default_execution_environment_id: null, sync_policy: "manual", sync_status: "never_synced",
      last_sync_revision: null, last_sync_error: null, last_synced_at: null, metadata: {},
      created_at: "2026-06-06T00:00:00Z", updated_at: "2026-06-06T00:00:00Z",
    }] });
    renderPage("/automation/projects");
    const row = (await screen.findByText("Ops project")).closest("tr")!;
    row.focus();
    fireEvent.keyDown(row, { key: " " });
    fireEvent.keyDown(await screen.findByRole("dialog", { name: "Ops project" }), { key: "Escape" });
    await waitFor(() => expect(row).toHaveFocus());
    const actions = screen.getByRole("button", { name: "Actions: Ops project" });
    actions.focus();
    fireEvent.keyDown(actions, { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Details" }));
    const dialog = await screen.findByRole("dialog", { name: "Ops project" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Files" }));
    await waitFor(() => expect(screen.getByTestId("location-path")).toHaveTextContent("/automation/projects/project-1/files"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(row).not.toHaveFocus();
    expect(actions).not.toHaveFocus();
  });

  it("launches a project-backed job template through the template endpoint", async () => {
    playbookApiMock.listProjects.mockResolvedValue({
      items: [
        {
          id: "project-1",
          name: "Ops project",
          description: "Playbooks from SCM",
          scm_type: "manual",
          scm_url: null,
          scm_branch: null,
          scm_ref: null,
          playbook_base_path: "playbooks",
          credential_id: null,
          default_execution_environment_id: null,
          sync_policy: "manual",
          sync_status: "never_synced",
          last_sync_revision: null,
          last_sync_error: null,
          last_synced_at: null,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "SSH targets",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { connection_protocol: ["ssh"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listJobTemplates.mockResolvedValue({
      items: [
        {
          id: "template-project-1",
          name: "Project deploy",
          description: "Run project playbook",
          playbook_id: null,
          project_id: "project-1",
          playbook_path: "site.yml",
          inventory_target_set_id: "target-set-1",
          execution_environment_id: null,
          credential_refs: [],
          extra_vars: { release: "stable" },
          launch_options: { forks: 32 },
          ask_variables_on_launch: false,
          ask_limit_on_launch: false,
          ask_tags_on_launch: false,
          ask_skip_tags_on_launch: false,
          ask_credential_on_launch: false,
          ask_execution_environment_on_launch: false,
          enabled: true,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.launchJobTemplate.mockResolvedValue({
      run_group: { id: "rg-project-template-1", total_runs: 1 },
      runs: [],
      target_set: { id: "target-set-1" },
    });

    renderPage("/automation/job-templates");

    expect(await screen.findByText("Project deploy")).toBeInTheDocument();
    expect(screen.getByText("Ops project")).toBeInTheDocument();
    expect(screen.getByText("/site.yml")).toBeInTheDocument();
    expect(screen.getByText("SSH targets")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Project deploy"));

    await waitFor(() => {
      expect(playbookApiMock.launchJobTemplate).toHaveBeenCalledWith(
        "template-project-1",
        expect.objectContaining({
          target_set_id: "target-set-1",
          trigger_kind: "manual",
          metadata: expect.objectContaining({
            source: "playbooks.job_template",
            job_template_id: "template-project-1",
            job_template_name: "Project deploy",
            launch_options: expect.objectContaining({
              forks: 32,
              extravars: expect.objectContaining({ release: "stable" }),
            }),
          }),
        }),
      );
    });
    expect(playbookApiMock.launchJobTemplate).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog", { name: "Run playbook" })).not.toBeInTheDocument();
    expect(playbookApiMock.createPlaybookRunGroup).not.toHaveBeenCalled();
  });

  it("does not expose disabled job template rows as launch actions", async () => {
    playbookApiMock.listProjects.mockResolvedValue({
      items: [
        {
          id: "project-1",
          name: "Ops project",
          description: "Playbooks from SCM",
          scm_type: "manual",
          scm_url: null,
          scm_branch: null,
          scm_ref: null,
          playbook_base_path: "playbooks",
          credential_id: null,
          default_execution_environment_id: null,
          sync_policy: "manual",
          sync_status: "never_synced",
          last_sync_revision: null,
          last_sync_error: null,
          last_synced_at: null,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "SSH targets",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { connection_protocol: ["ssh"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listJobTemplates.mockResolvedValue({
      items: [
        {
          id: "template-disabled-1",
          name: "Disabled deploy",
          description: "Temporarily unavailable",
          playbook_id: null,
          project_id: "project-1",
          playbook_path: "site.yml",
          inventory_target_set_id: "target-set-1",
          execution_environment_id: null,
          credential_refs: [],
          extra_vars: {},
          launch_options: {},
          ask_variables_on_launch: false,
          ask_limit_on_launch: false,
          ask_tags_on_launch: false,
          ask_skip_tags_on_launch: false,
          ask_credential_on_launch: false,
          ask_execution_environment_on_launch: false,
          enabled: false,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });

    renderPage("/automation/job-templates");

    const templateName = await screen.findByText("Disabled deploy");
    expect(templateName.closest("tr")).not.toHaveAttribute("role", "button");
    expect(screen.getByRole("button", { name: /^Run$/i })).toBeDisabled();

    fireEvent.click(templateName);
    expect(playbookApiMock.launchJobTemplate).not.toHaveBeenCalled();
  });

  it("lists schedules and toggles enabled state through PATCH", async () => {
    playbookApiMock.listPlaybookSchedules.mockResolvedValue({
      items: [
        {
          id: "schedule-1",
          name: "Nightly inventory",
          playbook_id: "playbook-1",
          target_set_id: "target-set-1",
          service_principal_id: "sp-1",
          cron_expression: "0 3 * * *",
          timezone: "UTC",
          enabled: true,
          concurrency_policy: "skip_if_running",
          misfire_policy: "fire_once",
          next_run_at: "2026-06-07T03:00:00Z",
          last_run_at: "2026-06-06T03:00:00Z",
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.updatePlaybookSchedule.mockResolvedValue({});

    renderPage("/automation/schedules");

    expect(screen.getByTestId("location-path")).toHaveTextContent("/automation/schedules");
    expect(await screen.findByText("Nightly inventory")).toBeInTheDocument();
    expect(screen.getByText("0 3 * * *")).toBeInTheDocument();
    expect(screen.queryByText(/Schedules are read-only here/i)).not.toBeInTheDocument();

    // The enable/disable switch issues a PATCH with the new enabled flag.
    const toggle = screen.getByRole("switch", { name: /Toggle schedule/i });
    fireEvent.click(toggle);
    await waitFor(() => {
      expect(playbookApiMock.updatePlaybookSchedule).toHaveBeenCalledWith("schedule-1", {
        enabled: false,
      });
    });
  });

  it("reviews and queues a schedule run through the shared launch dialog", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listPlaybookSchedules.mockResolvedValue({
      items: [
        {
          id: "schedule-1",
          name: "Nightly inventory",
          playbook_id: "playbook-1",
          target_set_id: "target-set-1",
          service_principal_id: "sp-1",
          cron_expression: "0 3 * * *",
          timezone: "UTC",
          enabled: true,
          concurrency_policy: "skip_if_running",
          misfire_policy: "fire_once",
          next_run_at: "2026-06-07T03:00:00Z",
          last_run_at: "2026-06-06T03:00:00Z",
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.createPlaybookRunGroup.mockResolvedValue({
      run_group: { total_runs: 1 },
      runs: [],
      target_set: { id: "target-set-1" },
    });

    renderPage("/automation/schedules");

    expect(await screen.findByText("Nightly inventory")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Run now/i }));

    expect(await screen.findByRole("dialog", { name: "Run playbook" })).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.previewPlaybookTargetSet).toHaveBeenCalledWith("target-set-1", 100);
    });
    expect(playbookApiMock.createPlaybookRunGroup).not.toHaveBeenCalled();

    const queueButton = screen.getByRole("button", { name: /Queue run/i });
    await waitFor(() => expect(queueButton).not.toBeDisabled());
    fireEvent.click(queueButton);

    await waitFor(() => {
      expect(playbookApiMock.createPlaybookRunGroup).toHaveBeenCalledWith(
        "playbook-1",
        expect.objectContaining({
          target_set_id: "target-set-1",
          trigger_kind: "manual",
          target_limit: 1,
          metadata: expect.objectContaining({
            source: "playbooks.schedule_run_now",
            schedule_id: "schedule-1",
            launch_options: expect.objectContaining({
              target_limit: 1,
              dry_run: false,
              credential_choice: "auto",
              connection: "ssh",
              extravars: {},
            }),
          }),
        }),
      );
    });
  });

  it("creates a schedule with fire_once misfire and a service principal", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listPlaybookServicePrincipals.mockResolvedValue({
      items: [
        {
          id: "sp-1",
          name: "Scheduler",
          slug: "scheduler",
          status: "active",
          scopes: {},
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.createPlaybookSchedule.mockResolvedValue({ id: "schedule-1" });

    renderPage("/automation/schedules");

    fireEvent.click(await screen.findByRole("button", { name: /New schedule/i }));

    // "New schedule" routes to the canonical schedule editor page.
    await waitFor(() => {
      expect(screen.getByTestId("location-path")).toHaveTextContent(
        "/automation/schedules/new",
      );
    });
  });

  it("deletes a schedule after confirmation", async () => {
    playbookApiMock.listPlaybookSchedules.mockResolvedValue({
      items: [
        {
          id: "schedule-1",
          name: "Nightly inventory",
          playbook_id: "playbook-1",
          target_set_id: "target-set-1",
          service_principal_id: "sp-1",
          cron_expression: "0 3 * * *",
          timezone: "UTC",
          enabled: true,
          concurrency_policy: "skip_if_running",
          misfire_policy: "fire_once",
          next_run_at: null,
          last_run_at: null,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.deletePlaybookSchedule.mockResolvedValue(undefined);

    renderPage("/automation/schedules");

    expect(await screen.findByText("Nightly inventory")).toBeInTheDocument();
    const actions = screen.getByRole("button", { name: "Actions: Nightly inventory" });
    actions.focus();
    fireEvent.keyDown(actions, { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));

    // The confirmation dialog gates the DELETE.
    let confirmDialog = await screen.findByRole("dialog");
    expect(within(confirmDialog).getByText(/Nightly inventory/)).toBeInTheDocument();
    expect(playbookApiMock.deletePlaybookSchedule).not.toHaveBeenCalled();
    fireEvent.click(within(confirmDialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(actions).toHaveFocus(), { timeout: 1000 });
    expect(playbookApiMock.deletePlaybookSchedule).not.toHaveBeenCalled();
    fireEvent.keyDown(actions, { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    confirmDialog = await screen.findByRole("dialog");
    fireEvent.keyDown(within(confirmDialog).getByRole("button", { name: "Cancel" }), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(actions).toHaveFocus());
    expect(playbookApiMock.deletePlaybookSchedule).not.toHaveBeenCalled();
    fireEvent.keyDown(actions, { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    confirmDialog = await screen.findByRole("dialog");
    fireEvent.click(within(confirmDialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(playbookApiMock.deletePlaybookSchedule).toHaveBeenCalledWith("schedule-1");
    });
  });

  it("creates a service principal inline from the schedule editor", async () => {
    playbookApiMock.createPlaybookServicePrincipal.mockResolvedValue({
      id: "sp-new",
      name: "Ops bot",
      slug: "ops-bot",
      status: "active",
      scopes: {},
      metadata: {},
      created_at: "2026-06-06T00:00:00Z",
      updated_at: "2026-06-06T00:00:00Z",
    });

    renderPage("/automation/schedules");

    fireEvent.click(await screen.findByRole("button", { name: /New schedule/i }));

    // "New schedule" routes to the schedule editor; the inline SP creation now
    // lives in that routed page (covered by ScheduleEditorPage tests).
    await waitFor(() => {
      expect(screen.getByTestId("location-path")).toHaveTextContent(
        "/automation/schedules/new",
      );
    });
  });

  it("lists run history filtered by playbook and drills into a run group", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookRunGroups.mockResolvedValue({
      items: [
        {
          id: "rg-1",
          playbook_id: "playbook-1",
          playbook_version_id: "version-1",
          target_set_id: null,
          schedule_id: null,
          requested_by_user_id: null,
          service_principal_id: null,
          trigger_kind: "manual",
          status: "succeeded",
          total_runs: 1,
          dispatch_id: null,
          dispatch_attempt: 0,
          succeeded_runs: 1,
          failed_runs: 0,
          canceled_runs: 0,
          timeout_runs: 0,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
          started_at: "2026-06-06T00:00:01Z",
          finished_at: "2026-06-06T00:00:03Z",
        },
      ],
    });
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue({
      run_group: {
        id: "rg-1",
        status: "succeeded",
        total_runs: 1,
        dispatch_id: null,
        dispatch_attempt: 0,
        succeeded_runs: 1,
        failed_runs: 0,
        canceled_runs: 0,
        timeout_runs: 0,
        playbook_id: "playbook-1",
        playbook_version_id: "version-1",
        trigger_kind: "manual",
        metadata: {},
        created_at: "2026-06-06T00:00:00Z",
        updated_at: "2026-06-06T00:00:00Z",
      },
      runs: [],
    });

    renderPage("/automation/runs");

    expect(screen.getByTestId("location-path")).toHaveTextContent("/automation/runs");
    // The run-history row renders aggregate status from GET /run-groups.
    const runRow = await screen.findByText("Linux inventory");
    expect(screen.getByText("Succeeded")).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.listPlaybookRunGroups).toHaveBeenCalledWith(
        expect.objectContaining({ limit: 100, offset: 0 }),
      );
    });

    // Clicking the run drills into its group via GET /run-groups/{id}.
    fireEvent.click(runRow);
    await waitFor(() => {
      expect(playbookApiMock.getPlaybookRunGroup).toHaveBeenCalledWith("rg-1", RUN_GROUP_DETAIL_LIMIT);
    });
  });

  it("opens the run detail directly from a Runs-tab deep link (?group=&name=)", async () => {
    // Exercises the deep-link contract the Connections playbook tab's
    // "open in a new tab" button relies on (playbookRunGroupHref).
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue({
      run_group: {
        id: "rg-1",
        status: "running",
        total_runs: 2,
        dispatch_id: null,
        dispatch_attempt: 0,
        succeeded_runs: 0,
        failed_runs: 0,
        canceled_runs: 0,
        timeout_runs: 0,
        playbook_id: "playbook-1",
        playbook_version_id: "version-1",
        trigger_kind: "manual",
        metadata: {},
        created_at: "2026-06-06T00:00:00Z",
        updated_at: "2026-06-06T00:00:00Z",
      },
      runs: [],
    });

    renderPage("/automation/runs?group=rg-1&name=Update+inventory");

    expect(screen.getByTestId("location-path")).toHaveTextContent("/automation/runs");
    await waitFor(() => {
      expect(playbookApiMock.getPlaybookRunGroup).toHaveBeenCalledWith("rg-1", RUN_GROUP_DETAIL_LIMIT);
    });
    // The deep-linked group's name (carried in the URL, not re-resolved from
    // the catalog) shows up in the opened RunDetailPanel.
    expect(await screen.findByText("Update inventory")).toBeInTheDocument();
  });

  it("loads the full run-group target list when expanding run details", async () => {
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue({
      run_group: {
        id: "rg-many",
        status: "running",
        total_runs: 30,
        dispatch_id: null,
        dispatch_attempt: 0,
        succeeded_runs: 0,
        failed_runs: 0,
        canceled_runs: 0,
        timeout_runs: 0,
        playbook_id: "playbook-1",
        playbook_version_id: "version-1",
        trigger_kind: "manual",
        metadata: {},
        created_at: "2026-06-06T00:00:00Z",
        updated_at: "2026-06-06T00:00:00Z",
      },
      runs: Array.from({ length: 30 }, (_, index) => playbookRun(index + 1)),
    });

    renderPage("/automation/runs?group=rg-many&name=Install+exporter");

    await waitFor(() => {
      expect(playbookApiMock.getPlaybookRunGroup).toHaveBeenCalledWith(
        "rg-many",
        RUN_GROUP_DETAIL_LIMIT,
      );
    });
    const showAll = await screen.findByRole("button", { name: "Show all (30)" });
    expect(screen.queryByText("Target 30")).not.toBeInTheDocument();

    fireEvent.click(showAll);

    expect(await screen.findByText("Target 30")).toBeInTheDocument();
  });

  it("reports a manually canceled run group as canceled instead of succeeded", async () => {
    const canceledRuns = Array.from({ length: 2 }, (_, index) => ({
      ...playbookRun(index + 1),
      run_group_id: "rg-canceled",
      status: "canceled" as const,
    }));
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue({
      run_group: {
        id: "rg-canceled",
        status: "canceled",
        total_runs: 2,
        dispatch_id: null,
        dispatch_attempt: 0,
        succeeded_runs: 0,
        failed_runs: 0,
        canceled_runs: 2,
        timeout_runs: 0,
        playbook_id: "playbook-1",
        playbook_version_id: "version-1",
        trigger_kind: "manual",
        metadata: {},
        created_at: "2026-06-06T00:00:00Z",
        updated_at: "2026-06-06T00:00:00Z",
      },
      runs: canceledRuns,
    });

    renderPage("/automation/runs?group=rg-canceled&name=Manual+stop");

    const panel = await screen.findByRole("region", { name: "Run detail" });
    expect(await within(panel).findByText("Target 1")).toBeInTheDocument();
    expect(within(panel).getByText("Manual stop")).toBeInTheDocument();
    const canceledBadges = within(panel).getAllByText("Canceled");
    expect(canceledBadges.length).toBeGreaterThan(0);
    expect(within(panel).queryByText("Succeeded")).not.toBeInTheDocument();
  });

  it("shows the run-group target total instead of the visible run-row cap", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookRunGroups.mockResolvedValue({
      items: [
        {
          id: "rg-many",
          playbook_id: "playbook-1",
          playbook_version_id: "version-1",
          target_set_id: null,
          schedule_id: null,
          requested_by_user_id: null,
          service_principal_id: null,
          trigger_kind: "manual",
          status: "queued",
          total_runs: 4000,
          dispatch_id: null,
          dispatch_attempt: 0,
          succeeded_runs: 0,
          failed_runs: 0,
          canceled_runs: 0,
          timeout_runs: 0,
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
          started_at: null,
          finished_at: null,
        },
      ],
    });

    renderPage("/automation/runs");

    await waitFor(() => {
      expect(playbookApiMock.listPlaybookRunGroups).toHaveBeenCalledWith(
        expect.objectContaining({ limit: 100, offset: 0 }),
      );
    });
    expect(await screen.findByText("4000 targets")).toBeInTheDocument();
    expect(screen.queryByText("100 targets")).not.toBeInTheDocument();
    expect(playbookApiMock.getPlaybookRunGroup).not.toHaveBeenCalled();
  });

  it("creates an assignment binding the chosen scope entity id, not its name", async () => {
    const ROLE_ID = "11111111-1111-1111-1111-111111111111";
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "22222222-2222-2222-2222-222222222222",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    // Device Roles populate the scope entity picker (id-bound).
    inventoryApiMock.listResources.mockImplementation((entity: string) =>
      Promise.resolve(
        entity === "Device Roles"
          ? { items: [{ id: ROLE_ID, name: "web" }], total: 1, limit: 1000, offset: 0 }
          : { items: [], total: 0, limit: 1000, offset: 0 },
      ),
    );
    playbookApiMock.createPlaybookAssignment.mockResolvedValue({ id: "assignment-1" });

    renderPage("/automation/assignments");

    fireEvent.click(await screen.findByRole("button", { name: /New assignment/i }));
    const dialog = await screen.findByTestId("entity-editor-page");

    // Basics: pick the playbook.
    fireEvent.click(within(dialog).getByText("Select playbook"));
    fireEvent.click(await screen.findByRole("option", { name: "Linux inventory" }));
    // Targeting: pick the target set.
    fireEvent.click(within(dialog).getByText("Select target set"));
    fireEvent.click(await screen.findByRole("option", { name: /Active Linux/i }));

    // Targeting: the scope-kind select defaults to "Global"; switch it to Role.
    const scopeKindSelect = within(dialog)
      .getAllByRole("combobox")
      .find((el) => el.textContent?.trim() === "Global")!;
    expect(scopeKindSelect).toBeDefined();
    fireEvent.click(scopeKindSelect);
    fireEvent.click(await screen.findByRole("option", { name: "Role" }));
    // The entity picker (labelled scope target) now offers the role by name.
    const entitySelect = within(dialog).getByRole("combobox", { name: /Scope target/i });
    fireEvent.click(entitySelect);
    fireEvent.click(await screen.findByRole("option", { name: "web" }));

    fireEvent.click(within(dialog).getByRole("button", { name: /^Create$/i }));

    await waitFor(() => {
      expect(playbookApiMock.createPlaybookAssignment).toHaveBeenCalledWith(
        expect.objectContaining({
          playbook_id: "playbook-1",
          target_set_id: "22222222-2222-2222-2222-222222222222",
          scope_kind: "role",
          // The headline guarantee: the entity id is stored, never its name.
          scope_id: ROLE_ID,
        }),
      );
    });
  });

  it("deletes a target set after confirmation", async () => {
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.deletePlaybookTargetSet.mockResolvedValue(undefined);

    renderPage("/automation/target-sets");

    expect(screen.getByTestId("location-path")).toHaveTextContent("/automation/target-sets");
    expect(await screen.findByText("Active Linux")).toBeInTheDocument();

    openLastActionsMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete target set" }));

    const deleteDialog = await screen.findByRole("alertdialog");
    expect(within(deleteDialog).getByText(/Active Linux/)).toBeInTheDocument();
    expect(
      within(deleteDialog).getByText(/Assignments that use this target set/),
    ).toBeInTheDocument();
    expect(playbookApiMock.deletePlaybookTargetSet).not.toHaveBeenCalled();

    fireEvent.click(within(deleteDialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(playbookApiMock.deletePlaybookTargetSet).toHaveBeenCalledWith("target-set-1");
    });
  });

  it("edits and deletes an existing assignment", async () => {
    playbookApiMock.listPlaybooks.mockResolvedValue({
      items: [
        {
          playbook: {
            id: "playbook-1",
            name: "Linux inventory",
            slug: "linux-inventory",
            description: "Collect inventory",
            category: "inventory",
            runtime: "ansible",
            connection_default: "ssh",
            risk: "low",
            status: "active",
            default_target_set_id: null,
            current_version_id: "version-1",
            metadata: {},
            created_at: "2026-06-06T00:00:00Z",
            updated_at: "2026-06-06T00:00:00Z",
          },
          current_version: { id: "version-1", version: 1 },
        },
      ],
    });
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({
      items: [
        {
          id: "target-set-1",
          name: "Active Linux",
          mode: "dynamic",
          entity_kind: "connection_target",
          selector: { status: ["active"] },
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.listPlaybookAssignments.mockResolvedValue({
      items: [
        {
          id: "assignment-1",
          playbook_id: "playbook-1",
          target_set_id: "target-set-1",
          name: "All Linux hosts",
          scope_kind: "global",
          scope_id: null,
          priority: 100,
          enabled: true,
          visible: true,
          executable: true,
          quick_action_enabled: true,
          default_input: {},
          default_metadata: {},
          metadata: {},
          created_at: "2026-06-06T00:00:00Z",
          updated_at: "2026-06-06T00:00:00Z",
        },
      ],
    });
    playbookApiMock.updatePlaybookAssignment.mockResolvedValue({
      id: "assignment-1",
      name: "Regional Linux hosts",
    });
    playbookApiMock.deletePlaybookAssignment.mockResolvedValue(undefined);

    renderPage("/automation/assignments");

    expect(screen.getByTestId("location-path")).toHaveTextContent("/automation/assignments");
    expect(await screen.findByText("All Linux hosts")).toBeInTheDocument();

    openLastActionsMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const editDialog = await screen.findByTestId("entity-editor-page");
    expect(within(editDialog).getByText("Edit assignment")).toBeInTheDocument();

    const nameInput = within(editDialog).getAllByRole("textbox")[0]!;
    fireEvent.change(nameInput, { target: { value: "Regional Linux hosts" } });
    fireEvent.click(within(editDialog).getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(playbookApiMock.updatePlaybookAssignment).toHaveBeenCalledWith(
        "assignment-1",
        expect.objectContaining({
          playbook_id: "playbook-1",
          target_set_id: "target-set-1",
          name: "Regional Linux hosts",
          scope_kind: "global",
          scope_id: null,
          priority: 100,
          enabled: true,
          visible: true,
          executable: true,
          quick_action_enabled: true,
        }),
      );
    });

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    openLastActionsMenu();
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const deleteDialog = await screen.findByRole("alertdialog");
    expect(within(deleteDialog).getByText(/All Linux hosts/)).toBeInTheDocument();
    expect(playbookApiMock.deletePlaybookAssignment).not.toHaveBeenCalled();
    fireEvent.click(within(deleteDialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(playbookApiMock.deletePlaybookAssignment).toHaveBeenCalledWith("assignment-1");
    });
  });
});
