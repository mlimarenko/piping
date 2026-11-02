import type { ReactNode } from "react";
import { Link, useParams } from "react-router";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { Button } from "@/shared/ui-primitives/button";
import PageState from "@/shared/ui-shared/PageState";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ListChecks, Pencil, Trash2 } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import { deleteMetricQuery, listMetricQueries } from "./api";
import { METRIC_QUERIES_KEY, invalidateInspectorMetricCaches } from "./keys";
import { useObservabilityEntityDelete } from "./useObservabilityEntityDelete";
import {
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityEditHref,
  observabilityListHref,
} from "./observabilityRoutes";

const { base, view } = OBSERVABILITY_ENTITY_ROUTES.metricQueries;

/** Read-only detail route `/observability/metrics/:id`. */
export default function MetricQueryDetail() {
  const { id } = useParams();
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const query = useQuery({ queryKey: METRIC_QUERIES_KEY, queryFn: () => listMetricQueries() });
  const listHref = observabilityListHref(view);
  const { deleteError, isDeleting, deleteConfirm } = useObservabilityEntityDelete({
    deleteFn: deleteMetricQuery,
    invalidate: () => {
      void qc.invalidateQueries({ queryKey: METRIC_QUERIES_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    listHref,
  });
  const renderState = (content: ReactNode) => <><PageChrome title={t("observability.title")} icon={ListChecks} actions={<Button asChild variant="ghost"><Link to={listHref}><ArrowLeft className="size-4" />{t("back")}</Link></Button>} />{content}</>;
  if (query.isPending) return renderState(<PageState kind="loading" title={t("loading")} />);
  if (query.isError) return renderState(<PageState kind="error" title={t("observability.entityLoadFailed")} action={<Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button>} />);
  const item = query.data.items.find(candidate => candidate.id === id);
  if (!item) return renderState(<PageState kind="empty" title={t("observability.entityNotFound")} />);

  return (
    <EntityDetail
      icon={ListChecks}
      title={item.label || item.name}
      description={item.slug}
      backHref={listHref}
      status={{ tone: item.enabled ? "positive" : "neutral", label: t(item.enabled ? "enabled" : "disabled") }}
      error={deleteError}
      fields={[
        { label: t("observability.metric.name"), value: item.name },
        { label: t("observability.metric.slug"), value: item.slug },
        { label: t("observability.metric.label"), value: item.label ?? "" },
        { label: t("observability.metric.unit"), value: item.unit ?? "" },
        { label: t("observability.source.family"), value: item.source_type },
        { label: t("observability.metric.queryType"), value: item.query_type },
        { label: t("observability.metric.valueType"), value: item.value_type },
        { label: t("observability.metric.widgetType"), value: item.widget_type },
        {
          label: t("observability.metric.query"),
          value: <span className="break-all font-mono">{item.query}</span>,
        },
        { label: t("observability.metric.description"), value: item.description ?? "" },
        { label: t("observability.metric.showByDefault"), value: item.default_show_in_inspector ? t("enabled") : t("disabled") },
        { label: t("observability.metric.showInList"), value: item.default_show_in_list ? t("enabled") : t("disabled") },
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
          confirm: deleteConfirm(item.label || item.name, item.id),
        },
      ]}
    />
  );
}
