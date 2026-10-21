import { useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Layers, Loader2, Plus, Trash2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { RequireCapability } from "@/shared/auth/permissions";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormField from "@/shared/ui-shared/FormField";
import { Switch } from "@/shared/ui-primitives/switch";
import FormSelect from "@/shared/ui-shared/FormSelect";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import EmptyState from "@/shared/ui-shared/EmptyState";
import {
  createMetricSet,
  createMetricSetItem,
  deleteMetricSetItem,
  listMetricQueries,
  listMetricSets,
  updateMetricSet,
  updateMetricSetItem,
  type MetricSetDetail,
  type MetricSetItemEntry,
  type MetricSetListResponse,
  type MetricSetRequest,
} from "@/features/observability/api";
import { METRIC_QUERIES_KEY, METRIC_SETS_KEY, invalidateInspectorMetricCaches } from "./keys";
import { replaceById } from "./metricHelpers";
import {
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityListHref,
  observabilityRecordHref,
} from "./observabilityRoutes";

function slugify(value: string): string {
  return value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

const { base: METRIC_SETS_BASE, view: METRIC_SETS_VIEW } = OBSERVABILITY_ENTITY_ROUTES.metricSets;

const MetricSetForm = ({ item }: { item: MetricSetDetail | null }) => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [name, setName] = useState(item?.name ?? "");
  const [slug, setSlug] = useState(item?.slug ?? "");
  const [slugTouched, setSlugTouched] = useState(!!item);
  const [description, setDescription] = useState(item?.description ?? "");
  const [enabled, setEnabled] = useState(item?.enabled ?? true);
  const [addQueryId, setAddQueryId] = useState("");

  const back = () => {
    if (busy) return;
    void navigate(item ? observabilityRecordHref(METRIC_SETS_BASE, item.id) : observabilityListHref(METRIC_SETS_VIEW));
  };

  const effectiveSlug = slugTouched ? slug : slugify(name);

  const queriesQuery = useQuery({
    queryKey: METRIC_QUERIES_KEY,
    queryFn: () => listMetricQueries(),
    enabled: !!item,
    staleTime: 60_000,
  });
  const metricQueries = useMemo(() => queriesQuery.data?.items ?? [], [queriesQuery.data]);
  const queryName = (id: string) => metricQueries.find((q) => q.id === id)?.name ?? id;

  // Re-read the edited set from the live list cache so item add/remove/toggle
  // mutations (which invalidate METRIC_SETS_KEY) reflect immediately — the
  // `item` prop is only a snapshot from when the page loaded.
  const setsQuery = useQuery({
    queryKey: METRIC_SETS_KEY,
    queryFn: () => listMetricSets(),
    enabled: !!item,
    staleTime: 60_000,
  });
  const liveSet = useMemo<MetricSetDetail | null>(() => {
    if (!item) return null;
    return setsQuery.data?.items.find((s) => s.id === item.id) ?? item;
  }, [item, setsQuery.data]);

  const items = useMemo<MetricSetItemEntry[]>(
    () => [...(liveSet?.items ?? [])].sort((a, b) => a.sort_order - b.sort_order),
    [liveSet?.items],
  );
  const usedQueryIds = useMemo(() => new Set(items.map((i) => i.metric_query_id)), [items]);
  const availableQueries = metricQueries.filter((q) => !usedQueryIds.has(q.id));

  const invalidateSets = () => {
    invalidateInspectorMetricCaches(qc);
    return qc.invalidateQueries({ queryKey: METRIC_SETS_KEY });
  };

  const setMutation = useApiMutation({
    mutationFn: (body: MetricSetRequest) =>
      item ? updateMetricSet(item.id, body) : createMetricSet(body),
    onSuccess: (saved) => {
      qc.setQueryData<MetricSetListResponse>(METRIC_SETS_KEY, (current) => {
        const rows = current?.items ?? [];
        const known = rows.some((row) => row.id === saved.id);
        const items = known ? replaceById(rows, saved) : [...rows, saved];
        return { items, total: known ? (current?.total ?? rows.length) : (current?.total ?? rows.length) + 1 };
      });
      void invalidateSets();
      notify(
        "success",
        item
          ? t("observability.set.updated", { defaultValue: "Set saved" })
          : t("observability.set.created", { defaultValue: "Set created" }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const addItemMutation = useApiMutation({
    mutationFn: (metricQueryId: string) =>
      createMetricSetItem(item!.id, {
        metric_query_id: metricQueryId,
        show_in_inspector: true,
        show_in_list: true,
        show_in_hover: false,
        list_render: "auto",
        sort_order: items.length,
      }),
    onSuccess: async () => {
      await invalidateSets();
      setAddQueryId("");
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const patchItemMutation = useApiMutation({
    mutationFn: ({
      entry,
      patch,
    }: {
      entry: MetricSetItemEntry;
      patch: Partial<
        Pick<MetricSetItemEntry, "show_in_inspector" | "show_in_list" | "show_in_hover" | "list_render">
      >;
    }) =>
      updateMetricSetItem(item!.id, entry.id, {
        metric_query_id: entry.metric_query_id,
        show_in_inspector: patch.show_in_inspector ?? entry.show_in_inspector,
        show_in_list: patch.show_in_list ?? entry.show_in_list,
        show_in_hover: patch.show_in_hover ?? entry.show_in_hover,
        list_render: patch.list_render ?? entry.list_render,
        sort_order: entry.sort_order,
        label_override: entry.label_override,
        unit_override: entry.unit_override,
      }),
    onSuccess: () => invalidateSets(),
    onError: (err: ApiError) => notify("error", err.message),
  });

  const removeItemMutation = useApiMutation({
    mutationFn: (itemId: string) => deleteMetricSetItem(item!.id, itemId),
    onSuccess: () => invalidateSets(),
    onError: (err: ApiError) => notify("error", err.message),
  });

  const busy = setMutation.isPending || addItemMutation.isPending || patchItemMutation.isPending || removeItemMutation.isPending;
  const saveSet = () => {
    if (!canSaveSet) return;
    setMutation.mutate({
      name: name.trim(),
      slug: effectiveSlug,
      description: description.trim() || null,
      enabled,
    }, { onSuccess: (saved) => { void navigate(observabilityRecordHref(METRIC_SETS_BASE, saved.id)); } });
  };

  const canSaveSet = !!name.trim() && !!effectiveSlug && !busy;

  // Only the editable detail fields participate in the unsaved-changes guard:
  // item add/remove/toggle persist immediately, so they must not mark the page
  // dirty (which is why this is passed explicitly, mirroring the editors that
  // own the same EntityEditorPage chrome).
  const baselineDetails = {
    name: item?.name ?? "",
    slug: item?.slug ?? "",
    description: item?.description ?? "",
    enabled: item?.enabled ?? true,
  };
  const dirty =
    JSON.stringify({ name, slug: effectiveSlug, description, enabled }) !==
    JSON.stringify(baselineDetails);

  return (
    <EntityEditorPage scope="routed"
      dirtyMessage={t("editor.unsavedChanges")}
      title={t(item ? "observability.set.editTitle" : "observability.set.createTitle")}
      description={t("observability.set.dialogDescription", {
        defaultValue: "A named bundle of metric queries shown together in inspectors.",
      })}
      onBack={back}
      backLabel={t("back")}
      onCancel={back}
      cancelLabel={t("cancel")}
      onSave={saveSet}
      saveLabel={item ? t("save") : t("create")}
      saving={busy}
      saveDisabled={!canSaveSet}
      dirty={dirty}
    >
      <fieldset disabled={busy} className="grid min-w-0 gap-4">
        <div className="grid gap-3">
          <div className="grid gap-4 sm:grid-cols-2">
            <FormField htmlFor="metric-set-name" label={t("observability.set.name", { defaultValue: "Name" })} required>
              <Input id="metric-set-name" value={name} onChange={(e) => { if (!busy) setName(e.target.value); }} />
            </FormField>
            <FormField htmlFor="metric-set-slug" label={t("observability.set.slug", { defaultValue: "Slug" })} required>
              <Input
                className="font-mono"
                value={effectiveSlug}
                id="metric-set-slug"
                onChange={(e) => {
                  if (busy) return;
                  setSlug(e.target.value);
                  setSlugTouched(true);
                }}
              />
            </FormField>
          </div>
          <FormField htmlFor="metric-set-description" label={t("observability.set.description", { defaultValue: "Description" })}>
            <Input
              value={description}
              id="metric-set-description"
              onChange={(e) => { if (!busy) setDescription(e.target.value); }}
            />
          </FormField>
          <label className="flex items-center justify-between gap-4 py-2">
            <span className="text-sm font-medium">
              {t("observability.set.enabled", { defaultValue: "Enabled" })}
            </span>
            <Switch checked={enabled} onCheckedChange={(value) => { if (!busy) setEnabled(value); }} />
          </label>
        </div>

        {/* Items can only be managed once the set exists (needs an id). */}
        {item ? (
          <div className="grid gap-2 border-t border-border pt-3">
            <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground/80">
              {t("observability.set.items", { defaultValue: "Metrics in this set" })}
            </p>

            <div className="space-y-1.5">
              {items.length === 0 && (
                <p className="rounded-md border border-dashed border-border px-3 py-3 text-center text-xs text-muted-foreground">
                  {t("observability.set.noItems", { defaultValue: "No metrics yet." })}
                </p>
              )}
              {items.map((entry) => {
                // Keep membership writes locked through the cache refresh so
                // the next patch uses the persisted row, not its old snapshot.
                const itemBusy = busy;
                return (
                <div
                  key={entry.id}
                  className="flex items-center gap-2.5 rounded-md border border-input/30 bg-accent px-3 py-2"
                >
                  <span className="w-8 text-center font-mono text-xs text-muted-foreground tabular-nums">
                    {entry.sort_order}
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-xs font-medium">
                      {entry.label_override || entry.metric_query_name}
                    </p>
                    <p className="truncate text-xs text-muted-foreground">
                      {entry.metric_query_slug}
                    </p>
                  </div>
                  <label className="flex items-center gap-1.5">
                    <span className="text-xs text-muted-foreground">
                      {t("observability.set.show", { defaultValue: "Show" })}
                    </span>
                    <Switch
                      checked={entry.show_in_inspector}
                      disabled={itemBusy}
                      onCheckedChange={(v) =>
                        !busy && patchItemMutation.mutate({ entry, patch: { show_in_inspector: v } })
                      }
                    />
                  </label>
                  <label className="flex items-center gap-1.5">
                    <span className="text-xs text-muted-foreground">
                      {t("observability.metric.showInList", { defaultValue: "List" })}
                    </span>
                    <Switch
                      checked={entry.show_in_list}
                      disabled={itemBusy}
                      onCheckedChange={(v) =>
                        !busy && patchItemMutation.mutate({ entry, patch: { show_in_list: v } })
                      }
                    />
                  </label>
                  {entry.show_in_list && (
                    <FormSelect
                      value={entry.list_render || "auto"}
                      disabled={itemBusy}
                      onValueChange={(v) =>
                        !busy && patchItemMutation.mutate({ entry, patch: { list_render: v } })
                      }
                      options={(["auto", "dot", "value", "boolean", "gauge"] as const).map(
                        (type) => ({
                          value: type,
                          label: t(`observability.metric.listRenderTypes.${type}`, {
                            defaultValue: type,
                          }),
                        }),
                      )}
                      triggerClassName="h-8 w-24"
                    />
                  )}
                  <label className="flex items-center gap-1.5">
                    <span className="text-xs text-muted-foreground">
                      {t("observability.metric.showInHover", { defaultValue: "Hover" })}
                    </span>
                    <Switch
                      checked={entry.show_in_hover}
                      disabled={itemBusy}
                      onCheckedChange={(v) =>
                        !busy && patchItemMutation.mutate({ entry, patch: { show_in_hover: v } })
                      }
                    />
                  </label>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 text-destructive hover:text-destructive"
                    aria-label={t("delete", { defaultValue: "Delete" })}
                    disabled={busy}
                    onClick={() => { if (!busy) removeItemMutation.mutate(entry.id); }}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
                );
              })}
            </div>

            <div className="flex items-end gap-2 pt-1">
              <FormField
                htmlFor="metric-set-add-query"
                className="flex-1"
                label={t("observability.set.addMetric", { defaultValue: "Add a metric query" })}
              >
                <EntitySelect
                  id="metric-set-add-query"
                  value={addQueryId}
                  disabled={busy}
                  onValueChange={(value) => { if (!busy) setAddQueryId(value); }}
                  placeholder={
                    queriesQuery.isPending
                      ? t("loading", { defaultValue: "Loading…" })
                      : t("observability.set.pickMetric", { defaultValue: "Pick a metric…" })
                  }
                  searchPlaceholder={t("search")}
                  items={availableQueries.map((q) => ({
                    id: q.id,
                    label: q.name,
                    searchText: q.slug,
                  }))}
                />
              </FormField>
              <Button
                className="gap-1.5"
                disabled={!addQueryId || busy}
                onClick={() => { if (!busy) addItemMutation.mutate(addQueryId); }}
              >
                {addItemMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Plus className="h-3.5 w-3.5" />
                )}
                {t("add", { defaultValue: "Add", count: 1 })}
              </Button>
            </div>
            {addQueryId && (
              <p className="text-xs text-muted-foreground">
                {queryName(addQueryId)}
              </p>
            )}
          </div>
        ) : (
          <p className="rounded-md border border-dashed border-border px-3 py-3 text-center text-xs text-muted-foreground">
            {t("observability.set.createFirst", {
              defaultValue: "Create the set first, then add metrics to it.",
            })}
          </p>
        )}
      </fieldset>
    </EntityEditorPage>
  );
};

/**
 * Routed create/edit page for a metric set: `/observability/sets/new` and
 * `/observability/sets/:id/edit`.
 */
const MetricSetEditor = () => {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery({ queryKey: METRIC_SETS_KEY, queryFn: () => listMetricSets() });
  const item = query.data?.items.find(candidate => candidate.id === id) ?? null;
  if (query.isPending || query.isError || (id && !item)) {
    return <EntityEditorPage scope="routed" width="form"
      title={t(id ? "observability.set.editTitle" : "observability.set.createTitle")}
      onBack={() => { void navigate(observabilityListHref(METRIC_SETS_VIEW)); }}
      backLabel={t("back")} cancelLabel={t("cancel")}>
      {query.isPending ? <output className="flex justify-center p-8"><Loader2 className="size-6 animate-spin" aria-label={t("loading")} /></output>
        : <EmptyState icon={Layers} title={t(query.isError ? "observability.entityLoadFailed" : "observability.entityNotFound")}
          actionLabel={query.isError ? t("retry") : undefined}
          onAction={query.isError ? () => { void query.refetch(); } : undefined} />}
    </EntityEditorPage>;
  }

  return (
    <RequireCapability check={(p) => p.can(id ? "UPDATE" : "CREATE", "MONITORING")}>
      <MetricSetForm key={id ?? "new"} item={item} />
    </RequireCapability>
  );
};

export default MetricSetEditor;
