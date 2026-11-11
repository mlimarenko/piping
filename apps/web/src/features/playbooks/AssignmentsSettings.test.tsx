import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { updateTargetSetReturnFlowTarget } from "./targetSetReturnFlow";
import { AssignmentsSettings } from "./AssignmentsSettings";
import type { AssignmentForm } from "./assignmentFormModel";

const api = vi.hoisted(() => ({ listPlaybookAssignments: vi.fn(), listJobTemplates: vi.fn(), listPlaybookTargetSets: vi.fn(), previewPlaybookTargetSet: vi.fn(), createPlaybookAssignment: vi.fn(), updatePlaybookAssignment: vi.fn(), deletePlaybookAssignment: vi.fn() }));
vi.mock("./api", () => api);
vi.mock("./usePlaybookScopeEntities", () => ({ usePlaybookScopeEntities: () => ({ optionsByScope: {}, resolveName: () => "", isPending: false }) }));
vi.mock("./AssignmentsPanel", () => ({ AssignmentsPanel: ({ assignments, onEdit, onDelete }: { assignments: { id: string }[]; onEdit: (item: unknown) => void; onDelete: (item: unknown) => void }) => <div>{assignments.map(item => <div key={item.id}><button onClick={() => onEdit(item)}>Edit assignment</button><button onClick={() => onDelete(item)}>Delete assignment</button></div>)}</div> }));
vi.mock("./AssignmentDialog", () => ({ AssignmentDialog: ({ open, form, onCreateTargetSet, onSubmit, onFormChange, error }: { open: boolean; form: AssignmentForm; onCreateTargetSet?: () => void; onSubmit: () => void; onFormChange: (form: AssignmentForm) => void; error?: string }) => open ? <div><output data-testid="dialog-target">{form.targetSetId}</output><output data-testid="dialog-job">{form.jobTemplateId}</output>{onCreateTargetSet && <button onClick={onCreateTargetSet}>New target set</button>}<button onClick={() => onFormChange({ jobTemplateId: jobId, targetSetId: targetId, name: "Changed", scopeKind: "global", scopeId: "", priority: "3", enabled: true, visible: true, executable: true, quickActionEnabled: false })}>Fill assignment</button><button onClick={() => onFormChange({ jobTemplateId: jobId, targetSetId: targetId, name: "", scopeKind: "global", scopeId: "", priority: "3", enabled: true, visible: true, executable: true, quickActionEnabled: false })}>Clear name</button><button onClick={onSubmit}>Save assignment</button>{error && <div role="alert">{error}</div>}</div> : null }));

const jobId = "11111111-1111-4111-8111-111111111111";
const targetId = "22222222-2222-4222-8222-222222222222";
const row = { id: "assignment", job_template_id: jobId, target_set_id: targetId, name: "Changed", scope_kind: "global", scope_id: null, priority: 3, enabled: true, visible: true, executable: true, quick_action_enabled: false };
const body = { job_template_id: jobId, target_set_id: targetId, name: "Changed", scope_kind: "global", scope_id: null, priority: 3, enabled: true, visible: true, executable: true, quick_action_enabled: false };

function Probe() {
  const location = useLocation();
  const navigate = useNavigate();
  return <>
    <output data-testid="route-location">{location.pathname}{location.search}</output>
    <output data-testid="flow-id">{(location.state as { targetSetReturnFlow?: { flowId?: string } } | null)?.targetSetReturnFlow?.flowId ?? ""}</output>
    <button onClick={() => { const returned = updateTargetSetReturnFlowTarget(location.state, "created-target"); if (returned) void navigate(returned.returnHref, { state: returned.state }); }}>Return from target set</button>
  </>;
}

function setup() {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}><AllPermissions><MemoryRouter initialEntries={["/playbooks/settings/assignments"]}><Routes><Route path="/playbooks/settings/assignments" element={<AssignmentsSettings />} /><Route path="*" element={null} /></Routes><Probe /></MemoryRouter></AllPermissions></QueryClientProvider>);
}

