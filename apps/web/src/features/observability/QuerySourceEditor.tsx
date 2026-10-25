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
  createQuerySource,
  listQuerySources,
  updateQuerySource,
  type QuerySourceItem,
  type QuerySourceListResponse,
  type QuerySourceRequest,
} from "@/features/observability/api";
import { MONITORING_SOURCE_TYPES, isMonitoringSourceType } from "./monitoringTaxonomy";
import { QUERY_SOURCES_KEY, invalidateInspectorMetricCaches } from "./keys";
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

interface DraftState {
  name: string;
  slug: string;
  slugTouched: boolean;
  queryUrl: string;
  credentialRef: string;
  timeoutMs: string;
  sourceType: string;
  enabled: boolean;
}

const EMPTY_DRAFT: DraftState = {
  name: "",
  slug: "",
  slugTouched: false,
  queryUrl: "",
  credentialRef: "",
  timeoutMs: "5000",
  sourceType: "prometheus",
  enabled: true,
};

const SOURCE_FAMILY_FALLBACKS: Record<string, { label: string; description: string }> = {
  zabbix: {
    label: "Zabbix",
    description: "JSON-RPC item values from a Zabbix server.",
  },
  victoriametrics: {
    label: "VictoriaMetrics (PromQL-compatible)",
    description: "VictoriaMetrics instant query endpoint using PromQL.",
  },
};

const DEFAULT_SOURCE_FAMILY = {
  label: "Prometheus-compatible",
  description: "Prometheus or compatible instant query endpoint.",
};

function draftFromItem(item: QuerySourceItem): DraftState {
  return {
    name: item.name,
    slug: item.slug,
    slugTouched: true,
    queryUrl: item.query_url,
    credentialRef: item.credential_ref ?? "",
    timeoutMs: String(item.timeout_ms),
    sourceType: item.source_type,
    enabled: item.enabled,
  };
}

const { base: QUERY_SOURCES_BASE, view: QUERY_SOURCES_VIEW } =
  OBSERVABILITY_ENTITY_ROUTES.querySources;

