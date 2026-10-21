import type { ReactNode } from "react";
import { useParams } from "react-router";
import PageState from "@/shared/ui-shared/PageState";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Layers, Loader2, Pencil, Trash2 } from "lucide-react";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import { deleteMetricSet, listMetricSets } from "./api";
import { METRIC_SETS_KEY, invalidateInspectorMetricCaches } from "./keys";
import { useObservabilityEntityDelete } from "./useObservabilityEntityDelete";
import {
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityEditHref,
  observabilityListHref,
} from "./observabilityRoutes";

const { base, view } = OBSERVABILITY_ENTITY_ROUTES.metricSets;

/** Read-only detail route `/observability/sets/:id`. */
export default function MetricSetDetail() {
  const { id } = useParams();
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const query = useQuery({ queryKey: METRIC_SETS_KEY, queryFn: () => listMetricSets() });
  const listHref = observabilityListHref(view);
  const { deleteError, isDeleting, deleteConfirm } = useObservabilityEntityDelete({
    deleteFn: deleteMetricSet,
    invalidate: () => {
      void qc.invalidateQueries({ queryKey: METRIC_SETS_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    listHref,
  });
  const renderState = (content: ReactNode) => <PageState title={t("observability.title")} icon={Layers} backHref={listHref}>{content}</PageState>;
  if (query.isPending) return renderState(<output className="flex justify-center p-8"><Loader2 className="size-6 animate-spin" aria-label={t("loading")} /></output>);
  if (query.isError) return renderState(<EmptyState icon={Layers} title={t("observability.entityLoadFailed")} actionLabel={t("retry")} onAction={() => { void query.refetch(); }} />);
  const item = query.data.items.find(candidate => candidate.id === id);
  if (!item) return renderState(<EmptyState icon={Layers} title={t("observability.entityNotFound")} />);

  const items = [...item.items].sort((a, b) => a.sort_order - b.sort_order);
  return (
    <EntityDetail
      icon={Layers}
      title={item.name}
      description={item.slug}
      backHref={listHref}
      status={{ tone: item.enabled ? "positive" : "neutral", label: t(item.enabled ? "enabled" : "disabled") }}
      error={deleteError}
      fields={[
        { label: t("observability.set.slug"), value: item.slug },
        { label: t("observability.set.description"), value: item.description ?? "" },
        { label: t("observability.set.metrics"), value: String(items.length) },
      ]}
      actions={[
        {
          key: "edit",
          label: t("edit"),
          icon: Pencil,
          href: observabilityEditHref(base, item.id),
          allowed: can("UPDATE", "MONITORING"),
        },
        {
          key: "delete",
          label: t("delete"),
          icon: Trash2,
          variant: "outline",
          allowed: can("DELETE", "MONITORING"),
          disabled: isDeleting,
          confirm: deleteConfirm(item.name, item.id),
        },
      ]}
    >
      <div className="mt-4 border-t border-border pt-4">
        <h2 className="text-sm font-semibold">
          {t("observability.set.items", { defaultValue: "Metrics in this set" })}
        </h2>
        {items.length === 0 ? (
          <p className="mt-2 text-sm text-muted-foreground">
            {t("observability.set.noItems", { defaultValue: "No metrics yet." })}
          </p>
        ) : (
          <ul className="mt-2 grid gap-2">
            {items.map((entry) => (
              <li
                key={entry.id}
                className="flex items-center justify-between gap-2 rounded-md border border-border bg-background px-3 py-2"
              >
                <span className="truncate text-sm font-medium">
                  {entry.label_override || entry.metric_query_name}
                </span>
                <span className="truncate font-mono text-xs text-muted-foreground">
                  {entry.metric_query_slug}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </EntityDetail>
  );
}