describe("assignment settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.sessionStorage.clear();
    api.listPlaybookAssignments.mockResolvedValue({ items: [row], total: 1 });
    api.listJobTemplates.mockResolvedValue({ items: [{ id: jobId, name: "Job" }] });
    api.listPlaybookTargetSets.mockResolvedValue({ items: [{ id: targetId, name: "Targets", mode: "static", selector: {}, resolved_count: 1 }] });
    api.previewPlaybookTargetSet.mockResolvedValue({ total: 1, items: [] });
    api.createPlaybookAssignment.mockResolvedValue(row);
    api.updatePlaybookAssignment.mockResolvedValue(row);
    api.deletePlaybookAssignment.mockResolvedValue(undefined);
  });
  it("creates and updates with job_template_id and consumes the saved response", async () => {
    setup();
    await waitFor(() => expect(screen.getByRole("button", { name: "Create" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    fireEvent.click(screen.getByRole("button", { name: "Fill assignment" }));
    fireEvent.click(screen.getByRole("button", { name: "Save assignment" }));
    await waitFor(() => expect(api.createPlaybookAssignment).toHaveBeenCalledWith(body));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Save assignment" })).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Edit assignment" }));
    fireEvent.click(screen.getByRole("button", { name: "Fill assignment" }));
    fireEvent.click(screen.getByRole("button", { name: "Save assignment" }));
    await waitFor(() => expect(api.updatePlaybookAssignment).toHaveBeenCalledWith("assignment", body));
  });
  it("keeps the form open for rejected or partially applied saves", async () => {
    api.updatePlaybookAssignment.mockResolvedValue({ ...row, name: "unchanged" });
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "Edit assignment" }));
    fireEvent.click(screen.getByRole("button", { name: "Save assignment" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/did not apply/i);
    expect(screen.getByRole("button", { name: "Save assignment" })).toBeInTheDocument();
  });
  it("shows a failed save without closing the editor", async () => {
    api.updatePlaybookAssignment.mockRejectedValue(new Error("rejected"));
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "Edit assignment" }));
    fireEvent.click(screen.getByRole("button", { name: "Save assignment" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save assignment" })).toBeInTheDocument();
  });
  it("sends a cleared name as null and accepts only the returned value", async () => {
    api.updatePlaybookAssignment.mockResolvedValueOnce(row).mockResolvedValueOnce({ ...row, name: null });
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "Edit assignment" }));
    fireEvent.click(screen.getByRole("button", { name: "Clear name" }));
    fireEvent.click(screen.getByRole("button", { name: "Save assignment" }));
    await waitFor(() => expect(api.updatePlaybookAssignment).toHaveBeenCalledWith("assignment", { ...body, name: null }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/did not apply/i);
    expect(screen.getByRole("button", { name: "Save assignment" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save assignment" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Save assignment" })).not.toBeInTheDocument());
  });
  it("creates a target set from the dialog and reopens it with the created set", async () => {
    setup();
    await waitFor(() => expect(screen.getByRole("button", { name: "Create" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    fireEvent.click(screen.getByRole("button", { name: "Fill assignment" }));
    expect(screen.getByTestId("dialog-target")).toHaveTextContent(targetId);
    fireEvent.click(screen.getByRole("button", { name: "New target set" }));
    await waitFor(() => expect(screen.getByTestId("route-location")).toHaveTextContent("/playbooks/settings/target-sets/new"));
    const flowId = screen.getByTestId("flow-id").textContent ?? "";
    expect(flowId).not.toBe("");
    expect(screen.queryByTestId("dialog-target")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Return from target set" }));
    await waitFor(() => expect(screen.getByTestId("route-location")).toHaveTextContent("/playbooks/settings/assignments"));
    expect(await screen.findByTestId("dialog-target")).toHaveTextContent("created-target");
    expect(screen.getByTestId("dialog-job")).toHaveTextContent(jobId);
    expect(window.sessionStorage.getItem(`piping.target-set-return-flow:${flowId}`)).toBeNull();
  });
  it("deletes only after confirmation", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "Delete assignment" }));
    expect(api.deletePlaybookAssignment).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    await waitFor(() => expect(api.deletePlaybookAssignment).toHaveBeenCalledWith("assignment"));
  }, 15000);
});
