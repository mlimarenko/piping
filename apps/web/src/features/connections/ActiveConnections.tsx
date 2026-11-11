import { useMemo, useRef, useState, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import PageChrome from "@/shared/ui-shared/PageChrome";
import DataTable, {
  type DataTableSortState,
  type DataTableFilterOption,
} from "@/shared/ui-shared/DataTable";
import { useServerTable, type ServerTableListParams } from "@/shared/ui-shared/useServerTable";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import { useSearchParams } from "react-router";
import { RefreshCw, Activity } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import { formatAbsoluteTime, formatDuration } from "@/shared/lib/format";
import {
  closeSession,
  CONNECTION_TARGETS_QUERY_KEY,
  listActiveSessions,
  listConnectionTargets,
  terminateSession,
} from "@/features/connections/api";
import { listUsers, usersAllQueryKey } from "@/features/access-control/api";
import type { ActiveSession } from "@/shared/api/types";
import type { ApiError } from "@/shared/api/client";
import { isPipingDesktopShell } from "@/app/desktop/runtime";
import SessionDetailPanel from "@/features/connections/components/connections/SessionDetailPanel";
import LiveSessionViewer from "@/features/connections/components/connections/LiveSessionViewer";
import PaneResizer from "@/shared/ui-shared/PaneResizer";
import {
  observationHandoffFor,
  observationWebsocketUrl,
  openObservationWindow,
} from "@/features/connections/observation";
import {
  clampObservationPreviewHeight,
  clampSessionInspectorWidth,
  loadSessionInspectorLayout,
  observationPreviewMaxHeight,
  saveSessionInspectorLayout,
  sessionInspectorMaxWidth,
  OBSERVATION_PREVIEW_MIN_HEIGHT,
  SESSION_INSPECTOR_MIN_WIDTH,
} from "@/features/connections/sessionInspectorLayout";
import {
  ACTIVE_CONNECTIONS_TABLE_CONFIG,
} from "@/features/connections/activeConnectionsTableConfig";

// ── Component ─────────────────────────────────────────────

const ActiveConnections = () => {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const qc = useQueryClient();
  const [params] = useSearchParams();
  const view = params.get("view") === "history" ? "history" : "active";
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [observing, setObserving] = useState(false);
  const [inspectorLayout, setInspectorLayout] = useState(loadSessionInspectorLayout);
  const detailsTrigger = useRef<HTMLElement | null>(null);
  const table = useServerTable<ActiveSession, ServerTableListParams & { limit: number; offset: number } &
    Partial<Record<"target_id" | "user_id" | "protocol" | "status", string[]>>>({
    ...ACTIVE_CONNECTIONS_TABLE_CONFIG,
    queryKey: ["connections", "sessions", view],
    listFn: ({ limit, offset, sort, order, search, target_id, user_id, protocol, status }) =>
      listActiveSessions({
        limit, offset, sort, order, search, view,
        filters: Object.fromEntries(
          Object.entries({ target_id, user_id, protocol, status }).filter(
            (entry): entry is [string, string[]] => Array.isArray(entry[1]),
          ),
        ),
      }),
    refetchInterval: 5000,
    keepPreviousData: true,
  });
  const { page, setPage, pageSize, setPageSize, search, setSearch, query: sessionsQuery,
    sortState: sanitizedSortState, columnFilters: sanitizedColumnFilters } = table;
  const sessions = useMemo(() => sessionsQuery.data?.items ?? [], [sessionsQuery.data]);

  // Enrichment queries — errors are silently ignored
  // Shares the workspace target cache: `listConnectionTargets()` always loads
  // the full read model, so both consumers reuse one cache entry.
  const hostsQuery = useQuery({
    queryKey: CONNECTION_TARGETS_QUERY_KEY,
    queryFn: () => listConnectionTargets(),
    staleTime: 60_000,
  });
  const usersQuery = useQuery({
    queryKey: usersAllQueryKey(),
    queryFn: () => listUsers({ limit: 500 }),
    staleTime: 60_000,
  });

  const hostFilterOptions = useMemo<DataTableFilterOption[]>(
    () =>
      (hostsQuery.data?.items ?? []).map((host) => ({
        value: host.id,
        label: host.name,
      })),
    [hostsQuery.data],
  );
  const userFilterOptions = useMemo<DataTableFilterOption[]>(
    () =>
      (usersQuery.data?.items ?? []).map((user) => ({
        value: user.id,
        label: user.username,
      })),
    [usersQuery.data],
  );

  // Host/user are FK filters; session statuses and protocols are static enums.
  const filterOptionsByKey = useMemo<Record<string, DataTableFilterOption[]>>(
    () => ({
      host: hostFilterOptions,
      user: userFilterOptions,
      protocol: (["ssh", "rdp", "vnc"] as const).map((value) => ({
        value,
        label: value.toUpperCase(),
      })),
      status: (["pending", "active", "disconnected", "terminated", "failed"] as const).map(
        (value) => ({ value, label: t(`statuses.${value}`, value) }),
      ),
    }),
    [hostFilterOptions, t, userFilterOptions],
  );

  // Build lookup maps
  const hostMap = useMemo(() => {
    const m = new Map<string, { name: string }>();
    for (const h of hostsQuery.data?.items ?? []) m.set(h.id, { name: h.name });
    return m;
  }, [hostsQuery.data]);

  const userMap = useMemo(() => {
    const m = new Map<string, { username: string; email?: string }>();
    for (const u of usersQuery.data?.items ?? []) m.set(u.id, { username: u.username, email: u.email || undefined });
    return m;
  }, [usersQuery.data]);

  // Hydrate sessions with resolved names
  const hydrated = useMemo(
    () =>
      sessions.map((s) => ({
        ...s,
        hostName: s.hostName ?? hostMap.get(s.hostId)?.name,
        userDisplayName: s.userDisplayName ?? userMap.get(s.userId)?.username,
        userEmail: s.userEmail ?? userMap.get(s.userId)?.email,
      })),
    [sessions, hostMap, userMap],
  );

  // Selected session state
  const selectedSession = useMemo(
    () => hydrated.find((s) => s.id === selectedId) ?? null,
    [hydrated, selectedId],
  );
  const observationUrl = useMemo(
    () =>
      selectedSession
        ? observationWebsocketUrl(selectedSession.observe, selectedSession.id, selectedSession.protocol)
        : null,
    [selectedSession],
  );
  // A capability revoked by a background refresh unmounts the viewer without
  // leaving the panel in a stale "observing" state.
  const observingNow = observing && observationUrl !== null;
  // A width persisted on a wider screen is clamped to the current viewport.
  const inspectorWidth = clampSessionInspectorWidth(
    inspectorLayout.width,
    sessionInspectorMaxWidth(),
  );
  const previewHeight = clampObservationPreviewHeight(inspectorLayout.previewHeight);
  // The desktop shell opens its own windows through the native command, which
  // only accepts operator detached routes; keep the action to the browser.
  const canOpenObservationWindow =
    observationUrl !== null && !isPipingDesktopShell({ allowStoredHint: true });

  const rememberInspectorWidth = (width: number) => {
    setInspectorLayout((current) => {
      const next = { ...current, width: clampSessionInspectorWidth(width, sessionInspectorMaxWidth()) };
      saveSessionInspectorLayout(next);
      return next;
    });
  };

  const rememberObservationHeight = (height: number) => {
    setInspectorLayout((current) => {
      const next = { ...current, previewHeight: clampObservationPreviewHeight(height) };
      saveSessionInspectorLayout(next);
      return next;
    });
  };

  const openObservationInWindow = () => {
    if (!selectedSession || !observationUrl) return;
    const handoff = observationHandoffFor(selectedSession, observationUrl);
    if (!handoff || !openObservationWindow(handoff)) {
      notify("warning", t("activeConnections.openInNewWindow"), {
        description: t("connections.popupBlocked"),
      });
      return;
    }
    // The detached window owns the observer socket; keep a single read-only
    // stream per session from this page.
    setObserving(false);
  };

  // Mutations
  const disconnectMutation = useApiMutation({
    mutationFn: (id: string) => closeSession(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["connections", "sessions"] });
      setSelectedId(null);
      notify("success", t("activeConnections.sessionDisconnected"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("activeConnections.disconnectFailed")),
  });

  const terminateMutation = useApiMutation({
    mutationFn: (id: string) => terminateSession(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["connections", "sessions"] });
      setSelectedId(null);
      notify("success", t("activeConnections.sessionTerminated"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("activeConnections.terminateFailed")),
  });

  // Table columns
  const now = Date.now();
  const columns = [
    {
      key: "host",
      label: t("activeConnections.host"),
      sortable: true,
      filterOptions: filterOptionsByKey.host,
      render: (r: ActiveSession) => (
        <button type="button" className="font-medium text-foreground underline-offset-4 hover:underline focus-visible:outline focus-visible:outline-2" onClick={(event) => { event.stopPropagation(); detailsTrigger.current = event.currentTarget; setObserving(false); setSelectedId(r.id); }}>
          {r.hostName ?? r.hostId.slice(0, 8) + "…"}
        </button>
      ),
    },
    {
      key: "user",
      label: t("activeConnections.user"),
      sortable: true,
      filterOptions: filterOptionsByKey.user,
      render: (r: ActiveSession) => (
        <span className="text-foreground">
          {r.userDisplayName ?? r.userEmail ?? r.userId.slice(0, 8) + "…"}
        </span>
      ),
    },
    {
      key: "protocol",
      label: t("activeConnections.protocol"),
      sortable: true,
      filterOptions: filterOptionsByKey.protocol,
      render: (r: ActiveSession) => (
        <span className="surface-chip px-2 py-0.5 rounded text-xs font-medium uppercase">
          {r.protocol}
        </span>
      ),
    },
    {
      key: "status",
      label: t("status"),
      sortable: true,
      filterOptions: filterOptionsByKey.status,
      render: (r: ActiveSession) => (
        <StatusChip
          status={
             r.status === "active" && r.presence?.state === "live"
              ? "positive"
              : r.status === "failed"
                ? "negative"
                : r.status === "terminated" || r.status === "disconnected"
                  ? "neutral"
                  : "info"
          }
          label={r.status === "active" && r.presence?.state !== "live" ? t(`activeConnections.presence.${r.presence?.state ?? "unknown"}`, r.presence?.state ?? "unknown") : t(`statuses.${r.status}`, r.status)}
        />
      ),
    },
    {
      key: "started_at",
      label: t("activeConnections.duration"),
      sortable: true,
      render: (r: ActiveSession) => {
        const end = r.endedAt != null
          ? new Date(r.endedAt).getTime()
          : r.status === "active" || r.status === "pending" ? now : Number.NaN;
        return (
          <span className="text-muted-foreground text-xs">
            {formatDuration(end - new Date(r.connectedAt).getTime(), locale)}
          </span>
        );
      },
    },
    {
      key: "lastActivity",
      label: t("activeConnections.trafficActivity"),
      sortable: true,
      render: (r: ActiveSession) => (
        <span className="text-muted-foreground text-xs">{formatAbsoluteTime(r.lastActivity, locale)}</span>
      ),
    },
  ];

  // Sort/filter change resets to page 0 and clears selection (the visible row
  // set changes, so a kept selection could span an unseen page).
  const handleSortChange = (next: DataTableSortState | null) => {
    table.setSortState(next);
    setSelectedId(null);
  };

  const handleColumnFilterChange = (key: string, values: string[]) => {
    table.setColumnFilter(key, values);
    setSelectedId(null);
  };

  const total = sessionsQuery.data?.total ?? 0;

  const observationPreview = selectedSession && observingNow && observationUrl ? (
    <section aria-label={t("activeConnections.readOnly")} className="flex flex-col gap-2">
      <strong className="text-sm font-medium text-muted-foreground">{t("activeConnections.readOnly")}</strong>
      <div
        className="min-w-0 overflow-hidden rounded-lg border border-border/60"
        style={{ height: previewHeight }}
      >
        <LiveSessionViewer
          sessionId={selectedSession.id}
          protocol={selectedSession.protocol}
          websocketUrl={observationUrl}
          token=""
          hostId={selectedSession.hostId}
          isActive
          readOnly
        />
      </div>
      <PaneResizer
        orientation="vertical"
        size={previewHeight}
        min={OBSERVATION_PREVIEW_MIN_HEIGHT}
        max={observationPreviewMaxHeight()}
        onResize={rememberObservationHeight}
        label={t("activeConnections.resizeObservation")}
      />
    </section>
  ) : undefined;

  return (
    <div className="flex min-w-0 flex-col gap-4 xl:flex-row xl:items-start">
      {/* Left: table */}
      <div className="flex min-w-0 flex-1 flex-col gap-4">
        <PageChrome
          title={t("activeConnections.title")}
          description={t("activeConnections.description")}
          actions={
            <Button
              variant="outline"
              size="sm"
              className="gap-2 text-sm"
              onClick={() => sessionsQuery.refetch()}
              disabled={sessionsQuery.isFetching}
            >
              <RefreshCw className={`h-3 w-3 ${sessionsQuery.isFetching ? "animate-spin" : ""}`} />{" "}
              {t("refresh")}
            </Button>
          }
        />
        <SectionLinks label={t("activeConnections.title")} activeValue={view} items={(["active", "history"] as const).map(value => ({ value, label: t(`activeConnections.views.${value}`), to: { search: `?${new URLSearchParams({ ...Object.fromEntries(params), view: value })}` } }))} onNavigate={() => { setPage(0); setSelectedId(null); }} />
        <DataTable
          tableId="connections.active"
          className="min-h-0 flex-1"
          columns={columns}
          data={hydrated}
          getRowKey={(r) => r.id}
          loading={sessionsQuery.isPending}
          error={sessionsQuery.error}
          onRetry={() => sessionsQuery.refetch()}
          onRowClick={(row, element) => { detailsTrigger.current = element; setObserving(false); setSelectedId(row.id === selectedId ? null : row.id); }}
          selectedRow={selectedSession}
          sortState={sanitizedSortState}
          onSortChange={handleSortChange}
          columnFilters={sanitizedColumnFilters}
          onColumnFilterChange={handleColumnFilterChange}
          pagination={{
            page,
            pageSize,
            total,
            onPageChange: setPage,
            onPageSizeChange: setPageSize,
          }}
          emptyState={
              <EmptyState
                icon={Activity}
                title={t("activeConnections.noActiveSessions")}
                description={t("activeConnections.noActiveDescription")}
              />
          }
          toolbar={
            <SearchInput
              value={search}
              onValueChange={(value) => { setSearch(value); setSelectedId(null); }}
              placeholder={t("activeConnections.searchPlaceholder")}
            />
          }
        />
      </div>

      {/* Right: resizable session inspector */}
      {selectedSession && (
        <div
          className="flex w-full min-w-0 flex-col xl:w-[var(--session-inspector-width)] xl:flex-none xl:flex-row xl:items-stretch"
          style={{ "--session-inspector-width": `${inspectorWidth}px` } as CSSProperties}
        >
          <div className="hidden self-stretch xl:flex">
            <PaneResizer
              orientation="horizontal"
              reverse
              size={inspectorWidth}
              min={SESSION_INSPECTOR_MIN_WIDTH}
              max={sessionInspectorMaxWidth()}
              onResize={rememberInspectorWidth}
              label={t("activeConnections.resizeDetails")}
            />
          </div>
          <div className="min-w-0 flex-1">
            <SessionDetailPanel
              session={selectedSession}
              onDisconnect={(id) => disconnectMutation.mutate(id)}
              onTerminate={(id) => terminateMutation.mutate(id)}
              onClose={() => { setObserving(false); setSelectedId(null); detailsTrigger.current?.focus(); }}
              onObserve={observationUrl ? () => setObserving(true) : undefined}
              observing={observingNow}
              onCloseObservation={() => setObserving(false)}
              onObserveInWindow={canOpenObservationWindow ? openObservationInWindow : undefined}
              observeReason={selectedSession.observe?.reason ?? undefined}
              preview={observationPreview}
            />
          </div>
        </div>
      )}
    </div>
  );
};

export default ActiveConnections;
