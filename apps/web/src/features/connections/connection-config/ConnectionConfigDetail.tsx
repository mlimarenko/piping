import { useState, type ReactNode } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2, Network, Pencil, Trash2 } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useApiMutation } from "@/shared/api/hooks";
import { Button } from "@/shared/ui-primitives/button";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import { notify } from "@/shared/lib/notifications";
import PageState from "@/shared/ui-shared/PageState";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import { asParams, deleteRecord, listRecords } from "./api";
import AssignmentsPanel from "./AssignmentsPanel";
import EffectivePreview from "./EffectivePreview";
import { CONNECTION_CONFIG_ROUTE, connectionConfigRecordHref } from "./recordRoutes";

export default function ConnectionConfigDetail() {
  const { id } = useParams();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [params, setParams] = useSearchParams();
  const [error, setError] = useState("");
  const query = useQuery({ queryKey: ["connection-config", "records", "options"], queryFn: () => listRecords() });
  const mutation = useApiMutation({ mutationFn: (recordId: string) => deleteRecord(recordId),
    onSuccess: () => { void qc.invalidateQueries({ queryKey: ["connection-config", "records"] }); notify("success", t("connectionConfig.recordDeleted")); void navigate(CONNECTION_CONFIG_ROUTE); },
    onError: () => setError(t("connectionConfig.deleteFailed")),
  });
  const renderState = (content: ReactNode) => <PageState title={t("connectionConfig.title")} icon={Network} backHref={CONNECTION_CONFIG_ROUTE} backLabel={t("connectionConfig.backToList")}>
    {content}
  </PageState>;
  if (query.isPending) return renderState(<output className="flex justify-center p-8"><Loader2 className="size-6 animate-spin" aria-label={t("loading")} /></output>);
  if (query.isError) return renderState(<EmptyState icon={Network} title={t("connectionConfig.loadFailed")} actionLabel={t("retry")} onAction={() => { void query.refetch(); }} />);
  const record = query.data.find(candidate => candidate.id === id);
  if (!record) return renderState(<EmptyState icon={Network} title={t("connectionConfig.notFound")} />);
  const config = asParams(record.params);
  const tab = ["assignments", "preview"].includes(params.get("tab") ?? "") ? params.get("tab")! : "overview";
  const items = [
    { value: "overview", label: t("connectionConfig.overview") },
    { value: "assignments", label: t("connectionConfig.records.tabs.assignments") },
    { value: "preview", label: t("connectionConfig.records.tabs.preview") },
  ];
  const baseRecordName = record.base_record_id
    ? query.data.find(r => r.id === record.base_record_id)?.name ?? t("connectionConfig.notFound")
    : null;
  return <EntityDetail
    icon={Network}
    title={record.name}
    description={record.description || undefined}
    backHref={CONNECTION_CONFIG_ROUTE}
    backLabel={t("connectionConfig.backToList")}
    error={error || undefined}
    fields={[
      { label: t("connectionConfig.fields.protocol"), value: <span className="font-medium uppercase">{record.protocol}</span> },
      { label: t("status"), value: <StatusChip status={record.enabled ? "positive" : "neutral"} label={t(record.enabled ? "enabled" : "disabled")} /> },
      { label: t("connectionConfig.fields.baseRecord"), value: record.base_record_id && baseRecordName ? <Link className="text-primary hover:underline" to={connectionConfigRecordHref(record.base_record_id)}>{baseRecordName}</Link> : t("connectionConfig.fields.baseRecordNone") },
      { label: t(record.protocol === "app" ? "connectionConfig.fields.uri" : "connectionConfig.fields.host"), value: <span className="break-all font-mono text-[11px]">{(record.protocol === "app" ? config.url : config.host) || t(record.base_record_id ? "connectionConfig.inherited" : "connectionConfig.targetAddress")}</span> },
      ...(config.port != null ? [{ label: t("connectionConfig.fields.port"), value: String(config.port) }] : []),
      { label: t("connectionConfig.section.presentation"), value: config.ui?.label || record.name },
    ]}
    actions={[
      { key: "edit", label: t("edit"), icon: Pencil, href: `${connectionConfigRecordHref(record.id)}/edit`, allowed: can("UPDATE", "CONFIG_CONTEXT") },
      { key: "delete", label: t("delete"), icon: Trash2, variant: "outline", allowed: can("DELETE", "CONFIG_CONTEXT"), disabled: mutation.isPending, confirm: { title: t("connectionConfig.deleteTitle"), description: t("connectionConfig.deleteConfirm", { name: record.name }), onConfirm: () => mutation.mutate(record.id) } },
    ]}
  >
    <Tabs value={tab} onValueChange={next => setParams(next === "overview" ? {} : { tab: next }, { replace: true })}>
      <SectionTabs items={items} />
      <TabsContent value="overview">
        <p className="mt-4 border-t border-border pt-4 text-sm text-muted-foreground">{t("connectionConfig.detailHint")}</p>
        <Button variant="outline" className="mt-3" onClick={() => setParams({ tab: "assignments" }, { replace: true })}>{t("connectionConfig.records.tabs.assignments")}</Button>
      </TabsContent>
      <TabsContent value="assignments"><AssignmentsPanel recordId={record.id} /></TabsContent>
      <TabsContent value="preview"><EffectivePreview /></TabsContent>
    </Tabs>
  </EntityDetail>;
}
