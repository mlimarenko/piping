import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";

import {
  createJobTemplate,
  listExecutionEnvironments,
  listPlaybooks,
  listPlaybookTargetSets,
  listProjects,
  type CreateJobTemplateRequest,
} from "@/features/playbooks/api";
import { JobTemplateEditor } from "@/features/playbooks/JobTemplateEditor";
import {
  emptyJobTemplateForm,
  type JobTemplateForm,
} from "@/features/playbooks/playbookFormModels";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/**
 * Routed create page for a playbook job template. Replaces the in-memory
 * full-page swap with a deep-linkable URL.
 */
export default function JobTemplateEditorPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  const [form, setForm] = useState<JobTemplateForm>(emptyJobTemplateForm());
  const [baseline, setBaseline] = useState<JobTemplateForm>(emptyJobTemplateForm());

  const playbooksQuery = useQuery({
    queryKey: ["playbooks", "catalog"],
    queryFn: () => listPlaybooks({ limit: 100, offset: 0 }),
  });
  const projectsQuery = useQuery({
    queryKey: ["playbooks", "projects"],
    queryFn: () => listProjects({ limit: 100, offset: 0 }),
  });
  const targetSetsQuery = useQuery({
    queryKey: ["playbooks", "target-sets", "all"],
    queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }),
  });
  const executionEnvironmentsQuery = useQuery({
    queryKey: ["playbooks", "execution-environments"],
    queryFn: () => listExecutionEnvironments({ limit: 100, offset: 0 }),
  });

  const playbooks = useMemo(() => playbooksQuery.data?.items ?? [], [playbooksQuery.data]);
  const runnablePlaybooks = useMemo(
    () => playbooks.filter((item) => item.playbook.status === "active" && Boolean(item.current_version)),
    [playbooks],
  );
  const projects = useMemo(() => projectsQuery.data?.items ?? [], [projectsQuery.data]);
  const targetSets = useMemo(() => targetSetsQuery.data?.items ?? [], [targetSetsQuery.data]);
  const executionEnvironments = useMemo(
    () => executionEnvironmentsQuery.data?.items ?? [],
    [executionEnvironmentsQuery.data],
  );

  // Seed the form defaults from the first options once the reference data is
  // ready, mirroring the old open handler in PlaybooksPage.
  const [seeded, setSeeded] = useState(false);
  if (!seeded && playbooksQuery.data && targetSetsQuery.data && executionEnvironmentsQuery.data) {
    setSeeded(true);
    const next: JobTemplateForm = {
      ...emptyJobTemplateForm(),
      playbookId: runnablePlaybooks[0]?.playbook.id ?? "",
      targetSetId: targetSets[0]?.id ?? "",
      executionEnvironmentId: executionEnvironments[0]?.id ?? "",
    };
    setForm(next);
    setBaseline(next);
  }

  const back = () => {
    void navigate(PLAYBOOK_TAB_ROUTES["job-templates"]);
  };

  const createMutation = useApiMutation({
    mutationFn: (body: CreateJobTemplateRequest) => createJobTemplate(body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "job-templates"] });
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const submit = () => {
    if (!form.name.trim()) {
      notify("error", t("playbooks.page.errors.jobTemplateRequired"));
      return;
    }
    if (form.sourceKind === "playbook" && !form.playbookId) {
      notify("error", t("playbooks.page.errors.playbookRequired"));
      return;
    }
    if (
      form.sourceKind === "project" &&
      (!form.projectId || !form.playbookPath.trim())
    ) {
      notify("error", t("playbooks.page.errors.jobTemplateProjectRequired"));
      return;
    }
    createMutation.mutate({
      name: form.name.trim(),
      description: form.description.trim() || null,
      playbook_id: form.sourceKind === "playbook" ? form.playbookId : null,
      project_id: form.sourceKind === "project" ? form.projectId : null,
      playbook_path:
        form.sourceKind === "project" ? form.playbookPath.trim() : null,
      inventory_target_set_id: form.targetSetId || null,
      execution_environment_id: form.executionEnvironmentId || null,
      enabled: form.enabled,
    });
  };

  return (
    <JobTemplateEditor
      form={form}
      playbooks={runnablePlaybooks}
      projects={projects}
      targetSets={targetSets}
      executionEnvironments={executionEnvironments}
      onFormChange={setForm}
      onBack={back}
      onSubmit={submit}
      submitting={createMutation.isPending}
      dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
    />
  );
}
