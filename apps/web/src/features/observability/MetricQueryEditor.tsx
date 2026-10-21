import { useMemo, useState, type ReactNode } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ListChecks, Loader2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { RequireCapability } from "@/shared/auth/permissions";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import FormSection from "@/shared/ui-shared/FormSection";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import EmptyState from "@/shared/ui-shared/EmptyState";
import {
  createMetricQuery,
  listMetricQueries,
  listQuerySources,
  testMetricQuery,
  updateMetricQuery,
  type MetricQueryItem,
  type MetricQueryListResponse,
  type MetricQueryRequest,
  type MetricQueryTestResponse,
} from "@/features/observability/api";
import { searchConnectionTargets } from "@/features/connections/api";
import ThresholdEditor from "./ThresholdEditor";
import {
  THRESHOLD_EDITOR_DEFAULTS,
  statusFromThresholdValue,
  thresholdValueFromStatus,
  validateThresholdValue,
  type ThresholdEditorValue,
} from "./thresholdValue";
import {
  MONITORING_SOURCE_TYPES,
  defaultQueryTypeForSourceType,
  isMetricQueryTypeCompatible,
  isMonitoringSourceType,
  isZabbixSourceType,
  queryTypesForSourceType,
} from "./monitoringTaxonomy";
import { METRIC_QUERIES_KEY, invalidateInspectorMetricCaches } from "./keys";
import { replaceById } from "./metricHelpers";
import {
  OBSERVABILITY_ENTITY_ROUTES,
  observabilityListHref,
  observabilityRecordHref,
} from "./observabilityRoutes";

const VALUE_TYPES = [
  "auto",
  "state",
  "number",
  "percent",
  "bytes",
  "duration",
  "text",
  "timestamp",
  "log",
  "table",
] as const;
const WIDGET_TYPES = ["stat", "badge", "gauge", "sparkline", "table", "log"] as const;
const UNIT_PRESETS = ["%", "s", "B", "B/s", "state", "count"] as const;
const TEST_TARGET_SEARCH_LIMIT = 25;

type JsonRecord = Record<string, unknown>;

function asRecord(value: unknown): JsonRecord {
  return value && typeof value === "object" && !Array.isArray(value)
    ? { ...(value as JsonRecord) }
    : {};
}

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
  label: string;
  query: string;
  unit: string;
  description: string;
  sortOrder: string;
  defaultShowInInspector: boolean;
  default_show_in_list: boolean;
  enabled: boolean;
  sourceType: string;
  queryType: string;
  lastPromqlQueryType: string;
  lastZabbixQueryType: string;
  valueType: string;
  widgetType: string;
  threshold: ThresholdEditorValue;
  previewSampleText: string;
  testTargetId: string;
}

const EMPTY_DRAFT: DraftState = {
  name: "",
  slug: "",
  slugTouched: false,
  label: "",
  query: "",
  unit: "",
  description: "",
  sortOrder: "0",
  defaultShowInInspector: true,
  default_show_in_list: true,
  enabled: true,
  sourceType: "prometheus",
  queryType: "promql",
  lastPromqlQueryType: "promql",
  lastZabbixQueryType: "zabbix_item_key",
  valueType: "auto",
  widgetType: "stat",
  threshold: { ...THRESHOLD_EDITOR_DEFAULTS },
  previewSampleText: "",
  testTargetId: "",
};

function draftFromItem(item: MetricQueryItem): DraftState {
  const options = asRecord(item.options);
  const display = asRecord(options.display);
  const status = display.status;
  const lastPromqlQueryType =
    !isZabbixSourceType(item.source_type) &&
    isMetricQueryTypeCompatible(item.source_type, item.query_type)
      ? item.query_type
      : EMPTY_DRAFT.lastPromqlQueryType;
  const lastZabbixQueryType =
    isZabbixSourceType(item.source_type) &&
    isMetricQueryTypeCompatible(item.source_type, item.query_type)
      ? item.query_type
      : EMPTY_DRAFT.lastZabbixQueryType;
  return {
    name: item.name,
    slug: item.slug,
    slugTouched: true,
    label: item.label ?? "",
    query: item.query,
    unit: item.unit ?? "",
    description: item.description ?? "",
    sortOrder: String(item.sort_order),
    defaultShowInInspector: item.default_show_in_inspector,
    default_show_in_list: item.default_show_in_list,
    enabled: item.enabled,
    sourceType: item.source_type,
    queryType: item.query_type,
    lastPromqlQueryType,
    lastZabbixQueryType,
    valueType: item.value_type,
    widgetType: item.widget_type,
    threshold: thresholdValueFromStatus(status),
    previewSampleText: "",
    testTargetId: "",
  };
}

