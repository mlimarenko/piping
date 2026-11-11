import { screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import ScheduleEditorPage from "./ScheduleEditorPage";
import { emptyScheduleForm } from "./playbookFormModels";
import { scheduleReturnFlowDraft } from "./scheduleModel";
import { readTargetSetReturnFlow, startTargetSetReturnFlow, TARGET_SET_RETURN_LABELS, updateTargetSetReturnFlowTarget } from "./targetSetReturnFlow";

const api = vi.hoisted(() => ({ list: vi.fn(), get: vi.fn(), update: vi.fn(), create: vi.fn(), preview: vi.fn() }));
vi.mock("./api", () => ({ listJobTemplates: api.list, listPlaybookTargetSets: api.list, getSchedule: api.get, updatePlaybookSchedule: api.update, createPlaybookSchedule: api.create, previewPlaybookTargetSet: api.preview }));
vi.mock("@/features/connections/api", () => ({ CONNECTION_TARGETS_QUERY_KEY: ["test", "connection-targets"], listConnectionTargets: vi.fn().mockResolvedValue({ items: [] }) }));

const createdId = "33333333-3333-4333-8333-333333333333";
const previousId = "44444444-4444-4444-8444-444444444444";
const createdSet = { id: createdId, name: "Created set", entity_kind: "connection_target", mode: "dynamic", selector: { custom: true }, resolved_count: 2, usage: { job_templates: [], assignments: [], schedules: [] }, created_at: "", updated_at: "" };
const previousSet = { ...createdSet, id: previousId, name: "Previous set", resolved_count: 1 };
const draft = {
  form: {
    ...emptyScheduleForm(),
    name: "Edited schedule",
    jobTemplateId: "job",
    selectionMode: "dynamic" as const,
    targetsJson: '{"custom":true}',
    targetSetId: previousId,
  },
  baseline: emptyScheduleForm(),
};

function mount(state: unknown) {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}><AllPermissions>
    <MemoryRouter initialEntries={[{ pathname: "/playbooks/schedules/new", state }]}><Routes>
      <Route path="/playbooks/schedules/new" element={<ScheduleEditorPage />} />
      <Route path="*" element={<p>Returned</p>} />
    </Routes></MemoryRouter>
  </AllPermissions></QueryClientProvider>);
}

describe("schedule editor create-and-return", () => {
  beforeEach(() => {
    window.sessionStorage.clear();
    api.list.mockReset().mockResolvedValue({ items: [createdSet, previousSet] });
    api.preview.mockReset().mockResolvedValue({ target_set: createdSet, items: [], total: 2, limit: 50, truncated: false });
  });

  it("restores the edited draft, selects the created set and consumes the flow", async () => {
    const state = startTargetSetReturnFlow("/playbooks/schedules/new", draft, { labelKey: TARGET_SET_RETURN_LABELS.schedule });
    updateTargetSetReturnFlowTarget(state, createdId);
    mount(state);
    expect(await screen.findByRole("textbox", { name: /^name$/i })).toHaveValue("Edited schedule");
    expect(screen.getByLabelText("Dynamic selector (JSON)")).toHaveValue('{"custom":true}');
    expect(screen.getByRole("combobox", { name: /dynamic target set/i })).toHaveTextContent("Created set · Dynamic selector · 2 hosts");
    await waitFor(() => expect(readTargetSetReturnFlow(state, scheduleReturnFlowDraft)).toBeNull());
  });

  it("restores the edited draft and its dirty baseline after browsing the target-set list", async () => {
    const state = startTargetSetReturnFlow("/playbooks/schedules/new", draft, { labelKey: TARGET_SET_RETURN_LABELS.schedule });
    mount(state);
    expect(await screen.findByRole("textbox", { name: /^name$/i })).toHaveValue("Edited schedule");
    expect(screen.getByRole("combobox", { name: /dynamic target set/i })).toHaveTextContent("Previous set");
    expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
  });
});
