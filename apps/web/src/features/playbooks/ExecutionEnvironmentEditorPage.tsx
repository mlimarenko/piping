import { useState } from "react";
import { useNavigate } from "react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";

import {
  createExecutionEnvironment,
  type CreateExecutionEnvironmentRequest,
} from "@/features/playbooks/api";
import { ExecutionEnvironmentEditor } from "@/features/playbooks/ExecutionEnvironmentEditor";
import {
  emptyExecutionEnvironmentForm,
  type ExecutionEnvironmentForm,
} from "@/features/playbooks/playbookFormModels";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/**
 * Routed create page for a playbook execution environment. Replaces the
 * in-memory full-page swap with a deep-linkable URL.
 */
export default function ExecutionEnvironmentEditorPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  const [form, setForm] = useState<ExecutionEnvironmentForm>(emptyExecutionEnvironmentForm());
  const baseline = emptyExecutionEnvironmentForm();

  const back = () => {
    void navigate(PLAYBOOK_TAB_ROUTES["execution-environments"]);
  };

  const createMutation = useApiMutation({
    mutationFn: (body: CreateExecutionEnvironmentRequest) => createExecutionEnvironment(body),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: ["playbooks", "execution-environments"],
      });
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const submit = () => {
    if (!form.name.trim() || !form.image.trim()) {
      notify("error", t("playbooks.page.errors.executionEnvironmentRequired"));
      return;
    }
    createMutation.mutate({
      name: form.name.trim(),
      image: form.image.trim(),
      description: form.description.trim() || null,
      pull_policy: form.pullPolicy,
      runner_group: form.runnerGroup.trim() || "default",
      enabled: form.enabled,
    });
  };

  return (
    <ExecutionEnvironmentEditor
      form={form}
      onFormChange={setForm}
      onBack={back}
      onSubmit={submit}
      submitting={createMutation.isPending}
      dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
    />
  );
}
