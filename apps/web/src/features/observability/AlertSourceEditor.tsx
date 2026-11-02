import { useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "@/shared/ui-primitives/button";
import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { RequireCapability } from "@/shared/auth/permissions";
import { notify } from "@/shared/lib/notifications";
import { Input } from "@/shared/ui-primitives/input";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Switch } from "@/shared/ui-primitives/switch";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageState from "@/shared/ui-shared/PageState";
import {
  createAlertSource,
  listAlertSources,
  updateAlertSource,
  type AlertSourceItem,
  type AlertSourceListResponse,
  type AlertSourceRequest,
} from "@/features/observability/api";
import {
  MONITORING_ALERT_SOURCE_TYPES,
  isMonitoringAlertSourceType,
} from "./monitoringTaxonomy";
import { ALERT_SOURCES_KEY } from "./keys";
import {
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityListHref,
  observabilityRecordHref,
} from "./observabilityRoutes";

interface DraftState {
  name: string;
  queryUrl: string;
  credentialRef: string;
  timeoutMs: string;
  sourceType: string;
  enabled: boolean;
}

const EMPTY_DRAFT: DraftState = {
  name: "",
  queryUrl: "",
  credentialRef: "",
  timeoutMs: "5000",
  sourceType: "alertmanager",
  enabled: true,
};

function draftFromItem(item: AlertSourceItem): DraftState {
  return {
    name: item.name,
    queryUrl: item.query_url,
    credentialRef: item.credential_ref ?? "",
    timeoutMs: String(item.timeout_ms),
    sourceType: item.source_type,
    enabled: item.enabled,
  };
}

const { base: ALERT_SOURCES_BASE, view: ALERT_SOURCES_VIEW } =
  OBSERVABILITY_ENTITY_ROUTES.alertSources;

