import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { ProjectEditor } from "./ProjectEditor";
import { JobTemplateEditor } from "./JobTemplateEditor";
import { ExecutionEnvironmentEditor } from "./ExecutionEnvironmentEditor";
import { ServicePrincipalEditor } from "./ServicePrincipalEditor";
import { TargetSetEditor } from "./TargetSetEditor";
import { ScheduleEditor } from "./ScheduleEditor";
import { emptyExecutionEnvironmentForm, emptyJobTemplateForm, emptyProjectForm, emptyScheduleForm } from "./playbookFormModels";
import { emptySelectorModel } from "./selectorModel";

describe("automation authoring", () => {
  it("changes target mode as a field choice and locks it during submission", () => {
    const onModeChange = vi.fn();
    const editor = (submitting: boolean) => <AllPermissions><TargetSetEditor
      editing={false} name="Targets" onNameChange={vi.fn()} mode="static" onModeChange={onModeChange}
      selectorModel={emptySelectorModel()} onSelectorModelChange={vi.fn()}
      staticHosts={[]} onStaticHostsChange={vi.fn()} connectionTargets={[]} connectionTargetsPending={false}
      onBack={vi.fn()} onSubmit={vi.fn()} submitting={submitting}
    /></AllPermissions>;
    const { rerender } = render(editor(false));
    const dynamic = screen.getByRole("radio", { name: /dynamic/i });
    expect(screen.getByRole("radio", { name: /static/i })).toBeChecked();
    fireEvent.click(dynamic);
    expect(onModeChange).toHaveBeenCalledWith("dynamic");
    onModeChange.mockClear();
    rerender(editor(true));
    expect(dynamic).toBeDisabled();
    fireEvent.click(dynamic);
    expect(onModeChange).not.toHaveBeenCalled();
  });

  for (const family of ["project", "job template", "execution environment", "service principal", "target set", "schedule"] as const) {
    it(`associates the ${family} name and locks changes only during submission`, () => {
      const change = vi.fn();
      const back = vi.fn();
      const base = { onBack: back, onSubmit: vi.fn() };
      const editor = (submitting: boolean) => {
        switch (family) {
          case "project": return <ProjectEditor {...base} submitting={submitting} form={emptyProjectForm()} executionEnvironments={[]} onFormChange={change} />;
          case "job template": return <JobTemplateEditor {...base} submitting={submitting} form={emptyJobTemplateForm()} playbooks={[]} projects={[]} targetSets={[]} executionEnvironments={[]} onFormChange={change} />;
          case "execution environment": return <ExecutionEnvironmentEditor {...base} submitting={submitting} form={emptyExecutionEnvironmentForm()} onFormChange={change} />;
          case "service principal": return <ServicePrincipalEditor {...base} submitting={submitting} name="" onNameChange={change} canReadPolicy={false} onGrantAccess={vi.fn()} />;
          case "target set": return <TargetSetEditor {...base} submitting={submitting} editing={false} name="" onNameChange={change} mode="static" onModeChange={vi.fn()} selectorModel={emptySelectorModel()} onSelectorModelChange={vi.fn()} staticHosts={[]} onStaticHostsChange={vi.fn()} connectionTargets={[]} connectionTargetsPending={false} />;
          case "schedule": return <ScheduleEditor {...base} submitting={submitting} editing={false} form={emptyScheduleForm()} errors={{}} playbooks={[]} targetSets={[]} servicePrincipals={[]} onFormChange={change} spCreating={false} spName="" onSpCreatingChange={vi.fn()} onSpNameChange={vi.fn()} onSpSubmit={vi.fn()} spSubmitting={false} />;
        }
      };
      const tree = (submitting: boolean) => <AllPermissions>{editor(submitting)}</AllPermissions>;
      const { rerender } = render(tree(false));
      const field = screen.getByRole("textbox", { name: /^name$/i });
      expect(field).toBeEnabled();
      fireEvent.change(field, { target: { value: "Draft name" } });
      expect(change).toHaveBeenCalledTimes(1);
      change.mockClear();
      rerender(tree(true));
      expect(field).toBeDisabled();
      fireEvent.change(field, { target: { value: "Ignored while saving" } });
      fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
      expect(change).not.toHaveBeenCalled();
      expect(back).not.toHaveBeenCalled();
      rerender(tree(false));
      expect(field).toBeEnabled();
      fireEvent.change(field, { target: { value: "Retry name" } });
      expect(change).toHaveBeenCalledTimes(1);
    });
  }
});
