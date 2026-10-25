import type { ReactNode } from "react";
import { Link, useParams } from "react-router";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { Button } from "@/shared/ui-primitives/button";
import PageState from "@/shared/ui-shared/PageState";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Database, Pencil, Trash2 } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import { deleteQuerySource, listQuerySources } from "./api";
import { QUERY_SOURCES_KEY, invalidateInspectorMetricCaches } from "./keys";
import { useObservabilityEntityDelete } from "./useObservabilityEntityDelete";
import {
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityEditHref,
  observabilityListHref,
} from "./observabilityRoutes";

const { base, view } = OBSERVABILITY_ENTITY_ROUTES.querySources;

/** Read-only detail route `/observability/sources/:id`. */
export default function QuerySourceDetail() {
  const { id } = useParams();
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const query = useQuery({ queryKey: QUERY_SOURCES_KEY, queryFn: () => listQuerySources() });
  const listHref = observabilityListHref(view);
  const { deleteError, isDeleting, deleteConfirm } = useObservabilityEntityDelete({
    deleteFn: deleteQuerySource,
    invalidate: () => {
      void qc.invalidateQueries({ queryKey: QUERY_SOURCES_KEY });
      invalidateInspectorMetricCaches(qc);
    },
    listHref,
  });
  const renderState = (content: ReactNode) => <><PageChrome title={t("observability.title")} icon={Database} actions={<Button asChild variant="ghost"><Link to={listHref}><ArrowLeft className="size-4" />{t("back")}</Link></Button>} />{content}</>;
  if (query.isPending) return renderState(<PageState kind="loading" title={t("loading")} />);
  if (query.isError) return renderState(<PageState kind="error" title={t("observability.entityLoadFailed")} action={<Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button>} />);
  const item = query.data.items.find(candidate => candidate.id === id);
  if (!item) return renderState(<PageState kind="empty" title={t("observability.entityNotFound")} />);

  return (
    <EntityDetail
      icon={Database}
      title={item.name}
      description={item.slug}
      backHref={listHref}
      status={{ tone: item.enabled ? "positive" : "neutral", label: t(item.enabled ? "enabled" : "disabled") }}
      error={deleteError}
      fields={[
        { label: t("observability.source.slug"), value: item.slug },
        { label: t("observability.source.type"), value: item.source_type },
        {
          label: t("observability.source.url"),
          value: <span className="break-all font-mono">{item.query_url}</span>,
        },
        {
          label: t("observability.source.credentialRef"),
          value: item.credential_ref ? <span className="break-all font-mono">{item.credential_ref}</span> : "",
        },
        { label: t("observability.source.timeout"), value: `${item.timeout_ms} ms` },
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
