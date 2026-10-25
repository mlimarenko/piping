import { useState, type ReactNode } from "react";
import { useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useApiMutation } from "@/shared/api/hooks";
import { RequireCapability } from "@/shared/auth/permissions";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageState from "@/shared/ui-shared/PageState";
import { Button } from "@/shared/ui-primitives/button";
import { notify } from "@/shared/lib/notifications";
import RecordForm from "./RecordForm";
import { createRecord, listRecords, updateRecord, type ConnConfRecord } from "./api";
import { draftFromRecord, emptyDraft } from "./recordDraft";
import { CONNECTION_CONFIG_ROUTE, connectionConfigRecordHref } from "./recordRoutes";

function RecordEditor({ record, records }: Readonly<{ record?: ConnConfRecord; records: ConnConfRecord[] }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [baseline] = useState(() => record ? draftFromRecord(record) : emptyDraft());
  const [draft, setDraft] = useState(baseline);
  const [error, setError] = useState("");
  const back = () => { void navigate(record ? connectionConfigRecordHref(record.id) : CONNECTION_CONFIG_ROUTE); };
  const mutation = useApiMutation({
    mutationFn: () => {
      const body = {
        name: draft.name.trim(), description: draft.description.trim(),
        protocol: draft.protocol, base_record_id: draft.baseRecordId || null,
        enabled: draft.enabled, params: draft.params,
      };
      return record ? updateRecord(record.id, body) : createRecord(body);
    },
    onSuccess: (saved) => {
      qc.setQueryData<ConnConfRecord[]>(["connection-config", "records", "options"], current =>
        current?.some(item => item.id === saved.id) ? current.map(item => item.id === saved.id ? saved : item) : [...(current ?? []), saved]);
      void qc.invalidateQueries({ queryKey: ["connection-config", "records"] });
      notify("success", record ? t("connectionConfig.recordUpdated") : t("connectionConfig.recordCreated", { name: saved.name }));
      void navigate(connectionConfigRecordHref(saved.id) + (record ? "" : "?tab=assignments"));
    },
    onError: () => setError(t(record ? "connectionConfig.updateFailed" : "connectionConfig.createFailed")),
  });
  return <EntityEditorPage scope="routed" title={t(record ? "connectionConfig.editTitle" : "connectionConfig.createTitle")}
    description={t("connectionConfig.editorHint")} onBack={back} backLabel={t(record ? "connectionConfig.backToRecord" : "connectionConfig.backToList")}
    onCancel={back} cancelLabel={t("cancel")} onSave={() => {
      if (!draft.name.trim()) { setError(t("connectionConfig.nameRequired")); return; }
      setError(""); mutation.mutate();
    }} saveLabel={t(record ? "saveChanges" : "connectionConfig.createRecord")}
    saving={mutation.isPending} dirty={JSON.stringify(draft) !== JSON.stringify(baseline)} error={error ? <p role="alert" className="text-destructive">{error}</p> : undefined}>
    <fieldset disabled={mutation.isPending} inert={mutation.isPending} className="min-w-0" aria-busy={mutation.isPending}><RecordForm draft={draft} onChange={next => { if (!mutation.isPending) setDraft(next); }}
      baseRecords={records.filter(candidate => candidate.protocol === draft.protocol && candidate.id !== record?.id)}
      recordId={record?.id} /></fieldset>
  </EntityEditorPage>;
}

export default function ConnectionConfigEditor() {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery({ queryKey: ["connection-config", "records", "options"], queryFn: () => listRecords() });
  const back = () => { void navigate(CONNECTION_CONFIG_ROUTE); };
  const renderState = (content: ReactNode) => <EntityEditorPage scope="routed"
    title={t(id ? "connectionConfig.editTitle" : "connectionConfig.createTitle")}
    description={t("connectionConfig.editorHint")} onBack={back} backLabel={t("connectionConfig.backToList")}
    onCancel={back} cancelLabel={t("cancel")}>
    {content}
  </EntityEditorPage>;
  if (query.isPending) return renderState(<PageState kind="loading" title={t("loading")} />);
  if (query.isError) return renderState(<PageState kind="error" title={t("connectionConfig.loadFailed")} action={<Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button>} />);
  const record = query.data.find(candidate => candidate.id === id);
  if (id && !record) return renderState(<PageState kind="empty" title={t("connectionConfig.notFound")} />);
  return <RequireCapability check={p => p.can(id ? "UPDATE" : "CREATE", "CONFIG_CONTEXT")}>
    <RecordEditor key={id ?? "create"} record={record} records={query.data} />
  </RequireCapability>;
}
