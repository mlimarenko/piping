import { useState, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import EntityRef from "@/shared/ui-shared/EntityRef";
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
  UserRound,
  X,
} from "lucide-react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { cn } from "@/shared/lib/utils";
import { listAuditLogs } from "@/features/audit/api";
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

const AUDIT_EVENT_FILTERS = [
  { value: "connection.session.created", key: "connectionSessionCreated" },
  { value: "connection.session.activated", key: "connectionSessionActivated" },
  { value: "connection.session.websocket.connected", key: "connectionWebsocketConnected" },
  { value: "connection.session.disconnected", key: "connectionSessionDisconnected" },
  { value: "connection.session.terminated", key: "connectionSessionTerminated" },
  { value: "connection.session.failed", key: "connectionSessionFailed" },
  { value: "mcp.tool.invoked", key: "mcpToolInvoked" },
  { value: "access_control.user_api_token.created", key: "apiTokenCreated" },
  { value: "access_control.user_api_token.revoked", key: "apiTokenRevoked" },
  { value: "playbook.created", key: "playbookCreated" },
  { value: "playbook.version.created", key: "playbookVersionCreated" },
  { value: "playbook.updated", key: "playbookUpdated" },
  { value: "playbook.deleted", key: "playbookDeleted" },
  { value: "playbook.target_set.created", key: "playbookTargetSetCreated" },
  { value: "playbook.assignment.created", key: "playbookAssignmentCreated" },
  { value: "playbook.run_group.queued", key: "playbookRunGroupQueued" },
  { value: "playbook.artifact.created", key: "playbookArtifactCreated" },
  { value: "playbook.run_event.appended", key: "playbookRunEventAppended" },
  { value: "playbook.run.completed", key: "playbookRunCompleted" },
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
  "allowed",
  "scope_denied",
  "resource_denied",
  "invalid_arguments",
  "dispatch_failed",
  "rest_denied",
  "rest_error",
  "denied",
  "success",
  "rejected",
  "queued",
  "leased",
  "dispatching",
  "running",
  "succeeded",
  "canceled",
  "timed_out",
  "used",
  "revoked",
  "expired",
  "pending",
  "active",
  "failed",
  "disconnected",
  "terminated",
] as const;

const MCP_TOOL_GROUP_FILTERS = [
  "mcp",
  "rest",
  "inventory",
  "access_control",
  "ssh",
  "target_diagnostics",
  "vnc_observe",
  "vnc_control",
  "visual_credentials",
] as const;

const MCP_PROTOCOL_FILTERS = ["ssh", "vnc"] as const;
const MCP_RISK_FILTERS = ["read", "rest", "interactive_session", "control"] as const;

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

function eventKey(eventType: string): string {
  return eventType.replace(/[^a-zA-Z0-9]+/g, "_").replace(/^_+|_+$/g, "");
}

