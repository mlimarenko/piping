import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import type { JobTemplateEditor } from "./JobTemplateEditor";
import type { ScheduleEditor } from "./ScheduleEditor";
import JobTemplateEditorPage from "./JobTemplateEditorPage";
import ScheduleEditorPage from "./ScheduleEditorPage";
import "@/shared/i18n";

const api = vi.hoisted(() => ({ list: vi.fn().mockResolvedValue({ items: [] }), createJob: vi.fn(), createSchedule: vi.fn() }));
vi.mock("./api", () => ({ listProjects: api.list, listPlaybookTargetSets: api.list, listExecutionEnvironments: api.list, listJobTemplates: api.list, createJobTemplate: api.createJob, createPlaybookSchedule: api.createSchedule, getProjectRevision: vi.fn().mockResolvedValue({ entries: [] }) }));
vi.mock("@/features/automation-secrets/api", () => ({ AUTOMATION_SECRETS_QUERY_KEY: ["automation-secrets"], listAutomationSecrets: vi.fn().mockResolvedValue([]) }));
vi.mock("./JobTemplateEditor", () => ({ JobTemplateEditor: (props: ComponentProps<typeof JobTemplateEditor>) => <><button onClick={() => props.onFormChange((prev) => ({ ...prev, name: "Created job", projectId: "project", playbookPath: '["play.yml"]', targetSetId: "target", executionEnvironmentId: "environment" }))}>Fill job</button><button onClick={props.onSave}>Save job</button>{props.referenceError && <p role="alert">{props.referenceError}</p>}</> }));
vi.mock("./ScheduleEditor", () => ({ ScheduleEditor: (props: ComponentProps<typeof ScheduleEditor>) => <><button onClick={() => props.onFormChange((prev) => ({ ...prev, name: "Created schedule", jobTemplateId: "job", selectionMode: "dynamic", targetsJson: '{"custom":true}' }))}>Fill schedule</button><button onClick={props.onSave}>Save schedule</button>{props.referenceError && <p role="alert">{props.referenceError}</p>}</> }));

function mount(path: string) {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AllPermissions><MemoryRouter initialEntries={[path]}><Routes><Route path="/job/new" element={<JobTemplateEditorPage />} /><Route path="/schedule/new" element={<ScheduleEditorPage />} /><Route path="*" element={<p>Returned</p>} /></Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
}

describe("create pages", () => {
  it("posts a generated job request and navigates after the response", async () => {
    api.createJob.mockResolvedValue({ id: "created" });
    mount("/job/new");
    fireEvent.click(await screen.findByRole("button", { name: "Fill job" }));
    fireEvent.click(screen.getByRole("button", { name: "Save job" }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.createJob).toHaveBeenCalledWith(expect.objectContaining({ name: "Created job", playbook_paths: ["play.yml"], inventory: { kind: "managed", target_set_id: "target" } }));
  });
  it("posts a generated dynamic schedule request after filling the draft", async () => {
    api.createSchedule.mockResolvedValue({ id: "created" });
    mount("/schedule/new");
    fireEvent.click(await screen.findByRole("button", { name: "Fill schedule" }));
    fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.createSchedule).toHaveBeenCalledWith(expect.objectContaining({ name: "Created schedule", template_id: "job", targets: { kind: "dynamic", selector: { custom: true } } }));
  });
});
