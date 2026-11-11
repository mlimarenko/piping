import { useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate, useParams, useSearchParams } from "react-router";
import { readTargetSetInitialSelector, updateTargetSetReturnFlowTarget } from "./targetSetReturnFlow";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Button } from "@/shared/ui-primitives/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from "@/shared/ui-primitives/dialog";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { SelectorPreview } from "./SelectorPreview";

import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { useConnectionTargetLabels } from "@/features/connections/useConnectionTargetLabels";
import type { ConnectionTarget } from "@/shared/api/types";

import {
  createPlaybookTargetSet,
  getTargetSet,
  updateTargetSet,
  previewTargetSelector,
  type CreateTargetSetRequest,
} from "@/features/playbooks/api";
import { TargetSetEditor } from "@/features/playbooks/TargetSetEditor";
import {
  emptySelectorModel,
  modelToSelector,
  selectorToModel,
  type SelectorModel,
} from "@/features/playbooks/selectorModel";
import {
  selectedHostsSignature,
  selectedHostsToStaticTargetSetMembers,
  staticTargetSetMembersToHosts,
  toSelectedHost,
  type SelectedHost,
} from "@/features/playbooks/hostSelection";
import type { TargetSetMode } from "@/features/playbooks/playbookFormModels";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";
import {
  TARGET_SET_DUPLICATE_PARAM,
  TARGET_SET_TEMPLATE_PARAM,
} from "@/features/playbooks/playbookEditorRoutes";
import { targetSetTemplate } from "@/features/playbooks/targetSetTemplates";

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
  const [searchParams] = useSearchParams();
  const [chosenTemplateKey, setChosenTemplateKey] = useState(searchParams.get(TARGET_SET_TEMPLATE_PARAM) ?? "");
  const template = targetSetTemplate(chosenTemplateKey);
  const duplicateId = searchParams.get(TARGET_SET_DUPLICATE_PARAM);

  const back = (targetSetId?: string) => {
    // Mutation callbacks outlive their observer: never let a departed editor navigate or update a flow.
    if (activeLocationKey.current !== location.key) return;
    try {
      const returned = updateTargetSetReturnFlowTarget(location.state, targetSetId);
      if (returned) {
        void navigate(returned.returnHref, { state: returned.state });
        return;
      }
    } catch {
      // A failed write cannot produce a second, divergent history-state draft.
      notify("error", t("unknownError"));
    }
    void navigate(PLAYBOOK_TAB_ROUTES["target-sets"]);
  };

  const targetSetQuery = useQuery({
    queryKey: ["playbooks", "target-set", id],
    queryFn: () => getTargetSet(id),
    enabled: editing,
  });

  // "Duplicate" opens the create editor pre-filled from the source set. The
  // source is fetched by id so reloading the URL restores the same draft.
  const duplicateQuery = useQuery({
    queryKey: ["playbooks", "target-set", duplicateId],
    queryFn: () => getTargetSet(duplicateId ?? ""),
    enabled: !editing && Boolean(duplicateId),
  });

  const memberIds = useMemo(() => [...new Set(
    (targetSetQuery.data?.members ?? duplicateQuery.data?.members ?? []).map(member => member.target_id),
  )], [targetSetQuery.data, duplicateQuery.data]);
  const memberLabels = useConnectionTargetLabels(memberIds);

  const [name, setName] = useState("");
  const [mode, setMode] = useState<TargetSetMode>(template?.mode ?? "dynamic");
  const [selectorModel, setSelectorModel] = useState<SelectorModel>(
    () => {
      const seed = readTargetSetInitialSelector(location.state);
      return template?.selector ?? (seed && !editing ? selectorToModel(seed) : emptySelectorModel());
    },
  );
  const [staticHosts, setStaticHosts] = useState<SelectedHost[]>([]);
  const displayedHosts = useMemo(() => {
    const byId = new Map<string, ConnectionTarget>((memberLabels.data?.items ?? []).map(target => [target.id, target]));
    return staticHosts.map(host => {
      const target = byId.get(host.id);
      return target ? { ...host, name: target.name, subtitle: toSelectedHost(target).subtitle } : host;
    });
  }, [staticHosts, memberLabels.data]);
  const [baseline, setBaseline] = useState<{
    name: string;
    mode: TargetSetMode;
    selector: SelectorModel;
    hosts: SelectedHost[];
  }>(() => ({
    name: "",
    mode: template?.mode ?? "dynamic",
    selector: template?.selector ?? emptySelectorModel(),
    hosts: [],
  }));

  const [seededId, setSeededId] = useState<string | null>(null);
  if (editing && targetSetQuery.data && seededId !== id) {
    const targetSet = targetSetQuery.data;
    const nextMode: TargetSetMode =
      targetSet.mode === "static" ? "static" : "dynamic";
    const nextSelector = selectorToModel(targetSet.selector);
    const nextHosts = staticTargetSetMembersToHosts(
      targetSet.members,
      memberLabels.data?.items ?? [],
    );
    setSeededId(id);
    setName(targetSet.name);
    setMode(nextMode);
    setSelectorModel(nextSelector);
    setStaticHosts(nextHosts);
    setBaseline({ name: targetSet.name, mode: nextMode, selector: nextSelector, hosts: nextHosts });
  }

  const [seededDuplicateId, setSeededDuplicateId] = useState<string | null>(null);
  if (
    !editing &&
    duplicateQuery.data &&
    seededDuplicateId !== duplicateQuery.data.id
  ) {
    const source = duplicateQuery.data;
    const nextMode: TargetSetMode = source.mode === "static" ? "static" : "dynamic";
    const nextSelector = selectorToModel(source.selector);
    const nextHosts = staticTargetSetMembersToHosts(source.members, memberLabels.data?.items ?? []);
    const copyName = t("playbooks.page.duplicate.defaultName", { name: source.name });
    setSeededDuplicateId(source.id);
    setName(copyName);
    setMode(nextMode);
    setSelectorModel(nextSelector);
    setStaticHosts(nextHosts);
    setBaseline({ name: copyName, mode: nextMode, selector: nextSelector, hosts: nextHosts });
  }

  const dirty = useMemo(
    () =>
      name !== baseline.name ||
      mode !== baseline.mode ||
      JSON.stringify(selectorModel) !== JSON.stringify(baseline.selector) ||
      selectedHostsSignature(staticHosts) !== selectedHostsSignature(baseline.hosts),
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
  const { can } = usePermissions();
  const canPreview = can("READ", "PLAYBOOK_TARGET_SET");
  const selectorJson = JSON.stringify(modelToSelector(selectorModel));
  const debouncedSelector = useDebouncedValue(selectorJson, 400);
  const livePreview = useQuery({
    queryKey: ["playbooks", "unsaved-selector-preview", debouncedSelector],
    queryFn: ({ signal }) => previewTargetSelector(JSON.parse(debouncedSelector) as Record<string, unknown>, signal),
    enabled: mode === "dynamic" && canPreview,
    retry: false,
  });
  const previewPending = selectorJson !== debouncedSelector || livePreview.isFetching || livePreview.isPending;
  const [emptyConfirmation, setEmptyConfirmation] = useState<string | null>(null);

  const submit = (confirmedSelector?: string) => {
    if (submitting) return;
    if (!name.trim()) {
      notify("error", t("playbooks.page.errors.targetSetNameRequired"));
      return;
    }
    if (mode === "dynamic") {
      if (template?.requiredFacet && !selectorModel.facets[template.requiredFacet].length) {
        notify("error", t("playbooks.targetOverrides.requiredFacet", { facet: t(`playbooks.page.selectorBuilder.facets.${template.requiredFacet}`) }));
        return;
      }
      if (!canPreview || previewPending || livePreview.isError || !livePreview.data) {
        notify("error", t("playbooks.selectorPreview.failed"));
        return;
      }
      if (livePreview.data.total === 0 && confirmedSelector !== selectorJson) {
        setEmptyConfirmation(selectorJson);
        return;
      }
    }
    const members = selectedHostsToStaticTargetSetMembers(staticHosts);
    if (mode === "static" && members.length === 0) {
      notify("error", t("playbooks.page.errors.staticHostsRequired"));
      return;
    }
    if (editing) {
      updateMutation.mutate({
        id,
        body: {
          name: name.trim(),
          mode,
          selector: mode === "dynamic" ? modelToSelector(selectorModel) : {},
          // Send the full list even for dynamic mode so switching static ->
          // dynamic -> static does not silently drop the persisted members.
          members,
        },
      });
      return;
    }
    if (mode === "static") {
      createMutation.mutate({
        name: name.trim(),
        mode: "static",
        members,
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
    <>
    <TargetSetEditor
      editing={editing}
      name={name}
      onNameChange={setName}
      mode={mode}
      onModeChange={setMode}
      selectorModel={selectorModel}
      templateKey={chosenTemplateKey}
      focusFacet={template?.requiredFacet}
      onApplyTemplate={key => {
        const selected = targetSetTemplate(key);
        if (!selected) return;
        setChosenTemplateKey(key);
        setMode(selected.mode);
        setSelectorModel(structuredClone(selected.selector ?? emptySelectorModel()));
      }}
      onSelectorModelChange={setSelectorModel}
      staticHosts={displayedHosts}
      onStaticHostsChange={setStaticHosts}
      memberLabelWarning={(memberLabels.data?.unavailable ?? 0) > 0}
      onBack={back}
      onSubmit={() => submit()}
      previewPending={!canPreview || previewPending || livePreview.isError}
      preview={<SelectorPreview data={livePreview.data} pending={previewPending && canPreview} failed={!canPreview || livePreview.isError} retry={() => { void livePreview.refetch(); }} model={selectorModel} />}
      submitting={submitting}
      dirty={dirty}
    />
    <Dialog open={mode === "dynamic" && emptyConfirmation === selectorJson} onOpenChange={open => { if (!open) setEmptyConfirmation(null); }}>
      <DialogContent><DialogHeader><DialogTitle>{t("playbooks.selectorPreview.emptyTitle")}</DialogTitle><DialogDescription>{t("playbooks.selectorPreview.emptyConfirm")}</DialogDescription></DialogHeader><DialogFooter>
        <Button type="button" variant="outline" onClick={() => setEmptyConfirmation(null)}>{t("cancel")}</Button>
        <Button type="button" disabled={submitting || previewPending} onClick={() => { const confirmed = emptyConfirmation; setEmptyConfirmation(null); if (confirmed) submit(confirmed); }}>{t("save")}</Button>
      </DialogFooter></DialogContent>
    </Dialog>
    </>
  );
}
