import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route, useLocation } from "react-router";
import { AllPermissions } from "@/test/permissions";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { TargetSetEditor } from "./TargetSetEditor";
import TargetSetEditorPage from "./TargetSetEditorPage";
import { modelToSelector } from "./selectorModel";
import { TARGET_SET_RETURN_LABELS } from "./targetSetReturnFlow";
import {
  consumeTargetSetReturnFlow,
  readTargetSetReturnFlow,
  startTargetSetReturnFlow,
} from "./targetSetReturnFlow";
import "@/shared/i18n";

const api = vi.hoisted(() => ({ create: vi.fn(), preview: vi.fn() }));
vi.mock("./api", () => ({ createPlaybookTargetSet: api.create, getTargetSet: vi.fn(), updateTargetSet: vi.fn(), previewTargetSelector: api.preview }));
vi.mock("@/features/connections/api", () => ({ getConnectionTarget: vi.fn() }));
vi.mock("./TargetSetEditor", () => ({ TargetSetEditor: (props: ComponentProps<typeof TargetSetEditor>) => <>
  <input aria-label="Target name" value={props.name} onChange={(event) => props.onNameChange(event.target.value)} />
  <button onClick={() => props.onBack()}>Cancel target</button>
  <button disabled={props.previewPending} onClick={props.onSubmit}>Save target</button>
  <button onClick={() => props.onApplyTemplate?.("linuxVm")}>Linux template</button>
  <button onClick={() => props.onSelectorModelChange({ ...props.selectorModel, facets: { ...props.selectorModel.facets, platform: ["33333333-3333-4333-8333-333333333333"] } })}>Choose platform</button>
  <output data-testid="selector">{JSON.stringify(modelToSelector(props.selectorModel))}</output>
</> }));

type Draft = { playbookId: string; targetSetId: string; name: string; note: string };
const draft: Draft = {
  playbookId: "11111111-1111-4111-8111-111111111111",
  targetSetId: "previous",
  name: "Draft name",
  note: "keep every field",
};
const returnHref = "/playbooks/settings/assignments?assignments.enabled=false";

function parseDraft(value: unknown): Draft | null {
  const candidate = value as Partial<Draft> | null;
  return typeof candidate?.playbookId === "string" &&
    typeof candidate.targetSetId === "string" &&
    typeof candidate.name === "string" &&
    typeof candidate.note === "string"
    ? { playbookId: candidate.playbookId, targetSetId: candidate.targetSetId, name: candidate.name, note: candidate.note }
    : null;
}

function Destination() {
  const location = useLocation();
  return <output>{JSON.stringify({ path: location.pathname + location.search, flow: readTargetSetReturnFlow(location.state, parseDraft) })}</output>;
}

function renderEditor(state: unknown) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}><AllPermissions>
    <MemoryRouter initialEntries={[{ pathname: "/playbooks/settings/target-sets/new", state }]}><Routes>
      <Route path="/playbooks/settings/target-sets/new" element={<TargetSetEditorPage />} />
      <Route path="/playbooks/settings/assignments" element={<Destination />} />
      <Route path="/playbooks/settings/target-sets" element={<Destination />} />
    </Routes></MemoryRouter>
  </AllPermissions></QueryClientProvider>);
}

describe("target-set return through the create flow", () => {
  beforeEach(() => {
    window.sessionStorage.clear();
    api.preview.mockResolvedValue({ items: [], total: 1, limit: 50, truncated: false });
    api.create.mockReset();
  });

  it("cancels back to the exact parent/filter URL with the complete draft and no created set", async () => {
    renderEditor(startTargetSetReturnFlow(returnHref, draft, { labelKey: TARGET_SET_RETURN_LABELS.job }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel target" }));
    const result = JSON.parse((await screen.findByRole("status")).textContent);
    expect(result).toEqual({ path: returnHref, flow: { returnHref, targetSetId: "", draft } });
    expect(api.create).not.toHaveBeenCalled();
  });

  it("returns the created target id without dropping draft fields", async () => {
    api.create.mockResolvedValue({ target_set: { id: "new-target" } });
    renderEditor(startTargetSetReturnFlow(returnHref, draft, { labelKey: TARGET_SET_RETURN_LABELS.job }));
    fireEvent.change(screen.getByLabelText("Target name"), { target: { value: "New set" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Save target" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    await waitFor(() => expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ name: "New set" })));
    const result = JSON.parse((await screen.findByRole("status")).textContent);
    expect(result).toEqual({ path: returnHref, flow: { returnHref, targetSetId: "new-target", draft } });
  });

  it("requires an explicit platform for the Linux VM template instead of guessing from a host name", async () => {
    api.create.mockResolvedValue({ target_set: { id: "new-target" } });
    renderEditor(startTargetSetReturnFlow(returnHref, draft));
    fireEvent.change(screen.getByLabelText("Target name"), { target: { value: "Linux selection" } });
    fireEvent.click(screen.getByRole("button", { name: "Linux template" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save target" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    expect(api.create).not.toHaveBeenCalled();
    expect(JSON.parse(screen.getByTestId("selector").textContent ?? "{}")).toMatchObject({ source_kind: ["virtual_machine"], runtime: ["ansible"] });
    fireEvent.click(screen.getByRole("button", { name: "Choose platform" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save target" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    await waitFor(() => expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ selector: expect.objectContaining({ platform_ids: ["33333333-3333-4333-8333-333333333333"] }) })));
  });

  it("ignores a history state without a live flow and falls back to the target-set list", async () => {
    renderEditor({ assignmentDraft: { returnHref: "https://foreign.example", editingId: null, form: draft } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel target" }));
    const result = JSON.parse((await screen.findByRole("status")).textContent);
    expect(result).toEqual({ path: "/playbooks/settings/target-sets", flow: null });
  });

  it("refuses an external return destination stored behind a valid reference", async () => {
    const reference = startTargetSetReturnFlow(returnHref, draft, { labelKey: TARGET_SET_RETURN_LABELS.job });
    const flowId = reference.targetSetReturnFlow.flowId;
    window.sessionStorage.setItem(`piping.target-set-return-flow:${flowId}`, JSON.stringify({ returnHref: "https://foreign.example", targetSetId: "", draft }));
    renderEditor(reference);
    fireEvent.click(screen.getByRole("button", { name: "Cancel target" }));
    const result = JSON.parse((await screen.findByRole("status")).textContent);
    expect(result).toEqual({ path: "/playbooks/settings/target-sets", flow: null });
  });

  it.each(["cancel", "save"])("a mounted editor cannot %s-return to an already consumed reference", async (action) => {
    const state = startTargetSetReturnFlow(returnHref, draft, { labelKey: TARGET_SET_RETURN_LABELS.job });
    renderEditor(state);
    consumeTargetSetReturnFlow(state);
    if (action === "save") {
      api.create.mockResolvedValue({ target_set: { id: "late-target" } });
      fireEvent.change(screen.getByLabelText("Target name"), { target: { value: "New target" } });
      await waitFor(() => expect(screen.getByRole("button", { name: "Save target" })).toBeEnabled());
      fireEvent.click(screen.getByRole("button", { name: "Save target" }));
      await waitFor(() => expect(api.create).toHaveBeenCalled());
      await act(async () => { await Promise.resolve(); });
    } else fireEvent.click(screen.getByRole("button", { name: "Cancel target" }));
    const result = JSON.parse((await screen.findByRole("status")).textContent);
    expect(result).toEqual({ path: "/playbooks/settings/target-sets", flow: null });
    expect(readTargetSetReturnFlow(state, parseDraft)).toBeNull();
  });
});