function buildOptions(baseOptions: unknown, draft: DraftState): JsonRecord {
  const options = asRecord(baseOptions);
  const display = asRecord(options.display);
  display.status = statusFromThresholdValue(draft.threshold);
  options.display = display;
  return options;
}

const METRIC_QUERIES_BASE = OBSERVABILITY_ENTITY_ROUTES.metricQueries.base;
const METRIC_QUERIES_VIEW = OBSERVABILITY_ENTITY_ROUTES.metricQueries.view;

const MetricQueryForm = ({ item }: { item: MetricQueryItem | null }) => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [baseline] = useState<DraftState>(() => (item ? draftFromItem(item) : EMPTY_DRAFT));
  const [draft, setDraft] = useState<DraftState>(baseline);

  const back = () => {
    if (mutation.isPending) return;
    void navigate(item ? observabilityRecordHref(METRIC_QUERIES_BASE, item.id) : observabilityListHref(METRIC_QUERIES_VIEW));
  };

  const set = <K extends keyof DraftState>(key: K, value: DraftState[K]) =>
    { if (!mutation.isPending) setDraft((prev) => ({ ...prev, [key]: value })); };

  const effectiveSlug = draft.slugTouched ? draft.slug : slugify(draft.name);
  const compatibleQueryTypes = queryTypesForSourceType(draft.sourceType);
  const queryTypeCompatible = isMetricQueryTypeCompatible(draft.sourceType, draft.queryType);

  const sourceLabel = (type: string) =>
    t(`observability.source.families.${type}.label`, {
      defaultValue:
        type === "zabbix"
          ? "Zabbix"
          : type === "victoriametrics"
            ? "VictoriaMetrics (PromQL-compatible)"
            : "Prometheus-compatible",
    });

  const sourceDescription = (type: string) =>
    t(`observability.source.families.${type}.description`, {
      defaultValue:
        type === "zabbix"
          ? "JSON-RPC item values from a Zabbix server."
          : type === "victoriametrics"
            ? "VictoriaMetrics instant query endpoint using PromQL."
            : "Prometheus or compatible instant query endpoint.",
    });

  const queryTypeLabel = (type: string) =>
    t(`observability.metric.queryTypes.${type}`, {
      defaultValue:
        type === "promql"
          ? "PromQL (Prometheus / VictoriaMetrics)"
          : type === "zabbix_item_key"
            ? "Zabbix item key"
            : type === "zabbix_item_name"
              ? "Zabbix item name"
              : type === "zabbix_itemid"
                ? "Zabbix item ID"
                : type,
    });

  const handleSourceTypeChange = (nextSourceType: string) => {
    if (mutation.isPending) return;
    if (!isMonitoringSourceType(nextSourceType)) return;
    setDraft((prev) => {
      const next: DraftState = { ...prev, sourceType: nextSourceType };
      if (isMetricQueryTypeCompatible(prev.sourceType, prev.queryType)) {
        if (isZabbixSourceType(prev.sourceType)) {
          next.lastZabbixQueryType = prev.queryType;
        } else {
          next.lastPromqlQueryType = prev.queryType;
        }
      }
      if (!isMetricQueryTypeCompatible(nextSourceType, prev.queryType)) {
        const remembered = isZabbixSourceType(nextSourceType)
          ? next.lastZabbixQueryType
          : next.lastPromqlQueryType;
        next.queryType = isMetricQueryTypeCompatible(nextSourceType, remembered)
          ? remembered
          : defaultQueryTypeForSourceType(nextSourceType);
      }
      return next;
    });
  };

  const handleQueryTypeChange = (nextQueryType: string) => {
    if (mutation.isPending) return;
    setDraft((prev) => ({
      ...prev,
      queryType: nextQueryType,
      lastPromqlQueryType: isZabbixSourceType(prev.sourceType)
        ? prev.lastPromqlQueryType
        : nextQueryType,
      lastZabbixQueryType: isZabbixSourceType(prev.sourceType)
        ? nextQueryType
        : prev.lastZabbixQueryType,
    }));
  };

  const mutation = useApiMutation({
    mutationFn: (body: MetricQueryRequest) =>
      item ? updateMetricQuery(item.id, body) : createMetricQuery(body),
    onSuccess: (saved) => {
      qc.setQueryData<MetricQueryListResponse>(METRIC_QUERIES_KEY, (current) => {
        const rows = current?.items ?? [];
        const known = rows.some((row) => row.id === saved.id);
        const items = known ? replaceById(rows, saved) : [...rows, saved];
        return { items, total: known ? (current?.total ?? rows.length) : (current?.total ?? rows.length) + 1 };
      });
      void qc.invalidateQueries({ queryKey: METRIC_QUERIES_KEY });
      invalidateInspectorMetricCaches(qc);
      notify(
        "success",
        item
          ? t("observability.metric.updated", { defaultValue: "Metric updated" })
          : t("observability.metric.created", { defaultValue: "Metric created" }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const submit = () => {
    if (!canSubmit) return;
    mutation.mutate({
      name: draft.name.trim(),
      slug: effectiveSlug,
      label: draft.label.trim() || null,
      query: draft.query.trim(),
      unit: draft.unit.trim() || null,
      description: draft.description.trim() || null,
      sort_order: Number.parseInt(draft.sortOrder, 10) || 0,
      default_show_in_inspector: draft.defaultShowInInspector,
      default_show_in_list: draft.default_show_in_list,
      enabled: draft.enabled,
      source_type: draft.sourceType,
      query_type: draft.queryType,
      value_type: draft.valueType,
      widget_type: draft.widgetType,
      options: buildOptions(item?.options, draft),
    }, { onSuccess: (saved) => { void navigate(observabilityRecordHref(METRIC_QUERIES_BASE, saved.id)); } });
  };

  const thresholdValidationError = validateThresholdValue(draft.threshold, t);
  const canSubmit =
    !!draft.name.trim() &&
    !!effectiveSlug &&
    !!draft.query.trim() &&
    queryTypeCompatible &&
    !thresholdValidationError &&
    !mutation.isPending;

  // ── Live PromQL/Zabbix test ───────────────
  //
  // The backend test endpoint needs a `query_url`. We default to the first
  // query source compatible with the chosen source family (global is fine —
  // the host-scoped picker is what `target_id` provides). Operators with no
  // configured query source see a disabled Test button with a hint.
  const querySourcesQuery = useQuery({
    queryKey: ["observability", "metric-query-test", "query-sources"],
    queryFn: () => listQuerySources(),
    staleTime: 30_000,
  });
  // Only match a query source whose family matches the chosen source_type.
  // A cross-family fallback would happily send a Zabbix item key to a
  // Prometheus URL (or vice-versa) and surface a misleading decode error;
  // the disabled-button hint below is the honest signal that no source of
  // this family exists.
  const firstQuerySource = useMemo(() => {
    const items = querySourcesQuery.data?.items ?? [];
    const sourceFamily = draft.sourceType;
    return (
      items.find((source) => source.source_type === sourceFamily && source.enabled) ?? null
    );
  }, [querySourcesQuery.data, draft.sourceType]);

  const targetSearch = useQuery({
    queryKey: ["observability", "metric-query-test", "target-search"],
    queryFn: () => searchConnectionTargets("", TEST_TARGET_SEARCH_LIMIT),
    staleTime: 30_000,
  });
  const targetOptions = targetSearch.data ?? [];

  const testRequest = {
    query: draft.query.trim(),
    query_url: firstQuerySource?.query_url ?? "",
    source_type: draft.sourceType,
    target_id: draft.testTargetId || targetOptions[0]?.id || null,
  };
  const testIdentity = JSON.stringify(testRequest);
  const [preview, setPreview] = useState<{
    identity: string;
    result?: MetricQueryTestResponse;
    error?: string;
  } | null>(null);
  const testResult = preview?.identity === testIdentity ? preview.result : null;
  const testError = preview?.identity === testIdentity ? preview.error : null;
  const testMutation = useApiMutation({
    mutationFn: testMetricQuery,
    onMutate: () => setPreview(null),
    onSuccess: (response, request) => setPreview({ identity: JSON.stringify(request), result: response }),
    onError: (err: ApiError, request) => setPreview({ identity: JSON.stringify(request), error: err.message }),
  });

  const handleRunTest = () => {
    if (!firstQuerySource) return;
    // When no target was explicitly picked, fall back to the first one the
    // catalog returned so $host_* renders against a real host by default.
    if (mutation.isPending || testMutation.isPending) return;
    testMutation.mutate(testRequest);
  };
  const canTest =
    !!draft.query.trim() && !!firstQuerySource && !testMutation.isPending && !mutation.isPending && queryTypeCompatible;

  const renderTestResult = () => {
    let valueNode: ReactNode = null;
    if (testError) {
      valueNode = <p className="text-status-negative">{testError}</p>;
    } else if (testResult?.error) {
      valueNode = <p className="text-status-negative">{testResult.error}</p>;
    } else if (testResult?.value !== undefined) {
      valueNode = (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-all rounded bg-muted p-1.5 font-mono text-xs">
          {JSON.stringify(testResult.value, null, 2)}
        </pre>
      );
    }
    return (
      <output
        aria-live="polite"
        className="grid gap-1 rounded-md border border-border bg-background p-2 text-xs"
      >
        <div className="flex items-center justify-between">
          <span className="font-medium text-muted-foreground">
            {t("observability.metric.testResult", { defaultValue: "Test result" })}
          </span>
          {firstQuerySource ? (
            <span className="truncate text-xs text-muted-foreground">
              {firstQuerySource.name}
            </span>
          ) : null}
        </div>
        {testResult?.executed_query ? (
          <pre className="max-h-24 overflow-auto whitespace-pre-wrap break-all rounded bg-muted p-1.5 font-mono text-xs">
            {testResult.executed_query}
          </pre>
        ) : null}
        {valueNode}
        {testResult && !testResult.source_resolved ? (
          <p className="text-muted-foreground">
            {t("observability.metric.testNoResolvedSource", {
              defaultValue: "No source resolved.",
            })}
          </p>
        ) : null}
      </output>
    );
  };

  const renderEditor = () => (
    <EntityEditorPage scope="routed"
      dirtyMessage={t("editor.unsavedChanges")}
      width="form"
      dirty={JSON.stringify(draft) !== JSON.stringify(baseline)}
      title={item ? t("observability.metric.editTitle") : t("observability.metric.createTitle")}
      onBack={back}
      backLabel={t("back")}
      onCancel={back}
      cancelLabel={t("cancel")}
      onSave={submit}
      saveLabel={item ? t("save") : t("create")}
      saving={mutation.isPending}
      saveDisabled={!canSubmit}
      description={t("observability.metric.dialogDescription", {
        defaultValue: "A single query and how to render its value in inspectors.",
      })}
      >
        <fieldset disabled={mutation.isPending} className="grid min-w-0 gap-4">
          <div className="grid gap-4 sm:grid-cols-2">
            <FormField htmlFor="metric-query-name" label={t("observability.metric.name", { defaultValue: "Name" })} required>
              <Input id="metric-query-name" className="text-sm" value={draft.name} onChange={(e) => set("name", e.target.value)} />
            </FormField>
            <FormField htmlFor="metric-query-slug" label={t("observability.metric.slug", { defaultValue: "Slug" })} required>
              <Input
                className="text-sm font-mono"
                value={effectiveSlug}
                id="metric-query-slug"
                onChange={(e) => {
                  set("slug", e.target.value);
                  set("slugTouched", true);
                }}
              />
            </FormField>
          </div>

          <div className="grid gap-4 sm:grid-cols-2">
            <FormField htmlFor="metric-query-label" label={t("observability.metric.label", { defaultValue: "Label" })}>
              <Input id="metric-query-label" className="text-sm" value={draft.label} onChange={(e) => set("label", e.target.value)} />
            </FormField>
            <FormField
              label={t("observability.metric.unit", { defaultValue: "Unit" })}
              htmlFor="metric-query-unit"
              help={t("observability.metric.unitHint", {
                defaultValue:
                  "Used only for display formatting. Common units: %, s, B, B/s, state, count.",
              })}
            >
              <FormFieldControl><Input id="metric-query-unit" className="text-sm" value={draft.unit} onChange={(e) => set("unit", e.target.value)} /></FormFieldControl>
              <div className="flex flex-wrap gap-1">
                {UNIT_PRESETS.map((unit) => (
                  <Button
                    key={unit}
                    type="button"
                    variant={draft.unit === unit ? "secondary" : "outline"}
                    size="sm"
                    className="h-8 px-2 font-mono text-sm"
                    onClick={() => set("unit", unit)}
                  >
                    {unit}
                  </Button>
                ))}
              </div>
            </FormField>
          </div>

          <FormSection
            title={t("observability.metric.sourceAndQuery", { defaultValue: "Source and query" })}
            description={t("observability.metric.sourceAndQueryHint", {
                  defaultValue:
                    "Choose where this metric is read from. VictoriaMetrics uses the same PromQL query type as Prometheus.",
                })}
            contentClassName="grid gap-4"
          >
            <FormSelect
                  disabled={mutation.isPending}
              label={t("observability.source.family", { defaultValue: "Source family" })}
              value={draft.sourceType}
              onValueChange={handleSourceTypeChange}
              contentClassName="max-w-[--radix-select-trigger-width]"
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
            />
            <FormSelect
                  disabled={mutation.isPending}
              label={t("observability.metric.queryType", { defaultValue: "Query type" })}
              value={draft.queryType}
              onValueChange={handleQueryTypeChange}
              error={
                !queryTypeCompatible
                  ? t("observability.metric.incompatibleQueryType", {
                      defaultValue:
                        "This saved query type does not work with the selected source family. Pick a compatible query type before saving.",
                    })
                  : undefined
              }
              options={[
                ...(!queryTypeCompatible
                  ? [{
                      value: draft.queryType,
                      label: t("observability.metric.unsupportedQueryType", {
                        type: draft.queryType,
                        defaultValue: `Unsupported: ${draft.queryType}`,
                      }),
                    }]
                  : []),
                ...compatibleQueryTypes.map((type) => ({
                  value: type,
                  label: queryTypeLabel(type),
                })),
              ]}
            />

            <FormField
              required
              htmlFor="metric-query-expression"
              label={
                isZabbixSourceType(draft.sourceType)
                  ? t("observability.metric.zabbixQuery", { defaultValue: "Item selector" })
                  : t("observability.metric.promqlQuery", { defaultValue: "PromQL query" })
              }
              help={
                isZabbixSourceType(draft.sourceType)
                  ? t("observability.metric.zabbixQueryHint", {
                      defaultValue:
                        "Use an item key, item name, or item ID according to the selected query type.",
                    })
                  : t("observability.metric.promqlQueryHint", {
                      defaultValue:
                        "Use $host as the connection target placeholder. Prometheus and VictoriaMetrics both use PromQL here.",
                    })
              }
            >
              <div className="grid gap-2">
                <FormFieldControl><textarea
                  id="metric-query-expression"
                  className="min-h-[72px] rounded-md border border-input bg-background px-2 py-1.5 font-mono text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  value={draft.query}
                  onChange={(e) => set("query", e.target.value)}
                  placeholder={
                    isZabbixSourceType(draft.sourceType)
                      ? "system.cpu.load[percpu,avg1]"
                      : 'up{piping_host_name="$host",job="node"}'
                  }
                  spellCheck={false}
                /></FormFieldControl>
                <div className="flex flex-wrap items-center gap-2">
                  <FormSelect
                  disabled={mutation.isPending}
                    label={t("observability.metric.testTarget", { defaultValue: "Test target" })}
                    value={draft.testTargetId}
                    onValueChange={(v) => set("testTargetId", v)}
                    contentClassName="max-w-[--radix-select-trigger-width]"
                    options={[
                      {
                        value: "",
                        label: t("observability.metric.testTargetAuto", {
                          defaultValue: "First available",
                        }),
                      },
                      ...targetOptions.map((option) => ({
                        value: option.id,
                        label: option.name,
                      })),
                    ]}
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="ml-auto h-8 text-sm"
                    onClick={handleRunTest}
                    disabled={!canTest}
                    title={
                      !firstQuerySource
                        ? t("observability.metric.testNoSourceForType", {
                            type: sourceLabel(draft.sourceType),
                            defaultValue:
                              "Configure a {{type}} query source before running a live test.",
                          })
                        : undefined
                    }
                  >
                    {testMutation.isPending ? (
                      <Loader2 className="mr-1 h-3 w-3 animate-spin" />
                    ) : null}
                    {t("observability.metric.testButton", { defaultValue: "Test" })}
                  </Button>
                </div>
                {(testResult || testError) && renderTestResult()}
              </div>
            </FormField>
          </FormSection>

          <FormField htmlFor="metric-query-description" label={t("observability.metric.description", { defaultValue: "Description" })}>
            <Input
              className="text-sm"
              value={draft.description}
              id="metric-query-description"
              onChange={(e) => set("description", e.target.value)}
            />
          </FormField>

          <FormSection
            title={t("observability.metric.displayTitle", { defaultValue: "Display and colors" })}
            description={t("observability.metric.displayHint", {
                  defaultValue:
                    "Controls the compact inspector card: value formatting, widget shape, and status color thresholds.",
                })}
            contentClassName="grid gap-4"
          >
            <div className="grid gap-4 sm:grid-cols-2">
              <FormSelect
                  disabled={mutation.isPending}
                label={t("observability.metric.valueType", { defaultValue: "Value type" })}
                value={draft.valueType}
                onValueChange={(v) => set("valueType", v)}
                help={t("observability.metric.valueTypeHint", {
                  defaultValue:
                    "Pick auto unless the raw result needs state, percent, bytes, duration, log, or table rendering.",
                })}
                options={VALUE_TYPES.map((type) => ({
                  value: type,
                  label: t(`observability.metric.valueTypes.${type}`, { defaultValue: type }),
                }))}
              />
              <FormSelect
                  disabled={mutation.isPending}
                label={t("observability.metric.widgetType", { defaultValue: "Widget" })}
                value={draft.widgetType}
                onValueChange={(v) => set("widgetType", v)}
                help={t("observability.metric.widgetTypeHint", {
                  defaultValue:
                    "Stat is compact; sparkline draws recent samples while the inspector is open; badge is best for states; gauge shows a small range bar.",
                })}
                options={WIDGET_TYPES.map((type) => ({
                  value: type,
                  label: t(`observability.metric.widgetTypes.${type}`, { defaultValue: type }),
                }))}
              />
            </div>
            <ThresholdEditor
              disabled={mutation.isPending}
              value={draft.threshold}
              onChange={(next) => set("threshold", next)}
              bare
              preview={{
                label: draft.label.trim() || draft.name.trim() || "preview",
                unit: draft.unit.trim() || null,
                sampleValue:
                  testResult?.value !== undefined && testResult.value !== null
                    ? testResult.value
                    : undefined,
                sampleText: draft.previewSampleText,
                onSampleTextChange: (v) => set("previewSampleText", v),
              }}
            />
          </FormSection>

          <div className="grid gap-4 sm:grid-cols-3">
            <label className="flex items-center justify-between rounded-md border border-border px-3 py-2">
              <span className="text-sm font-medium">
                {t("observability.metric.showByDefault", { defaultValue: "Show in inspector" })}
              </span>
              <Switch
                checked={draft.defaultShowInInspector}
                onCheckedChange={(v) => set("defaultShowInInspector", v)}
              />
            </label>
            <label className="flex items-center justify-between rounded-md border border-border px-3 py-2">
              <span className="text-sm font-medium">
                {t("observability.metric.showInList", { defaultValue: "Show in list" })}
              </span>
              <Switch
                checked={draft.default_show_in_list}
                onCheckedChange={(v) => set("default_show_in_list", v)}
              />
            </label>
            <label className="flex items-center justify-between rounded-md border border-border px-3 py-2">
              <span className="text-sm font-medium">
                {t("observability.metric.enabled", { defaultValue: "Enabled" })}
              </span>
              <Switch checked={draft.enabled} onCheckedChange={(v) => set("enabled", v)} />
            </label>
          </div>
        </fieldset>

    </EntityEditorPage>
  );

  return renderEditor();
};

/**
 * Routed create/edit page for a metric query: `/observability/metrics/new` and
 * `/observability/metrics/:id/edit`.
 */
const MetricQueryEditor = () => {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery({ queryKey: METRIC_QUERIES_KEY, queryFn: () => listMetricQueries() });
  const item = query.data?.items.find(candidate => candidate.id === id) ?? null;
  if (query.isPending || query.isError || (id && !item)) {
    return <EntityEditorPage scope="routed" width="form"
      title={t(id ? "observability.metric.editTitle" : "observability.metric.createTitle")}
      onBack={() => { void navigate(observabilityListHref(METRIC_QUERIES_VIEW)); }}
      backLabel={t("back")} cancelLabel={t("cancel")}>
      {query.isPending ? <output className="flex justify-center p-8"><Loader2 className="size-6 animate-spin" aria-label={t("loading")} /></output>
        : <EmptyState icon={ListChecks} title={t(query.isError ? "observability.entityLoadFailed" : "observability.entityNotFound")}
          actionLabel={query.isError ? t("retry") : undefined}
          onAction={query.isError ? () => { void query.refetch(); } : undefined} />}
    </EntityEditorPage>;
  }

  return (
    <RequireCapability check={(p) => p.can(id ? "UPDATE" : "CREATE", "MONITORING")}>
      <MetricQueryForm key={id ?? "new"} item={item} />
    </RequireCapability>
  );
};

export default MetricQueryEditor;
