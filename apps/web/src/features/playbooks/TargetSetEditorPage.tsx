import { useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router";
import { isAssignmentDraftReference, updateAssignmentDraftTarget } from "./assignmentDraft";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Button } from "@/shared/ui-primitives/button";

import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { listConnectionTargets, CONNECTION_TARGETS_QUERY_KEY } from "@/features/connections/api";
import type { ConnectionTarget } from "@/shared/api/types";

import {
  createPlaybookTargetSet,
  getTargetSet,
  updateTargetSet,
  type CreateTargetSetRequest,
} from "@/features/playbooks/api";
import { TargetSetEditor } from "@/features/playbooks/TargetSetEditor";
import {
  emptySelectorModel,
  modelToSelector,
  selectorToModel,
  type SelectorModel,
} from "@/features/playbooks/selectorModel";
import type { SelectedHost } from "@/features/playbooks/hostSelection";
import type { TargetSetMode } from "@/features/playbooks/playbookFormModels";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/**
 * Routed create/edit page for a playbook target set. Replaces the in-memory
 * full-page swap in PlaybooksPage with a deep-linkable URL, so back/reload
 * restores the editor and the target set is fetched by id instead of held in
 * page state.
 */
export default function TargetSetEditorPage() {
  const { id = "" } = useParams();
  const editing = id !== "";
  const navigate = useNavigate();
  const location = useLocation();
  const activeLocationKey = useRef<string | null>(location.key);
  useEffect(() => {
    activeLocationKey.current = location.key;
    return () => { activeLocationKey.current = null; };
  }, [location.key]);
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  const back = (targetSetId?: string) => {
    // Mutation callbacks outlive their observer: never let a departed editor navigate or update a flow.
    if (activeLocationKey.current !== location.key) return;
    if (isAssignmentDraftReference(location.state)) {
      try {
        const returned = updateAssignmentDraftTarget(location.state, targetSetId);
        if (returned) void navigate(returned.returnHref, { state: returned.state });
      } catch {
        // A failed write cannot produce a second, divergent history-state draft.
        notify("error", t("unknownError"));
      }
      return;
    }
    void navigate(PLAYBOOK_TAB_ROUTES["target-sets"]);
  };

  const targetSetQuery = useQuery({
    queryKey: ["playbooks", "target-set", id],
    queryFn: () => getTargetSet(id),
    enabled: editing,
  });

  const connectionTargetsQuery = useQuery({
    queryKey: CONNECTION_TARGETS_QUERY_KEY,
    queryFn: () => listConnectionTargets(),
    staleTime: 60_000,
  });
  const connectionTargets = useMemo<ConnectionTarget[]>(
    () => connectionTargetsQuery.data?.items ?? [],
    [connectionTargetsQuery.data],
  );

  const [name, setName] = useState("");
  const [mode, setMode] = useState<TargetSetMode>("dynamic");
  const [selectorModel, setSelectorModel] = useState<SelectorModel>(emptySelectorModel());
  const [staticHosts, setStaticHosts] = useState<SelectedHost[]>([]);
  const [baseline, setBaseline] = useState<{
    name: string;
    mode: TargetSetMode;
    selector: SelectorModel;
    hosts: SelectedHost[];
  }>({ name: "", mode: "dynamic", selector: emptySelectorModel(), hosts: [] });

  // Seed the form once the persisted target set arrives (edit mode only).
  const [seededId, setSeededId] = useState<string | null>(null);
  if (editing && targetSetQuery.data && seededId !== id) {
    const targetSet = targetSetQuery.data;
    const nextMode: TargetSetMode =
      targetSet.mode === "static" ? "static" : "dynamic";
    const nextSelector = selectorToModel(targetSet.selector);
    setSeededId(id);
    setName(targetSet.name);
    setMode(nextMode);
    setSelectorModel(nextSelector);
    setStaticHosts([]);
    setBaseline({ name: targetSet.name, mode: nextMode, selector: nextSelector, hosts: [] });
  }

  const dirty = useMemo(
    () =>
      JSON.stringify({ name, mode, selector: selectorModel, hosts: staticHosts }) !==
      JSON.stringify(baseline),
    [name, mode, selectorModel, staticHosts, baseline],
  );

  const createMutation = useApiMutation({
    mutationFn: (body: CreateTargetSetRequest) => createPlaybookTargetSet(body),
    onSuccess: (created) => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "target-sets"] });
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "target-sets", "all"] });
      back(created.target_set.id);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const updateMutation = useApiMutation({
    mutationFn: ({ id: targetSetId, body }: { id: string; body: CreateTargetSetRequest }) =>
      updateTargetSet(targetSetId, body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "target-sets"] });
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "target-sets", "all"] });
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const submitting = createMutation.isPending || updateMutation.isPending;

  const submit = () => {
    if (submitting) return;
    if (!name.trim()) {
      notify("error", t("playbooks.page.errors.targetSetNameRequired"));
      return;
    }
    if (editing) {
      updateMutation.mutate({
        id,
        body: {
          name: name.trim(),
          mode,
          selector: mode === "dynamic" ? modelToSelector(selectorModel) : {},
        },
      });
      return;
    }
    if (mode === "static") {
      if (staticHosts.length === 0) {
        notify("error", t("playbooks.page.errors.staticHostsRequired"));
        return;
      }
      createMutation.mutate({
        name: name.trim(),
        mode: "static",
        members: staticHosts.map((host) => ({
          target_id: host.id,
          target_kind: "connection_target",
          source_id: host.sourceId,
          source_kind: host.sourceKind,
        })),
      });
      return;
    }
    createMutation.mutate({
      name: name.trim(),
      mode: "dynamic",
      selector: modelToSelector(selectorModel),
    });
  };

  if (editing && !targetSetQuery.data) {
    return (
      <EntityEditorPage scope="routed"
        title={t("playbooks.page.dialogs.targetSet.editTitle")}
        description={t("playbooks.page.dialogs.targetSet.description")}
        onBack={() => back()} backLabel={t("back")} cancelLabel={t("cancel")}
      >
        {targetSetQuery.isPending ? <output className="flex justify-center p-8">
          <Loader2 className="size-6 animate-spin" aria-label={t("loading")} />
        </output> : <div role="alert" className="flex flex-col items-start gap-4">
          <p>{t(targetSetQuery.isError && !(targetSetQuery.error instanceof ApiError && targetSetQuery.error.status === 404) ? "unknownError" : "notFound.title")}</p>
          {targetSetQuery.isError && <Button variant="outline" onClick={() => void targetSetQuery.refetch()}>{t("retry")}</Button>}
        </div>}
      </EntityEditorPage>
    );
  }

  return (
    <TargetSetEditor
      editing={editing}
      name={name}
      onNameChange={setName}
      mode={mode}
      onModeChange={setMode}
      selectorModel={selectorModel}
      onSelectorModelChange={setSelectorModel}
      staticHosts={staticHosts}
      onStaticHostsChange={setStaticHosts}
      connectionTargets={connectionTargets}
      connectionTargetsPending={connectionTargetsQuery.isPending}
      onBack={back}
      onSubmit={submit}
      submitting={submitting}
      dirty={dirty}
    />
  );
}
