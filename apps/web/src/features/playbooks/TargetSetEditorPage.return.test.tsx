import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route, useLocation } from "react-router";
import { AllPermissions } from "@/test/permissions";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { TargetSetEditor } from "./TargetSetEditor";
import TargetSetEditorPage from "./TargetSetEditorPage";
import { consumeAssignmentDraft, readAssignmentDraft, startAssignmentDraft } from "./assignmentDraft";
import "@/shared/i18n";

const api = vi.hoisted(() => ({ create: vi.fn(), connections: vi.fn() }));
vi.mock("./api", () => ({ createPlaybookTargetSet: api.create, getTargetSet: vi.fn(), updateTargetSet: vi.fn() }));
vi.mock("@/features/connections/api", () => ({ listConnectionTargets: api.connections, CONNECTION_TARGETS_QUERY_KEY: ["connections", "targets"] }));
vi.mock("./TargetSetEditor", () => ({ TargetSetEditor: (props: ComponentProps<typeof TargetSetEditor>) => <>
  <input aria-label="Target name" value={props.name} onChange={(event) => props.onNameChange(event.target.value)} />
  <button onClick={() => props.onBack()}>Cancel target</button>
  <button onClick={props.onSubmit}>Save target</button>
</> }));

const form = { playbookId: "11111111-1111-4111-8111-111111111111", targetSetId: "previous", name: "Draft name", scopeKind: "site" as const, scopeId: "site-id", priority: "72", enabled: false, visible: true, executable: false, quickActionEnabled: true };
const returnHref = `/playbooks/${form.playbookId}?section=assignments&assignments.enabled=false`;
function Destination() {
  const location = useLocation();
  return <output>{JSON.stringify({ path: location.pathname + location.search, draft: readAssignmentDraft(location.state) })}</output>;
}
function renderEditor(state: unknown) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}><AllPermissions>
    <MemoryRouter initialEntries={[{ pathname: "/automation/target-sets/new", state }]}><Routes>
      <Route path="/automation/target-sets/new" element={<TargetSetEditorPage />} />
      <Route path="/playbooks/:id" element={<Destination />} />
      <Route path="/automation/target-sets" element={<Destination />} />
    </Routes></MemoryRouter>
  </AllPermissions></QueryClientProvider>);
}
describe("target-set return to assignment", () => {
  beforeEach(() => { api.connections.mockResolvedValue({ items: [] }); api.create.mockReset(); });
  it("cancels back to the exact parent/filter URL with the complete unsaved draft", async () => {
    renderEditor(startAssignmentDraft({ returnHref, editingId: null, form }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel target" }));
    const result = JSON.parse((await screen.findByRole("status")).textContent);
    expect(result).toEqual({ path: returnHref, draft: { returnHref, editingId: null, form } });
    expect(api.create).not.toHaveBeenCalled();
  });
  it("returns the created target id without dropping draft fields or edit identity", async () => {
    const editingId = "33333333-3333-4333-8333-333333333333";
    api.create.mockResolvedValue({ target_set: { id: "new-target" } });
    renderEditor(startAssignmentDraft({ returnHref, editingId, form }));
    fireEvent.change(screen.getByLabelText("Target name"), { target: { value: "New set" } });
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    await waitFor(() => expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ name: "New set" })));
    const result = JSON.parse((await screen.findByRole("status")).textContent);
    expect(result).toEqual({ path: returnHref, draft: { returnHref, editingId, form: { ...form, targetSetId: "new-target" } } });
  });
  it("rejects untrusted external return destinations", async () => {
    renderEditor({ assignmentDraft: { returnHref: "https://foreign.example", editingId: null, form } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel target" }));
    expect(JSON.parse((await screen.findByRole("status")).textContent).path).toBe("/automation/target-sets");
  });
  it.each(["cancel", "save"])("a mounted editor cannot %s-return to an already consumed reference", async (action) => {
    const state = startAssignmentDraft({ returnHref, editingId: null, form });
    renderEditor(state);
    consumeAssignmentDraft(state);
    if (action === "save") {
      api.create.mockResolvedValue({ target_set: { id: "late-target" } });
      fireEvent.change(screen.getByLabelText("Target name"), { target: { value: "New target" } });
      fireEvent.click(screen.getByRole("button", { name: "Save target" }));
      await waitFor(() => expect(api.create).toHaveBeenCalled());
      await act(async () => { await Promise.resolve(); });
    } else fireEvent.click(screen.getByRole("button", { name: "Cancel target" }));
    expect(screen.getByLabelText("Target name")).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(readAssignmentDraft(state)).toBeNull();
  });
});
