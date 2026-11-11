import { screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import JobTemplateEditorPage from "./JobTemplateEditorPage";
import { emptyJobTemplateForm } from "./playbookFormModels";
import { jobTemplateReturnFlowDraft } from "./jobTemplateModel";
import { readTargetSetReturnFlow, startTargetSetReturnFlow, TARGET_SET_RETURN_LABELS, updateTargetSetReturnFlowTarget } from "./targetSetReturnFlow";

const api = vi.hoisted(() => ({ list: vi.fn(), get: vi.fn(), update: vi.fn(), create: vi.fn(), revision: vi.fn(), preview: vi.fn() }));
vi.mock("./api", () => ({ listProjects: api.list, listPlaybookTargetSets: api.list, listExecutionEnvironments: api.list, getJobTemplate: api.get, updateJobTemplate: api.update, createJobTemplate: api.create, getProjectRevision: api.revision, previewPlaybookTargetSet: api.preview }));
vi.mock("@/features/automation-secrets/api", () => ({ AUTOMATION_SECRETS_QUERY_KEY: ["automation-secrets"], listAutomationSecrets: vi.fn().mockResolvedValue([]) }));

const projectId = "11111111-1111-4111-8111-111111111111";
const environmentId = "22222222-2222-4222-8222-222222222222";
const createdId = "33333333-3333-4333-8333-333333333333";
const createdSet = { id: createdId, name: "Created set", entity_kind: "connection_target", mode: "static", selector: {}, resolved_count: 4, usage: { job_templates: [], assignments: [], schedules: [] }, created_at: "", updated_at: "" };
const draft = {
  form: {
    ...emptyJobTemplateForm(),
    name: "Edited job",
    projectId,
    playbookPath: '["site.yml"]',
    executionEnvironmentId: environmentId,
    targetSetId: "previous-target",
  },
  baseline: emptyJobTemplateForm(),
};

function mount(state: unknown) {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}><AllPermissions>
    <MemoryRouter initialEntries={[{ pathname: "/playbooks/jobs/new", state }]}><Routes>
      <Route path="/playbooks/jobs/new" element={<JobTemplateEditorPage />} />
      <Route path="*" element={<p>Returned</p>} />
    </Routes></MemoryRouter>
  </AllPermissions></QueryClientProvider>);
}

describe("job editor create-and-return", () => {
  beforeEach(() => {
    window.sessionStorage.clear();
    api.list.mockReset().mockResolvedValue({ items: [createdSet] });
    api.preview.mockReset().mockResolvedValue({ target_set: createdSet, items: [], total: 4, limit: 50, truncated: false });
    api.revision.mockReset().mockResolvedValue({ entries: [] });
  });

  it("restores the edited draft, selects the created set and consumes the flow", async () => {
    const state = startTargetSetReturnFlow("/playbooks/jobs/new", draft, { labelKey: TARGET_SET_RETURN_LABELS.job });
    updateTargetSetReturnFlowTarget(state, createdId);
    mount(state);
    expect(await screen.findByRole("textbox", { name: /^name$/i })).toHaveValue("Edited job");
    expect(screen.getByRole("combobox", { name: /target set/i })).toHaveTextContent("Created set · Fixed set · 4 hosts");
    await waitFor(() => expect(readTargetSetReturnFlow(state, jobTemplateReturnFlowDraft)).toBeNull());
  });

  it("restores the edited draft and its dirty baseline after browsing the target-set list", async () => {
    const state = startTargetSetReturnFlow("/playbooks/jobs/new", draft, { labelKey: TARGET_SET_RETURN_LABELS.job });
    api.list.mockResolvedValue({ items: [{ ...createdSet, id: "previous-target", name: "Previous set" }] });
    mount(state);
    expect(await screen.findByRole("textbox", { name: /^name$/i })).toHaveValue("Edited job");
    expect(screen.getByRole("combobox", { name: /target set/i })).toHaveTextContent("Previous set");
    expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
  });

  it("starts from the empty form without a live flow", async () => {
    mount(undefined);
    expect(await screen.findByRole("textbox", { name: /^name$/i })).toHaveValue("");
    expect(screen.getByRole("combobox", { name: /target set/i })).toHaveTextContent("Select target set");
  });
});