const QuerySourceForm = ({ item }: { item: QuerySourceItem | null }) => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [baseline] = useState<DraftState>(() => (item ? draftFromItem(item) : EMPTY_DRAFT));
  const [draft, setDraft] = useState<DraftState>(baseline);

  const back = () => {
    if (mutation.isPending) return;
    void navigate(item ? observabilityRecordHref(QUERY_SOURCES_BASE, item.id) : observabilityListHref(QUERY_SOURCES_VIEW));
  };

  const set = <K extends keyof DraftState>(key: K, value: DraftState[K]) =>
    { if (!mutation.isPending) setDraft((prev) => ({ ...prev, [key]: value })); };

  const effectiveSlug = draft.slugTouched ? draft.slug : slugify(draft.name);

  const sourceLabel = (type: string) =>
    t(`observability.source.families.${type}.label`, {
      defaultValue: (SOURCE_FAMILY_FALLBACKS[type] ?? DEFAULT_SOURCE_FAMILY).label,
    });

  const sourceDescription = (type: string) =>
    t(`observability.source.families.${type}.description`, {
      defaultValue: (SOURCE_FAMILY_FALLBACKS[type] ?? DEFAULT_SOURCE_FAMILY).description,
    });

  const sourceUrlPlaceholder =
    draft.sourceType === "zabbix"
      ? "https://zabbix.example/api_jsonrpc.php"
      : "http://victoriametrics:8428/api/v1/query";

  const mutation = useApiMutation({
    mutationFn: (body: QuerySourceRequest) =>
      item ? updateQuerySource(item.id, body) : createQuerySource(body),
    onSuccess: (saved) => {
      qc.setQueryData<QuerySourceListResponse>(QUERY_SOURCES_KEY, (current) => {
        const items = current?.items ?? [];
        return items.some((row) => row.id === saved.id)
          ? { items: items.map((row) => (row.id === saved.id ? saved : row)), total: current?.total ?? items.length }
          : { items: [...items, saved], total: (current?.total ?? items.length) + 1 };
      });
      void qc.invalidateQueries({ queryKey: QUERY_SOURCES_KEY });
      invalidateInspectorMetricCaches(qc);
      notify(
        "success",
        item
          ? t("observability.source.updated", { defaultValue: "Source updated" })
          : t("observability.source.created", { defaultValue: "Source created" }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const submit = () => {
    if (!canSubmit) return;
    mutation.mutate({
      name: draft.name.trim(),
      slug: effectiveSlug,
      query_url: draft.queryUrl.trim(),
      credential_ref: draft.credentialRef.trim() || null,
      timeout_ms: Number.parseInt(draft.timeoutMs, 10) || 5000,
      source_type: isMonitoringSourceType(draft.sourceType) ? draft.sourceType : "prometheus",
      enabled: draft.enabled,
    }, { onSuccess: (saved) => { void navigate(observabilityRecordHref(QUERY_SOURCES_BASE, saved.id)); } });
  };

  const canSubmit =
    !!draft.name.trim() && !!effectiveSlug && !!draft.queryUrl.trim() && !mutation.isPending;

  return (
    <EntityEditorPage scope="routed"
      dirtyMessage={t("editor.unsavedChanges")}
      title={t(item ? "observability.source.editTitle" : "observability.source.createTitle")}
      description={t("observability.source.dialogDescription", {
        defaultValue: "A backend that serves metric values for inspectors.",
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
      <fieldset disabled={mutation.isPending} className="grid min-w-0 gap-4">
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField
            htmlFor="query-source-name"
            label={t("observability.source.name", { defaultValue: "Name" })}
            required
            className="grid gap-1.5"
          >
            <Input
              id="query-source-name"
              className="text-sm"
              value={draft.name}
              onChange={(e) => set("name", e.target.value)}
            />
          </FormField>
          <FormField
            label={t("observability.source.slug", { defaultValue: "Slug" })}
            htmlFor="query-source-slug"
            required
            className="grid gap-1.5"
          >
            <Input
              className="text-sm font-mono"
              value={effectiveSlug}
              id="query-source-slug"
              onChange={(e) => {
                set("slug", e.target.value);
                set("slugTouched", true);
              }}
            />
          </FormField>
        </div>

        <FormSelect
          label={t("observability.source.family", { defaultValue: "Source family" })}
          value={draft.sourceType}
          disabled={mutation.isPending}
          onValueChange={(v) => set("sourceType", v)}
          options={MONITORING_SOURCE_TYPES.map((type) => ({
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
          help={t("observability.source.familyHint", {
            defaultValue:
              "Prometheus and VictoriaMetrics use PromQL instant queries; Zabbix uses JSON-RPC item reads.",
          })}
          itemClassName="py-2"
          className="grid gap-1.5"
        />

        <FormField
          label={t("observability.source.url", { defaultValue: "Query URL" })}
          htmlFor="query-source-url"
          required
          help={
            draft.sourceType === "zabbix"
              ? t("observability.source.zabbixUrlHint", {
                  defaultValue:
                    "Use the Zabbix JSON-RPC endpoint without userinfo, query parameters, or fragments. Put credentials in the named-secret field.",
                })
              : t("observability.source.promqlUrlHint", {
                defaultValue:
                    "Use a Prometheus-compatible /api/v1/query endpoint without userinfo, query parameters, or fragments. Put credentials in the named-secret field.",
                })
          }
          className="grid gap-1.5"
        >
          <FormFieldControl><Input
            className="text-sm font-mono"
            value={draft.queryUrl}
            id="query-source-url"
            onChange={(e) => set("queryUrl", e.target.value)}
            placeholder={sourceUrlPlaceholder}
          /></FormFieldControl>
        </FormField>

        <FormField
          label={t("observability.source.credentialRef", {
            defaultValue: "Bearer-token named secret ID (optional)",
          })}
          htmlFor="query-source-credential"
          help={t("observability.source.credentialRefHint", {
            defaultValue:
              "UUID of an existing named secret. The secret value is never stored in the source.",
          })}
          className="grid gap-1.5"
        >
          <FormFieldControl><Input
            className="text-sm font-mono"
            value={draft.credentialRef}
            id="query-source-credential"
            onChange={(e) => set("credentialRef", e.target.value)}
            placeholder="00000000-0000-0000-0000-000000000000"
          /></FormFieldControl>
        </FormField>

        <FormField
          label={t("observability.source.timeout", { defaultValue: "Timeout (ms)" })}
          htmlFor="query-source-timeout"
          className="grid gap-1.5"
        >
          <Input
            type="number"
            className="text-sm text-center font-mono"
            value={draft.timeoutMs}
            id="query-source-timeout"
            onChange={(e) => set("timeoutMs", e.target.value)}
          />
        </FormField>

        <label className="flex items-center justify-between gap-4 py-2">
          <span className="text-sm font-medium">
            {t("observability.source.enabled", { defaultValue: "Enabled" })}
          </span>
          <Switch checked={draft.enabled} onCheckedChange={(v) => set("enabled", v)} />
        </label>
      </fieldset>
    </EntityEditorPage>
  );
};

/**
 * Routed create/edit page for a query source: `/observability/sources/new` and
 * `/observability/sources/:id/edit`, mirroring the connection-config editor.
 */
const QuerySourceEditor = () => {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery({ queryKey: QUERY_SOURCES_KEY, queryFn: () => listQuerySources() });
  const item = query.data?.items.find(candidate => candidate.id === id) ?? null;
  if (query.isPending || query.isError || (id && !item)) {
    return <EntityEditorPage scope="routed"
      title={t(id ? "observability.source.editTitle" : "observability.source.createTitle")}
      onBack={() => { void navigate(observabilityListHref(QUERY_SOURCES_VIEW)); }}
      backLabel={t("back")} cancelLabel={t("cancel")}>
      <PageState kind={query.isPending ? "loading" : query.isError ? "error" : "empty"}
        title={t(query.isPending ? "loading" : query.isError ? "observability.entityLoadFailed" : "observability.entityNotFound")}
        action={query.isError ? <Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button> : undefined} />
    </EntityEditorPage>;
  }

  return (
    <RequireCapability check={(p) => p.can(id ? "UPDATE" : "CREATE", "MONITORING")}>
      <QuerySourceForm key={id ?? "new"} item={item} />
    </RequireCapability>
  );
};

export default QuerySourceEditor;
