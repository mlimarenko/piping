import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";

import {
  createProject,
  listExecutionEnvironments,
  type CreateProjectRequest,
} from "@/features/playbooks/api";
import { ProjectEditor } from "@/features/playbooks/ProjectEditor";
import { emptyProjectForm, type ProjectForm } from "@/features/playbooks/playbookFormModels";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/**
 * Routed create page for a playbook project. Replaces the in-memory full-page
 * swap with a deep-linkable URL.
 */
export default function ProjectEditorPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  const [form, setForm] = useState<ProjectForm>(emptyProjectForm());
  const baseline = emptyProjectForm();

  const executionEnvironmentsQuery = useQuery({
    queryKey: ["playbooks", "execution-environments"],
    queryFn: () => listExecutionEnvironments({ limit: 100, offset: 0 }),
  });
  const executionEnvironments = useMemo(
    () => executionEnvironmentsQuery.data?.items ?? [],
    [executionEnvironmentsQuery.data],
  );

  const back = () => {
    void navigate(PLAYBOOK_TAB_ROUTES.projects);
  };

  const createMutation = useApiMutation({
    mutationFn: (body: CreateProjectRequest) => createProject(body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] });
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const submit = () => {
    if (!form.name.trim()) {
      notify("error", t("playbooks.page.errors.projectRequired"));
      return;
    }
    if (form.scmType === "git" && !form.scmUrl.trim()) {
      notify("error", t("playbooks.page.errors.projectScmUrlRequired"));
      return;
    }
    createMutation.mutate({
      name: form.name.trim(),
      description: form.description.trim() || null,
      scm_type: form.scmType,
      scm_url: form.scmType === "git" ? form.scmUrl.trim() : null,
      scm_branch: form.scmType === "git" ? form.scmBranch.trim() || null : null,
      scm_ref: form.scmRef.trim() || null,
      playbook_base_path: form.playbookBasePath.trim() || null,
      default_execution_environment_id: form.defaultExecutionEnvironmentId || null,
      sync_policy: form.syncPolicy,
    });
  };

  return (
    <ProjectEditor
      form={form}
      executionEnvironments={executionEnvironments}
      onFormChange={setForm}
      onBack={back}
      onSubmit={submit}
      submitting={createMutation.isPending}
      dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
    />
  );
}