const AlertSourceForm = ({ item }: { item: AlertSourceItem | null }) => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [baseline] = useState<DraftState>(() => (item ? draftFromItem(item) : EMPTY_DRAFT));
  const [draft, setDraft] = useState<DraftState>(baseline);

  const back = () => {
    if (mutation.isPending) return;
    void navigate(item ? observabilityRecordHref(ALERT_SOURCES_BASE, item.id) : observabilityListHref(ALERT_SOURCES_VIEW));
  };

  const set = <K extends keyof DraftState>(key: K, value: DraftState[K]) =>
    { if (!mutation.isPending) setDraft((prev) => ({ ...prev, [key]: value })); };

  const sourceLabel = (type: string) =>
    t(`observability.alertSource.families.${type}.label`, {
      defaultValue:
        type === "zabbix"
          ? "Zabbix"
          : type === "vmalert"
            ? "vmalert"
            : "Alertmanager",
    });

  const sourceDescription = (type: string) =>
    t(`observability.alertSource.families.${type}.description`, {
      defaultValue:
        type === "zabbix"
          ? "Active problems from a Zabbix server."
          : type === "vmalert"
            ? "Firing alerts from a vmalert instance."
            : "Active alerts from an Alertmanager instance.",
    });

  const sourceUrlPlaceholder =
    draft.sourceType === "zabbix"
      ? "https://zabbix.example/api_jsonrpc.php"
      : draft.sourceType === "vmalert"
        ? "http://vmalert:8880/api/v1/alerts"
        : "http://alertmanager:9093/api/v2/alerts"; // allow: canonical-vocab

  const mutation = useApiMutation({
    mutationFn: (body: AlertSourceRequest) =>
      item ? updateAlertSource(item.id, body) : createAlertSource(body),
    onSuccess: (saved) => {
      qc.setQueryData<AlertSourceListResponse>(ALERT_SOURCES_KEY, (current) => {
        const items = current?.items ?? [];
        return items.some((row) => row.id === saved.id)
          ? { items: items.map((row) => (row.id === saved.id ? saved : row)), total: current?.total ?? items.length }
          : { items: [...items, saved], total: (current?.total ?? items.length) + 1 };
      });
      void qc.invalidateQueries({ queryKey: ALERT_SOURCES_KEY });
      notify(
        "success",
        item
          ? t("observability.alertSource.updated", { defaultValue: "Alert source updated" })
          : t("observability.alertSource.created", { defaultValue: "Alert source created" }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const submit = () => {
    if (!canSubmit) return;
    mutation.mutate({
      name: draft.name.trim(),
      query_url: draft.queryUrl.trim(),
      credential_ref: draft.credentialRef.trim() || null,
      timeout_ms: Number.parseInt(draft.timeoutMs, 10) || 5000,
      source_type: isMonitoringAlertSourceType(draft.sourceType) ? draft.sourceType : "alertmanager",
      enabled: draft.enabled,
    }, { onSuccess: (saved) => { void navigate(observabilityRecordHref(ALERT_SOURCES_BASE, saved.id)); } });
  };

  const canSubmit = !!draft.name.trim() && !!draft.queryUrl.trim() && !mutation.isPending;

  return (
    <EntityEditorPage scope="routed"
      dirtyMessage={t("editor.unsavedChanges")}
      title={t(item ? "observability.alertSource.editTitle" : "observability.alertSource.createTitle")}
      description={t("observability.alertSource.dialogDescription", {
        defaultValue: "An alerting backend Piping pulls active alerts from.",
      })}
      onBack={back}
      backLabel={t("back")}
      onCancel={back}
      cancelLabel={t("cancel")}
      onSave={submit}
      saveLabel={item ? t("save") : t("create")}
      saving={mutation.isPending}
      saveDisabled={!canSubmit}
      dirty={JSON.stringify(draft) !== JSON.stringify(baseline)}
    >
      <fieldset disabled={mutation.isPending} className="grid min-w-0 gap-4 sm:grid-cols-2">
        <FormField
          htmlFor="alert-source-name"
          label={t("observability.alertSource.name", { defaultValue: "Name" })}
          required
          className="grid gap-1.5"
        >
          <Input
            id="alert-source-name"
            className="text-sm"
            value={draft.name}
            onChange={(e) => set("name", e.target.value)}
          />
        </FormField>

        <FormSelect
          label={t("observability.alertSource.family", { defaultValue: "Alert source family" })}
          value={draft.sourceType}
          disabled={mutation.isPending}
          onValueChange={(v) => set("sourceType", v)}
          options={MONITORING_ALERT_SOURCE_TYPES.map((type) => ({
            value: type,
            label: (
              <>
                <span className="block font-medium">{sourceLabel(type)}</span>
                <span className="block text-xs text-muted-foreground">
                  {sourceDescription(type)}
                </span>
              </>
            ),
          }))}
          help={t("observability.alertSource.familyHint", {
            defaultValue:
              "Alertmanager and vmalert expose active alerts over their REST API; Zabbix exposes current problems over JSON-RPC.",
          })}
          itemClassName="py-2"
          className="grid gap-1.5"
        />

        <FormField
          label={t("observability.alertSource.url", { defaultValue: "Alerts URL" })}
          htmlFor="alert-source-url"
          required
          help={
            draft.sourceType === "zabbix"
              ? t("observability.alertSource.zabbixUrlHint", {
                  defaultValue: "Use the Zabbix JSON-RPC endpoint, usually /api_jsonrpc.php.",
                })
              : draft.sourceType === "vmalert"
                ? t("observability.alertSource.vmalertUrlHint", {
                    defaultValue: "Use the vmalert active-alerts endpoint, usually /api/v1/alerts.",
                  })
                : t("observability.alertSource.alertmanagerUrlHint", {
                    defaultValue:
                      "Use the Alertmanager active-alerts endpoint, usually /api/v2/alerts.", // allow: canonical-vocab
                  })
          }
          className="grid gap-1.5"
        >
          <FormFieldControl><Input
            className="text-sm font-mono"
            value={draft.queryUrl}
            id="alert-source-url"
            onChange={(e) => set("queryUrl", e.target.value)}
            placeholder={sourceUrlPlaceholder}
          /></FormFieldControl>
        </FormField>

        <FormField
          label={t("observability.alertSource.credentialRef", {
            defaultValue: "Bearer-token named secret ID (optional)",
          })}
          htmlFor="alert-source-credential"
          help={t("observability.alertSource.credentialRefHint", {
            defaultValue:
              "UUID of an existing named secret. The secret value is never stored in the source.",
          })}
          className="grid gap-1.5"
        >
          <FormFieldControl><Input
            className="text-sm font-mono"
            value={draft.credentialRef}
            id="alert-source-credential"
            onChange={(e) => set("credentialRef", e.target.value)}
            placeholder="00000000-0000-0000-0000-000000000000"
          /></FormFieldControl>
        </FormField>

        <FormField
          label={t("observability.alertSource.timeout", { defaultValue: "Timeout (ms)" })}
          htmlFor="alert-source-timeout"
          className="grid gap-1.5"
        >
          <Input
            type="number"
            min={100}
            className="text-sm text-center font-mono"
            value={draft.timeoutMs}
            id="alert-source-timeout"
            onChange={(e) => set("timeoutMs", e.target.value)}
          />
        </FormField>

        <label className="flex items-center justify-between gap-4 py-2">
          <span className="text-sm font-medium">
            {t("observability.alertSource.enabled", { defaultValue: "Enabled" })}
          </span>
          <Switch checked={draft.enabled} onCheckedChange={(v) => set("enabled", v)} />
        </label>
      </fieldset>
    </EntityEditorPage>
  );
};

/**
 * Routed create/edit page for an alert source: `/observability/alert-sources/new`
 * and `/observability/alert-sources/:id/edit`.
 */
const AlertSourceEditor = () => {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery({ queryKey: ALERT_SOURCES_KEY, queryFn: () => listAlertSources() });
  const item = query.data?.items.find(candidate => candidate.id === id) ?? null;
  if (query.isPending || query.isError || (id && !item)) {
    return <EntityEditorPage scope="routed"
      title={t(id ? "observability.alertSource.editTitle" : "observability.alertSource.createTitle")}
      onBack={() => { void navigate(observabilityListHref(ALERT_SOURCES_VIEW)); }}
      backLabel={t("back")} cancelLabel={t("cancel")}>
      <PageState kind={query.isPending ? "loading" : query.isError ? "error" : "empty"}
        title={t(query.isPending ? "loading" : query.isError ? "observability.entityLoadFailed" : "observability.entityNotFound")}
        action={query.isError ? <Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button> : undefined} />
    </EntityEditorPage>;
  }

  return (
    <RequireCapability check={(p) => p.can(id ? "UPDATE" : "CREATE", "MONITORING")}>
      <AlertSourceForm key={id ?? "new"} item={item} />
    </RequireCapability>
  );
};

export default AlertSourceEditor;
