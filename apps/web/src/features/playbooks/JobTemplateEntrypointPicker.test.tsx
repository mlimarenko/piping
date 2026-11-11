import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import JobTemplateEditorPage from "./JobTemplateEditorPage";

const api = vi.hoisted(() => ({
  projects: vi.fn().mockResolvedValue({ items: [] }),
  targetSets: vi.fn().mockResolvedValue({ items: [] }),
  environments: vi.fn().mockResolvedValue({ items: [] }),
  revision: vi.fn().mockResolvedValue({ entries: [] }),
  create: vi.fn(),
  get: vi.fn(),
}));
vi.mock("./api", () => ({
  listProjects: api.projects,
  listPlaybookTargetSets: api.targetSets,
  listExecutionEnvironments: api.environments,
  getProjectRevision: api.revision,
  createJobTemplate: api.create,
  getJobTemplate: api.get,
  updateJobTemplate: vi.fn(),
}));
vi.mock("@/features/automation-secrets/api", () => ({ AUTOMATION_SECRETS_QUERY_KEY: ["automation-secrets"], listAutomationSecrets: vi.fn().mockResolvedValue([]) }));

const projectId = "11111111-1111-4111-8111-111111111111";
const project = { id: projectId, name: "Ops", source: { kind: "editable" as const }, current_revision_id: "r1", created_at: "", updated_at: "" };
const targetSet = { id: "t", name: "All hosts", mode: "static" as const, entity_kind: "connection_target", selector: {}, created_at: "", updated_at: "" };
const environment = { id: "e", name: "Runner", image: "runner:1", enabled: true, env: {}, metadata: {}, pull_policy: "if_not_present", runner_group: "default", settings: {}, created_at: "", updated_at: "" };

function mount(edit = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={[edit ? "/job/job-1/edit" : `/job/new?project_id=${projectId}`]}><Routes><Route path="/job/new" element={<JobTemplateEditorPage />} /><Route path="/job/:jobId/edit" element={<JobTemplateEditorPage />} /><Route path="*" element={<p>Returned</p>} /></Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
}

describe("job entrypoint picker", () => {
  beforeEach(() => {
    api.projects.mockResolvedValue({ items: [project] });
    api.targetSets.mockResolvedValue({ items: [targetSet] });
    api.environments.mockResolvedValue({ items: [environment] });
    api.revision.mockResolvedValue({ project_id: projectId, revision_id: "r1", created_at: "", tree_sha256: "hash", entries: [
      { path: "site.yml", content: { kind: "file", bytes: [45, 45], executable: false } },
      { path: "roles/update.yml", content: { kind: "file", bytes: [45, 45], executable: false } },
      { path: "README.md", content: { kind: "file", bytes: [35], executable: false } },
    ] });
  });

  it("adds a project file to playbook_paths and saves without JSON", async () => {
    api.create.mockResolvedValue({ id: "created" });
    mount();
    fireEvent.change(await screen.findByRole("textbox", { name: /^name$/i }), { target: { value: "Nightly deploy" } });
    await waitFor(() => expect(api.revision).toHaveBeenCalledWith(projectId, "r1"));

    fireEvent.click(screen.getByRole("combobox", { name: /entrypoints/i }));
    fireEvent.click(await screen.findByRole("option", { name: "roles/update.yml" }));
    expect(screen.getByText("roles/update.yml")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("combobox", { name: /target set/i }));
    fireEvent.click(await screen.findByRole("option", { name: /all hosts/i }));
    fireEvent.click(screen.getByRole("combobox", { name: /execution environment/i }));
    fireEvent.click(await screen.findByRole("option", { name: /runner/i }));

    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(api.create).toHaveBeenCalled());
    expect(api.create).toHaveBeenCalledWith(expect.objectContaining({ name: "Nightly deploy", project_id: projectId, playbook_paths: ["roles/update.yml"] }));
  });

  it("shows the entrypoint error at the field when no path is selected", async () => {
    mount();
    await screen.findByRole("textbox", { name: /^name$/i });
    await waitFor(() => expect(api.revision).toHaveBeenCalled());
    fireEvent.change(screen.getByRole("textbox", { name: /^name$/i }), { target: { value: "Draft" } });
    expect(screen.getByText("Entrypoints must be project-relative paths without parent traversal.")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: /entrypoints/i })).toBeInTheDocument();
  });

  it("loads entrypoints from the selected fixed revision instead of the current project head", async () => {
    api.revision.mockImplementation((_projectId: string, revisionId: string) => Promise.resolve({
      project_id: projectId, revision_id: revisionId, created_at: "", tree_sha256: "hash",
      entries: [{ path: revisionId === "older" ? "pinned.yml" : "current.yml", content: { kind: "file", bytes: [45, 45], executable: false } }],
    }));
    api.get.mockResolvedValue({ id: "job-1", name: "Pinned job", project_id: projectId, revision: { kind: "fixed", revision_id: "older" }, playbook_paths: [], workdir: ".", inventory: { kind: "native", sources: ["inventory.yml"] }, execution_environment_id: "e", options: { argv: [], env: {}, extra_vars: {}, secret_refs: {}, timeout_seconds: 60 } });
    mount(true);
    expect(await screen.findByDisplayValue("older", {}, { timeout: 1000 })).toBeInTheDocument();
    await waitFor(() => expect(api.revision).toHaveBeenCalledWith(projectId, "older"));
    fireEvent.click(screen.getByRole("combobox", { name: /entrypoints/i }));
    expect(await screen.findByRole("option", { name: "pinned.yml" }, { timeout: 1000 })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "current.yml" })).not.toBeInTheDocument();
  });
});
