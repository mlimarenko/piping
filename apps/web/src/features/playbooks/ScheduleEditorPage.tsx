import { useMemo, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { playbookReturnHref } from "./playbookDetailRoutes";
import { isUuid } from "@/shared/lib/ids";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Button } from "@/shared/ui-primitives/button";

import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";

import {
  createPlaybookSchedule,
  createPlaybookServicePrincipal,
  getSchedule,
  getPlaybook,
  listPlaybooks,
  listPlaybookServicePrincipals,
  listPlaybookTargetSets,
  updatePlaybookSchedule,
  type CreatePlaybookScheduleRequest,
  type CreatePlaybookServicePrincipalRequest,
  type UpdatePlaybookScheduleRequest,
} from "@/features/playbooks/api";
import { ScheduleEditor } from "@/features/playbooks/ScheduleEditor";
import { emptyScheduleForm, type ScheduleForm } from "@/features/playbooks/playbookFormModels";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/**
 * Routed create/edit page for a playbook schedule. Replaces the in-memory
 * full-page swap with a deep-linkable URL; back/reload restores the editor and
 * the schedule is fetched by id.
 */
export default function ScheduleEditorPage() {
  const { id = "" } = useParams();
  const editing = id !== "";
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const parentId = searchParams.get("playbook");
  const seedPlaybookId = isUuid(parentId) ? parentId : "";
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  const back = () => {
    void navigate(searchParams.has("returnTo") ? playbookReturnHref(searchParams.get("returnTo")) : PLAYBOOK_TAB_ROUTES.schedules);
  };

  const scheduleQuery = useQuery({
    queryKey: ["playbooks", "schedule", id],
    queryFn: () => getSchedule(id),
    enabled: editing,
  });

  const playbooksQuery = useQuery({
    queryKey: ["playbooks", "catalog"],
    queryFn: () => listPlaybooks({ limit: 100, offset: 0 }),
    enabled: !seedPlaybookId,
  });
  const parentQuery = useQuery({
    queryKey: ["playbooks", "detail", seedPlaybookId],
    queryFn: () => getPlaybook(seedPlaybookId),
    enabled: Boolean(seedPlaybookId),
  });
  const targetSetsQuery = useQuery({
    queryKey: ["playbooks", "target-sets", "all"],
    queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }),
  });
  const servicePrincipalsQuery = useQuery({
    queryKey: ["playbooks", "service-principals"],
    queryFn: () => listPlaybookServicePrincipals({ limit: 100, offset: 0 }),
  });

  const playbooks = useMemo(() => seedPlaybookId ? parentQuery.data ? [parentQuery.data] : [] : playbooksQuery.data?.items ?? [], [playbooksQuery.data, parentQuery.data, seedPlaybookId]);
  const targetSets = useMemo(() => targetSetsQuery.data?.items ?? [], [targetSetsQuery.data]);
  const servicePrincipals = useMemo(
    () => servicePrincipalsQuery.data?.items ?? [],
    [servicePrincipalsQuery.data],
  );

  const [form, setForm] = useState<ScheduleForm>(() => ({ ...emptyScheduleForm(), playbookId: seedPlaybookId }));
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [baseline, setBaseline] = useState<ScheduleForm>(() => ({ ...emptyScheduleForm(), playbookId: seedPlaybookId }));
  const [spCreating, setSpCreating] = useState(false);
  const [spName, setSpName] = useState("");

  const [seededId, setSeededId] = useState<string | null>(null);
  if (editing && scheduleQuery.data && seededId !== id) {
    const schedule = scheduleQuery.data;
    const next: ScheduleForm = {
      name: schedule.name,
      playbookId: schedule.playbook_id,
      targetSetId: schedule.target_set_id,
      servicePrincipalId: schedule.service_principal_id,
      cronExpression: schedule.cron_expression,
      timezone: schedule.timezone,
      concurrencyPolicy: schedule.concurrency_policy,
      enabled: schedule.enabled,
    };
    setSeededId(id);
    setForm(next);
    setBaseline(next);
    setErrors({});
    setSpCreating(false);
    setSpName("");
  }

  const dirty = useMemo(
    () => JSON.stringify(form) !== JSON.stringify(baseline),
    [form, baseline],
  );

  const invalidateSchedules = () =>
    queryClient.invalidateQueries({ queryKey: ["playbooks", "schedules"] });

  const createMutation = useApiMutation({
    mutationFn: (body: CreatePlaybookScheduleRequest) => createPlaybookSchedule(body),
    onSuccess: () => {
      void invalidateSchedules();
      back();
    },
    onError: (err: ApiError) => {
      setErrors(err.fields ?? {});
      notify("error", err.message || t("unknownError"));
    },
  });

  const updateMutation = useApiMutation({
    mutationFn: (vars: { id: string; body: UpdatePlaybookScheduleRequest }) =>
      updatePlaybookSchedule(vars.id, vars.body),
    onSuccess: () => {
      void invalidateSchedules();
      back();
    },
    onError: (err: ApiError) => {
      setErrors(err.fields ?? {});
      notify("error", err.message || t("unknownError"));
    },
  });

  const createServicePrincipalMutation = useApiMutation({
    mutationFn: (body: CreatePlaybookServicePrincipalRequest) =>
      createPlaybookServicePrincipal(body),
    onSuccess: (principal) => {
      void queryClient.invalidateQueries({
        queryKey: ["playbooks", "service-principals"],
      });
      setForm((prev) => ({ ...prev, servicePrincipalId: principal.id }));
      setSpCreating(false);
      setSpName("");
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const submit = () => {
    const missing =
      !form.name.trim() ||
      !form.playbookId ||
      !form.targetSetId ||
      !form.cronExpression.trim();
    if (missing) {
      notify("error", t("playbooks.page.errors.scheduleRequired"));
      return;
    }
    if (editing) {
      updateMutation.mutate({
        id,
        body: {
          name: form.name.trim(),
          target_set_id: form.targetSetId,
          cron_expression: form.cronExpression.trim(),
          timezone: form.timezone.trim() || "UTC",
          concurrency_policy: form.concurrencyPolicy,
          misfire_policy: "fire_once",
          enabled: form.enabled,
        },
      });
      return;
    }
    if (!form.servicePrincipalId) {
      notify("error", t("playbooks.page.errors.servicePrincipalRequired"));
      return;
    }
    createMutation.mutate({
      name: form.name.trim(),
      playbook_id: form.playbookId,
      target_set_id: form.targetSetId,
      service_principal_id: form.servicePrincipalId,
      cron_expression: form.cronExpression.trim(),
      timezone: form.timezone.trim() || "UTC",
      concurrency_policy: form.concurrencyPolicy,
      misfire_policy: "fire_once",
      enabled: form.enabled,
    });
  };

  const submitServicePrincipal = () => {
    if (!spName.trim()) {
      notify("error", t("playbooks.page.errors.servicePrincipalNameRequired"));
      return;
    }
    createServicePrincipalMutation.mutate({ name: spName.trim() });
  };

  const awaitingSchedule = editing && !scheduleQuery.data;
  const awaitingParent = !editing && Boolean(seedPlaybookId) && !parentQuery.data;
  if (awaitingSchedule || awaitingParent) {
    const stateQuery = awaitingSchedule ? scheduleQuery : parentQuery;
    return (
      <EntityEditorPage scope="routed"
        title={t(editing ? "playbooks.page.dialogs.schedule.editTitle" : "playbooks.page.dialogs.schedule.title")}
        description={t("playbooks.page.dialogs.schedule.description")}
        onBack={back} backLabel={t("back")} cancelLabel={t("cancel")}
      >
        {stateQuery.isPending ? <output className="flex justify-center p-8">
          <Loader2 className="size-6 animate-spin" aria-label={t("loading")} />
        </output> : <div role="alert" className="flex flex-col items-start gap-4">
          <p>{t(stateQuery.isError && !(stateQuery.error instanceof ApiError && stateQuery.error.status === 404) ? "unknownError" : "notFound.title")}</p>
          {stateQuery.isError && <Button variant="outline" onClick={() => void stateQuery.refetch()}>{t("retry")}</Button>}
        </div>}
      </EntityEditorPage>
    );
  }

  const submitting = createMutation.isPending || updateMutation.isPending;

  return (
    <ScheduleEditor
      editing={editing}
      form={form}
      errors={errors}
      playbooks={playbooks}
      targetSets={targetSets}
      servicePrincipals={servicePrincipals}
      onFormChange={setForm}
      spCreating={spCreating}
      spName={spName}
      onSpCreatingChange={setSpCreating}
      onSpNameChange={setSpName}
      onSpSubmit={submitServicePrincipal}
      spSubmitting={createServicePrincipalMutation.isPending}
      onBack={back}
      onSubmit={submit}
      submitting={submitting}
      dirty={dirty}
    />
  );
}
