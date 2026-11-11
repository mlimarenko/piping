import { useState, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import PageChrome from "@/shared/ui-shared/PageChrome";
import DataTable, { type DataTableSortState, type DataTableFilterOption } from "@/shared/ui-shared/DataTable";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import type { EntityOption } from "@/shared/ui-shared/EntitySelect";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import {
  CalendarDays,
  Database,
  Eye,
  RotateCcw,
  ScrollText,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { cn } from "@/shared/lib/utils";
import { getAuditLog, listAuditLogs } from "@/features/audit/api";
import { AuditObjectMultiPicker } from "@/features/audit/components/AuditObjectMultiPicker";
import { AuditMetadata } from "@/features/audit/components/AuditMetadata";
import { AUDIT_CHANNELS, auditChannelIcon } from "@/features/audit/channels";
import { formatAuditTimestamp, formatAuditTimestampUtc } from "@/features/audit/lib/auditTime";
import {
  AUDIT_TABLE_CONFIG,
  AUDIT_FILTER_DIMENSION_BY_COLUMN,
  AUDIT_SORTABLE_COLUMN_KEYS,
  auditSortParamForColumn,
  buildAuditFilterParams,
} from "@/features/audit/tableConfig";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";
import type { AuditEntry, AuditLookupOption, AuditObjectRef } from "@/shared/api/types";

type Translate = (key: string, options?: Record<string, unknown>) => string;
const EMPTY_AUDIT_ENTRIES: AuditEntry[] = [];

const AUDIT_EVENT_FILTERS = [
  "connection.session.created", "connection.session.activated", "connection.session.websocket.connected",
  "connection.session.disconnected", "connection.session.terminated", "connection.session.failed",
  "mcp.tool.invoked", "access_control.user_api_token.created", "access_control.user_api_token.revoked",
  "playbook.created", "playbook.version.created", "playbook.updated", "playbook.deleted",
  "playbook.target_set.created", "playbook.assignment.created", "playbook.run_group.queued",
  "playbook.artifact.created", "playbook.run_event.appended", "playbook.run.completed", "secrets.resolve",
  "inventory.custom_fields.reveal", "inventory.fhrp_group.auth_key.reveal",
  "timeline.signin_completed", "timeline.signin_rejected", "timeline.signup_completed",
  "timeline.signup_rejected", "timeline.activation_completed", "timeline.activation_rejected",
  "inventory.tenant.created", "inventory.tenant.updated", "inventory.tenant.deleted",
  "connection.session.websocket.disconnected", "connection.session.websocket.failed",
  "connection.session.websocket.attach_denied", "connection.session.observe.started",
  "connection.session.observe.stopped", "connection.session.observe.denied", "mcp.resource.read",
] as const;

const AUDIT_TARGET_TYPE_FILTERS = [
  { value: "organization", key: "organization" },
  { value: "user", key: "user" },
  { value: "user_api_token", key: "apiToken" },
  { value: "connection_session", key: "connectionSession" },
  { value: "connection_config:record", key: "connectionConfigRecord" },
  { value: "connection_target", key: "connectionTarget" },
  { value: "device", key: "device" },
  { value: "virtual_machine", key: "virtualMachine" },
  { value: "playbook", key: "playbook" },
  { value: "playbook_version", key: "playbookVersion" },
  { value: "playbook_target_set", key: "playbookTargetSet" },
  { value: "playbook_assignment", key: "playbookAssignment" },
  { value: "playbook_run_group", key: "playbookRunGroup" },
  { value: "playbook_run", key: "playbookRun" },
  { value: "playbook_schedule", key: "playbookSchedule" },
  { value: "playbook_artifact", key: "playbookArtifact" },
  { value: "playbook_run_event", key: "playbookRunEvent" },
  { value: "playbook_job_token", key: "playbookJobToken" },
] as const;

const AUDIT_RESULT_FILTERS = [
  "succeeded",
  "failed",
  "denied",
  "unknown",
  "not_applicable",
] as const;

/** Relative date-range presets: each sets `created_from` to N hours before now. */
const AUDIT_DATE_PRESETS = [
  { key: "lastHour", hours: 1 },
  { key: "last24h", hours: 24 },
  { key: "last7d", hours: 24 * 7 },
] as const;

type FilterChip = {
  key: string;
  label: string;
  onClear: () => void;
};

const FILTER_CONTROL_CLASS = "h-9 text-sm";
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const LIFECYCLE_STATES: Record<string, string[]> = {
  session: ["pending", "active", "disconnected", "terminated", "failed"],
  transport: ["connected", "disconnected", "detached", "failed"],
  automation: ["queued", "leased", "dispatching", "running", "succeeded", "failed", "canceled", "timed_out", "unreported"],
};

function eventKey(eventType: string): string {
  return eventType.replace(/[^a-zA-Z0-9]+/g, "_").replace(/^_+|_+$/g, "");
}

function humanizeEventType(eventType: string): string {
  return eventType
    .split(/[._-]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function objectLabel(ref: AuditObjectRef | undefined, fallback: string): string {
  return ref?.label || fallback;
}

function recordedName(...labels: (string | null | undefined)[]): string | undefined {
  return labels.find((label): label is string => typeof label === "string" && !!label.trim() && !UUID_PATTERN.test(label.trim()));
}

function actorLabel(entry: AuditEntry, t: Translate): string {
  return recordedName(entry.context?.display?.actor, entry.actorRef?.label, entry.actor)
    ?? (entry.actorKind === "system" ? t("auditLog.actorKinds.system") : undefined)
    ?? (entry.actorRef?.id || UUID_PATTERN.test(entry.actor) ? t("auditLog.actorNameMissing") : t("auditLog.unknownActor"));
}

function kindLabel(t: Translate, kind?: string): string {
  if (!kind) return "";
  const inventoryKind = kind === "inventory:dcim.device" ? "device"
    : kind === "inventory:virtualization.virtualmachine" ? "virtual_machine" : kind;
  if (inventoryKind.startsWith("inventory:")) return t("auditLog.inventoryObject");
  const auditKind = inventoryKind === "connection_config:record" ? "connection_config_record" : inventoryKind;
  if (!/^[a-z][a-z0-9_]*$/.test(auditKind)) return t("auditLog.unknownResourceType");
  const fallback = t(`auditLog.resourceKinds.${auditKind}`, { defaultValue: t("auditLog.unknownResourceType") });
  return inventoryKind === auditKind ? t(`accessControl.policies.resourceTypes.${inventoryKind}`, { defaultValue: fallback }) : fallback;
}

function actorKindLabel(t: Translate, kind?: string): string {
  if (!kind) return "";
  return t(`auditLog.actorKinds.${eventKey(kind)}`, { defaultValue: humanizeEventType(kind) });
}

function lifecycleLabel(t: Translate, state: { domain: string; value: string }): string {
  const knownValues = Object.hasOwn(LIFECYCLE_STATES, state.domain) ? LIFECYCLE_STATES[state.domain] : undefined;
  if (!knownValues || !knownValues.includes(state.value)) return t("auditLog.unknownState");
  return `${t(`auditLog.stateDomains.${state.domain}`)} / ${t(`auditLog.states.${state.value}`)}`;
}

function knownLabel(t: Translate, group: "decisions" | "emitters", value: string, known: readonly string[]): string {
  return known.includes(value) ? t(`auditLog.${group}.${value}`) : t(`auditLog.unknown${group === "decisions" ? "Decision" : "Emitter"}`);
}

function formatAuditAction(entry: Pick<AuditEntry, "action">, t: Translate): string {
  return t(`auditLog.events.${eventKey(entry.action)}.action`, {
    defaultValue: entry.action,
  });
}

function formatAuditResult(entry: AuditEntry, t: Translate): string {
  const outcome = entry.context?.outcome ?? "unknown";
  return t(`auditLog.outcomes.${outcome}`, { defaultValue: t("auditLog.outcomes.unknown") });
}

function actionClassName(entry: AuditEntry): string {
  const result = entry.context?.outcome ?? "unknown";
  if (result === "denied" || result === "failed") {
    return "bg-status-negative/10 text-status-negative border border-status-negative/15";
  }
  if (entry.action.includes("terminated") || entry.action.includes("disconnected")) {
    return "bg-status-warning/10 text-status-warning border border-status-warning/15";
  }
  if (entry.action.startsWith("mcp.")) {
    return "bg-status-info/10 text-status-info border border-status-info/15";
  }
  if (result === "succeeded") {
    return "bg-status-positive/10 text-status-positive border border-status-positive/15";
  }
  return "surface-chip";
}

function toIsoDateTime(value: string): string | undefined {
  if (!value) return undefined;
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return undefined;
  return parsed.toISOString();
}

/** Format a Date as a `datetime-local` wall-clock value (`YYYY-MM-DDTHH:mm`). */
function toDatetimeLocal(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function DetailRow({ label, value, title }: { label: string; value?: string; title?: string }) {
  if (!value) return null;
  return (
    <div className="grid grid-cols-1 gap-1 sm:grid-cols-[8.5rem_minmax(0,1fr)] sm:gap-3">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className="min-w-0 [overflow-wrap:anywhere] text-sm sm:text-right font-mono text-foreground" title={title ?? value}>
        {value}
      </span>
    </div>
  );
}

const AuditLog = () => {
  const { t, i18n } = useTranslation();
  const [page, setPage] = useState(0);
  const { value: pageSize, setValue: setPageSize } = useTablePageSize();
  const [search, setSearch] = useState("");
  const [actorId, setActorId] = useState("");
  const [targetFilters, setTargetFilters] = useState<AuditLookupOption[]>([]);
  // Channel (`actor_kind`) multiselect — the headline dimension separating
  // interactive UI / MCP / API tokens / runner / service principals / system.
  const [channels, setChannels] = useState<string[]>([]);
  // Canonical multi-value store for the three IN-filter dimensions
  // (event_type / target_type / outcome), keyed by the table column key
  // (`action` / `target` / `outcome`). The toolbar MultiSelects below are views
  // over these arrays, and the DataTable funnel menu is another editor of the
  // same store — one state, one query param, no double-apply.
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [detailEntry, setDetailEntry] = useState<AuditEntry | null>(null);
  const [stateDomain, setStateDomain] = useState("");
  const [stateValue, setStateValue] = useState("");
  const [correlation, setCorrelation] = useState<{ key: "requestId" | "sessionId" | "runId" | "runGroupId"; value: string } | null>(null);
  const primarySearchInput = useRef<HTMLInputElement | null>(null);
  const detailFocus = useCollectionPreviewFocus(primarySearchInput);
  const [showAdvancedFilters, setShowAdvancedFilters] = useState(false);
  const auditFilterKeys = useMemo(
    () => AUDIT_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
    [],
  );
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, AUDIT_SORTABLE_COLUMN_KEYS),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () => sanitizeDataTableColumnFilters(columnFilters, auditFilterKeys),
    [auditFilterKeys, columnFilters],
  );

  const debouncedSearch = useDebouncedValue(search);

  // The three IN-filter arrays, read from the canonical `columnFilters` store.
  // Memoized so the `?? []` default is a stable reference across renders (these
  // feed several useMemo dep arrays).
  const eventTypes = useMemo(
    () => sanitizedColumnFilters.action ?? [],
    [sanitizedColumnFilters.action],
  );
  const targetTypes = useMemo(
    () => sanitizedColumnFilters.target ?? [],
    [sanitizedColumnFilters.target],
  );
  const results = useMemo(
    () => sanitizedColumnFilters.outcome ?? [],
    [sanitizedColumnFilters.outcome],
  );
  // The target picker scopes its lookup by a single target type. With exactly
  // one target type selected it scopes; with 0 or 2+ it searches all types. A
  // user may then pick a target whose type is not in the set → zero rows (the
  // backend ANDs target_type and target_id); that is the user's choice.
  const targetTypeForPicker = targetTypes.length === 1 ? targetTypes[0] : undefined;

  const actorIds = useMemo(() => UUID_PATTERN.test(actorId) ? [actorId] : [], [actorId]);
  const targetIds = useMemo(() => targetFilters.map((option) => option.id), [targetFilters]);

  const auditFilters = useMemo(() => ({
    query: debouncedSearch.trim() || undefined,
    eventTypes,
    actorKinds: channels,
    actorIds,
    targetTypes,
    targetIds,
    outcomes: results,
    stateDomain: stateDomain || undefined,
    stateValue: stateValue.trim() || undefined,
    ...(correlation && UUID_PATTERN.test(correlation.value) ? { [correlation.key]: correlation.value } : {}),
    from: toIsoDateTime(from),
    to: toIsoDateTime(to),
  }), [actorIds, channels, correlation, debouncedSearch, eventTypes, from, results, stateDomain, stateValue, targetIds, targetTypes, to]);

  // Empty arrays must NOT count as active (Boolean([]) is true); check length
  // for the array dims and Boolean for the scalars explicitly.
  const hasActiveFilters =
    eventTypes.length > 0 ||
    channels.length > 0 ||
    Boolean(actorId) ||
    targetTypes.length > 0 ||
    targetIds.length > 0 ||
    results.length > 0 ||
    Boolean(auditFilters.query) ||
    Boolean(stateDomain || stateValue || correlation) ||
    Boolean(auditFilters.from) ||
    Boolean(auditFilters.to);
  const advancedFilterCount = [
    targetTypes.length > 0,
    targetIds.length > 0,
    results.length > 0,
    Boolean(stateDomain || stateValue),
    Boolean(correlation),
    Boolean(from),
    Boolean(to),
  ].filter(Boolean).length;

  const activeFilterChips = useMemo<FilterChip[]>(() => {
    const chips: FilterChip[] = [];
    if (correlation) chips.push({ key: "correlation", label: `${t(`auditLog.correlation.${correlation.key}`)}: ${correlation.value}`, onClear: () => { setCorrelation(null); setPage(0); } });
    if (stateDomain || stateValue) chips.push({ key: "state", label: `${t("auditLog.lifecycleState")}: ${stateValue ? lifecycleLabel(t, { domain: stateDomain, value: stateValue }) : t(`auditLog.stateDomains.${stateDomain}`)}`, onClear: () => { setStateDomain(""); setStateValue(""); setPage(0); } });
    const trimmedSearch = search.trim();
    if (trimmedSearch) {
      chips.push({
        key: "query",
        label: `${t("auditLog.queryFilter")}: ${trimmedSearch}`,
        onClear: () => { setSearch(""); setPage(0); },
      });
    }
    if (channels.length > 0) {
      const labels = channels.map((value) => actorKindLabel(t, value)).join(", ");
      chips.push({
        key: "channel",
        label: `${t("auditLog.channel")}: ${labels}`,
        onClear: () => { setChannels([]); setPage(0); },
      });
    }
    if (actorId) {
      chips.push({
        key: "actor",
        label: `${t("auditLog.actor")}: ${actorId}`,
        onClear: () => { setActorId(""); setPage(0); },
      });
    }
    if (eventTypes.length > 0) {
      const labels = eventTypes
        .map((value) => {
           return formatAuditAction({ action: value }, t);
        })
        .join(", ");
      chips.push({
        key: "eventType",
        label: `${t("auditLog.action")}: ${labels}`,
        onClear: () => { setColumnFilter("action", []); },
      });
    }
    if (targetFilters.length > 0) {
      const labels = targetFilters.map((option) => option.label).join(", ");
      chips.push({
        key: "target",
        label: `${t("auditLog.target")}: ${labels}`,
        onClear: () => { setTargetFilters([]); setPage(0); },
      });
    }
    if (targetTypes.length > 0) {
      const labels = targetTypes
        .map((value) => kindLabel(t, value))
        .join(", ");
      chips.push({
        key: "targetType",
        label: `${t("auditLog.resourceType")}: ${labels}`,
        onClear: () => { setColumnFilter("target", []); },
      });
    }
    if (results.length > 0) {
      const labels = results
        .map((value) => t(`auditLog.outcomes.${eventKey(value)}`, { defaultValue: value }))
        .join(", ");
      chips.push({
        key: "result",
        label: `${t("auditLog.result")}: ${labels}`,
        onClear: () => { setColumnFilter("outcome", []); },
      });
    }
    if (from) {
      chips.push({
        key: "from",
        label: `${t("auditLog.fromPlaceholder")}: ${from}`,
        onClear: () => { setFrom(""); setPage(0); },
      });
    }
    if (to) {
      chips.push({
        key: "to",
        label: `${t("auditLog.toPlaceholder")}: ${to}`,
        onClear: () => { setTo(""); setPage(0); },
      });
    }
    return chips;
  }, [actorId, channels, correlation, eventTypes, from, results, search, stateDomain, stateValue, t, targetFilters, targetTypes, to]);

  // Column key -> backend sort key happens only here, at query-build time;
  // `sortState.key` stays the column key so the table's sort indicator matches.
  const sortKey = auditSortParamForColumn(sanitizedSortState?.key);
  const auditColumnFilterParams = useMemo(
    () => buildAuditFilterParams(sanitizedColumnFilters),
    [sanitizedColumnFilters],
  );
  const auditQuery = useQuery({
    queryKey: ["audit", {
      ...auditFilters,
      ...auditColumnFilterParams,
      sort: sortKey,
      order: sanitizedSortState?.dir,
      page,
      pageSize,
    }],
    queryFn: () =>
      listAuditLogs({
        ...auditFilters,
        ...auditColumnFilterParams,
        sort: sortKey,
        order: sanitizedSortState?.dir,
        limit: pageSize,
        offset: page * pageSize,
      }),
    placeholderData: (prev) => prev,
  });
  const logs = auditQuery.data?.items ?? EMPTY_AUDIT_ENTRIES;
  const total = auditQuery.data?.total ?? 0;
  const detailQuery = useQuery({
    queryKey: ["audit", "detail", detailEntry?.id],
    queryFn: () => getAuditLog(detailEntry!.id),
    enabled: !!detailEntry,
  });

  const handlePageChange = (p: number) => setPage(p);
  const handlePageSizeChange = (s: number) => { setPageSize(s); setPage(0); };

  // Sort change resets to page 1 (the visible row set changes).
  const handleSortChange = (next: DataTableSortState | null) => {
    setSortState(next);
    setPage(0);
  };

  // Canonical writer for the three IN-filter dimensions. Drives the funnel menu,
  // the toolbar MultiSelects, and the active-filter chips through one store.
  // Changing `target` (target type) also clears any picked targets — they were
  // scoped to the previous target-type set.
  const setColumnFilter = (key: string, values: string[]) => {
    setColumnFilters((prev) => updateDataTableColumnFilters(prev, key, values));
    if (key === "target") setTargetFilters([]);
    setPage(0);
  };

  const applyDatePreset = (hours: number) => {
    setFrom(toDatetimeLocal(new Date(Date.now() - hours * 3_600_000)));
    setTo("");
    setPage(0);
  };

  const resetFilters = () => {
    setSearch("");
    setActorId("");
    setTargetFilters([]);
    setChannels([]);
    setColumnFilters({});
    setStateDomain("");
    setStateValue("");
    setCorrelation(null);
    setFrom("");
    setTo("");
    setPage(0);
  };

  // Static MultiSelect option lists (id + localized label), memoized on `t`.
  const channelItems = useMemo<EntityOption[]>(
    () => AUDIT_CHANNELS.map((channel) => ({
      id: channel.value,
      label: actorKindLabel(t, channel.value),
      icon: channel.icon,
    })),
    [t],
  );
  const eventTypeItems = useMemo<EntityOption[]>(
    () => Array.from(new Set([
      ...AUDIT_EVENT_FILTERS,
      ...logs.map(entry => entry.action), ...eventTypes,
    ])).map(action => ({ id: action, label: formatAuditAction({ action }, t) })),
    [eventTypes, logs, t],
  );
  const targetTypeItems = useMemo<EntityOption[]>(
    () => AUDIT_TARGET_TYPE_FILTERS.map((option) => ({ id: option.value, label: kindLabel(t, option.value) })),
    [t],
  );
  const resultItems = useMemo<EntityOption[]>(
    () => AUDIT_RESULT_FILTERS.map((value) => ({ id: value, label: t(`auditLog.outcomes.${eventKey(value)}`, { defaultValue: value }) })),
    [t],
  );

  // Per-column funnel options for the three IN-filter dimensions, resolved from
  // the same static option lists the toolbar MultiSelects use.
  const filterOptionsByColumn: Record<string, DataTableFilterOption[]> = {
    action: eventTypeItems.map(option => ({ value: option.id, label: option.label })),
    target: AUDIT_TARGET_TYPE_FILTERS.map((o) => ({ value: o.value, label: kindLabel(t, o.value) })),
    outcome: AUDIT_RESULT_FILTERS.map((value) => ({
      value,
      label: t(`auditLog.outcomes.${eventKey(value)}`, { defaultValue: value }),
    })),
  };

  // Sort/filter attributes for a column. Sort defaults on for labeled columns in
  // controlled-sort mode, so every visible data column maps through tableConfig.
  const columnMeta = (key: string): { sortable: boolean; filterOptions?: DataTableFilterOption[] } => ({
    sortable: AUDIT_SORTABLE_COLUMN_KEYS.has(key),
    filterOptions: key in AUDIT_FILTER_DIMENSION_BY_COLUMN ? filterOptionsByColumn[key] : undefined,
  });

  const auditLocale = i18n.resolvedLanguage ?? i18n.language;
  const columns = [
    { key: "timestamp", label: t("auditLog.timestamp"), ...columnMeta("timestamp"), render: (r: AuditEntry) => {
      const localTimestamp = formatAuditTimestamp(r.timestamp, auditLocale);
      const utcTimestamp = formatAuditTimestampUtc(r.timestamp);
      return (
        <span
          className="font-mono text-xs text-muted-foreground"
          title={`${localTimestamp}\n${t("auditLog.utcTimestamp")}: ${utcTimestamp}`}
        >
          {localTimestamp}
        </span>
      );
    } },
    { key: "actor", label: t("auditLog.actor"), ...columnMeta("actor"), render: (r: AuditEntry) => {
      const actor = actorLabel(r, t);
      const identity = r.actorRef?.id || (UUID_PATTERN.test(r.actor) ? r.actor : undefined);
      const via = r.actorRef?.secondary || (!recordedName(r.context?.display?.actor, r.actorRef?.label, r.actor) ? identity : undefined);
      const ChannelIcon = auditChannelIcon(r.actorKind);
      const channelLabel = actorKindLabel(t, r.actorKind);
      return (
        <div className="flex min-w-0 max-w-[18rem] items-start gap-1.5">
          <span
            className="mt-0.5 shrink-0 text-muted-foreground"
            title={`${t("auditLog.channel")}: ${channelLabel}`}
            aria-label={`${t("auditLog.channel")}: ${channelLabel}`}
          >
            <ChannelIcon className="size-4" />
          </span>
          <span className="min-w-0">
            <span className="block truncate font-medium text-foreground" title={actor}>{actor}</span>
            {via && <span className="block truncate text-xs text-muted-foreground" title={via}>{via}</span>}
          </span>
        </div>
      );
    } },
    { key: "action", label: t("auditLog.action"), ...columnMeta("action"), render: (r: AuditEntry) => (
      <span className={`inline-flex max-w-[20rem] whitespace-normal rounded-md px-2 py-1 text-xs font-medium ${actionClassName(r)}`} title={r.action}>
        {formatAuditAction(r, t)}
      </span>
    )},
    { key: "target", label: t("auditLog.target"), ...columnMeta("target"), render: (r: AuditEntry) => {
      const primary = r.targetRef;
      const secondary = primary
        ? [kindLabel(t, primary.kind), primary.secondary].filter(Boolean).join(" · ")
        : kindLabel(t, r.resourceType);
      return (
        <div className="min-w-0 max-w-[18rem]">
          <span className="block truncate text-foreground" title={primary?.id || r.resourceId}>
            {recordedName(r.context?.display?.target, primary?.label) || (r.resourceId ? t("auditLog.targetNameMissing") : t("auditLog.noTarget"))}
          </span>
          {secondary && <span className="block truncate text-xs text-muted-foreground" title={r.resourceType || primary?.kind}>{secondary}</span>}
        </div>
      );
    } },
    { key: "outcome", label: t("auditLog.result"), ...columnMeta("outcome"), render: (r: AuditEntry) => <div className="text-sm">{formatAuditResult(r, t)}{r.context?.state && <div className="text-xs text-muted-foreground" title={`${r.context.state.domain} / ${r.context.state.value}`}>{t("auditLog.lifecycleState")}: {lifecycleLabel(t, r.context.state)}</div>}</div> },
    {
      key: "details", label: "", excludeFromSettings: true, render: (r: AuditEntry) => (
        <Button variant="ghost" size="icon-sm" aria-label={t("auditLog.detailsLabel")} onClick={event => { detailFocus.capture(event.currentTarget); setDetailEntry(r); }}>
          <Eye className="h-4 w-4" />
        </Button>
      )
    },
  ];

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageChrome
        title={t("auditLog.title")}
        icon={ScrollText}
        description={t("auditLog.description")}
        badge={`${total} ${t("items")}`}
      />
      <DataTable
        tableId="audit.log"
        density="comfortable"
        columns={columns}
        data={logs}
        getRowKey={r => r.id}
        loading={auditQuery.isPending}
        error={auditQuery.isError ? t("auditLog.loadError") : undefined}
        onRetry={() => auditQuery.refetch()}
        emptyState={
          <EmptyState
            icon={ScrollText}
            title={t("auditLog.noEntries")}
            description={t("auditLog.noEntriesDescription")}
          />
        }
        pagination={{ page, pageSize, total, onPageChange: handlePageChange, onPageSizeChange: handlePageSizeChange }}
        sortState={sanitizedSortState}
        onSortChange={handleSortChange}
        columnFilters={sanitizedColumnFilters}
        onColumnFilterChange={setColumnFilter}
        toolbar={
          <div className="flex w-full flex-col gap-2" data-audit-toolbar>
            <div className="flex w-full flex-wrap items-start gap-2">
              <SearchInput
                ref={primarySearchInput}
                value={search}
                onValueChange={(v) => { setSearch(v); setPage(0); }}
                placeholder={t("auditLog.searchPlaceholder")}
                className={cn(FILTER_CONTROL_CLASS, "pl-8")}
              />
              <div className="min-w-[11rem] flex-1 basis-[12rem] lg:max-w-[15rem]">
                <MultiSelect
                  items={channelItems}
                  value={channels}
                  onValueChange={(values) => { setChannels(values); setPage(0); }}
                  placeholder={t("auditLog.channelPlaceholder")}
                  searchPlaceholder={t("auditLog.channelSearchPlaceholder")}
                  emptyText={t("auditLog.lookupEmpty")}
                  aria-label={t("auditLog.channel")}
                  triggerClassName={FILTER_CONTROL_CLASS}
                />
              </div>
              <div className="min-w-[11rem] flex-1 basis-[11rem] lg:max-w-[16rem]">
                <MultiSelect
                  items={eventTypeItems}
                  value={eventTypes}
                  onValueChange={(values) => setColumnFilter("action", values)}
                  placeholder={t("auditLog.allTypes")}
                  searchPlaceholder={t("auditLog.eventTypeSearchPlaceholder")}
                  emptyText={t("auditLog.lookupEmpty")}
                  aria-label={t("auditLog.action")}
                  triggerClassName={FILTER_CONTROL_CLASS}
                />
              </div>
              <Button
                variant="ghost"
                type="button"
                className={cn(
                  "shrink-0",
                  showAdvancedFilters && "border border-primary/40 bg-primary/15 text-primary",
                )}
                aria-expanded={showAdvancedFilters}
                title={t("auditLog.moreFilters")}
                onClick={() => setShowAdvancedFilters((value) => !value)}
              >
                <SlidersHorizontal className="size-4" />
                <span className="hidden sm:inline">{t("auditLog.moreFilters")}</span>
                {advancedFilterCount > 0 && (
                  <span className="ml-0.5 rounded bg-primary/15 px-1.5 py-0.5 text-xs font-semibold text-primary">
                    {advancedFilterCount}
                  </span>
                )}
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                className="shrink-0"
                disabled={!hasActiveFilters}
                aria-label={t("auditLog.clearFilters")}
                title={t("auditLog.clearFilters")}
                onClick={resetFilters}
              >
                <RotateCcw className="size-4" />
              </Button>
              <Button variant="outline" aria-pressed={results.length === 2 && results.includes("failed") && results.includes("denied")} onClick={() => setColumnFilter("outcome", results.length === 2 && results.includes("failed") && results.includes("denied") ? [] : ["failed", "denied"])}>
                {t("auditLog.errorsOnly")}
              </Button>
              <Button variant="outline" aria-pressed={stateDomain === "session"} onClick={() => { setStateDomain(stateDomain === "session" ? "" : "session"); setStateValue(""); setPage(0); }}>
                {t("auditLog.sessionLifecycle")}
              </Button>
            </div>
             {showAdvancedFilters && (
              <div className="grid w-full grid-cols-1 gap-3 rounded-md border border-border bg-card p-2 md:grid-cols-2 xl:grid-cols-4">
                <label className="grid min-w-0 gap-1 text-xs">{t("auditLog.actorIdFilter")}
                  <Input value={actorId} aria-invalid={!!actorId && !UUID_PATTERN.test(actorId)} onChange={e => { setActorId(e.target.value.trim()); setPage(0); }} />
                  {!!actorId && !UUID_PATTERN.test(actorId) && <span role="alert">{t("auditLog.invalidUuid")}</span>}
                </label>
                <label className="grid gap-1 text-xs">{t("auditLog.stateDomain")}
                  <select className="h-9 rounded-md border border-input bg-background px-2 text-sm" value={stateDomain} onChange={e => { setStateDomain(e.target.value); setStateValue(""); setPage(0); }}>
                    <option value="">{t("auditLog.anyStateDomain")}</option>
                    {["session", "transport", "automation"].map(value => <option key={value} value={value}>{t(`auditLog.stateDomains.${value}`)}</option>)}
                  </select>
                </label>
                <label className="grid gap-1 text-xs">{t("auditLog.stateValue")}
                  <select className="h-9 rounded-md border border-input bg-background px-2 text-sm" value={stateValue} disabled={!stateDomain} onChange={e => { setStateValue(e.target.value); setPage(0); }}>
                    <option value="">{t("auditLog.anyStateValue")}</option>
                    {(LIFECYCLE_STATES[stateDomain] ?? []).map(value => <option key={value} value={value}>{t(`auditLog.states.${value}`)}</option>)}
                  </select>
                </label>
                {(["requestId", "sessionId", "runId", "runGroupId"] as const).map(key => (
                  <label key={key} className="grid gap-1 text-xs">{t(`auditLog.correlation.${key}`)}
                    <Input aria-invalid={correlation?.key === key && !UUID_PATTERN.test(correlation.value)} value={correlation?.key === key ? correlation.value : ""} onChange={e => { setCorrelation(e.target.value ? { key, value: e.target.value.trim() } : null); setPage(0); }} />
                    {correlation?.key === key && !UUID_PATTERN.test(correlation.value) && <span role="alert">{t("auditLog.invalidUuid")}</span>}
                  </label>
                ))}
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.target")}
                  </span>
                  <AuditObjectMultiPicker
                    value={targetFilters}
                    onChange={(value) => { setTargetFilters(value); setPage(0); }}
                    targetType={targetTypeForPicker}
                    placeholder={t("auditLog.targetPickerPlaceholder")}
                    searchPlaceholder={t("auditLog.targetSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    loadingText={t("auditLog.lookupLoading")}
                    clearText={t("auditLog.clearTargetFilter")}
                    groupLabel={t("auditLog.lookupGroup")}
                    kindLabel={(kind) => kindLabel(t, kind)}
                    icon={Database}
                  />
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.targetType")}
                  </span>
                  <MultiSelect
                    items={targetTypeItems}
                    value={targetTypes}
                    onValueChange={(values) => setColumnFilter("target", values)}
                    placeholder={t("auditLog.allTargetTypes")}
                    searchPlaceholder={t("auditLog.targetTypeSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    aria-label={t("auditLog.targetType")}
                    triggerClassName={cn(FILTER_CONTROL_CLASS, "w-full")}
                  />
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.result")}
                  </span>
                  <MultiSelect
                    items={resultItems}
                    value={results}
                    onValueChange={(values) => setColumnFilter("outcome", values)}
                    placeholder={t("auditLog.allResults")}
                    searchPlaceholder={t("auditLog.resultSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    aria-label={t("auditLog.result")}
                    triggerClassName={cn(FILTER_CONTROL_CLASS, "w-full")}
                  />
                </div>
                <div className="min-w-0 space-y-1 md:col-span-2 xl:col-span-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.dateRange")}
                  </span>
                  <div className="flex flex-wrap gap-1">
                    {AUDIT_DATE_PRESETS.map((preset) => (
                      <Button
                        key={preset.key}
                        type="button"
                        variant="outline"
                        className="px-2 font-normal"
                        onClick={() => applyDatePreset(preset.hours)}
                      >
                        {t(`auditLog.presets.${preset.key}`)}
                      </Button>
                    ))}
                  </div>
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.fromPlaceholder")}
                  </span>
                  <div className="relative min-w-0">
                    <CalendarDays className="absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                    <Input
                      type="datetime-local"
                      value={from}
                      onChange={e => { setFrom(e.target.value); setPage(0); }}
                      aria-label={t("auditLog.fromPlaceholder")}
                      title={t("auditLog.fromPlaceholder")}
                      className="min-w-0 pl-8"
                    />
                  </div>
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.toPlaceholder")}
                  </span>
                  <div className="relative min-w-0">
                    <CalendarDays className="absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                    <Input
                      type="datetime-local"
                      value={to}
                      onChange={e => { setTo(e.target.value); setPage(0); }}
                      aria-label={t("auditLog.toPlaceholder")}
                      title={t("auditLog.toPlaceholder")}
                      className="min-w-0 pl-8"
                    />
                  </div>
                </div>
              </div>
            )}
            {activeFilterChips.length > 0 && (
              <div className="flex w-full flex-wrap items-center gap-1.5">
                {activeFilterChips.map((chip) => (
                  <button
                    key={chip.key}
                    type="button"
                    onClick={chip.onClear}
                    className="inline-flex h-6 max-w-[18rem] items-center gap-1 rounded-md border border-input/30 bg-muted px-2 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    title={chip.label}
                  >
                    <span className="truncate">{chip.label}</span>
                    <X className="size-3.5 shrink-0" />
                  </button>
                ))}
              </div>
            )}
          </div>
        }
      />

      <Dialog open={!!detailEntry} onOpenChange={() => setDetailEntry(null)}>
        <DialogContent className="border border-border bg-card max-h-[90dvh] overflow-y-auto max-w-2xl" description={t("auditLog.auditDetailDescription")} onCloseAutoFocus={detailFocus.onCloseAutoFocus}>
          <DialogHeader><DialogTitle className="text-sm">{t("auditLog.auditDetail")}</DialogTitle></DialogHeader>
          {detailQuery.isPending && detailEntry && <p>{t("auditLog.detailLoading")}</p>}
          {detailQuery.isError && <p role="alert">{t("auditLog.detailError")}</p>}
          {detailQuery.data && (() => { const detailEntry = detailQuery.data; return (
            <div className="min-w-0 space-y-3">
              <div className="space-y-2">
                <DetailRow label={t("auditLog.logId")} value={detailEntry.id} />
                <DetailRow
                  label={t("auditLog.timestamp")}
                  value={formatAuditTimestamp(detailEntry.timestamp, auditLocale)}
                />
                <DetailRow
                  label={t("auditLog.utcTimestamp")}
                  value={formatAuditTimestampUtc(detailEntry.timestamp)}
                />
                <DetailRow label={t("auditLog.actor")} value={actorLabel(detailEntry, t)} />
                <DetailRow label={t("auditLog.actorIdFilter")} value={detailEntry.actorRef?.id || (UUID_PATTERN.test(detailEntry.actor) ? detailEntry.actor : undefined)} />
                <DetailRow label={t("auditLog.channel")} value={actorKindLabel(t, detailEntry.actorKind)} />
                <DetailRow label={t("auditLog.action")} value={formatAuditAction(detailEntry, t)} />
                <DetailRow label={t("auditLog.rawEventType")} value={detailEntry.action} />
                <DetailRow label={t("auditLog.target")} value={detailEntry.context?.display?.target || objectLabel(detailEntry.targetRef, detailEntry.resourceId)} />
                <DetailRow label={t("auditLog.resourceId")} value={detailEntry.resourceId || detailEntry.targetRef?.id} />
                <DetailRow label={t("auditLog.targetType")} value={detailEntry.resourceType ? `${kindLabel(t, detailEntry.resourceType)} (${detailEntry.resourceType})` : undefined} />
                <DetailRow label={t("auditLog.result")} value={formatAuditResult(detailEntry, t)} />
                <DetailRow label={t("auditLog.lifecycleState")} value={detailEntry.context?.state ? lifecycleLabel(t, detailEntry.context.state) : undefined} title={detailEntry.context?.state ? `${detailEntry.context.state.domain} / ${detailEntry.context.state.value}` : undefined} />
                <DetailRow label={t("auditLog.decision")} value={detailEntry.context?.decision ? knownLabel(t, "decisions", detailEntry.context.decision, ["allowed", "denied"]) : undefined} title={detailEntry.context?.decision ?? undefined} />
                <DetailRow label={t("auditLog.emitter")} value={detailEntry.context?.emitter ? [knownLabel(t, "emitters", detailEntry.context.emitter.kind, ["api", "worker", "proxy", "system"]), detailEntry.context.emitter.id].filter(Boolean).join(" · ") : undefined} title={detailEntry.context?.emitter ? [detailEntry.context.emitter.kind, detailEntry.context.emitter.id].filter(Boolean).join(" · ") : undefined} />
                <DetailRow label={t("auditLog.delegatedPrincipal")} value={detailEntry.context?.delegated_principal ? `${detailEntry.context.delegated_principal.kind} · ${detailEntry.context.delegated_principal.id}` : undefined} />
                {([ ["request_id", "requestId"], ["session_id", "sessionId"], ["run_id", "runId"], ["run_group_id", "runGroupId"] ] as const).map(([field, key]) => {
                  const value = detailEntry.context?.correlation?.[field];
                  return value ? <div key={field} className="flex flex-wrap items-center justify-between gap-2"><DetailRow label={t(`auditLog.correlation.${key}`)} value={value} /><Button variant="outline" onClick={() => { setCorrelation({ key, value }); setPage(0); setDetailEntry(null); }}>{t("auditLog.relatedEvents")}</Button></div> : null;
                })}
                <DetailRow label={t("auditLog.causationId")} value={detailEntry.context?.correlation?.causation_id ?? undefined} />
              </div>
              {detailEntry.details !== undefined ? <AuditMetadata key={detailEntry.id} details={detailEntry.details} /> : <p>{t("auditLog.detailEmpty")}</p>}
            </div>
          ); })()}
        </DialogContent>
      </Dialog>
    </div>
  );
};

export default AuditLog;
