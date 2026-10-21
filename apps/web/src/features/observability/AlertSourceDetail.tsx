import type { ReactNode } from "react";
import { useParams } from "react-router";
import PageState from "@/shared/ui-shared/PageState";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { BellRing, Loader2, Pencil, Trash2 } from "lucide-react";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import { deleteAlertSource, listAlertSources } from "./api";
import { ALERT_SOURCES_KEY, invalidateInspectorMetricCaches } from "./keys";
import { useObservabilityEntityDelete } from "./useObservabilityEntityDelete";
import {
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityEditHref,
  observabilityListHref,
} from "./observabilityRoutes";

const { base, view } = OBSERVABILITY_ENTITY_ROUTES.alertSources;

/** Read-only detail route `/observability/alert-sources/:id`. */
export default function AlertSourceDetail() {
  const { id } = useParams();
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const query = useQuery({ queryKey: ALERT_SOURCES_KEY, queryFn: () => listAlertSources() });
  const listHref = observabilityListHref(view);
  const { deleteError, isDeleting, deleteConfirm } = useObservabilityEntityDelete({
    deleteFn: deleteAlertSource,
    invalidate: () => {
      void qc.invalidateQueries({ queryKey: ALERT_SOURCES_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    listHref,
  });
  const renderState = (content: ReactNode) => <PageState title={t("observability.title")} icon={BellRing} backHref={listHref}>{content}</PageState>;
  if (query.isPending) return renderState(<output className="flex justify-center p-8"><Loader2 className="size-6 animate-spin" aria-label={t("loading")} /></output>);
  if (query.isError) return renderState(<EmptyState icon={BellRing} title={t("observability.entityLoadFailed")} actionLabel={t("retry")} onAction={() => { void query.refetch(); }} />);
  const item = query.data.items.find(candidate => candidate.id === id);
  if (!item) return renderState(<EmptyState icon={BellRing} title={t("observability.entityNotFound")} />);

  return (
    <EntityDetail
      icon={BellRing}
      title={item.name}
      description={item.source_type}
      backHref={listHref}
      status={{ tone: item.enabled ? "positive" : "neutral", label: t(item.enabled ? "enabled" : "disabled") }}
      error={deleteError}
      fields={[
        { label: t("observability.alertSource.family"), value: item.source_type },
        {
          label: t("observability.alertSource.url"),
          value: <span className="break-all font-mono">{item.query_url}</span>,
        },
        {
          label: t("observability.alertSource.credentialRef"),
          value: item.credential_ref ? <span className="break-all font-mono">{item.credential_ref}</span> : "",
        },
        { label: t("observability.alertSource.timeout"), value: `${item.timeout_ms} ms` },
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
    />
  );
}
