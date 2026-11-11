import { useState } from "react";
import { useNavigate } from "react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { useApiMutation } from "@/shared/api/hooks";
import { notify } from "@/shared/lib/notifications";
import { usePermissions } from "@/shared/auth/permissionsContext";

import {
  createProject,
  type CreateProjectRequest,
} from "@/features/playbooks/api";
import { ProjectEditor } from "@/features/playbooks/ProjectEditor";
import { emptyProjectForm, projectSource, validProjectForm, type ProjectForm } from "@/features/playbooks/projectFormModel";
import { useProjectAuthOptions } from "@/features/playbooks/useProjectAuthOptions";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";
import { projectFilesHref } from "@/features/playbooks/projectRoutes";

/**
 * Routed create page for a playbook project. Replaces the in-memory full-page
 * swap with a deep-linkable URL.
 */
export default function ProjectEditorPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const authOptions = useProjectAuthOptions();

  const [form, setForm] = useState<ProjectForm>(emptyProjectForm());
  const baseline = emptyProjectForm();

  const back = () => {
    void navigate(PLAYBOOK_TAB_ROUTES.projects);
  };

  const createMutation = useApiMutation({
    mutationFn: (body: CreateProjectRequest) => createProject(body),
    onSuccess: (project) => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] });
      void navigate(projectFilesHref(project.id));
    },
    onError: () => notify("error", t("playbooks.page.projects.workspace.createFailed", { defaultValue: "Could not create the project. Your changes are preserved." })),
  });

  const submit = () => {
    if (!can("CREATE", "PLAYBOOK_PROJECT")) return;
    if (!validProjectForm(form)) {
      notify("error", t("playbooks.page.errors.projectRequired"));
      return;
    }
    createMutation.mutate({
      name: form.name.trim(),
      description: form.description.trim() || null,
      source: projectSource(form),
    });
  };

  return (
    <ProjectEditor
      form={form}
      onFormChange={setForm}
      onBack={back}
      onSubmit={submit}
      submitting={createMutation.isPending}
      canCreate={can("CREATE", "PLAYBOOK_PROJECT")}
      authOptions={authOptions}
      dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
    />
  );
}
