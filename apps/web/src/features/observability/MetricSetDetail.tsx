import type { ReactNode } from "react";
import { Link, useParams } from "react-router";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { Button } from "@/shared/ui-primitives/button";
import PageState from "@/shared/ui-shared/PageState";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Layers, Pencil, Trash2 } from "lucide-react";
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
  const renderState = (content: ReactNode) => <><PageChrome title={t("observability.title")} icon={Layers} actions={<Button asChild variant="ghost"><Link to={listHref}><ArrowLeft className="size-4" />{t("back")}</Link></Button>} />{content}</>;
  if (query.isPending) return renderState(<PageState kind="loading" title={t("loading")} />);
  if (query.isError) return renderState(<PageState kind="error" title={t("observability.entityLoadFailed")} action={<Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button>} />);
  const item = query.data.items.find(candidate => candidate.id === id);
  if (!item) return renderState(<PageState kind="empty" title={t("observability.entityNotFound")} />);

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