function metadataString(entry: AuditEntry, key: string): string | undefined {
  const value = entry.details?.[key];
  if (typeof value === "string") {
    const trimmed = value.trim();
    return trimmed ? trimmed : undefined;
  }
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return undefined;
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

function kindLabel(t: Translate, kind?: string): string {
  if (!kind) return "";
  return t(`auditLog.resourceKinds.${eventKey(kind)}`, { defaultValue: humanizeEventType(kind) });
}

function actorKindLabel(t: Translate, kind?: string): string {
  if (!kind) return "";
  return t(`auditLog.actorKinds.${eventKey(kind)}`, { defaultValue: humanizeEventType(kind) });
}

function formatAuditAction(entry: AuditEntry, t: Translate): string {
  return t(`auditLog.events.${eventKey(entry.action)}.action`, {
    defaultValue: humanizeEventType(entry.action),
  });
}

function formatAuditResult(entry: AuditEntry, t: Translate): string {
  const decision = metadataString(entry, "decision");
  if (decision) {
    return t(`auditLog.results.${eventKey(decision)}`, { defaultValue: decision });
  }
  const status = metadataString(entry, "status");
  if (status) {
    return t(`auditLog.results.${eventKey(status)}`, { defaultValue: status });
  }
  return "—";
}

function formatAuditSummary(entry: AuditEntry, t: Translate): string {
  const actor = objectLabel(entry.actorRef, entry.actor || t("auditLog.unknownActor"));
  const via = objectLabel(entry.actorVia, "");
  const target = objectLabel(
    entry.resourceRef ?? entry.targetRef,
    entry.resourceId || t("auditLog.unknownTarget"),
  );
  const action = metadataString(entry, "action") ?? "";
  const tool = metadataString(entry, "tool") ?? "";
  const protocol = metadataString(entry, "protocol") ?? entry.targetRef?.label ?? "";
  const resourceType = metadataString(entry, "resource_type") ?? entry.resourceType;
  const resourceId = metadataString(entry, "resource_id") ?? entry.resourceId;
  const decision = formatAuditResult(entry, t);
  const metadataSummary = metadataString(entry, "summary");
  return t(`auditLog.events.${eventKey(entry.action)}.summary`, {
    actor,
    via,
    target,
    action,
    tool,
    protocol,
    resourceType,
    resourceId,
    decision,
    event: entry.action,
    defaultValue:
      metadataSummary ??
      t("auditLog.events.default.summary", {
        actor,
        target,
        event: entry.action,
      }),
  });
}

function actionClassName(entry: AuditEntry): string {
  const result = formatRawResult(entry).toLowerCase();
  if (entry.action.includes("failed") || result === "denied" || result === "failed") {
    return "bg-status-negative/10 text-status-negative border border-status-negative/15";
  }
  if (entry.action.includes("terminated") || entry.action.includes("disconnected")) {
    return "bg-status-warning/10 text-status-warning border border-status-warning/15";
  }
  if (entry.action.startsWith("mcp.")) {
    return "bg-status-info/10 text-status-info border border-status-info/15";
  }
  if (entry.action.includes("created") || entry.action.includes("connected") || result === "allowed") {
    return "bg-status-positive/10 text-status-positive border border-status-positive/15";
  }
  return "surface-chip";
}

function formatRawResult(entry: AuditEntry): string {
  return metadataString(entry, "decision") ?? metadataString(entry, "status") ?? "";
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

function DetailRow({ label, value }: { label: string; value?: string }) {
  if (!value) return null;
  return (
    <div className="grid grid-cols-1 gap-1 sm:grid-cols-[8.5rem_minmax(0,1fr)] sm:gap-3">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className="min-w-0 [overflow-wrap:anywhere] text-sm sm:text-right font-mono text-foreground" title={value}>
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
  // Multi async-lookup selections (users for the actor filter, hydrated targets
  // for the target filter). Empty arrays send no param.
  const [actorFilters, setActorFilters] = useState<AuditLookupOption[]>([]);
  const [targetFilters, setTargetFilters] = useState<AuditLookupOption[]>([]);
  // Channel (`actor_kind`) multiselect — the headline dimension separating
  // interactive UI / MCP / API tokens / runner / service principals / system.
  const [channels, setChannels] = useState<string[]>([]);
  // Canonical multi-value store for the three IN-filter dimensions
  // (event_type / target_type / result), keyed by the table column key
  // (`action` / `target` / `result`). The toolbar MultiSelects below are views
  // over these arrays, and the DataTable funnel menu is another editor of the
  // same store — one state, one query param, no double-apply.
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [mcpTool, setMcpTool] = useState("");
  const [mcpToolGroups, setMcpToolGroups] = useState<string[]>([]);
  const [mcpProtocols, setMcpProtocols] = useState<string[]>([]);
  const [mcpRisks, setMcpRisks] = useState<string[]>([]);
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [detailEntry, setDetailEntry] = useState<AuditEntry | null>(null);
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
    () => sanitizedColumnFilters.result ?? [],
    [sanitizedColumnFilters.result],
  );
  // The target picker scopes its lookup by a single target type. With exactly
  // one target type selected it scopes; with 0 or 2+ it searches all types. A
  // user may then pick a target whose type is not in the set → zero rows (the
  // backend ANDs target_type and target_id); that is the user's choice.
  const targetTypeForPicker = targetTypes.length === 1 ? targetTypes[0] : undefined;

  const actorIds = useMemo(() => actorFilters.map((option) => option.id), [actorFilters]);
  const targetIds = useMemo(() => targetFilters.map((option) => option.id), [targetFilters]);

  const auditFilters = useMemo(() => ({
    query: debouncedSearch.trim() || undefined,
    eventTypes,
    actorKinds: channels,
    actorIds,
    targetTypes,
    targetIds,
    results,
    mcpTool: mcpTool.trim() || undefined,
    mcpToolGroups,
    mcpProtocols,
    mcpRisks,
    from: toIsoDateTime(from),
    to: toIsoDateTime(to),
  }), [actorIds, channels, debouncedSearch, eventTypes, from, mcpProtocols, mcpRisks, mcpTool, mcpToolGroups, results, targetIds, targetTypes, to]);

  // Empty arrays must NOT count as active (Boolean([]) is true); check length
  // for the array dims and Boolean for the scalars explicitly.
  const hasActiveFilters =
    eventTypes.length > 0 ||
    channels.length > 0 ||
    actorIds.length > 0 ||
    targetTypes.length > 0 ||
    targetIds.length > 0 ||
    results.length > 0 ||
    mcpToolGroups.length > 0 ||
    mcpProtocols.length > 0 ||
    mcpRisks.length > 0 ||
    Boolean(auditFilters.query) ||
    Boolean(auditFilters.mcpTool) ||
    Boolean(auditFilters.from) ||
    Boolean(auditFilters.to);
  const advancedFilterCount = [
    targetTypes.length > 0,
    targetIds.length > 0,
    results.length > 0,
    Boolean(mcpTool.trim()),
    mcpToolGroups.length > 0,
    mcpProtocols.length > 0,
    mcpRisks.length > 0,
    Boolean(from),
    Boolean(to),
  ].filter(Boolean).length;

  const activeFilterChips = useMemo<FilterChip[]>(() => {
    const chips: FilterChip[] = [];
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
    if (actorFilters.length > 0) {
      const labels = actorFilters.map((option) => option.label).join(", ");
      chips.push({
        key: "actor",
        label: `${t("auditLog.actor")}: ${labels}`,
        onClear: () => { setActorFilters([]); setPage(0); },
      });
    }
    if (eventTypes.length > 0) {
      const labels = eventTypes
        .map((value) => {
          const option = AUDIT_EVENT_FILTERS.find((item) => item.value === value);
          return option ? t(`auditLog.filters.${option.key}`) : humanizeEventType(value);
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
        .map((value) => {
          const option = AUDIT_TARGET_TYPE_FILTERS.find((item) => item.value === value);
          return option ? t(`auditLog.targetTypes.${option.key}`) : humanizeEventType(value);
        })
        .join(", ");
      chips.push({
        key: "targetType",
        label: `${t("auditLog.resourceType")}: ${labels}`,
        onClear: () => { setColumnFilter("target", []); },
      });
    }
    if (results.length > 0) {
      const labels = results
        .map((value) => t(`auditLog.results.${eventKey(value)}`, { defaultValue: value }))
        .join(", ");
      chips.push({
        key: "result",
        label: `${t("auditLog.result")}: ${labels}`,
        onClear: () => { setColumnFilter("result", []); },
      });
    }
    const trimmedMcpTool = mcpTool.trim();
    if (trimmedMcpTool) {
      chips.push({
        key: "mcpTool",
        label: `${t("auditLog.mcpTool")}: ${trimmedMcpTool}`,
        onClear: () => { setMcpTool(""); setPage(0); },
      });
    }
    if (mcpToolGroups.length > 0) {
      const labels = mcpToolGroups
        .map((value) => t(`auditLog.mcpToolGroups.${eventKey(value)}`, { defaultValue: value }))
        .join(", ");
      chips.push({
        key: "mcpToolGroup",
        label: `${t("auditLog.mcpToolGroup")}: ${labels}`,
        onClear: () => { setMcpToolGroups([]); setPage(0); },
      });
    }
    if (mcpProtocols.length > 0) {
      const labels = mcpProtocols
        .map((value) => t(`auditLog.protocols.${eventKey(value)}`, { defaultValue: value }))
        .join(", ");
      chips.push({
        key: "mcpProtocol",
        label: `${t("auditLog.mcpProtocol")}: ${labels}`,
        onClear: () => { setMcpProtocols([]); setPage(0); },
      });
    }
    if (mcpRisks.length > 0) {
      const labels = mcpRisks
        .map((value) => t(`auditLog.mcpRisks.${eventKey(value)}`, { defaultValue: value }))
        .join(", ");
      chips.push({
        key: "mcpRisk",
        label: `${t("auditLog.mcpRisk")}: ${labels}`,
        onClear: () => { setMcpRisks([]); setPage(0); },
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
  }, [actorFilters, channels, eventTypes, from, mcpProtocols, mcpRisks, mcpTool, mcpToolGroups, results, search, t, targetFilters, targetTypes, to]);

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
  const logs = auditQuery.data?.items ?? [];
  const total = auditQuery.data?.total ?? 0;

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
    setActorFilters([]);
    setTargetFilters([]);
    setChannels([]);
    setColumnFilters({});
    setMcpTool("");
    setMcpToolGroups([]);
    setMcpProtocols([]);
    setMcpRisks([]);
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
    () => AUDIT_EVENT_FILTERS.map((option) => ({ id: option.value, label: t(`auditLog.filters.${option.key}`) })),
    [t],
  );
  const targetTypeItems = useMemo<EntityOption[]>(
    () => AUDIT_TARGET_TYPE_FILTERS.map((option) => ({ id: option.value, label: t(`auditLog.targetTypes.${option.key}`) })),
    [t],
  );
  const resultItems = useMemo<EntityOption[]>(
    () => AUDIT_RESULT_FILTERS.map((value) => ({ id: value, label: t(`auditLog.results.${eventKey(value)}`, { defaultValue: value }) })),
    [t],
  );
  const mcpToolGroupItems = useMemo<EntityOption[]>(
    () => MCP_TOOL_GROUP_FILTERS.map((value) => ({ id: value, label: t(`auditLog.mcpToolGroups.${eventKey(value)}`, { defaultValue: value }) })),
    [t],
  );
  const mcpProtocolItems = useMemo<EntityOption[]>(
    () => MCP_PROTOCOL_FILTERS.map((value) => ({ id: value, label: t(`auditLog.protocols.${eventKey(value)}`, { defaultValue: value }) })),
    [t],
  );
  const mcpRiskItems = useMemo<EntityOption[]>(
    () => MCP_RISK_FILTERS.map((value) => ({ id: value, label: t(`auditLog.mcpRisks.${eventKey(value)}`, { defaultValue: value }) })),
    [t],
  );

  // Per-column funnel options for the three IN-filter dimensions, resolved from
  // the same static option lists the toolbar MultiSelects use.
  const filterOptionsByColumn: Record<string, DataTableFilterOption[]> = {
    action: AUDIT_EVENT_FILTERS.map((o) => ({ value: o.value, label: t(`auditLog.filters.${o.key}`) })),
    target: AUDIT_TARGET_TYPE_FILTERS.map((o) => ({ value: o.value, label: t(`auditLog.targetTypes.${o.key}`) })),
    result: AUDIT_RESULT_FILTERS.map((value) => ({
      value,
      label: t(`auditLog.results.${eventKey(value)}`, { defaultValue: value }),
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
      const actor = objectLabel(r.actorRef, r.actor || t("auditLog.unknownActor"));
      const via = r.actorVia
        ? t("auditLog.viaActor", {
            kind: actorKindLabel(t, r.actorVia.kind),
            label: r.actorVia.label,
          })
        : r.actorRef?.secondary;
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
      <span className={`inline-flex max-w-[15rem] truncate rounded-full px-2.5 py-1 text-xs font-medium ${actionClassName(r)}`} title={r.action}>
        {formatAuditAction(r, t)}
      </span>
    )},
    { key: "target", label: t("auditLog.target"), ...columnMeta("target"), render: (r: AuditEntry) => {
      const primary = r.resourceRef ?? r.targetRef;
      const secondary = primary
        ? [kindLabel(t, primary.kind), primary.secondary].filter(Boolean).join(" · ")
        : r.resourceType;
      return (
        <div className="min-w-0 max-w-[18rem]">
          <EntityRef name={primary?.label} id={r.resourceId} className="block truncate text-foreground" />
          {secondary && <span className="block truncate text-xs text-muted-foreground" title={secondary}>{secondary}</span>}
        </div>
      );
    } },
    { key: "result", label: t("auditLog.result"), ...columnMeta("result"), render: (r: AuditEntry) => <span className="text-sm text-muted-foreground">{formatAuditResult(r, t)}</span> },
    { key: "summary", label: t("auditLog.summary"), ...columnMeta("summary"), render: (r: AuditEntry) => {
      const summary = formatAuditSummary(r, t);
      return <span className="block max-w-[52ch] truncate" title={summary}>{summary}</span>;
    } },
    {
      key: "details", label: "", excludeFromSettings: true, render: (r: AuditEntry) => (
        <Button variant="ghost" size="icon-sm" aria-label={t("auditLog.detailsLabel")} onClick={event => { detailFocus.capture(event.currentTarget); setDetailEntry(r); }}>
          <Eye className="h-4 w-4" />
        </Button>
      )
    },
  ];

  return (
    <PageLayout className="flex flex-col gap-6">
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
        error={auditQuery.error}
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
                containerClassName="min-w-[14rem] flex-1 basis-[18rem] lg:max-w-[26rem]"
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
              <div className="min-w-[11rem] flex-1 basis-[12rem] lg:max-w-[16rem]">
                <AuditObjectMultiPicker
                  value={actorFilters}
                  onChange={(value) => { setActorFilters(value); setPage(0); }}
                  targetType="user"
                  placeholder={t("auditLog.actorPlaceholder")}
                  searchPlaceholder={t("auditLog.actorSearchPlaceholder")}
                  emptyText={t("auditLog.lookupEmpty")}
                  loadingText={t("auditLog.lookupLoading")}
                  clearText={t("auditLog.clearActorFilter")}
                  groupLabel={t("auditLog.lookupGroup")}
                  kindLabel={(kind) => kindLabel(t, kind)}
                  icon={UserRound}
                />
              </div>
              <div className="min-w-[11rem] flex-1 basis-[12rem] lg:max-w-[16rem]">
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
            </div>
            {showAdvancedFilters && (
              <div className="grid w-full grid-cols-1 gap-3 rounded-md border border-border bg-card p-2 md:grid-cols-2 xl:grid-cols-4">
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.target")}
                  </span>
                  <AuditObjectMultiPicker
                    value={targetFilters}
                    onChange={(value) => { setTargetFilters(value); setPage(0); }}
                    targetType={targetTypeForPicker}
                    placeholder={targetTypeForPicker === "organization" ? t("auditLog.targetPickerDisabled") : t("auditLog.targetPickerPlaceholder")}
                    searchPlaceholder={t("auditLog.targetSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    loadingText={t("auditLog.lookupLoading")}
                    clearText={t("auditLog.clearTargetFilter")}
                    groupLabel={t("auditLog.lookupGroup")}
                    kindLabel={(kind) => kindLabel(t, kind)}
                    icon={Database}
                    disabled={targetTypeForPicker === "organization"}
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
                    onValueChange={(values) => setColumnFilter("result", values)}
                    placeholder={t("auditLog.allResults")}
                    searchPlaceholder={t("auditLog.resultSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    aria-label={t("auditLog.result")}
                    triggerClassName={cn(FILTER_CONTROL_CLASS, "w-full")}
                  />
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.mcpTool")}
                  </span>
                  <SearchInput
                    containerClassName="min-w-0"
                    value={mcpTool}
                    onValueChange={(v) => { setMcpTool(v); setPage(0); }}
                    placeholder={t("auditLog.mcpToolPlaceholder")}
                    aria-label={t("auditLog.mcpTool")}
                    title={t("auditLog.mcpTool")}
                    className={cn(FILTER_CONTROL_CLASS, "pl-8")}
                  />
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.mcpToolGroup")}
                  </span>
                  <MultiSelect
                    items={mcpToolGroupItems}
                    value={mcpToolGroups}
                    onValueChange={(values) => { setMcpToolGroups(values); setPage(0); }}
                    placeholder={t("auditLog.allMcpToolGroups")}
                    searchPlaceholder={t("auditLog.mcpToolGroupSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    aria-label={t("auditLog.mcpToolGroup")}
                    triggerClassName={cn(FILTER_CONTROL_CLASS, "w-full")}
                  />
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.mcpProtocol")}
                  </span>
                  <MultiSelect
                    items={mcpProtocolItems}
                    value={mcpProtocols}
                    onValueChange={(values) => { setMcpProtocols(values); setPage(0); }}
                    placeholder={t("auditLog.allProtocols")}
                    searchPlaceholder={t("auditLog.mcpProtocolSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    aria-label={t("auditLog.mcpProtocol")}
                    triggerClassName={cn(FILTER_CONTROL_CLASS, "w-full")}
                  />
                </div>
                <div className="min-w-0 space-y-1">
                  <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {t("auditLog.mcpRisk")}
                  </span>
                  <MultiSelect
                    items={mcpRiskItems}
                    value={mcpRisks}
                    onValueChange={(values) => { setMcpRisks(values); setPage(0); }}
                    placeholder={t("auditLog.allMcpRisks")}
                    searchPlaceholder={t("auditLog.mcpRiskSearchPlaceholder")}
                    emptyText={t("auditLog.lookupEmpty")}
                    aria-label={t("auditLog.mcpRisk")}
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
          {detailEntry && (
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
                <DetailRow label={t("auditLog.actor")} value={objectLabel(detailEntry.actorRef, detailEntry.actor)} />
                <DetailRow label={t("auditLog.channel")} value={actorKindLabel(t, detailEntry.actorKind)} />
                <DetailRow
                  label={t("auditLog.actorVia")}
                  value={detailEntry.actorVia ? `${actorKindLabel(t, detailEntry.actorVia.kind)} · ${detailEntry.actorVia.label}` : undefined}
                />
                <DetailRow label={t("auditLog.action")} value={formatAuditAction(detailEntry, t)} />
                <DetailRow label={t("auditLog.rawEventType")} value={detailEntry.action} />
                <DetailRow label={t("auditLog.target")} value={objectLabel(detailEntry.resourceRef ?? detailEntry.targetRef, detailEntry.resourceId)} />
                <DetailRow label={t("auditLog.result")} value={formatAuditResult(detailEntry, t)} />
                <DetailRow label={t("auditLog.summary")} value={formatAuditSummary(detailEntry, t)} />
              </div>
              {detailEntry.details !== undefined && <AuditMetadata key={detailEntry.id} details={detailEntry.details} />}
            </div>
          )}
        </DialogContent>
      </Dialog>
    </PageLayout>
  );
};

export default AuditLog;
