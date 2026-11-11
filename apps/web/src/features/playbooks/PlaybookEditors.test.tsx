import { fireEvent, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { render } from "@/test/pageChrome";
import { describe, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { ProjectEditor } from "./ProjectEditor";
import { JobTemplateEditor } from "./JobTemplateEditor";
import { ExecutionEnvironmentEditor } from "./ExecutionEnvironmentEditor";
import { TargetSetEditor } from "./TargetSetEditor";
import { ScheduleEditor } from "./ScheduleEditor";
import { emptyExecutionEnvironmentForm, emptyJobTemplateForm, emptyScheduleForm } from "./playbookFormModels";
import { emptyProjectForm } from "./projectFormModel";
import { emptySelectorModel } from "./selectorModel";
import type { ProjectAuthOptions } from "./useProjectAuthOptions";
import type * as ConnectionsApi from "@/features/connections/api";

const emptyAuthOptions: ProjectAuthOptions = { secrets: [], secretsLoading: false, secretsGap: null, sshRecords: [], sshRecordsLoading: false, sshRecordsGap: null };

vi.mock("@/features/connections/api", async (importOriginal) => {
  const actual = await importOriginal<typeof ConnectionsApi>();
  return {
    ...actual,
    listConnectionTargetPage: () =>
      Promise.resolve({ items: [], total: 0, limit: 50, offset: 0 }),
  };
});

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });

function withQuery(node: ReactNode) {
  return <QueryClientProvider client={queryClient}>{node}</QueryClientProvider>;
}

describe("automation authoring", () => {
  it("offers a job save action supplied by its owner", () => {
    render(<AllPermissions><JobTemplateEditor form={{ ...emptyJobTemplateForm(), name: "Job", projectId: "project" }} projects={[]} targetSets={[]} executionEnvironments={[]} onFormChange={vi.fn()} onBack={vi.fn()} onSave={vi.fn()} submitting={false} /></AllPermissions>);
    expect(screen.getByRole("button", { name: /^save$/i })).toBeInTheDocument();
    expect(screen.queryByText(/source playbook/i)).not.toBeInTheDocument();
  });

  it("offers schedule save without inventing occurrence previews", () => {
    render(<AllPermissions><ScheduleEditor form={{ ...emptyScheduleForm(), name: "Nightly", jobTemplateId: "job" }} jobTemplates={[]} targetSets={[]} onFormChange={vi.fn()} onBack={vi.fn()} onSave={vi.fn()} submitting={false} dirty /></AllPermissions>);
    expect(screen.getByRole("button", { name: /^save$/i })).toBeInTheDocument();
    expect(screen.getByText(/^target selection$/i)).toBeInTheDocument();
    expect(screen.queryByText(/next occurrences/i)).not.toBeInTheDocument();
  });

  it("translates schedule validation codes through i18n", () => {
    i18n.addResource("en", "translation", "playbooks.scheduleDraft.errors.invalid", "Localized executor error");
    render(<AllPermissions><ScheduleEditor form={emptyScheduleForm()} jobTemplates={[]} targetSets={[]} onFormChange={vi.fn()} onBack={vi.fn()} submitting={false} dirty /></AllPermissions>);
    expect(screen.getByText("Localized executor error")).toBeInTheDocument();
  });
  it("changes target mode as a field choice and locks it during submission", () => {
    const onModeChange = vi.fn();
    const editor = (submitting: boolean) => <AllPermissions><TargetSetEditor
      editing={false} name="Targets" onNameChange={vi.fn()} mode="static" onModeChange={onModeChange}
      selectorModel={emptySelectorModel()} onSelectorModelChange={vi.fn()}
      staticHosts={[]} onStaticHostsChange={vi.fn()}
      onBack={vi.fn()} onSubmit={vi.fn()} submitting={submitting}
    /></AllPermissions>;
    const { rerender } = render(withQuery(editor(false)));
    const dynamic = screen.getByRole("radio", { name: /dynamic/i });
    expect(screen.getByRole("radio", { name: /static/i })).toBeChecked();
    fireEvent.click(dynamic);
    expect(onModeChange).toHaveBeenCalledWith("dynamic");
    onModeChange.mockClear();
    rerender(withQuery(editor(true)));
    expect(dynamic).toBeDisabled();
    fireEvent.click(dynamic);
    expect(onModeChange).not.toHaveBeenCalled();
  });

  for (const family of ["project", "job template", "execution environment", "target set", "schedule"] as const) {
    it(`associates the ${family} name and locks changes only during submission`, () => {
      const change = vi.fn();
      const back = vi.fn();
      const base = { onBack: back, onSubmit: vi.fn() };
      const editor = (submitting: boolean) => {
        switch (family) {
          case "project": return <ProjectEditor {...base} submitting={submitting} form={emptyProjectForm()} onFormChange={change} authOptions={emptyAuthOptions} />;
          case "job template": return <JobTemplateEditor {...base} submitting={submitting} form={emptyJobTemplateForm()} projects={[]} targetSets={[]} executionEnvironments={[]} onFormChange={change} />;
          case "execution environment": return <ExecutionEnvironmentEditor {...base} submitting={submitting} form={emptyExecutionEnvironmentForm()} onFormChange={change} />;
          case "target set": return <TargetSetEditor {...base} submitting={submitting} editing={false} name="" onNameChange={change} mode="static" onModeChange={vi.fn()} selectorModel={emptySelectorModel()} onSelectorModelChange={vi.fn()} staticHosts={[]} onStaticHostsChange={vi.fn()} />;
          case "schedule": return <ScheduleEditor onBack={back} submitting={submitting} form={emptyScheduleForm()} jobTemplates={[]} targetSets={[]} onFormChange={change} />;
        }
      };
      const tree = (submitting: boolean) => <AllPermissions>{editor(submitting)}</AllPermissions>;
      const { rerender } = render(withQuery(tree(false)));
      const field = screen.getByRole("textbox", { name: /^name$/i });
      expect(field).toBeEnabled();
      fireEvent.change(field, { target: { value: "Draft name" } });
      expect(change).toHaveBeenCalledTimes(1);
      change.mockClear();
      rerender(withQuery(tree(true)));
      expect(field).toBeDisabled();
      fireEvent.change(field, { target: { value: "Ignored while saving" } });
      fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
      expect(change).not.toHaveBeenCalled();
      expect(back).not.toHaveBeenCalled();
      rerender(withQuery(tree(false)));
      expect(field).toBeEnabled();
      fireEvent.change(field, { target: { value: "Retry name" } });
      expect(change).toHaveBeenCalledTimes(1);
    });
  }
});
