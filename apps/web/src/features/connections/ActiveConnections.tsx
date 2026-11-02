import { useMemo, useState } from "react";
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
import { RefreshCw, Activity } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
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
import SessionDetailPanel from "@/features/connections/components/connections/SessionDetailPanel";
import {
  ACTIVE_CONNECTIONS_TABLE_CONFIG,
} from "@/features/connections/activeConnectionsTableConfig";

// ── Local utilities ───────────────────────────────────────

function formatDuration(ms: number): string {
  if (ms < 0) ms = 0;
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  const h = Math.floor(m / 60);
  if (h > 0) return `${h}h ${m % 60}m`;
  if (m > 0) return `${m}m ${s % 60}s`;
  return `${s}s`;
}

function formatRelative(iso: string): string {
  try {
    const diff = Date.now() - new Date(iso).getTime();
    if (diff < 0) return "just now";
    const s = Math.floor(diff / 1000);
    const m = Math.floor(s / 60);
    const h = Math.floor(m / 60);
    const d = Math.floor(h / 24);
    if (d >= 1) return d === 1 ? "yesterday" : `${d}d ago`;
    if (h > 0) return `${h}h ago`;
    if (m > 0) return `${m}m ago`;
    return `${s}s ago`;
  } catch {
    return iso;
  }
}

// ── Component ─────────────────────────────────────────────

const ActiveConnections = () => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [search, setSearch] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // Search deliberately stays local to the fetched page. Only table filters
  // are nested for the sessions API; shared state knows no connection terms.
  const table = useServerTable<ActiveSession, ServerTableListParams & { limit: number; offset: number } &
    Partial<Record<"target_id" | "user_id" | "protocol" | "status", string[]>>>({
    ...ACTIVE_CONNECTIONS_TABLE_CONFIG,
    queryKey: ["connections", "sessions"],
    listFn: ({ limit, offset, sort, order, target_id, user_id, protocol, status }) =>
      listActiveSessions({
        limit, offset, sort, order,
        filters: Object.fromEntries(
          Object.entries({ target_id, user_id, protocol, status }).filter(
            (entry): entry is [string, string[]] => Array.isArray(entry[1]),
          ),
        ),
      }),
    refetchInterval: 5000,
    keepPreviousData: true,
  });
  const { page, setPage, pageSize, setPageSize, query: sessionsQuery,
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

  // Server owns ordering + status/protocol filtering; only client-side search
  // narrows the fetched page (the sessions route has no server `search` param).
  const q = search.trim().toLowerCase();
  const filtered = hydrated.filter(
    (s) =>
      (s.userDisplayName ?? s.userEmail ?? s.userId).toLowerCase().includes(q) ||
      (s.hostName ?? s.hostId).toLowerCase().includes(q) ||
      s.protocol.toLowerCase().includes(q),
  );

  // Selected session state
  const selectedSession = useMemo(
    () => filtered.find((s) => s.id === selectedId) ?? null,
    [filtered, selectedId],
  );

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
        <span className="font-medium text-foreground">
          {r.hostName ?? r.hostId.slice(0, 8) + "…"}
        </span>
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
            r.status === "active"
              ? "positive"
              : r.status === "failed"
                ? "negative"
                : r.status === "terminated" || r.status === "disconnected"
                  ? "neutral"
                  : "info"
          }
          label={t(`statuses.${r.status}`, r.status)}
        />
      ),
    },
    {
      key: "started_at",
      label: t("activeConnections.duration"),
      sortable: true,
      render: (r: ActiveSession) => (
        <span className="text-muted-foreground text-xs">
          {formatDuration(now - new Date(r.connectedAt).getTime())}
        </span>
      ),
    },
    {
      key: "lastActivity",
      label: t("activeConnections.lastActive"),
      sortable: true,
      render: (r: ActiveSession) => (
        <span className="text-muted-foreground text-xs">{formatRelative(r.lastActivity)}</span>
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

  return (
    <div className="grid min-w-0 grid-cols-1 items-start gap-4 xl:grid-cols-[minmax(0,1fr)_380px]">
      {/* Left: table */}
      <div className="flex min-w-0 flex-col gap-4">
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
        <DataTable
          tableId="connections.active"
          className="min-h-0 flex-1"
          columns={columns}
          data={filtered}
          getRowKey={(r) => r.id}
          loading={sessionsQuery.isPending}
          error={sessionsQuery.error}
          onRetry={() => sessionsQuery.refetch()}
          onRowClick={(row) => setSelectedId(row.id === selectedId ? null : row.id)}
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
              onValueChange={setSearch}
              placeholder={t("activeConnections.searchPlaceholder")}
            />
          }
        />
      </div>

      {/* Right: detail panel */}
      <div className="min-w-0">
        <SessionDetailPanel
          session={selectedSession}
          onDisconnect={(id) => disconnectMutation.mutate(id)}
          onTerminate={(id) => terminateMutation.mutate(id)}
        />
      </div>
    </div>
  );
};

export default ActiveConnections;
