import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import JobTemplateEditorPage from "./JobTemplateEditorPage";

const api = vi.hoisted(() => ({ list: vi.fn(), get: vi.fn(), update: vi.fn(), create: vi.fn(), revision: vi.fn().mockResolvedValue({ entries: [] }) }));
vi.mock("./api", () => ({ listProjects: api.list, listPlaybookTargetSets: api.list, listExecutionEnvironments: api.list, getJobTemplate: api.get, updateJobTemplate: api.update, createJobTemplate: api.create, getProjectRevision: api.revision }));
vi.mock("@/features/automation-secrets/api", () => ({ AUTOMATION_SECRETS_QUERY_KEY: ["automation-secrets"], listAutomationSecrets: vi.fn().mockResolvedValue([]) }));
const secretId = ["11111111","1111","4111","8111","111111111111"].join("-");
const job = { id: "job", name: "Saved job", project_id: "project", workdir: ".", playbook_paths: ["site.yml"], revision: { kind: "current" }, inventory: { kind: "managed", target_set_id: "targets" }, execution_environment_id: "environment", options: { argv: [], extra_vars: {}, env: {}, secret_refs: { DB_PASSWORD: secretId }, timeout_seconds: 60 } };

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={["/playbooks/jobs/job/edit"]}><Routes><Route path="/playbooks/jobs/:jobId/edit" element={<JobTemplateEditorPage />} /><Route path="*" element={<p>Returned</p>} /></Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
}

describe("job editor persistence", () => {
  it("keeps the draft and secret references after a failed update", async () => {
    api.list.mockResolvedValue({ items: [] }); api.get.mockResolvedValue(job); api.update.mockRejectedValue(new Error("Save denied"));
    mount();
    const name = await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.change(name, { target: { value: "Edited job" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/could not save the job/i);
    expect(screen.queryByText("Save denied")).not.toBeInTheDocument();
    expect(name).toHaveValue("Edited job");
    expect(api.update).toHaveBeenCalledWith("job", expect.objectContaining({ name: "Edited job", options: expect.objectContaining({ secret_refs: { DB_PASSWORD: secretId } }) }));
  });
  it("navigates only after the update returns a job", async () => {
    api.list.mockResolvedValue({ items: [] }); api.get.mockResolvedValue(job); api.update.mockReset().mockResolvedValue(job);
    mount();
    await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.update).toHaveBeenCalledOnce();
  });
});
